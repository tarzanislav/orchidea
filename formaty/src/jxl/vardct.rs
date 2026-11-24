//! Režim VarDCT JPEG XL (stratové kódovanie): LF obraz, metadáta HF, kvantizačné matice, koeficienty,
//! spätné DCT premenlivých veľkostí, Gaborov filter a EPF, prevod XYB → lineárne RGB.
//!
//! Plávajúce výpočty používajú len základné operácie a deterministickú matematiku jadra (kosínusy,
//! mocniny) – výsledok je rovnaký na každej platforme.

use super::bity::{Citac, D, ceil_log2, chyba, so_znamienkom};
use super::entropia::Histogramy;
use super::hlavicky::{HlavickaSnimky, Metadata};
use super::modular::{self, Kanal, Obrazok, StromAKody};
use super::snimka::{Rozmery, permutacia};
use super::zvacsenie;
use orchidea_jadro::{Chyba, Vysledok, matematika, paralel};
use std::sync::{Arc, OnceLock};

/// Druhy transformácií blokov (DctSelect) – v poradí normy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Typ {
    Dct8,
    Hornuss,
    Dct2,
    Dct4,
    Dct16,
    Dct32,
    Dct16x8,
    Dct8x16,
    Dct32x8,
    Dct8x32,
    Dct32x16,
    Dct16x32,
    Dct4x8,
    Dct8x4,
    Afv0,
    Afv1,
    Afv2,
    Afv3,
    Dct64,
    Dct64x32,
    Dct32x64,
    Dct128,
    Dct128x64,
    Dct64x128,
    Dct256,
    Dct256x128,
    Dct128x256,
}

const TYPY: [Typ; 27] = [
    Typ::Dct8,
    Typ::Hornuss,
    Typ::Dct2,
    Typ::Dct4,
    Typ::Dct16,
    Typ::Dct32,
    Typ::Dct16x8,
    Typ::Dct8x16,
    Typ::Dct32x8,
    Typ::Dct8x32,
    Typ::Dct32x16,
    Typ::Dct16x32,
    Typ::Dct4x8,
    Typ::Dct8x4,
    Typ::Afv0,
    Typ::Afv1,
    Typ::Afv2,
    Typ::Afv3,
    Typ::Dct64,
    Typ::Dct64x32,
    Typ::Dct32x64,
    Typ::Dct128,
    Typ::Dct128x64,
    Typ::Dct64x128,
    Typ::Dct256,
    Typ::Dct256x128,
    Typ::Dct128x256,
];

impl Typ {
    /// Rozmer v blokoch 8×8 (šírka, výška).
    fn bloky(self) -> (usize, usize) {
        use Typ::*;
        match self {
            Dct8 | Hornuss | Dct2 | Dct4 | Dct4x8 | Dct8x4 | Afv0 | Afv1 | Afv2 | Afv3 => (1, 1),
            Dct16 => (2, 2),
            Dct32 => (4, 4),
            Dct16x8 => (1, 2),
            Dct8x16 => (2, 1),
            Dct32x8 => (1, 4),
            Dct8x32 => (4, 1),
            Dct32x16 => (2, 4),
            Dct16x32 => (4, 2),
            Dct64 => (8, 8),
            Dct64x32 => (4, 8),
            Dct32x64 => (8, 4),
            Dct128 => (16, 16),
            Dct128x64 => (8, 16),
            Dct64x128 => (16, 8),
            Dct256 => (32, 32),
            Dct256x128 => (16, 32),
            Dct128x256 => (32, 16),
        }
    }

    /// Index kvantizačnej matice (17 druhov).
    fn matica(self) -> usize {
        use Typ::*;
        match self {
            Dct8 => 0,
            Hornuss => 1,
            Dct2 => 2,
            Dct4 => 3,
            Dct16 => 4,
            Dct32 => 5,
            Dct16x8 | Dct8x16 => 6,
            Dct32x8 | Dct8x32 => 7,
            Dct32x16 | Dct16x32 => 8,
            Dct4x8 | Dct8x4 => 9,
            Afv0 | Afv1 | Afv2 | Afv3 => 10,
            Dct64 => 11,
            Dct64x32 | Dct32x64 => 12,
            Dct128 => 13,
            Dct128x64 | Dct64x128 => 14,
            Dct256 => 15,
            Dct256x128 | Dct128x256 => 16,
        }
    }

    /// Poradie koeficientov (13 druhov).
    fn poradie(self) -> usize {
        use Typ::*;
        match self {
            Dct8 => 0,
            Hornuss | Dct2 | Dct4 | Dct4x8 | Dct8x4 | Afv0 | Afv1 | Afv2 | Afv3 => 1,
            Dct16 => 2,
            Dct32 => 3,
            Dct16x8 | Dct8x16 => 4,
            Dct32x8 | Dct8x32 => 5,
            Dct32x16 | Dct16x32 => 6,
            Dct64 => 7,
            Dct64x32 | Dct32x64 => 8,
            Dct128 => 9,
            Dct128x64 | Dct64x128 => 10,
            Dct256 => 11,
            Dct256x128 | Dct128x256 => 12,
        }
    }

    /// Súradnice poradia sú pre širší tvar; vyšší (alebo štvorcový) blok DCT ich má prehodené.
    fn prehodit(self) -> bool {
        use Typ::*;
        if matches!(self, Hornuss | Dct2 | Dct4 | Dct4x8 | Dct8x4 | Afv0 | Afv1 | Afv2 | Afv3) {
            return false;
        }
        let (w, h) = self.bloky();
        h >= w
    }
}

/// Rozmery matice (šírka ≥ výška) podľa indexu matice.
const ROZMER_MATICE: [(usize, usize); 17] = [
    (8, 8),
    (8, 8),
    (8, 8),
    (8, 8),
    (16, 16),
    (32, 32),
    (16, 8),
    (32, 8),
    (32, 16),
    (8, 8),
    (8, 8),
    (64, 64),
    (64, 32),
    (128, 128),
    (128, 64),
    (256, 256),
    (256, 128),
];

/// Rozmery poradí koeficientov (šírka ≥ výška).
const ROZMER_PORADIA: [(usize, usize); 13] = [
    (8, 8),
    (8, 8),
    (16, 16),
    (32, 32),
    (16, 8),
    (32, 8),
    (32, 16),
    (64, 64),
    (64, 32),
    (128, 128),
    (128, 64),
    (256, 256),
    (256, 128),
];

/// Prirodzené poradie koeficientov: najprv najnižšie frekvencie (LLF), potom uhlopriečky štvorca.
fn prirodzene_poradie(i: usize) -> &'static [(u16, u16)] {
    static P: [OnceLock<Vec<(u16, u16)>>; 13] = [const { OnceLock::new() }; 13];
    P[i].get_or_init(|| {
        let (bw, bh) = ROZMER_PORADIA[i];
        let mierka = bw / bh;
        let (lbw, lbh) = (bw / 8, bh / 8);
        let mut v = Vec::with_capacity(bw * bh);
        for k in 0..lbw * lbh {
            v.push(((k % lbw) as u16, (k / lbw) as u16));
        }
        for vzd in 1..2 * bw {
            let okraj = vzd.saturating_sub(bw);
            for p in okraj..vzd - okraj {
                let (x, y) = if vzd % 2 == 1 { (p, vzd - 1 - p) } else { (vzd - 1 - p, p) };
                if x < lbw && y < lbw {
                    continue;
                }
                if y % mierka != 0 {
                    continue;
                }
                v.push((x as u16, (y / mierka) as u16));
            }
        }
        v
    })
}

// ---------------------------------------------------------------- kvantizačné matice

enum Kodovanie {
    Hornuss([[f32; 3]; 3]),
    Dct2([[f32; 6]; 3]),
    Dct4([[f32; 2]; 3], [Vec<f32>; 3]),
    Dct4x8([[f32; 1]; 3], [Vec<f32>; 3]),
    Afv([[f32; 9]; 3], [Vec<f32>; 3], [Vec<f32>; 3]),
    Dct([Vec<f32>; 3]),
    Surova(f32, Vec<Vec<i32>>),
}

const SEQ_A: [f32; 7] = [-1.025, -0.78, -0.65012, -0.19041574, -0.20819396, -0.421064, -0.32733846];
const SEQ_B: [f32; 7] =
    [-0.30419582, -0.36330363, -0.3566038, -0.34430745, -0.33699593, -0.30180866, -0.27321684];
const SEQ_C: [f32; 7] = [-1.2, -1.2, -0.8, -0.7, -0.7, -0.4, -0.5];
const DCT4X8_P: [[f32; 4]; 3] = [
    [2198.0505, -0.96269625, -0.7619425, -0.65511405],
    [764.36554, -0.926302, -0.967523, -0.2784529],
    [527.10754, -1.4594386, -1.4500821, -1.5843723],
];
const DCT4_P: [[f32; 4]; 3] = [[2200.0, 0.0, 0.0, 0.0], [392.0, 0.0, 0.0, 0.0], [112.0, -0.25, -0.25, -0.5]];

fn spolocna(a: f32, b: f32, c: f32) -> Kodovanie {
    let r = |z: f32, s: &[f32; 7]| {
        let mut v = vec![z];
        v.extend_from_slice(s);
        v
    };
    Kodovanie::Dct([r(a, &SEQ_A), r(b, &SEQ_B), r(c, &SEQ_C)])
}

fn predvolene(i: usize) -> Kodovanie {
    let d = |a: &[f32], b: &[f32], c: &[f32]| Kodovanie::Dct([a.to_vec(), b.to_vec(), c.to_vec()]);
    match i {
        0 => d(
            &[3150.0, 0.0, -0.4, -0.4, -0.4, -2.0],
            &[560.0, 0.0, -0.3, -0.3, -0.3, -0.3],
            &[512.0, -2.0, -1.0, 0.0, -1.0, -2.0],
        ),
        1 => Kodovanie::Hornuss([[280.0, 3160.0, 3160.0], [60.0, 864.0, 864.0], [18.0, 200.0, 200.0]]),
        2 => Kodovanie::Dct2([
            [3840.0, 2560.0, 1280.0, 640.0, 480.0, 300.0],
            [960.0, 640.0, 320.0, 180.0, 140.0, 120.0],
            [640.0, 320.0, 128.0, 64.0, 32.0, 16.0],
        ]),
        3 => Kodovanie::Dct4([[1.0; 2]; 3], DCT4_P.map(|v| v.to_vec())),
        4 => d(
            &[8996.873, -1.3000778, -0.4942453, -0.43909377, -0.6350102, -0.9017726, -1.6162099],
            &[3191.4836, -0.67424583, -0.80745816, -0.4492584, -0.3586544, -0.3132239, -0.37615025],
            &[1157.504, -2.0531423, -1.4, -0.5068713, -0.4270873, -1.4856834, -4.920914],
        ),
        5 => d(
            &[15718.408, -1.025, -0.98, -0.9012, -0.4, -0.48819396, -0.421064, -0.27],
            &[
                7305.7637,
                -0.8041958,
                -0.76330364,
                -0.5566038,
                -0.49785304,
                -0.43699592,
                -0.40180868,
                -0.27321684,
            ],
            &[3803.5317, -3.0607336, -2.041327, -2.023565, -0.54953897, -0.4, -0.4, -0.3],
        ),
        6 => d(
            &[7240.7734, -0.7, -0.7, -0.2, -0.2, -0.2, -0.5],
            &[1448.1547, -0.5, -0.5, -0.5, -0.2, -0.2, -0.2],
            &[506.85413, -1.4, -0.2, -0.5, -0.5, -1.5, -3.6],
        ),
        7 => d(
            &[16283.249, -1.7812846, -1.6309059, -1.0382179, -0.85, -0.7, -0.9, -1.2360638],
            &[5089.1577, -0.3200494, -0.3536285, -0.3034, -0.61, -0.5, -0.5, -0.6],
            &[3397.7761, -0.32132736, -0.3450762, -0.7034, -0.9, -1.0, -1.0, -1.1754606],
        ),
        8 => d(
            &[13844.971, -0.971138, -0.658, -0.42026, -0.22712, -0.2206, -0.226, -0.6],
            &[
                4798.964,
                -0.6112531,
                -0.8377079,
                -0.7901486,
                -0.26927274,
                -0.38272768,
                -0.22924222,
                -0.20719099,
            ],
            &[1807.2369, -1.2, -1.2, -0.7, -0.7, -0.7, -0.4, -0.5],
        ),
        9 => Kodovanie::Dct4x8([[1.0]; 3], DCT4X8_P.map(|v| v.to_vec())),
        10 => Kodovanie::Afv(
            [
                [3072.0, 3072.0, 256.0, 256.0, 256.0, 414.0, 0.0, 0.0, 0.0],
                [1024.0, 1024.0, 50.0, 50.0, 50.0, 58.0, 0.0, 0.0, 0.0],
                [384.0, 384.0, 12.0, 12.0, 12.0, 22.0, -0.25, -0.25, -0.25],
            ],
            DCT4X8_P.map(|v| v.to_vec()),
            DCT4_P.map(|v| v.to_vec()),
        ),
        11 => spolocna(23966.166, 8380.191, 4493.024),
        12 => spolocna(15358.898, 5597.3604, 2919.9617),
        13 => spolocna(47932.332, 16760.383, 8986.048),
        14 => spolocna(30717.797, 11194.721, 5839.9233),
        15 => spolocna(95864.664, 33520.766, 17972.096),
        _ => spolocna(61435.594, 24209.441, 12979.847),
    }
}

fn nasob(x: f32) -> f32 {
    if x > 0.0 { 1.0 + x } else { 1.0 / (1.0 - x) }
}

fn interpoluj(poz: f32, max: f32, pasma: &[f32]) -> f32 {
    if pasma.len() == 1 {
        return pasma[0];
    }
    let s = poz * (pasma.len() - 1) as f32 / max;
    let i = s as usize;
    let f = s - i as f32;
    let (a, b) = (pasma[i], pasma[(i + 1).min(pasma.len() - 1)]);
    a * matematika::mocnina((b / a) as f64, f as f64) as f32
}

fn vahy_dct(p: &[f32], w: usize, h: usize) -> Vysledok<Vec<f32>> {
    let mut pasma = Vec::with_capacity(p.len());
    let mut posledne = p[0];
    pasma.push(posledne);
    for &v in &p[1..] {
        posledne *= nasob(v);
        if posledne <= 0.0 {
            return Err(chyba("kvantizačná matica: pásmo ≤ 0"));
        }
        pasma.push(posledne);
    }
    let mut von = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 / (w - 1) as f32;
            let dy = y as f32 / (h - 1) as f32;
            let vzd = matematika::odmocnina((dx * dx + dy * dy) as f64) as f32;
            von.push(interpoluj(vzd, std::f32::consts::SQRT_2 + 1e-6, &pasma));
        }
    }
    Ok(von)
}

/// Matica (prevrátené váhy – hodnota, ktorou sa násobí koeficient) pre tri kanály.
fn matica(k: Kodovanie, i: usize) -> Vysledok<[Vec<f32>; 3]> {
    let (w, h) = ROZMER_MATICE[i];
    let mut surova = false;
    let mut m: [Vec<f32>; 3] = match k {
        Kodovanie::Dct(p) => [vahy_dct(&p[0], w, h)?, vahy_dct(&p[1], w, h)?, vahy_dct(&p[2], w, h)?],
        Kodovanie::Hornuss(p) => p.map(|p| {
            let mut r = vec![p[0]; 64];
            r[0] = 1.0;
            r[1] = p[1];
            r[8] = p[1];
            r[9] = p[2];
            r
        }),
        Kodovanie::Dct2(p) => p.map(|p| {
            let mut r = vec![0f32; 64];
            r[0] = 1.0;
            for (i, &v) in p.iter().enumerate() {
                let rozmer = 1usize << (i / 2);
                if i % 2 == 0 {
                    for y in 0..rozmer {
                        for x in rozmer..2 * rozmer {
                            r[y * 8 + x] = v;
                            r[x * 8 + y] = v;
                        }
                    }
                } else {
                    for y in rozmer..2 * rozmer {
                        for x in rozmer..2 * rozmer {
                            r[y * 8 + x] = v;
                        }
                    }
                }
            }
            r
        }),
        Kodovanie::Dct4(p, dp) => {
            let mut v: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for c in 0..3 {
                let mat = vahy_dct(&dp[c], 4, 4)?;
                let mut r = vec![0f32; 64];
                for y in 0..4 {
                    for x in 0..4 {
                        let a = mat[y * 4 + x];
                        r[y * 16 + x * 2] = a;
                        r[y * 16 + x * 2 + 1] = a;
                        r[(y * 2 + 1) * 8 + x * 2] = a;
                        r[(y * 2 + 1) * 8 + x * 2 + 1] = a;
                    }
                }
                r[1] /= p[c][0];
                r[8] /= p[c][0];
                r[9] /= p[c][1];
                v[c] = r;
            }
            v
        }
        Kodovanie::Dct4x8(p, dp) => {
            let mut v: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for c in 0..3 {
                let mat = vahy_dct(&dp[c], 8, 4)?;
                let mut r: Vec<f32> =
                    mat.chunks_exact(8).flat_map(|riadok| [riadok, riadok]).flatten().copied().collect();
                r[8] /= p[c][0];
                v[c] = r;
            }
            v
        }
        Kodovanie::Afv(p, dp, d4) => {
            const FREKV: [f32; 16] = [
                0.0, 0.0, 0.8517779, 5.3777843, 0.0, 0.0, 4.734748, 5.4492455, 1.659827, 4.0, 7.275749,
                10.423227, 2.6629324, 7.6306577, 8.962389, 12.971662,
            ];
            let (lo, hi) = (FREKV[2], FREKV[15]);
            let mut v: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for c in 0..3 {
                let w48 = vahy_dct(&dp[c], 8, 4)?;
                let w44 = vahy_dct(&d4[c], 4, 4)?;
                let mut pasma = [p[c][5], 0.0, 0.0, 0.0];
                for k in 1..4 {
                    pasma[k] = pasma[k - 1] * nasob(p[c][5 + k]);
                }
                let mut r = vec![0f32; 64];
                for y in 0..4 {
                    for x in 0..4 {
                        r[16 * y + 2 * x] = match (x, y) {
                            (0, 0) => 1.0,
                            (0, 1) => p[c][2],
                            (1, 0) => p[c][3],
                            (1, 1) => p[c][4],
                            _ => interpoluj(FREKV[y * 4 + x] - lo, hi - lo + 1e-6, &pasma),
                        };
                    }
                }
                for y in 0..4 {
                    for x in 0..8 {
                        r[16 * y + 8 + x] = if y == 0 && x == 0 { p[c][0] } else { w48[y * 8 + x] };
                    }
                    for x in 0..4 {
                        r[16 * y + 2 * x + 1] = if y == 0 && x == 0 { p[c][1] } else { w44[y * 4 + x] };
                    }
                }
                v[c] = r;
            }
            v
        }
        Kodovanie::Surova(menovatel, kanaly) => {
            surova = true;
            let mut v: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for c in 0..3 {
                v[c] = kanaly[c].iter().map(|&x| x as f32 * menovatel).collect();
            }
            v
        }
    };
    if !surova {
        for x in m.iter_mut().flatten() {
            *x = 1.0 / *x;
        }
    }
    if m.iter().flatten().any(|&x| !(x > 0.0 && x < 1e8)) {
        return Err(chyba("kvantizačná matica s neplatnou hodnotou"));
    }
    Ok(m)
}

type Matica = Arc<[Vec<f32>; 3]>;

/// Predvolené matice (počítajú sa raz za beh programu).
fn predvolene_matice() -> Vysledok<Vec<Matica>> {
    static M: OnceLock<Vec<Matica>> = OnceLock::new();
    if let Some(m) = M.get() {
        return Ok(m.clone());
    }
    let mut v = Vec::with_capacity(17);
    for i in 0..17 {
        v.push(Arc::new(matica(predvolene(i), i)?));
    }
    Ok(M.get_or_init(|| v).clone())
}

/// Prečíta kódovanie jednej matice.
fn citaj_kodovanie(
    c: &mut Citac,
    i: usize,
    stream: u32,
    strom: Option<&StromAKody>,
    bitov: u32,
) -> Vysledok<Kodovanie> {
    let citaj_pevne = |c: &mut Citac, n: usize| -> Vysledok<Vec<[f32; 9]>> {
        let mut v = vec![[0f32; 9]; 3];
        for r in v.iter_mut() {
            for x in r.iter_mut().take(n) {
                *x = c.f16()?;
            }
        }
        Ok(v)
    };
    let citaj_dct = |c: &mut Citac| -> Vysledok<[Vec<f32>; 3]> {
        let n = c.u(4) as usize + 1;
        let mut v: [Vec<f32>; 3] = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
        for r in v.iter_mut() {
            for x in r.iter_mut() {
                *x = c.f16()?;
            }
        }
        for r in v.iter_mut() {
            r[0] *= 64.0;
        }
        Ok(v)
    };
    let rezim = c.u(3);
    if (1..=5).contains(&rezim) && !matches!(i, 0 | 1 | 2 | 3 | 9 | 10) {
        return Err(chyba("neplatné kódovanie kvantizačnej matice"));
    }
    let k = |v: &Vec<[f32; 9]>, n: usize| -> Vec<Vec<f32>> { v.iter().map(|r| r[..n].to_vec()).collect() };
    Ok(match rezim {
        0 => predvolene(i),
        1 => {
            let v = citaj_pevne(c, 3)?;
            let x = k(&v, 3);
            Kodovanie::Hornuss([
                [x[0][0], x[0][1], x[0][2]],
                [x[1][0], x[1][1], x[1][2]],
                [x[2][0], x[2][1], x[2][2]],
            ])
        }
        2 => {
            let v = citaj_pevne(c, 6)?;
            let mut p = [[0f32; 6]; 3];
            for (a, b) in p.iter_mut().zip(&v) {
                a.copy_from_slice(&b[..6]);
            }
            Kodovanie::Dct2(p)
        }
        3 => {
            let v = citaj_pevne(c, 2)?;
            let p = [[v[0][0], v[0][1]], [v[1][0], v[1][1]], [v[2][0], v[2][1]]];
            Kodovanie::Dct4(p, citaj_dct(c)?)
        }
        4 => {
            let v = citaj_pevne(c, 1)?;
            Kodovanie::Dct4x8([[v[0][0]], [v[1][0]], [v[2][0]]], citaj_dct(c)?)
        }
        5 => {
            let mut v = citaj_pevne(c, 9)?;
            for r in v.iter_mut() {
                for x in r.iter_mut().take(6) {
                    *x *= 64.0;
                }
            }
            let p = [v[0], v[1], v[2]];
            let a = citaj_dct(c)?;
            let b = citaj_dct(c)?;
            Kodovanie::Afv(p, a, b)
        }
        6 => Kodovanie::Dct(citaj_dct(c)?),
        _ => {
            let (w, h) = ROZMER_MATICE[i];
            let menovatel = c.f16()?;
            let mut o = Obrazok { kanaly: (0..3).map(|_| Kanal::novy(w, h, 0, 0)).collect(), meta: 0, bitov };
            let t = modular::dekoduj(c, &mut o, stream, strom, usize::MAX)?;
            modular::spat(&mut o, &t)?;
            Kodovanie::Surova(menovatel, o.kanaly.into_iter().map(|k| k.data).collect())
        }
    })
}

// ---------------------------------------------------------------- DCT

/// Sekanty pre rekurzívne DCT veľkosti n: `1 / (2 cos(π (2i+1) / 2n))` pre i < n/2.
fn sekanty(n: usize) -> &'static [f32] {
    static S: [OnceLock<Vec<f32>>; 9] = [const { OnceLock::new() }; 9];
    S[n.trailing_zeros() as usize].get_or_init(|| {
        (0..n / 2)
            .map(|i| {
                let u = std::f64::consts::PI * (2 * i + 1) as f64 / (2 * n) as f64;
                (0.5 / matematika::cos(u)) as f32
            })
            .collect()
    })
}

/// Nenormovaná DCT-III (`x[i] = Σ G[k] cos(π k (2i+1) / 2n)`) na mieste; `t` je pomocné pole dĺžky ≥ n.
fn dct3(v: &mut [f32], t: &mut [f32]) {
    let n = v.len();
    if n == 1 {
        return;
    }
    let h = n / 2;
    let (e, o) = t[..n].split_at_mut(h);
    for m in 0..h {
        e[m] = v[2 * m];
        o[m] = v[2 * m + 1] + if m > 0 { v[2 * m - 1] } else { 0.0 };
    }
    let (v1, v2) = v.split_at_mut(h);
    dct3(e, v1);
    dct3(o, v2);
    let sec = sekanty(n);
    for i in 0..h {
        let r = o[i] * sec[i];
        v[i] = e[i] + r;
        v[n - 1 - i] = e[i] - r;
    }
}

/// Nenormovaná DCT-II (`Y[k] = Σ x[i] cos(π k (2i+1) / 2n)`) na mieste.
fn dct2(v: &mut [f32], t: &mut [f32]) {
    let n = v.len();
    if n == 1 {
        return;
    }
    let h = n / 2;
    let sec = sekanty(n);
    let (a, b) = t[..n].split_at_mut(h);
    for i in 0..h {
        let (x, y) = (v[i], v[n - 1 - i]);
        a[i] = x + y;
        b[i] = (x - y) * sec[i];
    }
    let (v1, v2) = v.split_at_mut(h);
    dct2(a, v1);
    dct2(b, v2);
    for m in 0..h {
        v[2 * m] = a[m];
        v[2 * m + 1] = b[m] + if m + 1 < h { b[m + 1] } else { 0.0 };
    }
}

/// Spätná DCT 1D v norme JPEG XL: `x[i] = Σ c_k F[k] cos(…)`, c_0 = 1, inak √2.
fn idct_1d(v: &mut [f32], t: &mut [f32]) {
    for x in v.iter_mut().skip(1) {
        *x *= std::f32::consts::SQRT_2;
    }
    dct3(v, t);
}

/// Dopredná DCT 1D: `F[k] = (c_k / n) Σ x[i] cos(…)` (F[0] je priemer).
fn dct_1d(v: &mut [f32], t: &mut [f32]) {
    let n = v.len();
    dct2(v, t);
    let (a, b) = (1.0 / n as f32, std::f32::consts::SQRT_2 / n as f32);
    v[0] *= a;
    for x in v.iter_mut().skip(1) {
        *x *= b;
    }
}

/// 2D DCT obdĺžnika `w × h` v `d` (riadky s krokom `krok`), dopredne alebo spätne.
fn dct_2d(d: &mut [f32], krok: usize, w: usize, h: usize, spatne: bool) {
    let mut riadok = [0f32; 256];
    let mut t = [0f32; 256];
    let f = if spatne { idct_1d } else { dct_1d };
    if w > 1 {
        for y in 0..h {
            f(&mut d[y * krok..y * krok + w], &mut t);
        }
    }
    if h > 1 {
        for x in 0..w {
            for y in 0..h {
                riadok[y] = d[y * krok + x];
            }
            f(&mut riadok[..h], &mut t);
            for y in 0..h {
                d[y * krok + x] = riadok[y];
            }
        }
    }
}

#[allow(clippy::excessive_precision)]
/// Mierky medzi DCT priemerov blokov (LF) a najnižšími koeficientmi veľkej DCT (pre N = 256).
const MIERKA_F: [f32; 32] = [
    1.0, 0.9996047, 0.99841945, 0.9964458, 0.9936866, 0.9901456, 0.9858278, 0.9807392, 0.9748868, 0.9682788,
    0.9609244, 0.95283375, 0.9440181, 0.93448964, 0.9242616, 0.9133481, 0.9017642, 0.8895259, 0.8766501,
    0.8631544, 0.8490575, 0.8343786, 0.8191379, 0.80335615, 0.7870549, 0.7702564, 0.7529834, 0.7352593,
    0.7171081, 0.6985543, 0.67962285, 0.6603391,
];

/// Báza AFV (4×4 bloky, norma).
#[allow(clippy::excessive_precision)]
#[rustfmt::skip]
const AFV_BAZA: [[f32; 16]; 16] = [
    [0.25; 16],
    [
        0.876902929799142, 0.2206518106944235, -0.10140050393753763, -0.1014005039375375,
        0.2206518106944236, -0.10140050393753777, -0.10140050393753772, -0.10140050393753763,
        -0.10140050393753758, -0.10140050393753769, -0.1014005039375375, -0.10140050393753768,
        -0.10140050393753768, -0.10140050393753759, -0.10140050393753763, -0.10140050393753741,
    ],
    [
        0.0, 0.0, 0.40670075830260755, 0.44444816619734445,
        0.0, 0.0, 0.19574399372042936, 0.2929100136981264,
        -0.40670075830260716, -0.19574399372042872, 0.0, 0.11379074460448091,
        -0.44444816619734384, -0.29291001369812636, -0.1137907446044814, 0.0,
    ],
    [
        0.0, 0.0, -0.21255748058288748, 0.3085497062849767,
        0.0, 0.4706702258572536, -0.1621205195722993, 0.0,
        -0.21255748058287047, -0.16212051957228327, -0.47067022585725277, -0.1464291867126764,
        0.3085497062849487, 0.0, -0.14642918671266536, 0.4251149611657548,
    ],
    [
        0.0, -std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0,
        std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0,
    ],
    [
        -0.4105377591765233, 0.6235485373547691, -0.06435071657946274, -0.06435071657946266,
        0.6235485373547694, -0.06435071657946284, -0.0643507165794628, -0.06435071657946274,
        -0.06435071657946272, -0.06435071657946279, -0.06435071657946266, -0.06435071657946277,
        -0.06435071657946277, -0.06435071657946273, -0.06435071657946274, -0.0643507165794626,
    ],
    [
        0.0, 0.0, -0.4517556589999482, 0.15854503551840063,
        0.0, -0.04038515160822202, 0.0074182263792423875, 0.39351034269210167,
        -0.45175565899994635, 0.007418226379244351, 0.1107416575309343, 0.08298163094882051,
        0.15854503551839705, 0.3935103426921022, 0.0829816309488214, -0.45175565899994796,
    ],
    [
        0.0, 0.0, -0.304684750724869, 0.5112616136591823,
        0.0, 0.0, -0.290480129728998, -0.06578701549142804,
        0.304684750724884, 0.2904801297290076, 0.0, -0.23889773523344604,
        -0.5112616136592012, 0.06578701549142545, 0.23889773523345467, 0.0,
    ],
    [
        0.0, 0.0, 0.3017929516615495, 0.25792362796341184,
        0.0, 0.16272340142866204, 0.09520022653475037, 0.0,
        0.3017929516615503, 0.09520022653475055, -0.16272340142866173, -0.35312385449816297,
        0.25792362796341295, 0.0, -0.3531238544981624, -0.6035859033230976,
    ],
    [
        0.0, 0.0, 0.40824829046386274, 0.0,
        0.0, 0.0, 0.0, -0.4082482904638628,
        -0.4082482904638635, 0.0, 0.0, -0.40824829046386296,
        0.0, 0.4082482904638634, 0.408248290463863, 0.0,
    ],
    [
        0.0, 0.0, 0.1747866975480809, 0.0812611176717539,
        0.0, 0.0, -0.3675398009862027, -0.307882213957909,
        -0.17478669754808135, 0.3675398009862011, 0.0, 0.4826689115059883,
        -0.08126111767175039, 0.30788221395790305, -0.48266891150598584, 0.0,
    ],
    [
        0.0, 0.0, -0.21105601049335784, 0.18567180916109802,
        0.0, 0.0, 0.49215859013738733, -0.38525013709251915,
        0.21105601049335806, -0.49215859013738905, 0.0, 0.17419412659916217,
        -0.18567180916109904, 0.3852501370925211, -0.1741941265991621, 0.0,
    ],
    [
        0.0, 0.0, -0.14266084808807264, -0.3416446842253372,
        0.0, 0.7367497537172237, 0.24627107722075148, -0.08574019035519306,
        -0.14266084808807344, 0.24627107722075137, 0.14883399227113567, -0.04768680350229251,
        -0.3416446842253373, -0.08574019035519267, -0.047686803502292804, -0.14266084808807242,
    ],
    [
        0.0, 0.0, -0.13813540350758585, 0.3302282550303788,
        0.0, 0.08755115000587084, -0.07946706605909573, -0.4613374887461511,
        -0.13813540350758294, -0.07946706605910261, 0.49724647109535086, 0.12538059448563663,
        0.3302282550303805, -0.4613374887461554, 0.12538059448564315, -0.13813540350758452,
    ],
    [
        0.0, 0.0, -0.17437602599651067, 0.0702790691196284,
        0.0, -0.2921026642334881, 0.3623817333531167, 0.0,
        -0.1743760259965108, 0.36238173335311646, 0.29210266423348785, -0.4326608024727445,
        0.07027906911962818, 0.0, -0.4326608024727457, 0.34875205199302267,
    ],
    [
        0.0, 0.0, 0.11354987314994337, -0.07417504595810355,
        0.0, 0.19402893032594343, -0.435190496523228, 0.21918684838857466,
        0.11354987314994257, -0.4351904965232251, 0.5550443808910661, -0.25468277124066463,
        -0.07417504595810233, 0.2191868483885728, -0.25468277124066413, 0.1135498731499429,
    ],
];

/// Spätná transformácia bloku 8×8 a menších druhov; `b[y * k + x]`.
fn transformacia_mala(typ: Typ, b: &mut [f32], k: usize) {
    let g = |b: &[f32], x: usize, y: usize| b[y * k + x];
    // spätné 2×2 (súčty a rozdiely) nad kvadrantmi veľkosti n
    let idct2 = |b: &mut [f32], n: usize| {
        let p = n / 2;
        let mut t = [[0f32; 8]; 8];
        for y in 0..p {
            for x in 0..p {
                let (c00, c01, c10, c11) =
                    (b[y * k + x], b[y * k + x + p], b[(y + p) * k + x], b[(y + p) * k + x + p]);
                t[2 * y][2 * x] = c00 + c01 + c10 + c11;
                t[2 * y][2 * x + 1] = c00 + c01 - c10 - c11;
                t[2 * y + 1][2 * x] = c00 - c01 + c10 - c11;
                t[2 * y + 1][2 * x + 1] = c00 - c01 - c10 + c11;
            }
        }
        for y in 0..n {
            b[y * k..y * k + n].copy_from_slice(&t[y][..n]);
        }
    };
    match typ {
        Typ::Dct2 => {
            idct2(b, 2);
            idct2(b, 4);
            idct2(b, 8);
        }
        Typ::Dct4 => {
            idct2(b, 2);
            let mut s = [[0f32; 16]; 4];
            for y in 0..2 {
                for x in 0..2 {
                    let q = &mut s[y * 2 + x];
                    for iy in 0..4 {
                        for ix in 0..4 {
                            // prehodené: riadok ix, stĺpec iy
                            q[ix * 4 + iy] = g(b, x + ix * 2, y + iy * 2);
                        }
                    }
                    dct_2d(q, 4, 4, 4, true);
                }
            }
            for y in 0..2 {
                for x in 0..2 {
                    for iy in 0..4 {
                        for ix in 0..4 {
                            b[(y * 4 + iy) * k + x * 4 + ix] = s[y * 2 + x][iy * 4 + ix];
                        }
                    }
                }
            }
        }
        Typ::Hornuss => {
            idct2(b, 2);
            let mut s = [[0f32; 16]; 4];
            for y in 0..2 {
                for x in 0..2 {
                    let q = &mut s[y * 2 + x];
                    for iy in 0..4 {
                        for ix in 0..4 {
                            q[iy * 4 + ix] = g(b, x + ix * 2, y + iy * 2);
                        }
                    }
                    let zvysok: f32 = q[1..].iter().sum();
                    let priemer = q[0] - zvysok / 16.0;
                    q[0] = q[5];
                    q[5] = 0.0;
                    for v in q.iter_mut() {
                        *v += priemer;
                    }
                }
            }
            for y in 0..2 {
                for x in 0..2 {
                    for iy in 0..4 {
                        for ix in 0..4 {
                            b[(y * 4 + iy) * k + x * 4 + ix] = s[y * 2 + x][iy * 4 + ix];
                        }
                    }
                }
            }
        }
        Typ::Dct4x8 | Typ::Dct8x4 => {
            let (c0, c1) = (g(b, 0, 0), g(b, 0, 1));
            b[0] = c0 + c1;
            b[k] = c0 - c1;
            let mut s = [0f32; 64];
            for i in 0..2 {
                for iy in 0..4 {
                    for ix in 0..8 {
                        s[i * 32 + iy * 8 + ix] = g(b, ix, iy * 2 + i);
                    }
                }
                dct_2d(&mut s[i * 32..i * 32 + 32], 8, 8, 4, true);
            }
            for y in 0..8 {
                for x in 0..8 {
                    if typ == Typ::Dct8x4 {
                        b[x * k + y] = s[y * 8 + x];
                    } else {
                        b[y * k + x] = s[y * 8 + x];
                    }
                }
            }
        }
        Typ::Afv0 | Typ::Afv1 | Typ::Afv2 | Typ::Afv3 => {
            let n = match typ {
                Typ::Afv0 => 0,
                Typ::Afv1 => 1,
                Typ::Afv2 => 2,
                _ => 3,
            };
            let (fx, fy) = (n % 2, n / 2);
            let mut ka = [0f32; 16];
            ka[0] = (g(b, 0, 0) + g(b, 1, 0) + g(b, 0, 1)) * 4.0;
            for (i, v) in ka.iter_mut().enumerate().skip(1) {
                *v = g(b, 2 * (i % 4), 2 * (i / 4));
            }
            let mut vz = [0f32; 16];
            for (koef, baza) in ka.iter().zip(&AFV_BAZA) {
                for (s, bz) in vz.iter_mut().zip(baza) {
                    *s += koef * bz;
                }
            }
            let mut s44 = [0f32; 16];
            s44[0] = g(b, 0, 0) - g(b, 1, 0) + g(b, 0, 1);
            for iy in 0..4 {
                for ix in 0..4 {
                    if ix | iy != 0 {
                        s44[ix * 4 + iy] = g(b, 2 * ix + 1, 2 * iy);
                    }
                }
            }
            dct_2d(&mut s44, 4, 4, 4, true);
            let mut s48 = [0f32; 32];
            s48[0] = g(b, 0, 0) - g(b, 0, 1);
            for iy in 0..4 {
                for ix in 0..8 {
                    if ix | iy != 0 {
                        s48[iy * 8 + ix] = g(b, ix, 2 * iy + 1);
                    }
                }
            }
            dct_2d(&mut s48, 8, 8, 4, true);
            for iy in 0..4 {
                let ay = if fy == 0 { iy } else { 3 - iy };
                for ix in 0..4 {
                    let ax = if fx == 0 { ix } else { 3 - ix };
                    b[(fy * 4 + iy) * k + fx * 4 + ix] = vz[ay * 4 + ax];
                }
            }
            for iy in 0..4 {
                for ix in 0..4 {
                    b[(fy * 4 + iy) * k + (1 - fx) * 4 + ix] = s44[iy * 4 + ix];
                }
            }
            for iy in 0..4 {
                for ix in 0..8 {
                    b[((1 - fy) * 4 + iy) * k + ix] = s48[iy * 8 + ix];
                }
            }
        }
        _ => dct_2d(b, k, 8, 8, true),
    }
}

// ---------------------------------------------------------------- stav

#[derive(Debug, Clone, Copy, PartialEq)]
enum Blok {
    Prazdny,
    Obsadeny,
    Zaciatok { typ: Typ, hf_mul: i32 },
}

struct KontextBloku {
    qf_prahy: Vec<u32>,
    lf_prahy: [Vec<i32>; 3],
    mapa: Vec<u8>,
    pocet: usize,
}

struct Prechod {
    poradia: Vec<[Vec<(u16, u16)>; 3]>,
    kody: Histogramy,
}

/// Koeficienty jednej skupiny (tri kanály, šírka × výška v pixeloch skupiny).
struct KoefSkupiny {
    k: [Vec<i32>; 3],
}

pub(super) struct Stav {
    roz: Rozmery,
    /// Bloky 8×8 obrazu (bez zarovnania pre podvzorkovanie) – rozmery LF v Modular.
    w8: usize,
    h8: usize,
    /// Mriežka blokov (pri podvzorkovaní farby zarovnaná na párny počet).
    bw8: usize,
    bh8: usize,
    /// Posuny kanálov X (Cb), Y, B (Cr) pri podvzorkovaní farby (vodorovne, zvisle).
    sh: [(usize, usize); 3],
    podvzorkovane: bool,
    /// Rozmery mriežky blokov každého kanála a rozmery jeho roviny v pixeloch.
    kw8: [usize; 3],
    kh8: [usize; 3],
    pw: [usize; 3],
    ph: [usize; 3],
    globalna_mierka: u32,
    kvant_lf: u32,
    lf_m: [f32; 3],
    bctx: KontextBloku,
    cfl_faktor: f32,
    cfl_x: f32,
    cfl_b: f32,
    cfl_x_lf: f32,
    cfl_b_lf: f32,
    lf: [Vec<f32>; 3],
    lf_kvant: [Vec<i32>; 3],
    bloky: Vec<Blok>,
    x_z_y: Vec<i32>,
    b_z_y: Vec<i32>,
    w64: usize,
    sigma: Vec<f32>,
    epf: Option<(f32, [f32; 8])>,
    matice: Vec<Matica>,
    presety: u32,
    prechody: Vec<Prechod>,
    koef: Vec<Option<KoefSkupiny>>,
    xyb: [Vec<f32>; 3],
    hlbka: u32,
    bez_vyhladenia: bool,
    x_qm: u32,
    b_qm: u32,
    posuny: Vec<u32>,
    kvant_bias: [f32; 3],
    kvant_citatel: f32,
    lf_zo_snimky: bool,
}

const PRIZNAK_BEZ_VYHLADENIA_LF: u64 = 128;

impl Stav {
    pub fn lf_global(
        c: &mut Citac,
        h: &HlavickaSnimky,
        m: &Metadata,
        r: Rozmery,
        lf: [f32; 3],
        lf_snimka: Option<&[Vec<f32>; 3]>,
    ) -> Vysledok<Stav> {
        let lf_zo_snimky = h.priznaky & 32 != 0;
        let pv = h.jpeg_podvzorkovanie;
        let podvzorkovane = h.ycbcr && pv.iter().any(|&x| x != 0);
        if pv.iter().any(|&x| x > 3) {
            return Err(chyba("neplatné podvzorkovanie farby"));
        }
        let hs = pv.iter().any(|&x| x == 1 || x == 2);
        let vs = pv.iter().any(|&x| x == 1 || x == 3);
        let sh: [(usize, usize); 3] = if podvzorkovane {
            pv.map(|u| match u {
                0 => (hs as usize, vs as usize),
                1 => (0, 0),
                2 => (0, vs as usize),
                _ => (hs as usize, 0),
            })
        } else {
            [(0, 0); 3]
        };
        if podvzorkovane && (lf_zo_snimky || h.filtre.gabor || h.filtre.epf_iteracie > 0) {
            return Err(chyba("podvzorkovanie farby s LF snímkou alebo filtrami zatiaľ nie je podporené"));
        }
        if lf_zo_snimky && lf_snimka.is_none() {
            return Err(chyba("snímka odkazuje na chýbajúcu LF snímku"));
        }
        let globalna_mierka = c.u32([D::B(11, 1), D::B(11, 2049), D::B(12, 4097), D::B(16, 8193)]);
        let kvant_lf = c.u32([D::V(16), D::B(5, 1), D::B(8, 1), D::B(16, 1)]);
        let bctx = if c.bool() {
            KontextBloku {
                qf_prahy: Vec::new(),
                lf_prahy: [Vec::new(), Vec::new(), Vec::new()],
                mapa: vec![
                    0, 1, 2, 2, 3, 3, 4, 5, 6, 6, 6, 6, 6, 7, 8, 9, 9, 10, 11, 12, 13, 14, 14, 14, 14, 14, 7,
                    8, 9, 9, 10, 11, 12, 13, 14, 14, 14, 14, 14,
                ],
                pocet: 15,
            }
        } else {
            let mut lf_prahy = [Vec::new(), Vec::new(), Vec::new()];
            let mut velkost = 1usize;
            for p in lf_prahy.iter_mut() {
                let n = c.u(4);
                velkost *= n as usize + 1;
                for _ in 0..n {
                    p.push(so_znamienkom(c.u32([D::B(4, 0), D::B(8, 16), D::B(16, 272), D::B(32, 65_808)])));
                }
            }
            let n = c.u(4);
            velkost *= n as usize + 1;
            let mut qf_prahy = Vec::new();
            for _ in 0..n {
                qf_prahy.push(1 + c.u32([D::B(2, 0), D::B(3, 4), D::B(5, 12), D::B(8, 44)]));
            }
            if velkost > 64 {
                return Err(chyba("priveľa kontextov blokov"));
            }
            let h = Histogramy::citaj_mapu(c, velkost * 39)?;
            let pocet = *h.iter().max().unwrap_or(&0) as usize + 1;
            if pocet > 16 {
                return Err(chyba("priveľa zhlukov kontextov blokov"));
            }
            KontextBloku { qf_prahy, lf_prahy, mapa: h, pocet }
        };
        let (mut faktor, mut zx, mut zb, mut xlf, mut blf) = (84u32, 0.0f32, 1.0f32, 128u32, 128u32);
        if !c.bool() {
            faktor = c.u32([D::V(84), D::V(256), D::B(8, 2), D::B(16, 258)]);
            zx = c.f16()?;
            zb = c.f16()?;
            xlf = c.u(8);
            blf = c.u(8);
        }
        c.over()?;
        let w8 = r.w.div_ceil(8);
        let h8 = r.h.div_ceil(8);
        let bw8 = if podvzorkovane && hs { w8.div_ceil(2) * 2 } else { w8 };
        let bh8 = if podvzorkovane && vs { h8.div_ceil(2) * 2 } else { h8 };
        let kw8 = sh.map(|(a, _)| bw8.div_ceil(1 << a));
        let kh8 = sh.map(|(_, b)| bh8.div_ceil(1 << b));
        let pw = sh.map(|(a, _)| r.w.div_ceil(1 << a));
        let ph = sh.map(|(_, b)| r.h.div_ceil(1 << b));
        let ff = faktor as f32;
        Ok(Stav {
            roz: r,
            w8,
            h8,
            bw8,
            bh8,
            sh,
            podvzorkovane,
            kw8,
            kh8,
            pw,
            ph,
            globalna_mierka,
            kvant_lf,
            lf_m: [lf[0] * 128.0, lf[1] * 128.0, lf[2] * 128.0],
            bctx,
            cfl_faktor: ff,
            cfl_x: zx,
            cfl_b: zb,
            cfl_x_lf: zx + (xlf as i32 - 128) as f32 / ff,
            cfl_b_lf: zb + (blf as i32 - 128) as f32 / ff,
            lf: [0, 1, 2].map(|c| vec![0.0; kw8[c] * kh8[c]]),
            lf_kvant: [0, 1, 2].map(|c| vec![0; kw8[c] * kh8[c]]),
            bloky: vec![Blok::Prazdny; bw8 * bh8],
            x_z_y: vec![0; r.w.div_ceil(64) * r.h.div_ceil(64)],
            b_z_y: vec![0; r.w.div_ceil(64) * r.h.div_ceil(64)],
            w64: r.w.div_ceil(64),
            sigma: vec![0.0; bw8 * bh8],
            epf: (h.filtre.epf_iteracie > 0)
                .then(|| (h.filtre.epf_kvant_nas * 65536.0 / globalna_mierka as f32, h.filtre.epf_ostrost)),
            matice: Vec::new(),
            presety: 1,
            prechody: Vec::new(),
            koef: (0..r.skupin()).map(|_| None).collect(),
            xyb: [0, 1, 2].map(|c| vec![0.0; pw[c] * ph[c]]),
            hlbka: m.hlbka.bitov,
            bez_vyhladenia: h.priznaky & PRIZNAK_BEZ_VYHLADENIA_LF != 0,
            x_qm: h.x_qm,
            b_qm: h.b_qm,
            posuny: h.prechody.posun.clone(),
            kvant_bias: m.kvant_bias,
            kvant_citatel: m.kvant_bias_citatel,
            lf_zo_snimky,
        })
        .and_then(|mut s| {
            if let Some(l) = lf_snimka.filter(|_| lf_zo_snimky) {
                if l.iter().any(|v| v.len() != w8 * h8) {
                    return Err(chyba("LF snímka má iné rozmery"));
                }
                s.lf = l.clone();
            }
            Ok(s)
        })
    }

    /// Obdĺžnik LF skupiny v blokoch: (x0, y0, šírka, výška).
    fn lf_obdlznik(&self, g: usize) -> (usize, usize, usize, usize) {
        let d = self.roz.skupina; // LF skupina má 8 × skupina pixelov = skupina blokov
        let (x0, y0) = ((g % self.roz.lf_x) * d, (g / self.roz.lf_x) * d);
        (x0, y0, d.min(self.w8 - x0), d.min(self.h8 - y0))
    }

    pub fn lf_skupina(&mut self, c: &mut Citac, g: usize, strom: Option<&StromAKody>) -> Vysledok<()> {
        if self.lf_zo_snimky {
            return Ok(());
        }
        let presnost = c.u(2);
        let (x0, y0, w, h) = self.lf_obdlznik(g);
        // modulárne kanály sú v poradí Y, X, B; podvzorkované majú menšie rozmery
        let poradie = [1usize, 0, 2];
        // pri podvzorkovaní majú aj nepodvzorkované kanály rozmer zarovnaný na párny počet
        let (ph, pv) = (
            self.bw8 != self.w8 || self.podvzorkovane && self.sh.iter().any(|s| s.0 == 1),
            self.bh8 != self.h8 || self.podvzorkovane && self.sh.iter().any(|s| s.1 == 1),
        );
        let rozmer = |cc: usize| {
            let (a, b) = self.sh[cc];
            let kw = if ph { w.div_ceil(2) << (1 - a) } else { w };
            let kh = if pv { h.div_ceil(2) << (1 - b) } else { h };
            (kw, kh, a as i32, b as i32)
        };
        let mut o = Obrazok {
            kanaly: poradie
                .iter()
                .map(|&cc| {
                    let (kw, kh, a, b) = rozmer(cc);
                    Kanal::novy(kw, kh, a, b)
                })
                .collect(),
            meta: 0,
            bitov: self.hlbka,
        };
        let t = modular::dekoduj(c, &mut o, 1 + g as u32, strom, usize::MAX)?;
        modular::spat(&mut o, &t)?;
        for k in o.kanaly.iter_mut() {
            if k.data.len() != k.w * k.h {
                k.data = vec![0; k.w * k.h];
            }
        }
        let mierka_inv = self.globalna_mierka as f64 * self.kvant_lf as f64;
        for (mc, &cc) in poradie.iter().enumerate() {
            let m = (self.lf_m[cc] as f64 * (1u64 << (9 - presnost)) as f64 / mierka_inv) as f32;
            let k = &o.kanaly[mc];
            let (kw, kh, a, b) = rozmer(cc);
            if k.w != kw || k.h != kh {
                return Err(chyba("LF kanál má zlé rozmery"));
            }
            let (cx0, cy0) = (x0 >> a, y0 >> b);
            let sirka = self.kw8[cc];
            for y in 0..kh.min(self.kh8[cc] - cy0) {
                for x in 0..kw.min(sirka - cx0) {
                    let q = k.data[y * kw + x];
                    let i = (cy0 + y) * sirka + cx0 + x;
                    self.lf_kvant[cc][i] = q;
                    self.lf[cc][i] = q as f32 * m;
                }
            }
        }
        Ok(())
    }

    pub fn hf_metadata(&mut self, c: &mut Citac, g: usize, strom: Option<&StromAKody>) -> Vysledok<()> {
        let (x0, y0, bw, bh) = self.lf_obdlznik(g);
        // korelácia farieb po 64 pixeloch skutočnej šírky; mriežka blokov zarovnaná pre podvzorkovanie
        let (cw, ch) =
            ((self.roz.w - x0 * 8).min(bw * 8).div_ceil(64), (self.roz.h - y0 * 8).min(bh * 8).div_ceil(64));
        let bw = if self.bw8 != self.w8 { bw.div_ceil(2) * 2 } else { bw };
        let bh = if self.bh8 != self.h8 { bh.div_ceil(2) * 2 } else { bh };
        let blokov = 1 + c.u(ceil_log2((bw * bh) as u32)) as usize;
        let mut o = Obrazok {
            kanaly: vec![
                Kanal::novy(cw, ch, 0, 0),
                Kanal::novy(cw, ch, 0, 0),
                Kanal::novy(blokov, 2, 0, 0),
                Kanal::novy(bw, bh, 0, 0),
            ],
            meta: 0,
            bitov: self.hlbka,
        };
        let stream = 1 + 2 * self.roz.lf_skupin() + g;
        let t = modular::dekoduj(c, &mut o, stream as u32, strom, usize::MAX)?;
        modular::spat(&mut o, &t)?;
        for k in o.kanaly.iter_mut() {
            if k.data.len() != k.w * k.h {
                k.data = vec![0; k.w * k.h];
            }
        }
        if o.kanaly.len() != 4 || o.kanaly[2].w != blokov {
            return Err(chyba("metadáta HF majú zlé kanály"));
        }
        // korelácia farieb po dlaždiciach 64×64
        let (tx0, ty0) = (x0 / 8, y0 / 8);
        for y in 0..ch {
            for x in 0..cw {
                let i = (ty0 + y) * self.w64 + tx0 + x;
                self.x_z_y[i] = o.kanaly[0].data[y * cw + x];
                self.b_z_y[i] = o.kanaly[1].data[y * cw + x];
            }
        }
        let info = &o.kanaly[2].data;
        let ostrost = &o.kanaly[3].data;
        let mut n = 0usize;
        for y in 0..bh {
            let mut x = 0;
            while x < bw {
                let i = (y0 + y) * self.bw8 + x0 + x;
                if self.bloky[i] != Blok::Prazdny {
                    x += 1;
                    continue;
                }
                if n >= blokov {
                    return Err(chyba("málo údajov o blokoch"));
                }
                let t = info[n];
                let typ = *TYPY.get(t as usize).ok_or_else(|| chyba("neznámy druh transformácie"))?;
                let hf_mul = info[blokov + n].saturating_add(1);
                if hf_mul <= 0 {
                    return Err(chyba("nekladný násobok kvantizácie"));
                }
                let (dw, dh) = typ.bloky();
                if self.podvzorkovane && typ != Typ::Dct8 {
                    return Err(chyba("pri podvzorkovaní farby sú podporené len bloky DCT 8×8"));
                }
                if (x0 + x) % 32 + dw > 32 || (y0 + y) % 32 + dh > 32 || x + dw > bw || y + dh > bh {
                    return Err(chyba("blok presahuje skupinu"));
                }
                for dy in 0..dh {
                    for dx in 0..dw {
                        let j = (y0 + y + dy) * self.bw8 + x0 + x + dx;
                        if self.bloky[j] != Blok::Prazdny {
                            return Err(chyba("bloky sa prekrývajú"));
                        }
                        self.bloky[j] =
                            if dx == 0 && dy == 0 { Blok::Zaciatok { typ, hf_mul } } else { Blok::Obsadeny };
                        let s = ostrost[(y + dy) * bw + x + dx];
                        if !(0..8).contains(&s) {
                            return Err(chyba("neplatná ostrosť EPF"));
                        }
                        if let Some((q, lut)) = self.epf {
                            self.sigma[j] = q / hf_mul as f32 * lut[s as usize];
                        }
                    }
                }
                n += 1;
                x += dw;
            }
        }
        Ok(())
    }

    pub fn hf_global(&mut self, c: &mut Citac, strom: Option<&StromAKody>) -> Vysledok<()> {
        self.matice = if c.bool() {
            predvolene_matice()?
        } else {
            let mut matice = Vec::with_capacity(17);
            for i in 0..17 {
                let stream = 1 + 3 * self.roz.lf_skupin() + i;
                let k = citaj_kodovanie(c, i, stream as u32, strom, self.hlbka)?;
                matice.push(Arc::new(matica(k, i)?));
            }
            matice
        };
        self.presety = c.u(ceil_log2(self.roz.skupin() as u32)) + 1;
        let pocet_prechodov = self.posuny.len() + 1;
        for _ in 0..pocet_prechodov {
            let mut pouzite = c.u32([D::V(0x5F), D::V(0x13), D::V(0), D::B(13, 0)]);
            let mut poradia: Vec<[Vec<(u16, u16)>; 3]> =
                (0..13).map(|_| [Vec::new(), Vec::new(), Vec::new()]).collect();
            if pouzite != 0 {
                let h = Histogramy::citaj(c, 8)?;
                let mut t = h.citac(c);
                for (i, p) in poradia.iter_mut().enumerate() {
                    if pouzite & 1 == 1 {
                        let (bw, bh) = ROZMER_PORADIA[i];
                        let n = bw * bh;
                        let prir = prirodzene_poradie(i);
                        for kanal in p.iter_mut() {
                            let perm = permutacia(c, &h, &mut t, n, n / 64)?;
                            *kanal = perm.iter().map(|&j| prir[j]).collect();
                        }
                    }
                    pouzite >>= 1;
                }
                t.koniec(&h)?;
            }
            let kody = Histogramy::citaj(c, 495 * self.presety as usize * self.bctx.pocet)?;
            self.prechody.push(Prechod { poradia, kody });
        }
        c.over()?;
        self.dokonci_lf();
        Ok(())
    }

    /// Korelácia farieb a adaptívne vyhladenie LF obrazu (po prečítaní všetkých LF skupín).
    fn dokonci_lf(&mut self) {
        if self.lf_zo_snimky || self.podvzorkovane {
            return;
        }
        let n = self.kw8[0] * self.kh8[0];
        for i in 0..n {
            let y = self.lf[1][i];
            self.lf[0][i] += self.cfl_x_lf * y;
            self.lf[2][i] += self.cfl_b_lf * y;
        }
        if self.bez_vyhladenia || self.kw8[0] <= 2 || self.kh8[0] <= 2 {
            return;
        }
        let mierka_inv = self.globalna_mierka as f64 * self.kvant_lf as f64;
        let lfm = self.lf_m.map(|m| (512.0 * m as f64 / mierka_inv) as f32);
        const VLASTNY: f32 = 0.052262735;
        const STRANA: f32 = 0.2034514;
        const DIAG: f32 = 0.03348292;
        let (w, h) = (self.kw8[0], self.kh8[0]);
        let povodne = self.lf.clone();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let mut wa = [0f32; 3];
                let mut medzera = 0.5f32;
                for c in 0..3 {
                    let p = &povodne[c];
                    let s = p[y * w + x];
                    let strana =
                        p[y * w + x - 1] + p[y * w + x + 1] + (p[(y - 1) * w + x] + p[(y + 1) * w + x]);
                    let diag = (p[(y - 1) * w + x - 1] + p[(y + 1) * w + x - 1])
                        + (p[(y - 1) * w + x + 1] + p[(y + 1) * w + x + 1]);
                    wa[c] = s * VLASTNY + strana * STRANA + diag * DIAG;
                    medzera = medzera.max((wa[c] - s).abs() / lfm[c]);
                }
                let m = (3.0 - 4.0 * medzera).max(0.0);
                for c in 0..3 {
                    let s = povodne[c][y * w + x];
                    self.lf[c][y * w + x] = (wa[c] - s) * m + s;
                }
            }
        }
    }

    /// Prázdne koeficienty skupiny `g`.
    fn nove_koef(&self, g: usize) -> KoefSkupiny {
        KoefSkupiny {
            k: [0, 1, 2].map(|c| {
                let (_, _, bw, bh) = self.skupina_kanala(g, c);
                vec![0; bw * 8 * bh * 8]
            }),
        }
    }

    /// Skupina `g` v blokoch kanála `c`: (x0, y0, šírka, výška).
    fn skupina_kanala(&self, g: usize, c: usize) -> (usize, usize, usize, usize) {
        let (a, b) = self.sh[c];
        let bx0 = (g % self.roz.skup_x) * 32;
        let by0 = (g / self.roz.skup_x) * 32;
        let bw = 32.min(self.bw8 - bx0);
        let bh = 32.min(self.bh8 - by0);
        (bx0 >> a, by0 >> b, bw.div_ceil(1 << a), bh.div_ceil(1 << b))
    }

    /// Dekóduje koeficienty HF prechodu `prechod` vo všetkých skupinách (paralelne po riadkoch skupín);
    /// v poslednom prechode skupiny hneď vykreslí. `citace[g]` číta sekciu skupiny g.
    pub fn prechod(&mut self, prechod: usize, citace: Vec<&mut Citac>) -> Vysledok<()> {
        struct Uloha<'a, 'b> {
            roviny: [&'a mut [f32]; 3],
            skupiny: Vec<(usize, &'a mut Citac<'b>, &'a mut Option<KoefSkupiny>)>,
            vysledok: Vysledok<()>,
        }
        let posledny = prechod + 1 == self.prechody.len();
        let (sx, w) = (self.roz.skup_x, self.roz.w);
        if citace.len() != self.roz.skupin() || prechod >= self.prechody.len() {
            return Err(chyba("nesprávny počet skupín prechodu"));
        }
        let _ = w;
        let riadok = [0, 1, 2].map(|c| self.pw[c] * (256 >> self.sh[c].1));
        let mut xyb = std::mem::take(&mut self.xyb);
        let mut koef = std::mem::take(&mut self.koef);
        let mut ulohy = Vec::with_capacity(self.roz.skup_y);
        {
            let mut cit = citace.into_iter();
            let mut kf = koef.iter_mut();
            let [r0, r1, r2] = &mut xyb;
            for ((a, b), c) in
                r0.chunks_mut(riadok[0]).zip(r1.chunks_mut(riadok[1])).zip(r2.chunks_mut(riadok[2]))
            {
                let mut sk = Vec::with_capacity(sx);
                for _ in 0..sx {
                    let g = ulohy.len() * sx + sk.len();
                    match (cit.next(), kf.next()) {
                        (Some(ci), Some(k)) => sk.push((g, ci, k)),
                        _ => return Err(chyba("nesprávny počet skupín prechodu")),
                    }
                }
                ulohy.push(Uloha { roviny: [a, b, c], skupiny: sk, vysledok: Ok(()) });
            }
        }
        let ja = &*self;
        paralel::pre_kusy(&mut ulohy, 1, |_, u| {
            let u = &mut u[0];
            let mut f: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            for (g, c, k) in u.skupiny.iter_mut() {
                let kk = k.get_or_insert_with(|| ja.nove_koef(*g));
                if let Err(e) = ja.dekoduj_hf(c, prechod, *g, kk) {
                    u.vysledok = Err(e);
                    return;
                }
                if posledny && let Some(kk) = k.take() {
                    ja.vykresli(*g, &kk, &mut f);
                    ja.zapis(*g, &f, &mut u.roviny);
                }
            }
        });
        let chyby: Vec<Chyba> = ulohy.into_iter().filter_map(|u| u.vysledok.err()).collect();
        self.xyb = xyb;
        self.koef = koef;
        if let Some(e) = chyby.into_iter().next() {
            return Err(e);
        }
        Ok(())
    }

    /// Koeficienty HF jednej skupiny v jednom prechode (pripočítajú sa ku `koef`).
    fn dekoduj_hf(&self, c: &mut Citac, prechod: usize, g: usize, koef: &mut KoefSkupiny) -> Vysledok<()> {
        let gx = g % self.roz.skup_x;
        let gy = g / self.roz.skup_x;
        let (bx0, by0) = (gx * 32, gy * 32);
        let bw = 32.min(self.bw8 - bx0);
        let bh = 32.min(self.bh8 - by0);
        let sirka = [0, 1, 2].map(|c| self.skupina_kanala(g, c).2 * 8);
        let posun = self.posuny.get(prechod).copied().unwrap_or(0);
        let p = &self.prechody[prechod];
        let hfp = c.u(ceil_log2(self.presety));
        if hfp >= self.presety {
            return Err(chyba("neplatná predvoľba HF"));
        }
        let velkost_ctx = 495 * self.bctx.pocet;
        let zaklad_ctx = velkost_ctx * hfp as usize;
        let mut t = p.kody.citac(c);
        let bctx = &self.bctx;
        let lf_nas =
            (bctx.lf_prahy[0].len() + 1) * (bctx.lf_prahy[1].len() + 1) * (bctx.lf_prahy[2].len() + 1);
        let hf_nas = bctx.qf_prahy.len() + 1;
        let mut nenulove = [0, 1, 2].map(|c| vec![0u32; sirka[c] / 8]);
        for y in 0..bh {
            for x in 0..bw {
                let gi = (by0 + y) * self.bw8 + bx0 + x;
                let Blok::Zaciatok { typ, hf_mul } = self.bloky[gi] else { continue };
                let (w8, h8) = typ.bloky();
                let pocet_blokov = (w8 * h8) as u32;
                let log = pocet_blokov.trailing_zeros();
                let por = typ.poradie();
                let mut lf_idx = 0usize;
                for cc in [0usize, 2, 1].into_iter().filter(|_| !self.lf_zo_snimky) {
                    let prahy = &bctx.lf_prahy[cc];
                    lf_idx *= prahy.len() + 1;
                    let (a, b) = self.sh[cc];
                    let q = self.lf_kvant[cc][((by0 + y) >> b) * self.kw8[cc] + ((bx0 + x) >> a)];
                    lf_idx += prahy.iter().filter(|&&p| q > p).count();
                }
                let hf_idx = bctx.qf_prahy.iter().filter(|&&p| hf_mul > p as i32).count();
                for (ci, cc) in [1usize, 0, 2].into_iter().enumerate() {
                    let ch_idx = ci * 13 + por;
                    // podvzorkovaný kanál má koeficienty len v blokoch na párnych pozíciách
                    let (a, b) = self.sh[cc];
                    let (x, y) = (x >> a, y >> b);
                    if a != 0 || b != 0 {
                        let zarovnany = (x << a) == gi % self.bw8 - bx0 && (y << b) == gi / self.bw8 - by0;
                        let data =
                            matches!(self.bloky[(by0 + y) * self.bw8 + bx0 + x], Blok::Zaciatok { .. });
                        if !zarovnany || !data {
                            continue;
                        }
                    }
                    let idx = (ch_idx * hf_nas + hf_idx) * lf_nas + lf_idx;
                    let blok_ctx =
                        *bctx.mapa.get(idx).ok_or_else(|| chyba("kontext bloku mimo mapy"))? as usize;
                    let predikcia = if y == 0 {
                        if x == 0 { 32 } else { nenulove[cc][x - 1] }
                    } else if x == 0 {
                        nenulove[cc][x]
                    } else {
                        (nenulove[cc][x] + nenulove[cc][x - 1]).div_ceil(2)
                    };
                    let pi = if predikcia >= 8 { 4 + predikcia / 2 } else { predikcia } as usize;
                    let ctx = zaklad_ctx + blok_ctx + pi * bctx.pocet;
                    let mut nz = t.citaj(&p.kody, c, ctx)?;
                    if nz > 63 << log {
                        return Err(chyba("priveľa nenulových koeficientov"));
                    }
                    let hodnota = (nz + pocet_blokov - 1) >> log;
                    for v in nenulove[cc][x..x + w8].iter_mut() {
                        *v = hodnota;
                    }
                    if nz == 0 {
                        continue;
                    }
                    let poradie: &[(u16, u16)] = if p.poradia[por][cc].is_empty() {
                        prirodzene_poradie(por)
                    } else {
                        &p.poradia[por][cc]
                    };
                    let mut predosly = (nz <= pocet_blokov * 4) as usize;
                    let ctx_koef = zaklad_ctx + blok_ctx * 458 + 37 * bctx.pocet;
                    for (k, &(mut dx, mut dy)) in poradie[pocet_blokov as usize..].iter().enumerate() {
                        let nzi = ((nz - 1) >> log) as usize;
                        let fi = k >> log;
                        if nzi >= 63 || fi >= 63 {
                            return Err(chyba("koeficient mimo kontextov"));
                        }
                        let lokalny = (NENULOVE_CTX[nzi] as usize + FREKV_CTX[fi] as usize) * 2 + predosly;
                        if lokalny >= 458 {
                            return Err(chyba("priveľa núl v bloku koeficientov"));
                        }
                        let ctx = ctx_koef + lokalny;
                        let u = t.citaj(&p.kody, c, ctx)?;
                        if u == 0 {
                            predosly = 0;
                            continue;
                        }
                        let v = so_znamienkom(u).wrapping_shl(posun);
                        if typ.prehodit() {
                            std::mem::swap(&mut dx, &mut dy);
                        }
                        let px = x * 8 + dx as usize;
                        let py = y * 8 + dy as usize;
                        let i = py * sirka[cc] + px;
                        koef.k[cc][i] = koef.k[cc][i].wrapping_add(v);
                        predosly = 1;
                        nz -= 1;
                        if nz == 0 {
                            break;
                        }
                    }
                }
            }
        }
        t.koniec(&p.kody)?;
        Ok(())
    }

    /// Bloky kanála `c` v skupine: (x, y v blokoch kanála, druh, násobok) – pri podvzorkovaní len bloky na
    /// zarovnaných pozíciách.
    fn bloky_kanala(&self, g: usize, c: usize) -> Vec<(usize, usize, Typ, i32)> {
        let (a, b) = self.sh[c];
        let bx0 = (g % self.roz.skup_x) * 32;
        let by0 = (g / self.roz.skup_x) * 32;
        let bw = 32.min(self.bw8 - bx0);
        let bh = 32.min(self.bh8 - by0);
        let mut v = Vec::new();
        for y in 0..bh {
            for x in 0..bw {
                let Blok::Zaciatok { typ, hf_mul } = self.bloky[(by0 + y) * self.bw8 + bx0 + x] else {
                    continue;
                };
                let (sx, sy) = (x >> a, y >> b);
                if a != 0 || b != 0 {
                    if (sx << a) != x || (sy << b) != y {
                        continue;
                    }
                    if !matches!(self.bloky[(by0 + sy) * self.bw8 + bx0 + sx], Blok::Zaciatok { .. }) {
                        continue;
                    }
                }
                v.push((sx, sy, typ, hf_mul));
            }
        }
        v
    }

    /// Dekvantizácia, korelácia farieb, najnižšie frekvencie z LF a spätné transformácie jednej skupiny
    /// do rovín `f` (šírka skupiny × výška skupiny v pixeloch kanála, zarovnané na bloky).
    fn vykresli(&self, g: usize, k: &KoefSkupiny, f: &mut [Vec<f32>; 3]) {
        let qm = [
            matematika::mocnina(0.8, self.x_qm as f64 - 2.0) as f32,
            1.0,
            matematika::mocnina(0.8, self.b_qm as f64 - 2.0) as f32,
        ];
        let bloky: [Vec<(usize, usize, Typ, i32)>; 3] = [0, 1, 2].map(|c| self.bloky_kanala(g, c));
        let skup = [0, 1, 2].map(|c| self.skupina_kanala(g, c));
        for (c, fc) in f.iter_mut().enumerate() {
            fc.clear();
            fc.resize(skup[c].2 * 8 * skup[c].3 * 8, 0.0);
        }
        for c in 0..3 {
            let sirka = skup[c].2 * 8;
            for &(x, y, typ, hf_mul) in &bloky[c] {
                let (w8, h8) = typ.bloky();
                let (w, h) = (w8 * 8, h8 * 8);
                let mi = typ.matica();
                let (mw, _) = ROZMER_MATICE[mi];
                let prehod = typ.prehodit();
                let nas = 65536.0 / (self.globalna_mierka as f32 * hf_mul as f32) * qm[c];
                let m = &self.matice[mi][c];
                for yy in 0..h {
                    for xx in 0..w {
                        // matica je pre širší tvar; pri vyššom bloku prehodená
                        let mv = if prehod { m[xx * mw + yy] } else { m[yy * mw + xx] };
                        let i = (y * 8 + yy) * sirka + x * 8 + xx;
                        let q = k.k[c][i];
                        if q == 0 {
                            continue;
                        }
                        let mut v = q as f32;
                        if v.abs() <= 1.0 {
                            v *= self.kvant_bias[c];
                        } else {
                            v -= self.kvant_citatel / v;
                        }
                        f[c][i] = v * mv * nas;
                    }
                }
            }
        }
        if !self.podvzorkovane {
            // korelácia farieb HF (po dlaždiciach 64×64)
            let (bx0, by0, bw, bh) = skup[1];
            let sirka = bw * 8;
            for py in 0..bh * 8 {
                for px in 0..sirka {
                    let (fx, fy) = (bx0 * 8 + px, by0 * 8 + py);
                    let ti = (fy / 64) * self.w64 + fx / 64;
                    let kx = self.cfl_x + self.x_z_y[ti] as f32 / self.cfl_faktor;
                    let kb = self.cfl_b + self.b_z_y[ti] as f32 / self.cfl_faktor;
                    let i = py * sirka + px;
                    let yv = f[1][i];
                    f[0][i] += kx * yv;
                    f[2][i] += kb * yv;
                }
            }
        }
        // LLF a spätné transformácie
        let mut lfb = [0f32; 1024];
        for (c, fc) in f.iter_mut().enumerate() {
            let (bx0, by0, bw, _) = skup[c];
            let sirka = bw * 8;
            let lw8 = self.kw8[c];
            for &(x, y, typ, _) in &bloky[c] {
                let (w8, h8) = typ.bloky();
                let zac = y * 8 * sirka + x * 8;
                if w8 * h8 == 1 {
                    fc[zac] = self.lf[c][(by0 + y) * lw8 + bx0 + x];
                } else {
                    let lfb = &mut lfb[..w8 * h8];
                    for yy in 0..h8 {
                        for xx in 0..w8 {
                            lfb[yy * w8 + xx] = self.lf[c][(by0 + y + yy) * lw8 + bx0 + x + xx];
                        }
                    }
                    dct_2d(lfb, w8, w8, h8, false);
                    let (lw, lh) = (5 - w8.trailing_zeros(), 5 - h8.trailing_zeros());
                    for yy in 0..h8 {
                        for xx in 0..w8 {
                            fc[zac + yy * sirka + xx] =
                                lfb[yy * w8 + xx] / (MIERKA_F[yy << lh] * MIERKA_F[xx << lw]);
                        }
                    }
                }
                let blok = &mut fc[zac..];
                if w8 * h8 == 1 {
                    transformacia_mala(typ, blok, sirka);
                } else {
                    dct_2d(blok, sirka, w8 * 8, h8 * 8, true);
                }
            }
        }
    }

    /// Zapíše vykreslenú skupinu do riadku skupín (bez zarovnania na bloky 8×8).
    fn zapis(&self, g: usize, f: &[Vec<f32>; 3], roviny: &mut [&mut [f32]; 3]) {
        for (c, (fc, r)) in f.iter().zip(roviny.iter_mut()).enumerate() {
            let (bx0, _, bw, _) = self.skupina_kanala(g, c);
            let (w, h) = (self.pw[c], self.ph[c]);
            let sirka = bw * 8;
            let x0 = bx0 * 8;
            if x0 >= w {
                continue;
            }
            let sw = sirka.min(w - x0);
            let riadkov = (r.len() / w).min(h).min(fc.len() / sirka.max(1));
            for py in 0..riadkov {
                let z = py * w + x0;
                r[z..z + sw].copy_from_slice(&fc[py * sirka..py * sirka + sw]);
            }
        }
    }

    /// Dokončí snímku: filtre obnovy; vráti roviny X, Y, B.
    pub fn dokonci(mut self, h: &HlavickaSnimky) -> Vysledok<[Vec<f32>; 3]> {
        if self.koef.iter().any(|s| s.is_some()) {
            return Err(chyba("neúplné prechody skupín"));
        }
        let (w, hh) = (self.roz.w, self.roz.h);
        let mut obraz = std::mem::take(&mut self.xyb);
        if self.podvzorkovane {
            // zväčšenie podvzorkovaných kanálov ako v JPEG (váhy 3/4 a 1/4)
            for (c, o) in obraz.iter_mut().enumerate() {
                let (a, b) = self.sh[c];
                let (mut cw, ch) = (self.pw[c], self.ph[c]);
                if a == 1 {
                    *o = zvacsi_jpeg_vodorovne(o, cw, ch, w);
                    cw = w;
                }
                if b == 1 {
                    *o = zvacsi_jpeg_zvisle(o, cw, ch, hh);
                }
            }
            return Ok(obraz);
        }
        let sigma = Sigma::Bloky { hodnoty: &self.sigma, w8: self.bw8 };
        obnov(&mut obraz, w, hh, h, sigma);
        Ok(obraz)
    }
}

/// Vodorovné zdvojenie riadkov (výstup 3/4 · stred + 1/4 · sused, okraje opakované) na šírku `ciel`.
fn zvacsi_jpeg_vodorovne(d: &[f32], w: usize, h: usize, ciel: usize) -> Vec<f32> {
    let mut v = vec![0f32; ciel * h];
    for y in 0..h {
        let r = &d[y * w..(y + 1) * w];
        for x in 0..ciel {
            let i = x / 2;
            let s = r[i.min(w - 1)];
            let sused = if x % 2 == 0 { r[i.saturating_sub(1)] } else { r[(i + 1).min(w - 1)] };
            v[y * ciel + x] = 0.25 * sused + 0.75 * s;
        }
    }
    v
}

/// Zvislé zdvojenie stĺpcov na výšku `ciel`.
fn zvacsi_jpeg_zvisle(d: &[f32], w: usize, h: usize, ciel: usize) -> Vec<f32> {
    let mut v = vec![0f32; w * ciel];
    for y in 0..ciel {
        let i = (y / 2).min(h - 1);
        let j = if y % 2 == 0 { i.saturating_sub(1) } else { (i + 1).min(h - 1) };
        for x in 0..w {
            let (s, sused) = (d[i * w + x], d[j * w + x]);
            v[y * w + x] = 0.75 * s + 0.25 * sused;
        }
    }
    v
}

/// Zdroj parametra sigma pre EPF.
pub(super) enum Sigma<'a> {
    /// Po blokoch 8×8 (VarDCT).
    Bloky { hodnoty: &'a [f32], w8: usize },
    /// Jedna hodnota (Modular).
    Stala(f32),
}

/// Filtre obnovy (Gabor, EPF) nad tromi rovinami `w × hh`.
pub(super) fn obnov(obraz: &mut [Vec<f32>; 3], w: usize, hh: usize, h: &HlavickaSnimky, sigma: Sigma) {
    let f = &h.filtre;
    if f.gabor || f.epf_iteracie > 0 {
        let mut p: Vec<[f32; 3]> = (0..w * hh).map(|i| [obraz[0][i], obraz[1][i], obraz[2][i]]).collect();
        if f.gabor {
            let pod = podloz(|i| p[i], w, hh, 1);
            p = gabor(&pod, w, hh, f.gabor_vahy);
        }
        if f.epf_iteracie > 0 {
            let stala;
            let (hodnoty, w8): (&[f32], usize) = match sigma {
                Sigma::Bloky { hodnoty, w8 } => (hodnoty, w8),
                Sigma::Stala(s) => {
                    stala = vec![s; w.div_ceil(8) * hh.div_ceil(8)];
                    (&stala, w.div_ceil(8))
                }
            };
            p = epf(p, w, hh, w8, hodnoty, f);
        }
        for (i, v) in p.iter().enumerate() {
            for c in 0..3 {
                obraz[c][i] = v[c];
            }
        }
    }
}

/// Zväčší rovinu faktorom `k` a oreže ju na `cw × ch`.
pub(super) fn zvacsi_rovinu(
    o: &[f32],
    w: usize,
    hh: usize,
    k: usize,
    m: &Metadata,
    cw: usize,
    ch: usize,
) -> Vec<f32> {
    let i = k.trailing_zeros() as usize - 1;
    let predvolene: &[f32] = match k {
        2 => &zvacsenie::VAHY_2,
        4 => &zvacsenie::VAHY_4,
        _ => &zvacsenie::VAHY_8,
    };
    let vahy = m.vahy_zvacsenia[i].as_deref().unwrap_or(predvolene);
    let v = zvacsenie::zvacsi(o, w, hh, k, vahy);
    let mut orez = Vec::with_capacity(cw * ch);
    for y in 0..ch.min(hh * k) {
        orez.extend_from_slice(&v[y * w * k..y * w * k + cw.min(w * k)]);
    }
    orez
}

/// XYB → lineárne RGB (na mieste).
pub(super) fn xyb_na_rgb(mut obraz: [Vec<f32>; 3], m: &Metadata) -> [Vec<f32>; 3] {
    let mierka = 255.0 / m.cielovy_jas;
    let bias = m.opsin_bias;
    let kor: [f32; 3] = bias.map(kubicka_odmocnina);
    let [ox, oy, ob] = &mut obraz;
    for ((x, y), b) in ox.iter_mut().zip(oy.iter_mut()).zip(ob.iter_mut()) {
        let gamma = [*y + *x, *y - *x, *b];
        let mut zmes = [0f32; 3];
        for c in 0..3 {
            let t = gamma[c] - kor[c];
            zmes[c] = (t * t * t + bias[c]) * mierka;
        }
        let o = &m.opsin;
        *x = o[0][0] * zmes[0] + o[0][1] * zmes[1] + o[0][2] * zmes[2];
        *y = o[1][0] * zmes[0] + o[1][1] * zmes[1] + o[1][2] * zmes[2];
        *b = o[2][0] * zmes[0] + o[2][1] * zmes[1] + o[2][2] * zmes[2];
    }
    obraz
}

/// Kubická odmocnina so znamienkom (mocnina a dva Newtonove kroky – deterministicky).
fn kubicka_odmocnina(x: f32) -> f32 {
    if x == 0.0 {
        return 0.0;
    }
    let a = (x as f64).abs();
    let mut y = matematika::mocnina(a, 1.0 / 3.0);
    for _ in 0..2 {
        y -= (y * y * y - a) / (3.0 * y * y);
    }
    (if x < 0.0 { -y } else { y }) as f32
}

const FREKV_CTX: [u8; 63] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15, 16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 21, 21,
    22, 22, 23, 23, 23, 23, 24, 24, 24, 24, 25, 25, 25, 25, 26, 26, 26, 26, 27, 27, 27, 27, 28, 28, 28, 28,
    29, 29, 29, 29, 30, 30, 30, 30,
];
const NENULOVE_CTX: [u8; 63] = [
    0, 31, 62, 62, 93, 93, 93, 93, 123, 123, 123, 123, 152, 152, 152, 152, 152, 152, 152, 152, 180, 180, 180,
    180, 180, 180, 180, 180, 180, 180, 180, 180, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206,
    206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206, 206,
];

/// Zrkadlenie súradnice do rozsahu 0..n (okraj sa opakuje).
#[inline]
fn zrkadli(mut i: isize, n: usize) -> usize {
    loop {
        if i < 0 {
            i = -(i + 1);
        } else if i as usize >= n {
            i = 2 * n as isize - i - 1;
        } else {
            return i as usize;
        }
    }
}

/// Prepletené roviny s okrajom `p` zrkadlených pixelov (riadok má `w + 2p` pixelov).
fn podloz(z: impl Fn(usize) -> [f32; 3], w: usize, h: usize, p: usize) -> Vec<[f32; 3]> {
    let pw = w + 2 * p;
    let stlpce: Vec<usize> = (0..pw).map(|x| zrkadli(x as isize - p as isize, w)).collect();
    let mut v = Vec::with_capacity(pw * (h + 2 * p));
    for y in 0..h + 2 * p {
        let sy = zrkadli(y as isize - p as isize, h);
        v.extend(stlpce.iter().map(|&sx| z(sy * w + sx)));
    }
    v
}

/// Gaborov filter: 3×3, stred 1, strany w1, uhlopriečky w2, normované; okraje zrkadlené.
fn gabor(src: &[[f32; 3]], w: usize, h: usize, vahy: [(f32, f32); 3]) -> Vec<[f32; 3]> {
    let pw = w + 2;
    let norma = vahy.map(|(w1, w2)| 1.0 / (1.0 + 4.0 * w1 + 4.0 * w2));
    let mut von = vec![[0f32; 3]; w * h];
    paralel::pre_kusy(&mut von, w * 16, |i, kus| {
        for (j, o) in kus.iter_mut().enumerate() {
            let (y, x) = ((i * w * 16 + j) / w, (i * w * 16 + j) % w);
            let b = (y + 1) * pw + x + 1;
            for c in 0..3 {
                let (w1, w2) = vahy[c];
                let g = |k: usize| src[k][c];
                let strana = g(b - pw) + g(b - 1) + g(b + 1) + g(b + pw);
                let diag = g(b - pw - 1) + g(b - pw + 1) + g(b + pw - 1) + g(b + pw + 1);
                o[c] = (g(b) + strana * w1 + diag * w2) * norma[c];
            }
        }
    });
    von
}

type Posuny = &'static [(isize, isize)];

/// Filter zachovávajúci hrany (EPF), 1–3 kroky nad prepletenými rovinami (riadky paralelne).
fn epf(
    src: Vec<[f32; 3]>,
    w: usize,
    h: usize,
    w8: usize,
    sigma: &[f32],
    f: &super::hlavicky::Filtre,
) -> Vec<[f32; 3]> {
    const JADRO_2: [(isize, isize); 12] = [
        (0, -2),
        (-1, -1),
        (0, -1),
        (1, -1),
        (-2, 0),
        (-1, 0),
        (1, 0),
        (2, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
        (0, 2),
    ];
    const JADRO_1: [(isize, isize); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
    const VZD_0: [(isize, isize); 5] = [(0, -1), (1, 0), (0, 0), (-1, 0), (0, 1)];
    const VZD_1: [(isize, isize); 5] = [(0, -1), (0, 0), (0, 1), (-1, 0), (1, 0)];
    const VZD_2: [(isize, isize); 1] = [(0, 0)];
    const P: usize = 3;
    let pw = w + 2 * P;
    let kroky: &[usize] = match f.epf_iteracie {
        1 => &[1],
        2 => &[1, 2],
        _ => &[0, 1, 2],
    };
    let mut src = podloz(|i| src[i], w, h, P);
    let mut von = vec![[0f32; 3]; w * h];
    for (n, &krok) in kroky.iter().enumerate() {
        let (jadro, vzd): (Posuny, Posuny) = match krok {
            0 => (&JADRO_2, &VZD_0),
            1 => (&JADRO_1, &VZD_1),
            _ => (&JADRO_1, &VZD_2),
        };
        let o = |(dx, dy): (isize, isize)| dy * pw as isize + dx;
        let jadro: Vec<isize> = jadro.iter().map(|&d| o(d)).collect();
        let vzd: Vec<isize> = vzd.iter().map(|&d| o(d)).collect();
        let nasobok = match krok {
            0 => f.epf_sigma0,
            2 => f.epf_sigma2,
            _ => 1.0,
        };
        let sc = f.epf_kanaly;
        let zdroj = &src;
        paralel::pre_kusy(&mut von, w * 8, |i, kus| {
            for (j, out) in kus.iter_mut().enumerate() {
                let (y, x) = ((i * w * 8 + j) / w, (i * w * 8 + j) % w);
                let b = ((y + P) * pw + x + P) as isize;
                let stred = zdroj[b as usize];
                let s = sigma[(y / 8) * w8 + x / 8];
                if s < 0.3 {
                    *out = stred;
                    continue;
                }
                let mut sm = nasobok;
                if (y + 1) & 0b110 == 0 || x % 8 == 0 || x % 8 == 7 {
                    sm *= f.epf_okraj;
                }
                let neg_inv = 6.6 * (std::f32::consts::FRAC_1_SQRT_2 - 1.0) / s * sm;
                let mut suma_vah = 1.0f32;
                let mut suma = stred;
                for &k in &jadro {
                    let mut a = [0f32; 3];
                    for &d in &vzd {
                        let p1 = zdroj[(b + k + d) as usize];
                        let p0 = zdroj[(b + d) as usize];
                        for c in 0..3 {
                            a[c] += (p1[c] - p0[c]).abs();
                        }
                    }
                    let vzdial = sc[0] * a[0] + sc[1] * a[1] + sc[2] * a[2];
                    let vaha = (1.0 + vzdial * neg_inv).max(0.0);
                    suma_vah += vaha;
                    let pk = zdroj[(b + k) as usize];
                    for c in 0..3 {
                        suma[c] += vaha * pk[c];
                    }
                }
                *out = suma.map(|v| v / suma_vah);
            }
        });
        if n + 1 < kroky.len() {
            src = podloz(|i| von[i], w, h, P);
        }
    }
    von
}
