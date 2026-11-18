//! Deterministická matematika – rovnaké bity na každej platforme.
//!
//! Funkcie `exp`, `ln`, `sin`, `cos`, `tanh`… zo štandardnej knižnice volajú matematickú knižnicu
//! operačného systému a tá sa na Windows, Linuxe, macOS, Androide a webe môže líšiť v posledných bitoch.
//! Pre zásadu A9 (všetko je opakovateľné) a pre bitovo presné odtlačky preto jadro používa vlastné
//! implementácie postavené len na základných operáciách IEEE 754 (+, −, ×, ÷, √, zaokrúhlenie), ktoré sú
//! všade presne rovnaké. Presnosť je do niekoľkých jednotiek posledného miesta (ulp) v type `f64`.

#![allow(clippy::excessive_precision)] // konštanty sú zapísané presne podľa ich odvodenia

/// ln 2 rozdelený na dve časti (horná časť má dosť núl na konci, aby bol súčin s k presný).
const LN2_HORNA: f64 = 6.931_471_803_691_238_164_90e-01;
const LN2_DOLNA: f64 = 1.908_214_929_270_587_700_02e-10;
const LOG2_E: f64 = std::f64::consts::LOG2_E;
const SQRT2: f64 = std::f64::consts::SQRT_2;

/// π/2 rozdelené na tri časti pre presné zmenšenie argumentu.
const PI2_1: f64 = 1.570_796_326_734_125_614_17e+00;
const PI2_2: f64 = 6.077_100_506_303_965_976_60e-11;
const PI2_3: f64 = 2.022_266_248_711_166_455_80e-21;
const DVE_PI: f64 = std::f64::consts::FRAC_2_PI;

/// Koeficienty 1/n! pre n = 0..=20 (počítané pri preklade, deterministicky).
const FAKTORIALY: [f64; 21] = {
    let mut k = [0.0f64; 21];
    let mut f = 1.0f64;
    let mut n = 0;
    while n < 21 {
        if n > 0 {
            f *= n as f64;
        }
        k[n] = 1.0 / f;
        n += 1;
    }
    k
};

/// 2^k pre ľubovoľné k (aj mimo normálneho rozsahu).
fn mocnina_dvoch(k: i32) -> f64 {
    if (-1022..=1023).contains(&k) {
        f64::from_bits(((k + 1023) as u64) << 52)
    } else if k > 1023 {
        f64::INFINITY
    } else if k >= -1074 {
        f64::from_bits(1u64 << (k + 1074))
    } else {
        0.0
    }
}

fn krat_mocnina_dvoch(x: f64, k: i32) -> f64 {
    if (-1022..=1023).contains(&k) {
        x * mocnina_dvoch(k)
    } else if k > 1023 {
        x * mocnina_dvoch(1023) * mocnina_dvoch(k - 1023)
    } else {
        x * mocnina_dvoch(k + 1000) * mocnina_dvoch(-1000)
    }
}

/// Zaokrúhlenie na najbližšie celé číslo (polovice od nuly) – len základné operácie.
fn zaokruhli(x: f64) -> f64 {
    x.round()
}

/// e^r pre malé |r| (≤ 0,35) Hornerovou schémou Taylorovho radu stupňa 13.
fn exp_male(r: f64) -> f64 {
    let mut p = FAKTORIALY[13];
    let mut n = 12;
    loop {
        p = p * r + FAKTORIALY[n];
        if n == 0 {
            return p;
        }
        n -= 1;
    }
}

/// e^x.
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x > 709.782_712_893_384 {
        return f64::INFINITY;
    }
    if x < -745.133_219_101_941_1 {
        return 0.0;
    }
    let k = zaokruhli(x * LOG2_E);
    let r = (x - k * LN2_HORNA) - k * LN2_DOLNA;
    krat_mocnina_dvoch(exp_male(r), k as i32)
}

/// e^x − 1, presne aj pre malé x.
pub fn expm1(x: f64) -> f64 {
    if x.abs() < 0.5 {
        // Taylorov rad bez člena 1, stupeň 18.
        let mut p = FAKTORIALY[18];
        let mut n = 17;
        while n >= 1 {
            p = p * x + FAKTORIALY[n];
            n -= 1;
        }
        p * x
    } else {
        exp(x) - 1.0
    }
}

/// Prirodzený logaritmus.
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    let (mut x, mut e) = (x, 0i32);
    if x < f64::MIN_POSITIVE {
        x *= mocnina_dvoch(54);
        e -= 54;
    }
    let bity = x.to_bits();
    e += ((bity >> 52) & 0x7ff) as i32 - 1023;
    let mut m = f64::from_bits((bity & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if m > SQRT2 {
        m *= 0.5;
        e += 1;
    }
    let f = m - 1.0;
    let s = f / (2.0 + f);
    let s2 = s * s;
    // 2·atanh(s) = 2(s + s³/3 + s⁵/5 + …), 13 členov stačí pre |s| ≤ 0,1716.
    let mut p = 1.0 / 25.0;
    let mut n = 23i32;
    while n >= 1 {
        p = p * s2 + 1.0 / n as f64;
        n -= 2;
    }
    let ln_m = 2.0 * s * p;
    let ef = e as f64;
    ef * LN2_HORNA + (ln_m + ef * LN2_DOLNA)
}

/// Logaritmus so základom 2.
pub fn log2(x: f64) -> f64 {
    ln(x) * LOG2_E
}

/// Logaritmus so základom 10.
pub fn log10(x: f64) -> f64 {
    ln(x) * std::f64::consts::LOG10_E
}

fn sin_male(r: f64) -> f64 {
    let r2 = r * r;
    let mut p = -FAKTORIALY[19];
    let mut n = 17;
    let mut znamienko = 1.0;
    loop {
        p = p * r2 + znamienko * FAKTORIALY[n];
        znamienko = -znamienko;
        if n == 1 {
            return p * r;
        }
        n -= 2;
    }
}

fn cos_male(r: f64) -> f64 {
    let r2 = r * r;
    let mut p = FAKTORIALY[20];
    let mut n = 18;
    let mut znamienko = -1.0;
    loop {
        p = p * r2 + znamienko * FAKTORIALY[n];
        znamienko = -znamienko;
        if n == 0 {
            return p;
        }
        n -= 2;
    }
}

/// Zmenšenie argumentu: x = k·π/2 + r, |r| ≤ π/4. Vracia (r, k mod 4).
fn zmensi(x: f64) -> (f64, u32) {
    let k = zaokruhli(x * DVE_PI);
    let r = ((x - k * PI2_1) - k * PI2_2) - k * PI2_3;
    let kv = (k.rem_euclid(4.0)) as u32;
    (r, kv)
}

/// Sínus (presný pre |x| do ~10⁶, deterministický vždy).
pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let (r, k) = zmensi(x);
    match k {
        0 => sin_male(r),
        1 => cos_male(r),
        2 => -sin_male(r),
        _ => -cos_male(r),
    }
}

/// Kosínus.
pub fn cos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let (r, k) = zmensi(x);
    match k {
        0 => cos_male(r),
        1 => -sin_male(r),
        2 => -cos_male(r),
        _ => sin_male(r),
    }
}

/// Tangens.
pub fn tan(x: f64) -> f64 {
    sin(x) / cos(x)
}

/// Hyperbolický tangens.
pub fn tanh(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x > 22.0 {
        return 1.0;
    }
    if x < -22.0 {
        return -1.0;
    }
    let e = expm1(2.0 * x);
    e / (e + 2.0)
}

/// Logistická funkcia 1 / (1 + e^−x).
pub fn sigmoida(x: f64) -> f64 {
    1.0 / (1.0 + exp(-x))
}

/// Mocnina x^y. Pre celé |y| ≤ 1024 počíta presne opakovaným násobením.
pub fn mocnina(x: f64, y: f64) -> f64 {
    if y == 0.0 {
        return 1.0;
    }
    if y.fract() == 0.0 && y.abs() <= 1024.0 {
        let mut n = y.abs() as u32;
        let mut zaklad = x;
        let mut v = 1.0;
        while n > 0 {
            if n & 1 == 1 {
                v *= zaklad;
            }
            zaklad *= zaklad;
            n >>= 1;
        }
        return if y < 0.0 { 1.0 / v } else { v };
    }
    if x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return if y > 0.0 { 0.0 } else { f64::INFINITY };
    }
    exp(y * ln(x))
}

/// Odmocnina (IEEE 754 ju definuje presne zaokrúhlenú – je všade rovnaká).
pub fn odmocnina(x: f64) -> f64 {
    x.sqrt()
}

/// Verzie pre f32 – výpočet v f64 a jedno zaokrúhlenie.
pub mod f32v {
    pub fn exp(x: f32) -> f32 {
        super::exp(x as f64) as f32
    }
    pub fn ln(x: f32) -> f32 {
        super::ln(x as f64) as f32
    }
    pub fn tanh(x: f32) -> f32 {
        super::tanh(x as f64) as f32
    }
    pub fn sigmoida(x: f32) -> f32 {
        super::sigmoida(x as f64) as f32
    }
    pub fn sin(x: f32) -> f32 {
        super::sin(x as f64) as f32
    }
    pub fn cos(x: f32) -> f32 {
        super::cos(x as f64) as f32
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    fn ulp_rozdiel(a: f64, b: f64) -> u64 {
        if a == b {
            return 0;
        }
        let ia = a.to_bits() as i64;
        let ib = b.to_bits() as i64;
        let ia = if ia < 0 { i64::MIN - ia } else { ia };
        let ib = if ib < 0 { i64::MIN - ib } else { ib };
        ia.abs_diff(ib)
    }

    fn body(od: f64, po: f64, n: usize) -> impl Iterator<Item = f64> {
        (0..=n).map(move |i| od + (po - od) * i as f64 / n as f64)
    }

    #[test]
    fn exp_presnost() {
        for x in body(-700.0, 700.0, 20_000).chain(body(-1.0, 1.0, 5_000)) {
            assert!(ulp_rozdiel(exp(x), x.exp()) <= 2, "exp({x}) = {} ≠ {}", exp(x), x.exp());
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(exp(1000.0), f64::INFINITY);
        assert_eq!(exp(-1000.0), 0.0);
        assert!(exp(-740.0) > 0.0);
    }

    #[test]
    fn ln_presnost() {
        for x in body(1e-300, 1e300, 20_000).chain(body(0.5, 2.0, 5_000)).chain(body(1e-310, 1e-305, 100)) {
            assert!(ulp_rozdiel(ln(x), x.ln()) <= 2, "ln({x}) = {} ≠ {}", ln(x), x.ln());
        }
        assert_eq!(ln(1.0), 0.0);
        assert!(ln(-1.0).is_nan());
        assert_eq!(ln(0.0), f64::NEG_INFINITY);
    }

    #[test]
    fn sin_cos_presnost() {
        for x in body(-1000.0, 1000.0, 40_000) {
            let (s, c) = (sin(x), cos(x));
            assert!((s - x.sin()).abs() <= 4.0 * f64::EPSILON, "sin({x})");
            assert!((c - x.cos()).abs() <= 4.0 * f64::EPSILON, "cos({x})");
        }
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
    }

    #[test]
    fn tanh_expm1_presnost() {
        for x in body(-30.0, 30.0, 20_000).chain(body(-1e-6, 1e-6, 1000)) {
            assert!(ulp_rozdiel(tanh(x), x.tanh()) <= 3, "tanh({x}) = {} ≠ {}", tanh(x), x.tanh());
            if x.abs() < 700.0 {
                let ocak = x.exp_m1();
                assert!(ulp_rozdiel(expm1(x), ocak) <= 3 || (expm1(x) - ocak).abs() < 1e-300, "expm1({x})");
            }
        }
    }

    #[test]
    fn mocnina_presnost() {
        assert_eq!(mocnina(2.0, 10.0), 1024.0);
        assert_eq!(mocnina(-3.0, 3.0), -27.0);
        assert_eq!(mocnina(2.0, -2.0), 0.25);
        for x in body(0.1, 100.0, 2000) {
            let v = mocnina(x, 2.5);
            assert!((v - x.powf(2.5)).abs() <= 1e-13 * v, "pow({x}, 2.5)");
        }
    }
}
