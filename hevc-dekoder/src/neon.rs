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

use super::interpolacia::{predvolene_dve_skalarne, predvolene_jedna_skalarne};
use super::obraz::Vzorka;
use core::arch::aarch64::*;

/// Načíta 16 bajtov od `zdroj[i]`; ak za `i` nie je 16 bajtov, cez dočasné pole doplnené nulami (zvyšok riadka).
#[inline(always)]
fn nacitaj_u8x16(zdroj: &[u8], i: usize) -> uint8x16_t {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        if i + 16 <= zdroj.len() {
            // SAFETY: i + 16 ≤ zdroj.len() – načíta sa 16 bajtov vnútri rezu.
            unsafe { vld1q_u8(zdroj.as_ptr().add(i)) }
        } else {
            let mut t = [0u8; 16];
            let n = zdroj.len() - i;
            t[..n].copy_from_slice(&zdroj[i..]);
            // SAFETY: t má 16 bajtov.
            unsafe { vld1q_u8(t.as_ptr()) }
        }
    }
}

/// Načíta 8 bajtov od `zdroj[i]` (menej na konci rezu – doplní nulami).
#[inline(always)]
fn nacitaj_u8x8(zdroj: &[u8], i: usize) -> uint8x8_t {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        if i + 8 <= zdroj.len() {
            // SAFETY: i + 8 ≤ zdroj.len().
            unsafe { vld1_u8(zdroj.as_ptr().add(i)) }
        } else {
            let mut t = [0u8; 8];
            let n = zdroj.len() - i;
            t[..n].copy_from_slice(&zdroj[i..]);
            // SAFETY: t má 8 bajtov.
            unsafe { vld1_u8(t.as_ptr()) }
        }
    }
}

/// Načíta 8 hodnôt i16 od `zdroj[i]` (menej na konci rezu – doplní nulami).
#[inline(always)]
fn nacitaj_s16x8(zdroj: &[i16], i: usize) -> int16x8_t {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        if i + 8 <= zdroj.len() {
            // SAFETY: i + 8 ≤ zdroj.len().
            unsafe { vld1q_s16(zdroj.as_ptr().add(i)) }
        } else {
            let mut t = [0i16; 8];
            let n = zdroj.len() - i;
            t[..n].copy_from_slice(&zdroj[i..]);
            // SAFETY: t má 8 hodnôt.
            unsafe { vld1q_s16(t.as_ptr()) }
        }
    }
}

/// Zapíše 8 hodnôt i16 do `ciel[i..]`; ak ich je v reze menej, len toľko, koľko sa zmestí.
#[inline(always)]
fn uloz_s16x8(ciel: &mut [i16], i: usize, v: int16x8_t) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        if i + 8 <= ciel.len() {
            // SAFETY: i + 8 ≤ ciel.len().
            unsafe { vst1q_s16(ciel.as_mut_ptr().add(i), v) };
        } else {
            let mut t = [0i16; 8];
            // SAFETY: t má 8 hodnôt.
            unsafe { vst1q_s16(t.as_mut_ptr(), v) };
            let n = ciel.len() - i;
            ciel[i..].copy_from_slice(&t[..n]);
        }
    }
}

/// Zapíše 8 vzoriek do `ciel[i..]`; ak ich je v reze menej, len toľko, koľko sa zmestí.
#[inline(always)]
fn uloz_u8x8(ciel: &mut [u8], i: usize, v: uint8x8_t) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        if i + 8 <= ciel.len() {
            // SAFETY: i + 8 ≤ ciel.len().
            unsafe { vst1_u8(ciel.as_mut_ptr().add(i), v) };
        } else {
            let mut t = [0u8; 8];
            // SAFETY: t má 8 bajtov.
            unsafe { vst1_u8(t.as_mut_ptr(), v) };
            let n = ciel.len() - i;
            ciel[i..].copy_from_slice(&t[..n]);
        }
    }
}

/// Vodorovný filter N bodov (N = 4 alebo 8) pre 8 bitov: `ciel[i] = Σ_k koef[k] · zdroj[i + k] − odcitaj`
/// v i16 modulo 2^16 (ako [`vodorovne8_skalarne`]). Aj zvyšok riadka kratší ako 8 vzoriek (napr. farba blokov
/// 8 × 8 so šírkou 4) ide vektorom – dráhy za koncom sa počítajú z núl a nezapíšu sa. `zdroj` môže byť dlhší
/// ako w + N − 1 (zvyšok roviny) – číta sa z neho priamo bez kópie.
pub fn vodorovne8<const N: usize>(zdroj: &[Vzorka], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(N <= 8 && zdroj.len() >= w + N - 1, "HEVC: krátky riadok zdroja");
        let odc = vdupq_n_s16(odcitaj);
        let mut i = 0;
        while i < w {
            let v = nacitaj_u8x16(zdroj, i);
            let (a, b) =
                (vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(v))), vreinterpretq_s16_u16(vmovl_high_u8(v)));
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
            uloz_s16x8(ciel, i, vsubq_s16(s, odc));
            i += 8;
        }
    }
}

/// Zvislý filter N bodov zo vzoriek pre 8 bitov: `ciel[i] = Σ_k koef[k] · riadky[k][i] − odcitaj` v i16
/// modulo 2^16 (ako [`zvislo8_skalarne`]); aj zvyšok kratší ako 8 vzoriek vektorom.
pub fn zvislo8<const N: usize>(riadky: &[&[Vzorka]; N], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(riadky.iter().all(|r| r.len() >= w), "HEVC: krátky riadok zdroja");
        let odc = vdupq_n_s16(odcitaj);
        let mut i = 0;
        while i < w {
            let mut s = vdupq_n_s16(0);
            for k in 0..N {
                let v = nacitaj_u8x8(riadky[k], i);
                s = vmlaq_n_s16(s, vreinterpretq_s16_u16(vmovl_u8(v)), koef[k]);
            }
            uloz_s16x8(ciel, i, vsubq_s16(s, odc));
            i += 8;
        }
    }
}

/// Druhý stupeň 2D interpolácie: `ciel[i] = ((Σ_k koef[k] · riadky[k][i]) >> 6) − 8192`, súčet v i32,
/// výsledok skrátený na i16 (ako [`zvislo_medzi_skalarne`]); aj zvyšok kratší ako 8 hodnôt vektorom.
pub fn zvislo_medzi<const N: usize>(riadky: &[&[i16]; N], koef: &[i16; N], ciel: &mut [i16]) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(riadky.iter().all(|r| r.len() >= w), "HEVC: krátky riadok medzivýsledku");
        let posun = vdupq_n_s32(8192);
        let mut i = 0;
        while i < w {
            let (mut lo, mut hi) = (vdupq_n_s32(0), vdupq_n_s32(0));
            for k in 0..N {
                let v = nacitaj_s16x8(riadky[k], i);
                lo = vmlal_n_s16(lo, vget_low_s16(v), koef[k]);
                hi = vmlal_high_n_s16(hi, v, koef[k]);
            }
            let lo = vsubq_s32(vshrq_n_s32::<6>(lo), posun);
            let hi = vsubq_s32(vshrq_n_s32::<6>(hi), posun);
            uloz_s16x8(ciel, i, vcombine_s16(vmovn_s32(lo), vmovn_s32(hi)));
            i += 8;
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
        let (pred0, pred1) = (&pred0[..w], &pred1[..w]);
        let (pr, sh) = (vdupq_n_s32(pridaj), vdupq_n_s32(-(shift as i32)));
        let mut i = 0;
        while i < w {
            let (a, b) = (nacitaj_s16x8(pred0, i), nacitaj_s16x8(pred1, i));
            let lo = vshlq_s32(vaddq_s32(vaddl_s16(vget_low_s16(a), vget_low_s16(b)), pr), sh);
            let hi = vshlq_s32(vaddq_s32(vaddl_high_s16(a, b), pr), sh);
            uloz_u8x8(ciel, i, na_vzorky(lo, hi));
            i += 8;
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
        let pred = &pred[..w];
        let (pr, sh) = (vdupq_n_s32(pridaj), vdupq_n_s32(-(shift as i32)));
        let mut i = 0;
        while i < w {
            let a = nacitaj_s16x8(pred, i);
            let lo = vshlq_s32(vaddq_s32(vmovl_s16(vget_low_s16(a)), pr), sh);
            let hi = vshlq_s32(vaddq_s32(vmovl_high_s16(a), pr), sh);
            uloz_u8x8(ciel, i, na_vzorky(lo, hi));
            i += 8;
        }
    }
}

#[cfg(test)]
mod testy {
    use super::super::interpolacia::{vodorovne8_skalarne, zvislo_medzi_skalarne, zvislo8_skalarne};
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

                // riadky môžu byť dlhšie ako w (zvyšok roviny) – výsledok sa nesmie zmeniť
                let riadky: Vec<Vec<u8>> = (0..N)
                    .map(|_| {
                        let navyse = (n.dalsie() % 20) as usize;
                        bajty(n, w + navyse)
                    })
                    .collect();
                let r: [&[u8]; N] = std::array::from_fn(|k| riadky[k].as_slice());
                zvislo8(&r, koef, &mut a, odcitaj);
                zvislo8_skalarne(&r, koef, &mut b, odcitaj);
                assert_eq!(a, b, "zvislo N={N} w={w}");

                let medzi: Vec<Vec<i16>> = (0..N)
                    .map(|_| {
                        let navyse = (n.dalsie() % 20) as usize;
                        i16ky(n, w + navyse)
                    })
                    .collect();
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
        for w in [1, 2, 3, 4, 5, 7, 8, 12, 16, 32, 64] {
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
