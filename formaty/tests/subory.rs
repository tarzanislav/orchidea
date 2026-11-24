//! Skúšobné súbory cudzích kodérov (`tests/data`): bezstratové naše čítanie musí dať presne to isté, čo
//! referenčná knižnica – kontrolný súčet CRC-32 vzoriek (little-endian) je spočítaný z jej výstupu. Stratové
//! súbory sa porovnávajú s výstupom referenčnej knižnice uloženým v PNG (odchýlka najviac 1 – zaokrúhlenie).
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

pub const SUBORY: [Subor; 8] = [
    subor!("bezstratovy_f32.jxl", 16, 12, 3, 0x9006_90C6),
    subor!("bezstratovy_paleta.jxl", 32, 24, 3, 0x69C4_C6AE),
    subor!("bezstratovy_rgb_e1.jxl", 32, 24, 3, 0x9ED3_EC4C),
    subor!("bezstratovy_rgb_e2.jxl", 32, 24, 3, 0x9ED3_EC4C),
    subor!("bezstratovy_rgb_e9.jxl", 32, 24, 3, 0x9ED3_EC4C),
    subor!("bezstratovy_rgba16.jxl", 32, 24, 4, 0x83B7_1D9E),
    subor!("bezstratovy_sivy.jxl", 32, 24, 1, 0xA061_79BA),
    subor!("bezstratovy_zaplaty.jxl", 160, 120, 3, 0x0B5A_127E),
];

#[test]
fn rovnako_ako_referencna_kniznica() {
    for (meno, data, w, h, k, crc) in SUBORY {
        let o = orchidea_formaty::citaj(data, &Limity::default()).unwrap_or_else(|e| panic!("{meno}: {e}"));
        assert_eq!((o.sirka, o.vyska, o.kanaly()), (w, h, k), "{meno}");
        assert_eq!(crc32(&bajty(&o.vzorky)), crc, "{meno}: vzorky sa líšia od referenčnej knižnice");
    }
}

macro_rules! stratovy {
    ($m:literal) => {
        (
            $m,
            include_bytes!(concat!("data/", $m, ".jxl")).as_slice(),
            include_bytes!(concat!("data/", $m, ".png")).as_slice(),
        )
    };
}

/// Stratový súbor JPEG XL (VarDCT) a výstup libjxl v PNG.
pub const STRATOVE: [(&str, &[u8], &[u8]); 7] = [
    stratovy!("stratovy_d1"),
    stratovy!("stratovy_d2"),
    stratovy!("stratovy_d5"),
    stratovy!("stratovy_d12"),
    stratovy!("stratovy_rgba"),
    stratovy!("stratovy_zaplaty"),
    stratovy!("stratovy_jpeg420"),
];

#[test]
fn stratove_ako_referencna_kniznica() {
    for (meno, jxl, png) in STRATOVE {
        let o = orchidea_formaty::citaj(jxl, &Limity::default()).unwrap_or_else(|e| panic!("{meno}: {e}"));
        let r =
            orchidea_formaty::citaj(png, &Limity::default()).unwrap_or_else(|e| panic!("{meno}.png: {e}"));
        assert_eq!((o.sirka, o.vyska, o.kanaly()), (r.sirka, r.vyska, r.kanaly()), "{meno}");
        let (Vzorky::U8(a), Vzorky::U8(b)) = (&o.vzorky, &r.vzorky) else {
            panic!("{meno}: čakané 8-bitové vzorky")
        };
        let max = a.iter().zip(b).map(|(&x, &y)| x.abs_diff(y)).max().unwrap_or(0);
        assert!(max <= 1, "{meno}: odchýlka {max} od libjxl");
    }
}
