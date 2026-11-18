//! Rýchle jadrá pre ARM64 s NEON (NEON je na ARM64 vždy prítomný).
//!
//! Jediné miesto Neurónu pre ARM s nebezpečným kódom: vektorové načítania a zápisy cez ukazovatele.
//! Každá funkcia na začiatku overí rozmery a všetky prístupy ležia vnútri overených rozsahov.

#![allow(unsafe_code)]

use std::arch::aarch64::*;

/// C = A · B; mikrojadro 4 riadky × 8 stĺpcov, zvyšky skalárne.
pub fn nasob(m: usize, n: usize, k: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    assert!(a.len() >= m * k && b.len() >= k * n && c.len() >= m * n);
    let (pa, pb, pc) = (a.as_ptr(), b.as_ptr(), c.as_mut_ptr());
    let mut i = 0;
    while i + 4 <= m {
        let mut j = 0;
        while j + 8 <= n {
            // SAFETY: všetky indexy sú overené podmienkami cyklov (i + 4 ≤ m, j + 8 ≤ n, p < k).
            unsafe {
                let mut acc = [vdupq_n_f32(0.0); 8];
                for p in 0..k {
                    let b0 = vld1q_f32(pb.add(p * n + j));
                    let b1 = vld1q_f32(pb.add(p * n + j + 4));
                    for r in 0..4 {
                        let av = vdupq_n_f32(*pa.add((i + r) * k + p));
                        acc[2 * r] = vfmaq_f32(acc[2 * r], av, b0);
                        acc[2 * r + 1] = vfmaq_f32(acc[2 * r + 1], av, b1);
                    }
                }
                for r in 0..4 {
                    vst1q_f32(pc.add((i + r) * n + j), acc[2 * r]);
                    vst1q_f32(pc.add((i + r) * n + j + 4), acc[2 * r + 1]);
                }
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
            for (x, &y) in cr.iter_mut().zip(&b[p * n..(p + 1) * n]) {
                *x = arp.mul_add(y, *x);
            }
        }
    }
}

/// Skalárny súčin i8 · i8 → i32 (presný).
pub fn sucin_i8(a: &[i8], b: &[i8]) -> i32 {
    assert_eq!(a.len(), b.len());
    let n = a.len();
    let mut i = 0;
    // SAFETY: i + 16 ≤ n = a.len() = b.len() v každom kroku cyklu.
    let mut s = unsafe {
        let mut acc = vdupq_n_s32(0);
        while i + 16 <= n {
            let va = vld1q_s8(a.as_ptr().add(i));
            let vb = vld1q_s8(b.as_ptr().add(i));
            let nizke = vmull_s8(vget_low_s8(va), vget_low_s8(vb));
            let vysoke = vmull_s8(vget_high_s8(va), vget_high_s8(vb));
            acc = vpadalq_s16(acc, nizke);
            acc = vpadalq_s16(acc, vysoke);
            i += 16;
        }
        vaddvq_s32(acc)
    };
    for j in i..n {
        s += a[j] as i32 * b[j] as i32;
    }
    s
}
