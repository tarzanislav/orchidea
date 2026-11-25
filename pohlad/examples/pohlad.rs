//! Príkazový riadok Pohľadu (aj pre nezávislé overenie v CI).
//!
//! - `pohlad davka <vstupný priečinok> <výstupný priečinok> [--format jpg|png|tif|exr] [--max N] [--meno ŠABLÓNA]
//!   [--vlakna N] [--kvalita Q] [--prestavka MS]` – riadok na každý súbor: `SUBOR poradie ms_od_začiatku ok|chyba text`
//! - `pohlad nahlady <priečinok> <strana> [--vlakna N] [--vystup PRIEČINOK]` – zoznam obrázkov a náhľady na pozadí;
//!   riadok `NAHLAD poradie ms šírka výška meno` (s `--vystup` sa náhľady uložia ako PNG)
//! - `pohlad porovnaj <a> <b> [--rozdiel výstup.png] [--zosilnenie K] [--deleny výstup.png]` – riadok
//!   `NAJVIAC x PRIEMER x PSNR x SSIM x ROZDIELNYCH n`
//! - `pohlad zmensi <vstup> <výstup.surove> <šírka> <výška>` – zmenšenie a surové vzorky (ako `formaty prevod`)

use orchidea_formaty::obraz::Vzorky;
use orchidea_formaty::{Format, Limity};
use orchidea_pohlad::davka::{self, Nastavenie, Recept, Udalost};
use orchidea_pohlad::{nahlady, porovnanie, zmensenie};
use std::process::ExitCode;
use std::time::{Duration, Instant};

fn hodnota<'a>(a: &'a [String], meno: &str) -> Option<&'a str> {
    a.iter().position(|x| x == meno).and_then(|i| a.get(i + 1)).map(String::as_str)
}

fn spusti(a: &[String]) -> Result<(), String> {
    match a.get(1).map(String::as_str) {
        Some("davka") if a.len() >= 4 => {
            let mut subory: Vec<_> = std::fs::read_dir(&a[2])
                .map_err(|e| e.to_string())?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file())
                .collect();
            subory.sort();
            let recept = Recept {
                format: hodnota(a, "--format").and_then(Format::z_pripony),
                max_strana: hodnota(a, "--max").and_then(|v| v.parse().ok()),
                meno: hodnota(a, "--meno").unwrap_or("{meno}").to_string(),
                kvalita_jpeg: hodnota(a, "--kvalita").and_then(|v| v.parse().ok()).unwrap_or(90),
            };
            let nastavenie = Nastavenie {
                vlakna: hodnota(a, "--vlakna").and_then(|v| v.parse().ok()).unwrap_or(0),
                prestavka: Duration::from_millis(hodnota(a, "--prestavka").and_then(|v| v.parse().ok()).unwrap_or(0)),
                limity: Limity::default(),
            };
            let vlakna = nastavenie.pocet_vlakien();
            let start = Instant::now();
            let beh = davka::spusti(subory, a[3].clone().into(), recept, nastavenie);
            let mut chyby = 0;
            for u in beh.udalosti.iter() {
                match u {
                    Udalost::Subor { poradie, vstup, vysledok } => {
                        let ms = start.elapsed().as_millis();
                        match vysledok {
                            Ok(c) => println!("SUBOR {poradie} {ms} ok {}", c.display()),
                            Err(e) => {
                                chyby += 1;
                                println!("SUBOR {poradie} {ms} chyba {}: {e}", vstup.display());
                            }
                        }
                    }
                    Udalost::Koniec { spracovanych, chyb, trvanie, zrusene } => {
                        println!("KONIEC {spracovanych} chyb={chyb} ms={} vlakna={vlakna} zrusene={zrusene}", trvanie.as_millis());
                    }
                }
            }
            beh.cakaj();
            if chyby > 0 { Err(format!("{chyby} súborov s chybou")) } else { Ok(()) }
        }
        Some("nahlady") if a.len() >= 4 => {
            let zoznam = nahlady::zoznam(a[2].as_ref()).map_err(|e| e.to_string())?;
            let strana: usize = a[3].parse().map_err(|_| "strana")?;
            let vlakna = hodnota(a, "--vlakna").and_then(|v| v.parse().ok()).unwrap_or(0);
            let vystup = hodnota(a, "--vystup").map(std::path::PathBuf::from);
            if let Some(v) = &vystup {
                std::fs::create_dir_all(v).map_err(|e| e.to_string())?;
            }
            let start = Instant::now();
            let cesty: Vec<_> = zoznam.iter().map(|p| p.cesta.clone()).collect();
            let beh = nahlady::spusti(cesty, strana, vlakna, Limity::default());
            let mut chyby = 0;
            for u in beh.udalosti.iter() {
                match u {
                    nahlady::Udalost::Nahlad { poradie, vysledok } => {
                        let ms = start.elapsed().as_millis();
                        let meno = zoznam[poradie].cesta.file_name().map(|m| m.to_string_lossy().into_owned()).unwrap_or_default();
                        match vysledok {
                            Ok(o) => {
                                println!("NAHLAD {poradie} {ms} {} {} {meno}", o.sirka, o.vyska);
                                if let Some(v) = &vystup {
                                    let d = orchidea_formaty::zapis(&o, Format::Png).map_err(|e| e.to_string())?;
                                    std::fs::write(v.join(format!("{meno}.png")), d).map_err(|e| e.to_string())?;
                                }
                            }
                            Err(e) => {
                                chyby += 1;
                                println!("NAHLAD {poradie} {ms} chyba {meno}: {e}");
                            }
                        }
                    }
                    nahlady::Udalost::Koniec { hotovych } => println!("KONIEC {hotovych} chyb={chyby}"),
                }
            }
            beh.cakaj();
            Ok(())
        }
        Some("porovnaj") if a.len() >= 4 => {
            let citaj = |c: &str| -> Result<orchidea_formaty::Obraz, String> {
                let d = std::fs::read(c).map_err(|e| e.to_string())?;
                orchidea_formaty::citaj(&d, &Limity::default()).map_err(|e| e.to_string())
            };
            let (x, y) = (citaj(&a[2])?, citaj(&a[3])?);
            let m = porovnanie::porovnaj(&x, &y).map_err(|e| e.to_string())?;
            let ssim = m.ssim.map_or("-".into(), |s| format!("{s:.10}"));
            println!("NAJVIAC {} PRIEMER {:.10} PSNR {:.10} SSIM {ssim} ROZDIELNYCH {}", m.najviac, m.priemer, m.psnr, m.rozdielnych_bodov);
            if let Some(c) = hodnota(a, "--rozdiel") {
                let k = hodnota(a, "--zosilnenie").and_then(|v| v.parse().ok()).unwrap_or(10.0);
                let r = porovnanie::obraz_rozdielu(&x, &y, k).map_err(|e| e.to_string())?;
                std::fs::write(c, orchidea_formaty::zapis(&r, Format::Png).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            }
            if let Some(c) = hodnota(a, "--deleny") {
                let r = porovnanie::deleny(&x, &y, x.sirka / 2).map_err(|e| e.to_string())?;
                std::fs::write(c, orchidea_formaty::zapis(&r, Format::Png).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Some("zmensi") if a.len() == 6 => {
            let d = std::fs::read(&a[2]).map_err(|e| e.to_string())?;
            let o = orchidea_formaty::citaj(&d, &Limity::default()).map_err(|e| e.to_string())?;
            let (w, h) = (a[4].parse().map_err(|_| "šírka")?, a[5].parse().map_err(|_| "výška")?);
            let z = zmensenie::zmensi(&o, w, h).map_err(|e| e.to_string())?;
            let (typ, bajty): (&str, Vec<u8>) = match &z.vzorky {
                Vzorky::U8(v) => ("u8", v.clone()),
                Vzorky::U16(v) => ("u16", v.iter().flat_map(|x| x.to_le_bytes()).collect()),
                Vzorky::F32(v) => ("f32", v.iter().flat_map(|x| x.to_le_bytes()).collect()),
            };
            let mut von = format!("{} {} {} {typ}\n", z.sirka, z.vyska, z.kanaly()).into_bytes();
            von.extend(bajty);
            std::fs::write(&a[3], von).map_err(|e| e.to_string())
        }
        _ => Err("použitie: pohlad davka <vstup> <výstup> [voľby] | zmensi <vstup> <výstup.surove> <šírka> <výška>".into()),
    }
}

fn main() -> ExitCode {
    let a: Vec<String> = std::env::args().collect();
    match spusti(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("chyba: {e}");
            ExitCode::FAILURE
        }
    }
}
