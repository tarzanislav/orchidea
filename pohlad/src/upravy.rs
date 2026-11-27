//! Základné úpravy obrázka s históriou (späť / znova): otočenie a zrkadlenie, orezanie, jas, kontrast, gama,
//! kvalitné zmenšenie; uloženie – JPEG s čisto geometrickými úpravami bezstratovo (v oblasti DCT).
//!
//! Úpravy sa robia v súradniciach obrazu postaveného podľa Exif (tak, ako ho vidno). Pri uložení sa obraz
//! vždy uloží postavený a natočenie v Exif sa nastaví na 1. Pôvodný súbor sa neprepisuje.
//!
//! Bodová úprava pracuje s kódovanými hodnotami 0–1 (ako ich vidí oko): v' = (v^(1/γ) − ½) · kontrast + ½ + jas.

use crate::{nahlady, zmensenie};
use orchidea_formaty::jpeg::bezstratovo::{self, Transformacia, Vyrez};
use orchidea_formaty::obraz::{Limity, Obraz, Vzorky};
use orchidea_formaty::{Format, jpeg};
use orchidea_jadro::matematika::mocnina;
use orchidea_jadro::{Chyba, Vysledok};
use std::path::{Path, PathBuf};

/// Jedna úprava.
#[derive(Debug, Clone, PartialEq)]
pub enum Uprava {
    Transformacia(Transformacia),
    /// Orezanie (súradnice obrazu pred touto úpravou).
    Orez {
        x: usize,
        y: usize,
        sirka: usize,
        vyska: usize,
    },
    /// Jas (posun), kontrast (násobok okolo stredu), gama.
    Bodova {
        jas: f32,
        kontrast: f32,
        gama: f32,
    },
    /// Kvalitné zmenšenie (Lanczos v lineárnom svetle) na najdlhšiu stranu.
    Zmensi {
        max: usize,
    },
}

/// História úprav so späť / znova.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Historia {
    kroky: Vec<Uprava>,
    pozicia: usize,
}

impl Historia {
    /// Pridá úpravu (zahodí kroky „znova“).
    pub fn pridaj(&mut self, u: Uprava) {
        self.kroky.truncate(self.pozicia);
        self.kroky.push(u);
        self.pozicia += 1;
    }

    /// Späť; vráti, či sa niečo zmenilo.
    pub fn spat(&mut self) -> bool {
        let z = self.pozicia > 0;
        self.pozicia = self.pozicia.saturating_sub(1);
        z
    }

    /// Znova; vráti, či sa niečo zmenilo.
    pub fn znova(&mut self) -> bool {
        let z = self.pozicia < self.kroky.len();
        self.pozicia = (self.pozicia + 1).min(self.kroky.len());
        z
    }

    /// Dá sa ísť späť?
    pub fn moze_spat(&self) -> bool {
        self.pozicia > 0
    }

    /// Dá sa ísť znova?
    pub fn moze_znova(&self) -> bool {
        self.pozicia < self.kroky.len()
    }

    /// Platné úpravy (po späť / znova).
    pub fn aktivne(&self) -> &[Uprava] {
        &self.kroky[..self.pozicia]
    }

    /// Ak sú platné úpravy len geometrické s nanajvýš jedným orezaním na konci – výsledná transformácia
    /// a výrez (bezstratová cesta pre JPEG).
    pub fn bezstratova(&self) -> Option<(Transformacia, Option<Vyrez>)> {
        let mut t = Transformacia::Ziadna;
        let mut vyrez = None;
        for u in self.aktivne() {
            match u {
                Uprava::Transformacia(x) if vyrez.is_none() => t = t.potom(*x),
                Uprava::Orez { x, y, sirka, vyska } if vyrez.is_none() => {
                    vyrez = Some(Vyrez { x: *x, y: *y, sirka: *sirka, vyska: *vyska })
                }
                _ => return None,
            }
        }
        Some((t, vyrez))
    }
}

/// Geometrická transformácia v bodoch.
pub fn transformuj(o: &Obraz, t: Transformacia) -> Vysledok<Obraz> {
    let n = match t {
        Transformacia::Ziadna => return Ok(o.clone()),
        Transformacia::ZrkadloVodorovne => 2,
        Transformacia::Otoc180 => 3,
        Transformacia::ZrkadloZvisle => 4,
        Transformacia::Transpozicia => 5,
        Transformacia::Otoc90 => 6,
        Transformacia::Transverzia => 7,
        Transformacia::Otoc270 => 8,
    };
    // natočenie Exif n je presne transformácia, ktorá obraz postaví – použije sa tá istá funkcia
    let mut x = o.clone();
    x.metadata.natocenie = n;
    let mut r = nahlady::natoc(&x)?;
    r.metadata.natocenie = o.metadata.natocenie;
    Ok(r)
}

/// Orezanie na obdĺžnik (orezaný na obraz).
pub fn orez(o: &Obraz, x: usize, y: usize, sirka: usize, vyska: usize) -> Vysledok<Obraz> {
    if x >= o.sirka || y >= o.vyska || sirka == 0 || vyska == 0 {
        return Err(Chyba::MimoRozsahu("orezanie mimo obrazu".into()));
    }
    let (w, h) = (sirka.min(o.sirka - x), vyska.min(o.vyska - y));
    let k = o.kanaly();
    fn vyber<T: Copy>(d: &[T], sw: usize, k: usize, x: usize, y: usize, w: usize, h: usize) -> Vec<T> {
        (y..y + h).flat_map(|r| d[(r * sw + x) * k..(r * sw + x + w) * k].iter().copied()).collect()
    }
    let vz = match &o.vzorky {
        Vzorky::U8(d) => Vzorky::U8(vyber(d, o.sirka, k, x, y, w, h)),
        Vzorky::U16(d) => Vzorky::U16(vyber(d, o.sirka, k, x, y, w, h)),
        Vzorky::F32(d) => Vzorky::F32(vyber(d, o.sirka, k, x, y, w, h)),
    };
    let mut r = Obraz::novy(w, h, o.model, o.alfa, vz)?;
    r.mena_kanalov = o.mena_kanalov.clone();
    r.bitov = o.bitov;
    r.farba = o.farba.clone();
    r.metadata = o.metadata.clone();
    Ok(r)
}

/// Bodová úprava jasu, kontrastu a gamy (alfa sa nemení).
pub fn bodova(o: &Obraz, jas: f32, kontrast: f32, gama: f32) -> Obraz {
    let f = |v: f32| -> f32 {
        let g = if gama > 0.0 { mocnina(v.max(0.0) as f64, 1.0 / gama as f64) as f32 } else { v };
        (g - 0.5) * kontrast + 0.5 + jas
    };
    let k = o.kanaly();
    let alfa = o.alfa.then_some(k - 1);
    let mut r = o.clone();
    match &mut r.vzorky {
        Vzorky::U8(d) => {
            let t: Vec<u8> =
                (0..256).map(|i| (f(i as f32 / 255.0).clamp(0.0, 1.0) * 255.0).round() as u8).collect();
            for (i, x) in d.iter_mut().enumerate() {
                if Some(i % k) != alfa {
                    *x = t[*x as usize];
                }
            }
        }
        Vzorky::U16(d) => {
            let t: Vec<u16> = (0..65536)
                .map(|i| (f(i as f32 / 65535.0).clamp(0.0, 1.0) * 65535.0).round() as u16)
                .collect();
            for (i, x) in d.iter_mut().enumerate() {
                if Some(i % k) != alfa {
                    *x = t[*x as usize];
                }
            }
        }
        Vzorky::F32(d) => {
            for (i, x) in d.iter_mut().enumerate() {
                if Some(i % k) != alfa {
                    *x = f(*x);
                }
            }
        }
    }
    r
}

/// Použije jednu úpravu.
pub fn aplikuj(o: &Obraz, u: &Uprava) -> Vysledok<Obraz> {
    match u {
        Uprava::Transformacia(t) => transformuj(o, *t),
        Uprava::Orez { x, y, sirka, vyska } => orez(o, *x, *y, *sirka, *vyska),
        Uprava::Bodova { jas, kontrast, gama } => Ok(bodova(o, *jas, *kontrast, *gama)),
        Uprava::Zmensi { max } => {
            let (w, h) = zmensenie::rozmer_do(o.sirka, o.vyska, *max);
            zmensenie::zmensi(o, w, h)
        }
    }
}

/// Použije všetky úpravy po rade.
pub fn aplikuj_vsetky(o: &Obraz, kroky: &[Uprava]) -> Vysledok<Obraz> {
    let mut x = o.clone();
    for u in kroky {
        x = aplikuj(&x, u)?;
    }
    Ok(x)
}

/// Voľná cesta „meno_upravene.prípona“ (nič sa neprepíše).
pub fn cesta_ulozenia(zdroj: &Path, pripona: &str) -> PathBuf {
    let kmen =
        zdroj.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "obrazok".into());
    let priecinok = zdroj.parent().unwrap_or(Path::new("."));
    let mut c = priecinok.join(format!("{kmen}_upravene.{pripona}"));
    let mut n = 2;
    while c.exists() {
        c = priecinok.join(format!("{kmen}_upravene ({n}).{pripona}"));
        n += 1;
    }
    c
}

/// Výsledok uloženia.
#[derive(Debug, Clone, PartialEq)]
pub struct Ulozene {
    pub cesta: PathBuf,
    /// Uložené bezstratovo (JPEG v oblasti DCT, alebo bezstratový formát).
    pub bezstratovo: bool,
}

/// Uloží upravený obraz vedľa pôvodného súboru (`meno_upravene`). JPEG s geometrickými úpravami bezstratovo,
/// inak sa obraz upraví v plnej presnosti a uloží v pôvodnom formáte (JPEG kvalita 92; JPEG XL ako PNG).
pub fn uloz(zdroj: &Path, kroky: &[Uprava], limity: &Limity) -> Vysledok<Ulozene> {
    let data = std::fs::read(zdroj).map_err(|e| Chyba::VstupVystup(format!("{}: {e}", zdroj.display())))?;
    let format =
        orchidea_formaty::rozpoznaj(&data).ok_or_else(|| Chyba::PoskodeneData("neznámy formát".into()))?;
    let mut h = Historia::default();
    for u in kroky {
        h.pridaj(u.clone());
    }
    if format == Format::Jpeg
        && let Some((t, vyrez)) = h.bezstratova()
    {
        let n = orchidea_formaty::citaj(&data, limity)?.metadata.natocenie;
        let t = Transformacia::z_natocenia(n).potom(t);
        // výrez je v súradniciach prehliadača; bezstratový výsledok nemá neúplný okraj vľavo a hore
        let posun = bezstratovo::mriezka(&data, t).map_or((0, 0), |m| m.posun);
        let vyrez = vyrez.and_then(|v| {
            let (x, y) = (v.x.saturating_sub(posun.0), v.y.saturating_sub(posun.1));
            let sirka = v.sirka.saturating_sub(posun.0.saturating_sub(v.x));
            let vyska = v.vyska.saturating_sub(posun.1.saturating_sub(v.y));
            (sirka > 0 && vyska > 0).then_some(Vyrez { x, y, sirka, vyska })
        });
        let d = bezstratovo::uprav(&data, t, vyrez, true, limity)?;
        let cesta = cesta_ulozenia(zdroj, "jpg");
        std::fs::write(&cesta, d).map_err(|e| Chyba::VstupVystup(e.to_string()))?;
        return Ok(Ulozene { cesta, bezstratovo: true });
    }
    let o = orchidea_formaty::citaj(&data, limity)?;
    drop(data);
    let mut o = aplikuj_vsetky(&nahlady::natoc(&o)?, kroky)?;
    o.metadata.natocenie = 1;
    if let Some(e) = o.metadata.exif.as_mut() {
        orchidea_formaty::exif::nastav_natocenie(e, 1);
    }
    let (ciel, pripona) = match format {
        Format::Jxl => (Format::Png, "png"),
        Format::Png => (Format::Png, "png"),
        Format::Tiff => (Format::Tiff, "tif"),
        Format::Exr => (Format::Exr, "exr"),
        Format::Jpeg => (Format::Jpeg, "jpg"),
    };
    let d = match ciel {
        Format::Jpeg => jpeg::zapis(&o, &jpeg::NastavenieJpeg { kvalita: 92, ..Default::default() })?,
        f => orchidea_formaty::zapis(&o, f)?,
    };
    let cesta = cesta_ulozenia(zdroj, pripona);
    std::fs::write(&cesta, d).map_err(|e| Chyba::VstupVystup(e.to_string()))?;
    Ok(Ulozene { cesta, bezstratovo: ciel != Format::Jpeg })
}

#[cfg(test)]
mod skusky {
    use super::*;
    use orchidea_formaty::obraz::Model;

    #[test]
    fn historia_spat_znova() {
        let mut h = Historia::default();
        h.pridaj(Uprava::Transformacia(Transformacia::Otoc90));
        h.pridaj(Uprava::Transformacia(Transformacia::Otoc90));
        assert_eq!(h.bezstratova(), Some((Transformacia::Otoc180, None)));
        assert!(h.spat());
        assert_eq!(h.aktivne().len(), 1);
        h.pridaj(Uprava::Bodova { jas: 0.1, kontrast: 1.0, gama: 1.0 });
        assert!(!h.znova());
        assert_eq!(h.bezstratova(), None);
        assert!(h.spat() && h.spat() && !h.spat());
    }

    /// Otočený a orezaný JPEG s neúplným MCU: bezstratový výsledok ukazuje to isté miesto ako náhľad v prehliadači.
    #[test]
    fn bezstratove_orezanie_po_otoceni_sedi_s_nahladom() {
        use orchidea_formaty::obraz::Model;
        let (w, h) = (100usize, 75usize);
        let mut b = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                b.extend_from_slice(&[
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x * 7 + y * 13) % 256) as u8,
                ]);
            }
        }
        let o = Obraz::novy(w, h, Model::Rgb, false, Vzorky::U8(b)).unwrap();
        let j = jpeg::zapis(&o, &jpeg::NastavenieJpeg { kvalita: 98, ..Default::default() }).unwrap();
        let cas =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let koren = if cfg!(target_family = "wasm") { PathBuf::from(".") } else { std::env::temp_dir() };
        let p = koren.join(format!("upravy_orez_{cas}"));
        std::fs::create_dir_all(&p).unwrap();
        let zdroj = p.join("f.jpg");
        std::fs::write(&zdroj, &j).unwrap();
        let l = Limity::default();
        // otočiť vpravo (75 × 100), orezať od bodu na mriežke (posun 11 + 16, 32)
        let kroky = [
            Uprava::Transformacia(Transformacia::Otoc90),
            Uprava::Orez { x: 27, y: 32, sirka: 40, vyska: 50 },
        ];
        let u = uloz(&zdroj, &kroky, &l).unwrap();
        assert!(u.bezstratovo);
        let bez = orchidea_formaty::citaj(&std::fs::read(&u.cesta).unwrap(), &l).unwrap();
        let nahlad = aplikuj_vsetky(&orchidea_formaty::citaj(&j, &l).unwrap(), &kroky).unwrap();
        assert_eq!((bez.sirka, bez.vyska), (40, 50));
        let (Vzorky::U8(a), Vzorky::U8(c)) = (&bez.vzorky, &nahlad.vzorky) else { panic!() };
        let najviac = a.iter().zip(c).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap();
        assert!(najviac <= 4, "bezstratový výrez sa líši od náhľadu o {najviac}");
        let _ = std::fs::remove_dir_all(&p);
    }

    #[test]
    fn otocenie_a_orezanie() {
        let o = Obraz::novy(3, 2, Model::Siva, false, Vzorky::U8((0..6).collect())).unwrap();
        let r = transformuj(&o, Transformacia::Otoc90).unwrap();
        let Vzorky::U8(v) = &r.vzorky else { panic!() };
        assert_eq!((r.sirka, r.vyska, v.as_slice()), (2, 3, [3u8, 0, 4, 1, 5, 2].as_slice()));
        let c = orez(&o, 1, 0, 2, 2).unwrap();
        let Vzorky::U8(v) = &c.vzorky else { panic!() };
        assert_eq!(v, &[1, 2, 4, 5]);
        let b = bodova(&o, 0.0, 1.0, 1.0);
        assert_eq!(b.vzorky, o.vzorky);
    }
}
