//! **Matroska a WebM** – čítačka kontajnera podľa RFC 9559 (Matroska) a RFC 8794 (EBML), etapa L1.
//!
//! - Pri otvorení sa prečíta hlavička EBML, `SeekHead`, `Info`, `Tracks` a `Cues` (cez `SeekHead` aj z konca
//!   súboru) – zhluky (`Cluster`) sa pri otvorení nečítajú.
//! - Pakety idú v poradí súboru: `SimpleBlock` aj `BlockGroup`, lacing Xiph, EBML aj pevný, odstránenie hlavičky
//!   (`ContentCompAlgo` 3), `DiscardPadding` (orezanie konca zvuku), zhluky s neznámou veľkosťou (živé toky).
//! - Posun podľa `Cues`; bez nich delením intervalu so synchronizáciou na ID zhluku a kontrolou jeho času.
//! - Poškodený zhluk sa preskočí – čítanie pokračuje ďalším nájdeným zhlukom a ďalšie pakety majú príznak
//!   prerušenia. Nikdy pád, alokácie len po kontrole voči [`Limity`] a zvyšku súboru.

use crate::cas::{na_ns, z_ns, zakladna_z_ns};
use crate::kontajner::{FormatKontajnera, Kontajner, Posun};
use crate::limity::Limity;
use crate::paket::Paket;
use crate::stopa::{
    CisloPcm, DruhStopy, FarbaVidea, Kodek, ParametreVidea, ParametreZvuku, PopisPcm, PoradieBajtov, Stopa,
};
use crate::zdroj::{Citac, ZdrojBajtov};
use crate::zvuk::RozlozenieKanalov;
use orchidea_jadro::{Chyba, Vysledok, Zlomok};
use std::collections::VecDeque;

/// Identifikátory prvkov EBML a Matrosky (RFC 8794, RFC 9559) – s bitmi dĺžky, ako sú v súbore.
mod id {
    pub const EBML: u32 = 0x1A45_DFA3;
    pub const DOC_TYPE: u32 = 0x4282;
    pub const DOC_TYPE_READ_VERSION: u32 = 0x4285;
    pub const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
    pub const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
    pub const SEGMENT: u32 = 0x1853_8067;
    pub const SEEK_HEAD: u32 = 0x114D_9B74;
    pub const SEEK: u32 = 0x4DBB;
    pub const SEEK_ID: u32 = 0x53AB;
    pub const SEEK_POSITION: u32 = 0x53AC;
    pub const INFO: u32 = 0x1549_A966;
    pub const TIMESTAMP_SCALE: u32 = 0x2AD7B1;
    pub const DURATION: u32 = 0x4489;
    pub const TRACKS: u32 = 0x1654_AE6B;
    pub const TRACK_ENTRY: u32 = 0xAE;
    pub const TRACK_NUMBER: u32 = 0xD7;
    pub const TRACK_TYPE: u32 = 0x83;
    pub const FLAG_DEFAULT: u32 = 0x88;
    pub const DEFAULT_DURATION: u32 = 0x23E383;
    pub const NAME: u32 = 0x536E;
    pub const LANGUAGE: u32 = 0x22B59C;
    pub const LANGUAGE_BCP47: u32 = 0x22B59D;
    pub const CODEC_ID: u32 = 0x86;
    pub const CODEC_PRIVATE: u32 = 0x63A2;
    pub const CODEC_DELAY: u32 = 0x56AA;
    pub const SEEK_PRE_ROLL: u32 = 0x56BB;
    pub const VIDEO: u32 = 0xE0;
    pub const PIXEL_WIDTH: u32 = 0xB0;
    pub const PIXEL_HEIGHT: u32 = 0xBA;
    pub const DISPLAY_WIDTH: u32 = 0x54B0;
    pub const DISPLAY_HEIGHT: u32 = 0x54BA;
    pub const COLOUR: u32 = 0x55B0;
    pub const MATRIX_COEFFICIENTS: u32 = 0x55B1;
    pub const BITS_PER_CHANNEL: u32 = 0x55B2;
    pub const RANGE: u32 = 0x55B9;
    pub const TRANSFER_CHARACTERISTICS: u32 = 0x55BA;
    pub const PRIMARIES: u32 = 0x55BB;
    pub const AUDIO: u32 = 0xE1;
    pub const SAMPLING_FREQUENCY: u32 = 0xB5;
    pub const OUTPUT_SAMPLING_FREQUENCY: u32 = 0x78B5;
    pub const CHANNELS: u32 = 0x9F;
    pub const BIT_DEPTH: u32 = 0x6264;
    pub const CONTENT_ENCODINGS: u32 = 0x6D80;
    pub const CONTENT_ENCODING: u32 = 0x6240;
    pub const CONTENT_ENCODING_TYPE: u32 = 0x5033;
    pub const CONTENT_COMPRESSION: u32 = 0x5034;
    pub const CONTENT_COMP_ALGO: u32 = 0x4254;
    pub const CONTENT_COMP_SETTINGS: u32 = 0x4255;
    pub const CLUSTER: u32 = 0x1F43_B675;
    pub const TIMESTAMP: u32 = 0xE7;
    pub const SIMPLE_BLOCK: u32 = 0xA3;
    pub const BLOCK_GROUP: u32 = 0xA0;
    pub const BLOCK: u32 = 0xA1;
    pub const BLOCK_DURATION: u32 = 0x9B;
    pub const REFERENCE_BLOCK: u32 = 0xFB;
    pub const DISCARD_PADDING: u32 = 0x75A2;
    pub const CUES: u32 = 0x1C53_BB6B;
    pub const CUE_POINT: u32 = 0xBB;
    pub const CUE_TIME: u32 = 0xB3;
    pub const CUE_TRACK_POSITIONS: u32 = 0xB7;
    pub const CUE_TRACK: u32 = 0xF7;
    pub const CUE_CLUSTER_POSITION: u32 = 0xF1;
    pub const CHAPTERS: u32 = 0x1043_A770;
    pub const TAGS: u32 = 0x1254_C367;
    pub const ATTACHMENTS: u32 = 0x1941_A469;
}

/// Prvky najvyššej úrovne segmentu – ukončia zhluk s neznámou veľkosťou.
const UROVEN_1: [u32; 8] =
    [id::SEEK_HEAD, id::INFO, id::TRACKS, id::CUES, id::CHAPTERS, id::TAGS, id::ATTACHMENTS, id::CLUSTER];

/// Najviac prvkov najvyššej úrovne prejdených pri otvorení (súbor s miliónmi prázdnych prvkov sa neotvorí naveky).
const NAJVIAC_PRVKOV_PRI_OTVORENI: usize = 100_000;
/// Koľko bajtov najviac prehľadať pri hľadaní ďalšieho zhluku (synchronizácia po poškodení, posun bez Cues).
const NAJVIAC_HLADANIA: u64 = 64 << 20;

/// Hlavička prvku EBML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hlavicka {
    id: u32,
    /// `None` = neznáma veľkosť (všetky bity hodnoty sú 1).
    velkost: Option<u64>,
    dlzka: usize,
}

/// Dĺžka čísla s premenlivou dĺžkou podľa prvého bajtu (počet úvodných núl + 1).
fn dlzka_vint(prvy: u8) -> Option<usize> {
    if prvy == 0 { None } else { Some(prvy.leading_zeros() as usize + 1) }
}

/// Hlavička prvku zo začiatku `d`. `Ok(None)` = bajtov je málo (koniec súboru).
fn hlavicka(d: &[u8]) -> Vysledok<Option<Hlavicka>> {
    let Some(&a) = d.first() else { return Ok(None) };
    let n = dlzka_vint(a)
        .filter(|&n| n <= 4)
        .ok_or_else(|| Chyba::PoskodeneData("neplatné ID prvku EBML".into()))?;
    let Some(bajty_id) = d.get(..n) else { return Ok(None) };
    let id = bajty_id.iter().fold(0u32, |x, &b| (x << 8) | b as u32);
    let Some(&b) = d.get(n) else { return Ok(None) };
    let m = dlzka_vint(b).ok_or_else(|| Chyba::PoskodeneData("neplatná veľkosť prvku EBML".into()))?;
    let Some(bajty) = d.get(n..n + m) else { return Ok(None) };
    let maska = if m == 8 { u64::MAX >> 8 } else { (1u64 << (7 * m)) - 1 };
    let hodnota = bajty.iter().fold(0u64, |x, &c| (x << 8) | c as u64) & maska;
    let velkost = if hodnota == maska { None } else { Some(hodnota) };
    Ok(Some(Hlavicka { id, velkost, dlzka: n + m }))
}

/// Číslo s premenlivou dĺžkou bez bitu dĺžky (číslo stopy v bloku, veľkosti v lacingu EBML): (hodnota, dĺžka).
fn vint(d: &[u8]) -> Vysledok<(u64, usize)> {
    let chyba = || Chyba::PoskodeneData("neplatné číslo s premenlivou dĺžkou".into());
    let n = dlzka_vint(*d.first().ok_or_else(chyba)?).ok_or_else(chyba)?;
    let b = d.get(..n).ok_or_else(chyba)?;
    let maska = if n == 8 { u64::MAX >> 8 } else { (1u64 << (7 * n)) - 1 };
    Ok((b.iter().fold(0u64, |x, &c| (x << 8) | c as u64) & maska, n))
}

/// Prvky vnútri načítaného nadradeného prvku.
struct Prvky<'a> {
    d: &'a [u8],
    p: usize,
}

fn prvky(d: &[u8]) -> Prvky<'_> {
    Prvky { d, p: 0 }
}

impl<'a> Iterator for Prvky<'a> {
    type Item = Vysledok<(u32, &'a [u8])>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.p >= self.d.len() {
            return None;
        }
        let zvysok = &self.d[self.p..];
        let h = match hlavicka(zvysok) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.p = self.d.len();
                return Some(Err(Chyba::PoskodeneData("skrátený prvok EBML".into())));
            }
            Err(e) => {
                self.p = self.d.len();
                return Some(Err(e));
            }
        };
        let Some(v) =
            h.velkost.and_then(|v| usize::try_from(v).ok()).filter(|&v| v <= zvysok.len() - h.dlzka)
        else {
            self.p = self.d.len();
            return Some(Err(Chyba::PoskodeneData(format!("prvok {:X} presahuje nadradený prvok", h.id))));
        };
        let obsah = &zvysok[h.dlzka..h.dlzka + v];
        self.p += h.dlzka + v;
        Some(Ok((h.id, obsah)))
    }
}

fn uint(d: &[u8]) -> Vysledok<u64> {
    if d.len() > 8 {
        return Err(Chyba::PoskodeneData("celé číslo EBML dlhšie ako 8 B".into()));
    }
    Ok(d.iter().fold(0u64, |x, &b| (x << 8) | b as u64))
}

fn sint(d: &[u8]) -> Vysledok<i64> {
    let u = uint(d)?;
    if d.is_empty() {
        return Ok(0);
    }
    let s = 64 - 8 * d.len() as u32;
    Ok(((u << s) as i64) >> s)
}

fn float(d: &[u8]) -> Vysledok<f64> {
    match d.len() {
        0 => Ok(0.0),
        4 => Ok(f32::from_be_bytes([d[0], d[1], d[2], d[3]]) as f64),
        8 => Ok(f64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])),
        n => Err(Chyba::PoskodeneData(format!("číslo s pohyblivou čiarkou s {n} B"))),
    }
}

fn text(d: &[u8]) -> String {
    let d = match d.iter().position(|&b| b == 0) {
        Some(i) => &d[..i],
        None => d,
    };
    String::from_utf8_lossy(d).into_owned()
}

/// Kodek podľa `CodecID` (kódy kodekov Matrosky).
fn kodek(codec_id: &str, bitov: Option<u8>) -> Kodek {
    let pcm = |poradie, cislo| {
        let b = bitov.unwrap_or(16).max(1);
        // 8-bitové PCM v Matroske je bez znamienka
        let cislo = if cislo == CisloPcm::SoZnamienkom && b <= 8 { CisloPcm::BezZnamienka } else { cislo };
        Kodek::Pcm(PopisPcm { bitov: b, bajtov: b.div_ceil(8), poradie, cislo })
    };
    match codec_id {
        "A_PCM/INT/LIT" => pcm(PoradieBajtov::MaloKoncove, CisloPcm::SoZnamienkom),
        "A_PCM/INT/BIG" => pcm(PoradieBajtov::VelkoKoncove, CisloPcm::SoZnamienkom),
        "A_PCM/FLOAT/IEEE" => pcm(PoradieBajtov::MaloKoncove, CisloPcm::Plavajuce),
        "A_FLAC" => Kodek::Flac,
        "A_OPUS" => Kodek::Opus,
        "A_MPEG/L1" => Kodek::MpegAudio { vrstva: 1 },
        "A_MPEG/L2" => Kodek::MpegAudio { vrstva: 2 },
        "A_MPEG/L3" => Kodek::MpegAudio { vrstva: 3 },
        "A_AC3" | "A_AC3/BSID9" | "A_AC3/BSID10" => Kodek::Ac3,
        "A_EAC3" => Kodek::Eac3,
        "A_TRUEHD" => Kodek::TrueHd,
        "A_VORBIS" => Kodek::Vorbis,
        "V_MPEG4/ISO/AVC" => Kodek::H264,
        "V_MPEGH/ISO/HEVC" => Kodek::Hevc,
        "V_AV1" => Kodek::Av1,
        "V_VP9" => Kodek::Vp9,
        "V_VP8" => Kodek::Vp8,
        "V_MPEG1" | "V_MPEG2" => Kodek::Mpeg2Video,
        "V_MPEG4/ISO/SP" | "V_MPEG4/ISO/ASP" | "V_MPEG4/ISO/AP" => Kodek::Mpeg4Cast2,
        "S_TEXT/UTF8" => Kodek::TextUtf8,
        "S_TEXT/ASS" | "S_TEXT/SSA" | "S_ASS" | "S_SSA" => Kodek::Ass,
        "S_HDMV/PGS" => Kodek::Pgs,
        "S_VOBSUB" => Kodek::VobSub,
        "S_DVBSUB" => Kodek::DvbTitulky,
        s if s.starts_with("A_AAC") => Kodek::Aac,
        s if s.starts_with("A_DTS") => Kodek::Dts,
        s => Kodek::Neznamy(s.to_string()),
    }
}

/// Údaje stopy potrebné pri čítaní blokov.
#[derive(Debug, Clone, Default)]
struct StavStopy {
    cislo: u64,
    /// Bajty odstránené z hlavičky každého rámca (`ContentCompAlgo` 3) – vrátia sa pred rámec.
    hlavicka: Vec<u8>,
    /// Predvolené trvanie rámca (ns).
    trvanie_ns: Option<u64>,
    /// Ďalší paket je po posune alebo poškodení.
    prerusenie: bool,
}

/// Bod posunu z `Cues`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BodPosunu {
    cas: i64,
    stopa: u64,
    pozicia: u64,
}

/// Rozpracovaný zhluk.
#[derive(Debug, Clone, Copy)]
struct Zhluk {
    cas: Option<i64>,
    koniec: Option<u64>,
}

/// Čítačka Matroska / WebM.
#[derive(Debug)]
pub struct Mkv<Z> {
    c: Citac<Z>,
    limity: Limity,
    format: FormatKontajnera,
    stopy: Vec<Stopa>,
    stav: Vec<StavStopy>,
    zakladna: Zlomok,
    ns_na_tik: u64,
    segment: u64,
    koniec: u64,
    prvy_zhluk: Option<u64>,
    body: Vec<BodPosunu>,
    trvanie_ns: Option<i64>,
    p: u64,
    zhluk: Option<Zhluk>,
    fronta: VecDeque<Paket>,
}

/// Výsledok čítania prvku zo súboru.
enum Prvok {
    Koniec,
    Hlavicka(Hlavicka),
}

impl<Z: ZdrojBajtov> Mkv<Z> {
    /// Otvorí súbor Matroska alebo WebM.
    pub fn otvor(zdroj: Z, limity: Limity) -> Vysledok<Mkv<Z>> {
        let mut m = Mkv {
            c: Citac::novy(zdroj, Citac::<Z>::PREDVOLENA_DAVKA),
            limity,
            format: FormatKontajnera::Matroska,
            stopy: Vec::new(),
            stav: Vec::new(),
            zakladna: zakladna_z_ns(1_000_000).unwrap_or(Zlomok::JEDNA),
            ns_na_tik: 1_000_000,
            segment: 0,
            koniec: 0,
            prvy_zhluk: None,
            body: Vec::new(),
            trvanie_ns: None,
            p: 0,
            zhluk: None,
            fronta: VecDeque::new(),
        };
        m.otvor_hlavicky()?;
        Ok(m)
    }

    /// Čítač (štatistika čítaní – chladné čítanie).
    pub fn citac(&self) -> &Citac<Z> {
        &self.c
    }

    fn hlavicka_na(&mut self, p: u64) -> Vysledok<Prvok> {
        let d = self.c.bajty(p, 12)?;
        Ok(match hlavicka(d)? {
            None => Prvok::Koniec,
            Some(h) => Prvok::Hlavicka(h),
        })
    }

    /// Načíta obsah prvku celý (po kontrole limitu a zvyšku súboru).
    fn obsah(&mut self, p: u64, h: Hlavicka, limit: u64) -> Vysledok<Vec<u8>> {
        let v =
            h.velkost.ok_or_else(|| Chyba::PoskodeneData(format!("prvok {:X} má neznámu veľkosť", h.id)))?;
        let zaciatok = p + h.dlzka as u64;
        let zostava = self.c.dlzka().saturating_sub(zaciatok);
        let n = Limity { prvok: limit, ..self.limity }.over_prvok(v, zostava)?;
        Ok(self.c.presne(zaciatok, n)?.to_vec())
    }

    fn otvor_hlavicky(&mut self) -> Vysledok<()> {
        let Prvok::Hlavicka(h) = self.hlavicka_na(0)? else {
            return Err(Chyba::PoskodeneData("prázdny súbor".into()));
        };
        if h.id != id::EBML {
            return Err(Chyba::PoskodeneData("súbor nezačína hlavičkou EBML".into()));
        }
        let ebml = self.obsah(0, h, 64 << 10)?;
        let mut doc_type = String::from("matroska");
        for prvok in prvky(&ebml) {
            let (i, d) = prvok?;
            match i {
                id::DOC_TYPE => doc_type = text(d),
                id::DOC_TYPE_READ_VERSION if uint(d)? > 4 => {
                    return Err(Chyba::MimoRozsahu(format!(
                        "Matroska verzie na čítanie {} nie je podporená",
                        uint(d)?
                    )));
                }
                id::EBML_MAX_ID_LENGTH if uint(d)? > 4 => {
                    return Err(Chyba::MimoRozsahu("ID dlhšie ako 4 B".into()));
                }
                id::EBML_MAX_SIZE_LENGTH if uint(d)? > 8 => {
                    return Err(Chyba::MimoRozsahu("veľkosti dlhšie ako 8 B".into()));
                }
                _ => {}
            }
        }
        self.format = match doc_type.as_str() {
            "matroska" => FormatKontajnera::Matroska,
            "webm" => FormatKontajnera::WebM,
            d => return Err(Chyba::MimoRozsahu(format!("dokument EBML typu „{d}“ nie je Matroska"))),
        };
        // segment (pred ním smú byť len prvky Void)
        let mut p = h.dlzka as u64 + h.velkost.unwrap_or(0);
        loop {
            let Prvok::Hlavicka(h) = self.hlavicka_na(p)? else {
                return Err(Chyba::PoskodeneData("súbor nemá segment".into()));
            };
            if h.id == id::SEGMENT {
                self.segment = p + h.dlzka as u64;
                let dlzka = self.c.dlzka();
                self.koniec = h.velkost.map_or(dlzka, |v| self.segment.saturating_add(v).min(dlzka));
                break;
            }
            let v = h
                .velkost
                .ok_or_else(|| Chyba::PoskodeneData("prvok pred segmentom má neznámu veľkosť".into()))?;
            p = p.saturating_add(h.dlzka as u64).saturating_add(v);
        }
        self.prechod_segmentu()
    }

    /// Prejde prvky najvyššej úrovne segmentu po prvý zhluk; chýbajúce Info, Tracks a Cues dočíta cez SeekHead.
    fn prechod_segmentu(&mut self) -> Vysledok<()> {
        let (mut info, mut tracks, mut cues) = (false, false, false);
        let mut odkazy: Vec<(u32, u64)> = Vec::new();
        let mut p = self.segment;
        let mut prejdenych = 0;
        while p < self.koniec {
            prejdenych += 1;
            if prejdenych > NAJVIAC_PRVKOV_PRI_OTVORENI {
                return Err(Chyba::Limit("príliš veľa prvkov pred stopami".into()));
            }
            let Prvok::Hlavicka(h) = self.hlavicka_na(p)? else { break };
            match h.id {
                id::CLUSTER => {
                    self.prvy_zhluk.get_or_insert(p);
                    if tracks || h.velkost.is_none() {
                        break;
                    }
                }
                id::SEEK_HEAD => {
                    let d = self.obsah(p, h, self.limity.prvok)?;
                    odkazy.extend(seek_head(&d)?);
                }
                id::INFO => {
                    let d = self.obsah(p, h, self.limity.prvok)?;
                    self.citaj_info(&d)?;
                    info = true;
                }
                id::TRACKS => {
                    let d = self.obsah(p, h, self.limity.prvok)?;
                    self.citaj_stopy(&d)?;
                    tracks = true;
                }
                id::CUES => {
                    let d = self.obsah(p, h, self.limity.prvok.max(self.limity.index))?;
                    self.citaj_cues(&d)?;
                    cues = true;
                }
                _ => {}
            }
            let Some(v) = h.velkost else { break };
            p = p.saturating_add(h.dlzka as u64).saturating_add(v);
        }
        for (i, poz) in odkazy {
            let hotovo = match i {
                id::INFO => info,
                id::TRACKS => tracks,
                id::CUES => cues,
                _ => true,
            };
            if hotovo {
                continue;
            }
            let p = self.segment.saturating_add(poz);
            let Ok(Prvok::Hlavicka(h)) = self.hlavicka_na(p) else { continue };
            if h.id != i {
                continue; // zlý odkaz – nevadí, len ho nepoužijeme
            }
            match i {
                id::INFO => {
                    let d = self.obsah(p, h, self.limity.prvok)?;
                    self.citaj_info(&d)?;
                    info = true;
                }
                id::TRACKS => {
                    let d = self.obsah(p, h, self.limity.prvok)?;
                    self.citaj_stopy(&d)?;
                    tracks = true;
                }
                _ => {
                    // Cues na konci súboru: chyba v nich nebráni prehrávaniu (posun pôjde delením intervalu)
                    if let Ok(d) = self.obsah(p, h, self.limity.prvok.max(self.limity.index)) {
                        cues = self.citaj_cues(&d).is_ok();
                    }
                }
            }
        }
        if !tracks || self.stopy.is_empty() {
            return Err(Chyba::PoskodeneData("súbor Matroska nemá stopy".into()));
        }
        self.p = self.prvy_zhluk.unwrap_or(self.koniec);
        for s in &mut self.stav {
            s.prerusenie = true;
        }
        Ok(())
    }

    fn citaj_info(&mut self, d: &[u8]) -> Vysledok<()> {
        let mut trvanie = None;
        for prvok in prvky(d) {
            let (i, d) = prvok?;
            match i {
                id::TIMESTAMP_SCALE => {
                    let s = uint(d)?;
                    self.zakladna =
                        zakladna_z_ns(s).ok_or_else(|| Chyba::PoskodeneData("TimestampScale 0".into()))?;
                    self.ns_na_tik = s;
                }
                id::DURATION => trvanie = Some(float(d)?),
                _ => {}
            }
        }
        self.trvanie_ns = trvanie
            .map(|t| t * self.ns_na_tik as f64)
            .filter(|t| t.is_finite() && *t >= 0.0 && *t < 9.0e18)
            .map(|t| t as i64);
        for s in &mut self.stopy {
            s.zakladna = self.zakladna;
        }
        Ok(())
    }

    fn citaj_stopy(&mut self, d: &[u8]) -> Vysledok<()> {
        for prvok in prvky(d) {
            let (i, d) = prvok?;
            if i != id::TRACK_ENTRY {
                continue;
            }
            self.limity.over_stopy(self.stopy.len() + 1)?;
            let (stopa, stav) = self.citaj_stopu(d)?;
            if self.stav.iter().any(|s| s.cislo == stav.cislo) {
                return Err(Chyba::PoskodeneData(format!("dve stopy s číslom {}", stav.cislo)));
            }
            self.stopy.push(stopa);
            self.stav.push(stav);
        }
        Ok(())
    }

    fn citaj_stopu(&self, d: &[u8]) -> Vysledok<(Stopa, StavStopy)> {
        let (mut cislo, mut typ, mut codec_id, mut privat) = (0u64, 0u64, String::new(), Vec::new());
        let (mut meno, mut jazyk, mut jazyk_bcp, mut predvolena) = (None, None, None, true);
        let (mut trvanie_ns, mut oneskorenie, mut predbeh) = (None, 0u64, 0u64);
        let mut video: Option<ParametreVidea> = None;
        let (mut frekvencia, mut vystupna, mut kanalov, mut bitov) = (8000.0f64, None, 1u64, None);
        let mut je_zvuk = false;
        let mut hlavicka = Vec::new();
        let mut nepodporene = None;
        for prvok in prvky(d) {
            let (i, d) = prvok?;
            match i {
                id::TRACK_NUMBER => cislo = uint(d)?,
                id::TRACK_TYPE => typ = uint(d)?,
                id::FLAG_DEFAULT => predvolena = uint(d)? != 0,
                id::DEFAULT_DURATION => trvanie_ns = Some(uint(d)?).filter(|&t| t > 0),
                id::NAME => meno = Some(text(d)),
                id::LANGUAGE => jazyk = Some(text(d)),
                id::LANGUAGE_BCP47 => jazyk_bcp = Some(text(d)),
                id::CODEC_ID => codec_id = text(d),
                id::CODEC_PRIVATE => privat = d.to_vec(),
                id::CODEC_DELAY => oneskorenie = uint(d)?,
                id::SEEK_PRE_ROLL => predbeh = uint(d)?,
                id::VIDEO => video = Some(citaj_video(d)?),
                id::AUDIO => {
                    je_zvuk = true;
                    for prvok in prvky(d) {
                        let (i, d) = prvok?;
                        match i {
                            id::SAMPLING_FREQUENCY => frekvencia = float(d)?,
                            id::OUTPUT_SAMPLING_FREQUENCY => vystupna = Some(float(d)?),
                            id::CHANNELS => kanalov = uint(d)?,
                            id::BIT_DEPTH => bitov = Some(uint(d)?),
                            _ => {}
                        }
                    }
                }
                id::CONTENT_ENCODINGS => {
                    for prvok in prvky(d) {
                        let (i, d) = prvok?;
                        if i == id::CONTENT_ENCODING {
                            match citaj_kodovanie(d)? {
                                Kodovanie::OdstranenaHlavicka(h) => hlavicka = h,
                                Kodovanie::Nepodporene(s) => nepodporene = Some(s),
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if cislo == 0 {
            return Err(Chyba::PoskodeneData("stopa bez čísla".into()));
        }
        let bitov = bitov.and_then(|b| u8::try_from(b).ok()).filter(|b| (1..=64).contains(b));
        let mut k = kodek(&codec_id, bitov);
        if let Some(s) = nepodporene {
            // stopa ostáva viditeľná, ale jej pakety dekodér nepozná
            k = Kodek::Neznamy(format!("{codec_id} ({s})"));
        }
        let mut stopa = Stopa::nova(cislo, k, self.zakladna);
        stopa.druh = match typ {
            1 => DruhStopy::Video,
            2 => DruhStopy::Zvuk,
            0x11 => DruhStopy::Titulky,
            _ => stopa.druh,
        };
        stopa.konfiguracia = privat;
        stopa.meno = meno;
        stopa.jazyk = jazyk_bcp.or(jazyk).or_else(|| Some("eng".into()));
        stopa.predvolena = predvolena;
        stopa.oneskorenie_kodeku_ns = i64::try_from(oneskorenie).unwrap_or(0);
        stopa.predbeh_posunu_ns = i64::try_from(predbeh).unwrap_or(0);
        if let Some(mut v) = video {
            v.snimkova_frekvencia = trvanie_ns.and_then(|t| crate::cas::typicka_frekvencia(t as i64));
            stopa.video = Some(v);
        }
        if je_zvuk || stopa.druh == DruhStopy::Zvuk {
            let f = if stopa.kodek == Kodek::Opus { 48_000.0 } else { vystupna.unwrap_or(frekvencia) };
            let kanalov = u8::try_from(kanalov).unwrap_or(0).max(1);
            stopa.zvuk = Some(ParametreZvuku {
                frekvencia: if (1.0..4.0e9).contains(&f) { f.round() as u32 } else { 0 },
                kanalov,
                rozlozenie: RozlozenieKanalov::predvolene(kanalov).unwrap_or_default(),
                bitov,
            });
        }
        let stav = StavStopy { cislo, hlavicka, trvanie_ns, prerusenie: true };
        Ok((stopa, stav))
    }

    fn citaj_cues(&mut self, d: &[u8]) -> Vysledok<()> {
        let mut body = Vec::new();
        for prvok in prvky(d) {
            let (i, d) = prvok?;
            if i != id::CUE_POINT {
                continue;
            }
            let mut cas = None;
            let mut pozicie = Vec::new();
            for prvok in prvky(d) {
                let (i, d) = prvok?;
                match i {
                    id::CUE_TIME => cas = Some(uint(d)?),
                    id::CUE_TRACK_POSITIONS => {
                        let (mut stopa, mut poz) = (0, None);
                        for prvok in prvky(d) {
                            let (i, d) = prvok?;
                            match i {
                                id::CUE_TRACK => stopa = uint(d)?,
                                id::CUE_CLUSTER_POSITION => poz = Some(uint(d)?),
                                _ => {}
                            }
                        }
                        if let Some(p) = poz {
                            pozicie.push((stopa, p));
                        }
                    }
                    _ => {}
                }
            }
            let Some(cas) = cas.and_then(|c| i64::try_from(c).ok()) else { continue };
            for (stopa, p) in pozicie {
                let pozicia = self.segment.saturating_add(p);
                if pozicia < self.koniec {
                    self.limity.over_index(body.len() as u64 * 24, 24)?;
                    body.push(BodPosunu { cas, stopa, pozicia });
                }
            }
        }
        body.sort_by_key(|b| (b.stopa, b.cas, b.pozicia));
        self.body = body;
        Ok(())
    }

    fn index_stopy(&self, cislo: u64) -> Option<usize> {
        self.stav.iter().position(|s| s.cislo == cislo)
    }

    /// Nájde ďalší zhluk od `od` (ID zhluku, prijateľná veľkosť a prvý prvok `Timestamp`): (poloha, čas).
    fn hladaj_zhluk(&mut self, od: u64, do_: u64) -> Vysledok<Option<(u64, i64)>> {
        const KUS: usize = 64 << 10;
        let do_ = do_.min(self.koniec).min(od.saturating_add(NAJVIAC_HLADANIA));
        let mut p = od;
        while p + 4 <= do_ {
            let n = ((do_ - p) as usize).min(KUS + 3);
            let kus = self.c.bajty(p, n)?.to_vec();
            if kus.len() < 4 {
                break;
            }
            for i in 0..kus.len() - 3 {
                if kus[i..i + 4] != [0x1F, 0x43, 0xB6, 0x75] {
                    continue;
                }
                let q = p + i as u64;
                if let Some(cas) = self.over_zhluk(q)? {
                    return Ok(Some((q, cas)));
                }
            }
            p += (kus.len() - 3) as u64;
        }
        Ok(None)
    }

    /// Je na `q` platný zhluk? Vráti jeho čas.
    fn over_zhluk(&mut self, q: u64) -> Vysledok<Option<i64>> {
        let d = self.c.bajty(q, 32)?.to_vec();
        let Ok(Some(h)) = hlavicka(&d) else { return Ok(None) };
        if h.id != id::CLUSTER || h.velkost.is_some_and(|v| v > self.koniec.saturating_sub(q)) {
            return Ok(None);
        }
        let zvysok = &d[h.dlzka..];
        let Ok(Some(t)) = hlavicka(zvysok) else { return Ok(None) };
        let Some(v) = t.velkost.filter(|&v| t.id == id::TIMESTAMP && v <= 8) else { return Ok(None) };
        let Some(b) = zvysok.get(t.dlzka..t.dlzka + v as usize) else { return Ok(None) };
        Ok(uint(b).ok().and_then(|c| i64::try_from(c).ok()))
    }

    /// Po poškodení: pokračuj najbližším platným zhlukom za `p`.
    fn synchronizuj(&mut self, p: u64) -> Vysledok<()> {
        self.zhluk = None;
        for s in &mut self.stav {
            s.prerusenie = true;
        }
        self.p = match self.hladaj_zhluk(p + 1, self.koniec)? {
            Some((q, _)) => q,
            None => self.koniec,
        };
        Ok(())
    }

    /// Rozbor bloku (`SimpleBlock` alebo `Block` v `BlockGroup`) na pakety vo fronte.
    fn blok(&mut self, d: &[u8], pozicia: u64, skupina: Option<Skupina>) -> Vysledok<()> {
        let (cislo, n) = vint(d)?;
        let Some(ix) = self.index_stopy(cislo) else { return Ok(()) }; // blok neznámej stopy sa ignoruje
        let hlava = d.get(n..n + 3).ok_or_else(|| Chyba::PoskodeneData("skrátený blok".into()))?;
        let rel = i16::from_be_bytes([hlava[0], hlava[1]]) as i64;
        let priznaky = hlava[2];
        let telo = &d[n + 3..];
        let ramce = lacing(telo, (priznaky >> 1) & 3)?;
        let klucovy = match skupina {
            None => priznaky & 0x80 != 0,
            Some(s) => !s.referencia,
        };
        let zaklad = self.zhluk.and_then(|z| z.cas).unwrap_or(0).saturating_add(rel);
        // trvanie rámca v ns: BlockDuration bloku delené počtom rámcov, inak DefaultDuration stopy
        let pocet = ramce.len() as u64;
        let trvanie_ns = skupina
            .and_then(|s| s.trvanie)
            .map(|t| (t as u128 * self.ns_na_tik as u128 / pocet as u128) as u64)
            .or(self.stav[ix].trvanie_ns);
        let tiky = |ns: u128| (ns + self.ns_na_tik as u128 / 2) / self.ns_na_tik as u128;
        let frekvencia = self.stopy[ix].zvuk.map(|z| z.frekvencia).unwrap_or(0);
        for (k, ramec) in ramce.iter().enumerate() {
            let posun = match (k, trvanie_ns) {
                (0, _) => Some(0),
                (_, Some(t)) => i64::try_from(tiky(k as u128 * t as u128)).ok(),
                (_, None) => None,
            };
            let mut data = Vec::with_capacity(self.stav[ix].hlavicka.len() + ramec.len());
            data.extend_from_slice(&self.stav[ix].hlavicka);
            data.extend_from_slice(ramec);
            let mut paket = Paket {
                stopa: ix,
                data,
                pts: posun.map(|p| zaklad.saturating_add(p)),
                dts: None,
                trvanie: trvanie_ns.and_then(|t| i64::try_from(tiky(t as u128)).ok()),
                klucovy,
                prerusenie: std::mem::take(&mut self.stav[ix].prerusenie),
                orezat_zaciatok: 0,
                orezat_koniec: 0,
                pozicia,
            };
            if k + 1 == ramce.len()
                && let Some(ns) = skupina.and_then(|s| s.orezanie_ns).filter(|&ns| ns > 0 && frekvencia > 0)
            {
                let vzorky = (ns as u128 * frekvencia as u128 + 500_000_000) / 1_000_000_000;
                paket.orezat_koniec = u32::try_from(vzorky).unwrap_or(u32::MAX);
            }
            self.fronta.push_back(paket);
        }
        Ok(())
    }
}

/// Údaje `BlockGroup` okrem samotného bloku.
#[derive(Debug, Clone, Copy, Default)]
struct Skupina {
    trvanie: Option<u64>,
    referencia: bool,
    orezanie_ns: Option<i64>,
}

enum Kodovanie {
    OdstranenaHlavicka(Vec<u8>),
    Nepodporene(String),
}

fn citaj_kodovanie(d: &[u8]) -> Vysledok<Kodovanie> {
    let (mut typ, mut algo, mut nastavenie) = (0u64, 0u64, Vec::new());
    for prvok in prvky(d) {
        let (i, d) = prvok?;
        match i {
            id::CONTENT_ENCODING_TYPE => typ = uint(d)?,
            id::CONTENT_COMPRESSION => {
                for prvok in prvky(d) {
                    let (i, d) = prvok?;
                    match i {
                        id::CONTENT_COMP_ALGO => algo = uint(d)?,
                        id::CONTENT_COMP_SETTINGS => nastavenie = d.to_vec(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(match (typ, algo) {
        (0, 3) => Kodovanie::OdstranenaHlavicka(nastavenie),
        (0, 0) => Kodovanie::Nepodporene("komprimované zlib".into()),
        (0, _) => Kodovanie::Nepodporene("nepodporená kompresia".into()),
        _ => Kodovanie::Nepodporene("šifrované".into()),
    })
}

fn citaj_video(d: &[u8]) -> Vysledok<ParametreVidea> {
    let mut v = ParametreVidea::default();
    let u32_ = |d: &[u8]| uint(d).map(|x| u32::try_from(x).unwrap_or(0));
    for prvok in prvky(d) {
        let (i, d) = prvok?;
        match i {
            id::PIXEL_WIDTH => v.sirka = u32_(d)?,
            id::PIXEL_HEIGHT => v.vyska = u32_(d)?,
            id::DISPLAY_WIDTH => v.zobrazena_sirka = Some(u32_(d)?),
            id::DISPLAY_HEIGHT => v.zobrazena_vyska = Some(u32_(d)?),
            id::COLOUR => v.farba = citaj_farbu(d, &mut v.bitov)?,
            _ => {}
        }
    }
    Ok(v)
}

fn citaj_farbu(d: &[u8], bitov: &mut Option<u8>) -> Vysledok<FarbaVidea> {
    let mut f = FarbaVidea::default();
    let u8_ = |d: &[u8]| uint(d).map(|x| u8::try_from(x).unwrap_or(2));
    for prvok in prvky(d) {
        let (i, d) = prvok?;
        match i {
            id::MATRIX_COEFFICIENTS => f.matica = u8_(d)?,
            id::TRANSFER_CHARACTERISTICS => f.prenos = u8_(d)?,
            id::PRIMARIES => f.zakladne = u8_(d)?,
            id::RANGE => {
                f.plny_rozsah = match uint(d)? {
                    1 => Some(false),
                    2 => Some(true),
                    _ => None,
                }
            }
            id::BITS_PER_CHANNEL => *bitov = u8::try_from(uint(d)?).ok().filter(|&b| b > 0),
            _ => {}
        }
    }
    Ok(f)
}

fn seek_head(d: &[u8]) -> Vysledok<Vec<(u32, u64)>> {
    let mut v = Vec::new();
    for prvok in prvky(d) {
        let (i, d) = prvok?;
        if i != id::SEEK {
            continue;
        }
        let (mut cielove, mut poz) = (None, None);
        for prvok in prvky(d) {
            let (i, d) = prvok?;
            match i {
                id::SEEK_ID => cielove = u32::try_from(uint(d)?).ok(),
                id::SEEK_POSITION => poz = Some(uint(d)?),
                _ => {}
            }
        }
        if let (Some(c), Some(p)) = (cielove, poz) {
            v.push((c, p));
        }
    }
    Ok(v)
}

/// Rozdelenie tela bloku na rámce podľa lacingu (0 žiadny, 1 Xiph, 2 pevný, 3 EBML – RFC 9559, kap. 10.3).
fn lacing(telo: &[u8], druh: u8) -> Vysledok<Vec<&[u8]>> {
    if druh == 0 {
        return Ok(vec![telo]);
    }
    let chyba = || Chyba::PoskodeneData("poškodený lacing bloku".into());
    let pocet = *telo.first().ok_or_else(chyba)? as usize + 1;
    let mut p = 1usize;
    let mut velkosti = Vec::with_capacity(pocet);
    match druh {
        1 => {
            for _ in 0..pocet - 1 {
                let mut v = 0usize;
                loop {
                    let b = *telo.get(p).ok_or_else(chyba)?;
                    p += 1;
                    v += b as usize;
                    if b != 255 {
                        break;
                    }
                }
                velkosti.push(v);
            }
        }
        3 if pocet > 1 => {
            let (prva, n) = vint(telo.get(p..).ok_or_else(chyba)?)?;
            p += n;
            let mut v = i64::try_from(prva).map_err(|_| chyba())?;
            velkosti.push(usize::try_from(v).map_err(|_| chyba())?);
            for _ in 1..pocet - 1 {
                let (r, n) = vint(telo.get(p..).ok_or_else(chyba)?)?;
                p += n;
                // rozdiel so znamienkom: hodnota − (2^(7n−1) − 1)
                let rozdiel = r as i64 - ((1i64 << (7 * n - 1)) - 1);
                v = v.checked_add(rozdiel).filter(|&x| x >= 0).ok_or_else(chyba)?;
                velkosti.push(v as usize);
            }
        }
        3 => {}
        _ => {
            let zvysok = telo.len() - p;
            if !zvysok.is_multiple_of(pocet) {
                return Err(chyba());
            }
            velkosti.resize(pocet - 1, zvysok / pocet);
        }
    }
    let spolu: usize = velkosti.iter().try_fold(0usize, |a, &v| a.checked_add(v)).ok_or_else(chyba)?;
    let posledna = telo.len().checked_sub(p).and_then(|z| z.checked_sub(spolu)).ok_or_else(chyba)?;
    velkosti.push(posledna);
    let mut ramce = Vec::with_capacity(pocet);
    for v in velkosti {
        ramce.push(&telo[p..p + v]);
        p += v;
    }
    Ok(ramce)
}

impl<Z: ZdrojBajtov> Kontajner for Mkv<Z> {
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
        loop {
            if let Some(p) = self.fronta.pop_front() {
                return Ok(Some(p));
            }
            if self.p >= self.koniec {
                return Ok(None);
            }
            if let Some(Zhluk { koniec: Some(k), .. }) = self.zhluk
                && self.p >= k
            {
                self.zhluk = None;
            }
            let p = self.p;
            let h = match self.hlavicka_na(p) {
                Ok(Prvok::Hlavicka(h)) => h,
                Ok(Prvok::Koniec) => return Ok(None), // skrátený súbor – koniec, nie chyba
                Err(Chyba::VstupVystup(e)) => return Err(Chyba::VstupVystup(e)),
                Err(_) => {
                    self.synchronizuj(p)?;
                    continue;
                }
            };
            let zaciatok = p + h.dlzka as u64;
            if h.id == id::CLUSTER {
                let koniec = h.velkost.map(|v| zaciatok.saturating_add(v).min(self.koniec));
                self.zhluk = Some(Zhluk { cas: None, koniec });
                self.p = zaciatok;
                continue;
            }
            if UROVEN_1.contains(&h.id) {
                self.zhluk = None;
            }
            let Some(v) = h.velkost else {
                self.synchronizuj(p)?;
                continue;
            };
            let koniec_prvku = zaciatok.saturating_add(v);
            let hranica = self.zhluk.and_then(|z| z.koniec).unwrap_or(self.koniec);
            if self.zhluk.is_some() && koniec_prvku > hranica {
                self.synchronizuj(p)?;
                continue;
            }
            self.p = koniec_prvku;
            if self.zhluk.is_none() {
                continue; // prvok najvyššej úrovne mimo zhlukov (Cues, Tags, Void …)
            }
            match h.id {
                id::TIMESTAMP => match self.obsah(p, h, 8) {
                    Ok(d) => {
                        if let Some(z) = &mut self.zhluk {
                            z.cas = uint(&d).ok().and_then(|c| i64::try_from(c).ok());
                        }
                    }
                    Err(Chyba::VstupVystup(e)) => return Err(Chyba::VstupVystup(e)),
                    Err(_) => self.synchronizuj(p)?,
                },
                id::SIMPLE_BLOCK | id::BLOCK_GROUP => {
                    let d = match self.obsah(p, h, self.limity.paket) {
                        Ok(d) => d,
                        Err(Chyba::VstupVystup(e)) => return Err(Chyba::VstupVystup(e)),
                        Err(_) => {
                            self.synchronizuj(p)?;
                            continue;
                        }
                    };
                    let r = if h.id == id::SIMPLE_BLOCK {
                        self.blok(&d, zaciatok, None)
                    } else {
                        self.skupina(&d, zaciatok)
                    };
                    if r.is_err() {
                        // poškodený blok sa preskočí, ostatné pokračujú
                        for s in &mut self.stav {
                            s.prerusenie = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn posun(&mut self, ciel_ns: i64, sposob: Posun) -> Vysledok<i64> {
        let predbeh = match sposob {
            Posun::KlucovyPred => 0,
            Posun::Presny => self.stopy.iter().map(|s| s.predbeh_posunu_ns).max().unwrap_or(0),
        };
        let ciel = z_ns(ciel_ns.saturating_sub(predbeh).max(0), self.zakladna).unwrap_or(0);
        // stopa, podľa ktorej sa posúva: prvé video, inak prvá stopa s bodmi posunu
        let cislo = self
            .stopy
            .iter()
            .find(|s| s.druh == DruhStopy::Video)
            .or_else(|| self.stopy.first())
            .map(|s| s.cislo)
            .unwrap_or(0);
        let cislo = if self.body.iter().any(|b| b.stopa == cislo) {
            Some(cislo)
        } else {
            self.body.first().map(|b| b.stopa)
        };
        let (pozicia, cas) = match cislo {
            Some(c) => {
                let body: Vec<_> = self.body.iter().filter(|b| b.stopa == c).collect();
                let i = body.partition_point(|b| b.cas <= ciel).saturating_sub(1);
                (body[i].pozicia, body[i].cas)
            }
            None => self.posun_delenim(ciel)?,
        };
        self.p = pozicia;
        self.zhluk = None;
        self.fronta.clear();
        for s in &mut self.stav {
            s.prerusenie = true;
        }
        Ok(na_ns(cas, self.zakladna).unwrap_or(0))
    }
}

impl<Z: ZdrojBajtov> Mkv<Z> {
    fn skupina(&mut self, d: &[u8], pozicia: u64) -> Vysledok<()> {
        let mut s = Skupina::default();
        let mut blok = None;
        for prvok in prvky(d) {
            let (i, d) = prvok?;
            match i {
                id::BLOCK => blok = Some(d),
                id::BLOCK_DURATION => s.trvanie = Some(uint(d)?),
                id::REFERENCE_BLOCK => s.referencia = true,
                id::DISCARD_PADDING => s.orezanie_ns = Some(sint(d)?),
                _ => {}
            }
        }
        match blok {
            Some(b) => self.blok(b, pozicia, Some(s)),
            None => Err(Chyba::PoskodeneData("BlockGroup bez bloku".into())),
        }
    }

    /// Posun bez Cues: delenie intervalu podľa polohy v súbore – najneskorší zhluk s časom ≤ cieľ.
    fn posun_delenim(&mut self, ciel: i64) -> Vysledok<(u64, i64)> {
        let prvy = self.prvy_zhluk.unwrap_or(self.koniec);
        let Some((prvy, cas_prveho)) = self.hladaj_zhluk(prvy, self.koniec)? else {
            return Ok((self.koniec, 0));
        };
        let davka = self.c.davka();
        self.c.nastav_davku(64 << 10); // malé čítania pri hľadaní
        let mut najlepsi = (prvy, cas_prveho);
        let (mut lo, mut hi) = (prvy, self.koniec);
        let mut pokusov = 0;
        while hi > lo + 1 && pokusov < 64 {
            pokusov += 1;
            let stred = lo + (hi - lo) / 2;
            match self.hladaj_zhluk(stred, hi)? {
                Some((q, cas)) if cas <= ciel => {
                    if cas >= najlepsi.1 {
                        najlepsi = (q, cas);
                    }
                    lo = q.max(stred);
                }
                _ => hi = stred,
            }
        }
        self.c.nastav_davku(davka);
        Ok(najlepsi)
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn hlavicky_ebml() {
        assert_eq!(
            hlavicka(&[0x1A, 0x45, 0xDF, 0xA3, 0x84]).unwrap(),
            Some(Hlavicka { id: id::EBML, velkost: Some(4), dlzka: 5 })
        );
        // neznáma veľkosť (všetky bity 1) v 1 aj 8 bajtoch
        assert_eq!(hlavicka(&[0xA3, 0xFF]).unwrap().unwrap().velkost, None);
        assert_eq!(
            hlavicka(&[0xA3, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).unwrap().unwrap().velkost,
            None
        );
        assert_eq!(
            hlavicka(&[0xA3, 0x40, 0x02]).unwrap().unwrap(),
            Hlavicka { id: 0xA3, velkost: Some(2), dlzka: 3 }
        );
        assert!(hlavicka(&[0x00]).is_err());
        assert!(hlavicka(&[0x08, 1, 2, 3, 4]).is_err()); // ID dlhšie ako 4 B
        assert_eq!(hlavicka(&[0x1A, 0x45]).unwrap(), None);
        assert_eq!(hlavicka(&[]).unwrap(), None);
    }

    #[test]
    fn cisla() {
        assert_eq!(uint(&[]).unwrap(), 0);
        assert_eq!(uint(&[1, 0]).unwrap(), 256);
        assert!(uint(&[0; 9]).is_err());
        assert_eq!(sint(&[0xFF]).unwrap(), -1);
        assert_eq!(sint(&[0xFF, 0x38]).unwrap(), -200);
        assert_eq!(float(&1.5f32.to_be_bytes()).unwrap(), 1.5);
        assert_eq!(float(&2.25f64.to_be_bytes()).unwrap(), 2.25);
        assert!(float(&[0; 3]).is_err());
        assert_eq!(vint(&[0x81]).unwrap(), (1, 1));
        assert_eq!(vint(&[0x40, 0x02]).unwrap(), (2, 2));
        assert_eq!(text(b"eng\0\0"), "eng");
    }

    #[test]
    fn lacing_vsetky_druhy() {
        // Xiph: 3 rámce 300, 2, zvyšok 4
        let mut t = vec![2u8, 255, 45, 2];
        t.extend(std::iter::repeat_n(7u8, 306));
        let r = lacing(&t, 1).unwrap();
        assert_eq!(r.iter().map(|r| r.len()).collect::<Vec<_>>(), [300, 2, 4]);
        // pevný: 4 rámce po 3
        let r = lacing(&[3, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 2).unwrap();
        assert_eq!(r, [&[1u8, 2, 3][..], &[4, 5, 6], &[7, 8, 9], &[10, 11, 12]]);
        assert!(lacing(&[3, 1, 2, 3, 4, 5], 2).is_err());
        // EBML: 3 rámce – 5, 5 − 2 = 3, zvyšok 1 (rozdiel −2 v 1 B: 63 − 2 = 61 → 0x80 | 61)
        let mut t = vec![2u8, 0x85, 0x80 | 61];
        t.extend([1u8; 9]);
        let r = lacing(&t, 3).unwrap();
        assert_eq!(r.iter().map(|r| r.len()).collect::<Vec<_>>(), [5, 3, 1]);
        // poškodené: veľkosti presahujú telo
        assert!(lacing(&[1, 200, 1, 2], 1).is_err());
        assert!(lacing(&[], 1).is_err());
        assert!(lacing(&[2, 0x85, 0x80], 3).is_err());
    }

    #[test]
    fn kody_kodekov() {
        assert_eq!(kodek("V_MPEG4/ISO/AVC", None), Kodek::H264);
        assert_eq!(kodek("A_AAC/MPEG4/LC/SBR", None), Kodek::Aac);
        assert_eq!(
            kodek("A_PCM/INT/LIT", Some(8)),
            Kodek::Pcm(PopisPcm {
                bitov: 8,
                bajtov: 1,
                poradie: PoradieBajtov::MaloKoncove,
                cislo: CisloPcm::BezZnamienka
            })
        );
        assert_eq!(
            kodek("A_PCM/INT/BIG", Some(24)),
            Kodek::Pcm(PopisPcm {
                bitov: 24,
                bajtov: 3,
                poradie: PoradieBajtov::VelkoKoncove,
                cislo: CisloPcm::SoZnamienkom
            })
        );
        assert_eq!(kodek("X_NIECO", None), Kodek::Neznamy("X_NIECO".into()));
    }
}
