//! JPEG (ITU-T T.81 | ISO/IEC 10918-1, JFIF, Exif, Adobe) – vlastný dekodér a kvalitný kodér.
//!
//! **Dekodér:** základný (baseline) a rozšírený sekvenčný režim (8 a 12 bitov), progresívny režim
//! (spektrálny výber aj postupné spresňovanie), reštarty, ľubovoľné podvzorkovanie 1–4, sivé, YCbCr, RGB,
//! CMYK a YCCK (Adobe), profil ICC po častiach, Exif s natočením, XMP, komentáre. Zmenšené dekódovanie
//! 1/2, 1/4 a 1/8 počíta len potrebné frekvencie – náhľad fotky 24 Mpx stojí zlomok práce (Z1, Z7).
//! Aritmetické kódovanie a bezstratový režim (SOF3) sa zatiaľ nečítajú – oznámi sa to chybou.
//!
//! **Kodér:** presná DCT v plávajúcej čiarke, kvantizačné tabuľky normy (príloha K) podľa kvality 1–100,
//! podvzorkovanie 4:4:4, 4:2:2, 4:2:0 (pri vysokej kvalite automaticky 4:4:4), Huffmanove tabuľky
//! optimalizované pre každý obrázok, voliteľne progresívny zápis; ICC, Exif a XMP sa zachovajú.

pub mod dct;
mod dekoder;
mod koder;

pub use dekoder::{Hlavicka, citaj, citaj_hlavicku, citaj_zmensene};
pub use koder::{NastavenieJpeg, Podvzorkovanie, zapis};

/// Prirodzený index (riadok·8 + stĺpec) k-teho koeficientu v poradí cik-cak.
pub(crate) const CIK_CAK: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7,
    14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39,
    46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Kvantizačné tabuľky z prílohy K normy (prirodzené poradie): jas a farebnosť.
pub(crate) const KVANT_JAS: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17,
    22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78,
    87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
pub(crate) const KVANT_FAREBNOST: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

/// Huffmanove tabuľky z prílohy K.3 (počty kódov dĺžky 1–16 a hodnoty) – použijú sa, keď súbor
/// tabuľky neobsahuje (Motion JPEG z kamier).
pub(crate) const STD_DC_JAS: ([u8; 16], &[u8]) =
    ([0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
pub(crate) const STD_DC_FAREBNOST: ([u8; 16], &[u8]) =
    ([0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
pub(crate) const STD_AC_JAS: ([u8; 16], &[u8]) = (
    [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125],
    &[
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22,
        0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33,
        0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34,
        0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55,
        0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76,
        0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
        0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5,
        0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
        0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1,
        0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ],
);
pub(crate) const STD_AC_FAREBNOST: ([u8; 16], &[u8]) = (
    [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119],
    &[
        0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13,
        0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62,
        0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29,
        0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54,
        0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75,
        0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94,
        0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3,
        0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
        0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
        0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ],
);

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn cik_cak_je_permutacia() {
        let mut videne = [false; 64];
        for &i in &CIK_CAK {
            assert!(!videne[i]);
            videne[i] = true;
        }
        // susedné kroky cik-cak sa líšia o jeden riadok alebo stĺpec (diagonály)
        for k in 1..64 {
            let (a, b) = (CIK_CAK[k - 1], CIK_CAK[k]);
            let (ra, sa, rb, sb) = (a / 8, a % 8, b / 8, b % 8);
            assert!((ra as i32 - rb as i32).abs() <= 1 && (sa as i32 - sb as i32).abs() <= 1);
        }
        for t in [STD_DC_JAS, STD_DC_FAREBNOST, STD_AC_JAS, STD_AC_FAREBNOST] {
            assert_eq!(t.0.iter().map(|&x| x as usize).sum::<usize>(), t.1.len());
        }
    }
}

#[cfg(test)]
mod skusky_tam_a_spat {
    use super::*;
    use crate::obraz::{Limity, Model, Obraz, Vzorky};
    use orchidea_jadro::nahoda::Nahoda;

    fn fotka(w: usize, h: usize, model: Model) -> Obraz {
        let mut n = Nahoda::zo_semienka((w * 7 + h) as u64);
        let k = match model {
            Model::Siva => 1,
            Model::Cmyk => 4,
            _ => 3,
        };
        let mut v = Vec::with_capacity(w * h * k);
        for y in 0..h {
            for x in 0..w {
                for c in 0..k {
                    let z = 128.0 + 90.0 * ((x as f64 / (9.0 + c as f64)).sin() * (y as f64 / 13.0).cos());
                    v.push((z + n.mensie_ako(9) as f64 - 4.0).clamp(0.0, 255.0) as u8);
                }
            }
        }
        Obraz::novy(w, h, model, false, Vzorky::U8(v)).unwrap()
    }

    fn psnr(a: &Obraz, b: &Obraz) -> f64 {
        let (Vzorky::U8(x), Vzorky::U8(y)) = (&a.vzorky, &b.vzorky) else { panic!() };
        let mse: f64 =
            x.iter().zip(y).map(|(&p, &q)| (p as f64 - q as f64).powi(2)).sum::<f64>() / x.len() as f64;
        10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
    }

    #[test]
    fn tam_a_spat_vsetky_rezimy() {
        for model in [Model::Siva, Model::Rgb, Model::Cmyk] {
            for (w, h) in [(1, 1), (17, 9), (64, 48), (33, 70)] {
                let o = fotka(w, h, model);
                for podv in [Podvzorkovanie::S444, Podvzorkovanie::S422, Podvzorkovanie::S420] {
                    for progresivny in [false, true] {
                        let n = NastavenieJpeg { kvalita: 95, podvzorkovanie: podv, progresivny };
                        let j = zapis(&o, &n).unwrap();
                        let s = citaj(&j, &Limity::default()).unwrap();
                        assert_eq!((s.sirka, s.vyska, s.model), (w, h, model));
                        let p = psnr(&o, &s);
                        let min =
                            if podv == Podvzorkovanie::S444 || model != Model::Rgb { 38.0 } else { 30.0 };
                        assert!(p > min, "{model:?} {w}×{h} {podv:?} prog {progresivny}: PSNR {p:.1}");
                        // zmenšené dekódovanie dá správne rozmery
                        for m in [2, 4, 8] {
                            let z = citaj_zmensene(&j, &Limity::default(), m).unwrap();
                            assert_eq!((z.sirka, z.vyska), (w.div_ceil(m), h.div_ceil(m)));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn metadata_sa_zachovaju() {
        let mut o = fotka(20, 20, Model::Rgb);
        o.farba.icc = Some((0..150_000u32).map(|i| (i % 251) as u8).collect());
        o.metadata.exif = Some(crate::exif::s_natocenim(6));
        o.metadata.dpi = Some((300.0, 300.0));
        let s = citaj(&zapis(&o, &NastavenieJpeg::default()).unwrap(), &Limity::default()).unwrap();
        assert_eq!(s.farba.icc, o.farba.icc);
        assert_eq!(s.metadata.natocenie, 6);
        assert_eq!(s.metadata.dpi, Some((300.0, 300.0)));
    }
}
