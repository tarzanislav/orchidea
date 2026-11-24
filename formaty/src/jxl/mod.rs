//! JPEG XL (ISO/IEC 18181-1 a -2) – vlastný dekodér.
//!
//! Kontajner (krabice `jxlc`, `jxlp`, `Exif`, `xml `) aj holý kódový tok. Režim **Modular** (bezstratové
//! obrázky: rozhodovací strom, prediktory vrátane váženého, RCT, paleta, Squeeze) a režim **VarDCT**
//! (stratové obrázky). Entropické kódovanie ANS aj prefixové kódy s LZ77. Výstup v pôvodnom farebnom
//! priestore súboru (pri XYB prevod do RGB s prenosovou funkciou z hlavičky).
//!
//! Zatiaľ nie: animácie (číta sa prvá snímka), vrstvy s prekrytím, záplaty, krivky, šum, prekódovaný JPEG,
//! stratový Modular v XYB – oznámi sa to chybou. Obraz v XYB s vloženým profilom ICC sa zatiaľ vydá v sRGB.

mod bity;
mod entropia;
mod hlavicky;
mod icc;
mod modular;
mod snimka;
mod vardct;
mod zvacsenie;

use crate::obraz::{Cicp, Limity, Model, Obraz, Vzorky};
use bity::{Citac, chyba};
use orchidea_jadro::Vysledok;
use snimka::Vysledok3;

pub const PODPIS_TOKU: [u8; 2] = [0xFF, 0x0A];
pub const PODPIS_KONTAJNERA: [u8; 12] = [0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A];

/// Kódový tok a metadáta z kontajnera.
struct Obsah {
    tok: Vec<u8>,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
}

fn rozbal_kontajner(data: &[u8]) -> Vysledok<Obsah> {
    if data.starts_with(&PODPIS_TOKU) {
        return Ok(Obsah { tok: data.to_vec(), exif: None, xmp: None });
    }
    if !data.starts_with(&PODPIS_KONTAJNERA) {
        return Err(chyba("chýba podpis"));
    }
    let mut p = 0usize;
    let mut tok = Vec::new();
    let mut casti: Vec<(u32, Vec<u8>)> = Vec::new();
    let (mut exif, mut xmp) = (None, None);
    while p + 8 <= data.len() {
        let mut dlzka = u32::from_be_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]) as u64;
        let typ = &data[p + 4..p + 8];
        let mut hlavicka = 8u64;
        if dlzka == 1 {
            if p + 16 > data.len() {
                return Err(chyba("neúplná krabica"));
            }
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[p + 8..p + 16]);
            dlzka = u64::from_be_bytes(b);
            hlavicka = 16;
        } else if dlzka == 0 {
            dlzka = (data.len() - p) as u64;
        }
        if dlzka < hlavicka || p as u64 + dlzka > data.len() as u64 {
            return Err(chyba("krabica presahuje súbor"));
        }
        let obsah = &data[p + hlavicka as usize..p + dlzka as usize];
        match typ {
            b"jxlc" => tok.extend_from_slice(obsah),
            b"jxlp" if obsah.len() >= 4 => {
                let i = u32::from_be_bytes([obsah[0], obsah[1], obsah[2], obsah[3]]) & 0x7FFF_FFFF;
                casti.push((i, obsah[4..].to_vec()));
            }
            b"Exif" if obsah.len() >= 4 => {
                let posun = u32::from_be_bytes([obsah[0], obsah[1], obsah[2], obsah[3]]) as usize;
                if 4 + posun <= obsah.len() {
                    exif = Some(obsah[4 + posun..].to_vec());
                }
            }
            b"xml " => xmp = Some(obsah.to_vec()),
            _ => {}
        }
        p += dlzka as usize;
    }
    if tok.is_empty() {
        casti.sort_by_key(|c| c.0);
        for (_, c) in casti {
            tok.extend_from_slice(&c);
        }
    }
    if !tok.starts_with(&PODPIS_TOKU) {
        return Err(chyba("kontajner bez kódového toku"));
    }
    Ok(Obsah { tok, exif, xmp })
}

/// Rozmery obrázka bez dekódovania.
pub fn citaj_hlavicku(data: &[u8]) -> Vysledok<(usize, usize)> {
    let o = rozbal_kontajner(data)?;
    let mut c = Citac::novy(&o.tok[2..]);
    let (w, h) = hlavicky::velkost(&mut c)?;
    c.over()?;
    Ok((w as usize, h as usize))
}

/// Prečíta obrázok JPEG XL (prvú zobrazenú snímku).
pub fn citaj(data: &[u8], limity: &Limity) -> Vysledok<Obraz> {
    let obsah = rozbal_kontajner(data)?;
    let tok = &obsah.tok[2..];
    let mut c = Citac::novy(tok);
    let (w, h) = hlavicky::velkost(&mut c)?;
    let m = hlavicky::metadata(&mut c)?;
    let kanalov = 3 + m.dodatocne.len() as u64;
    limity.over(w as u64, h as u64, kanalov * 8)?;
    let icc = if m.farba.chce_icc { Some(icc::citaj(&mut c, limity.metadat)?) } else { None };
    c.zarovnaj();
    c.over()?;
    let mut posun = (c.precitane() / 8) as usize;
    if let Some((nw, nh)) = m.nahlad {
        // náhľad sa dekóduje len na preskočenie
        let mut cs = Citac::novy(&tok[posun..]);
        let (_, koniec) = snimka::dekoduj(&mut cs, &tok[posun..], &m, nw, nh, &[], limity)?;
        posun += koniec;
    }
    // prvá zobrazená snímka; pred ňou môžu byť LF snímky (progresívny obraz) a referenčné snímky
    let mut lf_snimky: [Option<[Vec<f32>; 3]>; 4] = [None, None, None, None];
    let s = loop {
        if posun >= tok.len() {
            return Err(chyba("súbor nemá zobrazenú snímku"));
        }
        let mut cs = Citac::novy(&tok[posun..]);
        let (s, koniec) = snimka::dekoduj(&mut cs, &tok[posun..], &m, w, h, &lf_snimky, limity)?;
        posun += koniec;
        match (s.hlavicka.typ, s.vysledok) {
            (1, Vysledok3::Xyb(r)) => lf_snimky[s.hlavicka.lf_uroven as usize] = Some(r),
            (1, _) => return Err(chyba("LF snímka bez XYB zatiaľ nie je podporená")),
            (2, _) => {} // referenčná snímka sa zatiaľ nepoužíva (záplaty nie sú podporené)
            (_, v) => break snimka::Snimka { hlavicka: s.hlavicka, vysledok: v },
        }
    };
    if !s.hlavicka.posledna && m.animacia.is_none() {
        return Err(chyba("vrstvy s prekrytím zatiaľ nie sú podporené"));
    }
    let xyb = matches!(s.vysledok, Vysledok3::Plavajuce(..));
    let mut o = vystup(s, &m, w as usize, h as usize)?;
    if let Some(p) = icc {
        if xyb {
            // pixely sú prevedené do sRGB; prevod do vloženého profilu príde s balíkom farba
            o.farba.cicp = Some(Cicp { zakladne: 1, prenos: 13, matica: 0, plny_rozsah: true });
            o.farba.srgb = Some(0);
        } else {
            o.farba.icc = Some(p);
        }
    }
    o.metadata.natocenie = m.natocenie as u8;
    o.metadata.exif = obsah.exif;
    o.metadata.xmp = obsah.xmp;
    Ok(o)
}

/// Prenosová funkcia sRGB z lineárneho svetla.
fn srgb_z_lin(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        (1.055 * orchidea_jadro::matematika::mocnina(v as f64, 1.0 / 2.4) - 0.055) as f32
    }
}

/// BT.709 z lineárneho svetla.
fn bt709_z_lin(v: f32) -> f32 {
    if v < 0.018 {
        4.5 * v
    } else {
        (1.099 * orchidea_jadro::matematika::mocnina(v as f64, 0.45) - 0.099) as f32
    }
}

fn vystup(s: snimka::Snimka, m: &hlavicky::Metadata, w: usize, h: usize) -> Vysledok<Obraz> {
    if s.hlavicka.sirka as usize != w
        || s.hlavicka.vyska as usize != h
        || s.hlavicka.x0 != 0
        || s.hlavicka.y0 != 0
    {
        return Err(chyba("snímka s orezaním zatiaľ nie je podporená"));
    }
    let sivy = m.farba.priestor == 1;
    let farebnych = if sivy { 1 } else { 3 };
    let alfa_i = m.dodatocne.iter().position(|d| d.typ == hlavicky::KANAL_ALFA);
    let k = farebnych + alfa_i.is_some() as usize;
    let model = if sivy { Model::Siva } else { Model::Rgb };
    let hlbka = m.hlbka;
    let px = w * h;
    let mut o = match s.vysledok {
        Vysledok3::Xyb(_) => return Err(chyba("zobrazená snímka nemá prevod farieb")),
        Vysledok3::Cele(kanaly) => {
            let alfa = alfa_i.map(|i| &kanaly[farebnych + i]);
            let zdroj: Vec<&Vec<i32>> =
                kanaly[..farebnych].iter().map(|k| &k.data).chain(alfa.map(|a| &a.data)).collect();
            let hlbky: Vec<hlavicky::Hlbka> =
                (0..farebnych).map(|_| hlbka).chain(alfa_i.map(|i| m.dodatocne[i].hlbka)).collect();
            if hlbka.plavajuca {
                let mut v = vec![0f32; px * k];
                for (c, (d, hb)) in zdroj.iter().zip(&hlbky).enumerate() {
                    for i in 0..px {
                        v[i * k + c] = cele_na_plavajuce(d[i], hb);
                    }
                }
                Obraz::novy(w, h, model, alfa_i.is_some(), Vzorky::F32(v))?
            } else if hlbka.bitov <= 8 {
                let mut v = vec![0u8; px * k];
                for (c, (d, hb)) in zdroj.iter().zip(&hlbky).enumerate() {
                    let max = ((1u64 << hb.bitov) - 1) as i64;
                    for i in 0..px {
                        let x = (d[i] as i64).clamp(0, max);
                        v[i * k + c] = ((x * 255 + max / 2) / max) as u8;
                    }
                }
                Obraz::novy(w, h, model, alfa_i.is_some(), Vzorky::U8(v))?
            } else {
                let mut v = vec![0u16; px * k];
                for (c, (d, hb)) in zdroj.iter().zip(&hlbky).enumerate() {
                    let max = ((1u64 << hb.bitov.min(31)) - 1) as i64;
                    for i in 0..px {
                        let x = (d[i] as i64).clamp(0, max);
                        v[i * k + c] = ((x * 65_535 + max / 2) / max) as u16;
                    }
                }
                Obraz::novy(w, h, model, alfa_i.is_some(), Vzorky::U16(v))?
            }
        }
        Vysledok3::Kodovane(roviny, kanaly) => {
            let alfa = alfa_i.map(|i| (&kanaly[i].data, m.dodatocne[i].hlbka));
            let mut f = vec![0f32; px * k];
            for i in 0..px {
                for (c, r) in roviny.iter().enumerate().take(farebnych) {
                    f[i * k + c] = r[i];
                }
                if let Some((a, hb)) = &alfa {
                    f[i * k + farebnych] = if hb.plavajuca {
                        cele_na_plavajuce(a[i], hb)
                    } else {
                        a[i] as f32 / ((1u64 << hb.bitov) - 1) as f32
                    };
                }
            }
            let vzorky = if hlbka.plavajuca {
                Vzorky::F32(f)
            } else if hlbka.bitov <= 8 {
                Vzorky::U8(f.iter().map(|&x| (x.clamp(0.0, 1.0) * 255.0).round() as u8).collect())
            } else {
                Vzorky::U16(f.iter().map(|&x| (x.clamp(0.0, 1.0) * 65_535.0).round() as u16).collect())
            };
            Obraz::novy(w, h, model, alfa_i.is_some(), vzorky)?
        }
        Vysledok3::Plavajuce(roviny, kanaly) => {
            // lineárne RGB → prenosová funkcia súboru
            let prenos = |x: f32| -> f32 {
                match (m.farba.gama, m.farba.prenos) {
                    (Some(g), _) => orchidea_jadro::matematika::mocnina(x.max(0.0) as f64, g) as f32,
                    (None, 8) => x,
                    (None, 1) => bt709_z_lin(x),
                    _ => srgb_z_lin(x),
                }
            };
            // pre celé vzorky stačí tabuľka s lineárnou interpoláciou (chyba < 1e-7)
            const N: usize = 65_536;
            let tabulka: Option<Vec<f32>> =
                (!hlbka.plavajuca).then(|| (0..=N).map(|i| prenos(i as f32 / N as f32)).collect());
            let prenos = |x: f32| -> f32 {
                match &tabulka {
                    Some(t) => {
                        let p = x.clamp(0.0, 1.0) * N as f32;
                        let i = (p as usize).min(N - 1);
                        let z = p - i as f32;
                        t[i] + (t[i + 1] - t[i]) * z
                    }
                    None => prenos(x),
                }
            };
            let alfa = alfa_i.map(|i| (&kanaly[i].data, m.dodatocne[i].hlbka));
            let mut f = vec![0f32; px * k];
            for i in 0..px {
                if sivy {
                    f[i * k] = prenos(roviny[1][i]);
                } else {
                    for c in 0..3 {
                        f[i * k + c] = prenos(roviny[c][i]);
                    }
                }
                if let Some((a, hb)) = &alfa {
                    f[i * k + farebnych] = if hb.plavajuca {
                        cele_na_plavajuce(a[i], hb)
                    } else {
                        a[i] as f32 / ((1u64 << hb.bitov) - 1) as f32
                    };
                }
            }
            let vzorky = if hlbka.plavajuca {
                Vzorky::F32(f)
            } else if hlbka.bitov <= 8 {
                Vzorky::U8(f.iter().map(|&x| (x.clamp(0.0, 1.0) * 255.0).round() as u8).collect())
            } else {
                Vzorky::U16(f.iter().map(|&x| (x.clamp(0.0, 1.0) * 65_535.0).round() as u16).collect())
            };
            Obraz::novy(w, h, model, alfa_i.is_some(), vzorky)?
        }
    };
    o.bitov = hlbka.bitov.min(32) as u8;
    if alfa_i.is_some_and(|i| m.dodatocne[i].alfa_nasobena) {
        crate::obraz::rozdel_alfu(&mut o);
    }
    let f = &m.farba;
    if f.priestor != 2 && !f.chce_icc {
        let zakladne = match f.zakladne {
            1 => 1,
            9 => 9,
            11 => 12,
            _ => 2,
        };
        let prenos = match (f.gama, f.prenos) {
            (Some(_), _) => 2,
            (None, p) => p as u8,
        };
        o.farba.cicp = Some(Cicp { zakladne, prenos, matica: 0, plny_rozsah: true });
        if zakladne == 1 && prenos == 13 {
            o.farba.srgb = Some(f.zamer as u8);
        }
        if let Some(g) = f.gama {
            o.farba.gama = Some(g);
        }
        o.farba.linearne = prenos == 8;
    }
    Ok(o)
}

/// Plávajúca vzorka uložená ako celé číslo (bity IEEE so zadaným počtom bitov exponentu).
fn cele_na_plavajuce(v: i32, h: &hlavicky::Hlbka) -> f32 {
    match (h.bitov, h.exp_bitov) {
        (32, 8) => f32::from_bits(v as u32),
        (16, 5) => crate::polovicna::f16_na_f32(v as u16),
        (bitov, exp) => {
            let mant_bitov = bitov - exp - 1;
            let u = v as u32;
            let znamienko = if u >> (bitov - 1) & 1 == 1 { -1.0f64 } else { 1.0 };
            let e = ((u >> mant_bitov) & ((1 << exp) - 1)) as i32;
            let mant = (u & ((1 << mant_bitov) - 1)) as f64;
            let bias = (1i32 << (exp - 1)) - 1;
            let hodnota = if e == 0 {
                mant / (1u64 << mant_bitov) as f64 * 2f64.powi(1 - bias)
            } else {
                (1.0 + mant / (1u64 << mant_bitov) as f64) * 2f64.powi(e - bias)
            };
            (znamienko * hodnota) as f32
        }
    }
}
