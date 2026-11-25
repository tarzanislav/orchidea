//! Farebný priestor obrázka pre balík `farba` – z profilu ICC, kódov cICP (H.273), alebo z gAMA a cHRM (PNG).
//!
//! Poradie podľa spoľahlivosti: profil ICC → cICP → sRGB → lineárne → chromatickosti a gama → len gama.
//! `None` znamená „nevieme“ (napr. CMYK bez profilu) – zobrazí sa ako sRGB.

use crate::obraz::{Model, Obraz};
use orchidea_farba::{Prenos, Priestor, Primarne, primarne};

/// Farebný priestor farebných kanálov obrázka.
pub fn priestor(o: &Obraz) -> Option<Priestor> {
    let f = &o.farba;
    let sivy = o.model == Model::Siva;
    let z_prenosu = |p: Prenos| {
        if sivy {
            Priestor::Sivy { prenos: p }
        } else {
            Priestor::Rgb { primarne: primarne::SRGB, prenos: p }
        }
    };
    if let Some(d) = &f.icc
        && let Ok(p) = Priestor::z_icc(d)
    {
        return Some(p);
    }
    if let Some(c) = &f.cicp
        && let Some(p) = Priestor::z_cicp(c.zakladne, c.prenos)
    {
        return Some(match p {
            Priestor::Rgb { prenos, .. } if sivy => Priestor::Sivy { prenos },
            p => p,
        });
    }
    if f.srgb.is_some() {
        return Some(z_prenosu(Prenos::Srgb));
    }
    if f.linearne {
        return Some(z_prenosu(Prenos::Linearny));
    }
    if !matches!(o.model, Model::Rgb | Model::Siva) {
        return None;
    }
    // PNG: gAMA je exponent kódovania (0,45455 ≈ 1/2,2)
    let prenos = f.gama.filter(|&g| g > 0.0).map(|g| Prenos::Gama(1.0 / g));
    match (f.chromaticity, prenos) {
        (Some([wx, wy, rx, ry, gx, gy, bx, by]), p) if !sivy => Some(Priestor::Rgb {
            primarne: Primarne { r: (rx, ry), g: (gx, gy), b: (bx, by), biely: (wx, wy) },
            prenos: p.unwrap_or(Prenos::Srgb),
        }),
        (_, Some(p)) => Some(z_prenosu(p)),
        _ => None,
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::obraz::{Cicp, Vzorky};

    #[test]
    fn poradie_zdrojov() {
        let mut o = Obraz::novy(1, 1, Model::Rgb, false, Vzorky::U8(vec![0; 3])).unwrap();
        assert!(priestor(&o).is_none());
        o.farba.gama = Some(1.0 / 2.2);
        assert!(
            matches!(priestor(&o), Some(Priestor::Rgb { prenos: Prenos::Gama(g), .. }) if (g - 2.2).abs() < 1e-9)
        );
        o.farba.cicp = Some(Cicp { zakladne: 9, prenos: 16, matica: 0, plny_rozsah: true });
        assert!(
            matches!(priestor(&o), Some(Priestor::Rgb { primarne: p, prenos: Prenos::Pq }) if p == primarne::BT2020)
        );
    }
}
