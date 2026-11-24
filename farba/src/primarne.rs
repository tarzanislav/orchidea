//! Základné farby (primaries), biele body a chromatická adaptácia (Bradford, CAT02, von Kries).

use crate::matica::{self, M3, V3};

/// Chromatickosť (x, y).
pub type Xy = (f64, f64);

/// Biely bod D65 (sRGB, BT.709, BT.2020, Display P3).
pub const D65: Xy = (0.3127, 0.3290);
/// Biely bod D50 (spojovací priestor ICC, ProPhoto).
pub const D50: Xy = (0.3457, 0.3585);
/// Biely bod ACES (≈ D60).
pub const D60_ACES: Xy = (0.32168, 0.33767);
/// Biely bod kina DCI.
pub const DCI: Xy = (0.314, 0.351);
/// Rovnoenergetický biely bod E.
pub const E: Xy = (1.0 / 3.0, 1.0 / 3.0);

/// Biely bod spojovacieho priestoru ICC (XYZ, D50 podľa ICC.1).
pub const ICC_D50: V3 = [0.9642, 1.0, 0.8249];

/// Tri základné farby a biely bod.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Primarne {
    pub r: Xy,
    pub g: Xy,
    pub b: Xy,
    pub biely: Xy,
}

/// sRGB a BT.709.
pub const SRGB: Primarne = Primarne { r: (0.64, 0.33), g: (0.30, 0.60), b: (0.15, 0.06), biely: D65 };
/// BT.2020 a BT.2100.
pub const BT2020: Primarne = Primarne { r: (0.708, 0.292), g: (0.170, 0.797), b: (0.131, 0.046), biely: D65 };
/// DCI-P3 (kino, biely bod DCI).
pub const DCI_P3: Primarne = Primarne { r: (0.680, 0.320), g: (0.265, 0.690), b: (0.150, 0.060), biely: DCI };
/// Display P3 (P3 s bielym bodom D65).
pub const DISPLAY_P3: Primarne =
    Primarne { r: (0.680, 0.320), g: (0.265, 0.690), b: (0.150, 0.060), biely: D65 };
/// Adobe RGB (1998).
pub const ADOBE_RGB: Primarne = Primarne { r: (0.64, 0.33), g: (0.21, 0.71), b: (0.15, 0.06), biely: D65 };
/// ProPhoto RGB (ROMM).
pub const PROPHOTO: Primarne =
    Primarne { r: (0.7347, 0.2653), g: (0.1596, 0.8404), b: (0.0366, 0.0001), biely: D50 };
/// ACES AP0 (ACES2065-1).
pub const ACES_AP0: Primarne =
    Primarne { r: (0.7347, 0.2653), g: (0.0, 1.0), b: (0.0001, -0.0770), biely: D60_ACES };
/// ACES AP1 (ACEScg, ACEScc, ACEScct).
pub const ACES_AP1: Primarne =
    Primarne { r: (0.713, 0.293), g: (0.165, 0.830), b: (0.128, 0.044), biely: D60_ACES };

/// XYZ z chromatickosti pri Y = 1.
pub fn xy_na_xyz((x, y): Xy) -> V3 {
    [x / y, 1.0, (1.0 - x - y) / y]
}

impl Primarne {
    /// Matica lineárne RGB → XYZ (biela RGB 1, 1, 1 má Y = 1 a chromatickosť bieleho bodu).
    pub fn na_xyz(&self) -> M3 {
        let [r, g, b] = [self.r, self.g, self.b].map(xy_na_xyz);
        let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
        let s = matica::aplikuj(&matica::inverzna(&m).unwrap_or(matica::JEDNOTKOVA), xy_na_xyz(self.biely));
        matica::nasob(&m, &matica::diagonala(s))
    }

    /// Matica XYZ → lineárne RGB.
    pub fn z_xyz(&self) -> M3 {
        matica::inverzna(&self.na_xyz()).unwrap_or(matica::JEDNOTKOVA)
    }
}

/// Spôsob chromatickej adaptácie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adaptacia {
    /// Bradford (ICC, ACES).
    Bradford,
    /// CAT02 (CIECAM02).
    Cat02,
    /// von Kries s kužeľmi Hunt-Pointer-Estévez.
    VonKries,
    /// Prosté škálovanie XYZ.
    Xyz,
}

impl Adaptacia {
    fn kuzele(self) -> M3 {
        match self {
            Adaptacia::Bradford => {
                [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]]
            }
            Adaptacia::Cat02 => {
                [[0.7328, 0.4296, -0.1624], [-0.7036, 1.6975, 0.0061], [0.0030, 0.0136, 0.9834]]
            }
            Adaptacia::VonKries => {
                [[0.40024, 0.70760, -0.08081], [-0.22630, 1.16532, 0.04570], [0.0, 0.0, 0.91822]]
            }
            Adaptacia::Xyz => matica::JEDNOTKOVA,
        }
    }
}

/// Matica adaptácie XYZ zo zdrojového bieleho (XYZ) na cieľový.
pub fn adaptacia(zdroj: V3, ciel: V3, sposob: Adaptacia) -> M3 {
    let k = sposob.kuzele();
    let (z, c) = (matica::aplikuj(&k, zdroj), matica::aplikuj(&k, ciel));
    let d = matica::diagonala([c[0] / z[0], c[1] / z[1], c[2] / z[2]]);
    let ki = matica::inverzna(&k).unwrap_or(matica::JEDNOTKOVA);
    matica::nasob(&ki, &matica::nasob(&d, &k))
}

/// Matica lineárne RGB zdroja → lineárne RGB cieľa (s adaptáciou bieleho bodu).
pub fn rgb_na_rgb(zdroj: &Primarne, ciel: &Primarne, sposob: Adaptacia) -> M3 {
    let a = adaptacia(xy_na_xyz(zdroj.biely), xy_na_xyz(ciel.biely), sposob);
    matica::nasob(&ciel.z_xyz(), &matica::nasob(&a, &zdroj.na_xyz()))
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn srgb_na_xyz_ako_norma() {
        // IEC 61966-2-1 (zaokrúhlené na 4 miesta)
        let m = SRGB.na_xyz();
        let n = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[i][j] - n[i][j]).abs() < 1e-4, "{i} {j}: {}", m[i][j]);
            }
        }
    }

    #[test]
    fn adaptacia_zachova_bielu() {
        let a = adaptacia(xy_na_xyz(D65), ICC_D50, Adaptacia::Bradford);
        let b = matica::aplikuj(&a, xy_na_xyz(D65));
        for i in 0..3 {
            assert!((b[i] - ICC_D50[i]).abs() < 1e-12);
        }
    }
}
