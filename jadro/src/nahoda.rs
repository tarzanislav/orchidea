//! Opakovateľná náhoda – rovnaké semienko dá rovnakú postupnosť na každej platforme (zásada A9).
//!
//! Generátor xoshiro256** so štartom cez SplitMix64. Rozdelenia používajú len deterministickú matematiku.

use crate::matematika;

/// SplitMix64 – rozšírenie jedného semienka na stav generátora.
fn splitmix64(stav: &mut u64) -> u64 {
    *stav = stav.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *stav;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Generátor náhodných čísel xoshiro256**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nahoda {
    s: [u64; 4],
}

impl Nahoda {
    /// Generátor zo semienka.
    pub fn zo_semienka(semienko: u64) -> Self {
        let mut st = semienko;
        let s = [splitmix64(&mut st), splitmix64(&mut st), splitmix64(&mut st), splitmix64(&mut st)];
        Nahoda { s }
    }

    /// Ďalšie 64-bitové číslo.
    pub fn dalsie(&mut self) -> u64 {
        let vysledok = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        vysledok
    }

    /// Nezávislý podgenerátor (napr. pre jednu vrstvu modelu alebo jedno vlákno).
    pub fn rozdel(&mut self) -> Nahoda {
        Nahoda::zo_semienka(self.dalsie())
    }

    /// Rovnomerne v [0, 1) s 53 bitmi presnosti.
    pub fn f64(&mut self) -> f64 {
        (self.dalsie() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Rovnomerne v [0, 1) s 24 bitmi presnosti.
    pub fn f32(&mut self) -> f32 {
        (self.dalsie() >> 40) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// Rovnomerne v [od, po).
    pub fn v_rozsahu(&mut self, od: f64, po: f64) -> f64 {
        od + (po - od) * self.f64()
    }

    /// Nestranné celé číslo v [0, n) (Lemireho metóda).
    pub fn mensie_ako(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        loop {
            let x = self.dalsie();
            let m = (x as u128) * (n as u128);
            let nizke = m as u64;
            if nizke >= n || nizke >= n.wrapping_neg() % n {
                return (m >> 64) as u64;
            }
        }
    }

    /// Normálne rozdelenie N(0, 1) (Boxova–Mullerova metóda, deterministická).
    pub fn normalne(&mut self) -> f64 {
        let u1 = 1.0 - self.f64(); // (0, 1]
        let u2 = self.f64();
        matematika::odmocnina(-2.0 * matematika::ln(u1)) * matematika::cos(2.0 * std::f64::consts::PI * u2)
    }

    /// Náhodné poradie prvkov (Fisher–Yates).
    pub fn zamiesaj<T>(&mut self, prvky: &mut [T]) {
        for i in (1..prvky.len()).rev() {
            let j = self.mensie_ako(i as u64 + 1) as usize;
            prvky.swap(i, j);
        }
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn opakovatelnost() {
        let mut a = Nahoda::zo_semienka(42);
        let mut b = Nahoda::zo_semienka(42);
        for _ in 0..1000 {
            assert_eq!(a.dalsie(), b.dalsie());
        }
    }

    #[test]
    fn rozdelenia() {
        let mut n = Nahoda::zo_semienka(7);
        let mut sucet = 0.0;
        let mut stvorce = 0.0;
        let pocet = 200_000;
        for _ in 0..pocet {
            let x = n.normalne();
            sucet += x;
            stvorce += x * x;
        }
        let priemer = sucet / pocet as f64;
        let rozptyl = stvorce / pocet as f64 - priemer * priemer;
        assert!(priemer.abs() < 0.01, "priemer {priemer}");
        assert!((rozptyl - 1.0).abs() < 0.02, "rozptyl {rozptyl}");
        let mut pocty = [0u32; 10];
        for _ in 0..100_000 {
            pocty[n.mensie_ako(10) as usize] += 1;
        }
        assert!(pocty.iter().all(|&p| (9_500..10_500).contains(&p)), "{pocty:?}");
    }
}
