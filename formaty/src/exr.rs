//! OpenEXR (súborový formát 2.0, OpenEXR 3) – obraz s vysokým dynamickým rozsahom v lineárnom svetle scény.
//!
//! **Čítanie:** jednodielne aj viacdielne súbory (číta sa prvá časť), riadky aj dlaždice (úroveň 0 aj pri
//! mipmapách), vzorky HALF, FLOAT a UINT, podvzorkované kanály; kompresia NONE, RLE, ZIPS, ZIP, PIZ (vlnková
//! transformácia + Huffman), PXR24 a B44/B44A. Kompresia DWAA/DWAB a hlboké (deep) obrazy zatiaľ nie.
//! Kanály R, G, B, A sa usporiadajú ako RGB(A); iné kanály a vrstvy ostanú pod svojimi menami.
//!
//! **Zápis:** riadky s kanálmi HALF alebo FLOAT, kompresia ZIP (predvolená), ZIPS, RLE alebo NONE.

use crate::deflate;
use crate::obraz::{Limity, Model, Obraz, Vzorky};
use crate::polovicna::{f16_na_f32, f32_na_f16};
use orchidea_jadro::{Chyba, Vysledok};

pub const PODPIS: [u8; 4] = [0x76, 0x2F, 0x31, 0x01];

fn chyba(text: &str) -> Chyba {
    Chyba::PoskodeneData(format!("OpenEXR: {text}"))
}

/// Typ vzoriek kanála.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypVzorky {
    Uint,
    Half,
    Float,
}

impl TypVzorky {
    fn bajtov(self) -> usize {
        match self {
            TypVzorky::Half => 2,
            _ => 4,
        }
    }
}

#[derive(Debug, Clone)]
struct Kanal {
    meno: String,
    typ: TypVzorky,
    linearny: bool,
    xs: i32,
    ys: i32,
}

/// Kompresia blokov.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kompresia {
    Ziadna,
    Rle,
    Zips,
    Zip,
    Piz,
    Pxr24,
    B44,
    B44a,
    Dwaa,
    Dwab,
}

impl Kompresia {
    fn z_cisla(c: u8) -> Vysledok<Kompresia> {
        Ok(match c {
            0 => Kompresia::Ziadna,
            1 => Kompresia::Rle,
            2 => Kompresia::Zips,
            3 => Kompresia::Zip,
            4 => Kompresia::Piz,
            5 => Kompresia::Pxr24,
            6 => Kompresia::B44,
            7 => Kompresia::B44a,
            8 => Kompresia::Dwaa,
            9 => Kompresia::Dwab,
            _ => return Err(chyba(&format!("neznáma kompresia {c}"))),
        })
    }

    fn cislo(self) -> u8 {
        self as u8
    }

    /// Riadkov v jednom bloku (pri riadkovom ukladaní).
    fn riadkov(self) -> i32 {
        match self {
            Kompresia::Ziadna | Kompresia::Rle | Kompresia::Zips => 1,
            Kompresia::Zip | Kompresia::Pxr24 => 16,
            Kompresia::Piz | Kompresia::B44 | Kompresia::B44a | Kompresia::Dwaa => 32,
            Kompresia::Dwab => 256,
        }
    }
}

/// Obdĺžnik so začiatkom a koncom (vrátane), ako box2i v OpenEXR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Okno {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Okno {
    fn sirka(&self) -> i64 {
        self.x1 as i64 - self.x0 as i64 + 1
    }
    fn vyska(&self) -> i64 {
        self.y1 as i64 - self.y0 as i64 + 1
    }
}

#[derive(Debug, Clone, Copy)]
struct Dlazdice {
    sx: u32,
    sy: u32,
    uroven: u8,
    nahor: bool,
}

#[derive(Debug, Clone)]
struct Hlavicka {
    kanaly: Vec<Kanal>,
    kompresia: Kompresia,
    data: Okno,
    dlazdice: Option<Dlazdice>,
    chromaticity: Option<[f64; 8]>,
    hlboky: bool,
    texty: Vec<(String, String)>,
}

/// Čítač bajtov hlavičky (little-endian).
struct Citac<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Citac<'a> {
    fn bajty(&mut self, n: usize) -> Vysledok<&'a [u8]> {
        let k = self.p.checked_add(n).filter(|&k| k <= self.d.len()).ok_or_else(|| chyba("neúplný súbor"))?;
        let b = &self.d[self.p..k];
        self.p = k;
        Ok(b)
    }
    fn i32(&mut self) -> Vysledok<i32> {
        let b = self.bajty(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Vysledok<u64> {
        let b = self.bajty(8)?;
        let mut p = [0u8; 8];
        p.copy_from_slice(b);
        Ok(u64::from_le_bytes(p))
    }
    fn f32(&mut self) -> Vysledok<f32> {
        Ok(f32::from_bits(self.i32()? as u32))
    }
    fn retazec(&mut self) -> Vysledok<String> {
        let zvysok = &self.d[self.p.min(self.d.len())..];
        let n = zvysok.iter().position(|&b| b == 0).ok_or_else(|| chyba("neukončený reťazec"))?;
        if n > 255 {
            return Err(chyba("príliš dlhé meno"));
        }
        let s = String::from_utf8_lossy(&zvysok[..n]).into_owned();
        self.p += n + 1;
        Ok(s)
    }
}

fn hlavicka(c: &mut Citac, viacdielny: bool) -> Vysledok<Option<Hlavicka>> {
    let mut kanaly = None;
    let mut kompresia = None;
    let mut data = None;
    let mut dlazdice = None;
    let mut chromaticity = None;
    let mut hlboky = false;
    let mut texty = Vec::new();
    let mut prvy = true;
    loop {
        let meno = c.retazec()?;
        if meno.is_empty() {
            if prvy && viacdielny {
                return Ok(None); // koniec zoznamu hlavičiek
            }
            break;
        }
        prvy = false;
        let typ = c.retazec()?;
        let velkost = c.i32()?;
        if velkost < 0 {
            return Err(chyba("záporná veľkosť atribútu"));
        }
        let hodnota = c.bajty(velkost as usize)?;
        let mut h = Citac { d: hodnota, p: 0 };
        match (meno.as_str(), typ.as_str()) {
            ("channels", "chlist") => {
                let mut v = Vec::new();
                loop {
                    let m = h.retazec()?;
                    if m.is_empty() {
                        break;
                    }
                    let t = match h.i32()? {
                        0 => TypVzorky::Uint,
                        1 => TypVzorky::Half,
                        2 => TypVzorky::Float,
                        t => return Err(chyba(&format!("neznámy typ vzorky {t}"))),
                    };
                    let linearny = h.bajty(4)?[0] != 0;
                    let (xs, ys) = (h.i32()?, h.i32()?);
                    if !(1..=1 << 16).contains(&xs) || !(1..=1 << 16).contains(&ys) {
                        return Err(chyba("neplatné podvzorkovanie kanála"));
                    }
                    v.push(Kanal { meno: m, typ: t, linearny, xs, ys });
                    if v.len() > 1024 {
                        return Err(Chyba::Limit("OpenEXR: priveľa kanálov".into()));
                    }
                }
                kanaly = Some(v);
            }
            ("compression", "compression") => kompresia = Some(Kompresia::z_cisla(h.bajty(1)?[0])?),
            ("dataWindow", "box2i") => {
                data = Some(Okno { x0: h.i32()?, y0: h.i32()?, x1: h.i32()?, y1: h.i32()? })
            }
            ("tiles", "tiledesc") => {
                let (sx, sy) = (h.i32()? as u32, h.i32()? as u32);
                let m = h.bajty(1)?[0];
                dlazdice = Some(Dlazdice { sx, sy, uroven: m & 15, nahor: m & 16 != 0 });
            }
            ("chromaticities", "chromaticities") => {
                let mut ch = [0.0; 8];
                // poradie v súbore: červená, zelená, modrá, biela – ukladáme bielu prvú
                let mut v = [0.0f64; 8];
                for x in v.iter_mut() {
                    *x = h.f32()? as f64;
                }
                ch[..2].copy_from_slice(&v[6..8]);
                ch[2..].copy_from_slice(&v[..6]);
                chromaticity = Some(ch);
            }
            ("type", "string") => {
                let t = String::from_utf8_lossy(hodnota).into_owned();
                hlboky = t.starts_with("deep");
            }
            (m, "string") if matches!(m, "comments" | "owner" | "capDate" | "software" | "name") => {
                texty.push((m.to_string(), String::from_utf8_lossy(hodnota).into_owned()));
            }
            _ => {}
        }
    }
    let data = data.ok_or_else(|| chyba("chýba dataWindow"))?;
    if data.x1 < data.x0 || data.y1 < data.y0 {
        return Err(chyba("prázdne dataWindow"));
    }
    Ok(Some(Hlavicka {
        kanaly: kanaly.ok_or_else(|| chyba("chýbajú kanály"))?,
        kompresia: kompresia.ok_or_else(|| chyba("chýba kompresia"))?,
        data,
        dlazdice,
        chromaticity,
        hlboky,
        texty,
    }))
}

/// Počet vzoriek s podvzorkovaním `s` v rozsahu [a, b] (súradnice deliteľné `s`).
fn pocet_vzoriek(s: i32, a: i32, b: i32) -> usize {
    let s = s as i64;
    let a1 = (a as i64).div_euclid(s) * s;
    let a1 = if a1 < a as i64 { a1 + s } else { a1 };
    let b1 = (b as i64).div_euclid(s) * s;
    if b1 < a1 { 0 } else { ((b1 - a1) / s + 1) as usize }
}

/// Rozmery hlavičky prvej časti (šírka, výška) bez dekódovania.
pub fn citaj_hlavicku(data: &[u8]) -> Vysledok<(usize, usize)> {
    let (h, _, _) = otvor(data)?;
    Ok((h.data.sirka() as usize, h.data.vyska() as usize))
}

/// Hlavička prvej časti, poloha za všetkými hlavičkami (začiatok tabuliek polôh) a či je súbor viacdielny.
fn otvor(data: &[u8]) -> Vysledok<(Hlavicka, usize, bool)> {
    if data.len() < 8 || data[..4] != PODPIS {
        return Err(chyba("chýba podpis"));
    }
    let verzia = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if verzia & 0xFF != 2 {
        return Err(chyba(&format!("verzia {}", verzia & 0xFF)));
    }
    let viacdielny = verzia & 0x1000 != 0;
    if verzia & 0x800 != 0 && !viacdielny {
        return Err(chyba("hlboké (deep) obrazy nie sú podporené"));
    }
    let mut c = Citac { d: data, p: 8 };
    let mut prva = hlavicka(&mut c, false)?.ok_or_else(|| chyba("bez hlavičky"))?;
    if verzia & 0x200 != 0 && prva.dlazdice.is_none() {
        return Err(chyba("dlaždicový súbor bez popisu dlaždíc"));
    }
    if verzia & 0x200 == 0 && !viacdielny {
        prva.dlazdice = None;
    }
    let mut casti = 1;
    if viacdielny {
        while hlavicka(&mut c, true)?.is_some() {
            casti += 1;
            if casti > 10_000 {
                return Err(Chyba::Limit("OpenEXR: priveľa častí".into()));
            }
        }
    }
    Ok((prva, c.p, viacdielny))
}

/// Počet úrovní a veľkosť úrovne pri mipmapách (zaokrúhlenie nadol alebo nahor).
fn urovne(velkost: i64, nahor: bool) -> u32 {
    let mut n = 0u32;
    let mut v = velkost;
    while v > 1 {
        v = if nahor { (v + 1) / 2 } else { v / 2 };
        n += 1;
    }
    n + 1
}

fn velkost_urovne(velkost: i64, l: u32, nahor: bool) -> i64 {
    let d = 1i64 << l.min(62);
    (if nahor { (velkost + d - 1) / d } else { velkost / d }).max(1)
}

/// Počet blokov v tabuľke polôh (všetky úrovne dlaždíc).
fn pocet_blokov(h: &Hlavicka) -> Vysledok<u64> {
    let (w, v) = (h.data.sirka(), h.data.vyska());
    let Some(t) = h.dlazdice else {
        return Ok((v as u64).div_ceil(h.kompresia.riadkov() as u64));
    };
    if t.sx == 0 || t.sy == 0 || t.sx > 1 << 20 || t.sy > 1 << 20 {
        return Err(chyba("neplatná veľkosť dlaždice"));
    }
    let dl = |a: i64, s: u32| (a as u64).div_ceil(s as u64);
    Ok(match t.uroven {
        0 => dl(w, t.sx) * dl(v, t.sy),
        1 => {
            let n = urovne(w.max(v), t.nahor);
            (0..n)
                .map(|l| dl(velkost_urovne(w, l, t.nahor), t.sx) * dl(velkost_urovne(v, l, t.nahor), t.sy))
                .sum()
        }
        2 => {
            let (nx, ny) = (urovne(w, t.nahor), urovne(v, t.nahor));
            let mut s = 0;
            for ly in 0..ny {
                for lx in 0..nx {
                    s += dl(velkost_urovne(w, lx, t.nahor), t.sx) * dl(velkost_urovne(v, ly, t.nahor), t.sy);
                }
            }
            s
        }
        _ => return Err(chyba("neznámy režim úrovní dlaždíc")),
    })
}

/// Prečíta prvú časť súboru OpenEXR.
pub fn citaj(data: &[u8], limity: &Limity) -> Vysledok<Obraz> {
    let (h, koniec_hlavicky, viacdielny) = otvor(data)?;
    if h.hlboky {
        return Err(chyba("hlboké (deep) obrazy nie sú podporené"));
    }
    if matches!(h.kompresia, Kompresia::Dwaa | Kompresia::Dwab) {
        return Err(chyba("kompresia DWAA/DWAB zatiaľ nie je podporená"));
    }
    let (w, v) = (h.data.sirka(), h.data.vyska());
    let kanalov = h.kanaly.len() as u64;
    if kanalov == 0 {
        return Err(chyba("žiadne kanály"));
    }
    limity.over(w as u64, v as u64, kanalov * 4)?;
    let (w, v) = (w as usize, v as usize);
    // tabuľka polôh prvej časti (pri viacdielnom súbore sú tabuľky všetkých častí za sebou)
    let blokov = pocet_blokov(&h)?;
    if blokov > data.len() as u64 / 8 {
        return Err(chyba("tabuľka polôh je väčšia ako súbor"));
    }
    let mut c = Citac { d: data, p: koniec_hlavicky };
    let mut polohy = Vec::with_capacity(blokov as usize);
    for _ in 0..blokov {
        polohy.push(c.u64()?);
    }
    // výsledné roviny kanálov v plnom rozlíšení dataWindow
    let mut roviny: Vec<Vec<f32>> = vec![vec![0.0; w * v]; h.kanaly.len()];
    for &p in &polohy {
        if p == 0 || p as usize >= data.len() {
            continue; // chýbajúci blok (neúplný súbor) – ostane nula
        }
        let mut b = Citac { d: data, p: p as usize };
        if viacdielny && b.i32()? != 0 {
            continue; // blok inej časti
        }
        let (okno, dlzka) = if let Some(t) = h.dlazdice {
            let (tx, ty, lx, ly) = (b.i32()?, b.i32()?, b.i32()?, b.i32()?);
            if lx != 0 || ly != 0 {
                continue; // zmenšené úrovne mipmapy sa nečítajú
            }
            let x0 = h.data.x0 as i64 + tx as i64 * t.sx as i64;
            let y0 = h.data.y0 as i64 + ty as i64 * t.sy as i64;
            if tx < 0 || ty < 0 || x0 > h.data.x1 as i64 || y0 > h.data.y1 as i64 {
                return Err(chyba("dlaždica mimo obrazu"));
            }
            let x1 = (x0 + t.sx as i64 - 1).min(h.data.x1 as i64);
            let y1 = (y0 + t.sy as i64 - 1).min(h.data.y1 as i64);
            (Okno { x0: x0 as i32, y0: y0 as i32, x1: x1 as i32, y1: y1 as i32 }, b.i32()?)
        } else {
            let y = b.i32()?;
            if y < h.data.y0 || y > h.data.y1 {
                return Err(chyba("blok mimo obrazu"));
            }
            let y1 = (y as i64 + h.kompresia.riadkov() as i64 - 1).min(h.data.y1 as i64) as i32;
            (Okno { x0: h.data.x0, y0: y, x1: h.data.x1, y1 }, b.i32()?)
        };
        if dlzka < 0 {
            return Err(chyba("záporná dĺžka bloku"));
        }
        let udaje = b.bajty(dlzka as usize)?;
        let surove = rozbal_blok(&h, &okno, udaje)?;
        rozloz_blok(&h, &okno, &surove, &mut roviny, w)?;
    }
    zostav(h, roviny, w, v)
}

/// Bajty nekomprimovaného bloku: riadky, v každom kanály za sebou.
fn velkost_bloku(h: &Hlavicka, o: &Okno) -> usize {
    let mut n = 0;
    for y in o.y0..=o.y1 {
        for k in &h.kanaly {
            if y.rem_euclid(k.ys) == 0 {
                n += pocet_vzoriek(k.xs, o.x0, o.x1) * k.typ.bajtov();
            }
        }
    }
    n
}

fn rozbal_blok(h: &Hlavicka, o: &Okno, udaje: &[u8]) -> Vysledok<Vec<u8>> {
    let velkost = velkost_bloku(h, o);
    if udaje.len() == velkost && h.kompresia != Kompresia::Ziadna {
        // kodér uloží blok bez kompresie, ak by kompresia nepomohla
        return Ok(udaje.to_vec());
    }
    let mut v = match h.kompresia {
        Kompresia::Ziadna => udaje.to_vec(),
        Kompresia::Rle => {
            let mut t = rle_rozbal(udaje, velkost)?;
            prediktor_spat(&mut t);
            preloz_spat(&t)
        }
        Kompresia::Zips | Kompresia::Zip => {
            let mut t = deflate::zlib_rozbal(udaje, velkost)?;
            prediktor_spat(&mut t);
            preloz_spat(&t)
        }
        Kompresia::Piz => piz_rozbal(h, o, udaje, velkost)?,
        Kompresia::Pxr24 => pxr24_rozbal(h, o, udaje, velkost)?,
        Kompresia::B44 | Kompresia::B44a => b44_rozbal(h, o, udaje, velkost)?,
        Kompresia::Dwaa | Kompresia::Dwab => return Err(chyba("DWA nie je podporené")),
    };
    if v.len() < velkost {
        return Err(chyba("blok je kratší, ako má byť"));
    }
    v.truncate(velkost);
    Ok(v)
}

/// Rozloží nekomprimovaný blok do rovín kanálov (podvzorkované kanály sa rozšíria opakovaním).
fn rozloz_blok(h: &Hlavicka, o: &Okno, d: &[u8], roviny: &mut [Vec<f32>], w: usize) -> Vysledok<()> {
    let mut p = 0;
    for y in o.y0..=o.y1 {
        for (ki, k) in h.kanaly.iter().enumerate() {
            if y.rem_euclid(k.ys) != 0 {
                continue;
            }
            let n = pocet_vzoriek(k.xs, o.x0, o.x1);
            let b = k.typ.bajtov();
            let riadok = d.get(p..p + n * b).ok_or_else(|| chyba("krátky blok"))?;
            p += n * b;
            let prvy_x = {
                let a = (o.x0 as i64).div_euclid(k.xs as i64) * k.xs as i64;
                if a < o.x0 as i64 { a + k.xs as i64 } else { a }
            };
            for i in 0..n {
                let hodnota = match k.typ {
                    TypVzorky::Half => f16_na_f32(u16::from_le_bytes([riadok[2 * i], riadok[2 * i + 1]])),
                    TypVzorky::Float => f32::from_le_bytes([
                        riadok[4 * i],
                        riadok[4 * i + 1],
                        riadok[4 * i + 2],
                        riadok[4 * i + 3],
                    ]),
                    TypVzorky::Uint => u32::from_le_bytes([
                        riadok[4 * i],
                        riadok[4 * i + 1],
                        riadok[4 * i + 2],
                        riadok[4 * i + 3],
                    ]) as f32,
                };
                // vzorka pokrýva xs × ys pixelov
                let x = prvy_x + i as i64 * k.xs as i64 - h.data.x0 as i64;
                let yy = y as i64 - h.data.y0 as i64;
                for dy in 0..k.ys as i64 {
                    let ry = yy + dy;
                    if ry < 0 || ry as usize >= roviny[ki].len() / w {
                        break;
                    }
                    for dx in 0..k.xs as i64 {
                        let rx = x + dx;
                        if rx >= 0 && (rx as usize) < w {
                            roviny[ki][ry as usize * w + rx as usize] = hodnota;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn zostav(h: Hlavicka, mut roviny: Vec<Vec<f32>>, w: usize, v: usize) -> Vysledok<Obraz> {
    let mena: Vec<&str> = h.kanaly.iter().map(|k| k.meno.as_str()).collect();
    let najdi = |m: &str| mena.iter().position(|&x| x == m);
    let (poradie, model, alfa): (Vec<usize>, Model, bool) =
        match (najdi("R"), najdi("G"), najdi("B"), najdi("Y")) {
            (Some(r), Some(g), Some(b), _) if mena.len() <= 4 => {
                let a = najdi("A");
                let mut p = vec![r, g, b];
                if let Some(a) = a {
                    p.push(a);
                }
                if p.len() == mena.len() {
                    (p, Model::Rgb, a.is_some())
                } else {
                    ((0..mena.len()).collect(), Model::Iny, false)
                }
            }
            (None, None, None, Some(y)) if mena.len() <= 2 => {
                let a = najdi("A");
                let mut p = vec![y];
                if let Some(a) = a {
                    p.push(a);
                }
                if p.len() == mena.len() {
                    (p, Model::Siva, a.is_some())
                } else {
                    ((0..mena.len()).collect(), Model::Iny, false)
                }
            }
            _ => ((0..mena.len()).collect(), Model::Iny, false),
        };
    let k = poradie.len();
    let mut vz = vec![0f32; w * v * k];
    for (c, &i) in poradie.iter().enumerate() {
        let r = std::mem::take(&mut roviny[i]);
        for (p, x) in r.into_iter().enumerate() {
            vz[p * k + c] = x;
        }
    }
    let mut o = Obraz::novy(w, v, model, alfa, Vzorky::F32(vz))?;
    o.mena_kanalov = poradie.iter().map(|&i| h.kanaly[i].meno.clone()).collect();
    o.bitov = if h.kanaly.iter().all(|k| k.typ == TypVzorky::Half) { 16 } else { 32 };
    o.farba.linearne = true;
    o.farba.chromaticity = h.chromaticity;
    o.metadata.texty = h.texty;
    Ok(o)
}

// ---------------------------------------------------------------- spoločná príprava RLE a ZIP

/// Späť z rozdielov: t[i] = t[i−1] + d[i] − 128.
fn prediktor_spat(t: &mut [u8]) {
    for i in 1..t.len() {
        t[i] = t[i - 1].wrapping_add(t[i]).wrapping_sub(128);
    }
}

fn prediktor_tam(t: &mut [u8]) {
    for i in (1..t.len()).rev() {
        t[i] = t[i].wrapping_sub(t[i - 1]).wrapping_add(128);
    }
}

/// Bajty boli rozdelené na párne (prvá polovica) a nepárne (druhá polovica) – spojenie späť.
fn preloz_spat(t: &[u8]) -> Vec<u8> {
    let polovica = t.len().div_ceil(2);
    let mut v = Vec::with_capacity(t.len());
    for i in 0..polovica {
        v.push(t[i]);
        if polovica + i < t.len() {
            v.push(t[polovica + i]);
        }
    }
    v
}

fn preloz_tam(t: &[u8]) -> Vec<u8> {
    let mut v: Vec<u8> = t.iter().step_by(2).copied().collect();
    v.extend(t.iter().skip(1).step_by(2));
    v
}

/// RLE v OpenEXR: záporný počet −n = n bajtov doslova, inak n + 1 opakovaní nasledujúceho bajtu.
fn rle_rozbal(d: &[u8], najviac: usize) -> Vysledok<Vec<u8>> {
    let mut v = Vec::with_capacity(najviac);
    let mut i = 0;
    while i < d.len() {
        let n = d[i] as i8;
        i += 1;
        if n < 0 {
            let k = (-(n as i32)) as usize;
            v.extend_from_slice(d.get(i..i + k).ok_or_else(|| chyba("RLE za koncom"))?);
            i += k;
        } else {
            let b = *d.get(i).ok_or_else(|| chyba("RLE za koncom"))?;
            i += 1;
            v.extend(std::iter::repeat_n(b, n as usize + 1));
        }
        if v.len() > najviac {
            return Err(chyba("RLE dáva viac údajov, ako má blok"));
        }
    }
    Ok(v)
}

fn rle_zbal(d: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(d.len() + d.len() / 64 + 2);
    let mut i = 0;
    while i < d.len() {
        let mut beh = 1;
        while i + beh < d.len() && d[i + beh] == d[i] && beh < 128 {
            beh += 1;
        }
        if beh >= 3 {
            v.push((beh - 1) as u8);
            v.push(d[i]);
            i += beh;
        } else {
            // doslova až po ďalší beh aspoň troch rovnakých
            let zac = i;
            while i < d.len() && i - zac < 127 {
                if i + 2 < d.len() && d[i] == d[i + 1] && d[i] == d[i + 2] {
                    break;
                }
                i += 1;
            }
            v.push((-((i - zac) as i32)) as u8);
            v.extend_from_slice(&d[zac..i]);
        }
    }
    v
}

// ---------------------------------------------------------------- PIZ

const BITMAPA: usize = 8192;

/// Kanál v dočasnej vyrovnávacej pamäti PIZ/B44 (roviny kanálov za sebou, 16-bitové jednotky).
struct KanalBloku {
    zaciatok: usize,
    nx: usize,
    ny: usize,
    ys: i32,
    velkost: usize,
}

fn kanaly_bloku(h: &Hlavicka, o: &Okno) -> (Vec<KanalBloku>, usize) {
    let mut v = Vec::new();
    let mut p = 0;
    for k in &h.kanaly {
        let nx = pocet_vzoriek(k.xs, o.x0, o.x1);
        let ny = pocet_vzoriek(k.ys, o.y0, o.y1);
        let velkost = k.typ.bajtov() / 2;
        v.push(KanalBloku { zaciatok: p, nx, ny, ys: k.ys, velkost });
        p += nx * ny * velkost;
    }
    (v, p)
}

/// Zloží roviny 16-bitových jednotiek do riadkov bloku (bajty little-endian).
fn roviny_do_riadkov(kanaly: &[KanalBloku], tmp: &[u16], o: &Okno, velkost: usize) -> Vec<u8> {
    let mut von = Vec::with_capacity(velkost);
    let mut konce: Vec<usize> = kanaly.iter().map(|k| k.zaciatok).collect();
    for y in o.y0..=o.y1 {
        for (i, k) in kanaly.iter().enumerate() {
            if y.rem_euclid(k.ys) != 0 {
                continue;
            }
            let n = k.nx * k.velkost;
            for &x in &tmp[konce[i]..konce[i] + n] {
                von.extend_from_slice(&x.to_le_bytes());
            }
            konce[i] += n;
        }
    }
    von
}

fn piz_rozbal(h: &Hlavicka, o: &Okno, d: &[u8], velkost: usize) -> Vysledok<Vec<u8>> {
    let (kanaly, jednotiek) = kanaly_bloku(h, o);
    let mut c = Citac { d, p: 0 };
    let b = c.bajty(4)?;
    let (min, max) = (u16::from_le_bytes([b[0], b[1]]) as usize, u16::from_le_bytes([b[2], b[3]]) as usize);
    if max >= BITMAPA {
        return Err(chyba("PIZ: bitová mapa mimo rozsahu"));
    }
    let mut bitmapa = vec![0u8; BITMAPA];
    if min <= max {
        bitmapa[min..=max].copy_from_slice(c.bajty(max - min + 1)?);
    }
    // tabuľka hodnôt: index → skutočná 16-bitová hodnota (nula je vždy)
    let mut lut = vec![0u16; 1 << 16];
    let mut k = 0usize;
    for i in 0..1usize << 16 {
        if i == 0 || bitmapa[i >> 3] & (1 << (i & 7)) != 0 {
            lut[k] = i as u16;
            k += 1;
        }
    }
    let max_hodnota = (k - 1) as u32;
    let dlzka = c.i32()?;
    if dlzka < 0 {
        return Err(chyba("PIZ: záporná dĺžka"));
    }
    let komp = c.bajty(dlzka as usize)?;
    let mut tmp = vec![0u16; jednotiek];
    huffman_piz::rozbal(komp, &mut tmp)?;
    for kb in &kanaly {
        for j in 0..kb.velkost {
            vlnka_spat(&mut tmp[kb.zaciatok..], j, kb.nx, kb.velkost, kb.ny, kb.nx * kb.velkost, max_hodnota);
        }
    }
    for x in tmp.iter_mut() {
        *x = lut[*x as usize];
    }
    Ok(roviny_do_riadkov(&kanaly, &tmp, o, velkost))
}

/// Krok spätnej vlnkovej transformácie pre hodnoty do 14 bitov (so znamienkom).
#[inline]
fn wdec14(l: u16, h: u16) -> (u16, u16) {
    let (ls, hs) = (l as i16 as i32, h as i16 as i32);
    let ai = ls + (hs & 1) + (hs >> 1);
    ((ai as i16) as u16, ((ai - hs) as i16) as u16)
}

/// Krok spätnej vlnkovej transformácie pre plných 16 bitov (modulárne).
#[inline]
fn wdec16(l: u16, h: u16) -> (u16, u16) {
    let (m, d) = (l as i32, h as i32);
    let bb = (m - (d >> 1)) & 0xFFFF;
    let aa = (d + bb - 0x8000) & 0xFFFF;
    (aa as u16, bb as u16)
}

/// Spätná dvojrozmerná vlnková transformácia PIZ na rovine s krokom `ox` (stĺpec) a `oy` (riadok).
fn vlnka_spat(t: &mut [u16], zac: usize, nx: usize, ox: usize, ny: usize, oy: usize, max: u32) {
    let w14 = max < (1 << 14);
    let dec = |a: u16, b: u16| if w14 { wdec14(a, b) } else { wdec16(a, b) };
    let n = nx.min(ny);
    let mut p = 1usize;
    while p <= n {
        p <<= 1;
    }
    p >>= 1;
    let mut p2 = p;
    p >>= 1;
    while p >= 1 {
        let (oy1, oy2, ox1, ox2) = (oy * p, oy * p2, ox * p, ox * p2);
        let mut py = zac;
        let ey = zac + oy * (ny - p2);
        while py <= ey {
            let mut px = py;
            let ex = py + ox * (nx - p2);
            while px <= ex {
                let (p01, p10) = (px + ox1, px + oy1);
                let p11 = p10 + ox1;
                let (i00, i10) = dec(t[px], t[p10]);
                let (i01, i11) = dec(t[p01], t[p11]);
                let (a, b) = dec(i00, i01);
                t[px] = a;
                t[p01] = b;
                let (a, b) = dec(i10, i11);
                t[p10] = a;
                t[p11] = b;
                px += ox2;
            }
            if nx & p != 0 {
                let p10 = px + oy1;
                let (i00, x) = dec(t[px], t[p10]);
                t[p10] = x;
                t[px] = i00;
            }
            py += oy2;
        }
        if ny & p != 0 {
            let mut px = py;
            let ex = py + ox * (nx - p2);
            while px <= ex {
                let p01 = px + ox1;
                let (i00, x) = dec(t[px], t[p01]);
                t[p01] = x;
                t[px] = i00;
                px += ox2;
            }
        }
        p2 = p;
        p >>= 1;
    }
}

/// Huffmanovo kódovanie PIZ (kanonické kódy od najdlhších, beh opakovaní ako zvláštny znak).
mod huffman_piz {
    use super::chyba;
    use orchidea_jadro::Vysledok;

    const VELKOST: usize = (1 << 16) + 1;
    const DEC_BITY: u32 = 14;
    const KRATKY_BEH: u64 = 59;
    const DLHY_BEH: u64 = 63;
    const NAJKRATSI_DLHY: u64 = 2 + DLHY_BEH - KRATKY_BEH;

    struct Bity<'a> {
        d: &'a [u8],
        p: usize,
        c: u64,
        lc: u32,
    }

    impl Bity<'_> {
        fn bajt(&mut self) -> Vysledok<()> {
            let b = *self.d.get(self.p).ok_or_else(|| chyba("PIZ: Huffman za koncom"))?;
            self.p += 1;
            self.c = (self.c << 8) | b as u64;
            self.lc += 8;
            Ok(())
        }
        fn bity(&mut self, n: u32) -> Vysledok<u64> {
            while self.lc < n {
                self.bajt()?;
            }
            self.lc -= n;
            Ok((self.c >> self.lc) & ((1 << n) - 1))
        }
    }

    /// Kanonické kódy: dĺžka v dolných 6 bitoch, kód nad nimi (najdlhšie kódy majú najmenšie hodnoty).
    fn kanonicke(h: &mut [u64]) {
        let mut n = [0u64; 59];
        for &x in h.iter() {
            n[x as usize] += 1;
        }
        let mut c = 0u64;
        for i in (1..59).rev() {
            let nc = (c + n[i]) >> 1;
            n[i] = c;
            c = nc;
        }
        for x in h.iter_mut() {
            let l = *x;
            if l > 0 {
                *x = l | (n[l as usize] << 6);
                n[l as usize] += 1;
            }
        }
    }

    /// Rozbalí Huffmanove údaje PIZ do `von` (presne toľko 16-bitových hodnôt).
    pub fn rozbal(d: &[u8], von: &mut [u16]) -> Vysledok<()> {
        if d.is_empty() {
            return if von.is_empty() { Ok(()) } else { Err(chyba("PIZ: chýbajú údaje")) };
        }
        if d.len() < 20 {
            return Err(chyba("PIZ: krátka hlavička Huffmana"));
        }
        let u = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
        let (im, i_m, n_bitov) = (u(0), u(4), u(12));
        if im >= VELKOST || i_m >= VELKOST || im > i_m {
            return Err(chyba("PIZ: neplatný rozsah kódov"));
        }
        if n_bitov.div_ceil(8) > d.len() - 20 {
            return Err(chyba("PIZ: počet bitov presahuje údaje"));
        }
        // tabuľka dĺžok
        let mut kody = vec![0u64; VELKOST];
        let mut b = Bity { d: &d[20..], p: 0, c: 0, lc: 0 };
        let mut i = im;
        while i <= i_m {
            let l = b.bity(6)?;
            if l == DLHY_BEH {
                let beh = (b.bity(8)? + NAJKRATSI_DLHY) as usize;
                if i + beh > i_m + 1 {
                    return Err(chyba("PIZ: beh núl za koncom tabuľky"));
                }
                i += beh;
                continue;
            } else if l >= KRATKY_BEH {
                let beh = (l - KRATKY_BEH + 2) as usize;
                if i + beh > i_m + 1 {
                    return Err(chyba("PIZ: beh núl za koncom tabuľky"));
                }
                i += beh;
                continue;
            }
            kody[i] = l;
            i += 1;
        }
        kanonicke(&mut kody);
        let tabulka_bajtov = b.p;
        // dekódovacia tabuľka: krátke kódy priamo, dlhé zoznamom kandidátov
        #[derive(Clone)]
        enum Polozka {
            Prazdna,
            Kratky(u32, u32),
            Dlhe(Vec<u32>),
        }
        let mut dec = vec![Polozka::Prazdna; 1 << DEC_BITY];
        for (znak, &k) in kody.iter().enumerate().take(i_m + 1).skip(im) {
            let (c, l) = (k >> 6, (k & 63) as u32);
            if l == 0 {
                continue;
            }
            if c >> l != 0 {
                return Err(chyba("PIZ: neplatný kód"));
            }
            if l > DEC_BITY {
                let idx = (c >> (l - DEC_BITY)) as usize;
                match &mut dec[idx] {
                    Polozka::Prazdna => dec[idx] = Polozka::Dlhe(vec![znak as u32]),
                    Polozka::Dlhe(v) => v.push(znak as u32),
                    Polozka::Kratky(..) => return Err(chyba("PIZ: konflikt kódov")),
                }
            } else {
                let zac = (c << (DEC_BITY - l)) as usize;
                for e in &mut dec[zac..zac + (1 << (DEC_BITY - l))] {
                    if !matches!(e, Polozka::Prazdna) {
                        return Err(chyba("PIZ: konflikt kódov"));
                    }
                    *e = Polozka::Kratky(l, znak as u32);
                }
            }
        }
        // dekódovanie
        let beh_znak = i_m as u32;
        let udaje = &d[20 + tabulka_bajtov..];
        let koniec = n_bitov.div_ceil(8).min(udaje.len());
        let mut s = Bity { d: &udaje[..koniec], p: 0, c: 0, lc: 0 };
        let mut o = 0usize;
        let vloz = |znak: u32, s: &mut Bity, o: &mut usize, von: &mut [u16]| -> Vysledok<()> {
            if znak == beh_znak {
                if s.lc < 8 {
                    s.bajt()?;
                }
                s.lc -= 8;
                let n = ((s.c >> s.lc) & 0xFF) as usize;
                if *o == 0 || *o + n > von.len() {
                    return Err(chyba("PIZ: beh mimo rozsahu"));
                }
                let x = von[*o - 1];
                von[*o..*o + n].fill(x);
                *o += n;
            } else {
                if *o >= von.len() {
                    return Err(chyba("PIZ: viac hodnôt, ako má blok"));
                }
                von[*o] = znak as u16;
                *o += 1;
            }
            Ok(())
        };
        while s.p < s.d.len() {
            s.bajt()?;
            while s.lc >= DEC_BITY {
                let idx = ((s.c >> (s.lc - DEC_BITY)) & ((1 << DEC_BITY) - 1)) as usize;
                match &dec[idx] {
                    Polozka::Kratky(l, z) => {
                        s.lc -= l;
                        vloz(*z, &mut s, &mut o, von)?;
                    }
                    Polozka::Dlhe(kandidati) => {
                        let mut najdeny = false;
                        for &z in kandidati {
                            let l = (kody[z as usize] & 63) as u32;
                            while s.lc < l && s.p < s.d.len() {
                                s.bajt()?;
                            }
                            if s.lc >= l && (kody[z as usize] >> 6) == ((s.c >> (s.lc - l)) & ((1 << l) - 1))
                            {
                                s.lc -= l;
                                vloz(z, &mut s, &mut o, von)?;
                                najdeny = true;
                                break;
                            }
                        }
                        if !najdeny {
                            return Err(chyba("PIZ: neplatný dlhý kód"));
                        }
                    }
                    Polozka::Prazdna => return Err(chyba("PIZ: neplatný kód")),
                }
            }
        }
        let i = ((8 - n_bitov as i64) & 7) as u32;
        s.c >>= i;
        s.lc = s.lc.saturating_sub(i);
        while s.lc > 0 {
            let idx = ((s.c << (DEC_BITY - s.lc.min(DEC_BITY))) & ((1 << DEC_BITY) - 1)) as usize;
            match &dec[idx] {
                Polozka::Kratky(l, z) if *l <= s.lc => {
                    s.lc -= l;
                    vloz(*z, &mut s, &mut o, von)?;
                }
                _ => return Err(chyba("PIZ: neplatný kód na konci")),
            }
        }
        if o != von.len() {
            return Err(chyba("PIZ: málo hodnôt"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- PXR24

fn pxr24_rozbal(h: &Hlavicka, o: &Okno, d: &[u8], velkost: usize) -> Vysledok<Vec<u8>> {
    // veľkosť rozbalených údajov: UINT 4, HALF 2, FLOAT 3 bajty na vzorku
    let mut ocakavane = 0;
    for y in o.y0..=o.y1 {
        for k in &h.kanaly {
            if y.rem_euclid(k.ys) == 0 {
                let n = pocet_vzoriek(k.xs, o.x0, o.x1);
                ocakavane += n * match k.typ {
                    TypVzorky::Uint => 4,
                    TypVzorky::Half => 2,
                    TypVzorky::Float => 3,
                };
            }
        }
    }
    let t = deflate::zlib_rozbal(d, ocakavane)?;
    if t.len() < ocakavane {
        return Err(chyba("PXR24: krátky blok"));
    }
    let mut von = Vec::with_capacity(velkost);
    let mut p = 0;
    for y in o.y0..=o.y1 {
        for k in &h.kanaly {
            if y.rem_euclid(k.ys) != 0 {
                continue;
            }
            let n = pocet_vzoriek(k.xs, o.x0, o.x1);
            match k.typ {
                TypVzorky::Uint => {
                    let mut px = 0u32;
                    for j in 0..n {
                        let r = (t[p + j] as u32) << 24
                            | (t[p + n + j] as u32) << 16
                            | (t[p + 2 * n + j] as u32) << 8
                            | t[p + 3 * n + j] as u32;
                        px = px.wrapping_add(r);
                        von.extend_from_slice(&px.to_le_bytes());
                    }
                    p += 4 * n;
                }
                TypVzorky::Half => {
                    let mut px = 0u16;
                    for j in 0..n {
                        px = px.wrapping_add((t[p + j] as u16) << 8 | t[p + n + j] as u16);
                        von.extend_from_slice(&px.to_le_bytes());
                    }
                    p += 2 * n;
                }
                TypVzorky::Float => {
                    let mut px = 0u32;
                    for j in 0..n {
                        let r = (t[p + j] as u32) << 24
                            | (t[p + n + j] as u32) << 16
                            | (t[p + 2 * n + j] as u32) << 8;
                        px = px.wrapping_add(r);
                        von.extend_from_slice(&px.to_le_bytes());
                    }
                    p += 3 * n;
                }
            }
        }
    }
    Ok(von)
}

// ---------------------------------------------------------------- B44

fn b44_rozbal(h: &Hlavicka, o: &Okno, d: &[u8], velkost: usize) -> Vysledok<Vec<u8>> {
    let (kanaly, jednotiek) = kanaly_bloku(h, o);
    let mut tmp = vec![0u16; jednotiek];
    let mut p = 0usize;
    for (kb, k) in kanaly.iter().zip(&h.kanaly) {
        if k.typ != TypVzorky::Half {
            // iné typy sú uložené bez kompresie (little-endian)
            let n = kb.nx * kb.ny * kb.velkost;
            let b = d.get(p..p + 2 * n).ok_or_else(|| chyba("B44: krátky blok"))?;
            for i in 0..n {
                tmp[kb.zaciatok + i] = u16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
            }
            p += 2 * n;
            continue;
        }
        if k.linearny {
            return Err(chyba("B44 s logaritmickým kanálom (pLinear) nie je podporené"));
        }
        for by in (0..kb.ny).step_by(4) {
            for bx in (0..kb.nx).step_by(4) {
                let mut s = [0u16; 16];
                let b = d.get(p..p + 3).ok_or_else(|| chyba("B44: krátky blok"))?;
                if b[2] >= 13 << 2 {
                    s.fill(rozbal_znamienko((b[0] as u16) << 8 | b[1] as u16));
                    p += 3;
                } else {
                    let b = d.get(p..p + 14).ok_or_else(|| chyba("B44: krátky blok"))?;
                    rozbal14(b, &mut s);
                    p += 14;
                }
                for y in 0..4.min(kb.ny - by) {
                    for x in 0..4.min(kb.nx - bx) {
                        tmp[kb.zaciatok + (by + y) * kb.nx + bx + x] = s[y * 4 + x];
                    }
                }
            }
        }
    }
    Ok(roviny_do_riadkov(&kanaly, &tmp, o, velkost))
}

/// Hodnoty B44 sú uložené tak, aby sa dali porovnávať ako čísla bez znamienka – návrat k bitom half.
#[inline]
fn rozbal_znamienko(x: u16) -> u16 {
    if x & 0x8000 != 0 { x & 0x7FFF } else { !x }
}

/// Blok 4×4 zo 14 bajtov: prvá hodnota celá, ďalšie ako 6-bitové rozdiely posunuté o `shift`.
fn rozbal14(b: &[u8], s: &mut [u16; 16]) {
    let b: Vec<u16> = b.iter().map(|&x| x as u16).collect();
    s[0] = (b[0] << 8) | b[1];
    let shift = b[2] >> 2;
    let bias = 0x20u16.wrapping_shl(shift as u32);
    let r = |x: u16| (x & 0x3F).wrapping_shl(shift as u32);
    s[4] = s[0].wrapping_add(r((b[2] << 4) | (b[3] >> 4))).wrapping_sub(bias);
    s[8] = s[4].wrapping_add(r((b[3] << 2) | (b[4] >> 6))).wrapping_sub(bias);
    s[12] = s[8].wrapping_add(r(b[4])).wrapping_sub(bias);
    s[1] = s[0].wrapping_add(r(b[5] >> 2)).wrapping_sub(bias);
    s[5] = s[4].wrapping_add(r((b[5] << 4) | (b[6] >> 4))).wrapping_sub(bias);
    s[9] = s[8].wrapping_add(r((b[6] << 2) | (b[7] >> 6))).wrapping_sub(bias);
    s[13] = s[12].wrapping_add(r(b[7])).wrapping_sub(bias);
    s[2] = s[1].wrapping_add(r(b[8] >> 2)).wrapping_sub(bias);
    s[6] = s[5].wrapping_add(r((b[8] << 4) | (b[9] >> 4))).wrapping_sub(bias);
    s[10] = s[9].wrapping_add(r((b[9] << 2) | (b[10] >> 6))).wrapping_sub(bias);
    s[14] = s[13].wrapping_add(r(b[10])).wrapping_sub(bias);
    s[3] = s[2].wrapping_add(r(b[11] >> 2)).wrapping_sub(bias);
    s[7] = s[6].wrapping_add(r((b[11] << 4) | (b[12] >> 4))).wrapping_sub(bias);
    s[11] = s[10].wrapping_add(r((b[12] << 2) | (b[13] >> 6))).wrapping_sub(bias);
    s[15] = s[14].wrapping_add(r(b[13])).wrapping_sub(bias);
    for x in s.iter_mut() {
        *x = rozbal_znamienko(*x);
    }
}

// ---------------------------------------------------------------- zápis

/// Nastavenie zápisu OpenEXR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NastavenieExr {
    /// Kanály s polovičnou presnosťou (HALF) – bežné pre farbu; inak FLOAT.
    pub polovicna: bool,
    /// Kompresia: ZIP (predvolená), ZIPS, RLE alebo žiadna.
    pub kompresia: Kompresia,
}

impl Default for NastavenieExr {
    fn default() -> Self {
        NastavenieExr { polovicna: true, kompresia: Kompresia::Zip }
    }
}

fn atribut(von: &mut Vec<u8>, meno: &str, typ: &str, hodnota: &[u8]) {
    von.extend_from_slice(meno.as_bytes());
    von.push(0);
    von.extend_from_slice(typ.as_bytes());
    von.push(0);
    von.extend_from_slice(&(hodnota.len() as i32).to_le_bytes());
    von.extend_from_slice(hodnota);
}

/// Zapíše obrázok do OpenEXR (riadky). Celočíselné vzorky sa prevedú do 0–1 (bez zmeny prenosovej funkcie –
/// prevod do lineárneho svetla robí balík `farba`).
pub fn zapis(o: &Obraz, n: &NastavenieExr) -> Vysledok<Vec<u8>> {
    o.over()?;
    if !matches!(n.kompresia, Kompresia::Ziadna | Kompresia::Rle | Kompresia::Zips | Kompresia::Zip) {
        return Err(Chyba::MimoRozsahu("OpenEXR: zápis vie kompresie NONE, RLE, ZIPS a ZIP".into()));
    }
    if o.sirka > i32::MAX as usize || o.vyska > i32::MAX as usize {
        return Err(Chyba::Limit("OpenEXR: rozmer nad 2^31 − 1".into()));
    }
    let k = o.kanaly();
    let mut mena: Vec<(String, usize)> = o.mena_kanalov.iter().cloned().zip(0..k).collect();
    mena.sort();
    for w in mena.windows(2) {
        if w[0].0 == w[1].0 {
            return Err(Chyba::NezhodaTvaru(format!("OpenEXR: kanál {} je dvakrát", w[0].0)));
        }
    }
    let typ: i32 = if n.polovicna { 1 } else { 2 };
    let bajtov = if n.polovicna { 2 } else { 4 };
    let mut von = PODPIS.to_vec();
    von.extend_from_slice(&2u32.to_le_bytes());
    let mut ch = Vec::new();
    for (m, _) in &mena {
        ch.extend_from_slice(m.as_bytes());
        ch.push(0);
        ch.extend_from_slice(&typ.to_le_bytes());
        ch.extend_from_slice(&[0, 0, 0, 0]);
        ch.extend_from_slice(&1i32.to_le_bytes());
        ch.extend_from_slice(&1i32.to_le_bytes());
    }
    ch.push(0);
    atribut(&mut von, "channels", "chlist", &ch);
    atribut(&mut von, "compression", "compression", &[n.kompresia.cislo()]);
    let mut okno = Vec::new();
    for v in [0i32, 0, o.sirka as i32 - 1, o.vyska as i32 - 1] {
        okno.extend_from_slice(&v.to_le_bytes());
    }
    atribut(&mut von, "dataWindow", "box2i", &okno);
    atribut(&mut von, "displayWindow", "box2i", &okno);
    atribut(&mut von, "lineOrder", "lineOrder", &[0]);
    atribut(&mut von, "pixelAspectRatio", "float", &1.0f32.to_le_bytes());
    atribut(&mut von, "screenWindowCenter", "v2f", &[0; 8]);
    atribut(&mut von, "screenWindowWidth", "float", &1.0f32.to_le_bytes());
    if let Some(c) = o.farba.chromaticity {
        let poradie = [2, 3, 4, 5, 6, 7, 0, 1];
        let d: Vec<u8> = poradie.iter().flat_map(|&i| (c[i] as f32).to_le_bytes()).collect();
        atribut(&mut von, "chromaticities", "chromaticities", &d);
    }
    atribut(&mut von, "software", "string", b"ORCHIDEA");
    von.push(0);
    let riadkov = n.kompresia.riadkov() as usize;
    let blokov = o.vyska.div_ceil(riadkov);
    let tabulka = von.len();
    von.resize(tabulka + 8 * blokov, 0);
    let vzorka = |i: usize| -> f32 { o.vzorky.normovana(i) };
    for b in 0..blokov {
        let (y0, y1) = (b * riadkov, ((b + 1) * riadkov).min(o.vyska));
        let mut surove = Vec::with_capacity((y1 - y0) * o.sirka * k * bajtov);
        for y in y0..y1 {
            for (_, c) in &mena {
                for x in 0..o.sirka {
                    let v = vzorka((y * o.sirka + x) * k + c);
                    if n.polovicna {
                        surove.extend_from_slice(&f32_na_f16(v).to_le_bytes());
                    } else {
                        surove.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        let komp = match n.kompresia {
            Kompresia::Ziadna => surove.clone(),
            Kompresia::Rle => {
                let mut t = preloz_tam(&surove);
                prediktor_tam(&mut t);
                rle_zbal(&t)
            }
            _ => {
                let mut t = preloz_tam(&surove);
                prediktor_tam(&mut t);
                deflate::zlib_zbal(&t, 6)
            }
        };
        let udaje = if komp.len() < surove.len() { komp } else { surove };
        let poloha = von.len() as u64;
        von[tabulka + 8 * b..tabulka + 8 * b + 8].copy_from_slice(&poloha.to_le_bytes());
        von.extend_from_slice(&(y0 as i32).to_le_bytes());
        von.extend_from_slice(&(udaje.len() as i32).to_le_bytes());
        von.extend_from_slice(&udaje);
    }
    Ok(von)
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    #[test]
    fn tam_a_spat() {
        let mut n = Nahoda::zo_semienka(9);
        for (model, alfa) in [(Model::Rgb, true), (Model::Rgb, false), (Model::Siva, false)] {
            for kompresia in [Kompresia::Ziadna, Kompresia::Rle, Kompresia::Zips, Kompresia::Zip] {
                for polovicna in [true, false] {
                    let (w, h) = (1 + n.mensie_ako(50) as usize, 1 + n.mensie_ako(50) as usize);
                    let k = if model == Model::Siva { 1 } else { 3 } + alfa as usize;
                    let v: Vec<f32> = (0..w * h * k)
                        .map(|i| {
                            let x = (i as f32 * 0.37).sin() * 8.0;
                            if polovicna { f16_na_f32(f32_na_f16(x)) } else { x }
                        })
                        .collect();
                    let mut o = Obraz::novy(w, h, model, alfa, Vzorky::F32(v)).unwrap();
                    o.farba.linearne = true;
                    let e = zapis(&o, &NastavenieExr { polovicna, kompresia }).unwrap();
                    let s = citaj(&e, &Limity::default()).unwrap();
                    assert_eq!(s.vzorky, o.vzorky, "{model:?} {kompresia:?} half {polovicna}");
                    assert_eq!(
                        (s.model, s.alfa, s.mena_kanalov.clone()),
                        (model, alfa, o.mena_kanalov.clone())
                    );
                }
            }
        }
    }

    #[test]
    fn vlnka_a_b44_okrajove() {
        // spätné kroky vlnky obrátia dopredné (priemer a rozdiel; 16-bitová verzia modulárne)
        let wenc14 = |a: u16, b: u16| {
            let (a, b) = (a as i16 as i32, b as i16 as i32);
            ((((a + b) >> 1) as i16) as u16, ((a - b) as i16) as u16)
        };
        let wenc16 = |a: u16, b: u16| {
            let ao = (a as i32 + 0x8000) & 0xFFFF;
            let mut m = (ao + b as i32) >> 1;
            let d = ao - b as i32;
            if d < 0 {
                m = (m + 0x8000) & 0xFFFF;
            }
            (m as u16, (d & 0xFFFF) as u16)
        };
        let mut n = Nahoda::zo_semienka(14);
        for _ in 0..100_000 {
            let (a, b) = (n.dalsie() as u16, n.dalsie() as u16);
            let (l, h) = wenc16(a, b);
            assert_eq!(wdec16(l, h), (a, b));
            let (a, b) =
                ((n.mensie_ako(1 << 14) as i32 - 8192) as u16, (n.mensie_ako(1 << 14) as i32 - 8192) as u16);
            let (l, h) = wenc14(a, b);
            assert_eq!(wdec14(l, h), (a, b));
        }
        assert_eq!(rozbal_znamienko(0x8000 | 0x3C00), 0x3C00);
        assert_eq!(pocet_vzoriek(2, -3, 4), 4);
        assert_eq!(pocet_vzoriek(1, 0, 9), 10);
        assert_eq!(pocet_vzoriek(3, 1, 2), 0);
    }
}
