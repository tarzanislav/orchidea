//! Spoločné pre okná všetkých systémov (Windows, Linux): povely znakov, samotest v CI, snímka okna.
//!
//! Okno samo nič nepočíta – vstupy podáva jadru (`prehliadac`) a ukazuje jeho plátno.

use crate::prehliadac::{Povel, Prehliadac, Vstup};
use orchidea_pismo::Platno;
use std::path::{Path, PathBuf};

/// Povel napísaného znaku (mimo písania do poľa hľadania). Riadiace znaky sú Ctrl + písmeno.
pub fn povel_znaku(c: char) -> Option<Povel> {
    Some(match c {
        '0' | '*' => Povel::Prisposobit,
        '1' => Povel::Skutocna,
        'm' | 'M' => Povel::Oznac,
        'c' | 'C' => Povel::Rezim,
        't' | 'T' => Povel::Pas,
        'i' | 'I' => Povel::Udaje,
        // úpravy: malé písmeno = viac / vpravo, veľké = menej / vľavo
        'r' => Povel::OtocVpravo,
        'R' => Povel::OtocVlavo,
        'f' | 'F' => Povel::Zrkadlo,
        'x' | 'X' => Povel::Orezat,
        'j' => Povel::JasViac,
        'J' => Povel::JasMenej,
        'k' => Povel::KontrastViac,
        'K' => Povel::KontrastMenej,
        'g' => Povel::GamaViac,
        'G' => Povel::GamaMenej,
        'z' => Povel::Zmensit,
        'p' | 'P' => Povel::Pauza,
        '?' => Povel::Napoveda,
        '\t' => Povel::Panel,
        '\u{1A}' => Povel::Spat,
        '\u{19}' => Povel::Znova,
        '\u{13}' => Povel::Ulozit,
        '\u{f}' => Povel::Otvorit,
        '\u{b}' => Povel::PridajKluc,
        // Enter mimo písania: potvrdiť orezanie
        '\r' => Povel::OrezPotvrd,
        _ => return None,
    })
}

/// Krok samotestu: vstup alebo kliknutie na tlačidlo panela podľa mena.
#[derive(Debug, Clone, PartialEq)]
pub enum Krok {
    Vstup(Vstup),
    Stlac(String),
    /// Napísať text do poľa hľadania (znak po znaku).
    Pis(String),
    /// Súbor katalógu fotografií.
    Katalog(PathBuf),
    /// Kontrola: jadro si doteraz vyžiadalo práve toľkoto dialógov.
    Dialogy(usize),
}

/// Čo má okno urobiť po kroku samotestu.
#[derive(Debug, Clone, PartialEq)]
pub enum Dalej {
    /// Jadro ešte pracuje – pokračovať po hotovej práci.
    Cakaj,
    /// Podať tieto vstupy a hneď pokračovať ďalším krokom.
    Vstupy(Vec<Vstup>),
    /// Všetky kroky sú hotové – uložiť snímku a zavrieť okno.
    Koniec,
}

/// Samotest v CI: povely, po ktorých sa uloží snímka okna.
#[derive(Debug, Clone)]
pub struct Samotest {
    pub povely: Vec<Krok>,
    pub vystup: PathBuf,
    pub kod: i32,
    /// Tlačidlo zo samotestu nie je na paneli (alebo nesedí počet dialógov).
    pub chyba: bool,
    /// Koľko dialógov si jadro vyžiadalo (v samoteste sa neukážu).
    pub dialogy: usize,
}

impl Samotest {
    /// Ďalší krok (jadro musí byť nečinné).
    pub fn krok(&mut self, jadro: &mut Prehliadac) -> Dalej {
        if jadro.pracuje() {
            return Dalej::Cakaj;
        }
        if self.povely.is_empty() {
            return Dalej::Koniec;
        }
        let v = match self.povely.remove(0) {
            Krok::Vstup(v) => Some(vec![v]),
            Krok::Stlac(id) => jadro.tlacidlo(&id).map(|(x, y)| vec![Vstup::Klik { x, y }]),
            Krok::Pis(t) => Some(t.chars().map(Vstup::Znak).chain(Some(Vstup::Tik)).collect()),
            Krok::Katalog(c) => {
                jadro.nastav_katalog(c);
                Some(vec![Vstup::Tik])
            }
            Krok::Dialogy(n) => (self.dialogy == n).then(|| vec![Vstup::Tik]),
        };
        Dalej::Vstupy(v.unwrap_or_else(|| {
            self.chyba = true;
            Vec::new()
        }))
    }

    /// Po poslednom kroku: vykreslí jadro, uloží snímku a nastaví kód (0 = v poriadku).
    pub fn dokonci(&mut self, jadro: &mut Prehliadac, platno: &mut Platno) -> i32 {
        jadro.vykresli(platno);
        let ok = uloz_png(platno, &self.vystup);
        self.kod = if self.chyba {
            3
        } else if ok && jadro.pocet() > 0 {
            0
        } else {
            2
        };
        println!("SAMOTEST vykreslení {} obrázkov {} kód {}", jadro.vykresleni, jadro.pocet(), self.kod);
        self.kod
    }
}

/// Plátno ako PNG (vlastný zápis z balíka `formaty`).
pub fn uloz_png(p: &Platno, cesta: &Path) -> bool {
    let body: Vec<u8> = p.body.iter().flat_map(|&f| [(f >> 16) as u8, (f >> 8) as u8, f as u8]).collect();
    orchidea_formaty::Obraz::novy(
        p.sirka,
        p.vyska,
        orchidea_formaty::obraz::Model::Rgb,
        false,
        orchidea_formaty::obraz::Vzorky::U8(body),
    )
    .and_then(|o| orchidea_formaty::zapis(&o, orchidea_formaty::Format::Png))
    .map(|d| std::fs::write(cesta, d).is_ok())
    .unwrap_or(false)
}

/// Argumenty programu: `[cesta]` alebo `--samotest <cesta> <výstup.png> [povely …]`.
pub fn argumenty(argumenty: &[String]) -> (Option<PathBuf>, Option<Samotest>) {
    match argumenty.first().map(String::as_str) {
        Some("--samotest") if argumenty.len() >= 3 => {
            let povely = argumenty[3..].iter().filter_map(|p| krok_samotestu(p)).collect();
            (
                Some(PathBuf::from(&argumenty[1])),
                Some(Samotest {
                    povely,
                    vystup: PathBuf::from(&argumenty[2]),
                    kod: 1,
                    chyba: false,
                    dialogy: 0,
                }),
            )
        }
        Some(c) => (Some(PathBuf::from(c)), None),
        None => (None, None),
    }
}

fn krok_samotestu(p: &str) -> Option<Krok> {
    let povel = |p| Some(Krok::Vstup(Vstup::Povel(p)));
    match p {
        "dalsi" => povel(Povel::Dalsi),
        "oznac" => povel(Povel::Oznac),
        "rezim" => povel(Povel::Rezim),
        "priblizit" => povel(Povel::Priblizit),
        "prezentacia" => povel(Povel::Prezentacia),
        "tik" => Some(Krok::Vstup(Vstup::Tik)),
        "napoveda" => povel(Povel::Napoveda),
        "katalog" => povel(Povel::Katalog),
        "enter" => Some(Krok::Vstup(Vstup::Znak('\r'))),
        "ctrl-k" => povel(Povel::PridajKluc),
        t if t.starts_with("hodnotenie:") => t[11..].parse().ok().and_then(|n| povel(Povel::Hodnotenie(n))),
        t if t.starts_with("pis:") => Some(Krok::Pis(t[4..].to_string())),
        t if t.starts_with("katalog-subor:") => Some(Krok::Katalog(PathBuf::from(&t[14..]))),
        t if t.starts_with("dialogy:") => t[8..].parse().ok().map(Krok::Dialogy),
        // stlac:<tlačidlo> – kliknutie na tlačidlo panela (poloha sa zistí až v okne)
        t if t.starts_with("stlac:") => Some(Krok::Stlac(t[6..].to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn argumenty_samotestu() {
        let a: Vec<String> = [
            "--samotest",
            "p",
            "s.png",
            "dalsi",
            "pis:ab",
            "hodnotenie:4",
            "stlac:otvorit",
            "dialogy:1",
            "nic",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (c, t) = argumenty(&a);
        assert_eq!(c, Some(PathBuf::from("p")));
        let t = t.unwrap();
        assert_eq!(t.vystup, PathBuf::from("s.png"));
        assert_eq!(
            t.povely,
            vec![
                Krok::Vstup(Vstup::Povel(Povel::Dalsi)),
                Krok::Pis("ab".into()),
                Krok::Vstup(Vstup::Povel(Povel::Hodnotenie(4))),
                Krok::Stlac("otvorit".into()),
                Krok::Dialogy(1),
            ]
        );
        assert_eq!(argumenty(&["x.jpg".to_string()]).0, Some(PathBuf::from("x.jpg")));
        assert!(argumenty(&[]).0.is_none());
        assert_eq!(povel_znaku('\u{1A}'), Some(Povel::Spat));
        assert_eq!(povel_znaku('q'), None);
    }
}
