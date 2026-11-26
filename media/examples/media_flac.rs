//! Dôkaz bitovej zhody bezstratového zvuku (etapa L2): dekóduje celý súbor (FLAC, alebo Matroska s FLAC/PCM),
//! spočíta MD5 dekódovaného zvuku podľa RFC 9639, kap. 8.2 (vzorky prekladané, malokoncové, ⌈bitov/8⌉ bajtov
//! na vzorku podľa hĺbky rámca, so znamienkom) a porovná ho s MD5 v STREAMINFO.
//!
//! Použitie: `media_flac [--surove <výstup.raw>] <súbor>` – na stdout jeden riadok:
//! ```text
//! FLAC subor=<meno> ramcov=<n> vzoriek=<na kanál> kanalov=<k> bitov=<b> md5=<32 hex> streaminfo=<32 hex|-> cas_ms=<ms> x_realny_cas=<x> stav=zhoda|nezhoda|bez_md5|chyba
//! ```
//! `--surove` zapíše dekódované vzorky v rovnakom tvare (pre porovnanie s iným dekodérom). Návratový kód: 0 zhoda
//! alebo bez MD5, 1 nezhoda alebo chyba, 2 zlé argumenty.

use orchidea_hevc::odtlacok::{Md5, hex};
use orchidea_media::{
    BlokZvuku, DruhStopy, Limity, RozlozenieKanalov, SuborovyZdroj, VzorkyZvuku, dekoder_zvuku, otvor,
};
use std::io::Write;
use std::time::Instant;

fn main() {
    let mut argumenty: Vec<String> = std::env::args().skip(1).collect();
    let surove = match argumenty.iter().position(|a| a == "--surove") {
        Some(i) if i + 1 < argumenty.len() => {
            let c = argumenty.remove(i + 1);
            argumenty.remove(i);
            Some(c)
        }
        _ => None,
    };
    let Some(cesta) = argumenty.first() else {
        eprintln!("použitie: media_flac [--surove <výstup.raw>] <súbor>");
        std::process::exit(2);
    };
    let meno = std::path::Path::new(cesta)
        .file_name()
        .map(|m| m.to_string_lossy().replace(' ', "_"))
        .unwrap_or_default();
    let koniec = |text: String, kod: i32| -> ! {
        println!("FLAC subor={meno} {text}");
        std::process::exit(kod)
    };
    let zdroj = SuborovyZdroj::otvor(cesta).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let mut k =
        otvor(zdroj, Limity::default()).unwrap_or_else(|e| koniec(format!("stav=chyba chyba=\"{e}\""), 1));
    let Some(ix) = k.stopy().iter().position(|s| s.druh == DruhStopy::Zvuk) else {
        koniec("stav=chyba chyba=\"bez zvukovej stopy\"".into(), 1)
    };
    let stopa = k.stopy()[ix].clone();
    let streaminfo = orchidea_media::flac::StreamInfo::z_konfiguracie(&stopa.konfiguracia).ok();
    let mut d = dekoder_zvuku(&stopa).unwrap_or_else(|e| koniec(format!("stav=chyba chyba=\"{e}\""), 1));
    let mut vystup =
        surove.map(|c| std::io::BufWriter::new(std::fs::File::create(c).expect("výstupný súbor")));
    let mut md5 = Md5::novy();
    let mut blok = BlokZvuku::cele(0, RozlozenieKanalov::STEREO, 16);
    let (mut ramcov, mut vzoriek, mut kanalov, mut bitov) = (0u64, 0u64, 0usize, 0u8);
    let mut riadok = Vec::new();
    let start = Instant::now();
    loop {
        let paket = match k.dalsi_paket() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => koniec(format!("ramcov={ramcov} stav=chyba chyba=\"{e}\""), 1),
        };
        if paket.stopa != ix {
            continue;
        }
        if let Err(e) = d.dekoduj(&paket, &mut blok) {
            koniec(format!("ramcov={ramcov} stav=chyba chyba=\"{e}\""), 1);
        }
        ramcov += 1;
        let VzorkyZvuku::Cele { bitov: b, data } = &blok.vzorky else {
            koniec("stav=chyba chyba=\"stratový kodek nemá MD5\"".into(), 1)
        };
        (kanalov, bitov) = (blok.kanalov(), *b);
        let n = blok.ramcov;
        // šírka vzorky podľa bitovej hĺbky rámca (STREAMINFO môže byť zlé alebo sa hĺbka mení)
        let bajtov_vzorky = bitov.div_ceil(8) as usize;
        riadok.clear();
        for i in 0..n {
            for c in 0..kanalov {
                riadok.extend_from_slice(&data[c * n + i].to_le_bytes()[..bajtov_vzorky]);
            }
        }
        md5.pridaj(&riadok);
        if let Some(v) = &mut vystup {
            v.write_all(&riadok).expect("zápis");
        }
        vzoriek += n as u64;
    }
    if let Some(mut v) = vystup {
        v.flush().expect("zápis"); // process::exit nevyprázdni vyrovnávaciu pamäť sám
    }
    let cas = start.elapsed().as_secs_f64();
    let nase = md5.vysledok();
    let frekvencia = stopa.zvuk.map(|z| z.frekvencia).unwrap_or(0).max(1);
    let x = vzoriek as f64 / frekvencia as f64 / cas.max(1e-9);
    let (ocak, stav, kod) = match streaminfo.map(|s| s.md5) {
        Some(m) if m != [0; 16] => (hex(&m), if m == nase { "zhoda" } else { "nezhoda" }, (m != nase) as i32),
        _ => ("-".into(), "bez_md5", 0),
    };
    koniec(
        format!(
            "ramcov={ramcov} vzoriek={vzoriek} kanalov={kanalov} bitov={bitov} md5={} streaminfo={ocak} cas_ms={:.1} x_realny_cas={x:.0} stav={stav}",
            hex(&nase),
            cas * 1000.0
        ),
        kod,
    )
}
