//! Kvantizácia – 8-bitové aktivácie a trojhodnotové váhy (−1, 0, +1) podľa myšlienky BitNet b1.58.
//!
//! Trojhodnotová váha nepotrebuje násobenie: výstup je súčet vstupov s váhou +1 mínus súčet vstupov
//! s váhou −1. Váhy sa ukladajú po 2 bitoch (4 v bajte, rýchle rozbalenie) alebo kompaktne v trojkovej
//! sústave (5 váh v bajte = 1,6 bitu na váhu) pre súbory.

use crate::jadra::{self, Rezim};
use orchidea_jadro::paralel;

/// Kvantizované aktivácie: hodnoty i8 po riadkoch a mierka každého riadku (x ≈ q / mierka).
#[derive(Debug, Clone, PartialEq)]
pub struct AktivacieI8 {
    pub riadky: usize,
    pub stlpce: usize,
    pub hodnoty: Vec<i8>,
    pub mierky: Vec<f32>,
}

/// Kvantizácia po riadkoch podľa najväčšej absolútnej hodnoty (rozsah −127…127).
pub fn kvantizuj_aktivacie(riadky: usize, stlpce: usize, x: &[f32]) -> AktivacieI8 {
    assert_eq!(x.len(), riadky * stlpce);
    let mut hodnoty = vec![0i8; x.len()];
    let mut mierky = vec![1.0f32; riadky];
    for r in 0..riadky {
        let xr = &x[r * stlpce..(r + 1) * stlpce];
        let max = xr.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let s = if max > 0.0 { 127.0 / max } else { 1.0 };
        mierky[r] = s;
        for (q, &v) in hodnoty[r * stlpce..(r + 1) * stlpce].iter_mut().zip(xr) {
            *q = (v * s).round().clamp(-127.0, 127.0) as i8;
        }
    }
    AktivacieI8 { riadky, stlpce, hodnoty, mierky }
}

/// Trojhodnotové váhy: riadok = jeden výstup, stĺpec = jeden vstup; w ≈ t · mierka.
#[derive(Debug, Clone, PartialEq)]
pub struct TrojhodnotoveVahy {
    pub vystupy: usize,
    pub vstupy: usize,
    /// 2 bity na váhu, 4 váhy v bajte; každý riadok začína na novom bajte.
    pub zbalene: Vec<u8>,
    pub mierka: f32,
}

const KOD_NULA: u8 = 0b00;
const KOD_PLUS: u8 = 0b01;
const KOD_MINUS: u8 = 0b10;

fn bajtov_na_riadok(vstupy: usize) -> usize {
    vstupy.div_ceil(4)
}

/// Tabuľka rozbalenia jedného bajtu na 4 váhy.
fn tabulka() -> &'static [[i8; 4]; 256] {
    static T: std::sync::OnceLock<[[i8; 4]; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [[0i8; 4]; 256];
        for (b, w) in t.iter_mut().enumerate() {
            for (i, x) in w.iter_mut().enumerate() {
                *x = match (b >> (2 * i)) & 0b11 {
                    0b01 => 1,
                    0b10 => -1,
                    _ => 0,
                };
            }
        }
        t
    })
}

impl TrojhodnotoveVahy {
    /// Kvantizácia podľa priemernej absolútnej hodnoty (absmean): t = obmedz(zaokrúhli(w / γ), −1, 1).
    pub fn z_f32(vystupy: usize, vstupy: usize, w: &[f32]) -> TrojhodnotoveVahy {
        assert_eq!(w.len(), vystupy * vstupy);
        let sucet: f64 = w.iter().map(|v| v.abs() as f64).sum();
        let gama = (sucet / w.len().max(1) as f64) as f32;
        let gama = gama.max(1e-8);
        let t: Vec<i8> = w.iter().map(|&v| (v / gama).round().clamp(-1.0, 1.0) as i8).collect();
        Self::z_trojic(vystupy, vstupy, &t, gama)
    }

    /// Zo zoznamu hodnôt −1, 0, +1.
    pub fn z_trojic(vystupy: usize, vstupy: usize, t: &[i8], mierka: f32) -> TrojhodnotoveVahy {
        assert_eq!(t.len(), vystupy * vstupy);
        let br = bajtov_na_riadok(vstupy);
        let mut zbalene = vec![0u8; vystupy * br];
        for r in 0..vystupy {
            for c in 0..vstupy {
                let kod = match t[r * vstupy + c] {
                    1 => KOD_PLUS,
                    -1 => KOD_MINUS,
                    0 => KOD_NULA,
                    x => panic!("trojhodnotová váha {x} nie je −1, 0 ani +1"),
                };
                zbalene[r * br + c / 4] |= kod << (2 * (c % 4));
            }
        }
        TrojhodnotoveVahy { vystupy, vstupy, zbalene, mierka }
    }

    /// Rozbalí riadok do i8 (−1, 0, +1).
    pub fn rozbal_riadok(&self, r: usize, ciel: &mut [i8]) {
        let br = bajtov_na_riadok(self.vstupy);
        let t = tabulka();
        let riadok = &self.zbalene[r * br..(r + 1) * br];
        for (i, &b) in riadok.iter().enumerate() {
            let w = &t[b as usize];
            let od = i * 4;
            let kolko = (self.vstupy - od).min(4);
            ciel[od..od + kolko].copy_from_slice(&w[..kolko]);
        }
    }

    /// Všetky váhy ako i8.
    pub fn trojice(&self) -> Vec<i8> {
        let mut t = vec![0i8; self.vystupy * self.vstupy];
        for r in 0..self.vystupy {
            self.rozbal_riadok(r, &mut t[r * self.vstupy..(r + 1) * self.vstupy]);
        }
        t
    }

    /// Váhy po dekvantizácii (t · mierka) – pre spätné šírenie.
    pub fn ako_f32(&self) -> Vec<f32> {
        self.trojice().into_iter().map(|t| t as f32 * self.mierka).collect()
    }

    /// Kompaktný zápis: 5 váh v bajte (3⁵ = 243 ≤ 256).
    pub fn kompaktne(&self) -> Vec<u8> {
        let t = self.trojice();
        t.chunks(5)
            .map(|k| {
                let mut b = 0u8;
                for &x in k.iter().rev() {
                    b = b * 3 + (x + 1) as u8;
                }
                b
            })
            .collect()
    }

    /// Načíta kompaktný zápis.
    pub fn z_kompaktneho(
        vystupy: usize,
        vstupy: usize,
        data: &[u8],
        mierka: f32,
    ) -> Option<TrojhodnotoveVahy> {
        let n = vystupy.checked_mul(vstupy)?;
        if data.len() != n.div_ceil(5) {
            return None;
        }
        let mut t = Vec::with_capacity(n);
        for &b in data {
            if b >= 243 {
                return None;
            }
            let mut x = b;
            for _ in 0..5 {
                if t.len() < n {
                    t.push((x % 3) as i8 - 1);
                }
                x /= 3;
            }
        }
        Some(Self::z_trojic(vystupy, vstupy, &t, mierka))
    }

    /// Y (m×výstupy) = X · Wᵀ pre kvantizované aktivácie: celočíselný súčet, potom jedna dekvantizácia.
    /// Výsledok je v oboch režimoch bitovo rovnaký.
    pub fn nasob(&self, rezim: Rezim, x: &AktivacieI8) -> Vec<f32> {
        assert_eq!(x.stlpce, self.vstupy);
        let (m, n, k) = (x.riadky, self.vystupy, self.vstupy);
        let mut y = vec![0f32; m * n];
        if n == 0 || m == 0 {
            return y;
        }
        // Rozbalené váhy sa zdieľajú medzi riadkami vstupu (rozbaľujú sa raz).
        let w = self.trojice();
        let mut acc = vec![0i32; m * n];
        jadra::nasob_i8(rezim, m, n, k, &x.hodnoty, &w, &mut acc);
        paralel::pre_kusy(&mut y, n.max(1), |r, yr| {
            let deq = self.mierka / x.mierky[r];
            for (v, &a) in yr.iter_mut().zip(&acc[r * n..(r + 1) * n]) {
                *v = a as f32 * deq;
            }
        });
        y
    }

    /// Veľkosť zbalených váh v bajtoch.
    pub fn bajtov(&self) -> usize {
        self.zbalene.len()
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    #[test]
    fn zbalenie_a_kompaktny_zapis() {
        let mut n = Nahoda::zo_semienka(1);
        let t: Vec<i8> = (0..7 * 13).map(|_| n.mensie_ako(3) as i8 - 1).collect();
        let w = TrojhodnotoveVahy::z_trojic(7, 13, &t, 0.5);
        assert_eq!(w.trojice(), t);
        let k = w.kompaktne();
        assert_eq!(k.len(), (7 * 13usize).div_ceil(5));
        assert_eq!(TrojhodnotoveVahy::z_kompaktneho(7, 13, &k, 0.5).unwrap(), w);
        assert!(TrojhodnotoveVahy::z_kompaktneho(7, 13, &[255; 19], 0.5).is_none());
    }

    #[test]
    fn absmean_kvantizacia() {
        let w = [0.9f32, -0.05, 0.4, -1.2, 0.0, 0.6];
        let q = TrojhodnotoveVahy::z_f32(2, 3, &w);
        // γ = priemer |w| = 0.525
        assert!((q.mierka - 0.525).abs() < 1e-6);
        assert_eq!(q.trojice(), vec![1, 0, 1, -1, 0, 1]);
    }

    #[test]
    fn trojhodnotove_nasobenie_je_presne_a_rovnake_v_oboch_rezimoch() {
        let mut n = Nahoda::zo_semienka(5);
        let (m, vstupy, vystupy) = (9, 131, 17);
        let x: Vec<f32> = (0..m * vstupy).map(|_| n.f32() * 2.0 - 1.0).collect();
        let w: Vec<f32> = (0..vystupy * vstupy).map(|_| n.f32() * 2.0 - 1.0).collect();
        let q = TrojhodnotoveVahy::z_f32(vystupy, vstupy, &w);
        let a = kvantizuj_aktivacie(m, vstupy, &x);
        let presne = q.nasob(Rezim::Presny, &a);
        let rychlo = q.nasob(Rezim::Rychly, &a);
        assert_eq!(presne, rychlo);
        // porovnanie s výpočtom v plávajúcej čiarke nad dekvantizovanými hodnotami
        let wd = q.ako_f32();
        for r in 0..m {
            for j in 0..vystupy {
                let s: f64 = (0..vstupy)
                    .map(|c| {
                        (a.hodnoty[r * vstupy + c] as f64 / a.mierky[r] as f64) * wd[j * vstupy + c] as f64
                    })
                    .sum();
                assert!(
                    (s - presne[r * vystupy + j] as f64).abs() < 1e-4,
                    "{s} vs {}",
                    presne[r * vystupy + j]
                );
            }
        }
    }
}
