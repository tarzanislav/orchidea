//! **Dlaždicový raster s kopírovaním pri zápise (R70) a pyramída náhľadu (R71)** – základ prehliadača
//! a editora fotografií (Pohľad) a celej práce s obrazom v ORCHIDEI.
//!
//! - **Dlaždice:** obraz je mriežka štvorcových dlaždíc. Dlaždica je nemenná a zdieľaná; úprava vytvorí novú
//!   len pre dlaždice, ktorých sa dotkne. História (späť, znova) tak zdieľa všetko nezmenené – sto ťahov
//!   štetcom na obraze 4K stojí pamäť sto malých plôch, nie sto obrazov (FOTOSYNTÉZA Z2).
//! - **Jednoliate dlaždice:** dlaždica, ktorej všetky pixely sú rovnaké, sa uloží ako jedna hodnota – prázdne
//!   plátno 8K zaberie pár bajtov a vyplnenie plochy nič neprepisuje (Z10).
//! - **Odtlačky:** každá dlaždica má odtlačok BLAKE3 v kanonickom tvare (jednoliata a rozbalená dlaždica
//!   s rovnakým obsahom majú rovnaký odtlačok); odtlačok obrazu je strom odtlačkov – po úprave sa prepočíta
//!   len to, čo sa zmenilo.
//! - **Pyramída náhľadu:** každá ďalšia úroveň má polovičné rozmery; po úprave sa prepočítajú len dlaždice nad
//!   zmenou. Náhľad sa berie z úrovne, ktorá má aspoň rozlíšenie obrazovky (Z7); export je vždy z plnej úrovne.
//! - **Bitová zhoda:** výsledok nezávisí od veľkosti dlaždice ani od počtu vlákien. Veľkosť dlaždice preto
//!   môže voliť profil hardvéru podľa vyrovnávacej pamäte procesora (Z11) – [`strana_pre`].
//!
//! Súradnice sú v pixeloch: `x` zľava doprava, `y` zhora nadol. Hodnoty pixela sú kanály za sebou
//! (napríklad R, G, B, A). Prevod na signál jadra má osi „y“, „x“ a „kanál“.

mod dlazdica;
mod pyramida;
mod rastr;

pub use dlazdica::Dlazdica;
pub use pyramida::Pyramida;
pub use rastr::Rastr;

use orchidea_zariadenie::ProfilHardveru;

/// Najmenšia a najväčšia strana dlaždice (v pixeloch, mocnina dvoch).
pub const NAJMENSIA_STRANA: usize = 16;
pub const NAJVACSIA_STRANA: usize = 1024;

/// Obdĺžnik v pixeloch: ľavý horný roh `x`, `y` a rozmery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Obdlznik {
    pub x: usize,
    pub y: usize,
    pub sirka: usize,
    pub vyska: usize,
}

impl Obdlznik {
    pub const fn novy(x: usize, y: usize, sirka: usize, vyska: usize) -> Obdlznik {
        Obdlznik { x, y, sirka, vyska }
    }

    pub fn je_prazdny(&self) -> bool {
        self.sirka == 0 || self.vyska == 0
    }

    /// Prvý stĺpec za obdĺžnikom.
    pub fn koniec_x(&self) -> usize {
        self.x + self.sirka
    }

    /// Prvý riadok pod obdĺžnikom.
    pub fn koniec_y(&self) -> usize {
        self.y + self.vyska
    }

    /// Spoločná časť dvoch obdĺžnikov (prázdna, ak sa neprekrývajú).
    pub fn prienik(&self, iny: &Obdlznik) -> Obdlznik {
        let x = self.x.max(iny.x);
        let y = self.y.max(iny.y);
        let kx = self.koniec_x().min(iny.koniec_x());
        let ky = self.koniec_y().min(iny.koniec_y());
        if kx <= x || ky <= y { Obdlznik::novy(x, y, 0, 0) } else { Obdlznik::novy(x, y, kx - x, ky - y) }
    }

    /// Najmenší obdĺžnik, ktorý obsahuje oba (prázdny sa neráta).
    pub fn obal(&self, iny: &Obdlznik) -> Obdlznik {
        if self.je_prazdny() {
            return *iny;
        }
        if iny.je_prazdny() {
            return *self;
        }
        let (x, y) = (self.x.min(iny.x), self.y.min(iny.y));
        Obdlznik::novy(x, y, self.koniec_x().max(iny.koniec_x()) - x, self.koniec_y().max(iny.koniec_y()) - y)
    }

    /// Obdĺžnik na úrovni s polovičnými rozmermi, ktorý pokryje všetky pixely závislé od tohto obdĺžnika.
    pub fn polovica(&self) -> Obdlznik {
        let (x, y) = (self.x / 2, self.y / 2);
        Obdlznik::novy(x, y, self.koniec_x().div_ceil(2) - x, self.koniec_y().div_ceil(2) - y)
    }

    /// Obsahuje obdĺžnik celý obdĺžnik `iny`?
    pub fn obsahuje(&self, iny: &Obdlznik) -> bool {
        iny.x >= self.x
            && iny.y >= self.y
            && iny.koniec_x() <= self.koniec_x()
            && iny.koniec_y() <= self.koniec_y()
    }
}

/// Veľkosť dlaždice podľa profilu hardvéru (FOTOSYNTÉZA Z11): najväčšia mocnina dvoch od 64 do 256, pri ktorej
/// sa vstupná aj výstupná dlaždica zmestia do vyrovnávacej pamäte L2 jedného jadra. Bez údaja o L2 sa počíta so 256 kB
/// (bežné ARM, napríklad Raspberry Pi 3 má 512 kB pre štyri jadrá, teda 128 kB na jadro).
///
/// Veľkosť dlaždice mení len rýchlosť a pamäť, nikdy výsledok (bitová zhoda).
pub fn strana_pre(profil: &ProfilHardveru, bajtov_na_pixel: usize) -> usize {
    let l2 = profil.l2_na_jadro_kb.unwrap_or(256) as usize * 1024;
    let rozpocet = (l2 / 2).max(1);
    let bpp = bajtov_na_pixel.max(1);
    // najviac 256: pri úprave sa kopíruje celá dlaždica, preto väčšia by zväčšila históriu aj obnovu náhľadu
    let mut strana = 256;
    while strana > 64 && strana * strana * bpp > rozpocet {
        strana /= 2;
    }
    strana
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn prienik_obdlznikov() {
        let a = Obdlznik::novy(10, 10, 20, 20);
        assert_eq!(a.prienik(&Obdlznik::novy(25, 0, 100, 15)), Obdlznik::novy(25, 10, 5, 5));
        assert!(a.prienik(&Obdlznik::novy(30, 10, 5, 5)).je_prazdny());
        assert!(a.obsahuje(&Obdlznik::novy(10, 10, 20, 20)));
        assert!(!a.obsahuje(&Obdlznik::novy(9, 10, 2, 2)));
        assert_eq!(a.obal(&Obdlznik::novy(0, 40, 5, 5)), Obdlznik::novy(0, 10, 30, 35));
        assert_eq!(Obdlznik::novy(5, 4, 2, 3).polovica(), Obdlznik::novy(2, 2, 2, 2));
    }

    #[test]
    fn strana_podla_vyrovnavacej_pamate() {
        // Raspberry Pi 3: 512 kB L2 pre 4 jadrá
        let mut p = ProfilHardveru { l2_na_jadro_kb: Some(128), ..Default::default() };
        assert_eq!(strana_pre(&p, 4), 128);
        assert_eq!(strana_pre(&p, 16), 64);
        // PC s 1 MB L2 na jadro
        p.l2_na_jadro_kb = Some(1024);
        assert_eq!(strana_pre(&p, 4), 256);
        assert_eq!(strana_pre(&p, 16), 128);
        p.l2_na_jadro_kb = None;
        assert_eq!(strana_pre(&p, 4), 128);
    }
}
