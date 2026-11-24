//! Prenosové funkcie (kódovanie ↔ lineárne svetlo): sRGB, BT.709, gama, PQ, HLG, ProPhoto, ACEScc, ACEScct
//! a krivky profilov ICC (gama, tabuľka, parametrické krivky typov 0–4).
//!
//! Výpočty sú deterministické (mocniny a logaritmy z jadra). Záporné hodnoty sa pri funkciách okolo nuly
//! zrkadlia (rozšírený rozsah), logaritmické a PQ/HLG ich orežú.

use orchidea_jadro::matematika::{exp, ln, log2, mocnina};

/// Znamienko zachovávajúca funkcia: `f(−x) = −f(x)`.
#[inline]
fn zrkadlo(x: f64, f: impl Fn(f64) -> f64) -> f64 {
    if x < 0.0 { -f(-x) } else { f(x) }
}

/// Krivka profilu ICC (`curv` alebo `para`), definovaná na 0–1.
#[derive(Debug, Clone, PartialEq)]
pub enum Krivka {
    /// Y = X.
    Identita,
    /// Y = X^γ.
    Gama(f64),
    /// Rovnomerne rozložené vzorky na 0–1 s lineárnou interpoláciou.
    Tabulka(Vec<f64>),
    /// Parametrická krivka ICC typu 0–4 s parametrami g, a, b, c, d, e, f.
    Parametricka { typ: u16, p: [f64; 7] },
}

impl Krivka {
    /// Hodnota krivky (dopredu – zo zariadenia k lineárnemu svetlu pri TRC).
    pub fn hodnota(&self, x: f64) -> f64 {
        match self {
            Krivka::Identita => x,
            Krivka::Gama(g) => zrkadlo(x, |x| mocnina(x, *g)),
            Krivka::Tabulka(t) => tabulka(t, x),
            Krivka::Parametricka { typ, p } => {
                let [g, a, b, c, d, e, f] = *p;
                let moc = |v: f64| if v > 0.0 { mocnina(v, g) } else { 0.0 };
                match typ {
                    0 => zrkadlo(x, |x| mocnina(x, g)),
                    1 => {
                        if x >= -b / a {
                            moc(a * x + b)
                        } else {
                            0.0
                        }
                    }
                    2 => {
                        if x >= -b / a {
                            moc(a * x + b) + c
                        } else {
                            c
                        }
                    }
                    3 => {
                        if x >= d {
                            moc(a * x + b)
                        } else {
                            c * x
                        }
                    }
                    _ => {
                        if x >= d {
                            moc(a * x + b) + e
                        } else {
                            c * x + f
                        }
                    }
                }
            }
        }
    }

    /// Inverzná hodnota (od lineárneho svetla ku kódovaniu).
    pub fn inverzna(&self, y: f64) -> f64 {
        match self {
            Krivka::Identita => y,
            Krivka::Gama(g) => zrkadlo(y, |y| mocnina(y, 1.0 / g)),
            Krivka::Tabulka(t) => tabulka_inverzna(t, y),
            Krivka::Parametricka { typ, p } => {
                let [g, a, b, c, d, e, f] = *p;
                let odm = |v: f64| if v > 0.0 { mocnina(v, 1.0 / g) } else { 0.0 };
                match typ {
                    0 => zrkadlo(y, |y| mocnina(y, 1.0 / g)),
                    1 => {
                        if y > 0.0 {
                            (odm(y) - b) / a
                        } else {
                            -b / a
                        }
                    }
                    2 => {
                        if y > c {
                            (odm(y - c) - b) / a
                        } else {
                            -b / a
                        }
                    }
                    3 => {
                        let zlom = c * d;
                        if y >= zlom || c == 0.0 { (odm(y) - b) / a } else { y / c }
                    }
                    _ => {
                        let zlom = c * d + f;
                        if y >= zlom || c == 0.0 { (odm(y - e) - b) / a } else { (y - f) / c }
                    }
                }
            }
        }
    }
}

fn tabulka(t: &[f64], x: f64) -> f64 {
    match t.len() {
        0 => x,
        1 => t[0],
        n => {
            let p = x.clamp(0.0, 1.0) * (n - 1) as f64;
            let i = (p as usize).min(n - 2);
            let z = p - i as f64;
            t[i] + (t[i + 1] - t[i]) * z
        }
    }
}

/// Spätná interpolácia rastúcej (alebo klesajúcej) tabuľky.
fn tabulka_inverzna(t: &[f64], y: f64) -> f64 {
    let n = t.len();
    if n < 2 {
        return y;
    }
    let rastuca = t[n - 1] >= t[0];
    let (lo, hi) = if rastuca { (t[0], t[n - 1]) } else { (t[n - 1], t[0]) };
    if y <= lo {
        return if rastuca { 0.0 } else { 1.0 };
    }
    if y >= hi {
        return if rastuca { 1.0 } else { 0.0 };
    }
    // binárne hľadanie úseku (nemonotónne úseky sa vyrovnajú prvým výskytom)
    let (mut a, mut b) = (0usize, n - 1);
    while b - a > 1 {
        let m = (a + b) / 2;
        if (t[m] <= y) == rastuca {
            a = m;
        } else {
            b = m;
        }
    }
    let (ya, yb) = (t[a], t[b]);
    let z = if yb != ya { (y - ya) / (yb - ya) } else { 0.0 };
    (a as f64 + z) / (n - 1) as f64
}

// ---------------------------------------------------------------- prenosové funkcie

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;
const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 0.284_668_92;
const HLG_C: f64 = 0.559_910_73;
const ACES_POLOVICNE_MAX: f64 = 65504.0;
/// Hodnota OETF BT.709 v bode 0,018 mocninovou vetvou (1,099 · 0,018^0,45 − 0,099). Kódy medzi 0,081 a týmto
/// bodom OETF nikdy nevydá; inverzia ich priradí lineárnemu úseku.
const BT709_ZLOM: f64 = 0.081_247_944_035_140_46;

/// Prenosová funkcia (kódovanie ↔ lineárne svetlo).
#[derive(Debug, Clone, PartialEq)]
pub enum Prenos {
    /// Bez kódovania.
    Linearny,
    /// IEC 61966-2-1 (sRGB, Display P3).
    Srgb,
    /// BT.709 / BT.2020 (OETF fotoaparátu a jej inverzia).
    Bt709,
    /// Čistá mocnina: lineárne = kódované^γ (napr. 2,2; 2,6 pre DCI; 563/256 pre Adobe RGB).
    Gama(f64),
    /// SMPTE ST 2084 (PQ); lineárne 1,0 = 10 000 cd/m².
    Pq,
    /// BT.2100 HLG (OETF scény a jej inverzia, bez OOTF).
    Hlg,
    /// ROMM (ProPhoto RGB).
    ProPhoto,
    /// ACEScc (logaritmické, S-2014-003).
    Acescc,
    /// ACEScct (logaritmické s lineárnou pätou, S-2016-001).
    Acescct,
    /// Krivka profilu ICC.
    Krivka(Krivka),
}

impl Prenos {
    /// Z kódovanej hodnoty na lineárne svetlo.
    pub fn do_linearneho(&self, v: f64) -> f64 {
        match self {
            Prenos::Linearny => v,
            Prenos::Srgb => {
                zrkadlo(v, |v| if v <= 0.04045 { v / 12.92 } else { mocnina((v + 0.055) / 1.055, 2.4) })
            }
            Prenos::Bt709 => {
                zrkadlo(
                    v,
                    |v| {
                        if v < BT709_ZLOM { v / 4.5 } else { mocnina((v + 0.099) / 1.099, 1.0 / 0.45) }
                    },
                )
            }
            Prenos::Gama(g) => zrkadlo(v, |v| mocnina(v, *g)),
            Prenos::Pq => {
                let e = mocnina(v.max(0.0), 1.0 / PQ_M2);
                let n = (e - PQ_C1).max(0.0);
                let d = PQ_C2 - PQ_C3 * e;
                if d <= 0.0 { 1.0 } else { mocnina(n / d, 1.0 / PQ_M1) }
            }
            Prenos::Hlg => {
                let v = v.max(0.0);
                if v <= 0.5 { v * v / 3.0 } else { (exp((v - HLG_C) / HLG_A) + HLG_B) / 12.0 }
            }
            Prenos::ProPhoto => zrkadlo(v, |v| if v < 16.0 / 512.0 { v / 16.0 } else { mocnina(v, 1.8) }),
            Prenos::Acescc => {
                if v < (9.72 - 15.0) / 17.52 {
                    (mocnina(2.0, v * 17.52 - 9.72) - mocnina(2.0, -16.0)) * 2.0
                } else if v < (log2(ACES_POLOVICNE_MAX) + 9.72) / 17.52 {
                    mocnina(2.0, v * 17.52 - 9.72)
                } else {
                    ACES_POLOVICNE_MAX
                }
            }
            Prenos::Acescct => {
                if v <= 0.155_251_141_552_511 {
                    (v - 0.072_905_534_195_835_5) / 10.540_237_741_654_5
                } else if v < (log2(ACES_POLOVICNE_MAX) + 9.72) / 17.52 {
                    mocnina(2.0, v * 17.52 - 9.72)
                } else {
                    ACES_POLOVICNE_MAX
                }
            }
            Prenos::Krivka(k) => k.hodnota(v),
        }
    }

    /// Z lineárneho svetla na kódovanú hodnotu.
    pub fn z_linearneho(&self, l: f64) -> f64 {
        match self {
            Prenos::Linearny => l,
            Prenos::Srgb => {
                zrkadlo(
                    l,
                    |l| {
                        if l <= 0.003_130_8 { 12.92 * l } else { 1.055 * mocnina(l, 1.0 / 2.4) - 0.055 }
                    },
                )
            }
            Prenos::Bt709 => {
                zrkadlo(l, |l| if l < 0.018 { 4.5 * l } else { 1.099 * mocnina(l, 0.45) - 0.099 })
            }
            Prenos::Gama(g) => zrkadlo(l, |l| mocnina(l, 1.0 / g)),
            Prenos::Pq => {
                let y = mocnina(l.max(0.0), PQ_M1);
                mocnina((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2)
            }
            Prenos::Hlg => {
                let l = l.max(0.0);
                if l <= 1.0 / 12.0 { (3.0 * l).sqrt_det() } else { HLG_A * ln(12.0 * l - HLG_B) + HLG_C }
            }
            Prenos::ProPhoto => {
                zrkadlo(l, |l| if l < 1.0 / 512.0 { 16.0 * l } else { mocnina(l, 1.0 / 1.8) })
            }
            Prenos::Acescc => {
                if l <= 0.0 {
                    (-16.0 + 9.72) / 17.52
                } else if l < mocnina(2.0, -15.0) {
                    (log2(mocnina(2.0, -16.0) + l * 0.5) + 9.72) / 17.52
                } else {
                    (log2(l) + 9.72) / 17.52
                }
            }
            Prenos::Acescct => {
                if l <= 0.007_812_5 {
                    10.540_237_741_654_5 * l + 0.072_905_534_195_835_5
                } else {
                    (log2(l) + 9.72) / 17.52
                }
            }
            Prenos::Krivka(k) => k.inverzna(l),
        }
    }

    /// Krivka ICC, ktorá funkciu vyjadruje presne (ak existuje).
    pub fn ako_krivka(&self) -> Option<Krivka> {
        Some(match self {
            Prenos::Linearny => Krivka::Identita,
            Prenos::Srgb => Krivka::Parametricka {
                typ: 3,
                p: [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045, 0.0, 0.0],
            },
            Prenos::Bt709 => Krivka::Parametricka {
                typ: 3,
                p: [1.0 / 0.45, 1.0 / 1.099, 0.099 / 1.099, 1.0 / 4.5, BT709_ZLOM, 0.0, 0.0],
            },
            Prenos::Gama(g) => Krivka::Gama(*g),
            Prenos::ProPhoto => {
                Krivka::Parametricka { typ: 3, p: [1.8, 1.0, 0.0, 1.0 / 16.0, 16.0 / 512.0, 0.0, 0.0] }
            }
            Prenos::Krivka(k) => k.clone(),
            _ => return None,
        })
    }
}

/// Odmocnina z jadra (deterministická) ako metóda.
trait Odmocnina {
    fn sqrt_det(self) -> f64;
}

impl Odmocnina for f64 {
    fn sqrt_det(self) -> f64 {
        orchidea_jadro::matematika::odmocnina(self)
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn tam_a_spat() {
        let funkcie = [
            Prenos::Srgb,
            Prenos::Bt709,
            Prenos::Gama(2.2),
            Prenos::Pq,
            Prenos::Hlg,
            Prenos::ProPhoto,
            Prenos::Acescc,
            Prenos::Acescct,
        ];
        for f in &funkcie {
            for i in 1..100 {
                let l = i as f64 / 100.0;
                let v = f.z_linearneho(l);
                assert!((f.do_linearneho(v) - l).abs() < 1e-9, "{f:?} {l}");
            }
        }
    }

    #[test]
    fn krivky_icc_ako_funkcie() {
        for f in [Prenos::Srgb, Prenos::Bt709, Prenos::ProPhoto] {
            let k = f.ako_krivka().unwrap();
            for i in 0..=100 {
                let v = i as f64 / 100.0;
                assert!((k.hodnota(v) - f.do_linearneho(v)).abs() < 1e-9, "{f:?} {v}");
                assert!((k.inverzna(k.hodnota(v)) - v).abs() < 1e-9, "{f:?} {v}");
            }
        }
        let t = Krivka::Tabulka((0..=256).map(|i| (i as f64 / 256.0).powi(2)).collect());
        assert!((t.inverzna(t.hodnota(0.3)) - 0.3).abs() < 1e-9);
    }
}
