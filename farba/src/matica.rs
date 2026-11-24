//! Matice 3 × 3 a vektory v dvojitej presnosti (stĺpcový vektor: `v' = M · v`).

/// Matica 3 × 3 po riadkoch.
pub type M3 = [[f64; 3]; 3];
/// Vektor troch zložiek.
pub type V3 = [f64; 3];

/// Jednotková matica.
pub const JEDNOTKOVA: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Súčin `a · b` (najprv sa použije `b`).
pub fn nasob(a: &M3, b: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for (i, r) in m.iter_mut().enumerate() {
        for (j, x) in r.iter_mut().enumerate() {
            *x = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

/// `m · v`.
#[inline]
pub fn aplikuj(m: &M3, v: V3) -> V3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Inverzná matica (ak determinant nie je takmer nulový).
pub fn inverzna(m: &M3) -> Option<M3> {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let k0 = e * i - f * h;
    let k1 = f * g - d * i;
    let k2 = d * h - e * g;
    let det = a * k0 + b * k1 + c * k2;
    if det.abs() < 1e-300 || !det.is_finite() {
        return None;
    }
    let r = 1.0 / det;
    Some([
        [k0 * r, (c * h - b * i) * r, (b * f - c * e) * r],
        [k1 * r, (a * i - c * g) * r, (c * d - a * f) * r],
        [k2 * r, (b * g - a * h) * r, (a * e - b * d) * r],
    ])
}

/// Diagonálna matica.
pub fn diagonala(v: V3) -> M3 {
    [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]]
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn inverzia() {
        let m = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
        let n = nasob(&m, &inverzna(&m).unwrap());
        for (i, r) in n.iter().enumerate() {
            for (j, &x) in r.iter().enumerate() {
                assert!((x - JEDNOTKOVA[i][j]).abs() < 1e-12);
            }
        }
        assert!(inverzna(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 1.0]]).is_none());
    }
}
