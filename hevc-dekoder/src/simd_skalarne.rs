//! Jadrá interpolácie bez ručného NEON – priamo skalárne vzory z `interpolacia` (prekladač ich vektorizuje sám).
//! Použije sa pre 10 bitov a na procesoroch iných ako aarch64; rozhranie je rovnaké ako v module `neon`.

use super::interpolacia::{
    predvolene_dve_skalarne, predvolene_jedna_skalarne, vodorovne8_skalarne, zvislo_medzi_skalarne,
    zvislo8_skalarne,
};
use super::obraz::Vzorka;

#[inline]
pub fn vodorovne8<const N: usize>(zdroj: &[Vzorka], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    vodorovne8_skalarne(zdroj, koef, ciel, odcitaj);
}

#[inline]
pub fn zvislo8<const N: usize>(riadky: &[&[Vzorka]; N], koef: &[i16; N], ciel: &mut [i16], odcitaj: i16) {
    zvislo8_skalarne(riadky, koef, ciel, odcitaj);
}

#[inline]
pub fn zvislo_medzi<const N: usize>(riadky: &[&[i16]; N], koef: &[i16; N], ciel: &mut [i16]) {
    zvislo_medzi_skalarne(riadky, koef, ciel);
}

#[inline]
pub fn predvolene_dve(pred0: &[i16], pred1: &[i16], ciel: &mut [Vzorka], pridaj: i32, shift: u32, max: i32) {
    predvolene_dve_skalarne(pred0, pred1, ciel, pridaj, shift, max);
}

#[inline]
pub fn predvolene_jedna(pred: &[i16], ciel: &mut [Vzorka], pridaj: i32, shift: u32, max: i32) {
    predvolene_jedna_skalarne(pred, ciel, pridaj, shift, max);
}
