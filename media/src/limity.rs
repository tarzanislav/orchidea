//! **Limity pamäte a veľkosti** pre čítačky kontajnerov a dekodéry (R211). Údaje zo súboru nikdy neurčia
//! alokáciu samy: každá veľkosť sa overí voči limitu **a voči zvyšku súboru**, až potom sa pamäť vyhradí.

use orchidea_jadro::{Chyba, Vysledok};
use orchidea_zariadenie::ProfilHardveru;

/// Bezpečnostné limity čítania médií.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limity {
    /// Najväčší prvok načítaný celý do pamäte (prvok EBML, box MP4, sekcia TS) – metadáta a konfigurácia kodeku.
    pub prvok: u64,
    /// Najväčší paket (komprimovaný rámec).
    pub paket: u64,
    /// Najviac bajtov pamäte na indexy všetkých stôp spolu (tabuľky vzoriek MP4, body posunu Matrosky).
    pub index: u64,
    /// Najviac stôp v jednom súbore.
    pub stop: usize,
    /// Najväčšia hĺbka vnorenia prvkov a boxov.
    pub hlbka: usize,
}

impl Default for Limity {
    fn default() -> Self {
        Limity { prvok: 16 << 20, paket: 64 << 20, index: 256 << 20, stop: 256, hlbka: 32 }
    }
}

impl Limity {
    /// Limity podľa zariadenia: indexy smú zabrať najviac osminu pamäte (Raspberry Pi 1 s 512 MB → 64 MB,
    /// teda ≈ 16 miliónov vzoriek MP4), najmenej 16 MB.
    pub fn pre_profil(profil: &ProfilHardveru) -> Limity {
        let pamat = if profil.pamat_kb > 0 { profil.pamat_kb * 1024 } else { 4 << 30 };
        Limity { index: (pamat / 8).clamp(16 << 20, 256 << 20), ..Limity::default() }
    }

    fn over(velkost: u64, limit: u64, zostava: u64, co: &str) -> Vysledok<usize> {
        if velkost > zostava {
            return Err(Chyba::PoskodeneData(format!(
                "{co} má {velkost} B, v súbore ostáva len {zostava} B"
            )));
        }
        if velkost > limit {
            return Err(Chyba::Limit(format!("{co} má {velkost} B, limit je {limit} B")));
        }
        usize::try_from(velkost).map_err(|_| Chyba::Limit(format!("{co} sa nezmestí do pamäte")))
    }

    /// Overí prvok, ktorý sa má načítať celý (`zostava` = bajtov do konca nadradeného prvku alebo súboru).
    pub fn over_prvok(&self, velkost: u64, zostava: u64) -> Vysledok<usize> {
        Self::over(velkost, self.prvok, zostava, "prvok")
    }

    /// Overí veľkosť paketu.
    pub fn over_paket(&self, velkost: u64, zostava: u64) -> Vysledok<usize> {
        Self::over(velkost, self.paket, zostava, "paket")
    }

    /// Overí, či sa ďalších `bajtov` indexu zmestí do zvyšku limitu (`pouzite` = doteraz použité bajty indexov).
    pub fn over_index(&self, pouzite: u64, bajtov: u64) -> Vysledok<()> {
        match pouzite.checked_add(bajtov) {
            Some(s) if s <= self.index => Ok(()),
            _ => Err(Chyba::Limit(format!(
                "index by mal {} B, limit je {} B",
                pouzite.saturating_add(bajtov),
                self.index
            ))),
        }
    }

    /// Overí počet stôp.
    pub fn over_stopy(&self, pocet: usize) -> Vysledok<()> {
        if pocet > self.stop {
            return Err(Chyba::Limit(format!("{pocet} stôp, limit je {}", self.stop)));
        }
        Ok(())
    }

    /// Overí hĺbku vnorenia.
    pub fn over_hlbku(&self, hlbka: usize) -> Vysledok<()> {
        if hlbka > self.hlbka {
            return Err(Chyba::Limit(format!("vnorenie {hlbka}, limit je {}", self.hlbka)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn prvok_nad_zvysok_suboru_je_poskodenie() {
        let l = Limity::default();
        assert_eq!(l.over_prvok(100, 200).unwrap(), 100);
        assert!(matches!(l.over_prvok(4 << 30, 10_000), Err(Chyba::PoskodeneData(_))));
        assert!(matches!(l.over_prvok(32 << 20, 1 << 40), Err(Chyba::Limit(_))));
        assert!(matches!(l.over_paket(65 << 20, 1 << 40), Err(Chyba::Limit(_))));
    }

    #[test]
    fn index_stopy_hlbka() {
        let l = Limity { index: 1000, stop: 2, hlbka: 3, ..Limity::default() };
        assert!(l.over_index(900, 100).is_ok());
        assert!(l.over_index(901, 100).is_err());
        assert!(l.over_index(u64::MAX, 1).is_err());
        assert!(l.over_stopy(2).is_ok() && l.over_stopy(3).is_err());
        assert!(l.over_hlbku(3).is_ok() && l.over_hlbku(4).is_err());
    }

    #[test]
    fn profil_malej_dosky() {
        let p = ProfilHardveru { pamat_kb: 512 * 1024, ..ProfilHardveru::default() };
        assert_eq!(Limity::pre_profil(&p).index, 64 << 20);
        let p = ProfilHardveru { pamat_kb: 8 * 1024 * 1024, ..ProfilHardveru::default() };
        assert_eq!(Limity::pre_profil(&p).index, 256 << 20);
        let p = ProfilHardveru { pamat_kb: 64 * 1024, ..ProfilHardveru::default() };
        assert_eq!(Limity::pre_profil(&p).index, 16 << 20);
    }
}
