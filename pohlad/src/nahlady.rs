//! Náhľady priečinka bez importu: zoznam obrázkov podľa podpisu (nie podľa prípony), rýchly náhľad každého
//! súboru a ich tvorba na pozadí s obmedzeným počtom vlákien.
//!
//! - nič sa nekopíruje ani neukladá do knižnice – priečinok sa len číta,
//! - JPEG sa dekóduje rovno zmenšený (1/2–1/8 v oblasti DCT), ostatné formáty celé a zmenšia sa Lanczosom,
//! - náhľad je vždy 8-bitové sRGB (s alfou, ak ju obrázok má), natočený podľa Exif; profil ICC sa prevedie
//!   do sRGB balíkom `farba`, lineárne a plávajúce obrazy sa zakódujú krivkou sRGB (orezanie na 0–1),
//! - náhľady na pozadí: jedno pracovné vlákno v zadanom poradí (najprv viditeľné), zrušenie, udalosti.

use crate::zmensenie;
use orchidea_farba::{Prenos, Prevod, Priestor, Zamer};
use orchidea_formaty::obraz::{Limity, Model, Obraz, Vzorky};
use orchidea_formaty::{Format, jpeg};
use orchidea_jadro::{Chyba, Vysledok, paralel};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::SystemTime;

/// Obrázok v priečinku.
#[derive(Debug, Clone, PartialEq)]
pub struct Polozka {
    pub cesta: PathBuf,
    pub format: Format,
    pub bajtov: u64,
    pub zmenene: Option<SystemTime>,
}

/// Obrázky v priečinku (len podľa podpisu prvých bajtov), zoradené podľa mena bez ohľadu na veľkosť písmen
/// a s číslami podľa hodnoty („foto2“ pred „foto10“).
pub fn zoznam(priecinok: &Path) -> Vysledok<Vec<Polozka>> {
    let mut v = Vec::new();
    let citac = std::fs::read_dir(priecinok)
        .map_err(|e| Chyba::VstupVystup(format!("{}: {e}", priecinok.display())))?;
    for e in citac.flatten() {
        let cesta = e.path();
        let Ok(m) = e.metadata() else { continue };
        if !m.is_file() {
            continue;
        }
        let mut hlavicka = [0u8; 32];
        let n = std::fs::File::open(&cesta).and_then(|mut f| f.read(&mut hlavicka)).unwrap_or(0);
        if let Some(format) = orchidea_formaty::rozpoznaj(&hlavicka[..n]) {
            v.push(Polozka { cesta, format, bajtov: m.len(), zmenene: m.modified().ok() });
        }
    }
    v.sort_by(|a, b| prirodzene(&a.cesta.to_string_lossy(), &b.cesta.to_string_lossy()));
    Ok(v)
}

/// Prirodzené poradie mien (čísla podľa hodnoty, písmená bez ohľadu na veľkosť).
fn prirodzene(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let cislo = |i: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = i.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        i.next();
                    }
                    s
                };
                let (s, t) = (cislo(&mut x), cislo(&mut y));
                let (s0, t0) = (s.trim_start_matches('0'), t.trim_start_matches('0'));
                let o = s0.len().cmp(&t0.len()).then(s0.cmp(t0));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(p), Some(q)) => {
                let o = p.to_lowercase().cmp(q.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Otočí obraz podľa natočenia Exif (1–8) do správnej polohy.
pub fn natoc(o: &Obraz) -> Vysledok<Obraz> {
    let n = o.metadata.natocenie;
    if !(2..=8).contains(&n) {
        return Ok(o.clone());
    }
    let (w, h, k) = (o.sirka, o.vyska, o.kanaly());
    let prehod = n >= 5;
    let (nw, nh) = if prehod { (h, w) } else { (w, h) };
    // zdrojový bod pre cieľový (x, y)
    let zdroj = |x: usize, y: usize| -> (usize, usize) {
        match n {
            2 => (w - 1 - x, y),
            3 => (w - 1 - x, h - 1 - y),
            4 => (x, h - 1 - y),
            5 => (y, x),
            6 => (y, h - 1 - x),
            7 => (w - 1 - y, h - 1 - x),
            _ => (w - 1 - y, x),
        }
    };
    fn premiestni<T: Copy + Default>(
        d: &[T],
        nw: usize,
        nh: usize,
        k: usize,
        w: usize,
        z: impl Fn(usize, usize) -> (usize, usize),
    ) -> Vec<T> {
        let mut v = vec![T::default(); nw * nh * k];
        for y in 0..nh {
            for x in 0..nw {
                let (sx, sy) = z(x, y);
                v[(y * nw + x) * k..(y * nw + x + 1) * k]
                    .copy_from_slice(&d[(sy * w + sx) * k..(sy * w + sx + 1) * k]);
            }
        }
        v
    }
    let vzorky = match &o.vzorky {
        Vzorky::U8(d) => Vzorky::U8(premiestni(d, nw, nh, k, w, zdroj)),
        Vzorky::U16(d) => Vzorky::U16(premiestni(d, nw, nh, k, w, zdroj)),
        Vzorky::F32(d) => Vzorky::F32(premiestni(d, nw, nh, k, w, zdroj)),
    };
    let mut r = Obraz::novy(nw, nh, o.model, o.alfa, vzorky)?;
    r.mena_kanalov = o.mena_kanalov.clone();
    r.bitov = o.bitov;
    r.farba = o.farba.clone();
    r.metadata = o.metadata.clone();
    r.metadata.natocenie = 1;
    Ok(r)
}

/// Prevedie obraz na 8-bitové sRGB (sivá ostane sivou, alfa ostane).
fn na_srgb8(o: Obraz) -> Vysledok<Obraz> {
    let k = o.kanaly();
    let farebnych = if o.alfa { k - 1 } else { k };
    // profil ICC → sRGB (tabuľkové aj maticové profily, aj CMYK)
    if let (Some(icc), Vzorky::U8(d)) = (&o.farba.icc, &o.vzorky)
        && let Ok(z) = Priestor::z_icc(icc)
        && z.kanalov() == farebnych
        && let Ok(p) = Prevod::novy(&z, &Priestor::srgb(), Zamer::Percepcny)
        && let Ok(v) = p.preved_u8(d, k, 3 + o.alfa as usize)
    {
        let mut n = Obraz::novy(o.sirka, o.vyska, Model::Rgb, o.alfa, Vzorky::U8(v))?;
        n.metadata = o.metadata;
        return Ok(n);
    }
    let linearne = matches!(o.vzorky, Vzorky::F32(_)) || o.farba.linearne;
    let srgb = Prenos::Srgb;
    let kod = |x: f32| -> u8 {
        let l = x.clamp(0.0, 1.0) as f64;
        ((if linearne { srgb.z_linearneho(l) } else { l }) * 255.0).round() as u8
    };
    let mut v: Vec<u8> = match &o.vzorky {
        Vzorky::U8(d) if !linearne => d.clone(),
        Vzorky::U8(d) => d.iter().map(|&x| kod(x as f32 / 255.0)).collect(),
        Vzorky::U16(d) if !linearne => d.iter().map(|&x| ((x as u32 + 128) / 257) as u8).collect(),
        Vzorky::U16(d) => d.iter().map(|&x| kod(x as f32 / 65535.0)).collect(),
        Vzorky::F32(d) => {
            d.iter()
                .enumerate()
                .map(|(i, &x)| {
                    if o.alfa && i % k == k - 1 { (x.clamp(0.0, 1.0) * 255.0).round() as u8 } else { kod(x) }
                })
                .collect()
        }
    };
    let model = match o.model {
        Model::Cmyk if farebnych == 4 => {
            // CMYK bez profilu: jednoduchý prevod (len pre náhľad)
            let mut r = Vec::with_capacity(o.sirka * o.vyska * (3 + o.alfa as usize));
            for p in v.chunks_exact(k) {
                let kk = 255 - p[3] as u32;
                r.extend((0..3).map(|c| ((255 - p[c] as u32) * kk / 255) as u8));
                if o.alfa {
                    r.push(p[4]);
                }
            }
            v = r;
            Model::Rgb
        }
        Model::Iny | Model::Cmyk => {
            // iné kanály: prvý (alebo prvé tri) ako jas
            let n = if farebnych >= 3 { 3 } else { 1 };
            v = v.chunks_exact(k).flat_map(|p| p[..n].to_vec()).collect();
            let mut r = Obraz::novy(
                o.sirka,
                o.vyska,
                if n == 3 { Model::Rgb } else { Model::Siva },
                false,
                Vzorky::U8(v),
            )?;
            r.metadata = o.metadata;
            return Ok(r);
        }
        m => m,
    };
    let mut r = Obraz::novy(o.sirka, o.vyska, model, o.alfa, Vzorky::U8(v))?;
    r.metadata = o.metadata;
    Ok(r)
}

/// Obraz na zobrazenie: 8-bitové sRGB (s alfou, ak ju má), natočené podľa Exif.
pub fn na_zobrazenie(o: Obraz) -> Vysledok<Obraz> {
    natoc(&na_srgb8(o)?)
}

/// Náhľad súboru: zmestí sa do štvorca `strana` × `strana`, 8-bitové sRGB, natočený podľa Exif.
pub fn nahlad(cesta: &Path, strana: usize, limity: &Limity) -> Vysledok<Obraz> {
    let data = std::fs::read(cesta).map_err(|e| Chyba::VstupVystup(format!("{}: {e}", cesta.display())))?;
    nahlad_z_dat(&data, strana, limity)
}

/// Náhľad z bajtov súboru.
pub fn nahlad_z_dat(data: &[u8], strana: usize, limity: &Limity) -> Vysledok<Obraz> {
    let (o, (w, h)) = match orchidea_formaty::rozpoznaj(data) {
        Some(Format::Jpeg) => {
            let hl = jpeg::citaj_hlavicku(data)?;
            let (cw, ch) = zmensenie::rozmer_do(hl.sirka, hl.vyska, strana);
            // aspoň dvojnásobok cieľa: zmenšenie v DCT priemeruje v kódovaných hodnotách, Lanczos potom v lineárnom svetle
            let mierka = [8, 4, 2, 1]
                .into_iter()
                .find(|&m| hl.sirka / m >= 2 * cw && hl.vyska / m >= 2 * ch)
                .unwrap_or(1);
            (jpeg::citaj_zmensene(data, limity, mierka)?, (hl.sirka, hl.vyska))
        }
        _ => {
            let o = orchidea_formaty::citaj(data, limity)?;
            let r = (o.sirka, o.vyska);
            (o, r)
        }
    };
    let (cw, ch) = zmensenie::rozmer_do(w, h, strana);
    let o = if (cw, ch) != (o.sirka, o.vyska) { zmensenie::zmensi(&o, cw, ch)? } else { o };
    natoc(&na_srgb8(o)?)
}

/// Udalosť tvorby náhľadov.
#[derive(Debug)]
pub enum Udalost {
    /// Náhľad (alebo chyba) položky s daným poradím.
    Nahlad { poradie: usize, vysledok: Result<Box<Obraz>, String> },
    /// Všetko hotové (alebo zrušené).
    Koniec { hotovych: usize },
}

/// Tvorba náhľadov na pozadí.
#[derive(Debug)]
pub struct Beh {
    pub udalosti: Receiver<Udalost>,
    zrus: Arc<AtomicBool>,
    vlakno: Option<std::thread::JoinHandle<()>>,
}

impl Beh {
    pub fn zrus(&self) {
        self.zrus.store(true, Ordering::Relaxed);
    }

    pub fn cakaj(mut self) {
        if let Some(v) = self.vlakno.take() {
            let _ = v.join();
        }
    }
}

/// Spustí tvorbu náhľadov pre `polozky` v danom poradí s najviac `vlakna` vláknami (0 = polovica jadier).
pub fn spusti(polozky: Vec<PathBuf>, strana: usize, vlakna: usize, limity: Limity) -> Beh {
    let (tx, rx) = channel();
    let zrus = Arc::new(AtomicBool::new(false));
    let z = zrus.clone();
    let n = if vlakna == 0 { (paralel::pocet_vlakien() / 2).max(1) } else { vlakna };
    let vlakno = std::thread::spawn(move || {
        paralel::obmedz_vlakna(n);
        let mut hotovych = 0;
        for (i, c) in polozky.iter().enumerate() {
            if z.load(Ordering::Relaxed) {
                break;
            }
            let vysledok = nahlad(c, strana, &limity).map(Box::new).map_err(|e| e.to_string());
            hotovych += 1;
            if tx.send(Udalost::Nahlad { poradie: i, vysledok }).is_err() {
                break;
            }
        }
        paralel::obmedz_vlakna(0);
        let _ = tx.send(Udalost::Koniec { hotovych });
    });
    Beh { udalosti: rx, zrus, vlakno: Some(vlakno) }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn poradie_mien() {
        let mut v = vec!["foto10.jpg", "Foto2.jpg", "foto1.jpg", "a.png"];
        v.sort_by(|a, b| prirodzene(a, b));
        assert_eq!(v, ["a.png", "foto1.jpg", "Foto2.jpg", "foto10.jpg"]);
    }

    #[test]
    fn natocenie_vsetkych_osem() {
        // obraz 3 × 2 s rôznymi hodnotami; natočenie n a potom späť musí sedieť s definíciou Exif
        let d: Vec<u8> = (0..6).collect();
        for n in 1..=8u8 {
            let mut o = Obraz::novy(3, 2, Model::Siva, false, Vzorky::U8(d.clone())).unwrap();
            o.metadata.natocenie = n;
            let r = natoc(&o).unwrap();
            assert_eq!((r.sirka, r.vyska), if n >= 5 { (2, 3) } else { (3, 2) });
            assert_eq!(r.metadata.natocenie, 1);
        }
        // 6 = otočiť o 90° v smere hodinových ručičiek: ľavý dolný roh ide hore doľava
        let mut o = Obraz::novy(3, 2, Model::Siva, false, Vzorky::U8(d)).unwrap();
        o.metadata.natocenie = 6;
        let Vzorky::U8(v) = natoc(&o).unwrap().vzorky else { panic!() };
        assert_eq!(v, [3, 0, 4, 1, 5, 2]);
    }
}
