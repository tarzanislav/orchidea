//! Dekódovanie MPEG audio vrstvy III (L8) – nástroj rozhodcu `media/nastroje/over_mpa.py`.
//!
//! Použitie: `media_mpa <súbor.mp3|.bit> [výstup.f32]` – preskočí ID3v2, nájde rámce podľa hlavičky a každý rámec
//! vrstvy III MPEG-1 dekóduje. S druhým argumentom zapíše zvuk ako prekladané f32 LE (všetky rámce, bez orezania
//! začiatku a konca – porovnáva sa so surovým dekódovaním rozhodcu). Na stdout `MPA …` (spotreba bitov
//! a pokrytie), `ZVUK kanalov= frekvencia= vzoriek=` a `VYSLEDOK ramcov=<n> chyb=<n>`.

use orchidea_media::mpa::{DekoderMpa, Hlavicka};
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        eprintln!("použitie: media_mpa <súbor.mp3> [výstup.f32]");
        std::process::exit(2);
    }
    let d = std::fs::read(&a[1]).expect("súbor");
    let mut von = a.get(2).map(|c| std::io::BufWriter::new(std::fs::File::create(c).expect("výstup")));
    let mut i = 0usize;
    // ID3v2: „ID3“, verzia, príznaky, veľkosť 4 × 7 bitov (+ 10 B hlavička, + 10 B pätička pri príznaku 0x10)
    if d.len() >= 10 && &d[..3] == b"ID3" {
        let n = d[6..10].iter().fold(0usize, |v, &b| (v << 7) | (b & 0x7F) as usize);
        i = 10 + n + if d[5] & 0x10 != 0 { 10 } else { 0 };
    }
    let mut dek = DekoderMpa::novy();
    let (mut ramcov, mut chyb, mut vzoriek, mut kanalov, mut frekvencia) = (0, 0, 0usize, 0, 0);
    while i + 4 <= d.len() {
        match Hlavicka::z_bajtov(&d[i..]) {
            Some(h) if i + h.dlzka <= d.len() => {
                match dek.dekoduj_ramec(&d[i..i + h.dlzka]) {
                    Ok(pcm) => {
                        kanalov = pcm.len();
                        frekvencia = h.frekvencia;
                        vzoriek += pcm[0].len();
                        if let Some(v) = von.as_mut() {
                            let mut b = Vec::with_capacity(pcm[0].len() * pcm.len() * 4);
                            for t in 0..pcm[0].len() {
                                for k in &pcm {
                                    b.extend_from_slice(&k[t].to_le_bytes());
                                }
                            }
                            v.write_all(&b).expect("zápis");
                        }
                    }
                    Err(_) => chyb += 1,
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
         intenzita={} intenzitne_pasma={} stereo_nezhoda={}",
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
        s.intenzita,
        s.intenzitne_pasma,
        s.stereo_nezhoda
    );
    println!("ZVUK kanalov={kanalov} frekvencia={frekvencia} vzoriek={vzoriek}");
    println!("VYSLEDOK ramcov={ramcov} chyb={chyb}");
}
