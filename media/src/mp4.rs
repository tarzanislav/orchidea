//! **MP4 a MOV** – čítačka ISO BMFF (ISO/IEC 14496-12) a QuickTime (Apple QuickTime File Format), etapa L3.
//!
//! - Boxy s 32- a 64-bitovou veľkosťou, veľkosť 0 = po koniec, `uuid`; `moov` aj na konci súboru (jedno čítanie).
//! - Tabuľky vzoriek ostanú **kompaktné** ako v súbore (4 B na vzorku pri `stsz`, 8 B na kúsok pri `stco/co64`,
//!   behy pri `stts`, `ctts`, `stsc`) – poloha vzorky sa počíta kurzorom; film 3 h pri 60 snímkach/s ≈ 2,6 MB.
//! - Pakety v poradí súboru: z kurzorov stôp sa berie vzorka s najmenšou polohou – čítanie ide súvisle dopredu.
//! - Zoznam úprav (`elst`): prázdna úprava posunie čas dopredu, `media_time` posunie začiatok (snímky B, oneskorenie
//!   kodéra zvuku) – čas zobrazenia podľa ISO/IEC 14496-12, kap. 8.6.6.
//! - Fragmentované MP4: `mvex/trex`, `moof/traf` (`tfhd`, `tfdt`, `trun`), dáta vzhľadom na `moof` alebo základ.
//! - PCM v QuickTime (`twos`, `sowt`, `in24`, `in32`, `fl32`, `fl64`, `raw `, `lpcm`) a ISO `ipcm`/`fpcm` – vzorky jedného
//!   kúska sa zlúčia do paketov ≈ 20 ms (nie po jednej vzorke).

use crate::cas::{na_ns, z_ns, zakladna_z_frekvencie};
use crate::kontajner::{FormatKontajnera, Kontajner, Posun};
use crate::limity::Limity;
use crate::paket::Paket;
use crate::stopa::{
    CisloPcm, DruhStopy, FarbaVidea, Kodek, ParametreVidea, ParametreZvuku, PopisPcm, PoradieBajtov, Stopa,
};
use crate::zdroj::{Citac, ZdrojBajtov};
use crate::znacky::{self, Znacky};
use crate::zvuk::RozlozenieKanalov;
use orchidea_jadro::{Chyba, Vysledok};
use std::collections::VecDeque;

fn chyba(s: impl Into<String>) -> Chyba {
    Chyba::PoskodeneData(format!("MP4: {}", s.into()))
}

/// Box v pamäti: typ a obsah (bez hlavičky).
struct Boxy<'a> {
    d: &'a [u8],
    p: usize,
}

fn boxy(d: &[u8]) -> Boxy<'_> {
    Boxy { d, p: 0 }
}

impl<'a> Iterator for Boxy<'a> {
    type Item = Vysledok<([u8; 4], &'a [u8])>;

    fn next(&mut self) -> Option<Self::Item> {
        let zvysok = &self.d[self.p..];
        if zvysok.len() < 8 {
            // výplň na konci (niektoré zapisovače dajú 4 nulové bajty) sa ignoruje
            self.p = self.d.len();
            return None;
        }
        let v = u32::from_be_bytes([zvysok[0], zvysok[1], zvysok[2], zvysok[3]]) as u64;
        let typ = [zvysok[4], zvysok[5], zvysok[6], zvysok[7]];
        let (hlava, velkost) = match v {
            0 => (8, zvysok.len() as u64),
            1 if zvysok.len() >= 16 => (16, u64::from_be_bytes(zvysok[8..16].try_into().unwrap_or([0; 8]))),
            _ => (8, v),
        };
        if velkost < hlava as u64 || velkost > zvysok.len() as u64 {
            self.p = self.d.len();
            return Some(Err(chyba(format!("box {} presahuje nadradený box", meno_typu(typ)))));
        }
        let obsah = &zvysok[hlava..velkost as usize];
        self.p += velkost as usize;
        Some(Ok((typ, obsah)))
    }
}

fn meno_typu(t: [u8; 4]) -> String {
    t.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' }).collect()
}

/// Najprv nájdený podbox daného typu.
fn podbox<'a>(d: &'a [u8], typ: &[u8; 4]) -> Vysledok<Option<&'a [u8]>> {
    for b in boxy(d) {
        let (t, o) = b?;
        if &t == typ {
            return Ok(Some(o));
        }
    }
    Ok(None)
}

/// Čítač čísel big-endian.
struct C<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> C<'a> {
    fn bajty(&mut self, n: usize) -> Vysledok<&'a [u8]> {
        let k = self.p.checked_add(n).filter(|&k| k <= self.d.len()).ok_or_else(|| chyba("skrátený box"))?;
        let s = &self.d[self.p..k];
        self.p = k;
        Ok(s)
    }
    fn u8(&mut self) -> Vysledok<u8> {
        Ok(self.bajty(1)?[0])
    }
    fn u16(&mut self) -> Vysledok<u16> {
        let b = self.bajty(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Vysledok<u32> {
        let b = self.bajty(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Vysledok<u64> {
        let b = self.bajty(8)?;
        Ok(u64::from_be_bytes(b.try_into().unwrap_or([0; 8])))
    }
    /// Verzia a príznaky plného boxu.
    fn plny(&mut self) -> Vysledok<(u8, u32)> {
        let v = self.u32()?;
        Ok(((v >> 24) as u8, v & 0xFF_FFFF))
    }
    fn zvysok(&self) -> &'a [u8] {
        &self.d[self.p..]
    }
}

/// Veľkosti vzoriek: pevná (`stsz` s nenulovou veľkosťou) alebo pole.
#[derive(Debug, Clone, Default)]
enum Velkosti {
    #[default]
    Ziadne,
    Pevna(u32, u32),
    Pole(Vec<u32>),
}

impl Velkosti {
    fn pocet(&self) -> u32 {
        match self {
            Velkosti::Ziadne => 0,
            Velkosti::Pevna(_, n) => *n,
            Velkosti::Pole(v) => v.len() as u32,
        }
    }
    fn velkost(&self, i: u32) -> u32 {
        match self {
            Velkosti::Ziadne => 0,
            Velkosti::Pevna(v, _) => *v,
            Velkosti::Pole(v) => v.get(i as usize).copied().unwrap_or(0),
        }
    }
}

/// Kompaktné tabuľky vzoriek jednej stopy (ISO/IEC 14496-12, kap. 8.6–8.7).
#[derive(Debug, Clone, Default)]
struct Tabulky {
    /// `stts`: (počet, trvanie) a súčty pred behom (prvá vzorka, DTS) na binárne hľadanie.
    stts: Vec<(u32, u32)>,
    stts_zaciatky: Vec<(u32, u64)>,
    /// `ctts`: (počet, posun zobrazenia).
    ctts: Vec<(u32, i64)>,
    ctts_zaciatky: Vec<u32>,
    /// `stsc`: (prvý kúsok od 0, vzoriek v kúsku) a prvá vzorka behu.
    stsc: Vec<(u32, u32)>,
    stsc_vzorky: Vec<u32>,
    velkosti: Velkosti,
    kusky: Vec<u64>,
    /// `stss` (čísla od 0); `None` = všetky vzorky sú kľúčové.
    kluce: Option<Vec<u32>>,
}

impl Tabulky {
    fn pripravi(&mut self) {
        let mut s = 0u32;
        let mut t = 0u64;
        self.stts_zaciatky = self
            .stts
            .iter()
            .map(|&(n, d)| {
                let z = (s, t);
                s = s.saturating_add(n);
                t = t.saturating_add(n as u64 * d as u64);
                z
            })
            .collect();
        let mut s = 0u32;
        self.ctts_zaciatky = self
            .ctts
            .iter()
            .map(|&(n, _)| {
                let z = s;
                s = s.saturating_add(n);
                z
            })
            .collect();
        // počty vzoriek pred behmi stsc
        self.stsc_vzorky.clear();
        let mut prva = 0u32;
        for i in 0..self.stsc.len() {
            self.stsc_vzorky.push(prva);
            let (kusok, n) = self.stsc[i];
            let dalsi = self.stsc.get(i + 1).map(|r| r.0).unwrap_or(self.kusky.len() as u32);
            prva = prva.saturating_add(dalsi.saturating_sub(kusok).saturating_mul(n));
        }
    }

    fn pocet(&self) -> u32 {
        let z_casu = self
            .stts_zaciatky
            .last()
            .map(|&(s, _)| s)
            .unwrap_or(0)
            .saturating_add(self.stts.last().map(|r| r.0).unwrap_or(0));
        let z_kuskov = self.stsc_vzorky.last().copied().unwrap_or(0).saturating_add(
            self.stsc
                .last()
                .map(|&(k, n)| (self.kusky.len() as u32).saturating_sub(k).saturating_mul(n))
                .unwrap_or(0),
        );
        self.velkosti.pocet().min(z_casu).min(z_kuskov)
    }

    /// DTS a trvanie vzorky `i`.
    fn dts(&self, i: u32) -> (u64, u32) {
        let r = self.stts_zaciatky.partition_point(|&(s, _)| s <= i).saturating_sub(1);
        match (self.stts_zaciatky.get(r), self.stts.get(r)) {
            (Some(&(s, t)), Some(&(_, d))) => (t.saturating_add((i - s) as u64 * d as u64), d),
            _ => (0, 0),
        }
    }

    fn cts_posun(&self, i: u32) -> i64 {
        let r = self.ctts_zaciatky.partition_point(|&s| s <= i).saturating_sub(1);
        self.ctts.get(r).map(|c| c.1).unwrap_or(0)
    }

    fn klucova(&self, i: u32) -> bool {
        match &self.kluce {
            None => true,
            Some(k) => k.binary_search(&i).is_ok(),
        }
    }

    /// Kúsok a poradie vzorky v ňom.
    fn kusok(&self, i: u32) -> Option<(u32, u32, u32)> {
        let r = self.stsc_vzorky.partition_point(|&s| s <= i).checked_sub(1)?;
        let (prvy, n) = self.stsc[r];
        if n == 0 {
            return None;
        }
        let od = i - self.stsc_vzorky[r];
        Some((prvy.checked_add(od / n)?, od % n, n))
    }

    /// Poloha vzorky `i` v súbore.
    fn poloha(&self, i: u32) -> Option<u64> {
        let (k, v, _) = self.kusok(i)?;
        let mut p = *self.kusky.get(k as usize)?;
        for j in i - v..i {
            p = p.checked_add(self.velkost(j) as u64)?;
        }
        Some(p)
    }

    fn velkost(&self, i: u32) -> u32 {
        self.velkosti.velkost(i)
    }

    /// Posledná vzorka s DTS ≤ `t` (pri `t` pred začiatkom prvá).
    fn vzorka_pred(&self, t: u64) -> u32 {
        let r = self.stts_zaciatky.partition_point(|&(_, z)| z <= t).saturating_sub(1);
        match (self.stts_zaciatky.get(r), self.stts.get(r)) {
            (Some(&(s, z)), Some(&(n, d))) if d > 0 => s.saturating_add(
                (((t - z.min(t)) / d as u64).min(u32::MAX as u64) as u32).min(n.saturating_sub(1)),
            ),
            (Some(&(s, _)), _) => s,
            _ => 0,
        }
    }

    fn bajtov_indexu(&self) -> u64 {
        let v = match &self.velkosti {
            Velkosti::Pole(v) => v.len() as u64 * 4,
            _ => 0,
        };
        v + self.kusky.len() as u64 * 8
            + (self.stts.len() + self.ctts.len() + self.stsc.len()) as u64 * 24
            + self.kluce.as_ref().map_or(0, |k| k.len() as u64 * 4)
    }
}

/// Vzorka z fragmentu (`moof`).
#[derive(Debug, Clone, Copy)]
struct VzorkaFragmentu {
    stopa: usize,
    poloha: u64,
    velkost: u32,
    dts: u64,
    trvanie: u32,
    posun: i64,
    klucova: bool,
}

/// Predvolené hodnoty fragmentov stopy (`trex`).
#[derive(Debug, Clone, Copy, Default)]
struct Trex {
    trvanie: u32,
    velkost: u32,
    priznaky: u32,
}

/// Stav čítania jednej stopy.
#[derive(Debug, Clone, Default)]
struct StavStopy {
    id: u32,
    t: Tabulky,
    /// Ďalšia vzorka z tabuliek.
    dalsia: u32,
    pocet: u32,
    /// Posun času zobrazenia zo zoznamu úprav (tiky stopy): odčíta sa `media_time`, pripočíta prázdna úprava.
    posun_uprav: i64,
    /// PCM: bajtov na rámec a najviac rámcov v pakete.
    pcm_ramec: Option<(u32, u32)>,
    trex: Trex,
    /// DTS ďalšej vzorky fragmentu (bez `tfdt`).
    dts_fragmentu: u64,
    prerusenie: bool,
}

/// Čítačka MP4/MOV.
#[derive(Debug)]
pub struct Mp4<Z> {
    c: Citac<Z>,
    limity: Limity,
    format: FormatKontajnera,
    stopy: Vec<Stopa>,
    stav: Vec<StavStopy>,
    trvanie_ns: Option<i64>,
    fragmentovane: bool,
    /// Poloha ďalšieho boxu najvyššej úrovne pri čítaní fragmentov.
    p_fragmentov: u64,
    fronta: VecDeque<VzorkaFragmentu>,
    koniec: u64,
    znacky: Option<Znacky>,
}

impl<Z: ZdrojBajtov> Mp4<Z> {
    pub fn otvor(zdroj: Z, limity: Limity) -> Vysledok<Mp4<Z>> {
        let c = Citac::novy(zdroj, Citac::<Z>::PREDVOLENA_DAVKA);
        let koniec = c.dlzka();
        let mut m = Mp4 {
            c,
            limity,
            format: FormatKontajnera::Mp4,
            stopy: Vec::new(),
            stav: Vec::new(),
            trvanie_ns: None,
            fragmentovane: false,
            p_fragmentov: 0,
            fronta: VecDeque::new(),
            koniec,
            znacky: None,
        };
        let mut p = 0u64;
        let mut moov = None;
        let mut prvy_moof = None;
        let mut ftyp = false;
        let mut boxov = 0;
        while p + 8 <= koniec {
            boxov += 1;
            if boxov > 100_000 {
                return Err(Chyba::Limit("MP4: príliš veľa boxov najvyššej úrovne".into()));
            }
            let (typ, hlava, velkost) = m.hlavicka_boxu(p)?;
            match &typ {
                b"ftyp" => {
                    ftyp = true;
                    if m.c.bajty(p + hlava, 4)? == b"qt  " {
                        m.format = FormatKontajnera::QuickTime;
                    }
                }
                b"moov" if moov.is_none() => {
                    let n = Limity { prvok: m.limity.prvok.max(m.limity.index), ..m.limity }
                        .over_prvok(velkost - hlava, koniec - p - hlava)?;
                    moov = Some(m.c.presne(p + hlava, n)?.to_vec());
                }
                b"moof" if prvy_moof.is_none() => prvy_moof = Some(p),
                _ => {}
            }
            p = p.saturating_add(velkost);
        }
        if !ftyp {
            m.format = FormatKontajnera::QuickTime;
        }
        let moov = moov.ok_or_else(|| chyba("súbor nemá box moov"))?;
        m.citaj_moov(&moov)?;
        if m.stopy.is_empty() {
            return Err(chyba("súbor nemá stopy"));
        }
        m.p_fragmentov = prvy_moof.unwrap_or(koniec);
        Ok(m)
    }

    /// Čítač (štatistika čítaní – chladné čítanie).
    pub fn citac(&self) -> &Citac<Z> {
        &self.c
    }

    /// Typ, dĺžka hlavičky a celková veľkosť boxu na `p` (veľkosť 0 = po koniec súboru).
    fn hlavicka_boxu(&mut self, p: u64) -> Vysledok<([u8; 4], u64, u64)> {
        let h = self.c.presne(p, 8)?.to_vec();
        let typ = [h[4], h[5], h[6], h[7]];
        let v = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let (hlava, velkost) = match v {
            0 => (8, self.koniec - p),
            1 => (16, u64::from_be_bytes(self.c.presne(p + 8, 8)?.try_into().unwrap_or([0; 8]))),
            _ => (8, v),
        };
        if velkost < hlava || velkost > self.koniec - p {
            return Err(chyba(format!("box {} na {p} presahuje súbor", meno_typu(typ))));
        }
        Ok((typ, hlava, velkost))
    }

    fn citaj_moov(&mut self, moov: &[u8]) -> Vysledok<()> {
        let mut casova_mierka_filmu = 0u32;
        let mut trex = Vec::new();
        for b in boxy(moov) {
            let (t, o) = b?;
            match &t {
                b"mvhd" => {
                    let mut c = C { d: o, p: 0 };
                    let (v, _) = c.plny()?;
                    let (mierka, trvanie) = if v == 1 {
                        c.bajty(16)?;
                        (c.u32()?, c.u64()?)
                    } else {
                        c.bajty(8)?;
                        (c.u32()?, c.u32()? as u64)
                    };
                    casova_mierka_filmu = mierka;
                    if mierka > 0 && trvanie != u64::MAX && trvanie != u32::MAX as u64 {
                        self.trvanie_ns = zakladna_z_frekvencie(mierka as u64)
                            .and_then(|z| na_ns(i64::try_from(trvanie).ok()?, z));
                    }
                }
                // značky iTunes/QuickTime (`udta/meta/ilst`) – chyba v nich nevadí
                b"udta" => {
                    self.znacky = znacky::mp4_udta(o).filter(|z| !z.je_prazdne());
                }
                b"mvex" => {
                    self.fragmentovane = true;
                    for b in boxy(o) {
                        let (t, o) = b?;
                        if &t == b"trex" {
                            let mut c = C { d: o, p: 0 };
                            c.plny()?;
                            let id = c.u32()?;
                            c.u32()?;
                            trex.push((
                                id,
                                Trex { trvanie: c.u32()?, velkost: c.u32()?, priznaky: c.u32()? },
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        for b in boxy(moov) {
            let (t, o) = b?;
            if &t == b"trak" {
                self.limity.over_stopy(self.stopy.len() + 1)?;
                if let Some((stopa, stav)) = self.citaj_trak(o, casova_mierka_filmu)? {
                    self.stopy.push(stopa);
                    self.stav.push(stav);
                }
            }
        }
        let mut index = 0u64;
        for s in &mut self.stav {
            index += s.t.bajtov_indexu();
            self.limity.over_index(0, index)?;
            if let Some(&(_, t)) = trex.iter().find(|(id, _)| *id == s.id) {
                s.trex = t;
            }
        }
        Ok(())
    }

    /// Jedna stopa; `None` = stopa bez podporeného druhu (napríklad časový kód, nápoveda).
    fn citaj_trak(&self, trak: &[u8], mierka_filmu: u32) -> Vysledok<Option<(Stopa, StavStopy)>> {
        let tkhd = podbox(trak, b"tkhd")?.ok_or_else(|| chyba("stopa bez tkhd"))?;
        let mut c = C { d: tkhd, p: 0 };
        let (v, _) = c.plny()?;
        c.bajty(if v == 1 { 16 } else { 8 })?;
        let id = c.u32()?;
        let mdia = podbox(trak, b"mdia")?.ok_or_else(|| chyba("stopa bez mdia"))?;
        let mdhd = podbox(mdia, b"mdhd")?.ok_or_else(|| chyba("stopa bez mdhd"))?;
        let mut c = C { d: mdhd, p: 0 };
        let (v, _) = c.plny()?;
        c.bajty(if v == 1 { 16 } else { 8 })?;
        let mierka = c.u32()?;
        c.bajty(if v == 1 { 8 } else { 4 })?;
        let jazyk = c.u16()?;
        let zakladna = zakladna_z_frekvencie(mierka as u64).ok_or_else(|| chyba("časová mierka stopy 0"))?;
        let hdlr = podbox(mdia, b"hdlr")?.ok_or_else(|| chyba("stopa bez hdlr"))?;
        let druh = match hdlr.get(8..12) {
            Some(b"vide") => DruhStopy::Video,
            Some(b"soun") => DruhStopy::Zvuk,
            Some(b"sbtl" | b"subt" | b"text" | b"clcp") => DruhStopy::Titulky,
            _ => return Ok(None),
        };
        let minf = podbox(mdia, b"minf")?.ok_or_else(|| chyba("stopa bez minf"))?;
        let stbl = podbox(minf, b"stbl")?.ok_or_else(|| chyba("stopa bez stbl"))?;
        let stsd = podbox(stbl, b"stsd")?.ok_or_else(|| chyba("stopa bez stsd"))?;
        let mut stopa = Stopa::nova(id as u64, Kodek::Neznamy(String::new()), zakladna);
        stopa.druh = druh;
        stopa.predvolena = true;
        if jazyk != 0 && jazyk != 0x7FFF {
            let p: String = (0..3).rev().map(|k| (((jazyk >> (5 * k)) & 31) as u8 + 0x60) as char).collect();
            stopa.jazyk = Some(p);
        }
        let mut stav = StavStopy { id, prerusenie: true, ..StavStopy::default() };
        popis_vzorky(stsd, &mut stopa, &mut stav)?;
        let t = &mut stav.t;
        for b in boxy(stbl) {
            let (typ, o) = b?;
            let mut c = C { d: o, p: 0 };
            match &typ {
                b"stts" => {
                    c.plny()?;
                    let n = self.pocet_zaznamov(&mut c, 8)?;
                    t.stts = (0..n).map(|_| Ok((c.u32()?, c.u32()?))).collect::<Vysledok<_>>()?;
                }
                b"ctts" => {
                    let (v, _) = c.plny()?;
                    let n = self.pocet_zaznamov(&mut c, 8)?;
                    t.ctts = (0..n)
                        .map(|_| {
                            let k = c.u32()?;
                            let x = c.u32()?;
                            Ok((k, if v == 0 { x as i64 } else { x as i32 as i64 }))
                        })
                        .collect::<Vysledok<_>>()?;
                }
                b"stsc" => {
                    c.plny()?;
                    let n = self.pocet_zaznamov(&mut c, 12)?;
                    t.stsc = (0..n)
                        .map(|_| {
                            let k = c.u32()?;
                            let v = c.u32()?;
                            c.u32()?;
                            Ok((k.saturating_sub(1), v))
                        })
                        .collect::<Vysledok<_>>()?;
                    if t.stsc.windows(2).any(|w| w[1].0 <= w[0].0) {
                        return Err(chyba("stsc nie je vzostupné"));
                    }
                }
                b"stsz" => {
                    c.plny()?;
                    let pevna = c.u32()?;
                    let n = c.u32()?;
                    t.velkosti = if pevna != 0 {
                        Velkosti::Pevna(pevna, n)
                    } else {
                        if (n as u64) * 4 > c.zvysok().len() as u64 {
                            return Err(chyba("stsz je skrátené"));
                        }
                        Velkosti::Pole((0..n).map(|_| c.u32()).collect::<Vysledok<_>>()?)
                    };
                }
                b"stz2" => {
                    c.plny()?;
                    c.bajty(3)?;
                    let pole = c.u8()?;
                    let n = c.u32()?;
                    if !matches!(pole, 4 | 8 | 16) || (n as u64) * pole as u64 > c.zvysok().len() as u64 * 8 {
                        return Err(chyba("neplatné stz2"));
                    }
                    let mut v = Vec::with_capacity(n as usize);
                    for i in 0..n {
                        v.push(match pole {
                            4 => {
                                let b = c.zvysok()[(i / 2) as usize];
                                if i % 2 == 0 { (b >> 4) as u32 } else { (b & 15) as u32 }
                            }
                            8 => c.u8()? as u32,
                            _ => c.u16()? as u32,
                        });
                    }
                    t.velkosti = Velkosti::Pole(v);
                }
                b"stco" => {
                    c.plny()?;
                    let n = self.pocet_zaznamov(&mut c, 4)?;
                    t.kusky = (0..n).map(|_| c.u32().map(|x| x as u64)).collect::<Vysledok<_>>()?;
                }
                b"co64" => {
                    c.plny()?;
                    let n = self.pocet_zaznamov(&mut c, 8)?;
                    t.kusky = (0..n).map(|_| c.u64()).collect::<Vysledok<_>>()?;
                }
                b"stss" => {
                    c.plny()?;
                    let n = self.pocet_zaznamov(&mut c, 4)?;
                    let mut k: Vec<u32> =
                        (0..n).map(|_| c.u32().map(|x| x.saturating_sub(1))).collect::<Vysledok<_>>()?;
                    k.sort_unstable();
                    t.kluce = Some(k);
                }
                _ => {}
            }
        }
        t.pripravi();
        stav.pocet = t.pocet();
        // zoznam úprav: prázdne úpravy na začiatku posunú dopredu, prvá úprava s médiom posunie začiatok
        if let Some(elst) = podbox(trak, b"edts")?.map(|e| podbox(e, b"elst")).transpose()?.flatten() {
            let mut c = C { d: elst, p: 0 };
            let (v, _) = c.plny()?;
            let n = c.u32()?.min(1000);
            let mut prazdne = 0i64;
            for _ in 0..n {
                let (trvanie, cas) = if v == 1 {
                    (c.u64()? as i64, c.u64()? as i64)
                } else {
                    (c.u32()? as i64, c.u32()? as i32 as i64)
                };
                c.u32()?;
                if cas == -1 {
                    // prázdna úprava v mierke filmu → mierka stopy
                    if mierka_filmu > 0 {
                        prazdne = prazdne.saturating_add(
                            (trvanie as i128 * mierka as i128 / mierka_filmu as i128)
                                .clamp(0, i64::MAX as i128) as i64,
                        );
                    }
                } else {
                    stav.posun_uprav = prazdne.saturating_sub(cas);
                    break;
                }
            }
        }
        Ok(Some((stopa, stav)))
    }

    /// Počet záznamov tabuľky so záznamom `velkost` B – overený voči zvyšku boxu (žiadna alokácia podľa
    /// nepravdivého počtu).
    fn pocet_zaznamov(&self, c: &mut C, velkost: u64) -> Vysledok<u32> {
        let n = c.u32()?;
        if n as u64 * velkost > c.zvysok().len() as u64 {
            return Err(chyba("tabuľka vzoriek je skrátená"));
        }
        Ok(n)
    }

    /// Paket zo vzorky tabuliek stopy `ix` (PCM: zlúčené vzorky jedného kúska).
    fn paket_z_tabuliek(&mut self, ix: usize) -> Vysledok<Paket> {
        let s = &self.stav[ix];
        let i = s.dalsia;
        let poloha = s.t.poloha(i).ok_or_else(|| chyba("vzorka mimo tabuliek"))?;
        let (dts, trvanie) = s.t.dts(i);
        let (mut n, mut velkost, mut trvanie_spolu) = (1u32, s.t.velkost(i) as u64, trvanie as u64);
        if let Some((ramec, najviac)) = s.pcm_ramec {
            let (_, v, v_kusku) = s.t.kusok(i).ok_or_else(|| chyba("vzorka mimo tabuliek"))?;
            n = (v_kusku - v).min(najviac).min(s.pocet - i).max(1);
            velkost = n as u64 * ramec as u64;
            trvanie_spolu = n as u64 * trvanie as u64;
        }
        let klucovy = s.t.klucova(i);
        let dts = (dts.min(i64::MAX as u64) as i64).saturating_add(s.posun_uprav);
        let pts = dts.saturating_add(s.t.cts_posun(i));
        let zostava = self.koniec.saturating_sub(poloha);
        let dlzka = self.limity.over_paket(velkost, zostava)?;
        let data = self.c.presne(poloha, dlzka)?.to_vec();
        // zvuk pred začiatkom úpravy (media_time v elst – oneskorenie kodéra) sa neprehrá: orezanie začiatku
        // rámca v počte vzoriek (ISO/IEC 14496-12 §8.6.6)
        let orezat_zaciatok = match self.stopy[ix].zvuk {
            Some(z) if pts < 0 && trvanie_spolu > 0 => {
                let tikov = (-pts).min(trvanie_spolu as i64) as i128;
                let zakladna = self.stopy[ix].zakladna;
                let vzoriek =
                    tikov * zakladna.citatel() as i128 * z.frekvencia as i128 / zakladna.menovatel() as i128;
                vzoriek.clamp(0, u32::MAX as i128) as u32
            }
            _ => 0,
        };
        let s = &mut self.stav[ix];
        s.dalsia += n;
        Ok(Paket {
            stopa: ix,
            data,
            pts: Some(pts),
            dts: Some(dts),
            trvanie: Some(trvanie_spolu as i64),
            klucovy,
            prerusenie: std::mem::take(&mut s.prerusenie),
            orezat_zaciatok,
            orezat_koniec: 0,
            pozicia: poloha,
        })
    }

    /// Prečíta ďalší `moof` do fronty vzoriek; `false` = už nie sú.
    fn dalsi_fragment(&mut self) -> Vysledok<bool> {
        while self.p_fragmentov + 8 <= self.koniec {
            let p = self.p_fragmentov;
            let (typ, hlava, velkost) = self.hlavicka_boxu(p)?;
            self.p_fragmentov = p + velkost;
            if &typ != b"moof" {
                continue;
            }
            let n = self.limity.over_prvok(velkost - hlava, self.koniec - p - hlava)?;
            let moof = self.c.presne(p + hlava, n)?.to_vec();
            let mut vzorky = Vec::new();
            for b in boxy(&moof) {
                let (t, o) = b?;
                if &t == b"traf" {
                    self.citaj_traf(o, p, &mut vzorky)?;
                }
            }
            vzorky.sort_by_key(|v: &VzorkaFragmentu| v.poloha);
            self.fronta.extend(vzorky);
            return Ok(true);
        }
        Ok(false)
    }

    fn citaj_traf(&mut self, traf: &[u8], moof: u64, vzorky: &mut Vec<VzorkaFragmentu>) -> Vysledok<()> {
        let tfhd = podbox(traf, b"tfhd")?.ok_or_else(|| chyba("traf bez tfhd"))?;
        let mut c = C { d: tfhd, p: 0 };
        let (_, f) = c.plny()?;
        let id = c.u32()?;
        let Some(ix) = self.stav.iter().position(|s| s.id == id) else { return Ok(()) };
        let trex = self.stav[ix].trex;
        let zaklad = if f & 1 != 0 { c.u64()? } else { moof };
        if f & 2 != 0 {
            c.u32()?;
        }
        let trvanie_pred = if f & 8 != 0 { c.u32()? } else { trex.trvanie };
        let velkost_pred = if f & 0x10 != 0 { c.u32()? } else { trex.velkost };
        let priznaky_pred = if f & 0x20 != 0 { c.u32()? } else { trex.priznaky };
        let mut dts = match podbox(traf, b"tfdt")? {
            Some(d) => {
                let mut c = C { d, p: 0 };
                let (v, _) = c.plny()?;
                if v == 1 { c.u64()? } else { c.u32()? as u64 }
            }
            None => self.stav[ix].dts_fragmentu,
        };
        let mut poloha_dat = zaklad;
        for b in boxy(traf) {
            let (t, o) = b?;
            if &t != b"trun" {
                continue;
            }
            let mut c = C { d: o, p: 0 };
            let (v, f) = c.plny()?;
            let n = c.u32()?;
            if f & 1 != 0 {
                poloha_dat = (zaklad as i64).saturating_add(c.u32()? as i32 as i64) as u64;
            }
            let prve = if f & 4 != 0 { Some(c.u32()?) } else { None };
            let na_vzorku = 4 * ((f >> 8) & 0xF).count_ones() as u64;
            if n as u64 * na_vzorku > c.zvysok().len() as u64 {
                return Err(chyba("trun je skrátený"));
            }
            self.limity.over_index(vzorky.len() as u64 * 48, n as u64 * 48)?;
            for k in 0..n {
                let trvanie = if f & 0x100 != 0 { c.u32()? } else { trvanie_pred };
                let velkost = if f & 0x200 != 0 { c.u32()? } else { velkost_pred };
                let priz = if f & 0x400 != 0 {
                    c.u32()?
                } else if k == 0 {
                    prve.unwrap_or(priznaky_pred)
                } else {
                    priznaky_pred
                };
                let posun = if f & 0x800 != 0 {
                    let x = c.u32()?;
                    if v == 0 { x as i64 } else { x as i32 as i64 }
                } else {
                    0
                };
                vzorky.push(VzorkaFragmentu {
                    stopa: ix,
                    poloha: poloha_dat,
                    velkost,
                    dts,
                    trvanie,
                    posun,
                    klucova: priz & 0x1_0000 == 0,
                });
                poloha_dat = poloha_dat.saturating_add(velkost as u64);
                dts = dts.saturating_add(trvanie as u64);
            }
        }
        self.stav[ix].dts_fragmentu = dts;
        Ok(())
    }
}

/// Popis vzorky (prvý záznam `stsd`): kodek, konfigurácia, parametre.
fn popis_vzorky(stsd: &[u8], stopa: &mut Stopa, stav: &mut StavStopy) -> Vysledok<()> {
    let mut c = C { d: stsd, p: 0 };
    c.plny()?;
    if c.u32()? == 0 {
        return Err(chyba("prázdne stsd"));
    }
    let Some(zaznam) = boxy(c.zvysok()).next() else { return Err(chyba("prázdne stsd")) };
    let (typ, o) = zaznam?;
    match stopa.druh {
        DruhStopy::Video => popis_videa(typ, o, stopa),
        DruhStopy::Zvuk => popis_zvuku(typ, o, stopa, stav),
        _ => {
            stopa.kodek = match &typ {
                b"tx3g" | b"text" => Kodek::TextUtf8,
                b"wvtt" => Kodek::TextUtf8,
                _ => Kodek::Neznamy(meno_typu(typ)),
            };
            Ok(())
        }
    }
}

fn popis_videa(typ: [u8; 4], o: &[u8], stopa: &mut Stopa) -> Vysledok<()> {
    let mut c = C { d: o, p: 0 };
    c.bajty(24)?;
    let mut v =
        ParametreVidea { sirka: c.u16()? as u32, vyska: c.u16()? as u32, ..ParametreVidea::default() };
    c.bajty(50)?;
    let deti = c.zvysok();
    let (kodek, konfig) = match &typ {
        b"avc1" | b"avc3" => (Kodek::H264, podbox(deti, b"avcC")?),
        b"hvc1" | b"hev1" => (Kodek::Hevc, podbox(deti, b"hvcC")?),
        b"av01" => (Kodek::Av1, podbox(deti, b"av1C")?),
        b"vp09" => (Kodek::Vp9, podbox(deti, b"vpcC")?),
        b"vp08" => (Kodek::Vp8, podbox(deti, b"vpcC")?),
        b"mp4v" => (esds_kodek(deti)?.unwrap_or(Kodek::Mpeg4Cast2), esds_konfig(deti)?),
        b"mp2v" | b"m2v1" => (Kodek::Mpeg2Video, None),
        t => (Kodek::Neznamy(meno_typu(*t)), None),
    };
    stopa.kodek = kodek;
    stopa.konfiguracia = konfig.unwrap_or_default().to_vec();
    if let Some(colr) = podbox(deti, b"colr")?
        && colr.get(..4) == Some(b"nclx")
        && colr.len() >= 11
    {
        let u = |i: usize| u16::from_be_bytes([colr[i], colr[i + 1]]).min(255) as u8;
        v.farba = FarbaVidea {
            zakladne: u(4),
            prenos: u(6),
            matica: u(8),
            plny_rozsah: Some(colr[10] & 0x80 != 0),
        };
    }
    if let Some(pasp) = podbox(deti, b"pasp")?
        && pasp.len() >= 8
    {
        let h = u32::from_be_bytes([pasp[0], pasp[1], pasp[2], pasp[3]]);
        let w = u32::from_be_bytes([pasp[4], pasp[5], pasp[6], pasp[7]]);
        if h > 0 && w > 0 && h != w {
            v.zobrazena_sirka = Some((v.sirka as u64 * h as u64 / w as u64) as u32);
            v.zobrazena_vyska = Some(v.vyska);
        }
    }
    stopa.video = Some(v);
    Ok(())
}

/// Kodek z `esds` (ObjectTypeIndication, ISO/IEC 14496-1).
fn esds_kodek(deti: &[u8]) -> Vysledok<Option<Kodek>> {
    let Some(esds) = podbox(deti, b"esds")? else { return Ok(None) };
    Ok(esds_deskriptor(esds).map(|(oti, _)| kodek_z_oti(oti)))
}

/// Kodek podľa ObjectTypeIndication (register MP4RA).
fn kodek_z_oti(oti: u8) -> Kodek {
    match oti {
        0x40 | 0x66..=0x68 => Kodek::Aac,
        0x69 | 0x6B => Kodek::MpegAudio { vrstva: 3 },
        0x20 => Kodek::Mpeg4Cast2,
        0x60..=0x65 | 0x6A => Kodek::Mpeg2Video,
        0x21 => Kodek::H264,
        0xA5 => Kodek::Ac3,
        0xA6 => Kodek::Eac3,
        0xAD => Kodek::Opus,
        x => Kodek::Neznamy(format!("esds 0x{x:02X}")),
    }
}

fn esds_konfig(deti: &[u8]) -> Vysledok<Option<&[u8]>> {
    Ok(podbox(deti, b"esds")?.and_then(|e| esds_deskriptor(e).map(|(_, k)| k)))
}

/// Deskriptory ES (ISO/IEC 14496-1, 7.2.6): ObjectTypeIndication a DecoderSpecificInfo.
fn esds_deskriptor(esds: &[u8]) -> Option<(u8, &[u8])> {
    let d = esds.get(4..)?;
    // deskriptor: značka, dĺžka v 7-bitových skupinách (najviac 4 B)
    fn desk(d: &[u8]) -> Option<(u8, &[u8], &[u8])> {
        let tag = *d.first()?;
        let mut n = 0usize;
        let mut p = 1;
        for _ in 0..4 {
            let b = *d.get(p)?;
            p += 1;
            n = (n << 7) | (b & 0x7F) as usize;
            if b & 0x80 == 0 {
                break;
            }
        }
        let obsah = d.get(p..p.checked_add(n)?)?;
        Some((tag, obsah, &d[p + n..]))
    }
    let (tag, es, _) = desk(d)?;
    if tag != 3 {
        return None;
    }
    let priz = *es.get(2)?;
    let mut p = 3 + if priz & 0x80 != 0 { 2 } else { 0 };
    if priz & 0x40 != 0 {
        p += 1 + *es.get(p)? as usize;
    }
    if priz & 0x20 != 0 {
        p += 2;
    }
    let (tag, dcd, _) = desk(es.get(p..)?)?;
    if tag != 4 {
        return None;
    }
    let oti = *dcd.first()?;
    let konfig = match desk(dcd.get(13..)?) {
        Some((5, k, _)) => k,
        _ => &[],
    };
    Some((oti, konfig))
}

fn popis_zvuku(typ: [u8; 4], o: &[u8], stopa: &mut Stopa, stav: &mut StavStopy) -> Vysledok<()> {
    let mut c = C { d: o, p: 0 };
    c.bajty(8)?;
    let verzia = c.u16()?;
    c.bajty(6)?;
    let mut kanalov = c.u16()? as u32;
    let mut bitov = c.u16()? as u32;
    c.bajty(4)?;
    let mut frekvencia = c.u32()? >> 16;
    let mut ramec_v1 = None;
    match verzia {
        1 => {
            c.u32()?; // vzoriek na paket
            c.u32()?; // bajtov na paket
            ramec_v1 = Some(c.u32()?); // bajtov na rámec
            c.u32()?;
        }
        2 => {
            c.u32()?;
            let f = f64::from_bits(c.u64()?);
            frekvencia = if f.is_finite() && (1.0..1e7).contains(&f) { f.round() as u32 } else { 0 };
            kanalov = c.u32()?;
            c.u32()?;
            bitov = c.u32()?;
            let priznaky = c.u32()?;
            ramec_v1 = Some(c.u32()?);
            c.u32()?;
            // LPCM v2: príznaky (plávajúce, big-endian, so znamienkom) – ako fl32/twos/sowt
            if &typ == b"lpcm" {
                let plav = priznaky & 1 != 0;
                let velko = priznaky & 2 != 0;
                let zn = priznaky & 4 != 0;
                let b = bitov.clamp(8, 64) as u8;
                stopa.kodek = Kodek::Pcm(PopisPcm {
                    bitov: b,
                    bajtov: b.div_ceil(8),
                    poradie: if velko { PoradieBajtov::VelkoKoncove } else { PoradieBajtov::MaloKoncove },
                    cislo: if plav {
                        CisloPcm::Plavajuce
                    } else if zn || b > 8 {
                        CisloPcm::SoZnamienkom
                    } else {
                        CisloPcm::BezZnamienka
                    },
                });
            }
        }
        _ => {}
    }
    let deti = c.zvysok();
    // QuickTime: konfigurácia môže byť vo vnorenom boxe 'wave'
    let wave = podbox(deti, b"wave")?;
    let hladaj = |t: &[u8; 4]| -> Vysledok<Option<&[u8]>> {
        if let Some(x) = podbox(deti, t)? {
            return Ok(Some(x));
        }
        match wave {
            Some(w) => podbox(w, t),
            None => Ok(None),
        }
    };
    let pcm = |bitov: u32, poradie, cislo| {
        let b = bitov.clamp(1, 64) as u8;
        Kodek::Pcm(PopisPcm { bitov: b, bajtov: b.div_ceil(8), poradie, cislo })
    };
    use CisloPcm::*;
    use PoradieBajtov::*;
    let (kodek, konfig): (Kodek, Option<Vec<u8>>) = match &typ {
        b"mp4a" => {
            let d = hladaj(b"esds")?.and_then(esds_deskriptor);
            (d.map_or(Kodek::Aac, |(oti, _)| kodek_z_oti(oti)), d.map(|(_, k)| k.to_vec()))
        }
        b"Opus" => {
            frekvencia = 48_000;
            (Kodek::Opus, hladaj(b"dOps")?.map(<[u8]>::to_vec))
        }
        b"fLaC" => (Kodek::Flac, hladaj(b"dfLa")?.map(|d| d.get(4..).unwrap_or_default().to_vec())),
        b"ac-3" => (Kodek::Ac3, hladaj(b"dac3")?.map(<[u8]>::to_vec)),
        b"ec-3" => (Kodek::Eac3, hladaj(b"dec3")?.map(<[u8]>::to_vec)),
        b".mp3" => (Kodek::MpegAudio { vrstva: 3 }, None),
        b"sowt" => (pcm(bitov, MaloKoncove, SoZnamienkom), None),
        b"twos" => (pcm(bitov, VelkoKoncove, SoZnamienkom), None),
        b"raw " => (pcm(8, MaloKoncove, BezZnamienka), None),
        b"in24" | b"in32" => {
            let malo = hladaj(b"enda")?.is_some_and(|e| e.get(1) == Some(&1) || e.first() == Some(&1));
            let b = if &typ == b"in24" { 24 } else { 32 };
            (pcm(b, if malo { MaloKoncove } else { VelkoKoncove }, SoZnamienkom), None)
        }
        b"fl32" | b"fl64" => {
            let malo = hladaj(b"enda")?.is_some_and(|e| e.get(1) == Some(&1) || e.first() == Some(&1));
            let b = if &typ == b"fl32" { 32 } else { 64 };
            (pcm(b, if malo { MaloKoncove } else { VelkoKoncove }, Plavajuce), None)
        }
        b"ipcm" | b"fpcm" => {
            // pcmC (ISO/IEC 23003-5): príznaky formátu (bit 0 = malokoncové), veľkosť vzorky v bitoch
            let p = hladaj(b"pcmC")?.ok_or_else(|| chyba("ipcm bez pcmC"))?;
            let (f, b) = (*p.get(4).unwrap_or(&0), *p.get(5).unwrap_or(&16) as u32);
            let poradie = if f & 1 != 0 { MaloKoncove } else { VelkoKoncove };
            (pcm(b, poradie, if &typ == b"fpcm" { Plavajuce } else { SoZnamienkom }), None)
        }
        b"lpcm" => (stopa.kodek.clone(), None),
        t => (Kodek::Neznamy(meno_typu(*t)), None),
    };
    stopa.kodek = kodek;
    stopa.konfiguracia = konfig.unwrap_or_default();
    if let Kodek::Pcm(p) = &stopa.kodek {
        // bajtov na rámec: z popisu QuickTime v1/v2, inak bajty vzorky × kanály
        let ramec = ramec_v1.filter(|&r| r > 0).unwrap_or((p.bajtov as u32).saturating_mul(kanalov.max(1)));
        stav.pcm_ramec = Some((ramec, (frekvencia / 50).max(1)));
    }
    let kanalov = u8::try_from(kanalov).unwrap_or(0).max(1);
    stopa.zvuk = Some(ParametreZvuku {
        frekvencia,
        kanalov,
        rozlozenie: RozlozenieKanalov::predvolene(kanalov).unwrap_or_default(),
        bitov: match &stopa.kodek {
            Kodek::Pcm(p) => Some(p.bitov),
            _ => None,
        },
    });
    Ok(())
}

impl<Z: ZdrojBajtov> Kontajner for Mp4<Z> {
    fn znacky(&self) -> Option<&Znacky> {
        self.znacky.as_ref()
    }

    fn format(&self) -> FormatKontajnera {
        self.format
    }

    fn stopy(&self) -> &[Stopa] {
        &self.stopy
    }

    fn trvanie_ns(&self) -> Option<i64> {
        self.trvanie_ns
    }

    fn dalsi_paket(&mut self) -> Vysledok<Option<Paket>> {
        // 1. vzorky z tabuliek: stopa, ktorej ďalšia vzorka je v súbore najskôr
        let mut najlepsia: Option<(u64, usize)> = None;
        for (ix, s) in self.stav.iter().enumerate() {
            if s.dalsia < s.pocet
                && let Some(p) = s.t.poloha(s.dalsia)
                && najlepsia.is_none_or(|(q, _)| p < q)
            {
                najlepsia = Some((p, ix));
            }
        }
        if let Some((_, ix)) = najlepsia {
            return self.paket_z_tabuliek(ix).map(Some);
        }
        // 2. fragmenty
        if !self.fragmentovane {
            return Ok(None);
        }
        loop {
            if let Some(v) = self.fronta.pop_front() {
                let s = &mut self.stav[v.stopa];
                let dts = (v.dts.min(i64::MAX as u64) as i64).saturating_add(s.posun_uprav);
                let pts = dts.saturating_add(v.posun);
                let prerusenie = std::mem::take(&mut s.prerusenie);
                let zostava = self.koniec.saturating_sub(v.poloha);
                let n = self.limity.over_paket(v.velkost as u64, zostava)?;
                let data = self.c.presne(v.poloha, n)?.to_vec();
                return Ok(Some(Paket {
                    stopa: v.stopa,
                    data,
                    pts: Some(pts),
                    dts: Some(dts),
                    trvanie: Some(v.trvanie as i64),
                    klucovy: v.klucova,
                    prerusenie,
                    orezat_zaciatok: 0,
                    orezat_koniec: 0,
                    pozicia: v.poloha,
                }));
            }
            if !self.dalsi_fragment()? {
                return Ok(None);
            }
        }
    }

    fn posun(&mut self, ciel_ns: i64, sposob: Posun) -> Vysledok<i64> {
        let predbeh = match sposob {
            Posun::KlucovyPred => 0,
            Posun::Presny => self.stopy.iter().map(|s| s.predbeh_posunu_ns).max().unwrap_or(0),
        };
        let ciel = ciel_ns.saturating_sub(predbeh);
        let ref_ix = self.stopy.iter().position(|s| s.druh == DruhStopy::Video).unwrap_or(0);
        self.fronta.clear();
        // fragmentované bez vzoriek v tabuľkách: od začiatku fragmentov (index fragmentov – neskôr sidx/mfra)
        if self.stav.iter().all(|s| s.pocet == 0) {
            for s in &mut self.stav {
                s.prerusenie = true;
                s.dts_fragmentu = 0;
            }
            self.p_fragmentov = 0;
            while self.p_fragmentov + 8 <= self.koniec {
                let (t, _, v) = self.hlavicka_boxu(self.p_fragmentov)?;
                if &t == b"moof" {
                    break;
                }
                self.p_fragmentov += v;
            }
            return Ok(0);
        }
        // čas kľúčovej vzorky referenčnej stopy
        let s = &self.stav[ref_ix];
        let z = self.stopy[ref_ix].zakladna;
        let t = z_ns(ciel, z).unwrap_or(0).saturating_sub(s.posun_uprav).max(0) as u64;
        let mut i = s.t.vzorka_pred(t).min(s.pocet.saturating_sub(1));
        if let Some(k) = &s.t.kluce {
            let j = k.partition_point(|&x| x <= i);
            i = if j > 0 { k[j - 1] } else { k.first().copied().unwrap_or(0) };
        }
        let dts_i = (s.t.dts(i).0.min(i64::MAX as u64) as i64).saturating_add(s.posun_uprav);
        let cas_ns = na_ns(dts_i.saturating_add(s.t.cts_posun(i)), z).unwrap_or(0);
        let cas_dts_ns = na_ns(dts_i, z).unwrap_or(0);
        for (ix, s) in self.stav.iter_mut().enumerate() {
            s.prerusenie = true;
            if ix == ref_ix {
                s.dalsia = i;
                continue;
            }
            // ostatné stopy: posledná vzorka s časom ≤ čas kľúčovej vzorky (zvuk začne o chvíľu skôr, nie neskôr)
            let z = self.stopy[ix].zakladna;
            let t = z_ns(cas_dts_ns, z).unwrap_or(0).saturating_sub(s.posun_uprav).max(0) as u64;
            s.dalsia = s.t.vzorka_pred(t).min(s.pocet);
        }
        Ok(cas_ns)
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn esds_mp3_a_aac() {
        // ES_Descriptor(3) → DecoderConfigDescriptor(4, OTI 0x40) → DecoderSpecificInfo(5) [0x12, 0x10]
        let dsi = [5u8, 2, 0x12, 0x10];
        let mut dcd = vec![4u8, (13 + dsi.len()) as u8, 0x40, 0x15];
        dcd.extend([0; 11]);
        dcd.extend(dsi);
        let mut es = vec![3u8, (3 + dcd.len()) as u8, 0, 1, 0];
        es.extend(&dcd);
        let mut esds = vec![0u8; 4];
        esds.extend(&es);
        assert_eq!(esds_deskriptor(&esds), Some((0x40, &[0x12u8, 0x10][..])));
        esds[4 + 5 + 2] = 0x6B;
        assert_eq!(esds_deskriptor(&esds).unwrap().0, 0x6B);
        for n in 0..esds.len() {
            let _ = esds_deskriptor(&esds[..n]);
        }
    }

    #[test]
    fn tabulky_kompaktne() {
        // 10 vzoriek, trvanie 2 a potom 3, kúsky po 4, 4, 2 vzorkách
        let mut t = Tabulky {
            stts: vec![(6, 2), (4, 3)],
            stsc: vec![(0, 4), (2, 2)],
            velkosti: Velkosti::Pole((1..=10).collect()),
            kusky: vec![1000, 2000, 3000],
            kluce: Some(vec![0, 5]),
            ..Tabulky::default()
        };
        t.pripravi();
        assert_eq!(t.pocet(), 10);
        assert_eq!(t.dts(7), (12 + 3, 3));
        assert_eq!(t.poloha(0), Some(1000));
        assert_eq!(t.poloha(3), Some(1000 + 1 + 2 + 3));
        assert_eq!(t.poloha(5), Some(2000 + 5));
        assert_eq!(t.poloha(9), Some(3000 + 9));
        assert_eq!(t.vzorka_pred(13), 6);
        assert_eq!(t.vzorka_pred(0), 0);
        assert_eq!(t.vzorka_pred(1000), 9);
        assert!(t.klucova(5) && !t.klucova(6));
    }
}
