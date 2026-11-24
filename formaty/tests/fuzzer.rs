//! Vlastný fuzzer čítačiek (R211): náhodné a pozmenené súbory nesmú spôsobiť pád, zacyklenie ani vyčerpanie
//! pamäte – len chybu. Základ sú platné súbory z našich kodérov; mutácie: zmena bajtov, vloženie, vymazanie,
//! skrátenie, zväčšenie rozmerov v hlavičke a zámena úsekov. Semienko je pevné – chyba sa dá zopakovať.

use orchidea_formaty::obraz::{Limity, Model, Obraz, Vzorky};
use orchidea_formaty::{deflate, exr, jpeg, jxl, png, tiff};
use orchidea_jadro::nahoda::Nahoda;

#[allow(dead_code)]
mod subory;

/// Prísne limity – fuzzer nesmie alokovať veľa pamäte ani pri „platnom“ obrovskom rozmere. Obraz s 2^18
/// pixelmi stačí na všetky cesty kódu (viac skupín JPEG XL, viac pruhov TIFF) a beh ostane rýchly aj v QEMU.
fn limity() -> Limity {
    Limity { pixelov: 1 << 18, bajtov: 16 << 20, metadat: 1 << 20 }
}

fn pocet_pokusov() -> usize {
    // bežné skúšky (aj v emulátoroch ARM a Androidu) krátko; dlhý beh v CI cez ORCHIDEA_FUZZER
    let zaklad = if cfg!(debug_assertions) { 2_000 } else { 10_000 };
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
    fuzz_n(meno, zaklady, citaj, semienko, pocet_pokusov());
}

fn fuzz_n(meno: &str, zaklady: &[Vec<u8>], citaj: impl Fn(&[u8]), semienko: u64, pokusov: usize) {
    // ORCHIDEA_FUZZER_SEMIENKO mení semienko (dlhé behy mimo CI); bez neho je beh vždy rovnaký
    let extra: u64 = std::env::var("ORCHIDEA_FUZZER_SEMIENKO").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut n = Nahoda::zo_semienka(semienko ^ extra.wrapping_mul(0x9E37_79B9_7F4A_7C15));
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
fn fuzzer_tiff() {
    let mut n = Nahoda::zo_semienka(5);
    let mut zaklady = Vec::new();
    for (model, alfa) in [(Model::Siva, false), (Model::Rgb, true), (Model::Cmyk, false)] {
        let mut o = obraz(11, 7, model, alfa, &mut n);
        o.farba.icc = Some(vec![1; 64]);
        o.metadata.dpi = Some((72.0, 72.0));
        for deflate in [false, true] {
            zaklady.push(tiff::zapis(&o, &tiff::NastavenieTiff { deflate, uroven: 6 }).unwrap());
        }
        let v: Vec<u16> = (0..11 * 7 * o.kanaly()).map(|i| (i * 999) as u16).collect();
        let o16 = Obraz::novy(11, 7, model, alfa, Vzorky::U16(v)).unwrap();
        zaklady.push(tiff::zapis(&o16, &tiff::NastavenieTiff::default()).unwrap());
        let f: Vec<f32> = (0..11 * 7 * o.kanaly()).map(|i| i as f32 * 0.01).collect();
        let of = Obraz::novy(11, 7, model, alfa, Vzorky::F32(f)).unwrap();
        zaklady.push(tiff::zapis(&of, &tiff::NastavenieTiff::default()).unwrap());
    }
    let l = limity();
    fuzz(
        "TIFF",
        &zaklady,
        |d| {
            let _ = tiff::citaj(d, &l);
            let _ = tiff::citaj_hlavicku(d);
            let _ = tiff::pocet_stran(d);
        },
        14,
    );
}

#[test]
fn fuzzer_exr() {
    let mut zaklady = Vec::new();
    for (model, alfa) in [(Model::Rgb, true), (Model::Siva, false)] {
        let k = if model == Model::Siva { 1 } else { 3 } + alfa as usize;
        let v: Vec<f32> = (0..9 * 37 * k).map(|i| (i as f32 * 0.31).sin() * 3.0).collect();
        let o = Obraz::novy(9, 37, model, alfa, Vzorky::F32(v)).unwrap();
        for kompresia in [exr::Kompresia::Zip, exr::Kompresia::Rle, exr::Kompresia::Ziadna] {
            for polovicna in [true, false] {
                let e = exr::zapis(&o, &exr::NastavenieExr { polovicna, kompresia }).unwrap();
                // tá istá hlavička s inými kompresiami – fuzzer tak skúša aj PIZ, PXR24, B44 a DWA s cudzími údajmi
                let p = e.windows(12).position(|w| w == b"compression\0").unwrap() + 12 + 12 + 4;
                for c in 0..10u8 {
                    let mut x = e.clone();
                    x[p] = c;
                    zaklady.push(x);
                }
            }
        }
    }
    let l = limity();
    fuzz(
        "OpenEXR",
        &zaklady,
        |d| {
            let _ = exr::citaj(d, &l);
            let _ = exr::citaj_hlavicku(d);
        },
        15,
    );
}

#[test]
fn fuzzer_jxl() {
    let zaklady: Vec<Vec<u8>> = subory::SUBORY
        .iter()
        .map(|s| s.1.to_vec())
        .chain(subory::STRATOVE.iter().map(|s| s.1.to_vec()))
        .collect();
    let l = limity();
    // JPEG XL je najpomalší (dekóduje aj „prázdne“ obrazy bez údajov) – tretina pokusov
    fuzz_n(
        "JPEG XL",
        &zaklady,
        |d| {
            let _ = jxl::citaj(d, &l);
            let _ = jxl::citaj_hlavicku(d);
        },
        16,
        pocet_pokusov() / 3,
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
