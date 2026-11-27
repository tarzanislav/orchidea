//! Panel nástrojov (toolbar) a nápoveda klávesov – kreslí ich jadro na plátno, okno len odovzdá kliknutie
//! a polohu myši. Chladne: pohyb myši prekreslí len vtedy, keď sa zmení tlačidlo pod kurzorom.

use crate::orezanie::Pomer;
use crate::prehliadac::Povel;
use orchidea_pismo::{Farba, Pismo, Platno};

/// Výška panela v bodoch.
pub const VYSKA: usize = 34;
const VELKOST: f32 = 12.0;
const OKRAJ: f32 = 5.0;
const MEDZI: f32 = 2.0;
const SKUPINA: f32 = 6.0;

pub const POZADIE: Farba = 0x26282B;
const TLACIDLO: Farba = 0x34373B;
const POD_MYSOU: Farba = 0x474C52;
/// Zapnutá voľba (napríklad zvolený pomer strán orezania).
const ZAPNUTE: Farba = 0x2F4F7E;
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
    t(
        "otvorit",
        "Otvoriť",
        "Otvoriť",
        "Otvoriť obrázok – otvorí sa celý jeho priečinok (Ctrl+O)",
        Povel::Otvorit,
    ),
    Prvok::Medzera,
    Prvok::Popis("Otočiť"),
    t("otoc_vlavo", "vľavo", "Vľavo", "Otočiť vľavo (Shift+R)", Povel::OtocVlavo),
    t("otoc_vpravo", "vpravo", "Vpravo", "Otočiť vpravo (R)", Povel::OtocVpravo),
    t("zrkadlo", "Zrkadliť", "Zrkadliť", "Zrkadliť vodorovne (F)", Povel::Zrkadlo),
    t(
        "orezat",
        "Orezať",
        "Orezať",
        "Orezať (X) – alebo nakreslite výber ťahaním po obraze; Enter oreže, Esc zruší",
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

/// Panel v režime orezania.
pub const OREZANIE: &[Prvok] = &[
    Prvok::Popis("Pomer strán"),
    t("orez_volny", "Voľný", "Voľný", "Ľubovoľný pomer strán", Povel::OrezPomer(Pomer::Volny)),
    t("orez_1x1", "1:1", "1:1", "Štvorec", Povel::OrezPomer(Pomer::Stvorec)),
    t("orez_4x3", "4:3", "4:3", "4:3 – znova kliknúť = na výšku", Povel::OrezPomer(Pomer::P4x3)),
    t(
        "orez_3x2",
        "3:2",
        "3:2",
        "3:2 ako fotoaparát – znova kliknúť = na výšku",
        Povel::OrezPomer(Pomer::P3x2),
    ),
    t(
        "orez_16x9",
        "16:9",
        "16:9",
        "16:9 ako obrazovka – znova kliknúť = na výšku",
        Povel::OrezPomer(Pomer::P16x9),
    ),
    t(
        "orez_otoc",
        "Na výšku / na šírku",
        "Otočiť",
        "Vymeniť pomer na výšku a na šírku",
        Povel::OrezOtocPomer,
    ),
    Prvok::Medzera,
    t("orez_potvrd", "Orezať", "Orezať", "Orezať na výber (Enter)", Povel::OrezPotvrd),
    t("orez_zrus", "Zrušiť", "Zrušiť", "Zrušiť orezanie, obraz ostane (Esc)", Povel::OrezZrus),
    Prvok::Medzera,
    t("napoveda", "?", "?", "Nápoveda klávesov (F1)", Povel::Napoveda),
];

/// Panel katalógu fotografií (Herbár).
pub const KATALOG: &[Prvok] = &[
    t(
        "katalog_pridaj",
        "Pridať priečinok",
        "Pridať",
        "Pridá priečinok aj s podpriečinkami – fotky ostanú na mieste (priečinok sa dá aj pretiahnuť sem)",
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
                    let w = (pismo.sirka(text, VELKOST, 1.0) + 2.0 * OKRAJ).max(22.0);
                    v.push(Umiestnenie { prvok: *p, x, sirka: w, text });
                    x += w + MEDZI;
                }
                Prvok::Popis(text) if !kratke => {
                    let w = pismo.sirka(text, VELKOST, 1.0);
                    v.push(Umiestnenie { prvok: *p, x: x + 2.0, sirka: w, text });
                    x += w + 4.0;
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
    zapnute: impl Fn(Povel) -> bool,
) {
    p.vypln(0, 0, p.sirka as i32, VYSKA as i32, POZADIE);
    p.vypln(0, VYSKA as i32 - 1, p.sirka as i32, 1, 0x1A1B1D);
    let zakladna = VYSKA as f32 / 2.0 + VELKOST * 0.36;
    for u in u {
        match u.prvok {
            Prvok::Tlacidlo(t) => {
                let a = aktivne(t.povel);
                let pozadie = if a && pod_mysou == Some(t.id) {
                    POD_MYSOU
                } else if zapnute(t.povel) {
                    ZAPNUTE
                } else {
                    TLACIDLO
                };
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
    ("ťahanie ľavým tlačidlom", "nakresliť výber na orezanie (Enter oreže, Esc zruší)"),
    ("ťahanie pravým / stredným", "posun priblíženého obrazu (pri porovnaní ľavým; pravým deliaca čiara)"),
    ("0  /  1", "zmestiť do okna  /  skutočná veľkosť"),
    ("T  /  I  /  Tab", "pás náhľadov  /  riadok s údajmi  /  panel nástrojov"),
    ("M, C, pravé tlačidlo", "označiť A, porovnať (delený, rozdiel), posun deliacej čiary"),
    ("F5 alebo F11", "prezentácia na celú obrazovku (P pauza, Esc koniec)"),
    ("", "Úpravy (originál sa neprepíše)"),
    ("R  /  Shift+R", "otočiť vpravo  /  vľavo"),
    ("F  /  X", "zrkadliť  /  orezať: rámik celého obrazu – ťahať rohy a okraje, vnútri posun"),
    ("J  K  G  (Shift = menej)", "jas, kontrast, gama"),
    ("Z", "zmenšiť na polovicu"),
    ("Ctrl+Z  /  Ctrl+Y", "späť  /  znova"),
    ("Ctrl+S", "uložiť vedľa originálu (JPEG s otočením a orezaním bezstratovo)"),
    ("Ctrl+O", "otvoriť obrázok (otvorí sa celý jeho priečinok)"),
    ("", "Hodnotenie a kľúčové slová – do súboru XMP vedľa fotky (foto.jpg.xmp), fotka sa nemení"),
    ("Ctrl+1 … Ctrl+5, Ctrl+0", "hviezdičky (aj kliknutím); Ctrl+0 zruší hodnotenie"),
    ("Ctrl+K", "pridať kľúčové slovo (Enter uloží, Esc zruší); × slovo odoberie"),
    ("", "Katalóg fotografií (Herbár) – bez importu, fotky ostanú na mieste"),
    ("F3 alebo Katalóg", "otvoriť a zavrieť katalóg; písanie hneď hľadá"),
    ("šípky, Enter, kliknutie", "vybrať a otvoriť fotku; pretiahnutý priečinok sa pridá"),
    ("dotaz", "kvet -vianoce  datum:2024-05  fotoaparat:canon  iso>=800  gps:ano"),
    ("", "Ostatné"),
    ("F1 alebo ?", "táto nápoveda (zavrie ju Esc alebo kliknutie)"),
    ("Esc", "zavrieť nápovedu, prezentáciu, inak koniec"),
    ("pretiahnutie do okna", "otvoriť súbor alebo priečinok (aj Ctrl+O)"),
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

// ---------------------------------------------------------------- riadok hodnotenia a kľúčových slov

/// Výška riadku s hodnotením a kľúčovými slovami.
pub const VYSKA_META: usize = 30;
const HVIEZDA: f32 = 20.0;
const ZLATA: Farba = 0xF2C14E;
const ZLATA_SVETLA: Farba = 0xFFDD80;
const OBRYS: Farba = 0x8A8D91;
const POZADIE_META: Farba = 0x232528;
const CHYBA: Farba = 0xFF8A80;

/// Prvok riadku hodnotenia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrvokMeta {
    /// Hviezdička 1–5.
    Hviezda(i8),
    /// Kľúčové slovo (poradie medzi zobrazenými).
    Kluc(usize),
    /// Odobrať kľúčové slovo.
    Odober(usize),
    /// Pridať kľúčové slovo.
    Pridaj,
    /// Pole na písanie nového slova.
    Vstup,
}

/// Umiestnený prvok riadku hodnotenia.
#[derive(Debug, Clone, PartialEq)]
pub struct UmiestnenieMeta {
    pub prvok: PrvokMeta,
    pub x: f32,
    pub sirka: f32,
    pub text: String,
}

/// Rozloží riadok: „Hodnotenie ☆☆☆☆☆   Kľúčové slová: [slovo ×] … [+ Pridať]“ (čo sa nezmestí, vypadne).
pub fn rozloz_meta(
    pismo: &Pismo,
    sirka: usize,
    kluce: &[String],
    vstup: Option<&str>,
) -> Vec<UmiestnenieMeta> {
    let mut v = Vec::new();
    let max = sirka as f32 - 8.0;
    let mut x = 10.0 + pismo.sirka("Hodnotenie", VELKOST, 1.0) + 8.0;
    for n in 1..=5 {
        v.push(UmiestnenieMeta { prvok: PrvokMeta::Hviezda(n), x, sirka: HVIEZDA, text: String::new() });
        x += HVIEZDA;
    }
    x += 18.0 + pismo.sirka("Kľúčové slová", VELKOST, 1.0) + 8.0;
    // pole na písanie alebo „+ Pridať“ má prednosť pred ďalšími slovami
    let koncove = match vstup {
        Some(t) => {
            let t = format!("{t}_");
            (PrvokMeta::Vstup, (pismo.sirka(&t, VELKOST, 1.0) + 16.0).max(160.0), t)
        }
        None => {
            (PrvokMeta::Pridaj, pismo.sirka("+ Pridať", VELKOST, 1.0) + 2.0 * OKRAJ, "+ Pridať".to_string())
        }
    };
    let mut vynechane = 0;
    for (i, k) in kluce.iter().enumerate() {
        let w = pismo.sirka(k, VELKOST, 1.0) + 2.0 * OKRAJ;
        if x + w + 16.0 + 4.0 + koncove.1 > max {
            vynechane = kluce.len() - i;
            break;
        }
        v.push(UmiestnenieMeta { prvok: PrvokMeta::Kluc(i), x, sirka: w, text: k.clone() });
        v.push(UmiestnenieMeta { prvok: PrvokMeta::Odober(i), x: x + w, sirka: 16.0, text: "×".into() });
        x += w + 16.0 + 4.0;
    }
    if vynechane > 0 {
        let t = format!("+{vynechane}");
        let w = pismo.sirka(&t, VELKOST, 1.0) + 8.0;
        v.push(UmiestnenieMeta { prvok: PrvokMeta::Kluc(usize::MAX), x, sirka: w, text: t });
        x += w + 4.0;
    }
    if x + koncove.1 <= max {
        v.push(UmiestnenieMeta { prvok: koncove.0, x, sirka: koncove.1, text: koncove.2 });
    }
    v
}

/// Prvok riadku hodnotenia na bode (x; y vo vnútri riadku).
pub fn na_bode_meta(u: &[UmiestnenieMeta], x: f32) -> Option<PrvokMeta> {
    u.iter()
        .find(|u| x >= u.x && x < u.x + u.sirka && !matches!(u.prvok, PrvokMeta::Kluc(usize::MAX)))
        .map(|u| u.prvok)
}

/// Bublina k prvku riadku hodnotenia.
pub fn tip_meta(p: PrvokMeta, hodnotenie: Option<i8>) -> String {
    match p {
        PrvokMeta::Hviezda(n) if hodnotenie == Some(n) => {
            "Zrušiť hodnotenie (Ctrl+0) – zapíše sa do súboru XMP vedľa fotky".to_string()
        }
        PrvokMeta::Hviezda(n) => {
            format!("Hodnotenie {n} (Ctrl+{n}) – zapíše sa do súboru XMP vedľa fotky, fotka sa nemení")
        }
        PrvokMeta::Kluc(_) => "Kľúčové slovo zo súboru XMP alebo z fotky".into(),
        PrvokMeta::Odober(_) => "Odobrať kľúčové slovo".into(),
        PrvokMeta::Pridaj => "Pridať kľúčové slovo (Ctrl+K), Enter uloží".into(),
        PrvokMeta::Vstup => "Enter uloží, Esc zruší".into(),
    }
}

/// Nakreslí hviezdu (päť cípov) so stredom (cx, cy) a polomerom r – vyplnený mnohouholník po riadkoch.
pub fn hviezda(p: &mut Platno, cx: f32, cy: f32, r: f32, farba: Farba) {
    let body: Vec<(f32, f32)> = (0..10)
        .map(|i| {
            let uhol = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
            let rr = if i % 2 == 0 { r } else { r * 0.42 };
            (cx + rr * uhol.cos(), cy + rr * uhol.sin())
        })
        .collect();
    let (y0, y1) = ((cy - r).floor().max(0.0) as i32, (cy + r).ceil().min(p.vyska as f32 - 1.0) as i32);
    for y in y0..=y1 {
        let yc = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..10 {
            let (a, b) = (body[i], body[(i + 1) % 10]);
            if (a.1 <= yc && b.1 > yc) || (b.1 <= yc && a.1 > yc) {
                xs.push(a.0 + (yc - a.1) / (b.1 - a.1) * (b.0 - a.0));
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for d in xs.chunks(2) {
            if let [od, po] = d {
                let (od, po) = (od.round() as i32, po.round() as i32);
                if po > od {
                    p.vypln(od, y, po - od, 1, farba);
                }
            }
        }
    }
}

/// Nakreslí riadok hodnotenia na výšku `y0`.
#[allow(clippy::too_many_arguments)]
pub fn kresli_meta(
    p: &mut Platno,
    pismo: &Pismo,
    y0: i32,
    u: &[UmiestnenieMeta],
    hodnotenie: Option<i8>,
    pod_mysou: Option<PrvokMeta>,
    sprava: Option<&str>,
) {
    p.vypln(0, y0, p.sirka as i32, VYSKA_META as i32, POZADIE_META);
    let z = y0 as f32 + VYSKA_META as f32 / 2.0 + VELKOST * 0.36;
    p.text(pismo, "Hodnotenie", 10.0, z, VELKOST, 1.0, POPIS);
    let mut koniec = 0.0f32;
    let nahlad = match pod_mysou {
        Some(PrvokMeta::Hviezda(n)) => Some(n),
        _ => None,
    };
    for m in u {
        koniec = koniec.max(m.x + m.sirka);
        match m.prvok {
            PrvokMeta::Hviezda(n) => {
                let (cx, cy) = (m.x + HVIEZDA / 2.0, y0 as f32 + VYSKA_META as f32 / 2.0);
                let plna = nahlad.map_or(hodnotenie.unwrap_or(0) >= n, |h| h >= n);
                if plna {
                    hviezda(p, cx, cy, 8.0, if nahlad.is_some() { ZLATA_SVETLA } else { ZLATA });
                } else {
                    hviezda(p, cx, cy, 8.0, OBRYS);
                    hviezda(p, cx, cy, 6.2, POZADIE_META);
                }
                if n == 1 {
                    let x = m.x + 5.0 * HVIEZDA + 18.0;
                    p.text(pismo, "Kľúčové slová", x, z, VELKOST, 1.0, POPIS);
                }
            }
            PrvokMeta::Kluc(usize::MAX) => {
                p.text(pismo, &m.text, m.x + 4.0, z, VELKOST, 1.0, POPIS);
            }
            PrvokMeta::Kluc(_) => {
                p.vypln(m.x as i32, y0 + 5, m.sirka as i32 + 16, VYSKA_META as i32 - 10, TLACIDLO);
                p.text(pismo, &m.text, m.x + OKRAJ, z, VELKOST, 1.0, TEXT);
            }
            PrvokMeta::Odober(_) => {
                let f = if pod_mysou == Some(m.prvok) { CHYBA } else { POPIS };
                p.text(pismo, "×", m.x + 3.0, z, VELKOST, 1.0, f);
            }
            PrvokMeta::Pridaj => {
                let f = if pod_mysou == Some(m.prvok) { POD_MYSOU } else { TLACIDLO };
                p.vypln(m.x as i32, y0 + 5, m.sirka as i32, VYSKA_META as i32 - 10, f);
                p.text(pismo, &m.text, m.x + OKRAJ, z, VELKOST, 1.0, TEXT);
            }
            PrvokMeta::Vstup => {
                p.vypln(m.x as i32, y0 + 4, m.sirka as i32, VYSKA_META as i32 - 8, 0x141517);
                p.vypln(m.x as i32, y0 + VYSKA_META as i32 - 5, m.sirka as i32, 1, ZLATA);
                p.text(pismo, &m.text, m.x + 6.0, z, VELKOST, 1.0, 0xFFFFFF);
            }
        }
    }
    if let Some(t) = sprava {
        let w = pismo.sirka(t, VELKOST, 1.0);
        let x = (p.sirka as f32 - w - 10.0).max(koniec + 12.0);
        let t = pismo.skrat(t, p.sirka as f32 - x - 6.0, VELKOST, 1.0);
        p.text(pismo, &t, x, z, VELKOST, 1.0, CHYBA);
    }
}

/// Bublina nad riadkom hodnotenia.
pub fn kresli_tip_meta(
    p: &mut Platno,
    pismo: &Pismo,
    u: &[UmiestnenieMeta],
    prvok: PrvokMeta,
    y0: i32,
    text: &str,
) {
    let Some(m) = u.iter().find(|m| m.prvok == prvok) else { return };
    let w = pismo.sirka(text, VELKOST, 1.0) + 16.0;
    let x = m.x.min(p.sirka as f32 - w - 4.0).max(4.0);
    let y = y0 - 28;
    p.vypln(x as i32, y, w as i32, 24, TIP);
    p.text(pismo, text, x + 8.0, y as f32 + 16.5, VELKOST, 1.0, TEXT);
}
