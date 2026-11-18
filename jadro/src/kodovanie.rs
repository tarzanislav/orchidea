//! Kanonické binárne kódovanie – jeden presný zápis pre každú hodnotu.
//!
//! Používa sa pre odtlačky (rovnaká hodnota = rovnaké bajty = rovnaký odtlačok), pre denník zmien
//! a pre súbory diela. Čítanie je bezpečné: každá dĺžka sa overí voči limitom a zvyšku dát,
//! poškodené dáta vrátia chybu, nikdy nespôsobia pád ani nadmerné pridelenie pamäte.

use crate::chyba::{Chyba, Vysledok};
use crate::meno::Meno;

/// Limity čítania – chránia pred poškodenými alebo škodlivými dátami.
#[derive(Debug, Clone, Copy)]
pub struct Limity {
    /// Najväčší počet prvkov jedného zoznamu.
    pub prvkov: u64,
    /// Najväčšia dĺžka jedného reťazca alebo bloku bajtov.
    pub bajtov: u64,
    /// Najväčšia hĺbka vnorenia.
    pub hlbka: u32,
}

impl Default for Limity {
    fn default() -> Self {
        Limity { prvkov: 1 << 28, bajtov: 1 << 34, hlbka: 64 }
    }
}

/// Zapisovač kanonického kódovania.
#[derive(Debug, Default, Clone)]
pub struct Zapisovac {
    bajty: Vec<u8>,
}

impl Zapisovac {
    pub fn novy() -> Self {
        Zapisovac { bajty: Vec::new() }
    }

    pub fn bajty(&self) -> &[u8] {
        &self.bajty
    }

    pub fn do_bajtov(self) -> Vec<u8> {
        self.bajty
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.bajty.push(v);
        self
    }

    /// Celé číslo bez znamienka ako LEB128 (najkratší zápis).
    pub fn cislo(&mut self, mut v: u64) -> &mut Self {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.bajty.push(b);
                return self;
            }
            self.bajty.push(b | 0x80);
        }
    }

    /// Celé číslo so znamienkom (cik-cak + LEB128).
    pub fn cele(&mut self, v: i64) -> &mut Self {
        self.cislo(((v << 1) ^ (v >> 63)) as u64)
    }

    pub fn logicka(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }

    /// Číslo s plávajúcou čiarkou – presné bity (NaN sa zjednotí na jeden kanonický tvar).
    pub fn f64(&mut self, v: f64) -> &mut Self {
        let bity = if v.is_nan() { 0x7ff8_0000_0000_0000 } else { v.to_bits() };
        self.bajty.extend_from_slice(&bity.to_le_bytes());
        self
    }

    pub fn f32(&mut self, v: f32) -> &mut Self {
        let bity = if v.is_nan() { 0x7fc0_0000 } else { v.to_bits() };
        self.bajty.extend_from_slice(&bity.to_le_bytes());
        self
    }

    pub fn blok(&mut self, b: &[u8]) -> &mut Self {
        self.cislo(b.len() as u64);
        self.bajty.extend_from_slice(b);
        self
    }

    /// Surové bajty bez dĺžky (dĺžku musí poznať čitateľ).
    pub fn surove(&mut self, b: &[u8]) -> &mut Self {
        self.bajty.extend_from_slice(b);
        self
    }

    pub fn text(&mut self, t: &str) -> &mut Self {
        self.blok(t.as_bytes())
    }

    pub fn meno(&mut self, m: &Meno) -> &mut Self {
        self.text(m.ako_text())
    }
}

/// Čitateľ kanonického kódovania s limitmi.
#[derive(Debug, Clone)]
pub struct Citac<'a> {
    data: &'a [u8],
    poloha: usize,
    limity: Limity,
    hlbka: u32,
}

impl<'a> Citac<'a> {
    pub fn novy(data: &'a [u8]) -> Self {
        Self::s_limitmi(data, Limity::default())
    }

    pub fn s_limitmi(data: &'a [u8], limity: Limity) -> Self {
        Citac { data, poloha: 0, limity, hlbka: 0 }
    }

    pub fn poloha(&self) -> usize {
        self.poloha
    }

    pub fn zostava(&self) -> usize {
        self.data.len() - self.poloha
    }

    pub fn je_koniec(&self) -> bool {
        self.poloha == self.data.len()
    }

    /// Overí, že sa prečítali všetky dáta.
    pub fn koniec(&self) -> Vysledok<()> {
        if self.je_koniec() {
            Ok(())
        } else {
            Err(Chyba::PoskodeneData(format!("{} nadbytočných bajtov na konci", self.zostava())))
        }
    }

    /// Vstup do vnorenej úrovne (stráži hĺbku).
    pub fn vnor(&mut self) -> Vysledok<()> {
        self.hlbka += 1;
        if self.hlbka > self.limity.hlbka {
            return Err(Chyba::Limit(format!("hĺbka vnorenia nad {}", self.limity.hlbka)));
        }
        Ok(())
    }

    pub fn vynor(&mut self) {
        self.hlbka = self.hlbka.saturating_sub(1);
    }

    pub fn u8(&mut self) -> Vysledok<u8> {
        let b = *self
            .data
            .get(self.poloha)
            .ok_or_else(|| Chyba::PoskodeneData("neočakávaný koniec dát".into()))?;
        self.poloha += 1;
        Ok(b)
    }

    pub fn cislo(&mut self) -> Vysledok<u64> {
        let mut v: u64 = 0;
        for i in 0..10 {
            let b = self.u8()?;
            let cast = (b & 0x7f) as u64;
            if i == 9 && cast > 1 {
                return Err(Chyba::PoskodeneData("číslo presahuje 64 bitov".into()));
            }
            v |= cast << (7 * i);
            if b & 0x80 == 0 {
                if i > 0 && b == 0 {
                    return Err(Chyba::PoskodeneData("číslo nie je v najkratšom zápise".into()));
                }
                return Ok(v);
            }
        }
        Err(Chyba::PoskodeneData("príliš dlhé číslo".into()))
    }

    pub fn cele(&mut self) -> Vysledok<i64> {
        let v = self.cislo()?;
        Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
    }

    pub fn logicka(&mut self) -> Vysledok<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(Chyba::PoskodeneData(format!("neplatná logická hodnota {b}"))),
        }
    }

    pub fn surove(&mut self, n: usize) -> Vysledok<&'a [u8]> {
        if n > self.zostava() {
            return Err(Chyba::PoskodeneData(format!("chýba {} bajtov", n - self.zostava())));
        }
        let s = &self.data[self.poloha..self.poloha + n];
        self.poloha += n;
        Ok(s)
    }

    pub fn f64(&mut self) -> Vysledok<f64> {
        let b = self.surove(8)?;
        let mut p = [0u8; 8];
        p.copy_from_slice(b);
        Ok(f64::from_bits(u64::from_le_bytes(p)))
    }

    pub fn f32(&mut self) -> Vysledok<f32> {
        let b = self.surove(4)?;
        Ok(f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
    }

    /// Počet prvkov zoznamu – overený voči limitu a voči zvyšku dát (každý prvok má aspoň `najmenej` bajtov).
    pub fn pocet(&mut self, najmenej: usize) -> Vysledok<usize> {
        let n = self.cislo()?;
        if n > self.limity.prvkov {
            return Err(Chyba::Limit(format!("{n} prvkov je nad limit {}", self.limity.prvkov)));
        }
        if najmenej > 0 && n.saturating_mul(najmenej as u64) > self.zostava() as u64 {
            return Err(Chyba::PoskodeneData(format!("{n} prvkov sa nezmestí do zvyšných dát")));
        }
        usize::try_from(n).map_err(|_| Chyba::Limit("počet prvkov".into()))
    }

    pub fn blok(&mut self) -> Vysledok<&'a [u8]> {
        let n = self.cislo()?;
        if n > self.limity.bajtov {
            return Err(Chyba::Limit(format!("blok {n} bajtov je nad limit")));
        }
        let n = usize::try_from(n).map_err(|_| Chyba::Limit("dĺžka bloku".into()))?;
        self.surove(n)
    }

    pub fn text(&mut self) -> Vysledok<&'a str> {
        let b = self.blok()?;
        std::str::from_utf8(b).map_err(|_| Chyba::PoskodeneData("text nie je platné UTF-8".into()))
    }

    pub fn meno(&mut self) -> Vysledok<Meno> {
        Meno::nove(self.text()?)
    }
}

/// Hodnota, ktorá má kanonický zápis.
pub trait Kodovatelne: Sized {
    fn zakoduj(&self, z: &mut Zapisovac);
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self>;

    /// Kanonické bajty hodnoty.
    fn do_bajtov(&self) -> Vec<u8> {
        let mut z = Zapisovac::novy();
        self.zakoduj(&mut z);
        z.do_bajtov()
    }

    /// Dekódovanie celého bloku (nesmú ostať nadbytočné bajty).
    fn z_bajtov(b: &[u8]) -> Vysledok<Self> {
        let mut c = Citac::novy(b);
        let v = Self::dekoduj(&mut c)?;
        c.koniec()?;
        Ok(v)
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn cisla_tam_a_spat() {
        for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            let mut z = Zapisovac::novy();
            z.cislo(v);
            let mut c = Citac::novy(z.bajty());
            assert_eq!(c.cislo().unwrap(), v);
            assert!(c.je_koniec());
        }
        for v in [0i64, -1, 1, i64::MIN, i64::MAX, -64, 64] {
            let mut z = Zapisovac::novy();
            z.cele(v);
            assert_eq!(Citac::novy(z.bajty()).cele().unwrap(), v);
        }
    }

    #[test]
    fn odmietne_nekanonicke_a_poskodene() {
        assert!(Citac::novy(&[0x80, 0x00]).cislo().is_err());
        assert!(Citac::novy(&[0xff; 11]).cislo().is_err());
        assert!(Citac::novy(&[0x05, b'a']).blok().is_err());
        assert!(Citac::novy(&[0x02, 0xff, 0xfe]).text().is_err());
        let mut c = Citac::novy(&[0xff, 0xff, 0xff, 0x0f]);
        assert!(c.pocet(1).is_err());
    }

    #[test]
    fn nan_je_kanonicky() {
        let mut a = Zapisovac::novy();
        a.f64(f64::NAN);
        let mut b = Zapisovac::novy();
        b.f64(-f64::NAN);
        assert_eq!(a.bajty(), b.bajty());
    }
}
