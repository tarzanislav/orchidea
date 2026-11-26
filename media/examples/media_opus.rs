//! Dekódovanie stopy Opus vlastným dekodérom (etapa L7) – na porovnanie s rozhodcom (libopus cez `over_opus.py`).
//!
//! Použitie: `media_opus <súbor> <výstup.f32>` – prvá stopa Opus kontajnera alebo surové pakety `.opuspak`
//! (dĺžka u16 LE + bajty; `--kanaly=N` – počet výstupných kanálov); na stdout pre každý paket
//! `PAKET index=<i> rng=<8 hex> vzoriek=<n> frekvencia=<Hz>`, pokrytie (`CELT …`, `REZIMY …`) a nakoniec
//! `VYSLEDOK paketov=<n> kanalov=<n>`; `--vnutorna` – SILK-only pri vnútornej frekvencii (8–16 kHz);
//! do súboru vzorky `f32` malokoncovo, kanály prekladané. Návratový kód 0 v poriadku, 1 chyba.

use orchidea_media::opus::DekoderOpus;
use orchidea_media::opus::dekoder::OpusDekoder;
use orchidea_media::zvuk::{BlokZvuku, DekoderZvuku, VzorkyZvuku};
use orchidea_media::{Kodek, Limity, SuborovyZdroj, otvor};
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cesta), Some(von)) = (a.first(), a.get(1)) else {
        eprintln!("použitie: media_opus <súbor> <výstup.f32> [--vnutorna | --stopa | --kanaly=N]");
        std::process::exit(2);
    };
    let chyba = |e: &dyn std::fmt::Display| -> ! {
        println!("CHYBA {e}");
        std::process::exit(1)
    };
    if a.iter().any(|x| x == "--stopa") {
        stopa(cesta, von);
        return;
    }
    // vstup: kontajner (prvá stopa Opus) alebo surové pakety `.opuspak` (opakovane dĺžka u16 LE + bajty paketu)
    let mut pakety: Box<dyn Iterator<Item = Vec<u8>>>;
    let mut d;
    if cesta.ends_with(".opuspak") {
        let b = std::fs::read(cesta).unwrap_or_else(|e| chyba(&e));
        let mut v = Vec::new();
        let mut i = 0;
        while i + 2 <= b.len() {
            let n = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
            v.push(b.get(i + 2..i + 2 + n).unwrap_or_else(|| chyba(&"skrátený .opuspak")).to_vec());
            i += 2 + n;
        }
        let kanalov = match a.iter().find_map(|x| x.strip_prefix("--kanaly=")) {
            Some(k) => k.parse().unwrap_or(2),
            None => v.first().and_then(|p| p.first()).map_or(1, |&t| 1 + ((t >> 2) & 1) as usize),
        };
        d = DekoderOpus::s_kanalmi(kanalov);
        pakety = Box::new(v.into_iter());
    } else {
        let zdroj = SuborovyZdroj::otvor(cesta).unwrap_or_else(|e| chyba(&e));
        let mut k = otvor(zdroj, Limity::default()).unwrap_or_else(|e| chyba(&e));
        let ix =
            k.stopy().iter().position(|s| s.kodek == Kodek::Opus).unwrap_or_else(|| chyba(&"bez stopy Opus"));
        d = match k.stopy()[ix].zvuk {
            Some(z) if z.kanalov > 0 => DekoderOpus::s_kanalmi(z.kanalov.min(2) as usize),
            _ => DekoderOpus::novy(),
        };
        pakety = Box::new(std::iter::from_fn(move || {
            loop {
                let p = k.dalsi_paket().unwrap_or_else(|e| chyba(&e))?;
                if p.stopa == ix {
                    return Some(p.data.to_vec());
                }
            }
        }));
    }
    if a.iter().any(|x| x == "--vnutorna") {
        d = d.vnutorna_frekvencia_silk(); // SILK-only pri vnútornej frekvencii (porovnanie s libopus pri 8–16 kHz)
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(von).expect("výstup"));
    let (mut n, mut kanalov) = (0usize, 0usize);
    for p in pakety.by_ref() {
        let v = d.dekoduj_paket(&p).unwrap_or_else(|e| chyba(&e));
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
    let o = d.statistika();
    println!(
        "REZIMY silk={} hybrid={} celt={} nadbytocne_zaciatok={} nadbytocne_koniec={} prechody_bez={} dobeh_hybridu={}",
        o.silk, o.hybrid, o.celt, o.nadbytocne_zaciatok, o.nadbytocne_koniec, o.prechody_bez, o.dobeh_hybridu
    );
    println!("VYSLEDOK paketov={n} kanalov={kanalov}");
}

/// `--stopa`: dekodér stopy (`DekoderZvuku` – hlavička, viac prúdov, pre-skip, orezanie, poradie kanálov);
/// výstup v spoločnom poradí kanálov, prekladane.
fn stopa(cesta: &str, von: &str) {
    fn chyba(e: &dyn std::fmt::Display) -> ! {
        println!("CHYBA {e}");
        std::process::exit(1)
    }
    let zdroj = SuborovyZdroj::otvor(cesta).unwrap_or_else(|e| chyba(&e));
    let mut k = otvor(zdroj, Limity::default()).unwrap_or_else(|e| chyba(&e));
    let ix =
        k.stopy().iter().position(|s| s.kodek == Kodek::Opus).unwrap_or_else(|| chyba(&"bez stopy Opus"));
    let s = k.stopy()[ix].clone();
    let mut d = OpusDekoder::novy(&s.konfiguracia, s.zvuk).unwrap_or_else(|e| chyba(&e));
    let h = d.hlavicka().clone();
    let mapovanie: Vec<String> = h.mapovanie.iter().map(|m| m.to_string()).collect();
    println!(
        "HLAVICKA kanalov={} predskok={} prudov={} spojenych={} zisk={} rodina={} mapovanie={} rozlozenie={:#x}",
        h.kanalov,
        h.predskok,
        h.prudov,
        h.spojenych,
        h.zisk_q8,
        h.rodina,
        mapovanie.join(","),
        d.parametre().rozlozenie.0
    );
    let mut f = std::io::BufWriter::new(std::fs::File::create(von).expect("výstup"));
    let mut blok = BlokZvuku::f32(48_000, d.parametre().rozlozenie);
    let (mut n, mut ramcov) = (0usize, 0usize);
    while let Some(p) = k.dalsi_paket().unwrap_or_else(|e| chyba(&e)) {
        if p.stopa != ix {
            continue;
        }
        d.dekoduj(&p, &mut blok).unwrap_or_else(|e| chyba(&e));
        println!("PAKET index={n} rng={:08x} vzoriek={} frekvencia=48000", d.konecny_rng(), blok.ramcov);
        if let VzorkyZvuku::F32(v) = &blok.vzorky {
            let c = blok.kanalov();
            for i in 0..blok.ramcov {
                for k in 0..c {
                    f.write_all(&v[k * blok.ramcov + i].to_le_bytes()).expect("zápis");
                }
            }
        }
        ramcov += blok.ramcov;
        n += 1;
    }
    f.flush().expect("zápis");
    println!("VYSLEDOK paketov={n} kanalov={} ramcov={ramcov}", h.kanalov);
}
