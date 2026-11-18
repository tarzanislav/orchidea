//! Referenčné jadrá – jednoduché, pevné poradie operácií, bitovo rovnaké na každej platforme.

/// C = A · B (m×k · k×n). Každý prvok C sa sčíta v poradí p = 0, 1, …, k−1 bez FMA.
pub fn nasob(m: usize, n: usize, k: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    for i in 0..m {
        let ci = &mut c[i * n..(i + 1) * n];
        ci.fill(0.0);
        let ai = &a[i * k..(i + 1) * k];
        for (p, &aip) in ai.iter().enumerate() {
            let bp = &b[p * n..(p + 1) * n];
            for (cij, &bpj) in ci.iter_mut().zip(bp) {
                *cij += aip * bpj;
            }
        }
    }
}

/// Skalárny súčin i8 · i8 → i32.
pub fn sucin_i8(a: &[i8], b: &[i8]) -> i32 {
    a.iter().zip(b).map(|(&x, &y)| x as i32 * y as i32).sum()
}
