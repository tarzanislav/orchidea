//! Rýchle jadrá pre x86-64 s AVX2 a FMA (zistené za behu).
//!
//! Jediné miesto Neurónu s nebezpečným kódom: vektorové načítania a zápisy cez ukazovatele. Každá funkcia
//! na začiatku overí rozmery a všetky prístupy ležia preukázateľne vnútri overených rozsahov.

#![allow(unsafe_code)]

use std::arch::x86_64::*;
use std::sync::OnceLock;

pub fn ma_avx2_fma() -> bool {
    static MA: OnceLock<bool> = OnceLock::new();
    *MA.get_or_init(|| is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma"))
}

/// C = A · B; mikrojadro 4 riadky × 16 stĺpcov, zvyšky skalárne.
pub fn nasob(m: usize, n: usize, k: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    assert!(a.len() >= m * k && b.len() >= k * n && c.len() >= m * n);
    assert!(ma_avx2_fma());
    // SAFETY: procesor podporuje AVX2 a FMA (overené vyššie), rozmery sú overené.
    unsafe { nasob_avx2(m, n, k, a, b, c) }
}

#[target_feature(enable = "avx2,fma")]
unsafe fn nasob_avx2(m: usize, n: usize, k: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    let (pa, pb, pc) = (a.as_ptr(), b.as_ptr(), c.as_mut_ptr());
    let mut i = 0;
    while i + 4 <= m {
        let mut j = 0;
        while j + 16 <= n {
            let mut acc = [_mm256_setzero_ps(); 8];
            for p in 0..k {
                // SAFETY: p < k, j + 16 ≤ n, teda p·n + j + 15 < k·n ≤ b.len().
                let (b0, b1) =
                    unsafe { (_mm256_loadu_ps(pb.add(p * n + j)), _mm256_loadu_ps(pb.add(p * n + j + 8))) };
                for r in 0..4 {
                    // SAFETY: i + r < m, p < k, teda (i + r)·k + p < m·k ≤ a.len().
                    let av = _mm256_set1_ps(unsafe { *pa.add((i + r) * k + p) });
                    acc[2 * r] = _mm256_fmadd_ps(av, b0, acc[2 * r]);
                    acc[2 * r + 1] = _mm256_fmadd_ps(av, b1, acc[2 * r + 1]);
                }
            }
            for r in 0..4 {
                // SAFETY: (i + r)·n + j + 15 < m·n ≤ c.len().
                unsafe {
                    _mm256_storeu_ps(pc.add((i + r) * n + j), acc[2 * r]);
                    _mm256_storeu_ps(pc.add((i + r) * n + j + 8), acc[2 * r + 1]);
                }
            }
            j += 16;
        }
        while j + 8 <= n {
            let mut acc = [_mm256_setzero_ps(); 4];
            for p in 0..k {
                // SAFETY: ako vyššie, j + 8 ≤ n.
                let b0 = unsafe { _mm256_loadu_ps(pb.add(p * n + j)) };
                for (r, ac) in acc.iter_mut().enumerate() {
                    let av = _mm256_set1_ps(unsafe { *pa.add((i + r) * k + p) });
                    *ac = _mm256_fmadd_ps(av, b0, *ac);
                }
            }
            for (r, ac) in acc.iter().enumerate() {
                // SAFETY: (i + r)·n + j + 7 < m·n.
                unsafe { _mm256_storeu_ps(pc.add((i + r) * n + j), *ac) };
            }
            j += 8;
        }
        for r in i..i + 4 {
            for jj in j..n {
                let mut s = 0.0f32;
                for p in 0..k {
                    s = a[r * k + p].mul_add(b[p * n + jj], s);
                }
                c[r * n + jj] = s;
            }
        }
        i += 4;
    }
    for r in i..m {
        let cr = &mut c[r * n..(r + 1) * n];
        cr.fill(0.0);
        for p in 0..k {
            let arp = a[r * k + p];
            let bp = &b[p * n..(p + 1) * n];
            for (x, &y) in cr.iter_mut().zip(bp) {
                *x = arp.mul_add(y, *x);
            }
        }
    }
}

/// Skalárny súčin i8 · i8 → i32 (presný).
pub fn sucin_i8(a: &[i8], b: &[i8]) -> i32 {
    assert_eq!(a.len(), b.len());
    assert!(ma_avx2_fma());
    // SAFETY: procesor podporuje AVX2, dĺžky sú rovnaké.
    unsafe { sucin_i8_avx2(a, b) }
}

#[target_feature(enable = "avx2")]
unsafe fn sucin_i8_avx2(a: &[i8], b: &[i8]) -> i32 {
    let n = a.len();
    let mut acc = _mm256_setzero_si256();
    let mut i = 0;
    while i + 16 <= n {
        // SAFETY: i + 16 ≤ n = a.len() = b.len().
        let (va, vb) = unsafe {
            (
                _mm256_cvtepi8_epi16(_mm_loadu_si128(a.as_ptr().add(i) as *const __m128i)),
                _mm256_cvtepi8_epi16(_mm_loadu_si128(b.as_ptr().add(i) as *const __m128i)),
            )
        };
        acc = _mm256_add_epi32(acc, _mm256_madd_epi16(va, vb));
        i += 16;
    }
    let mut casti = [0i32; 8];
    // SAFETY: zápis 32 bajtov do poľa s 8 × i32.
    unsafe { _mm256_storeu_si256(casti.as_mut_ptr() as *mut __m256i, acc) };
    let mut s: i32 = casti.iter().sum();
    for j in i..n {
        s += a[j] as i32 * b[j] as i32;
    }
    s
}
