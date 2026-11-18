//! Krivka – parameter ako funkcia osi (najčastejšie času). Kľúčové body s interpoláciou: skok, lineárna,
//! hladká (Hermitova s vlastnými alebo automatickými dotyčnicami) a monotónna (bez prekmitov, Fritsch–Carlson).

use crate::chyba::{Chyba, Vysledok};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::Meno;

/// Spôsob prechodu od kľúča k nasledujúcemu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Interpolacia {
    /// Hodnota ostáva až do ďalšieho kľúča.
    Skok,
    Linearna,
    /// Hermitova krivka; dotyčnice z kľúčov, inak automatické (Catmull–Rom).
    Hladka,
    /// Hladká bez prekmitov – zachová monotónnosť dát.
    Monotonna,
}

/// Kľúčový bod krivky.
#[derive(Debug, Clone, PartialEq)]
pub struct Kluc {
    pub poloha: f64,
    pub hodnota: f64,
    pub interpolacia: Interpolacia,
    /// Dotyčnica (smernica) v kľúči pre hladkú interpoláciu; None = automatická.
    pub dotycnica: Option<f64>,
}

impl Kluc {
    pub fn novy(poloha: f64, hodnota: f64, interpolacia: Interpolacia) -> Kluc {
        Kluc { poloha, hodnota, interpolacia, dotycnica: None }
    }
}

/// Čo krivka vracia mimo rozsahu kľúčov.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mimo {
    /// Drží prvú alebo poslednú hodnotu.
    Drz,
    /// Opakuje krivku dokola.
    Opakuj,
}

/// Krivka nad pomenovanou osou.
#[derive(Debug, Clone, PartialEq)]
pub struct Krivka {
    pub os: Meno,
    kluce: Vec<Kluc>,
    pub mimo: Mimo,
}

impl Krivka {
    /// Nová krivka; kľúče sa zoradia podľa polohy a nesmú mať rovnakú polohu.
    pub fn nova(os: Meno, mut kluce: Vec<Kluc>, mimo: Mimo) -> Vysledok<Krivka> {
        if kluce.is_empty() {
            return Err(Chyba::Chybajuce("krivka potrebuje aspoň jeden kľúč".into()));
        }
        if kluce.iter().any(|k| !k.poloha.is_finite() || !k.hodnota.is_finite()) {
            return Err(Chyba::MimoRozsahu("kľúč krivky musí mať konečnú polohu aj hodnotu".into()));
        }
        kluce.sort_by(|a, b| a.poloha.total_cmp(&b.poloha));
        if kluce.windows(2).any(|w| w[0].poloha == w[1].poloha) {
            return Err(Chyba::MimoRozsahu("dva kľúče krivky majú rovnakú polohu".into()));
        }
        Ok(Krivka { os, kluce, mimo })
    }

    pub fn kluce(&self) -> &[Kluc] {
        &self.kluce
    }

    fn smernica(&self, i: usize) -> f64 {
        let k = &self.kluce;
        if let Some(d) = k[i].dotycnica {
            return d;
        }
        let n = k.len();
        if n < 2 {
            return 0.0;
        }
        let s = |a: usize, b: usize| (k[b].hodnota - k[a].hodnota) / (k[b].poloha - k[a].poloha);
        if i == 0 {
            s(0, 1)
        } else if i == n - 1 {
            s(n - 2, n - 1)
        } else {
            s(i - 1, i + 1)
        }
    }

    /// Fritsch–Carlsonove dotyčnice pre monotónnu interpoláciu medzi kľúčmi i a i+1.
    fn monotonne_smernice(&self, i: usize) -> (f64, f64) {
        let k = &self.kluce;
        let n = k.len();
        let d = |a: usize| (k[a + 1].hodnota - k[a].hodnota) / (k[a + 1].poloha - k[a].poloha);
        let dotyk = |j: usize| -> f64 {
            if j == 0 {
                return d(0);
            }
            if j == n - 1 {
                return d(n - 2);
            }
            let (a, b) = (d(j - 1), d(j));
            if a * b <= 0.0 {
                0.0
            } else {
                // vážený harmonický priemer zaručí monotónnosť
                let (h0, h1) = (k[j].poloha - k[j - 1].poloha, k[j + 1].poloha - k[j].poloha);
                let (w1, w2) = (2.0 * h1 + h0, h1 + 2.0 * h0);
                (w1 + w2) / (w1 / a + w2 / b)
            }
        };
        let (mut m0, mut m1) = (dotyk(i), dotyk(i + 1));
        let di = d(i);
        if di == 0.0 {
            return (0.0, 0.0);
        }
        let (a, b) = (m0 / di, m1 / di);
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m0 = t * a * di;
            m1 = t * b * di;
        }
        (m0, m1)
    }

    /// Hodnota krivky v polohe.
    pub fn hodnota(&self, poloha: f64) -> f64 {
        let k = &self.kluce;
        let (prvy, posledny) = (k[0].poloha, k[k.len() - 1].poloha);
        let mut p = poloha;
        if k.len() == 1 || !p.is_finite() {
            return if p.is_nan() { f64::NAN } else { k[0].hodnota };
        }
        match self.mimo {
            Mimo::Drz => {
                if p <= prvy {
                    return k[0].hodnota;
                }
                if p >= posledny {
                    return k[k.len() - 1].hodnota;
                }
            }
            Mimo::Opakuj => {
                p = prvy + (p - prvy).rem_euclid(posledny - prvy);
            }
        }
        let i = match k.binary_search_by(|x| x.poloha.total_cmp(&p)) {
            Ok(i) => return k[i].hodnota,
            Err(i) => i - 1,
        };
        let (a, b) = (&k[i], &k[i + 1]);
        let h = b.poloha - a.poloha;
        let t = (p - a.poloha) / h;
        match a.interpolacia {
            Interpolacia::Skok => a.hodnota,
            Interpolacia::Linearna => a.hodnota + (b.hodnota - a.hodnota) * t,
            Interpolacia::Hladka | Interpolacia::Monotonna => {
                let (m0, m1) = if a.interpolacia == Interpolacia::Monotonna {
                    self.monotonne_smernice(i)
                } else {
                    (self.smernica(i), self.smernica(i + 1))
                };
                let t2 = t * t;
                let t3 = t2 * t;
                let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
                let h10 = t3 - 2.0 * t2 + t;
                let h01 = -2.0 * t3 + 3.0 * t2;
                let h11 = t3 - t2;
                h00 * a.hodnota + h10 * h * m0 + h01 * b.hodnota + h11 * h * m1
            }
        }
    }
}

impl Kodovatelne for Krivka {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.meno(&self.os).u8(match self.mimo {
            Mimo::Drz => 0,
            Mimo::Opakuj => 1,
        });
        z.cislo(self.kluce.len() as u64);
        for k in &self.kluce {
            z.f64(k.poloha).f64(k.hodnota).u8(k.interpolacia as u8);
            match k.dotycnica {
                Some(d) => z.u8(1).f64(d),
                None => z.u8(0),
            };
        }
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        let os = c.meno()?;
        let mimo = match c.u8()? {
            0 => Mimo::Drz,
            1 => Mimo::Opakuj,
            _ => return Err(Chyba::PoskodeneData("režim krivky".into())),
        };
        let n = c.pocet(18)?;
        let mut kluce = Vec::with_capacity(n);
        for _ in 0..n {
            let poloha = c.f64()?;
            let hodnota = c.f64()?;
            let interpolacia = match c.u8()? {
                0 => Interpolacia::Skok,
                1 => Interpolacia::Linearna,
                2 => Interpolacia::Hladka,
                3 => Interpolacia::Monotonna,
                _ => return Err(Chyba::PoskodeneData("interpolácia".into())),
            };
            let dotycnica = match c.u8()? {
                0 => None,
                1 => Some(c.f64()?),
                _ => return Err(Chyba::PoskodeneData("dotyčnica".into())),
            };
            kluce.push(Kluc { poloha, hodnota, interpolacia, dotycnica });
        }
        Krivka::nova(os, kluce, mimo).map_err(|e| Chyba::PoskodeneData(e.to_string()))
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::meno::m;

    fn krivka(i: Interpolacia) -> Krivka {
        let k = vec![
            Kluc::novy(0.0, 0.0, i),
            Kluc::novy(1.0, 1.0, i),
            Kluc::novy(2.0, 1.0, i),
            Kluc::novy(3.0, 4.0, i),
        ];
        Krivka::nova(m("t"), k, Mimo::Drz).unwrap()
    }

    #[test]
    fn interpolacie() {
        assert_eq!(krivka(Interpolacia::Skok).hodnota(0.7), 0.0);
        assert_eq!(krivka(Interpolacia::Linearna).hodnota(2.5), 2.5);
        assert_eq!(krivka(Interpolacia::Linearna).hodnota(-1.0), 0.0);
        assert_eq!(krivka(Interpolacia::Linearna).hodnota(9.0), 4.0);
        let h = krivka(Interpolacia::Hladka);
        assert_eq!(h.hodnota(1.0), 1.0);
        // Hladká krivka na rovnom úseku prekmitne, monotónna nie.
        let prekmit = (0..100).map(|i| h.hodnota(1.0 + i as f64 / 100.0)).fold(f64::INFINITY, f64::min);
        assert!(prekmit < 1.0);
        let mono = krivka(Interpolacia::Monotonna);
        for i in 0..=300 {
            let p = i as f64 / 100.0;
            let a = mono.hodnota(p);
            let b = mono.hodnota(p + 0.01);
            assert!(b >= a - 1e-12, "monotónnosť v {p}");
        }
        let opak = Krivka::nova(
            m("t"),
            vec![Kluc::novy(0.0, 0.0, Interpolacia::Linearna), Kluc::novy(1.0, 1.0, Interpolacia::Linearna)],
            Mimo::Opakuj,
        )
        .unwrap();
        assert!((opak.hodnota(2.25) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn overenie_a_kodovanie() {
        assert!(Krivka::nova(m("t"), vec![], Mimo::Drz).is_err());
        let dve = vec![Kluc::novy(1.0, 0.0, Interpolacia::Skok), Kluc::novy(1.0, 1.0, Interpolacia::Skok)];
        assert!(Krivka::nova(m("t"), dve, Mimo::Drz).is_err());
        let mut k = krivka(Interpolacia::Hladka);
        k.kluce[1].dotycnica = Some(0.5);
        assert_eq!(Krivka::z_bajtov(&k.do_bajtov()).unwrap(), k);
    }
}
