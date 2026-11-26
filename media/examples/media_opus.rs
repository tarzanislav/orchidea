//! Dekódovanie stopy Opus vlastným dekodérom (etapa L7) – na porovnanie s rozhodcom (libopus cez `over_opus.py`).
//!
//! Použitie: `media_opus <súbor> <výstup.f32>` – prvá stopa Opus kontajnera; na stdout pre každý paket
//! `PAKET index=<i> rng=<8 hex> vzoriek=<n> frekvencia=<Hz>` a nakoniec `VYSLEDOK paketov=<n> kanalov=<n>`;
//! do súboru vzorky `f32` malokoncovo, kanály prekladané. Návratový kód 0 v poriadku, 1 chyba.

use orchidea_media::opus::DekoderOpus;
use orchidea_media::{Kodek, Limity, SuborovyZdroj, otvor};
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cesta), Some(von)) = (a.first(), a.get(1)) else {
        eprintln!("použitie: media_opus <súbor> <výstup.f32>");
        std::process::exit(2);
    };
    let chyba = |e: &dyn std::fmt::Display| -> ! {
        println!("CHYBA {e}");
        std::process::exit(1)
    };
    let zdroj = SuborovyZdroj::otvor(cesta).unwrap_or_else(|e| chyba(&e));
    let mut k = otvor(zdroj, Limity::default()).unwrap_or_else(|e| chyba(&e));
    let ix =
        k.stopy().iter().position(|s| s.kodek == Kodek::Opus).unwrap_or_else(|| chyba(&"bez stopy Opus"));
    let mut d = match k.stopy()[ix].zvuk {
        Some(z) if z.kanalov > 0 => DekoderOpus::s_kanalmi(z.kanalov.min(2) as usize),
        _ => DekoderOpus::novy(),
    };
    let mut f = std::io::BufWriter::new(std::fs::File::create(von).expect("výstup"));
    let (mut n, mut kanalov) = (0usize, 0usize);
    while let Some(p) = k.dalsi_paket().unwrap_or_else(|e| chyba(&e)) {
        if p.stopa != ix {
            continue;
        }
        let v = d.dekoduj_paket(&p.data).unwrap_or_else(|e| chyba(&e));
        let vzoriek = v.kanaly.first().map_or(0, |x| x.len());
        kanalov = v.kanaly.len();
        println!("PAKET index={n} rng={:08x} vzoriek={vzoriek} frekvencia={}", v.konecny_rng, v.frekvencia);
        for i in 0..vzoriek {
            for c in &v.kanaly {
                f.write_all(&c[i].to_le_bytes()).expect("zápis");
            }
        }
        n += 1;
    }
    f.flush().expect("zápis");
    let s = d.statistika_celt();
    if s.ramcov > 0 {
        println!(
            "CELT ramcov={} ticho={} prechodovych={} postfilter={} intra={} posilnenie={} preskocene={} intenzita={} \
             dual={} anti_kolaps={} tf_zmena={}",
            s.ramcov,
            s.ticho,
            s.prechodovych,
            s.postfilter,
            s.intra,
            s.posilnenie,
            s.preskocene,
            s.intenzita,
            s.dual,
            s.anti_kolaps,
            s.tf_zmena
        );
    }
    println!("VYSLEDOK paketov={n} kanalov={kanalov}");
}
