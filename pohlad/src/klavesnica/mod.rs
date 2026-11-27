//! Klávesnica pre okno v Linuxe: rozloženie XKB, ktoré pošle kompozitor (Wayland) ako text,
//! a prevod stlačeného klávesu na klávesový symbol a znak.
//!
//! Vlastný kód podľa opisu formátu XKB (sekcie `xkb_keycodes`, `xkb_types`, `xkb_compatibility`,
//! `xkb_symbols`) a kódovania klávesových symbolov X11 (KEYSYM). Podporuje to, čo okno potrebuje:
//! skupiny (viac rozložení), úrovne podľa typov klávesov (Shift, Caps Lock, AltGr, Num Lock),
//! virtuálne modifikátory, mŕtve klávesy (ˇ ´ ^ ¨ ° …) a znaky s Ctrl. Akcie XKB (zámky, prepínanie
//! skupín) robí kompozitor – okno dostane hotové modifikátory a skupinu.
//!
//! Bez `unsafe`, nezávislé od systému (skúša sa na všetkých platformách).

mod tabulky;

use std::collections::HashMap;

/// Klávesový symbol X11.
pub type Symbol = u32;

pub const BACKSPACE: Symbol = 0xff08;
pub const TAB: Symbol = 0xff09;
pub const ENTER: Symbol = 0xff0d;
pub const ESCAPE: Symbol = 0xff1b;
pub const DELETE: Symbol = 0xffff;
pub const HOME: Symbol = 0xff50;
pub const VLAVO: Symbol = 0xff51;
pub const HORE: Symbol = 0xff52;
pub const VPRAVO: Symbol = 0xff53;
pub const DOLE: Symbol = 0xff54;
pub const PAGE_UP: Symbol = 0xff55;
pub const PAGE_DOWN: Symbol = 0xff56;
pub const END: Symbol = 0xff57;
pub const KP_ENTER: Symbol = 0xff8d;
pub const KP_PLUS: Symbol = 0xffab;
pub const KP_MINUS: Symbol = 0xffad;
pub const KP_0: Symbol = 0xffb0;
pub const F1: Symbol = 0xffbe;
pub const ISO_LEFT_TAB: Symbol = 0xfe20;

/// Reálne modifikátory XKB (bity masky).
pub const SHIFT: u8 = 0x01;
pub const LOCK: u8 = 0x02;
pub const CTRL: u8 = 0x04;
pub const MOD1: u8 = 0x08;

/// Symbol s menom, ktoré tabuľka nepozná (novší register): nič nepíše, ale drží počet úrovní klávesu.
const NEZNAMY: Symbol = 0x00ff_fffe;
const SKUPIN_MAX: usize = 4;
const UROVNI_MAX: usize = 8;

/// Meno klávesového symbolu → hodnota (`plus`, `scaron`, `dead_caron`, `U20AC`, `0x1000159` …).
pub fn symbol_z_mena(meno: &str) -> Option<Symbol> {
    if let Ok(i) = tabulky::MENA.binary_search_by(|(m, _)| m.cmp(&meno)) {
        return Some(tabulky::MENA[i].1);
    }
    let b = meno.as_bytes();
    if b.len() == 1 && b[0].is_ascii_alphanumeric() {
        return Some(b[0] as Symbol);
    }
    if let Some(h) = meno.strip_prefix("0x") {
        return u32::from_str_radix(h, 16).ok();
    }
    if let Some(h) = meno.strip_prefix('U')
        && (4..=6).contains(&h.len())
        && let Ok(u) = u32::from_str_radix(h, 16)
        && char::from_u32(u).is_some()
    {
        return Some(if (0x20..0x7f).contains(&u) || (0xa0..0x100).contains(&u) {
            u
        } else {
            0x0100_0000 + u
        });
    }
    None
}

/// Znak klávesového symbolu (Enter = `\r`, Backspace = `\u{8}`, Tab = `\t`, Esc = `\u{1b}`).
pub fn znak(s: Symbol) -> Option<char> {
    match s {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(s),
        0x0100_0000..=0x0110_ffff => char::from_u32(s - 0x0100_0000),
        BACKSPACE => Some('\u{8}'),
        TAB | ISO_LEFT_TAB => Some('\t'),
        0xff0a => Some('\n'),
        ENTER | KP_ENTER => Some('\r'),
        ESCAPE => Some('\u{1b}'),
        DELETE => Some('\u{7f}'),
        0xff80 => Some(' '),
        0xff89 => Some('\t'),
        0xffaa..=0xffb9 | 0xffbd => char::from_u32(s - 0xff80),
        _ => tabulky::ZNAKY.binary_search_by_key(&s, |&(k, _)| k).ok().map(|i| tabulky::ZNAKY[i].1),
    }
}

fn je_mrtvy(s: Symbol) -> bool {
    (0xfe50..=0xfe5c).contains(&s)
}

fn je_klavesnica_cisel(s: Symbol) -> bool {
    (0xff80..=0xffbd).contains(&s)
}

/// Je symbol modifikátor (Shift, Ctrl, Alt, AltGr, Caps Lock …) – ten sa neopakuje a nič nepíše.
pub fn je_modifikator(s: Symbol) -> bool {
    matches!(s, 0xffe1..=0xffee | 0xff7e | 0xff7f | 0xfe01..=0xfe0f | 0xfe11..=0xfe13 | 0xff14)
}

/// Prvé je malé a druhé veľké písmeno (každé samo; nemusia byť dvojicou – ako pri automatickom type XKB).
fn male_velke(a: Symbol, b: Symbol) -> bool {
    matches!((znak(a), znak(b)), (Some(m), Some(v)) if m.is_lowercase() && v.is_uppercase())
}

/// Typ klávesu: ktoré modifikátory vyberajú úroveň.
#[derive(Debug, Clone)]
struct Typ {
    meno: String,
    maska: u8,
    /// (maska modifikátorov, úroveň od 0, zachované modifikátory – nespotrebuje ich)
    mapy: Vec<(u8, u8, u8)>,
}

#[derive(Debug, Clone, Default)]
struct Kluc {
    /// Pre každú skupinu: index typu (alebo None = automatický) a symboly úrovní.
    skupiny: Vec<(Option<usize>, Vec<Symbol>)>,
}

/// Výsledok stlačenia klávesu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stlacenie {
    /// Klávesový symbol podľa rozloženia a modifikátorov (0 = žiadny).
    pub symbol: Symbol,
    /// Napísaný znak (po mŕtvom klávese spojený, s Ctrl riadiaci znak, s Alt žiadny).
    pub znak: Option<char>,
}

/// Rozloženie klávesnice a stav modifikátorov.
#[derive(Debug, Clone)]
pub struct Klavesnica {
    klavesy: HashMap<u32, Kluc>,
    typy: Vec<Typ>,
    /// Reálne modifikátory virtuálnych (v poradí vyhlásenia – index 8, 9, … v maske kompozitora).
    virtualne: Vec<(String, u8)>,
    modifikatory: u8,
    skupina: u32,
    mrtvy: Option<Symbol>,
}

// ————— rozbor textu XKB —————

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Slovo(String),
    Text(String),
    Meno(String),
    Cislo(i64),
    Znak(char),
}

fn tokeny(t: &str) -> Vec<Tok> {
    let mut v = Vec::new();
    let z: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < z.len() {
        let c = z[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && z.get(i + 1) == Some(&'/') || c == '#' {
            while i < z.len() && z[i] != '\n' {
                i += 1;
            }
        } else if c == '"' {
            let mut s = String::new();
            i += 1;
            while i < z.len() && z[i] != '"' {
                if z[i] == '\\' && i + 1 < z.len() {
                    i += 1;
                }
                s.push(z[i]);
                i += 1;
            }
            i += 1;
            v.push(Tok::Text(s));
        } else if c == '<' {
            let mut s = String::new();
            i += 1;
            while i < z.len() && z[i] != '>' {
                s.push(z[i]);
                i += 1;
            }
            i += 1;
            v.push(Tok::Meno(s));
        } else if c.is_ascii_digit() {
            let mut s = String::new();
            while i < z.len() && (z[i].is_ascii_alphanumeric() || z[i] == '_') {
                s.push(z[i]);
                i += 1;
            }
            let n = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                i64::from_str_radix(h, 16).ok()
            } else {
                s.parse().ok()
            };
            // názvy ako „1“ alebo „0x1000159“ sú v zozname symbolov aj čísla
            v.push(n.map(Tok::Cislo).unwrap_or(Tok::Slovo(s)));
        } else if c.is_alphanumeric() || c == '_' {
            let mut s = String::new();
            while i < z.len() && (z[i].is_alphanumeric() || z[i] == '_') {
                s.push(z[i]);
                i += 1;
            }
            v.push(Tok::Slovo(s));
        } else {
            v.push(Tok::Znak(c));
            i += 1;
        }
    }
    v
}

struct Citac {
    t: Vec<Tok>,
    i: usize,
}

impl Citac {
    fn dalsi(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn pozri(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn je(&self, c: char) -> bool {
        self.pozri() == Some(&Tok::Znak(c))
    }
    fn preskoc(&mut self, c: char) -> bool {
        if self.je(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    /// Preskočí po `;` alebo po koniec bloku (vnorené `{}`/`[]`/`()` sa preskočia celé).
    fn po_bodkociarku(&mut self) {
        let mut hlbka = 0i32;
        while let Some(t) = self.pozri() {
            match t {
                Tok::Znak('{' | '[' | '(') => hlbka += 1,
                Tok::Znak('}' | ']' | ')') => {
                    if hlbka == 0 {
                        return;
                    }
                    hlbka -= 1;
                }
                Tok::Znak(';') if hlbka == 0 => {
                    self.i += 1;
                    return;
                }
                _ => {}
            }
            self.i += 1;
        }
    }
    /// Preskočí uzavretú zátvorku (aktuálny token je otváracia).
    fn preskoc_blok(&mut self) {
        let mut hlbka = 0i32;
        while let Some(t) = self.dalsi() {
            match t {
                Tok::Znak('{' | '[' | '(') => hlbka += 1,
                Tok::Znak('}' | ']' | ')') => {
                    hlbka -= 1;
                    if hlbka <= 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

fn realny(meno: &str) -> Option<u8> {
    Some(match meno.to_ascii_lowercase().as_str() {
        "shift" => 0x01,
        "lock" => 0x02,
        "control" | "ctrl" => 0x04,
        "mod1" => 0x08,
        "mod2" => 0x10,
        "mod3" => 0x20,
        "mod4" => 0x40,
        "mod5" => 0x80,
        "none" => 0,
        "all" | "any" => 0xff,
        _ => return None,
    })
}

/// Výraz modifikátorov (`Shift+LevelThree`): reálne bity a mená virtuálnych.
fn modifikatory(c: &mut Citac) -> (u8, Vec<String>) {
    let (mut m, mut v) = (0u8, Vec::new());
    loop {
        match c.pozri().cloned() {
            Some(Tok::Slovo(s)) => {
                c.i += 1;
                match realny(&s) {
                    Some(r) => m |= r,
                    None => v.push(s),
                }
            }
            Some(Tok::Cislo(n)) => {
                c.i += 1;
                m |= n as u8;
            }
            _ => break,
        }
        if !c.preskoc('+') && !c.preskoc('|') {
            break;
        }
    }
    (m, v)
}

fn uroven(t: Option<Tok>) -> Option<u8> {
    let n = match t? {
        Tok::Cislo(n) => n,
        Tok::Slovo(s) => s.strip_prefix("Level").or_else(|| s.strip_prefix("level"))?.parse().ok()?,
        _ => return None,
    };
    (1..=UROVNI_MAX as i64).contains(&n).then(|| (n - 1) as u8)
}

fn skupina(c: &mut Citac) -> Option<usize> {
    if !c.preskoc('[') {
        return None;
    }
    let g = match c.dalsi() {
        Some(Tok::Slovo(s)) => {
            s.strip_prefix("Group").or_else(|| s.strip_prefix("group"))?.parse::<usize>().ok()?
        }
        Some(Tok::Cislo(n)) => n as usize,
        _ => return None,
    };
    c.preskoc(']');
    (1..=SKUPIN_MAX).contains(&g).then(|| g - 1)
}

/// Zoznam symbolov `[ a, A, { b, c }, NoSymbol ]` – z viacerých symbolov na úrovni sa berie prvý.
fn symboly(c: &mut Citac) -> Vec<Symbol> {
    let mut v = Vec::new();
    if !c.preskoc('[') {
        return v;
    }
    loop {
        match c.dalsi() {
            Some(Tok::Znak(']')) | None => break,
            Some(Tok::Znak(',')) => {}
            Some(Tok::Znak('{')) => {
                let mut prvy = None;
                while let Some(t) = c.dalsi() {
                    match t {
                        Tok::Znak('}') => break,
                        Tok::Slovo(s) => prvy = prvy.or(Some(symbol_z_mena(&s).unwrap_or(NEZNAMY))),
                        Tok::Cislo(n) => prvy = prvy.or(Some(symbol_cisla(n))),
                        _ => {}
                    }
                }
                v.push(prvy.unwrap_or(0));
            }
            Some(Tok::Slovo(s)) => v.push(symbol_z_mena(&s).unwrap_or(NEZNAMY)),
            Some(Tok::Cislo(n)) => v.push(symbol_cisla(n)),
            Some(_) => {}
        }
    }
    v.truncate(UROVNI_MAX);
    v
}

/// Číslo v zozname symbolov: 0–9 je číslica, inak hodnota symbolu.
fn symbol_cisla(n: i64) -> Symbol {
    if (0..10).contains(&n) { b'0' as Symbol + n as Symbol } else { n as Symbol }
}

/// Mapa typu z textu: modifikátory (reálne, virtuálne), úroveň, zachované (reálne, virtuálne).
type MapaZTextu = (u8, Vec<String>, u8, (u8, Vec<String>));
/// Typ z textu: meno, reálne a virtuálne modifikátory, mapy.
type TypZTextu = (String, u8, Vec<String>, Vec<MapaZTextu>);
/// Kláves z textu: meno kódu, skupiny (typ, symboly), typ pre všetky skupiny, virtuálne modifikátory.
type KlucZTextu = (String, Vec<(Option<String>, Vec<Symbol>)>, Option<String>, Vec<String>);

#[derive(Default)]
struct Rozbor {
    kody: HashMap<String, u32>,
    aliasy: Vec<(String, String)>,
    typy: Vec<TypZTextu>,
    virtualne: Vec<String>,
    virt_priradene: HashMap<String, u8>,
    /// symbol → virtuálny modifikátor (z `interpret`); `true` = len na prvej úrovni prvej skupiny
    vyklady: Vec<(Symbol, String, bool)>,
    /// predvolené `interpret.useModMapMods`
    len_prva: bool,
    klavesy: Vec<KlucZTextu>,
    mapa_mod: Vec<(u8, Tok)>,
}

impl Rozbor {
    fn virtualne(&mut self, c: &mut Citac) {
        // virtual_modifiers NumLock,Alt=Mod1,…;
        loop {
            match c.dalsi() {
                Some(Tok::Slovo(s)) => {
                    if !self.virtualne.contains(&s) {
                        self.virtualne.push(s.clone());
                    }
                    if c.preskoc('=') {
                        let (m, _) = modifikatory(c);
                        self.virt_priradene.insert(s, m);
                    }
                }
                Some(Tok::Znak(',')) => {}
                Some(Tok::Znak(';')) | None => break,
                _ => {}
            }
        }
    }

    fn keycodes(&mut self, c: &mut Citac) {
        while let Some(t) = c.pozri().cloned() {
            match t {
                Tok::Znak('}') => return,
                Tok::Meno(m) => {
                    c.i += 1;
                    if c.preskoc('=')
                        && let Some(Tok::Cislo(n)) = c.dalsi()
                    {
                        self.kody.insert(m, n as u32);
                    }
                    c.po_bodkociarku();
                }
                Tok::Slovo(s) if s == "alias" => {
                    c.i += 1;
                    if let (Some(Tok::Meno(a)), true, Some(Tok::Meno(b))) =
                        (c.dalsi(), c.preskoc('='), c.dalsi())
                    {
                        self.aliasy.push((a, b));
                    }
                    c.po_bodkociarku();
                }
                _ => {
                    c.i += 1;
                    c.po_bodkociarku();
                }
            }
        }
    }

    fn types(&mut self, c: &mut Citac) {
        while let Some(t) = c.pozri().cloned() {
            match t {
                Tok::Znak('}') => return,
                Tok::Slovo(s) if s == "virtual_modifiers" => {
                    c.i += 1;
                    self.virtualne(c);
                }
                Tok::Slovo(s) if s == "type" => {
                    c.i += 1;
                    let Some(Tok::Text(meno)) = c.dalsi() else {
                        c.po_bodkociarku();
                        continue;
                    };
                    let (mut maska, mut vmaska, mut mapy) = (0u8, Vec::new(), Vec::new());
                    if c.preskoc('{') {
                        while let Some(t) = c.pozri().cloned() {
                            match t {
                                Tok::Znak('}') => {
                                    c.i += 1;
                                    break;
                                }
                                Tok::Slovo(p) if p == "modifiers" => {
                                    c.i += 1;
                                    c.preskoc('=');
                                    (maska, vmaska) = modifikatory(c);
                                    c.po_bodkociarku();
                                }
                                Tok::Slovo(p) if p == "map" => {
                                    c.i += 1;
                                    if c.preskoc('[') {
                                        let (m, v) = modifikatory(c);
                                        c.preskoc(']');
                                        c.preskoc('=');
                                        if let Some(u) = uroven(c.dalsi()) {
                                            mapy.push((m, v, u, (0, Vec::new())));
                                        }
                                    }
                                    c.po_bodkociarku();
                                }
                                Tok::Slovo(p) if p == "preserve" => {
                                    c.i += 1;
                                    if c.preskoc('[') {
                                        let (m, v) = modifikatory(c);
                                        c.preskoc(']');
                                        c.preskoc('=');
                                        let z = modifikatory(c);
                                        if let Some(x) = mapy.iter_mut().find(|x| x.0 == m && x.1 == v) {
                                            x.3 = z;
                                        }
                                    }
                                    c.po_bodkociarku();
                                }
                                _ => {
                                    c.i += 1;
                                    c.po_bodkociarku();
                                }
                            }
                        }
                    }
                    c.preskoc(';');
                    self.typy.push((meno, maska, vmaska, mapy));
                }
                _ => {
                    c.i += 1;
                    c.po_bodkociarku();
                }
            }
        }
    }

    fn compat(&mut self, c: &mut Citac) {
        while let Some(t) = c.pozri().cloned() {
            match t {
                Tok::Znak('}') => return,
                Tok::Slovo(s) if s == "virtual_modifiers" => {
                    c.i += 1;
                    self.virtualne(c);
                }
                Tok::Slovo(s) if s == "interpret" => {
                    c.i += 1;
                    let sym = match c.dalsi() {
                        Some(Tok::Slovo(m)) => symbol_z_mena(&m),
                        Some(Tok::Cislo(n)) => Some(symbol_cisla(n)),
                        _ => None,
                    };
                    // podmienka (+AnyOf(…)) a telo
                    while !c.je('{') && !c.je(';') && c.pozri().is_some() {
                        if c.je('(') {
                            c.preskoc_blok();
                        } else {
                            c.i += 1;
                        }
                    }
                    let mut virt = None;
                    let mut len_prva = self.len_prva;
                    if c.preskoc('{') {
                        while let Some(t) = c.pozri().cloned() {
                            match t {
                                Tok::Znak('}') => {
                                    c.i += 1;
                                    break;
                                }
                                Tok::Slovo(p) if p.eq_ignore_ascii_case("virtualModifier") => {
                                    c.i += 1;
                                    c.preskoc('=');
                                    if let Some(Tok::Slovo(v)) = c.dalsi() {
                                        virt = Some(v);
                                    }
                                    c.po_bodkociarku();
                                }
                                Tok::Slovo(p)
                                    if p.eq_ignore_ascii_case("useModMapMods")
                                        || p.eq_ignore_ascii_case("usemodmap") =>
                                {
                                    c.i += 1;
                                    c.preskoc('=');
                                    len_prva = matches!(c.dalsi(), Some(Tok::Slovo(v)) if v.eq_ignore_ascii_case("level1") || v.eq_ignore_ascii_case("levelone"));
                                    c.po_bodkociarku();
                                }
                                _ => {
                                    c.i += 1;
                                    c.po_bodkociarku();
                                }
                            }
                        }
                    }
                    c.preskoc(';');
                    if let (Some(s), Some(v)) = (sym, virt) {
                        self.vyklady.push((s, v, len_prva));
                    }
                }
                Tok::Slovo(s) if s == "interpret" => {}
                Tok::Slovo(s) if s.starts_with("interpret") => {
                    // interpret.useModMapMods= AnyLevel;
                    c.i += 1;
                    let mut t = Vec::new();
                    while let Some(x) = c.dalsi() {
                        if x == Tok::Znak(';') {
                            break;
                        }
                        t.push(x);
                    }
                    if t.iter().any(|x| matches!(x, Tok::Slovo(p) if p.eq_ignore_ascii_case("useModMapMods")))
                    {
                        self.len_prva =
                            t.iter().any(|x| matches!(x, Tok::Slovo(v) if v.eq_ignore_ascii_case("level1")));
                    }
                }
                _ => {
                    c.i += 1;
                    c.po_bodkociarku();
                }
            }
        }
    }

    fn symbols(&mut self, c: &mut Citac) {
        while let Some(t) = c.pozri().cloned() {
            match t {
                Tok::Znak('}') => return,
                Tok::Slovo(s) if s == "key" => {
                    c.i += 1;
                    let Some(Tok::Meno(meno)) = c.dalsi() else {
                        c.po_bodkociarku();
                        continue;
                    };
                    let mut skupiny: Vec<(Option<String>, Vec<Symbol>)> = Vec::new();
                    let (mut typ_vsetky, mut virt) = (None, Vec::new());
                    let mut dalsia_hola = 0usize;
                    if c.preskoc('{') {
                        loop {
                            match c.pozri().cloned() {
                                None => break,
                                Some(Tok::Znak('}')) => {
                                    c.i += 1;
                                    break;
                                }
                                Some(Tok::Znak(',')) => c.i += 1,
                                Some(Tok::Znak('[')) => {
                                    let s = symboly(c);
                                    let g = dalsia_hola;
                                    dalsia_hola += 1;
                                    if g < SKUPIN_MAX {
                                        skupiny.resize_with(skupiny.len().max(g + 1), Default::default);
                                        skupiny[g].1 = s;
                                    }
                                }
                                Some(Tok::Slovo(p)) => {
                                    c.i += 1;
                                    let g = skupina(c);
                                    c.preskoc('=');
                                    match p.as_str() {
                                        "symbols" => {
                                            let s = symboly(c);
                                            let g = g.unwrap_or(0);
                                            dalsia_hola = dalsia_hola.max(g + 1);
                                            skupiny.resize_with(skupiny.len().max(g + 1), Default::default);
                                            skupiny[g].1 = s;
                                        }
                                        "type" => {
                                            if let Some(Tok::Text(t)) = c.dalsi() {
                                                match g {
                                                    Some(g) => {
                                                        skupiny.resize_with(
                                                            skupiny.len().max(g + 1),
                                                            Default::default,
                                                        );
                                                        skupiny[g].0 = Some(t);
                                                    }
                                                    None => typ_vsetky = Some(t),
                                                }
                                            }
                                        }
                                        "virtualMods" | "virtualmods" | "vmods" => {
                                            virt = modifikatory(c).1;
                                        }
                                        _ => {
                                            // actions[…] = [ … ], repeat = …, overlay …
                                            while !c.je(',') && !c.je('}') && c.pozri().is_some() {
                                                if c.je('[') || c.je('(') || c.je('{') {
                                                    c.preskoc_blok();
                                                } else {
                                                    c.i += 1;
                                                }
                                            }
                                        }
                                    }
                                }
                                Some(_) => c.i += 1,
                            }
                        }
                    }
                    c.preskoc(';');
                    self.klavesy.push((meno, skupiny, typ_vsetky, virt));
                }
                Tok::Slovo(s) if s == "modifier_map" || s == "modmap" || s == "mod_map" => {
                    c.i += 1;
                    let m = match c.dalsi() {
                        Some(Tok::Slovo(m)) => realny(&m).unwrap_or(0),
                        _ => 0,
                    };
                    if c.preskoc('{') {
                        while let Some(t) = c.dalsi() {
                            match t {
                                Tok::Znak('}') => break,
                                Tok::Meno(_) | Tok::Slovo(_) | Tok::Cislo(_) => self.mapa_mod.push((m, t)),
                                _ => {}
                            }
                        }
                    }
                    c.preskoc(';');
                }
                Tok::Slovo(s) if s == "virtual_modifiers" => {
                    c.i += 1;
                    self.virtualne(c);
                }
                _ => {
                    c.i += 1;
                    c.po_bodkociarku();
                }
            }
        }
    }
}

/// Typy, ktoré XKB priraďuje automaticky (keď kláves nemá `type=`), a zakladné typy pre prípad,
/// že ich rozloženie nemá.
const ZAKLADNE_TYPY: &str = r#"
    type "ONE_LEVEL" { modifiers= none; };
    type "TWO_LEVEL" { modifiers= Shift; map[Shift]= 2; };
    type "ALPHABETIC" { modifiers= Shift+Lock; map[Shift]= 2; map[Lock]= 2; };
    type "KEYPAD" { modifiers= Shift+NumLock; map[None]= 1; map[Shift]= 2; map[NumLock]= 2; map[Shift+NumLock]= 1; };
    type "FOUR_LEVEL" { modifiers= Shift+LevelThree; map[Shift]= 2; map[LevelThree]= 3; map[Shift+LevelThree]= 4; };
    type "FOUR_LEVEL_ALPHABETIC" { modifiers= Shift+Lock+LevelThree; map[Shift]= 2; map[Lock]= 2;
        map[LevelThree]= 3; map[Shift+LevelThree]= 4; map[Lock+LevelThree]= 4; map[Shift+Lock+LevelThree]= 3; };
    type "FOUR_LEVEL_SEMIALPHABETIC" { modifiers= Shift+Lock+LevelThree; map[Shift]= 2; map[Lock]= 2;
        map[LevelThree]= 3; map[Shift+LevelThree]= 4; map[Lock+LevelThree]= 3; map[Shift+Lock+LevelThree]= 4; };
    type "FOUR_LEVEL_KEYPAD" { modifiers= Shift+NumLock+LevelThree; map[Shift]= 2; map[NumLock]= 2;
        map[LevelThree]= 3; map[Shift+LevelThree]= 4; map[NumLock+LevelThree]= 4; map[Shift+NumLock+LevelThree]= 3; };
    type "EIGHT_LEVEL" { modifiers= Shift+LevelThree+LevelFive; map[Shift]= 2; map[LevelThree]= 3;
        map[Shift+LevelThree]= 4; map[LevelFive]= 5; map[Shift+LevelFive]= 6; map[LevelThree+LevelFive]= 7;
        map[Shift+LevelThree+LevelFive]= 8; };
    type "EIGHT_LEVEL_ALPHABETIC" { modifiers= Shift+Lock+LevelThree+LevelFive; map[Shift]= 2; map[Lock]= 2;
        map[LevelThree]= 3; map[Shift+LevelThree]= 4; map[Lock+LevelThree]= 4; map[Shift+Lock+LevelThree]= 3;
        map[LevelFive]= 5; map[Shift+LevelFive]= 6; map[Lock+LevelFive]= 6; map[LevelThree+LevelFive]= 7;
        map[Shift+LevelThree+LevelFive]= 8; map[Lock+LevelThree+LevelFive]= 8; map[Shift+Lock+LevelThree+LevelFive]= 7; };
    type "EIGHT_LEVEL_SEMIALPHABETIC" { modifiers= Shift+Lock+LevelThree+LevelFive; map[Shift]= 2; map[Lock]= 2;
        map[LevelThree]= 3; map[Shift+LevelThree]= 4; map[Lock+LevelThree]= 3; map[Shift+Lock+LevelThree]= 4;
        map[LevelFive]= 5; map[Shift+LevelFive]= 6; map[Lock+LevelFive]= 6; map[LevelThree+LevelFive]= 7;
        map[Shift+LevelThree+LevelFive]= 8; map[Lock+LevelThree+LevelFive]= 7; map[Shift+Lock+LevelThree+LevelFive]= 8; };
"#;

/// Americké rozloženie (evdev kód → malé, veľké), ak kompozitor nepošle rozloženie alebo sa nedá prečítať.
const ZAKLADNE: &[(u32, char, char)] = &[
    (2, '1', '!'),
    (3, '2', '@'),
    (4, '3', '#'),
    (5, '4', '$'),
    (6, '5', '%'),
    (7, '6', '^'),
    (8, '7', '&'),
    (9, '8', '*'),
    (10, '9', '('),
    (11, '0', ')'),
    (12, '-', '_'),
    (13, '=', '+'),
    (16, 'q', 'Q'),
    (17, 'w', 'W'),
    (18, 'e', 'E'),
    (19, 'r', 'R'),
    (20, 't', 'T'),
    (21, 'y', 'Y'),
    (22, 'u', 'U'),
    (23, 'i', 'I'),
    (24, 'o', 'O'),
    (25, 'p', 'P'),
    (26, '[', '{'),
    (27, ']', '}'),
    (30, 'a', 'A'),
    (31, 's', 'S'),
    (32, 'd', 'D'),
    (33, 'f', 'F'),
    (34, 'g', 'G'),
    (35, 'h', 'H'),
    (36, 'j', 'J'),
    (37, 'k', 'K'),
    (38, 'l', 'L'),
    (39, ';', ':'),
    (40, '\'', '"'),
    (41, '`', '~'),
    (43, '\\', '|'),
    (44, 'z', 'Z'),
    (45, 'x', 'X'),
    (46, 'c', 'C'),
    (47, 'v', 'V'),
    (48, 'b', 'B'),
    (49, 'n', 'N'),
    (50, 'm', 'M'),
    (51, ',', '<'),
    (52, '.', '>'),
    (53, '/', '?'),
    (57, ' ', ' '),
];

/// Klávesy mimo písmen podľa evdev kódu (rovnaké na každom rozložení).
const ZAKLADNE_OSTATNE: &[(u32, Symbol)] = &[
    (1, ESCAPE),
    (14, BACKSPACE),
    (15, TAB),
    (28, ENTER),
    (29, 0xffe3),
    (42, 0xffe1),
    (54, 0xffe2),
    (56, 0xffe9),
    (58, 0xffe5),
    (59, F1),
    (60, F1 + 1),
    (61, F1 + 2),
    (62, F1 + 3),
    (63, F1 + 4),
    (64, F1 + 5),
    (65, F1 + 6),
    (66, F1 + 7),
    (67, F1 + 8),
    (68, F1 + 9),
    (87, F1 + 10),
    (88, F1 + 11),
    (74, KP_MINUS),
    (78, KP_PLUS),
    (96, KP_ENTER),
    (97, 0xffe4),
    (100, 0xfe03),
    (102, HOME),
    (103, HORE),
    (104, PAGE_UP),
    (105, VLAVO),
    (106, VPRAVO),
    (107, END),
    (108, DOLE),
    (109, PAGE_DOWN),
    (111, DELETE),
];

impl Default for Klavesnica {
    fn default() -> Self {
        Self::zakladna()
    }
}

impl Klavesnica {
    /// Americké rozloženie bez súboru XKB.
    pub fn zakladna() -> Klavesnica {
        let mut k =
            Klavesnica::z_textu(&format!("xkb_types {{ {ZAKLADNE_TYPY} }};")).unwrap_or_else(|| Klavesnica {
                klavesy: HashMap::new(),
                typy: Vec::new(),
                virtualne: Vec::new(),
                modifikatory: 0,
                skupina: 0,
                mrtvy: None,
            });
        for &(kod, m, v) in ZAKLADNE {
            let (m, v) = (m as Symbol, v as Symbol);
            k.klavesy.insert(kod + 8, Kluc { skupiny: vec![(None, vec![m, v])] });
        }
        for &(kod, s) in ZAKLADNE_OSTATNE {
            k.klavesy.insert(kod + 8, Kluc { skupiny: vec![(None, vec![s])] });
        }
        k
    }

    /// Prečíta rozloženie v textovom formáte XKB (`xkb_keymap { … }`), ako ho posiela kompozitor.
    pub fn z_textu(text: &str) -> Option<Klavesnica> {
        let mut c = Citac { t: tokeny(text), i: 0 };
        let mut r = Rozbor::default();
        while let Some(t) = c.dalsi() {
            let Tok::Slovo(s) = t else { continue };
            let sekcia = match s.as_str() {
                "xkb_keycodes" => 0,
                "xkb_types" => 1,
                "xkb_compatibility" | "xkb_compat" | "xkb_compatibility_map" => 2,
                "xkb_symbols" => 3,
                _ => continue,
            };
            if matches!(c.pozri(), Some(Tok::Text(_))) {
                c.i += 1;
            }
            if !c.preskoc('{') {
                continue;
            }
            match sekcia {
                0 => r.keycodes(&mut c),
                1 => r.types(&mut c),
                2 => r.compat(&mut c),
                _ => r.symbols(&mut c),
            }
            c.preskoc('}');
            c.preskoc(';');
        }
        let typov_z_textu = r.typy.len();
        // chýbajúce základné typy (automatické priradenie)
        let mut z = Citac { t: tokeny(ZAKLADNE_TYPY), i: 0 };
        let mut zr = Rozbor::default();
        zr.types(&mut z);
        for t in zr.typy {
            if !r.typy.iter().any(|x| x.0 == t.0) {
                r.typy.push(t);
            }
        }
        for v in ["NumLock", "Alt", "LevelThree", "LevelFive", "Meta", "Super", "Hyper", "ScrollLock"] {
            if !r.virtualne.iter().any(|x| x == v) {
                r.virtualne.push(v.to_string());
            }
        }

        // aliasy kódov
        for (a, b) in &r.aliasy {
            if let Some(&k) = r.kody.get(b) {
                r.kody.entry(a.clone()).or_insert(k);
            }
        }
        // mapa modifikátorov: kód klávesu → reálny modifikátor
        let mut mod_klavesu: HashMap<String, u8> = HashMap::new();
        for (m, t) in &r.mapa_mod {
            match t {
                Tok::Meno(n) => *mod_klavesu.entry(n.clone()).or_default() |= m,
                Tok::Slovo(s) => {
                    if let Some(sym) = symbol_z_mena(s) {
                        for (n, sk, _, _) in &r.klavesy {
                            if sk.iter().any(|(_, v)| v.contains(&sym)) {
                                *mod_klavesu.entry(n.clone()).or_default() |= m;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        // virtuálne modifikátory → reálne (z `virtualMods` klávesu alebo z `interpret`)
        let mut virt: HashMap<String, u8> = r.virt_priradene.clone();
        for (n, sk, _, vm) in &r.klavesy {
            let Some(&m) = mod_klavesu.get(n) else { continue };
            let mut mena: Vec<&String> = vm.iter().collect();
            if mena.is_empty() {
                for (sym, v, len_prva) in &r.vyklady {
                    let ma = if *len_prva {
                        sk.first().and_then(|g| g.1.first()) == Some(sym)
                    } else {
                        sk.iter().any(|(_, s)| s.contains(sym))
                    };
                    if ma {
                        mena.push(v);
                    }
                }
            }
            for v in mena {
                *virt.entry(v.clone()).or_default() |= m;
            }
        }
        let skutocna = |m: u8, v: &[String]| -> (u8, bool) {
            let mut nenaviazany = false;
            let mut r = m;
            for n in v {
                match virt.get(n) {
                    Some(&x) if x != 0 => r |= x,
                    _ => nenaviazany = true,
                }
            }
            (r, nenaviazany)
        };
        let typy: Vec<Typ> = r
            .typy
            .iter()
            .map(|(meno, m, v, mapy)| Typ {
                meno: meno.clone(),
                maska: skutocna(*m, v).0,
                mapy: mapy
                    .iter()
                    .filter_map(|(m, v, u, (zm, zv))| {
                        let (s, nenaviazany) = skutocna(*m, v);
                        // záznam s nenaviazaným virtuálnym modifikátorom neplatí
                        (!nenaviazany).then(|| (s, *u, skutocna(*zm, zv).0))
                    })
                    .collect(),
            })
            .collect();
        let index_typu = |m: &str| typy.iter().position(|t| t.meno == m);
        let mut klavesy = HashMap::new();
        for (n, sk, typ_vsetky, _) in &r.klavesy {
            let Some(&kod) = r.kody.get(n) else { continue };
            let skupiny = sk
                .iter()
                .map(|(t, s)| {
                    let t = t.as_deref().or(typ_vsetky.as_deref()).and_then(index_typu);
                    (t, s.clone())
                })
                .collect();
            klavesy.insert(kod, Kluc { skupiny });
        }
        if klavesy.is_empty() && typov_z_textu == 0 {
            return None;
        }
        Some(Klavesnica {
            klavesy,
            typy,
            virtualne: r.virtualne.iter().map(|v| (v.clone(), virt.get(v).copied().unwrap_or(0))).collect(),
            modifikatory: 0,
            skupina: 0,
            mrtvy: None,
        })
    }

    /// Počet klávesov v rozložení.
    pub fn klavesov(&self) -> usize {
        self.klavesy.len()
    }

    /// Modifikátory a skupina od kompozitora (`wl_keyboard.modifiers`).
    pub fn nastav(&mut self, stlacene: u32, zachytene: u32, zamknute: u32, skupina: u32) {
        let m = stlacene | zachytene | zamknute;
        let mut r = (m & 0xff) as u8;
        for (i, (_, real)) in self.virtualne.iter().enumerate() {
            if i + 8 < 32 && m & (1 << (i + 8)) != 0 {
                r |= real;
            }
        }
        self.modifikatory = r;
        self.skupina = skupina;
    }

    /// Aktuálne reálne modifikátory.
    pub fn modifikatory(&self) -> u8 {
        self.modifikatory
    }

    /// Zabudne rozpísaný mŕtvy kláves (strata zamerania okna).
    pub fn zrus_mrtvy(&mut self) {
        self.mrtvy = None;
    }

    fn automaticky_typ(&self, s: &[Symbol]) -> Option<usize> {
        let n = s.iter().rposition(|&x| x != 0).map_or(1, |i| i + 1);
        let kp = |i: usize| s.get(i).is_some_and(|&x| je_klavesnica_cisel(x));
        let mv = |i: usize| s.len() > i + 1 && male_velke(s[i], s[i + 1]);
        let meno = match n {
            0 | 1 => "ONE_LEVEL",
            2 if kp(0) || kp(1) => "KEYPAD",
            2 if mv(0) => "ALPHABETIC",
            2 => "TWO_LEVEL",
            3 | 4 if kp(0) || kp(1) => "FOUR_LEVEL_KEYPAD",
            3 | 4 if mv(0) && mv(2) => "FOUR_LEVEL_ALPHABETIC",
            3 | 4 if mv(0) => "FOUR_LEVEL_SEMIALPHABETIC",
            3 | 4 => "FOUR_LEVEL",
            _ if mv(0) && mv(2) => "EIGHT_LEVEL_ALPHABETIC",
            _ if mv(0) => "EIGHT_LEVEL_SEMIALPHABETIC",
            _ => "EIGHT_LEVEL",
        };
        self.typy.iter().position(|t| t.meno == meno)
    }

    /// Symbol klávesu pri daných modifikátoroch a skupine; vráti aj spotrebované modifikátory.
    fn symbol_pri(&self, kod_xkb: u32, modifikatory: u8, skupina: u32) -> (Symbol, u8) {
        let Some(k) = self.klavesy.get(&kod_xkb) else { return (0, 0) };
        let platne: Vec<&(Option<usize>, Vec<Symbol>)> =
            k.skupiny.iter().filter(|g| !g.1.is_empty()).collect();
        if platne.is_empty() {
            return (0, 0);
        }
        let g = platne[skupina as usize % platne.len()];
        let typ = g.0.or_else(|| self.automaticky_typ(&g.1)).and_then(|i| self.typy.get(i));
        let (uroven, spotrebovane) = match typ {
            Some(t) => {
                let m = modifikatory & t.maska;
                match t.mapy.iter().find(|x| x.0 == m) {
                    Some(&(_, u, zachovane)) => (u as usize, t.maska & !zachovane),
                    None => (0, t.maska),
                }
            }
            None => (0, 0),
        };
        (g.1.get(uroven).copied().unwrap_or(0), spotrebovane)
    }

    /// Klávesový symbol pre evdev kód klávesu (Wayland posiela evdev kód; kód XKB = evdev + 8).
    pub fn symbol(&self, kod_evdev: u32) -> Symbol {
        self.symbol_pri(kod_evdev + 8, self.modifikatory, self.skupina).0
    }

    /// Stlačenie klávesu: symbol a napísaný znak (mŕtve klávesy sa spoja s ďalším písmenom).
    pub fn stlac(&mut self, kod_evdev: u32) -> Stlacenie {
        let kod = kod_evdev + 8;
        let (symbol, spotrebovane) = self.symbol_pri(kod, self.modifikatory, self.skupina);
        if symbol == 0 || je_modifikator(symbol) {
            return Stlacenie { symbol, znak: None };
        }
        let volne = self.modifikatory & !spotrebovane;
        if je_mrtvy(symbol) && volne & (CTRL | MOD1) == 0 {
            if let Some(m) = self.mrtvy.take() {
                // ten istý mŕtvy kláves dvakrát = samotné znamienko
                return Stlacenie { symbol, znak: samostatny(m) };
            }
            self.mrtvy = Some(symbol);
            return Stlacenie { symbol, znak: None };
        }
        let mut z = znak(symbol);
        // Caps Lock pri klávese, ktorého typ ho nespotreboval
        if volne & LOCK != 0
            && let Some(c) = z
            && c.is_lowercase()
        {
            let mut v = c.to_uppercase();
            if let (Some(x), None) = (v.next(), v.next()) {
                z = Some(x);
            }
        }
        if volne & CTRL != 0 {
            // Ctrl + písmeno = riadiaci znak; pri nelatinskom rozložení písmeno z inej skupiny
            let mut c = z;
            if !c.is_some_and(|c| c.is_ascii()) {
                let skupin = self.klavesy.get(&kod).map_or(0, |k| k.skupiny.len()) as u32;
                for g in 0..skupin {
                    let (s, _) = self.symbol_pri(kod, self.modifikatory, g);
                    if znak(s).is_some_and(|c| c.is_ascii_alphabetic()) {
                        c = znak(s);
                        break;
                    }
                }
            }
            self.mrtvy = None;
            let r = c.and_then(|c| match c {
                'a'..='z' | 'A'..='Z' => char::from_u32(c.to_ascii_uppercase() as u32 - 0x40),
                '@' | '[' | '\\' | ']' | '^' | '_' => char::from_u32(c as u32 - 0x40),
                '\r' | '\t' | '\u{8}' | '\u{1b}' => Some(c),
                _ => None,
            });
            return Stlacenie { symbol, znak: r };
        }
        if volne & MOD1 != 0 {
            self.mrtvy = None;
            return Stlacenie { symbol, znak: None };
        }
        if let Some(m) = self.mrtvy.take() {
            z = match z {
                Some(' ') => samostatny(m),
                Some(c) => spoj(m, c).or(Some(c)),
                None => None,
            };
        }
        Stlacenie { symbol, znak: z }
    }
}

fn samostatny(mrtvy: Symbol) -> Option<char> {
    tabulky::SAMOSTATNE.iter().find(|(k, _)| *k == mrtvy).map(|&(_, c)| c)
}

fn spoj(mrtvy: Symbol, c: char) -> Option<char> {
    tabulky::SPOJENIA.iter().find(|&&(k, b, _)| k == mrtvy && b == c).map(|&(_, _, z)| z)
}

#[cfg(test)]
mod skusky {
    use super::*;

    /// Zhustená časť rozloženia „sk,us“ vo formáte, ako ho posiela kompozitor.
    const SK_US: &str = r#"
xkb_keymap {
xkb_keycodes "(unnamed)" {
	minimum = 8;
	maximum = 708;
	<ESC>                = 9;
	<AE01>               = 10;
	<AE02>               = 11;
	<AE12>               = 21;
	<AD01>               = 24;
	<AD06>               = 29;
	<AC02>               = 39;
	<AC10>               = 47;
	<LFSH>               = 50;
	<AB01>               = 52;
	<KP1>                = 87;
	<LVL3>               = 92;
	<NMLK>               = 77;
	<CAPS>               = 66;
	<LCTL>               = 37;
	<RALT>               = 108;
	<FK11>               = 95;
	alias <ALGR>         = <RALT>;
	indicator 1 = "Caps Lock";
};
xkb_types "(unnamed)" {
	virtual_modifiers NumLock,Alt,LevelThree,LevelFive,Meta,Super,Hyper,ScrollLock;
	type "ONE_LEVEL" { modifiers= none; level_name[1]= "Any"; };
	type "TWO_LEVEL" { modifiers= Shift; map[Shift]= 2; level_name[1]= "Base"; level_name[2]= "Shift"; };
	type "ALPHABETIC" { modifiers= Shift+Lock; map[Shift]= 2; map[Lock]= 2; };
	type "CTRL+ALT" {
		modifiers= Shift+Control+Alt+LevelThree;
		map[Shift]= 2; preserve[Shift]= Shift; map[LevelThree]= 3; map[Shift+LevelThree]= 4;
		preserve[Shift+LevelThree]= Shift; map[Control+Alt]= 5;
	};
	type "FOUR_LEVEL" { modifiers= Shift+LevelThree; map[Shift]= 2; map[LevelThree]= 3; map[Shift+LevelThree]= 4; };
	type "FOUR_LEVEL_ALPHABETIC" {
		modifiers= Shift+Lock+LevelThree; map[Shift]= 2; map[Lock]= 2; map[LevelThree]= 3;
		map[Shift+LevelThree]= 4; map[Lock+LevelThree]= 4; map[Shift+Lock+LevelThree]= 3;
	};
	type "KEYPAD" { modifiers= Shift+NumLock; map[Shift]= 2; level_name[1]= "Base"; map[NumLock]= 2; };
};
xkb_compatibility "(unnamed)" {
	virtual_modifiers NumLock,Alt,LevelThree,LevelFive,Meta,Super,Hyper,ScrollLock;
	interpret.useModMapMods= AnyLevel;
	interpret Num_Lock+AnyOf(all) { virtualModifier= NumLock; action= LockMods(modifiers=NumLock); };
	interpret ISO_Level3_Shift+AnyOf(all) {
		virtualModifier= LevelThree; useModMapMods=level1; action= SetMods(modifiers=LevelThree,clearLocks);
	};
	interpret Alt_L+AnyOf(all) { virtualModifier= Alt; action= SetMods(modifiers=modMapMods,clearLocks); };
	indicator "Caps Lock" { whichModState= locked; modifiers= Lock; };
};
xkb_symbols "(unnamed)" {
	name[Group1]="Slovak";
	name[Group2]="English (US)";
	key <ESC>                {	[          Escape ] };
	key <AE01>               {
		symbols[Group1]= [            plus,               1,          exclam,      dead_tilde ],
		symbols[Group2]= [               1,          exclam ]
	};
	key <AE02>               {
		symbols[Group1]= [          lcaron,               2,              at,      dead_caron ],
		symbols[Group2]= [               2,              at ]
	};
	key <AE12>               {
		symbols[Group1]= [      dead_acute,      dead_caron,      dead_cedilla,     dead_ogonek ],
		symbols[Group2]= [           equal,            plus ]
	};
	key <AD01>               {
		type[Group1]= "FOUR_LEVEL_ALPHABETIC",
		symbols[Group1]= [               q,               Q,       backslash,         Greek_OMEGA ],
		symbols[Group2]= [               q,               Q ]
	};
	key <AD06>               {
		symbols[Group1]= [               z,               Z,           grave,      apostrophe ],
		symbols[Group2]= [               y,               Y ]
	};
	key <AC02>               {	symbols[Group1]= [ s, S, dstroke, section ], symbols[Group2]= [ s, S ] };
	key <AC10>               {
		symbols[Group1]= [     ocircumflex,        quotedbl,          dollar, dead_doubleacute ],
		symbols[Group2]= [       semicolon,           colon ]
	};
	key <LFSH>               {	[         Shift_L ] };
	key <AB01>               {	[ y, Y ], [ z, Z ] };
	key <KP1>                {	[          KP_End,            KP_1 ] };
	key <LVL3>               {	[ ISO_Level3_Shift ] };
	key <NMLK>               {	[ Num_Lock ] };
	key <CAPS>               {	[ Caps_Lock ] };
	key <LCTL>               {	[ Control_L ] };
	key <RALT>               { type= "ONE_LEVEL", symbols[Group1]= [ ISO_Level3_Shift ] };
	key <FK11>               {
		type= "CTRL+ALT",
		symbols[Group1]= [             F11,             F11,             F11,             F11, XF86Switch_VT_11 ]
	};
	modifier_map Shift { <LFSH> };
	modifier_map Lock { <CAPS> };
	modifier_map Control { <LCTL> };
	modifier_map Mod2 { <NMLK> };
	modifier_map Mod5 { <LVL3>, <RALT> };
};
};
"#;

    fn zn(k: &mut Klavesnica, kod: u32) -> Option<char> {
        k.stlac(kod).znak
    }

    #[test]
    fn mena_symbolov_a_znaky() {
        assert_eq!(symbol_z_mena("scaron"), Some(0x1b9));
        assert_eq!(znak(0x1b9), Some('š'));
        assert_eq!(znak(symbol_z_mena("lcaron").unwrap()), Some('ľ'));
        assert_eq!(znak(symbol_z_mena("ocircumflex").unwrap()), Some('ô'));
        assert_eq!(znak(symbol_z_mena("EuroSign").unwrap()), Some('€'));
        assert_eq!(symbol_z_mena("U010F").map(znak), Some(Some('ď')));
        assert_eq!(symbol_z_mena("U0041"), Some(0x41));
        assert_eq!(symbol_z_mena("0x1000159").map(znak), Some(Some('ř')));
        assert_eq!(symbol_z_mena("a"), Some(0x61));
        assert_eq!(symbol_z_mena("F11"), Some(0xffc8));
        assert_eq!(symbol_z_mena("Page_Down"), Some(PAGE_DOWN));
        assert_eq!(symbol_z_mena("nejestvuje"), None);
        assert_eq!(znak(ENTER), Some('\r'));
        assert_eq!(znak(BACKSPACE), Some('\u{8}'));
        assert_eq!(znak(symbol_z_mena("KP_5").unwrap()), Some('5'));
        assert_eq!(znak(symbol_z_mena("KP_Add").unwrap()), Some('+'));
        // tabuľka je zoradená (binárne vyhľadávanie)
        assert!(tabulky::MENA.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(tabulky::ZNAKY.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn slovenske_rozlozenie_urovne_a_skupiny() {
        let mut k = Klavesnica::z_textu(SK_US).expect("rozloženie");
        assert!(k.klavesov() >= 15);
        // evdev = XKB − 8
        assert_eq!(zn(&mut k, 2), Some('+'));
        assert_eq!(zn(&mut k, 3), Some('ľ'));
        assert_eq!(zn(&mut k, 39), Some('ô'));
        assert_eq!(zn(&mut k, 21), Some('z'));
        k.nastav(1, 0, 0, 0); // Shift
        assert_eq!(zn(&mut k, 2), Some('1'));
        assert_eq!(zn(&mut k, 3), Some('2'));
        assert_eq!(zn(&mut k, 21), Some('Z'));
        assert_eq!(zn(&mut k, 39), Some('"'));
        k.nastav(0x80, 0, 0, 0); // AltGr (Mod5 = LevelThree)
        assert_eq!(zn(&mut k, 3), Some('@'));
        assert_eq!(zn(&mut k, 31), Some('đ'));
        k.nastav(0, 0, 0, 1); // druhá skupina (US)
        assert_eq!(zn(&mut k, 2), Some('1'));
        assert_eq!(zn(&mut k, 21), Some('y'));
        assert_eq!(zn(&mut k, 44), Some('z'));
        assert_eq!(zn(&mut k, 39), Some(';'));
        // kláves s jednou skupinou platí v každej skupine
        assert_eq!(zn(&mut k, 1), Some('\u{1b}'));
        k.nastav(0, 0, 0, 0);
        // virtuálne modifikátory v maske kompozitora (index 8 + poradie vyhlásenia: LevelThree = 10)
        k.nastav(1 << 10, 0, 0, 0);
        assert_eq!(zn(&mut k, 3), Some('@'));
        // Caps Lock: písmená veľké, číselný rad nie
        k.nastav(0, 0, 2, 0);
        assert_eq!(zn(&mut k, 21), Some('Z'));
        assert_eq!(zn(&mut k, 3), Some('Ľ'));
        assert_eq!(zn(&mut k, 2), Some('+'));
        // Num Lock na numerickej klávesnici
        k.nastav(0, 0, 0x10, 0);
        assert_eq!(zn(&mut k, 79), Some('1'));
        k.nastav(0, 0, 0, 0);
        assert_eq!(k.stlac(79).symbol, symbol_z_mena("KP_End").unwrap());
        // explicitný typ CTRL+ALT
        assert_eq!(k.stlac(87).symbol, 0xffc8);
        // AltGr+Q na type FOUR_LEVEL_ALPHABETIC
        k.nastav(0x80, 0, 0, 0);
        assert_eq!(zn(&mut k, 16), Some('\\'));
    }

    #[test]
    fn mrtve_klavesy_a_ctrl() {
        let mut k = Klavesnica::z_textu(SK_US).unwrap();
        // ´ + z = ź, ˇ (Shift+´) + s = š
        assert_eq!(zn(&mut k, 13), None);
        assert_eq!(zn(&mut k, 21), Some('ź'));
        k.nastav(1, 0, 0, 0);
        assert_eq!(zn(&mut k, 13), None);
        k.nastav(0, 0, 0, 0);
        assert_eq!(zn(&mut k, 31), Some('š'));
        // ˇ + medzera chýba v skúšobnom rozložení – dvakrát ´ = samotné znamienko
        assert_eq!(zn(&mut k, 13), None);
        assert_eq!(zn(&mut k, 13), Some('´'));
        // mŕtvy kláves + nespojiteľné písmeno = písmeno
        k.nastav(1, 0, 0, 0);
        assert_eq!(zn(&mut k, 13), None);
        k.nastav(0, 0, 0, 0);
        assert_eq!(zn(&mut k, 39), Some('ô'));
        // Ctrl + písmeno podľa rozloženia (QWERTZ: Z je na AD06 → Ctrl+Z = 0x1A)
        k.nastav(4, 0, 0, 0);
        assert_eq!(zn(&mut k, 21), Some('\u{1a}'));
        assert_eq!(zn(&mut k, 44), Some('\u{19}'));
        assert_eq!(zn(&mut k, 31), Some('\u{13}'));
        // Ctrl na nepísmene nič nepíše
        assert_eq!(zn(&mut k, 3), None);
        // modifikátor sám nič nepíše
        k.nastav(0, 0, 0, 0);
        assert_eq!(k.stlac(42).znak, None);
        assert!(je_modifikator(k.stlac(42).symbol));
    }

    #[test]
    fn zakladne_rozlozenie_a_poskodeny_text() {
        let mut k = Klavesnica::zakladna();
        assert_eq!(zn(&mut k, 30), Some('a'));
        k.nastav(1, 0, 0, 0);
        assert_eq!(zn(&mut k, 30), Some('A'));
        k.nastav(0, 0, 2, 0);
        assert_eq!(zn(&mut k, 30), Some('A'));
        assert_eq!(zn(&mut k, 2), Some('1'));
        assert_eq!(k.symbol(106), VPRAVO);
        assert!(Klavesnica::z_textu("").is_none());
        // poškodený alebo neúplný text nespadne
        for t in [
            "xkb_keymap {",
            "xkb_symbols { key <A> { [ a, ",
            "xkb_types { type \"X\" { map[ = ; } };",
            "}}}}",
        ] {
            let _ = Klavesnica::z_textu(t);
        }
        let mut pol = SK_US.to_string();
        while !pol.is_empty() {
            let n = pol.len() * 2 / 3;
            let mut n = n;
            while !pol.is_char_boundary(n) {
                n -= 1;
            }
            pol.truncate(n);
            if let Some(mut k) = Klavesnica::z_textu(&pol) {
                for kod in 0..120 {
                    let _ = k.stlac(kod);
                }
            }
        }
    }
}
