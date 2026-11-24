//! Skúšobné súbory cudzích kodérov (`tests/data`): naše čítanie musí dať presne to isté, čo referenčná knižnica –
//! kontrolný súčet CRC-32 vzoriek (little-endian) je spočítaný z jej výstupu.
//!
//! Súbory sú vložené do programu skúšok (`include_bytes!`), takže skúšky bežia rovnako vo WebAssembly,
//! na Androide aj na doske bez prístupu k zdrojovým súborom.

use orchidea_formaty::kontrola::crc32;
use orchidea_formaty::obraz::{Limity, Vzorky};

fn bajty(v: &Vzorky) -> Vec<u8> {
    match v {
        Vzorky::U8(x) => x.clone(),
        Vzorky::U16(x) => x.iter().flat_map(|a| a.to_le_bytes()).collect(),
        Vzorky::F32(x) => x.iter().flat_map(|a| a.to_le_bytes()).collect(),
    }
}

macro_rules! subor {
    ($m:literal, $w:expr, $h:expr, $k:expr, $crc:expr) => {
        ($m, include_bytes!(concat!("data/", $m)).as_slice(), $w, $h, $k, $crc)
    };
}

/// Súbor, údaje, šírka, výška, kanály, CRC-32 vzoriek z referenčnej knižnice.
pub type Subor = (&'static str, &'static [u8], usize, usize, usize, u32);

pub const SUBORY: [Subor; 7] = [
    subor!("bezstratovy_f32.jxl", 16, 12, 3, 0x9006_90C6),
    subor!("bezstratovy_paleta.jxl", 32, 24, 3, 0x69C4_C6AE),
    subor!("bezstratovy_rgb_e1.jxl", 32, 24, 3, 0x9ED3_EC4C),
    subor!("bezstratovy_rgb_e2.jxl", 32, 24, 3, 0x9ED3_EC4C),
    subor!("bezstratovy_rgb_e9.jxl", 32, 24, 3, 0x9ED3_EC4C),
    subor!("bezstratovy_rgba16.jxl", 32, 24, 4, 0x83B7_1D9E),
    subor!("bezstratovy_sivy.jxl", 32, 24, 1, 0xA061_79BA),
];

#[test]
fn rovnako_ako_referencna_kniznica() {
    for (meno, data, w, h, k, crc) in SUBORY {
        let o = orchidea_formaty::citaj(data, &Limity::default()).unwrap_or_else(|e| panic!("{meno}: {e}"));
        assert_eq!((o.sirka, o.vyska, o.kanaly()), (w, h, k), "{meno}");
        assert_eq!(crc32(&bajty(&o.vzorky)), crc, "{meno}: vzorky sa líšia od referenčnej knižnice");
    }
}
