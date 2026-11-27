//! **AC-3** (L9) podľa textu normy **ATSC A/52:2018** (Digital Audio Compression Standard). Čísla tabuliek a oddielov
//! v komentároch sú z normy; tabuľky generuje `media/nastroje/ac3_tabulky.py` z textu normy (kód iných dekodérov
//! sa nečíta).
//!
//! Postup dekódovania (§6.1, obrázok 6.1): synchronizácia a CRC (§7.10.1) → BSI (tabuľka 5.2) → pre každý zo 6
//! blokov rozbor `audblk` (tabuľka 5.3) → exponenty (§7.1.3) → rozdelenie bitov (§7.2.2, celočíselne presne podľa
//! pseudokódu) → mantisy (§7.3: skupiny, symetrická a asymetrická kvantizácia, dither) → väzba kanálov (§7.4) →
//! rematrix (§7.5) → dynamický rozsah dynrng (§7.7.1, predvolene zapnutý – „shall, by default“) → IMDCT, okno,
//! prekrytie (§7.9). Výstup: všetky kanály toku v spoločnom poradí (FL FR FC LFE … podľa masky rozloženia),
//! plávajúca čiarka ±1,0, bez zmiešania (§7.8 patrí výstupu s menej kanálmi – R5b zmiešava na stereo).
//!
//! **Miesta, kde text nestačí alebo je nepresný – rozhodnuté a zapísané:**
//! - pseudokód `calc_lowcomp` (§7.2.2.4) má za `if ((b0 + 256) == b1)` bodkočiarku (prázdny príkaz) – v ďalších
//!   vetvách rovnakého tvaru nie je; čítame ho bez nej (podmienka platí pre priradenie 384), rozhodne meranie;
//! - `dexp = M − 2` (§7.1.3 krok 2 – v PDF chýba znamienko mínus; pseudokód ďalej ho má);
//! - floortab[7] = 0xf800 je 16-bitové −2048 (§7.2.1: doplnkový kód);
//! - dither (§7.3.4): ľubovoľná „dostatočne náhodná“ postupnosť, rovnomerne ±0,707 – vlastný generátor (xorshift),
//!   pri väzbe sa generuje pre každý kanál zvlášť a násobí koordinátom väzby kanála.

#![allow(clippy::needless_range_loop)]

pub mod dekoder;
mod imdct;
#[allow(clippy::unreadable_literal, clippy::excessive_precision)]
mod tabulky;

use crate::zvuk::RozlozenieKanalov;
use orchidea_jadro::{Chyba, Vysledok};
use tabulky::*;

fn chyba(sprava: &str) -> Chyba {
    Chyba::PoskodeneData(format!("AC-3: {sprava}"))
}

/// Čítanie bitov od najvyššieho (A/52 §5.2: prvý bit je najvyšší).
#[derive(Clone, Debug)]
struct Bity<'a> {
    data: &'a [u8],
    poloha: usize,
}

impl<'a> Bity<'a> {
    fn bit(&mut self) -> u32 {
        let b = self.data.get(self.poloha >> 3).map_or(0, |&b| (b >> (7 - (self.poloha & 7))) & 1);
        self.poloha += 1;
        b as u32
    }
    fn citaj(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |v, _| (v << 1) | self.bit())
    }
    fn priznak(&mut self) -> bool {
        self.bit() == 1
    }
    fn preskoc(&mut self, n: usize) {
        self.poloha += n;
    }
}

/// CRC-16 s mnohočlenom x^16 + x^15 + x^2 + 1 (§7.10.1), register od nuly, bity od najvyššieho.
pub fn crc16(data: &[u8]) -> u16 {
    let mut r: u16 = 0;
    for &b in data {
        r ^= (b as u16) << 8;
        for _ in 0..8 {
            r = if r & 0x8000 != 0 { (r << 1) ^ 0x8005 } else { r << 1 };
        }
    }
    r
}

/// Hlavička rámca – syncinfo (tabuľka 5.1) a začiatok BSI (tabuľka 5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hlavicka {
    pub fscod: u8,
    pub frmsizecod: u8,
    pub frekvencia: u32,
    /// Dĺžka rámca v bajtoch (tabuľka 5.18).
    pub dlzka: usize,
    pub bsid: u8,
    pub bsmod: u8,
    pub acmod: u8,
    pub lfeon: bool,
}

impl Hlavicka {
    /// Rozpozná hlavičku AC-3 (bsid ≤ 8; E-AC-3 s bsid 11–16 nie je AC-3).
    pub fn z_bajtov(b: &[u8]) -> Option<Hlavicka> {
        if b.len() < 7 || b[0] != 0x0B || b[1] != 0x77 {
            return None;
        }
        let fscod = b[4] >> 6;
        let frmsizecod = b[4] & 0x3F;
        if fscod == 3 || frmsizecod >= 38 {
            return None; // tabuľka 5.6 (vyhradené) a tabuľka 5.18
        }
        let bsid = b[5] >> 3;
        if bsid > 8 {
            return None; // §5.4.2.1 – vyššie bsid sú prílohy (E-AC-3)
        }
        let acmod = b[6] >> 5;
        // lfeon nasleduje za voliteľnými poliami cmixlev, surmixlev, dsurmod (tabuľka 5.2)
        let mut poloha = 3;
        if acmod & 1 != 0 && acmod != 1 {
            poloha += 2;
        }
        if acmod & 4 != 0 {
            poloha += 2;
        }
        if acmod == 2 {
            poloha += 2;
        }
        let lfeon = (b[6] >> (7 - poloha)) & 1 != 0;
        Some(Hlavicka {
            fscod,
            frmsizecod,
            frekvencia: [48_000, 44_100, 32_000][fscod as usize],
            dlzka: VELKOST_RAMCA[frmsizecod as usize][fscod as usize] as usize * 2,
            bsid,
            bsmod: b[5] & 7,
            acmod,
            lfeon,
        })
    }

    /// Počet kanálov plnej šírky (tabuľka 5.8).
    pub fn nfchans(&self) -> usize {
        [2, 1, 2, 3, 3, 4, 4, 5][self.acmod as usize]
    }

    /// Všetky kanály vrátane LFE.
    pub fn kanalov(&self) -> usize {
        self.nfchans() + self.lfeon as usize
    }

    /// Rozloženie výstupu (spoločné poradie): 1+1 ako dva kanály, 2/1 so zadným stredom, 2/2 a 3/2 s bočnými.
    pub fn rozlozenie(&self) -> RozlozenieKanalov {
        type R = RozlozenieKanalov;
        let maska = match self.acmod {
            0 | 2 => R::STEREO.0,
            1 => R::PREDNY_STRED,
            3 => R::STEREO.0 | R::PREDNY_STRED,
            4 => R::STEREO.0 | R::ZADNY_STRED,
            5 => R::STEREO.0 | R::PREDNY_STRED | R::ZADNY_STRED,
            6 => R::STEREO.0 | R::BOCNY_LAVY | R::BOCNY_PRAVY,
            _ => R::STEREO.0 | R::PREDNY_STRED | R::BOCNY_LAVY | R::BOCNY_PRAVY,
        } | if self.lfeon { R::BASY } else { 0 };
        RozlozenieKanalov(maska)
    }

    /// Poradie výstupu: pre každý výstupný kanál index v poradí toku (tabuľka 5.8; LFE je posledný v toku).
    fn poradie_vystupu(&self) -> Vec<usize> {
        let lfe = self.nfchans();
        let mut p: Vec<usize> = match self.acmod {
            3 => vec![0, 2, 1],       // L C R → L R C
            5 => vec![0, 2, 1, 3],    // L C R S → L R C S
            7 => vec![0, 2, 1, 3, 4], // L C R SL SR → L R C SL SR
            _ => (0..self.nfchans()).collect(),
        };
        if self.lfeon {
            // LFE je v maske za predným stredom (FL FR FC LFE …)
            let za = if self.acmod & 1 != 0 && self.acmod != 1 {
                3
            } else if self.acmod == 1 {
                1
            } else {
                2
            };
            p.insert(za, lfe);
        }
        p
    }
}

/// Stratégia exponentov (tabuľka 7.4): 0 = opakovať, 1 = D15, 2 = D25, 3 = D45.
fn velkost_skupiny(strategia: u8) -> usize {
    [0, 1, 2, 4][strategia as usize]
}

/// Delta rozdelenia bitov jedného kanála (§5.4.3.47–57, §7.2.2.6): segmenty (posun, dĺžka, delta kód).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Delta {
    segmenty: Vec<(u8, u8, u8)>,
}

/// Parametre rozdelenia bitov spoločné pre blok (§5.4.3.30–35).
#[derive(Clone, Copy, Debug, Default)]
struct ParametreRozdelenia {
    sdcycod: u8,
    fdcycod: u8,
    sgaincod: u8,
    dbpbcod: u8,
    floorcod: u8,
    csnroffst: u8,
}

/// Štatistika pokrytia nástrojov (pre rozhodcu).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatistikaAc3 {
    pub ramcov: u64,
    pub blokov: u64,
    pub kratke: u64,
    pub vazba: u64,
    pub fazove_priznaky: u64,
    pub rematrix: u64,
    pub d15: u64,
    pub d25: u64,
    pub d45: u64,
    pub opakovane_exponenty: u64,
    pub delta: u64,
    pub dither: u64,
    pub dynrng: u64,
    pub preskok: u64,
    pub lfe: u64,
    pub nulove_rozdelenie: u64,
    pub chyby_crc: u64,
}

/// Stav kanála plnej šírky (index 0–4), väzbového kanála (5) a LFE (6) v rámci rámca.
const VAZBOVY: usize = 5;
const LFE: usize = 6;

#[derive(Clone, Debug)]
struct StavRamca {
    blksw: [bool; 5],
    dithflag: [bool; 5],
    dynrng: [u8; 2],
    cplinu: bool,
    chincpl: [bool; 5],
    phsflginu: bool,
    cplbegf: usize,
    cplendf: usize,
    cplbndstrc: [bool; 18],
    /// Koordináty väzby po podpásmach (už rozšírené podľa cplbndstrc).
    cplco: [[f32; 18]; 5],
    phsflg: [bool; 18],
    rematflg: [bool; 4],
    strategia: [u8; 7],
    endmant: [usize; 5],
    exp: [[u8; 256]; 7],
    rozdelenie: ParametreRozdelenia,
    fsnroffst: [u8; 7],
    fgaincod: [u8; 7],
    cplfleak: u8,
    cplsleak: u8,
    delta: [Option<Delta>; 6],
    /// Väzba bola v predošlom bloku (kontroly §7.10.2).
    cplinu_predtym: bool,
}

impl StavRamca {
    fn novy() -> StavRamca {
        StavRamca {
            blksw: [false; 5],
            dithflag: [false; 5],
            dynrng: [0; 2],
            cplinu: false,
            chincpl: [false; 5],
            phsflginu: false,
            cplbegf: 0,
            cplendf: 0,
            cplbndstrc: [false; 18],
            cplco: [[0.0; 18]; 5],
            phsflg: [false; 18],
            rematflg: [false; 4],
            strategia: [0; 7],
            endmant: [0; 5],
            exp: [[0; 256]; 7],
            rozdelenie: ParametreRozdelenia::default(),
            fsnroffst: [0; 7],
            fgaincod: [0; 7],
            cplfleak: 0,
            cplsleak: 0,
            delta: Default::default(),
            cplinu_predtym: false,
        }
    }

    fn cplstrtmant(&self) -> usize {
        37 + 12 * self.cplbegf
    }
    fn cplendmant(&self) -> usize {
        37 + 12 * (self.cplendf + 3)
    }
}

/// Dekodér AC-3: jeden rámec → 1536 vzoriek každého kanála.
#[derive(Clone, Debug)]
pub struct DekoderAc3 {
    imdct: imdct::Imdct,
    /// Druhá polovica okna predošlého bloku, v poradí toku (kanály plnej šírky, potom LFE).
    oneskorenie: [[f32; 256]; 6],
    nahoda: u32,
    hlavicka: Option<Hlavicka>,
    statistika: StatistikaAc3,
    /// Ladenie rozhodcu: zakáže dither (výstup nuly ako pri dithflag = 0).
    pub bez_ditheru: bool,
    /// Kontrola crc1 a crc2 (predvolene zapnutá; fuzzer ju vypne, aby skúšal rozbor poškodených rámcov).
    pub kontrola_crc: bool,
    /// Pri zázname: bitové polohy blksw a dithflag v každom bloku (nástroj rozhodcu `media_ac3 --uprav`).
    polohy: Option<Vec<(Vec<usize>, Vec<usize>)>>,
}

impl Default for DekoderAc3 {
    fn default() -> Self {
        Self::novy()
    }
}

impl DekoderAc3 {
    pub fn novy() -> DekoderAc3 {
        DekoderAc3 {
            imdct: imdct::Imdct::nova(),
            oneskorenie: [[0.0; 256]; 6],
            nahoda: 0x2545_F491,
            hlavicka: None,
            statistika: StatistikaAc3::default(),
            bez_ditheru: false,
            kontrola_crc: true,
            polohy: None,
        }
    }

    pub fn reset(&mut self) {
        self.oneskorenie = [[0.0; 256]; 6];
        self.hlavicka = None;
    }

    pub fn hlavicka(&self) -> Option<Hlavicka> {
        self.hlavicka
    }

    pub fn statistika(&self) -> StatistikaAc3 {
        self.statistika
    }

    /// Bitové polohy príznakov blksw a dithflag v každom bloku rámca (od začiatku rámca) – rámec sa pritom dekóduje.
    pub fn polohy_priznakov(&mut self, ramec: &[u8]) -> Vysledok<Vec<(Vec<usize>, Vec<usize>)>> {
        self.polohy = Some(Vec::new());
        let v = self.dekoduj_ramec(ramec);
        let p = self.polohy.take().unwrap_or_default();
        v.map(|_| p)
    }

    /// Dither: rovnomerne v ±0,707 (§7.3.4) – xorshift32, horných 16 bitov.
    fn dither(&mut self) -> f32 {
        let mut x = self.nahoda;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.nahoda = x;
        ((x >> 16) as i32 - 32768) as f32 * (0.707 / 32768.0)
    }

    /// Dekóduje jeden synchronizačný rámec: kanály v spoločnom poradí (`Hlavicka::rozlozenie`), po 1536 vzoriek.
    pub fn dekoduj_ramec(&mut self, ramec: &[u8]) -> Vysledok<Vec<Vec<f32>>> {
        let h = Hlavicka::z_bajtov(ramec).ok_or_else(|| chyba("hlavička"))?;
        if ramec.len() < h.dlzka {
            return Err(chyba("rámec je kratší ako frmsizecod"));
        }
        let ramec = &ramec[..h.dlzka];
        // §7.10.1: crc1 – prvých 5/8 bez synchronizačného slova, crc2 – celý rámec bez synchronizačného slova
        let slov = h.dlzka / 2;
        let pat_osmin = (slov >> 1) + (slov >> 3);
        if self.kontrola_crc && (crc16(&ramec[2..pat_osmin * 2]) != 0 || crc16(&ramec[2..]) != 0) {
            self.statistika.chyby_crc += 1;
            return Err(chyba("CRC"));
        }
        if let Some(p) = self.hlavicka
            && (p.acmod != h.acmod || p.lfeon != h.lfeon)
        {
            // zmena kanálov: oneskorenie patrí iným kanálom
            self.oneskorenie = [[0.0; 256]; 6];
        }
        self.hlavicka = Some(h);
        self.statistika.ramcov += 1;
        let nfchans = h.nfchans();
        let mut b = Bity { data: ramec, poloha: 40 };
        // BSI (tabuľka 5.2)
        b.preskoc(5 + 3 + 3); // bsid, bsmod, acmod (už v hlavičke)
        if h.acmod & 1 != 0 && h.acmod != 1 {
            b.preskoc(2); // cmixlev
        }
        if h.acmod & 4 != 0 {
            b.preskoc(2); // surmixlev
        }
        if h.acmod == 2 {
            b.preskoc(2); // dsurmod
        }
        b.preskoc(1); // lfeon
        for druhy in 0..if h.acmod == 0 { 2 } else { 1 } {
            let _ = druhy;
            b.preskoc(5); // dialnorm
            if b.priznak() {
                b.preskoc(8); // compr
            }
            if b.priznak() {
                b.preskoc(8); // langcod
            }
            if b.priznak() {
                b.preskoc(5 + 2); // mixlevel, roomtyp
            }
        }
        b.preskoc(2); // copyrightb, origbs
        if b.priznak() {
            b.preskoc(14); // timecod1 (príloha D: xbsi1 rovnakej dĺžky)
        }
        if b.priznak() {
            b.preskoc(14); // timecod2 (príloha D: xbsi2)
        }
        if b.priznak() {
            let addbsil = b.citaj(6) as usize;
            b.preskoc((addbsil + 1) * 8);
        }
        let mut stav = StavRamca::novy();
        let poradie = h.poradie_vystupu();
        let mut vystup: Vec<Vec<f32>> = vec![vec![0.0; 1536]; h.kanalov()];
        let mut pcm = [0f32; 256];
        for blk in 0..6 {
            let koef = self.blok(&mut b, &h, &mut stav, blk)?;
            if b.poloha > h.dlzka * 8 {
                return Err(chyba("blok presahuje koniec rámca")); // §7.10.2 bod 21
            }
            self.statistika.blokov += 1;
            for ch in 0..h.kanalov() {
                let kratke = ch < nfchans && stav.blksw[ch];
                self.imdct.blok(&koef[ch], kratke, &mut self.oneskorenie[ch], &mut pcm);
                let ciel = poradie.iter().position(|&p| p == ch).unwrap_or(ch);
                vystup[ciel][blk * 256..(blk + 1) * 256].copy_from_slice(&pcm);
            }
        }
        Ok(vystup)
    }

    /// Jeden blok `audblk` (tabuľka 5.3): rozbor a výpočet transformačných koeficientov všetkých kanálov.
    fn blok(
        &mut self,
        b: &mut Bity,
        h: &Hlavicka,
        s: &mut StavRamca,
        blk: usize,
    ) -> Vysledok<Vec<[f32; 256]>> {
        let nfchans = h.nfchans();
        let acmod = h.acmod;
        let zac = b.poloha;
        for ch in 0..nfchans {
            s.blksw[ch] = b.priznak();
            self.statistika.kratke += s.blksw[ch] as u64;
        }
        for ch in 0..nfchans {
            s.dithflag[ch] = b.priznak();
        }
        if let Some(p) = self.polohy.as_mut() {
            p.push(((zac..zac + nfchans).collect(), (zac + nfchans..zac + 2 * nfchans).collect()));
        }
        // dynamický rozsah (§5.4.3.3–6): bez nového slova sa opakuje, v bloku 0 je 0
        for i in 0..if acmod == 0 { 2 } else { 1 } {
            if b.priznak() {
                s.dynrng[i] = b.citaj(8) as u8;
                self.statistika.dynrng += (s.dynrng[i] != 0) as u64;
            } else if blk == 0 {
                s.dynrng[i] = 0;
            }
        }
        // stratégia väzby (§5.4.3.7–13)
        let cplinu_predtym = if blk == 0 { false } else { s.cplinu };
        s.cplinu_predtym = cplinu_predtym;
        if b.priznak() {
            s.cplinu = b.priznak();
            if s.cplinu {
                if acmod < 2 {
                    return Err(chyba("väzba v režime 1/0 alebo 1+1")); // §7.10.2 bod 22
                }
                for ch in 0..nfchans {
                    s.chincpl[ch] = b.priznak();
                }
                s.phsflginu = acmod == 2 && b.priznak();
                s.cplbegf = b.citaj(4) as usize;
                s.cplendf = b.citaj(4) as usize;
                if s.cplbegf > s.cplendf + 2 {
                    return Err(chyba("cplbegf > cplendf + 2")); // bod 3
                }
                let ncplsubnd = 3 + s.cplendf - s.cplbegf;
                s.cplbndstrc = [false; 18];
                for bnd in 1..ncplsubnd {
                    s.cplbndstrc[s.cplbegf + bnd] = b.priznak();
                }
                if s.chincpl[..nfchans].iter().filter(|&&c| c).count() < 2 {
                    return Err(chyba("menej ako dva kanály vo väzbe")); // bod 2
                }
            } else {
                s.chincpl = [false; 5];
                s.phsflginu = false;
            }
        } else if blk == 0 {
            return Err(chyba("cplstre = 0 v bloku 0")); // bod 1
        }
        if s.cplinu {
            self.statistika.vazba += 1;
            // koordináty väzby (§5.4.3.14–18, §7.4.3)
            let ncplsubnd = 3 + s.cplendf - s.cplbegf;
            let mut pasma: Vec<(usize, usize)> = Vec::new(); // (prvé podpásmo, počet)
            for sb in s.cplbegf..s.cplbegf + ncplsubnd {
                if sb > s.cplbegf && s.cplbndstrc[sb] {
                    pasma.last_mut().expect("prvé pásmo").1 += 1;
                } else {
                    pasma.push((sb, 1));
                }
            }
            let mut cplcoe_niektory = false;
            for ch in 0..nfchans {
                if !s.chincpl[ch] {
                    continue;
                }
                let cplcoe = b.priznak();
                if !cplcoe && !cplinu_predtym {
                    return Err(chyba("chýbajú koordináty väzby")); // bod 4
                }
                if cplcoe {
                    cplcoe_niektory |= ch < 2;
                    let mstrcplco = b.citaj(2) as i32;
                    for &(sb, n) in &pasma {
                        let e = b.citaj(4) as i32;
                        let m = b.citaj(4) as f32;
                        let docasne = if e == 15 { m / 16.0 } else { (m + 16.0) / 32.0 };
                        let co = docasne * mocnina2(-(e + 3 * mstrcplco));
                        for k in sb..sb + n {
                            s.cplco[ch][k] = co;
                        }
                    }
                }
            }
            if acmod == 2 && s.phsflginu && cplcoe_niektory {
                for &(sb, n) in &pasma {
                    let f = b.priznak();
                    for k in sb..sb + n {
                        s.phsflg[k] = f;
                    }
                }
            }
            if !s.phsflginu {
                s.phsflg = [false; 18];
            }
        }
        // rematrix (§5.4.3.19–20, tabuľka 5.15)
        if acmod == 2 {
            if b.priznak() {
                let pasiem = rematrix_pasiem(s);
                for r in 0..4 {
                    s.rematflg[r] = r < pasiem && b.priznak();
                }
            } else if blk == 0 {
                return Err(chyba("rematstr = 0 v bloku 0")); // bod 5
            }
        }
        // stratégie exponentov (§5.4.3.21–24)
        if s.cplinu {
            s.strategia[VAZBOVY] = b.citaj(2) as u8;
            if s.strategia[VAZBOVY] == 0 && !cplinu_predtym {
                return Err(chyba("cplexpstr = opakovať bez predošlej väzby")); // bod 6
            }
        }
        for ch in 0..nfchans {
            s.strategia[ch] = b.citaj(2) as u8;
            if s.strategia[ch] == 0 && blk == 0 {
                return Err(chyba("chexpstr = opakovať v bloku 0")); // bod 8
            }
        }
        if h.lfeon {
            s.strategia[LFE] = b.citaj(1) as u8;
            if s.strategia[LFE] == 0 && blk == 0 {
                return Err(chyba("lfeexpstr = opakovať v bloku 0")); // bod 10
            }
            self.statistika.lfe += 1;
        }
        for ch in 0..nfchans {
            if s.strategia[ch] != 0 {
                if s.chincpl[ch] {
                    s.endmant[ch] = s.cplstrtmant();
                } else {
                    let chbwcod = b.citaj(6) as usize;
                    if chbwcod > 60 {
                        return Err(chyba("chbwcod > 60")); // bod 11
                    }
                    s.endmant[ch] = (chbwcod + 12) * 3 + 37;
                }
            }
        }
        // exponenty (§7.1.3)
        if s.cplinu && s.strategia[VAZBOVY] != 0 {
            let absexp = (b.citaj(4) as i32) << 1;
            let n = s.cplendmant() - s.cplstrtmant();
            let skupina = velkost_skupiny(s.strategia[VAZBOVY]);
            let ngrps = n / (3 * skupina);
            let mut docasne = [0u8; 256];
            dekoduj_exponenty(b, absexp, ngrps, skupina, &mut docasne)?;
            let zac = s.cplstrtmant();
            s.exp[VAZBOVY][zac..zac + n].copy_from_slice(&docasne[1..1 + n]);
        }
        for ch in 0..nfchans {
            if s.strategia[ch] != 0 {
                let absexp = b.citaj(4) as i32;
                let skupina = velkost_skupiny(s.strategia[ch]);
                let endmant = s.endmant[ch];
                let ngrps = match skupina {
                    1 => (endmant - 1) / 3,
                    2 => (endmant - 1 + 3) / 6,
                    _ => (endmant - 1 + 9) / 12,
                };
                let mut docasne = [0u8; 256];
                dekoduj_exponenty(b, absexp, ngrps, skupina, &mut docasne)?;
                s.exp[ch] = docasne;
                b.preskoc(2); // gainrng (§7.9.5 – len pre pevnú čiarku)
            }
        }
        if h.lfeon && s.strategia[LFE] != 0 {
            let absexp = b.citaj(4) as i32;
            let mut docasne = [0u8; 256];
            dekoduj_exponenty(b, absexp, 2, 1, &mut docasne)?;
            s.exp[LFE] = docasne;
        }
        for (i, &st) in s.strategia.iter().enumerate() {
            let pouzity = (i < nfchans) || (i == VAZBOVY && s.cplinu) || (i == LFE && h.lfeon);
            if pouzity {
                match st {
                    0 => self.statistika.opakovane_exponenty += 1,
                    1 => self.statistika.d15 += 1,
                    2 => self.statistika.d25 += 1,
                    _ => self.statistika.d45 += 1,
                }
            }
        }
        // parametre rozdelenia bitov (§5.4.3.30–46)
        if b.priznak() {
            s.rozdelenie.sdcycod = b.citaj(2) as u8;
            s.rozdelenie.fdcycod = b.citaj(2) as u8;
            s.rozdelenie.sgaincod = b.citaj(2) as u8;
            s.rozdelenie.dbpbcod = b.citaj(2) as u8;
            s.rozdelenie.floorcod = b.citaj(3) as u8;
        } else if blk == 0 {
            return Err(chyba("baie = 0 v bloku 0")); // bod 12
        }
        if b.priznak() {
            s.rozdelenie.csnroffst = b.citaj(6) as u8;
            if s.cplinu {
                s.fsnroffst[VAZBOVY] = b.citaj(4) as u8;
                s.fgaincod[VAZBOVY] = b.citaj(3) as u8;
            }
            for ch in 0..nfchans {
                s.fsnroffst[ch] = b.citaj(4) as u8;
                s.fgaincod[ch] = b.citaj(3) as u8;
            }
            if h.lfeon {
                s.fsnroffst[LFE] = b.citaj(4) as u8;
                s.fgaincod[LFE] = b.citaj(3) as u8;
            }
        } else if blk == 0 {
            return Err(chyba("snroffste = 0 v bloku 0")); // bod 13
        }
        if s.cplinu {
            if b.priznak() {
                s.cplfleak = b.citaj(3) as u8;
                s.cplsleak = b.citaj(3) as u8;
            } else if !cplinu_predtym {
                return Err(chyba("cplleake = 0 pri zapnutí väzby")); // bod 14
            }
        }
        // delta rozdelenia bitov (§5.4.3.47–57): kód 0 opakovať, 1 nové, 2 žiadna, 3 vyhradené
        if b.priznak() {
            let mut kody = [2u8; 6];
            if s.cplinu {
                kody[VAZBOVY] = b.citaj(2) as u8;
            }
            for ch in 0..nfchans {
                kody[ch] = b.citaj(2) as u8;
            }
            for i in [VAZBOVY, 0, 1, 2, 3, 4] {
                if (i == VAZBOVY && !s.cplinu) || (i < 5 && i >= nfchans) {
                    continue;
                }
                match kody[i] {
                    0 => {}
                    1 => {
                        let nseg = b.citaj(3) as usize + 1;
                        let mut d = Delta::default();
                        for _ in 0..nseg {
                            let offst = b.citaj(5) as u8;
                            let len = b.citaj(4) as u8;
                            let ba = b.citaj(3) as u8;
                            d.segmenty.push((offst, len, ba));
                        }
                        s.delta[i] = Some(d);
                    }
                    2 => s.delta[i] = None,
                    _ => return Err(chyba("vyhradený kód deltbae")),
                }
            }
        } else if blk == 0 {
            s.delta = Default::default();
        }
        // preskočenie (§5.4.3.58–60)
        if b.priznak() {
            let skipl = b.citaj(9) as usize;
            b.preskoc(skipl * 8);
            self.statistika.preskok += 1;
        }
        // rozdelenie bitov (§7.2.2) a mantisy (§7.3)
        let mut bap = [[0u8; 256]; 7];
        let nulove = s.rozdelenie.csnroffst == 0
            && (0..nfchans).all(|ch| s.fsnroffst[ch] == 0)
            && (!s.cplinu || s.fsnroffst[VAZBOVY] == 0)
            && (!h.lfeon || s.fsnroffst[LFE] == 0);
        if nulove {
            self.statistika.nulove_rozdelenie += 1;
        } else {
            for ch in 0..nfchans {
                rozdel_bity(s, h, ch, 0, s.endmant[ch], &mut bap[ch])?;
            }
            if s.cplinu {
                rozdel_bity(s, h, VAZBOVY, s.cplstrtmant(), s.cplendmant(), &mut bap[VAZBOVY])?;
            }
            if h.lfeon {
                rozdel_bity(s, h, LFE, 0, 7, &mut bap[LFE])?;
            }
        }
        for i in 0..6 {
            self.statistika.delta += s.delta[i].is_some() as u64;
        }
        // mantisy v poradí toku: kanály, väzbový kanál po prvom kanáli vo väzbe, LFE
        let mut mantisy = [[0f32; 256]; 7]; // hodnota mantisy (bez exponentu); NaN = bap 0 (dither)
        let mut skupiny = Skupiny::default();
        let mut vazbovy_precitany = false;
        for ch in 0..nfchans {
            let end = if s.chincpl[ch] { s.cplstrtmant() } else { s.endmant[ch] };
            citaj_mantisy(b, &bap[ch], 0, end, &mut skupiny, &mut mantisy[ch])?;
            if s.cplinu && s.chincpl[ch] && !vazbovy_precitany {
                let (zac, kon) = (s.cplstrtmant(), s.cplendmant());
                citaj_mantisy(b, &bap[VAZBOVY], zac, kon, &mut skupiny, &mut mantisy[VAZBOVY])?;
                vazbovy_precitany = true;
            }
        }
        if h.lfeon {
            citaj_mantisy(b, &bap[LFE], 0, 7, &mut skupiny, &mut mantisy[LFE])?;
        }
        // koeficienty: mantisa · 2^−exp, väzba, dither
        let mut koef = vec![[0f32; 256]; h.kanalov()];
        for ch in 0..nfchans {
            let end = if s.chincpl[ch] { s.cplstrtmant() } else { s.endmant[ch] };
            for k in 0..end {
                koef[ch][k] = self.hodnota(mantisy[ch][k], s.dithflag[ch]) * mocnina2(-(s.exp[ch][k] as i32));
            }
            if s.cplinu && s.chincpl[ch] {
                for k in s.cplstrtmant()..s.cplendmant() {
                    let sb = (k - 37) / 12;
                    let m = self.hodnota(mantisy[VAZBOVY][k], s.dithflag[ch]);
                    let mut v = m * mocnina2(-(s.exp[VAZBOVY][k] as i32)) * s.cplco[ch][sb] * 8.0;
                    if ch == 1 && acmod == 2 && s.phsflg[sb] {
                        v = -v;
                    }
                    koef[ch][k] = v;
                }
            }
        }
        if s.cplinu && acmod == 2 && s.phsflg.iter().any(|&f| f) {
            self.statistika.fazove_priznaky += 1;
        }
        if h.lfeon {
            for k in 0..7 {
                // LFE nemá dithflag (tabuľka 5.3) – bap 0 je nula
                koef[nfchans][k] = self.hodnota(mantisy[LFE][k], false) * mocnina2(-(s.exp[LFE][k] as i32));
            }
        }
        // rematrix (§7.5.4) – len pod hranicou väzby, najviac po užšiu šírku kanálov
        if acmod == 2 {
            let hranice = [13usize, 25, 37, 61, 253];
            let horna = if s.cplinu { s.cplstrtmant() } else { s.endmant[0].min(s.endmant[1]) };
            for r in 0..rematrix_pasiem(s) {
                if !s.rematflg[r] {
                    continue;
                }
                self.statistika.rematrix += 1;
                for k in hranice[r]..hranice[r + 1].min(horna) {
                    let (l, p) = (koef[0][k], koef[1][k]);
                    koef[0][k] = l + p;
                    koef[1][k] = l - p;
                }
            }
        }
        // dynamický rozsah (§7.7.1): Ch2 v režime 1+1 má dynrng2, inak všetky kanály (aj LFE) dynrng
        for ch in 0..h.kanalov() {
            let kod = s.dynrng[if acmod == 0 && ch == 1 { 1 } else { 0 }];
            if kod != 0 {
                let zisk = dynrng_zisk(kod);
                for v in koef[ch].iter_mut() {
                    *v *= zisk;
                }
            }
        }
        Ok(koef)
    }

    /// Hodnota mantisy; NaN označuje bap = 0 – dither pri dithflag (§7.3.4), inak nula.
    fn hodnota(&mut self, m: f32, dithflag: bool) -> f32 {
        if !m.is_nan() {
            m
        } else if dithflag && !self.bez_ditheru {
            self.statistika.dither += 1;
            self.dither()
        } else {
            0.0
        }
    }
}

/// 2^e presne (|e| ≤ 60).
fn mocnina2(e: i32) -> f32 {
    f32::from_bits(((127 + e.clamp(-126, 127)) as u32) << 23)
}

/// Zisk dynrng (§7.7.1.2, tabuľka 7.29): X (3 bity so znamienkom) → 2^(X+1), Y → 0.1Y₃Y₄Y₅Y₆Y₇ (base 2).
fn dynrng_zisk(kod: u8) -> f32 {
    let x = ((kod as i8) >> 5) as i32;
    let y = (kod & 0x1F) as f32;
    mocnina2(x + 1) * (32.0 + y) / 64.0
}

/// Počet pásiem rematrixu (tabuľka 5.15).
fn rematrix_pasiem(s: &StavRamca) -> usize {
    if !s.cplinu || s.cplbegf > 2 {
        4
    } else if s.cplbegf > 0 {
        3
    } else {
        2
    }
}

/// Dekódovanie skupín exponentov (§7.1.3, pseudokód): `exp[0]` = absolútny exponent, ďalej každý absolútny
/// exponent skopírovaný do `skupina` polôh (D15 1, D25 2, D45 4).
fn dekoduj_exponenty(
    b: &mut Bity,
    absexp: i32,
    ngrps: usize,
    skupina: usize,
    exp: &mut [u8; 256],
) -> Vysledok<()> {
    exp[0] = absexp as u8;
    let mut predosly = absexp;
    let mut poloha = 1;
    for _ in 0..ngrps {
        let gexp = b.citaj(7) as i32;
        if gexp > 124 {
            return Err(chyba("skupina exponentov > 124")); // §7.10.2 bod 17
        }
        for m in [gexp / 25, (gexp % 25) / 5, (gexp % 25) % 5] {
            predosly += m - 2; // dexp = M − 2
            if !(0..=24).contains(&predosly) {
                return Err(chyba("exponent mimo 0–24"));
            }
            for _ in 0..skupina {
                if poloha < 256 {
                    exp[poloha] = predosly as u8;
                }
                poloha += 1;
            }
        }
    }
    Ok(())
}

/// Skupiny mantís (§7.3.5) – zdieľané v rámci bloku naprieč sadami exponentov.
#[derive(Default)]
struct Skupiny {
    b1: Vec<f32>,
    b2: Vec<f32>,
    b4: Vec<f32>,
}

/// Mantisy bap[zac..kon] (§7.3): NaN pre bap = 0.
fn citaj_mantisy(
    b: &mut Bity,
    bap: &[u8; 256],
    zac: usize,
    kon: usize,
    skup: &mut Skupiny,
    von: &mut [f32; 256],
) -> Vysledok<()> {
    for k in zac..kon {
        von[k] = match bap[k] {
            0 => f32::NAN,
            1 => {
                if skup.b1.is_empty() {
                    let kod = b.citaj(5) as usize;
                    if kod > 26 {
                        return Err(chyba("skupina 3 úrovní > 26")); // bod 18
                    }
                    skup.b1 = vec![KVANT_3[kod % 3], KVANT_3[(kod % 9) / 3], KVANT_3[kod / 9]];
                }
                skup.b1.pop().expect("skupina")
            }
            2 => {
                if skup.b2.is_empty() {
                    let kod = b.citaj(7) as usize;
                    if kod > 124 {
                        return Err(chyba("skupina 5 úrovní > 124")); // bod 19
                    }
                    skup.b2 = vec![KVANT_5[kod % 5], KVANT_5[(kod % 25) / 5], KVANT_5[kod / 25]];
                }
                skup.b2.pop().expect("skupina")
            }
            3 => KVANT_7[(b.citaj(3) as usize).min(6)],
            4 => {
                if skup.b4.is_empty() {
                    let kod = b.citaj(7) as usize;
                    if kod > 120 {
                        return Err(chyba("skupina 11 úrovní > 120")); // bod 20
                    }
                    skup.b4 = vec![KVANT_11[kod % 11], KVANT_11[kod / 11]];
                }
                skup.b4.pop().expect("skupina")
            }
            5 => KVANT_15[(b.citaj(4) as usize).min(14)],
            bap => {
                // asymetrická (§7.3.2): doplnkový kód, desatinná čiarka za znamienkom
                let n = BITY_MANTISY[bap as usize] as u32;
                let m = b.citaj(n) as i32;
                let so_znamienkom = (m << (32 - n)) >> (32 - n);
                so_znamienkom as f32 * mocnina2(1 - n as i32)
            }
        };
    }
    Ok(())
}

/// Rozdelenie bitov jedného kanála (§7.2.2, celočíselne podľa pseudokódu) do `bap[start..end]`.
fn rozdel_bity(
    s: &StavRamca,
    h: &Hlavicka,
    i: usize,
    start: usize,
    end: usize,
    bap: &mut [u8; 256],
) -> Vysledok<()> {
    if start >= end {
        return Ok(());
    }
    let r = &s.rozdelenie;
    let sdecay = SLOWDEC[r.sdcycod as usize];
    let fdecay = FASTDEC[r.fdcycod as usize];
    let sgain = SLOWGAIN[r.sgaincod as usize];
    let dbknee = DBPBTAB[r.dbpbcod as usize];
    let floor = FLOORTAB[r.floorcod as usize];
    let fgain = FASTGAIN[s.fgaincod[i] as usize];
    let snroffset = (((r.csnroffst as i32 - 15) << 4) + s.fsnroffst[i] as i32) << 2;
    let exp = &s.exp[i];
    // §7.2.2.2
    let mut psd = [0i32; 256];
    for bin in start..end {
        psd[bin] = 3072 - ((exp[bin] as i32) << 7);
    }
    // §7.2.2.3 – integrácia po pásmach s logaritmickým sčítaním
    let mut bndpsd = [0i32; 50];
    let mut j = start;
    let mut k = MASKTAB[start] as usize;
    loop {
        let lastbin = (BNDTAB[k] as usize + BNDSZ[k] as usize).min(end);
        bndpsd[k] = psd[j];
        j += 1;
        while j < lastbin {
            bndpsd[k] = logadd(bndpsd[k], psd[j]);
            j += 1;
        }
        k += 1;
        if end <= lastbin {
            break;
        }
    }
    // §7.2.2.4 – excitácia
    let bndstrt = MASKTAB[start] as usize;
    let bndend = MASKTAB[end - 1] as usize + 1;
    let mut excite = [0i32; 50];
    let (mut fastleak, mut slowleak);
    let mut lowcomp = 0;
    let begin;
    if bndstrt == 0 {
        lowcomp = calc_lowcomp(lowcomp, bndpsd[0], bndpsd[1], 0);
        excite[0] = bndpsd[0] - fgain - lowcomp;
        lowcomp = calc_lowcomp(lowcomp, bndpsd[1], bndpsd[2], 1);
        excite[1] = bndpsd[1] - fgain - lowcomp;
        let mut zac = 7;
        fastleak = 0;
        slowleak = 0;
        for bin in 2..7 {
            let posledny_lfe = bndend == 7 && bin == 6;
            if !posledny_lfe {
                lowcomp = calc_lowcomp(lowcomp, bndpsd[bin], bndpsd[bin + 1], bin);
            }
            fastleak = bndpsd[bin] - fgain;
            slowleak = bndpsd[bin] - sgain;
            excite[bin] = fastleak - lowcomp;
            if !posledny_lfe && bndpsd[bin] <= bndpsd[bin + 1] {
                zac = bin + 1;
                break;
            }
        }
        for bin in zac..bndend.min(22) {
            if !(bndend == 7 && bin == 6) {
                lowcomp = calc_lowcomp(lowcomp, bndpsd[bin], bndpsd[bin + 1], bin);
            }
            fastleak -= fdecay;
            fastleak = fastleak.max(bndpsd[bin] - fgain);
            slowleak -= sdecay;
            slowleak = slowleak.max(bndpsd[bin] - sgain);
            excite[bin] = (fastleak - lowcomp).max(slowleak);
        }
        begin = 22;
    } else {
        // väzbový kanál
        fastleak = ((s.cplfleak as i32) << 8) + 768;
        slowleak = ((s.cplsleak as i32) << 8) + 768;
        begin = bndstrt;
    }
    for bin in begin..bndend {
        fastleak -= fdecay;
        fastleak = fastleak.max(bndpsd[bin] - fgain);
        slowleak -= sdecay;
        slowleak = slowleak.max(bndpsd[bin] - sgain);
        excite[bin] = fastleak.max(slowleak);
    }
    // §7.2.2.5 – maskovacia krivka
    let mut mask = [0i32; 50];
    for bin in bndstrt..bndend {
        if bndpsd[bin] < dbknee {
            excite[bin] += (dbknee - bndpsd[bin]) >> 2;
        }
        mask[bin] = excite[bin].max(HTH[h.fscod as usize][bin]);
    }
    // §7.2.2.6 – delta rozdelenia bitov
    if i < 6
        && let Some(d) = &s.delta[i]
    {
        let mut band = 0usize;
        for &(offst, len, ba) in &d.segmenty {
            band += offst as usize;
            let delta = if ba >= 4 { (ba as i32 - 3) << 7 } else { (ba as i32 - 4) << 7 };
            for _ in 0..len {
                if band >= 50 {
                    return Err(chyba("delta rozdelenia bitov za pásmom 49")); // body 15, 16
                }
                mask[band] += delta;
                band += 1;
            }
        }
    }
    // §7.2.2.7 – ukazovatele rozdelenia bitov
    let mut i_bin = start;
    let mut j = MASKTAB[start] as usize;
    loop {
        let lastbin = (BNDTAB[j] as usize + BNDSZ[j] as usize).min(end);
        let mut m = mask[j] - snroffset - floor;
        if m < 0 {
            m = 0;
        }
        m &= 0x1fe0;
        m += floor;
        while i_bin < lastbin {
            let adresa = ((psd[i_bin] - m) >> 5).clamp(0, 63);
            bap[i_bin] = BAPTAB[adresa as usize];
            i_bin += 1;
        }
        j += 1;
        if end <= lastbin {
            break;
        }
    }
    Ok(())
}

/// logadd (§7.2.2.3).
fn logadd(a: i32, b: i32) -> i32 {
    let c = a - b;
    let adresa = ((c.abs() >> 1) as usize).min(255);
    if c >= 0 { a + LATAB[adresa] } else { b + LATAB[adresa] }
}

/// calc_lowcomp (§7.2.2.4).
fn calc_lowcomp(mut a: i32, b0: i32, b1: i32, bin: usize) -> i32 {
    if bin < 7 {
        if b0 + 256 == b1 {
            a = 384;
        } else if b0 > b1 {
            a = (a - 64).max(0);
        }
    } else if bin < 20 {
        if b0 + 256 == b1 {
            a = 320;
        } else if b0 > b1 {
            a = (a - 64).max(0);
        }
    } else {
        a = (a - 128).max(0);
    }
    a
}

#[cfg(test)]
mod skusky;
