//! Prevzorkovanie výstupu SILK na 48 kHz (RFC 6716 §4.2.9 „Resampling“): prevzorkovač **nie je normatívny**,
//! normatívne je len oneskorenie (tabuľka „SILK Resampler Delay Allocations“), aby sa hybrid zarovnal s CELT.
//!
//! Vlastný návrh: celočíselné zvýšenie frekvencie (×6, ×4, ×3) polyfázovým filtrom FIR s lineárnou fázou –
//! okno Kaiser so sinc; skupinové oneskorenie (pri 48 kHz) je stred filtra. Hodnoty oneskorenia sú **zmerané** proti
//! libopus (zarovnanie výstupu SILK pri 48 kHz), medza a tvar okna zvolené meraním odstupu od libopus.

use orchidea_jadro::matematika;

/// Parametre podľa vstupnej frekvencie: (pomer, oneskorenie pri 48 kHz vo vzorkách, medza / Nyquist vstupu, beta).
fn parametre(frekvencia: u32) -> (usize, f64, f64, f64) {
    match frekvencia {
        8000 => (6, ONESKORENIE_MS[0] * 48.0, 1.0, 5.0),
        12000 => (4, ONESKORENIE_MS[1] * 48.0, 1.0, 5.0),
        _ => (3, ONESKORENIE_MS[2] * 48.0, 1.0, 5.0),
    }
}

/// Oneskorenie prevzorkovania NB, MB, WB v ms – RFC 6716 §4.2.9, tabuľka „SILK Resampler Delay Allocations“
/// (zlomok vzorky pri 48 kHz: 25,824; 33,216; 33,888). Meranie libopus pri nízkych frekvenciách: ≈ 25,9; 33,24;
/// 34,0 – rozdiel do 0,11 vzorky (2 µs); celé vzorky (26, 33, 34) dávali v nízkom pásme MB len 29 dB.
pub(crate) const ONESKORENIE_MS: [f64; 3] = [0.538, 0.692, 0.706];

fn bessel_i0(x: f64) -> f64 {
    // rad Σ ((x/2)^k / k!)^2
    let mut suma = 1.0;
    let mut clen = 1.0;
    for k in 1..50 {
        clen *= (x / 2.0) / k as f64;
        suma += clen * clen;
        if clen * clen < 1e-17 * suma {
            break;
        }
    }
    suma
}

/// Prevzorkovač jedného kanála (stav medzi rámcami).
#[derive(Clone, Debug)]
pub struct Prevzorkovac {
    frekvencia: u32,
    pomer: usize,
    /// fázy filtra: `fazy[p][q]` = L · h[p + q·L]
    fazy: Vec<Vec<f32>>,
    historia: Vec<f32>,
}

impl Prevzorkovac {
    pub fn novy(frekvencia: u32) -> Prevzorkovac {
        let (l, d, medza, beta) = parametre(frekvencia);
        // filter s lineárnou fázou okolo zlomkového stredu `d`: sinc aj okno Kaiser vystredené na d
        let n = (2.0 * d).floor() as usize + 1;
        let fc = medza * 0.5 / l as f64; // medza v násobkoch 48 kHz
        let i0b = bessel_i0(beta);
        let h: Vec<f64> = (0..n)
            .map(|k| {
                let t = k as f64 - d;
                let sinc = if t.abs() < 1e-12 {
                    2.0 * fc
                } else {
                    matematika::sin(2.0 * core::f64::consts::PI * fc * t) / (core::f64::consts::PI * t)
                };
                let r = t / d;
                let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
                sinc * w
            })
            .collect();
        // normovanie: jednosmerný zisk každej fázy ≈ 1 (súčet h = 1/L → po vynásobení L)
        let suma: f64 = h.iter().sum();
        let taps = n.div_ceil(l);
        let fazy = (0..l)
            .map(|p| {
                (0..taps).map(|q| h.get(p + q * l).map_or(0.0, |&v| (v / suma) as f32 * l as f32)).collect()
            })
            .collect();
        Prevzorkovac { frekvencia, pomer: l, fazy, historia: vec![0.0; taps - 1] }
    }

    pub fn frekvencia(&self) -> u32 {
        self.frekvencia
    }

    /// Ticho v pamäti (reset dekodéra SILK, RFC 6716 §4.2.9).
    pub fn reset(&mut self) {
        self.historia.fill(0.0);
    }

    /// Zvýši frekvenciu bloku na 48 kHz (výstup má `pomer`-krát viac vzoriek).
    pub fn spracuj(&mut self, x: &[f32]) -> Vec<f32> {
        let t = self.historia.len();
        let mut buf = Vec::with_capacity(t + x.len());
        buf.extend_from_slice(&self.historia);
        buf.extend_from_slice(x);
        let mut y = Vec::with_capacity(x.len() * self.pomer);
        for j in 0..x.len() {
            let posledny = j + t; // index x[j] v buf
            for faza in &self.fazy {
                let mut s = 0.0f32;
                for (q, &c) in faza.iter().enumerate() {
                    s += c * buf[posledny - q];
                }
                y.push(s);
            }
        }
        let k = buf.len() - t;
        self.historia.copy_from_slice(&buf[k..]);
        y
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn sinus_prejde_s_oneskorenim() {
        for (f, ix) in [(8000u32, 0usize), (12000, 1), (16000, 2)] {
            let mut p = Prevzorkovac::novy(f);
            let l = 48000 / f as usize;
            let ton = 1000.0f64;
            let x: Vec<f32> = (0..f as usize / 10)
                .map(|i| (2.0 * core::f64::consts::PI * ton * i as f64 / f as f64).sin() as f32)
                .collect();
            let y: Vec<f32> = x.chunks(160).flat_map(|c| p.spracuj(c)).collect();
            assert_eq!(y.len(), x.len() * l);
            let d = ONESKORENIE_MS[ix] * 48.0;
            let mut chyba: f64 = 0.0;
            for (m, &v) in y.iter().enumerate().skip(400) {
                let ocakavane = (2.0 * core::f64::consts::PI * ton * (m as f64 - d) / 48000.0).sin();
                chyba = chyba.max((v as f64 - ocakavane).abs());
            }
            assert!(chyba < 0.01, "{f} Hz: chyba {chyba}");
        }
    }
}
