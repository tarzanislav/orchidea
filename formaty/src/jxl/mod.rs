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

use orchidea_farba::{Adaptacia, Prenos, Prevod, Priestor, Primarne, Zamer, primarne};
mod modular;
mod skladanie;
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
    let ciel = ciel_xyb(&m, icc.as_deref());
    let mut posun = (c.precitane() / 8) as usize;
    if let Some((nw, nh)) = m.nahlad {
        // náhľad sa dekóduje len na preskočenie
        let mut cs = Citac::novy(&tok[posun..]);
        let (_, koniec) = snimka::dekoduj(&mut cs, &tok[posun..], &m, nw, nh, &[], &[], limity)?;
        posun += koniec;
    }
    // prvá zobrazená snímka; pred ňou môžu byť LF snímky (progresívny obraz), referenčné snímky (záplaty)
    // a vrstvy, ktoré sa skladajú na plátno
    let mut lf_snimky: [Option<[Vec<f32>; 3]>; 4] = [None, None, None, None];
    let mut referencie: [Option<skladanie::Referencia>; 4] = [None, None, None, None];
    let (wu, hu) = (w as usize, h as usize);
    let vysledok = loop {
        if posun >= tok.len() {
            return Err(chyba("súbor nemá zobrazenú snímku"));
        }
        let mut cs = Citac::novy(&tok[posun..]);
        let (s, koniec) = snimka::dekoduj(&mut cs, &tok[posun..], &m, w, h, &lf_snimky, &referencie, limity)?;
        posun += koniec;
        let hl = s.hlavicka;
        match hl.typ {
            1 => {
                let Vysledok3::Platno(p) = s.vysledok else {
                    return Err(chyba("LF snímka bez XYB zatiaľ nie je podporená"));
                };
                let Ok(r) = <[Vec<f32>; 3]>::try_from(p.farba) else {
                    return Err(chyba("LF snímka nemá tri roviny"));
                };
                lf_snimky[hl.lf_uroven as usize] = Some(r);
                continue;
            }
            2 => {
                let platno = na_platno(s.vysledok, &m);
                referencie[hl.uloz_ako as usize] =
                    Some(skladanie::Referencia { platno, pred_ct: hl.uloz_pred_ct });
                continue;
            }
            _ => {}
        }
        let zobrazena = hl.posledna || (m.animacia.is_some() && hl.trvanie > 0);
        let cela = hl.x0 == 0 && hl.y0 == 0 && hl.sirka == w && hl.vyska == h;
        let nahrad = hl.prekrytie.rezim == 0 && hl.prekrytie_dodatocnych.iter().all(|p| p.rezim == 0);
        if zobrazena && cela && nahrad {
            break Konecne::Snimka(s.vysledok);
        }
        let nova = na_platno(s.vysledok, &m);
        let nova = if hl.uloz_pred_ct && !zobrazena { nova } else { prevod_farieb(nova, &m, &ciel) };
        let zaklad = referencie[hl.prekrytie.zdroj as usize].as_ref().map(|r| &r.platno);
        let p = skladanie::prekry(
            zaklad,
            &nova,
            (hl.x0 as i64, hl.y0 as i64),
            (wu, hu),
            &hl.prekrytie,
            &hl.prekrytie_dodatocnych,
            &m,
        )?;
        if zobrazena {
            break Konecne::Platno(p);
        }
        if hl.trvanie == 0 || hl.uloz_ako != 0 {
            referencie[hl.uloz_ako as usize] =
                Some(skladanie::Referencia { platno: p, pred_ct: hl.uloz_pred_ct });
        }
    };
    let xyb_s_icc = m.xyb && icc.is_some();
    // bodové farby (spot colors) sa vykreslia do farby ako pri libjxl
    let bodove = m.dodatocne.iter().any(|d| d.bodova.is_some());
    let mut o = match vysledok {
        Konecne::Snimka(Vysledok3::Cele(k)) if !bodove => vystup_cele(k, &m, wu, hu)?,
        Konecne::Snimka(v) => {
            vystup_platno(vykresli_bodove(prevod_farieb(na_platno(v, &m), &m, &ciel), &m), &m)?
        }
        Konecne::Platno(p) => vystup_platno(vykresli_bodove(p, &m), &m)?,
    };
    o.bitov = m.hlbka.bitov.min(32) as u8;
    let alfa_i = m.dodatocne.iter().position(|d| d.typ == hlavicky::KANAL_ALFA);
    if alfa_i.is_some_and(|i| m.dodatocne[i].alfa_nasobena) {
        crate::obraz::rozdel_alfu(&mut o);
    }
    popis_farby(&mut o, &m, &ciel);
    if let Some(p) = icc {
        if xyb_s_icc && !matches!(ciel, CielXyb::Icc(_)) {
            // profil sa nedal použiť (nepodporovaný druh) – pixely ostali v sRGB
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

/// Výsledok skladania snímok.
enum Konecne {
    /// Jediná snímka cez celé plátno (pred prevodom farieb).
    Snimka(Vysledok3),
    /// Zložené plátno po prevode farieb.
    Platno(skladanie::Platno),
}

/// Výsledok snímky ako plávajúce plátno (pred prevodom farieb).
fn na_platno(v: Vysledok3, m: &hlavicky::Metadata) -> skladanie::Platno {
    match v {
        Vysledok3::Platno(p) => p,
        Vysledok3::Cele(mut kanaly) => {
            let farebnych = kanaly.len() - m.dodatocne.len().min(kanaly.len());
            let dodatocne = kanaly.split_off(farebnych);
            let (w, h) = kanaly.first().map_or((0, 0), |k| (k.w, k.h));
            let plav = |k: &modular::Kanal, hb: &hlavicky::Hlbka| -> Vec<f32> {
                if hb.plavajuca {
                    k.data.iter().map(|&x| cele_na_plavajuce(x, hb)).collect()
                } else {
                    let max = ((1u64 << hb.bitov.min(31)) - 1) as f32;
                    k.data.iter().map(|&x| x as f32 / max).collect()
                }
            };
            skladanie::Platno {
                w,
                h,
                farba: kanaly.iter().map(|k| plav(k, &m.hlbka)).collect(),
                dodatocne: dodatocne.iter().zip(&m.dodatocne).map(|(k, d)| plav(k, &d.hlbka)).collect(),
            }
        }
    }
}

/// Prevod farieb plátna: XYB → lineárne RGB → prenosová funkcia súboru (pri sivom obraze jedna rovina).
/// Kam sa prevedie lineárne sRGB, ktoré vznikne z XYB.
#[derive(Debug)]
enum CielXyb {
    /// Kódovanie podľa hlavičky: matica z lineárneho sRGB (ak iné základné farby), prenosová funkcia, mierka
    /// lineárneho svetla pred prenosom (PQ: cieľový jas / 10 000 cd/m²).
    Kodovanie { matica: Option<[[f32; 3]; 3]>, prenos: Prenos, mierka: f32 },
    /// Vložený profil ICC (prevod balíkom farba).
    Icc(Box<Prevod>),
}

fn ciel_xyb(m: &hlavicky::Metadata, icc: Option<&[u8]>) -> CielXyb {
    let f = &m.farba;
    let srgb = CielXyb::Kodovanie { matica: None, prenos: Prenos::Srgb, mierka: 1.0 };
    if let Some(d) = icc {
        let linearne_srgb = Priestor::Rgb { primarne: primarne::SRGB, prenos: Prenos::Linearny };
        return Priestor::z_icc(d)
            .and_then(|p| {
                let zamer = match &p {
                    Priestor::Icc(pr) if pr.zamer <= 3 => [
                        Zamer::Percepcny,
                        Zamer::RelativnyKolorimetricky,
                        Zamer::Sytost,
                        Zamer::AbsolutnyKolorimetricky,
                    ][pr.zamer as usize],
                    _ => Zamer::Percepcny,
                };
                Prevod::novy(&linearne_srgb, &p, zamer)
            })
            .map(|p| CielXyb::Icc(Box::new(p)))
            .unwrap_or(srgb);
    }
    let (prenos, mierka) = match (f.gama, f.prenos) {
        (Some(g), _) if g > 0.0 => (Prenos::Gama(1.0 / g), 1.0),
        (_, 1) => (Prenos::Bt709, 1.0),
        (_, 8) => (Prenos::Linearny, 1.0),
        (_, 16) => (Prenos::Pq, m.cielovy_jas / 10_000.0),
        (_, 17) => (Prenos::Gama(2.6), 1.0),
        // HLG potrebuje OOTF podľa jasu displeja – zatiaľ sRGB
        _ => (Prenos::Srgb, 1.0),
    };
    let matica = (f.priestor != 1 && (f.zakladne != 1 || f.biela != 1)).then(|| {
        let [r, g, b] = f.zakladne_xy;
        let ciel = Primarne { r, g, b, biely: f.biela_xy };
        primarne::rgb_na_rgb(&primarne::SRGB, &ciel, Adaptacia::Bradford).map(|r| r.map(|x| x as f32))
    });
    CielXyb::Kodovanie { matica, prenos, mierka }
}

#[allow(clippy::needless_range_loop)]
fn prevod_farieb(mut p: skladanie::Platno, m: &hlavicky::Metadata, ciel: &CielXyb) -> skladanie::Platno {
    if !m.xyb || p.farba.len() != 3 {
        return p;
    }
    let [x, y, b]: [Vec<f32>; 3] = std::mem::take(&mut p.farba).try_into().unwrap_or_else(|_| unreachable!());
    let mut roviny = vardct::xyb_na_rgb([x, y, b], m);
    let sivy = m.farba.priestor == 1;
    let (matica, prenos, mierka) = match ciel {
        CielXyb::Icc(prevod) => {
            // lineárne sRGB → zariadenie profilu (presne, paralelne)
            let n = roviny[0].len();
            let mut v = Vec::with_capacity(3 * n);
            for i in 0..n {
                v.extend([roviny[0][i], roviny[1][i], roviny[2][i]]);
            }
            let kc = prevod.vystupov();
            if let Ok(o) = prevod.preved_f32(&v, 3, kc) {
                p.farba = (0..kc).map(|c| o.iter().skip(c).step_by(kc).copied().collect()).collect();
                return p;
            }
            (&None, &Prenos::Srgb, &1.0)
        }
        CielXyb::Kodovanie { matica, prenos, mierka } => (matica, prenos, mierka),
    };
    if let Some(mt) = matica {
        let n = roviny[0].len();
        for i in 0..n {
            let v = [roviny[0][i], roviny[1][i], roviny[2][i]];
            for (c, r) in mt.iter().enumerate() {
                roviny[c][i] = r[0] * v[0] + r[1] * v[1] + r[2] * v[2];
            }
        }
    }
    // pre celé vzorky stačí tabuľka s lineárnou interpoláciou (chyba < 1e-7)
    const N: usize = 65_536;
    let kod = |x: f32| prenos.z_linearneho((x * *mierka) as f64) as f32;
    let tabulka: Option<Vec<f32>> =
        (!m.hlbka.plavajuca).then(|| (0..=N).map(|i| kod(i as f32 / N as f32)).collect());
    let zakoduj = |x: f32| -> f32 {
        match &tabulka {
            Some(t) => {
                let q = x.clamp(0.0, 1.0) * N as f32;
                let i = (q as usize).min(N - 1);
                t[i] + (t[i + 1] - t[i]) * (q - i as f32)
            }
            None => kod(x),
        }
    };
    p.farba = roviny
        .into_iter()
        .enumerate()
        .filter(|&(i, _)| !sivy || i == 1)
        .map(|(_, r)| r.into_iter().map(zakoduj).collect())
        .collect();
    p
}

/// Vykreslí kanály bodových farieb: `farba = zmes · bodová + (1 − zmes) · farba`, zmes = hodnota · krytie.
fn vykresli_bodove(mut p: skladanie::Platno, m: &hlavicky::Metadata) -> skladanie::Platno {
    if p.farba.len() != 3 {
        return p;
    }
    for (k, d) in p.dodatocne.iter().zip(&m.dodatocne) {
        let Some([r, g, b, krytie]) = d.bodova else { continue };
        for (rovina, farba) in p.farba.iter_mut().zip([r, g, b]) {
            for (v, &s) in rovina.iter_mut().zip(k) {
                let zmes = s * krytie;
                *v = zmes * farba + (1.0 - zmes) * *v;
            }
        }
    }
    p
}

/// Model a alfa obrazu podľa metadát.
fn tvar(m: &hlavicky::Metadata) -> (Model, usize, Option<usize>) {
    let sivy = m.farba.priestor == 1;
    let alfa_i = m.dodatocne.iter().position(|d| d.typ == hlavicky::KANAL_ALFA);
    (if sivy { Model::Siva } else { Model::Rgb }, if sivy { 1 } else { 3 }, alfa_i)
}

/// Celé vzorky Modular → obraz (presne, bez plávajúcej čiarky).
fn vystup_cele(kanaly: Vec<modular::Kanal>, m: &hlavicky::Metadata, w: usize, h: usize) -> Vysledok<Obraz> {
    let (model, farebnych, alfa_i) = tvar(m);
    let k = farebnych + alfa_i.is_some() as usize;
    let hlbka = m.hlbka;
    let px = w * h;
    if kanaly.len() < farebnych {
        return Err(chyba("chýbajú farebné kanály"));
    }
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
        Obraz::novy(w, h, model, alfa_i.is_some(), Vzorky::F32(v))
    } else if hlbka.bitov <= 8 {
        let mut v = vec![0u8; px * k];
        for (c, (d, hb)) in zdroj.iter().zip(&hlbky).enumerate() {
            let max = ((1u64 << hb.bitov) - 1) as i64;
            for i in 0..px {
                let x = (d[i] as i64).clamp(0, max);
                v[i * k + c] = ((x * 255 + max / 2) / max) as u8;
            }
        }
        Obraz::novy(w, h, model, alfa_i.is_some(), Vzorky::U8(v))
    } else {
        let mut v = vec![0u16; px * k];
        for (c, (d, hb)) in zdroj.iter().zip(&hlbky).enumerate() {
            let max = ((1u64 << hb.bitov.min(31)) - 1) as i64;
            for i in 0..px {
                let x = (d[i] as i64).clamp(0, max);
                v[i * k + c] = ((x * 65_535 + max / 2) / max) as u16;
            }
        }
        Obraz::novy(w, h, model, alfa_i.is_some(), Vzorky::U16(v))
    }
}

/// Plátno v prenosovej funkcii súboru (0–1) → obraz.
fn vystup_platno(p: skladanie::Platno, m: &hlavicky::Metadata) -> Vysledok<Obraz> {
    let (model, farebnych, alfa_i) = tvar(m);
    if p.farba.len() != farebnych {
        return Err(chyba("plátno má iný počet farebných rovín"));
    }
    let k = farebnych + alfa_i.is_some() as usize;
    let px = p.w * p.h;
    let alfa = alfa_i.map(|i| &p.dodatocne[i]);
    let mut f = vec![0f32; px * k];
    for i in 0..px {
        for (c, r) in p.farba.iter().enumerate() {
            f[i * k + c] = r[i];
        }
        if let Some(a) = alfa {
            f[i * k + farebnych] = a[i];
        }
    }
    let hlbka = m.hlbka;
    let vzorky = if hlbka.plavajuca {
        Vzorky::F32(f)
    } else if hlbka.bitov <= 8 {
        Vzorky::U8(f.iter().map(|&x| (x.clamp(0.0, 1.0) * 255.0).round() as u8).collect())
    } else {
        Vzorky::U16(f.iter().map(|&x| (x.clamp(0.0, 1.0) * 65_535.0).round() as u16).collect())
    };
    Obraz::novy(p.w, p.h, model, alfa_i.is_some(), vzorky)
}

/// Popis farby z hlavičky (cICP, sRGB, gama).
fn popis_farby(o: &mut Obraz, m: &hlavicky::Metadata, ciel: &CielXyb) {
    let f = &m.farba;
    if f.priestor != 2 && !f.chce_icc {
        let mut zakladne = match (f.zakladne, f.biela) {
            (1, 1) => 1,
            (9, 1) => 9,
            (11, 11) => 11,
            (11, _) => 12,
            _ => 2,
        };
        let mut prenos = match (f.gama, f.prenos) {
            (Some(_), _) => 2,
            (None, p) => p as u8,
        };
        if m.xyb {
            // z XYB sa obraz prevedie do kódovania z hlavičky (základné farby, prenos); HLG zatiaľ do sRGB
            if let CielXyb::Kodovanie { prenos: Prenos::Srgb, .. } = ciel
                && prenos != 13
            {
                zakladne = 1;
                prenos = 13;
            }
        }
        o.farba.cicp = Some(Cicp { zakladne, prenos, matica: 0, plny_rozsah: true });
        if zakladne == 1 && prenos == 13 {
            o.farba.srgb = Some(f.zamer as u8);
        }
        if let Some(g) = f.gama {
            o.farba.gama = Some(g);
        }
        o.farba.linearne = prenos == 8;
    }
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
