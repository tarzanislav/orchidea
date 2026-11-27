//! Spätná MDCT a okno s malým prekrytím pre CELT (RFC 6716 §4.3.7) – vlastná FFT so zmiešaným základom.
//!
//! Matematika (odvodená z definície, nie z referenčného kódu): výstup bloku s `n` koeficientmi
//! `y[t] = Σ_k X[k] · cos(π/n · (t + ½ + n/2) · (k + ½))`, `t = 0 … 2n − 1`. Ide o DCT-IV
//! `u[m] = Σ_k X[k] · cos(π/n · (m + ½)(k + ½))` vyčíslenú v `m = t + n/2` s rozšírením
//! `u[m] = −u[2n − 1 − m]` (n ≤ m < 2n) a `u[m] = −u[m − 2n]` (2n ≤ m < 3n). DCT-IV dĺžky `n` sa počíta komplexnou
//! FFT dĺžky `n/2`: `z_j = (X[2j] + i·X[n−1−2j]) · e^(−iπj/n)`, `S_p = FFT(z)_p · e^(−iπ(p + ¼)/n)`,
//! `u[2p] = Re S_p`, `u[n−1−2p] = −Im S_p`.
//!
//! Okno (RFC 6716 §4.3.7): `W(i) = sin(π/2 · sin²(π/2 · (i + ½)/L))`, `L = 120` (stúpajúca polovica), vložené
//! do dĺžky `2n` s nulami a jednotkami (malé prekrytie, výkonová komplementárnosť).

use orchidea_jadro::matematika;

/// Dĺžka prekrytia CELT pri 48 kHz (2,5 ms).
pub const PREKRYTIE: usize = 120;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Komplex {
    pub(crate) re: f32,
    pub(crate) im: f32,
}

impl Komplex {
    pub(crate) fn nasob(self, o: Komplex) -> Komplex {
        Komplex { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
    pub(crate) fn plus(self, o: Komplex) -> Komplex {
        Komplex { re: self.re + o.re, im: self.im + o.im }
    }
}

fn jednotkovy(uhol: f64) -> Komplex {
    Komplex { re: matematika::cos(uhol) as f32, im: matematika::sin(uhol) as f32 }
}

/// Plán FFT dĺžky `n` (n = 2^a · 3^b · 5^c): rozklad na základy a koreňe jednotky `e^(−2πik/n)`.
#[derive(Clone, Debug)]
pub(crate) struct PlanFft {
    n: usize,
    zaklady: Vec<usize>,
    korene: Vec<Komplex>,
}

impl PlanFft {
    pub(crate) fn novy(n: usize) -> PlanFft {
        let mut zaklady = Vec::new();
        let mut m = n;
        for r in [4, 2, 3, 5] {
            while m.is_multiple_of(r) {
                zaklady.push(r);
                m /= r;
            }
        }
        assert_eq!(m, 1, "FFT: dĺžka {n} nie je 2^a·3^b·5^c");
        let korene = (0..n).map(|k| jednotkovy(-2.0 * core::f64::consts::PI * k as f64 / n as f64)).collect();
        PlanFft { n, zaklady, korene }
    }

    /// Dopredná FFT (decimácia v čase, rekurzívne): `von[k] = Σ_j vstup[j·krok] · e^(−2πijk/m)`.
    pub(crate) fn fft(&self, vstup: &[Komplex], krok: usize, von: &mut [Komplex], uroven: usize) {
        let m = von.len();
        if m == 1 {
            von[0] = vstup[0];
            return;
        }
        let r = self.zaklady[uroven];
        let d = m / r;
        for q in 0..r {
            self.fft(&vstup[q * krok..], krok * r, &mut von[q * d..(q + 1) * d], uroven + 1);
        }
        let mierka = self.n / m; // index koreňa celej dĺžky
        let mut a = [Komplex::default(); 5];
        for k in 0..d {
            for (q, aq) in a.iter_mut().enumerate().take(r) {
                // otočenie (twiddle) e^(−2πi·q·k/m) pred motýľom
                *aq = von[q * d + k].nasob(self.korene[(q * k * mierka) % self.n]);
            }
            for s in 0..r {
                let mut suma = a[0];
                for (q, aq) in a.iter().enumerate().take(r).skip(1) {
                    suma = suma.plus(aq.nasob(self.korene[(q * s * d * mierka) % self.n]));
                }
                von[k + s * d] = suma;
            }
        }
    }
}

/// Spätná MDCT jednej dĺžky bloku `n` (počet koeficientov).
#[derive(Clone, Debug)]
pub struct SpatnaMdct {
    n: usize,
    fft: PlanFft,
    pred: Vec<Komplex>,
    po: Vec<Komplex>,
}

impl SpatnaMdct {
    pub fn nova(n: usize) -> SpatnaMdct {
        assert!(n.is_multiple_of(4));
        let pi = core::f64::consts::PI;
        let pred = (0..n / 2).map(|j| jednotkovy(-pi * j as f64 / n as f64)).collect();
        let po = (0..n / 2).map(|p| jednotkovy(-pi * (p as f64 + 0.25) / n as f64)).collect();
        SpatnaMdct { n, fft: PlanFft::novy(n / 2), pred, po }
    }

    /// DCT-IV: `u[m] = Σ_k x[k·krok] · cos(π/n (m + ½)(k + ½))` – vstup s krokom (prekladané krátke bloky).
    fn dct4(&self, x: &[f32], krok: usize, u: &mut [f32]) {
        let n = self.n;
        let z: Vec<Komplex> = (0..n / 2)
            .map(|j| Komplex { re: x[2 * j * krok], im: x[(n - 1 - 2 * j) * krok] }.nasob(self.pred[j]))
            .collect();
        let mut w = vec![Komplex::default(); n / 2];
        self.fft.fft(&z, 1, &mut w, 0);
        for p in 0..n / 2 {
            let s = w[p].nasob(self.po[p]);
            u[2 * p] = s.re;
            u[n - 1 - 2 * p] = -s.im;
        }
    }

    /// Spätná MDCT bloku (koeficienty `x[k·krok]`) s oknom s malým prekrytím; výsledok sa **pripočíta** do
    /// `von` od pozície `zaciatok` – `n + PREKRYTIE` vzoriek (nenulová časť okna). `mierka` násobí výstup.
    pub fn pripocitaj(&self, x: &[f32], krok: usize, okno: &[f32], mierka: f32, von: &mut [f32]) {
        let n = self.n;
        let ov = PREKRYTIE;
        let mut u = vec![0.0f32; n];
        self.dct4(x, krok, &mut u);
        let hodnota = |m: usize| -> f32 {
            if m < n {
                u[m]
            } else if m < 2 * n {
                -u[2 * n - 1 - m]
            } else {
                -u[m - 2 * n]
            }
        };
        let t0 = (n - ov) / 2;
        for s in 0..n + ov {
            let t = t0 + s;
            let w = if s < ov {
                okno[s]
            } else if s >= n {
                okno[n + ov - 1 - s]
            } else {
                1.0
            };
            von[s] += mierka * w * hodnota(t + n / 2);
        }
    }
}

/// Stúpajúca polovica okna (RFC 6716 §4.3.7).
pub fn okno_celt() -> Vec<f32> {
    let pi = core::f64::consts::PI;
    (0..PREKRYTIE)
        .map(|i| {
            let s = matematika::sin(0.5 * pi * (i as f64 + 0.5) / PREKRYTIE as f64);
            matematika::sin(0.5 * pi * s * s) as f32
        })
        .collect()
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn priama(x: &[f32], n: usize) -> Vec<f64> {
        (0..2 * n)
            .map(|t| {
                (0..n)
                    .map(|k| {
                        x[k] as f64
                            * (core::f64::consts::PI / n as f64
                                * (t as f64 + 0.5 + n as f64 / 2.0)
                                * (k as f64 + 0.5))
                                .cos()
                    })
                    .sum()
            })
            .collect()
    }

    #[test]
    fn rychla_rovna_sa_priamej() {
        for n in [120usize, 240, 480, 960] {
            let x: Vec<f32> = (0..n).map(|k| ((k * 7919 % 101) as f32 - 50.0) / 50.0).collect();
            let m = SpatnaMdct::nova(n);
            let jednotky = vec![1.0f32; PREKRYTIE];
            let mut von = vec![0.0f32; n + PREKRYTIE];
            m.pripocitaj(&x, 1, &jednotky, 1.0, &mut von);
            let p = priama(&x, n);
            let t0 = (n - PREKRYTIE) / 2;
            let mut chyba: f64 = 0.0;
            for s in 0..n + PREKRYTIE {
                chyba = chyba.max((von[s] as f64 - p[t0 + s]).abs());
            }
            assert!(chyba < 2e-3 * (n as f64).sqrt(), "n={n} chyba {chyba}");
        }
    }

    /// Dopredná MDCT s rovnakým oknom (len v skúške) + spätná so sčítaním prekrytia = pôvodný signál.
    #[test]
    fn dokonala_rekonstrukcia() {
        let okno = okno_celt();
        for n in [120usize, 960] {
            let blokov = 6;
            let sig: Vec<f64> =
                (0..n * (blokov + 2)).map(|i| ((i * 31337 % 977) as f64 / 977.0) - 0.5).collect();
            let ov = PREKRYTIE;
            let t0 = (n - ov) / 2;
            let w = |s: usize| -> f64 {
                if s < ov {
                    okno[s] as f64
                } else if s >= n {
                    okno[n + ov - 1 - s] as f64
                } else {
                    1.0
                }
            };
            let m = SpatnaMdct::nova(n);
            let mut von = vec![0.0f32; n * (blokov + 2)];
            for b in 0..blokov {
                // dopredná: X[k] = Σ_s w(s)·x(b·n + s)·cos(π/n (t0 + s + ½ + n/2)(k + ½))
                let x: Vec<f32> = (0..n)
                    .map(|k| {
                        (0..n + ov)
                            .map(|s| {
                                w(s) * sig[b * n + s]
                                    * (core::f64::consts::PI / n as f64
                                        * ((t0 + s) as f64 + 0.5 + n as f64 / 2.0)
                                        * (k as f64 + 0.5))
                                        .cos()
                            })
                            .sum::<f64>() as f32
                    })
                    .collect();
                m.pripocitaj(&x, 1, &okno, 2.0 / n as f32, &mut von[b * n..]);
            }
            // vnútorné bloky (úplne prekryté) sa zrekonštruujú
            for i in n + ov..(blokov - 1) * n {
                assert!((von[i] as f64 - sig[i]).abs() < 1e-3, "n={n} i={i}: {} vs {}", von[i], sig[i]);
            }
        }
    }
}
