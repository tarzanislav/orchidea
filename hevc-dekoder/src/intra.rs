//! Vnútrosnímková predikcia (intra prediction) – H.265, kap. 8.4.4.2.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`Susedia`] a [`predikuj`]. Dekodér vyplní susedné vzorky a ich
//! dostupnosť (6.4.1 a obmedzená vnútrosnímková predikcia); tento modul urobí náhradu nedostupných vzoriek
//! (8.4.4.2.2), filtrovanie susedov (8.4.4.2.3) a samotnú predikciu (8.4.4.2.4–8.4.4.2.6).

use crate::obraz::Vzorka;

/// Najväčší transformačný blok 32×32 → 4 · 32 + 1 = 129 susedných vzoriek.
pub const MAX_SUSEDOV: usize = 129;

/// Susedné vzorky bloku nTbS × nTbS v poradí, v akom ich prechádza náhrada (8.4.4.2.2):
/// - index `0` = p[−1][2·nTbS − 1] (dole vľavo), smerom nahor po index `2·nTbS − 1` = p[−1][0],
/// - index `2·nTbS` = p[−1][−1] (roh),
/// - index `2·nTbS + 1 + x` = p[x][−1] pre x = 0 … 2·nTbS − 1 (hore, zľava doprava).
///
/// Platných je prvých `4·nTbS + 1` prvkov. `dostupne[i] == false` = vzorka nie je dostupná (mimo obrazu, ešte
/// nedekódovaná, iný rez či dlaždica, alebo medzisnímkový blok pri constrained_intra_pred_flag).
#[derive(Debug, Clone)]
pub struct Susedia {
    pub vzorky: [u16; MAX_SUSEDOV],
    pub dostupne: [bool; MAX_SUSEDOV],
}

impl Default for Susedia {
    fn default() -> Self {
        Susedia { vzorky: [0; MAX_SUSEDOV], dostupne: [false; MAX_SUSEDOV] }
    }
}

/// Vnútrosnímková predikcia jedného transformačného bloku nTbS × nTbS, nTbS = 1 << `log2_n` (4 … 32).
///
/// - `rezim`: predModeIntra 0 (planárna), 1 (DC), 2 … 34 (uhlové),
/// - `jas`: true pre zložku jasu (cIdx = 0). Pri 4:2:0 sa susedia filtrujú (8.4.4.2.3) a hraničné filtre DC,
///   režimu 10 a 26 použijú **len pre jas** a len pri nTbS < 32,
/// - `silne_vyhladenie`: strong_intra_smoothing_enabled_flag (bilineárne vyhladenie len jas 32×32),
/// - výsledok sa zapíše do `ciel`: vzorka (x, y) na `ciel[y * krok + x]`.
#[allow(clippy::too_many_arguments)]
pub fn predikuj(
    susedia: &Susedia,
    log2_n: u32,
    rezim: u32,
    jas: bool,
    bitova_hlbka: u32,
    silne_vyhladenie: bool,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    // ZÁSTUPNÁ IMPLEMENTÁCIA – nahradí ju modul vnútrosnímkovej predikcie
    let _ = (susedia, rezim, jas, silne_vyhladenie);
    let n = 1usize << log2_n;
    let stred = (1u32 << (bitova_hlbka - 1)) as Vzorka;
    for y in 0..n {
        ciel[y * krok..y * krok + n].fill(stred);
    }
}
