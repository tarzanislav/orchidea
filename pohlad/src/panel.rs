//! Panel nástrojov (toolbar) a nápoveda klávesov – kreslí ich jadro na plátno, okno len odovzdá kliknutie
//! a polohu myši. Chladne: pohyb myši prekreslí len vtedy, keď sa zmení tlačidlo pod kurzorom.

use crate::prehliadac::Povel;
use orchidea_pismo::{Farba, Pismo, Platno};

/// Výška panela v bodoch.
pub const VYSKA: usize = 34;
const VELKOST: f32 = 12.0;
const OKRAJ: f32 = 6.0;
const MEDZI: f32 = 3.0;
const SKUPINA: f32 = 6.0;

pub const POZADIE: Farba = 0x26282B;
const TLACIDLO: Farba = 0x34373B;
const POD_MYSOU: Farba = 0x474C52;
const TEXT: Farba = 0xE2E2E2;
const NEAKTIVNE: Farba = 0x74777B;
const POPIS: Farba = 0x9A9DA1;
const TIP: Farba = 0x3B3F45;

/// Tlačidlo panela.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tlacidlo {
    /// Stále meno (skúšky, samotest).
    pub id: &'static str,
    pub text: &'static str,
    /// Text pri úzkom okne.
    pub kratky: &'static str,
    /// Bublina s klávesom.
    pub tip: &'static str,
    pub povel: Povel,
}

/// Prvok panela.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prvok {
    Tlacidlo(Tlacidlo),
    /// Nadpis skupiny (pri úzkom okne sa vynechá).
    Popis(&'static str),
    /// Medzera medzi skupinami.
    Medzera,
}

const fn t(
    id: &'static str,
    text: &'static str,
    kratky: &'static str,
    tip: &'static str,
    povel: Povel,
) -> Prvok {
    Prvok::Tlacidlo(Tlacidlo { id, text, kratky, tip, povel })
}

/// Panel prehliadača a editora fotografií.
pub const PREHLIADAC: &[Prvok] = &[
    Prvok::Popis("Otočiť"),
    t("otoc_vlavo", "vľavo", "Vľavo", "Otočiť vľavo (Shift+R)", Povel::OtocVlavo),
    t("otoc_vpravo", "vpravo", "Vpravo", "Otočiť vpravo (R)", Povel::OtocVpravo),
    t("zrkadlo", "Zrkadliť", "Zrkadliť", "Zrkadliť vodorovne (F)", Povel::Zrkadlo),
    t(
        "orezat",
        "Orezať",
        "Orezať",
        "Orezať na zobrazený výrez (X) – najprv priblížte časť, ktorá má ostať",
        Povel::Orezat,
    ),
    Prvok::Medzera,
    Prvok::Popis("Jas"),
    t("jas_menej", "-", "J-", "Jas menej (Shift+J)", Povel::JasMenej),
    t("jas_viac", "+", "J+", "Jas viac (J)", Povel::JasViac),
    Prvok::Popis("Kontrast"),
    t("kontrast_menej", "-", "K-", "Kontrast menej (Shift+K)", Povel::KontrastMenej),
    t("kontrast_viac", "+", "K+", "Kontrast viac (K)", Povel::KontrastViac),
    Prvok::Popis("Gama"),
    t("gama_menej", "-", "G-", "Gama menej – tmavšie stredné tóny (Shift+G)", Povel::GamaMenej),
    t("gama_viac", "+", "G+", "Gama viac – svetlejšie stredné tóny (G)", Povel::GamaViac),
    t("zmensit", "Zmenšiť", "Zmenšiť", "Zmenšiť na polovicu – kvalitné zmenšenie (Z)", Povel::Zmensit),
    Prvok::Medzera,
    t("spat", "Späť", "Späť", "Späť (Ctrl+Z)", Povel::Spat),
    t("znova", "Znova", "Znova", "Znova (Ctrl+Y)", Povel::Znova),
    t(
        "ulozit",
        "Uložiť",
        "Uložiť",
        "Uložiť vedľa originálu ako „…_upravene“ (Ctrl+S) – originál ostáva",
        Povel::Ulozit,
    ),
    Prvok::Medzera,
    t("oznac", "Označiť", "A", "Označiť obraz A na porovnanie (M)", Povel::Oznac),
    t("porovnat", "Porovnať", "A/B", "Porovnať A s aktuálnym: delený pohľad, rozdiel (C)", Povel::Rezim),
    t(
        "prezentacia",
        "Prezentácia",
        "Prez.",
        "Prezentácia na celú obrazovku (F5 alebo F11, Esc koniec)",
        Povel::Prezentacia,
    ),
    t("katalog", "Katalóg", "Kat.", "Katalóg fotografií (Herbár) – hľadanie vo fotkách (F3)", Povel::Katalog),
    t("napoveda", "?", "?", "Nápoveda klávesov (F1 alebo ?)", Povel::Napoveda),
];

/// Panel katalógu fotografií (Herbár).
pub const KATALOG: &[Prvok] = &[
    t(
        "katalog_pridaj",
        "Pridať priečinok",
        "Pridať",
        "Pridá otvorený priečinok aj s podpriečinkami – fotky ostanú na mieste (priečinok sa dá aj pretiahnuť sem)",
        Povel::KatalogPridaj,
    ),
    t(
        "katalog_obnov",
        "Obnoviť",
        "Obnoviť",
        "Prečíta nové a zmenené súbory, zmazané vynechá",
        Povel::KatalogObnov,
    ),
    Prvok::Medzera,
    t(
        "katalog_zavriet",
        "Zavrieť katalóg",
        "Zavrieť",
        "Späť na prehliadanie priečinka (Esc alebo F3)",
        Povel::Katalog,
    ),
    t("napoveda", "?", "?", "Nápoveda klávesov (F1)", Povel::Napoveda),
];

/// Umiestnený prvok.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Umiestnenie {
    pub prvok: Prvok,
    pub x: f32,
    pub sirka: f32,
    pub text: &'static str,
}

/// Rozloží prvky do šírky okna (pri úzkom okne krátke texty bez nadpisov skupín; čo sa nezmestí, vypadne).
pub fn rozloz(prvky: &[Prvok], pismo: &Pismo, sirka: usize) -> Vec<Umiestnenie> {
    let pokus = |prvky: &[Prvok], kratke: bool| {
        let mut v = Vec::new();
        let mut x = 6.0f32;
        for p in prvky {
            match *p {
                Prvok::Tlacidlo(t) => {
                    let text = if kratke { t.kratky } else { t.text };
                    let w = (pismo.sirka(text, VELKOST, 1.0) + 2.0 * OKRAJ).max(26.0);
                    v.push(Umiestnenie { prvok: *p, x, sirka: w, text });
                    x += w + MEDZI;
                }
                Prvok::Popis(text) if !kratke => {
                    let w = pismo.sirka(text, VELKOST, 1.0);
                    v.push(Umiestnenie { prvok: *p, x: x + 2.0, sirka: w, text });
                    x += w + 5.0;
                }
                Prvok::Popis(_) => {}
                Prvok::Medzera => x += SKUPINA,
            }
        }
        (v, x)
    };
    let max = sirka as f32 - 2.0;
    let (dlhe, koniec) = pokus(prvky, false);
    if koniec <= max {
        return dlhe;
    }
    // úzke okno: krátke texty bez nadpisov, potom vypadnú menej potrebné tlačidlá (nápoveda ostane vždy)
    let mut prvky = prvky.to_vec();
    let mut vypadnu = VYPADNU.iter();
    loop {
        let (v, koniec) = pokus(&prvky, true);
        if koniec <= max {
            return v;
        }
        match vypadnu.next() {
            Some(id) => prvky.retain(|p| !matches!(p, Prvok::Tlacidlo(t) if t.id == *id)),
            None => {
                // stále široké: od konca vypadne posledné tlačidlo okrem nápovedy
                match prvky.iter().rposition(|p| matches!(p, Prvok::Tlacidlo(t) if t.id != "napoveda")) {
                    Some(i) => {
                        prvky.remove(i);
                    }
                    None => return v,
                }
            }
        }
    }
}

/// Poradie, v akom tlačidlá vypadnú pri úzkom okne.
const VYPADNU: &[&str] = &[
    "oznac",
    "porovnat",
    "zrkadlo",
    "gama_menej",
    "gama_viac",
    "kontrast_menej",
    "kontrast_viac",
    "zmensit",
    "znova",
    "prezentacia",
    "katalog",
];

/// Tlačidlo na bode (x, y v súradniciach okna).
pub fn na_bode(u: &[Umiestnenie], x: f32, y: f32) -> Option<Tlacidlo> {
    if !(0.0..VYSKA as f32).contains(&y) {
        return None;
    }
    u.iter().find_map(|u| match u.prvok {
        Prvok::Tlacidlo(t) if x >= u.x && x < u.x + u.sirka => Some(t),
        _ => None,
    })
}

/// Nakreslí panel; `aktivne` povie, či tlačidlo práve niečo urobí.
pub fn kresli(
    p: &mut Platno,
    pismo: &Pismo,
    u: &[Umiestnenie],
    pod_mysou: Option<&str>,
    aktivne: impl Fn(Povel) -> bool,
) {
    p.vypln(0, 0, p.sirka as i32, VYSKA as i32, POZADIE);
    p.vypln(0, VYSKA as i32 - 1, p.sirka as i32, 1, 0x1A1B1D);
    let zakladna = VYSKA as f32 / 2.0 + VELKOST * 0.36;
    for u in u {
        match u.prvok {
            Prvok::Tlacidlo(t) => {
                let a = aktivne(t.povel);
                let pozadie = if a && pod_mysou == Some(t.id) { POD_MYSOU } else { TLACIDLO };
                p.vypln(u.x as i32, 5, u.sirka as i32, VYSKA as i32 - 10, pozadie);
                let tw = pismo.sirka(u.text, VELKOST, 1.0);
                let farba = if a { TEXT } else { NEAKTIVNE };
                p.text(pismo, u.text, u.x + (u.sirka - tw) / 2.0, zakladna, VELKOST, 1.0, farba);
            }
            Prvok::Popis(_) => {
                p.text(pismo, u.text, u.x, zakladna, VELKOST, 1.0, POPIS);
            }
            Prvok::Medzera => {}
        }
    }
}

/// Bublina s popisom tlačidla pod panelom.
pub fn kresli_tip(p: &mut Platno, pismo: &Pismo, u: &[Umiestnenie], id: &str) {
    let Some((x, tip)) = u.iter().find_map(|u| match u.prvok {
        Prvok::Tlacidlo(t) if t.id == id => Some((u.x, t.tip)),
        _ => None,
    }) else {
        return;
    };
    let w = pismo.sirka(tip, VELKOST, 1.0) + 16.0;
    let x = x.min(p.sirka as f32 - w - 4.0).max(4.0);
    let y = VYSKA as i32 + 4;
    p.vypln(x as i32, y, w as i32, 24, TIP);
    p.text(pismo, tip, x + 8.0, y as f32 + 16.5, VELKOST, 1.0, TEXT);
}

/// Riadky nápovedy (kláves, čo robí).
pub const NAPOVEDA: &[(&str, &str)] = &[
    ("", "Prehliadanie"),
    ("šípky, medzera, Page Up/Down", "ďalší a predošlý obraz"),
    ("Home / End", "prvý a posledný obraz"),
    ("koliesko, + / -", "priblížiť a oddialiť okolo kurzora"),
    ("ťahanie myšou", "posun priblíženého obrazu"),
    ("0  /  1", "zmestiť do okna  /  skutočná veľkosť"),
    ("T  /  I  /  Tab", "pás náhľadov  /  riadok s údajmi  /  panel nástrojov"),
    ("M, C, pravé tlačidlo", "označiť A, porovnať (delený, rozdiel), posun deliacej čiary"),
    ("F5 alebo F11", "prezentácia na celú obrazovku (P pauza, Esc koniec)"),
    ("", "Úpravy (originál sa neprepíše)"),
    ("R  /  Shift+R", "otočiť vpravo  /  vľavo"),
    ("F  /  X", "zrkadliť  /  orezať na zobrazený výrez"),
    ("J  K  G  (Shift = menej)", "jas, kontrast, gama"),
    ("Z", "zmenšiť na polovicu"),
    ("Ctrl+Z  /  Ctrl+Y", "späť  /  znova"),
    ("Ctrl+S", "uložiť vedľa originálu (JPEG s otočením a orezaním bezstratovo)"),
    ("", "Katalóg fotografií (Herbár) – bez importu, fotky ostanú na mieste"),
    ("F3 alebo Katalóg", "otvoriť a zavrieť katalóg; písanie hneď hľadá"),
    ("šípky, Enter, kliknutie", "vybrať a otvoriť fotku; pretiahnutý priečinok sa pridá"),
    ("dotaz", "kvet -vianoce  datum:2024-05  fotoaparat:canon  iso>=800  gps:ano"),
    ("", "Ostatné"),
    ("F1 alebo ?", "táto nápoveda (zavrie ju Esc alebo kliknutie)"),
    ("Esc", "zavrieť nápovedu, prezentáciu, inak koniec"),
    ("pretiahnutie do okna", "otvoriť súbor alebo priečinok"),
];

/// Nakreslí nápovedu do stredu plátna.
pub fn kresli_napovedu(p: &mut Platno, pismo: &Pismo, riadky: &[(&str, &str)]) {
    let v = 14.0;
    let ludsky = riadky.iter().map(|(k, _)| pismo.sirka(k, v, 1.0)).fold(0.0, f32::max);
    let opis = riadky.iter().map(|(_, o)| pismo.sirka(o, v, 1.0)).fold(0.0, f32::max);
    let w = (ludsky + opis + 70.0).min(p.sirka as f32 - 20.0);
    let h = riadky.len() as f32 * 22.0 + 56.0;
    let x0 = ((p.sirka as f32 - w) / 2.0).max(10.0);
    let y0 = ((p.vyska as f32 - h) / 2.0).max(10.0);
    p.vypln(x0 as i32 - 1, y0 as i32 - 1, w as i32 + 2, h as i32 + 2, 0x5A8DEE);
    p.vypln(x0 as i32, y0 as i32, w as i32, h as i32, 0x202225);
    p.text(pismo, "Nápoveda klávesov", x0 + 20.0, y0 + 30.0, 17.0, 1.2, TEXT);
    let mut y = y0 + 58.0;
    for (k, o) in riadky {
        if k.is_empty() {
            p.text(pismo, o, x0 + 20.0, y, v, 1.2, 0x8FB4FF);
        } else {
            p.text(pismo, k, x0 + 30.0, y, v, 1.0, TEXT);
            p.text(pismo, o, x0 + 50.0 + ludsky, y, v, 1.0, POPIS);
        }
        y += 22.0;
    }
}
#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn rozlozenie_podla_sirky() {
        let p = Pismo::stonka();
        let pocet = |u: &[Umiestnenie]| u.iter().filter(|u| matches!(u.prvok, Prvok::Tlacidlo(_))).count();
        let vsetky = PREHLIADAC.iter().filter(|p| matches!(p, Prvok::Tlacidlo(_))).count();
        // okno 1280 bodov (plocha ≈ 1264): všetko s plnými textami
        let u = rozloz(PREHLIADAC, &p, 1264);
        assert_eq!(pocet(&u), vsetky);
        let l = u.last().unwrap();
        assert!(u.iter().any(|u| u.text == "Prezentácia"), "{}", l.x + l.sirka);
        for s in [1000, 800, 600, 400] {
            let u = rozloz(PREHLIADAC, &p, s);
            let l = u.last().unwrap();
            assert!(l.x + l.sirka <= s as f32, "{s}");
            assert!(u.iter().any(|u| matches!(u.prvok, Prvok::Tlacidlo(t) if t.id == "napoveda")), "{s}");
            if s >= 800 {
                assert!(u.iter().any(|u| matches!(u.prvok, Prvok::Tlacidlo(t) if t.id == "spat")), "{s}");
            }
        }
        // tlačidlo na bode
        let u = rozloz(PREHLIADAC, &p, 1264);
        let t = u.iter().find(|u| matches!(u.prvok, Prvok::Tlacidlo(t) if t.id == "ulozit")).unwrap();
        assert_eq!(na_bode(&u, t.x + 1.0, 10.0).map(|t| t.id), Some("ulozit"));
        assert_eq!(na_bode(&u, t.x + 1.0, VYSKA as f32 + 1.0), None);
    }
}
