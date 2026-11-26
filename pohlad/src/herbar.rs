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
//! Formát súboru: prvý riadok `ORCHIDEA-HERBAR 1`, riadky `P\t<priečinok>\t<0|1 podpriečinky>` a záznamy
//! `Z\t…` (polia oddelené tabulátorom; `\t`, `\n`, `\\` sa zapisujú so spätnou lomkou).

use orchidea_formaty::obraz::{Limity, Popis};
use orchidea_formaty::{Format, exif};
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
    pub hodnotenie: Option<u8>,
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

fn hodnota_xml<'a>(xmp: &'a str, znacka: &str) -> Option<&'a str> {
    // atribút  znacka="…"  alebo prvok  <znacka>…</znacka>
    if let Some(i) = xmp.find(&format!("{znacka}=\"")) {
        let z = i + znacka.len() + 2;
        return xmp[z..].find('"').map(|k| &xmp[z..z + k]);
    }
    let otv = format!("<{znacka}");
    let i = xmp.find(&otv)?;
    let z = i + xmp[i..].find('>')? + 1;
    let k = xmp[z..].find(&format!("</{znacka}>"))?;
    Some(&xmp[z..z + k])
}

/// Položky `<rdf:li>` vnútri prvku (alebo text prvku, ak ich nemá).
fn polozky_xml(xmp: &str, znacka: &str) -> Vec<String> {
    let Some(vnutro) = hodnota_xml(xmp, znacka) else { return Vec::new() };
    let mut v = Vec::new();
    let mut zvysok = vnutro;
    while let Some(i) = zvysok.find("<rdf:li") {
        let Some(z) = zvysok[i..].find('>') else { break };
        let z = i + z + 1;
        let Some(k) = zvysok[z..].find("</rdf:li>") else { break };
        v.push(xml_text(&zvysok[z..z + k]));
        zvysok = &zvysok[z + k..];
    }
    if v.is_empty() && !vnutro.contains('<') && !vnutro.trim().is_empty() {
        v.push(xml_text(vnutro));
    }
    v
}

fn xml_text(t: &str) -> String {
    t.trim()
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

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
    if let Some(x) = &p.metadata.xmp {
        let x = String::from_utf8_lossy(x);
        for k in polozky_xml(&x, "dc:subject") {
            if !z.kluce.iter().any(|j| normalizuj(j) == normalizuj(&k)) {
                z.kluce.push(k);
            }
        }
        if z.nazov.is_empty() {
            z.nazov = polozky_xml(&x, "dc:title").into_iter().next().unwrap_or_default();
        }
        if z.popis.is_empty() {
            z.popis = polozky_xml(&x, "dc:description").into_iter().next().unwrap_or_default();
        }
        if z.autor.is_empty() {
            z.autor = polozky_xml(&x, "dc:creator").into_iter().next().unwrap_or_default();
        }
        z.hodnotenie = hodnota_xml(&x, "xmp:Rating")
            .and_then(|r| r.trim().parse::<i32>().ok())
            .map(|r| r.clamp(0, 5) as u8);
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
        if r.next() != Some("ORCHIDEA-HERBAR 1") {
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
        let mut s = String::from("ORCHIDEA-HERBAR 1\n");
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
            ];
            s += "Z\t";
            s += &polia.join("\t");
            s.push('\n');
        }
        if let Some(p) = cesta.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(p).map_err(|e| Chyba::VstupVystup(e.to_string()))?;
        }
        let docasny = cesta.with_extension("herbar-novy");
        std::fs::write(&docasny, s).map_err(|e| Chyba::VstupVystup(e.to_string()))?;
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
        for (p, rek) in self.priecinky.clone() {
            zbieraj(&p, rek, &mut subory, 0);
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
            match stare.get(c) {
                Some(z) if z.bajtov == m.len() && z.zmenene == zmenene => nove.push(Some(z.clone())),
                _ => {
                    nove.push(None);
                    citat.push((i, m.len(), zmenene));
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
                    let (i, bajtov, zmenene) = citat[j];
                    let c = &subory[i];
                    let d = std::fs::read(c).ok()?;
                    let f = orchidea_formaty::rozpoznaj(&d)?;
                    let p = orchidea_formaty::citaj_popis(&d, limity).ok()?;
                    Some((i, z_popisu(c.clone(), bajtov, zmenene, f, &p)))
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

/// Obrázky v priečinku (podľa podpisu prvých bajtov), voliteľne s podpriečinkami.
fn zbieraj(p: &Path, rek: bool, v: &mut Vec<PathBuf>, hlbka: usize) {
    let Ok(citac) = std::fs::read_dir(p) else { return };
    for e in citac.flatten() {
        let Ok(typ) = e.file_type() else { continue };
        let c = e.path();
        if typ.is_dir() && rek && hlbka < 64 {
            zbieraj(&c, rek, v, hlbka + 1);
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
}
