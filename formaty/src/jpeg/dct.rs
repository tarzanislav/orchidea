//! Diskrétna kosínusová transformácia 8×8 (DCT-II a jej inverzia) v plávajúcej čiarke.
//!
//! Koeficienty bázy sa počítajú deterministickou matematikou jadra, súčty majú pevné poradie – výsledok je
//! bitovo rovnaký na každej platforme. Jednorozmerná transformácia sa delí na párnu a nepárnu časť
//! (`x[7−k]` sa líši od `x[k]` len znamienkom nepárnych členov) – polovica násobení.
//!
//! Zmenšená inverzia (N = 4, 2, 1 bodov z 8×8 koeficientov) počíta hodnoty v stredoch väčších pixelov len
//! z nízkych frekvencií – zmenšenie s vyhladením zadarmo (FOTOSYNTÉZA Z1, Z7: náhľad 1/8 je len DC).

use orchidea_jadro::matematika;
use std::sync::OnceLock;

/// `C[u][x] = c(u)/2 · cos((2x+1)uπ/16)`, c(0) = 1/√2, inak 1 – ortonormálna báza 8 bodov.
fn baza8() -> &'static [[f32; 8]; 8] {
    static B: OnceLock<[[f32; 8]; 8]> = OnceLock::new();
    B.get_or_init(|| baza(8))
}

fn baza(n: usize) -> [[f32; 8]; 8] {
    let mut b = [[0f32; 8]; 8];
    for (u, riadok) in b.iter_mut().enumerate().take(n) {
        let c = if u == 0 { 1.0 / matematika::odmocnina(2.0) } else { 1.0 };
        for (x, v) in riadok.iter_mut().enumerate().take(n) {
            let uhol = ((2 * x + 1) * u) as f64 * std::f64::consts::PI / (2 * n) as f64;
            *v = (c / 2.0 * matematika::cos(uhol)) as f32;
        }
    }
    b
}

/// Bázy zmenšenej inverzie pre N = 4 a 2.
fn baza_zmensena(n: usize) -> &'static [[f32; 8]; 8] {
    static B4: OnceLock<[[f32; 8]; 8]> = OnceLock::new();
    static B2: OnceLock<[[f32; 8]; 8]> = OnceLock::new();
    match n {
        4 => B4.get_or_init(|| baza(4)),
        _ => B2.get_or_init(|| baza(2)),
    }
}

/// Jednorozmerná inverzia 8 bodov s krokom `krok` (riadok alebo stĺpec bloku).
#[inline]
fn inverzia_1d(b: &[[f32; 8]; 8], vstup: &[f32; 64], zaciatok: usize, krok: usize, von: &mut [f32; 64]) {
    let f = |u: usize| vstup[zaciatok + u * krok];
    let (f0, f1, f2, f3, f4, f5, f6, f7) = (f(0), f(1), f(2), f(3), f(4), f(5), f(6), f(7));
    if f1 == 0.0 && f2 == 0.0 && f3 == 0.0 && f4 == 0.0 && f5 == 0.0 && f6 == 0.0 && f7 == 0.0 {
        let d = f0 * b[0][0];
        for k in 0..8 {
            von[zaciatok + k * krok] = d;
        }
        return;
    }
    for k in 0..4 {
        let p = f0 * b[0][k] + f2 * b[2][k] + f4 * b[4][k] + f6 * b[6][k];
        let n = f1 * b[1][k] + f3 * b[3][k] + f5 * b[5][k] + f7 * b[7][k];
        von[zaciatok + k * krok] = p + n;
        von[zaciatok + (7 - k) * krok] = p - n;
    }
}

/// Inverzná DCT bloku 8×8 (koeficienty v prirodzenom poradí, už vynásobené kvantizáciou). Výsledok je bez
/// posunu o polovicu rozsahu.
pub fn inverzia(koef: &[f32; 64], von: &mut [f32; 64]) {
    let b = baza8();
    let mut stlpce = [0f32; 64];
    for x in 0..8 {
        inverzia_1d(b, koef, x, 8, &mut stlpce);
    }
    for y in 0..8 {
        inverzia_1d(b, &stlpce, y * 8, 1, von);
    }
}

/// Zmenšená inverzia: z 8×8 koeficientov N×N bodov (N = 1, 2, 4); výsledok v `von[y * n + x]`.
pub fn inverzia_zmensena(koef: &[f32; 64], n: usize, von: &mut [f32; 64]) {
    if n == 1 {
        von[0] = koef[0] / 8.0;
        return;
    }
    let b = baza_zmensena(n);
    let mut stlpce = [0f32; 64];
    for x in 0..n {
        for k in 0..n {
            let mut s = 0.0;
            for v in 0..n {
                s += koef[v * 8 + x] * b[v][k];
            }
            stlpce[k * 8 + x] = s;
        }
    }
    for y in 0..n {
        for k in 0..n {
            let mut s = 0.0;
            for u in 0..n {
                s += stlpce[y * 8 + u] * b[u][k];
            }
            von[y * n + k] = s;
        }
    }
}

/// Dopredná DCT bloku 8×8 (vzorky už posunuté o polovicu rozsahu). Koeficienty v prirodzenom poradí.
pub fn dopredna(vzorky: &[f32; 64], von: &mut [f32; 64]) {
    let b = baza8();
    let mut riadky = [0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut s = 0.0;
            for x in 0..8 {
                s += vzorky[y * 8 + x] * b[u][x];
            }
            riadky[y * 8 + u] = s;
        }
    }
    for u in 0..8 {
        for v in 0..8 {
            let mut s = 0.0;
            for y in 0..8 {
                s += riadky[y * 8 + u] * b[v][y];
            }
            von[v * 8 + u] = s;
        }
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    #[test]
    fn tam_a_spat_presne() {
        let mut n = Nahoda::zo_semienka(8);
        for _ in 0..200 {
            let mut v = [0f32; 64];
            for x in v.iter_mut() {
                *x = n.mensie_ako(256) as f32 - 128.0;
            }
            let (mut k, mut s) = ([0f32; 64], [0f32; 64]);
            dopredna(&v, &mut k);
            inverzia(&k, &mut s);
            for i in 0..64 {
                assert!((s[i] - v[i]).abs() < 1e-3, "{} vs {}", s[i], v[i]);
            }
        }
    }

    #[test]
    fn zmensena_inverzia_je_priemer_pre_dc() {
        let mut k = [0f32; 64];
        k[0] = 800.0;
        let mut s = [0f32; 64];
        for n in [1, 2, 4] {
            inverzia_zmensena(&k, n, &mut s);
            for &v in &s[..n * n] {
                assert!((v - 100.0).abs() < 1e-4);
            }
        }
        // hladký prechod: zmenšenie 4×4 sa blíži priemerom dvojíc plného výsledku
        let mut n = Nahoda::zo_semienka(3);
        let mut v = [0f32; 64];
        for (i, x) in v.iter_mut().enumerate() {
            *x = (i % 8) as f32 * 10.0 + (i / 8) as f32 * 5.0 + n.mensie_ako(3) as f32;
        }
        let mut kf = [0f32; 64];
        dopredna(&v, &mut kf);
        inverzia_zmensena(&kf, 4, &mut s);
        for y in 0..4 {
            for x in 0..4 {
                let p = (v[2 * y * 8 + 2 * x]
                    + v[2 * y * 8 + 2 * x + 1]
                    + v[(2 * y + 1) * 8 + 2 * x]
                    + v[(2 * y + 1) * 8 + 2 * x + 1])
                    / 4.0;
                assert!((s[y * 4 + x] - p).abs() < 4.0, "{} vs {p}", s[y * 4 + x]);
            }
        }
    }
}
