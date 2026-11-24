//! ACES (Academy Color Encoding System) 1.3: priestory ACES2065-1 (AP0), ACEScg (AP1, lineárne), ACEScc a ACEScct
//! a výstupná transformácia pre displej – referenčné vykreslenie RRT a výstup ODT pre sRGB a BT.709 (100 cd/m²,
//! šero okolia).
//!
//! RRT a ODT sú v ACES 1.x normatívne definované textom transformácií Akadémie (ACES 1.3, jazyk CTL) a technickými
//! bulletinmi TB-2014-002 (prehľad systému) a TB-2014-012 (ODT). Všetky kroky sú tu napísané nanovo z ich
//! matematického popisu: tónová krivka je po úsekoch kvadratický B-splajn v log10, „žiara“ (glow) a úprava
//! červenej používajú kubický B-splajn v odtieni, desaturácia je lineárna interpolácia k jasu. Matice sa počítajú
//! zo základných farieb (`primarne`), nie z prepísaných tabuliek. Nezávislý rozhodca: OpenColorIO 2 (vstavaná
//! transformácia „ACES-OUTPUT – SDR-VIDEO_1.0“) v CI.

use crate::matica::{self, M3, V3};
use crate::primarne::{self, Adaptacia};
use orchidea_jadro::matematika::{log10, mocnina, odmocnina};
use std::sync::OnceLock;

// ---------------------------------------------------------------- uhol (deterministický arkustangens)

/// atan(x) s redukciou argumentu dvojnásobným polovičným uhlom a Taylorovým radom (chyba < 1e-17).
fn atan(x: f64) -> f64 {
    if !x.is_finite() {
        return if x.is_nan() { x } else { x.signum() * std::f64::consts::FRAC_PI_2 };
    }
    let (z, znamienko) = (x.abs(), x.signum());
    let (z, doplnok) = if z > 1.0 { (1.0 / z, true) } else { (z, false) };
    // atan(z) = 2 · atan(z / (1 + √(1 + z²))), dvakrát → |z| ≤ tan(π/16) ≈ 0,199
    let mut t = z;
    for _ in 0..2 {
        t /= 1.0 + odmocnina(1.0 + t * t);
    }
    let t2 = t * t;
    let mut s = 0.0;
    let mut clen = t;
    for k in 0..14 {
        s += clen / (2 * k + 1) as f64 * if k % 2 == 0 { 1.0 } else { -1.0 };
        clen *= t2;
    }
    let r = 4.0 * s;
    znamienko * if doplnok { std::f64::consts::FRAC_PI_2 - r } else { r }
}

/// atan2(y, x) v radiánoch (−π, π].
pub(crate) fn atan2(y: f64, x: f64) -> f64 {
    use std::f64::consts::PI;
    if x > 0.0 {
        atan(y / x)
    } else if x < 0.0 {
        if y >= 0.0 { atan(y / x) + PI } else { atan(y / x) - PI }
    } else if y > 0.0 {
        PI / 2.0
    } else if y < 0.0 {
        -PI / 2.0
    } else {
        0.0
    }
}

// ---------------------------------------------------------------- priestory

/// Matice medzi AP0, AP1 a XYZ (biely bod ACES).
#[derive(Debug)]
struct Matice {
    ap0_na_ap1: M3,
    ap1_na_ap0: M3,
    ap1_na_xyz: M3,
    xyz_na_ap1: M3,
    /// Jas Y z AP1.
    ap1_y: V3,
}

fn matice() -> &'static Matice {
    static M: OnceLock<Matice> = OnceLock::new();
    M.get_or_init(|| {
        let ap1_na_xyz = primarne::ACES_AP1.na_xyz();
        let xyz_na_ap1 = primarne::ACES_AP1.z_xyz();
        let ap0_na_ap1 = matica::nasob(&xyz_na_ap1, &primarne::ACES_AP0.na_xyz());
        Matice {
            ap1_na_ap0: matica::inverzna(&ap0_na_ap1).unwrap_or(matica::JEDNOTKOVA),
            ap0_na_ap1,
            ap1_na_xyz,
            xyz_na_ap1,
            ap1_y: ap1_na_xyz[1],
        }
    })
}

/// ACES2065-1 (AP0) → ACEScg (AP1); oba lineárne s rovnakým bielym bodom.
pub fn ap0_na_ap1(v: V3) -> V3 {
    matica::aplikuj(&matice().ap0_na_ap1, v)
}

/// ACEScg (AP1) → ACES2065-1 (AP0).
pub fn ap1_na_ap0(v: V3) -> V3 {
    matica::aplikuj(&matice().ap1_na_ap0, v)
}

// ---------------------------------------------------------------- pomocné funkcie vykreslenia

const RRT_GLOW_GAIN: f64 = 0.05;
const RRT_GLOW_MID: f64 = 0.08;
const RRT_RED_SCALE: f64 = 0.82;
const RRT_RED_PIVOT: f64 = 0.03;
const RRT_RED_HUE: f64 = 0.0;
const RRT_RED_WIDTH: f64 = 135.0;
const RRT_SAT: f64 = 0.96;
const ODT_SAT: f64 = 0.93;
const KINO_BIELA: f64 = 48.0;
const KINO_CIERNA: f64 = 0.02;
const SERO_GAMA: f64 = 0.9811;
const MALO: f64 = 1e-10;
/// Najmenšia kladná hodnota typu half (logaritmus sa nikdy nepočíta z nuly).
const HALF_NAJMENSIE: f64 = 5.960_464_48e-8;
const HALF_NAJVIAC: f64 = 65504.0;

/// Sýtosť (max − min) / max.
fn sytost([r, g, b]: V3) -> f64 {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    (mx.max(MALO) - mn.max(MALO)) / mx.max(1e-2)
}

/// Jas so zložkou chrómy (yc) – priemer RGB zväčšený o polomer chrómy s váhou 1,75.
fn jas_yc([r, g, b]: V3) -> f64 {
    let chroma = odmocnina((b * (b - g) + g * (g - r) + r * (r - b)).max(0.0));
    (b + g + r + 1.75 * chroma) / 3.0
}

/// Geometrický odtieň v stupňoch 0–360; pre neutrálne farby `None`.
fn odtien([r, g, b]: V3) -> Option<f64> {
    if r == g && g == b {
        return None;
    }
    let h = atan2(3f64.sqrt_det() * (g - b), 2.0 * r - g - b).to_degrees();
    Some(if h < 0.0 { h + 360.0 } else { h })
}

trait Odmocnina {
    fn sqrt_det(self) -> f64;
}
impl Odmocnina for f64 {
    fn sqrt_det(self) -> f64 {
        odmocnina(self)
    }
}

/// Esovitá krivka na −2…2 (dva parabolické oblúky), výsledok 0–1.
fn esovita(x: f64) -> f64 {
    let t = (1.0 - (x / 2.0).abs()).max(0.0);
    let y = 1.0 + x.signum() * (1.0 - t * t);
    if x == 0.0 { 0.5 } else { y / 2.0 }
}

/// Zosilnenie „žiary“ tmavých sýtych farieb.
fn ziara(yc: f64, zisk: f64, stred: f64) -> f64 {
    if yc <= 2.0 / 3.0 * stred {
        zisk
    } else if yc >= 2.0 * stred {
        0.0
    } else {
        zisk * (stred / yc - 0.5)
    }
}

/// Odtieň posunutý okolo stredu do (−180, 180].
fn stred_odtiena(h: f64, stred: f64) -> f64 {
    let d = h - stred;
    if d < -180.0 {
        d + 360.0
    } else if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

/// Rovnomerný kubický B-splajn so šírkou nosiča `w`, vrchol 1 v strede.
fn b_splajn(x: f64, w: f64) -> f64 {
    if x <= -w / 2.0 || x >= w / 2.0 {
        return 0.0;
    }
    let u = (x + w / 2.0) * 4.0 / w;
    let j = (u as usize).min(3);
    let t = u - j as f64;
    let y = match j {
        0 => t * t * t,
        1 => -3.0 * t * t * t + 3.0 * t * t + 3.0 * t + 1.0,
        2 => 3.0 * t * t * t - 6.0 * t * t + 4.0,
        _ => {
            let s = 1.0 - t;
            s * s * s
        }
    } / 6.0;
    y * 1.5
}

/// Desaturácia: lineárna interpolácia medzi jasom (váhy `y`) a farbou.
fn desaturuj(v: V3, y: V3, s: f64) -> V3 {
    let l = (1.0 - s) * (y[0] * v[0] + y[1] * v[1] + y[2] * v[2]);
    [l + s * v[0], l + s * v[1], l + s * v[2]]
}

/// Parametre tónovej krivky (úseky kvadratického B-splajnu v log10).
#[derive(Debug)]
struct Krivka {
    nizke: &'static [f64],
    vysoke: &'static [f64],
    min: (f64, f64),
    stred: (f64, f64),
    max: (f64, f64),
    sklon_nizko: f64,
    sklon_vysoko: f64,
}

/// Kvadratický B-splajn (uniformný) s tromi susednými koeficientmi.
fn usek(c: &[f64], j: usize, t: f64) -> f64 {
    let (c0, c1, c2) = (c[j], c[j + 1], c[j + 2]);
    t * t * (0.5 * c0 - c1 + 0.5 * c2) + t * (c1 - c0) + 0.5 * (c0 + c1)
}

impl Krivka {
    fn hodnota(&self, x: f64) -> f64 {
        let lx = log10(x.max(HALF_NAJMENSIE));
        let (l_min, l_stred, l_max) = (log10(self.min.0), log10(self.stred.0), log10(self.max.0));
        let ly = if lx <= l_min {
            lx * self.sklon_nizko + (log10(self.min.1) - self.sklon_nizko * l_min)
        } else if lx < l_stred {
            let uzly = (self.nizke.len() - 3) as f64;
            let k = uzly * (lx - l_min) / (l_stred - l_min);
            let j = (k as usize).min(self.nizke.len() - 3);
            usek(self.nizke, j, k - j as f64)
        } else if lx < l_max {
            let uzly = (self.vysoke.len() - 3) as f64;
            let k = uzly * (lx - l_stred) / (l_max - l_stred);
            let j = (k as usize).min(self.vysoke.len() - 3);
            usek(self.vysoke, j, k - j as f64)
        } else {
            lx * self.sklon_vysoko + (log10(self.max.1) - self.sklon_vysoko * l_max)
        };
        mocnina(10.0, ly)
    }
}

/// Tónová krivka referenčného vykreslenia (RRT, 5 uzlov na stranu + okrajové).
static RRT_KRIVKA: Krivka = Krivka {
    nizke: &[-4.0, -4.0, -3.157_376_577_3, -0.485_249_995_8, 1.847_732_470_6, 1.847_732_470_6],
    vysoke: &[-0.718_548_242_5, 2.081_030_717_2, 3.668_124_123_7, 4.0, 4.0, 4.0],
    min: (0.18 * 3.051_757_812_5e-5, 0.0001),
    stred: (0.18, 4.8),
    max: (0.18 * 262_144.0, 10000.0),
    sklon_nizko: 0.0,
    sklon_vysoko: 0.0,
};

const ODT_NIZKE: [f64; 10] = [
    -1.698_970_004_3,
    -1.698_970_004_3,
    -1.477_9,
    -1.229_1,
    -0.864_8,
    -0.448,
    0.005_18,
    0.451_108_033_4,
    0.911_374_441_4,
    0.911_374_441_4,
];
const ODT_VYSOKE: [f64; 10] = [
    0.515_438_696_5,
    0.847_043_778_3,
    1.135_8,
    1.380_2,
    1.519_7,
    1.598_5,
    1.646_7,
    1.674_609_135_7,
    1.687_873_339,
    1.687_873_339,
];

/// Tónová krivka výstupu 48 cd/m² (ODT, 8 uzlov na stranu); body sa odvodzujú z krivky RRT.
fn odt_krivka() -> &'static Krivka {
    static K: OnceLock<Krivka> = OnceLock::new();
    K.get_or_init(|| Krivka {
        nizke: &ODT_NIZKE,
        vysoke: &ODT_VYSOKE,
        min: (RRT_KRIVKA.hodnota(0.18 * mocnina(2.0, -6.5)), 0.02),
        stred: (RRT_KRIVKA.hodnota(0.18), 4.8),
        max: (RRT_KRIVKA.hodnota(0.18 * mocnina(2.0, 6.5)), 48.0),
        sklon_nizko: 0.0,
        sklon_vysoko: 0.04,
    })
}

/// Referenčné vykreslenie (RRT): ACES2065-1 → OCES (AP0, jednotky cd/m² v pomere ku kinu).
pub fn rrt(aces: V3) -> V3 {
    let m = matice();
    let mut a = aces;
    // žiara
    let s = sytost(a);
    let pridane = 1.0 + ziara(jas_yc(a), RRT_GLOW_GAIN * esovita((s - 0.4) / 0.2), RRT_GLOW_MID);
    a = [a[0] * pridane, a[1] * pridane, a[2] * pridane];
    // úprava červenej
    if let Some(h) = odtien(a) {
        let vaha = b_splajn(stred_odtiena(h, RRT_RED_HUE), RRT_RED_WIDTH);
        a[0] += vaha * s * (RRT_RED_PIVOT - a[0]) * (1.0 - RRT_RED_SCALE);
    }
    // do priestoru vykreslenia (AP1), desaturácia, tónová krivka po zložkách
    let a = a.map(|x| x.max(0.0));
    let pred = matica::aplikuj(&m.ap0_na_ap1, a).map(|x| x.clamp(0.0, HALF_NAJVIAC));
    let pred = desaturuj(pred, m.ap1_y, RRT_SAT);
    let po = pred.map(|x| RRT_KRIVKA.hodnota(x));
    matica::aplikuj(&m.ap1_na_ap0, po)
}

/// Displej výstupu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Displej {
    /// sRGB (IEC 61966-2-1, mocninová krivka 2,4 s lineárnou pätou), 100 cd/m², šero.
    Srgb,
    /// BT.709 s displejom BT.1886 (gama 2,4), 100 cd/m², šero.
    Bt709,
}

/// Výstup ODT pred kódovaním: OCES → lineárne XYZ (D65), biela = Y 1.
pub fn odt_xyz(oces: V3) -> V3 {
    let m = matice();
    let k = odt_krivka();
    let pred = matica::aplikuj(&m.ap0_na_ap1, oces);
    let po = pred.map(|x| k.hodnota(x));
    // jas na lineárnu hodnotu kódu
    let lin = po.map(|y| (y - KINO_CIERNA) / (KINO_BIELA - KINO_CIERNA));
    // tmavé → šero: mocnina jasu Y pri zachovaní chromatickosti
    let xyz = matica::aplikuj(&m.ap1_na_xyz, lin);
    let sucet = xyz[0] + xyz[1] + xyz[2];
    let sucet = if sucet == 0.0 { MALO } else { sucet };
    let (x, y) = (xyz[0] / sucet, xyz[1] / sucet);
    let jas = mocnina(xyz[1].max(0.0), SERO_GAMA);
    let yy = y.max(MALO);
    let xyz = [x * jas / yy, jas, (1.0 - x - y) * jas / yy];
    let lin = matica::aplikuj(&m.xyz_na_ap1, xyz);
    // desaturácia kvôli nižšiemu jasu displeja
    let lin = desaturuj(lin, m.ap1_y, ODT_SAT);
    // AP1 → XYZ a adaptácia bieleho ACES na D65 (Bradford)
    let xyz = matica::aplikuj(&m.ap1_na_xyz, lin);
    matica::aplikuj(&d60_na_d65(), xyz)
}

fn d60_na_d65() -> M3 {
    static A: OnceLock<M3> = OnceLock::new();
    *A.get_or_init(|| {
        primarne::adaptacia(
            primarne::xy_na_xyz(primarne::D60_ACES),
            primarne::xy_na_xyz(primarne::D65),
            Adaptacia::Bradford,
        )
    })
}

/// Celá výstupná transformácia: ACES2065-1 → kódované RGB displeja (0–1).
pub fn zobraz(aces: V3, d: Displej) -> V3 {
    let xyz = odt_xyz(rrt(aces));
    let lin = matica::aplikuj(&xyz_na_709(), xyz).map(|x| x.clamp(0.0, 1.0));
    match d {
        Displej::Srgb => lin.map(srgb_odt),
        Displej::Bt709 => lin.map(|x| mocnina(x, 1.0 / 2.4)),
    }
}

fn xyz_na_709() -> M3 {
    static M: OnceLock<M3> = OnceLock::new();
    *M.get_or_init(|| primarne::SRGB.z_xyz())
}

/// Krivka displeja sRGB v ODT: mocnina 2,4 s posunom 0,055 a lineárnou pätou v bode dotyku (spojitá derivácia).
fn srgb_odt(y: f64) -> f64 {
    let (g, o) = (2.4, 0.055);
    let zlom = mocnina(o * g / ((g - 1.0) * (1.0 + o)), g);
    let sklon = mocnina((g - 1.0) / o, g - 1.0) * mocnina((1.0 + o) / g, g);
    if y >= zlom { (1.0 + o) * mocnina(y, 1.0 / g) - o } else { y * sklon }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn arkustangens() {
        for i in -200..=200 {
            let (y, x) = (i as f64 / 37.0, (i % 17) as f64 / 5.0 - 1.3);
            assert!((atan2(y, x) - y.atan2(x)).abs() < 1e-14, "{y} {x}");
        }
    }

    #[test]
    fn b_splajn_vrchol_a_okraje() {
        assert!((b_splajn(0.0, 135.0) - 1.0).abs() < 1e-12);
        assert_eq!(b_splajn(67.5, 135.0), 0.0);
        assert!((b_splajn(-33.75, 135.0) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn stredna_siva() {
        // 18 % sivá na sRGB displeji ≈ 0,356 (Y ≈ 0,104 podľa rozhodcu OpenColorIO)
        let v = zobraz([0.18; 3], Displej::Srgb);
        assert!((v[0] - v[1]).abs() < 1e-6 && (v[1] - v[2]).abs() < 1e-6);
        assert!(v[0] > 0.35 && v[0] < 0.36, "{v:?}");
        // čierna → 0, veľmi jasná → 1
        assert!(zobraz([0.0; 3], Displej::Srgb)[1] < 1e-3);
        assert!(zobraz([1000.0; 3], Displej::Srgb)[1] > 0.999);
    }
}
