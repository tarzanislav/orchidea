//! Znak ORCHIDEE (logo) – tri návrhy na výber vlastníkom. Kritika A (28. 11. 2026): pôvodný kruh s dvoma bodkami
//! sa čítal ako ⓘ (informácie) alebo ikona profilu a v hlavičke vyzeral ako tlačidlo. Návrhy sú vlastný kvet
//! orchidey kreslený kódom na mriežke 20 × 20 bodov – **vždy plný a dvojfarebný**, aby sa nepletal s jednofarebnými
//! ťahovými ikonami, a **bez kruhového podkladu**, aby nevyzeral ako tlačidlo.

use crate::kreslic::Kreslic;
use crate::tema::Tema;
use orchidea_pismo::{Farba, zmiesaj};

/// Návrh znaku.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Znak {
    /// kvet orchidey spredu – päť okvetných lístkov rôznej veľkosti a pysk (prirodzený tvar)
    Orchidea,
    /// päť rovnakých okvetných lístkov v uhloch kvetu orchidey a pysk (geometrický tvar)
    Geometria,
    /// písmeno O z dvoch okvetných lístkov a pysk dole (monogram ORCHIDEA)
    Monogram,
}

/// Všetky návrhy (list `system/logo.png`).
pub const ZNAKY: [Znak; 3] = [Znak::Orchidea, Znak::Geometria, Znak::Monogram];

impl Znak {
    /// Meno návrhu (na výber vlastníkom).
    pub fn meno(self) -> &'static str {
        match self {
            Znak::Orchidea => "1 · orchidea",
            Znak::Geometria => "2 · geometrický kvet",
            Znak::Monogram => "3 · monogram O",
        }
    }

    /// Krátky opis návrhu.
    pub fn opis(self) -> &'static str {
        match self {
            Znak::Orchidea => "kvet orchidey spredu, prirodzené tvary",
            Znak::Geometria => "päť rovnakých lístkov, geometria",
            Znak::Monogram => "písmeno O z lístkov a pysk",
        }
    }
}

/// Body elipsy so stredom (cx, cy), polosou `a` pozdĺž smeru `uhol` (stupne, 0 = doprava, 90 = nadol) a `b` naprieč.
fn elipsa(cx: f32, cy: f32, a: f32, b: f32, uhol: f32) -> Vec<(f32, f32)> {
    let (s, c) = uhol.to_radians().sin_cos();
    (0..48)
        .map(|i| {
            let u = i as f32 / 48.0 * std::f32::consts::TAU;
            let (x, y) = (a * u.cos(), b * u.sin());
            (cx + x * c - y * s, cy + x * s + y * c)
        })
        .collect()
}

/// Okvetný lístok vychádzajúci zo stredu (cx, cy) smerom `uhol` – elipsa dĺžky `dlzka` a šírky `sirka`.
fn listok(cx: f32, cy: f32, uhol: f32, dlzka: f32, sirka: f32) -> Vec<(f32, f32)> {
    let (s, c) = uhol.to_radians().sin_cos();
    let d = dlzka / 2.0;
    elipsa(cx + d * c, cy + d * s, d, sirka / 2.0, uhol)
}

/// Farby znaku v téme: okvetné lístky v akcente, pysk tmavší (vo svetlej téme svetlejší) a stĺpik svetlý.
fn farby(t: &Tema) -> (Farba, Farba, Farba) {
    let pysk = if t.tmava { zmiesaj(t.akcent, 0x000000, 90) } else { zmiesaj(t.akcent, 0xFFFFFF, 70) };
    (t.akcent, pysk, zmiesaj(t.akcent, 0xFFFFFF, 190))
}

/// Nakreslí znak `z` veľkosti `s` bodov s ľavým horným rohom v (x, y).
pub fn kresli(k: &mut Kreslic, z: Znak, t: &Tema, x: f32, y: f32, s: f32) {
    let q = s / 20.0;
    let (listky, pysk, stlpik) = farby(t);
    let mut tvar = |body: Vec<(f32, f32)>, f: Farba| {
        let b: Vec<(f32, f32)> = body.into_iter().map(|(bx, by)| (x + bx * q, y + by * q)).collect();
        k.mnohouholnik(&b, f);
    };
    match z {
        Znak::Orchidea => {
            // dva dolné kališné lístky, horný kališný lístok, dva široké bočné lístky, pysk a stĺpik
            tvar(listok(10.0, 10.2, 125.0, 8.6, 3.2), listky);
            tvar(listok(10.0, 10.2, 55.0, 8.6, 3.2), listky);
            tvar(listok(10.0, 10.2, -90.0, 8.6, 3.6), listky);
            tvar(listok(10.0, 10.2, 192.0, 8.4, 5.8), listky);
            tvar(listok(10.0, 10.2, -12.0, 8.4, 5.8), listky);
            tvar(elipsa(10.0, 13.4, 2.6, 3.4, 90.0), pysk);
            tvar(elipsa(10.0, 10.0, 1.5, 1.2, 90.0), stlpik);
        }
        Znak::Geometria => {
            for u in [-90.0, -18.0, 198.0, 54.0, 126.0] {
                tvar(listok(10.0, 10.0, u, 7.6, 4.2), listky);
            }
            tvar(elipsa(10.0, 12.6, 2.2, 2.6, 90.0), pysk);
            tvar(elipsa(10.0, 10.0, 1.3, 1.3, 0.0), stlpik);
        }
        Znak::Monogram => {
            // písmeno O z dvoch polmesiacov (ľavý a pravý lístok), pysk dole spája ich konce
            let polmesiac = |strana: f32| -> Vec<(f32, f32)> {
                let mut b = Vec::new();
                for i in 0..=24 {
                    let u = (-80.0 + i as f32 * 160.0 / 24.0f32).to_radians();
                    b.push((10.0 + strana * 7.4 * u.cos(), 10.0 + 7.4 * u.sin()));
                }
                for i in (0..=24).rev() {
                    let u = (-80.0 + i as f32 * 160.0 / 24.0f32).to_radians();
                    b.push((10.0 + strana * 4.6 * u.cos() + strana * 0.6, 10.0 + 5.6 * u.sin()));
                }
                b
            };
            tvar(polmesiac(1.0), listky);
            tvar(polmesiac(-1.0), listky);
            tvar(elipsa(10.0, 15.2, 2.4, 2.9, 90.0), pysk);
            tvar(elipsa(10.0, 4.4, 1.2, 1.2, 0.0), stlpik);
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::tema::{LAN, NOC};

    #[test]
    fn znak_je_v_mriezke_a_nie_je_kruh_s_bodkami() {
        for z in ZNAKY {
            for t in [&NOC, &LAN] {
                let mut k = Kreslic::novy(40.0, 40.0, 1.0, t.plocha);
                kresli(&mut k, z, t, 10.0, 10.0, 20.0);
                // nič mimo mriežky 20 × 20 (s okrajom 1 bodu na vyhladenie)
                for (i, &b) in k.p.body.iter().enumerate() {
                    let (x, y) = ((i % 40) as f32 + 0.5, (i / 40) as f32 + 0.5);
                    if b != t.plocha {
                        assert!(
                            (9.0..=31.0).contains(&x) && (9.0..=31.0).contains(&y),
                            "{z:?}: bod ({x}, {y})"
                        );
                    }
                }
                // nie plný kruh: rohy štvorca opísaného znaku ostanú prázdne aj blízko stredu hrán
                let pokryte = k.p.body.iter().filter(|&&b| b != t.plocha).count();
                assert!(pokryte < 300, "{z:?}: znak zaberá {pokryte} bodov – príliš plný (kruh má ~314)");
                assert!(pokryte > 120, "{z:?}: znak je príliš malý ({pokryte} bodov)");
            }
        }
    }

    #[test]
    fn mena_navrhov_su_jedinecne() {
        for (i, a) in ZNAKY.iter().enumerate() {
            assert!(ZNAKY[i + 1..].iter().all(|b| b.meno() != a.meno()));
        }
    }
}
