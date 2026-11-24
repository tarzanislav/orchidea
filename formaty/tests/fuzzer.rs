//! Vlastný fuzzer čítačiek (R211): náhodné a pozmenené súbory nesmú spôsobiť pád, zacyklenie ani vyčerpanie
//! pamäte – len chybu. Základ sú platné súbory z našich kodérov; mutácie: zmena bajtov, vloženie, vymazanie,
//! skrátenie, zväčšenie rozmerov v hlavičke a zámena úsekov. Semienko je pevné – chyba sa dá zopakovať.

use orchidea_formaty::obraz::{Limity, Model, Obraz, Vzorky};
use orchidea_formaty::{deflate, jpeg, png};
use orchidea_jadro::nahoda::Nahoda;

/// Prísne limity – fuzzer nesmie alokovať veľa pamäte ani pri „platnom“ obrovskom rozmere.
fn limity() -> Limity {
    Limity { pixelov: 1 << 22, bajtov: 64 << 20, metadat: 1 << 20 }
}

fn pocet_pokusov() -> usize {
    let zaklad = if cfg!(debug_assertions) { 3_000 } else { 30_000 };
    std::env::var("ORCHIDEA_FUZZER").ok().and_then(|v| v.parse().ok()).unwrap_or(zaklad)
}

fn obraz(w: usize, h: usize, model: Model, alfa: bool, n: &mut Nahoda) -> Obraz {
    let k = match model {
        Model::Siva => 1,
        Model::Cmyk => 4,
        _ => 3,
    } + alfa as usize;
    let v: Vec<u8> = (0..w * h * k).map(|i| ((i * 37) as u8).wrapping_add(n.mensie_ako(20) as u8)).collect();
    Obraz::novy(w, h, model, alfa, Vzorky::U8(v)).unwrap()
}

fn zmutuj(zaklad: &[u8], n: &mut Nahoda) -> Vec<u8> {
    let mut d = zaklad.to_vec();
    for _ in 0..1 + n.mensie_ako(6) {
        if d.is_empty() {
            break;
        }
        let i = n.mensie_ako(d.len() as u64) as usize;
        match n.mensie_ako(8) {
            0..=2 => d[i] = n.dalsie() as u8,
            3 => d[i] ^= 1 << n.mensie_ako(8),
            4 => d.insert(i, n.dalsie() as u8),
            5 => {
                d.remove(i);
            }
            6 => d.truncate(i),
            _ => {
                // zámena dvoch úsekov (presunuté kúsky, segmenty)
                let j = n.mensie_ako(d.len() as u64) as usize;
                let l = (n.mensie_ako(32) as usize).min(d.len() - i.max(j));
                for k in 0..l {
                    d.swap(i + k, j + k);
                }
            }
        }
    }
    d
}

fn fuzz(meno: &str, zaklady: &[Vec<u8>], citaj: impl Fn(&[u8]), semienko: u64) {
    // ORCHIDEA_FUZZER_SEMIENKO mení semienko (dlhé behy mimo CI); bez neho je beh vždy rovnaký
    let extra: u64 = std::env::var("ORCHIDEA_FUZZER_SEMIENKO").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut n = Nahoda::zo_semienka(semienko ^ extra.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let pokusov = pocet_pokusov();
    for i in 0..pokusov {
        let data = if i % 10 == 0 {
            let l = n.mensie_ako(300) as usize;
            (0..l).map(|_| n.dalsie() as u8).collect()
        } else {
            zmutuj(&zaklady[i % zaklady.len()], &mut n)
        };
        citaj(&data);
    }
    eprintln!("{meno}: {pokusov} pokusov bez pádu");
}

#[test]
fn fuzzer_zlib() {
    let mut n = Nahoda::zo_semienka(1);
    let texty: Vec<Vec<u8>> = (0..4)
        .map(|k| (0..3000).map(|i| ((i * (k + 1)) % 61) as u8 ^ n.mensie_ako(2) as u8).collect::<Vec<u8>>())
        .collect();
    let zaklady: Vec<Vec<u8>> =
        texty.iter().enumerate().map(|(k, t)| deflate::zlib_zbal(t, k as u8 * 3)).collect();
    fuzz(
        "zlib",
        &zaklady,
        |d| {
            let _ = deflate::zlib_rozbal(d, 1 << 20);
            let _ = deflate::rozbal(d, 1 << 20);
        },
        11,
    );
}

#[test]
fn fuzzer_png() {
    let mut n = Nahoda::zo_semienka(2);
    let mut zaklady = Vec::new();
    for (model, alfa) in [(Model::Siva, false), (Model::Rgb, true), (Model::Rgb, false)] {
        let mut o = obraz(13, 9, model, alfa, &mut n);
        o.farba.icc = Some(vec![7; 300]);
        o.metadata.texty.push(("k".into(), "v".into()));
        zaklady.push(png::zapis(&o, &png::NastaveniePng { uroven: 6, paleta: false }).unwrap());
        zaklady.push(png::zapis(&o, &png::NastaveniePng::default()).unwrap());
    }
    let l = limity();
    fuzz(
        "PNG",
        &zaklady,
        |d| {
            let _ = png::citaj(d, &l);
            let _ = png::citaj_hlavicku(d);
        },
        12,
    );
}

#[test]
fn fuzzer_jpeg() {
    let mut n = Nahoda::zo_semienka(3);
    let mut zaklady = Vec::new();
    for model in [Model::Siva, Model::Rgb, Model::Cmyk] {
        let mut o = obraz(19, 11, model, false, &mut n);
        o.farba.icc = Some(vec![3; 200]);
        o.metadata.exif = Some(orchidea_formaty::exif::s_natocenim(3));
        for (podv, progresivny) in [(jpeg::Podvzorkovanie::S420, false), (jpeg::Podvzorkovanie::S422, true)] {
            let nast = jpeg::NastavenieJpeg { kvalita: 70, podvzorkovanie: podv, progresivny };
            zaklady.push(jpeg::zapis(&o, &nast).unwrap());
        }
    }
    let l = limity();
    fuzz(
        "JPEG",
        &zaklady,
        |d| {
            let _ = jpeg::citaj(d, &l);
            let _ = jpeg::citaj_zmensene(d, &l, 8);
            let _ = jpeg::citaj_hlavicku(d);
        },
        13,
    );
}

#[test]
fn rozpoznanie_a_obrovske_rozmery() {
    // hlavička s rozmerom 65 535 × 65 535 – limit zastaví skôr, než sa alokuje pamäť
    let mut n = Nahoda::zo_semienka(4);
    let o = obraz(8, 8, Model::Rgb, false, &mut n);
    let mut j = jpeg::zapis(&o, &jpeg::NastavenieJpeg::default()).unwrap();
    let sof = j.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    j[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(matches!(jpeg::citaj(&j, &limity()), Err(orchidea_jadro::Chyba::Limit(_))));
    assert_eq!(orchidea_formaty::rozpoznaj(&j), Some(orchidea_formaty::Format::Jpeg));
}
