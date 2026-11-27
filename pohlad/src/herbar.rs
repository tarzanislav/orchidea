//! Katalóg fotografií (Herbár) – základ: index zvolených priečinkov bez importu (súbory ostávajú, kde sú),
//! metadáta z Exif, IPTC a XMP, rýchle vyhľadávanie jedným jazykom dotazov.
//!
//! - **Bez importu:** katalóg je jeden textový súbor s údajmi o súboroch (cesta, veľkosť, čas zmeny, rozmery,
//!   dátum snímania, fotoaparát, objektív, expozícia, GPS, kľúčové slová, názov, popis, autor, hodnotenie).
//!   Fotky sa nekopírujú, nič sa do nich nezapisuje. Indexuje sa len to, čo používateľ pridá.
//! - **Chladne:** pri obnove sa súbor s rovnakou veľkosťou a časom zmeny preskočí; čítajú sa len hlavičky
//!   (JPEG, PNG, TIFF bez dekódovania bodov); vlákna sú obmedzené.
//! - **Dotaz:** slová (všetky musia sedieť, bez ohľadu na veľkosť písmen a diakritiku), `-slovo` (nesmie),
//!   `pole:hodnota` (fotoaparat, objektiv, kluc, autor, nazov, popis, format, priecinok, meno), `datum:2024`,
//!   `datum:2024-05`, `datum:2024-05-01..2024-08-31`, čísla `sirka>4000`, `vyska<1000`, `iso>=800`,
//!   `hodnotenie>=4`, `gps:ano`.
//!
//! - **Hodnotenie a kľúčové slová:** trvalý záznam je súbor XMP vedľa fotky `foto.jpg.xmp` (ten istý, aký
//!   používa darktable a digiKam); katalóg je len rýchly zoznam a dá sa z fotiek a súborov XMP kedykoľvek
//!   poskladať znova. Do fotky sa nezapisuje nič. Čítanie (každé ďalšie prekryje predošlé, ak pole obsahuje):
//!   fotka (Exif, IPTC, XMP) → `foto.xmp` (iné programy, len čítanie) → `foto.jpg.xmp`. Zápis len do
//!   `foto.jpg.xmp` – úpravou na mieste (`formaty::xmp::uprav`), cez dočasný súbor a premenovanie.
//!
//! Formát súboru: prvý riadok `ORCHIDEA-HERBAR 2` (prečíta aj 1), riadky `P\t<priečinok>\t<0|1 podpriečinky>`
//! a záznamy `Z\t…` (polia oddelené tabulátorom; `\t`, `\n`, `\\` sa zapisujú so spätnou lomkou).

use orchidea_formaty::obraz::{Limity, Popis};
use orchidea_formaty::{Format, exif, xmp};
use orchidea_jadro::{Chyba, Vysledok, paralel};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Záznam o jednom súbore.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Zaznam {
    pub cesta: PathBuf,
    pub bajtov: u64,
    /// Čas zmeny v ns od 1970.
    pub zmenene: u128,
    pub format: String,
    pub sirka: usize,
    pub vyska: usize,
    pub bitov: u8,
    /// Dátum a čas snímania „RRRR-MM-DD hh:mm:ss“ (Exif DateTimeOriginal, inak IPTC, inak prázdny).
    pub datum: String,
    pub fotoaparat: String,
    pub objektiv: String,
    pub iso: Option<f64>,
    pub clona: Option<f64>,
    /// Čas expozície v sekundách.
    pub expozicia: Option<f64>,
    pub ohnisko: Option<f64>,
    pub gps: Option<(f64, f64)>,
    pub kluce: Vec<String>,
    pub nazov: String,
    pub popis: String,
    pub autor: String,
    /// −1 odmietnutá, 0 bez hodnotenia, 1–5 hviezdičky.
    pub hodnotenie: Option<i8>,
    /// Veľkosť a čas zmeny súborov XMP vedľa fotky (pri obnove sa prečíta znova aj pri zmene XMP).
    pub xmp_stopa: String,
}

impl Zaznam {
    /// Položka zoznamu prehliadača (na náhľady a otvorenie).
    pub fn polozka(&self) -> Option<crate::nahlady::Polozka> {
        let format = match self.format.as_str() {
            "png" => Format::Png,
            "jpeg" => Format::Jpeg,
            "tiff" => Format::Tiff,
            "exr" => Format::Exr,
            "jxl" => Format::Jxl,
            _ => return None,
        };
        let zmenene =
            u64::try_from(self.zmenene).ok().map(|ns| UNIX_EPOCH + std::time::Duration::from_nanos(ns));
        Some(crate::nahlady::Polozka { cesta: self.cesta.clone(), format, bajtov: self.bajtov, zmenene })
    }
}

/// Katalóg.
#[derive(Debug, Clone, Default)]
pub struct Herbar {
    /// Indexované priečinky (cesta, aj podpriečinky).
    pub priecinky: Vec<(PathBuf, bool)>,
    pub zaznamy: Vec<Zaznam>,
    /// Text na vyhľadávanie každého záznamu (malé písmená bez diakritiky).
    texty: Vec<String>,
}

/// Malé písmená bez diakritiky (slovenčina, čeština a bežné latinské písmená).
pub fn normalizuj(t: &str) -> String {
    t.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'á' | 'ä' | 'à' | 'â' | 'ã' | 'å' => 'a',
            'č' | 'ç' | 'ć' => 'c',
            'ď' => 'd',
            'é' | 'ě' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ĺ' | 'ľ' | 'ł' => 'l',
            'ň' | 'ñ' | 'ń' => 'n',
            'ó' | 'ô' | 'ö' | 'ò' | 'õ' | 'ő' => 'o',
            'ŕ' | 'ř' => 'r',
            'š' | 'ś' => 's',
            'ť' => 't',
            'ú' | 'ů' | 'ü' | 'ù' | 'û' | 'ű' => 'u',
            'ý' | 'ÿ' => 'y',
            'ž' | 'ź' | 'ż' => 'z',
            c => c,
        })
        .collect()
}

// ---------------------------------------------------------------- metadáta → záznam

fn z_popisu(cesta: PathBuf, bajtov: u64, zmenene: u128, format: Format, p: &Popis) -> Zaznam {
    let mut z = Zaznam {
        cesta,
        bajtov,
        zmenene,
        format: format!("{format:?}").to_lowercase(),
        sirka: p.sirka,
        vyska: p.vyska,
        bitov: p.bitov,
        ..Default::default()
    };
    // natočenie 5–8: rozmery vymenené (tak, ako fotku vidno)
    if (5..=8).contains(&p.metadata.natocenie) {
        std::mem::swap(&mut z.sirka, &mut z.vyska);
    }
    let mut vyrobca = String::new();
    let mut model = String::new();
    let mut gps = [None::<f64>; 4];
    let mut lat_ref = String::new();
    let mut lon_ref = String::new();
    if let Some(e) = &p.metadata.exif {
        for (skupina, _, znacka, h) in exif::polozky(e) {
            let cislo = || match &h {
                exif::Hodnota::Cisla(c) => c.first().copied(),
                _ => None,
            };
            match (skupina, znacka) {
                ("GPS", 1) => lat_ref = h.text(),
                ("GPS", 3) => lon_ref = h.text(),
                ("GPS", 2) | ("GPS", 4) => {
                    if let exif::Hodnota::Cisla(c) = &h
                        && c.len() >= 3
                    {
                        gps[if znacka == 2 { 0 } else { 1 }] = Some(c[0] + c[1] / 60.0 + c[2] / 3600.0);
                    }
                }
                ("GPS", _) => {}
                (_, 271) => vyrobca = h.text(),
                (_, 272) => model = h.text(),
                (_, 315) if z.autor.is_empty() => z.autor = h.text(),
                (_, 270) if z.popis.is_empty() => z.popis = h.text(),
                (_, 36867) => z.datum = h.text(),
                (_, 306) if z.datum.is_empty() => z.datum = h.text(),
                (_, 33434) => z.expozicia = cislo(),
                (_, 33437) => z.clona = cislo(),
                (_, 34855) => z.iso = cislo(),
                (_, 37386) => z.ohnisko = cislo(),
                (_, 42036) => z.objektiv = h.text(),
                _ => {}
            }
        }
    }
    if let (Some(la), Some(lo)) = (gps[0], gps[1]) {
        let la = if lat_ref.starts_with('S') { -la } else { la };
        let lo = if lon_ref.starts_with('W') { -lo } else { lo };
        z.gps = Some((la, lo));
    }
    // fotoaparát: model často už obsahuje výrobcu
    z.fotoaparat = if model.to_lowercase().starts_with(&vyrobca.to_lowercase()) || vyrobca.is_empty() {
        model
    } else {
        format!("{vyrobca} {model}")
    };
    if let Some(i) = &p.metadata.iptc {
        let mut datum_iptc = String::new();
        for (meno, t) in exif::polozky_iptc(i) {
            match meno.as_str() {
                "Keywords" => z.kluce.push(t),
                "ObjectName" if z.nazov.is_empty() => z.nazov = t,
                "Headline" if z.nazov.is_empty() => z.nazov = t,
                "Caption-Abstract" => z.popis = t,
                "By-line" => z.autor = t,
                "DateCreated" if t.len() == 8 => {
                    datum_iptc = format!("{}:{}:{} 00:00:00", &t[..4], &t[4..6], &t[6..])
                }
                _ => {}
            }
        }
        if z.datum.is_empty() {
            z.datum = datum_iptc;
        }
    }
    if let Some(x) = &p.metadata.xmp
        && let Ok(u) = xmp::citaj(x)
    {
        for k in u.kluce.unwrap_or_default() {
            if !z.kluce.iter().any(|j| normalizuj(j) == normalizuj(&k)) {
                z.kluce.push(k);
            }
        }
        if z.nazov.is_empty() {
            z.nazov = u.nazov.unwrap_or_default();
        }
        if z.popis.is_empty() {
            z.popis = u.popis.unwrap_or_default();
        }
        if z.autor.is_empty() {
            z.autor = u.autor.unwrap_or_default();
        }
        z.hodnotenie = u.hodnotenie;
    }
    // dátum na tvar RRRR-MM-DD hh:mm:ss
    if z.datum.len() >= 10 {
        let b: Vec<char> = z.datum.chars().collect();
        if b.len() >= 10 && b[4] == ':' && b[7] == ':' {
            let mut d: String = b.iter().collect();
            d.replace_range(4..5, "-");
            d.replace_range(7..8, "-");
            z.datum = d;
        }
    }
    z
}

// ---------------------------------------------------------------- súbory XMP vedľa fotky

/// Súbor XMP vedľa fotky, do ktorého sa zapisuje: `foto.jpg.xmp`.
pub fn subor_xmp(foto: &Path) -> PathBuf {
    let mut s = foto.as_os_str().to_owned();
    s.push(".xmp");
    PathBuf::from(s)
}

/// Súbory XMP, ktoré sa čítajú, v poradí od slabšieho: `foto.xmp` (iné programy), `foto.jpg.xmp`.
fn subory_xmp(foto: &Path) -> Vec<PathBuf> {
    let hlavny = subor_xmp(foto);
    let iny = foto.with_extension("xmp");
    if iny == hlavny || iny == foto { vec![hlavny] } else { vec![iny, hlavny] }
}

fn podpis(m: Option<(u64, u128)>) -> String {
    m.map_or_else(|| "-".into(), |(b, t)| format!("{b}:{t}"))
}

fn velkost_a_cas(m: &std::fs::Metadata) -> (u64, u128) {
    (m.len(), m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos()))
}

/// Stopa súborov XMP podľa mapy z prechodu priečinkov (bez ďalších volaní systému).
fn stopa_z_mapy(foto: &Path, mapa: &HashMap<PathBuf, (u64, u128)>) -> String {
    subory_xmp(foto).iter().map(|s| podpis(mapa.get(s).copied())).collect::<Vec<_>>().join(";")
}

/// Stopa súborov XMP podľa aktuálneho stavu na disku.
fn stopa(foto: &Path) -> String {
    subory_xmp(foto)
        .iter()
        .map(|s| podpis(std::fs::metadata(s).ok().filter(|m| m.is_file()).map(|m| velkost_a_cas(&m))))
        .collect::<Vec<_>>()
        .join(";")
}

/// Prekryje hodnotenie, kľúčové slová, názov, popis a autora údajmi zo súborov XMP vedľa fotky (len čítanie).
fn pouzi_xmp_vedla(z: &mut Zaznam) {
    for s in subory_xmp(&z.cesta) {
        let Ok(d) = std::fs::read(&s) else { continue };
        let Ok(u) = xmp::citaj(&d) else { continue };
        if let Some(h) = u.hodnotenie {
            z.hodnotenie = Some(h);
        }
        if let Some(k) = u.kluce {
            z.kluce = k;
        }
        if let Some(t) = u.nazov {
            z.nazov = t;
        }
        if let Some(t) = u.popis {
            z.popis = t;
        }
        if let Some(t) = u.autor {
            z.autor = t;
        }
    }
}

fn precitaj(c: &Path, bajtov: u64, zmenene: u128, xmp_stopa: String, limity: &Limity) -> Option<Zaznam> {
    let d = std::fs::read(c).ok()?;
    let f = orchidea_formaty::rozpoznaj(&d)?;
    let p = orchidea_formaty::citaj_popis(&d, limity).ok()?;
    let mut z = z_popisu(c.to_path_buf(), bajtov, zmenene, f, &p);
    z.xmp_stopa = xmp_stopa;
    pouzi_xmp_vedla(&mut z);
    Some(z)
}

/// Záznam jedného súboru (fotka + súbory XMP vedľa nej), napríklad pre práve zobrazenú fotku.
pub fn zaznam_suboru(c: &Path, limity: &Limity) -> Vysledok<Zaznam> {
    let m = std::fs::metadata(c)?;
    let (bajtov, zmenene) = velkost_a_cas(&m);
    precitaj(c, bajtov, zmenene, stopa(c), limity)
        .ok_or_else(|| Chyba::PoskodeneData(format!("{}: nie je to čitateľný obrázok", c.display())))
}

/// Zmena hodnotenia alebo kľúčových slov jednej fotky.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ZmenaFotky {
    /// −1 odmietnutá, 0 bez hodnotenia, 1–5.
    Hodnotenie(i8),
    PridajKluc(String),
    OdoberKluc(String),
}

/// Zapíše zmenu do súboru XMP vedľa fotky (`foto.jpg.xmp`) a vráti nový záznam fotky.
///
/// Existujúci súbor sa upraví na mieste – polia iných programov ostanú bajt po bajte. Zápis: dočasný súbor
/// v tom istom priečinku → zápis na disk (`sync`) → kontrola, že pôvodný súbor sa medzitým nezmenil →
/// premenovanie na pôvodné meno. Výpadok prúdu tak nechá buď starý, alebo nový celý súbor.
pub fn zapis_xmp(foto: &Path, zmena: &ZmenaFotky, limity: &Limity) -> Vysledok<Zaznam> {
    let hlavny = subor_xmp(foto);
    for _ in 0..3 {
        let povodne = match std::fs::read(&hlavny) {
            Ok(d) => Some(d),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(Chyba::VstupVystup(format!("{}: {e}", hlavny.display()))),
        };
        // aktuálne platné hodnoty (fotka + súbory XMP) – z nich sa počíta nový zoznam kľúčových slov
        let teraz = zaznam_suboru(foto, limity)?;
        let z = match zmena {
            ZmenaFotky::Hodnotenie(h) => xmp::Zmena { hodnotenie: Some(*h), kluce: None },
            ZmenaFotky::PridajKluc(k) => {
                let k = k.trim().to_string();
                let mut v = teraz.kluce.clone();
                if k.is_empty() || v.contains(&k) {
                    return Ok(teraz);
                }
                v.push(k);
                xmp::Zmena { hodnotenie: None, kluce: Some(v) }
            }
            ZmenaFotky::OdoberKluc(k) => {
                let mut v = teraz.kluce.clone();
                v.retain(|x| x != k);
                xmp::Zmena { hodnotenie: None, kluce: Some(v) }
            }
        };
        let nove = match &povodne {
            Some(d) => xmp::uprav(d, &z)?,
            None => xmp::novy(&z)?,
        };
        if povodne.as_deref() == Some(&nove[..]) {
            return Ok(teraz);
        }
        if bezpecne_zapis(&hlavny, &nove, povodne.as_deref())? {
            return zaznam_suboru(foto, limity);
        }
        // súbor medzitým zmenil iný program – znova od začiatku s jeho novým obsahom
    }
    Err(Chyba::VstupVystup(format!("{}: súbor sa stále mení iným programom – nezapísané", hlavny.display())))
}

/// Zapíše cez dočasný súbor a premenovanie; `false`, ak sa cieľ medzitým zmenil (nič sa neprepíše).
fn bezpecne_zapis(ciel: &Path, data: &[u8], povodne: Option<&[u8]>) -> Vysledok<bool> {
    use std::io::Write;
    let mut docasny = ciel.as_os_str().to_owned();
    docasny.push(".orchidea-docasny");
    let docasny = PathBuf::from(docasny);
    let chyba = |e: std::io::Error| Chyba::VstupVystup(format!("{}: {e}", ciel.display()));
    let zapis = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&docasny)?;
        f.write_all(data)?;
        f.sync_all()
    };
    if let Err(e) = zapis() {
        let _ = std::fs::remove_file(&docasny);
        return Err(chyba(e));
    }
    let teraz = match std::fs::read(ciel) {
        Ok(d) => Some(d),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            let _ = std::fs::remove_file(&docasny);
            return Err(chyba(e));
        }
    };
    if teraz.as_deref() != povodne {
        let _ = std::fs::remove_file(&docasny);
        return Ok(false);
    }
    if let Err(e) = std::fs::rename(&docasny, ciel) {
        let _ = std::fs::remove_file(&docasny);
        return Err(chyba(e));
    }
    // aj záznam v priečinku na disk (Unix); vo Windows premenovanie zapisuje systém súborov sám
    #[cfg(unix)]
    if let Some(p) = ciel.parent() {
        let p = if p.as_os_str().is_empty() { Path::new(".") } else { p };
        if let Ok(d) = std::fs::File::open(p) {
            let _ = d.sync_all();
        }
    }
    Ok(true)
}

fn text_hladania(z: &Zaznam) -> String {
    let meno = z.cesta.file_name().map(|m| m.to_string_lossy().into_owned()).unwrap_or_default();
    normalizuj(&format!(
        "{meno} {} {} {} {} {} {}",
        z.fotoaparat,
        z.objektiv,
        z.kluce.join(" "),
        z.nazov,
        z.popis,
        z.autor
    ))
}

// ---------------------------------------------------------------- zápis a čítanie súboru

fn unik(t: &str) -> String {
    t.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

fn odunik(t: &str) -> String {
    let mut s = String::with_capacity(t.len());
    let mut it = t.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => s.push('\t'),
                Some('n') => s.push('\n'),
                Some('r') => s.push('\r'),
                Some(x) => s.push(x),
                None => {}
            }
        } else {
            s.push(c);
        }
    }
    s
}

fn cislo_text(v: Option<f64>) -> String {
    v.map(|x| format!("{x}")).unwrap_or_default()
}

impl Herbar {
    /// Prázdny katalóg.
    pub fn novy() -> Herbar {
        Herbar::default()
    }

    /// Načíta katalóg zo súboru (neexistujúci = prázdny).
    pub fn nacitaj(cesta: &Path) -> Vysledok<Herbar> {
        let t = match std::fs::read_to_string(cesta) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Herbar::novy()),
            Err(e) => return Err(Chyba::VstupVystup(e.to_string())),
        };
        let mut r = t.lines();
        if !matches!(r.next(), Some("ORCHIDEA-HERBAR 1" | "ORCHIDEA-HERBAR 2")) {
            return Err(Chyba::PoskodeneData(
                "katalóg fotografií (Herbár): neznámy formát súboru katalógu".into(),
            ));
        }
        let mut h = Herbar::novy();
        for riadok in r {
            let p: Vec<String> = riadok.split('\t').map(odunik).collect();
            match p.first().map(String::as_str) {
                Some("P") if p.len() >= 3 => h.priecinky.push((PathBuf::from(&p[1]), p[2] == "1")),
                Some("Z") if p.len() >= 22 => {
                    let f = |i: usize| p[i].parse::<f64>().ok();
                    let z = Zaznam {
                        cesta: PathBuf::from(&p[1]),
                        bajtov: p[2].parse().unwrap_or(0),
                        zmenene: p[3].parse().unwrap_or(0),
                        format: p[4].clone(),
                        sirka: p[5].parse().unwrap_or(0),
                        vyska: p[6].parse().unwrap_or(0),
                        bitov: p[7].parse().unwrap_or(0),
                        datum: p[8].clone(),
                        fotoaparat: p[9].clone(),
                        objektiv: p[10].clone(),
                        iso: f(11),
                        clona: f(12),
                        expozicia: f(13),
                        ohnisko: f(14),
                        gps: f(15).zip(f(16)),
                        kluce: if p[17].is_empty() {
                            Vec::new()
                        } else {
                            p[17].split('\u{1F}').map(String::from).collect()
                        },
                        nazov: p[18].clone(),
                        popis: p[19].clone(),
                        autor: p[20].clone(),
                        hodnotenie: p[21].parse().ok(),
                        xmp_stopa: p.get(22).cloned().unwrap_or_default(),
                    };
                    h.texty.push(text_hladania(&z));
                    h.zaznamy.push(z);
                }
                _ => {}
            }
        }
        Ok(h)
    }

    /// Uloží katalóg (cez dočasný súbor – pri výpadku ostane starý).
    pub fn uloz(&self, cesta: &Path) -> Vysledok<()> {
        let mut s = String::from("ORCHIDEA-HERBAR 2\n");
        for (p, rek) in &self.priecinky {
            s += &format!("P\t{}\t{}\n", unik(&p.to_string_lossy()), *rek as u8);
        }
        for z in &self.zaznamy {
            let gps = z.gps.map_or((String::new(), String::new()), |(a, b)| (format!("{a}"), format!("{b}")));
            let polia = [
                unik(&z.cesta.to_string_lossy()),
                z.bajtov.to_string(),
                z.zmenene.to_string(),
                unik(&z.format),
                z.sirka.to_string(),
                z.vyska.to_string(),
                z.bitov.to_string(),
                unik(&z.datum),
                unik(&z.fotoaparat),
                unik(&z.objektiv),
                cislo_text(z.iso),
                cislo_text(z.clona),
                cislo_text(z.expozicia),
                cislo_text(z.ohnisko),
                gps.0,
                gps.1,
                unik(&z.kluce.join("\u{1F}")),
                unik(&z.nazov),
                unik(&z.popis),
                unik(&z.autor),
                z.hodnotenie.map(|h| h.to_string()).unwrap_or_default(),
                unik(&z.xmp_stopa),
            ];
            s += "Z\t";
            s += &polia.join("\t");
            s.push('\n');
        }
        if let Some(p) = cesta.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(p).map_err(|e| Chyba::VstupVystup(e.to_string()))?;
        }
        let docasny = cesta.with_extension("herbar-novy");
        let zapis = || -> std::io::Result<()> {
            use std::io::Write;
            let mut f = std::fs::File::create(&docasny)?;
            f.write_all(s.as_bytes())?;
            f.sync_all()
        };
        zapis().map_err(|e| Chyba::VstupVystup(e.to_string()))?;
        std::fs::rename(&docasny, cesta).map_err(|e| Chyba::VstupVystup(e.to_string()))
    }

    /// Pridá priečinok do indexu (ak tam ešte nie je).
    pub fn pridaj_priecinok(&mut self, p: &Path, podpriecinky: bool) {
        if !self.priecinky.iter().any(|(q, _)| q == p) {
            self.priecinky.push((p.to_path_buf(), podpriecinky));
        }
    }

    /// Obnoví index všetkých priečinkov: nové a zmenené súbory sa prečítajú (len hlavičky), nezmenené
    /// preskočia, zmazané vypadnú. `vlakna` = 0 → polovica jadier. Vráti (prečítaných, preskočených, zmazaných).
    pub fn obnov(&mut self, vlakna: usize, limity: &Limity) -> (usize, usize, usize) {
        let mut subory = Vec::new();
        let mut xmp_mapa = HashMap::new();
        for (p, rek) in self.priecinky.clone() {
            zbieraj(&p, rek, &mut subory, &mut xmp_mapa, 0);
        }
        subory.sort();
        subory.dedup();
        let stare: HashMap<PathBuf, Zaznam> = self.zaznamy.drain(..).map(|z| (z.cesta.clone(), z)).collect();
        let zmazanych = stare.keys().filter(|c| subory.binary_search(c).is_err()).count();
        // (cesta, veľkosť, čas zmeny) – rozhodne sa, čo treba čítať
        let mut nove: Vec<Option<Zaznam>> = Vec::with_capacity(subory.len());
        let mut citat = Vec::new();
        for (i, c) in subory.iter().enumerate() {
            let Ok(m) = std::fs::metadata(c) else {
                nove.push(None);
                continue;
            };
            let zmenene =
                m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            let st = stopa_z_mapy(c, &xmp_mapa);
            match stare.get(c) {
                Some(z) if z.bajtov == m.len() && z.zmenene == zmenene && z.xmp_stopa == st => {
                    nove.push(Some(z.clone()))
                }
                _ => {
                    nove.push(None);
                    citat.push((i, m.len(), zmenene, st));
                }
            }
        }
        let preskocenych = nove.iter().filter(|z| z.is_some()).count();
        // obmedzenie vlákien platí pre celý proces – po obnove sa vráti predošlé
        let predtym = paralel::pocet_vlakien();
        let n = if vlakna == 0 { (predtym / 2).max(1) } else { vlakna };
        paralel::obmedz_vlakna(n);
        let precitane = paralel::mapuj_rozsah(citat.len(), 16, |rozsah| {
            rozsah
                .map(|j| {
                    let (i, bajtov, zmenene, st) = &citat[j];
                    Some((*i, precitaj(&subory[*i], *bajtov, *zmenene, st.clone(), limity)?))
                })
                .collect::<Vec<_>>()
        });
        paralel::obmedz_vlakna(predtym);
        let mut precitanych = 0;
        for (i, z) in precitane.into_iter().flatten().flatten() {
            nove[i] = Some(z);
            precitanych += 1;
        }
        self.zaznamy = nove.into_iter().flatten().collect();
        self.texty = self.zaznamy.iter().map(text_hladania).collect();
        (precitanych, preskocenych, zmazanych)
    }

    /// Poskladá katalóg znova z fotiek a súborov XMP (zabudne všetko uložené okrem zoznamu priečinkov).
    pub fn poskladaj_znova(&mut self, vlakna: usize, limity: &Limity) -> (usize, usize, usize) {
        self.zaznamy.clear();
        self.texty.clear();
        self.obnov(vlakna, limity)
    }

    /// Nahradí záznam fotky (po zápise hodnotenia alebo kľúčových slov); `false`, ak fotka v katalógu nie je.
    pub fn aktualizuj(&mut self, z: Zaznam) -> bool {
        match self.zaznamy.iter().position(|x| x.cesta == z.cesta) {
            Some(i) => {
                self.texty[i] = text_hladania(&z);
                self.zaznamy[i] = z;
                true
            }
            None => false,
        }
    }

    /// Nájde záznamy podľa dotazu (poradie: dátum snímania, potom cesta).
    pub fn hladaj(&self, dotaz: &str) -> Vec<&Zaznam> {
        self.hladaj_indexy(dotaz).into_iter().map(|i| &self.zaznamy[i]).collect()
    }

    /// Ako `hladaj`, ale vráti poradové čísla záznamov.
    pub fn hladaj_indexy(&self, dotaz: &str) -> Vec<usize> {
        let d = Dotaz::rozober(dotaz);
        let mut v: Vec<usize> =
            (0..self.zaznamy.len()).filter(|&i| d.sedi(&self.zaznamy[i], &self.texty[i])).collect();
        v.sort_by(|&a, &b| {
            let (a, b) = (&self.zaznamy[a], &self.zaznamy[b]);
            a.datum.cmp(&b.datum).then_with(|| a.cesta.cmp(&b.cesta))
        });
        v
    }
}

/// Obrázky v priečinku (podľa podpisu prvých bajtov), voliteľne s podpriečinkami; popri tom veľkosť a čas
/// zmeny súborov `.xmp` (bez ďalšieho volania systému – údaje sú v zázname priečinka).
fn zbieraj(p: &Path, rek: bool, v: &mut Vec<PathBuf>, xmp: &mut HashMap<PathBuf, (u64, u128)>, hlbka: usize) {
    let Ok(citac) = std::fs::read_dir(p) else { return };
    for e in citac.flatten() {
        let Ok(typ) = e.file_type() else { continue };
        let c = e.path();
        if typ.is_dir() && rek && hlbka < 64 {
            zbieraj(&c, rek, v, xmp, hlbka + 1);
        } else if typ.is_file() && c.extension().is_some_and(|p| p.eq_ignore_ascii_case("xmp")) {
            if let Ok(m) = e.metadata() {
                xmp.insert(c, velkost_a_cas(&m));
            }
        } else if typ.is_file() {
            let mut h = [0u8; 32];
            let n = std::fs::File::open(&c).and_then(|mut f| f.read(&mut h)).unwrap_or(0);
            if orchidea_formaty::rozpoznaj(&h[..n]).is_some() {
                v.push(c);
            }
        }
    }
}

// ---------------------------------------------------------------- dotaz

#[derive(Debug)]
enum Podmienka {
    Slovo(String),
    Pole(String, String),
    Cislo(String, String, f64),
    Datum(String, String),
}

#[derive(Debug)]
struct Dotaz {
    podmienky: Vec<(bool, Podmienka)>,
}

impl Dotaz {
    fn rozober(t: &str) -> Dotaz {
        let mut podmienky = Vec::new();
        // slová v úvodzovkách ostanú spolu
        let mut casti = Vec::new();
        let mut it = t.split('"');
        let mut v_uvodzovkach = false;
        for kus in it.by_ref() {
            if v_uvodzovkach {
                casti.push(kus.to_string());
            } else {
                casti.extend(kus.split_whitespace().map(String::from));
            }
            v_uvodzovkach = !v_uvodzovkach;
        }
        for c in casti {
            let (zaporne, c) = match c.strip_prefix('-') {
                Some(r) if !r.is_empty() => (true, r.to_string()),
                _ => (false, c),
            };
            let n = normalizuj(&c);
            let porovnanie = [">=", "<=", ">", "<", "="]
                .into_iter()
                .find_map(|op| n.split_once(op).map(|(a, b)| (op, a, b)));
            let p = if let Some((pole, hodnota)) = n.split_once(':') {
                if pole == "datum" {
                    let (od, po) = hodnota.split_once("..").unwrap_or((hodnota, hodnota));
                    Podmienka::Datum(od.to_string(), po.to_string())
                } else {
                    Podmienka::Pole(pole.to_string(), hodnota.to_string())
                }
            } else if let Some((op, pole, hodnota)) = porovnanie
                && let Ok(x) = hodnota.parse::<f64>()
                && !pole.is_empty()
            {
                Podmienka::Cislo(pole.to_string(), op.to_string(), x)
            } else {
                Podmienka::Slovo(n)
            };
            podmienky.push((zaporne, p));
        }
        Dotaz { podmienky }
    }

    fn sedi(&self, z: &Zaznam, text: &str) -> bool {
        self.podmienky.iter().all(|(zaporne, p)| {
            let s = match p {
                Podmienka::Slovo(w) => text.contains(w.as_str()),
                Podmienka::Pole(pole, h) => {
                    let obsahuje = |t: &str| normalizuj(t).contains(h.as_str());
                    match pole.as_str() {
                        "fotoaparat" => obsahuje(&z.fotoaparat),
                        "objektiv" => obsahuje(&z.objektiv),
                        "kluc" => {
                            z.kluce.iter().any(|k| normalizuj(k) == *h || normalizuj(k).contains(h.as_str()))
                        }
                        "autor" => obsahuje(&z.autor),
                        "nazov" => obsahuje(&z.nazov),
                        "popis" => obsahuje(&z.popis),
                        "format" => z.format == *h || (h == "jpg" && z.format == "jpeg"),
                        "priecinok" => z.cesta.parent().is_some_and(|p| obsahuje(&p.to_string_lossy())),
                        "meno" => z.cesta.file_name().is_some_and(|m| obsahuje(&m.to_string_lossy())),
                        "gps" => z.gps.is_some() == matches!(h.as_str(), "ano" | "1" | "yes"),
                        _ => false,
                    }
                }
                Podmienka::Cislo(pole, op, x) => {
                    let v = match pole.as_str() {
                        "sirka" => Some(z.sirka as f64),
                        "vyska" => Some(z.vyska as f64),
                        "iso" => z.iso,
                        "clona" => z.clona,
                        "ohnisko" => z.ohnisko,
                        "hodnotenie" => z.hodnotenie.map(f64::from),
                        "bitov" => Some(z.bitov as f64),
                        "megapixel" | "mpx" => Some((z.sirka * z.vyska) as f64 / 1e6),
                        _ => None,
                    };
                    v.is_some_and(|v| match op.as_str() {
                        ">=" => v >= *x,
                        "<=" => v <= *x,
                        ">" => v > *x,
                        "<" => v < *x,
                        _ => (v - *x).abs() < 1e-9,
                    })
                }
                Podmienka::Datum(od, po) => {
                    !z.datum.is_empty() && z.datum.as_str() >= od.as_str() && {
                        // horná hranica vrátane celého dňa/mesiaca/roka
                        let d = &z.datum[..po.len().min(z.datum.len())];
                        d <= po.as_str()
                    }
                }
            };
            s != *zaporne
        })
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn zaznam(meno: &str, datum: &str, kluce: &[&str], fotoaparat: &str, sirka: usize) -> Zaznam {
        Zaznam {
            cesta: PathBuf::from(format!("/fotky/{meno}")),
            datum: datum.into(),
            kluce: kluce.iter().map(|k| k.to_string()).collect(),
            fotoaparat: fotoaparat.into(),
            sirka,
            vyska: 100,
            format: "jpeg".into(),
            ..Default::default()
        }
    }

    #[test]
    fn dotazy() {
        let mut h = Herbar::novy();
        h.zaznamy = vec![
            zaznam("a.jpg", "2024-05-10 10:00:00", &["Orchidea", "kvet"], "Canon EOS R6", 6000),
            zaznam("b.jpg", "2023-12-24 18:00:00", &["Vianoce"], "Nikon Z6", 4000),
            zaznam("c.jpg", "", &["kvet"], "", 800),
        ];
        h.texty = h.zaznamy.iter().map(text_hladania).collect();
        let mena = |d: &str| {
            h.hladaj(d)
                .iter()
                .map(|z| z.cesta.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(mena("kvet"), ["c.jpg", "a.jpg"]);
        assert_eq!(mena("kvet -orchidea"), ["c.jpg"]);
        assert_eq!(mena("datum:2024"), ["a.jpg"]);
        assert_eq!(mena("datum:2023-12..2024-05"), ["b.jpg", "a.jpg"]);
        assert_eq!(mena("sirka>=4000 fotoaparat:nikon"), ["b.jpg"]);
        assert_eq!(mena("ORCHÍDEA"), ["a.jpg"]);
        assert_eq!(mena("kluc:vianoce"), ["b.jpg"]);
    }

    #[test]
    fn subor_tam_a_spat() {
        let mut h = Herbar::novy();
        h.pridaj_priecinok(Path::new("/fotky\tx"), true);
        let mut z = zaznam("a\tb.jpg", "2024-05-10 10:00:00", &["kvet", "orchidea"], "Canon", 10);
        z.gps = Some((48.1, -17.5));
        z.popis = "riadok 1\nriadok 2 \\ koniec".into();
        z.hodnotenie = Some(4);
        h.zaznamy.push(z.clone());
        // vo WASI nie je dočasný priečinok ani číslo procesu (#18)
        let priecinok = if cfg!(target_family = "wasm") { PathBuf::from(".") } else { std::env::temp_dir() };
        let cas = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let c = priecinok.join(format!("herbar_skuska_{cas}.txt"));
        h.uloz(&c).unwrap();
        let n = Herbar::nacitaj(&c).unwrap();
        let _ = std::fs::remove_file(&c);
        assert_eq!(n.priecinky, h.priecinky);
        assert_eq!(n.zaznamy, vec![z]);
    }

    #[test]
    fn hodnotenie_a_kluce_do_xmp_vedla_fotky() {
        use orchidea_formaty::obraz::{Model, Obraz, Vzorky};
        let koren = if cfg!(target_family = "wasm") { PathBuf::from(".") } else { std::env::temp_dir() };
        let cas = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let p = koren.join(format!("herbar_xmp_{cas}"));
        std::fs::create_dir_all(&p).unwrap();
        let foto = p.join("foto.png");
        let o = Obraz::novy(4, 4, Model::Rgb, false, Vzorky::U8(vec![90; 48])).unwrap();
        let png = orchidea_formaty::zapis(&o, Format::Png).unwrap();
        std::fs::write(&foto, &png).unwrap();
        let l = Limity::default();
        // iný program: foto.xmp (len čítanie)
        let iny =
            xmp::novy(&xmp::Zmena { hodnotenie: Some(2), kluce: Some(vec!["od iného".into()]) }).unwrap();
        std::fs::write(p.join("foto.xmp"), &iny).unwrap();
        let z = zaznam_suboru(&foto, &l).unwrap();
        assert_eq!((z.hodnotenie, z.kluce.clone()), (Some(2), vec!["od iného".to_string()]));
        // zápis: vznikne foto.png.xmp s platnými hodnotami + zmenou; fotka ani foto.xmp sa nezmenia
        let z = zapis_xmp(&foto, &ZmenaFotky::PridajKluc("kvet".into()), &l).unwrap();
        assert_eq!(z.kluce, vec!["od iného".to_string(), "kvet".into()]);
        let z = zapis_xmp(&foto, &ZmenaFotky::Hodnotenie(5), &l).unwrap();
        assert_eq!(z.hodnotenie, Some(5));
        let z = zapis_xmp(&foto, &ZmenaFotky::OdoberKluc("od iného".into()), &l).unwrap();
        assert_eq!((z.hodnotenie, z.kluce.clone()), (Some(5), vec!["kvet".to_string()]));
        assert_eq!(std::fs::read(&foto).unwrap(), png);
        assert_eq!(std::fs::read(p.join("foto.xmp")).unwrap(), iny);
        assert!(!p.join("foto.png.xmp.orchidea-docasny").exists());
        // to isté znova nič nezapíše
        let pred = std::fs::read(subor_xmp(&foto)).unwrap();
        zapis_xmp(&foto, &ZmenaFotky::Hodnotenie(5), &l).unwrap();
        assert_eq!(std::fs::read(subor_xmp(&foto)).unwrap(), pred);
        // katalóg sa dá poskladať z XMP; zmena XMP iným programom sa pri obnove prečíta
        let mut h = Herbar::novy();
        h.pridaj_priecinok(&p, false);
        assert_eq!(h.obnov(1, &l), (1, 0, 0));
        assert_eq!(h.hladaj("hodnotenie>=5 kluc:kvet").len(), 1);
        assert_eq!(h.obnov(1, &l), (0, 1, 0));
        let ine = xmp::uprav(&pred, &xmp::Zmena { hodnotenie: Some(1), kluce: None }).unwrap();
        std::fs::write(subor_xmp(&foto), [&ine[..], b"\n"].concat()).unwrap();
        assert_eq!(h.obnov(1, &l), (1, 0, 0));
        assert_eq!(h.zaznamy[0].hodnotenie, Some(1));
        assert_eq!(h.poskladaj_znova(1, &l), (1, 0, 0));
        let _ = std::fs::remove_dir_all(&p);
    }
}
