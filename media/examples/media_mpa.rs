//! Rozbor MPEG audio vrstvy III (L8, časť 1) – nástroj rozhodcu `media/nastroje/over_mpa.py`.
//!
//! Použitie: `media_mpa <súbor.mp3>` – preskočí ID3v2, nájde rámce podľa hlavičky a pre každý rámec vrstvy III
//! MPEG-1 dekóduje vedľajšie informácie, škálovacie faktory a Huffmanove kódy. Na stdout `MPA …` (spotreba bitov
//! a pokrytie) a `VYSLEDOK ramcov=<n>`.

use orchidea_media::mpa::{DekoderMpa, Hlavicka};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        eprintln!("použitie: media_mpa <súbor.mp3>");
        std::process::exit(2);
    }
    let d = std::fs::read(&a[1]).expect("súbor");
    let mut i = 0usize;
    // ID3v2: „ID3“, verzia, príznaky, veľkosť 4 × 7 bitov (+ 10 B hlavička, + 10 B pätička pri príznaku 0x10)
    if d.len() >= 10 && &d[..3] == b"ID3" {
        let n = d[6..10].iter().fold(0usize, |v, &b| (v << 7) | (b & 0x7F) as usize);
        i = 10 + n + if d[5] & 0x10 != 0 { 10 } else { 0 };
    }
    let mut dek = DekoderMpa::novy();
    let mut ramcov = 0;
    let mut chyb = 0;
    while i + 4 <= d.len() {
        match Hlavicka::z_bajtov(&d[i..]) {
            Some(h) if i + h.dlzka <= d.len() => {
                if dek.rozober_ramec(&d[i..i + h.dlzka]).is_err() {
                    chyb += 1;
                }
                ramcov += 1;
                i += h.dlzka;
            }
            _ => i += 1,
        }
    }
    let s = dek.statistika();
    println!(
        "MPA granul={} presne={} prebeh={} vypln={} vypln_rovnaka={} chyby={} chyba_skalovacich={} nedostatok_zasobnika={} \
         kratke={} zmiesane={} start_stop={} scfsi={} preflag={} scalefac_scale={} linbits={} tabulka_b={} ms={} \
         intenzita={}",
        s.granul,
        s.presne,
        s.prebeh,
        s.vypln,
        s.vypln_rovnaka,
        s.chyby,
        s.chyba_skalovacich,
        s.nedostatok_zasobnika,
        s.kratke,
        s.zmiesane,
        s.start_stop,
        s.scfsi,
        s.preflag,
        s.scalefac_scale,
        s.linbits,
        s.tabulka_b,
        s.ms,
        s.intenzita
    );
    println!("VYSLEDOK ramcov={ramcov} chyb={chyb}");
}
