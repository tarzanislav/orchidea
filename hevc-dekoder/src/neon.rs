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

/// Načíta 16 bajtov od `zdroj[i]`; ak za `i` nie je 16 bajtov, cez dočasné pole doplnené nulami (koniec oblasti).
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

/// Načíta 8 bajtov od `zdroj[i]` (ak ich v reze nie je 8, cez dočasné pole doplnené nulami).
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

/// Dolné 4 vzorky (z 8) do `ciel[i … i + 3]` – kópia pevnej dĺžky, prekladač ju zapíše jednou inštrukciou.
#[inline(always)]
fn uloz4_u8(ciel: &mut [u8], i: usize, v: uint8x8_t) {
    let mut t = [0u8; 8];
    // SAFETY: t má 8 bajtov.
    unsafe { vst1_u8(t.as_mut_ptr(), v) };
    ciel[i..i + 4].copy_from_slice(&t[..4]);
}

/// Súčet vodorovného filtra N bodov pre 8 dráh zo 16 vzoriek `v` (dráha j = Σ_k koef[k] · v[j + k], i16).
#[inline(always)]
fn vodorovne_drahy<const N: usize>(v: uint8x16_t, koef: &[i16; N]) -> int16x8_t {
    // SAFETY: len aritmetika v registroch.
    unsafe {
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
        s
    }
}

/// Vodorovný filter N bodov (N = 4 alebo 8) pre 8 bitov: `ciel[i] = Σ_k koef[k] · zdroj[i + k] − odcitaj`
/// v i16 modulo 2^16 (ako [`vodorovne8_skalarne`]). Po 8 vzorkách, zvyšok 4 vzoriek (farba blokov 8 × 8) tiež
/// vektorom, menej ako 4 skalárne. `zdroj` môže byť dlhší ako w + N − 1 (zvyšok roviny) – číta sa priamo.
pub fn vodorovne8<const N: usize>(zdroj: &[Vzorka], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        let w = ciel.len();
        assert!(N <= 8 && zdroj.len() >= w + N - 1, "HEVC: krátky riadok zdroja");
        let odc = vdupq_n_s16(odcitaj);
        let mut i = 0;
        while i + 8 <= w {
            let s = vodorovne_drahy(nacitaj_u8x16(zdroj, i), koef);
            // SAFETY: i + 8 ≤ w = ciel.len().
            unsafe { vst1q_s16(ciel.as_mut_ptr().add(i), vsubq_s16(s, odc)) };
            i += 8;
        }
        if i + 4 <= w {
            let s = vodorovne_drahy(nacitaj_u8x16(zdroj, i), koef);
            // SAFETY: i + 4 ≤ w = ciel.len() – zapíšu sa 4 hodnoty.
            unsafe { vst1_s16(ciel.as_mut_ptr().add(i), vget_low_s16(vsubq_s16(s, odc))) };
            i += 4;
        }
        if i < w {
            vodorovne8_skalarne(&zdroj[i..], koef, &mut ciel[i..], odcitaj);
        }
    }
}

/// Zvislý filter N bodov zo vzoriek pre 8 bitov: `ciel[i] = Σ_k koef[k] · riadky[k][i] − odcitaj` v i16
/// modulo 2^16 (ako [`zvislo8_skalarne`]); zvyšok 4 vzoriek vektorom, menej ako 4 skalárne.
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
        if i + 4 <= w {
            let mut s = vdupq_n_s16(0);
            for k in 0..N {
                let v = nacitaj_u8x8(riadky[k], i);
                s = vmlaq_n_s16(s, vreinterpretq_s16_u16(vmovl_u8(v)), koef[k]);
            }
            // SAFETY: i + 4 ≤ w = ciel.len() – zapíšu sa 4 hodnoty.
            unsafe { vst1_s16(ciel.as_mut_ptr().add(i), vget_low_s16(vsubq_s16(s, odc))) };
            i += 4;
        }
        if i < w {
            let zvysok: [&[Vzorka]; N] = std::array::from_fn(|k| &riadky[k][i..]);
            zvislo8_skalarne(&zvysok, koef, &mut ciel[i..], odcitaj);
        }
    }
}

/// Druhý stupeň 2D interpolácie: `ciel[i] = ((Σ_k koef[k] · riadky[k][i]) >> 6) − 8192`, súčet v i32,
/// výsledok skrátený na i16 (ako [`zvislo_medzi_skalarne`]); zvyšok 4 hodnôt vektorom, menej ako 4 skalárne.
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
        if i + 4 <= w {
            let mut lo = vdupq_n_s32(0);
            for k in 0..N {
                // SAFETY: i + 4 ≤ w ≤ riadky[k].len() – načítajú sa 4 hodnoty.
                let v = unsafe { vld1_s16(riadky[k].as_ptr().add(i)) };
                lo = vmlal_n_s16(lo, v, koef[k]);
            }
            let lo = vsubq_s32(vshrq_n_s32::<6>(lo), posun);
            // SAFETY: i + 4 ≤ w = ciel.len() – zapíšu sa 4 hodnoty.
            unsafe { vst1_s16(ciel.as_mut_ptr().add(i), vmovn_s32(lo)) };
            i += 4;
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
        if i + 4 <= w {
            // SAFETY: i + 4 ≤ w ≤ pred0.len(), pred1.len() – načítajú sa 4 hodnoty.
            let (a, b) = unsafe { (vld1_s16(pred0.as_ptr().add(i)), vld1_s16(pred1.as_ptr().add(i))) };
            let lo = vshlq_s32(vaddq_s32(vaddl_s16(a, b), pr), sh);
            uloz4_u8(ciel, i, na_vzorky(lo, lo));
            i += 4;
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
        if i + 4 <= w {
            // SAFETY: i + 4 ≤ w ≤ pred.len() – načítajú sa 4 hodnoty.
            let a = unsafe { vld1_s16(pred.as_ptr().add(i)) };
            let lo = vshlq_s32(vaddq_s32(vmovl_s16(a), pr), sh);
            uloz4_u8(ciel, i, na_vzorky(lo, lo));
            i += 4;
        }
        if i < w {
            predvolene_jedna_skalarne(&pred[i..w], &mut ciel[i..], pridaj, shift, 255);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Odblokovanie jasu (8.7.2.5.3, 8.7.2.5.6, 8.7.2.5.7): jeden úsek hrany = 4 línie v dráhach int16x4. Na Cortex-A53
// má 64-bitový vektor plnú priepustnosť a 128-bitový polovičnú – 4 dráhy na úsek preto nič nestrácajú. Rozhodnutia
// (dE, dEp, dEq) sa berú z línií 0 a 3 ako v norme a ich hodnoty sa vyberú z dráh; filtre idú pre všetky 4 línie
// naraz, rozhodnutie slabého filtra po líniách (|Δ| < 10 · tC) je maska.
// ---------------------------------------------------------------------------------------------------------------

/// Rozhodnutia a filter úseku jasu (ako `filtruj_usek_jasu` v module `odblokovanie`): `v` = p3, p2, p1, p0, q0,
/// q1, q2, q3 (dráha = línia, hodnoty 0 … 255). Vráti nové hodnoty (pred orezaním Clip1, ktoré urobí zúženie
/// s nasýtením pri zápise) alebo `None`, ak sa úsek nefiltruje (d ≥ β). `bez` = [strana P, strana Q] sa nemení.
#[inline(always)]
fn filter_jasu(v: [int16x4_t; 8], beta: i32, tc: i32, bez: [bool; 2]) -> Option<[int16x4_t; 8]> {
    // SAFETY: len aritmetika v registroch.
    unsafe {
        let [p3, p2, p1, p0, q0, q1, q2, q3] = v;
        // 8.7.2.5.3: druhé diferencie, rozhodnutie dE z línií 0 a 3
        let dp = vabs_s16(vadd_s16(vsub_s16(p2, vadd_s16(p1, p1)), p0));
        let dq = vabs_s16(vadd_s16(vsub_s16(q2, vadd_s16(q1, q1)), q0));
        let (dp0, dp3) = (i32::from(vget_lane_s16::<0>(dp)), i32::from(vget_lane_s16::<3>(dp)));
        let (dq0, dq3) = (i32::from(vget_lane_s16::<0>(dq)), i32::from(vget_lane_s16::<3>(dq)));
        let (dpq0, dpq3) = (dp0 + dq0, dp3 + dq3);
        if dpq0 + dpq3 >= beta {
            return None;
        }
        // 8.7.2.5.6: dSam pre línie 0 a 3
        let okraj = vadd_s16(vabd_s16(p3, p0), vabd_s16(q0, q3));
        let skok = vabd_s16(p0, q0);
        let silna = |okraj: i16, skok: i16, dpq: i32| {
            2 * dpq < (beta >> 2) && i32::from(okraj) < (beta >> 3) && i32::from(skok) < ((5 * tc + 1) >> 1)
        };
        let mut n = v;
        if silna(vget_lane_s16::<0>(okraj), vget_lane_s16::<0>(skok), dpq0)
            && silna(vget_lane_s16::<3>(okraj), vget_lane_s16::<3>(skok), dpq3)
        {
            // dE = 2: silný filter (8.7.2.5.7), každá nová hodnota v p ± 2 · tC; súčty ≤ 8 · 255 sú v i16
            let t2 = vdup_n_s16((2 * tc) as i16);
            let orez = |x: int16x4_t, s: int16x4_t| vmax_s16(vmin_s16(x, vadd_s16(s, t2)), vsub_s16(s, t2));
            let sp = vadd_s16(vadd_s16(p1, p0), q0);
            let sq = vadd_s16(vadd_s16(p0, q0), q1);
            n[3] = orez(vrshr_n_s16::<3>(vadd_s16(vadd_s16(p2, q1), vadd_s16(sp, sp))), p0);
            n[2] = orez(vrshr_n_s16::<2>(vadd_s16(p2, sp)), p1);
            n[1] = orez(vrshr_n_s16::<3>(vadd_s16(vadd_s16(sp, p2), vshl_n_s16::<1>(vadd_s16(p3, p2)))), p2);
            n[4] = orez(vrshr_n_s16::<3>(vadd_s16(vadd_s16(p1, q2), vadd_s16(sq, sq))), q0);
            n[5] = orez(vrshr_n_s16::<2>(vadd_s16(sq, q2)), q1);
            n[6] = orez(vrshr_n_s16::<3>(vadd_s16(vadd_s16(sq, q2), vshl_n_s16::<1>(vadd_s16(q2, q3)))), q2);
        } else {
            // dE = 1: slabý filter, Δ = (9 · (q0 − p0) − 3 · (q1 − p1) + 8) >> 4; línia s |Δ| ≥ 10 · tC sa nemení
            let prah = (beta + (beta >> 1)) >> 3;
            let (dep, deq) = (dp0 + dp3 < prah, dq0 + dq3 < prah);
            let t1 = vdup_n_s16(tc as i16);
            let delta = vrshr_n_s16::<4>(vmls_n_s16(vmul_n_s16(vsub_s16(q0, p0), 9), vsub_s16(q1, p1), 3));
            let maska = vclt_s16(vabs_s16(delta), vdup_n_s16((10 * tc) as i16));
            let d = vmax_s16(vmin_s16(delta, t1), vneg_s16(t1));
            n[3] = vbsl_s16(maska, vadd_s16(p0, d), p0);
            n[4] = vbsl_s16(maska, vsub_s16(q0, d), q0);
            let t2 = vdup_n_s16((tc >> 1) as i16);
            let orez = |x: int16x4_t| vmax_s16(vmin_s16(x, t2), vneg_s16(t2));
            if dep {
                // Δp = Clip3(−(tC >> 1), tC >> 1, ((((p2 + p0 + 1) >> 1) − p1 + Δ) >> 1)
                let dp = orez(vshr_n_s16::<1>(vadd_s16(vsub_s16(vrhadd_s16(p2, p0), p1), d)));
                n[2] = vbsl_s16(maska, vadd_s16(p1, dp), p1);
            }
            if deq {
                let dq = orez(vshr_n_s16::<1>(vsub_s16(vsub_s16(vrhadd_s16(q2, q0), q1), d)));
                n[5] = vbsl_s16(maska, vadd_s16(q1, dq), q1);
            }
        }
        if bez[0] {
            (n[1], n[2], n[3]) = (p2, p1, p0);
        }
        if bez[1] {
            (n[4], n[5], n[6]) = (q0, q1, q2);
        }
        Some(n)
    }
}

/// 8 vzoriek bez znamienka → dolná a horná štvorica ako i16.
#[inline(always)]
fn rozsir_u8x8(v: uint8x8_t) -> (int16x4_t, int16x4_t) {
    // SAFETY: len aritmetika v registroch.
    unsafe {
        let w = vreinterpretq_s16_u16(vmovl_u8(v));
        (vget_low_s16(w), vget_high_s16(w))
    }
}

/// Úsek hrany jasu pre 8 bitov (Clip1Y = 255), bitovo ako `odblokovanie::odblokuj_usek_jasu_skalarne`: p3 prvej
/// línie je `data[i]`; zvislá hrana – línia k je riadok `i + k · krok` (p3 … q3 za sebou), vodorovná – línia k je
/// stĺpec `i + k` (vzorky po `krok`). Vráti `true` – úsek je vždy spracovaný.
pub fn odblokuj_jas(
    data: &mut [Vzorka],
    i: usize,
    krok: usize,
    zvisla: bool,
    beta: i32,
    tc: i32,
    bez: [bool; 2],
) -> bool {
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        if zvisla {
            assert!(i + 3 * krok + 8 <= data.len(), "HEVC: úsek hrany jasu mimo roviny");
            let z = data.as_ptr();
            // SAFETY: riadok k = 0 … 3 číta 8 vzoriek od i + k · krok ≤ data.len() − 8 (assert vyššie).
            let r: [uint8x8_t; 4] = std::array::from_fn(|k| unsafe { vld1_u8(z.add(i + k * krok)) });
            // transpozícia 4 × 8: stĺpce (p3 | q0), (p1 | q2), (p2 | q1), (p0 | q3), v každom 4 línie
            let (t0, t1) =
                (vreinterpret_u16_u8(vtrn1_u8(r[0], r[1])), vreinterpret_u16_u8(vtrn2_u8(r[0], r[1])));
            let (t2, t3) =
                (vreinterpret_u16_u8(vtrn1_u8(r[2], r[3])), vreinterpret_u16_u8(vtrn2_u8(r[2], r[3])));
            let (p3, q0) = rozsir_u8x8(vreinterpret_u8_u16(vtrn1_u16(t0, t2)));
            let (p1, q2) = rozsir_u8x8(vreinterpret_u8_u16(vtrn2_u16(t0, t2)));
            let (p2, q1) = rozsir_u8x8(vreinterpret_u8_u16(vtrn1_u16(t1, t3)));
            let (p0, q3) = rozsir_u8x8(vreinterpret_u8_u16(vtrn2_u16(t1, t3)));
            let Some([p3, p2, p1, p0, q0, q1, q2, q3]) =
                filter_jasu([p3, p2, p1, p0, q0, q1, q2, q3], beta, tc, bez)
            else {
                return true;
            };
            // späť: zúženie s nasýtením (Clip1Y) a opačná transpozícia
            let u0 = vreinterpret_u16_u8(vqmovun_s16(vcombine_s16(p3, q0)));
            let u1 = vreinterpret_u16_u8(vqmovun_s16(vcombine_s16(p1, q2)));
            let u2 = vreinterpret_u16_u8(vqmovun_s16(vcombine_s16(p2, q1)));
            let u3 = vreinterpret_u16_u8(vqmovun_s16(vcombine_s16(p0, q3)));
            let (t0, t2) = (vreinterpret_u8_u16(vtrn1_u16(u0, u1)), vreinterpret_u8_u16(vtrn2_u16(u0, u1)));
            let (t1, t3) = (vreinterpret_u8_u16(vtrn1_u16(u2, u3)), vreinterpret_u8_u16(vtrn2_u16(u2, u3)));
            let w = [vtrn1_u8(t0, t1), vtrn2_u8(t0, t1), vtrn1_u8(t2, t3), vtrn2_u8(t2, t3)];
            let c = data.as_mut_ptr();
            for (k, &w) in w.iter().enumerate() {
                // SAFETY: rovnaké miesto ako čítanie vyššie (8 vzoriek od i + k · krok); p3 a q3 sa nemenia.
                unsafe { vst1_u8(c.add(i + k * krok), w) };
            }
        } else {
            assert!(i + 7 * krok + 4 <= data.len(), "HEVC: úsek hrany jasu mimo roviny");
            // riadok j = p3 … q3, dráha k = stĺpec i + k (číta sa 8 vzoriek, použijú sa 4)
            let v: [int16x4_t; 8] = std::array::from_fn(|j| rozsir_u8x8(nacitaj_u8x8(data, i + j * krok)).0);
            let Some(n) = filter_jasu(v, beta, tc, bez) else {
                return true;
            };
            // p3 a q3 sa nemenia – zapíšu sa riadky p2 … q2
            for (j, &n) in n.iter().enumerate().take(7).skip(1) {
                uloz4_u8(data, i + j * krok, vqmovun_s16(vcombine_s16(n, n)));
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------------------------
// SAO (8.7.3.2) pre 8 bitov: posun sa vyberie tabuľkou v registri (TBL) a pripočíta so sčítaním s nasýtením
// (USQADD: vzorka bez znamienka + posun so znamienkom, výsledok 0 … 255 = Clip3(0, 255, …)). Posuny SaoOffsetVal
// sú pri 8 bitoch v −7 … 7, teda v i8 (inak sa vráti 0 a počíta skalárna cesta).
// ---------------------------------------------------------------------------------------------------------------

/// Hranový posun úseku riadka (ako `sao::hrana_riadok`): `ciel[i] = Clip3(0, 255, c + t[2 + Sign(c − a) +
/// Sign(c − b)])` pre c = `stred[i]`, a = `a[i]`, b = `b[i]`. Spracuje začiatok úseku po 16 a 8 vzorkách a vráti
/// ich počet – zvyšok (menej ako 8) dopočíta volajúci. Pri `max` ≠ 255 alebo posune mimo i8 vráti 0.
pub fn sao_hrana(
    ciel: &mut [Vzorka],
    stred: &[Vzorka],
    a: &[Vzorka],
    b: &[Vzorka],
    t: [i16; 5],
    max: i16,
) -> usize {
    if max != 255 || t.iter().any(|&p| i8::try_from(p).is_err()) {
        return 0;
    }
    let n = ciel.len();
    assert!(stred.len() >= n && a.len() >= n && b.len() >= n, "HEVC: krátky riadok SAO");
    let mut tab = [0i8; 16];
    for (d, &p) in tab.iter_mut().zip(&t) {
        *d = p as i8;
    }
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        // SAFETY: tab má 16 bajtov.
        let tab = unsafe { vld1q_s8(tab.as_ptr()) };
        let (zs, za, zb, zc) = (stred.as_ptr(), a.as_ptr(), b.as_ptr(), ciel.as_mut_ptr());
        let mut i = 0;
        while i + 16 <= n {
            // SAFETY: i + 16 ≤ n ≤ dĺžka každého rezu (assert vyššie).
            let (c, va, vb) = unsafe { (vld1q_u8(zs.add(i)), vld1q_u8(za.add(i)), vld1q_u8(zb.add(i))) };
            // Sign(c − x) = [c > x] − [c < x]; maska porovnania je −1 (0xFF), preto (x > c) − (c > x)
            let sa = vsubq_u8(vcgtq_u8(va, c), vcgtq_u8(c, va));
            let sb = vsubq_u8(vcgtq_u8(vb, c), vcgtq_u8(c, vb));
            let posun = vqtbl1q_s8(tab, vaddq_u8(vaddq_u8(sa, sb), vdupq_n_u8(2)));
            // SAFETY: i + 16 ≤ n = ciel.len().
            unsafe { vst1q_u8(zc.add(i), vsqaddq_u8(c, posun)) };
            i += 16;
        }
        if i + 8 <= n {
            // SAFETY: i + 8 ≤ n ≤ dĺžka každého rezu.
            let (c, va, vb) = unsafe { (vld1_u8(zs.add(i)), vld1_u8(za.add(i)), vld1_u8(zb.add(i))) };
            let sa = vsub_u8(vcgt_u8(va, c), vcgt_u8(c, va));
            let sb = vsub_u8(vcgt_u8(vb, c), vcgt_u8(c, vb));
            let posun = vqtbl1_s8(tab, vadd_u8(vadd_u8(sa, sb), vdup_n_u8(2)));
            // SAFETY: i + 8 ≤ n = ciel.len().
            unsafe { vst1_u8(zc.add(i), vsqadd_u8(c, posun)) };
            i += 8;
        }
        i
    }
}

/// Pásmový posun úseku riadka (ako `sao::pasmo_riadok`): pásmo p = c >> 3 má posun `posuny[k]` pre
/// k = (p − `pasmo`) & 31 < 4, inak 0; `ciel[i] = Clip3(0, 255, c + posun)`. Spracuje začiatok po 16 a 8 vzorkách
/// a vráti ich počet (zvyšok dopočíta volajúci). Pri inej hĺbke (`posun_pasma` ≠ 3, `max` ≠ 255) alebo posune
/// mimo i8 vráti 0.
pub fn sao_pasmo(
    ciel: &mut [Vzorka],
    stred: &[Vzorka],
    posuny: [i16; 4],
    pasmo: u16,
    posun_pasma: u32,
    max: i16,
) -> usize {
    if max != 255 || posun_pasma != 3 || posuny.iter().any(|&p| i8::try_from(p).is_err()) {
        return 0;
    }
    let n = ciel.len();
    assert!(stred.len() >= n, "HEVC: krátky riadok SAO");
    // bandTable: posun pre každé z 32 pásiem
    let mut tab = [0i8; 32];
    for (k, &p) in posuny.iter().enumerate() {
        tab[(k + usize::from(pasmo)) & 31] = p as i8;
    }
    // SAFETY: pozri komentáre pri jednotlivých prístupoch do pamäte nižšie.
    unsafe {
        // SAFETY: tab má 32 bajtov.
        let tab = unsafe { int8x16x2_t(vld1q_s8(tab.as_ptr()), vld1q_s8(tab.as_ptr().add(16))) };
        let (zs, zc) = (stred.as_ptr(), ciel.as_mut_ptr());
        let mut i = 0;
        while i + 16 <= n {
            // SAFETY: i + 16 ≤ n ≤ stred.len().
            let c = unsafe { vld1q_u8(zs.add(i)) };
            let posun = vqtbl2q_s8(tab, vshrq_n_u8::<3>(c));
            // SAFETY: i + 16 ≤ n = ciel.len().
            unsafe { vst1q_u8(zc.add(i), vsqaddq_u8(c, posun)) };
            i += 16;
        }
        if i + 8 <= n {
            // SAFETY: i + 8 ≤ n ≤ stred.len().
            let c = unsafe { vld1_u8(zs.add(i)) };
            let posun = vqtbl2_s8(tab, vshr_n_u8::<3>(c));
            // SAFETY: i + 8 ≤ n = ciel.len().
            unsafe { vst1_u8(zc.add(i), vsqadd_u8(c, posun)) };
            i += 8;
        }
        i
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

    /// Odblokovanie úseku jasu: NEON a skalárna cesta dávajú bitovo to isté – zvislé aj vodorovné hrany, silný aj
    /// slabý filter (hladké oblasti so skokom na hrane a šumom), okraje rozsahu 0 a 255, strany bez filtra.
    #[test]
    fn odblokovanie_jasu_rovnako_ako_skalarne() {
        use super::super::odblokovanie::odblokuj_usek_jasu_skalarne;
        let mut n = Nahoda::zo_semienka(0x0d_b10c);
        let mut filtrovanych = 0;
        for pokus in 0..200_000 {
            let krok = 8 + (n.dalsie() % 3) as usize * 8;
            let (vyska, zvisla) = (8 + (n.dalsie() % 3) as usize, pokus % 2 == 0);
            let zaklad = (n.dalsie() % 256) as i32;
            let skok = (n.dalsie() % 41) as i32 - 20;
            let sum = [0, 1, 2, 4, 8, 40, 255][(n.dalsie() % 7) as usize];
            let mut data = vec![0u8; krok * vyska];
            for (p, v) in data.iter_mut().enumerate() {
                let (x, y) = (p % krok, p / krok);
                let strana_q = if zvisla { x >= 4 } else { y >= 4 };
                let s = if sum > 0 { (n.dalsie() % (2 * sum + 1)) as i32 - sum as i32 } else { 0 };
                *v = (zaklad + if strana_q { skok } else { 0 } + s).clamp(0, 255) as u8;
            }
            let beta = [0, 6, 8, 12, 16, 24, 32, 48, 64][(n.dalsie() % 9) as usize];
            let tc = 1 + (n.dalsie() % 24) as i32;
            let bez = [n.dalsie().is_multiple_of(8), n.dalsie().is_multiple_of(8)];
            let i = if zvisla {
                (n.dalsie() % (vyska as u64 - 3)) as usize * krok
            } else {
                (n.dalsie() % 4) as usize
            };
            let (mut a, mut b) = (data.clone(), data.clone());
            assert!(odblokuj_jas(&mut a, i, krok, zvisla, beta, tc, bez));
            odblokuj_usek_jasu_skalarne(&mut b, i, krok, zvisla, beta, tc, bez, 255);
            assert_eq!(a, b, "pokus {pokus}: zvisla={zvisla} beta={beta} tc={tc} bez={bez:?}");
            filtrovanych += usize::from(b != data);
        }
        assert!(filtrovanych > 50_000, "skúška má filtrovať veľa úsekov: {filtrovanych}");
    }

    /// SAO: NEON časť úseku je bitovo rovnaká ako vzorec normy (8.7.3.2) – hranový aj pásmový posun, všetky dĺžky
    /// úseku, posuny −7 … 7 (aj krajné vzorky 0 a 255), všetky polohy pásma; zvyšok úseku sa nezmení.
    #[test]
    fn sao_rovnako_ako_norma() {
        let mut n = Nahoda::zo_semienka(0x5a0);
        let sign = |x: i32| x.signum();
        for pokus in 0..20_000 {
            let dlzka = (n.dalsie() % 70) as usize;
            let nahodne = |n: &mut Nahoda| -> Vec<u8> {
                let zaklad = (n.dalsie() % 256) as i32;
                let roz = [1, 3, 16, 256][(n.dalsie() % 4) as usize];
                (0..dlzka + 3)
                    .map(|_| (zaklad + (n.dalsie() % roz) as i32 - roz as i32 / 2).clamp(0, 255) as u8)
                    .collect()
            };
            let (c, a, b) = (nahodne(&mut n), nahodne(&mut n), nahodne(&mut n));
            let p: [i16; 4] = std::array::from_fn(|_| (n.dalsie() % 15) as i16 - 7);
            let t = [p[0], p[1], 0, p[2], p[3]];
            let mut ciel = vec![0xa5u8; dlzka];
            let hotovo = sao_hrana(&mut ciel, &c, &a, &b, t, 255);
            assert!(hotovo <= dlzka && dlzka - hotovo < 8, "pokus {pokus}: spracované {hotovo} z {dlzka}");
            for i in 0..dlzka {
                let ocakavane = if i < hotovo {
                    let e =
                        2 + sign(i32::from(c[i]) - i32::from(a[i])) + sign(i32::from(c[i]) - i32::from(b[i]));
                    (i32::from(c[i]) + i32::from(t[e as usize])).clamp(0, 255) as u8
                } else {
                    0xa5
                };
                assert_eq!(ciel[i], ocakavane, "hrana pokus {pokus} i={i}");
            }
            let pasmo = (n.dalsie() % 32) as u16;
            let hotovo = sao_pasmo(&mut ciel, &c, p, pasmo, 3, 255);
            assert!(hotovo <= dlzka && dlzka - hotovo < 8);
            for i in 0..hotovo {
                let k = ((u16::from(c[i]) >> 3).wrapping_sub(pasmo) & 31) as usize;
                let posun = if k < 4 { i32::from(p[k]) } else { 0 };
                assert_eq!(
                    ciel[i],
                    (i32::from(c[i]) + posun).clamp(0, 255) as u8,
                    "pásmo pokus {pokus} i={i}"
                );
            }
        }
        // posun mimo i8 alebo iná hĺbka: NEON nič nespracuje
        let mut ciel = [0u8; 16];
        assert_eq!(sao_hrana(&mut ciel, &[0; 16], &[0; 16], &[0; 16], [200, 0, 0, 0, 0], 255), 0);
        assert_eq!(sao_pasmo(&mut ciel, &[0; 16], [0; 4], 0, 3, 1023), 0);
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
