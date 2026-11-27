//! Spätná transformácia AC-3 (A/52:2018 §7.9.4): IMDCT 512 a dve IMDCT 256 (prepnutie bloku), okno (tabuľka 7.33),
//! prekrytie a sčítanie.
//!
//! Postup presne podľa textu: krok 2 komplexné násobenie pred IFFT (`xcos1`, `xsin1`, pri krátkych blokoch `xcos2`,
//! `xsin2`), krok 3 komplexná IFFT dĺžky N/4 (N/8) **bez delenia dĺžkou** (`z[n] = Σ Z[k]·e^(+j2πkn/M)` – počíta
//! sa ako `conj(FFT(conj(Z)))` vlastnou FFT z `opus::mdct`), krok 4 násobenie po IFFT, krok 5 okno a preusporiadanie,
//! krok 6 `pcm[n] = 2·(x[n] + oneskorenie[n])`. Výstup je v plávajúcej čiarke bez orezania (saturácia z textu
//! patrí výstupu v pevnej čiarke).

use super::tabulky::OKNO;
use crate::opus::mdct::{Komplex, PlanFft};
use orchidea_jadro::matematika;

/// N z textu – dĺžka okna.
const N: usize = 512;

#[derive(Clone, Debug)]
pub(super) struct Imdct {
    fft128: PlanFft,
    fft64: PlanFft,
    /// `xcos1[k] + j·xsin1[k]` (k < N/4).
    otocenie1: Vec<Komplex>,
    /// `xcos2[k] + j·xsin2[k]` (k < N/8).
    otocenie2: Vec<Komplex>,
}

fn otocenia(pocet: usize, menovatel: f64) -> Vec<Komplex> {
    (0..pocet)
        .map(|k| {
            let uhol = 2.0 * core::f64::consts::PI * (8 * k + 1) as f64 / menovatel;
            Komplex { re: -matematika::cos(uhol) as f32, im: -matematika::sin(uhol) as f32 }
        })
        .collect()
}

impl Imdct {
    pub(super) fn nova() -> Imdct {
        Imdct {
            fft128: PlanFft::novy(N / 4),
            fft64: PlanFft::novy(N / 8),
            otocenie1: otocenia(N / 4, 8.0 * N as f64),
            otocenie2: otocenia(N / 8, 4.0 * N as f64),
        }
    }

    /// Komplexná IFFT bez normovania: `conj(FFT(conj(Z)))`.
    fn ifft(plan: &PlanFft, z: &mut [Komplex], pomocne: &mut [Komplex]) {
        for c in z.iter_mut() {
            c.im = -c.im;
        }
        plan.fft(z, 1, pomocne, 0);
        for (c, p) in z.iter_mut().zip(pomocne.iter()) {
            *c = Komplex { re: p.re, im: -p.im };
        }
    }

    /// Krok 2–4 pre jednu transformáciu dĺžky `m` = N/4 alebo N/8: zo vstupu `x` (m·2 koeficientov) vráti `y`.
    fn jadro(&self, x: &[f32], dlhy: bool, y: &mut [Komplex]) {
        let m = y.len();
        let otocenie = if dlhy { &self.otocenie1 } else { &self.otocenie2 };
        let mut z = [Komplex::default(); N / 4];
        let mut pom = [Komplex::default(); N / 4];
        for k in 0..m {
            // Z[k] = (X[2m − 2k − 1] + j·X[2k]) · (xcos[k] + j·xsin[k])
            z[k] = Komplex { re: x[2 * m - 2 * k - 1], im: x[2 * k] }.nasob(otocenie[k]);
        }
        Self::ifft(if dlhy { &self.fft128 } else { &self.fft64 }, &mut z[..m], &mut pom[..m]);
        for n in 0..m {
            y[n] = z[n].nasob(otocenie[n]);
        }
    }

    /// Jeden blok kanála: 256 koeficientov → 256 vzoriek PCM (prekrytie s `oneskorenie`, ktoré sa obnoví).
    pub(super) fn blok(
        &self,
        koef: &[f32; 256],
        kratke: bool,
        oneskorenie: &mut [f32; 256],
        pcm: &mut [f32],
    ) {
        let w = &OKNO;
        let mut x = [0f32; N];
        if !kratke {
            // §7.9.4.1
            let mut y = [Komplex::default(); N / 4];
            self.jadro(koef, true, &mut y);
            for n in 0..N / 8 {
                x[2 * n] = -y[N / 8 + n].im * w[2 * n];
                x[2 * n + 1] = y[N / 8 - n - 1].re * w[2 * n + 1];
                x[N / 4 + 2 * n] = -y[n].re * w[N / 4 + 2 * n];
                x[N / 4 + 2 * n + 1] = y[N / 4 - n - 1].im * w[N / 4 + 2 * n + 1];
                x[N / 2 + 2 * n] = -y[N / 8 + n].re * w[N / 2 - 2 * n - 1];
                x[N / 2 + 2 * n + 1] = y[N / 8 - n - 1].im * w[N / 2 - 2 * n - 2];
                x[3 * N / 4 + 2 * n] = y[n].im * w[N / 4 - 2 * n - 1];
                x[3 * N / 4 + 2 * n + 1] = -y[N / 4 - n - 1].re * w[N / 4 - 2 * n - 2];
            }
        } else {
            // §7.9.4.2: koeficienty dvoch polovíc sú prekladané
            let mut x1 = [0f32; N / 4];
            let mut x2 = [0f32; N / 4];
            for k in 0..N / 4 {
                x1[k] = koef[2 * k];
                x2[k] = koef[2 * k + 1];
            }
            let mut y1 = [Komplex::default(); N / 8];
            let mut y2 = [Komplex::default(); N / 8];
            self.jadro(&x1, false, &mut y1);
            self.jadro(&x2, false, &mut y2);
            for n in 0..N / 8 {
                x[2 * n] = -y1[n].im * w[2 * n];
                x[2 * n + 1] = y1[N / 8 - n - 1].re * w[2 * n + 1];
                x[N / 4 + 2 * n] = -y1[n].re * w[N / 4 + 2 * n];
                x[N / 4 + 2 * n + 1] = y1[N / 8 - n - 1].im * w[N / 4 + 2 * n + 1];
                x[N / 2 + 2 * n] = -y2[n].re * w[N / 2 - 2 * n - 1];
                x[N / 2 + 2 * n + 1] = y2[N / 8 - n - 1].im * w[N / 2 - 2 * n - 2];
                x[3 * N / 4 + 2 * n] = y2[n].im * w[N / 4 - 2 * n - 1];
                x[3 * N / 4 + 2 * n + 1] = -y2[N / 8 - n - 1].re * w[N / 4 - 2 * n - 2];
            }
        }
        for n in 0..N / 2 {
            pcm[n] = 2.0 * (x[n] + oneskorenie[n]);
            oneskorenie[n] = x[N / 2 + n];
        }
    }
}
