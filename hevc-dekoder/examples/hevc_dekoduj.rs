//! Dekódovanie tokov HEVC (Annex B) vlastným softvérovým dekodérom a overenie **bitovej zhody**: odtlačok MD5
//! každého obrazu zo SEI (decoded picture hash) a – ak je pri toku súbor `*.md5` s MD5 celého výstupu YUV
//! (skúšobné toky normy) – aj MD5 orezaného výstupu v poradí zobrazenia.
//!
//! Spustenie: `cargo run --release -p orchidea-hevc-dekoder --example hevc_dekoduj -- <súbor alebo priečinok>…`
//! Premenné: `ORCHIDEA_HEVC_YUV=<priečinok>` zapíše výstup YUV, `ORCHIDEA_HEVC_PRISNE=1` vráti chybu pri
//! akejkoľvek nezhode (CI), `ORCHIDEA_HEVC_RYCHLOST=1` meria čisté dekódovanie (bez overovania MD5) – ako
//! pri prehrávaní.

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

/// Odtlačky MD5 zo súborov pri toku (skúšobné toky normy: `<meno>.md5`, `<meno>_yuv.md5`, `*md5*.txt`) – každý
/// súbor s jediným odtlačkom (MD5 celého výstupu YUV alebo samotného toku); zoznamy po snímkach sa nepoužijú.
fn odtlacky_pri_toku(tok: &Path) -> Vec<(String, String)> {
    let Some(priecinok) = tok.parent() else { return Vec::new() };
    let mut kandidati: Vec<PathBuf> = std::fs::read_dir(priecinok)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let m = p.file_name().and_then(|m| m.to_str()).unwrap_or("").to_ascii_lowercase();
            m.contains("md5") && !m.ends_with(".bit") && !m.ends_with(".bin")
        })
        .collect();
    kandidati.sort();
    let mut v = Vec::new();
    for k in kandidati {
        let Ok(t) = std::fs::read_to_string(&k) else { continue };
        let h: Vec<&str> = t.split(|c: char| !c.is_ascii_hexdigit()).filter(|s| s.len() == 32).collect();
        if h.len() == 1 {
            let meno = k.file_name().map_or_else(String::new, |m| m.to_string_lossy().into_owned());
            v.push((h[0].to_ascii_lowercase(), meno));
        }
    }
    v
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
    let rychlost = std::env::var_os("ORCHIDEA_HEVC_RYCHLOST").is_some();
    println!("| Tok | obrazov | MD5 SEI zhoda | nezhoda | bez SEI | výstup YUV | obr./s | poznámka |");
    println!("|---|---|---|---|---|---|---|---|");
    let (mut presnych, mut zlych, mut mimo, mut neoverenych) = (0usize, 0usize, 0usize, 0usize);
    for cesta in &zoznam {
        let Ok(data) = std::fs::read(cesta) else { continue };
        let meno = cesta.file_name().map_or_else(String::new, |m| m.to_string_lossy().into_owned());
        let mut d = if rychlost { Dekoder::na_prehravanie() } else { Dekoder::novy() };
        let mut md5 = Md5::novy();
        // varianty pre diagnostiku nezhody celého výstupu: 16-bitové vzorky a obraz bez orezania
        let mut md5_16 = Md5::novy();
        let mut md5_cely = Md5::novy();
        let ocakavane = odtlacky_pri_toku(cesta);
        let diagnostika = !ocakavane.is_empty() && !rychlost;
        let mut yuv = Vec::new();
        let mut vydanych = 0usize;
        let start = Instant::now();
        let mut spracuj = |obrazy: Vec<std::sync::Arc<orchidea_hevc_dekoder::DekodovanyObraz>>| {
            for o in obrazy {
                if !rychlost {
                    yuv.clear();
                    o.zapis_yuv(&mut yuv);
                    md5.pridaj(&yuv);
                }
                if diagnostika {
                    let v16: Vec<u8> = yuv.iter().flat_map(|&b| [b, 0]).collect();
                    md5_16.pridaj(&v16);
                    for r in &o.obraz.roviny {
                        for y in 0..r.vyska {
                            md5_cely.pridaj(&r.data[y * r.krok..y * r.krok + r.sirka]);
                        }
                    }
                }
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
        let (v16, vcely) = (hex(&md5_16.vysledok()), hex(&md5_cely.vysledok()));
        let tok_md5 = {
            let mut m = Md5::novy();
            m.pridaj(&data);
            hex(&m.vysledok())
        };
        let ma = |x: &str| ocakavane.iter().any(|(o, _)| o == x);
        let yuv_stav = if ocakavane.is_empty() {
            format!("{vydanych} obr.")
        } else if ma(&vystup_md5) {
            "zhoda".to_string()
        } else if ma(&v16) {
            "zhoda (16-bit vzorky)".to_string()
        } else if ma(&vcely) {
            "zhoda (bez orezania)".to_string()
        } else if ocakavane.iter().all(|(o, _)| *o == tok_md5) {
            format!("{vydanych} obr. (pri toku len MD5 toku)")
        } else {
            let (o, s) = ocakavane.iter().find(|(o, _)| *o != tok_md5).unwrap_or(&ocakavane[0]);
            format!("NEZHODA ({vydanych} obr., {s}: {o} ≠ {vystup_md5})")
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
        poznamka.extend(s.opis_nezhody.iter().take(1).cloned());
        // bitovo presný: žiadna nezhoda odtlačku obrazu, žiadna chyba, a ak je MD5 celého výstupu, zhoduje sa
        let bez_chyby = s.nezhoda.is_empty() && s.chyby.is_empty() && !yuv_stav.starts_with("NEZHODA");
        // overený = odtlačok každého obrazu zo SEI, alebo MD5 celého výstupu
        let overeny = (s.zhoda > 0 && s.bez_odtlacku == 0) || yuv_stav.starts_with("zhoda");
        if mimo_profilu {
            mimo += 1;
        } else if !bez_chyby {
            zlych += 1;
        } else if overeny {
            presnych += 1;
        } else {
            neoverenych += 1;
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
        "**Tokov: {}, bitovo presných: {presnych}, s chybou: {zlych}, bez odtlačku na overenie: {neoverenych}, \
         mimo profilu Main 8-bit: {mimo}**",
        zoznam.len()
    );
    if prisne && zlych > 0 {
        std::process::exit(1);
    }
}
