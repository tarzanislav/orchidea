//! Dekodér stopy MPEG audio pre prehrávač (`DekoderZvuku`): paket = jeden rámec, výstup planárne f32 ±1,0.
//!
//! Orezanie podľa paketu (bezmedzerovo – Xing/LAME v súbore .mp3, `elst` v MP4, `CodecDelay` v Matroske) uplatní
//! dekodér po dekódovaní celého rámca. Predbeh pri posune pokryje zásobník bitov (až 511 B späť, pri 32 kb/s
//! ≈ 8 rámcov) a prekrytie banky filtrov: 10 rámcov.

use super::DekoderMpa;
use crate::paket::Paket;
use crate::stopa::ParametreZvuku;
use crate::zvuk::{BlokZvuku, DekoderZvuku, RozlozenieKanalov, VzorkyZvuku, orez};
use orchidea_jadro::{Chyba, Vysledok};

/// Rámcov predbehu pri posune.
const PREDBEH_RAMCOV: i64 = 10;

#[derive(Debug)]
pub struct MpaDekoder {
    dekoder: DekoderMpa,
    parametre: ParametreZvuku,
}

impl MpaDekoder {
    pub fn novy(parametre: ParametreZvuku) -> Vysledok<MpaDekoder> {
        if parametre.frekvencia == 0 || !(1..=2).contains(&parametre.kanalov) {
            return Err(Chyba::PoskodeneData("MPEG audio: parametre stopy".into()));
        }
        let rozlozenie =
            if parametre.kanalov == 1 { RozlozenieKanalov::MONO } else { RozlozenieKanalov::STEREO };
        Ok(MpaDekoder {
            dekoder: DekoderMpa::novy(),
            parametre: ParametreZvuku { rozlozenie, bitov: None, ..parametre },
        })
    }
}

impl DekoderZvuku for MpaDekoder {
    fn parametre(&self) -> &ParametreZvuku {
        &self.parametre
    }

    fn dekoduj(&mut self, paket: &Paket, vystup: &mut BlokZvuku) -> Vysledok<()> {
        let kanaly = self.dekoder.dekoduj_ramec(&paket.data)?;
        let h = self.dekoder.hlavicka().ok_or_else(|| Chyba::PoskodeneData("MPEG audio: hlavička".into()))?;
        // tok s inou frekvenciou alebo počtom kanálov, než ohlásila stopa: chyba (zmena uprostred toku)
        if h.frekvencia != self.parametre.frekvencia || kanaly.len() != self.parametre.kanalov as usize {
            return Err(Chyba::PoskodeneData("MPEG audio: rámec nesedí s parametrami stopy".into()));
        }
        let ramcov = kanaly[0].len();
        let mut data = Vec::with_capacity(ramcov * kanaly.len());
        for k in &kanaly {
            data.extend_from_slice(k);
        }
        vystup.frekvencia = h.frekvencia;
        vystup.rozlozenie = self.parametre.rozlozenie;
        vystup.ramcov = ramcov;
        vystup.vzorky = VzorkyZvuku::F32(data);
        orez(vystup, paket.orezat_zaciatok as usize, paket.orezat_koniec as usize);
        Ok(())
    }

    fn predbeh_ns(&self) -> i64 {
        PREDBEH_RAMCOV * 1152 * 1_000_000_000 / self.parametre.frekvencia as i64
    }

    fn reset(&mut self) {
        self.dekoder.reset();
    }
}
