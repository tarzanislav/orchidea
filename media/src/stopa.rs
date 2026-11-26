//! **Stopy a kodeky** – čo kontajner obsahuje: druh stopy, kodek, jeho konfigurácia, časová základňa a parametre
//! zvuku alebo videa.

use crate::zvuk::RozlozenieKanalov;
use orchidea_jadro::Zlomok;

/// Druh stopy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DruhStopy {
    Video,
    Zvuk,
    Titulky,
    Ine,
}

/// Poradie bajtov vzoriek PCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoradieBajtov {
    MaloKoncove,
    VelkoKoncove,
}

/// Číselný typ vzoriek PCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CisloPcm {
    /// Celé číslo so znamienkom (dvojkový doplnok).
    SoZnamienkom,
    /// Celé číslo bez znamienka (8-bitové WAV a podobné) – stred je 2^(n−1).
    BezZnamienka,
    /// IEEE 754 – 32 alebo 64 bitov.
    Plavajuce,
}

/// Popis vzoriek PCM v kontajneri.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopisPcm {
    /// Bitov na vzorku v súbore (8, 16, 20, 24, 32, 64).
    pub bitov: u8,
    /// Bajtov na vzorku v súbore (napríklad 20 bitov v 3 bajtoch).
    pub bajtov: u8,
    pub poradie: PoradieBajtov,
    pub cislo: CisloPcm,
}

/// Kodek stopy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kodek {
    // zvuk – vlastné dekodéry (L2, L7–L9)
    Pcm(PopisPcm),
    Flac,
    Opus,
    /// MPEG audio – vrstva 1, 2 alebo 3 (MP3).
    MpegAudio {
        vrstva: u8,
    },
    Ac3,
    Eac3,
    // zvuk – len priechod alebo neskôr
    Aac,
    Dts,
    TrueHd,
    Vorbis,
    // video – hardvérový dekodér cez R5
    H264,
    Hevc,
    Av1,
    Vp9,
    Vp8,
    Mpeg2Video,
    Mpeg4Cast2,
    Vc1,
    // titulky
    TextUtf8,
    Ass,
    Pgs,
    VobSub,
    DvbTitulky,
    /// Neznámy kodek – identifikátor z kontajnera (CodecID, štvorznak, typ toku).
    Neznamy(String),
}

impl Kodek {
    /// Zrozumiteľné meno kodeku.
    pub fn meno(&self) -> String {
        match self {
            Kodek::Pcm(p) => format!("PCM {} b", p.bitov),
            Kodek::Flac => "FLAC".into(),
            Kodek::Opus => "Opus".into(),
            Kodek::MpegAudio { vrstva: 3 } => "MP3".into(),
            Kodek::MpegAudio { vrstva } => format!("MPEG audio vrstva {vrstva}"),
            Kodek::Ac3 => "AC-3".into(),
            Kodek::Eac3 => "E-AC-3".into(),
            Kodek::Aac => "AAC".into(),
            Kodek::Dts => "DTS".into(),
            Kodek::TrueHd => "TrueHD".into(),
            Kodek::Vorbis => "Vorbis".into(),
            Kodek::H264 => "H.264".into(),
            Kodek::Hevc => "HEVC".into(),
            Kodek::Av1 => "AV1".into(),
            Kodek::Vp9 => "VP9".into(),
            Kodek::Vp8 => "VP8".into(),
            Kodek::Mpeg2Video => "MPEG-2 video".into(),
            Kodek::Mpeg4Cast2 => "MPEG-4 časť 2".into(),
            Kodek::Vc1 => "VC-1".into(),
            Kodek::TextUtf8 => "text UTF-8".into(),
            Kodek::Ass => "ASS".into(),
            Kodek::Pgs => "PGS".into(),
            Kodek::VobSub => "VobSub".into(),
            Kodek::DvbTitulky => "titulky DVB".into(),
            Kodek::Neznamy(s) => format!("neznámy ({s})"),
        }
    }

    pub fn druh(&self) -> DruhStopy {
        match self {
            Kodek::Pcm(_)
            | Kodek::Flac
            | Kodek::Opus
            | Kodek::MpegAudio { .. }
            | Kodek::Ac3
            | Kodek::Eac3
            | Kodek::Aac
            | Kodek::Dts
            | Kodek::TrueHd
            | Kodek::Vorbis => DruhStopy::Zvuk,
            Kodek::H264
            | Kodek::Hevc
            | Kodek::Av1
            | Kodek::Vp9
            | Kodek::Vp8
            | Kodek::Mpeg2Video
            | Kodek::Mpeg4Cast2
            | Kodek::Vc1 => DruhStopy::Video,
            Kodek::TextUtf8 | Kodek::Ass | Kodek::Pgs | Kodek::VobSub | Kodek::DvbTitulky => {
                DruhStopy::Titulky
            }
            Kodek::Neznamy(_) => DruhStopy::Ine,
        }
    }
}

/// Parametre zvukovej stopy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParametreZvuku {
    /// Vzorkovacia frekvencia (Hz) – výstupná (pri Opuse vždy 48 000).
    pub frekvencia: u32,
    pub kanalov: u8,
    pub rozlozenie: RozlozenieKanalov,
    /// Bitová hĺbka, ak ju formát má (PCM, FLAC).
    pub bitov: Option<u8>,
}

/// Popis farby videa – kódy CICP podľa ITU-T H.273 (ako `formaty::Cicp`; 2 = nešpecifikované).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FarbaVidea {
    /// Základné farby: 1 = BT.709, 9 = BT.2020 …
    pub zakladne: u8,
    /// Prenosová funkcia: 1 = BT.709, 16 = PQ, 18 = HLG …
    pub prenos: u8,
    /// Matica: 1 = BT.709, 9 = BT.2020 NCL …
    pub matica: u8,
    /// Plný rozsah (nie obmedzený 16–235), ak je známy.
    pub plny_rozsah: Option<bool>,
}

impl Default for FarbaVidea {
    fn default() -> Self {
        FarbaVidea { zakladne: 2, prenos: 2, matica: 2, plny_rozsah: None }
    }
}

/// Parametre videostopy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParametreVidea {
    pub sirka: u32,
    pub vyska: u32,
    /// Rozmer na zobrazenie (pomer strán pixela), ak sa líši.
    pub zobrazena_sirka: Option<u32>,
    pub zobrazena_vyska: Option<u32>,
    /// Snímky za sekundu ako presný zlomok, ak ich kontajner udáva (alebo prichytené k typickej hodnote).
    pub snimkova_frekvencia: Option<Zlomok>,
    /// Bitová hĺbka jasu (8, 10, 12), ak je známa.
    pub bitov: Option<u8>,
    pub farba: FarbaVidea,
}

/// Stopa kontajnera.
#[derive(Debug, Clone, PartialEq)]
pub struct Stopa {
    /// Číslo stopy v kontajneri (Matroska `TrackNumber`, MP4 `track_ID`, MPEG-TS PID).
    pub cislo: u64,
    pub druh: DruhStopy,
    pub kodek: Kodek,
    /// Konfigurácia kodeku z kontajnera (Matroska `CodecPrivate`, MP4 `avcC`/`hvcC`/`dOps`/`dfLa` …).
    pub konfiguracia: Vec<u8>,
    /// Sekundy na jeden tik časových značiek paketov tejto stopy.
    pub zakladna: Zlomok,
    /// Jazyk (BCP 47 alebo ISO 639-2), meno stopy.
    pub jazyk: Option<String>,
    pub meno: Option<String>,
    /// Predvolená stopa pre prehrávanie.
    pub predvolena: bool,
    pub zvuk: Option<ParametreZvuku>,
    pub video: Option<ParametreVidea>,
    /// Oneskorenie kodeku – koľko ns zo začiatku dekódovaného zvuku zahodiť (Matroska `CodecDelay`, Opus pre-skip).
    pub oneskorenie_kodeku_ns: i64,
    /// Koľko ns pred cieľom posunu treba začať dekódovať (Matroska `SeekPreRoll`; Opus 80 ms).
    pub predbeh_posunu_ns: i64,
}

impl Stopa {
    /// Nová stopa s predvolenými údajmi (druh podľa kodeku).
    pub fn nova(cislo: u64, kodek: Kodek, zakladna: Zlomok) -> Stopa {
        Stopa {
            cislo,
            druh: kodek.druh(),
            kodek,
            konfiguracia: Vec::new(),
            zakladna,
            jazyk: None,
            meno: None,
            predvolena: false,
            zvuk: None,
            video: None,
            oneskorenie_kodeku_ns: 0,
            predbeh_posunu_ns: 0,
        }
    }
}
