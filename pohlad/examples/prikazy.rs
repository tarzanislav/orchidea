//! Príkazový riadok prehliadača a editora fotografií (Pohľad) (aj pre nezávislé overenie v CI). Okno je program `pohlad`.
//!
//! - `prikazy davka <vstupný priečinok> <výstupný priečinok> [--format jpg|png|tif|exr] [--max N] [--meno ŠABLÓNA]
//!   [--vlakna N] [--kvalita Q] [--prestavka MS]` – riadok na každý súbor: `SUBOR poradie ms_od_začiatku ok|chyba text`
//! - `prikazy nahlady <priečinok> <strana> [--vlakna N] [--vystup PRIEČINOK]` – zoznam obrázkov a náhľady na pozadí;
//!   riadok `NAHLAD poradie ms šírka výška meno` (s `--vystup` sa náhľady uložia ako PNG)
//! - `prikazy porovnaj <a> <b> [--rozdiel výstup.png] [--zosilnenie K] [--deleny výstup.png]` – riadok
//!   `NAJVIAC x PRIEMER x PSNR x SSIM x ROZDIELNYCH n`
//! - `prikazy snimka <súbor|priečinok> <šírka> <výška> <výstup.png> [povely …]` – jadro okna bez okna: povely
//!   `dalsi predosly prvy posledny priblizit oddialit 1:1 prisposobit oznac rezim pas udaje otoc otocvlavo zrkadlo
//!   orezat jas kontrast gama zmensit spat znova ulozit prezentacia pauza tik tahaj:dx,dy
//!   koliesko:k,x,y delic:x`; po každom sa počká na prácu na pozadí; riadky `VYKRESLENI n ZOBUDENI n POKOJ ok|…`
//! - `prikazy herbar <katalóg> pridaj <priečinok> [--bez-podpriecinkov]` / `obnov` / `hladaj <dotaz>` – katalóg
//!   fotiek; `hladaj` vypíše riadok `ZAZNAM cesta | dátum | fotoaparát | kľúče` na každý nájdený súbor
//! - `prikazy zmensi <vstup> <výstup.surove> <šírka> <výška>` – zmenšenie a surové vzorky (ako `formaty prevod`)

use orchidea_formaty::obraz::Vzorky;
use orchidea_formaty::{Format, Limity};
use orchidea_pohlad::davka::{self, Nastavenie, Recept, Udalost};
use orchidea_pohlad::prehliadac::{Povel, Prehliadac, Vstup};
use orchidea_pohlad::{nahlady, porovnanie, zmensenie};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
        Some("snimka") if a.len() >= 6 => {
            let zobudeni = Arc::new(AtomicU64::new(0));
            let z = zobudeni.clone();
            let mut p = Prehliadac::novy(Arc::new(move || {
                z.fetch_add(1, Ordering::Relaxed);
            }));
            let (w, h): (usize, usize) = (a[3].parse().map_err(|_| "šírka")?, a[4].parse().map_err(|_| "výška")?);
            let cakaj = |p: &mut Prehliadac| {
                let start = Instant::now();
                while p.pracuje() && start.elapsed() < Duration::from_secs(60) {
                    p.spracuj_pozadie();
                    std::thread::sleep(Duration::from_millis(2));
                }
                p.spracuj_pozadie();
            };
            p.vstup(Vstup::Velkost { sirka: w, vyska: h });
            p.vstup(Vstup::Otvor(a[2].clone().into()));
            cakaj(&mut p);
            let cisla = |t: &str| -> Vec<f32> { t.split(',').filter_map(|x| x.parse().ok()).collect() };
            for povel in &a[6..] {
                let v = match povel.split_once(':').map_or((povel.as_str(), ""), |(x, y)| (x, y)) {
                    ("dalsi", _) => Vstup::Povel(Povel::Dalsi),
                    ("predosly", _) => Vstup::Povel(Povel::Predosly),
                    ("prvy", _) => Vstup::Povel(Povel::Prvy),
                    ("posledny", _) => Vstup::Povel(Povel::Posledny),
                    ("priblizit", _) => Vstup::Povel(Povel::Priblizit),
                    ("oddialit", _) => Vstup::Povel(Povel::Oddialit),
                    ("1", "1") => Vstup::Povel(Povel::Skutocna),
                    ("prisposobit", _) => Vstup::Povel(Povel::Prisposobit),
                    ("oznac", _) => Vstup::Povel(Povel::Oznac),
                    ("rezim", _) => Vstup::Povel(Povel::Rezim),
                    ("pas", _) => Vstup::Povel(Povel::Pas),
                    ("udaje", _) => Vstup::Povel(Povel::Udaje),
                    ("otoc", _) => Vstup::Povel(Povel::OtocVpravo),
                    ("otocvlavo", _) => Vstup::Povel(Povel::OtocVlavo),
                    ("zrkadlo", _) => Vstup::Povel(Povel::Zrkadlo),
                    ("orezat", _) => Vstup::Povel(Povel::Orezat),
                    ("jas", _) => Vstup::Povel(Povel::JasViac),
                    ("kontrast", _) => Vstup::Povel(Povel::KontrastViac),
                    ("gama", _) => Vstup::Povel(Povel::GamaViac),
                    ("zmensit", _) => Vstup::Povel(Povel::Zmensit),
                    ("spat", _) => Vstup::Povel(Povel::Spat),
                    ("znova", _) => Vstup::Povel(Povel::Znova),
                    ("ulozit", _) => Vstup::Povel(Povel::Ulozit),
                    ("prezentacia", _) => Vstup::Povel(Povel::Prezentacia),
                    ("pauza", _) => Vstup::Povel(Povel::Pauza),
                    ("tik", _) => Vstup::Tik,
                    ("tahaj", t) => {
                        let c = cisla(t);
                        Vstup::Tahaj { dx: c[0], dy: c[1] }
                    }
                    ("koliesko", t) => {
                        let c = cisla(t);
                        Vstup::Koliesko { krokov: c[0], x: c[1], y: c[2] }
                    }
                    ("delic", t) => Vstup::Delic { x: cisla(t)[0] },
                    ("napoveda", _) => Vstup::Povel(Povel::Napoveda),
                    ("panel", _) => Vstup::Povel(Povel::Panel),
                    ("esc", _) => {
                        p.esc();
                        cakaj(&mut p);
                        continue;
                    }
                    ("mys", t) => {
                        let c = cisla(t);
                        Vstup::Mys { x: c[0], y: c[1] }
                    }
                    ("klik", t) => {
                        let c = cisla(t);
                        Vstup::Klik { x: c[0], y: c[1] }
                    }
                    ("mysnad", id) => {
                        let (x, y) = p.tlacidlo(id).ok_or(format!("tlačidlo {id} nie je na paneli"))?;
                        Vstup::Mys { x, y }
                    }
                    ("stlac", id) => {
                        let (x, y) = p.tlacidlo(id).ok_or(format!("tlačidlo {id} nie je na paneli"))?;
                        Vstup::Klik { x, y }
                    }
                    _ => return Err(format!("neznámy povel {povel}")),
                };
                p.vstup(v);
                cakaj(&mut p);
            }
            let mut platno = orchidea_pismo::Platno::nove(w, h, 0);
            p.vykresli(&mut platno);
            // pokoj: bez vstupu sa nič nepočíta ani neprekresľuje
            let pred = zobudeni.load(Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(500));
            let pokoj = !p.spracuj_pozadie() && !p.potrebuje_prekreslit() && zobudeni.load(Ordering::Relaxed) == pred;
            println!("VYKRESLENI {} ZOBUDENI {} POKOJ {}", p.vykresleni, pred, if pokoj { "ok" } else { "nie" });
            println!("TITULOK {}", p.titulok());
            println!("CASOVAC {}", p.casovac().map_or("ziadny".into(), |ms| format!("{ms} ms")));
            let body: Vec<u8> = platno.body.iter().flat_map(|&f| [(f >> 16) as u8, (f >> 8) as u8, f as u8]).collect();
            let o = orchidea_formaty::Obraz::novy(w, h, orchidea_formaty::obraz::Model::Rgb, false, Vzorky::U8(body))
                .map_err(|e| e.to_string())?;
            std::fs::write(&a[5], orchidea_formaty::zapis(&o, Format::Png).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        }
        Some("herbar") if a.len() >= 4 => {
            use orchidea_pohlad::herbar::Herbar;
            let katalog = std::path::PathBuf::from(&a[2]);
            let mut h = Herbar::nacitaj(&katalog).map_err(|e| e.to_string())?;
            match a[3].as_str() {
                "pridaj" if a.len() >= 5 => {
                    h.pridaj_priecinok(a[4].as_ref(), !a.iter().any(|x| x == "--bez-podpriecinkov"));
                    let start = Instant::now();
                    let (n, p, z) = h.obnov(0, &Limity::default());
                    println!("OBNOVA precitanych={n} preskocenych={p} zmazanych={z} ms={}", start.elapsed().as_millis());
                    h.uloz(&katalog).map_err(|e| e.to_string())
                }
                "obnov" => {
                    let start = Instant::now();
                    let (n, p, z) = h.obnov(0, &Limity::default());
                    println!("OBNOVA precitanych={n} preskocenych={p} zmazanych={z} ms={}", start.elapsed().as_millis());
                    h.uloz(&katalog).map_err(|e| e.to_string())
                }
                "hladaj" => {
                    let dotaz = a[4..].join(" ");
                    let start = Instant::now();
                    let v = h.hladaj(&dotaz);
                    let us = start.elapsed().as_micros();
                    for z in &v {
                        println!("ZAZNAM {} | {} | {} | {}", z.cesta.display(), z.datum, z.fotoaparat, z.kluce.join(";"));
                    }
                    println!("NAJDENYCH {} z {} za {us} µs", v.len(), h.zaznamy.len());
                    Ok(())
                }
                x => Err(format!("neznámy príkaz katalógu fotografií (Herbár): {x}")),
            }
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
        _ => Err("použitie: prikazy davka <vstup> <výstup> [voľby] | zmensi <vstup> <výstup.surove> <šírka> <výška>".into()),
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
