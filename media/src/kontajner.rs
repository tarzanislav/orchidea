//! **Kontajnery** – rozpoznanie formátu podľa obsahu (nie podľa prípony) a spoločné rozhranie čítačiek.
//!
//! Čítačky (Matroska/WebM – L1, MP4/MOV – L3, MPEG-TS – L4) dodržiavajú spoločné zásady z návrhu: nikdy pád,
//! alokácia až po kontrole limitov, pri otvorení len hlavička a indexy, pakety v poradí súboru (čítanie ide
//! súvisle dopredu), kompaktné indexy.

use crate::limity::Limity;
use crate::paket::Paket;
use crate::stopa::Stopa;
use crate::zdroj::ZdrojBajtov;
use orchidea_jadro::{Chyba, Vysledok};

/// Formát kontajnera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKontajnera {
    /// Matroska (EBML, RFC 9559). WebM je podmnožina – rozlíši sa podľa `DocType` pri otvorení.
    Matroska,
    WebM,
    /// ISO BMFF (MP4, M4A, 3GP …).
    Mp4,
    /// QuickTime (MOV) – značka `qt  ` alebo starý súbor bez `ftyp`.
    QuickTime,
    /// MPEG-TS: 188 B (vysielanie), 192 B (M2TS, Blu-ray – 4 B značka príchodu), 204 B (s Reed-Solomonom).
    MpegTs {
        velkost_paketu: u16,
    },
}

impl FormatKontajnera {
    pub fn meno(self) -> &'static str {
        match self {
            FormatKontajnera::Matroska => "Matroska",
            FormatKontajnera::WebM => "WebM",
            FormatKontajnera::Mp4 => "MP4",
            FormatKontajnera::QuickTime => "QuickTime",
            FormatKontajnera::MpegTs { velkost_paketu: 192 } => "MPEG-TS (M2TS)",
            FormatKontajnera::MpegTs { .. } => "MPEG-TS",
        }
    }
}

/// Typy boxov, ktorými môže začínať súbor ISO BMFF alebo QuickTime.
const BOXY_NA_ZACIATKU: [&[u8; 4]; 11] =
    [b"ftyp", b"styp", b"moov", b"mdat", b"free", b"skip", b"wide", b"pnot", b"uuid", b"moof", b"sidx"];

/// Rozpozná kontajner zo začiatku súboru (stačí 1 KiB; pri MPEG-TS sa kontroluje synchronizačný bajt
/// 0x47 v troch po sebe idúcich paketoch, ak je údajov dosť, inak v dvoch).
pub fn rozpoznaj(zaciatok: &[u8]) -> Option<FormatKontajnera> {
    if zaciatok.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(FormatKontajnera::Matroska);
    }
    if zaciatok.len() >= 8 {
        let velkost = u32::from_be_bytes([zaciatok[0], zaciatok[1], zaciatok[2], zaciatok[3]]);
        let typ = &zaciatok[4..8];
        // veľkosť 0 = do konca súboru, 1 = 64-bitová veľkosť nasleduje, inak aspoň hlavička 8 B
        if (velkost == 0 || velkost == 1 || velkost >= 8) && BOXY_NA_ZACIATKU.iter().any(|b| &b[..] == typ) {
            if typ == b"ftyp" {
                let qt = zaciatok.get(8..12) == Some(b"qt  ");
                return Some(if qt { FormatKontajnera::QuickTime } else { FormatKontajnera::Mp4 });
            }
            // bez ftyp: starý QuickTime alebo fragment – rozlíši sa pri otvorení
            return Some(if typ == b"styp" || typ == b"moof" || typ == b"sidx" {
                FormatKontajnera::Mp4
            } else {
                FormatKontajnera::QuickTime
            });
        }
    }
    for (velkost, posun) in [(188usize, 0usize), (192, 4), (204, 0)] {
        let pocet = if zaciatok.len() > posun + 2 * velkost { 3 } else { 2 };
        if (0..pocet).all(|k| zaciatok.get(posun + k * velkost) == Some(&0x47)) {
            return Some(FormatKontajnera::MpegTs { velkost_paketu: velkost as u16 });
        }
    }
    None
}

/// Spôsob posunu v súbore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posun {
    /// Najbližší kľúčový paket pred cieľom alebo v ňom (rýchle, napríklad pri ťahaní posuvníka).
    KlucovyPred,
    /// Kľúčový paket pred cieľom so začiatkom o predbeh dekodéra skôr; pakety pred cieľom sa dekódujú, ale
    /// nezobrazia ani nezahrajú (presné na snímku a vzorku).
    Presny,
}

/// Čítačka kontajnera.
pub trait Kontajner: std::fmt::Debug {
    fn format(&self) -> FormatKontajnera;

    /// Stopy súboru (index do tohto zoznamu je `Paket::stopa`).
    fn stopy(&self) -> &[Stopa];

    /// Trvanie v ns, ak je známe.
    fn trvanie_ns(&self) -> Option<i64>;

    /// Ďalší paket v poradí súboru (prekladané stopy); `None` = koniec.
    fn dalsi_paket(&mut self) -> Vysledok<Option<Paket>>;

    /// Posun na čas v ns (časová os média); vráti čas prvého kľúčového paketu, od ktorého čítanie pokračuje.
    fn posun(&mut self, ciel_ns: i64, sposob: Posun) -> Vysledok<i64>;
}

/// Otvorí kontajner podľa obsahu zdroja.
pub fn otvor<Z: ZdrojBajtov + 'static>(zdroj: Z, limity: Limity) -> Vysledok<Box<dyn Kontajner>> {
    let mut zaciatok = vec![0u8; 1024.min(zdroj.dlzka() as usize)];
    let n = zdroj.citaj_na(0, &mut zaciatok)?;
    zaciatok.truncate(n);
    match rozpoznaj(&zaciatok) {
        Some(FormatKontajnera::Matroska | FormatKontajnera::WebM) => {
            Ok(Box::new(crate::mkv::Mkv::otvor(zdroj, limity)?))
        }
        Some(f) => {
            Err(Chyba::MimoRozsahu(format!("čítačka kontajnera {} ešte nie je (etapy L3, L4)", f.meno())))
        }
        None => Err(Chyba::PoskodeneData("neznámy formát kontajnera".into())),
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn ts(velkost: usize, posun: usize, paketov: usize) -> Vec<u8> {
        let mut d = vec![0u8; velkost * paketov];
        for k in 0..paketov {
            d[k * velkost + posun] = 0x47;
        }
        d
    }

    #[test]
    fn rozpoznanie() {
        assert_eq!(rozpoznaj(&[0x1A, 0x45, 0xDF, 0xA3, 0x9F]), Some(FormatKontajnera::Matroska));
        let mut mp4 = vec![0, 0, 0, 0x18];
        mp4.extend_from_slice(b"ftypisom\0\0\x02\0isomiso2");
        assert_eq!(rozpoznaj(&mp4), Some(FormatKontajnera::Mp4));
        let mut mov = vec![0, 0, 0, 0x14];
        mov.extend_from_slice(b"ftypqt  \0\0\x02\0qt  ");
        assert_eq!(rozpoznaj(&mov), Some(FormatKontajnera::QuickTime));
        assert_eq!(rozpoznaj(b"\0\0\0\x08wide\0\0\0\x10mdat"), Some(FormatKontajnera::QuickTime));
        assert_eq!(rozpoznaj(b"\0\0\0\x10moof\0\0\0\x10mfhd"), Some(FormatKontajnera::Mp4));
        assert_eq!(rozpoznaj(&ts(188, 0, 4)), Some(FormatKontajnera::MpegTs { velkost_paketu: 188 }));
        assert_eq!(rozpoznaj(&ts(192, 4, 4)), Some(FormatKontajnera::MpegTs { velkost_paketu: 192 }));
        assert_eq!(rozpoznaj(&ts(204, 0, 4)), Some(FormatKontajnera::MpegTs { velkost_paketu: 204 }));
        // jeden synchronizačný bajt nestačí
        assert_eq!(rozpoznaj(&[0x47, 0, 0, 0]), None);
        assert_eq!(rozpoznaj(b"RIFF\0\0\0\0WAVE"), None);
        // box so zlou veľkosťou nie je MP4
        assert_eq!(rozpoznaj(b"\0\0\0\x04ftypisom"), None);
        for n in 0..12 {
            let _ = rozpoznaj(&mp4[..n]);
        }
    }
}
