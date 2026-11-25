//! Hromadné spracovanie (dávka): prevod formátu, premenovanie podľa šablóny a zmenšenie – pre tisíce súborov
//! bez zamrznutia a chladne.
//!
//! - beží v jednom pracovnom vlákne mimo rozhrania; rozhranie dostáva udalosti cez kanál a môže beh zrušiť,
//! - súbory sa spracujú po jednom – v pamäti je vždy len jeden obraz (pamäť nerastie s počtom súborov),
//! - vnútorná súbežnosť (dekódovanie, zmenšenie) má **obmedzený počet vlákien** (`Nastavenie::vlakna`,
//!   predvolene polovica jadier) a medzi súbormi môže byť prestávka – procesor sa neprehrieva,
//! - JPEG sa pri zmenšení dekóduje rovno zmenšený (1/2, 1/4, 1/8 v oblasti DCT), ak je stále aspoň dvakrát
//!   väčší ako cieľ – menej práce a tepla bez straty kvality,
//! - metadáta (Exif, XMP, IPTC, profil ICC) sa prenesú; existujúci súbor sa neprepíše (pridá sa „ (2)“).

use crate::zmensenie;
use orchidea_formaty::obraz::Limity;
use orchidea_formaty::{Format, jpeg};
use orchidea_jadro::paralel;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// Čo sa má s každým súborom urobiť.
#[derive(Debug, Clone)]
pub struct Recept {
    /// Cieľový formát (`None` = ako vstup).
    pub format: Option<Format>,
    /// Najdlhšia strana výstupu (`None` = bez zmenšenia; nikdy sa nezväčšuje).
    pub max_strana: Option<usize>,
    /// Šablóna mena bez prípony: `{meno}`, `{cislo}`, `{cislo:04}`, `{sirka}`, `{vyska}`, `{datum}` (Exif).
    pub meno: String,
    /// Kvalita JPEG 1–100.
    pub kvalita_jpeg: u8,
}

impl Default for Recept {
    fn default() -> Self {
        Recept { format: None, max_strana: None, meno: "{meno}".into(), kvalita_jpeg: 90 }
    }
}

/// Ako šetrne sa má dávka spracovať.
#[derive(Debug, Clone)]
pub struct Nastavenie {
    /// Najviac vlákien pre vnútornú prácu (0 = polovica jadier, aspoň 1).
    pub vlakna: usize,
    /// Prestávka po každom súbore (chladenie na slabých zariadeniach).
    pub prestavka: Duration,
    /// Limity čítania.
    pub limity: Limity,
}

impl Default for Nastavenie {
    fn default() -> Self {
        Nastavenie { vlakna: 0, prestavka: Duration::ZERO, limity: Limity::default() }
    }
}

impl Nastavenie {
    /// Skutočný počet vlákien.
    pub fn pocet_vlakien(&self) -> usize {
        match self.vlakna {
            0 => (paralel::pocet_vlakien() / 2).max(1),
            n => n,
        }
    }
}

/// Udalosť behu dávky.
#[derive(Debug)]
pub enum Udalost {
    /// Hotový súbor (poradie, vstup, výsledná cesta alebo chyba).
    Subor { poradie: usize, vstup: PathBuf, vysledok: Result<PathBuf, String> },
    /// Koniec (aj po zrušení).
    Koniec { spracovanych: usize, chyb: usize, trvanie: Duration, zrusene: bool },
}

/// Bežiaca dávka.
#[derive(Debug)]
pub struct Beh {
    pub udalosti: Receiver<Udalost>,
    zrus: Arc<AtomicBool>,
    vlakno: Option<std::thread::JoinHandle<()>>,
}

impl Beh {
    /// Požiada o zrušenie (dokončí sa rozrobený súbor).
    pub fn zrus(&self) {
        self.zrus.store(true, Ordering::Relaxed);
    }

    /// Počká na koniec.
    pub fn cakaj(mut self) {
        if let Some(v) = self.vlakno.take() {
            let _ = v.join();
        }
    }
}

/// Spustí dávku v pracovnom vlákne.
pub fn spusti(subory: Vec<PathBuf>, ciel: PathBuf, recept: Recept, nastavenie: Nastavenie) -> Beh {
    let (tx, rx) = channel();
    let zrus = Arc::new(AtomicBool::new(false));
    let z = zrus.clone();
    let vlakno = std::thread::spawn(move || {
        let zaciatok = Instant::now();
        // obmedzenie platí pre celý proces počas dávky, potom sa vráti
        paralel::obmedz_vlakna(nastavenie.pocet_vlakien());
        let (mut spracovanych, mut chyb) = (0, 0);
        for (i, vstup) in subory.into_iter().enumerate() {
            if z.load(Ordering::Relaxed) {
                break;
            }
            let vysledok = spracuj(&vstup, &ciel, i + 1, &recept, &nastavenie.limity);
            spracovanych += 1;
            chyb += vysledok.is_err() as usize;
            if tx.send(Udalost::Subor { poradie: i, vstup, vysledok }).is_err() {
                break;
            }
            if !nastavenie.prestavka.is_zero() {
                std::thread::sleep(nastavenie.prestavka);
            }
        }
        paralel::obmedz_vlakna(0);
        let _ = tx.send(Udalost::Koniec {
            spracovanych,
            chyb,
            trvanie: zaciatok.elapsed(),
            zrusene: z.load(Ordering::Relaxed),
        });
    });
    Beh { udalosti: rx, zrus, vlakno: Some(vlakno) }
}

/// Spracuje jeden súbor; vráti cestu výstupu.
pub fn spracuj(
    vstup: &Path,
    ciel: &Path,
    cislo: usize,
    recept: &Recept,
    limity: &Limity,
) -> Result<PathBuf, String> {
    let data = std::fs::read(vstup).map_err(|e| e.to_string())?;
    let zdroj = orchidea_formaty::rozpoznaj(&data).ok_or("neznámy formát")?;
    let format = recept.format.unwrap_or(zdroj);
    // pôvodné rozmery (zmenšené dekódovanie JPEG ich zaokrúhli nahor)
    let (mut o, povodne) = match (zdroj, recept.max_strana) {
        (Format::Jpeg, Some(m)) => {
            let (w, h) =
                jpeg::citaj_hlavicku(&data).map(|h| (h.sirka, h.vyska)).map_err(|e| e.to_string())?;
            let (cw, ch) = zmensenie::rozmer_do(w, h, m);
            let mierka = [8, 4, 2, 1].into_iter().find(|&k| w / k >= 2 * cw && h / k >= 2 * ch).unwrap_or(1);
            let o = if mierka > 1 {
                jpeg::citaj_zmensene(&data, limity, mierka)
            } else {
                orchidea_formaty::citaj(&data, limity)
            };
            (o.map_err(|e| e.to_string())?, (w, h))
        }
        _ => {
            let o = orchidea_formaty::citaj(&data, limity).map_err(|e| e.to_string())?;
            let r = (o.sirka, o.vyska);
            (o, r)
        }
    };
    drop(data);
    if let Some(m) = recept.max_strana {
        let (cw, ch) = zmensenie::rozmer_do(povodne.0, povodne.1, m);
        if (cw, ch) != (o.sirka, o.vyska) {
            o = zmensenie::zmensi(&o, cw, ch).map_err(|e| e.to_string())?;
        }
    }
    let kmen = vstup.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let meno = meno_podla_sablony(&recept.meno, &kmen, cislo, &o);
    let pripona = match format {
        Format::Png => "png",
        Format::Jpeg => "jpg",
        Format::Tiff => "tif",
        Format::Exr => "exr",
        Format::Jxl => return Err("zápis JPEG XL zatiaľ nie je".into()),
    };
    let bajty = match format {
        Format::Jpeg => {
            let n = jpeg::NastavenieJpeg { kvalita: recept.kvalita_jpeg, ..Default::default() };
            jpeg::zapis(&o, &n)
        }
        f => orchidea_formaty::zapis(&o, f),
    }
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(ciel).map_err(|e| e.to_string())?;
    let cesta = volna_cesta(ciel, &meno, pripona);
    std::fs::write(&cesta, bajty).map_err(|e| e.to_string())?;
    Ok(cesta)
}

fn volna_cesta(priecinok: &Path, meno: &str, pripona: &str) -> PathBuf {
    let mut c = priecinok.join(format!("{meno}.{pripona}"));
    let mut n = 2;
    while c.exists() {
        c = priecinok.join(format!("{meno} ({n}).{pripona}"));
        n += 1;
    }
    c
}

/// Dátum snímania z Exif (RRRR-MM-DD), ak je.
fn datum(o: &orchidea_formaty::Obraz) -> String {
    let Some(e) = &o.metadata.exif else { return String::new() };
    orchidea_formaty::exif::polozky(e)
        .into_iter()
        .find(|p| p.2 == 36867 || p.2 == 306)
        .map(|p| p.3.text().get(..10).unwrap_or("").replace(':', "-"))
        .unwrap_or_default()
}

/// Meno podľa šablóny; znaky, ktoré systémy súborov nedovoľujú, sa nahradia „_“.
pub fn meno_podla_sablony(sablona: &str, kmen: &str, cislo: usize, o: &orchidea_formaty::Obraz) -> String {
    let mut s = String::new();
    let mut zvysok = sablona;
    while let Some(z) = zvysok.find('{') {
        s.push_str(&zvysok[..z]);
        let Some(k) = zvysok[z..].find('}') else {
            s.push_str(&zvysok[z..]);
            zvysok = "";
            break;
        };
        let znacka = &zvysok[z + 1..z + k];
        let (meno, sirka) = znacka.split_once(':').map_or((znacka, 0), |(m, w)| (m, w.parse().unwrap_or(0)));
        match meno {
            "meno" => s.push_str(kmen),
            "cislo" => s.push_str(&format!("{cislo:0sirka$}")),
            "sirka" => s.push_str(&o.sirka.to_string()),
            "vyska" => s.push_str(&o.vyska.to_string()),
            "datum" => s.push_str(&datum(o)),
            _ => s.push_str(&zvysok[z..z + k + 1]),
        }
        zvysok = &zvysok[z + k + 1..];
    }
    s.push_str(zvysok);
    let s: String =
        s.chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "obrazok".into() } else { s }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use orchidea_formaty::obraz::{Model, Obraz, Vzorky};

    #[test]
    fn sablona() {
        let o = Obraz::novy(4, 3, Model::Siva, false, Vzorky::U8(vec![0; 12])).unwrap();
        assert_eq!(meno_podla_sablony("{meno}_{cislo:04}_{sirka}x{vyska}", "IMG 1", 7, &o), "IMG 1_0007_4x3");
        assert_eq!(meno_podla_sablony("a/b:{nic}", "x", 1, &o), "a_b_{nic}");
        assert_eq!(meno_podla_sablony("", "x", 1, &o), "obrazok");
    }
}
