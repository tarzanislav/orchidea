//! Skúšobné súbory cudzích kodérov (`tests/data`): naše čítanie musí dať presne to isté, čo referenčná knižnica –
//! kontrolný súčet CRC-32 vzoriek (little-endian) je spočítaný z jej výstupu.

use orchidea_formaty::kontrola::crc32;
use orchidea_formaty::obraz::{Limity, Vzorky};

fn bajty(v: &Vzorky) -> Vec<u8> {
    match v {
        Vzorky::U8(x) => x.clone(),
        Vzorky::U16(x) => x.iter().flat_map(|a| a.to_le_bytes()).collect(),
        Vzorky::F32(x) => x.iter().flat_map(|a| a.to_le_bytes()).collect(),
    }
}

/// (súbor, šírka, výška, kanály, CRC-32 vzoriek z libjxl)
const SUBORY: [(&str, usize, usize, usize, u32); 7] = [
    ("bezstratovy_f32.jxl", 16, 12, 3, 0x9006_90C6),
    ("bezstratovy_paleta.jxl", 32, 24, 3, 0x69C4_C6AE),
    ("bezstratovy_rgb_e1.jxl", 32, 24, 3, 0x9ED3_EC4C),
    ("bezstratovy_rgb_e2.jxl", 32, 24, 3, 0x9ED3_EC4C),
    ("bezstratovy_rgb_e9.jxl", 32, 24, 3, 0x9ED3_EC4C),
    ("bezstratovy_rgba16.jxl", 32, 24, 4, 0x83B7_1D9E),
    ("bezstratovy_sivy.jxl", 32, 24, 1, 0xA061_79BA),
];

pub fn nacitaj(meno: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(meno)).unwrap()
}

#[test]
fn jpeg_xl_rovnako_ako_libjxl() {
    for (meno, w, h, k, crc) in SUBORY {
        let o = orchidea_formaty::citaj(&nacitaj(meno), &Limity::default())
            .unwrap_or_else(|e| panic!("{meno}: {e}"));
        assert_eq!((o.sirka, o.vyska, o.kanaly()), (w, h, k), "{meno}");
        assert_eq!(crc32(&bajty(&o.vzorky)), crc, "{meno}: vzorky sa líšia od libjxl");
    }
}
