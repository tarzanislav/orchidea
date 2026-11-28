//! Návrh vzhľadu ORCHIDEA – obrázky na výber vlastníkom (nie je to ešte rozhranie programu; obrazovky sa
//! naprogramujú až po schválení). Kreslí ich ten istý kód dizajnového systému (`orchidea_vzhlad`), ktorý potom
//! použijú programy – čo sa schváli, to aj pobeží.
//!
//! `cargo run --release -p orchidea-vzhlad --example navrh -- <priečinok>` → `system/` (témy s kontrastom, prvky
//! v stavoch, ikony, písmo), `varianty/` (prehliadač a editor fotografií (Pohľad), prehrávač médií LÝRA,
//! obrazovka TV krabičky – každý v troch variantoch), `temy/` (každá téma na troch obrazovkách, výrez v 200 %).

// kreslenie návrhu: veľa súradníc v jednom volaní je tu čitateľnejšie ako štruktúry
#![allow(clippy::too_many_arguments)]

mod lyra;
mod obrazky;
mod pohlad;
mod system;
mod tv;

use orchidea_pismo::Platno;
use orchidea_vzhlad::kreslic::Kreslic;
use orchidea_vzhlad::tema::{LAN, NOC, TEMY};
use std::path::Path;

/// Meno súboru témy bez diakritiky.
fn subor_temy(meno: &str) -> String {
    meno.to_lowercase()
        .chars()
        .map(|c| match c {
            'ľ' => 'l',
            'ú' => 'u',
            'ŕ' => 'r',
            'á' => 'a',
            'ň' => 'n',
            c => c,
        })
        .collect()
}

fn uloz(p: &Platno, priecinok: &Path, meno: &str) {
    let cesta = priecinok.join(meno);
    std::fs::write(&cesta, p.png()).expect("zápis obrázka");
    println!("{} ({} × {})", cesta.display(), p.sirka, p.vyska);
}

/// Výrez plátna (v obrazových bodoch).
fn vyrez(k: &Kreslic, x: usize, y: usize, w: usize, h: usize) -> Platno {
    let mut p = Platno::nove(w, h, 0);
    for r in 0..h {
        let zdroj = (y + r) * k.p.sirka + x;
        p.body[r * w..(r + 1) * w].copy_from_slice(&k.p.body[zdroj..zdroj + w]);
    }
    p
}

fn main() {
    let koren = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "vzhlad/navrh".into()));
    let (sys, var, tem) = (koren.join("system"), koren.join("varianty"), koren.join("temy"));
    for p in [&sys, &var, &tem] {
        std::fs::create_dir_all(p).expect("priečinok");
    }
    let foto = obrazky::fotka(1200, 800);

    // dizajnový systém
    uloz(&system::temy(1.0).p, &sys, "temy.png");
    uloz(&system::prvky_list(&NOC, 1.0).p, &sys, "prvky_noc.png");
    uloz(&system::prvky_list(&LAN, 1.0).p, &sys, "prvky_lan.png");
    let k = system::prvky_list(&NOC, 2.0);
    uloz(&vyrez(&k, 0, 0, 1440, 900), &sys, "prvky_noc_200.png");
    uloz(&system::ikony(1.0).p, &sys, "ikony.png");
    uloz(&system::pismo(1.0).p, &sys, "pismo.png");

    // varianty obrazoviek v predvolenej téme
    for v in ['a', 'b', 'c'] {
        uloz(&pohlad::obrazovka(&NOC, v, 1.0, &foto).p, &var, &format!("pohlad_{v}.png"));
        uloz(&lyra::obrazovka(&NOC, v, 1.0, &foto).p, &var, &format!("lyra_{v}.png"));
        uloz(&tv::obrazovka(&NOC, v, 1.0, &foto).p, &var, &format!("tv_{v}.png"));
    }
    let k = tv::obrazovka(&NOC, 'a', 2.0, &foto);
    uloz(&vyrez(&k, 0, 400, 1920, 1080), &var, "tv_a_200.png");

    // každá téma na troch obrazovkách (variant A) a výrez Pohľadu v 200 %
    for t in TEMY {
        let s = subor_temy(t.meno);
        uloz(&pohlad::obrazovka(t, 'a', 1.0, &foto).p, &tem, &format!("{s}_pohlad.png"));
        uloz(&lyra::obrazovka(t, 'a', 1.0, &foto).p, &tem, &format!("{s}_lyra.png"));
        uloz(&tv::obrazovka(t, 'a', 1.0, &foto).p, &tem, &format!("{s}_tv.png"));
        let k = pohlad::obrazovka(t, 'a', 2.0, &foto);
        uloz(&vyrez(&k, k.p.sirka - 1440, 0, 1440, 900), &tem, &format!("{s}_pohlad_200.png"));
    }
}
