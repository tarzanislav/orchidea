//! Prevod farieb medzi priestormi cez spojovací priestor XYZ (D50, relatívne ako PCS v ICC).
//!
//! Dve rýchle cesty: (1) oba priestory sú lineárne modely (krivky + matica: parametrické RGB, sivá, profily ICC
//! s maticou) – výsledok je jedna matica 3 × 3 medzi krivkami, celé čísla idú cez tabuľky dekódovania a presné
//! zaokrúhlenie prahmi; (2) inak (tabuľky ICC, CMYK, Lab) sa pre celé čísla vytvorí mriežka zariadenie → zariadenie
//! (33 bodov na os pri 3 vstupoch, 17 pri 4) so štvorstenovou interpoláciou. Plávajúce vzorky sa počítajú presne.

use crate::icc::{self, Clut, Profil, Zamer};
use crate::matica::{self, M3, V3};
use crate::prenos::Prenos;
use crate::primarne::{self, Adaptacia, Primarne};
use orchidea_jadro::paralel;
use orchidea_jadro::{Chyba, Vysledok};
use std::sync::Arc;

/// Farebný priestor.
#[derive(Debug, Clone)]
pub enum Priestor {
    /// RGB so základnými farbami a prenosovou funkciou (rovnaká pre všetky kanály).
    Rgb { primarne: Primarne, prenos: Prenos },
    /// Odtiene sivej (achromatické, biela = biela spojovacieho priestoru).
    Sivy { prenos: Prenos },
    /// Profil ICC.
    Icc(Arc<Profil>),
    /// CIE XYZ (D50, relatívne; biela Y = 1).
    XyzD50,
    /// CIE L*a*b* (D50).
    Lab,
}

impl Priestor {
    /// sRGB (IEC 61966-2-1).
    pub fn srgb() -> Priestor {
        Priestor::Rgb { primarne: primarne::SRGB, prenos: Prenos::Srgb }
    }

    /// Lineárne sRGB (BT.709, lineárne svetlo).
    pub fn linearne_srgb() -> Priestor {
        Priestor::Rgb { primarne: primarne::SRGB, prenos: Prenos::Linearny }
    }

    /// Display P3.
    pub fn display_p3() -> Priestor {
        Priestor::Rgb { primarne: primarne::DISPLAY_P3, prenos: Prenos::Srgb }
    }

    /// Adobe RGB (1998), gama 563/256.
    pub fn adobe_rgb() -> Priestor {
        Priestor::Rgb { primarne: primarne::ADOBE_RGB, prenos: Prenos::Gama(563.0 / 256.0) }
    }

    /// ProPhoto RGB (ROMM).
    pub fn prophoto() -> Priestor {
        Priestor::Rgb { primarne: primarne::PROPHOTO, prenos: Prenos::ProPhoto }
    }

    /// ACES2065-1 (AP0, lineárne).
    pub fn aces2065() -> Priestor {
        Priestor::Rgb { primarne: primarne::ACES_AP0, prenos: Prenos::Linearny }
    }

    /// ACEScg (AP1, lineárne).
    pub fn acescg() -> Priestor {
        Priestor::Rgb { primarne: primarne::ACES_AP1, prenos: Prenos::Linearny }
    }

    /// ACEScct (AP1, logaritmické s pätou).
    pub fn acescct() -> Priestor {
        Priestor::Rgb { primarne: primarne::ACES_AP1, prenos: Prenos::Acescct }
    }

    /// Priestor podľa kódov ITU-T H.273 (cICP v PNG, JPEG XL, video). Nepodporované kombinácie → `None`.
    pub fn z_cicp(zakladne: u8, prenos: u8) -> Option<Priestor> {
        let p = match zakladne {
            1 => primarne::SRGB,
            5 => Primarne { r: (0.64, 0.33), g: (0.29, 0.60), b: (0.15, 0.06), biely: primarne::D65 },
            6 | 7 => {
                Primarne { r: (0.630, 0.340), g: (0.310, 0.595), b: (0.155, 0.070), biely: primarne::D65 }
            }
            9 => primarne::BT2020,
            11 => primarne::DCI_P3,
            12 => primarne::DISPLAY_P3,
            22 => Primarne { r: (0.630, 0.340), g: (0.295, 0.605), b: (0.155, 0.077), biely: primarne::D65 },
            _ => return None,
        };
        let t = match prenos {
            1 | 6 | 14 | 15 => Prenos::Bt709,
            4 => Prenos::Gama(2.2),
            5 => Prenos::Gama(2.8),
            8 => Prenos::Linearny,
            13 => Prenos::Srgb,
            16 => Prenos::Pq,
            18 => Prenos::Hlg,
            _ => return None,
        };
        Some(Priestor::Rgb { primarne: p, prenos: t })
    }

    /// Priestor z bajtov profilu ICC.
    pub fn z_icc(data: &[u8]) -> Vysledok<Priestor> {
        Ok(Priestor::Icc(Arc::new(Profil::citaj(data)?)))
    }

    /// Počet farebných kanálov.
    pub fn kanalov(&self) -> usize {
        match self {
            Priestor::Sivy { .. } => 1,
            Priestor::Icc(p) => p.kanalov(),
            _ => 3,
        }
    }
}

/// Zostavená strana prevodu.
#[derive(Debug, Clone)]
enum Strana {
    /// Krivky po kanáloch a matica lineárne → XYZ D50 (`z_xyz` opačne); sivá má jeden kanál.
    Linearna {
        krivky: Vec<Prenos>,
        do_xyz: M3,
        z_xyz: M3,
    },
    Icc(Arc<Profil>),
    Xyz,
    Lab,
}

fn chyba(s: &str) -> Chyba {
    Chyba::NezhodaTvaru(format!("prevod farieb: {s}"))
}

impl Strana {
    fn zostav(p: &Priestor, zamer: Zamer, ako_ciel: bool) -> Vysledok<(Strana, V3)> {
        let d50 = primarne::ICC_D50;
        Ok(match p {
            Priestor::Rgb { primarne, prenos } => {
                let a = primarne::adaptacia(primarne::xy_na_xyz(primarne.biely), d50, Adaptacia::Bradford);
                let do_xyz = matica::nasob(&a, &primarne.na_xyz());
                let z_xyz = matica::inverzna(&do_xyz).ok_or_else(|| chyba("základné farby bez inverzie"))?;
                (Strana::Linearna { krivky: vec![prenos.clone(); 3], do_xyz, z_xyz }, d50)
            }
            Priestor::Sivy { prenos } => (Strana::siva(prenos.clone()), d50),
            Priestor::Icc(profil) => {
                let biely = profil.biely.unwrap_or(d50);
                let ma = if ako_ciel { profil.ma_z_pcs(zamer) } else { profil.ma_do_pcs(zamer) };
                if !ma {
                    return Err(chyba("profil nemá potrebný smer"));
                }
                let tabulka =
                    if ako_ciel { profil.ma_tabulku_z_pcs(zamer) } else { profil.ma_tabulku_do_pcs(zamer) };
                if !tabulka {
                    if let (Some(m), Some(k)) = (profil.matica(), profil.krivky())
                        && let Some(z_xyz) = matica::inverzna(&m)
                    {
                        let krivky = k.into_iter().map(Prenos::Krivka).collect();
                        return Ok((Strana::Linearna { krivky, do_xyz: m, z_xyz }, biely));
                    }
                    if let Some(k) = profil.krivky().filter(|k| k.len() == 1 && !profil.pcs_lab) {
                        return Ok((Strana::siva(Prenos::Krivka(k[0].clone())), biely));
                    }
                }
                (Strana::Icc(profil.clone()), biely)
            }
            Priestor::XyzD50 => (Strana::Xyz, d50),
            Priestor::Lab => (Strana::Lab, d50),
        })
    }

    /// Čierny bod strany v XYZ D50 (neutrálny, relatívny kolorimetrický zámer), podľa postupu Adobe pre
    /// kompenzáciu čierneho bodu:
    /// - zdroj RGB a sivá: najtmavšia farba zariadenia (0),
    /// - cieľ s tabuľkami a CMYK aj ako zdroj: cesta tam a späť Lab(0, 0, 0) → zariadenie → Lab – najtmavšia farba,
    ///   ktorú profil naozaj vie (pri CMYK nie je nutne 100 % všetkých farieb – limit nánosu),
    /// - XYZ a Lab: nulová čierna.
    fn cierny_bod(&self, ako_ciel: bool) -> V3 {
        let rel = Zamer::RelativnyKolorimetricky;
        match self {
            Strana::Linearna { .. } => {
                let nula = vec![0.0; self.kanalov()];
                neutralny(self.do_xyz(&nula, rel))
            }
            Strana::Icc(p) => {
                let cmyk = p.priestor == icc::PriestorZariadenia::Cmyk;
                if (ako_ciel || cmyk) && p.ma_z_pcs(rel) && p.ma_do_pcs(rel) {
                    let zariadenie = p.z_pcs(icc::lab_na_xyz([0.0, 0.0, 0.0]), rel);
                    return neutralny(p.do_pcs(&zariadenie, rel));
                }
                let vstup: Vec<f64> = if cmyk { vec![1.0; 4] } else { vec![0.0; p.kanalov()] };
                if p.ma_do_pcs(rel) { neutralny(p.do_pcs(&vstup, rel)) } else { [0.0; 3] }
            }
            Strana::Xyz | Strana::Lab => [0.0; 3],
        }
    }

    fn siva(prenos: Prenos) -> Strana {
        let w = primarne::ICC_D50;
        Strana::Linearna {
            krivky: vec![prenos],
            do_xyz: [[w[0], 0.0, 0.0], [w[1], 0.0, 0.0], [w[2], 0.0, 0.0]],
            z_xyz: [[0.0, 1.0, 0.0], [0.0; 3], [0.0; 3]],
        }
    }

    fn kanalov(&self) -> usize {
        match self {
            Strana::Linearna { krivky, .. } => krivky.len(),
            Strana::Icc(p) => p.kanalov(),
            _ => 3,
        }
    }

    fn do_xyz(&self, v: &[f64], z: Zamer) -> V3 {
        match self {
            Strana::Linearna { krivky, do_xyz, .. } => {
                let mut l = [0.0; 3];
                for (i, k) in krivky.iter().enumerate() {
                    l[i] = k.do_linearneho(v[i]);
                }
                matica::aplikuj(do_xyz, l)
            }
            Strana::Icc(p) => p.do_pcs(v, z),
            Strana::Xyz => [v[0], v[1], v[2]],
            Strana::Lab => icc::lab_na_xyz([v[0], v[1], v[2]]),
        }
    }

    fn z_xyz(&self, x: V3, z: Zamer, von: &mut [f64]) {
        match self {
            Strana::Linearna { krivky, z_xyz, .. } => {
                let l = matica::aplikuj(z_xyz, x);
                for (i, k) in krivky.iter().enumerate() {
                    von[i] = k.z_linearneho(l[i]);
                }
            }
            Strana::Icc(p) => {
                let v = p.z_pcs(x, z);
                von[..v.len()].copy_from_slice(&v);
            }
            Strana::Xyz => von[..3].copy_from_slice(&x),
            Strana::Lab => von[..3].copy_from_slice(&icc::xyz_na_lab(x)),
        }
    }

    /// Celé číslo (podiel `u` = k / max) → prirodzené hodnoty priestoru.
    fn z_podielu(&self, u: f64, kanal: usize) -> f64 {
        match self {
            Strana::Lab => {
                if kanal == 0 {
                    u * 100.0
                } else {
                    u * 255.0 - 128.0
                }
            }
            Strana::Xyz => u * 65535.0 / 32768.0,
            _ => u,
        }
    }

    fn na_podiel(&self, v: f64, kanal: usize) -> f64 {
        match self {
            Strana::Lab => {
                if kanal == 0 {
                    v / 100.0
                } else {
                    (v + 128.0) / 255.0
                }
            }
            Strana::Xyz => v * 32768.0 / 65535.0,
            _ => v,
        }
    }
}

/// Zostavený prevod.
#[derive(Debug, Clone)]
pub struct Prevod {
    zdroj: Strana,
    ciel: Strana,
    zamer: Zamer,
    /// Pomer bielych médií pri absolútnom kolorimetrickom zámere.
    pomer: Option<V3>,
    /// Lineárne zdroja → lineárne cieľa (obe strany lineárne modely).
    matica: Option<M3>,
    /// Kompenzácia čierneho bodu: x' = x · mierka + posun (v XYZ D50, po zložkách).
    kompenzacia: Option<(V3, V3)>,
}

/// Voľby prevodu.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Volby {
    /// Kompenzácia čierneho bodu (ISO 18619): čierna zdroja sa zobrazí na čiernu cieľa, biela ostane – tiene sa
    /// nezlejú a neostanú sivé. Len pri relatívnom kolorimetrickom zámere (percepčný zámer mapuje čiernu sám
    /// tabuľkami profilu; absolútny ju zachováva zámerne).
    pub kompenzacia_ciernej: bool,
}

/// Bod v XYZ → neutrálny bod s rovnakým L* (a* = b* = 0) – čierny bod sa berie ako neutrálny.
fn neutralny(x: V3) -> V3 {
    let lab = icc::xyz_na_lab(x);
    icc::lab_na_xyz([lab[0].clamp(0.0, 50.0), 0.0, 0.0])
}

/// Mriežka pre celé čísla (prevod zariadenie → zariadenie).
#[derive(Debug)]
struct Mriezka {
    clut: Clut,
}

impl Prevod {
    /// Zostaví prevod zo zdroja do cieľa.
    pub fn novy(zdroj: &Priestor, ciel: &Priestor, zamer: Zamer) -> Vysledok<Prevod> {
        let (z, bz) = Strana::zostav(zdroj, zamer, false)?;
        let (c, bc) = Strana::zostav(ciel, zamer, true)?;
        let pomer = (zamer == Zamer::AbsolutnyKolorimetricky)
            .then(|| [bz[0] / bc[0], bz[1] / bc[1], bz[2] / bc[2]])
            .filter(|p| p.iter().all(|x| x.is_finite() && *x > 0.0));
        let matica = match (&z, &c) {
            (Strana::Linearna { do_xyz, .. }, Strana::Linearna { z_xyz, .. }) => {
                let d = matica::diagonala(pomer.unwrap_or([1.0; 3]));
                Some(matica::nasob(z_xyz, &matica::nasob(&d, do_xyz)))
            }
            _ => None,
        };
        Ok(Prevod { zdroj: z, ciel: c, zamer, pomer, matica, kompenzacia: None })
    }

    /// Zostaví prevod s voľbami (napríklad s kompenzáciou čierneho bodu).
    pub fn s_volbami(zdroj: &Priestor, ciel: &Priestor, zamer: Zamer, volby: Volby) -> Vysledok<Prevod> {
        let mut p = Prevod::novy(zdroj, ciel, zamer)?;
        if volby.kompenzacia_ciernej && zamer == Zamer::RelativnyKolorimetricky {
            let bz = p.zdroj.cierny_bod(false);
            let bc = p.ciel.cierny_bod(true);
            // postup Adobe (ISO 18619): po zložkách XYZ medzi čiernym bodom a bielou D50
            let w = primarne::ICC_D50;
            let mut mierka = [1.0; 3];
            let mut posun = [0.0; 3];
            for i in 0..3 {
                let t = (w[i] - bc[i]) / (w[i] - bz[i]);
                mierka[i] = t;
                posun[i] = bc[i] - bz[i] * t;
            }
            if (0..3).any(|i| (mierka[i] - 1.0).abs() > 1e-9 || posun[i].abs() > 1e-9)
                && mierka.iter().all(|m| m.is_finite() && *m > 0.0)
            {
                p.kompenzacia = Some((mierka, posun));
                // cesta len cez maticu nevie posun – prevod pôjde cez XYZ
                p.matica = None;
            }
        }
        Ok(p)
    }

    /// XYZ (D50) zo vstupu zdroja vrátane pomeru bielych a kompenzácie čierneho bodu.
    fn do_xyz(&self, vstup: &[f64]) -> V3 {
        let mut x = self.zdroj.do_xyz(vstup, self.zamer);
        if let Some(p) = self.pomer {
            x = [x[0] * p[0], x[1] * p[1], x[2] * p[2]];
        }
        if let Some((m, o)) = self.kompenzacia {
            x = [x[0] * m[0] + o[0], x[1] * m[1] + o[1], x[2] * m[2] + o[2]];
        }
        x
    }

    /// Počet vstupných farebných kanálov.
    pub fn vstupov(&self) -> usize {
        self.zdroj.kanalov()
    }

    /// Počet výstupných farebných kanálov.
    pub fn vystupov(&self) -> usize {
        self.ciel.kanalov()
    }

    /// Prevedie jeden bod (prirodzené hodnoty: RGB/sivá/CMYK 0–1, Lab L 0–100 a a, b, XYZ s bielou Y = 1).
    pub fn bod(&self, vstup: &[f64], von: &mut [f64]) {
        if let (Some(m), Strana::Linearna { krivky: kz, .. }, Strana::Linearna { krivky: kc, .. }) =
            (&self.matica, &self.zdroj, &self.ciel)
        {
            let mut l = [0.0; 3];
            for (i, k) in kz.iter().enumerate() {
                l[i] = k.do_linearneho(vstup[i]);
            }
            let l = matica::aplikuj(m, l);
            for (i, k) in kc.iter().enumerate() {
                von[i] = k.z_linearneho(l[i]);
            }
            return;
        }
        let x = self.do_xyz(vstup);
        self.ciel.z_xyz(x, self.zamer, von);
    }

    /// Prevod plávajúcich vzoriek (kanály za sebou). Kanály nad počet farebných (alfa …) sa skopírujú
    /// na koniec výstupu v rovnakom poradí, ak ich výstup má.
    pub fn preved_f32(&self, vstup: &[f32], kanalov_vstup: usize, kanalov_von: usize) -> Vysledok<Vec<f32>> {
        let (nv, nc) = self.kontrola(vstup.len(), kanalov_vstup, kanalov_von)?;
        let bodov = vstup.len() / kanalov_vstup;
        let mut von = vec![0f32; bodov * kanalov_von];
        paralel::pre_kusy(&mut von, 4096 * kanalov_von, |kus, o| {
            let zaciatok = kus * 4096;
            let mut a = [0f64; 16];
            let mut b = [0f64; 16];
            for (i, ob) in o.chunks_exact_mut(kanalov_von).enumerate() {
                let iv = &vstup[(zaciatok + i) * kanalov_vstup..(zaciatok + i + 1) * kanalov_vstup];
                for c in 0..nv {
                    a[c] = iv[c] as f64;
                }
                self.bod(&a[..nv], &mut b[..nc]);
                for c in 0..nc {
                    ob[c] = b[c] as f32;
                }
                for (d, s) in ob[nc..].iter_mut().zip(&iv[nv..]) {
                    *d = *s;
                }
            }
        });
        Ok(von)
    }

    /// Prevod 8-bitových vzoriek.
    pub fn preved_u8(&self, vstup: &[u8], kanalov_vstup: usize, kanalov_von: usize) -> Vysledok<Vec<u8>> {
        self.preved_cele(vstup, kanalov_vstup, kanalov_von, 255)
    }

    /// Prevod 16-bitových vzoriek.
    pub fn preved_u16(&self, vstup: &[u16], kanalov_vstup: usize, kanalov_von: usize) -> Vysledok<Vec<u16>> {
        self.preved_cele(vstup, kanalov_vstup, kanalov_von, 65535)
    }

    fn kontrola(&self, n: usize, kv: usize, kc: usize) -> Vysledok<(usize, usize)> {
        let (nv, nc) = (self.vstupov(), self.vystupov());
        if kv < nv || kc < nc || kv == 0 || !n.is_multiple_of(kv) || nv > 16 || nc > 16 {
            return Err(chyba("počet kanálov nesedí s priestormi"));
        }
        Ok((nv, nc))
    }

    fn preved_cele<T: Cele>(&self, vstup: &[T], kv: usize, kc: usize, max: u32) -> Vysledok<Vec<T>> {
        let (nv, nc) = self.kontrola(vstup.len(), kv, kc)?;
        let bodov = vstup.len() / kv;
        let mut von = vec![T::z_u32(0); bodov * kc];
        let maxf = max as f64;
        // zakódovanie lineárneho cieľa presným zaokrúhlením prahmi (ak je krivka rastúca)
        let (krivky_ciela, prahy): (Vec<Prenos>, Vec<Option<Vec<f64>>>) = match &self.ciel {
            Strana::Linearna { krivky, .. } => (
                krivky.clone(),
                krivky
                    .iter()
                    .map(|k| {
                        let t: Vec<f64> =
                            (0..max).map(|i| k.do_linearneho((i as f64 + 0.5) / maxf)).collect();
                        t.windows(2).all(|w| w[0] < w[1]).then_some(t)
                    })
                    .collect(),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        let zakoduj = |c: usize, l: f64| -> u32 {
            match &prahy[c] {
                Some(t) => t.partition_point(|&p| p <= l) as u32,
                None => (krivky_ciela[c].z_linearneho(l).clamp(0.0, 1.0) * maxf).round() as u32,
            }
        };
        match (&self.matica, &self.zdroj, &self.ciel) {
            (Some(m), Strana::Linearna { krivky: kz, .. }, Strana::Linearna { .. }) => {
                // dekódovanie tabuľkou, matica, zakódovanie prahmi
                let dek: Vec<Vec<f64>> = kz
                    .iter()
                    .map(|k| (0..=max).map(|i| k.do_linearneho(i as f64 / maxf)).collect())
                    .collect();
                paralel::pre_kusy(&mut von, 8192 * kc, |kus, o| {
                    let zaciatok = kus * 8192;
                    for (i, ob) in o.chunks_exact_mut(kc).enumerate() {
                        let iv = &vstup[(zaciatok + i) * kv..(zaciatok + i + 1) * kv];
                        let mut l = [0.0; 3];
                        for c in 0..nv {
                            l[c] = dek[c][iv[c].na_u32() as usize];
                        }
                        let l = matica::aplikuj(m, l);
                        for c in 0..nc {
                            ob[c] = T::z_u32(zakoduj(c, l[c]));
                        }
                        for (d, s) in ob[nc..].iter_mut().zip(&iv[nv..]) {
                            *d = T::z_u32(s.na_u32().min(max));
                        }
                    }
                });
            }
            _ if nv > 4 || bodov < uzlov_mriezky(nv) => {
                // veľa kanálov (n-farebné profily) alebo málo bodov: mriežka by bola priveľká či zbytočná –
                // presne po bodoch
                paralel::pre_kusy(&mut von, 1024 * kc, |kus, o| {
                    let zaciatok = kus * 1024;
                    let mut a = [0f64; 16];
                    let mut b = [0f64; 16];
                    for (i, ob) in o.chunks_exact_mut(kc).enumerate() {
                        let iv = &vstup[(zaciatok + i) * kv..(zaciatok + i + 1) * kv];
                        for c in 0..nv {
                            a[c] = self.zdroj.z_podielu(iv[c].na_u32() as f64 / maxf, c);
                        }
                        self.bod(&a[..nv], &mut b[..nc]);
                        for c in 0..nc {
                            ob[c] = T::z_u32(
                                (self.ciel.na_podiel(b[c], c).clamp(0.0, 1.0) * maxf).round() as u32
                            );
                        }
                        for (d, s) in ob[nc..].iter_mut().zip(&iv[nv..]) {
                            *d = T::z_u32(s.na_u32().min(max));
                        }
                    }
                });
            }
            _ => {
                let mriezka = self.mriezka();
                let linearny_ciel = !prahy.is_empty();
                paralel::pre_kusy(&mut von, 8192 * kc, |kus, o| {
                    let zaciatok = kus * 8192;
                    let mut a = [0f64; 16];
                    let mut b = [0f64; 16];
                    for (i, ob) in o.chunks_exact_mut(kc).enumerate() {
                        let iv = &vstup[(zaciatok + i) * kv..(zaciatok + i + 1) * kv];
                        for c in 0..nv {
                            a[c] = iv[c].na_u32() as f64 / maxf;
                        }
                        mriezka.clut.hodnota(&a[..nv], &mut b[..nc]);
                        for c in 0..nc {
                            ob[c] = T::z_u32(if linearny_ciel {
                                zakoduj(c, b[c])
                            } else {
                                (b[c].clamp(0.0, 1.0) * maxf).round() as u32
                            });
                        }
                        for (d, s) in ob[nc..].iter_mut().zip(&iv[nv..]) {
                            *d = T::z_u32(s.na_u32().min(max));
                        }
                    }
                });
            }
        }
        Ok(von)
    }

    /// Mriežka zariadenie → zariadenie v podieloch 0–1.
    fn mriezka(&self) -> Mriezka {
        let (nv, nc) = (self.vstupov(), self.vystupov());
        let body = bodov_mriezky(nv);
        let pocet = uzlov_mriezky(nv);
        let mut data = vec![0f64; pocet * nc];
        let mut a = vec![0f64; nv];
        let mut b = vec![0f64; nc];
        for (i, o) in data.chunks_exact_mut(nc).enumerate() {
            let mut r = i;
            for c in (0..nv).rev() {
                let u = (r % body) as f64 / (body - 1) as f64;
                a[c] = self.zdroj.z_podielu(u, c);
                r /= body;
            }
            match &self.ciel {
                Strana::Linearna { z_xyz, .. } => {
                    let x = self.do_xyz(&a);
                    let l = matica::aplikuj(z_xyz, x);
                    o.copy_from_slice(&l[..nc]);
                }
                _ => {
                    self.bod(&a, &mut b);
                    for c in 0..nc {
                        o[c] = self.ciel.na_podiel(b[c], c);
                    }
                }
            }
        }
        let mut clut = Clut::nova(vec![body; nv], nc, data);
        clut.trilinearna = matches!(self.zdroj, Strana::Lab);
        Mriezka { clut }
    }
}

/// Body mriežky na jednu os podľa počtu vstupov.
fn bodov_mriezky(nv: usize) -> usize {
    match nv {
        1 => 4096,
        2 => 257,
        3 => 33,
        _ => 17,
    }
}

fn uzlov_mriezky(nv: usize) -> usize {
    bodov_mriezky(nv).pow(nv.min(4) as u32)
}

/// Celočíselné vzorky.
trait Cele: Copy + Send + Sync {
    fn na_u32(self) -> u32;
    fn z_u32(v: u32) -> Self;
}

impl Cele for u8 {
    fn na_u32(self) -> u32 {
        self as u32
    }
    fn z_u32(v: u32) -> u8 {
        v.min(255) as u8
    }
}

impl Cele for u16 {
    fn na_u32(self) -> u32 {
        self as u32
    }
    fn z_u32(v: u32) -> u16 {
        v.min(65535) as u16
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn srgb_na_srgb_je_identita() {
        let p = Prevod::novy(&Priestor::srgb(), &Priestor::srgb(), Zamer::RelativnyKolorimetricky).unwrap();
        let v: Vec<u8> = (0..=255).flat_map(|i| [i as u8, (255 - i) as u8, (i * 7 % 256) as u8]).collect();
        assert_eq!(p.preved_u8(&v, 3, 3).unwrap(), v);
        let v16: Vec<u16> =
            (0..=65535).step_by(97).flat_map(|i| [i as u16, (65535 - i) as u16, 300]).collect();
        assert_eq!(p.preved_u16(&v16, 3, 3).unwrap(), v16);
    }

    #[test]
    fn tam_a_spat_cez_profil() {
        let d = icc::zapis_rgb(&primarne::ADOBE_RGB, &Prenos::Gama(563.0 / 256.0), "Adobe");
        let a = Priestor::z_icc(&d).unwrap();
        let tam = Prevod::novy(&Priestor::srgb(), &a, Zamer::Percepcny).unwrap();
        let spat = Prevod::novy(&a, &Priestor::srgb(), Zamer::Percepcny).unwrap();
        let mut x = [0.0; 3];
        let mut y = [0.0; 3];
        tam.bod(&[0.2, 0.6, 0.9], &mut x);
        spat.bod(&x, &mut y);
        assert!((y[0] - 0.2).abs() < 1e-6 && (y[1] - 0.6).abs() < 1e-6 && (y[2] - 0.9).abs() < 1e-6, "{y:?}");
        // porovnanie s parametrickým priestorom (presnosť s15Fixed16)
        let par = Prevod::novy(&Priestor::srgb(), &Priestor::adobe_rgb(), Zamer::Percepcny).unwrap();
        let mut z = [0.0; 3];
        par.bod(&[0.2, 0.6, 0.9], &mut z);
        for i in 0..3 {
            assert!((x[i] - z[i]).abs() < 2e-4, "{x:?} {z:?}");
        }
    }

    #[test]
    fn alfa_sa_skopiruje_a_siva() {
        let p = Prevod::novy(&Priestor::Sivy { prenos: Prenos::Srgb }, &Priestor::srgb(), Zamer::Percepcny)
            .unwrap();
        let v = p.preved_u8(&[0, 10, 128, 200, 255, 255], 2, 4).unwrap();
        assert_eq!(v, vec![0, 0, 0, 10, 128, 128, 128, 200, 255, 255, 255, 255]);
        let l = Prevod::novy(&Priestor::srgb(), &Priestor::Lab, Zamer::Percepcny).unwrap();
        let mut o = [0.0; 3];
        l.bod(&[1.0, 1.0, 1.0], &mut o);
        assert!((o[0] - 100.0).abs() < 1e-9 && o[1].abs() < 1e-9 && o[2].abs() < 1e-9, "{o:?}");
    }
}
