//! Pomocník nezávislého overenia (CI): číta čísla zo štandardného vstupu, vypíše výsledky.
//!
//! `rozhodca aces` – trojice ACES2065-1 → XYZ D65 výstupu SDR (RRT + ODT pred kódovaním)
//! `rozhodca prenos <meno> <do|z>` – jednotlivé hodnoty
//! `rozhodca matica <zdroj> <ciel>` – matica lineárne RGB → RGB (Bradford), 9 čísel
//! `rozhodca icc <zdroj.icc|srgb> <ciel.icc|srgb> <zamer> <kanalov_vstup>` – 8-bitové bajty zo vstupu → výstup

use orchidea_farba::{Adaptacia, Prenos, Prevod, Priestor, Zamer, aces, primarne};
use std::io::{Read, Write};

fn prenos(m: &str) -> Prenos {
    match m {
        "srgb" => Prenos::Srgb,
        "bt709" => Prenos::Bt709,
        "pq" => Prenos::Pq,
        "hlg" => Prenos::Hlg,
        "prophoto" => Prenos::ProPhoto,
        "acescc" => Prenos::Acescc,
        "acescct" => Prenos::Acescct,
        "gama22" => Prenos::Gama(2.2),
        _ => panic!("neznámy prenos {m}"),
    }
}

fn zakladne(m: &str) -> primarne::Primarne {
    match m {
        "srgb" => primarne::SRGB,
        "bt2020" => primarne::BT2020,
        "dcip3" => primarne::DCI_P3,
        "p3" => primarne::DISPLAY_P3,
        "adobe" => primarne::ADOBE_RGB,
        "prophoto" => primarne::PROPHOTO,
        "ap0" => primarne::ACES_AP0,
        "ap1" => primarne::ACES_AP1,
        _ => panic!("neznáme základné farby {m}"),
    }
}

fn priestor(m: &str) -> Priestor {
    if m == "srgb" {
        Priestor::srgb()
    } else {
        Priestor::z_icc(&std::fs::read(m).expect("profil")).expect("profil ICC")
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut vstup = Vec::new();
    std::io::stdin().read_to_end(&mut vstup).expect("vstup");
    let cisla = || -> Vec<f64> {
        String::from_utf8_lossy(&vstup).split_whitespace().map(|s| s.parse().expect("číslo")).collect()
    };
    let mut von = std::io::BufWriter::new(std::io::stdout());
    match a[0].as_str() {
        "aces" => {
            for t in cisla().chunks_exact(3) {
                let x = aces::odt_xyz(aces::rrt([t[0], t[1], t[2]]));
                writeln!(von, "{:.9e} {:.9e} {:.9e}", x[0], x[1], x[2]).unwrap();
            }
        }
        "prenos" => {
            let p = prenos(&a[1]);
            for v in cisla() {
                let r = if a[2] == "do" { p.do_linearneho(v) } else { p.z_linearneho(v) };
                writeln!(von, "{r:.12e}").unwrap();
            }
        }
        "matica" => {
            let m = primarne::rgb_na_rgb(&zakladne(&a[1]), &zakladne(&a[2]), Adaptacia::Bradford);
            for r in m {
                writeln!(von, "{:.12e} {:.12e} {:.12e}", r[0], r[1], r[2]).unwrap();
            }
        }
        "icc" => {
            let zamer = match a[3].as_str() {
                "0" => Zamer::Percepcny,
                "2" => Zamer::Sytost,
                "3" => Zamer::AbsolutnyKolorimetricky,
                _ => Zamer::RelativnyKolorimetricky,
            };
            let p = Prevod::novy(&priestor(&a[1]), &priestor(&a[2]), zamer).expect("prevod");
            let k: usize = a[4].parse().unwrap();
            let o = if a.get(5).map(String::as_str) == Some("presne") {
                // presne po bodoch (bez mriežky), zaokrúhlené na 8 bitov
                let mut o = Vec::new();
                let mut b = vec![0.0; p.vystupov()];
                for t in vstup.chunks_exact(k) {
                    let v: Vec<f64> = t.iter().map(|&x| x as f64 / 255.0).collect();
                    p.bod(&v, &mut b);
                    o.extend(b.iter().map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8));
                }
                o
            } else {
                p.preved_u8(&vstup, k, p.vystupov()).expect("prevod bajtov")
            };
            von.write_all(&o).unwrap();
        }
        _ => panic!("neznámy režim"),
    }
}
