//! Čítač bitov JPEG XL (od najnižšieho bitu) a základné typy polí normy: U32, U64, F16, Enum, U8.

use crate::polovicna::f16_na_f32;
use orchidea_jadro::{Chyba, Vysledok};

pub(super) fn chyba(text: &str) -> Chyba {
    Chyba::PoskodeneData(format!("JPEG XL: {text}"))
}

/// Rozdelenie hodnoty v poli U32: pevná hodnota alebo `n` bitov s posunom.
#[derive(Clone, Copy)]
pub(super) enum D {
    V(u32),
    B(u32, u32),
}

pub(super) struct Citac<'a> {
    data: &'a [u8],
    poz: usize,
    akum: u64,
    bitov: u32,
    /// Bity doplnené za koncom údajov (nuly).
    za_koncom: u64,
    prec: u64,
}

impl<'a> Citac<'a> {
    pub fn novy(data: &'a [u8]) -> Citac<'a> {
        Citac { data, poz: 0, akum: 0, bitov: 0, za_koncom: 0, prec: 0 }
    }

    #[inline]
    fn dopln(&mut self) {
        if self.poz + 8 <= self.data.len() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&self.data[self.poz..self.poz + 8]);
            self.akum |= u64::from_le_bytes(b) << self.bitov;
            let pridat = (63 - self.bitov) / 8;
            self.poz += pridat as usize;
            self.bitov += pridat * 8;
        } else {
            while self.bitov <= 56 {
                let b = if self.poz < self.data.len() {
                    self.poz += 1;
                    self.data[self.poz - 1]
                } else {
                    self.za_koncom += 8;
                    0
                };
                self.akum |= (b as u64) << self.bitov;
                self.bitov += 8;
            }
        }
    }

    /// Najbližších `n` bitov bez posunu (najviac 32).
    #[inline]
    pub fn pozri(&mut self, n: u32) -> u64 {
        if self.bitov < n {
            self.dopln();
        }
        self.akum & ((1u64 << n) - 1)
    }

    #[inline]
    pub fn zahod(&mut self, n: u32) {
        self.akum >>= n;
        self.bitov -= n;
        self.prec += n as u64;
    }

    /// `n` bitov (0–32).
    #[inline]
    pub fn u(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.pozri(n) as u32;
        self.zahod(n);
        v
    }

    pub fn bool(&mut self) -> bool {
        self.u(1) == 1
    }

    pub fn u32(&mut self, d: [D; 4]) -> u32 {
        match d[self.u(2) as usize] {
            D::V(v) => v,
            D::B(n, o) => self.u(n).wrapping_add(o),
        }
    }

    pub fn u64(&mut self) -> u64 {
        match self.u(2) {
            0 => 0,
            1 => 1 + self.u(4) as u64,
            2 => 17 + self.u(8) as u64,
            _ => {
                let mut v = self.u(12) as u64;
                let mut posun = 12;
                while self.bool() {
                    if posun == 60 {
                        v |= (self.u(4) as u64) << posun;
                        break;
                    }
                    v |= (self.u(8) as u64) << posun;
                    posun += 8;
                }
                v
            }
        }
    }

    pub fn f16(&mut self) -> Vysledok<f32> {
        let h = self.u(16) as u16;
        if h & 0x7C00 == 0x7C00 {
            return Err(chyba("nekonečná hodnota F16"));
        }
        Ok(f16_na_f32(h))
    }

    pub fn enum_(&mut self) -> Vysledok<u32> {
        let v = self.u32([D::V(0), D::V(1), D::B(4, 2), D::B(6, 18)]);
        if v >= 64 {
            return Err(chyba("hodnota výčtu nad 63"));
        }
        Ok(v)
    }

    pub fn u8_(&mut self) -> u32 {
        if self.bool() {
            let n = self.u(3);
            (1 << n) + self.u(n)
        } else {
            0
        }
    }

    pub fn zarovnaj(&mut self) {
        let n = (8 - (self.prec % 8) as u32) % 8;
        self.u(n);
    }

    /// Počet prečítaných bitov.
    pub fn precitane(&self) -> u64 {
        self.prec
    }

    /// Preskočí `n` bitov.
    pub fn preskoc(&mut self, mut n: u64) {
        while n > 0 {
            let k = n.min(32) as u32;
            self.u(k);
            n -= k as u64;
        }
    }

    /// Čítalo sa za koncom údajov.
    pub fn za_koncom(&self) -> bool {
        self.za_koncom > self.bitov as u64
    }

    pub fn over(&self) -> Vysledok<()> {
        if self.za_koncom() { Err(chyba("údaje sú neúplné")) } else { Ok(()) }
    }
}

/// Rozšírenie znamienka: 0, 1, 2, 3, 4 … → 0, −1, 1, −2, 2 …
#[inline]
pub(super) fn so_znamienkom(u: u32) -> i32 {
    if u & 1 == 0 { (u >> 1) as i32 } else { -(((u >> 1) + 1) as i64) as i32 }
}

/// Bity potrebné na hodnoty 0..n−1 (CeilLog2 pre n ≥ 1).
pub(super) fn ceil_log2(n: u32) -> u32 {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() }
}
