//! Príkazový riadok Pohľadu (aj pre nezávislé overenie v CI).
//!
//! - `pohlad davka <vstupný priečinok> <výstupný priečinok> [--format jpg|png|tif|exr] [--max N] [--meno ŠABLÓNA]
//!   [--vlakna N] [--kvalita Q] [--prestavka MS]` – riadok na každý súbor: `SUBOR poradie ms_od_začiatku ok|chyba text`
//! - `pohlad zmensi <vstup> <výstup.surove> <šírka> <výška>` – zmenšenie a surové vzorky (ako `formaty prevod`)

use orchidea_formaty::obraz::Vzorky;
use orchidea_formaty::{Format, Limity};
use orchidea_pohlad::davka::{self, Nastavenie, Recept, Udalost};
use orchidea_pohlad::zmensenie;
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
