//! Dekódovanie MPEG audio vrstvy III (L8) – nástroj rozhodcu `media/nastroje/over_mpa.py`.
//!
//! Použitie: `media_mpa <súbor.mp3|.bit> [výstup.f32]` – preskočí ID3v2, nájde rámce podľa hlavičky a každý rámec
//! vrstvy III MPEG-1 dekóduje. S druhým argumentom zapíše zvuk ako prekladané f32 LE (všetky rámce, bez orezania
//! začiatku a konca – porovnáva sa so surovým dekódovaním rozhodcu). Na stdout `MPA …` (spotreba bitov
//! a pokrytie), `ZVUK kanalov= frekvencia= vzoriek=` a `VYSLEDOK ramcov=<n> chyb=<n>`.
//!
//! `media_mpa --kontajner <súbor> <výstup.f32>` – súbor (.mp3, MKV, MP4, TS, Ogg) otvorí `kontajner::otvor`, prvú
//! zvukovú stopu dekóduje cez `DekoderZvuku` (bezmedzerové orezanie podľa paketov) a zapíše prekladané f32 LE.

use orchidea_media::mpa::{DekoderMpa, Hlavicka};
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        eprintln!("použitie: media_mpa <súbor.mp3> [výstup.f32]");
        std::process::exit(2);
    }
    if a[1] == "--kontajner" {
        kontajner(&a[2], &a[3]);
        return;
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
    let mut volny: Option<usize> = None;
    while i + 4 <= d.len() {
        // voľný formát: dĺžka sa určí raz zo vzdialenosti hlavičiek, ďalej + výplňový bajt
        let dlzka = if Hlavicka::volny_format(&d[i..]) {
            if volny.is_none() {
                volny = Hlavicka::dlzka_volneho_formatu(&d[i..]);
            }
            volny.map(|v| v + ((d[i + 2] >> 1) & 1) as usize)
        } else {
            Hlavicka::z_bajtov(&d[i..]).map(|h| h.dlzka)
        };
        match dlzka.and_then(|n| Hlavicka::z_bajtov(&d[i..(i + n).min(d.len())]).map(|h| (h, n))) {
            Some((h, n)) if i + n <= d.len() && n >= 4 => {
                let h = Hlavicka { dlzka: n, ..h };
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

fn kontajner(cesta: &str, vystup: &str) {
    use orchidea_media::limity::Limity;
    use orchidea_media::zdroj::SuborovyZdroj;
    use orchidea_media::zvuk::{BlokZvuku, VzorkyZvuku, dekoder_zvuku};
    let mut k =
        orchidea_media::kontajner::otvor(SuborovyZdroj::otvor(cesta).expect("súbor"), Limity::default())
            .expect("kontajner");
    let (index, stopa) = k.stopy().iter().enumerate().find(|(_, s)| s.zvuk.is_some()).expect("zvuková stopa");
    let stopa = stopa.clone();
    let mut dek = dekoder_zvuku(&stopa).expect("dekodér");
    let p = *dek.parametre();
    let mut von = std::io::BufWriter::new(std::fs::File::create(vystup).expect("výstup"));
    let mut blok = BlokZvuku::f32(p.frekvencia, p.rozlozenie);
    let (mut paketov, mut vzoriek, mut chyb) = (0u64, 0u64, 0u64);
    while let Some(paket) = k.dalsi_paket().expect("paket") {
        if paket.stopa != index {
            continue;
        }
        paketov += 1;
        if dek.dekoduj(&paket, &mut blok).is_err() {
            chyb += 1;
            continue;
        }
        // celé čísla (PCM, FLAC) sa prevedú na ±1,0 podľa bitovej hĺbky (x / 2^(bitov − 1))
        let d: Vec<f32> = match &blok.vzorky {
            VzorkyZvuku::F32(d) => d.clone(),
            VzorkyZvuku::Cele { bitov, data } => {
                let m = 1.0 / (1u64 << (bitov.saturating_sub(1)).min(62)) as f64;
                data.iter().map(|&x| (x as f64 * m) as f32).collect()
            }
        };
        let n = blok.ramcov;
        let kanalov = d.len() / n.max(1);
        let mut b = Vec::with_capacity(d.len() * 4);
        for t in 0..n {
            for c in 0..kanalov {
                b.extend_from_slice(&d[c * n + t].to_le_bytes());
            }
        }
        von.write_all(&b).expect("zápis");
        vzoriek += n as u64;
    }
    println!(
        "KONTAJNER format={} kodek={} paketov={paketov} vzoriek={vzoriek} chyb={chyb} trvanie_ns={}",
        k.format().meno(),
        stopa.kodek.meno(),
        k.trvanie_ns().unwrap_or(-1)
    );
}
