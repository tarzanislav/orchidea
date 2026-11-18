//! Meranie výkonu jadra a Neurónu (zmluva kvality: výkon je súčasť kvality).
//!
//! Spustenie: `cargo run --release -p orchidea-neuron --example meranie`
//! Výstup je tabuľka v Markdowne – CI ju pripojí k súhrnu behu.

use orchidea_jadro::nahoda::Nahoda;
use orchidea_jadro::{Odtlacok, paralel};
use orchidea_neuron::jadra::{self, Rezim};
use orchidea_neuron::kvant::{TrojhodnotoveVahy, kvantizuj_aktivacie};
use std::hint::black_box;
use std::time::Instant;

fn zmeraj(mut f: impl FnMut()) -> f64 {
    f();
    let mut najlepsie = f64::INFINITY;
    for _ in 0..5 {
        let t = Instant::now();
        f();
        najlepsie = najlepsie.min(t.elapsed().as_secs_f64());
    }
    najlepsie
}

fn main() {
    let mut n = Nahoda::zo_semienka(1);
    println!("| Meranie | Výsledok |");
    println!("|---|---|");
    println!("| vlákna | {} |", paralel::pocet_vlakien());
    println!("| vektorové rozšírenie | {} |", jadra::vektorove_rozsirenie());

    let data: Vec<u8> = (0..64 << 20).map(|_| n.dalsie() as u8).collect();
    let s = zmeraj(|| {
        black_box(Odtlacok::z(black_box(&data)));
    });
    println!("| BLAKE3 (1 vlákno) | {:.0} MB/s |", data.len() as f64 / s / 1e6);

    let (m, k, p) = (512, 512, 512);
    let a: Vec<f32> = (0..m * k).map(|_| n.f32()).collect();
    let b: Vec<f32> = (0..k * p).map(|_| n.f32()).collect();
    let mut c = vec![0.0f32; m * p];
    for rezim in [Rezim::Presny, Rezim::Rychly] {
        let s = zmeraj(|| jadra::nasob(rezim, m, p, k, &a, &b, &mut c));
        println!("| násobenie f32 512³, {rezim:?} | {:.1} GFLOP/s |", 2.0 * (m * k * p) as f64 / s / 1e9);
    }

    let (davka, vstupy, vystupy) = (64, 2048, 2048);
    let x: Vec<f32> = (0..davka * vstupy).map(|_| n.f32() * 2.0 - 1.0).collect();
    let w: Vec<f32> = (0..vystupy * vstupy).map(|_| n.f32() * 2.0 - 1.0).collect();
    let q = TrojhodnotoveVahy::z_f32(vystupy, vstupy, &w);
    let ax = kvantizuj_aktivacie(davka, vstupy, &x);
    for rezim in [Rezim::Presny, Rezim::Rychly] {
        let s = zmeraj(|| {
            black_box(q.nasob(rezim, &ax));
        });
        println!(
            "| trojhodnotová vrstva 2048×2048, dávka 64, {rezim:?} | {:.1} G operácií/s |",
            2.0 * (davka * vstupy * vystupy) as f64 / s / 1e9
        );
    }
    println!(
        "| pamäť váh 2048×2048 | f32: {} MiB, trojhodnotové: {} KiB (zbalené), {} KiB (kompaktne) |",
        (vystupy * vstupy * 4) >> 20,
        q.bajtov() >> 10,
        q.kompaktne().len() >> 10
    );
}
