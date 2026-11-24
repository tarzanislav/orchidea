//! Rozbor tokov HEVC (Annex B): jednotky NAL, parametrické sady, obrazy, odtlačky SEI, hlavičky rezov
//! a príprava parametrov pre hardvérové dekodéry V4L2 *stateless* (krok H1: POC, sady referencií, zoznamy,
//! poradie výstupu). V CI prejde skúšobné toky normy (ITU-T, JCT-VC) – každá parametrická sada a každá
//! hlavička rezu sa musí prečítať bez chyby a každý dekódovaný obraz sa musí vydať.
//!
//! Spustenie: `cargo run --release -p orchidea-hevc --example hevc_rozbor -- <súbor alebo priečinok>…`
//! (s premennou ORCHIDEA_HEVC_PODROBNE vypíše celé SPS)

use orchidea_hevc::nal::typ;
use orchidea_hevc::sei::{self, OdtlacokObrazu};
use orchidea_hevc::v4l2::UdalostV4l2;
use orchidea_hevc::{HlavickaNal, Pps, PripravaV4l2, Sps, Vps, jednotky_annex_b, rbsp};
use std::path::{Path, PathBuf};

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

fn main() {
    let mut zoznam = Vec::new();
    for a in std::env::args().skip(1) {
        subory(Path::new(&a), &mut zoznam);
    }
    if zoznam.is_empty() {
        println!("Žiadne toky HEVC na rozbor.");
        return;
    }
    println!("| Tok | NAL | obrazov | SPS | PPS | MD5 SEI | rezov (H1) | dekódovať / vydať (H1) | chyby |");
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut chyb_spolu = 0;
    for cesta in &zoznam {
        let Ok(data) = std::fs::read(cesta) else { continue };
        let (mut nal, mut obrazov, mut pps, mut md5) = (0usize, 0usize, 0usize, 0usize);
        let mut chyby = Vec::new();
        let mut popis_sps = String::from("–");
        let mut zloziek = 3;
        let mut priprava = PripravaV4l2::nova();
        let (mut rezov, mut dekodovat, mut vydat) = (0usize, 0usize, 0usize);
        let spocitaj =
            |udalosti: Vec<UdalostV4l2>, dekodovat: &mut usize, vydat: &mut usize, rezov: &mut usize| {
                for u in udalosti {
                    match u {
                        UdalostV4l2::Dekoduj(o) => {
                            *dekodovat += 1;
                            *rezov += o.parametre.rezy.len();
                        }
                        UdalostV4l2::Vystup(_) => *vydat += 1,
                        _ => {}
                    }
                }
            };
        for j in jednotky_annex_b(&data) {
            nal += 1;
            match priprava.nal(j) {
                Ok(u) => spocitaj(u, &mut dekodovat, &mut vydat, &mut rezov),
                Err(e) => chyby.push(format!("H1: {e}")),
            }
            let h = match HlavickaNal::citaj(j) {
                Ok(h) => h,
                Err(e) => {
                    chyby.push(e.to_string());
                    continue;
                }
            };
            let r = rbsp(&j[2..]);
            match h.typ {
                typ::VPS => {
                    if let Err(e) = Vps::citaj(&r) {
                        chyby.push(format!("VPS: {e}"));
                    }
                }
                typ::SPS => match Sps::citaj(&r) {
                    Ok(s) => {
                        if std::env::var_os("ORCHIDEA_HEVC_PODROBNE").is_some() {
                            eprintln!("{s:#?}");
                        }
                        zloziek = if s.chroma_format_idc == 0 { 1 } else { 3 };
                        let (w, h) = s.zobrazene_rozmery();
                        popis_sps =
                            format!("{w}×{h} {} b {} CTB {}", s.bity_jasu, s.profil.meno(), 1 << s.log2_ctb);
                    }
                    Err(e) => chyby.push(format!("SPS: {e}")),
                },
                typ::PPS => match Pps::citaj(&r) {
                    Ok(_) => pps += 1,
                    Err(e) => chyby.push(format!("PPS: {e}")),
                },
                typ::PREFIX_SEI | typ::SUFFIX_SEI => match sei::spravy(&r) {
                    Ok(spravy) => {
                        for (t, obsah) in spravy {
                            if t == sei::ODTLACOK_OBRAZU {
                                match OdtlacokObrazu::citaj(obsah, zloziek) {
                                    Ok(OdtlacokObrazu::Md5(_)) => md5 += 1,
                                    Ok(_) => {}
                                    Err(e) => chyby.push(format!("SEI: {e}")),
                                }
                            }
                        }
                    }
                    Err(e) => chyby.push(format!("SEI: {e}")),
                },
                t if t < 32 && r.first().is_some_and(|b| b & 0x80 != 0) => obrazov += 1,
                _ => {}
            }
        }
        spocitaj(priprava.koniec(), &mut dekodovat, &mut vydat, &mut rezov);
        let meno = cesta.file_name().map_or_else(String::new, |m| m.to_string_lossy().into_owned());
        if vydat > dekodovat {
            chyby.push(format!("H1: vydaných {vydat} obrazov z {dekodovat}"));
        }
        chyby.truncate(5);
        chyb_spolu += chyby.len();
        println!(
            "| {meno} | {nal} | {obrazov} | {popis_sps} | {pps} | {md5} | {rezov} | {dekodovat} / {vydat} | {} |",
            if chyby.is_empty() { "–".to_string() } else { chyby.join("; ") }
        );
    }
    println!();
    println!("**Tokov: {}, chýb rozboru: {chyb_spolu}**", zoznam.len());
    if chyb_spolu > 0 {
        std::process::exit(1);
    }
}
