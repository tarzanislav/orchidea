//! Adaptívny posun vzoriek (sample adaptive offset, SAO) – H.265, kap. 8.7.3.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`SaoCtb`] a [`sao`].

use crate::filtre::{InfoObrazu, MapyFiltra};
use crate::obraz::Obraz;

/// Parametre SAO jedného CTB pre zložky Y, Cb, Cr (už po zlúčení s ľavým či horným CTB).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaoCtb {
    /// SaoTypeIdx: 0 = vypnuté, 1 = pásmový posun (band offset), 2 = hranový posun (edge offset)
    pub typ: [u8; 3],
    /// SaoOffsetVal[1 … 4] so znamienkom (už posunuté o log2OffsetScale); SaoOffsetVal[0] = 0
    pub posuny: [[i16; 4]; 3],
    /// sao_band_position
    pub pasmo: [u8; 3],
    /// SaoEoClass: 0 vodorovne, 1 zvislo, 2 uhlopriečka 135°, 3 uhlopriečka 45°
    pub trieda: [u8; 3],
}

/// SAO celého obrazu: `vstup` je obraz po odblokovaní, do `vystup` (rovnaké rozmery) sa zapíše **každá** vzorka –
/// upravená podľa 8.7.3, alebo skopírovaná, ak sa nemení (SaoTypeIdx = 0, `bez_filtra`, sused mimo obrazu,
/// v inom reze či dlaždici, cez ktorú sa nefiltruje).
///
/// `ctb[ry * sirka_ctb + rx]` sú parametre CTB. Hranica rezu: sused v inom reze sa nepoužije, ak rez vzorky,
/// ktorá je v poradí dekódovania neskôr, má slice_loop_filter_across_slices_enabled_flag = 0 (poradie CTB je
/// `info.ctb_ts`); hranica dlaždice podľa `info.cez_dlazdice`.
pub fn sao(vstup: &Obraz, vystup: &mut Obraz, ctb: &[SaoCtb], mapy: &MapyFiltra, info: &InfoObrazu) {
    // ZÁSTUPNÁ IMPLEMENTÁCIA – nahradí ju modul SAO
    let _ = (ctb, mapy, info);
    vystup.clone_from(vstup);
}
