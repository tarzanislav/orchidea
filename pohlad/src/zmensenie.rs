//! Zmenšenie (prevzorkovanie) obrazu Lanczosovým filtrom s tromi lalokmi v lineárnom svetle.
//!
//! Vzorky sa dekódujú do lineárneho svetla (krivka sRGB, IEC 61966-2-1; plávajúce a lineárne obrazy bez zmeny),
//! farba sa vynásobí alfou, potom oddeliteľný filter po riadkoch a stĺpcoch a späť. Pri zmenšení sa filter
//! rozšíri o pomer (pásmový filter proti aliasu). Okraje: okno sa oreže na obraz a váhy sa znormujú.
//! Lanczos: L(x) = sinc(x) · sinc(x/3) pre |x| < 3 (Duchon 1979; Turkowski, Graphics Gems 1990).

use orchidea_farba::Prenos;
use orchidea_formaty::obraz::{Model, Obraz, Vzorky};
use orchidea_jadro::matematika::sin;
use orchidea_jadro::{Chyba, Vysledok, paralel};

/// Rozmery, aby sa obraz zmestil do štvorca `max` × `max` (nikdy sa nezväčšuje).
pub fn rozmer_do(sirka: usize, vyska: usize, max: usize) -> (usize, usize) {
    let dlhsia = sirka.max(vyska);
    if dlhsia <= max || max == 0 {
        return (sirka, vyska);
    }
    let k = max as f64 / dlhsia as f64;
    (((sirka as f64 * k).round() as usize).max(1), ((vyska as f64 * k).round() as usize).max(1))
}

fn lanczos(x: f64) -> f64 {
    let x = x.abs();
    if x >= 3.0 {
        return 0.0;
    }
    if x < 1e-12 {
        return 1.0;
    }
    let p = std::f64::consts::PI * x;
    3.0 * sin(p) * sin(p / 3.0) / (p * p)
}

/// Váhy pre jednu os: pre každý výstupný bod (prvý vstupný index, váhy).
fn vahy(vstup: usize, vystup: usize) -> Vec<(usize, Vec<f32>)> {
    let mierka = vstup as f64 / vystup as f64;
    let roztiahnutie = mierka.max(1.0);
    let dosah = 3.0 * roztiahnutie;
    (0..vystup)
        .map(|x| {
            let stred = (x as f64 + 0.5) * mierka;
            let od = ((stred - dosah + 0.5).floor().max(0.0)) as usize;
            let po = ((stred + dosah + 0.5).floor() as usize).min(vstup);
            let mut w: Vec<f64> =
                (od..po).map(|i| lanczos((i as f64 + 0.5 - stred) / roztiahnutie)).collect();
            let s: f64 = w.iter().sum();
            if s != 0.0 {
                w.iter_mut().for_each(|v| *v /= s);
            }
            (od, w.into_iter().map(|v| v as f32).collect())
        })
        .collect()
}

/// Zmenší (alebo zväčší) obraz na presné rozmery. Metadáta a popis farby ostanú.
pub fn zmensi(o: &Obraz, sirka: usize, vyska: usize) -> Vysledok<Obraz> {
    if sirka == 0 || vyska == 0 || sirka > 1 << 16 || vyska > 1 << 16 {
        return Err(Chyba::MimoRozsahu(format!("zmenšenie na {sirka} × {vyska}")));
    }
    if sirka == o.sirka && vyska == o.vyska {
        return Ok(o.clone());
    }
    let k = o.kanaly();
    let alfa = o.alfa.then_some(k - 1);
    let farebnych = if o.alfa { k - 1 } else { k };
    let linearne =
        matches!(o.vzorky, Vzorky::F32(_)) || o.farba.linearne || matches!(o.model, Model::Cmyk | Model::Iny);
    let srgb = Prenos::Srgb;
    // dekódovanie do lineárneho svetla (tabuľkou pre celé čísla)
    let tabulka = |max: usize| -> Vec<f32> {
        (0..=max)
            .map(|i| {
                if linearne {
                    i as f32 / max as f32
                } else {
                    srgb.do_linearneho(i as f64 / max as f64) as f32
                }
            })
            .collect()
    };
    let mut v: Vec<f32> = match &o.vzorky {
        Vzorky::U8(d) => {
            let t = tabulka(255);
            d.iter()
                .enumerate()
                .map(|(i, &x)| if Some(i % k) == alfa { x as f32 / 255.0 } else { t[x as usize] })
                .collect()
        }
        Vzorky::U16(d) => {
            let t = tabulka(65535);
            d.iter()
                .enumerate()
                .map(|(i, &x)| if Some(i % k) == alfa { x as f32 / 65535.0 } else { t[x as usize] })
                .collect()
        }
        Vzorky::F32(d) => d.clone(),
    };
    if let Some(a) = alfa {
        for p in v.chunks_exact_mut(k) {
            for c in 0..farebnych {
                p[c] *= p[a];
            }
        }
    }
    // vodorovne
    let vx = vahy(o.sirka, sirka);
    let mut medzi = vec![0f32; sirka * o.vyska * k];
    paralel::pre_kusy(&mut medzi, sirka * k, |y, riadok| {
        let zdroj = &v[y * o.sirka * k..(y + 1) * o.sirka * k];
        for (x, (od, w)) in vx.iter().enumerate() {
            let ciel = &mut riadok[x * k..(x + 1) * k];
            for (j, &wj) in w.iter().enumerate() {
                let s = &zdroj[(od + j) * k..(od + j + 1) * k];
                for c in 0..k {
                    ciel[c] += wj * s[c];
                }
            }
        }
    });
    drop(v);
    // zvislo
    let vy = vahy(o.vyska, vyska);
    let mut von = vec![0f32; sirka * vyska * k];
    paralel::pre_kusy(&mut von, sirka * k, |y, riadok| {
        let (od, w) = &vy[y];
        for (j, &wj) in w.iter().enumerate() {
            let s = &medzi[(od + j) * sirka * k..(od + j + 1) * sirka * k];
            for (d, &x) in riadok.iter_mut().zip(s) {
                *d += wj * x;
            }
        }
    });
    if let Some(a) = alfa {
        for p in von.chunks_exact_mut(k) {
            let al = p[a];
            for x in &mut p[..farebnych] {
                *x = if al > 1e-8 { *x / al } else { 0.0 };
            }
        }
    }
    // späť do pôvodného typu (celé čísla presným zaokrúhlením cez prahy krivky)
    let zakoduj = |max: u32| -> Vec<u32> {
        let prahy: Vec<f32> = (0..max)
            .map(|i| {
                let u = (i as f64 + 0.5) / max as f64;
                (if linearne { u } else { srgb.do_linearneho(u) }) as f32
            })
            .collect();
        let alfa_k = |x: f32| (x.clamp(0.0, 1.0) * max as f32).round() as u32;
        von.iter()
            .enumerate()
            .map(
                |(i, &x)| {
                    if Some(i % k) == alfa { alfa_k(x) } else { prahy.partition_point(|&p| p <= x) as u32 }
                },
            )
            .collect()
    };
    let vzorky = match o.vzorky {
        Vzorky::U8(_) => Vzorky::U8(zakoduj(255).into_iter().map(|x| x as u8).collect()),
        Vzorky::U16(_) => Vzorky::U16(zakoduj(65535).into_iter().map(|x| x as u16).collect()),
        Vzorky::F32(_) => Vzorky::F32(von),
    };
    let mut n = Obraz::novy(sirka, vyska, o.model, o.alfa, vzorky)?;
    n.mena_kanalov = o.mena_kanalov.clone();
    n.bitov = o.bitov;
    n.farba = o.farba.clone();
    n.metadata = o.metadata.clone();
    Ok(n)
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn rozmery() {
        assert_eq!(rozmer_do(4000, 3000, 1600), (1600, 1200));
        assert_eq!(rozmer_do(300, 200, 1600), (300, 200));
        assert_eq!(rozmer_do(3000, 1, 100), (100, 1));
    }

    #[test]
    fn jednofarebny_ostane_jednofarebny() {
        let o =
            Obraz::novy(37, 23, Model::Rgb, true, Vzorky::U8([200u8, 100, 30, 255].repeat(37 * 23))).unwrap();
        let z = zmensi(&o, 11, 7).unwrap();
        let Vzorky::U8(v) = &z.vzorky else { panic!() };
        assert!(v.chunks_exact(4).all(|p| p == [200, 100, 30, 255]), "{:?}", &v[..8]);
        let z = zmensi(&o, 80, 50).unwrap();
        let Vzorky::U8(v) = &z.vzorky else { panic!() };
        assert!(v.chunks_exact(4).all(|p| p == [200, 100, 30, 255]));
    }
}
