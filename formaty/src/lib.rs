//! **Obrazové formáty vlastným kódom** – PNG, JPEG, JPEG XL, TIFF a OpenEXR (E1, krok 3; R207, R211).
#![forbid(unsafe_code)]

pub mod deflate;
pub mod exif;
pub mod exr;
pub mod huffman;
pub mod jpeg;
pub mod jxl;
pub mod kontrola;
pub mod obraz;
pub mod png;
pub mod polovicna;
pub mod tiff;

pub use obraz::{Cicp, Farba, Limity, Metadata, Model, Obraz, Vzorky};

use orchidea_jadro::{Chyba, Vysledok};

/// Obrazový formát súboru.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Tiff,
    Exr,
    Jxl,
}

impl Format {
    pub fn meno(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Tiff => "TIFF",
            Format::Exr => "OpenEXR",
            Format::Jxl => "JPEG XL",
        }
    }

    /// Formát podľa prípony súboru (bez bodky, malé aj veľké písmená).
    pub fn z_pripony(pripona: &str) -> Option<Format> {
        match pripona.to_ascii_lowercase().as_str() {
            "png" | "apng" => Some(Format::Png),
            "jpg" | "jpeg" | "jpe" | "jfif" => Some(Format::Jpeg),
            "tif" | "tiff" => Some(Format::Tiff),
            "exr" => Some(Format::Exr),
            "jxl" => Some(Format::Jxl),
            _ => None,
        }
    }
}

/// Rozpozná formát podľa podpisu na začiatku údajov (nie podľa prípony – tá môže klamať).
pub fn rozpoznaj(data: &[u8]) -> Option<Format> {
    if data.starts_with(&png::PODPIS) {
        return Some(Format::Png);
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if data.len() >= 4 && matches!(&data[..4], b"II*\0" | b"MM\0*" | b"II+\0" | b"MM\0+") {
        return Some(Format::Tiff);
    }
    if data.starts_with(&exr::PODPIS) {
        return Some(Format::Exr);
    }
    if data.starts_with(&jxl::PODPIS_TOKU) || data.starts_with(&jxl::PODPIS_KONTAJNERA) {
        return Some(Format::Jxl);
    }
    None
}

/// Prečíta obrázok ľubovoľného podporovaného formátu.
pub fn citaj(data: &[u8], limity: &Limity) -> Vysledok<Obraz> {
    match rozpoznaj(data) {
        Some(Format::Png) => png::citaj(data, limity),
        Some(Format::Jpeg) => jpeg::citaj(data, limity),
        Some(Format::Tiff) => tiff::citaj(data, limity),
        Some(Format::Exr) => exr::citaj(data, limity),
        Some(Format::Jxl) => jxl::citaj(data, limity),
        None => Err(Chyba::NezhodaTypov { popis: "neznámy obrazový formát".into(), navrh: None }),
    }
}

/// Zapíše obrázok vo zvolenom formáte s predvoleným nastavením.
pub fn zapis(obraz: &Obraz, format: Format) -> Vysledok<Vec<u8>> {
    match format {
        Format::Png => png::zapis(obraz, &png::NastaveniePng::default()),
        Format::Jpeg => jpeg::zapis(obraz, &jpeg::NastavenieJpeg::default()),
        Format::Tiff => tiff::zapis(obraz, &tiff::NastavenieTiff::default()),
        Format::Exr => exr::zapis(obraz, &exr::NastavenieExr::default()),
        Format::Jxl => Err(Chyba::NezhodaTypov {
            popis: "zápis JPEG XL zatiaľ nie je".into(),
            navrh: Some("uložiť ako PNG alebo OpenEXR".into()),
        }),
    }
}
