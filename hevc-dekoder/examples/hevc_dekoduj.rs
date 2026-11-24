//! Dekódovanie tokov HEVC (Annex B) vlastným softvérovým dekodérom a overenie **bitovej zhody**: odtlačok MD5
//! každého obrazu zo SEI (decoded picture hash) a – ak je pri toku súbor `*.md5` s MD5 celého výstupu YUV
//! (skúšobné toky normy) – aj MD5 orezaného výstupu v poradí zobrazenia.
//!
//! Spustenie: `cargo run --release -p orchidea-hevc-dekoder --example hevc_dekoduj -- <súbor alebo priečinok>…`
//! Premenné: `ORCHIDEA_HEVC_YUV=<priečinok>` zapíše výstup YUV, `ORCHIDEA_HEVC_PRISNE=1` vráti chybu pri
//! akejkoľvek nezhode či neoverenom toku v profile (CI), `ORCHIDEA_HEVC_RYCHLOST=1` meria čisté dekódovanie (bez overovania MD5) – ako
//! pri prehrávaní, `ORCHIDEA_HEVC_VLAKNA=<n>` počet vlákien dekódovania (predvolene počet jadier).

use orchidea_hevc::jednotky_annex_b;
use orchidea_hevc::odtlacok::{Md5, hex};
use orchidea_hevc_dekoder::{Dekoder, Zdroj};
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

/// Odtlačky zo súborov pri toku (zip skúšobného toku normy sa rozbalí do vlastného priečinka): súbory s „md5“
/// v názve – jediný odtlačok (MD5 celého výstupu YUV alebo samotného toku) alebo zoznam po obrazoch či rovinách.
struct Odtlacky {
    /// (MD5, súbor) – súbory s jediným odtlačkom
    cele: Vec<(String, String)>,
    /// (zoznam MD5, súbor) – súbory s viacerými odtlačkami
    zoznamy: Vec<(Vec<String>, String)>,
    /// opis priečinka na diagnostiku (súbory a počet odtlačkov v nich)
    opis: Vec<String>,
}

fn odtlacky_pri_toku(tok: &Path) -> Odtlacky {
    let mut o = Odtlacky { cele: Vec::new(), zoznamy: Vec::new(), opis: Vec::new() };
    let Some(priecinok) = tok.parent() else { return o };
    let mut polozky: Vec<PathBuf> =
        std::fs::read_dir(priecinok).into_iter().flatten().flatten().map(|e| e.path()).collect();
    polozky.sort();
    for k in polozky {
        let meno = k.file_name().map_or_else(String::new, |m| m.to_string_lossy().into_owned());
        let male = meno.to_ascii_lowercase();
        if male.ends_with(".bit") || male.ends_with(".bin") || k.is_dir() {
            continue;
        }
        let velkost = std::fs::metadata(&k).map_or(0, |m| m.len());
        if !male.contains("md5") || velkost > 1 << 20 {
            // na diagnostiku len súbory, ktoré môžu niesť odtlačok výstupu (nie ďalšie toky v priečinku)
            if [".yuv", ".txt", ".sha1", ".crc"].iter().any(|p| male.ends_with(p)) {
                o.opis.push(format!("{meno} ({velkost} B)"));
            }
            continue;
        }
        let Ok(t) = std::fs::read(&k) else { continue };
        let t = String::from_utf8_lossy(&t);
        let h: Vec<String> = t
            .split(|c: char| !c.is_ascii_hexdigit())
            .filter(|s| s.len() == 32)
            .map(|s| s.to_ascii_lowercase())
            .collect();
        o.opis.push(format!("{meno} ({}× MD5)", h.len()));
        match h.len() {
            0 => {}
            1 => o.cele.push((h[0].clone(), meno)),
            _ => o.zoznamy.push((h, meno)),
        }
    }
    o
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
    let vlakna = std::env::var("ORCHIDEA_HEVC_VLAKNA").ok().and_then(|v| v.parse::<usize>().ok());
    println!("| Tok | obrazov | MD5 SEI zhoda | nezhoda | bez SEI | výstup YUV | obr./s | poznámka |");
    println!("|---|---|---|---|---|---|---|---|");
    let (mut presnych, mut zlych, mut mimo, mut neoverenych) = (0usize, 0usize, 0usize, 0usize);
    let (mut obrazov_spolu, mut cas_spolu) = (0usize, 0f64);
    for cesta in &zoznam {
        let Ok(data) = std::fs::read(cesta) else { continue };
        let meno = cesta.file_name().map_or_else(String::new, |m| m.to_string_lossy().into_owned());
        let mut d = if rychlost { Dekoder::na_prehravanie() } else { Dekoder::novy() };
        if let Some(n) = vlakna {
            d = d.s_vlaknami(n);
        }
        let mut md5 = Md5::novy();
        // varianty pre diagnostiku nezhody celého výstupu: 16-bitové vzorky a obraz bez orezania
        let mut md5_16 = Md5::novy();
        let mut md5_cely = Md5::novy();
        let ocakavane = if rychlost {
            Odtlacky { cele: Vec::new(), zoznamy: Vec::new(), opis: Vec::new() }
        } else {
            odtlacky_pri_toku(cesta)
        };
        let diagnostika = !(ocakavane.cele.is_empty() && ocakavane.zoznamy.is_empty()) && !rychlost;
        // MD5 každého vydaného obrazu a každej jeho roviny (pre zoznamy odtlačkov po obrazoch)
        let (mut po_obrazoch, mut po_rovinach) = (Vec::new(), Vec::new());
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
                    let mut m = Md5::novy();
                    m.pridaj(&yuv);
                    po_obrazoch.push(hex(&m.vysledok()));
                    let [l, p, h, d] = o.okno;
                    let mut od = 0;
                    for (c, r) in o.obraz.roviny.iter().enumerate() {
                        let s = usize::from(c > 0);
                        let n = (r.sirka - (p >> s) - (l >> s)) * (r.vyska - (d >> s) - (h >> s));
                        let mut m = Md5::novy();
                        m.pridaj(&yuv[od..(od + n).min(yuv.len())]);
                        po_rovinach.push(hex(&m.vysledok()));
                        od += n;
                    }
                    let v16: Vec<u8> = yuv.iter().flat_map(|&b| [b, 0]).collect();
                    md5_16.pridaj(&v16);
                    for r in &o.obraz.roviny {
                        for y in 0..r.vyska {
                            md5_cely.pridaj(r.riadok(y));
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
        obrazov_spolu += s.obrazov;
        cas_spolu += cas;
        let vystup_md5 = hex(&md5.vysledok());
        let (v16, vcely) = (hex(&md5_16.vysledok()), hex(&md5_cely.vysledok()));
        let tok_md5 = {
            let mut m = Md5::novy();
            m.pridaj(&data);
            hex(&m.vysledok())
        };
        let cele = &ocakavane.cele;
        let ma = |x: &str| cele.iter().any(|(o, _)| o == x);
        // zoznam odtlačkov po obrazoch alebo po rovinách s rovnakým počtom ako výstup
        let zoznam = ocakavane.zoznamy.iter().find_map(|(z, s)| {
            if z.len() == po_obrazoch.len() {
                Some((z, &po_obrazoch, s, "obrazoch"))
            } else if z.len() == po_rovinach.len() {
                Some((z, &po_rovinach, s, "rovinách"))
            } else {
                None
            }
        });
        let yuv_stav = if ma(&vystup_md5) {
            "zhoda".to_string()
        } else if ma(&v16) {
            "zhoda (16-bit vzorky)".to_string()
        } else if ma(&vcely) {
            "zhoda (bez orezania)".to_string()
        } else if let Some((z, nase, s, druh)) = zoznam {
            match z.iter().zip(nase.iter()).position(|(a, b)| a != b) {
                None => format!("zhoda (po {druh}, {s})"),
                Some(i) => format!("NEZHODA ({s}: prvá na položke {i} z {})", z.len()),
            }
        } else if cele.iter().any(|(o, _)| *o != tok_md5) {
            let (o, s) = cele.iter().find(|(o, _)| *o != tok_md5).unwrap_or(&cele[0]);
            format!("NEZHODA ({vydanych} obr., {s}: {o} ≠ {vystup_md5})")
        } else if !cele.is_empty() {
            format!("{vydanych} obr. (pri toku len MD5 toku)")
        } else {
            format!("{vydanych} obr.")
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
        let overeny = (s.zhoda > 0 && s.bez_odtlacku == 0) || yuv_stav.starts_with("zhoda");
        if !overeny && !ocakavane.opis.is_empty() {
            poznamka.push(format!("pri toku: {}", ocakavane.opis.join(", ")));
        }
        // bitovo presný: žiadna nezhoda odtlačku obrazu, žiadna chyba, a ak je MD5 celého výstupu, zhoduje sa
        let bez_chyby = s.nezhoda.is_empty() && s.chyby.is_empty() && !yuv_stav.starts_with("NEZHODA");
        // overený = odtlačok každého obrazu zo SEI, alebo MD5 celého výstupu (či po obrazoch)
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
    if rychlost {
        println!(
            "**Tokov: {}, obrazov: {obrazov_spolu}, spolu {:.1} obr./s, s chybou: {zlych}, mimo profilu Main 8-bit: {mimo}** \
             (meranie rýchlosti bez overovania)",
            zoznam.len(),
            obrazov_spolu as f64 / cas_spolu.max(1e-9)
        );
    } else {
        println!(
            "**Tokov: {}, bitovo presných: {presnych}, s chybou: {zlych}, bez odtlačku na overenie: {neoverenych}, \
             mimo profilu Main 8-bit: {mimo}**",
            zoznam.len()
        );
    }
    // prísne (CI): každý tok v profile musí byť bitovo presný a overený (SEI alebo MD5 celého výstupu)
    if prisne && (zlych > 0 || (neoverenych > 0 && !rychlost)) {
        std::process::exit(1);
    }
}
