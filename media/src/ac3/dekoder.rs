//! Dekodér stopy AC-3 pre prehrávač (`DekoderZvuku`): paket = jeden synchronizačný rámec (6 blokov po 256
//! vzoriek), výstup planárne f32 ±1,0 vo všetkých kanáloch toku (spoločné poradie FL FR FC LFE …).
//!
//! Rozloženie výstupu určí prvý rámec (acmod, lfeon); kontajner udáva len počet kanálov. Rámec s iným počtom
//! kanálov ako stopa (vysielanie prepína 2/0 a 3/2) – výstup ostáva v počte kanálov stopy: chýbajúce kanály sú
//! ticho, prebytočné sa vynechajú (zmiešanie podľa §7.8 patrí R5b). Predbeh pri posune: 1 rámec (prekrytie okna).

use super::{DekoderAc3, Hlavicka};
use crate::paket::Paket;
use crate::stopa::ParametreZvuku;
use crate::zvuk::{BlokZvuku, DekoderZvuku, RozlozenieKanalov, VzorkyZvuku, orez};
use orchidea_jadro::{Chyba, Vysledok};

#[derive(Debug)]
pub struct Ac3Dekoder {
    dekoder: DekoderAc3,
    parametre: ParametreZvuku,
    /// Rozloženie určené prvým rámcom.
    rozlozenie: Option<RozlozenieKanalov>,
}

impl Ac3Dekoder {
    pub fn novy(parametre: ParametreZvuku) -> Vysledok<Ac3Dekoder> {
        if ![32_000, 44_100, 48_000].contains(&parametre.frekvencia) || !(1..=6).contains(&parametre.kanalov)
        {
            return Err(Chyba::PoskodeneData("AC-3: parametre stopy".into()));
        }
        Ok(Ac3Dekoder {
            dekoder: DekoderAc3::novy(),
            parametre: ParametreZvuku { bitov: None, ..parametre },
            rozlozenie: None,
        })
    }
}

impl DekoderZvuku for Ac3Dekoder {
    fn parametre(&self) -> &ParametreZvuku {
        &self.parametre
    }

    fn dekoduj(&mut self, paket: &Paket, vystup: &mut BlokZvuku) -> Vysledok<()> {
        let h =
            Hlavicka::z_bajtov(&paket.data).ok_or_else(|| Chyba::PoskodeneData("AC-3: hlavička".into()))?;
        if h.frekvencia != self.parametre.frekvencia {
            return Err(Chyba::PoskodeneData("AC-3: frekvencia rámca nesedí so stopou".into()));
        }
        let kanaly = self.dekoder.dekoduj_ramec(&paket.data)?;
        let kanalov = self.parametre.kanalov as usize;
        let rozlozenie = *self.rozlozenie.get_or_insert(if h.kanalov() == kanalov {
            h.rozlozenie()
        } else {
            RozlozenieKanalov::predvolene(kanalov as u8).unwrap_or(RozlozenieKanalov::STEREO)
        });
        self.parametre.rozlozenie = rozlozenie;
        let ramcov = 1536;
        let mut data = vec![0f32; ramcov * kanalov];
        for (k, kanal) in kanaly.iter().enumerate().take(kanalov) {
            data[k * ramcov..(k + 1) * ramcov].copy_from_slice(kanal);
        }
        vystup.frekvencia = h.frekvencia;
        vystup.rozlozenie = rozlozenie;
        vystup.ramcov = ramcov;
        vystup.vzorky = VzorkyZvuku::F32(data);
        orez(vystup, paket.orezat_zaciatok as usize, paket.orezat_koniec as usize);
        Ok(())
    }

    fn predbeh_ns(&self) -> i64 {
        1536 * 1_000_000_000 / self.parametre.frekvencia as i64
    }

    fn reset(&mut self) {
        self.dekoder.reset();
    }
}
