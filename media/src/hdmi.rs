//! **Prevodník zvuku pre HDMI** (R5b, etapa L5) – z dekódovaných blokov alebo z paketov AC-3/E-AC-3 urobí
//! bajty pre ovládač zvuku HDMI a nastavenie výstupu.
//!
//! Cesta sa vyberie raz pri vytvorení (podľa stopy a [`SchopnostiHdmi`]):
//! 1. **priechod IEC 61937** – AC-3 / E-AC-3 ide nedekódovaný do televízora, ak ho vie a je dovolený
//!    (procesor nepracuje – chladný systém),
//! 2. **bitovo presné PCM** – celé vzorky (PCM, FLAC) sa len preložia do poradia CTA-861; hĺbka sa nezaokrúhľuje,
//! 3. **PCM s prevodom** – vzorky `f32` (stratové kodeky) sa zaokrúhlia na hĺbku výstupu; ak sa hĺbka zdroja
//!    nezmestí, zaokrúhli sa; ak rozloženie nejde do HDMI alebo televízor nemá dosť kanálov, **zmieša sa na stereo**
//!    (ITU-R BS.775, LFE sa vynechá, koeficienty sa normujú, aby nevzniklo orezanie).
//!
//! Frekvencia sa **neprevzorkúva** (R305): ak ju televízor nevie, prevodník vráti chybu a voľbu urobí volajúci.
//!
//! Zdroje (vlastná implementácia podľa textov):
//! - **CTA-861** (InfoFrame zvuku): poradie slotov LPCM FL, FR, LFE1, FC, potom podľa kódu rozloženia CA
//!   (RL/RR, RC, RLC/RRC, FLC/FRC) – kódy 0x00–0x1F,
//! - **IEC 60958-3** – bajty stavu kanála spotrebiteľského formátu (PCM/„nie PCM“, frekvencia, dĺžka slova),
//! - **IEC 61937-1/-3** – dávka (burst): preambuly Pa = 0xF872, Pb = 0x4E1F, Pc = typ údajov (AC-3 = 1 s bsmod
//!   v bitoch 8–10, E-AC-3 = 21), Pd = dĺžka (AC-3 v bitoch, E-AC-3 v bajtoch), údaje ako 16-bitové slová,
//!   výplň nulami do periódy opakovania (AC-3 1536 rámcov, E-AC-3 6144 rámcov pri 4× frekvencii).

use crate::cas::na_ns;
use crate::paket::Paket;
use crate::r5::{DruhPriechodu, FormatVzoriekHdmi, NastavenieHdmi, SchopnostiHdmi, ZvukHdmi};
use crate::ramce;
use crate::stopa::{CisloPcm, Kodek, Stopa};
use crate::zvuk::{BlokZvuku, RozlozenieKanalov, VzorkyZvuku};
use orchidea_jadro::{Chyba, Vysledok, Zlomok};

type R = RozlozenieKanalov;

/// Voľby používateľa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolbyZvuku {
    /// Dovoliť priechod AC-3/E-AC-3 do televízora (inak sa dekóduje).
    pub priechod: bool,
    /// Vždy zmiešať na stereo.
    pub stereo: bool,
}

impl Default for VolbyZvuku {
    fn default() -> Self {
        VolbyZvuku { priechod: true, stereo: false }
    }
}

/// Zvolená cesta zvuku.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CestaZvuku {
    /// Komprimovaný tok do televízora (IEC 61937).
    Priechod(DruhPriechodu),
    /// Celé vzorky bez zmeny hodnoty, len poradie kanálov CTA-861.
    BitovoPresne,
    /// Zaokrúhlenie na hĺbku výstupu (vzorky `f32` alebo hlbší zdroj), poradie CTA-861.
    Pcm,
    /// Zmiešanie na stereo (ITU-R BS.775) a zaokrúhlenie.
    ZmiesanieNaStereo,
}

/// Sloty LPCM podľa CTA-861 a kód rozloženia (CA) pre rozloženie zdroja; `None` = nejde bez zmiešania.
/// Vráti (CA, pre každý slot index kanála zdroja alebo `None` = ticho).
pub fn sloty_cta(r: RozlozenieKanalov) -> Option<(u8, Vec<Option<usize>>)> {
    let maska = r.0;
    // index kanála zdroja = poradie bitu v maske
    let index = |bit: u32| (maska & bit != 0).then(|| (maska & (bit - 1)).count_ones() as usize);
    let znama = R::PREDNY_LAVY
        | R::PREDNY_PRAVY
        | R::PREDNY_STRED
        | R::BASY
        | R::ZADNY_LAVY
        | R::ZADNY_PRAVY
        | R::PREDNY_LAVY_STRED
        | R::PREDNY_PRAVY_STRED
        | R::ZADNY_STRED
        | R::BOCNY_LAVY
        | R::BOCNY_PRAVY;
    if maska == 0 || maska & !znama != 0 {
        return None;
    }
    // mono: stredový kanál do oboch predných slotov (HDMI LPCM má aspoň 2 kanály)
    if maska == R::PREDNY_STRED {
        return Some((0, vec![Some(0), Some(0)]));
    }
    let par = |l: u32, p: u32| -> Option<Option<(usize, usize)>> {
        match (index(l), index(p)) {
            (Some(a), Some(b)) => Some(Some((a, b))),
            (None, None) => Some(None),
            _ => None, // len polovica páru
        }
    };
    let bocne = par(R::BOCNY_LAVY, R::BOCNY_PRAVY)?;
    let zadne = par(R::ZADNY_LAVY, R::ZADNY_PRAVY)?;
    let stredne = par(R::PREDNY_LAVY_STRED, R::PREDNY_PRAVY_STRED)?;
    let rc = index(R::ZADNY_STRED);
    let mut s: Vec<Option<usize>> =
        vec![index(R::PREDNY_LAVY), index(R::PREDNY_PRAVY), index(R::BASY), index(R::PREDNY_STRED)];
    // obvodový pár RL/RR: bočné (7.1 – zadné idú do RLC/RRC), inak zadné
    let (obvod, zadny_par) = match (bocne, zadne) {
        (Some(b), Some(z)) => (Some(b), Some(z)),
        (b, z) => (b.or(z), None),
    };
    let skupina = match (obvod, zadny_par, rc, stredne) {
        (None, None, None, None) => 0,
        (None, None, Some(_), None) => 1,
        (Some(_), None, None, None) => 2,
        (Some(_), None, Some(_), None) => 3,
        (Some(_), Some(_), None, None) => 4,
        (None, None, None, Some(_)) => 5,
        (None, None, Some(_), Some(_)) => 6,
        (Some(_), None, None, Some(_)) => 7,
        _ => return None,
    };
    let mut daj = |slot: usize, k: Option<usize>| {
        if s.len() <= slot {
            s.resize(slot + 1, None);
        }
        s[slot] = k;
    };
    if let Some((l, p)) = obvod {
        daj(4, Some(l));
        daj(5, Some(p));
    }
    match skupina {
        1 | 6 => daj(4, rc),
        3 => daj(6, rc),
        _ => {}
    }
    if let Some((l, p)) = zadny_par {
        daj(6, Some(l));
        daj(7, Some(p));
    }
    if let Some((l, p)) = stredne {
        daj(6, Some(l));
        daj(7, Some(p));
    }
    let ca = (skupina << 2) | (s[3].is_some() as u8) << 1 | s[2].is_some() as u8;
    // posledný použitý slot; počet kanálov párny (ovládače HDMI prijímajú párne počty)
    let pouzite = s.iter().rposition(|x| x.is_some()).map_or(2, |i| i + 1).max(2);
    s.truncate(pouzite);
    s.resize(pouzite.div_ceil(2) * 2, None);
    Some((ca, s))
}

/// Koeficienty zmiešania na stereo (ITU-R BS.775: stred a obvod −3 dB, LFE sa vynechá) – pre každý kanál zdroja
/// (ľavý, pravý), normované tak, aby súčet koeficientov výstupu bol najviac 1 (bez orezania).
pub fn zmiesanie_stereo(r: RozlozenieKanalov) -> Vec<[f64; 2]> {
    const C: f64 = std::f64::consts::FRAC_1_SQRT_2;
    let mut k: Vec<[f64; 2]> = (0..32)
        .filter(|b| r.0 & (1 << b) != 0)
        .map(|b| match 1u32 << b {
            R::BASY => [0.0, 0.0],
            R::PREDNY_LAVY => [1.0, 0.0],
            R::PREDNY_PRAVY => [0.0, 1.0],
            R::PREDNY_STRED
            | R::ZADNY_STRED
            | R::HORNY_STRED
            | R::HORNY_PREDNY_STRED
            | R::HORNY_ZADNY_STRED => [C, C],
            R::ZADNY_LAVY
            | R::BOCNY_LAVY
            | R::PREDNY_LAVY_STRED
            | R::HORNY_PREDNY_LAVY
            | R::HORNY_ZADNY_LAVY => [C, 0.0],
            R::ZADNY_PRAVY
            | R::BOCNY_PRAVY
            | R::PREDNY_PRAVY_STRED
            | R::HORNY_PREDNY_PRAVY
            | R::HORNY_ZADNY_PRAVY => [0.0, C],
            _ => [C, C],
        })
        .collect();
    if r.0 == R::PREDNY_STRED {
        k = vec![[1.0, 1.0]];
    }
    for strana in 0..2 {
        let sucet: f64 = k.iter().map(|x| x[strana]).sum();
        if sucet > 1.0 {
            for x in &mut k {
                x[strana] /= sucet;
            }
        }
    }
    k
}

/// Kód frekvencie v bajte 3 stavu kanála IEC 60958-3 (bity 24–27, prvý prenášaný bit je najnižší).
fn kod_frekvencie(f: u32) -> u8 {
    match f {
        44_100 => 0x00,
        48_000 => 0x02,
        32_000 => 0x03,
        22_050 => 0x04,
        24_000 => 0x06,
        88_200 => 0x08,
        768_000 => 0x09,
        96_000 => 0x0A,
        176_400 => 0x0C,
        192_000 => 0x0E,
        _ => 0x01, // neuvedená
    }
}

/// Bajty stavu kanála IEC 60958-3 (spotrebiteľský formát): PCM alebo „nie PCM“ (IEC 61937), kopírovanie
/// povolené, bez predzdôraznenia, kategória všeobecná, frekvencia, dĺžka slova (16, 20 alebo 24 bitov).
pub fn stav_kanala(pcm: bool, frekvencia: u32, bitov: u8) -> [u8; 24] {
    let mut c = [0u8; 24];
    c[0] = 0b0000_0100 | if pcm { 0 } else { 0b10 };
    c[3] = kod_frekvencie(frekvencia);
    if pcm {
        // bajt 4: bit 0 = najviac 24 bitov; bity 1–3 dĺžka slova (IEC 60958-3 tabuľka „word length“)
        c[4] = match bitov {
            0..=16 => 0b0010,  // najviac 20, 16 bitov
            17..=20 => 0b1010, // najviac 20, 20 bitov
            _ => 0b1011,       // najviac 24, 24 bitov
        };
    }
    c
}

/// Plávajúca vzorka na celé číslo s `bitov` bitmi (zaokrúhlenie polovicou nahor, orezanie, NaN → 0).
fn f_na_cele(x: f64, bitov: u32) -> i64 {
    let mierka = (1u64 << (bitov - 1)) as f64;
    let y = x * mierka;
    if y.is_nan() {
        return 0;
    }
    let y = y.clamp(-mierka, mierka) + 0.5;
    let t = y as i64;
    let t = if (t as f64) > y { t - 1 } else { t };
    t.clamp(-(mierka as i64), mierka as i64 - 1)
}

/// Celá vzorka so `z` bitmi na `na` bitov (zúženie zaokrúhlením polovicou nahor a orezaním).
fn cele_na(v: i32, z: u32, na: u32) -> i64 {
    let v = (v as i64).clamp(-(1i64 << (z - 1)), (1i64 << (z - 1)) - 1);
    if z <= na {
        v << (na - z)
    } else {
        let s = z - na;
        ((v + (1i64 << (s - 1))) >> s).clamp(-(1i64 << (na - 1)), (1i64 << (na - 1)) - 1)
    }
}

/// Prebiehajúca dávka priechodu E-AC-3 (rámce do 6 blokov).
#[derive(Debug, Clone, Default)]
struct Davka {
    data: Vec<u8>,
    blokov: u32,
    pts_ns: Option<i64>,
}

/// Prevodník zvuku jednej stopy pre HDMI.
#[derive(Debug, Clone)]
pub struct PrevodnikZvuku {
    nastavenie: NastavenieHdmi,
    cesta: CestaZvuku,
    frekvencia: u32,
    rozlozenie: RozlozenieKanalov,
    /// Sloty CTA-861 → kanál zdroja (pri zmiešaní nepoužité).
    sloty: Vec<Option<usize>>,
    zmiesanie: Vec<[f64; 2]>,
    /// Účinné bity zvuku a bajty kontajnera vzorky (2 alebo 4).
    bitov: u32,
    bajtov: usize,
    zakladna: Zlomok,
    davka: Davka,
}

impl PrevodnikZvuku {
    /// Vyberie cestu a nastavenie pre stopu.
    pub fn novy(stopa: &Stopa, schopnosti: &SchopnostiHdmi, volby: VolbyZvuku) -> Vysledok<PrevodnikZvuku> {
        let z = stopa.zvuk.ok_or_else(|| Chyba::Chybajuce("stopa nemá parametre zvuku".into()))?;
        let ma = |f: FormatVzoriekHdmi| schopnosti.formaty.contains(&f);
        let s32 =
            [FormatVzoriekHdmi::S32Le, FormatVzoriekHdmi::Iec958PodramecLe].into_iter().find(|&f| ma(f));
        if !ma(FormatVzoriekHdmi::S16Le) && s32.is_none() {
            return Err(Chyba::MimoRozsahu("ovládač HDMI neprijíma žiadny známy formát vzoriek".into()));
        }
        let mut p = PrevodnikZvuku {
            nastavenie: NastavenieHdmi {
                format: FormatVzoriekHdmi::S16Le,
                frekvencia: z.frekvencia,
                kanalov: 2,
                pridelenie_cta: 0,
                stav_kanala: [0; 24],
                priechod: None,
            },
            cesta: CestaZvuku::Pcm,
            frekvencia: z.frekvencia,
            rozlozenie: z.rozlozenie,
            sloty: Vec::new(),
            zmiesanie: Vec::new(),
            bitov: 16,
            bajtov: 2,
            zakladna: stopa.zakladna,
            davka: Davka::default(),
        };
        // 1. priechod
        let priechod = match stopa.kodek {
            Kodek::Ac3 if volby.priechod && schopnosti.priechod_ac3 => Some(DruhPriechodu::Ac3),
            Kodek::Eac3 if volby.priechod && schopnosti.priechod_eac3 => Some(DruhPriechodu::Eac3),
            _ => None,
        };
        if let Some(druh) = priechod {
            let linka = match druh {
                DruhPriechodu::Ac3 => z.frekvencia,
                DruhPriechodu::Eac3 => z.frekvencia.saturating_mul(4),
            };
            let (format, bajtov) = if ma(FormatVzoriekHdmi::S16Le) {
                (FormatVzoriekHdmi::S16Le, 2)
            } else {
                (s32.unwrap_or(FormatVzoriekHdmi::S32Le), 4)
            };
            p.nastavenie = NastavenieHdmi {
                format,
                frekvencia: linka,
                kanalov: 2,
                pridelenie_cta: 0,
                stav_kanala: stav_kanala(false, linka, 16),
                priechod: Some(druh),
            };
            p.cesta = CestaZvuku::Priechod(druh);
            p.bajtov = bajtov;
            return Ok(p);
        }
        // 2. a 3. PCM
        if !schopnosti.frekvencie.is_empty() && !schopnosti.frekvencie.contains(&z.frekvencia) {
            return Err(Chyba::MimoRozsahu(format!(
                "televízor neprijme {} Hz; prevzorkovanie len na výslovnú voľbu (R305)",
                z.frekvencia
            )));
        }
        let max_kanalov =
            if schopnosti.max_kanalov == 0 { 2 } else { schopnosti.max_kanalov.max(2) } as usize;
        let sloty =
            if volby.stereo { None } else { sloty_cta(z.rozlozenie).filter(|s| s.1.len() <= max_kanalov) };
        let max_bitov = if schopnosti.max_bitov == 0 { 24 } else { schopnosti.max_bitov } as u32;
        let cele = match stopa.kodek {
            Kodek::Pcm(p) => p.cislo != CisloPcm::Plavajuce,
            Kodek::Flac => true,
            _ => false,
        } && z.bitov.is_some();
        let zdroj = if cele { z.bitov.unwrap_or(24) as u32 } else { 24 };
        let mut bitov = zdroj.min(max_bitov).clamp(16, 24);
        // kontajner: 16 bitov, ak stačí, inak 32 (24 bitov hore; podrámce IEC 60958 nesú najviac 24)
        if bitov <= 16 && ma(FormatVzoriekHdmi::S16Le) {
            p.nastavenie.format = FormatVzoriekHdmi::S16Le;
            p.bajtov = 2;
        } else if let Some(f) = s32 {
            p.nastavenie.format = f;
            p.bajtov = 4;
        } else {
            p.nastavenie.format = FormatVzoriekHdmi::S16Le;
            p.bajtov = 2;
            bitov = 16;
        }
        // 32-bitový celý zdroj do formátu S32Le bez zaokrúhlenia, ak televízor nemá obmedzenie pod 24 bitov
        if cele && zdroj > 24 && p.nastavenie.format == FormatVzoriekHdmi::S32Le && max_bitov >= 24 {
            bitov = zdroj.min(32);
        }
        p.bitov = bitov;
        match sloty {
            Some((ca, s)) => {
                p.nastavenie.kanalov = s.len() as u8;
                p.nastavenie.pridelenie_cta = ca;
                p.sloty = s;
                p.cesta = if cele && zdroj <= bitov { CestaZvuku::BitovoPresne } else { CestaZvuku::Pcm };
            }
            None => {
                p.zmiesanie = zmiesanie_stereo(z.rozlozenie);
                p.cesta = CestaZvuku::ZmiesanieNaStereo;
            }
        }
        p.nastavenie.stav_kanala = stav_kanala(true, z.frekvencia, bitov.min(24) as u8);
        Ok(p)
    }

    pub fn nastavenie(&self) -> &NastavenieHdmi {
        &self.nastavenie
    }

    pub fn cesta(&self) -> CestaZvuku {
        self.cesta
    }

    /// Bajtov na jeden rámec výstupu.
    pub fn bajtov_na_ramec(&self) -> usize {
        self.bajtov * self.nastavenie.kanalov as usize
    }

    /// Po posune v súbore: rozpracovaná dávka priechodu sa zahodí.
    pub fn prerusenie(&mut self) {
        self.davka = Davka::default();
    }

    fn zapis(&self, v: i64, ciel: &mut Vec<u8>) {
        if self.bajtov == 2 {
            ciel.extend_from_slice(&(v as i16).to_le_bytes());
        } else {
            ciel.extend_from_slice(&((v << (32 - self.bitov)) as i32).to_le_bytes());
        }
    }

    /// Dekódovaný blok → prekladané rámce v poradí CTA-861 (alebo stereo zo zmiešania).
    pub fn pcm(&mut self, blok: &BlokZvuku) -> Vysledok<ZvukHdmi> {
        if matches!(self.cesta, CestaZvuku::Priechod(_)) {
            return Err(Chyba::MimoRozsahu("pri priechode sa posielajú pakety, nie vzorky".into()));
        }
        blok.over()?;
        if blok.frekvencia != self.frekvencia || blok.rozlozenie != self.rozlozenie {
            return Err(Chyba::MimoRozsahu(format!(
                "zmena formátu zvuku ({} Hz, {} kanálov) – treba nový prevodník a nové nastavenie výstupu",
                blok.frekvencia,
                blok.kanalov()
            )));
        }
        let n = blok.ramcov;
        let mut data = Vec::with_capacity(n * self.bajtov_na_ramec());
        let vzorka = |c: usize, i: usize| -> f64 {
            match &blok.vzorky {
                VzorkyZvuku::Cele { bitov, data } => data[c * n + i] as f64 / (1u64 << (*bitov - 1)) as f64,
                VzorkyZvuku::F32(d) => d[c * n + i] as f64,
            }
        };
        if self.cesta == CestaZvuku::ZmiesanieNaStereo {
            for i in 0..n {
                for strana in 0..2 {
                    let y: f64 =
                        self.zmiesanie.iter().enumerate().map(|(c, k)| k[strana] * vzorka(c, i)).sum();
                    self.zapis(f_na_cele(y, self.bitov), &mut data);
                }
            }
        } else {
            for i in 0..n {
                for s in &self.sloty {
                    let v = match (s, &blok.vzorky) {
                        (None, _) => 0,
                        (Some(c), VzorkyZvuku::Cele { bitov, data }) => {
                            cele_na(data[c * n + i], *bitov as u32, self.bitov)
                        }
                        (Some(c), VzorkyZvuku::F32(d)) => f_na_cele(d[c * n + i] as f64, self.bitov),
                    };
                    self.zapis(v, &mut data);
                }
            }
        }
        Ok(ZvukHdmi { data, ramcov: n, pts_ns: blok.pts_ns })
    }

    /// Paket AC-3/E-AC-3 → dávky IEC 61937 (môže byť prázdne, kým E-AC-3 nenazbiera 6 blokov).
    pub fn priechod(&mut self, paket: &Paket) -> Vysledok<ZvukHdmi> {
        let CestaZvuku::Priechod(druh) = self.cesta else {
            return Err(Chyba::MimoRozsahu("stopa nejde priechodom".into()));
        };
        if paket.prerusenie {
            self.prerusenie();
        }
        let mut vystup = ZvukHdmi::default();
        let mut o = 0;
        let mut vzoriek_od_zaciatku = 0u64;
        let pts_paketu = paket.pts.and_then(|t| na_ns(t, self.zakladna));
        while o < paket.data.len() {
            let h = ramce::ac3(&paket.data[o..])
                .filter(|h| h.dlzka >= 8 && o + h.dlzka <= paket.data.len())
                .ok_or_else(|| {
                    Chyba::PoskodeneData(format!("paket {} nie je celý rámec AC-3/E-AC-3", druh_meno(druh)))
                })?;
            let ramec = &paket.data[o..o + h.dlzka];
            let pts = pts_paketu.map(|t| {
                t.saturating_add((vzoriek_od_zaciatku as i64 * 1_000_000_000) / h.frekvencia.max(1) as i64)
            });
            match druh {
                DruhPriechodu::Ac3 => {
                    let bsmod = (ramec[5] & 7) as u16;
                    self.davka_von(
                        0x0001 | (bsmod << 8),
                        ramec,
                        (ramec.len() * 8) as u16,
                        1536,
                        pts,
                        &mut vystup,
                    );
                }
                DruhPriechodu::Eac3 => {
                    if !h.zavisly && self.davka.blokov >= 6 {
                        let d = std::mem::take(&mut self.davka);
                        self.davka_eac3(d, &mut vystup);
                    }
                    if self.davka.data.is_empty() {
                        self.davka.pts_ns = pts;
                    }
                    if !h.zavisly {
                        self.davka.blokov += h.vzoriek / 256;
                    }
                    self.davka.data.extend_from_slice(ramec);
                    if self.davka.data.len() > 6144 * 4 - 8 {
                        // viac ako sa zmestí do periódy – poškodený tok
                        self.davka = Davka::default();
                        return Err(Chyba::PoskodeneData("dávka E-AC-3 presahuje periódu IEC 61937".into()));
                    }
                }
            }
            if !h.zavisly {
                vzoriek_od_zaciatku += h.vzoriek as u64;
            }
            o += h.dlzka;
        }
        // úplná dávka E-AC-3 ide von hneď (nečaká na ďalší paket)
        if druh == DruhPriechodu::Eac3 && self.davka.blokov >= 6 {
            let d = std::mem::take(&mut self.davka);
            self.davka_eac3(d, &mut vystup);
        }
        Ok(vystup)
    }

    fn davka_eac3(&self, d: Davka, vystup: &mut ZvukHdmi) {
        self.davka_von(0x0015, &d.data, d.data.len() as u16, 6144, d.pts_ns, vystup);
    }

    /// Jedna dávka IEC 61937: preambuly, údaje ako 16-bitové slová, nuly do periódy `perioda` rámcov.
    fn davka_von(
        &self,
        pc: u16,
        udaje: &[u8],
        pd: u16,
        perioda: usize,
        pts: Option<i64>,
        vystup: &mut ZvukHdmi,
    ) {
        if vystup.ramcov == 0 {
            vystup.pts_ns = pts;
        }
        let slov = perioda * 2;
        let mut slova = [0xF872u16, 0x4E1F, pc, pd].to_vec();
        slova.extend(udaje.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])));
        slova.resize(slov.max(slova.len()), 0);
        for w in slova {
            if self.bajtov == 2 {
                vystup.data.extend_from_slice(&w.to_le_bytes());
            } else {
                vystup.data.extend_from_slice(&((w as u32) << 16).to_le_bytes());
            }
        }
        vystup.ramcov += perioda;
    }
}

fn druh_meno(d: DruhPriechodu) -> &'static str {
    match d {
        DruhPriechodu::Ac3 => "AC-3",
        DruhPriechodu::Eac3 => "E-AC-3",
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::cas::zakladna_z_frekvencie;
    use crate::stopa::{ParametreZvuku, PopisPcm, PoradieBajtov};

    fn stopa(kodek: Kodek, frekvencia: u32, rozlozenie: RozlozenieKanalov, bitov: Option<u8>) -> Stopa {
        let mut s = Stopa::nova(1, kodek, zakladna_z_frekvencie(frekvencia as u64).unwrap());
        s.zvuk = Some(ParametreZvuku { frekvencia, kanalov: rozlozenie.kanalov() as u8, rozlozenie, bitov });
        s
    }

    fn tv(formaty: &[FormatVzoriekHdmi], kanalov: u8) -> SchopnostiHdmi {
        SchopnostiHdmi {
            formaty: formaty.to_vec(),
            max_kanalov: kanalov,
            frekvencie: vec![32_000, 44_100, 48_000, 96_000, 192_000],
            max_bitov: 24,
            priechod_ac3: true,
            priechod_eac3: true,
            oneskorenie_tv_ns: None,
        }
    }

    #[test]
    fn sloty_a_kody_cta() {
        let x = |m: u32| sloty_cta(RozlozenieKanalov(m)).unwrap();
        assert_eq!(x(R::STEREO.0), (0x00, vec![Some(0), Some(1)]));
        assert_eq!(x(R::PREDNY_STRED), (0x00, vec![Some(0), Some(0)]));
        // 5.1 (FL FR FC LFE BL BR) → FL FR LFE FC RL RR, CA 0x0B
        assert_eq!(x(R::S5_1_ZADNE.0), (0x0B, vec![Some(0), Some(1), Some(3), Some(2), Some(4), Some(5)]));
        // 5.1 s bočnými (FL FR FC LFE SL SR)
        assert_eq!(x(R::S5_1.0), (0x0B, vec![Some(0), Some(1), Some(3), Some(2), Some(4), Some(5)]));
        // 7.1 (FL FR FC LFE BL BR SL SR) → bočné do RL/RR, zadné do RLC/RRC, CA 0x13
        assert_eq!(
            x(R::S7_1.0),
            (0x13, vec![Some(0), Some(1), Some(3), Some(2), Some(6), Some(7), Some(4), Some(5)])
        );
        // 3.0 → 4 kanály (LFE ticho), CA 0x02
        assert_eq!(x(R::STEREO.0 | R::PREDNY_STRED), (0x02, vec![Some(0), Some(1), None, Some(2)]));
        // 2.1 → CA 0x01; 4.0 (L R BL BR) → CA 0x08
        assert_eq!(x(R::STEREO.0 | R::BASY).0, 0x01);
        assert_eq!(
            x(R::STEREO.0 | R::ZADNY_LAVY | R::ZADNY_PRAVY),
            (0x08, vec![Some(0), Some(1), None, None, Some(2), Some(3)])
        );
        // 6.1 (5.1 bočné + zadný stred) → RC v slote 7, CA 0x0F
        let (ca, s) = x(R::S5_1.0 | R::ZADNY_STRED);
        assert_eq!((ca, s[6]), (0x0F, Some(4)));
        // nemožné bez zmiešania: horné kanály, polovica páru
        assert!(sloty_cta(RozlozenieKanalov(R::STEREO.0 | R::HORNY_STRED)).is_none());
        assert!(sloty_cta(RozlozenieKanalov(R::STEREO.0 | R::BOCNY_LAVY)).is_none());
    }

    #[test]
    fn stav_kanala_iec60958() {
        // zhodný so zapisovačom prúdu A (R226): 48 kHz, 24 bitov
        assert_eq!(&stav_kanala(true, 48_000, 24)[..5], &[0x04, 0, 0, 0x02, 0x0B]);
        assert_eq!(&stav_kanala(true, 44_100, 16)[..5], &[0x04, 0, 0, 0x00, 0x02]);
        assert_eq!(&stav_kanala(false, 192_000, 16)[..5], &[0x06, 0, 0, 0x0E, 0]);
    }

    #[test]
    fn bitovo_presne_pcm_v_poradi_cta() {
        let s = stopa(Kodek::Flac, 48_000, R::S5_1_ZADNE, Some(24));
        let mut p = PrevodnikZvuku::novy(
            &s,
            &tv(&[FormatVzoriekHdmi::S32Le, FormatVzoriekHdmi::S16Le], 8),
            VolbyZvuku::default(),
        )
        .unwrap();
        assert_eq!(p.cesta(), CestaZvuku::BitovoPresne);
        let n = p.nastavenie();
        assert_eq!(
            (n.format, n.kanalov, n.pridelenie_cta, n.frekvencia),
            (FormatVzoriekHdmi::S32Le, 6, 0x0B, 48_000)
        );
        let mut b = BlokZvuku::cele(48_000, R::S5_1_ZADNE, 24);
        b.ramcov = 2;
        b.vzorky = VzorkyZvuku::Cele {
            bitov: 24,
            data: vec![1, 2, 10, 20, 100, 200, 1000, 2000, -1, -2, 8_388_607, -8_388_608],
        };
        let z = p.pcm(&b).unwrap();
        let v: Vec<i32> = z.data.chunks(4).map(|c| i32::from_le_bytes(c.try_into().unwrap()) >> 8).collect();
        assert_eq!(v, [1, 10, 1000, 100, -1, 8_388_607, 2, 20, 2000, 200, -2, -8_388_608]);
        assert_eq!(z.ramcov, 2);
        // 16-bitový zdroj → S16Le
        let pcm16 = PopisPcm {
            bitov: 16,
            bajtov: 2,
            poradie: PoradieBajtov::MaloKoncove,
            cislo: CisloPcm::SoZnamienkom,
        };
        let s = stopa(Kodek::Pcm(pcm16), 44_100, R::STEREO, Some(16));
        let p = PrevodnikZvuku::novy(
            &s,
            &tv(&[FormatVzoriekHdmi::S32Le, FormatVzoriekHdmi::S16Le], 2),
            VolbyZvuku::default(),
        )
        .unwrap();
        assert_eq!((p.cesta(), p.nastavenie().format), (CestaZvuku::BitovoPresne, FormatVzoriekHdmi::S16Le));
        // len podrámce IEC 60958 (vc4): S32 s 24 bitmi hore, A zabalí
        let p =
            PrevodnikZvuku::novy(&s, &tv(&[FormatVzoriekHdmi::Iec958PodramecLe], 2), VolbyZvuku::default())
                .unwrap();
        assert_eq!(
            (p.cesta(), p.nastavenie().format, p.bajtov_na_ramec()),
            (CestaZvuku::BitovoPresne, FormatVzoriekHdmi::Iec958PodramecLe, 8)
        );
    }

    #[test]
    fn zmiesanie_a_obmedzenia_televizora() {
        let s = stopa(Kodek::Flac, 48_000, R::S5_1_ZADNE, Some(16));
        // televízor len 2 kanály → zmiešanie
        let mut p =
            PrevodnikZvuku::novy(&s, &tv(&[FormatVzoriekHdmi::S16Le], 2), VolbyZvuku::default()).unwrap();
        assert_eq!((p.cesta(), p.nastavenie().kanalov), (CestaZvuku::ZmiesanieNaStereo, 2));
        let mut b = BlokZvuku::cele(48_000, R::S5_1_ZADNE, 16);
        b.ramcov = 1;
        // plný rozsah na všetkých kanáloch: normovanie zabráni orezaniu (L = R = 32767·(1 + 2·0,707)/2,414)
        b.vzorky = VzorkyZvuku::Cele { bitov: 16, data: vec![32_767; 6] };
        let z = p.pcm(&b).unwrap();
        assert_eq!(z.data, [0xFF, 0x7F, 0xFF, 0x7F]);
        // iný blok ako stopa → chyba
        let b2 = BlokZvuku::cele(44_100, R::S5_1_ZADNE, 16);
        assert!(p.pcm(&b2).is_err());
        // frekvencia, ktorú televízor nevie – bez prevzorkovania chyba
        let s = stopa(Kodek::Flac, 88_200, R::STEREO, Some(16));
        assert!(
            PrevodnikZvuku::novy(&s, &tv(&[FormatVzoriekHdmi::S16Le], 2), VolbyZvuku::default()).is_err()
        );
        // televízor 16 bitov, zdroj 24 → zaokrúhlenie (cesta Pcm)
        let s = stopa(Kodek::Flac, 48_000, R::STEREO, Some(24));
        let mut t = tv(&[FormatVzoriekHdmi::S16Le, FormatVzoriekHdmi::S32Le], 2);
        t.max_bitov = 16;
        let mut p = PrevodnikZvuku::novy(&s, &t, VolbyZvuku::default()).unwrap();
        assert_eq!((p.cesta(), p.nastavenie().format), (CestaZvuku::Pcm, FormatVzoriekHdmi::S16Le));
        let mut b = BlokZvuku::cele(48_000, R::STEREO, 24);
        b.ramcov = 1;
        b.vzorky = VzorkyZvuku::Cele { bitov: 24, data: vec![0x7F_FFFF, -0x80] };
        // 0x7FFFFF → 0x7FFF (orezanie po zaokrúhlení), −128 → −0,5 → 0 (polovicou nahor)
        assert_eq!(p.pcm(&b).unwrap().data, [0xFF, 0x7F, 0, 0]);
        // f32 zdroj (stratový kodek) → 24 bitov, cesta Pcm
        let s = stopa(Kodek::Opus, 48_000, R::STEREO, None);
        let mut p =
            PrevodnikZvuku::novy(&s, &tv(&[FormatVzoriekHdmi::S32Le], 2), VolbyZvuku::default()).unwrap();
        assert_eq!(p.cesta(), CestaZvuku::Pcm);
        let mut b = BlokZvuku::f32(48_000, R::STEREO);
        b.ramcov = 1;
        b.vzorky = VzorkyZvuku::F32(vec![0.5, -1.0]);
        let v: Vec<i32> =
            p.pcm(&b).unwrap().data.chunks(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(v, [0x4000_0000, i32::MIN]);
    }

    fn ac3_ramec(bsmod: u8) -> Vec<u8> {
        let mut r = vec![0x0B, 0x77, 0x12, 0x34, 0x14, 0x40 | bsmod, 0x40, 0x00];
        r.resize(768, 0x5A);
        r
    }

    #[test]
    fn priechod_ac3_iec61937() {
        let s = stopa(Kodek::Ac3, 48_000, R::S5_1, None);
        let mut p =
            PrevodnikZvuku::novy(&s, &tv(&[FormatVzoriekHdmi::S16Le], 8), VolbyZvuku::default()).unwrap();
        assert_eq!(p.cesta(), CestaZvuku::Priechod(DruhPriechodu::Ac3));
        let n = p.nastavenie();
        assert_eq!((n.kanalov, n.frekvencia, n.stav_kanala[0]), (2, 48_000, 0x06));
        let paket = Paket {
            stopa: 0,
            data: [ac3_ramec(2), ac3_ramec(2)].concat(),
            pts: Some(48_000),
            ..Paket::default()
        };
        let z = p.priechod(&paket).unwrap();
        assert_eq!((z.ramcov, z.data.len()), (3072, 12_288));
        assert_eq!(z.pts_ns, Some(1_000_000_000));
        // Pa, Pb, Pc = 1 | bsmod 2 << 8, Pd = 768·8 bitov; slová malokoncovo, údaje po 16-bitových slovách
        assert_eq!(&z.data[..12], &[0x72, 0xF8, 0x1F, 0x4E, 0x01, 0x02, 0x00, 0x18, 0x77, 0x0B, 0x34, 0x12]);
        assert!(z.data[8 + 768..6144].iter().all(|&b| b == 0));
        assert_eq!(&z.data[6144..6148], &[0x72, 0xF8, 0x1F, 0x4E]);
        // televízor bez AC-3 alebo zakázaný priechod → dekódovanie (PCM)
        let mut t = tv(&[FormatVzoriekHdmi::S16Le], 8);
        t.priechod_ac3 = false;
        assert_eq!(PrevodnikZvuku::novy(&s, &t, VolbyZvuku::default()).unwrap().cesta(), CestaZvuku::Pcm);
        let v = VolbyZvuku { priechod: false, stereo: false };
        assert_ne!(
            PrevodnikZvuku::novy(&s, &tv(&[FormatVzoriekHdmi::S16Le], 8), v).unwrap().cesta(),
            CestaZvuku::Priechod(DruhPriechodu::Ac3)
        );
        // poškodený paket
        assert!(p.priechod(&Paket { data: vec![0x0B, 0x77, 0, 0], ..Paket::default() }).is_err());
    }

    #[test]
    fn priechod_eac3_davky_po_6_blokoch() {
        let s = stopa(Kodek::Eac3, 48_000, R::STEREO, None);
        let mut p =
            PrevodnikZvuku::novy(&s, &tv(&[FormatVzoriekHdmi::Iec958PodramecLe], 2), VolbyZvuku::default())
                .unwrap();
        let n = p.nastavenie();
        assert_eq!(
            (n.frekvencia, n.stav_kanala[3], n.format),
            (192_000, 0x0E, FormatVzoriekHdmi::Iec958PodramecLe)
        );
        // rámce po 3 blokoch (numblkscod 2) so závislým podtokom: dávka = 2 nezávislé + 2 závislé rámce
        let r = |zavisly: bool, v: u8| {
            let n = 200usize;
            let frmsiz = n / 2 - 1;
            let mut x = vec![
                0x0B,
                0x77,
                if zavisly { 0x40 } else { 0 } | (frmsiz >> 8) as u8,
                frmsiz as u8,
                0x24,
                0x80,
                0,
                0,
            ];
            x.resize(n, v);
            x
        };
        let z = p
            .priechod(&Paket { data: [r(false, 1), r(true, 2)].concat(), pts: Some(0), ..Paket::default() })
            .unwrap();
        assert_eq!(z.ramcov, 0);
        let z = p
            .priechod(&Paket { data: [r(false, 3), r(true, 4)].concat(), pts: Some(768), ..Paket::default() })
            .unwrap();
        assert_eq!((z.ramcov, z.data.len(), z.pts_ns), (6144, 6144 * 2 * 4, Some(0)));
        let slovo =
            |i: usize| (u32::from_le_bytes(z.data[i * 4..i * 4 + 4].try_into().unwrap()) >> 16) as u16;
        assert_eq!([slovo(0), slovo(1), slovo(2), slovo(3), slovo(4)], [0xF872, 0x4E1F, 0x0015, 800, 0x0B77]);
    }
}
