//! Medzisnímková predikcia vzoriek: interpolácia zlomkových polôh a vážená predikcia – H.265, kap. 8.5.3.3.3
//! a 8.5.3.3.4.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`Vahy`] a [`predikuj`].
//!
//! Postup pre jeden blok a jednu referenciu:
//! 1. **oblasť zdroja** – blok aj s okrajom filtra (jas 3 vzorky vľavo či hore a 4 vpravo či dole, farba 1 a 2;
//!    okraj len v smere, v ktorom je zlomok). Ak celá oblasť leží vnútri referenčnej roviny, filtre čítajú
//!    priamo z nej (rýchla cesta). Inak sa oblasť skopíruje do pracovného poľa s orezaním každej súradnice
//!    Clip3 (8.5.3.3.3.2, 8.5.3.3.3.3) – výsledok je totožný s orezávaním pri každom čítaní,
//! 2. **interpolácia** (8.5.3.3.3) – štyri oddelené cesty: celočíselný vektor, len vodorovný zlomok, len zvislý
//!    zlomok, oba zlomky (najprv vodorovne do medzipoľa, potom zvislo),
//! 3. **váženie** (8.5.3.3.4) – predvolené alebo explicitné, s orezaním na rozsah vzoriek.
//!
//! **Medzivýsledky.** predSamplesLX majú 14 bitov, ale po druhom stupni interpolácie pri 8-bitových vzorkách
//! siahajú od −16 830 po 33 150, čo sa do i16 nezmestí. Ukladajú sa preto ako i16 hodnota
//! predSample − 8192 (`POSUN_MEDZI`; rozsah −25 022 … 24 958, pri 10 a 12 bitoch podobne) a váženie počíta
//! v i32 s vrátením posunu v konštantách vzorcov. Výsledok je bitovo totožný s normou pre ľubovoľný obsah,
//! polovičná šírka medzivýsledkov šetrí pamäť aj dráhy vektorov (8 dráh NEON namiesto 4).
//!
//! **Pracovné polia** (medzivýsledky dvoch referencií, medzipole 2D filtra, kópia okraja – spolu ≈ 30 KiB) sa
//! pripravia raz pre každé vlákno. Blok nič nealokuje ani nenuluje: nulovanie takých polí na zásobníku pri
//! každom volaní by na Cortex-A53 stálo viac než samotná interpolácia malého bloku.
//!
//! 1D filtre riadka (`riadok_vodorovne`, `riadok_zvislo`, `riadok_zvislo_medzi`) a riadkové váženie sú
//! samostatné funkcie – miesto pre neskoršiu verziu NEON (krok H5). Slučky sú písané pre automatickú
//! vektorizáciu: pevný počet bodov filtra (`N`), dĺžky riadkov overené pred slučkou.

use super::obraz::{Vzorka, Zdroj};
use std::cell::RefCell;

/// Najväčší rozmer predikčného bloku (CTB 64 × 64).
const MAX_BLOK: usize = 64;
/// Najväčší okraj filtra v jednom smere: 8 bodov → 7 vzoriek navyše (3 pred, 4 za).
const MAX_OKRAJ: usize = 7;
/// Posun uložených medzivýsledkov: v i16 sa ukladá predSample − 8192 (pozri dokumentáciu modulu).
const POSUN_MEDZI: i32 = 1 << 13;
/// shift2 druhého stupňa 2D interpolácie (8.5.3.3.3.2).
const SHIFT2: u32 = 6;

/// Koeficienty filtra jasu fL[xFracL][i], i = 0 … 7 pre vzorky xInt − 3 … xInt + 4 (8.5.3.3.3.2). Riadok 0
/// (celočíselná poloha) sa vo filtroch nepoužíva – je tu len identita pre úplnosť.
const FILTER_JAS: [[i16; 8]; 4] = [
    [0, 0, 0, 64, 0, 0, 0, 0],
    [-1, 4, -10, 58, 17, -5, 1, 0],
    [-1, 4, -11, 40, 40, -11, 4, -1],
    [0, 1, -5, 17, 58, -10, 4, -1],
];

/// Koeficienty filtra farby fC[xFracC][i], i = 0 … 3 pre vzorky xInt − 1 … xInt + 2 (8.5.3.3.3.3). Riadok 0 sa
/// vo filtroch nepoužíva.
const FILTER_FARBA: [[i16; 4]; 8] = [
    [0, 64, 0, 0],
    [-2, 58, 10, -2],
    [-4, 54, 16, -2],
    [-6, 46, 28, -4],
    [-4, 36, 36, -4],
    [-4, 28, 46, -6],
    [-2, 16, 54, -4],
    [-2, 10, 58, -2],
];

/// Explicitné váhy (8.5.3.3.4.3) pre jednu zložku a jeden predikčný blok – hodnoty už odvodené dekodérom:
/// - `log2_denom`: luma_log2_weight_denom alebo ChromaLog2WeightDenom (bez shift1),
/// - `w[0]`, `w[1]`: LumaWeightL0/L1 alebo ChromaWeightL0/L1 použitého referenčného indexu,
/// - `o[0]`, `o[1]`: posuny už vynásobené (1 << (BitDepth − 8)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Vahy {
    pub log2_denom: u32,
    pub w: [i32; 2],
    pub o: [i32; 2],
}

/// Predikcia jedného bloku `w` × `h` jednej zložky (8.5.3.3):
/// - `jas`: zložka jasu – filter 8 bodov, vektor v štvrtinách vzorky; inak farba 4:2:0 – filter 4 body,
///   vektor v osminách vzorky farby (mvC = mv jasu),
/// - `x`, `y`: poloha ľavého horného rohu bloku v rovine zložky,
/// - `ref0`, `ref1`: referenčná rovina a vektor z zoznamu L0 a L1 (aspoň jedna je `Some`); súradnice mimo
///   referenčnej roviny sa orezávajú na jej okraj (xInt = Clip3(0, šírka − 1, …)),
/// - `vahy`: `Some` = explicitné váženie (weighted_pred_flag / weighted_bipred_flag), `None` = predvolené
///   (8.5.3.3.4.2); pri jednej referencii z L1 sa použijú `w[1]`, `o[1]`,
/// - výsledok sa zapíše do `ciel`: vzorka (i, j) bloku na `ciel[j * krok + i]`.
///
/// Medzivýsledky interpolácie majú 14 bitov a pri 8-bitových vzorkách môžu po druhom stupni presiahnuť rozsah
/// i16 – implementácia musí zostať bitovo presná pre ľubovoľný obsah.
///
/// Predpoklady (zaručí ich dekodér podľa 7.4.7.3): `w`, `h` ≤ 64, bitová hĺbka 8 … 12 v rozsahu typu
/// [`Vzorka`], `log2_denom` ≤ 7, váhy −128 … 255, |posun| < 2^15. Ak nie je zadaná ani jedna referencia,
/// `ciel` sa nezmení.
#[allow(clippy::too_many_arguments)]
pub fn predikuj<Z: Zdroj + ?Sized>(
    jas: bool,
    ref0: Option<(&Z, [i32; 2])>,
    ref1: Option<(&Z, [i32; 2])>,
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    vahy: Option<&Vahy>,
    bitova_hlbka: u32,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    if w == 0 || h == 0 {
        return;
    }
    assert!(w <= MAX_BLOK && h <= MAX_BLOK, "HEVC: predikčný blok {w}×{h} je väčší ako 64×64");
    debug_assert!(
        (8..=12).contains(&bitova_hlbka) && (1u32 << bitova_hlbka) - 1 <= Vzorka::MAX as u32,
        "HEVC: nepodporovaná bitová hĺbka {bitova_hlbka}"
    );
    debug_assert!(
        vahy.is_none_or(|v| v.log2_denom <= 7
            && v.w.iter().all(|w| (-128..=255).contains(w))
            && v.o.iter().all(|o| o.abs() < 1 << 15)),
        "HEVC: váhy mimo rozsahu normy: {vahy:?}"
    );
    let blok = Blok { x, y, w, h };

    // Najčastejší prípad (preskočené bloky, statické pozadie): jedna referencia, predvolené váženie,
    // celočíselný vektor. ((vzorka << shift3) + offset1) >> shift1 = vzorka, lebo shift3 = shift1 = 14 − BitDepth
    // – stačí kópia s orezaním súradníc.
    if vahy.is_none()
        && let (Some((rovina, mv)), None) | (None, Some((rovina, mv))) = (ref0, ref1)
    {
        let log2_jemnost = if jas { 2 } else { 3 };
        let maska = (1 << log2_jemnost) - 1;
        if mv[0] & maska == 0 && mv[1] & maska == 0 {
            let sx = x + (mv[0] >> log2_jemnost);
            let sy = y + (mv[1] >> log2_jemnost);
            kopiruj_s_orezanim(rovina, sx, sy, w, h, ciel, krok);
            return;
        }
    }

    PRACOVNE.with(|bunka| {
        let mut stav = bunka.borrow_mut();
        let Pracovne { pred: [pred0, pred1], medzi, okraj } = stav.get_or_insert_with(Pracovne::nove);
        let n = w * h;
        match (ref0, ref1) {
            (Some(r0), Some(r1)) => {
                interpoluj_zlozku(jas, r0, blok, bitova_hlbka, &mut pred0[..n], medzi, okraj);
                interpoluj_zlozku(jas, r1, blok, bitova_hlbka, &mut pred1[..n], medzi, okraj);
                vaz_dve(&pred0[..n], &pred1[..n], vahy, w, bitova_hlbka, ciel, krok);
            }
            (Some(r), None) | (None, Some(r)) => {
                interpoluj_zlozku(jas, r, blok, bitova_hlbka, &mut pred0[..n], medzi, okraj);
                let zoznam = usize::from(ref0.is_none());
                vaz_jednu(&pred0[..n], vahy, zoznam, w, bitova_hlbka, ciel, krok);
            }
            (None, None) => {}
        }
    });
}

/// Poloha a rozmer bloku v rovine zložky.
#[derive(Clone, Copy)]
struct Blok {
    x: i32,
    y: i32,
    w: usize,
    h: usize,
}

/// Pracovné polia jedného vlákna – pripravia sa pri prvom použití a potom sa len prepisujú.
struct Pracovne {
    /// predSamplesL0 a predSamplesL1 − 8192, riadok j bloku na `[j * w..]`
    pred: [Vec<i16>; 2],
    /// výsledok vodorovného stupňa 2D interpolácie: (h + 7) riadkov po w
    medzi: Vec<i16>,
    /// kópia oblasti zdroja s orezanými súradnicami (oblasť zasahuje mimo referenčnej roviny)
    okraj: Vec<Vzorka>,
}

impl Pracovne {
    fn nove() -> Pracovne {
        Pracovne {
            pred: [vec![0; MAX_BLOK * MAX_BLOK], vec![0; MAX_BLOK * MAX_BLOK]],
            medzi: vec![0; (MAX_BLOK + MAX_OKRAJ) * MAX_BLOK],
            okraj: vec![0; (MAX_BLOK + MAX_OKRAJ) * (MAX_BLOK + MAX_OKRAJ)],
        }
    }
}

thread_local! {
    static PRACOVNE: RefCell<Option<Pracovne>> = const { RefCell::new(None) };
}

/// Interpolácia bloku jednej zložky z jednej referencie (8.5.3.3.3.1): jas filtrom 8 bodov so štvrtinovou
/// presnosťou, farba 4:2:0 filtrom 4 body s osminovou presnosťou (mvC = mv).
fn interpoluj_zlozku<Z: Zdroj + ?Sized>(
    jas: bool,
    (rovina, mv): (&Z, [i32; 2]),
    blok: Blok,
    bitova_hlbka: u32,
    pred: &mut [i16],
    medzi: &mut [i16],
    okraj: &mut [Vzorka],
) {
    if jas {
        interpoluj(&FILTER_JAS, 2, rovina, mv, blok, bitova_hlbka, pred, medzi, okraj);
    } else {
        interpoluj(&FILTER_FARBA, 3, rovina, mv, blok, bitova_hlbka, pred, medzi, okraj);
    }
}

/// Interpolácia bloku filtrom `N` bodov (8.5.3.3.3.2 jas, 8.5.3.3.3.3 farba). Vektor má `log2_jemnost` bitov
/// zlomku: xInt = x + (mv[0] >> log2_jemnost) + i, xFrac = mv[0] & (2^log2_jemnost − 1), rovnako y.
/// Do `pred` (riadok j na `[j * w..]`) zapíše predSampleLX − 8192.
#[allow(clippy::too_many_arguments)]
fn interpoluj<const N: usize, Z: Zdroj + ?Sized>(
    filtre: &[[i16; N]],
    log2_jemnost: u32,
    rovina: &Z,
    mv: [i32; 2],
    blok: Blok,
    bitova_hlbka: u32,
    pred: &mut [i16],
    medzi: &mut [i16],
    okraj: &mut [Vzorka],
) {
    let Blok { x, y, w, h } = blok;
    let maska = (1 << log2_jemnost) - 1;
    let (fx, fy) = ((mv[0] & maska) as usize, (mv[1] & maska) as usize);
    let x_int = x + (mv[0] >> log2_jemnost);
    let y_int = y + (mv[1] >> log2_jemnost);
    // oblasť zdroja: okraj filtra len v smere so zlomkom (jas 3 pred a 4 za, farba 1 pred a 2 za)
    let pred_okraj = (N / 2 - 1) as i32;
    let (sx, sw) = if fx != 0 { (x_int - pred_okraj, w + N - 1) } else { (x_int, w) };
    let (sy, sh) = if fy != 0 { (y_int - pred_okraj, h + N - 1) } else { (y_int, h) };
    let (zdroj, krok) = oblast(rovina, sx, sy, sw, sh, okraj);

    let shift1 = (bitova_hlbka - 8).min(4);
    let shift3 = 14u32.saturating_sub(bitova_hlbka).max(2);
    let pred = &mut pred[..w * h];
    match (fx, fy) {
        (0, 0) => {
            for (j, p) in pred.chunks_exact_mut(w).enumerate() {
                riadok_cely(&zdroj[j * krok..], p, shift3);
            }
        }
        (_, 0) => {
            for (j, p) in pred.chunks_exact_mut(w).enumerate() {
                riadok_vodorovne(&zdroj[j * krok..], &filtre[fx], p, shift1, POSUN_MEDZI);
            }
        }
        (0, _) => {
            for (j, p) in pred.chunks_exact_mut(w).enumerate() {
                riadok_zvislo(&zdroj[j * krok..], krok, &filtre[fy], p, shift1, POSUN_MEDZI);
            }
        }
        _ => {
            // temp[n] pre riadky yInt − 3 … yInt + 4 (jas), ich zvislý filter >> shift2
            let medzi = &mut medzi[..sh * w];
            for (j, m) in medzi.chunks_exact_mut(w).enumerate() {
                riadok_vodorovne(&zdroj[j * krok..], &filtre[fx], m, shift1, 0);
            }
            for (j, p) in pred.chunks_exact_mut(w).enumerate() {
                riadok_zvislo_medzi(&medzi[j * w..], &filtre[fy], p);
            }
        }
    }
}

/// Oblasť zdroja `sw` × `sh` s ľavým horným rohom (`sx`, `sy`) ako (vzorky, krok riadka): priamo z roviny, ak v nej
/// leží celá, inak kópia s orezanými súradnicami v `okraj`.
fn oblast<'a, Z: Zdroj + ?Sized>(
    rovina: &'a Z,
    sx: i32,
    sy: i32,
    sw: usize,
    sh: usize,
    okraj: &'a mut [Vzorka],
) -> (&'a [Vzorka], usize) {
    let vnutri =
        sx >= 0 && sy >= 0 && sx as usize + sw <= rovina.sirka() && sy as usize + sh <= rovina.vyska();
    if vnutri && let Some((d, krok)) = rovina.riadky(sy as usize, sh) {
        return (&d[sx as usize..], krok);
    }
    kopiruj_s_orezanim(rovina, sx, sy, sw, sh, okraj, sw);
    (&okraj[..sw * sh], sw)
}

/// Kópia oblasti `sw` × `sh` z (`sx`, `sy`) do `ciel` (riadok j na `[j * krok..]`) s orezaním súradníc
/// Clip3(0, šírka − 1, …) a Clip3(0, výška − 1, …) – vzorky mimo roviny nahradí najbližšia vzorka okraja.
fn kopiruj_s_orezanim<Z: Zdroj + ?Sized>(
    rovina: &Z,
    sx: i32,
    sy: i32,
    sw: usize,
    sh: usize,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    debug_assert!(rovina.sirka() > 0 && rovina.vyska() > 0, "HEVC: prázdna referenčná rovina");
    let (sirka, vyska) = (rovina.sirka() as i32, rovina.vyska() as i32);
    // stĺpce [0, lavy) ležia vľavo od roviny, [lavy, pravy) vnútri, [pravy, sw) vpravo
    let lavy = (-sx).clamp(0, sw as i32);
    let pravy = (sirka - sx).clamp(lavy, sw as i32);
    let od = (sx + lavy) as usize;
    let (lavy, pravy) = (lavy as usize, pravy as usize);
    for j in 0..sh {
        let zdroj = rovina.riadok((sy + j as i32).clamp(0, vyska - 1) as usize);
        let riadok = &mut ciel[j * krok..][..sw];
        riadok[..lavy].fill(zdroj[0]);
        if pravy > lavy {
            riadok[lavy..pravy].copy_from_slice(&zdroj[od..od + (pravy - lavy)]);
        }
        riadok[pravy..].fill(zdroj[rovina.sirka() - 1]);
    }
}

/// Celočíselná poloha (8.5.3.3.3.2): predSample = vzorka << shift3; zapíše predSample − 8192.
fn riadok_cely(zdroj: &[Vzorka], ciel: &mut [i16], shift3: u32) {
    let zdroj = &zdroj[..ciel.len()];
    for (c, &v) in ciel.iter_mut().zip(zdroj) {
        *c = (((v as i32) << shift3) - POSUN_MEDZI) as i16;
    }
}

/// Vodorovný filter jedného riadka (N bodov): `ciel[i] = ((Σ_k koef[k] · zdroj[i + k]) >> shift1) − odcitaj`.
/// `zdroj[0]` je vzorka xInt − (N/2 − 1) prvej vzorky riadka; treba w + N − 1 vzoriek.
#[allow(clippy::needless_range_loop)]
fn riadok_vodorovne<const N: usize>(
    zdroj: &[Vzorka],
    koef: &[i16; N],
    ciel: &mut [i16],
    shift1: u32,
    odcitaj: i32,
) {
    let w = ciel.len();
    let zdroj = &zdroj[..w + N - 1];
    if shift1 == 0 {
        // 8 bitov: výsledok (aj s odčítaním 8192) leží v −14 312 … 22 440, teda v i16 – počítanie v i16
        // modulo 2^16 je presné a vektor má 8 dráh namiesto 4
        vodorovne8(zdroj, koef, ciel, odcitaj as i16);
    } else {
        for i in 0..w {
            let mut s = 0;
            for k in 0..N {
                s += koef[k] as i32 * zdroj[i + k] as i32;
            }
            ciel[i] = ((s >> shift1) - odcitaj) as i16;
        }
    }
}

/// Zvislý filter jedného riadka zo vzoriek (N bodov): `ciel[i] = ((Σ_k koef[k] · R_k[i]) >> shift1) − odcitaj`,
/// kde R_k je riadok `zdroj[k * krok..]` (riadok 0 = yInt − (N/2 − 1)).
#[allow(clippy::needless_range_loop)]
fn riadok_zvislo<const N: usize>(
    zdroj: &[Vzorka],
    krok: usize,
    koef: &[i16; N],
    ciel: &mut [i16],
    shift1: u32,
    odcitaj: i32,
) {
    let w = ciel.len();
    let riadky: [&[Vzorka]; N] = std::array::from_fn(|k| &zdroj[k * krok..][..w]);
    if shift1 == 0 {
        // 8 bitov: presné v i16 (pozri riadok_vodorovne)
        zvislo8(&riadky, koef, ciel, odcitaj as i16);
    } else {
        for i in 0..w {
            let mut s = 0;
            for k in 0..N {
                s += koef[k] as i32 * riadky[k][i] as i32;
            }
            ciel[i] = ((s >> shift1) - odcitaj) as i16;
        }
    }
}

/// Druhý stupeň 2D interpolácie (8.5.3.3.3.2): zvislý filter medzivýsledkov temp,
/// `ciel[i] = ((Σ_k koef[k] · T_k[i]) >> shift2) − 8192`, kde T_k je riadok `medzi[k * w..]`. Súčet presahuje
/// i16, preto súčiny i16 · i16 → i32.
fn riadok_zvislo_medzi<const N: usize>(medzi: &[i16], koef: &[i16; N], ciel: &mut [i16]) {
    let w = ciel.len();
    let riadky: [&[i16]; N] = std::array::from_fn(|k| &medzi[k * w..][..w]);
    zvislo_medzi(&riadky, koef, ciel);
}

// ---------------------------------------------------------------------------------------------------------------
// Jadrá pre 8 bitov: skalárne (rozhodca, aj zvyšok riadka) a na aarch64 ručné NEON (modul `neon`) – bitovo rovnaké.
// ---------------------------------------------------------------------------------------------------------------

/// `ciel[i] = Σ_k koef[k] · zdroj[i + k] − odcitaj` v i16 modulo 2^16 (8 bitov, pozri riadok_vodorovne).
#[allow(clippy::needless_range_loop)]
pub(crate) fn vodorovne8_skalarne<const N: usize>(
    zdroj: &[Vzorka],
    koef: &[i16; N],
    ciel: &mut [i16],
    odcitaj: i16,
) {
    let w = ciel.len();
    let zdroj = &zdroj[..w + N - 1];
    for i in 0..w {
        let mut s = 0i16;
        for k in 0..N {
            s = s.wrapping_add(koef[k].wrapping_mul(zdroj[i + k] as i16));
        }
        ciel[i] = s.wrapping_sub(odcitaj);
    }
}

/// `ciel[i] = Σ_k koef[k] · riadky[k][i] − odcitaj` v i16 modulo 2^16 (8 bitov).
#[allow(clippy::needless_range_loop)]
pub(crate) fn zvislo8_skalarne<const N: usize>(
    riadky: &[&[Vzorka]; N],
    koef: &[i16; N],
    ciel: &mut [i16],
    odcitaj: i16,
) {
    for i in 0..ciel.len() {
        let mut s = 0i16;
        for k in 0..N {
            s = s.wrapping_add(koef[k].wrapping_mul(riadky[k][i] as i16));
        }
        ciel[i] = s.wrapping_sub(odcitaj);
    }
}

/// `ciel[i] = ((Σ_k koef[k] · riadky[k][i]) >> 6) − 8192` (súčet v i32, výsledok skrátený na i16).
#[allow(clippy::needless_range_loop)]
pub(crate) fn zvislo_medzi_skalarne<const N: usize>(riadky: &[&[i16]; N], koef: &[i16; N], ciel: &mut [i16]) {
    for i in 0..ciel.len() {
        let mut s = 0;
        for k in 0..N {
            s += koef[k] as i32 * riadky[k][i] as i32;
        }
        ciel[i] = ((s >> SHIFT2) - POSUN_MEDZI) as i16;
    }
}

// jadrá podľa typu vzorky a procesora: ručné NEON (8 bitov, aarch64) alebo skalárne (modul `simd` nadradeného modulu)
fn vodorovne8<const N: usize>(zdroj: &[Vzorka], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    super::simd::vodorovne8(zdroj, koef, ciel, odcitaj);
}

fn zvislo8<const N: usize>(riadky: &[&[Vzorka]; N], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    super::simd::zvislo8(riadky, koef, ciel, odcitaj);
}

fn zvislo_medzi<const N: usize>(riadky: &[&[i16]; N], koef: &[i16; N], ciel: &mut [i16]) {
    super::simd::zvislo_medzi(riadky, koef, ciel);
}

/// Váženie jednej referencie zo zoznamu `zoznam` (0 = L0, 1 = L1) pre celý blok šírky `w`.
fn vaz_jednu(
    pred: &[i16],
    vahy: Option<&Vahy>,
    zoznam: usize,
    w: usize,
    bitova_hlbka: u32,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    let max = (1 << bitova_hlbka) - 1;
    let shift1 = 14 - bitova_hlbka;
    match vahy {
        None => {
            // 8.5.3.3.4.2: Clip3(0, max, (predSamplesLX + offset1) >> shift1)
            let offset1 = if shift1 > 0 { 1 << (shift1 - 1) } else { 0 };
            for (j, p) in pred.chunks_exact(w).enumerate() {
                riadok_predvolene_jedna(p, &mut ciel[j * krok..][..w], POSUN_MEDZI + offset1, shift1, max);
            }
        }
        Some(v) => {
            // 8.5.3.3.4.3: log2WD ≥ 1: Clip3(0, max, ((predSamplesLX · w + 2^(log2WD − 1)) >> log2WD) + o),
            // inak Clip3(0, max, predSamplesLX · w + o); 8192 · w vracia posun medzivýsledku
            let (vaha, posun) = (v.w[zoznam], v.o[zoznam]);
            let log2wd = v.log2_denom + shift1;
            let (pridaj, shift) = if log2wd >= 1 {
                (POSUN_MEDZI * vaha + (1 << (log2wd - 1)), log2wd)
            } else {
                (POSUN_MEDZI * vaha, 0)
            };
            for (j, p) in pred.chunks_exact(w).enumerate() {
                riadok_explicitne_jedna(p, &mut ciel[j * krok..][..w], vaha, pridaj, shift, posun, max);
            }
        }
    }
}

/// Váženie dvoch referencií (L0 a L1) pre celý blok šírky `w`.
fn vaz_dve(
    pred0: &[i16],
    pred1: &[i16],
    vahy: Option<&Vahy>,
    w: usize,
    bitova_hlbka: u32,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    let max = (1 << bitova_hlbka) - 1;
    let riadky = pred0.chunks_exact(w).zip(pred1.chunks_exact(w)).enumerate();
    match vahy {
        None => {
            // 8.5.3.3.4.2: Clip3(0, max, (predSamplesL0 + predSamplesL1 + offset2) >> shift2)
            let shift2 = 15 - bitova_hlbka;
            let pridaj = 2 * POSUN_MEDZI + (1 << (shift2 - 1));
            for (j, (p0, p1)) in riadky {
                riadok_predvolene_dve(p0, p1, &mut ciel[j * krok..][..w], pridaj, shift2, max);
            }
        }
        Some(v) => {
            // 8.5.3.3.4.3: Clip3(0, max, (predSamplesL0 · w0 + predSamplesL1 · w1
            //     + ((o0 + o1 + 1) << log2WD)) >> (log2WD + 1))
            let log2wd = v.log2_denom + 14 - bitova_hlbka;
            let pridaj = POSUN_MEDZI * (v.w[0] + v.w[1]) + ((v.o[0] + v.o[1] + 1) << log2wd);
            for (j, (p0, p1)) in riadky {
                riadok_explicitne_dve(p0, p1, &mut ciel[j * krok..][..w], v.w, pridaj, log2wd + 1, max);
            }
        }
    }
}

/// Predvolené váženie riadka jednej referencie: `Clip3(0, max, (p + pridaj) >> shift)`.
fn riadok_predvolene_jedna(pred: &[i16], ciel: &mut [Vzorka], pridaj: i32, shift: u32, max: i32) {
    let pred = &pred[..ciel.len()];
    super::simd::predvolene_jedna(pred, ciel, pridaj, shift, max);
}

/// `ciel[i] = Clip3(0, max, (pred[i] + pridaj) >> shift)`.
pub(crate) fn predvolene_jedna_skalarne(
    pred: &[i16],
    ciel: &mut [Vzorka],
    pridaj: i32,
    shift: u32,
    max: i32,
) {
    for (c, &p) in ciel.iter_mut().zip(pred) {
        *c = ((p as i32 + pridaj) >> shift).clamp(0, max) as Vzorka;
    }
}

/// Predvolené váženie riadka dvoch referencií: `Clip3(0, max, (p0 + p1 + pridaj) >> shift)`.
fn riadok_predvolene_dve(
    pred0: &[i16],
    pred1: &[i16],
    ciel: &mut [Vzorka],
    pridaj: i32,
    shift: u32,
    max: i32,
) {
    let (pred0, pred1) = (&pred0[..ciel.len()], &pred1[..ciel.len()]);
    super::simd::predvolene_dve(pred0, pred1, ciel, pridaj, shift, max);
}

/// `ciel[i] = Clip3(0, max, (pred0[i] + pred1[i] + pridaj) >> shift)`.
pub(crate) fn predvolene_dve_skalarne(
    pred0: &[i16],
    pred1: &[i16],
    ciel: &mut [Vzorka],
    pridaj: i32,
    shift: u32,
    max: i32,
) {
    for ((c, &a), &b) in ciel.iter_mut().zip(pred0).zip(pred1) {
        *c = ((a as i32 + b as i32 + pridaj) >> shift).clamp(0, max) as Vzorka;
    }
}

/// Explicitné váženie riadka jednej referencie: `Clip3(0, max, ((p · vaha + pridaj) >> shift) + posun)`.
fn riadok_explicitne_jedna(
    pred: &[i16],
    ciel: &mut [Vzorka],
    vaha: i32,
    pridaj: i32,
    shift: u32,
    posun: i32,
    max: i32,
) {
    let pred = &pred[..ciel.len()];
    for (c, &p) in ciel.iter_mut().zip(pred) {
        *c = (((p as i32 * vaha + pridaj) >> shift) + posun).clamp(0, max) as Vzorka;
    }
}

/// Explicitné váženie riadka dvoch referencií: `Clip3(0, max, (p0 · w0 + p1 · w1 + pridaj) >> shift)`.
fn riadok_explicitne_dve(
    pred0: &[i16],
    pred1: &[i16],
    ciel: &mut [Vzorka],
    vahy: [i32; 2],
    pridaj: i32,
    shift: u32,
    max: i32,
) {
    let (pred0, pred1) = (&pred0[..ciel.len()], &pred1[..ciel.len()]);
    for ((c, &a), &b) in ciel.iter_mut().zip(pred0).zip(pred1) {
        *c = ((a as i32 * vahy[0] + b as i32 * vahy[1] + pridaj) >> shift).clamp(0, max) as Vzorka;
    }
}

#[cfg(test)]
mod testy {
    use super::super::obraz::Rovina;
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    // ---------------------------------------------------------------------------------------------------------
    // Rozhodca: priamy, pomalý prepis vzorcov normy – každá vzorka zvlášť, každé čítanie s Clip3, všetko v i64.
    // ---------------------------------------------------------------------------------------------------------

    /// fL[xFracL][i] pre xFracL = 1, 2, 3 (8.5.3.3.3.2) – opísané znovu, nezávisle od `FILTER_JAS`.
    const FL: [[i64; 8]; 3] =
        [[-1, 4, -10, 58, 17, -5, 1, 0], [-1, 4, -11, 40, 40, -11, 4, -1], [0, 1, -5, 17, 58, -10, 4, -1]];

    /// fC[xFracC][i] pre xFracC = 1 … 7 (8.5.3.3.3.3) – opísané znovu, nezávisle od `FILTER_FARBA`.
    const FC: [[i64; 4]; 7] = [
        [-2, 58, 10, -2],
        [-4, 54, 16, -2],
        [-6, 46, 28, -4],
        [-4, 36, 36, -4],
        [-4, 28, 46, -6],
        [-2, 16, 54, -4],
        [-2, 10, 58, -2],
    ];

    fn clip3(x: i64, y: i64, z: i64) -> i64 {
        if z < x {
            x
        } else if z > y {
            y
        } else {
            z
        }
    }

    /// refPicLX[xA][yA] s xA = Clip3(0, pic_width − 1, x), yA = Clip3(0, pic_height − 1, y).
    fn ref_vzorka(r: &Rovina, x: i64, y: i64) -> i64 {
        let xa = clip3(0, r.sirka as i64 - 1, x);
        let ya = clip3(0, r.vyska as i64 - 1, y);
        r.vzorka(xa as usize, ya as usize) as i64
    }

    /// predSampleLX vzorky na polohe (`xp`, `yp`) = (xPb + xL, yPb + yL) – 8.5.3.3.3.1 až 8.5.3.3.3.3 doslova.
    fn rozhodca_vzorka(jas: bool, r: &Rovina, mv: [i32; 2], xp: i64, yp: i64, bd: u32) -> i64 {
        let shift1 = (bd as i64 - 8).min(4);
        let shift2 = 6;
        let shift3 = (14 - bd as i64).max(2);
        // jas: štvrtiny, body xInt − 3 … xInt + 4; farba 4:2:0: osminy, body xInt − 1 … xInt + 2
        let (log2, od, po) = if jas { (2, -3i64, 4i64) } else { (3, -1, 2) };
        let koef = |frac: i64, i: i64| -> i64 {
            if jas {
                FL[frac as usize - 1][(i + 3) as usize]
            } else {
                FC[frac as usize - 1][(i + 1) as usize]
            }
        };
        let (mv0, mv1) = (mv[0] as i64, mv[1] as i64);
        let x_int = xp + (mv0 >> log2);
        let y_int = yp + (mv1 >> log2);
        let x_frac = mv0 & ((1 << log2) - 1);
        let y_frac = mv1 & ((1 << log2) - 1);
        let refv = |dx: i64, dy: i64| ref_vzorka(r, x_int + dx, y_int + dy);
        if x_frac == 0 && y_frac == 0 {
            refv(0, 0) << shift3
        } else if y_frac == 0 {
            (od..=po).map(|i| koef(x_frac, i) * refv(i, 0)).sum::<i64>() >> shift1
        } else if x_frac == 0 {
            (od..=po).map(|i| koef(y_frac, i) * refv(0, i)).sum::<i64>() >> shift1
        } else {
            let temp = |n: i64| (od..=po).map(|i| koef(x_frac, i) * refv(i, n)).sum::<i64>() >> shift1;
            (od..=po).map(|n| koef(y_frac, n) * temp(n)).sum::<i64>() >> shift2
        }
    }

    /// Celý blok podľa normy: interpolácia (8.5.3.3.3) a vážená predikcia (8.5.3.3.4.2, 8.5.3.3.4.3).
    fn rozhodca(p: &Pripad) -> Vec<Vzorka> {
        let bd = 8u32;
        let max = (1i64 << bd) - 1;
        let shift1 = 14 - bd as i64;
        let mut vystup = vec![0; p.w * p.h];
        for yl in 0..p.h {
            for xl in 0..p.w {
                let (xp, yp) = (p.x as i64 + xl as i64, p.y as i64 + yl as i64);
                let pred = |r: &Option<(Rovina, [i32; 2])>| {
                    r.as_ref().map(|(r, mv)| rozhodca_vzorka(p.jas, r, *mv, xp, yp, bd))
                };
                let (p0, p1) = (pred(&p.r0), pred(&p.r1));
                let v = match &p.vahy {
                    None => match (p0, p1) {
                        (Some(a), Some(b)) => {
                            let shift2 = 15 - bd as i64;
                            let offset2 = 1 << (shift2 - 1);
                            clip3(0, max, (a + b + offset2) >> shift2)
                        }
                        (Some(a), None) | (None, Some(a)) => {
                            let offset1 = if shift1 > 0 { 1 << (shift1 - 1) } else { 0 };
                            clip3(0, max, (a + offset1) >> shift1)
                        }
                        (None, None) => panic!("bez referencie"),
                    },
                    Some(v) => {
                        let log2wd = v.log2_denom as i64 + shift1;
                        let (w0, w1) = (v.w[0] as i64, v.w[1] as i64);
                        let (o0, o1) = (v.o[0] as i64, v.o[1] as i64);
                        let jedna = |pr: i64, w: i64, o: i64| {
                            if log2wd >= 1 {
                                clip3(0, max, ((pr * w + (1 << (log2wd - 1))) >> log2wd) + o)
                            } else {
                                clip3(0, max, pr * w + o)
                            }
                        };
                        match (p0, p1) {
                            (Some(a), Some(b)) => {
                                clip3(0, max, (a * w0 + b * w1 + ((o0 + o1 + 1) << log2wd)) >> (log2wd + 1))
                            }
                            (Some(a), None) => jedna(a, w0, o0),
                            (None, Some(b)) => jedna(b, w1, o1),
                            (None, None) => panic!("bez referencie"),
                        }
                    }
                };
                vystup[yl * p.w + xl] = v as Vzorka;
            }
        }
        vystup
    }

    // ---------------------------------------------------------------------------------------------------------
    // Náhodné prípady
    // ---------------------------------------------------------------------------------------------------------

    /// Rozmery predikčných blokov jasu: 2N×2N, 2N×N, N×2N, 8×4, 4×8 a asymetrické delenia (AMP).
    const BLOKY_JAS: [(usize, usize); 24] = [
        (8, 8),
        (8, 4),
        (4, 8),
        (16, 16),
        (16, 8),
        (8, 16),
        (16, 4),
        (16, 12),
        (4, 16),
        (12, 16),
        (32, 32),
        (32, 16),
        (16, 32),
        (32, 8),
        (32, 24),
        (8, 32),
        (24, 32),
        (64, 64),
        (64, 32),
        (32, 64),
        (64, 16),
        (64, 48),
        (16, 64),
        (48, 64),
    ];

    struct Pripad {
        jas: bool,
        r0: Option<(Rovina, [i32; 2])>,
        r1: Option<(Rovina, [i32; 2])>,
        x: i32,
        y: i32,
        w: usize,
        h: usize,
        vahy: Option<Vahy>,
    }

    impl Pripad {
        fn predikuj(&self, ciel: &mut [Vzorka], krok: usize) {
            let r0 = self.r0.as_ref().map(|(r, mv)| (r, *mv));
            let r1 = self.r1.as_ref().map(|(r, mv)| (r, *mv));
            predikuj(self.jas, r0, r1, self.x, self.y, self.w, self.h, self.vahy.as_ref(), 8, ciel, krok);
        }
    }

    /// Celé číslo v [od, po].
    fn cislo(n: &mut Nahoda, od: i64, po: i64) -> i64 {
        od + n.mensie_ako((po - od + 1) as u64) as i64
    }

    /// Postupnosť znamienok koeficientov náhodného filtra (s periódou filtra) – z nej je zovšeobecnená
    /// šachovnica, pri ktorej súčty filtrov dosahujú krajné hodnoty. Polovica postupností patrí polovičnému
    /// zlomku jasu – len pri ňom medzivýsledok po druhom stupni presiahne i16.
    fn znamienka(n: &mut Nahoda, dlzka: usize) -> Vec<bool> {
        let filter: Vec<i16> = if n.dalsie() & 1 == 0 {
            FILTER_JAS[2].to_vec()
        } else if n.dalsie() & 1 == 0 {
            FILTER_JAS[1 + n.mensie_ako(3) as usize].to_vec()
        } else {
            FILTER_FARBA[1 + n.mensie_ako(7) as usize].to_vec()
        };
        let faza = n.mensie_ako(8) as usize;
        let inverzia = n.dalsie() & 1 == 0;
        (0..dlzka).map(|c| (filter[(c + faza) % filter.len()] > 0) != inverzia).collect()
    }

    /// Náhodná rovina – celé dáta vrátane výplne za koncom riadkov sú najprv náhodné (čítanie mimo platných
    /// vzoriek by sa prejavilo), potom sa platné vzorky vyplnia obsahom jedného druhu.
    fn nahodna_rovina(n: &mut Nahoda, sirka: usize, vyska: usize) -> Rovina {
        let mut r = Rovina::nova(sirka, vyska, 0);
        for v in r.data.iter_mut() {
            *v = n.dalsie() as Vzorka;
        }
        let druh = n.mensie_ako(9);
        let faza = n.mensie_ako(2) as usize;
        let konstanta = n.dalsie() as Vzorka;
        let perioda = 1 + n.mensie_ako(4) as usize;
        let (a, b) = (znamienka(n, sirka), znamienka(n, vyska));
        for yy in 0..vyska {
            for xx in 0..sirka {
                let v = match druh {
                    0 => continue, // šum
                    1 => [0, 255][(xx + yy + faza) % 2],
                    2 | 8 => [0, 255][usize::from(a[xx] == b[yy])],
                    3 => [0, 255][(n.dalsie() & 1) as usize],
                    4 => konstanta,
                    5 => ((xx * 7 + yy * 3 + faza * 100) & 255) as Vzorka,
                    6 => [0, 255][(xx / perioda + faza) % 2],
                    _ => [0, 255][(yy / perioda + faza) % 2],
                };
                r.data[yy * r.krok + xx] = v;
            }
        }
        r
    }

    /// Zložka vektora: náhodný zlomok (častejšie polovičný) a celá časť malá, pri okraji roviny alebo ďaleko
    /// mimo (|mv| až 2^15).
    fn zlozka_vektora(n: &mut Nahoda, jemnost: i64, poloha: i32, rozmer: usize, blok: usize) -> i32 {
        let zlomok = if n.mensie_ako(4) == 0 { jemnost / 2 } else { cislo(n, 0, jemnost - 1) };
        let cela = match n.mensie_ako(4) {
            0 | 1 => cislo(n, -6, 6),
            2 => cislo(n, -(blok as i64) - 10, rozmer as i64 + 10) - poloha as i64,
            _ => cislo(n, -(1 << 15) / jemnost, (1 << 15) / jemnost - 1),
        };
        (cela * jemnost + zlomok).clamp(-(1 << 15), (1 << 15) - 1) as i32
    }

    fn nahodny_pripad(n: &mut Nahoda) -> Pripad {
        let jas = n.dalsie() & 1 == 0;
        let (w, h) = if n.mensie_ako(8) == 0 {
            let m = if jas { 64 } else { 32 };
            (cislo(n, 1, m) as usize, cislo(n, 1, m) as usize)
        } else {
            let (w, h) = BLOKY_JAS[n.mensie_ako(BLOKY_JAS.len() as u64) as usize];
            if jas { (w, h) } else { (w / 2, h / 2) }
        };
        let rozmer = |n: &mut Nahoda| match n.mensie_ako(3) {
            0 => cislo(n, 1, 8) as usize,
            1 => cislo(n, 1, 40) as usize,
            _ => cislo(n, 16, 100) as usize,
        };
        let (sirka, vyska) = (rozmer(n), rozmer(n));
        let (x, y) = if n.mensie_ako(5) == 0 {
            (cislo(n, -70, sirka as i64 + 70) as i32, cislo(n, -70, vyska as i64 + 70) as i32)
        } else {
            (cislo(n, 0, sirka as i64 - 1) as i32, cislo(n, 0, vyska as i64 - 1) as i32)
        };
        let jemnost = if jas { 4 } else { 8 };
        let referencia = |n: &mut Nahoda| {
            let rovina = nahodna_rovina(n, sirka, vyska);
            let mv = [zlozka_vektora(n, jemnost, x, sirka, w), zlozka_vektora(n, jemnost, y, vyska, h)];
            (rovina, mv)
        };
        let (r0, r1) = match n.mensie_ako(4) {
            0 => (Some(referencia(n)), None),
            1 => (None, Some(referencia(n))),
            _ => {
                let r0 = referencia(n);
                let mut r1 = referencia(n);
                if n.mensie_ako(8) == 0 {
                    r1.0 = r0.0.clone(); // ten istý referenčný obraz v oboch zoznamoch
                }
                (Some(r0), Some(r1))
            }
        };
        let vahy = (n.dalsie() & 1 == 0).then(|| Vahy {
            log2_denom: n.mensie_ako(8) as u32,
            w: [cislo(n, -128, 255) as i32, cislo(n, -128, 255) as i32],
            o: [cislo(n, -128, 127) as i32, cislo(n, -128, 127) as i32],
        });
        Pripad { jas, r0, r1, x, y, w, h, vahy }
    }

    /// Predikcia do cieľa s náhodným krokom a náhodným okolím; overí zhodu s rozhodcom aj nezmenené okolie.
    fn over_pripad(n: &mut Nahoda, p: &Pripad) {
        let krok = p.w + n.mensie_ako(17) as usize;
        let mut ciel: Vec<Vzorka> = (0..p.h * krok + 13).map(|_| n.dalsie() as Vzorka).collect();
        let povodny = ciel.clone();
        p.predikuj(&mut ciel, krok);
        let ocakavany = rozhodca(p);
        for (i, (&c, &o)) in ciel.iter().zip(&povodny).enumerate() {
            let (yl, xl) = (i / krok, i % krok);
            if yl < p.h && xl < p.w {
                assert_eq!(
                    c,
                    ocakavany[yl * p.w + xl],
                    "vzorka ({xl}, {yl}): jas {}, blok {}×{} na ({}, {}), mv {:?} / {:?}, roviny {:?}, váhy {:?}",
                    p.jas,
                    p.w,
                    p.h,
                    p.x,
                    p.y,
                    p.r0.as_ref().map(|r| r.1),
                    p.r1.as_ref().map(|r| r.1),
                    p.r0.as_ref().or(p.r1.as_ref()).map(|r| (r.0.sirka, r.0.vyska)),
                    p.vahy
                );
            } else {
                assert_eq!(c, o, "zmenená vzorka mimo bloku na indexe {i}");
            }
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // Skúšky
    // ---------------------------------------------------------------------------------------------------------

    #[test]
    fn filtre_podla_normy() {
        for f in 0..3 {
            assert_eq!(FILTER_JAS[f + 1].map(i64::from), FL[f], "fL[{}]", f + 1);
        }
        for f in 0..7 {
            assert_eq!(FILTER_FARBA[f + 1].map(i64::from), FC[f], "fC[{}]", f + 1);
        }
        for f in 0..4 {
            assert_eq!(FILTER_JAS[f].iter().sum::<i16>(), 64);
            let mut zrkadlo = FILTER_JAS[(4 - f) % 4];
            zrkadlo.reverse();
            if f != 0 {
                assert_eq!(FILTER_JAS[f], zrkadlo, "symetria fL[{f}]");
            }
        }
        for f in 0..8 {
            assert_eq!(FILTER_FARBA[f].iter().sum::<i16>(), 64);
            let mut zrkadlo = FILTER_FARBA[(8 - f) % 8];
            zrkadlo.reverse();
            if f != 0 {
                assert_eq!(FILTER_FARBA[f], zrkadlo, "symetria fC[{f}]");
            }
        }
    }

    /// Medzivýsledky interpolácie (predSampleLX, 14 bitov) sa zhodujú s normou bit po bite.
    #[test]
    fn medzivysledky_zhodne_s_normou() {
        let mut n = Nahoda::zo_semienka(0x1A7E_2B01);
        let mut prac = Pracovne::nove();
        let mut nad_i16 = 0;
        for _ in 0..2500 {
            let p = nahodny_pripad(&mut n);
            let (r, mv) = p.r0.as_ref().or(p.r1.as_ref()).unwrap();
            let blok = Blok { x: p.x, y: p.y, w: p.w, h: p.h };
            let pred = &mut prac.pred[0][..p.w * p.h];
            interpoluj_zlozku(p.jas, (r, *mv), blok, 8, pred, &mut prac.medzi, &mut prac.okraj);
            for yl in 0..p.h {
                for xl in 0..p.w {
                    let (xp, yp) = (p.x as i64 + xl as i64, p.y as i64 + yl as i64);
                    let ocakavany = rozhodca_vzorka(p.jas, r, *mv, xp, yp, 8);
                    nad_i16 += usize::from(ocakavany > i16::MAX as i64);
                    assert_eq!(
                        pred[yl * p.w + xl] as i64 + POSUN_MEDZI as i64,
                        ocakavany,
                        "jas {}, blok {}×{} na ({}, {}), mv {mv:?}, vzorka ({xl}, {yl})",
                        p.jas,
                        p.w,
                        p.h,
                        p.x,
                        p.y
                    );
                }
            }
        }
        // náhodné prípady samy obsahujú medzivýsledky mimo rozsahu i16
        assert!(nad_i16 > 0, "žiadny medzivýsledok nad i16::MAX");
    }

    /// Celá predikcia (uni aj bi, predvolené aj explicitné váhy) sa zhoduje s normou a nemení okolie bloku.
    #[test]
    fn predikcia_zhodna_s_normou() {
        let mut n = Nahoda::zo_semienka(0x1A7E_2B02);
        for _ in 0..3000 {
            let p = nahodny_pripad(&mut n);
            over_pripad(&mut n, &p);
        }
    }

    /// Zovšeobecnená šachovnica 0/255 zarovnaná so znamienkami filtrov: medzivýsledky dosiahnu teoretické
    /// krajné hodnoty (pri jase so zlomkom (2, 2) až 33 150 > i16::MAX) a výsledok sa stále zhoduje s normou.
    #[test]
    fn extremny_obsah() {
        let mut n = Nahoda::zo_semienka(0x1A7E_2B03);
        let mut prac = Pracovne::nove();
        let mut najvacsi = 0;
        for jas in [true, false] {
            let (body, jemnost) = if jas { (8usize, 4i32) } else { (4, 8) };
            let filter = |f: i32| -> Vec<i64> {
                if jas { FL[f as usize - 1].to_vec() } else { FC[f as usize - 1].to_vec() }
            };
            for fx in 1..jemnost {
                for fy in 1..jemnost {
                    let (kx, ky) = (filter(fx), filter(fy));
                    let kladne = |k: &[i64]| k.iter().filter(|&&c| c > 0).sum::<i64>();
                    let zaporne = |k: &[i64]| -k.iter().filter(|&&c| c < 0).sum::<i64>();
                    for inverzia in [false, true] {
                        // p(c, r) = 255, ak sa znamienka fx[c mod N] a fy[r mod N] zhodujú (inverzia: líšia)
                        let mut r = Rovina::nova(3 * body, 3 * body, 0);
                        for yy in 0..r.vyska {
                            for xx in 0..r.sirka {
                                let zhoda = (kx[xx % body] > 0) == (ky[yy % body] > 0);
                                r.data[yy * r.krok + xx] = if zhoda != inverzia { 255 } else { 0 };
                            }
                        }
                        let ocakavany = if inverzia {
                            -((kladne(&kx) * zaporne(&ky) + zaporne(&kx) * kladne(&ky)) * 255) >> 6
                        } else {
                            ((kladne(&kx) * kladne(&ky) + zaporne(&kx) * zaporne(&ky)) * 255) >> 6
                        };
                        // blok na (N/2 − 1, N/2 − 1): okno filtra vzorky (0, 0) začína na (0, 0) roviny
                        let (x, y) = ((body / 2 - 1) as i32, (body / 2 - 1) as i32);
                        let (w, h) = (body * 2, body * 2);
                        let mv = [fx, fy];
                        assert_eq!(rozhodca_vzorka(jas, &r, mv, x as i64, y as i64, 8), ocakavany);
                        let pred = &mut prac.pred[0][..w * h];
                        let blok = Blok { x, y, w, h };
                        interpoluj_zlozku(jas, (&r, mv), blok, 8, pred, &mut prac.medzi, &mut prac.okraj);
                        assert_eq!(
                            pred[0] as i64 + POSUN_MEDZI as i64,
                            ocakavany,
                            "jas {jas}, zlomok ({fx}, {fy})"
                        );
                        najvacsi = najvacsi.max(ocakavany);

                        let vahy = [
                            None,
                            Some(Vahy { log2_denom: 7, w: [-1, 1], o: [127, -128] }),
                            Some(Vahy { log2_denom: 7, w: [1, -1], o: [0, 5] }),
                            Some(Vahy { log2_denom: 0, w: [-128, 255], o: [100, -100] }),
                            Some(Vahy { log2_denom: 3, w: [255, -128], o: [-128, 127] }),
                        ];
                        for v in vahy {
                            for zoznamy in 0..3 {
                                let druha =
                                    Rovina { data: r.data.iter().map(|&s| 255 - s).collect(), ..r.clone() };
                                let (r0, r1) = match zoznamy {
                                    0 => (Some((r.clone(), mv)), None),
                                    1 => (None, Some((r.clone(), mv))),
                                    _ => (Some((r.clone(), mv)), Some((druha, mv))),
                                };
                                let p = Pripad { jas, r0, r1, x, y, w, h, vahy: v };
                                over_pripad(&mut n, &p);
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(najvacsi, 33_150, "krajná hodnota medzivýsledku jasu so zlomkom (2, 2)");
        assert!(najvacsi > i16::MAX as i64);
    }

    #[test]
    fn konstantna_rovina_a_orezanie() {
        // filtre majú súčet 64 → konštantná rovina dá tú istú konštantu pri každom zlomku a každej polohe,
        // aj keď blok leží ďaleko mimo roviny
        for (jas, jemnost) in [(true, 4), (false, 8)] {
            for c in [0, 1, 128, 254, 255] {
                let r = Rovina::nova(5, 3, c);
                for fx in 0..jemnost {
                    for fy in 0..jemnost {
                        for posun in [0, 1 << 14, -(1 << 14)] {
                            let mv = [posun + fx, -posun + fy];
                            let mut ciel = [0; 16 * 8];
                            let rr = Some((&r, mv));
                            predikuj(jas, rr, None, 1, 1, 16, 8, None, 8, &mut ciel, 16);
                            assert!(ciel.iter().all(|&v| v == c), "jas {jas}, c {c}, mv {mv:?}");
                            predikuj(jas, rr, rr, 1, 1, 16, 8, None, 8, &mut ciel, 16);
                            assert!(ciel.iter().all(|&v| v == c));
                            let v = Vahy { log2_denom: 5, w: [32, 32], o: [0, 0] };
                            predikuj(jas, None, rr, 1, 1, 16, 8, Some(&v), 8, &mut ciel, 16);
                            assert!(ciel.iter().all(|&v| v == c));
                        }
                    }
                }
            }
        }
        // celočíselný vektor ďaleko vľavo hore → všade ľavá horná vzorka, vpravo dole → pravá dolná
        let mut r = Rovina::nova(4, 4, 0);
        for (i, v) in r.data.iter_mut().enumerate() {
            *v = i as Vzorka;
        }
        let mut ciel = [0; 8 * 8];
        predikuj(true, Some((&r, [-4000, -4000])), None, 0, 0, 8, 8, None, 8, &mut ciel, 8);
        assert!(ciel.iter().all(|&v| v == r.vzorka(0, 0)));
        predikuj(true, None, Some((&r, [4000, 4000])), 0, 0, 8, 8, None, 8, &mut ciel, 8);
        assert!(ciel.iter().all(|&v| v == r.vzorka(3, 3)));
        // celočíselný vektor vnútri → presná kópia
        let mut r = Rovina::nova(40, 30, 0);
        for (i, v) in r.data.iter_mut().enumerate() {
            *v = (i * 37 % 251) as Vzorka;
        }
        let mut ciel = [0; 16 * 8];
        predikuj(true, Some((&r, [4 * 3, 4 * 5])), None, 7, 2, 16, 8, None, 8, &mut ciel, 16);
        for j in 0..8 {
            assert_eq!(ciel[j * 16..][..16], r.riadok(2 + 5 + j)[10..26]);
        }
    }

    #[test]
    fn bez_referencie_a_prazdny_blok() {
        let r = Rovina::nova(8, 8, 77);
        let mut ciel = [5; 64];
        predikuj::<Rovina>(true, None, None, 0, 0, 8, 8, None, 8, &mut ciel, 8);
        assert!(ciel.iter().all(|&v| v == 5));
        predikuj(true, Some((&r, [1, 1])), None, 0, 0, 0, 8, None, 8, &mut ciel, 8);
        assert!(ciel.iter().all(|&v| v == 5));
    }
}
