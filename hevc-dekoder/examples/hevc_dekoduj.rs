//! Dekódovanie tokov HEVC (Annex B) vlastným softvérovým dekodérom a overenie **bitovej zhody**: odtlačok MD5
//! každého obrazu zo SEI (decoded picture hash) a – ak je pri toku súbor `*.md5` s MD5 celého výstupu YUV
//! (skúšobné toky normy) – aj MD5 orezaného výstupu v poradí zobrazenia.
//!
//! Spustenie: `cargo run --release -p orchidea-hevc-dekoder --example hevc_dekoduj -- <súbor alebo priečinok>…`
//! Premenné: `ORCHIDEA_HEVC_YUV=<priečinok>` zapíše výstup YUV, `ORCHIDEA_HEVC_PRISNE=1` vráti chybu pri
//! akejkoľvek nezhode (CI).

use orchidea_hevc::jednotky_annex_b;
use orchidea_hevc::odtlacok::{Md5, hex};
use orchidea_hevc_dekoder::Dekoder;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn subory(cesta: &Path, v: &mut Vec<PathBuf>) {
    if cesta.is_dir() {
        let mut polozky: Vec<_> =
            std::fs::read_dir(cesta).into_iter().flatten().flatten().map(|e| e.path()).collect();
        polozky.sort();
        for p in polozky {
            subory(&p, v);
        }
    } else if let Some(p) = cesta.extension().and_then(|e| e.to_str())
        && matches!(p.to_ascii_lowercase().as_str(), "bit" | "bin" | "hevc" | "h265" | "265")
    {
        v.push(cesta.to_path_buf());
    }
}

/// MD5 celého výstupu YUV zo súboru pri toku (skúšobné toky normy: `<meno>.md5`, `<meno>_yuv.md5`, `*md5*.txt`).
fn ocakavane_md5_yuv(tok: &Path) -> Option<String> {
    let priecinok = tok.parent()?;
    let mut kandidati: Vec<PathBuf> = std::fs::read_dir(priecinok)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let m = p.file_name().and_then(|m| m.to_str()).unwrap_or("").to_ascii_lowercase();
            m.contains("md5") && !m.ends_with(".bit") && !m.ends_with(".bin")
        })
        .collect();
    kandidati.sort();
    for k in kandidati {
        let Ok(t) = std::fs::read_to_string(&k) else { continue };
        // len súbor s jediným odtlačkom (MD5 celého výstupu); zoznamy MD5 po snímkach sa nepoužijú
        let h: Vec<&str> = t.split(|c: char| !c.is_ascii_hexdigit()).filter(|s| s.len() == 32).collect();
        if h.len() == 1 {
            return Some(h[0].to_ascii_lowercase());
        }
    }
    None
}

fn main() {
    let mut zoznam = Vec::new();
    for a in std::env::args().skip(1) {
        subory(Path::new(&a), &mut zoznam);
    }
    if zoznam.is_empty() {
        println!("Žiadne toky HEVC na dekódovanie.");
        return;
    }
    let yuv_priecinok = std::env::var_os("ORCHIDEA_HEVC_YUV").map(PathBuf::from);
    let prisne = std::env::var_os("ORCHIDEA_HEVC_PRISNE").is_some();
    println!("| Tok | obrazov | MD5 SEI zhoda | nezhoda | bez SEI | výstup YUV | obr./s | poznámka |");
    println!("|---|---|---|---|---|---|---|---|");
    let (mut presnych, mut zlych, mut mimo) = (0usize, 0usize, 0usize);
    for cesta in &zoznam {
        let Ok(data) = std::fs::read(cesta) else { continue };
        let meno = cesta.file_name().map_or_else(String::new, |m| m.to_string_lossy().into_owned());
        let mut d = Dekoder::novy();
        let mut md5 = Md5::novy();
        let mut yuv = Vec::new();
        let mut vydanych = 0usize;
        let start = Instant::now();
        let mut spracuj = |obrazy: Vec<std::sync::Arc<orchidea_hevc_dekoder::DekodovanyObraz>>| {
            for o in obrazy {
                yuv.clear();
                o.zapis_yuv(&mut yuv);
                md5.pridaj(&yuv);
                vydanych += 1;
            }
        };
        let mut cely_yuv = Vec::new();
        for j in jednotky_annex_b(&data) {
            let o = d.nal(j);
            if yuv_priecinok.is_some() {
                for x in &o {
                    x.zapis_yuv(&mut cely_yuv);
                }
            }
            spracuj(o);
        }
        let o = d.koniec();
        if yuv_priecinok.is_some() {
            for x in &o {
                x.zapis_yuv(&mut cely_yuv);
            }
        }
        spracuj(o);
        let cas = start.elapsed().as_secs_f64();
        let s = &d.statistika;
        let vystup_md5 = hex(&md5.vysledok());
        let yuv_stav = match ocakavane_md5_yuv(cesta) {
            Some(ocak) if ocak == vystup_md5 => "zhoda".to_string(),
            Some(_) => format!("NEZHODA ({vydanych} obr.)"),
            None => format!("{vydanych} obr."),
        };
        if let Some(p) = &yuv_priecinok {
            let _ = std::fs::create_dir_all(p);
            let _ = std::fs::write(p.join(format!("{meno}.yuv")), &cely_yuv);
        }
        let mimo_profilu = s.chyby.iter().any(|c| c.contains("mimo profilu"));
        let mut poznamka: Vec<String> = s.chyby.iter().take(2).cloned().collect();
        if !s.nezhoda.is_empty() {
            poznamka.push(format!("prvá nezhoda: obraz {} (POC {})", s.nezhoda[0].0, s.nezhoda[0].1));
        }
        if s.preskocene > 0 {
            poznamka.push(format!("preskočené RASL: {}", s.preskocene));
        }
        let presny = s.nezhoda.is_empty() && s.chyby.is_empty() && !yuv_stav.starts_with("NEZHODA");
        if mimo_profilu {
            mimo += 1;
        } else if presny {
            presnych += 1;
        } else {
            zlych += 1;
        }
        println!(
            "| {meno} | {} | {} | {} | {} | {yuv_stav} | {:.1} | {} |",
            s.obrazov,
            s.zhoda,
            s.nezhoda.len(),
            s.bez_odtlacku,
            s.obrazov as f64 / cas.max(1e-9),
            if poznamka.is_empty() { "–".into() } else { poznamka.join("; ") }
        );
    }
    println!();
    println!(
        "**Tokov: {}, bitovo presných: {presnych}, s chybou: {zlych}, mimo profilu Main 8-bit: {mimo}**",
        zoznam.len()
    );
    if prisne && zlych > 0 {
        std::process::exit(1);
    }
}
