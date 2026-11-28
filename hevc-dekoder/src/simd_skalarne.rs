//! Jadrá bez ručného NEON – priamo skalárne vzory z `interpolacia` (prekladač ich vektorizuje sám); odblokovanie
//! ostáva v module `odblokovanie`. Použije sa pre 10 bitov a na procesoroch iných ako aarch64; rozhranie je
//! rovnaké ako v module `neon`.

use super::interpolacia::{
    cely_dve_skalarne, predvolene_dve_skalarne, predvolene_jedna_skalarne, vodorovne8_dve_skalarne,
    vodorovne8_jedna_skalarne, vodorovne8_skalarne, zvislo_medzi_dve_skalarne, zvislo_medzi_jedna_skalarne,
    zvislo_medzi_skalarne, zvislo8_dve_skalarne, zvislo8_jedna_skalarne, zvislo8_skalarne,
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

// spojené posledný stupeň filtra a predvolené váženie (8 bitov) – priamo skalárne vzory z `interpolacia`

#[inline]
pub fn vodorovne8_jedna<const N: usize>(zdroj: &[Vzorka], koef: &[i16; N], ciel: &mut [Vzorka]) {
    vodorovne8_jedna_skalarne(zdroj, koef, ciel);
}

#[inline]
pub fn vodorovne8_dve<const N: usize>(zdroj: &[Vzorka], koef: &[i16; N], pred0: &[i16], ciel: &mut [Vzorka]) {
    vodorovne8_dve_skalarne(zdroj, koef, pred0, ciel);
}

#[inline]
pub fn zvislo8_jedna<const N: usize>(riadky: &[&[Vzorka]; N], koef: &[i16; N], ciel: &mut [Vzorka]) {
    zvislo8_jedna_skalarne(riadky, koef, ciel);
}

#[inline]
pub fn zvislo8_dve<const N: usize>(
    riadky: &[&[Vzorka]; N],
    koef: &[i16; N],
    pred0: &[i16],
    ciel: &mut [Vzorka],
) {
    zvislo8_dve_skalarne(riadky, koef, pred0, ciel);
}

#[inline]
pub fn zvislo_medzi_jedna<const N: usize>(riadky: &[&[i16]; N], koef: &[i16; N], ciel: &mut [Vzorka]) {
    zvislo_medzi_jedna_skalarne(riadky, koef, ciel);
}

#[inline]
pub fn zvislo_medzi_dve<const N: usize>(
    riadky: &[&[i16]; N],
    koef: &[i16; N],
    pred0: &[i16],
    ciel: &mut [Vzorka],
) {
    zvislo_medzi_dve_skalarne(riadky, koef, pred0, ciel);
}

#[inline]
pub fn cely_dve(zdroj: &[Vzorka], pred0: &[i16], ciel: &mut [Vzorka]) {
    cely_dve_skalarne(zdroj, pred0, ciel);
}

/// SAO hranový posun – bez ručného NEON sa tu nespracuje nič (0 vzoriek), počíta skalárna cesta v `sao`.
#[inline(always)]
pub fn sao_hrana(
    _ciel: &mut [Vzorka],
    _stred: &[Vzorka],
    _a: &[Vzorka],
    _b: &[Vzorka],
    _t: [i16; 5],
    _max: i16,
) -> usize {
    0
}

/// SAO pásmový posun – bez ručného NEON 0 vzoriek (ako `sao_hrana`).
#[inline(always)]
pub fn sao_pasmo(
    _ciel: &mut [Vzorka],
    _stred: &[Vzorka],
    _posuny: [i16; 4],
    _pasmo: u16,
    _posun_pasma: u32,
    _max: i16,
) -> usize {
    0
}

/// Úsek hrany jasu – bez ručného NEON sa nespracuje tu (`false`), odblokovanie použije skalárnu cestu.
#[inline(always)]
pub fn odblokuj_jas(
    _data: &mut [Vzorka],
    _i: usize,
    _krok: usize,
    _zvisla: bool,
    _beta: i32,
    _tc: i32,
    _bez: [bool; 2],
) -> bool {
    false
}
