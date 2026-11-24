//! Medzisnímková predikcia vzoriek: interpolácia zlomkových polôh a vážená predikcia – H.265, kap. 8.5.3.3.3
//! a 8.5.3.3.4.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`Vahy`] a [`predikuj`].

use crate::obraz::{Rovina, Vzorka};

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
#[allow(clippy::too_many_arguments)]
pub fn predikuj(
    jas: bool,
    ref0: Option<(&Rovina, [i32; 2])>,
    ref1: Option<(&Rovina, [i32; 2])>,
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    vahy: Option<&Vahy>,
    bitova_hlbka: u32,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    // ZÁSTUPNÁ IMPLEMENTÁCIA – nahradí ju modul interpolácie
    let _ = (jas, ref0, ref1, x, y, vahy, bitova_hlbka);
    for j in 0..h {
        ciel[j * krok..j * krok + w].fill(128);
    }
}
