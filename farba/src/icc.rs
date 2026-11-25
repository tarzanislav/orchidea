//! Profily ICC (ICC.1:2022 – verzie 2 a 4): čítanie hlavičky a značiek, modely matica + TRC, sivá TRC a
//! tabuľkové modely (lut8, lut16, lutAtoB, lutBtoA s krivkami, maticou a mriežkou CLUT), prevod zariadenie ↔
//! spojovací priestor (PCS, XYZ D50) a zápis profilu v4 s maticou a krivkami.

use crate::matica::{self, M3, V3};
use crate::prenos::{Krivka, Prenos};
use crate::primarne::{self, Adaptacia, Primarne};
use orchidea_jadro::{Chyba, Vysledok};

fn chyba(s: &str) -> Chyba {
    Chyba::PoskodeneData(format!("ICC: {s}"))
}

/// Najväčší prijatý profil.
const NAJVIAC: usize = 64 << 20;

/// Zámer prevodu (rendering intent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zamer {
    Percepcny = 0,
    RelativnyKolorimetricky = 1,
    Sytost = 2,
    AbsolutnyKolorimetricky = 3,
}

/// Priestor zariadenia podľa hlavičky.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriestorZariadenia {
    Rgb,
    Sivy,
    Cmyk,
    Lab,
    Xyz,
    Iny(usize),
}

impl PriestorZariadenia {
    /// Počet kanálov.
    pub fn kanalov(self) -> usize {
        match self {
            PriestorZariadenia::Sivy => 1,
            PriestorZariadenia::Cmyk => 4,
            PriestorZariadenia::Iny(n) => n,
            _ => 3,
        }
    }

    fn z_podpisu(s: &[u8]) -> PriestorZariadenia {
        match s {
            b"RGB " => PriestorZariadenia::Rgb,
            b"GRAY" => PriestorZariadenia::Sivy,
            b"CMYK" => PriestorZariadenia::Cmyk,
            b"Lab " => PriestorZariadenia::Lab,
            b"XYZ " => PriestorZariadenia::Xyz,
            b"CMY " | b"YCbr" | b"Luv " | b"Yxy " | b"HSV " | b"HLS " => PriestorZariadenia::Iny(3),
            _ => {
                // „2CLR“ … „FCLR“: n farieb
                if s.len() == 4 && &s[1..] == b"CLR" {
                    let n = (s[0] as char).to_digit(16).unwrap_or(3) as usize;
                    PriestorZariadenia::Iny(n.max(1))
                } else {
                    PriestorZariadenia::Iny(3)
                }
            }
        }
    }
}

/// Mriežka CLUT: vstupy (prvý sa mení najpomalšie) → výstupy, hodnoty 0–1.
#[derive(Debug, Clone)]
pub(crate) struct Clut {
    body: Vec<usize>,
    vystupov: usize,
    data: Vec<f64>,
    kroky: Vec<usize>,
    /// Multilineárna interpolácia aj v posledných troch rozmeroch (vstup Lab: neutrálna os nie je uhlopriečka
    /// kocky, štvorsteny by ju lámali – Kasson a kol., 1995).
    pub(crate) trilinearna: bool,
}

impl Clut {
    pub(crate) fn nova(body: Vec<usize>, vystupov: usize, data: Vec<f64>) -> Clut {
        let n = body.len();
        let mut kroky = vec![0usize; n];
        let mut k = vystupov;
        for i in (0..n).rev() {
            kroky[i] = k;
            k *= body[i];
        }
        Clut { body, vystupov, data, kroky, trilinearna: false }
    }

    pub(crate) fn hodnota(&self, vstup: &[f64], von: &mut [f64]) {
        self.rekurzia(vstup, &self.kroky, 0, 0, von);
    }

    fn rekurzia(&self, v: &[f64], kroky: &[usize], d: usize, zaklad: usize, von: &mut [f64]) {
        let zostava = self.body.len() - d;
        if zostava == 3 && !self.trilinearna {
            self.tetraeder(v, kroky, d, zaklad, von);
            return;
        }
        if zostava == 0 {
            von.copy_from_slice(&self.data[zaklad..zaklad + self.vystupov]);
            return;
        }
        // lineárne v tomto rozmere, zvyšok rekurzívne
        let mriezka = self.body[d] - 1;
        let p = v[d].clamp(0.0, 1.0) * mriezka as f64;
        let i = (p as usize).min(mriezka.saturating_sub(1));
        let z = p - i as f64;
        let mut a = vec![0f64; self.vystupov];
        self.rekurzia(v, kroky, d + 1, zaklad + i * kroky[d], &mut a);
        if mriezka == 0 || z == 0.0 {
            von.copy_from_slice(&a);
            return;
        }
        let mut b = vec![0f64; self.vystupov];
        self.rekurzia(v, kroky, d + 1, zaklad + (i + 1) * kroky[d], &mut b);
        for ((o, &x), &y) in von.iter_mut().zip(&a).zip(&b) {
            *o = x + (y - x) * z;
        }
    }

    /// Štvorstenová interpolácia v posledných troch rozmeroch.
    fn tetraeder(&self, v: &[f64], kroky: &[usize], d: usize, zaklad: usize, von: &mut [f64]) {
        let mut idx = [0usize; 3];
        let mut r = [0f64; 3];
        let mut dalsi = [0usize; 3];
        for k in 0..3 {
            let m = self.body[d + k] - 1;
            let p = v[d + k].clamp(0.0, 1.0) * m as f64;
            let i = (p as usize).min(m.saturating_sub(1));
            idx[k] = i;
            r[k] = if m == 0 { 0.0 } else { p - i as f64 };
            dalsi[k] = if m == 0 { 0 } else { kroky[d + k] };
        }
        let b0 = zaklad + idx[0] * kroky[d] + idx[1] * kroky[d + 1] + idx[2] * kroky[d + 2];
        let (x1, y1, z1) = (dalsi[0], dalsi[1], dalsi[2]);
        let (rx, ry, rz) = (r[0], r[1], r[2]);
        let g = |o: usize, c: usize| self.data[b0 + o + c];
        for (c, out) in von.iter_mut().enumerate() {
            let c0 = g(0, c);
            let (c1, c2, c3) = if rx >= ry && ry >= rz {
                (g(x1, c) - c0, g(x1 + y1, c) - g(x1, c), g(x1 + y1 + z1, c) - g(x1 + y1, c))
            } else if rx >= rz && rz >= ry {
                (g(x1, c) - c0, g(x1 + y1 + z1, c) - g(x1 + z1, c), g(x1 + z1, c) - g(x1, c))
            } else if rz >= rx && rx >= ry {
                (g(x1 + z1, c) - g(z1, c), g(x1 + y1 + z1, c) - g(x1 + z1, c), g(z1, c) - c0)
            } else if ry >= rx && rx >= rz {
                (g(x1 + y1, c) - g(y1, c), g(y1, c) - c0, g(x1 + y1 + z1, c) - g(x1 + y1, c))
            } else if ry >= rz && rz >= rx {
                (g(x1 + y1 + z1, c) - g(y1 + z1, c), g(y1, c) - c0, g(y1 + z1, c) - g(y1, c))
            } else {
                (g(x1 + y1 + z1, c) - g(y1 + z1, c), g(y1 + z1, c) - g(z1, c), g(z1, c) - c0)
            };
            *out = c0 + c1 * rx + c2 * ry + c3 * rz;
        }
    }
}

/// Kódovanie PCS v tabuľkách.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KodovaniePcs {
    Xyz,
    /// Lab verzie 4 (a 8-bitové lut8): L = u · 100, a = u · 255 − 128.
    Lab4,
    /// Lab verzie 2 v lut16: 0xFF00 = 100, 0x8000 = 0.
    Lab2,
}

/// Tabuľkový model (jeden smer).
#[derive(Debug, Clone)]
struct Lut {
    vstupov: usize,
    vystupov: usize,
    /// Smer A → B (zariadenie → PCS) alebo B → A.
    a_na_b: bool,
    /// Poradie lut8/lut16: matica (len pri vstupe XYZ), vstupné krivky, CLUT, výstupné krivky.
    stary: bool,
    matica_staru: Option<M3>,
    krivky_a: Vec<Krivka>,
    clut: Option<Clut>,
    krivky_m: Vec<Krivka>,
    matica: Option<(M3, V3)>,
    krivky_b: Vec<Krivka>,
    pcs: KodovaniePcs,
}

impl Lut {
    /// Vyhodnotí tabuľku: vstup aj výstup v 0–1 (strana PCS v kódovaní `pcs`).
    fn hodnota(&self, vstup: &[f64]) -> Vec<f64> {
        let mut v: Vec<f64> = vstup.to_vec();
        let krivky = |v: &mut Vec<f64>, k: &[Krivka]| {
            for (x, k) in v.iter_mut().zip(k) {
                *x = k.hodnota(*x);
            }
        };
        let clut = |v: &mut Vec<f64>, c: &Option<Clut>, vystupov: usize| {
            if let Some(c) = c {
                let mut o = vec![0f64; c.vystupov];
                c.hodnota(v, &mut o);
                *v = o;
            } else {
                v.resize(vystupov, 0.0);
            }
        };
        let matica = |v: &mut Vec<f64>, m: &Option<(M3, V3)>| {
            if let (Some((m, o)), true) = (m, v.len() >= 3) {
                let r = matica::aplikuj(m, [v[0], v[1], v[2]]);
                v[..3].copy_from_slice(&[r[0] + o[0], r[1] + o[1], r[2] + o[2]]);
            }
        };
        if self.stary {
            if let Some(m) = &self.matica_staru {
                let r = matica::aplikuj(m, [v[0], v[1], v[2]]);
                v[..3].copy_from_slice(&r);
            }
            krivky(&mut v, &self.krivky_a);
            clut(&mut v, &self.clut, self.vystupov);
            krivky(&mut v, &self.krivky_b);
        } else if self.a_na_b {
            // A → CLUT → M → matica → B
            krivky(&mut v, &self.krivky_a);
            clut(&mut v, &self.clut, self.vystupov);
            krivky(&mut v, &self.krivky_m);
            matica(&mut v, &self.matica);
            krivky(&mut v, &self.krivky_b);
        } else {
            // B → matica → M → CLUT → A
            krivky(&mut v, &self.krivky_b);
            matica(&mut v, &self.matica);
            krivky(&mut v, &self.krivky_m);
            clut(&mut v, &self.clut, self.vystupov);
            krivky(&mut v, &self.krivky_a);
        }
        v.truncate(self.vystupov);
        v
    }
}

fn pcs_na_xyz(u: &[f64], k: KodovaniePcs) -> V3 {
    match k {
        KodovaniePcs::Xyz => {
            let s = 65535.0 / 32768.0;
            [u[0] * s, u[1] * s, u[2] * s]
        }
        KodovaniePcs::Lab4 => lab_na_xyz([u[0] * 100.0, u[1] * 255.0 - 128.0, u[2] * 255.0 - 128.0]),
        KodovaniePcs::Lab2 => lab_na_xyz([
            u[0] * 65535.0 / 65280.0 * 100.0,
            u[1] * 65535.0 / 256.0 - 128.0,
            u[2] * 65535.0 / 256.0 - 128.0,
        ]),
    }
}

fn xyz_na_pcs(x: V3, k: KodovaniePcs) -> Vec<f64> {
    match k {
        KodovaniePcs::Xyz => {
            let s = 32768.0 / 65535.0;
            vec![x[0] * s, x[1] * s, x[2] * s]
        }
        KodovaniePcs::Lab4 => {
            let l = xyz_na_lab(x);
            vec![l[0] / 100.0, (l[1] + 128.0) / 255.0, (l[2] + 128.0) / 255.0]
        }
        KodovaniePcs::Lab2 => {
            let l = xyz_na_lab(x);
            vec![
                l[0] / 100.0 * 65280.0 / 65535.0,
                (l[1] + 128.0) * 256.0 / 65535.0,
                (l[2] + 128.0) * 256.0 / 65535.0,
            ]
        }
    }
}

/// CIE L*a*b* (D50 podľa ICC) → XYZ.
pub fn lab_na_xyz([l, a, b]: V3) -> V3 {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let e = 6.0 / 29.0;
    let f = |t: f64| if t > e { t * t * t } else { 3.0 * e * e * (t - 4.0 / 29.0) };
    let w = primarne::ICC_D50;
    [w[0] * f(fx), w[1] * f(fy), w[2] * f(fz)]
}

/// XYZ → CIE L*a*b* (D50).
pub fn xyz_na_lab(x: V3) -> V3 {
    let w = primarne::ICC_D50;
    let e = 216.0 / 24389.0;
    let f = |t: f64| {
        if t > e {
            orchidea_jadro::matematika::mocnina(t, 1.0 / 3.0)
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x[0] / w[0]), f(x[1] / w[1]), f(x[2] / w[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Model profilu.
#[derive(Debug, Clone)]
enum Model {
    MaticaTrc { matica: M3, trc: [Krivka; 3] },
    Siva { trc: Krivka },
    Ziadny,
}

/// Načítaný profil ICC.
#[derive(Debug, Clone)]
pub struct Profil {
    pub verzia: (u8, u8),
    pub trieda: [u8; 4],
    pub priestor: PriestorZariadenia,
    pub pcs_lab: bool,
    pub zamer: u32,
    /// Biely bod média (wtpt).
    pub biely: Option<V3>,
    /// Popis (desc).
    pub popis: Option<String>,
    model: Model,
    a2b: [Option<Lut>; 3],
    b2a: [Option<Lut>; 3],
}

struct Citac<'a> {
    d: &'a [u8],
}

impl<'a> Citac<'a> {
    fn u8_(&self, p: usize) -> Vysledok<u8> {
        self.d.get(p).copied().ok_or_else(|| chyba("údaje za koncom"))
    }
    fn u16_(&self, p: usize) -> Vysledok<u16> {
        Ok(u16::from_be_bytes([self.u8_(p)?, self.u8_(p + 1)?]))
    }
    fn u32_(&self, p: usize) -> Vysledok<u32> {
        Ok(u32::from_be_bytes([self.u8_(p)?, self.u8_(p + 1)?, self.u8_(p + 2)?, self.u8_(p + 3)?]))
    }
    fn s15f16(&self, p: usize) -> Vysledok<f64> {
        Ok(self.u32_(p)? as i32 as f64 / 65536.0)
    }
    fn u8f8(&self, p: usize) -> Vysledok<f64> {
        Ok(self.u16_(p)? as f64 / 256.0)
    }
    fn kus(&self, p: usize, n: usize) -> Vysledok<&'a [u8]> {
        p.checked_add(n).and_then(|k| self.d.get(p..k)).ok_or_else(|| chyba("údaje za koncom"))
    }
    fn xyz(&self, p: usize) -> Vysledok<V3> {
        Ok([self.s15f16(p)?, self.s15f16(p + 4)?, self.s15f16(p + 8)?])
    }
}

/// Krivka typu `curv` alebo `para` na pozícii `p`; vráti krivku a dĺžku v bajtoch.
fn citaj_krivku(c: &Citac, p: usize) -> Vysledok<(Krivka, usize)> {
    let typ = c.kus(p, 4)?;
    match typ {
        b"curv" => {
            let n = c.u32_(p + 8)? as usize;
            if n > 1 << 20 {
                return Err(chyba("priveľká tabuľka krivky"));
            }
            let k = match n {
                0 => Krivka::Identita,
                1 => Krivka::Gama(c.u8f8(p + 12)?),
                _ => {
                    c.kus(p + 12, 2 * n)?;
                    let mut t = Vec::with_capacity(n);
                    for i in 0..n {
                        t.push(c.u16_(p + 12 + 2 * i)? as f64 / 65535.0);
                    }
                    Krivka::Tabulka(t)
                }
            };
            Ok((k, 12 + 2 * n))
        }
        b"para" => {
            let typ = c.u16_(p + 8)?;
            let n = match typ {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return Err(chyba("neznáma parametrická krivka")),
            };
            let mut prm = [0f64; 7];
            for (i, x) in prm.iter_mut().enumerate().take(n) {
                *x = c.s15f16(p + 12 + 4 * i)?;
            }
            if typ != 0 && prm[1] == 0.0 {
                return Err(chyba("parametrická krivka s a = 0"));
            }
            Ok((Krivka::Parametricka { typ, p: prm }, 12 + 4 * n))
        }
        _ => Err(chyba("neznámy typ krivky")),
    }
}

fn citaj_krivky(c: &Citac, p: usize, n: usize) -> Vysledok<Vec<Krivka>> {
    let mut v = Vec::with_capacity(n);
    let mut q = p;
    for _ in 0..n {
        let (k, dlzka) = citaj_krivku(c, q)?;
        v.push(k);
        q += dlzka.div_ceil(4) * 4;
    }
    Ok(v)
}

fn citaj_clut(c: &Citac, p: usize, body: Vec<usize>, vystupov: usize, presnost: usize) -> Vysledok<Clut> {
    let mut n = vystupov;
    for &b in &body {
        if b == 0 {
            return Err(chyba("mriežka CLUT s nulovým počtom bodov"));
        }
        n = n.checked_mul(b).filter(|&n| n <= 1 << 24).ok_or_else(|| chyba("priveľká mriežka CLUT"))?;
    }
    // údaje musia byť v profile skôr, než sa pre ne vyhradí pamäť
    c.kus(p, n * presnost.max(1))?;
    let mut data = Vec::with_capacity(n);
    for i in 0..n {
        data.push(match presnost {
            1 => c.u8_(p + i)? as f64 / 255.0,
            _ => c.u16_(p + 2 * i)? as f64 / 65535.0,
        });
    }
    Ok(Clut::nova(body, vystupov, data))
}

fn citaj_lut(c: &Citac, p: usize, a_na_b: bool, pcs: KodovaniePcs, vstup_xyz: bool) -> Vysledok<Lut> {
    let typ = c.kus(p, 4)?;
    let vstupov = c.u8_(p + 8)? as usize;
    let vystupov = c.u8_(p + 9)? as usize;
    if vstupov == 0 || vystupov == 0 || vstupov > 15 || vystupov > 15 {
        return Err(chyba("neplatný počet kanálov tabuľky"));
    }
    match typ {
        b"mft1" | b"mft2" => {
            let body = c.u8_(p + 10)? as usize;
            let mut m = [[0f64; 3]; 3];
            for (i, r) in m.iter_mut().enumerate() {
                for (j, x) in r.iter_mut().enumerate() {
                    *x = c.s15f16(p + 12 + 4 * (3 * i + j))?;
                }
            }
            let (presnost, mut q, n_vst, n_vys) = if typ == b"mft1" {
                (1usize, p + 48, 256usize, 256usize)
            } else {
                (2, p + 52, c.u16_(p + 48)? as usize, c.u16_(p + 50)? as usize)
            };
            if !(2..=4096).contains(&n_vst) || !(2..=4096).contains(&n_vys) {
                return Err(chyba("neplatná dĺžka tabuľky lut16"));
            }
            let tab = |q: &mut usize, n: usize| -> Vysledok<Krivka> {
                let mut t = Vec::with_capacity(n);
                for i in 0..n {
                    t.push(if presnost == 1 {
                        c.u8_(*q + i)? as f64 / 255.0
                    } else {
                        c.u16_(*q + 2 * i)? as f64 / 65535.0
                    });
                }
                *q += n * presnost;
                Ok(Krivka::Tabulka(t))
            };
            let mut a = Vec::with_capacity(vstupov);
            for _ in 0..vstupov {
                a.push(tab(&mut q, n_vst)?);
            }
            let clut = citaj_clut(c, q, vec![body; vstupov], vystupov, presnost)?;
            q += clut.data.len() * presnost;
            let mut b = Vec::with_capacity(vystupov);
            for _ in 0..vystupov {
                b.push(tab(&mut q, n_vys)?);
            }
            let pcs = if typ == b"mft1" && pcs == KodovaniePcs::Lab2 { KodovaniePcs::Lab4 } else { pcs };
            Ok(Lut {
                vstupov,
                vystupov,
                a_na_b,
                stary: true,
                // matica sa použije len pri vstupe XYZ (napr. B2A s PCS XYZ)
                matica_staru: (vstup_xyz && vstupov == 3).then_some(m),
                krivky_a: a,
                clut: Some(clut),
                krivky_m: Vec::new(),
                matica: None,
                krivky_b: b,
                pcs,
            })
        }
        b"mAB " | b"mBA " => {
            let off = |i: usize| -> Vysledok<usize> { Ok(c.u32_(p + 12 + 4 * i)? as usize) };
            // poradie posunov podľa ICC.1 (10.12, 10.13): krivky B, matica, krivky M, CLUT, krivky A
            let (ob, om, omm, oc, oa) = (off(0)?, off(1)?, off(2)?, off(3)?, off(4)?);
            // pri A → B sú krivky A na strane vstupu (počet vstupov), B na strane PCS (výstupy); pri B → A naopak
            let (na, nb) = if a_na_b { (vstupov, vystupov) } else { (vystupov, vstupov) };
            let krivky_b = if ob != 0 { citaj_krivky(c, p + ob, nb)? } else { Vec::new() };
            let krivky_m = if omm != 0 { citaj_krivky(c, p + omm, nb)? } else { Vec::new() };
            let krivky_a = if oa != 0 { citaj_krivky(c, p + oa, na)? } else { Vec::new() };
            let matica = if om != 0 {
                let mut m = [[0f64; 3]; 3];
                for (i, r) in m.iter_mut().enumerate() {
                    for (j, x) in r.iter_mut().enumerate() {
                        *x = c.s15f16(p + om + 4 * (3 * i + j))?;
                    }
                }
                let o = [c.s15f16(p + om + 36)?, c.s15f16(p + om + 40)?, c.s15f16(p + om + 44)?];
                Some((m, o))
            } else {
                None
            };
            let clut = if oc != 0 {
                // mriežka má vždy toľko rozmerov, koľko má značka vstupov, a toľko hodnôt, koľko výstupov
                let q = p + oc;
                let mut body = Vec::with_capacity(vstupov);
                for i in 0..vstupov {
                    body.push(c.u8_(q + i)? as usize);
                }
                let presnost = c.u8_(q + 16)? as usize;
                if presnost != 1 && presnost != 2 {
                    return Err(chyba("neplatná presnosť CLUT"));
                }
                Some(citaj_clut(c, q + 20, body, vystupov, presnost)?)
            } else {
                None
            };
            Ok(Lut {
                vstupov,
                vystupov,
                a_na_b,
                stary: false,
                matica_staru: None,
                krivky_a,
                clut,
                krivky_m,
                matica,
                krivky_b,
                pcs: if pcs == KodovaniePcs::Lab2 { KodovaniePcs::Lab4 } else { pcs },
            })
        }
        _ => Err(chyba("nepodporovaný typ tabuľky")),
    }
}

fn citaj_text(c: &Citac, p: usize, n: usize) -> Option<String> {
    let typ = c.kus(p, 4).ok()?;
    match typ {
        b"desc" => {
            let l = c.u32_(p + 8).ok()? as usize;
            let t = c.kus(p + 12, l.min(n.saturating_sub(12))).ok()?;
            Some(String::from_utf8_lossy(t).trim_end_matches('\0').to_string())
        }
        b"text" => {
            let t = c.kus(p + 8, n.saturating_sub(8)).ok()?;
            Some(String::from_utf8_lossy(t).trim_end_matches('\0').to_string())
        }
        b"mluc" => {
            let pocet = c.u32_(p + 8).ok()? as usize;
            if pocet == 0 {
                return None;
            }
            let dlzka = c.u32_(p + 20).ok()? as usize;
            let posun = c.u32_(p + 24).ok()? as usize;
            let t = c.kus(p + posun, dlzka).ok()?;
            let u: Vec<u16> = t.chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
            Some(String::from_utf16_lossy(&u).trim_end_matches('\0').to_string())
        }
        _ => None,
    }
}

impl Profil {
    /// Prečíta profil z bajtov.
    pub fn citaj(d: &[u8]) -> Vysledok<Profil> {
        if d.len() < 132 || d.len() > NAJVIAC {
            return Err(chyba("neplatná veľkosť profilu"));
        }
        let c = Citac { d };
        if c.kus(36, 4)? != b"acsp" {
            return Err(chyba("chýba podpis acsp"));
        }
        let verzia = (c.u8_(8)?, c.u8_(9)? >> 4);
        let mut trieda = [0u8; 4];
        trieda.copy_from_slice(c.kus(12, 4)?);
        let priestor = PriestorZariadenia::z_podpisu(c.kus(16, 4)?);
        let pcs_lab = c.kus(20, 4)? == b"Lab ";
        let zamer = c.u32_(64)?;
        let pocet = c.u32_(128)? as usize;
        if pocet > 1000 {
            return Err(chyba("priveľa značiek"));
        }
        let mut znacky: Vec<([u8; 4], usize, usize)> = Vec::with_capacity(pocet);
        for i in 0..pocet {
            let q = 132 + 12 * i;
            let mut z = [0u8; 4];
            z.copy_from_slice(c.kus(q, 4)?);
            let posun = c.u32_(q + 4)? as usize;
            let dlzka = c.u32_(q + 8)? as usize;
            if posun.checked_add(dlzka).is_none_or(|k| k > d.len()) {
                return Err(chyba("značka mimo profilu"));
            }
            znacky.push((z, posun, dlzka));
        }
        let najdi = |z: &[u8; 4]| znacky.iter().find(|t| &t.0 == z).map(|t| (t.1, t.2));
        let biely = najdi(b"wtpt").and_then(|(p, _)| c.xyz(p + 8).ok());
        let popis = najdi(b"desc").and_then(|(p, n)| citaj_text(&c, p, n));
        let krivka = |z: &[u8; 4]| -> Vysledok<Option<Krivka>> {
            match najdi(z) {
                Some((p, _)) => Ok(Some(citaj_krivku(&c, p)?.0)),
                None => Ok(None),
            }
        };
        let model = match (priestor, najdi(b"rXYZ"), najdi(b"gXYZ"), najdi(b"bXYZ")) {
            (PriestorZariadenia::Rgb, Some(r), Some(g), Some(b)) => {
                let (r, g, b) = (c.xyz(r.0 + 8)?, c.xyz(g.0 + 8)?, c.xyz(b.0 + 8)?);
                let matica = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
                match (krivka(b"rTRC")?, krivka(b"gTRC")?, krivka(b"bTRC")?) {
                    (Some(kr), Some(kg), Some(kb)) => Model::MaticaTrc { matica, trc: [kr, kg, kb] },
                    _ => Model::Ziadny,
                }
            }
            (PriestorZariadenia::Sivy, ..) => match krivka(b"kTRC")? {
                Some(trc) => Model::Siva { trc },
                None => Model::Ziadny,
            },
            _ => Model::Ziadny,
        };
        let pcs = if pcs_lab { KodovaniePcs::Lab2 } else { KodovaniePcs::Xyz };
        let mut a2b: [Option<Lut>; 3] = [None, None, None];
        let mut b2a: [Option<Lut>; 3] = [None, None, None];
        let kanalov = priestor.kanalov();
        let sedi = |l: &Lut, vstup: usize, vystup: usize| -> Vysledok<()> {
            if l.vstupov == vstup && l.vystupov == vystup {
                Ok(())
            } else {
                Err(chyba("tabuľka nesedí s priestorom profilu"))
            }
        };
        // tabuľky iných typov (napr. „mpet“ z ICC v4.3+/iccMAX) sa vynechajú – profil sa použije cez maticu
        // s krivkami alebo cez ostatné tabuľky; ak nezostane nič, je to chyba nižšie
        let najdi_lut = |z: &[u8; 4]| {
            najdi(z).filter(|&(p, _)| matches!(c.kus(p, 4), Ok(b"mft1" | b"mft2" | b"mAB " | b"mBA ")))
        };
        for i in 0..3 {
            let za = [b'A', b'2', b'B', b'0' + i as u8];
            let zb = [b'B', b'2', b'A', b'0' + i as u8];
            if let Some((p, _)) = najdi_lut(&za) {
                let l = citaj_lut(&c, p, true, pcs, false)?;
                sedi(&l, kanalov, 3)?;
                a2b[i] = Some(l);
            }
            if let Some((p, _)) = najdi_lut(&zb) {
                let mut l = citaj_lut(&c, p, false, pcs, !pcs_lab)?;
                sedi(&l, 3, kanalov)?;
                if pcs_lab && let Some(cl) = l.clut.as_mut() {
                    cl.trilinearna = true;
                }
                b2a[i] = Some(l);
            }
        }
        if matches!(model, Model::Ziadny)
            && a2b.iter().all(|l| l.is_none())
            && b2a.iter().all(|l| l.is_none())
        {
            return Err(chyba("profil nemá maticu s krivkami ani tabuľky"));
        }
        Ok(Profil { verzia, trieda, priestor, pcs_lab, zamer, biely, popis, model, a2b, b2a })
    }

    /// Počet kanálov zariadenia.
    pub fn kanalov(&self) -> usize {
        self.priestor.kanalov()
    }

    /// Profil má priamy smer (zariadenie → PCS).
    pub fn ma_do_pcs(&self, z: Zamer) -> bool {
        self.lut_a2b(z).is_some() || !matches!(self.model, Model::Ziadny)
    }

    /// Profil má spätný smer (PCS → zariadenie).
    pub fn ma_z_pcs(&self, z: Zamer) -> bool {
        self.lut_b2a(z).is_some() || matches!(self.model, Model::MaticaTrc { .. } | Model::Siva { .. })
    }

    /// Priamy smer ide cez tabuľku (nie cez maticu s krivkami).
    pub fn ma_tabulku_do_pcs(&self, z: Zamer) -> bool {
        self.lut_a2b(z).is_some()
    }

    /// Spätný smer ide cez tabuľku.
    pub fn ma_tabulku_z_pcs(&self, z: Zamer) -> bool {
        self.lut_b2a(z).is_some()
    }

    fn index(z: Zamer) -> usize {
        match z {
            Zamer::Percepcny => 0,
            Zamer::RelativnyKolorimetricky | Zamer::AbsolutnyKolorimetricky => 1,
            Zamer::Sytost => 2,
        }
    }

    fn lut_a2b(&self, z: Zamer) -> Option<&Lut> {
        self.a2b[Profil::index(z)].as_ref().or(self.a2b[0].as_ref())
    }

    fn lut_b2a(&self, z: Zamer) -> Option<&Lut> {
        self.b2a[Profil::index(z)].as_ref().or(self.b2a[0].as_ref())
    }

    /// Matica lineárne RGB → XYZ (D50), ak je profil s maticou.
    pub fn matica(&self) -> Option<M3> {
        match &self.model {
            Model::MaticaTrc { matica, .. } => Some(*matica),
            _ => None,
        }
    }

    /// Krivky TRC (pre sivý profil jedna).
    pub fn krivky(&self) -> Option<Vec<Krivka>> {
        match &self.model {
            Model::MaticaTrc { trc, .. } => Some(trc.to_vec()),
            Model::Siva { trc } => Some(vec![trc.clone()]),
            Model::Ziadny => None,
        }
    }

    /// Hodnoty zariadenia (0–1) → XYZ v PCS (D50, relatívne: biela média Y = 1).
    pub fn do_pcs(&self, v: &[f64], z: Zamer) -> V3 {
        if let Some(l) = self.lut_a2b(z) {
            let mut vstup = v.to_vec();
            vstup.resize(l.vstupov, 0.0);
            return pcs_na_xyz(&l.hodnota(&vstup), l.pcs);
        }
        match &self.model {
            Model::MaticaTrc { matica, trc } => {
                let lin = [trc[0].hodnota(v[0]), trc[1].hodnota(v[1]), trc[2].hodnota(v[2])];
                matica::aplikuj(matica, lin)
            }
            Model::Siva { trc } => {
                let y = trc.hodnota(v[0]);
                if self.pcs_lab {
                    // pri PCS Lab dáva krivka L* (0–1 → 0–100), a* = b* = 0
                    return lab_na_xyz([y * 100.0, 0.0, 0.0]);
                }
                let w = primarne::ICC_D50;
                [w[0] * y, y, w[2] * y]
            }
            Model::Ziadny => [0.0; 3],
        }
    }

    /// XYZ v PCS → hodnoty zariadenia (0–1, mimo gamutu neorezané pri modeli s maticou).
    pub fn z_pcs(&self, x: V3, z: Zamer) -> Vec<f64> {
        if let Some(l) = self.lut_b2a(z) {
            return l.hodnota(&xyz_na_pcs(x, l.pcs));
        }
        match &self.model {
            Model::MaticaTrc { matica, trc } => {
                let inv = matica::inverzna(matica).unwrap_or(matica::JEDNOTKOVA);
                let lin = matica::aplikuj(&inv, x);
                (0..3).map(|i| trc[i].inverzna(lin[i])).collect()
            }
            Model::Siva { trc } => {
                let y = if self.pcs_lab { xyz_na_lab(x)[0] / 100.0 } else { x[1] };
                vec![trc.inverzna(y)]
            }
            Model::Ziadny => vec![0.0; self.kanalov()],
        }
    }
}

// ---------------------------------------------------------------- zápis

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn znacka_xyz(x: V3) -> Vec<u8> {
    let mut v = b"XYZ \0\0\0\0".to_vec();
    for c in x {
        v.extend_from_slice(&s15f16(c));
    }
    v
}

fn znacka_mluc(text: &str) -> Vec<u8> {
    let u: Vec<u8> = text.encode_utf16().flat_map(|c| c.to_be_bytes()).collect();
    let mut v = b"mluc\0\0\0\0".to_vec();
    v.extend_from_slice(&1u32.to_be_bytes());
    v.extend_from_slice(&12u32.to_be_bytes());
    v.extend_from_slice(b"enUS");
    v.extend_from_slice(&(u.len() as u32).to_be_bytes());
    v.extend_from_slice(&28u32.to_be_bytes());
    v.extend_from_slice(&u);
    v
}

fn znacka_krivka(k: &Krivka) -> Vec<u8> {
    match k {
        Krivka::Parametricka { typ, p } => {
            let n = [1, 3, 4, 5, 7][(*typ as usize).min(4)];
            let mut v = b"para\0\0\0\0".to_vec();
            v.extend_from_slice(&typ.to_be_bytes());
            v.extend_from_slice(&[0, 0]);
            for &x in &p[..n] {
                v.extend_from_slice(&s15f16(x));
            }
            v
        }
        Krivka::Gama(g) => {
            let mut v = b"para\0\0\0\0\0\0\0\0".to_vec();
            v.extend_from_slice(&s15f16(*g));
            v
        }
        Krivka::Identita => b"curv\0\0\0\0\0\0\0\0".to_vec(),
        Krivka::Tabulka(t) => {
            let mut v = b"curv\0\0\0\0".to_vec();
            v.extend_from_slice(&(t.len() as u32).to_be_bytes());
            for &x in t {
                v.extend_from_slice(&((x.clamp(0.0, 1.0) * 65535.0).round() as u16).to_be_bytes());
            }
            v
        }
    }
}

/// Zapíše profil ICC v4.3 s maticou a krivkami pre lineárne RGB s danými základnými farbami a prenosom
/// (prenos bez presnej krivky ICC sa zapíše ako tabuľka s 4096 bodmi).
pub fn zapis_rgb(p: &Primarne, prenos: &Prenos, popis: &str) -> Vec<u8> {
    let chad = primarne::adaptacia(primarne::xy_na_xyz(p.biely), primarne::ICC_D50, Adaptacia::Bradford);
    let m = matica::nasob(&chad, &p.na_xyz());
    let krivka = prenos.ako_krivka().unwrap_or_else(|| {
        Krivka::Tabulka((0..4096).map(|i| prenos.do_linearneho(i as f64 / 4095.0)).collect())
    });
    let mut chad_v = b"sf32\0\0\0\0".to_vec();
    for r in &chad {
        for &x in r {
            chad_v.extend_from_slice(&s15f16(x));
        }
    }
    let trc = znacka_krivka(&krivka);
    let znacky: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"desc", znacka_mluc(popis)),
        (*b"cprt", znacka_mluc("Bez obmedzení (ORCHIDEA)")),
        (*b"wtpt", znacka_xyz(primarne::ICC_D50)),
        (*b"chad", chad_v),
        (*b"rXYZ", znacka_xyz([m[0][0], m[1][0], m[2][0]])),
        (*b"gXYZ", znacka_xyz([m[0][1], m[1][1], m[2][1]])),
        (*b"bXYZ", znacka_xyz([m[0][2], m[1][2], m[2][2]])),
        (*b"rTRC", trc),
    ];
    let mut tabulka = Vec::new();
    let mut data = Vec::new();
    let zaciatok = 128 + 4 + 12 * (znacky.len() + 2);
    for (z, d) in &znacky {
        let posun = zaciatok + data.len();
        tabulka.push((*z, posun, d.len()));
        data.extend_from_slice(d);
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    // gTRC a bTRC zdieľajú údaje rTRC
    let r = tabulka[tabulka.len() - 1];
    tabulka.push((*b"gTRC", r.1, r.2));
    tabulka.push((*b"bTRC", r.1, r.2));
    let velkost = zaciatok + data.len();
    let mut v = Vec::with_capacity(velkost);
    v.extend_from_slice(&(velkost as u32).to_be_bytes());
    v.extend_from_slice(&[0; 4]); // CMM
    v.extend_from_slice(&[4, 0x30, 0, 0]); // verzia 4.3
    v.extend_from_slice(b"mntrRGB XYZ ");
    v.extend_from_slice(&[0x07, 0xEA, 0, 11, 0, 24, 0, 0, 0, 0, 0, 0]); // dátum (2026-11-24)
    v.extend_from_slice(b"acsp");
    v.extend_from_slice(&[0; 4]); // platforma
    v.extend_from_slice(&[0; 4]); // príznaky
    v.extend_from_slice(&[0; 8]); // výrobca, model
    v.extend_from_slice(&[0; 8]); // atribúty
    v.extend_from_slice(&[0; 4]); // zámer
    for x in primarne::ICC_D50 {
        v.extend_from_slice(&s15f16(x));
    }
    v.extend_from_slice(b"ORCH");
    v.extend_from_slice(&[0; 16]); // ID profilu
    v.resize(128, 0);
    v.extend_from_slice(&(tabulka.len() as u32).to_be_bytes());
    for (z, p, n) in &tabulka {
        v.extend_from_slice(z);
        v.extend_from_slice(&(*p as u32).to_be_bytes());
        v.extend_from_slice(&(*n as u32).to_be_bytes());
    }
    v.extend_from_slice(&data);
    v
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn zapis_a_citanie_srgb() {
        let d = zapis_rgb(&primarne::SRGB, &Prenos::Srgb, "sRGB");
        let p = Profil::citaj(&d).unwrap();
        assert_eq!(p.priestor, PriestorZariadenia::Rgb);
        assert_eq!(p.popis.as_deref(), Some("sRGB"));
        // biela → D50, späť na bielu
        let w = p.do_pcs(&[1.0, 1.0, 1.0], Zamer::RelativnyKolorimetricky);
        for i in 0..3 {
            assert!((w[i] - primarne::ICC_D50[i]).abs() < 1e-3, "{w:?}");
        }
        let s = p.z_pcs(
            p.do_pcs(&[0.2, 0.5, 0.9], Zamer::RelativnyKolorimetricky),
            Zamer::RelativnyKolorimetricky,
        );
        assert!((s[0] - 0.2).abs() < 1e-6 && (s[1] - 0.5).abs() < 1e-6 && (s[2] - 0.9).abs() < 1e-6, "{s:?}");
    }

    #[test]
    fn neznamy_typ_tabulky_sa_vynecha() {
        // značka chad (sf32) sa premenuje na A2B0 typu „mpet“ – profil sa má prečítať cez maticu s krivkami
        let mut d = zapis_rgb(&primarne::SRGB, &Prenos::Srgb, "sRGB");
        let n = u32::from_be_bytes([d[128], d[129], d[130], d[131]]) as usize;
        for i in 0..n {
            let q = 132 + 12 * i;
            if &d[q..q + 4] == b"chad" {
                let p = u32::from_be_bytes([d[q + 4], d[q + 5], d[q + 6], d[q + 7]]) as usize;
                d[q..q + 4].copy_from_slice(b"A2B0");
                d[p..p + 4].copy_from_slice(b"mpet");
            }
        }
        let p = Profil::citaj(&d).unwrap();
        assert!(!p.ma_tabulku_do_pcs(Zamer::Percepcny) && p.matica().is_some());
    }

    #[test]
    fn lab_tam_a_spat() {
        for x in [[0.5, 0.4, 0.3], [0.01, 0.002, 0.03], [0.9642, 1.0, 0.8249]] {
            let y = lab_na_xyz(xyz_na_lab(x));
            for i in 0..3 {
                assert!((x[i] - y[i]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn poskodene_profily_bez_padu() {
        let d = zapis_rgb(&primarne::SRGB, &Prenos::Srgb, "x");
        for i in 0..d.len() {
            let mut e = d.clone();
            e[i] ^= 0xA5;
            let _ = Profil::citaj(&e);
            let _ = Profil::citaj(&d[..i]);
        }
    }
}
