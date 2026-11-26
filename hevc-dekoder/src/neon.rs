//! **Ručné NEON pre aarch64** (RPi 3, Zero 2 W, RPi 4 a 5 so 64-bitovým systémom) – jadrá interpolácie pre
//! 8 bitov, krok H5. Každá funkcia počíta **bitovo to isté** ako jej skalárny vzor v [`crate::interpolacia`]
//! (rozhodca, ktorý počíta aj zvyšok riadka kratší ako 8 vzoriek) – overuje to skúška nižšie na náhodných dátach
//! a v CI všetky skúšobné toky normy (MD5 každého obrazu).
//!
//! Prečo ručne: automatická vektorizácia filtra s 8 bodmi číta pre každý bod zvlášť nezarovnaný vektor; tu sa
//! načíta 16 vzoriek raz a posunuté vektory vzniknú v registroch (`EXT`), súčty sú `MLA` s konštantou, výstup
//! s nasýtením (`SQXTUN`, `UQXTN`) – menej inštrukcií aj prístupov do pamäte, teda menej tepla na snímku.
//!
//! `unsafe` je potrebné pre načítanie a zápis cez ukazovatele (`vld1*`, `vst1*`); každý prístup je v rozsahu
//! rezu, čo kontroluje `assert!` na začiatku funkcie a podmienka cyklu (komentáre SAFETY). Telá funkcií sú celé
//! v `unsafe`, aby kód prešiel prekladom bez ohľadu na to, či sú aritmetické funkcie NEON v danej verzii Rustu
//! bezpečné na volanie (od Rustu 1.87 áno) – nadbytočné vnútorné bloky preto nie sú chyba.

#![allow(unsafe_code, unused_unsafe, clippy::needless_range_loop)]

use super::interpolacia::{
    predvolene_dve_skalarne, predvolene_jedna_skalarne, vodorovne8_skalarne, zvislo_medzi_skalarne,
    zvislo8_skalarne,
};
use super::obraz::Vzorka;
use core::arch::aarch64::*;

/// 8 vzoriek `zdroj[i … i + 15]` rozšírených na i16: (dolných 8, horných 8). Ak za koncom nie je 16 bajtov,
/// načíta sa cez dočasné pole (posledný úsek riadka).
#[inline(always)]
fn nacitaj16(zdroj: &[Vzorka], i: usize) -> (int16x8_t, int16x8_t) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let v = if i + 16 <= zdroj.len() {
            // SAFETY: i + 16 ≤ zdroj.len() – načíta sa 16 bajtov vnútri rezu.
            unsafe { vld1q_u8(zdroj.as_ptr().add(i)) }
        } else {
            let mut t = [0u8; 16];
            let n = zdroj.len() - i;
            t[..n].copy_from_slice(&zdroj[i..]);
            // SAFETY: t má 16 bajtov.
            unsafe { vld1q_u8(t.as_ptr()) }
        };
        (vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(v))), vreinterpretq_s16_u16(vmovl_high_u8(v)))
    }
}

/// Vodorovný filter N bodov (N = 4 alebo 8) pre 8 bitov: `ciel[i] = Σ_k koef[k] · zdroj[i + k] − odcitaj`
/// v i16 modulo 2^16 (ako [`vodorovne8_skalarne`]).
pub fn vodorovne8<const N: usize>(zdroj: &[Vzorka], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(N <= 8 && zdroj.len() >= w + N - 1, "HEVC: krátky riadok zdroja");
        let zdroj = &zdroj[..w + N - 1];
        let odc = vdupq_n_s16(odcitaj);
        let mut i = 0;
        while i + 8 <= w {
            let (a, b) = nacitaj16(zdroj, i);
            let mut s = vmulq_n_s16(a, koef[0]);
            s = vmlaq_n_s16(s, vextq_s16::<1>(a, b), koef[1]);
            s = vmlaq_n_s16(s, vextq_s16::<2>(a, b), koef[2]);
            s = vmlaq_n_s16(s, vextq_s16::<3>(a, b), koef[3]);
            if N == 8 {
                s = vmlaq_n_s16(s, vextq_s16::<4>(a, b), koef[4 % N]);
                s = vmlaq_n_s16(s, vextq_s16::<5>(a, b), koef[5 % N]);
                s = vmlaq_n_s16(s, vextq_s16::<6>(a, b), koef[6 % N]);
                s = vmlaq_n_s16(s, vextq_s16::<7>(a, b), koef[7 % N]);
            }
            // SAFETY: i + 8 ≤ w = ciel.len().
            unsafe { vst1q_s16(ciel.as_mut_ptr().add(i), vsubq_s16(s, odc)) };
            i += 8;
        }
        if i < w {
            vodorovne8_skalarne(&zdroj[i..], koef, &mut ciel[i..], odcitaj);
        }
    }
}

/// Zvislý filter N bodov zo vzoriek pre 8 bitov: `ciel[i] = Σ_k koef[k] · riadky[k][i] − odcitaj` v i16
/// modulo 2^16 (ako [`zvislo8_skalarne`]).
pub fn zvislo8<const N: usize>(riadky: &[&[Vzorka]; N], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(riadky.iter().all(|r| r.len() >= w), "HEVC: krátky riadok zdroja");
        let odc = vdupq_n_s16(odcitaj);
        let mut i = 0;
        while i + 8 <= w {
            let mut s = vdupq_n_s16(0);
            for k in 0..N {
                // SAFETY: i + 8 ≤ w ≤ riadky[k].len().
                let v = unsafe { vld1_u8(riadky[k].as_ptr().add(i)) };
                s = vmlaq_n_s16(s, vreinterpretq_s16_u16(vmovl_u8(v)), koef[k]);
            }
            // SAFETY: i + 8 ≤ w = ciel.len().
            unsafe { vst1q_s16(ciel.as_mut_ptr().add(i), vsubq_s16(s, odc)) };
            i += 8;
        }
        if i < w {
            let zvysok: [&[Vzorka]; N] = std::array::from_fn(|k| &riadky[k][i..]);
            zvislo8_skalarne(&zvysok, koef, &mut ciel[i..], odcitaj);
        }
    }
}

/// Druhý stupeň 2D interpolácie: `ciel[i] = ((Σ_k koef[k] · riadky[k][i]) >> 6) − 8192`, súčet v i32,
/// výsledok skrátený na i16 (ako [`zvislo_medzi_skalarne`]).
pub fn zvislo_medzi<const N: usize>(riadky: &[&[i16]; N], koef: &[i16; N], ciel: &mut [i16]) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(riadky.iter().all(|r| r.len() >= w), "HEVC: krátky riadok medzivýsledku");
        let posun = vdupq_n_s32(8192);
        let mut i = 0;
        while i + 8 <= w {
            let (mut lo, mut hi) = (vdupq_n_s32(0), vdupq_n_s32(0));
            for k in 0..N {
                // SAFETY: i + 8 ≤ w ≤ riadky[k].len().
                let v = unsafe { vld1q_s16(riadky[k].as_ptr().add(i)) };
                lo = vmlal_n_s16(lo, vget_low_s16(v), koef[k]);
                hi = vmlal_high_n_s16(hi, v, koef[k]);
            }
            let lo = vsubq_s32(vshrq_n_s32::<6>(lo), posun);
            let hi = vsubq_s32(vshrq_n_s32::<6>(hi), posun);
            // SAFETY: i + 8 ≤ w = ciel.len().
            unsafe { vst1q_s16(ciel.as_mut_ptr().add(i), vcombine_s16(vmovn_s32(lo), vmovn_s32(hi))) };
            i += 8;
        }
        if i < w {
            let zvysok: [&[i16]; N] = std::array::from_fn(|k| &riadky[k][i..]);
            zvislo_medzi_skalarne(&zvysok, koef, &mut ciel[i..]);
        }
    }
}

/// i32 × 8 → 8 vzoriek s orezaním na 0 … 255 (SQXTUN na 16 bitov bez znamienka, UQXTN na 8 bitov).
#[inline(always)]
fn na_vzorky(lo: int32x4_t, hi: int32x4_t) -> uint8x8_t {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe { vqmovn_u16(vcombine_u16(vqmovun_s32(lo), vqmovun_s32(hi))) }
}

/// Predvolené váženie dvoch referencií: `ciel[i] = Clip3(0, max, (p0 + p1 + pridaj) >> shift)` (NEON pre max 255).
pub fn predvolene_dve(pred0: &[i16], pred1: &[i16], ciel: &mut [Vzorka], pridaj: i32, shift: u32, max: i32) {
    if max != 255 {
        predvolene_dve_skalarne(pred0, pred1, ciel, pridaj, shift, max);
        return;
    }
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(pred0.len() >= w && pred1.len() >= w && shift < 32, "HEVC: krátky riadok predikcie");
        let (pr, sh) = (vdupq_n_s32(pridaj), vdupq_n_s32(-(shift as i32)));
        let mut i = 0;
        while i + 8 <= w {
            // SAFETY: i + 8 ≤ w ≤ pred0.len(), pred1.len().
            let (a, b) = unsafe { (vld1q_s16(pred0.as_ptr().add(i)), vld1q_s16(pred1.as_ptr().add(i))) };
            let lo = vshlq_s32(vaddq_s32(vaddl_s16(vget_low_s16(a), vget_low_s16(b)), pr), sh);
            let hi = vshlq_s32(vaddq_s32(vaddl_high_s16(a, b), pr), sh);
            // SAFETY: i + 8 ≤ w = ciel.len().
            unsafe { vst1_u8(ciel.as_mut_ptr().add(i), na_vzorky(lo, hi)) };
            i += 8;
        }
        if i < w {
            predvolene_dve_skalarne(&pred0[i..w], &pred1[i..w], &mut ciel[i..], pridaj, shift, 255);
        }
    }
}

/// Predvolené váženie jednej referencie: `ciel[i] = Clip3(0, max, (p + pridaj) >> shift)` (NEON pre max 255).
pub fn predvolene_jedna(pred: &[i16], ciel: &mut [Vzorka], pridaj: i32, shift: u32, max: i32) {
    if max != 255 {
        predvolene_jedna_skalarne(pred, ciel, pridaj, shift, max);
        return;
    }
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(pred.len() >= w && shift < 32, "HEVC: krátky riadok predikcie");
        let (pr, sh) = (vdupq_n_s32(pridaj), vdupq_n_s32(-(shift as i32)));
        let mut i = 0;
        while i + 8 <= w {
            // SAFETY: i + 8 ≤ w ≤ pred.len().
            let a = unsafe { vld1q_s16(pred.as_ptr().add(i)) };
            let lo = vshlq_s32(vaddq_s32(vmovl_s16(vget_low_s16(a)), pr), sh);
            let hi = vshlq_s32(vaddq_s32(vmovl_high_s16(a), pr), sh);
            // SAFETY: i + 8 ≤ w = ciel.len().
            unsafe { vst1_u8(ciel.as_mut_ptr().add(i), na_vzorky(lo, hi)) };
            i += 8;
        }
        if i < w {
            predvolene_jedna_skalarne(&pred[i..w], &mut ciel[i..], pridaj, shift, 255);
        }
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    const JAS: [[i16; 8]; 3] =
        [[-1, 4, -10, 58, 17, -5, 1, 0], [-1, 4, -11, 40, 40, -11, 4, -1], [0, 1, -5, 17, 58, -10, 4, -1]];
    const FARBA: [[i16; 4]; 3] = [[-2, 58, 10, -2], [-4, 36, 36, -4], [-2, 10, 58, -2]];

    fn bajty(n: &mut Nahoda, dlzka: usize) -> Vec<u8> {
        (0..dlzka).map(|_| (n.dalsie() & 0xff) as u8).collect()
    }

    fn i16ky(n: &mut Nahoda, dlzka: usize) -> Vec<i16> {
        // medzivýsledky prvého stupňa pre 8 bitov ležia v −14 312 … 22 440 (aj odchýlky mimo pre istotu)
        (0..dlzka).map(|_| ((n.dalsie() % 40_000) as i32 - 16_000) as i16).collect()
    }

    fn skus_filter<const N: usize>(n: &mut Nahoda, koef: &[i16; N]) {
        for w in [1, 2, 4, 6, 8, 12, 16, 24, 32, 48, 64] {
            for _ in 0..20 {
                let navyse = (n.dalsie() % 9) as usize;
                let z = bajty(n, w + N - 1 + navyse);
                let odcitaj = if n.dalsie() & 1 == 0 { 0 } else { 8192 };
                let (mut a, mut b) = (vec![0i16; w], vec![0i16; w]);
                vodorovne8(&z, koef, &mut a, odcitaj);
                vodorovne8_skalarne(&z, koef, &mut b, odcitaj);
                assert_eq!(a, b, "vodorovne N={N} w={w}");

                let riadky: Vec<Vec<u8>> = (0..N).map(|_| bajty(n, w)).collect();
                let r: [&[u8]; N] = std::array::from_fn(|k| riadky[k].as_slice());
                zvislo8(&r, koef, &mut a, odcitaj);
                zvislo8_skalarne(&r, koef, &mut b, odcitaj);
                assert_eq!(a, b, "zvislo N={N} w={w}");

                let medzi: Vec<Vec<i16>> = (0..N).map(|_| i16ky(n, w)).collect();
                let m: [&[i16]; N] = std::array::from_fn(|k| medzi[k].as_slice());
                zvislo_medzi(&m, koef, &mut a);
                zvislo_medzi_skalarne(&m, koef, &mut b);
                assert_eq!(a, b, "medzi N={N} w={w}");
            }
        }
    }

    /// NEON a skalárny rozhodca dávajú bitovo to isté (všetky šírky blokov aj zvyšky, náhodné vzorky).
    #[test]
    fn neon_rovnako_ako_skalarne() {
        let mut n = Nahoda::zo_semienka(0x4e45_4f4e);
        for k in &JAS {
            skus_filter(&mut n, k);
        }
        for k in &FARBA {
            skus_filter(&mut n, k);
        }
        for w in [1, 4, 8, 12, 16, 32, 64] {
            for _ in 0..50 {
                let (p0, p1) = (i16ky(&mut n, w), i16ky(&mut n, w));
                let (mut a, mut b) = (vec![0u8; w], vec![0u8; w]);
                predvolene_dve(&p0, &p1, &mut a, 2 * 8192 + 64, 7, 255);
                predvolene_dve_skalarne(&p0, &p1, &mut b, 2 * 8192 + 64, 7, 255);
                assert_eq!(a, b, "dve w={w}");
                predvolene_jedna(&p0, &mut a, 8192 + 32, 6, 255);
                predvolene_jedna_skalarne(&p0, &mut b, 8192 + 32, 6, 255);
                assert_eq!(a, b, "jedna w={w}");
            }
        }
    }
}
