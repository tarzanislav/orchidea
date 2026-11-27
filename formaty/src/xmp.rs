//! XMP – čítanie hodnotenia, kľúčových slov, názvu, popisu a autora a **úprava na mieste**.
//!
//! Podľa Adobe XMP Specification (Part 1: dátový model a serializácia v RDF/XML, Part 3: súbor vedľa fotky),
//! W3C XML 1.0 a Namespaces in XML 1.0. Vlastný malý čítač XML: pamätá si polohu každej značky, atribútu a textu,
//! takže úprava mení **len** hodnotu `xmp:Rating` a položky `rdf:li` v `dc:subject` – všetky ostatné bajty
//! (polia iných programov, poradie, odsadenie, komentáre) ostanú presne také, aké boli.
//!
//! - Menné priestory sa rozlišujú podľa URI, nie podľa predpony (`xap:Rating` je to isté ako `xmp:Rating`).
//! - Hodnotenie môže byť atribút `rdf:Description` aj samostatný prvok; kľúčové slová sú `rdf:Bag` (prijme sa
//!   aj `rdf:Seq` a `rdf:Alt`). Položky, ktoré nie sú obyčajný text, sa nikdy neodstránia.
//! - Ak súboru nerozumie (nie je UTF-8, chýba `rdf:RDF`, je v ňom DOCTYPE, nesedia značky), úprava sa odmietne –
//!   nič sa neprepíše.

use orchidea_jadro::{Chyba, Vysledok};

const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
const NS_DC: &str = "http://purl.org/dc/elements/1.1/";
const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";

/// Najväčší súbor XMP, ktorý sa spracuje.
pub const NAJVIAC_BAJTOV: usize = 64 << 20;
const NAJVIAC_HLBKA: usize = 256;
const NAJVIAC_PRVKOV: usize = 2_000_000;

/// Prázdny súbor XMP vedľa fotky – základ, do ktorého sa vloží hodnotenie a kľúčové slová.
const KOSTRA: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"ORCHIDEA\">\n \
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
<rdf:Description rdf:about=\"\"\n    \
xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n    \
xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n  \
</rdf:Description>\n \
</rdf:RDF>\n\
</x:xmpmeta>\n";

/// Údaje prečítané z XMP (`None` = pole v súbore nie je).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Udaje {
    /// `xmp:Rating`: −1 odmietnutá, 0 bez hodnotenia, 1–5 hviezdičky.
    pub hodnotenie: Option<i8>,
    /// `dc:subject` – kľúčové slová v poradí zo súboru (aj prázdny zoznam znamená „pole je“).
    pub kluce: Option<Vec<String>>,
    /// `dc:title` (x-default alebo prvý jazyk).
    pub nazov: Option<String>,
    /// `dc:description`.
    pub popis: Option<String>,
    /// `dc:creator` (prvý).
    pub autor: Option<String>,
}

/// Čo sa má zmeniť (`None` = nechať tak).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Zmena {
    pub hodnotenie: Option<i8>,
    /// Výsledný zoznam kľúčových slov; existujúce položky ostanú na mieste, chýbajúce sa doplnia na koniec.
    pub kluce: Option<Vec<String>>,
}

// ---------------------------------------------------------------- čítač XML so zapamätanými polohami

type Rozsah = (usize, usize);

#[derive(Debug)]
struct Atribut {
    meno: Rozsah,
    hodnota: Rozsah,
    /// Začiatok medzery pred menom.
    pred: usize,
    /// (URI, miestne meno); `None` – bez menného priestoru alebo deklarácia `xmlns`.
    ns: Option<(String, String)>,
}

#[derive(Debug)]
struct Prvok {
    meno: Rozsah,
    zaciatok: usize,
    koniec_atributov: usize,
    koniec_zaciatku: usize,
    zaciatok_konca: usize,
    koniec: usize,
    prazdny: bool,
    atributy: Vec<Atribut>,
    /// Vlastné deklarácie menných priestorov (predpona, URI); "" = predvolený.
    deklaracie: Vec<(String, String)>,
    rodic: Option<usize>,
    deti: Vec<usize>,
    /// Priamy text (rozsah, CDATA).
    text: Vec<(Rozsah, bool)>,
    ns: (String, String),
}

struct Dokument<'a> {
    s: &'a str,
    prvky: Vec<Prvok>,
}

fn chyba(t: &str) -> Chyba {
    Chyba::PoskodeneData(format!("XMP: {t}"))
}

fn je_medzera(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

fn najdi(b: &[u8], od: usize, co: &[u8]) -> Option<usize> {
    b.get(od..)?.windows(co.len()).position(|w| w == co).map(|i| od + i)
}

/// Rozdelí kvalifikované meno na (predponu, miestne meno).
fn rozdel(meno: &str) -> (&str, &str) {
    meno.split_once(':').unwrap_or(("", meno))
}

impl<'a> Dokument<'a> {
    fn rozober(data: &'a [u8]) -> Vysledok<Dokument<'a>> {
        if data.len() > NAJVIAC_BAJTOV {
            return Err(Chyba::Limit(format!("XMP má {} bajtov, najviac {NAJVIAC_BAJTOV}", data.len())));
        }
        if data.starts_with(&[0xFE, 0xFF]) || data.starts_with(&[0xFF, 0xFE]) {
            return Err(chyba("kódovanie UTF-16 nie je podporované"));
        }
        let s = std::str::from_utf8(data).map_err(|_| chyba("nie je v UTF-8"))?;
        let b = s.as_bytes();
        let n = b.len();
        let mut d = Dokument { s, prvky: Vec::new() };
        let mut zasobnik: Vec<usize> = Vec::new();
        let mut i = if s.starts_with('\u{FEFF}') { 3 } else { 0 };
        while i < n {
            if b[i] != b'<' {
                let k = najdi(b, i, b"<").unwrap_or(n);
                if let Some(&p) = zasobnik.last() {
                    d.prvky[p].text.push(((i, k), false));
                } else if !b[i..k].iter().all(|&c| je_medzera(c) || c == 0) {
                    return Err(chyba("text mimo koreňového prvku"));
                }
                i = k;
                continue;
            }
            let zvysok = &b[i..];
            if zvysok.starts_with(b"<?") {
                i = najdi(b, i + 2, b"?>").ok_or_else(|| chyba("neukončená inštrukcia"))? + 2;
            } else if zvysok.starts_with(b"<!--") {
                i = najdi(b, i + 4, b"-->").ok_or_else(|| chyba("neukončený komentár"))? + 3;
            } else if zvysok.starts_with(b"<![CDATA[") {
                let k = najdi(b, i + 9, b"]]>").ok_or_else(|| chyba("neukončené CDATA"))?;
                let &p = zasobnik.last().ok_or_else(|| chyba("CDATA mimo prvku"))?;
                d.prvky[p].text.push(((i + 9, k), true));
                i = k + 3;
            } else if zvysok.starts_with(b"<!") {
                // DOCTYPE a entity sa nespracúvajú (bezpečnosť) – taký súbor sa neupraví
                return Err(chyba("DOCTYPE nie je podporovaný"));
            } else if zvysok.starts_with(b"</") {
                let z = i + 2;
                let mut k = z;
                while k < n && !je_medzera(b[k]) && b[k] != b'>' {
                    k += 1;
                }
                let meno = &s[z..k];
                while k < n && je_medzera(b[k]) {
                    k += 1;
                }
                if k >= n || b[k] != b'>' {
                    return Err(chyba("neukončená koncová značka"));
                }
                let p = zasobnik.pop().ok_or_else(|| chyba("koncová značka bez začiatku"))?;
                let pr = &mut d.prvky[p];
                if &s[pr.meno.0..pr.meno.1] != meno {
                    return Err(chyba(&format!("koncová značka </{meno}> nesedí")));
                }
                pr.zaciatok_konca = i;
                pr.koniec = k + 1;
                i = k + 1;
            } else {
                i = d.zaciatocna_znacka(i, &mut zasobnik)?;
            }
        }
        if !zasobnik.is_empty() {
            return Err(chyba("neukončený prvok"));
        }
        if d.prvky.is_empty() {
            return Err(chyba("žiadny prvok"));
        }
        Ok(d)
    }

    fn zaciatocna_znacka(&mut self, i: usize, zasobnik: &mut Vec<usize>) -> Vysledok<usize> {
        let (s, b) = (self.s, self.s.as_bytes());
        let n = b.len();
        let meno_od = i + 1;
        let mut k = meno_od;
        while k < n && !je_medzera(b[k]) && !matches!(b[k], b'>' | b'/' | b'<' | b'=') {
            k += 1;
        }
        if k == meno_od {
            return Err(chyba("prázdne meno značky"));
        }
        let meno = (meno_od, k);
        let mut atributy = Vec::new();
        let (koniec_atributov, koniec_zaciatku, prazdny) = loop {
            let pred = k;
            while k < n && je_medzera(b[k]) {
                k += 1;
            }
            if k >= n {
                return Err(chyba("neukončená značka"));
            }
            match b[k] {
                b'>' => break (k, k + 1, false),
                b'/' if b.get(k + 1) == Some(&b'>') => break (k, k + 2, true),
                b'/' | b'<' | b'=' => return Err(chyba("chybná značka")),
                _ => {}
            }
            if k == pred {
                return Err(chyba("atribúty bez medzery"));
            }
            let a_od = k;
            while k < n && !je_medzera(b[k]) && !matches!(b[k], b'=' | b'>' | b'/' | b'<') {
                k += 1;
            }
            let a_meno = (a_od, k);
            while k < n && je_medzera(b[k]) {
                k += 1;
            }
            if k >= n || b[k] != b'=' {
                return Err(chyba("atribút bez hodnoty"));
            }
            k += 1;
            while k < n && je_medzera(b[k]) {
                k += 1;
            }
            let uvodzovka = *b.get(k).ok_or_else(|| chyba("neukončený atribút"))?;
            if uvodzovka != b'"' && uvodzovka != b'\'' {
                return Err(chyba("hodnota atribútu bez úvodzoviek"));
            }
            let h_od = k + 1;
            let h_po = najdi(b, h_od, &[uvodzovka]).ok_or_else(|| chyba("neukončená hodnota atribútu"))?;
            if b[h_od..h_po].contains(&b'<') {
                return Err(chyba("znak < v hodnote atribútu"));
            }
            atributy.push(Atribut { meno: a_meno, hodnota: (h_od, h_po), pred, ns: None });
            if atributy.len() > 100_000 {
                return Err(Chyba::Limit("XMP: priveľa atribútov".into()));
            }
            k = h_po + 1;
        };
        if zasobnik.len() >= NAJVIAC_HLBKA {
            return Err(Chyba::Limit("XMP: priveľké vnorenie".into()));
        }
        if self.prvky.len() >= NAJVIAC_PRVKOV {
            return Err(Chyba::Limit("XMP: priveľa prvkov".into()));
        }
        let rodic = zasobnik.last().copied();
        if rodic.is_none() && self.prvky.iter().any(|p| p.rodic.is_none()) {
            return Err(chyba("viac koreňových prvkov"));
        }
        // deklarácie menných priestorov
        let mut deklaracie = Vec::new();
        for a in &atributy {
            let m = &s[a.meno.0..a.meno.1];
            if m == "xmlns" {
                deklaracie.push((String::new(), odescapuj(&s[a.hodnota.0..a.hodnota.1])));
            } else if let Some(p) = m.strip_prefix("xmlns:") {
                deklaracie.push((p.to_string(), odescapuj(&s[a.hodnota.0..a.hodnota.1])));
            }
        }
        let index = self.prvky.len();
        self.prvky.push(Prvok {
            meno,
            zaciatok: i,
            koniec_atributov,
            koniec_zaciatku,
            zaciatok_konca: koniec_zaciatku,
            koniec: koniec_zaciatku,
            prazdny,
            atributy,
            deklaracie,
            rodic,
            deti: Vec::new(),
            text: Vec::new(),
            ns: (String::new(), String::new()),
        });
        // mená v menných priestoroch (po zapísaní vlastných deklarácií)
        let (pp, miestne) = rozdel(&s[meno.0..meno.1]);
        let uri = self.uri(index, pp).ok_or_else(|| chyba(&format!("neznáma predpona {pp}")))?;
        self.prvky[index].ns = (uri, miestne.to_string());
        for j in 0..self.prvky[index].atributy.len() {
            let a = &self.prvky[index].atributy[j];
            let m = &s[a.meno.0..a.meno.1];
            if m == "xmlns" || m.starts_with("xmlns:") {
                continue;
            }
            let ns = match rozdel(m) {
                ("", _) => None,
                (p, l) => Some((
                    self.uri(index, p).ok_or_else(|| chyba(&format!("neznáma predpona {p}")))?,
                    l.to_string(),
                )),
            };
            self.prvky[index].atributy[j].ns = ns;
        }
        if let Some(r) = rodic {
            self.prvky[r].deti.push(index);
        }
        if !prazdny {
            zasobnik.push(index);
        }
        Ok(koniec_zaciatku)
    }

    /// URI predpony platnej v prvku `p` ("" = predvolený menný priestor).
    fn uri(&self, mut p: usize, predpona: &str) -> Option<String> {
        if predpona == "xml" {
            return Some(NS_XML.to_string());
        }
        loop {
            if let Some((_, u)) = self.prvky[p].deklaracie.iter().rev().find(|(q, _)| q == predpona) {
                return Some(u.clone());
            }
            match self.prvky[p].rodic {
                Some(r) => p = r,
                None => return if predpona.is_empty() { Some(String::new()) } else { None },
            }
        }
    }

    /// Predpona, ktorá v prvku `p` znamená menný priestor `uri` (a nie je prekrytá).
    fn predpona(&self, p: usize, uri: &str) -> Option<String> {
        let mut q = Some(p);
        while let Some(i) = q {
            for (pp, u) in self.prvky[i].deklaracie.iter().rev() {
                if u == uri && !pp.is_empty() && self.uri(p, pp).as_deref() == Some(uri) {
                    return Some(pp.clone());
                }
            }
            q = self.prvky[i].rodic;
        }
        None
    }

    fn je(&self, p: usize, uri: &str, miestne: &str) -> bool {
        self.prvky[p].ns.0 == uri && self.prvky[p].ns.1 == miestne
    }

    fn meno(&self, p: usize) -> &str {
        let r = self.prvky[p].meno;
        &self.s[r.0..r.1]
    }

    /// Text prvku, ak nemá vnorené prvky.
    fn text(&self, p: usize) -> Option<String> {
        let pr = &self.prvky[p];
        if !pr.deti.is_empty() {
            return None;
        }
        let mut t = String::new();
        for &((a, b), cdata) in &pr.text {
            if cdata {
                t.push_str(&self.s[a..b]);
            } else {
                t.push_str(&odescapuj(&self.s[a..b]));
            }
        }
        Some(t)
    }

    /// Prvky `rdf:Description` vo všetkých `rdf:RDF`.
    fn opisy(&self) -> Vec<usize> {
        let mut v = Vec::new();
        for (i, _) in self.prvky.iter().enumerate().filter(|(i, _)| self.je(*i, NS_RDF, "RDF")) {
            v.extend(self.prvky[i].deti.iter().copied().filter(|&d| self.je(d, NS_RDF, "Description")));
        }
        v
    }

    fn rdf(&self) -> Option<usize> {
        (0..self.prvky.len()).find(|&i| self.je(i, NS_RDF, "RDF"))
    }

    /// Výskyty vlastnosti (atribút opisu alebo prvok – dieťa opisu).
    fn vlastnost(&self, uri: &str, miestne: &str) -> Vec<Vyskyt> {
        let mut v = Vec::new();
        for d in self.opisy() {
            for (j, a) in self.prvky[d].atributy.iter().enumerate() {
                if a.ns.as_ref().is_some_and(|(u, l)| u == uri && l == miestne) {
                    v.push(Vyskyt::Atribut(d, j));
                }
            }
            for &c in &self.prvky[d].deti {
                if self.je(c, uri, miestne) {
                    v.push(Vyskyt::Prvok(c));
                }
            }
        }
        v
    }

    fn hodnota(&self, v: &Vyskyt) -> Option<String> {
        match *v {
            Vyskyt::Atribut(d, j) => {
                let h = self.prvky[d].atributy[j].hodnota;
                Some(odescapuj(&self.s[h.0..h.1]))
            }
            Vyskyt::Prvok(p) => self.text(p),
        }
    }

    /// Kontajner `rdf:Bag`/`Seq`/`Alt` v prvku vlastnosti.
    fn kontajner(&self, p: usize) -> Option<usize> {
        self.prvky[p]
            .deti
            .iter()
            .copied()
            .find(|&c| self.je(c, NS_RDF, "Bag") || self.je(c, NS_RDF, "Seq") || self.je(c, NS_RDF, "Alt"))
    }

    /// Položky kontajnera: (prvok `rdf:li`, text – `None`, ak nie je obyčajný text).
    fn polozky(&self, k: usize) -> Vec<(usize, Option<String>)> {
        self.prvky[k]
            .deti
            .iter()
            .copied()
            .filter(|&c| self.je(c, NS_RDF, "li"))
            .map(|c| (c, self.text(c)))
            .collect()
    }

    fn zoznam(&self, uri: &str, miestne: &str) -> Option<Vec<String>> {
        let v = self.vlastnost(uri, miestne);
        let p = v.iter().find_map(|v| match v {
            Vyskyt::Prvok(p) => Some(*p),
            _ => None,
        })?;
        match self.kontajner(p) {
            Some(k) => Some(self.polozky(k).into_iter().filter_map(|(_, t)| t).collect()),
            None => Some(self.text(p).filter(|t| !t.trim().is_empty()).into_iter().collect()),
        }
    }

    /// Prvá položka (pri `rdf:Alt` jazyk x-default).
    fn prva(&self, uri: &str, miestne: &str) -> Option<String> {
        let v = self.vlastnost(uri, miestne);
        let vyskyt = v.first()?;
        let p = match vyskyt {
            Vyskyt::Atribut(..) => return self.hodnota(vyskyt),
            Vyskyt::Prvok(p) => *p,
        };
        let Some(k) = self.kontajner(p) else { return self.text(p) };
        let polozky = self.polozky(k);
        let jazyk = |c: usize| {
            self.prvky[c].atributy.iter().find_map(|a| {
                (a.ns.as_ref().is_some_and(|(u, l)| u == NS_XML && l == "lang"))
                    .then(|| self.s[a.hodnota.0..a.hodnota.1].to_string())
            })
        };
        polozky
            .iter()
            .find(|(c, t)| t.is_some() && jazyk(*c).as_deref() == Some("x-default"))
            .or_else(|| polozky.iter().find(|(_, t)| t.is_some()))
            .and_then(|(_, t)| t.clone())
    }

    /// Medzera tesne pred polohou `p` (len medzery, tabulátory a konce riadkov).
    fn medzera_pred(&self, p: usize) -> &str {
        let b = self.s.as_bytes();
        let mut z = p;
        while z > 0 && je_medzera(b[z - 1]) {
            z -= 1;
        }
        &self.s[z..p]
    }

    /// Odsadenie riadku, na ktorom začína prvok (koniec riadku + medzery pred ním).
    fn odsadenie(&self, p: usize) -> String {
        let m = self.medzera_pred(self.prvky[p].zaciatok);
        // bez konca riadku (všetko v jednom riadku) sa nič neodsadzuje
        m.rfind('\n').map(|i| m[i..].to_string()).unwrap_or_default()
    }

    /// Jednotka odsadenia (rozdiel odsadenia dieťaťa a rodiča), inak jedna medzera.
    fn jednotka(&self, p: usize) -> String {
        let rodic = self.odsadenie(p);
        if let Some(&c) = self.prvky[p].deti.first() {
            let dieta = self.odsadenie(c);
            if !rodic.is_empty() && dieta.len() > rodic.len() && dieta.starts_with(&rodic) {
                return dieta[rodic.len()..].to_string();
            }
            if dieta.is_empty() {
                return String::new();
            }
        }
        if rodic.is_empty() { String::new() } else { " ".to_string() }
    }
}

enum Vyskyt {
    Atribut(usize, usize),
    Prvok(usize),
}

/// Prevedie odkazy na znaky a päť preddefinovaných entít; neznáme ostanú, ako sú.
pub fn odescapuj(t: &str) -> String {
    if !t.contains('&') {
        return t.to_string();
    }
    let mut v = String::with_capacity(t.len());
    let mut zvysok = t;
    while let Some(i) = zvysok.find('&') {
        v.push_str(&zvysok[..i]);
        zvysok = &zvysok[i..];
        let Some(k) = zvysok[..zvysok.len().min(12)].find(';') else {
            v.push('&');
            zvysok = &zvysok[1..];
            continue;
        };
        let e = &zvysok[1..k];
        let znak = match e {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if e.starts_with("#x") || e.starts_with("#X") => {
                u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32)
            }
            _ if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match znak {
            Some(c) => {
                v.push(c);
                zvysok = &zvysok[k + 1..];
            }
            None => {
                v.push('&');
                zvysok = &zvysok[1..];
            }
        }
    }
    v.push_str(zvysok);
    v
}

/// Text do obsahu prvku.
fn escapuj(t: &str) -> String {
    let mut v = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '&' => v.push_str("&amp;"),
            '<' => v.push_str("&lt;"),
            '>' => v.push_str("&gt;"),
            '\r' => v.push_str("&#xD;"),
            c => v.push(c),
        }
    }
    v
}

fn hodnotenie_z_textu(t: &str) -> Option<i8> {
    let x: f64 = t.trim().parse().ok()?;
    x.is_finite().then(|| x.round().clamp(-1.0, 5.0) as i8)
}

/// Prečíta údaje z XMP (vložené vo fotke alebo súbor vedľa fotky).
pub fn citaj(data: &[u8]) -> Vysledok<Udaje> {
    let d = Dokument::rozober(data)?;
    d.rdf().ok_or_else(|| chyba("chýba rdf:RDF"))?;
    let hodnotenie =
        d.vlastnost(NS_XMP, "Rating").iter().find_map(|v| d.hodnota(v).and_then(|t| hodnotenie_z_textu(&t)));
    Ok(Udaje {
        hodnotenie,
        kluce: d.zoznam(NS_DC, "subject"),
        nazov: d.prva(NS_DC, "title"),
        popis: d.prva(NS_DC, "description"),
        autor: d.prva(NS_DC, "creator"),
    })
}

/// Nový súbor XMP vedľa fotky s hodnotením a kľúčovými slovami.
pub fn novy(z: &Zmena) -> Vysledok<Vec<u8>> {
    uprav(KOSTRA.as_bytes(), z)
}

struct Uprava {
    od: usize,
    po: usize,
    text: String,
}

/// Upraví XMP na mieste: zmení len hodnotenie a kľúčové slová, ostatné bajty ostanú presne také, aké boli.
/// Výsledok sa pred vrátením znova prečíta a porovná so zmenou.
pub fn uprav(data: &[u8], z: &Zmena) -> Vysledok<Vec<u8>> {
    if let Some(h) = z.hodnotenie
        && !(-1..=5).contains(&h)
    {
        return Err(Chyba::MimoRozsahu(format!("hodnotenie {h} (dovolené −1 až 5)")));
    }
    if let Some(k) = &z.kluce
        && let Some(zly) = k.iter().find(|k| k.chars().any(|c| c.is_control() && c != '\t'))
    {
        return Err(Chyba::MimoRozsahu(format!("kľúčové slovo s riadiacim znakom: {zly:?}")));
    }
    let d = Dokument::rozober(data)?;
    let rdf = d.rdf().ok_or_else(|| chyba("chýba rdf:RDF – súbor sa neupraví"))?;
    let mut upravy: Vec<Uprava> = Vec::new();
    // vloženia do začiatočnej značky prvého opisu: deklarácie a atribúty
    let mut do_opisu: Vec<(String, String)> = Vec::new();
    let mut deti_opisu: Vec<String> = Vec::new();
    let opis = d.opisy().first().copied();

    // ---- hodnotenie
    if let Some(h) = z.hodnotenie {
        let ciel = h.to_string();
        let vyskyty = d.vlastnost(NS_XMP, "Rating");
        for v in &vyskyty {
            let teraz = d.hodnota(v).and_then(|t| hodnotenie_z_textu(&t));
            if teraz == Some(h) {
                continue;
            }
            match *v {
                Vyskyt::Atribut(o, j) => {
                    let r = d.prvky[o].atributy[j].hodnota;
                    upravy.push(Uprava { od: r.0, po: r.1, text: ciel.clone() });
                }
                Vyskyt::Prvok(p) => {
                    let pr = &d.prvky[p];
                    if pr.prazdny || !pr.deti.is_empty() {
                        let m = d.meno(p);
                        upravy.push(Uprava {
                            od: pr.zaciatok,
                            po: pr.koniec,
                            text: format!("<{m}>{ciel}</{m}>"),
                        });
                    } else {
                        upravy.push(Uprava {
                            od: pr.koniec_zaciatku,
                            po: pr.zaciatok_konca,
                            text: ciel.clone(),
                        });
                    }
                }
            }
        }
        if vyskyty.is_empty() {
            let p = match opis.and_then(|o| d.predpona(o, NS_XMP)) {
                Some(p) => p,
                None => {
                    let p = volna_predpona(&d, opis, "xmp");
                    do_opisu.push((format!("xmlns:{p}"), NS_XMP.to_string()));
                    p
                }
            };
            do_opisu.push((format!("{p}:Rating"), ciel));
        }
    }

    // ---- kľúčové slová
    if let Some(ciel) = &z.kluce {
        let vyskyty: Vec<usize> = d
            .vlastnost(NS_DC, "subject")
            .into_iter()
            .filter_map(|v| match v {
                Vyskyt::Prvok(p) => Some(p),
                Vyskyt::Atribut(..) => None,
            })
            .collect();
        for &s in &vyskyty {
            uprav_zoznam(&d, s, ciel, &mut upravy);
        }
        if vyskyty.is_empty() {
            let dc = match opis.and_then(|o| d.predpona(o, NS_DC)) {
                Some(p) => p,
                None => {
                    let p = volna_predpona(&d, opis, "dc");
                    do_opisu.push((format!("xmlns:{p}"), NS_DC.to_string()));
                    p
                }
            };
            deti_opisu.push(dc);
        }
    }

    // ---- vloženie do opisu (alebo nový opis)
    if !do_opisu.is_empty() || !deti_opisu.is_empty() {
        match opis {
            Some(o) => {
                let pr = &d.prvky[o];
                if !do_opisu.is_empty() {
                    // rovnaká medzera ako pred posledným atribútom (darktable: nový riadok s odsadením)
                    let medzera = pr
                        .atributy
                        .last()
                        .map(|a| d.s[a.pred..a.meno.0].to_string())
                        .unwrap_or_else(|| " ".into());
                    let text: String = do_opisu
                        .iter()
                        .map(|(m, h)| format!("{medzera}{m}=\"{}\"", escapuj_atribut(h)))
                        .collect();
                    upravy.push(Uprava { od: pr.koniec_atributov, po: pr.koniec_atributov, text });
                }
                if let Some(dc) = deti_opisu.first() {
                    let rdfp = rozdel(d.meno(o)).0.to_string();
                    let ciel = z.kluce.as_deref().unwrap_or(&[]);
                    vloz_dieta(&d, o, &mut upravy, |odsadenie, jednotka| {
                        prvok_zoznamu(dc, &rdfp, ciel, odsadenie, jednotka)
                    });
                }
            }
            None => {
                // žiadny rdf:Description – vloží sa nový do rdf:RDF
                let rdfp = rozdel(d.meno(rdf)).0.to_string();
                let rdfp = if rdfp.is_empty() { String::new() } else { format!("{rdfp}:") };
                let ciel = z.kluce.clone().unwrap_or_default();
                let atributy: String =
                    do_opisu.iter().map(|(m, h)| format!(" {m}=\"{}\"", escapuj_atribut(h))).collect();
                let dc = deti_opisu.first().cloned();
                let rdfp2 = rdfp.trim_end_matches(':').to_string();
                vloz_dieta(&d, rdf, &mut upravy, |odsadenie, jednotka| {
                    let mut t = format!("<{rdfp}Description {rdfp}about=\"\"{atributy}");
                    match &dc {
                        Some(dc) => {
                            let vnutro = format!("{odsadenie}{jednotka}");
                            t += ">";
                            t += &vnutro;
                            t += &prvok_zoznamu(dc, &rdfp2, &ciel, &vnutro, jednotka);
                            t += &format!("{odsadenie}</{rdfp}Description>");
                        }
                        None => t += "/>",
                    }
                    t
                });
            }
        }
    }

    // ---- poskladanie
    upravy.sort_by_key(|u| (u.od, u.po));
    let mut v = Vec::with_capacity(data.len() + 256);
    let mut poz = 0;
    for u in &upravy {
        if u.od < poz {
            return Err(chyba("úpravy sa prekrývajú"));
        }
        v.extend_from_slice(&data[poz..u.od]);
        v.extend_from_slice(u.text.as_bytes());
        poz = u.po;
    }
    v.extend_from_slice(&data[poz..]);
    // kontrola: výsledok sa dá prečítať a obsahuje presne to, čo sa malo zapísať
    let u = citaj(&v)?;
    if let Some(h) = z.hodnotenie
        && u.hodnotenie != Some(h)
    {
        return Err(chyba("hodnotenie po úprave nesedí"));
    }
    if let Some(k) = &z.kluce
        && !rovnake_mnoziny(u.kluce.as_deref().unwrap_or(&[]), k)
    {
        return Err(chyba("kľúčové slová po úprave nesedia"));
    }
    Ok(v)
}

fn rovnake_mnoziny(a: &[String], b: &[String]) -> bool {
    a.iter().all(|x| b.contains(x)) && b.iter().all(|x| a.contains(x))
}

fn escapuj_atribut(t: &str) -> String {
    escapuj(t).replace('"', "&quot;")
}

/// Predpona, ktorá v prvku nie je obsadená.
fn volna_predpona(d: &Dokument, p: Option<usize>, zaklad: &str) -> String {
    let obsadena = |q: &str| p.is_some_and(|p| d.uri(p, q).is_some());
    if !obsadena(zaklad) {
        return zaklad.to_string();
    }
    (2..).map(|i| format!("{zaklad}{i}")).find(|q| !obsadena(q)).unwrap_or_default()
}

/// Text prvku `dc:subject` s kontajnerom `rdf:Bag`.
fn prvok_zoznamu(dc: &str, rdfp: &str, kluce: &[String], odsadenie: &str, jednotka: &str) -> String {
    let r = if rdfp.is_empty() { String::new() } else { format!("{rdfp}:") };
    let o1 = format!("{odsadenie}{jednotka}");
    let o2 = format!("{o1}{jednotka}");
    let mut t = format!("<{dc}:subject>{o1}");
    if kluce.is_empty() {
        t += &format!("<{r}Bag/>");
    } else {
        t += &format!("<{r}Bag>");
        for k in kluce {
            t += &format!("{o2}<{r}li>{}</{r}li>", escapuj(k));
        }
        t += &format!("{o1}</{r}Bag>");
    }
    t += &format!("{odsadenie}</{dc}:subject>");
    t
}

/// Vloží nové dieťa na koniec prvku `p` (pri prázdnom prvku `<x/>` ho rozbalí na `<x>…</x>`).
fn vloz_dieta(d: &Dokument, p: usize, upravy: &mut Vec<Uprava>, text: impl Fn(&str, &str) -> String) {
    let pr = &d.prvky[p];
    let odsadenie = d.odsadenie(p);
    let jednotka = d.jednotka(p);
    if pr.prazdny {
        let vnutro = format!("{odsadenie}{jednotka}");
        let t = format!(">{vnutro}{}{odsadenie}</{}>", text(&vnutro, &jednotka), d.meno(p));
        upravy.push(Uprava { od: pr.koniec_atributov, po: pr.koniec_zaciatku, text: t });
    } else if let Some(&posledne) = pr.deti.last() {
        // za posledné dieťa s rovnakým odsadením
        let o = d.odsadenie(posledne);
        upravy.push(Uprava {
            od: d.prvky[posledne].koniec,
            po: d.prvky[posledne].koniec,
            text: format!("{o}{}", text(&o, &jednotka)),
        });
    } else {
        // len medzera pred koncovou značkou – nový riadok pred ňou
        let pred = d.medzera_pred(pr.zaciatok_konca);
        let miesto = pr.zaciatok_konca - pred.len();
        let vnutro = format!("{odsadenie}{jednotka}");
        upravy.push(Uprava { od: miesto, po: miesto, text: format!("{vnutro}{}", text(&vnutro, &jednotka)) });
    }
}

/// Upraví položky existujúceho `dc:subject` podľa cieľového zoznamu (rozdiel: odstránené von, nové na koniec).
fn uprav_zoznam(d: &Dokument, s: usize, ciel: &[String], upravy: &mut Vec<Uprava>) {
    let prs = &d.prvky[s];
    let odsadenie = d.odsadenie(s);
    let jednotka = d.jednotka(s);
    let Some(k) = d.kontajner(s) else {
        // nie je v kontajneri (neštandardné) – prvok sa nahradí celý
        let dc = rozdel(d.meno(s)).0;
        let rdfp = d.predpona(s, NS_RDF).unwrap_or_else(|| "rdf".into());
        let mut t = prvok_zoznamu(dc, &rdfp, ciel, &odsadenie, &jednotka);
        if d.predpona(s, NS_RDF).is_none() {
            t = t.replacen("<rdf:Bag", &format!("<rdf:Bag xmlns:rdf=\"{NS_RDF}\""), 1);
        }
        upravy.push(Uprava { od: prs.zaciatok, po: prs.koniec, text: t });
        return;
    };
    let polozky = d.polozky(k);
    let mut ponechane: Vec<String> = Vec::new();
    for (li, t) in &polozky {
        let ponechat = match t {
            None => true,
            Some(t) => ciel.contains(t) && !ponechane.contains(t),
        };
        if ponechat {
            if let Some(t) = t {
                ponechane.push(t.clone());
            }
        } else {
            let pl = &d.prvky[*li];
            let od = pl.zaciatok - d.medzera_pred(pl.zaciatok).len();
            upravy.push(Uprava { od, po: pl.koniec, text: String::new() });
        }
    }
    let nove: Vec<&String> = ciel.iter().filter(|c| !ponechane.contains(c)).fold(Vec::new(), |mut v, c| {
        if !v.contains(&c) {
            v.push(c);
        }
        v
    });
    if nove.is_empty() {
        return;
    }
    let pk = &d.prvky[k];
    let r = rozdel(d.meno(k)).0;
    let r = if r.is_empty() { String::new() } else { format!("{r}:") };
    let li = |o: &str, c: &str| format!("{o}<{r}li>{}</{r}li>", escapuj(c));
    if pk.prazdny {
        let ok = d.odsadenie(k);
        let o = format!("{ok}{}", d.jednotka(k));
        let t: String = nove.iter().map(|c| li(&o, c)).collect();
        upravy.push(Uprava {
            od: pk.koniec_atributov,
            po: pk.koniec_zaciatku,
            text: format!(">{t}{ok}</{}>", d.meno(k)),
        });
    } else if let Some(&(posledna, _)) = polozky.last() {
        let o = d.odsadenie(polozky[0].0);
        let t: String = nove.iter().map(|c| li(&o, c)).collect();
        let miesto = d.prvky[posledna].koniec;
        upravy.push(Uprava { od: miesto, po: miesto, text: t });
    } else {
        let pred = d.medzera_pred(pk.zaciatok_konca);
        let miesto = pk.zaciatok_konca - pred.len();
        let o = format!("{}{}", d.odsadenie(k), d.jednotka(k));
        let t: String = nove.iter().map(|c| li(&o, c)).collect();
        upravy.push(Uprava { od: miesto, po: miesto, text: t });
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    /// Súbor vedľa fotky v tvare, aký zapisuje darktable (atribúty v jednom opise, história úprav).
    pub const DARKTABLE: &str = include_str!("../tests/data/xmp/darktable.jpg.xmp");

    fn kluce(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn citanie_darktable() {
        let u = citaj(DARKTABLE.as_bytes()).unwrap();
        assert_eq!(u.hodnotenie, Some(1));
        assert_eq!(u.kluce, kluce(&["kvet", "skleník"]));
    }

    #[test]
    fn uprava_meni_len_hodnotenie_a_kluce() {
        let z = Zmena { hodnotenie: Some(4), kluce: kluce(&["skleník", "orchidea", "a & <b> \"c\""]) };
        let v = uprav(DARKTABLE.as_bytes(), &z).unwrap();
        let t = String::from_utf8(v.clone()).unwrap();
        let u = citaj(&v).unwrap();
        assert_eq!(u.hodnotenie, Some(4));
        assert_eq!(u.kluce, kluce(&["skleník", "orchidea", "a & <b> \"c\""]));
        // presne očakávaný výsledok: zmenená hodnota hodnotenia, v dc:subject odstránený „kvet“ a dve nové
        // položky za poslednou; všetko ostatné (aj lr:hierarchicalSubject a história darktable) bajt po bajte
        let ocakavane = DARKTABLE
            .replacen("xmp:Rating=\"1\"", "xmp:Rating=\"4\"", 1)
            .replacen("\n     <rdf:li>kvet</rdf:li>", "", 1)
            .replacen(
                "<rdf:li>skleník</rdf:li>",
                "<rdf:li>skleník</rdf:li>\n     <rdf:li>orchidea</rdf:li>\n     <rdf:li>a &amp; &lt;b&gt; \"c\"</rdf:li>",
                1,
            );
        assert_eq!(t, ocakavane);
        // druhý zápis tej istej zmeny nezmení nič
        assert_eq!(uprav(&v, &z).unwrap(), v);
    }

    #[test]
    fn novy_subor_a_spat() {
        let z = Zmena { hodnotenie: Some(3), kluce: kluce(&["Štedrý večer", "😀", "x'y"]) };
        let v = novy(&z).unwrap();
        let u = citaj(&v).unwrap();
        assert_eq!((u.hodnotenie, u.kluce.clone()), (z.hodnotenie, z.kluce.clone()));
        assert_eq!(uprav(&v, &z).unwrap(), v);
        // len hodnotenie
        let v2 = novy(&Zmena { hodnotenie: Some(0), kluce: None }).unwrap();
        assert_eq!(citaj(&v2).unwrap().kluce, None);
        // prázdny zoznam je „pole je, bez slov“
        let v3 = uprav(&v, &Zmena { hodnotenie: None, kluce: Some(vec![]) }).unwrap();
        assert_eq!(citaj(&v3).unwrap().kluce, Some(vec![]));
        assert_eq!(citaj(&v3).unwrap().hodnotenie, Some(3));
    }

    #[test]
    fn ine_tvary() {
        // hodnotenie ako prvok, iná predpona, prázdny opis, opis bez deklarácií
        let a = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xap="http://ns.adobe.com/xap/1.0/"><xap:Rating>2</xap:Rating></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let v = uprav(a.as_bytes(), &Zmena { hodnotenie: Some(5), kluce: kluce(&["k"]) }).unwrap();
        let t = String::from_utf8(v.clone()).unwrap();
        assert!(t.contains("<xap:Rating>5</xap:Rating>"), "{t}");
        assert!(t.contains("xmlns:dc=\"http://purl.org/dc/elements/1.1/\""), "{t}");
        assert_eq!(citaj(&v).unwrap().kluce, kluce(&["k"]));
        let b = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="urn:iny"/></rdf:RDF>"#;
        let v = uprav(b.as_bytes(), &Zmena { hodnotenie: Some(1), kluce: kluce(&["a", "b"]) }).unwrap();
        let t = String::from_utf8(v.clone()).unwrap();
        assert!(t.contains("xmlns:xmp2=\"http://ns.adobe.com/xap/1.0/\""), "{t}");
        let u = citaj(&v).unwrap();
        assert_eq!((u.hodnotenie, u.kluce), (Some(1), kluce(&["a", "b"])));
        let c = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/>"#;
        let v = uprav(c.as_bytes(), &Zmena { hodnotenie: Some(2), kluce: kluce(&["x"]) }).unwrap();
        let u = citaj(&v).unwrap();
        assert_eq!((u.hodnotenie, u.kluce), (Some(2), kluce(&["x"])));
        // odstránenie všetkých a pridanie do prázdneho Bag
        let d = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:subject><rdf:Bag/></dc:subject></rdf:Description></rdf:RDF>"#;
        let v = uprav(d.as_bytes(), &Zmena { hodnotenie: None, kluce: kluce(&["p", "q"]) }).unwrap();
        assert_eq!(citaj(&v).unwrap().kluce, kluce(&["p", "q"]));
        let v = uprav(&v, &Zmena { hodnotenie: None, kluce: kluce(&[]) }).unwrap();
        assert_eq!(citaj(&v).unwrap().kluce, kluce(&[]));
        // názov v rdf:Alt (x-default), autor v rdf:Seq
        let e = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title><rdf:Alt><rdf:li xml:lang="sk">Kvet</rdf:li><rdf:li xml:lang="x-default">Flower &amp; leaf</rdf:li></rdf:Alt></dc:title><dc:creator><rdf:Seq><rdf:li>Ja</rdf:li></rdf:Seq></dc:creator></rdf:Description></rdf:RDF>"#;
        let u = citaj(e.as_bytes()).unwrap();
        assert_eq!(u.nazov.as_deref(), Some("Flower & leaf"));
        assert_eq!(u.autor.as_deref(), Some("Ja"));
    }

    #[test]
    fn odmietnutie_bez_zapisu() {
        let z = Zmena { hodnotenie: Some(1), kluce: None };
        for zle in [
            &b"<a>"[..],
            b"<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"></rdf:RDX>",
            b"<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY a \"b\">]><x/>",
            b"<x xmlns=\"urn:x\"/>",
            b"\xff\xfe<\0x\0/\0>\0",
            b"<a b=c/>",
            b"<p:a/>",
            b"",
        ] {
            assert!(uprav(zle, &z).is_err(), "{:?}", String::from_utf8_lossy(zle));
        }
        assert!(uprav(DARKTABLE.as_bytes(), &Zmena { hodnotenie: Some(9), kluce: None }).is_err());
        assert!(uprav(DARKTABLE.as_bytes(), &Zmena { hodnotenie: None, kluce: kluce(&["a\nb"]) }).is_err());
    }

    #[test]
    fn entity() {
        assert_eq!(odescapuj("a&lt;b&gt;&amp;&#x10D;&#269;&quot;&apos;&nic;&"), "a<b>&čč\"'&nic;&");
    }
}
