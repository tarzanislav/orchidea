//! Režim VarDCT JPEG XL (stratové kódovanie): LF obraz, metadáta HF, kvantizačné matice, koeficienty,
//! spätné DCT premenlivých veľkostí, Gaborov filter a EPF, prevod XYB → lineárne RGB.

use super::bity::{Citac, chyba};
use super::hlavicky::{HlavickaSnimky, Metadata};
use super::modular::StromAKody;
use super::snimka::Rozmery;
use orchidea_jadro::Vysledok;

pub(super) struct Stav {}

impl Stav {
    pub fn lf_global(
        _c: &mut Citac,
        _h: &HlavickaSnimky,
        _m: &Metadata,
        _r: Rozmery,
        _lf: [f32; 3],
    ) -> Vysledok<Stav> {
        Err(chyba("VarDCT zatiaľ nie je podporené"))
    }
    pub fn lf_skupina(&mut self, _c: &mut Citac, _g: usize, _s: Option<&StromAKody>) -> Vysledok<()> {
        Ok(())
    }
    pub fn hf_metadata(&mut self, _c: &mut Citac, _g: usize, _s: Option<&StromAKody>) -> Vysledok<()> {
        Ok(())
    }
    pub fn hf_global(&mut self, _c: &mut Citac, _s: Option<&StromAKody>) -> Vysledok<()> {
        Ok(())
    }
    pub fn skupina(&mut self, _c: &mut Citac, _p: usize, _g: usize) -> Vysledok<()> {
        Ok(())
    }
    pub fn dokonci(self, _h: &HlavickaSnimky, _m: &Metadata) -> Vysledok<[Vec<f32>; 3]> {
        Err(chyba("VarDCT zatiaľ nie je podporené"))
    }
}
