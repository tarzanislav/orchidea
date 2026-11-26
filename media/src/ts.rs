//! **Čítačka MPEG-TS a M2TS** podľa ISO/IEC 13818-1 (Rec. ITU-T H.222.0): transportné pakety 188 B (TS),
//! 192 B (M2TS – 4 B časovej značky pred paketom, Blu-ray a AVCHD) a 204 B (16 B Reed-Solomon za paketom, DVB).
//!
//! Postup:
//! - **PSI**: PAT (PID 0) → PMT každého programu (CRC-32 sekcie sa overí); stopy v poradí PMT, typ toku a
//!   deskriptory (registrácia, jazyk ISO 639, AC-3/E-AC-3 DVB, titulky DVB, Opus) určia kodek,
//! - **PES**: zloženie z transportných paketov (počítadlo kontinuity, indikátor nespojitosti), PTS a DTS
//!   s 33-bitovým rozbalením; PES končí ďalším začiatkom jednotky alebo dĺžkou z hlavičky,
//! - **pakety**: video = jeden PES (prístupová jednotka), kľúčovosť z obsahu (IDR, IRAP, snímka I) alebo
//!   z random_access_indicator; zvuk ADTS, MPEG audio, AC-3 a E-AC-3 sa vyreže po rámcoch (`ramce`) – rámec
//!   dostane PTS toho PES, v ktorom začína, ďalšie rámce čas dopočítaný z počtu vzoriek; Opus po prístupových
//!   jednotkách s riadiacou hlavičkou,
//! - **poškodenie**: stratená synchronizácia → hľadanie troch synchronizačných bajtov za sebou; chyba
//!   kontinuity, transport_error_indicator a nespojitosť → `prerusenie` na ďalšom pakete stopy,
//! - **posun**: delenie intervalu podľa PTS referenčnej stopy (video, inak prvá stopa) a kľúčový PES pred
//!   cieľom; **trvanie** = posledný PTS − prvý PTS (koniec súboru sa prehľadá).

use crate::cas::{Rozbalenie33, na_ns, z_ns, zakladna_90k};
use crate::kontajner::{FormatKontajnera, Kontajner, Posun};
use crate::limity::Limity;
use crate::paket::Paket;
use crate::ramce::{self, HlavickaRamca};
use crate::stopa::{DruhStopy, Kodek, ParametreZvuku, Stopa};
use crate::zdroj::{Citac, ZdrojBajtov};
use crate::zvuk::RozlozenieKanalov;
use orchidea_jadro::{Chyba, Vysledok};
use std::collections::VecDeque;

const SYNC: u8 = 0x47;
const ZIADNA: u16 = u16::MAX;
/// Koľko bajtov od začiatku sa prehľadá pri hľadaní PAT a PMT.
const SKUSKA_BAJTOV: u64 = 16 << 20;
/// Koľko bajtov od začiatku sa prečíta pri určení prvého času každej stopy a parametrov zvuku.
const SKUSKA_PAKETOV: u64 = 4 << 20;
/// Koľko bajtov od konca sa prehľadá pri určení trvania.
const KONIEC_BAJTOV: u64 = 4 << 20;
/// Najväčšia sekcia PSI (ISO/IEC 13818-1: section_length ≤ 1021 pri PAT a PMT, ≤ 4093 inak).
const NAJVIAC_SEKCIA: usize = 4096;
/// Koľko bajtov začiatku PES stačí na určenie kľúčovosti pri posune.
const KLUC_BAJTOV: usize = 64 << 10;
const MODUL: i64 = Rozbalenie33::MODUL;

/// CRC-32 sekcií PSI (polynóm 0x04C11DB7, začiatok 0xFFFFFFFF, bez zrkadlenia) – cez celú sekciu aj s CRC je 0.
fn crc32_mpeg(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c ^= (b as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
        }
    }
    c
}

/// 33-bitová časová značka PES (5 bajtov so značkovými bitmi).
fn casova_znacka(b: &[u8]) -> u64 {
    (((b[0] >> 1) & 7) as u64) << 30
        | (b[1] as u64) << 22
        | ((b[2] >> 1) as u64) << 15
        | (b[3] as u64) << 7
        | (b[4] >> 1) as u64
}

/// Hlavička PES: (PTS, DTS, začiatok údajov, koniec podľa PES_packet_length).
type HlavickaPes = (Option<u64>, Option<u64>, usize, Option<usize>);

fn hlavicka_pes(d: &[u8]) -> Option<HlavickaPes> {
    if d.len() < 6 || d[0] != 0 || d[1] != 0 || d[2] != 1 {
        return None;
    }
    let id = d[3];
    let dlzka = u16::from_be_bytes([d[4], d[5]]) as usize;
    let koniec = if dlzka == 0 { None } else { Some(6 + dlzka) };
    // toky bez voliteľnej hlavičky: mapa programu, výplň, súkromný tok 2, ECM, EMM, adresár, DSM-CC, H.222.1 E
    if matches!(id, 0xBC | 0xBE | 0xBF | 0xF0 | 0xF1 | 0xFF | 0xF2 | 0xF8) {
        return Some((None, None, 6, koniec));
    }
    if d.len() < 9 || d[6] & 0xC0 != 0x80 {
        return None;
    }
    let priznaky = d[7] >> 6;
    let zaciatok = 9 + d[8] as usize;
    if zaciatok > d.len() || koniec.is_some_and(|k| k < zaciatok) {
        return None;
    }
    let pts = (priznaky & 2 != 0 && zaciatok >= 14).then(|| casova_znacka(&d[9..14]));
    let dts = (priznaky == 3 && zaciatok >= 19).then(|| casova_znacka(&d[14..19]));
    Some((pts, dts, zaciatok, koniec))
}

/// Hľadá počiatočné kódy 00 00 01 a volá `f` s bajtmi za nimi; `f` vráti `Some`, keď je rozhodnuté.
fn za_pociatocnymi_kodmi(d: &[u8], mut f: impl FnMut(&[u8]) -> Option<bool>) -> Option<bool> {
    let mut i = 0;
    while i + 3 < d.len() {
        if d[i + 2] > 1 {
            i += 3;
        } else if d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1 {
            if let Some(v) = f(&d[i + 3..]) {
                return Some(v);
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    None
}

/// Kľúčovosť prístupovej jednotky videa podľa obsahu (`None` = nedá sa určiť).
fn klucova_jednotka(kodek: &Kodek, d: &[u8]) -> Option<bool> {
    match kodek {
        // H.264: IDR (typ 5) alebo SEI s bodom obnovy (typ správy 6) pred prvým rezom
        Kodek::H264 => {
            let mut obnova = false;
            za_pociatocnymi_kodmi(d, |n| {
                let typ = n.first()? & 0x1F;
                match typ {
                    5 => Some(true),
                    1..=4 => Some(obnova),
                    6 => {
                        let mut i = 1;
                        let mut typ_spravy = 0usize;
                        while let Some(&b) = n.get(i) {
                            typ_spravy += b as usize;
                            i += 1;
                            if b != 0xFF {
                                break;
                            }
                        }
                        obnova |= typ_spravy == 6;
                        None
                    }
                    _ => None,
                }
            })
        }
        // HEVC: obrázky IRAP (BLA, IDR, CRA – typy 16 až 23) pred prvým iným rezom
        Kodek::Hevc => za_pociatocnymi_kodmi(d, |n| match (n.first()? >> 1) & 0x3F {
            16..=23 => Some(true),
            0..=15 => Some(false),
            _ => None,
        }),
        // MPEG-1/2 video: hlavička obrázka (kód 0x00), picture_coding_type 1 = I
        Kodek::Mpeg2Video => za_pociatocnymi_kodmi(d, |n| match *n.first()? {
            0x00 => Some((n.get(2)? >> 3) & 7 == 1),
            _ => None,
        }),
        // MPEG-4 časť 2: VOP (kód 0xB6), vop_coding_type 0 = I
        Kodek::Mpeg4Cast2 => za_pociatocnymi_kodmi(d, |n| match *n.first()? {
            0xB6 => Some(n.get(1)? >> 6 == 0),
            _ => None,
        }),
        _ => None,
    }
}

/// Konfigurácia Opus (OpusHead podľa RFC 7845 §5.1) z počtu kanálov v deskriptore TS – poradie Vorbis
/// (rodina mapovania 1) so štandardným počtom tokov pre 3 až 8 kanálov.
fn opus_hlavicka(kanalov: u8) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.extend_from_slice(&[1, kanalov, 0, 0]);
    h.extend_from_slice(&48000u32.to_le_bytes());
    h.extend_from_slice(&[0, 0]);
    let mapovanie: Option<(u8, u8, &[u8])> = match kanalov {
        3 => Some((2, 1, &[0, 2, 1])),
        4 => Some((2, 2, &[0, 1, 2, 3])),
        5 => Some((3, 2, &[0, 4, 1, 2, 3])),
        6 => Some((4, 2, &[0, 4, 1, 2, 3, 5])),
        7 => Some((4, 3, &[0, 4, 1, 2, 3, 5, 6])),
        8 => Some((5, 3, &[0, 6, 1, 2, 3, 4, 5, 7])),
        _ => None,
    };
    match mapovanie {
        Some((tokov, parov, m)) => {
            h.extend_from_slice(&[1, tokov, parov]);
            h.extend_from_slice(m);
        }
        None => h.push(0),
    }
    h
}

/// Elementárny tok z PMT.
#[derive(Debug, Clone)]
struct TokPmt {
    pid: u16,
    typ: u8,
    deskriptory: Vec<u8>,
}

/// Kodek a údaje stopy z typu toku a deskriptorov.
fn stopa_z_pmt(t: &TokPmt, hdmv: bool) -> Stopa {
    let mut registracia = None;
    let mut jazyk = None;
    let mut kodek_deskriptora = None;
    let mut kanalov_opus = None;
    let mut i = 0;
    let d = &t.deskriptory;
    while i + 2 <= d.len() {
        let (tag, n) = (d[i], d[i + 1] as usize);
        let Some(obsah) = d.get(i + 2..i + 2 + n) else { break };
        match tag {
            0x05 if n >= 4 => registracia = Some([obsah[0], obsah[1], obsah[2], obsah[3]]),
            0x0A | 0x59 if n >= 3 => {
                if obsah[..3].iter().all(|c| c.is_ascii_alphabetic()) {
                    jazyk = Some(String::from_utf8_lossy(&obsah[..3]).to_ascii_lowercase());
                }
            }
            0x6A => kodek_deskriptora = Some(Kodek::Ac3),
            0x7A => kodek_deskriptora = Some(Kodek::Eac3),
            0x7B => kodek_deskriptora = Some(Kodek::Dts),
            0x7C => kodek_deskriptora = Some(Kodek::Aac),
            0x56 => kodek_deskriptora = Some(Kodek::Neznamy("teletext".into())),
            0x7F if n >= 2 && obsah[0] == 0x80 => kanalov_opus = Some(obsah[1]),
            _ => {}
        }
        if tag == 0x59 {
            kodek_deskriptora = Some(Kodek::DvbTitulky);
        }
        i += 2 + n;
    }
    let kodek = match (t.typ, registracia.as_ref().map(|r| &r[..])) {
        (0x01 | 0x02, _) => Kodek::Mpeg2Video,
        (0x03 | 0x04, _) => Kodek::MpegAudio { vrstva: 0 },
        (0x0F, _) => Kodek::Aac,
        (0x11, _) => Kodek::Neznamy("AAC LATM".into()),
        (0x10, _) => Kodek::Mpeg4Cast2,
        (0x1B, _) => Kodek::H264,
        (0x24, _) => Kodek::Hevc,
        (0xEA, _) => Kodek::Vc1,
        (0x81, _) => Kodek::Ac3,
        (0x87 | 0x84 | 0xA1, _) => Kodek::Eac3,
        (0x82 | 0x85 | 0x86 | 0xA2, _) => Kodek::Dts,
        (0x83, _) => Kodek::TrueHd,
        (0x90, _) => Kodek::Pgs,
        (0x80, _) if hdmv => Kodek::Neznamy("PCM Blu-ray".into()),
        (_, Some(b"AC-3")) => Kodek::Ac3,
        (_, Some(b"EAC3")) => Kodek::Eac3,
        (_, Some(b"Opus")) => Kodek::Opus,
        (_, Some(b"HEVC")) => Kodek::Hevc,
        (_, Some(b"AV01")) => Kodek::Av1,
        (_, Some(b"VC-1")) => Kodek::Vc1,
        (_, Some(b"DTS1" | b"DTS2" | b"DTS3")) => Kodek::Dts,
        (typ, _) => match (typ, kodek_deskriptora) {
            (0x06, Some(k)) => k,
            _ => Kodek::Neznamy(format!("typ toku 0x{typ:02X}")),
        },
    };
    let mut s = Stopa::nova(t.pid as u64, kodek, zakladna_90k());
    s.jazyk = jazyk;
    if s.kodek == Kodek::Opus {
        let k = kanalov_opus.filter(|&k| (1..=8).contains(&k)).unwrap_or(2);
        s.konfiguracia = opus_hlavicka(k);
        s.zvuk = Some(ParametreZvuku {
            frekvencia: 48000,
            kanalov: k,
            rozlozenie: RozlozenieKanalov::predvolene(k).unwrap_or(RozlozenieKanalov::STEREO),
            bitov: None,
        });
        s.predbeh_posunu_ns = 80_000_000;
    }
    s
}

/// Značka začiatku PES v toku rámcov: od bajtu `offset` platí PTS.
#[derive(Debug, Clone, Copy)]
struct Znacka {
    offset: usize,
    pts: Option<i64>,
    pozicia: u64,
}

/// Stav elementárneho toku.
#[derive(Debug, Default)]
struct StavToku {
    pes: Vec<u8>,
    zbieram: bool,
    pes_pozicia: u64,
    pes_nahodny_pristup: bool,
    pes_koniec: Option<usize>,
    cc: Option<u8>,
    prerusenie: bool,
    rozbalenie: Rozbalenie33,
    // rámcovanie zvuku
    tok: Vec<u8>,
    znacky: VecDeque<Znacka>,
    /// PTS základu a vzorky od neho (čas ďalších rámcov sa dopočíta presne z počtu vzoriek).
    zaklad: Option<(i64, u64)>,
    pozicia_ramca: u64,
}

impl StavToku {
    /// Zabudne rozpracované údaje (pamäť zásobníkov ostane) a označí prerušenie.
    fn vynuluj(&mut self, rozbalit_od: Option<i64>) {
        let mut pes = std::mem::take(&mut self.pes);
        let mut tok = std::mem::take(&mut self.tok);
        pes.clear();
        tok.clear();
        *self = StavToku { pes, tok, ..StavToku::default() };
        if let Some(v) = rozbalit_od {
            self.rozbalenie.zacni(v);
        }
        self.prerusenie = true;
    }
}

/// Čítačka MPEG-TS a M2TS.
#[derive(Debug)]
pub struct Ts<Z> {
    c: Citac<Z>,
    limity: Limity,
    velkost: u64,
    /// Posun synchronizačného bajtu v pakete (M2TS: 4).
    posun_sync: u64,
    /// Poloha prvého paketu (zarovnanie).
    zaciatok: u64,
    p: u64,
    koniec: u64,
    stopy: Vec<Stopa>,
    stav: Vec<StavToku>,
    pid_stopa: Vec<u16>,
    fronta: VecDeque<Paket>,
    pak: [u8; 204],
    prvy_pts: Option<i64>,
    trvanie_ns: Option<i64>,
    skoncene: bool,
}

impl<Z: ZdrojBajtov> Ts<Z> {
    pub fn otvor(zdroj: Z, limity: Limity) -> Vysledok<Ts<Z>> {
        let c = Citac::novy(zdroj, Citac::<Z>::PREDVOLENA_DAVKA);
        let koniec = c.dlzka();
        let mut t = Ts {
            c,
            limity,
            velkost: 188,
            posun_sync: 0,
            zaciatok: 0,
            p: 0,
            koniec,
            stopy: Vec::new(),
            stav: Vec::new(),
            pid_stopa: vec![ZIADNA; 8192],
            fronta: VecDeque::new(),
            pak: [0; 204],
            prvy_pts: None,
            trvanie_ns: None,
            skoncene: false,
        };
        t.zisti_velkost()?;
        t.nacitaj_psi()?;
        t.skusobne_citanie()?;
        t.zisti_trvanie()?;
        Ok(t)
    }

    /// Veľkosť paketu a zarovnanie: prvé miesto v prvých 64 KiB, kde sedí päť synchronizačných bajtov za sebou
    /// (pri kratšom súbore toľko, koľko sa zmestí, aspoň dva).
    fn zisti_velkost(&mut self) -> Vysledok<()> {
        let d = self.c.bajty(0, 64 << 10)?.to_vec();
        for zaciatok in 0..d.len().min(8192) {
            for (velkost, posun) in [(188usize, 0usize), (192, 4), (204, 0)] {
                let pocet = ((d.len() - zaciatok) / velkost).min(5);
                if pocet >= 2 && (0..pocet).all(|k| d.get(zaciatok + posun + k * velkost) == Some(&SYNC)) {
                    self.velkost = velkost as u64;
                    self.posun_sync = posun as u64;
                    self.zaciatok = zaciatok as u64;
                    self.p = zaciatok as u64;
                    return Ok(());
                }
            }
        }
        Err(Chyba::PoskodeneData("MPEG-TS: nenašla sa synchronizácia transportných paketov".into()))
    }

    /// Načíta ďalší transportný paket do `self.pak`; vráti jeho polohu. Pri strate synchronizácie hľadá tri
    /// synchronizačné bajty za sebou a označí prerušenie všetkých stôp.
    fn dalsi_ts(&mut self) -> Vysledok<Option<u64>> {
        let v = self.velkost as usize;
        loop {
            if self.p.saturating_add(self.velkost) > self.koniec {
                return Ok(None);
            }
            // paket platí, ak synchronizačný bajt má aj nasledujúci paket (alebo je koniec súboru) – odpad, ktorý
            // náhodou začína bajtom 0x47, nezoberie so sebou začiatok platného paketu
            let s = self.posun_sync as usize;
            let b = self.c.bajty(self.p, 2 * v)?;
            if b.len() >= v && b[s] == SYNC && b.get(v + s).is_none_or(|&x| x == SYNC) {
                self.pak[..v].copy_from_slice(&b[..v]);
                let poz = self.p;
                self.p += self.velkost;
                return Ok(Some(poz));
            }
            for s in &mut self.stav {
                s.prerusenie = true;
                s.zbieram = false;
                s.pes.clear();
                s.cc = None;
            }
            self.p = self.najdi_synchronizaciu(self.p + 1)?;
        }
    }

    fn najdi_synchronizaciu(&mut self, mut od: u64) -> Vysledok<u64> {
        let v = self.velkost as usize;
        let s = self.posun_sync as usize;
        while od < self.koniec {
            let okno = self.c.bajty(od, (256 << 10) + 3 * v)?;
            if okno.len() <= s {
                break;
            }
            let posledne = okno.len() < (256 << 10) + 3 * v;
            let mozne = if posledne { okno.len() } else { 256 << 10 };
            for i in 0..mozne {
                let sedi = |k: usize| okno.get(i + s + k * v).is_none_or(|&b| b == SYNC);
                if okno.get(i + s) == Some(&SYNC) && sedi(1) && sedi(2) {
                    return Ok(od + i as u64);
                }
            }
            if posledne {
                break;
            }
            od += mozne as u64;
        }
        Ok(self.koniec)
    }

    /// Hlavička transportného paketu v `self.pak`: (PID, začiatok jednotky, počítadlo, nespojitosť,
    /// náhodný prístup, chyba prenosu, rozsah užitočných údajov).
    fn rozober(&self) -> (u16, bool, u8, bool, bool, bool, std::ops::Range<usize>) {
        let o = self.posun_sync as usize;
        let b = &self.pak[o..o + 188];
        let chyba = b[1] & 0x80 != 0;
        let zaciatok = b[1] & 0x40 != 0;
        let pid = (((b[1] & 0x1F) as u16) << 8) | b[2] as u16;
        let riadenie = (b[3] >> 4) & 3;
        let cc = b[3] & 0x0F;
        let mut od = 4;
        let (mut nespojitost, mut nahodny) = (false, false);
        if riadenie & 2 != 0 {
            let n = b[4] as usize;
            if n > 0 {
                nespojitost = b[5] & 0x80 != 0;
                nahodny = b[5] & 0x40 != 0;
            }
            od = 5 + n;
        }
        let rozsah = if riadenie & 1 != 0 && od < 188 { o + od..o + 188 } else { 0..0 };
        (pid, zaciatok, cc, nespojitost, nahodny, chyba, rozsah)
    }

    /// Prečíta PAT a PMT od začiatku súboru (najviac `SKUSKA_BAJTOV`).
    fn nacitaj_psi(&mut self) -> Vysledok<()> {
        let mut sekcie: Vec<(u16, Vec<u8>)> = vec![(0, Vec::new())];
        let mut programy: Vec<(u16, u16)> = Vec::new(); // (číslo programu, PID PMT)
        let mut pmt: Vec<Option<(bool, Vec<TokPmt>)>> = Vec::new();
        let mut pat = false;
        while self.p < self.zaciatok + SKUSKA_BAJTOV {
            if pat && pmt.iter().all(|x| x.is_some()) {
                break;
            }
            if self.dalsi_ts()?.is_none() {
                break;
            }
            let (pid, zaciatok, _, _, _, chyba, rozsah) = self.rozober();
            if chyba || rozsah.is_empty() {
                continue;
            }
            let Some(sekcia) = sekcie.iter_mut().find(|s| s.0 == pid) else { continue };
            let data = &self.pak[rozsah];
            let buf = &mut sekcia.1;
            let mut hotove = Vec::new();
            if zaciatok {
                let ukazovatel = data[0] as usize;
                if let Some(zvysok) = data.get(1..1 + ukazovatel)
                    && !buf.is_empty()
                {
                    buf.extend_from_slice(zvysok);
                    hotove.append(&mut rozdel_sekcie(buf));
                }
                buf.clear();
                if let Some(nove) = data.get(1 + ukazovatel..) {
                    buf.extend_from_slice(nove);
                }
            } else if !buf.is_empty() {
                buf.extend_from_slice(data);
            }
            hotove.append(&mut rozdel_sekcie(buf));
            for s in hotove {
                if crc32_mpeg(&s) != 0 || s.len() < 12 || s[1] & 0x80 == 0 {
                    continue;
                }
                let telo = &s[8..s.len() - 4];
                match s[0] {
                    0x00 if pid == 0 && !pat => {
                        pat = true;
                        for z in telo.chunks_exact(4) {
                            let cislo = u16::from_be_bytes([z[0], z[1]]);
                            let pmt_pid = u16::from_be_bytes([z[2], z[3]]) & 0x1FFF;
                            if cislo != 0 && !programy.iter().any(|p| p.1 == pmt_pid) {
                                programy.push((cislo, pmt_pid));
                                pmt.push(None);
                                sekcie.push((pmt_pid, Vec::new()));
                            }
                        }
                    }
                    0x02 if pid != 0 => {
                        let cislo = u16::from_be_bytes([s[3], s[4]]);
                        let Some(k) = programy.iter().position(|p| p.1 == pid && p.0 == cislo) else {
                            continue;
                        };
                        if pmt[k].is_some() || telo.len() < 4 {
                            continue;
                        }
                        let dlzka_info = (u16::from_be_bytes([telo[2], telo[3]]) & 0x0FFF) as usize;
                        let Some(info) = telo.get(4..4 + dlzka_info) else { continue };
                        let hdmv = najdi_registraciu(info).is_some_and(|r| &r == b"HDMV" || &r == b"HDPR");
                        let mut toky = Vec::new();
                        let mut i = 4 + dlzka_info;
                        while i + 5 <= telo.len() {
                            let typ = telo[i];
                            let tok_pid = u16::from_be_bytes([telo[i + 1], telo[i + 2]]) & 0x1FFF;
                            let n = (u16::from_be_bytes([telo[i + 3], telo[i + 4]]) & 0x0FFF) as usize;
                            let deskriptory = telo.get(i + 5..i + 5 + n).unwrap_or(&[]).to_vec();
                            toky.push(TokPmt { pid: tok_pid, typ, deskriptory });
                            i += 5 + n;
                        }
                        pmt[k] = Some((hdmv, toky));
                    }
                    _ => {}
                }
            }
        }
        if !pat {
            return Err(Chyba::PoskodeneData("MPEG-TS: chýba tabuľka programov (PAT)".into()));
        }
        for (hdmv, toky) in pmt.into_iter().flatten() {
            for t in toky {
                if self.pid_stopa[t.pid as usize] != ZIADNA || t.pid < 0x10 || t.pid == 0x1FFF {
                    continue;
                }
                self.limity.over_stopy(self.stopy.len() + 1)?;
                self.pid_stopa[t.pid as usize] = self.stopy.len() as u16;
                self.stopy.push(stopa_z_pmt(&t, hdmv));
                self.stav.push(StavToku::default());
            }
        }
        if self.stopy.is_empty() {
            return Err(Chyba::PoskodeneData("MPEG-TS: žiadny elementárny tok v PMT".into()));
        }
        Ok(())
    }

    /// Prečíta začiatok súboru, kým nie sú známe parametre zvukových stôp a prvý PTS, potom sa vráti na začiatok.
    fn skusobne_citanie(&mut self) -> Vysledok<()> {
        self.vrat_na(self.zaciatok, None);
        let mut prvy: Option<i64> = None;
        let mut videne = vec![false; self.stopy.len()];
        while self.p < self.zaciatok + SKUSKA_PAKETOV {
            let hotovo = self.stopy.iter().enumerate().all(|(i, s)| {
                videne[i]
                    && (s.druh != DruhStopy::Zvuk || s.zvuk.is_some() || !ramce::ma_ramcovanie(&s.kodek))
            });
            if hotovo {
                break;
            }
            match self.dalsi_paket()? {
                Some(p) => {
                    // prvý čas súboru = najmenší čas prvého paketu každej stopy
                    if let (false, Some(t)) = (videne[p.stopa], p.dts.or(p.pts)) {
                        prvy = Some(prvy.map_or(t, |x| x.min(t)));
                    }
                    videne[p.stopa] = true;
                }
                None => break,
            }
        }
        self.prvy_pts = prvy;
        self.vrat_na(self.zaciatok, prvy);
        for s in &mut self.stav {
            s.prerusenie = false;
        }
        Ok(())
    }

    /// Trvanie od prvého po posledný PTS (posledné `KONIEC_BAJTOV` súboru).
    fn zisti_trvanie(&mut self) -> Vysledok<()> {
        let Some(prvy) = self.prvy_pts else { return Ok(()) };
        let od = self.zarovnaj(self.koniec.saturating_sub(KONIEC_BAJTOV).max(self.zaciatok));
        let mut najvac = None;
        self.prechadzaj(od, self.koniec, |_, _, pts, _| {
            najvac = Some(najvac.map_or(pts, |x: i64| x.max(pts)));
            false
        })?;
        if let Some(n) = najvac {
            self.trvanie_ns = na_ns(n.saturating_sub(prvy), zakladna_90k());
        }
        self.vrat_na(self.zaciatok, self.prvy_pts);
        for s in &mut self.stav {
            s.prerusenie = false;
        }
        Ok(())
    }

    fn zarovnaj(&self, p: u64) -> u64 {
        self.zaciatok + (p.saturating_sub(self.zaciatok) / self.velkost) * self.velkost
    }

    /// Rozbalí 33-bitovú značku k prvému PTS súboru (súbor kratší ako ≈ 24 hodín).
    fn rozbal_k_zaciatku(&self, v: u64) -> i64 {
        let p = self.prvy_pts.unwrap_or(0);
        let mut d = (v as i64 - p).rem_euclid(MODUL);
        if d > MODUL - MODUL / 16 {
            d -= MODUL;
        }
        p + d
    }

    /// Prejde transportné pakety v [od, do) a pre každý začiatok PES známej stopy zavolá
    /// `f(poloha, stopa, PTS, začiatok údajov PES)`; `f` vráti `true` = koniec prechádzania.
    fn prechadzaj(
        &mut self,
        od: u64,
        po: u64,
        mut f: impl FnMut(u64, usize, i64, &[u8]) -> bool,
    ) -> Vysledok<()> {
        let v = self.velkost as usize;
        let mut p = od;
        while p + self.velkost <= po.min(self.koniec) {
            let b = self.c.presne(p, v)?;
            if b[self.posun_sync as usize] == SYNC {
                self.pak[..v].copy_from_slice(b);
                let (pid, zaciatok, _, _, _, chyba, rozsah) = self.rozober();
                let stopa = self.pid_stopa[pid as usize];
                if zaciatok
                    && !chyba
                    && stopa != ZIADNA
                    && let Some((Some(pts), _, z, _)) = hlavicka_pes(&self.pak[rozsah.clone()])
                {
                    let pts = self.rozbal_k_zaciatku(pts);
                    let data = self.pak[rozsah].get(z..).unwrap_or(&[]).to_vec();
                    if f(p, stopa as usize, pts, &data) {
                        break;
                    }
                }
            }
            p += self.velkost;
        }
        Ok(())
    }

    fn vrat_na(&mut self, p: u64, rozbalit_od: Option<i64>) {
        self.p = p;
        self.fronta.clear();
        self.skoncene = false;
        for s in &mut self.stav {
            s.vynuluj(rozbalit_od);
        }
    }

    /// Spracuje transportný paket v `self.pak` na polohe `poz`.
    fn spracuj_ts(&mut self, poz: u64) -> Vysledok<()> {
        let (pid, zaciatok, cc, nespojitost, nahodny, chyba, rozsah) = self.rozober();
        let i = self.pid_stopa[pid as usize];
        if i == ZIADNA {
            return Ok(());
        }
        let i = i as usize;
        let s = &mut self.stav[i];
        if chyba {
            s.prerusenie = true;
            s.zbieram = false;
            s.pes.clear();
            return Ok(());
        }
        if nespojitost {
            s.prerusenie = true;
            s.rozbalenie.reset();
            s.cc = None;
        }
        if rozsah.is_empty() {
            return Ok(());
        }
        // počítadlo kontinuity: rovnaká hodnota = zopakovaný paket (zahodí sa), inak +1
        if let Some(predosle) = s.cc {
            if cc == predosle {
                return Ok(());
            }
            if cc != (predosle + 1) & 0x0F {
                s.prerusenie = true;
                if !zaciatok {
                    // údaje PES chýbajú – neúplný PES sa zahodí
                    s.zbieram = false;
                    s.pes.clear();
                }
            }
        }
        s.cc = Some(cc);
        if zaciatok {
            self.dokonci_pes(i)?;
            let s = &mut self.stav[i];
            s.zbieram = true;
            s.pes_pozicia = poz;
            s.pes_nahodny_pristup = nahodny;
            s.pes_koniec = None;
        }
        let s = &mut self.stav[i];
        if !s.zbieram {
            return Ok(());
        }
        let data = &self.pak[rozsah];
        if s.pes.len() + data.len() > self.limity.paket as usize {
            s.zbieram = false;
            s.pes.clear();
            s.prerusenie = true;
            return Ok(());
        }
        s.pes.extend_from_slice(data);
        if s.pes_koniec.is_none() && s.pes.len() >= 6 {
            let n = u16::from_be_bytes([s.pes[4], s.pes[5]]) as usize;
            if n > 0 {
                s.pes_koniec = Some(6 + n);
            }
        }
        if s.pes_koniec.is_some_and(|k| s.pes.len() >= k) {
            self.dokonci_pes(i)?;
        }
        Ok(())
    }

    /// Dokončí zbieraný PES stopy `i` a vyrobí z neho pakety.
    fn dokonci_pes(&mut self, i: usize) -> Vysledok<()> {
        let s = &mut self.stav[i];
        if !s.zbieram || s.pes.is_empty() {
            s.zbieram = false;
            return Ok(());
        }
        s.zbieram = false;
        let pes = std::mem::take(&mut s.pes);
        let Some((pts, dts, od, koniec)) = hlavicka_pes(&pes) else {
            s.prerusenie = true;
            s.pes = pes;
            s.pes.clear();
            return Ok(());
        };
        let po = koniec.unwrap_or(pes.len()).min(pes.len());
        let dts = dts.map(|d| s.rozbalenie.rozbal(d));
        let pts = pts.map(|p| s.rozbalenie.rozbal(p));
        let dts = dts.or(pts);
        let data = &pes[od.min(po)..po];
        let kodek = self.stopy[i].kodek.clone();
        let pozicia = s.pes_pozicia;
        if ramce::ma_ramcovanie(&kodek) {
            let s = &mut self.stav[i];
            if s.tok.len() + data.len() > self.limity.paket as usize {
                s.tok.clear();
                s.znacky.clear();
                s.prerusenie = true;
            }
            let offset = s.tok.len();
            s.tok.extend_from_slice(data);
            s.znacky.push_back(Znacka { offset, pts, pozicia });
            self.vyrez_ramce(i, false);
        } else if kodek == Kodek::Opus {
            self.opus_jednotky(i, data, pts, pozicia);
        } else if !data.is_empty() {
            let s = &mut self.stav[i];
            let klucovy = match self.stopy[i].druh {
                DruhStopy::Video => klucova_jednotka(&kodek, data).unwrap_or(s.pes_nahodny_pristup),
                _ => true,
            };
            let prerusenie = std::mem::take(&mut s.prerusenie);
            self.fronta.push_back(Paket {
                stopa: i,
                data: data.to_vec(),
                pts,
                dts,
                klucovy,
                prerusenie,
                pozicia,
                ..Paket::default()
            });
        }
        let s = &mut self.stav[i];
        s.pes = pes;
        s.pes.clear();
        Ok(())
    }

    /// Prístupové jednotky Opus z PES (riadiaca hlavička + paket).
    fn opus_jednotky(&mut self, i: usize, data: &[u8], pts: Option<i64>, pozicia: u64) {
        let s = &mut self.stav[i];
        let mut cas = pts;
        let mut o = 0;
        while o < data.len() {
            let Some(h) = ramce::opus_ts(&data[o..]) else {
                s.prerusenie = true;
                break;
            };
            let Some(paket) = data.get(o + h.hlavicka..o + h.hlavicka + h.dlzka) else {
                s.prerusenie = true;
                break;
            };
            // trvanie Opus je násobok 2,5 ms = 225 tikov 90 kHz
            let trvanie = crate::opus::trvanie_paketu_ns(paket).map(|ns| (ns * 9 / 100_000) as i64);
            self.fronta.push_back(Paket {
                stopa: i,
                data: paket.to_vec(),
                pts: cas,
                dts: cas,
                trvanie,
                klucovy: true,
                prerusenie: std::mem::take(&mut s.prerusenie),
                orezat_zaciatok: h.orezat_zaciatok as u32,
                orezat_koniec: h.orezat_koniec as u32,
                pozicia,
            });
            cas = match (cas, trvanie) {
                (Some(c), Some(t)) => Some(c.saturating_add(t)),
                _ => None,
            };
            o += h.hlavicka + h.dlzka;
        }
    }

    /// Vyreže úplné rámce z toku stopy `i`; `koniec` = koniec súboru (neúplný zvyšok sa zahodí).
    fn vyrez_ramce(&mut self, i: usize, koniec: bool) {
        let kodek = self.stopy[i].kodek.clone();
        loop {
            let s = &mut self.stav[i];
            if s.tok.len() < ramce::NAJMENEJ_HLAVICKY {
                break;
            }
            let Some(h) = ramce::hlavicka(&kodek, &s.tok) else {
                // nesynchronizované bajty: preskočí sa k ďalšej platnej hlavičke
                let n = s.tok.len();
                let dalsia = (1..n.saturating_sub(ramce::NAJMENEJ_HLAVICKY - 1))
                    .find(|&k| ramce::hlavicka(&kodek, &s.tok[k..]).is_some());
                let zahodit = dalsia.unwrap_or(n.saturating_sub(ramce::NAJMENEJ_HLAVICKY - 1).max(1));
                odstran(s, zahodit);
                s.prerusenie = true;
                continue;
            };
            let mut dlzka = h.dlzka;
            if dlzka > s.tok.len() {
                break;
            }
            // E-AC-3: závislé podtoky patria do paketu nezávislého rámca
            let mut cakaj = false;
            let skladat = matches!(kodek, Kodek::Ac3 | Kodek::Eac3) && !h.zavisly;
            if skladat {
                loop {
                    match s.tok.get(dlzka..).and_then(ramce::ac3) {
                        Some(z) if z.zavisly => {
                            if dlzka + z.dlzka > s.tok.len() {
                                cakaj = !koniec;
                                break;
                            }
                            dlzka += z.dlzka;
                        }
                        Some(_) => break,
                        None => {
                            cakaj = s.tok.len() - dlzka < ramce::NAJMENEJ_HLAVICKY && !koniec;
                            break;
                        }
                    }
                }
            }
            if cakaj {
                break;
            }
            self.vydaj_ramec(i, &h, dlzka);
        }
        if koniec {
            let s = &mut self.stav[i];
            s.tok.clear();
            s.znacky.clear();
        }
    }

    fn vydaj_ramec(&mut self, i: usize, h: &HlavickaRamca, dlzka: usize) {
        let stopa = &mut self.stopy[i];
        if stopa.zvuk.is_none() {
            stopa.zvuk = Some(ParametreZvuku {
                frekvencia: h.frekvencia,
                kanalov: h.kanalov,
                rozlozenie: h.rozlozenie.unwrap_or(RozlozenieKanalov(0)),
                bitov: None,
            });
            if let Kodek::MpegAudio { vrstva: 0 } = stopa.kodek {
                stopa.kodek = Kodek::MpegAudio { vrstva: h.vrstva };
            }
        }
        let s = &mut self.stav[i];
        // značky PES, ktoré začínajú najneskôr na začiatku rámca: posledná určí čas rámca
        let mut znacka = None;
        while s.znacky.front().is_some_and(|z| z.offset == 0) {
            znacka = s.znacky.pop_front();
        }
        if let Some(z) = znacka {
            s.pozicia_ramca = z.pozicia;
            if let Some(p) = z.pts {
                s.zaklad = Some((p, 0));
            }
        }
        let f = h.frekvencia.max(1) as i128;
        let tik = |(p, n): (i64, u64)| p.saturating_add(((n as i128 * 90_000 + f / 2) / f) as i64);
        let (pts, trvanie) = match s.zaklad {
            Some(z) => {
                let a = tik(z);
                let b = tik((z.0, z.1 + h.vzoriek as u64));
                s.zaklad = Some((z.0, z.1 + h.vzoriek as u64));
                (Some(a), Some(b - a))
            }
            None => (None, Some(((h.vzoriek as i128 * 90_000 + f / 2) / f) as i64)),
        };
        let data = s.tok[..dlzka].to_vec();
        let prerusenie = std::mem::take(&mut s.prerusenie);
        let pozicia = s.pozicia_ramca;
        odstran(s, dlzka);
        self.fronta.push_back(Paket {
            stopa: i,
            data,
            pts,
            dts: pts,
            trvanie,
            klucovy: true,
            prerusenie,
            pozicia,
            ..Paket::default()
        });
    }

    /// Kľúčové začiatky PES referenčnej stopy v [od, po): (poloha, PTS).
    fn kluce(&mut self, od: u64, po: u64, ref_stopa: usize) -> Vysledok<Vec<(u64, i64)>> {
        let kodek = self.stopy[ref_stopa].kodek.clone();
        let video = self.stopy[ref_stopa].druh == DruhStopy::Video;
        let pid = self.stopy[ref_stopa].cislo as u16;
        let v = self.velkost as usize;
        let mut vysledok = Vec::new();
        // (poloha, PTS, začiatok údajov, náhodný prístup)
        let mut kandidat: Option<(u64, i64, Vec<u8>, bool)> = None;
        let uzavri = |k: Option<(u64, i64, Vec<u8>, bool)>, vysledok: &mut Vec<(u64, i64)>| {
            if let Some((poz, pts, data, rai)) = k {
                let kluc = !video || klucova_jednotka(&kodek, &data).unwrap_or(rai);
                if kluc {
                    vysledok.push((poz, pts));
                }
            }
        };
        let mut p = od;
        while p + self.velkost <= po.min(self.koniec) {
            let b = self.c.presne(p, v)?;
            if b[self.posun_sync as usize] == SYNC {
                self.pak[..v].copy_from_slice(b);
                let (tpid, zaciatok, _, _, rai, chyba, rozsah) = self.rozober();
                if tpid == pid && !chyba {
                    let data = &self.pak[rozsah];
                    if zaciatok {
                        uzavri(kandidat.take(), &mut vysledok);
                        if let Some((Some(pts), _, z, _)) = hlavicka_pes(data) {
                            let pts = self.rozbal_k_zaciatku(pts);
                            kandidat = Some((p, pts, data.get(z..).unwrap_or(&[]).to_vec(), rai));
                        }
                    } else if let Some(k) = kandidat.as_mut()
                        && k.2.len() < KLUC_BAJTOV
                    {
                        k.2.extend_from_slice(data);
                    }
                }
            }
            p += self.velkost;
        }
        uzavri(kandidat.take(), &mut vysledok);
        Ok(vysledok)
    }

    /// PTS prvého začiatku PES referenčnej stopy od polohy `od` (najviac 2 MiB ďalej).
    fn pts_od(&mut self, od: u64, ref_stopa: usize) -> Vysledok<Option<i64>> {
        let mut nasiel = None;
        self.prechadzaj(od, od.saturating_add(2 << 20), |_, stopa, pts, _| {
            if stopa == ref_stopa {
                nasiel = Some(pts);
                true
            } else {
                false
            }
        })?;
        Ok(nasiel)
    }

    fn posun_na(&mut self, ciel: i64) -> Vysledok<i64> {
        // referenčná stopa: prvé video, inak prvá stopa (otvor zaručí aspoň jednu)
        let ref_stopa = (0..self.stopy.len()).find(|&i| self.stopy[i].druh == DruhStopy::Video).unwrap_or(0);
        let pocet = (self.koniec.saturating_sub(self.zaciatok)) / self.velkost;
        let krok = ((512u64 << 10) / self.velkost).max(1);
        let (mut lo, mut hi) = (0u64, pocet);
        while hi - lo > krok {
            let mid = lo + (hi - lo) / 2;
            match self.pts_od(self.zaciatok + mid * self.velkost, ref_stopa)? {
                Some(t) if t <= ciel => lo = mid,
                Some(_) => hi = mid,
                None => hi = mid,
            }
        }
        // kľúčový PES s PTS ≤ cieľ: okno pred nájdeným miestom sa zväčšuje, kým sa nenájde
        let po = (self.zaciatok + (lo + 2 * krok) * self.velkost).min(self.koniec);
        let mut okno = 1u64 << 20;
        let mut najdeny = None;
        loop {
            let od = self.zarovnaj(po.saturating_sub(okno).max(self.zaciatok));
            let kluce = self.kluce(od, po, ref_stopa)?;
            najdeny = kluce.iter().rev().find(|k| k.1 <= ciel).copied().or(najdeny);
            if najdeny.is_some() || od <= self.zaciatok {
                break;
            }
            okno = okno.saturating_mul(2);
        }
        let (poz, pts) = match najdeny {
            Some(k) => k,
            None => (self.zaciatok, self.prvy_pts.unwrap_or(0)),
        };
        self.vrat_na(poz, Some(pts));
        Ok(na_ns(pts, zakladna_90k()).unwrap_or(0))
    }
}

/// Odstráni `n` bajtov zo začiatku toku rámcov a posunie značky PES.
fn odstran(s: &mut StavToku, n: usize) {
    let n = n.min(s.tok.len());
    s.tok.drain(..n);
    // značka vnútri odstránených bajtov patrí ďalšiemu rámcu, ktorý začína v jej PES (posun na 0)
    for z in &mut s.znacky {
        z.offset = z.offset.saturating_sub(n);
    }
}

/// Registrácia (format_identifier) v slučke deskriptorov.
fn najdi_registraciu(d: &[u8]) -> Option<[u8; 4]> {
    let mut i = 0;
    while i + 2 <= d.len() {
        let n = d[i + 1] as usize;
        if d[i] == 0x05 && n >= 4 && i + 6 <= d.len() {
            return Some([d[i + 2], d[i + 3], d[i + 4], d[i + 5]]);
        }
        i += 2 + n;
    }
    None
}

/// Úplné sekcie zo začiatku zásobníka (zvyšok ostane); výplň 0xFF zásobník vyprázdni.
fn rozdel_sekcie(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut hotove = Vec::new();
    loop {
        if buf.first() == Some(&0xFF) || buf.len() > NAJVIAC_SEKCIA * 2 {
            buf.clear();
        }
        if buf.len() < 3 {
            break;
        }
        let dlzka = 3 + (u16::from_be_bytes([buf[1], buf[2]]) & 0x0FFF) as usize;
        if dlzka > NAJVIAC_SEKCIA {
            buf.clear();
            break;
        }
        if buf.len() < dlzka {
            break;
        }
        hotove.push(buf.drain(..dlzka).collect());
    }
    hotove
}

impl<Z: ZdrojBajtov> Kontajner for Ts<Z> {
    fn format(&self) -> FormatKontajnera {
        FormatKontajnera::MpegTs { velkost_paketu: self.velkost as u16 }
    }

    fn stopy(&self) -> &[Stopa] {
        &self.stopy
    }

    fn trvanie_ns(&self) -> Option<i64> {
        self.trvanie_ns
    }

    fn dalsi_paket(&mut self) -> Vysledok<Option<Paket>> {
        loop {
            if let Some(p) = self.fronta.pop_front() {
                return Ok(Some(p));
            }
            if self.skoncene {
                return Ok(None);
            }
            match self.dalsi_ts()? {
                Some(poz) => self.spracuj_ts(poz)?,
                None => {
                    for i in 0..self.stopy.len() {
                        self.dokonci_pes(i)?;
                        if ramce::ma_ramcovanie(&self.stopy[i].kodek) {
                            self.vyrez_ramce(i, true);
                        }
                    }
                    self.skoncene = true;
                }
            }
        }
    }

    fn posun(&mut self, ciel_ns: i64, sposob: Posun) -> Vysledok<i64> {
        let predbeh = match sposob {
            Posun::KlucovyPred => 0,
            Posun::Presny => self.stopy.iter().map(|s| s.predbeh_posunu_ns).max().unwrap_or(0),
        };
        let ciel = z_ns(ciel_ns.saturating_sub(predbeh), zakladna_90k()).unwrap_or(0);
        let davka = self.c.davka();
        self.c.nastav_davku(256 << 10);
        let r = self.posun_na(ciel);
        self.c.nastav_davku(davka);
        r
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn crc_sekcie() {
        // PAT s jedným programom (1 → PID 0x1000), CRC zo zapisovača FFmpeg
        let pat =
            [0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xF0, 0x00, 0x2A, 0xB1, 0x04, 0xB2];
        assert_eq!(crc32_mpeg(&pat), 0);
        assert_ne!(crc32_mpeg(&pat[..15]), 0);
    }

    #[test]
    fn pes_a_casove_znacky() {
        // PTS 0x1_2345_6789 so značkovými bitmi
        let v: u64 = 0x1_2345_6789;
        let b = [
            0x21 | (((v >> 30) & 7) << 1) as u8,
            (v >> 22) as u8,
            (((v >> 15) & 0x7F) << 1) as u8 | 1,
            (v >> 7) as u8,
            ((v & 0x7F) << 1) as u8 | 1,
        ];
        assert_eq!(casova_znacka(&b), v);
        let mut pes = vec![0, 0, 1, 0xE0, 0, 0, 0x80, 0x80, 5];
        pes.extend_from_slice(&b);
        pes.extend_from_slice(&[9, 9]);
        assert_eq!(hlavicka_pes(&pes), Some((Some(v), None, 14, None)));
        assert_eq!(hlavicka_pes(&pes[..8]), None);
        assert_eq!(hlavicka_pes(&[0, 0, 1, 0xBE, 0, 2, 0xFF, 0xFF]), Some((None, None, 6, Some(8))));
    }

    #[test]
    fn klucovost_videa() {
        assert_eq!(
            klucova_jednotka(&Kodek::H264, &[0, 0, 0, 1, 0x09, 0xF0, 0, 0, 1, 0x65, 0x88]),
            Some(true)
        );
        assert_eq!(klucova_jednotka(&Kodek::H264, &[0, 0, 1, 0x09, 0xF0, 0, 0, 1, 0x41, 0x9A]), Some(false));
        // SEI s bodom obnovy pred rezom
        assert_eq!(
            klucova_jednotka(&Kodek::H264, &[0, 0, 1, 0x06, 0x06, 0x01, 0x84, 0x80, 0, 0, 1, 0x41, 0x9A]),
            Some(true)
        );
        assert_eq!(
            klucova_jednotka(&Kodek::Hevc, &[0, 0, 1, 0x46, 0x01, 0x10, 0, 0, 1, 0x26, 0x01]),
            Some(true)
        );
        assert_eq!(klucova_jednotka(&Kodek::Hevc, &[0, 0, 1, 0x02, 0x01, 0xD0]), Some(false));
        assert_eq!(
            klucova_jednotka(&Kodek::Mpeg2Video, &[0, 0, 1, 0xB3, 1, 2, 0, 0, 1, 0x00, 0x00, 0x0F]),
            Some(true)
        );
        assert_eq!(klucova_jednotka(&Kodek::Mpeg2Video, &[0, 0, 1, 0x00, 0x00, 0x17]), Some(false));
        assert_eq!(klucova_jednotka(&Kodek::Vc1, &[0, 0, 1, 0x0F]), None);
        assert_eq!(klucova_jednotka(&Kodek::H264, &[0, 0]), None);
    }

    #[test]
    fn hlavicka_opus() {
        let h = opus_hlavicka(2);
        assert_eq!(h.len(), 19);
        assert_eq!(&h[..8], b"OpusHead");
        let h = opus_hlavicka(6);
        assert_eq!(&h[18..], &[1, 4, 2, 0, 4, 1, 2, 3, 5]);
    }
}
