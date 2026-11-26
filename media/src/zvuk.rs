//! **Zvuk** – bloky dekódovaných vzoriek, rozloženie kanálov a rozhranie dekodéra zvuku.
//!
//! Vzorky sú **planárne** (každý kanál súvisle): dekodéry aj efekty pracujú po kanáloch, prekladanie (interleave)
//! sa robí až na hranici výstupu (R5). Bezstratové kodeky dávajú celé čísla s udanou bitovou hĺbkou – bitovo
//! presne; stratové dávajú `f32` v rozsahu ±1,0 (plný rozsah).

use crate::paket::Paket;
use crate::stopa::ParametreZvuku;
use orchidea_jadro::{Chyba, Vysledok};

/// Rozloženie kanálov – bitová maska polôh reproduktorov. Poradie bitov je **spoločné poradie kanálov** v bloku
/// (rovnaké ako maska kanálov WAVE_FORMAT_EXTENSIBLE): kanál s nižším bitom ide skôr. Poradie podľa CTA-861 pre
/// HDMI sa robí až v R5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RozlozenieKanalov(pub u32);

impl RozlozenieKanalov {
    pub const PREDNY_LAVY: u32 = 1 << 0;
    pub const PREDNY_PRAVY: u32 = 1 << 1;
    pub const PREDNY_STRED: u32 = 1 << 2;
    /// Basový kanál (LFE).
    pub const BASY: u32 = 1 << 3;
    pub const ZADNY_LAVY: u32 = 1 << 4;
    pub const ZADNY_PRAVY: u32 = 1 << 5;
    pub const PREDNY_LAVY_STRED: u32 = 1 << 6;
    pub const PREDNY_PRAVY_STRED: u32 = 1 << 7;
    pub const ZADNY_STRED: u32 = 1 << 8;
    pub const BOCNY_LAVY: u32 = 1 << 9;
    pub const BOCNY_PRAVY: u32 = 1 << 10;
    pub const HORNY_STRED: u32 = 1 << 11;
    pub const HORNY_PREDNY_LAVY: u32 = 1 << 12;
    pub const HORNY_PREDNY_STRED: u32 = 1 << 13;
    pub const HORNY_PREDNY_PRAVY: u32 = 1 << 14;
    pub const HORNY_ZADNY_LAVY: u32 = 1 << 15;
    pub const HORNY_ZADNY_STRED: u32 = 1 << 16;
    pub const HORNY_ZADNY_PRAVY: u32 = 1 << 17;

    pub const MONO: RozlozenieKanalov = RozlozenieKanalov(Self::PREDNY_STRED);
    pub const STEREO: RozlozenieKanalov = RozlozenieKanalov(Self::PREDNY_LAVY | Self::PREDNY_PRAVY);
    /// 5.1 s bočnými reproduktormi (Matroska, FLAC, Opus, AC-3 podľa poradia kodeku po prevode).
    pub const S5_1: RozlozenieKanalov = RozlozenieKanalov(
        Self::PREDNY_LAVY
            | Self::PREDNY_PRAVY
            | Self::PREDNY_STRED
            | Self::BASY
            | Self::BOCNY_LAVY
            | Self::BOCNY_PRAVY,
    );
    /// 5.1 so zadnými reproduktormi.
    pub const S5_1_ZADNE: RozlozenieKanalov = RozlozenieKanalov(
        Self::PREDNY_LAVY
            | Self::PREDNY_PRAVY
            | Self::PREDNY_STRED
            | Self::BASY
            | Self::ZADNY_LAVY
            | Self::ZADNY_PRAVY,
    );
    /// 7.1 – predné, stred, basy, zadné a bočné.
    pub const S7_1: RozlozenieKanalov =
        RozlozenieKanalov(Self::S5_1_ZADNE.0 | Self::BOCNY_LAVY | Self::BOCNY_PRAVY);

    /// Počet kanálov.
    pub fn kanalov(self) -> u32 {
        self.0.count_ones()
    }

    /// Predvolené rozloženie podľa počtu kanálov (ako pri FLAC a Opuse s rodinou mapovania 1).
    pub fn predvolene(kanalov: u8) -> Option<RozlozenieKanalov> {
        let maska = match kanalov {
            1 => Self::PREDNY_STRED,
            2 => Self::STEREO.0,
            3 => Self::STEREO.0 | Self::PREDNY_STRED,
            4 => Self::STEREO.0 | Self::ZADNY_LAVY | Self::ZADNY_PRAVY,
            5 => Self::STEREO.0 | Self::PREDNY_STRED | Self::BOCNY_LAVY | Self::BOCNY_PRAVY,
            6 => Self::S5_1.0,
            7 => Self::S5_1.0 | Self::ZADNY_STRED,
            8 => Self::S7_1.0,
            _ => return None,
        };
        Some(RozlozenieKanalov(maska))
    }
}

/// Dekódované vzorky – planárne: kanál `k` je `data[k * ramcov .. (k + 1) * ramcov]`.
#[derive(Debug, Clone, PartialEq)]
pub enum VzorkyZvuku {
    /// Celé čísla so znamienkom v rozsahu `bitov` (PCM, FLAC) – bitovo presne podľa zdroja.
    Cele { bitov: u8, data: Vec<i32> },
    /// Plávajúca čiarka, plný rozsah ±1,0 (MP3, Opus, AC-3).
    F32(Vec<f32>),
}

/// Blok dekódovaných vzoriek.
#[derive(Debug, Clone, PartialEq)]
pub struct BlokZvuku {
    pub frekvencia: u32,
    pub rozlozenie: RozlozenieKanalov,
    /// Počet rámcov (vzoriek na kanál).
    pub ramcov: usize,
    pub vzorky: VzorkyZvuku,
    /// Čas prvej vzorky na časovej osi média (ns), ak je známy.
    pub pts_ns: Option<i64>,
}

impl BlokZvuku {
    /// Prázdny blok celých vzoriek.
    pub fn cele(frekvencia: u32, rozlozenie: RozlozenieKanalov, bitov: u8) -> BlokZvuku {
        BlokZvuku {
            frekvencia,
            rozlozenie,
            ramcov: 0,
            vzorky: VzorkyZvuku::Cele { bitov, data: Vec::new() },
            pts_ns: None,
        }
    }

    /// Prázdny blok vzoriek `f32`.
    pub fn f32(frekvencia: u32, rozlozenie: RozlozenieKanalov) -> BlokZvuku {
        BlokZvuku { frekvencia, rozlozenie, ramcov: 0, vzorky: VzorkyZvuku::F32(Vec::new()), pts_ns: None }
    }

    pub fn kanalov(&self) -> usize {
        self.rozlozenie.kanalov() as usize
    }

    /// Overí, že veľkosť údajov zodpovedá kanálom a rámcom.
    pub fn over(&self) -> Vysledok<()> {
        let n = match &self.vzorky {
            VzorkyZvuku::Cele { bitov, data } => {
                if !(1..=32).contains(bitov) {
                    return Err(Chyba::MimoRozsahu(format!("bitová hĺbka {bitov}")));
                }
                data.len()
            }
            VzorkyZvuku::F32(data) => data.len(),
        };
        if self.kanalov() == 0 || n != self.kanalov() * self.ramcov {
            return Err(Chyba::NezhodaTvaru(format!(
                "{n} vzoriek nie je {} kanálov × {} rámcov",
                self.kanalov(),
                self.ramcov
            )));
        }
        Ok(())
    }
}

/// Dekodér zvuku jednej stopy.
pub trait DekoderZvuku: std::fmt::Debug + Send {
    /// Parametre výstupu (frekvencia, kanály, rozloženie, bitová hĺbka).
    fn parametre(&self) -> &ParametreZvuku;

    /// Dekóduje jeden paket – **prepíše** obsah `vystup` (pamäť bloku sa použije znova, bez alokácie pri každom
    /// pakete). Orezanie podľa `Paket::orezat_*` je už uplatnené.
    fn dekoduj(&mut self, paket: &Paket, vystup: &mut BlokZvuku) -> Vysledok<()>;

    /// Koľko ns pred cieľom posunu treba začať dekódovať, aby bol výstup v cieli úplný (Opus 80 ms, MP3 zásobník
    /// bitov, prekrytie MDCT).
    fn predbeh_ns(&self) -> i64;

    /// Zabudne stav (po posune v súbore).
    fn reset(&mut self);
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn rozlozenia() {
        for n in 1..=8u8 {
            assert_eq!(RozlozenieKanalov::predvolene(n).unwrap().kanalov(), n as u32);
        }
        assert!(RozlozenieKanalov::predvolene(0).is_none() && RozlozenieKanalov::predvolene(9).is_none());
        assert_eq!(RozlozenieKanalov::S5_1.kanalov(), 6);
        assert_eq!(RozlozenieKanalov::S5_1_ZADNE.kanalov(), 6);
        assert_eq!(RozlozenieKanalov::S7_1.kanalov(), 8);
        // masky ako WAVE_FORMAT_EXTENSIBLE: 5.1 bočné 0x60F, 5.1 zadné 0x3F, 7.1 0x63F
        assert_eq!(RozlozenieKanalov::S5_1.0, 0x60F);
        assert_eq!(RozlozenieKanalov::S5_1_ZADNE.0, 0x3F);
        assert_eq!(RozlozenieKanalov::S7_1.0, 0x63F);
    }

    #[test]
    fn kontrola_bloku() {
        let mut b = BlokZvuku::cele(44_100, RozlozenieKanalov::STEREO, 16);
        b.ramcov = 3;
        assert!(b.over().is_err());
        b.vzorky = VzorkyZvuku::Cele { bitov: 16, data: vec![0; 6] };
        assert!(b.over().is_ok());
        b.vzorky = VzorkyZvuku::Cele { bitov: 33, data: vec![0; 6] };
        assert!(b.over().is_err());
        let mut f = BlokZvuku::f32(48_000, RozlozenieKanalov::MONO);
        f.ramcov = 2;
        f.vzorky = VzorkyZvuku::F32(vec![0.0; 2]);
        assert!(f.over().is_ok());
    }
}
