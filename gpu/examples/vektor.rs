//! Meranie: referenčný interpret vs. vektorový vykonávač na procesore (1 vlákno a všetky jadrá).
//!
//! `cargo run --release -p orchidea-gpu --example vektor`

use orchidea_gpu::kernely;
use orchidea_gpu::vektor;
use orchidea_gpu::zariadenie::{DataVazby, Hodnota32};
use orchidea_gpu::{Kernel, interpret};
use orchidea_jadro::paralel;
use std::time::Instant;

fn cas(mut f: impl FnMut()) -> f64 {
    f();
    let mut najlepsi = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        f();
        najlepsi = najlepsi.min(t.elapsed().as_secs_f64());
    }
    najlepsi
}

/// Spustí kernel nad pripravenými bufferami.
type Spustac<'a> = &'a dyn Fn(&dyn Fn(&mut [DataVazby<'_>]));

fn porovnaj(
    meno: &str,
    jednotka: &str,
    prace: f64,
    kernel: &Kernel,
    k: &[Hodnota32],
    s: [u32; 3],
    spusti: Spustac,
) {
    let program = vektor::preloz(kernel).expect("preklad");
    let ti = cas(|| spusti(&|v| interpret::vykonaj(kernel, v, k, s).expect("interpret")));
    paralel::obmedz_vlakna(1);
    let t1 = cas(|| spusti(&|v| program.vykonaj(v, k, s).expect("vektor")));
    paralel::obmedz_vlakna(0);
    let tn = cas(|| spusti(&|v| program.vykonaj(v, k, s).expect("vektor")));
    println!(
        "{meno:<28} interpret {:>8.2} {jednotka} | vektor 1 vlákno {:>8.2} ({:>5.1}×) | {} vlákien {:>8.2} ({:>6.1}×)",
        prace / ti,
        prace / t1,
        ti / t1,
        paralel::pocet_vlakien(),
        prace / tn,
        ti / tn
    );
}

fn main() {
    println!("procesor: {} vlákien, blok {} pruhov\n", paralel::pocet_vlakien(), vektor::PASY);

    let n = 1 << 20;
    let (a, b) = (vec![1.5f32; n], vec![2.25f32; n]);
    let s1 = [(n as u32).div_ceil(64), 1, 1];
    porovnaj(
        "súčet 1 M prvkov",
        "M prvkov/s",
        n as f64 / 1e6,
        &kernely::sucet(),
        &[Hodnota32::U32(n as u32)],
        s1,
        &|f| {
            let (mut a, mut b, mut c) = (a.clone(), b.clone(), vec![0.0f32; n]);
            f(&mut [DataVazby::F32(&mut a), DataVazby::F32(&mut b), DataVazby::F32(&mut c)]);
        },
    );

    let k = [Hodnota32::U32(n as u32), Hodnota32::F32(0.5), Hodnota32::F32(0.25)];
    porovnaj("škáluj 1 M prvkov", "M prvkov/s", n as f64 / 1e6, &kernely::skaluj(), &k, s1, &|f| {
        let (mut x, mut y) = (a.clone(), vec![0.0f32; n]);
        f(&mut [DataVazby::F32(&mut x), DataVazby::F32(&mut y)]);
    });

    let m = 256;
    let (ma, mb) = (vec![0.5f32; m * m], vec![0.25f32; m * m]);
    let (k, s) = ([Hodnota32::U32(m as u32); 3], [(m as u32).div_ceil(16), (m as u32).div_ceil(16), 1]);
    let flop = 2.0 * (m * m * m) as f64 / 1e9;
    porovnaj("násobenie 256×256×256", "GFLOP/s", flop, &kernely::nasobenie_presne(), &k, s, &|f| {
        let (mut a, mut b, mut c) = (ma.clone(), mb.clone(), vec![0.0f32; m * m]);
        f(&mut [DataVazby::F32(&mut a), DataVazby::F32(&mut b), DataVazby::F32(&mut c)]);
    });

    let p = 1 << 16;
    let vf: Vec<f32> = (0..p).map(|i| (i % 97) as f32 / 50.0 - 1.0).collect();
    let vu: Vec<u32> = (0..p).map(|i| (i * 7919) as u32 % 1000).collect();
    let (k, s) = (
        [Hodnota32::U32(p as u32), Hodnota32::F32(0.375), Hodnota32::I32(-11)],
        [(p as u32).div_ceil(32), 1, 1],
    );
    porovnaj(
        "skúška jazyka 64 K vlákien",
        "M vlákien/s",
        p as f64 / 1e6,
        &kernely::skuska_jazyka(),
        &k,
        s,
        &|f| {
            let (mut vf, mut vu) = (vf.clone(), vu.clone());
            let (mut of, mut ou, mut oi) = (vec![0.0f32; p], vec![0u32; p], vec![0i32; p]);
            f(&mut [
                DataVazby::F32(&mut vf),
                DataVazby::U32(&mut vu),
                DataVazby::F32(&mut of),
                DataVazby::U32(&mut ou),
                DataVazby::I32(&mut oi),
            ]);
        },
    );
}
