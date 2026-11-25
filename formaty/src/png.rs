//! PNG (ISO/IEC 15948, W3C PNG tretie vydanie 2025) – čítanie aj zápis vlastným kódom.
//!
//! **Čítanie:** všetky typy farieb (sivá, RGB, paleta, sivá s alfou, RGBA) a hĺbky 1–16 bitov, prekladanie
//! Adam7, priehľadnosť tRNS, farba (iCCP, sRGB, gAMA, cHRM, cICP), Exif (eXIf), texty (tEXt, zTXt, iTXt)
//! a fyzické rozlíšenie (pHYs). Animované PNG (APNG) – zatiaľ sa číta predvolený obrázok. Kontrolné súčty
//! CRC sa overujú; poškodený kritický kúsok je chyba, poškodený doplnkový sa preskočí.
//!
//! **Zápis:** 8 a 16 bitov, 1–4 kanály, voliteľne paleta (ak má obrázok najviac 256 farieb – bez straty).
//! Každý riadok dostane filter s najmenším súčtom absolútnych hodnôt (bežný odhad normy), dáta stlačí
//! vlastný DEFLATE.

use crate::deflate;
use crate::kontrola::{Crc32, crc32};
use crate::obraz::{Cicp, Limity, Metadata, Model, Obraz, Vzorky};
use orchidea_jadro::{Chyba, Vysledok};
use std::collections::HashMap;

pub const PODPIS: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Hlavička IHDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hlavicka {
    pub sirka: u32,
    pub vyska: u32,
    pub hlbka: u8,
    pub typ_farby: u8,
    pub prekladane: bool,
}

impl Hlavicka {
    fn vzoriek_na_pixel(&self) -> usize {
        match self.typ_farby {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        }
    }

    /// Bajtov na jeden riadok šírky `w` (bez bajtu filtra).
    fn bajtov_riadku(&self, w: usize) -> usize {
        (w * self.vzoriek_na_pixel() * self.hlbka as usize).div_ceil(8)
    }
}

fn chyba(text: &str) -> Chyba {
    Chyba::PoskodeneData(format!("PNG: {text}"))
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Prečíta len hlavičku (rozmery, typ) – rýchle prehliadanie priečinka bez rozbaľovania.
pub fn citaj_hlavicku(data: &[u8]) -> Vysledok<Hlavicka> {
    if data.len() < 33 || data[..8] != PODPIS || &data[12..16] != b"IHDR" {
        return Err(chyba("chýba podpis alebo hlavička IHDR"));
    }
    over_hlavicku(&data[16..29])
}

fn over_hlavicku(d: &[u8]) -> Vysledok<Hlavicka> {
    if d.len() != 13 {
        return Err(chyba("hlavička IHDR má zlú dĺžku"));
    }
    let h = Hlavicka {
        sirka: be32(d),
        vyska: be32(&d[4..]),
        hlbka: d[8],
        typ_farby: d[9],
        prekladane: d[12] == 1,
    };
    let dovolene: &[u8] = match h.typ_farby {
        0 => &[1, 2, 4, 8, 16],
        3 => &[1, 2, 4, 8],
        2 | 4 | 6 => &[8, 16],
        _ => return Err(chyba(&format!("neznámy typ farby {}", h.typ_farby))),
    };
    if !dovolene.contains(&h.hlbka) {
        return Err(chyba(&format!("hĺbka {} pri type farby {}", h.hlbka, h.typ_farby)));
    }
    if d[10] != 0 || d[11] != 0 || d[12] > 1 {
        return Err(chyba("neznáma kompresia, filter alebo prekladanie"));
    }
    if h.sirka == 0 || h.vyska == 0 || h.sirka > i32::MAX as u32 || h.vyska > i32::MAX as u32 {
        return Err(chyba("neplatné rozmery"));
    }
    Ok(h)
}

/// Text z bajtov Latin-1 (tEXt, zTXt).
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Prečíta obrázok PNG.
pub fn citaj(data: &[u8], limity: &Limity) -> Vysledok<Obraz> {
    if data.len() < 8 || data[..8] != PODPIS {
        return Err(chyba("chýba podpis"));
    }
    let mut poz = 8;
    let mut hlavicka: Option<Hlavicka> = None;
    let mut paleta: Vec<[u8; 3]> = Vec::new();
    let mut trns: Option<Vec<u8>> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut idat_skoncil = false;
    let mut meta = Metadata { natocenie: 1, ..Default::default() };
    let mut farba = crate::obraz::Farba::default();
    let mut metadat = 0u64;
    let mut koniec = false;
    while !koniec {
        if poz + 12 > data.len() {
            return Err(chyba("neúplný súbor (chýba IEND)"));
        }
        let dlzka = be32(&data[poz..]) as usize;
        if dlzka > i32::MAX as usize || poz + 12 + dlzka > data.len() {
            return Err(chyba("kúsok presahuje koniec súboru"));
        }
        let typ: [u8; 4] = data[poz + 4..poz + 8].try_into().unwrap_or_default();
        let obsah = &data[poz + 8..poz + 8 + dlzka];
        let crc = be32(&data[poz + 8 + dlzka..]);
        poz += 12 + dlzka;
        let kriticky = typ[0] & 0x20 == 0;
        let mut c = Crc32::novy();
        c.pridaj(&typ);
        c.pridaj(obsah);
        if c.hodnota() != crc {
            if kriticky {
                return Err(chyba(&format!("kontrolný súčet kúska {} nesedí", latin1(&typ))));
            }
            continue;
        }
        if &typ != b"IHDR" && hlavicka.is_none() {
            return Err(chyba("prvý kúsok nie je IHDR"));
        }
        if &typ != b"IDAT" && !idat.is_empty() {
            idat_skoncil = true;
        }
        if !kriticky {
            metadat += dlzka as u64;
            if metadat > limity.metadat {
                return Err(Chyba::Limit(format!("PNG: metadáta nad {} B", limity.metadat)));
            }
        }
        match &typ {
            b"IHDR" => {
                if hlavicka.is_some() {
                    return Err(chyba("dve hlavičky IHDR"));
                }
                let h = over_hlavicku(obsah)?;
                // najviac štyri výstupné kanály (paleta s tRNS → RGBA)
                limity.over(h.sirka as u64, h.vyska as u64, if h.hlbka == 16 { 8 } else { 4 })?;
                hlavicka = Some(h);
            }
            b"PLTE" => {
                if !dlzka.is_multiple_of(3) || dlzka == 0 || dlzka > 768 {
                    return Err(chyba("neplatná paleta"));
                }
                paleta = obsah.chunks(3).map(|p| [p[0], p[1], p[2]]).collect();
            }
            b"tRNS" => trns = Some(obsah.to_vec()),
            b"IDAT" => {
                if idat_skoncil {
                    return Err(chyba("kúsky IDAT nie sú za sebou"));
                }
                idat.extend_from_slice(obsah);
            }
            b"IEND" => koniec = true,
            b"gAMA" if dlzka == 4 => farba.gama = Some(be32(obsah) as f64 / 100_000.0),
            b"cHRM" if dlzka == 32 => {
                let mut ch = [0.0; 8];
                for (i, x) in ch.iter_mut().enumerate() {
                    *x = be32(&obsah[4 * i..]) as f64 / 100_000.0;
                }
                farba.chromaticity = Some(ch);
            }
            b"sRGB" if dlzka == 1 => farba.srgb = Some(obsah[0]),
            b"cICP" if dlzka == 4 => {
                farba.cicp = Some(Cicp {
                    zakladne: obsah[0],
                    prenos: obsah[1],
                    matica: obsah[2],
                    plny_rozsah: obsah[3] != 0,
                })
            }
            b"iCCP" => {
                let nula = obsah.iter().position(|&b| b == 0).ok_or_else(|| chyba("iCCP bez mena"))?;
                if nula + 2 > dlzka || obsah[nula + 1] != 0 {
                    return Err(chyba("iCCP: neznáma kompresia"));
                }
                // poškodený profil nie je dôvod obrázok nezobraziť – farbu potom určí sRGB
                if let Ok(p) = deflate::zlib_rozbal(&obsah[nula + 2..], limity.metadat as usize) {
                    farba.icc = Some(p);
                }
            }
            b"eXIf" => meta.exif = Some(obsah.to_vec()),
            b"pHYs" if dlzka == 9 && obsah[8] == 1 => {
                meta.dpi = Some((be32(obsah) as f64 * 0.0254, be32(&obsah[4..]) as f64 * 0.0254))
            }
            b"tEXt" => {
                if let Some(n) = obsah.iter().position(|&b| b == 0) {
                    meta.texty.push((latin1(&obsah[..n]), latin1(&obsah[n + 1..])));
                }
            }
            b"zTXt" => {
                if let Some(n) = obsah.iter().position(|&b| b == 0)
                    && n + 2 <= dlzka
                    && let Ok(t) = deflate::zlib_rozbal(&obsah[n + 2..], limity.metadat as usize)
                {
                    meta.texty.push((latin1(&obsah[..n]), latin1(&t)));
                }
            }
            b"iTXt" => {
                if let Some(t) = itxt(obsah, limity) {
                    if t.0 == "XML:com.adobe.xmp" {
                        meta.xmp = Some(t.1.into_bytes());
                    } else {
                        meta.texty.push(t);
                    }
                }
            }
            _ if kriticky => return Err(chyba(&format!("neznámy kritický kúsok {}", latin1(&typ)))),
            _ => {}
        }
    }
    // surové profily (konvencia ImageMagick/ExifTool): IPTC a Exif v textoch „Raw profile type …“
    meta.texty.retain(|(k, t)| match k.as_str() {
        "Raw profile type iptc" => {
            meta.iptc = crate::exif::surovy_profil(t).map(|d| {
                if d.starts_with(b"8BIM") { crate::exif::iptc_z_8bim(&d).unwrap_or_default() } else { d }
            });
            false
        }
        "Raw profile type exif" | "Raw profile type APP1" if meta.exif.is_none() => {
            meta.exif = crate::exif::surovy_profil(t)
                .map(|e| e.strip_prefix(b"Exif\0\0").map(<[u8]>::to_vec).unwrap_or(e));
            false
        }
        _ => true,
    });
    let h = hlavicka.ok_or_else(|| chyba("chýba IHDR"))?;
    if h.typ_farby == 3 && paleta.is_empty() {
        return Err(chyba("obrázok s paletou bez kúska PLTE"));
    }
    let surove = rozbal_riadky(&h, &idat)?;
    let mut o = rozvin(&h, &surove, &paleta, trns.as_deref())?;
    o.farba = farba;
    o.metadata = meta;
    if let Some(exif) = &o.metadata.exif {
        o.metadata.natocenie = crate::exif::natocenie(exif).unwrap_or(1);
    }
    Ok(o)
}

/// Text iTXt: kľúč, príznak kompresie, metóda, jazyk, preložený kľúč, text (UTF-8).
fn itxt(obsah: &[u8], limity: &Limity) -> Option<(String, String)> {
    let n = obsah.iter().position(|&b| b == 0)?;
    let kluc = String::from_utf8_lossy(&obsah[..n]).into_owned();
    let (komp, metoda) = (*obsah.get(n + 1)?, *obsah.get(n + 2)?);
    let mut z = n + 3;
    for _ in 0..2 {
        z += obsah.get(z..)?.iter().position(|&b| b == 0)? + 1;
    }
    let text = match (komp, metoda) {
        (0, _) => obsah[z..].to_vec(),
        (1, 0) => deflate::zlib_rozbal(&obsah[z..], limity.metadat as usize).ok()?,
        _ => return None,
    };
    Some((kluc, String::from_utf8_lossy(&text).into_owned()))
}

/// Polohy a kroky prechodov Adam7: (x0, y0, krok x, krok y).
const ADAM7: [(usize, usize, usize, usize); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// Rozmery prechodov (pri neprekladanom jeden prechod celého obrázka).
fn prechody(h: &Hlavicka) -> Vec<(usize, usize, usize, usize, usize, usize)> {
    let (w, v) = (h.sirka as usize, h.vyska as usize);
    if !h.prekladane {
        return vec![(0, 0, 1, 1, w, v)];
    }
    ADAM7
        .iter()
        .map(|&(x0, y0, dx, dy)| (x0, y0, dx, dy, (w + dx - 1 - x0) / dx, (v + dy - 1 - y0) / dy))
        .collect()
}

/// Rozbalí IDAT a odstráni filtre. Vráti riadky všetkých prechodov za sebou (bez bajtov filtra).
fn rozbal_riadky(h: &Hlavicka, idat: &[u8]) -> Vysledok<Vec<u8>> {
    let mut ocakavane = 0usize;
    for &(.., pw, pv) in &prechody(h) {
        if pw > 0 && pv > 0 {
            ocakavane += pv * (h.bajtov_riadku(pw) + 1);
        }
    }
    let mut d = deflate::zlib_rozbal(idat, ocakavane)?;
    if d.len() < ocakavane {
        return Err(chyba("obrazové údaje sú neúplné"));
    }
    let bpp = (h.vzoriek_na_pixel() * h.hlbka as usize).div_ceil(8);
    let mut von = Vec::with_capacity(ocakavane);
    let mut poz = 0;
    for &(.., pw, pv) in &prechody(h) {
        if pw == 0 || pv == 0 {
            continue;
        }
        let r = h.bajtov_riadku(pw);
        let zaciatok = von.len();
        for y in 0..pv {
            let filter = d[poz];
            let riadok = &mut d[poz + 1..poz + 1 + r];
            let predosly: &[u8] = if y == 0 { &[] } else { &von[zaciatok + (y - 1) * r..zaciatok + y * r] };
            odfiltruj(filter, riadok, predosly, bpp)?;
            von.extend_from_slice(riadok);
            poz += r + 1;
        }
    }
    Ok(von)
}

#[inline]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn odfiltruj(filter: u8, r: &mut [u8], predosly: &[u8], bpp: usize) -> Vysledok<()> {
    let hore = |i: usize| -> u8 { if predosly.is_empty() { 0 } else { predosly[i] } };
    match filter {
        0 => {}
        1 => {
            for i in bpp..r.len() {
                r[i] = r[i].wrapping_add(r[i - bpp]);
            }
        }
        2 => {
            if !predosly.is_empty() {
                for (x, &p) in r.iter_mut().zip(predosly) {
                    *x = x.wrapping_add(p);
                }
            }
        }
        3 => {
            for i in 0..r.len() {
                let a = if i >= bpp { r[i - bpp] as u16 } else { 0 };
                r[i] = r[i].wrapping_add(((a + hore(i) as u16) / 2) as u8);
            }
        }
        4 => {
            for i in 0..r.len() {
                let (a, c) = if i >= bpp { (r[i - bpp], hore(i - bpp)) } else { (0, 0) };
                r[i] = r[i].wrapping_add(paeth(a, hore(i), c));
            }
        }
        _ => return Err(chyba(&format!("neznámy filter riadku {filter}"))),
    }
    Ok(())
}

/// Vzorka `i` riadku s hĺbkou pod 8 bitov.
#[inline]
fn male_bity(riadok: &[u8], i: usize, hlbka: usize) -> u8 {
    let bit = i * hlbka;
    (riadok[bit / 8] >> (8 - hlbka - bit % 8)) & ((1 << hlbka) - 1) as u8
}

/// Rozvinie odfiltrované riadky do obrazu (paleta → RGB(A), tRNS → alfa, nízke hĺbky → 8 bitov, Adam7).
fn rozvin(h: &Hlavicka, surove: &[u8], paleta: &[[u8; 3]], trns: Option<&[u8]>) -> Vysledok<Obraz> {
    let (w, v) = (h.sirka as usize, h.vyska as usize);
    let hlbka = h.hlbka as usize;
    // výstupné kanály
    let (model, alfa_z_trns) = match h.typ_farby {
        0 => (Model::Siva, trns.is_some_and(|t| t.len() >= 2)),
        2 => (Model::Rgb, trns.is_some_and(|t| t.len() >= 6)),
        3 => (Model::Rgb, trns.is_some()),
        4 => (Model::Siva, false),
        _ => (Model::Rgb, false),
    };
    let alfa = alfa_z_trns || h.typ_farby == 4 || h.typ_farby == 6;
    let kanaly = if model == Model::Siva { 1 } else { 3 } + alfa as usize;
    let sesnast = hlbka == 16;
    let vstupne = h.vzoriek_na_pixel();
    let mut v8 = if sesnast { Vec::new() } else { vec![0u8; w * v * kanaly] };
    let mut v16 = if sesnast { vec![0u16; w * v * kanaly] } else { Vec::new() };
    // hodnota priehľadnej farby pre sivú a RGB
    let kluc: Option<[u16; 3]> = match (h.typ_farby, trns) {
        (0, Some(t)) if t.len() >= 2 => {
            let g = u16::from_be_bytes([t[0], t[1]]);
            Some([g, g, g])
        }
        (2, Some(t)) if t.len() >= 6 => Some([
            u16::from_be_bytes([t[0], t[1]]),
            u16::from_be_bytes([t[2], t[3]]),
            u16::from_be_bytes([t[4], t[5]]),
        ]),
        _ => None,
    };
    let skala = match hlbka {
        1 => 255,
        2 => 85,
        4 => 17,
        _ => 1,
    };
    let mut poz = 0;
    let mut px = [0u16; 4];
    for (x0, y0, dx, dy, pw, pv) in prechody(h) {
        if pw == 0 || pv == 0 {
            continue;
        }
        let r = h.bajtov_riadku(pw);
        for py in 0..pv {
            let riadok = &surove[poz..poz + r];
            poz += r;
            let y = y0 + py * dy;
            for pxi in 0..pw {
                let x = x0 + pxi * dx;
                // vzorky pixela v pôvodnej hĺbke
                for (k, p) in px.iter_mut().enumerate().take(vstupne) {
                    let i = pxi * vstupne + k;
                    *p = match hlbka {
                        16 => u16::from_be_bytes([riadok[2 * i], riadok[2 * i + 1]]),
                        8 => riadok[i] as u16,
                        _ => male_bity(riadok, i, hlbka) as u16,
                    };
                }
                let ciel = (y * w + x) * kanaly;
                if h.typ_farby == 3 {
                    let idx = px[0] as usize;
                    let farba = paleta.get(idx).ok_or_else(|| chyba("index mimo palety"))?;
                    v8[ciel..ciel + 3].copy_from_slice(farba);
                    if alfa {
                        v8[ciel + 3] = trns.and_then(|t| t.get(idx).copied()).unwrap_or(255);
                    }
                    continue;
                }
                let priehladny = kluc.is_some_and(|k| (0..vstupne.min(3)).all(|c| px[c] == k[c]));
                if sesnast {
                    v16[ciel..ciel + vstupne].copy_from_slice(&px[..vstupne]);
                    if alfa_z_trns {
                        v16[ciel + vstupne] = if priehladny { 0 } else { 65_535 };
                    }
                } else {
                    for c in 0..vstupne {
                        v8[ciel + c] = (px[c] * skala) as u8;
                    }
                    if alfa_z_trns {
                        v8[ciel + vstupne] = if priehladny { 0 } else { 255 };
                    }
                }
            }
        }
    }
    let vzorky = if sesnast { Vzorky::U16(v16) } else { Vzorky::U8(v8) };
    let mut o = Obraz::novy(w, v, model, alfa, vzorky)?;
    o.bitov = if h.typ_farby == 3 { 8 } else { h.hlbka };
    Ok(o)
}

/// Nastavenie zápisu PNG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NastaveniePng {
    /// Úroveň kompresie 0–9.
    pub uroven: u8,
    /// Skúsiť paletu, ak má obrázok najviac 256 farieb (bez straty).
    pub paleta: bool,
}

impl Default for NastaveniePng {
    fn default() -> Self {
        NastaveniePng { uroven: 6, paleta: true }
    }
}

fn kusok(von: &mut Vec<u8>, typ: &[u8; 4], obsah: &[u8]) {
    von.extend_from_slice(&(obsah.len() as u32).to_be_bytes());
    let zaciatok = von.len();
    von.extend_from_slice(typ);
    von.extend_from_slice(obsah);
    let crc = crc32(&von[zaciatok..]);
    von.extend_from_slice(&crc.to_be_bytes());
}

/// Zapíše obrázok do PNG. Plávajúce vzorky sa uložia ako 16 bitov (orezané do 0–1); CMYK nie je dovolené.
pub fn zapis(o: &Obraz, n: &NastaveniePng) -> Vysledok<Vec<u8>> {
    o.over()?;
    let typ_farby = match (o.model, o.alfa, o.kanaly()) {
        (Model::Siva, false, 1) => 0u8,
        (Model::Siva, true, 2) => 4,
        (Model::Rgb, false, 3) => 2,
        (Model::Rgb, true, 4) => 6,
        _ => {
            return Err(Chyba::NezhodaTypov {
                popis: format!("PNG nevie uložiť model {:?} s {} kanálmi", o.model, o.kanaly()),
                navrh: Some("previesť na RGB alebo sivú".into()),
            });
        }
    };
    if o.sirka > i32::MAX as usize || o.vyska > i32::MAX as usize {
        return Err(Chyba::Limit("PNG: rozmer nad 2^31 − 1".into()));
    }
    let k = o.kanaly();
    // vzorky v bajtoch po riadkoch (16 bitov big-endian)
    let (hlbka, bajty): (u8, Vec<u8>) = match &o.vzorky {
        Vzorky::U8(v) => (8, v.clone()),
        Vzorky::U16(v) => (16, v.iter().flat_map(|x| x.to_be_bytes()).collect()),
        Vzorky::F32(v) => (
            16,
            v.iter().flat_map(|&x| ((x.clamp(0.0, 1.0) * 65_535.0 + 0.5) as u16).to_be_bytes()).collect(),
        ),
    };
    // paleta: najviac 256 rôznych pixelov v 8 bitoch RGB/RGBA
    let paleta = if n.paleta && hlbka == 8 && o.model == Model::Rgb { najdi_paletu(&bajty, k) } else { None };
    let mut von = PODPIS.to_vec();
    let (typ, h_bitov, riadky, bpp) = if let Some((farby, indexy)) = &paleta {
        let bity = match farby.len() {
            0..=2 => 1,
            3..=4 => 2,
            5..=16 => 4,
            _ => 8,
        };
        let r = (o.sirka * bity).div_ceil(8);
        let mut zbalene = vec![0u8; r * o.vyska];
        for y in 0..o.vyska {
            for x in 0..o.sirka {
                let i = indexy[y * o.sirka + x] as usize;
                let bit = x * bity;
                zbalene[y * r + bit / 8] |= (i << (8 - bity - bit % 8)) as u8;
            }
        }
        (3u8, bity as u8, zbalene, 1)
    } else {
        (typ_farby, hlbka, bajty, k * hlbka as usize / 8)
    };
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(o.sirka as u32).to_be_bytes());
    ihdr.extend_from_slice(&(o.vyska as u32).to_be_bytes());
    ihdr.extend_from_slice(&[h_bitov, typ, 0, 0, 0]);
    kusok(&mut von, b"IHDR", &ihdr);
    // farba
    if let Some(c) = &o.farba.cicp {
        kusok(&mut von, b"cICP", &[c.zakladne, c.prenos, c.matica, c.plny_rozsah as u8]);
    }
    if let Some(icc) = &o.farba.icc {
        let mut d = b"ICC\0\0".to_vec();
        d.extend_from_slice(&deflate::zlib_zbal(icc, 9));
        kusok(&mut von, b"iCCP", &d);
    } else if let Some(z) = o.farba.srgb {
        kusok(&mut von, b"sRGB", &[z]);
    } else {
        if let Some(g) = o.farba.gama {
            kusok(&mut von, b"gAMA", &((g * 100_000.0).round() as u32).to_be_bytes());
        }
        if let Some(ch) = o.farba.chromaticity {
            let d: Vec<u8> =
                ch.iter().flat_map(|&x| ((x * 100_000.0).round() as u32).to_be_bytes()).collect();
            kusok(&mut von, b"cHRM", &d);
        }
    }
    if let Some((farby, _)) = &paleta {
        let plte: Vec<u8> = farby.iter().flat_map(|f| [f[0], f[1], f[2]]).collect();
        kusok(&mut von, b"PLTE", &plte);
        if k == 4 {
            let mut a: Vec<u8> = farby.iter().map(|f| f[3]).collect();
            while a.last() == Some(&255) {
                a.pop();
            }
            if !a.is_empty() {
                kusok(&mut von, b"tRNS", &a);
            }
        }
    }
    if let Some((x, y)) = o.metadata.dpi {
        let mut d = Vec::new();
        d.extend_from_slice(&((x / 0.0254).round() as u32).to_be_bytes());
        d.extend_from_slice(&((y / 0.0254).round() as u32).to_be_bytes());
        d.push(1);
        kusok(&mut von, b"pHYs", &d);
    }
    if let Some(e) = &o.metadata.exif {
        kusok(&mut von, b"eXIf", e);
    }
    for (kl, t) in &o.metadata.texty {
        // iTXt: UTF-8 bez kompresie
        let mut d = kl.as_bytes().iter().copied().take(79).collect::<Vec<u8>>();
        d.extend_from_slice(&[0, 0, 0, 0, 0]);
        d.extend_from_slice(t.as_bytes());
        kusok(&mut von, b"iTXt", &d);
    }
    if let Some(i) = &o.metadata.iptc {
        let mut d = b"Raw profile type iptc\0\0".to_vec();
        d.extend(deflate::zlib_zbal(crate::exif::zapis_surovy_profil("iptc", i).as_bytes(), 6));
        kusok(&mut von, b"zTXt", &d);
    }
    if let Some(x) = &o.metadata.xmp {
        let mut d = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        d.extend_from_slice(x);
        kusok(&mut von, b"iTXt", &d);
    }
    let r = riadky.len() / o.vyska;
    let filtrovane = filtruj(&riadky, r, bpp, typ == 3 || h_bitov < 8);
    let z = deflate::zlib_zbal(&filtrovane, n.uroven);
    for kus in z.chunks(1 << 20) {
        kusok(&mut von, b"IDAT", kus);
    }
    kusok(&mut von, b"IEND", &[]);
    Ok(von)
}

/// Paleta (najviac 256 farieb v poradí výskytu) a index každého pixela, ak sa obrázok do nej zmestí.
fn najdi_paletu(bajty: &[u8], k: usize) -> Option<(Vec<[u8; 4]>, Vec<u8>)> {
    let mut mapa: HashMap<[u8; 4], u8> = HashMap::new();
    let mut farby = Vec::new();
    let mut indexy = Vec::with_capacity(bajty.len() / k);
    for p in bajty.chunks_exact(k) {
        let f = [p[0], p[1], p[2], if k == 4 { p[3] } else { 255 }];
        let i = match mapa.get(&f) {
            Some(&i) => i,
            None => {
                if farby.len() == 256 {
                    return None;
                }
                farby.push(f);
                mapa.insert(f, (farby.len() - 1) as u8);
                (farby.len() - 1) as u8
            }
        };
        indexy.push(i);
    }
    // priehľadné farby dopredu – tRNS potom môže byť kratší
    if k == 4 {
        let mut poradie: Vec<usize> = (0..farby.len()).collect();
        poradie.sort_by_key(|&i| (farby[i][3] == 255, i));
        let mut nove = vec![0u8; farby.len()];
        for (novy, &stary) in poradie.iter().enumerate() {
            nove[stary] = novy as u8;
        }
        let farby2: Vec<[u8; 4]> = poradie.iter().map(|&i| farby[i]).collect();
        for i in indexy.iter_mut() {
            *i = nove[*i as usize];
        }
        return Some((farby2, indexy));
    }
    Some((farby, indexy))
}

/// Filtre riadkov. Pre paletu a nízke hĺbky norma odporúča bez filtra; inak sa vyberie filter
/// s najmenším súčtom absolútnych hodnôt (bajty ako čísla so znamienkom).
fn filtruj(riadky: &[u8], r: usize, bpp: usize, bez_filtra: bool) -> Vec<u8> {
    let vyska = riadky.len() / r;
    let mut von = Vec::with_capacity(vyska * (r + 1));
    let mut kandidat = vec![vec![0u8; r]; 5];
    let nuly = vec![0u8; r];
    for y in 0..vyska {
        let riadok = &riadky[y * r..(y + 1) * r];
        if bez_filtra {
            von.push(0);
            von.extend_from_slice(riadok);
            continue;
        }
        let hore = if y == 0 { &nuly[..] } else { &riadky[(y - 1) * r..y * r] };
        for i in 0..r {
            let a = if i >= bpp { riadok[i - bpp] } else { 0 };
            let c = if i >= bpp { hore[i - bpp] } else { 0 };
            let b = hore[i];
            let x = riadok[i];
            kandidat[0][i] = x;
            kandidat[1][i] = x.wrapping_sub(a);
            kandidat[2][i] = x.wrapping_sub(b);
            kandidat[3][i] = x.wrapping_sub(((a as u16 + b as u16) / 2) as u8);
            kandidat[4][i] = x.wrapping_sub(paeth(a, b, c));
        }
        let cena = |k: &[u8]| -> u64 { k.iter().map(|&x| (x as i8).unsigned_abs() as u64).sum() };
        let (najlepsi, _) =
            (0..5).map(|f| (f, cena(&kandidat[f]))).min_by_key(|&(f, c)| (c, f)).unwrap_or((0, 0));
        von.push(najlepsi as u8);
        von.extend_from_slice(&kandidat[najlepsi]);
    }
    von
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    fn obraz(w: usize, h: usize, model: Model, alfa: bool, sesnast: bool, farieb: u64) -> Obraz {
        let mut n = Nahoda::zo_semienka((w * 31 + h) as u64);
        let k = if model == Model::Siva { 1 } else { 3 } + alfa as usize;
        let vzorky = if sesnast {
            Vzorky::U16(
                (0..w * h * k).map(|i| (i as u16).wrapping_mul(97) ^ (n.mensie_ako(4) as u16)).collect(),
            )
        } else {
            let paleta: Vec<u8> = (0..farieb * k as u64).map(|_| n.dalsie() as u8).collect();
            let mut v = Vec::new();
            for _ in 0..w * h {
                let i = n.mensie_ako(farieb) as usize * k;
                v.extend_from_slice(&paleta[i..i + k]);
            }
            Vzorky::U8(v)
        };
        Obraz::novy(w, h, model, alfa, vzorky).unwrap()
    }

    #[test]
    fn tam_a_spat() {
        let l = Limity::default();
        for (model, alfa) in
            [(Model::Siva, false), (Model::Siva, true), (Model::Rgb, false), (Model::Rgb, true)]
        {
            for sesnast in [false, true] {
                for farieb in [2, 13, 200, 100_000] {
                    for (w, h) in [(1, 1), (7, 3), (64, 33)] {
                        let o = obraz(w, h, model, alfa, sesnast, farieb);
                        for paleta in [false, true] {
                            let p = zapis(&o, &NastaveniePng { uroven: 6, paleta }).unwrap();
                            let mut spat = citaj(&p, &l).unwrap();
                            spat.bitov = o.bitov;
                            assert_eq!(
                                spat, o,
                                "{model:?} alfa {alfa} 16b {sesnast} farieb {farieb} {w}×{h}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn metadata_a_farba() {
        let mut o = obraz(5, 4, Model::Rgb, false, false, 300);
        o.farba.icc = Some((0..3000u32).map(|i| (i * 7) as u8).collect());
        o.farba.cicp = Some(Cicp { zakladne: 9, prenos: 16, matica: 0, plny_rozsah: true });
        o.metadata.texty.push(("Názov".into(), "Orchidea – skúška".into()));
        o.metadata.dpi = Some((300.0, 300.0));
        let spat = citaj(&zapis(&o, &NastaveniePng::default()).unwrap(), &Limity::default()).unwrap();
        assert_eq!(spat.farba.icc, o.farba.icc);
        assert_eq!(spat.farba.cicp, o.farba.cicp);
        assert_eq!(spat.metadata.texty, o.metadata.texty);
        let (dx, dy) = spat.metadata.dpi.unwrap();
        assert!((dx - 300.0).abs() < 0.1 && (dy - 300.0).abs() < 0.1);
    }

    #[test]
    fn limit_a_poskodenie() {
        let o = obraz(40, 40, Model::Rgb, false, false, 100_000);
        let p = zapis(&o, &NastaveniePng::default()).unwrap();
        assert!(matches!(citaj(&p, &Limity { pixelov: 1000, ..Limity::default() }), Err(Chyba::Limit(_))));
        let mut zle = p.clone();
        zle[40] ^= 0x55;
        assert!(citaj(&zle, &Limity::default()).is_err());
        assert!(citaj(&p[..p.len() - 13], &Limity::default()).is_err());
        assert_eq!(citaj_hlavicku(&p).unwrap().sirka, 40);
    }
}
