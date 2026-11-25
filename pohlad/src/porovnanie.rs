//! Porovnanie dvoch obrázkov: miery rozdielu (najväčší a priemerný rozdiel, PSNR, SSIM) a obrazy na
//! zobrazenie (zosilnený rozdiel, vedľa seba, delený pohľad).
//!
//! SSIM podľa Wang, Bovik, Sheikh, Simoncelli (IEEE TIP 2004): Gaussovo okno σ = 1,5 (polomer 5), K1 = 0,01,
//! K2 = 0,03, rozsah hodnôt podľa typu vzoriek; priemer cez body, kde sa okno celé zmestí do obrazu, a cez kanály.
//! Porovnávajú sa uložené (kódované) hodnoty – ako ich vidí displej; obrázky musia mať rovnaké rozmery a kanály.

use orchidea_formaty::obraz::{Model, Obraz, Vzorky};
use orchidea_jadro::matematika::{exp, log10};
use orchidea_jadro::{Chyba, Vysledok, paralel};

/// Výsledok porovnania.
#[derive(Debug, Clone, PartialEq)]
pub struct Miery {
    /// Najväčší absolútny rozdiel (v jednotkách vzoriek).
    pub najviac: f64,
    /// Priemerný absolútny rozdiel.
    pub priemer: f64,
    /// Odstup signálu od šumu v dB (`f64::INFINITY` pri zhode).
    pub psnr: f64,
    /// Štrukturálna podobnosť 0–1 (1 = zhoda); `None`, ak je obraz menší ako okno 11 × 11.
    pub ssim: Option<f64>,
    /// Počet bodov, ktoré sa líšia aspoň v jednom kanáli.
    pub rozdielnych_bodov: usize,
}

fn vzorky(o: &Obraz) -> (Vec<f64>, f64) {
    match &o.vzorky {
        Vzorky::U8(d) => (d.iter().map(|&x| x as f64).collect(), 255.0),
        Vzorky::U16(d) => (d.iter().map(|&x| x as f64).collect(), 65535.0),
        Vzorky::F32(d) => (d.iter().map(|&x| x as f64).collect(), 1.0),
    }
}

fn zhodne(a: &Obraz, b: &Obraz) -> Vysledok<()> {
    if a.sirka != b.sirka || a.vyska != b.vyska || a.kanaly() != b.kanaly() {
        return Err(Chyba::NezhodaTvaru(format!(
            "porovnanie: {}×{}×{} a {}×{}×{}",
            a.sirka,
            a.vyska,
            a.kanaly(),
            b.sirka,
            b.vyska,
            b.kanaly()
        )));
    }
    Ok(())
}

/// Porovná dva obrázky rovnakého tvaru.
pub fn porovnaj(a: &Obraz, b: &Obraz) -> Vysledok<Miery> {
    zhodne(a, b)?;
    let ((x, rozsah), (y, _)) = (vzorky(a), vzorky(b));
    let k = a.kanaly();
    let (mut najviac, mut sucet, mut sucet2, mut rozdielnych) = (0f64, 0f64, 0f64, 0usize);
    for (p, q) in x.chunks_exact(k).zip(y.chunks_exact(k)) {
        let mut iny = false;
        for (u, v) in p.iter().zip(q) {
            let d = (u - v).abs();
            najviac = najviac.max(d);
            sucet += d;
            sucet2 += d * d;
            iny |= d != 0.0;
        }
        rozdielnych += iny as usize;
    }
    let n = x.len().max(1) as f64;
    let mse = sucet2 / n;
    let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * log10(rozsah * rozsah / mse) };
    let ssim = ssim(&x, &y, a.sirka, a.vyska, k, rozsah);
    Ok(Miery { najviac, priemer: sucet / n, psnr, ssim, rozdielnych_bodov: rozdielnych })
}

/// SSIM priemerovaný cez kanály.
fn ssim(x: &[f64], y: &[f64], w: usize, h: usize, k: usize, rozsah: f64) -> Option<f64> {
    const R: usize = 5;
    if w < 2 * R + 1 || h < 2 * R + 1 {
        return None;
    }
    let sigma = 1.5f64;
    let mut g: Vec<f64> =
        (0..=2 * R).map(|i| exp(-((i as f64 - R as f64).powi(2)) / (2.0 * sigma * sigma))).collect();
    let s: f64 = g.iter().sum();
    g.iter_mut().for_each(|v| *v /= s);
    let (c1, c2) = ((0.01 * rozsah).powi(2), (0.03 * rozsah).powi(2));
    let (vw, vh) = (w - 2 * R, h - 2 * R);
    let mut spolu = 0.0;
    for c in 0..k {
        // vodorovné rozmazanie piatich veličín (x, y, x², y², xy), potom zvislé len pre vnútorné body
        let mut vod = vec![[0f64; 5]; vw * h];
        paralel::pre_kusy(&mut vod, vw, |r, riadok| {
            for (i, o) in riadok.iter_mut().enumerate() {
                for (t, &gt) in g.iter().enumerate() {
                    let j = (r * w + i + t) * k + c;
                    let (a, b) = (x[j], y[j]);
                    o[0] += gt * a;
                    o[1] += gt * b;
                    o[2] += gt * a * a;
                    o[3] += gt * b * b;
                    o[4] += gt * a * b;
                }
            }
        });
        let mut riadky = vec![0f64; vh];
        paralel::pre_kusy(&mut riadky, 1, |r, s| {
            let mut sucet = 0.0;
            for i in 0..vw {
                let mut m = [0f64; 5];
                for (t, &gt) in g.iter().enumerate() {
                    let v = &vod[(r + t) * vw + i];
                    for q in 0..5 {
                        m[q] += gt * v[q];
                    }
                }
                let (mx, my) = (m[0], m[1]);
                let (vx, vy, cxy) = (m[2] - mx * mx, m[3] - my * my, m[4] - mx * my);
                sucet +=
                    ((2.0 * mx * my + c1) * (2.0 * cxy + c2)) / ((mx * mx + my * my + c1) * (vx + vy + c2));
            }
            s[0] = sucet;
        });
        spolu += riadky.iter().sum::<f64>() / (vw * vh) as f64;
    }
    Some(spolu / k as f64)
}

/// Obraz rozdielu na zobrazenie: |a − b| zosilnený `zosilnenie`-krát, 8-bitový (sivý pri jednom kanáli,
/// inak RGB z prvých troch kanálov; alfa sa vynechá).
pub fn obraz_rozdielu(a: &Obraz, b: &Obraz, zosilnenie: f64) -> Vysledok<Obraz> {
    zhodne(a, b)?;
    let ((x, rozsah), (y, _)) = (vzorky(a), vzorky(b));
    let k = a.kanaly();
    let farebnych = if a.alfa { k - 1 } else { k };
    let von_k = if farebnych >= 3 { 3 } else { 1 };
    let mut v = Vec::with_capacity(a.sirka * a.vyska * von_k);
    for (p, q) in x.chunks_exact(k).zip(y.chunks_exact(k)) {
        for c in 0..von_k {
            let d = (p[c] - q[c]).abs() / rozsah * 255.0 * zosilnenie;
            v.push(d.round().min(255.0) as u8);
        }
    }
    Obraz::novy(a.sirka, a.vyska, if von_k == 3 { Model::Rgb } else { Model::Siva }, false, Vzorky::U8(v))
}

/// Delený pohľad: vľavo od stĺpca `deliaca` obraz `a`, vpravo `b` (rovnaký tvar).
pub fn deleny(a: &Obraz, b: &Obraz, deliaca: usize) -> Vysledok<Obraz> {
    zhodne(a, b)?;
    let k = a.kanaly();
    let d = deliaca.min(a.sirka) * k;
    let rad = a.sirka * k;
    fn spoj<T: Copy>(x: &[T], y: &[T], rad: usize, d: usize) -> Vec<T> {
        x.chunks_exact(rad)
            .zip(y.chunks_exact(rad))
            .flat_map(|(p, q)| p[..d].iter().chain(&q[d..]).copied())
            .collect()
    }
    let vz = match (&a.vzorky, &b.vzorky) {
        (Vzorky::U8(x), Vzorky::U8(y)) => Vzorky::U8(spoj(x, y, rad, d)),
        (Vzorky::U16(x), Vzorky::U16(y)) => Vzorky::U16(spoj(x, y, rad, d)),
        (Vzorky::F32(x), Vzorky::F32(y)) => Vzorky::F32(spoj(x, y, rad, d)),
        _ => return Err(Chyba::NezhodaTvaru("porovnanie: rôzne typy vzoriek".into())),
    };
    let mut o = Obraz::novy(a.sirka, a.vyska, a.model, a.alfa, vz)?;
    o.farba = a.farba.clone();
    Ok(o)
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn obraz(f: impl Fn(usize, usize) -> u8) -> Obraz {
        let v: Vec<u8> = (0..32 * 24).flat_map(|i| [f(i % 32, i / 32); 3]).collect();
        Obraz::novy(32, 24, Model::Rgb, false, Vzorky::U8(v)).unwrap()
    }

    #[test]
    fn zhoda_a_rozdiel() {
        let a = obraz(|x, y| (x * 7 + y * 3) as u8);
        let m = porovnaj(&a, &a).unwrap();
        assert_eq!((m.najviac, m.psnr, m.rozdielnych_bodov), (0.0, f64::INFINITY, 0));
        assert!((m.ssim.unwrap() - 1.0).abs() < 1e-12);
        let b = obraz(|x, y| (x * 7 + y * 3) as u8 ^ ((x == 3) as u8));
        let m = porovnaj(&a, &b).unwrap();
        assert_eq!((m.najviac, m.rozdielnych_bodov), (1.0, 24));
        assert!(m.ssim.unwrap() < 1.0 && m.psnr > 40.0);
        let r = obraz_rozdielu(&a, &b, 10.0).unwrap();
        let Vzorky::U8(v) = &r.vzorky else { panic!() };
        assert_eq!(v[3 * 3], 10);
        let d = deleny(&a, &b, 16).unwrap();
        assert_eq!(porovnaj(&d, &a).unwrap().rozdielnych_bodov, 0);
    }
}
