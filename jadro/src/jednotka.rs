//! Fyzikálne jednotky a presné zlomky.
//!
//! Jednotka je súčin základných rozmerov SI s celočíselnými exponentmi a násobok voči koherentnej jednotke SI.
//! Vďaka tomu jadro vie, že `Hz` a `s⁻¹` sú to isté, že `nm` sa dá previesť na `m` a že čas nemožno spojiť
//! s dĺžkou (pravidlo S1). Zlomky dávajú presnú časovú základňu (napr. 1001/30000 s) bez zaokrúhľovania.

use crate::chyba::{Chyba, Vysledok};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

/// Poradie základných rozmerov: dĺžka, hmotnosť, čas, prúd, teplota, látkové množstvo, svietivosť.
const ZNACKY_ROZMEROV: [&str; 7] = ["m", "kg", "s", "A", "K", "mol", "cd"];

/// Fyzikálna jednotka.
#[derive(Clone, PartialEq)]
pub struct Jednotka {
    /// Exponenty základných rozmerov SI.
    pub rozmery: [i8; 7],
    /// Násobok voči koherentnej jednotke SI (napr. nm = 1e-9 m).
    pub nasobok: f64,
    /// Voliteľná značka na zobrazenie (napr. „Hz“, „nm“).
    pub znacka: Option<Arc<str>>,
}

impl fmt::Debug for Jednotka {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl Jednotka {
    const fn zakladna(index: usize) -> [i8; 7] {
        let mut r = [0i8; 7];
        r[index] = 1;
        r
    }

    fn so_znackou(rozmery: [i8; 7], nasobok: f64, znacka: &str) -> Jednotka {
        Jednotka { rozmery, nasobok, znacka: Some(Arc::from(znacka)) }
    }

    /// Bezrozmerná jednotka (počet, pomer, index vzorky).
    pub fn bezrozmerna() -> Jednotka {
        Jednotka { rozmery: [0; 7], nasobok: 1.0, znacka: None }
    }
    pub fn meter() -> Jednotka {
        Self::so_znackou(Self::zakladna(0), 1.0, "m")
    }
    pub fn kilogram() -> Jednotka {
        Self::so_znackou(Self::zakladna(1), 1.0, "kg")
    }
    pub fn sekunda() -> Jednotka {
        Self::so_znackou(Self::zakladna(2), 1.0, "s")
    }
    pub fn amper() -> Jednotka {
        Self::so_znackou(Self::zakladna(3), 1.0, "A")
    }
    pub fn kelvin() -> Jednotka {
        Self::so_znackou(Self::zakladna(4), 1.0, "K")
    }
    pub fn mol() -> Jednotka {
        Self::so_znackou(Self::zakladna(5), 1.0, "mol")
    }
    pub fn kandela() -> Jednotka {
        Self::so_znackou(Self::zakladna(6), 1.0, "cd")
    }
    pub fn hertz() -> Jednotka {
        Self::so_znackou([0, 0, -1, 0, 0, 0, 0], 1.0, "Hz")
    }
    pub fn nanometer() -> Jednotka {
        Self::so_znackou(Self::zakladna(0), 1e-9, "nm")
    }
    pub fn milimeter() -> Jednotka {
        Self::so_znackou(Self::zakladna(0), 1e-3, "mm")
    }
    pub fn milisekunda() -> Jednotka {
        Self::so_znackou(Self::zakladna(2), 1e-3, "ms")
    }
    pub fn radian() -> Jednotka {
        Self::so_znackou([0; 7], 1.0, "rad")
    }
    pub fn stupen() -> Jednotka {
        Self::so_znackou([0; 7], std::f64::consts::PI / 180.0, "°")
    }
    /// Kandela na štvorcový meter (jas, napr. pre HDR podľa BT.2100).
    pub fn nit() -> Jednotka {
        Self::so_znackou([-2, 0, 0, 0, 0, 0, 1], 1.0, "cd/m²")
    }

    /// Jednotka s inou značkou a násobkom (napr. kHz = hertz().nasobna(1e3, "kHz")).
    pub fn nasobna(&self, nasobok: f64, znacka: &str) -> Jednotka {
        Jednotka { rozmery: self.rozmery, nasobok: self.nasobok * nasobok, znacka: Some(Arc::from(znacka)) }
    }

    /// Súčin jednotiek.
    pub fn krat(&self, ina: &Jednotka) -> Jednotka {
        let mut r = [0i8; 7];
        for (i, x) in r.iter_mut().enumerate() {
            *x = self.rozmery[i].saturating_add(ina.rozmery[i]);
        }
        Jednotka { rozmery: r, nasobok: self.nasobok * ina.nasobok, znacka: None }
    }

    /// Podiel jednotiek.
    pub fn deleno(&self, ina: &Jednotka) -> Jednotka {
        self.krat(&ina.mocnina(-1))
    }

    /// Celočíselná mocnina jednotky.
    pub fn mocnina(&self, n: i8) -> Jednotka {
        let mut r = [0i8; 7];
        for (i, x) in r.iter_mut().enumerate() {
            *x = self.rozmery[i].saturating_mul(n);
        }
        Jednotka { rozmery: r, nasobok: crate::matematika::mocnina(self.nasobok, n as f64), znacka: None }
    }

    /// Majú jednotky rovnaký rozmer (dajú sa navzájom previesť)?
    pub fn je_zlucitelna(&self, ina: &Jednotka) -> bool {
        self.rozmery == ina.rozmery
    }

    pub fn je_bezrozmerna(&self) -> bool {
        self.rozmery == [0; 7]
    }

    /// Prevedie hodnotu z tejto jednotky do inej.
    pub fn preved(&self, hodnota: f64, na: &Jednotka) -> Vysledok<f64> {
        if !self.je_zlucitelna(na) {
            return Err(Chyba::NezhodaTypov {
                popis: format!("jednotku {self} nemožno previesť na {na}"),
                navrh: None,
            });
        }
        Ok(hodnota * (self.nasobok / na.nasobok))
    }
}

impl fmt::Display for Jednotka {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(z) = &self.znacka {
            return f.write_str(z);
        }
        let mut casti = Vec::new();
        if self.nasobok != 1.0 {
            casti.push(format!("{}", self.nasobok));
        }
        for (i, &e) in self.rozmery.iter().enumerate() {
            match e {
                0 => {}
                1 => casti.push(ZNACKY_ROZMEROV[i].to_string()),
                _ => casti.push(format!("{}^{}", ZNACKY_ROZMEROV[i], e)),
            }
        }
        if casti.is_empty() { f.write_str("1") } else { f.write_str(&casti.join("·")) }
    }
}

impl Kodovatelne for Jednotka {
    fn zakoduj(&self, z: &mut Zapisovac) {
        for &e in &self.rozmery {
            z.cele(e as i64);
        }
        z.f64(self.nasobok);
        match &self.znacka {
            Some(t) => z.u8(1).text(t),
            None => z.u8(0),
        };
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        let mut rozmery = [0i8; 7];
        for e in rozmery.iter_mut() {
            *e = i8::try_from(c.cele()?).map_err(|_| Chyba::PoskodeneData("exponent jednotky".into()))?;
        }
        let nasobok = c.f64()?;
        if !(nasobok.is_finite() && nasobok > 0.0) {
            return Err(Chyba::PoskodeneData("násobok jednotky musí byť kladný".into()));
        }
        let znacka = match c.u8()? {
            0 => None,
            1 => Some(Arc::from(c.text()?)),
            _ => return Err(Chyba::PoskodeneData("značka jednotky".into())),
        };
        Ok(Jednotka { rozmery, nasobok, znacka })
    }
}

fn nsd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Presný zlomok s kladným menovateľom, vždy v základnom tvare.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Zlomok {
    citatel: i64,
    menovatel: u64,
}

impl Zlomok {
    pub const NULA: Zlomok = Zlomok { citatel: 0, menovatel: 1 };
    pub const JEDNA: Zlomok = Zlomok { citatel: 1, menovatel: 1 };

    /// Nový zlomok (menovateľ nesmie byť 0).
    pub fn novy(citatel: i64, menovatel: u64) -> Vysledok<Zlomok> {
        if menovatel == 0 {
            return Err(Chyba::MimoRozsahu("menovateľ zlomku je 0".into()));
        }
        Self::zjednodus(citatel as i128, menovatel as u128).ok_or_else(|| Chyba::MimoRozsahu("zlomok".into()))
    }

    pub fn cele(n: i64) -> Zlomok {
        Zlomok { citatel: n, menovatel: 1 }
    }

    fn zjednodus(c: i128, m: u128) -> Option<Zlomok> {
        let d = nsd(c.unsigned_abs(), m).max(1);
        let c = c / d as i128;
        let m = m / d;
        Some(Zlomok { citatel: i64::try_from(c).ok()?, menovatel: u64::try_from(m).ok()? })
    }

    pub fn citatel(&self) -> i64 {
        self.citatel
    }
    pub fn menovatel(&self) -> u64 {
        self.menovatel
    }

    pub fn ako_f64(&self) -> f64 {
        self.citatel as f64 / self.menovatel as f64
    }

    /// Súčet (None pri pretečení).
    pub fn plus(&self, b: &Zlomok) -> Option<Zlomok> {
        let c = self.citatel as i128 * b.menovatel as i128 + b.citatel as i128 * self.menovatel as i128;
        Self::zjednodus(c, self.menovatel as u128 * b.menovatel as u128)
    }

    /// Súčin (None pri pretečení).
    pub fn krat(&self, b: &Zlomok) -> Option<Zlomok> {
        let c = self.citatel as i128 * b.citatel as i128;
        Self::zjednodus(c, self.menovatel as u128 * b.menovatel as u128)
    }

    /// Súčin s celým číslom (napr. poloha vzorky i = začiatok + i·krok).
    pub fn krat_cele(&self, n: i64) -> Option<Zlomok> {
        self.krat(&Zlomok::cele(n))
    }
}

impl Ord for Zlomok {
    fn cmp(&self, b: &Self) -> Ordering {
        (self.citatel as i128 * b.menovatel as i128).cmp(&(b.citatel as i128 * self.menovatel as i128))
    }
}

impl PartialOrd for Zlomok {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}

impl fmt::Debug for Zlomok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for Zlomok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.menovatel == 1 {
            write!(f, "{}", self.citatel)
        } else {
            write!(f, "{}/{}", self.citatel, self.menovatel)
        }
    }
}

impl Kodovatelne for Zlomok {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.cele(self.citatel).cislo(self.menovatel);
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        let citatel = c.cele()?;
        let menovatel = c.cislo()?;
        let z = Zlomok::novy(citatel, menovatel)?;
        if z.citatel != citatel || z.menovatel != menovatel {
            return Err(Chyba::PoskodeneData("zlomok nie je v základnom tvare".into()));
        }
        Ok(z)
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn jednotky() {
        let hz = Jednotka::hertz();
        let za_sekundu = Jednotka::bezrozmerna().deleno(&Jednotka::sekunda());
        assert!(hz.je_zlucitelna(&za_sekundu));
        assert!(!Jednotka::meter().je_zlucitelna(&Jednotka::sekunda()));
        assert_eq!(Jednotka::nanometer().preved(550.0, &Jednotka::meter()).unwrap(), 550e-9);
        assert!(Jednotka::meter().preved(1.0, &Jednotka::sekunda()).is_err());
        let khz = hz.nasobna(1e3, "kHz");
        assert_eq!(khz.preved(48.0, &hz).unwrap(), 48_000.0);
        assert_eq!(Jednotka::meter().deleno(&Jednotka::sekunda()).to_string(), "m·s^-1");
        let b = Jednotka::nanometer().do_bajtov();
        assert_eq!(Jednotka::z_bajtov(&b).unwrap(), Jednotka::nanometer());
    }

    #[test]
    fn zlomky() {
        let ntsc = Zlomok::novy(1001, 30000).unwrap();
        let sto_snimok = ntsc.krat_cele(100).unwrap();
        assert_eq!(sto_snimok, Zlomok::novy(1001, 300).unwrap());
        assert_eq!(Zlomok::novy(2, 4).unwrap(), Zlomok::novy(1, 2).unwrap());
        assert!(Zlomok::novy(1, 3).unwrap() < Zlomok::novy(1, 2).unwrap());
        assert!(Zlomok::novy(1, 0).is_err());
        assert_eq!(Zlomok::novy(-3, 6).unwrap().to_string(), "-1/2");
        assert!(Zlomok::z_bajtov(&[0x04, 0x04]).is_err()); // 2/4 nie je základný tvar
    }
}
