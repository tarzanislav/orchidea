//! Výpočtové jadrá Neurónu: násobenie matíc v f32, celočíselné skalárne súčiny v i8 a trojhodnotové váhy.
//!
//! Dva režimy:
//! - **Presný** – pevné poradie sčítania, bez spájaného násobenia a sčítania (FMA). Výsledok je bitovo
//!   rovnaký na každej platforme a pri ľubovoľnom počte vlákien (zásada A9, overené v CI).
//! - **Rýchly** – vektorové inštrukcie (AVX2 + FMA na x86-64, NEON na ARM64) zistené za behu. Výsledok
//!   sa od presného líši len zaokrúhlením (overuje sa tolerancia).
//!
//! Celočíselné jadrá (i8, trojhodnotové) sú v oboch režimoch bitovo rovnaké – celé čísla sa nezaokrúhľujú.

pub mod referencne;

#[cfg(target_arch = "aarch64")]
mod arm;
#[cfg(target_arch = "x86_64")]
mod x86;

use orchidea_jadro::paralel;

/// Režim výpočtu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rezim {
    Presny,
    Rychly,
}

/// Najväčšia dĺžka celočíselného skalárneho súčinu (zaručí, že súčet v i32 nepretečie).
pub const NAJDLHSI_SUCIN_I8: usize = 1 << 17;

/// Aké vektorové rozšírenia procesora sa použijú v rýchlom režime.
pub fn vektorove_rozsirenie() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if x86::ma_avx2_fma() {
            return "AVX2 + FMA";
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return "NEON";
    }
    #[allow(unreachable_code)]
    "žiadne (skalárne)"
}

/// Riadkov výstupu v jednom kúsku paralelnej práce.
fn riadkov_na_kus(m: usize, n: usize, k: usize) -> usize {
    let prace_na_riadok = (n * k).max(1);
    let ciel = (1usize << 16) / prace_na_riadok;
    let r = ciel.clamp(4, 256);
    let r = r.div_ceil(4) * 4;
    r.min(m.max(1))
}

/// C (m×n) = A (m×k) · B (k×n), všetko po riadkoch. Prepíše C.
pub fn nasob(rezim: Rezim, m: usize, n: usize, k: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    assert!(a.len() >= m * k && b.len() >= k * n && c.len() >= m * n, "rozmery matíc nesedia");
    let c = &mut c[..m * n];
    if m == 0 || n == 0 {
        return;
    }
    if k == 0 {
        c.fill(0.0);
        return;
    }
    let kus = riadkov_na_kus(m, n, k);
    paralel::pre_kusy(c, kus * n, |i, c_kus| {
        let od = i * kus;
        let riadkov = c_kus.len() / n;
        let a_kus = &a[od * k..(od + riadkov) * k];
        match rezim {
            Rezim::Presny => referencne::nasob(riadkov, n, k, a_kus, b, c_kus),
            Rezim::Rychly => nasob_rychlo(riadkov, n, k, a_kus, b, c_kus),
        }
    });
}

fn nasob_rychlo(m: usize, n: usize, k: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if x86::ma_avx2_fma() {
            return x86::nasob(m, n, k, a, b, c);
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return arm::nasob(m, n, k, a, b, c);
    }
    #[allow(unreachable_code)]
    referencne::nasob(m, n, k, a, b, c)
}

/// Transpozícia (r×s → s×r).
pub fn transponuj(r: usize, s: usize, a: &[f32]) -> Vec<f32> {
    let mut t = vec![0.0f32; r * s];
    const B: usize = 32;
    for i0 in (0..r).step_by(B) {
        for j0 in (0..s).step_by(B) {
            for i in i0..(i0 + B).min(r) {
                for j in j0..(j0 + B).min(s) {
                    t[j * r + i] = a[i * s + j];
                }
            }
        }
    }
    t
}

/// C = op(A) · op(B), kde op je voliteľná transpozícia. A je (m×k) alebo (k×m), B (k×n) alebo (n×k).
#[allow(clippy::too_many_arguments)]
pub fn nasob_t(
    rezim: Rezim,
    ta: bool,
    tb: bool,
    m: usize,
    n: usize,
    k: usize,
    a: &[f32],
    b: &[f32],
    c: &mut [f32],
) {
    let at;
    let a = if ta {
        at = transponuj(k, m, a);
        &at[..]
    } else {
        a
    };
    let bt;
    let b = if tb {
        bt = transponuj(n, k, b);
        &bt[..]
    } else {
        b
    };
    nasob(rezim, m, n, k, a, b, c);
}

/// Skalárny súčin dvoch vektorov i8 v i32 (presný v oboch režimoch).
pub fn sucin_i8(rezim: Rezim, a: &[i8], b: &[i8]) -> i32 {
    assert_eq!(a.len(), b.len());
    assert!(a.len() <= NAJDLHSI_SUCIN_I8, "príliš dlhý celočíselný súčin");
    if rezim == Rezim::Rychly {
        #[cfg(target_arch = "x86_64")]
        {
            if x86::ma_avx2_fma() {
                return x86::sucin_i8(a, b);
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            return arm::sucin_i8(a, b);
        }
    }
    referencne::sucin_i8(a, b)
}

/// Y (m×n, i32) = X (m×k, i8) · Wᵀ, kde W je (n×k, i8) – riadok W patrí jednému výstupu.
pub fn nasob_i8(rezim: Rezim, m: usize, n: usize, k: usize, x: &[i8], w: &[i8], y: &mut [i32]) {
    assert!(x.len() >= m * k && w.len() >= n * k && y.len() >= m * n);
    let y = &mut y[..m * n];
    if n == 0 {
        return;
    }
    let kus = riadkov_na_kus(m, n, k);
    paralel::pre_kusy(y, kus * n, |i, y_kus| {
        let od = i * kus;
        for (r, riadok) in y_kus.chunks_mut(n).enumerate() {
            let xr = &x[(od + r) * k..(od + r + 1) * k];
            for (j, v) in riadok.iter_mut().enumerate() {
                *v = sucin_i8(rezim, xr, &w[j * k..(j + 1) * k]);
            }
        }
    });
}
