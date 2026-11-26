//! **CELT** – vrstva MDCT kodeku Opus (RFC 6716 §4.3, opravy RFC 8251).
//!
//! Poradie symbolov podľa RFC 6716 tabuľky 56: ticho, postfilter, prechodový rámec, intra, hrubá energia (§4.3.2.1),
//! TF (§4.3.1), rozprestretie, posilnenie pásiem, sklon rozdelenia, rozdelenie bitov s preskočením pásiem,
//! intenzitou a duálnym stereom (§4.3.3), jemná energia (§4.3.2.2), tvar pásiem (§4.3.4), anti-kolaps (§4.3.5),
//! dokončenie energie. Syntéza: denormovanie (§4.3.6), spätná MDCT (§4.3.7), postfilter (§4.3.7.1), de-emfáza
//! (§4.3.7.2). Pravidlá, ktoré text neuvádza (napr. Laplaceovo rozdelenie hrubej energie, presné kroky
//! rozdelenia bitov), sú **odvodené meraním proti libopus** (rozhodnutie A, #15); číselné tabuľky z prílohy A.
//!
//! Výstup pri 48 kHz v rozsahu ±1 (f32).

#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use super::celt_pasma::{self as pasma, BITRES, NB_PASIEM};
use super::celt_tabulky as t;
use super::mdct::{self, PREKRYTIE, SpatnaMdct};
use super::rozsah::RozsahovyDekoder;

const MAX_JEMNYCH: i32 = 8;
const JEMNY_POSUN: i32 = 21;
/// História výstupu (po postfiltri) pre postfilter: perióda ≤ 1022 + 2 odbočky.
const HISTORIA: usize = 1024 + 8;
/// De-emfáza (RFC 6716 §4.3.7.2): alpha_p.
const DEEMFAZA: f32 = 0.850_006_1;
/// Škála vnútorného signálu (16-bitový rozsah) → ±1.
const SKALA: f32 = 1.0 / 32768.0;

/// Počty rámcov s jednotlivými nástrojmi CELT (pokrytie skúšok).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatistikaCelt {
    pub ramcov: u64,
    pub ticho: u64,
    pub prechodovych: u64,
    pub postfilter: u64,
    pub intra: u64,
    pub posilnenie: u64,
    pub preskocene: u64,
    pub intenzita: u64,
    pub dual: u64,
    pub anti_kolaps: u64,
    pub tf_zmena: u64,
}

/// Stav dekodéra CELT medzi rámcami.
#[derive(Clone, Debug)]
pub struct DekoderCelt {
    statistika: StatistikaCelt,
    /// Počet výstupných kanálov (1 alebo 2) – rámec môže mať iný počet kódovaných kanálov.
    kanalov: usize,
    energia: [f32; 2 * NB_PASIEM],
    log1: [f32; 2 * NB_PASIEM],
    log2: [f32; 2 * NB_PASIEM],
    semienko: u32,
    historia: [Vec<f32>; 2],
    chvost: [Vec<f32>; 2],
    pf: (i32, f32, usize),
    pf_stary: (i32, f32, usize),
    deemfaza: [f32; 2],
    mdct: [SpatnaMdct; 4],
    okno: Vec<f32>,
}

impl DekoderCelt {
    pub fn novy(kanalov: usize) -> DekoderCelt {
        let mut d = DekoderCelt {
            statistika: StatistikaCelt::default(),
            kanalov: kanalov.clamp(1, 2),
            energia: [0.0; 2 * NB_PASIEM],
            log1: [-28.0; 2 * NB_PASIEM],
            log2: [-28.0; 2 * NB_PASIEM],
            semienko: 0,
            historia: [vec![0.0; HISTORIA], vec![0.0; HISTORIA]],
            chvost: [vec![0.0; PREKRYTIE], vec![0.0; PREKRYTIE]],
            pf: (0, 0.0, 0),
            pf_stary: (0, 0.0, 0),
            deemfaza: [0.0; 2],
            mdct: [
                SpatnaMdct::nova(120),
                SpatnaMdct::nova(240),
                SpatnaMdct::nova(480),
                SpatnaMdct::nova(960),
            ],
            okno: mdct::okno_celt(),
        };
        d.reset();
        d
    }

    /// Pokrytie nástrojov od vytvorenia dekodéra.
    pub fn statistika(&self) -> StatistikaCelt {
        self.statistika
    }

    /// Nový začiatok (RFC 6716 §4.5.2 – prechod zo SILK, posun).
    pub fn reset(&mut self) {
        self.energia = [0.0; 2 * NB_PASIEM];
        self.log1 = [-28.0; 2 * NB_PASIEM];
        self.log2 = [-28.0; 2 * NB_PASIEM];
        self.semienko = 0;
        for c in 0..2 {
            self.historia[c].fill(0.0);
            self.chvost[c].fill(0.0);
        }
        self.pf = (0, 0.0, 0);
        self.pf_stary = (0, 0.0, 0);
        self.deemfaza = [0.0; 2];
    }

    /// Dekóduje rámec CELT s `vzoriek` (120, 240, 480, 960) vzorkami pri 48 kHz z `bajtov` bajtov rozsahového
    /// kódu (pri hybride pokračuje po SILK v tom istom dekodéri). `stereo` – kódované kanály rámca, pásma
    /// `zaciatok..koniec` (hybrid 17, inak 0; koniec podľa šírky pásma). Výsledok: `kanalov` kanálov.
    pub fn dekoduj(
        &mut self,
        rd: &mut RozsahovyDekoder,
        bajtov: usize,
        vzoriek: usize,
        stereo: bool,
        zaciatok: usize,
        koniec: usize,
    ) -> Vec<Vec<f32>> {
        let lm: i32 = match vzoriek {
            120 => 0,
            240 => 1,
            480 => 2,
            _ => 3,
        };
        let n = 120usize << lm;
        let c = 1 + stereo as usize;
        let koniec = koniec.clamp(zaciatok + 1, NB_PASIEM);
        let celkom = (bajtov * 8) as i32;
        let e = &t::HRANICE_PASIEM;

        if c == 1 {
            for i in 0..NB_PASIEM {
                self.energia[i] = self.energia[i].max(self.energia[NB_PASIEM + i]);
            }
        }
        let mut tell = rd.tell();
        let ticho = if tell >= celkom {
            true
        } else if tell == 1 {
            rd.bit_logp(15)
        } else {
            false
        };
        if ticho {
            rd.preskoc_na_koniec(bajtov);
            tell = celkom;
        }
        // postfilter (len bez hybridu)
        let mut pf_novy = (0i32, 0.0f32, 0usize);
        if zaciatok == 0 && tell + 16 <= celkom {
            if rd.bit_logp(1) {
                let oktava = rd.uint(6);
                let perioda = (16 << oktava) + rd.bity(4 + oktava) as i32 - 1;
                let qg = rd.bity(3);
                let tapset = if rd.tell() + 2 <= celkom { rd.icdf(&t::TAPSET_ICDF, 2) } else { 0 };
                pf_novy = (perioda, 0.09375 * (qg + 1) as f32, tapset);
            }
            tell = rd.tell();
        }
        let prechodovy = if lm > 0 && tell + 3 <= celkom {
            let p = rd.bit_logp(3);
            tell = rd.tell();
            p
        } else {
            false
        };
        let intra = tell + 3 <= celkom && rd.bit_logp(3);

        self.hruba_energia(rd, bajtov, zaciatok, koniec, intra, c, lm);
        let tf = dekoduj_tf(rd, bajtov, zaciatok, koniec, prechodovy, lm);
        let mut rozprestretie = 2;
        if rd.tell() + 4 <= celkom {
            rozprestretie = rd.icdf(&t::SPREAD_ICDF, 5);
        }
        // maximá pásiem (RFC 6716 §4.3.3, init_caps)
        let mut strop = [0i32; NB_PASIEM];
        for i in 0..NB_PASIEM {
            let sirka = ((e[i + 1] - e[i]) as i32) << lm;
            let ix = NB_PASIEM * (2 * lm as usize + c - 1) + i;
            strop[i] = ((t::CACHE_MAXIMA[ix] as i32 + 64) * c as i32 * sirka) >> 2;
        }
        // posilnenie pásiem
        let mut posilnenie = [0i32; NB_PASIEM];
        let mut logp = 6;
        let mut celkom8 = celkom << BITRES;
        let mut tell8 = rd.tell_frac();
        for i in zaciatok..koniec {
            let sirka = c as i32 * (((e[i + 1] - e[i]) as i32) << lm);
            let kvantum = (sirka << BITRES).min((6 << BITRES).max(sirka));
            let mut slucka_logp = logp;
            let mut boost = 0;
            while tell8 + (slucka_logp << BITRES) < celkom8 && boost < strop[i] {
                let bit = rd.bit_logp(slucka_logp as u32);
                tell8 = rd.tell_frac();
                if !bit {
                    break;
                }
                boost += kvantum;
                celkom8 -= kvantum;
                slucka_logp = 1;
            }
            posilnenie[i] = boost;
            if boost > 0 {
                logp = (logp - 1).max(2);
            }
        }
        let mut sklon = 5;
        if tell8 + (6 << BITRES) <= celkom8 {
            sklon = rd.icdf(&t::TRIM_ICDF, 7) as i32;
        }
        let mut bity = ((bajtov as i32 * 8) << BITRES) - rd.tell_frac() - 1;
        let anti_rez = if prechodovy && lm >= 2 && bity >= (lm + 2) << BITRES { 1 << BITRES } else { 0 };
        bity -= anti_rez;
        let a = rozdel_bity(rd, zaciatok, koniec, &posilnenie, &strop, sklon, bity, c, lm);
        self.jemna_energia(rd, zaciatok, koniec, &a.jemne, c);

        let mut x = vec![0.0f32; c * n];
        let mut masky = [0u8; 2 * NB_PASIEM];
        let parametre = pasma::ParametrePasiem {
            zaciatok,
            koniec,
            lm,
            kanalov: c,
            kratke: prechodovy,
            rozprestretie,
            dual: a.dual,
            intenzita: a.intenzita,
            tf: &tf,
            pulzy: &a.pulzy,
            celkom_bitov: ((bajtov as i32 * 8) << BITRES) - anti_rez,
            zostatok: a.zostatok,
            kodovanych: a.kodovanych,
        };
        pasma::dekoduj_pasma(rd, &parametre, &mut self.semienko, &mut x, n, &mut masky);
        let anti = anti_rez > 0 && rd.bity(1) != 0;
        let st = &mut self.statistika;
        st.ramcov += 1;
        st.ticho += ticho as u64;
        st.prechodovych += prechodovy as u64;
        st.postfilter += (pf_novy.1 > 0.0) as u64;
        st.intra += intra as u64;
        st.posilnenie += posilnenie.iter().any(|&b| b > 0) as u64;
        st.preskocene += (a.kodovanych < koniec) as u64;
        st.intenzita += (c == 2 && a.intenzita < a.kodovanych) as u64;
        st.dual += a.dual as u64;
        st.anti_kolaps += anti as u64;
        st.tf_zmena += tf[zaciatok..koniec].iter().any(|&v| v != 0) as u64;
        let zostava = bajtov as i32 * 8 - rd.tell();
        self.dokonci_energiu(rd, zaciatok, koniec, &a.jemne, &a.priorita, zostava, c);
        if anti {
            pasma::anti_kolaps(
                &mut x,
                &masky,
                lm,
                c,
                n,
                zaciatok,
                koniec,
                &self.energia,
                &self.log1,
                &self.log2,
                &a.pulzy,
                &mut self.semienko,
            );
        }
        if ticho {
            for v in self.energia[..c * NB_PASIEM].iter_mut() {
                *v = -28.0;
            }
        }
        // denormovanie (RFC 8251: strop energie 32 v log2)
        let mut spektrum = vec![vec![0.0f32; n]; c];
        if !ticho {
            for ch in 0..c {
                for i in zaciatok..koniec {
                    let lg = (self.energia[ch * NB_PASIEM + i] + t::STREDNA_ENERGIA[i]).min(32.0);
                    let g = orchidea_jadro::matematika::exp(lg as f64 * core::f64::consts::LN_2) as f32;
                    let (od, po) = ((e[i] as usize) << lm, (e[i + 1] as usize) << lm);
                    for j in od..po {
                        spektrum[ch][j] = x[ch * n + j] * g;
                    }
                }
            }
        }
        if self.kanalov == 2 && c == 1 {
            spektrum.push(spektrum[0].clone());
        } else if self.kanalov == 1 && c == 2 {
            let p = spektrum.pop().unwrap_or_default();
            for (l, r) in spektrum[0].iter_mut().zip(p) {
                *l = 0.5 * *l + 0.5 * r;
            }
        }
        // spätná MDCT, prekrytie, postfilter, de-emfáza
        let mut vystup = Vec::with_capacity(self.kanalov);
        for ch in 0..self.kanalov {
            let mut s = vec![0.0f32; n + PREKRYTIE];
            s[..PREKRYTIE].copy_from_slice(&self.chvost[ch]);
            if prechodovy {
                let blokov = 1usize << lm;
                for b in 0..blokov {
                    self.mdct[0].pripocitaj(&spektrum[ch][b..], blokov, &self.okno, 1.0, &mut s[120 * b..]);
                }
            } else {
                self.mdct[lm as usize].pripocitaj(&spektrum[ch], 1, &self.okno, 1.0, &mut s);
            }
            self.chvost[ch].copy_from_slice(&s[n..n + PREKRYTIE]);
            let mut buf = std::mem::take(&mut self.historia[ch]);
            buf.extend_from_slice(&s[..n]);
            let pf1 = (self.pf.0.max(15), self.pf.1, self.pf.2);
            let pf0 = (self.pf_stary.0.max(15), self.pf_stary.1, self.pf_stary.2);
            hrebenovy_filter(&mut buf, HISTORIA, 120, pf0, pf1, &self.okno);
            if lm != 0 {
                hrebenovy_filter(&mut buf, HISTORIA + 120, n - 120, pf1, pf_novy, &self.okno);
            }
            let mut v = Vec::with_capacity(n);
            let mut m = self.deemfaza[ch];
            for &x in &buf[HISTORIA..] {
                let tmp = x + m;
                m = DEEMFAZA * tmp;
                v.push(tmp * SKALA);
            }
            self.deemfaza[ch] = m;
            buf.drain(..n);
            self.historia[ch] = buf;
            vystup.push(v);
        }
        self.pf_stary = (self.pf.0.max(15), self.pf.1, self.pf.2);
        self.pf = pf_novy;
        if lm != 0 {
            self.pf_stary = self.pf;
        }
        // stav energií
        if c == 1 {
            let (a0, a1) = self.energia.split_at_mut(NB_PASIEM);
            a1.copy_from_slice(a0);
        }
        if !prechodovy {
            self.log2 = self.log1;
            self.log1 = self.energia;
        } else {
            for i in 0..2 * NB_PASIEM {
                self.log1[i] = self.log1[i].min(self.energia[i]);
            }
        }
        for ch in 0..2 {
            for i in (0..zaciatok).chain(koniec..NB_PASIEM) {
                self.energia[ch * NB_PASIEM + i] = 0.0;
                self.log1[ch * NB_PASIEM + i] = -28.0;
                self.log2[ch * NB_PASIEM + i] = -28.0;
            }
        }
        self.semienko = rd.rng();
        vystup
    }

    /// Hrubá energia (RFC 6716 §4.3.2.1): predikcia v čase (alpha) a vo frekvencii (beta), Laplaceovo rozdelenie
    /// rezídua s parametrami z `E_PROB_MODEL`.
    fn hruba_energia(
        &mut self,
        rd: &mut RozsahovyDekoder,
        bajtov: usize,
        zaciatok: usize,
        koniec: usize,
        intra: bool,
        c: usize,
        lm: i32,
    ) {
        let model = &t::E_PROB_MODEL[(lm as usize * 2 + intra as usize) * 42..][..42];
        let (alfa, beta) = if intra {
            (0.0, 4915.0 / 32768.0)
        } else {
            (t::PREDIKCIA_CAS[lm as usize] as f32 / 32768.0, t::PREDIKCIA_PASMO[lm as usize] as f32 / 32768.0)
        };
        let rozpocet = bajtov as i32 * 8;
        let mut pred = [0.0f32; 2];
        for i in zaciatok..koniec {
            for ch in 0..c {
                let tell = rd.tell();
                let qi = if rozpocet - tell >= 15 {
                    let pi = 2 * i.min(20);
                    laplace(rd, (model[pi] as u32) << 7, (model[pi + 1] as u32) << 6)
                } else if rozpocet - tell >= 2 {
                    let q = rd.icdf(&t::MALA_ENERGIA_ICDF, 2) as i32;
                    (q >> 1) ^ -(q & 1)
                } else if rozpocet - tell >= 1 {
                    -(rd.bit_logp(1) as i32)
                } else {
                    -1
                };
                let q = qi as f32;
                let ix = ch * NB_PASIEM + i;
                let stara = self.energia[ix].max(-9.0);
                self.energia[ix] = alfa * stara + pred[ch] + q;
                pred[ch] = pred[ch] + q - beta * q;
            }
        }
    }

    /// Jemná energia (RFC 6716 §4.3.2.2): oprava (f + ½)/2^B − ½.
    fn jemna_energia(
        &mut self,
        rd: &mut RozsahovyDekoder,
        zaciatok: usize,
        koniec: usize,
        jemne: &[i32; NB_PASIEM],
        c: usize,
    ) {
        for i in zaciatok..koniec {
            if jemne[i] <= 0 {
                continue;
            }
            for ch in 0..c {
                let q = rd.bity(jemne[i] as u32) as f32;
                self.energia[ch * NB_PASIEM + i] += (q + 0.5) / (1u32 << jemne[i]) as f32 - 0.5;
            }
        }
    }

    /// Dokončenie energie zvyšnými bitmi: najprv pásma s prioritou 0, potom 1 (RFC 6716 §4.3.2.2).
    fn dokonci_energiu(
        &mut self,
        rd: &mut RozsahovyDekoder,
        zaciatok: usize,
        koniec: usize,
        jemne: &[i32; NB_PASIEM],
        priorita: &[bool; NB_PASIEM],
        mut zostava: i32,
        c: usize,
    ) {
        for prio in [false, true] {
            for i in zaciatok..koniec {
                if zostava < c as i32 {
                    break;
                }
                if jemne[i] >= MAX_JEMNYCH || priorita[i] != prio {
                    continue;
                }
                for ch in 0..c {
                    let q = rd.bity(1) as f32;
                    self.energia[ch * NB_PASIEM + i] += (q - 0.5) / (1u32 << (jemne[i] + 1)) as f32;
                    zostava -= 1;
                }
            }
        }
    }
}

/// Laplaceovo rozdelenie hrubej energie (RFC 6716 §4.3.2.1; tvar výpočtu odvodený meraním): `fs` = pravdepodobnosť
/// nuly v 1/32768, `rozpad` – pomer susedných hodnôt v 1/16384; najmenšia pravdepodobnosť hodnoty 1/32768.
fn laplace(rd: &mut RozsahovyDekoder, fs0: u32, rozpad: u32) -> i32 {
    const MINP: u32 = 1;
    const NMIN: u32 = 16;
    let fm = rd.dekoduj_bin(15);
    let mut hodnota = 0i32;
    let mut fl = 0u32;
    let mut fs = fs0;
    if fm >= fs {
        hodnota += 1;
        fl = fs;
        let ft = 32768 - MINP * (2 * NMIN) - fs0;
        fs = ((ft * (16384 - rozpad)) >> 15) + MINP;
        while fs > MINP && fm >= fl + 2 * fs {
            fs *= 2;
            fl += fs;
            fs = (((fs - 2 * MINP) * rozpad) >> 15) + MINP;
            hodnota += 1;
        }
        if fs <= MINP {
            let di = (fm - fl) >> 1;
            hodnota += di as i32;
            fl += 2 * di * MINP;
        }
        if fm < fl + fs {
            hodnota = -hodnota;
        } else {
            fl += fs;
        }
    }
    rd.aktualizuj(fl, (fl + fs).min(32768), 32768);
    hodnota
}

/// Zmena TF po pásmach (RFC 6716 §4.3.1) → hodnoty z `TF_VYBER`.
fn dekoduj_tf(
    rd: &mut RozsahovyDekoder,
    bajtov: usize,
    zaciatok: usize,
    koniec: usize,
    prechodovy: bool,
    lm: i32,
) -> [i32; NB_PASIEM] {
    let mut rozpocet = bajtov as i32 * 8;
    let mut tell = rd.tell();
    let mut logp = if prechodovy { 2 } else { 4 };
    let rez = lm > 0 && tell + logp < rozpocet;
    rozpocet -= rez as i32;
    let mut zmena = 0;
    let mut aktualna = 0;
    let mut tf = [0i32; NB_PASIEM];
    for i in zaciatok..koniec {
        if tell + logp <= rozpocet {
            aktualna ^= rd.bit_logp(logp as u32) as i32;
            tell = rd.tell();
            zmena |= aktualna;
        }
        tf[i] = aktualna;
        logp = if prechodovy { 4 } else { 5 };
    }
    let p = 4 * prechodovy as usize;
    let riadok = &t::TF_VYBER[lm as usize * 8..][..8];
    let mut vyber = 0;
    if rez && riadok[p + zmena as usize] != riadok[p + 2 + zmena as usize] {
        vyber = rd.bit_logp(1) as usize;
    }
    for i in zaciatok..koniec {
        tf[i] = riadok[p + 2 * vyber + tf[i] as usize] as i32;
    }
    tf
}

struct Rozdelenie {
    pulzy: [i32; NB_PASIEM],
    jemne: [i32; NB_PASIEM],
    priorita: [bool; NB_PASIEM],
    intenzita: usize,
    dual: bool,
    zostatok: i32,
    kodovanych: usize,
}

/// Rozdelenie bitov (RFC 6716 §4.3.3): rezervy, minimá, sklon, hľadanie v tabuľke kvality, interpolácia po 1/64,
/// preskakovanie pásiem, intenzita a duálne stereo, rozdelenie zvyšku, delenie medzi jemnú energiu a tvar.
fn rozdel_bity(
    rd: &mut RozsahovyDekoder,
    zaciatok: usize,
    koniec: usize,
    posilnenie: &[i32; NB_PASIEM],
    strop: &[i32; NB_PASIEM],
    sklon: i32,
    celkom: i32,
    c: usize,
    lm: i32,
) -> Rozdelenie {
    let e = &t::HRANICE_PASIEM;
    let w = |j: usize| (e[j + 1] - e[j]) as i32;
    let ci = c as i32;
    let mut celkom = celkom.max(0);
    let preskok_rez = if celkom >= 1 << BITRES { 1 << BITRES } else { 0 };
    celkom -= preskok_rez;
    let (mut intenzita_rez, mut dual_rez) = (0, 0);
    if c == 2 {
        intenzita_rez = t::LOG2_ZLOMOK[koniec - zaciatok] as i32;
        if intenzita_rez > celkom {
            intenzita_rez = 0;
        } else {
            celkom -= intenzita_rez;
            dual_rez = if celkom >= 1 << BITRES { 1 << BITRES } else { 0 };
            celkom -= dual_rez;
        }
    }
    let mut prah = [0i32; NB_PASIEM];
    let mut sklon_posun = [0i32; NB_PASIEM];
    for j in zaciatok..koniec {
        prah[j] = (ci << BITRES).max((3 * w(j)) << lm << BITRES >> 4);
        sklon_posun[j] =
            (ci * w(j) * (sklon - 5 - lm) * (koniec as i32 - j as i32 - 1) * (1 << (lm + BITRES))) >> 6;
        if w(j) << lm == 1 {
            sklon_posun[j] -= ci << BITRES;
        }
    }
    let staticke = |q: usize, j: usize| (ci * w(j) * t::ROZDELENIE[q * NB_PASIEM + j] as i32) << lm >> 2;
    let (mut lo, mut hi) = (1usize, 10usize);
    while lo <= hi {
        let stred = (lo + hi) >> 1;
        let mut hotovo = false;
        let mut psum = 0;
        for j in (zaciatok..koniec).rev() {
            let mut b = staticke(stred, j);
            if b > 0 {
                b = (b + sklon_posun[j]).max(0);
            }
            b += posilnenie[j];
            if b >= prah[j] || hotovo {
                hotovo = true;
                psum += b.min(strop[j]);
            } else if b >= ci << BITRES {
                psum += ci << BITRES;
            }
        }
        if psum > celkom {
            hi = stred - 1;
        } else {
            lo = stred + 1;
        }
    }
    let hi = lo;
    let lo = lo - 1;
    let mut bity1 = [0i32; NB_PASIEM];
    let mut bity2 = [0i32; NB_PASIEM];
    let mut preskok_od = zaciatok;
    for j in zaciatok..koniec {
        let mut b1 = staticke(lo, j);
        let mut b2 = if hi >= 11 { strop[j] } else { staticke(hi, j) };
        if b1 > 0 {
            b1 = (b1 + sklon_posun[j]).max(0);
        }
        if b2 > 0 {
            b2 = (b2 + sklon_posun[j]).max(0);
        }
        if lo > 0 {
            b1 += posilnenie[j];
        }
        b2 += posilnenie[j];
        if posilnenie[j] > 0 {
            preskok_od = j;
        }
        bity1[j] = b1;
        bity2[j] = (b2 - b1).max(0);
    }
    // interpolácia po krokoch 1/64
    let podlaha = ci << BITRES;
    let (mut lo, mut hi) = (0i32, 64i32);
    for _ in 0..6 {
        let stred = (lo + hi) >> 1;
        let mut psum = 0;
        let mut hotovo = false;
        for j in (zaciatok..koniec).rev() {
            let tmp = bity1[j] + ((stred * bity2[j]) >> 6);
            if tmp >= prah[j] || hotovo {
                hotovo = true;
                psum += tmp.min(strop[j]);
            } else if tmp >= podlaha {
                psum += podlaha;
            }
        }
        if psum > celkom {
            hi = stred;
        } else {
            lo = stred;
        }
    }
    let mut bity = [0i32; NB_PASIEM];
    let mut psum = 0;
    let mut hotovo = false;
    for j in (zaciatok..koniec).rev() {
        let mut tmp = bity1[j] + ((lo * bity2[j]) >> 6);
        if tmp < prah[j] && !hotovo {
            tmp = if tmp >= podlaha { podlaha } else { 0 };
        } else {
            hotovo = true;
        }
        tmp = tmp.min(strop[j]);
        bity[j] = tmp;
        psum += tmp;
    }
    // preskakovanie pásiem od konca
    let mut kodovanych = koniec;
    loop {
        let j = kodovanych - 1;
        if j <= preskok_od {
            celkom += preskok_rez;
            break;
        }
        let mut zvysok = celkom - psum;
        let na_koef = zvysok / (e[kodovanych] - e[zaciatok]) as i32;
        zvysok -= (e[kodovanych] - e[zaciatok]) as i32 * na_koef;
        let rem = (zvysok - (e[j] - e[zaciatok]) as i32).max(0);
        let sirka = (e[kodovanych] - e[j]) as i32;
        let mut bity_pasma = bity[j] + na_koef * sirka + rem;
        if bity_pasma >= prah[j].max(podlaha + (1 << BITRES)) {
            if rd.bit_logp(1) {
                break;
            }
            psum += 1 << BITRES;
            bity_pasma -= 1 << BITRES;
        }
        psum -= bity[j] + intenzita_rez;
        if intenzita_rez > 0 {
            intenzita_rez = t::LOG2_ZLOMOK[j - zaciatok] as i32;
        }
        psum += intenzita_rez;
        if bity_pasma >= podlaha {
            psum += podlaha;
            bity[j] = podlaha;
        } else {
            bity[j] = 0;
        }
        kodovanych -= 1;
    }
    let intenzita =
        if intenzita_rez > 0 { zaciatok + rd.uint((kodovanych + 1 - zaciatok) as u32) as usize } else { 0 };
    if intenzita <= zaciatok {
        celkom += dual_rez;
        dual_rez = 0;
    }
    let dual = dual_rez > 0 && rd.bit_logp(1);
    // zvyšok rovnomerne
    let mut zvysok = celkom - psum;
    let na_koef = zvysok / (e[kodovanych] - e[zaciatok]) as i32;
    zvysok -= (e[kodovanych] - e[zaciatok]) as i32 * na_koef;
    for j in zaciatok..kodovanych {
        bity[j] += na_koef * w(j);
    }
    for j in zaciatok..kodovanych {
        let tmp = zvysok.min(w(j));
        bity[j] += tmp;
        zvysok -= tmp;
    }
    // jemná energia verzus tvar
    let mut jemne = [0i32; NB_PASIEM];
    let mut priorita = [false; NB_PASIEM];
    let stereo = (c > 1) as i32;
    let log_m = lm << BITRES;
    let mut zostatok = 0;
    for j in zaciatok..kodovanych {
        let n = w(j) << lm;
        let bit = bity[j] + zostatok;
        let mut prebytok;
        if n > 1 {
            prebytok = (bit - strop[j]).max(0);
            bity[j] = bit - prebytok;
            let den = ci * n + (c == 2 && n > 2 && !dual && j < intenzita) as i32;
            let nclogn = den * (t::LOG_N[j] as i32 + log_m);
            let mut posun = (nclogn >> 1) - den * JEMNY_POSUN;
            if n == 2 {
                posun += den << BITRES >> 2;
            }
            if bity[j] + posun < (den * 2) << BITRES {
                posun += nclogn >> 2;
            } else if bity[j] + posun < (den * 3) << BITRES {
                posun += nclogn >> 3;
            }
            let mut eb = (bity[j] + posun + (den << (BITRES - 1))).max(0);
            eb = (eb / den) >> BITRES;
            if ci * eb > bity[j] >> BITRES {
                eb = bity[j] >> stereo >> BITRES;
            }
            eb = eb.min(MAX_JEMNYCH);
            priorita[j] = eb * (den << BITRES) >= bity[j] + posun;
            bity[j] -= (ci * eb) << BITRES;
            jemne[j] = eb;
        } else {
            prebytok = (bit - (ci << BITRES)).max(0);
            bity[j] = bit - prebytok;
            jemne[j] = 0;
            priorita[j] = true;
        }
        if prebytok > 0 {
            let navyse = (prebytok >> (stereo + BITRES)).min(MAX_JEMNYCH - jemne[j]);
            jemne[j] += navyse;
            let navyse_bity = (navyse * ci) << BITRES;
            priorita[j] = navyse_bity >= prebytok - zostatok;
            prebytok -= navyse_bity;
        }
        zostatok = prebytok;
    }
    for j in kodovanych..koniec {
        jemne[j] = bity[j] >> stereo >> BITRES;
        bity[j] = 0;
        priorita[j] = jemne[j] < 1;
    }
    Rozdelenie { pulzy: bity, jemne, priorita, intenzita, dual, zostatok, kodovanych }
}

/// Hrebeňový postfilter (RFC 6716 §4.3.7.1) na mieste v `buf[od..od + n]` (minulosť pred `od` je už
/// filtrovaná): prvých 120 vzoriek plynulý prechod zo starých parametrov na nové so štvorcom okna.
fn hrebenovy_filter(
    buf: &mut [f32],
    od: usize,
    n: usize,
    stare: (i32, f32, usize),
    nove: (i32, f32, usize),
    okno: &[f32],
) {
    let (t0, g0, ts0) = stare;
    let (t1, g1, ts1) = nove;
    if g0 == 0.0 && g1 == 0.0 {
        return;
    }
    let z = &t::ZISKY_POSTFILTRA;
    let (g00, g01, g02) = (g0 * z[ts0 * 3], g0 * z[ts0 * 3 + 1], g0 * z[ts0 * 3 + 2]);
    let (g10, g11, g12) = (g1 * z[ts1 * 3], g1 * z[ts1 * 3 + 1], g1 * z[ts1 * 3 + 2]);
    let (t0, t1) = (t0 as usize, t1 as usize);
    let prekrytie = if g0 == g1 && t0 == t1 && ts0 == ts1 { 0 } else { PREKRYTIE.min(n) };
    for i in 0..prekrytie {
        let p = od + i;
        let f = okno[i] * okno[i];
        // nulový zisk: odbočky sa nečítajú (perióda nového filtra môže byť 0)
        let stara = if g0 == 0.0 {
            0.0
        } else {
            g00 * buf[p - t0]
                + g01 * (buf[p - t0 + 1] + buf[p - t0 - 1])
                + g02 * (buf[p - t0 + 2] + buf[p - t0 - 2])
        };
        let nova = if g1 == 0.0 {
            0.0
        } else {
            g10 * buf[p - t1]
                + g11 * (buf[p - t1 + 1] + buf[p - t1 - 1])
                + g12 * (buf[p - t1 + 2] + buf[p - t1 - 2])
        };
        buf[p] += (1.0 - f) * stara + f * nova;
    }
    if g1 == 0.0 {
        return;
    }
    for i in prekrytie..n {
        let p = od + i;
        buf[p] += g10 * buf[p - t1]
            + g11 * (buf[p - t1 + 1] + buf[p - t1 - 1])
            + g12 * (buf[p - t1 + 2] + buf[p - t1 - 2]);
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn nahodne_ramce_nikdy_pad() {
        let mut s = 1u32;
        for velkost in [2usize, 8, 40, 160, 1275] {
            for vzoriek in [120usize, 240, 480, 960] {
                for (stereo, zac, kon) in [(false, 0, 21), (true, 0, 21), (true, 17, 21), (false, 0, 13)] {
                    let mut d = DekoderCelt::novy(2);
                    for _ in 0..3 {
                        let data: Vec<u8> = (0..velkost)
                            .map(|_| {
                                s = pasma::lcg(s);
                                (s >> 24) as u8
                            })
                            .collect();
                        let mut rd = RozsahovyDekoder::novy(&data);
                        let v = d.dekoduj(&mut rd, velkost, vzoriek, stereo, zac, kon);
                        assert_eq!(v.len(), 2);
                        assert!(v.iter().all(|k| k.len() == vzoriek && k.iter().all(|x| x.is_finite())));
                    }
                }
            }
        }
    }
}
