//! Meranie rýchlosti formátov (podklad pre cenník práce, zákon Z14): fotka 12 Mpx – kódovanie a dekódovanie
//! PNG a JPEG, zmenšený náhľad JPEG 1/8. Výsledok je tabuľka v Markdowne pre súhrn CI.
use orchidea_formaty::obraz::{Limity, Model, Obraz, Vzorky};
use orchidea_formaty::{jpeg, png};
use std::time::Instant;

fn fotka(w: usize, h: usize) -> Obraz {
    let mut v = Vec::with_capacity(w * h * 3);
    let mut s = 1u32;
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let z = 128.0 + 100.0 * ((x as f32 / (37.0 + c as f32)).sin() * (y as f32 / 53.0).cos());
                v.push((z + (s >> 28) as f32).clamp(0.0, 255.0) as u8);
            }
        }
    }
    Obraz::novy(w, h, Model::Rgb, false, Vzorky::U8(v)).unwrap()
}

fn meraj<T>(f: impl Fn() -> T) -> (T, f64) {
    let mut najlepsi = f64::MAX;
    let mut r = f();
    for _ in 0..3 {
        let t = Instant::now();
        r = f();
        najlepsi = najlepsi.min(t.elapsed().as_secs_f64());
    }
    (r, najlepsi)
}

fn main() {
    let (w, h) = if std::env::var("ORCHIDEA_BEZ_GPU").is_ok() { (2000, 1500) } else { (4000, 3000) };
    let o = fotka(w, h);
    let mpx = (w * h) as f64 / 1e6;
    let l = Limity::default();
    println!(
        "| Formát – {w}×{h} ({mpx:.0} Mpx), {} vlákna | čas | Mpx/s | veľkosť |",
        orchidea_jadro::paralel::pocet_vlakien()
    );
    println!("|---|---|---|---|");
    let riadok = |meno: &str, t: f64, b: usize| {
        println!("| {meno} | {:.0} ms | {:.1} | {:.2} MB |", t * 1e3, mpx / t, b as f64 / 1e6)
    };
    let (j, t) =
        meraj(|| jpeg::zapis(&o, &jpeg::NastavenieJpeg { kvalita: 90, ..Default::default() }).unwrap());
    riadok("JPEG zápis q90 4:4:4", t, j.len());
    let (_, t) = meraj(|| jpeg::citaj(&j, &l).unwrap());
    riadok("JPEG čítanie", t, j.len());
    let (_, t) = meraj(|| jpeg::citaj_zmensene(&j, &l, 8).unwrap());
    riadok("JPEG náhľad 1/8 (len DC)", t, j.len());
    let (p, t) = meraj(|| png::zapis(&o, &png::NastaveniePng { uroven: 6, paleta: false }).unwrap());
    riadok("PNG zápis úroveň 6", t, p.len());
    let (_, t) = meraj(|| png::citaj(&p, &l).unwrap());
    riadok("PNG čítanie", t, p.len());
}
