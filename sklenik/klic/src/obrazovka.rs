//! Stavová obrazovka SKLENÍKA: model, meno zariadenia, teplota vo farbe, výkon a pripojené zariadenia –
//! po slovensky, česky alebo anglicky, vlastným písmom STONKA s diakritikou.
//!
//! Rozloženie sa počíta z rozlíšenia obrazovky (od malej dotykovej 800×480 po 4K). Kreslí sa do plátna
//! v pamäti; na obrazovku sa zapíšu len riadky, ktoré sa zmenili (časť pre Linux v `linux.rs`).

use crate::periferie::{DruhVstupu, Supis};
use orchidea_pismo::{Farba, Pismo, Platno};
use orchidea_zariadenie::ProfilHardveru;
use std::cell::RefCell;

const POZADIE: Farba = 0x0F1C15;
const CIARA: Farba = 0x2A4535;
const TEXT: Farba = 0xE8F0EA;
const TLMENY: Farba = 0x9DB3A5;
const ZELENA: Farba = 0x6FD08C;
const JANTAROVA: Farba = 0xF0B84A;
const CERVENA: Farba = 0xFF6B5B;

/// Jazyk obrazovky (z nastavenia `jazyk=` v SKLENIK.TXT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jazyk {
    Sk,
    Cs,
    En,
}

impl Jazyk {
    pub fn z_kodu(kod: Option<&str>) -> Jazyk {
        match kod {
            Some("cs") | Some("cz") => Jazyk::Cs,
            Some("en") => Jazyk::En,
            _ => Jazyk::Sk,
        }
    }

    /// Text v jazyku: (slovensky, česky, anglicky).
    fn t<'a>(self, sk: &'a str, cs: &'a str, en: &'a str) -> &'a str {
        match self {
            Jazyk::Sk => sk,
            Jazyk::Cs => cs,
            Jazyk::En => en,
        }
    }

    fn cislo(self, x: f64, desatinnych: usize) -> String {
        let s = format!("{x:.desatinnych$}");
        if self == Jazyk::En { s } else { s.replace('.', ",") }
    }
}

/// Priebeh merania rýchlosti karty.
#[derive(Debug, Clone, PartialEq)]
pub enum MeranieKarty {
    /// Meranie ešte nezačalo.
    Caka,
    /// Meria sa – koľko MB je už prečítaných.
    Bezi(u64),
    /// Súvislé čítanie MB/s a náhodné čítania 4 KB za sekundu.
    Hotovo(f64, f64),
    /// Karta dlho neodpovedá – pri koľkých MB sa meranie zastavilo.
    Zaseknute(u64),
    /// Chyba systému pri čítaní.
    Chyba(String),
}

/// Všetko, čo obrazovka ukazuje.
#[derive(Debug)]
pub struct Stav<'a> {
    pub model: &'a str,
    pub meno: Option<&'a str>,
    pub teplota: Option<f64>,
    pub ciel: f64,
    pub strop: f64,
    /// Výkon v pomere k plnému (0–1), ktorý tepelný strážca práve dovoľuje.
    pub vykon: f64,
    /// Napájanie: `Some(true)` = podpätie teraz, `Some(false)` = v poriadku, `None` = bez snímača.
    pub podpatie: Option<bool>,
    /// Bolo od štartu aspoň raz podpätie?
    pub podpatie_bolo: bool,
    pub supis: &'a Supis,
    /// Za koľko sekúnd od zapnutia jadra bol systém pripravený.
    pub pripravene_s: Option<f64>,
    /// Verzia SKLENÍKA na karte (dátum a odtlačok zostavenia).
    pub verzia: &'a str,
    /// Ako dlho beží systém od zapnutia (sekundy) – hodiny pre skúšky teploty.
    pub bezi_s: u64,
    /// Meranie rýchlosti karty, z ktorej systém štartoval.
    pub meranie_karty: &'a MeranieKarty,
    /// Meno zariadenia, ktoré sa meria (napr. mmcblk0).
    pub merana_karta: &'a str,
    /// Profil hardvéru (PÔDA).
    pub poda: &'a ProfilHardveru,
    /// Stlačená pamäť zram (MB), ak je zapnutá.
    pub zram_mb: Option<u64>,
    /// Hardvérový strážny časovač je zapnutý (R99).
    pub strazny_casovac: bool,
    /// Úsporný režim Ethernetu (R111) na prvom káblovom rozhraní.
    pub eee: Option<Eee>,
    /// Hlásenia jadra o chybách zariadení (karta, USB, obrazovka) – diagnostika (R115).
    pub hlasenia: &'a [String],
    /// Skúška pamäte RAM (R120): `None` – beží, `Ok(MB)` – v poriadku, `Err(MB)` – chyba na danom MB.
    pub test_pamate: Option<Result<usize, usize>>,
    /// Sieťové rozhrania, ktoré sa nedali zapnúť – čip siete neodpovedá (R120).
    pub siet_chyby: &'a [String],
    /// R253: rozhrania so zapojeným káblom, ktorých čip siete práve spí (R191) – nie „bez kábla“.
    pub siet_spi: &'a [String],
    /// Samomeranie (R122) – `None`, ak je vypnuté.
    pub samomeranie: Option<&'a Samomeranie>,
    /// Odhad okolia a prostredie (R123, R124) – `None` bez snímača teploty.
    pub okolie: Option<&'a orchidea_teplo::okolie::StavOkolia>,
    /// Úsporný režim pre horúce okolie (R126).
    pub uspora: bool,
    /// Správca chladu TV krabičky (R158–R161) – `None` na ostatných doskách.
    pub chlad: Option<&'a Chlad>,
}

/// Riadok „Chlad“: čo správca chladu práve robí a priebeh tepelného pokusu.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chlad {
    pub text: String,
    /// Tepelný pokus (R160): fáza a čas – zelenou.
    pub pokus: Option<String>,
}

/// Samomeranie (R122) – čo ukazuje riadok „Meranie“.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samomeranie {
    /// Čaká sa, kým bude systém pripravený (skúška pamäte, meranie karty).
    Pripravuje,
    /// Beží: záznamy (minúta, podaril sa zápis?), ďalší záznam pri `pri_s` sekundách hodín „beží“
    /// a poradové číslo merania (známe po prvom zápise). Text sa mení len pri zázname – žiadne prekresľovanie
    /// celej obrazovky každú minútu, ktoré by meranie teploty ovplyvnilo.
    Bezi { zaznamy: Vec<(u32, bool)>, pri_s: u64, poradie: Option<u32>, milniky: Vec<u32> },
    /// Všetko je zapísané: poradové číslo a nosič (napr. „SD karta“).
    Hotove { poradie: u32, nosic: String },
    /// Meranie skončilo, ale zápis sa nepodaril – skúša sa znova; dôvod.
    Nezapisane(String),
}

/// Úsporný režim Ethernetu (EEE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eee {
    /// Zapnutý a aktívny – Ethernet v pokoji šetrí energiu.
    Aktivny,
    /// Vypnutý, lebo spojenie vypadávalo (R111).
    VypnutyPreVypadky,
}

/// Rozmery rozloženia odvodené z rozlíšenia (spoločné pre celú obrazovku aj pre samotné hodiny).
struct Rozmery {
    /// Veľkosť bežného písma.
    v: f32,
    /// Hrúbka čiary písma.
    h: f32,
    /// Okraj vľavo a vpravo, okraj hore a dole.
    m: f32,
    mv: f32,
    /// Pravý okraj textu.
    pravy: f32,
}

impl Rozmery {
    fn pre(sirka: usize, vyska: usize) -> Rozmery {
        let v = (vyska as f32 / 30.0).clamp(10.0, 64.0);
        let h = (v * 0.11).max(1.3);
        // okraj aj pre televízory, ktoré obraz na krajoch orezávajú (asi 4–5 %)
        let m = (sirka as f32 * 0.045).max(1.2 * v).max(6.0);
        let mv = (vyska as f32 * 0.04).max(v).max(6.0);
        Rozmery { v, h, m, mv, pravy: sirka as f32 - m }
    }

    /// Základná čiara riadku s modelom (hlavička).
    fn hlavicka(&self) -> f32 {
        self.mv + 1.6 * self.v
    }

    /// Základná čiara hodín.
    fn hodiny_y(&self) -> f32 {
        self.hlavicka() + 2.2 * self.v
    }

    /// Písmo hodín.
    fn hodiny_pismo(&self) -> (f32, f32) {
        (0.65 * self.v, self.h * 0.7)
    }
}

/// Text hodín od zapnutia (nie skutočný čas – RPi nemá hodiny reálneho času).
fn hodiny_text(bezi_s: u64, j: Jazyk) -> String {
    let (hod, min, sek) = (bezi_s / 3600, bezi_s / 60 % 60, bezi_s % 60);
    let cas = if hod > 0 { format!("{hod}:{min:02}:{sek:02}") } else { format!("{min}:{sek:02}") };
    format!("{} {cas}", j.t("beží", "běží", "up"))
}

/// Najširšie hodiny, s ktorými rozloženie počíta (aj po 999 hodinách sa nič neprekryje).
fn hodiny_sirka(pismo: &Pismo, r: &Rozmery, j: Jazyk) -> f32 {
    let (velkost, hrubka) = r.hodiny_pismo();
    pismo.sirka(&hodiny_text(999 * 3600 + 59 * 60 + 59, j), velkost, hrubka)
}

/// Obdĺžnik hodín (x, y, šírka, výška) v bodoch – len ten sa prekresľuje každú sekundu (R109).
fn hodiny_obdlznik(pismo: &Pismo, r: &Rozmery, j: Jazyk) -> (i32, i32, i32, i32) {
    let w = hodiny_sirka(pismo, r, j) + 0.3 * r.v;
    let y0 = r.hodiny_y() - 0.72 * r.v;
    let y1 = r.hodiny_y() + 0.25 * r.v;
    ((r.pravy - w).floor() as i32, y0.floor() as i32, w.ceil() as i32 + 2, (y1 - y0).ceil() as i32 + 1)
}

/// Prekreslí len hodiny na už nakreslenej obrazovke (R109) – výsledok je bod po bode rovnaký ako celé
/// nové `nakresli`, ale stojí zlomok práce. Vráti prekreslený obdĺžnik (x, y, šírka, výška).
pub fn nakresli_hodiny(pismo: &Pismo, p: &mut Platno, bezi_s: u64, j: Jazyk) -> (usize, usize, usize, usize) {
    let r = Rozmery::pre(p.sirka, p.vyska);
    let (x, y, w, v) = hodiny_obdlznik(pismo, &r, j);
    p.vypln(x, y, w, v, POZADIE);
    kresli_hodiny(pismo, p, &r, bezi_s, j);
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = ((x + w).max(0) as usize).min(p.sirka);
    let y1 = ((y + v).max(0) as usize).min(p.vyska);
    (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

fn kresli_hodiny(pismo: &Pismo, p: &mut Platno, r: &Rozmery, bezi_s: u64, j: Jazyk) {
    let (velkost, hrubka) = r.hodiny_pismo();
    let text = hodiny_text(bezi_s, j);
    let w = pismo.sirka(&text, velkost, hrubka);
    p.text(pismo, &text, r.pravy - w, r.hodiny_y(), velkost, hrubka, TLMENY);
}

/// Veľkosť písma, pri ktorej sa text zmestí do šírky `max` (najviac zmenšené na `najmenej` × pôvodná).
fn zmesti(pismo: &Pismo, text: &str, max: f32, velkost: f32, hrubka: f32, najmenej: f32) -> (f32, f32) {
    let w = pismo.sirka(text, velkost, hrubka);
    if w <= max || w <= 0.0 {
        return (velkost, hrubka);
    }
    // malá rezerva – zaokrúhľovanie šírky by inak text na konci skrátilo
    let k = (0.98 * max / w).max(najmenej);
    (velkost * k, (hrubka * k).max(1.0))
}

/// Teplota slovom a farbou.
fn teplota_slovom(t: f64, ciel: f64, strop: f64, j: Jazyk) -> (&'static str, Farba) {
    if t < ciel - 5.0 {
        (j.t("chladné", "chladné", "cool"), ZELENA)
    } else if t < strop - 3.0 {
        (
            j.t(
                "teplé – regulátor drží teplotu",
                "teplé – regulátor drží teplotu",
                "warm – the governor holds it",
            ),
            JANTAROVA,
        )
    } else {
        (
            j.t(
                "horúce – regulátor znižuje výkon",
                "horké – regulátor snižuje výkon",
                "hot – the governor lowers power",
            ),
            CERVENA,
        )
    }
}

fn velkost_disku(bajtov: u64, j: Jazyk) -> String {
    if bajtov >= 1_000_000_000 {
        format!("{} GB", j.cislo(bajtov as f64 / 1e9, 1))
    } else {
        format!("{} MB", bajtov / 1_000_000)
    }
}

/// Nakreslí stavovú obrazovku v rozlíšení `sirka` × `vyska`.
pub fn nakresli(pismo: &Pismo, sirka: usize, vyska: usize, s: &Stav, j: Jazyk) -> Platno {
    nakresli_s_opisom(pismo, sirka, vyska, s, j).0
}

/// Nakreslí stavovú obrazovku a vráti aj jej text po riadkoch – pre správu samomerania (R122). Text obsahuje
/// všetky riadky, aj tie, ktoré sa na malú obrazovku nezmestili.
pub fn nakresli_s_opisom(
    pismo: &Pismo,
    sirka: usize,
    vyska: usize,
    s: &Stav,
    j: Jazyk,
) -> (Platno, Vec<String>) {
    let opis = RefCell::new(Vec::new());
    let mut p = Platno::nove(sirka, vyska, POZADIE);
    let r = Rozmery::pre(sirka, vyska);
    let Rozmery { v, h, m, mv, pravy } = r;

    // hlavička: ORCHIDEA OS a model
    let mut y = r.hlavicka();
    let koniec = p.text(pismo, "ORCHIDEA OS", m, y, 1.6 * v, (v * 0.17).max(2.0), ZELENA);
    let model = pismo.skrat(s.model, pravy - koniec - 2.0 * v, 0.8 * v, h * 0.8);
    let w = pismo.sirka(&model, 0.8 * v, h * 0.8);
    p.text(pismo, &model, pravy - w, y, 0.8 * v, h * 0.8, TLMENY);
    // verzia karty pod modelom – aby bolo hneď vidieť, ktorá karta beží
    let verzia = format!("{} {}", j.t("verzia", "verze", "version"), s.verzia);
    opis.borrow_mut().push(format!("ORCHIDEA OS · {} · {verzia} · {}", s.model, hodiny_text(s.bezi_s, j)));
    let wv = pismo.sirka(&verzia, 0.65 * v, h * 0.7);
    p.text(pismo, &verzia, pravy - wv, y + 1.2 * v, 0.65 * v, h * 0.7, TLMENY);
    // hodiny od zapnutia – každú sekundu sa prekreslí len ich obdĺžnik (R109)
    kresli_hodiny(pismo, &mut p, &r, s.bezi_s, j);
    // meno zariadenia nesmie zasahovať pod verziu a hodiny vpravo – radšej sa zmenší (R116)
    let vpravo = wv.max(hodiny_sirka(pismo, &r, j) + 0.3 * v) + v;
    let sirka_mena = (pravy - vpravo - m).max(4.0 * v);

    // meno zariadenia
    y += 3.0 * v;
    match s.meno {
        Some(meno) => {
            opis.borrow_mut().push(format!("{}: {meno}", j.t("meno", "jméno", "name")));
            let (velkost, hrubka) = zmesti(pismo, meno, sirka_mena, 2.2 * v, v * 0.16, 0.5);
            let meno = pismo.skrat(meno, sirka_mena, velkost, hrubka);
            p.text(pismo, &meno, m, y, velkost, hrubka, TEXT);
        }
        None => {
            let t = j.t(
                "bez mena – meno nastavíte v ORCHIDEA Zapisovači",
                "bez jména – jméno nastavíte v ORCHIDEA Zapisovači",
                "no name – set it with ORCHIDEA Zapisovač",
            );
            opis.borrow_mut().push(t.to_string());
            let (velkost, hrubka) = zmesti(pismo, t, sirka_mena, v, h, 0.6);
            let t = pismo.skrat(t, sirka_mena, velkost, hrubka);
            p.text(pismo, &t, m, y - 0.6 * v, velkost, hrubka, TLMENY);
        }
    }
    y += 1.3 * v;
    p.vypln(m as i32, y as i32, (pravy - m) as i32, (v / 12.0).max(1.0) as i32, CIARA);

    // riadky: popis vľavo, hodnota v stĺpci
    let popisy = [
        j.t("Teplota", "Teplota", "Temperature"),
        j.t("Napájanie", "Napájení", "Power"),
        j.t("Obrazovka", "Obrazovka", "Display"),
        j.t("Vstupy", "Vstupy", "Input"),
        j.t("Sieť", "Síť", "Network"),
        j.t("Úložisko", "Úložiště", "Storage"),
        "USB",
        j.t("Doska", "Deska", "Board"),
        j.t("Procesor", "Procesor", "Processor"),
        j.t("Pamäť", "Paměť", "Memory"),
        j.t("Jadro", "Jádro", "Kernel"),
        j.t("Meranie", "Měření", "Self-test"),
        j.t("Okolie", "Okolí", "Ambient"),
        j.t("Chlad", "Chlad", "Cooling"),
    ];
    let stlpec = m + popisy.iter().map(|t| pismo.sirka(t, v, h)).fold(0.0, f32::max) + 1.5 * v;
    // riadkov je podľa počtu sietí a úložísk – rozostup sa prispôsobí, aby sa zmestili všetky
    let hlasenia = s.hlasenia.len().min(2);
    let riadkov = 8
        + s.supis.siet.len().clamp(1, 2)
        + s.supis.ulozisko.len().clamp(1, 2)
        + hlasenia
        + usize::from(s.samomeranie.is_some())
        + usize::from(s.okolie.is_some())
        + usize::from(s.chlad.is_some());
    // posledný riadok musí ostať nad pätou (tri riadky malého písma)
    let spodok = vyska as f32 - mv - 4.0 * v;
    let volne = spodok - (y + 0.4 * v);
    let krok = (volne / riadkov as f32).clamp(1.3 * v, 1.8 * v);
    // dlhý riadok sa najprv zmenší (najviac na 70 %), až potom skráti – nič dôležité sa neodreže (R116)
    let riadok = |p: &mut Platno, y: &mut f32, popis: &str, casti: &[(&str, Farba)]| {
        // text pre správu (R122) – zapíše sa aj riadok, ktorý sa už na obrazovku nezmestí
        let text: Vec<&str> = casti.iter().map(|(t, _)| t.trim()).filter(|t| !t.is_empty()).collect();
        opis.borrow_mut().push(format!("  {popis:<11} {}", text.join(" ")));
        *y += krok;
        if *y > spodok + 0.5 {
            return;
        }
        p.text(pismo, popis, m, *y, v, h, TLMENY);
        let medzera = pismo.posun(' ', v, h);
        let spolu: f32 =
            casti.iter().filter(|(t, _)| !t.is_empty()).map(|(t, _)| pismo.sirka(t, v, h) + medzera).sum();
        let k = if spolu > pravy - stlpec { (0.98 * (pravy - stlpec) / spolu).max(0.7) } else { 1.0 };
        let (vk, hk) = (v * k, (h * k).max(1.0));
        let mut x = stlpec;
        for (text, farba) in casti {
            if text.is_empty() {
                continue;
            }
            let text = pismo.skrat(text, pravy - x, vk, hk);
            x = p.text(pismo, &text, x, *y, vk, hk, *farba) + pismo.posun(' ', vk, hk);
        }
    };
    y += 0.4 * v;

    let teplota = match s.teplota {
        Some(t) => {
            let (slovo, farba) = teplota_slovom(t, s.ciel, s.strop, j);
            (format!("{} °C", j.cislo(t, 1)), slovo, farba)
        }
        None => ("–".to_string(), j.t("bez snímača", "bez čidla", "no sensor"), TLMENY),
    };
    let vykon = if s.vykon >= 0.995 {
        j.t("plný výkon", "plný výkon", "full power").to_string()
    } else {
        format!("{} {:.0} %", j.t("výkon", "výkon", "power"), s.vykon * 100.0)
    };
    riadok(
        &mut p,
        &mut y,
        popisy[0],
        &[(&teplota.0, TEXT), ("–", TLMENY), (teplota.1, teplota.2), ("·", TLMENY), (&vykon, TLMENY)],
    );
    // R123, R124, R131: okolie hneď pod teplotou – pri zdroji tepla s radou, čo urobiť
    if let Some(o) = s.okolie {
        let casti = riadok_okolia(o, s.uspora, j);
        let casti: Vec<(&str, Farba)> = casti.iter().map(|(t, f)| (t.as_str(), *f)).collect();
        riadok(&mut p, &mut y, popisy[12], &casti);
    }
    // R158–R161: správca chladu TV krabičky
    if let Some(c) = s.chlad {
        let pokus = c.pokus.as_deref().map(|t| format!("· {t}")).unwrap_or_default();
        riadok(&mut p, &mut y, popisy[13], &[(&c.text, TEXT), (&pokus, ZELENA)]);
    }
    // R122: samomeranie hneď pod teplotou – je to to, na čo sa pri skúške čaká
    if let Some(m) = s.samomeranie {
        let casti = riadok_merania(m, j);
        let casti: Vec<(&str, Farba)> = casti.iter().map(|(t, f)| (t.as_str(), *f)).collect();
        riadok(&mut p, &mut y, popisy[11], &casti);
    }
    let napajanie = match (s.podpatie, s.podpatie_bolo) {
        (Some(true), _) => (
            j.t(
                "slabé – použite oficiálny zdroj Raspberry Pi",
                "slabé – použijte oficiální zdroj Raspberry Pi",
                "weak – use the official Raspberry Pi power supply",
            ),
            CERVENA,
        ),
        (Some(false), true) => (
            j.t(
                "chvíľami slabé – skúste iný zdroj alebo kratší kábel",
                "chvílemi slabé – zkuste jiný zdroj nebo kratší kabel",
                "sometimes weak – try another supply or a shorter cable",
            ),
            JANTAROVA,
        ),
        (Some(false), false) => (j.t("v poriadku", "v pořádku", "OK"), ZELENA),
        // doska bez snímača podpätia (Odroid) – nie je to chyba
        (None, _) => (j.t("nemeria sa", "neměří se", "not measured"), TLMENY),
    };
    // zdroj podľa firmvéru (RPi 5): 5 A = plný prúd pre USB, 3 A = USB obmedzené na 0,6 A
    let zdroj = match (s.poda.zdroj_ma, s.poda.usb_vysoky_prud) {
        (Some(ma), Some(false)) => format!(
            "· {} {} A – {}",
            j.t("zdroj", "zdroj", "supply"),
            j.cislo(ma as f64 / 1000.0, 0),
            j.t("USB len 0,6 A", "USB jen 0,6 A", "USB limited to 0.6 A")
        ),
        (Some(ma), _) => {
            format!("· {} {} A", j.t("zdroj", "zdroj", "supply"), j.cislo(ma as f64 / 1000.0, 0))
        }
        _ => String::new(),
    };
    riadok(&mut p, &mut y, popisy[1], &[napajanie, (&zdroj, TLMENY)]);

    // PÔDA: čip, jadrá a takt; pamäť, voľná pamäť, stlačená pamäť a úseky NUMA
    let mut procesor = s.poda.cip.clone().unwrap_or_else(|| "–".into());
    if s.poda.jadier > 0 {
        procesor += &format!(" · {}× {}", s.poda.jadier, s.poda.jadro.unwrap_or(""));
    }
    if let Some(mhz) = s.poda.max_mhz {
        procesor += &format!(" · {} GHz", j.cislo(mhz as f64 / 1000.0, 1));
    }
    // strážca pravdy (R100): jadrá overené známym výsledkom
    let (overenie, farba_overenia) = if !s.poda.chybne_jadra.is_empty() {
        (
            format!(
                "· {} {}",
                j.t("chybné jadro", "chybné jádro", "faulty core"),
                s.poda.chybne_jadra.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
            ),
            CERVENA,
        )
    } else if s.poda.overene_jadra > 0 {
        (format!("· {} ✓", j.t("overené", "ověřeno", "verified")), ZELENA)
    } else {
        (String::new(), TLMENY)
    };
    riadok(&mut p, &mut y, popisy[8], &[(procesor.trim_end(), TEXT), (&overenie, farba_overenia)]);
    let mb = |m: u64| {
        if m >= 1024 {
            format!("{} GB", j.cislo(m as f64 / 1024.0, if m.is_multiple_of(1024) { 0 } else { 1 }))
        } else {
            format!("{m} MB")
        }
    };
    let pamat = if s.poda.pamat_kb > 0 { mb(s.poda.ram_mb()) } else { "–".into() };
    let mut doplnok = String::new();
    if s.poda.dostupna_kb > 0 {
        doplnok += &format!("· {} {} ", j.t("voľné", "volné", "free"), mb(s.poda.dostupna_kb / 1024));
    }
    if let Some(z) = s.zram_mb {
        doplnok += &format!("· {} {} ", j.t("+ stlačená", "+ komprimovaná", "+ compressed"), mb(z));
    }
    if s.poda.uzly_numa > 1 {
        doplnok += &format!("· NUMA {}", s.poda.uzly_numa);
    }
    // R120: skúška pamäte pri štarte
    let (overena, farba_pamate) = match s.test_pamate {
        Some(Ok(_)) => (format!("· {} ✓", j.t("overená", "ověřena", "verified")), ZELENA),
        Some(Err(m)) => {
            (format!("· {} {m} MB", j.t("CHYBA PAMÄTE na", "CHYBA PAMĚTI na", "MEMORY ERROR at")), CERVENA)
        }
        None => (String::new(), TLMENY),
    };
    riadok(
        &mut p,
        &mut y,
        popisy[9],
        &[(&pamat, TEXT), (&overena, farba_pamate), (doplnok.trim_end(), TLMENY)],
    );

    let obrazovka = s
        .supis
        .obrazovky
        .first()
        .map(|o| o.split_once(' ').map_or(o.as_str(), |(_, r)| r).replace('x', "×"))
        .unwrap_or_else(|| "–".into());
    // R115: monitor vie viac, než jadro nastavilo (alebo neposlal svoj popis EDID) – povie sa to otvorene
    let rozmer = obrazovka.replace('×', "x");
    let monitor = match &s.supis.monitor {
        Some(mon) if mon.edid_bajtov == 0 => (
            format!(
                "· {}",
                j.t(
                    "monitor neposlal svoj popis (EDID)",
                    "monitor neposlal svůj popis (EDID)",
                    "no EDID from monitor"
                )
            ),
            JANTAROVA,
        ),
        Some(mon) => match &mon.najlepsi_rezim {
            Some(najlepsi) if !rozmer.contains(najlepsi.as_str()) => (
                format!(
                    "· {} {}",
                    j.t("monitor vie", "monitor umí", "monitor supports"),
                    najlepsi.replace('x', "×")
                ),
                JANTAROVA,
            ),
            _ => (String::new(), TLMENY),
        },
        None => (String::new(), TLMENY),
    };
    riadok(&mut p, &mut y, popisy[2], &[(&obrazovka, TEXT), (&monitor.0, monitor.1)]);

    let ma = |d: DruhVstupu| s.supis.vstupy.iter().any(|i| i.druh == d);
    let znak = |b: bool| if b { ("✓", ZELENA) } else { ("–", TLMENY) };
    let (k, mys, dotyk) =
        (znak(ma(DruhVstupu::Klavesnica)), znak(ma(DruhVstupu::Mys)), znak(ma(DruhVstupu::Dotyk)));
    // diaľkový ovládač (R158) – na TV krabičke namiesto dotyku (ten tam nebýva)
    let ovladac = ma(DruhVstupu::Ovladac);
    let (posledny, posledny_znak) = if ovladac {
        (j.t("  ovládač", "  ovladač", "  remote"), znak(true))
    } else {
        (j.t("  dotyk", "  dotyk", "  touch"), dotyk)
    };
    riadok(
        &mut p,
        &mut y,
        popisy[3],
        &[
            (j.t("klávesnica", "klávesnice", "keyboard"), TEXT),
            k,
            (j.t("  myš", "  myš", "  mouse"), TEXT),
            mys,
            (posledny, TEXT),
            posledny_znak,
        ],
    );

    if s.supis.siet.is_empty() {
        riadok(&mut p, &mut y, popisy[4], &[("–", TLMENY)]);
    }
    for (i, r) in s.supis.siet.iter().take(2).enumerate() {
        let (stav, farba) = match (r.bezdrotove, r.spojenie) {
            (true, _) => (
                j.t(
                    "Wi-Fi – zatiaľ nepodporované",
                    "Wi-Fi – zatím nepodporováno",
                    "Wi-Fi – not supported yet",
                ),
                TLMENY,
            ),
            (false, _) if s.siet_spi.contains(&r.meno) => (
                j.t(
                    "kábel pripojený · čip siete spí, kým sieť netreba",
                    "kabel připojen · čip sítě spí, dokud síť není potřeba",
                    "cable connected · network chip sleeps until needed",
                ),
                TLMENY,
            ),
            (false, Some(true)) => (j.t("kábel pripojený", "kabel připojen", "cable connected"), ZELENA),
            (false, Some(false)) => (j.t("bez kábla", "bez kabelu", "no cable"), TLMENY),
            // R120: rozhranie sa nedalo zapnúť – čip siete (PHY) neodpovedá
            (false, None) if s.siet_chyby.contains(&r.meno) => {
                (j.t("čip siete neodpovedá", "čip sítě neodpovídá", "network chip not responding"), CERVENA)
            }
            (false, None) => (j.t("vypnuté", "vypnuto", "off"), TLMENY),
        };
        // R120: dohodnutá rýchlosť – ukáže aj verziu dosky (X96 Max Plus: 1 Gb/s alebo 100 Mb/s)
        let rychlost = match r.rychlost_mbs {
            Some(v) if r.spojenie == Some(true) && v >= 1000 => {
                format!("· {} Gb/s", j.cislo(v as f64 / 1000.0, 0))
            }
            Some(v) if r.spojenie == Some(true) => format!("· {v} Mb/s"),
            _ => String::new(),
        };
        // čip siete (ovládač jadra) – na TV krabičkách býva iný, ako udáva výrobca
        let rychlost = match &r.cip {
            Some(c) => format!("{rychlost} · {}", c.split(" (").next().unwrap_or(c)),
            None => rychlost,
        };
        let meno = format!("{} –", r.meno);
        let eee = match (i, s.eee, r.spojenie) {
            (0, Some(Eee::Aktivny), Some(true)) => {
                j.t("· úsporný režim", "· úsporný režim", "· energy saving")
            }
            (0, Some(Eee::VypnutyPreVypadky), _) => j.t(
                "· úsporný režim vypnutý (výpadky)",
                "· úsporný režim vypnut (výpadky)",
                "· energy saving off (dropouts)",
            ),
            _ => "",
        };
        riadok(
            &mut p,
            &mut y,
            if i == 0 { popisy[4] } else { "" },
            &[(&meno, TEXT), (stav, farba), (&rychlost, TEXT), (eee, TLMENY)],
        );
    }

    if s.supis.ulozisko.is_empty() {
        riadok(&mut p, &mut y, popisy[5], &[("–", TLMENY)]);
    }
    for (i, u) in s.supis.ulozisko.iter().take(2).enumerate() {
        let druh = if u.meno.starts_with("mmcblk") && u.druh == "MMC" {
            j.t("vstavaná pamäť eMMC", "vestavěná paměť eMMC", "built-in eMMC")
        } else if u.meno.starts_with("mmcblk") && u.druh == "SD" {
            j.t("SD karta", "SD karta", "SD card")
        } else if u.meno.starts_with("mmcblk") {
            j.t("karta SD/eMMC", "karta SD/eMMC", "SD/eMMC card")
        } else if u.meno.starts_with("nvme") {
            "NVMe"
        } else {
            j.t("disk USB", "disk USB", "USB disk")
        };
        let text = format!("{druh} {}", velkost_disku(u.bajtov, j));
        let (rychlost, farba) = if u.meno != s.merana_karta {
            (String::new(), TLMENY)
        } else {
            match s.meranie_karty {
                MeranieKarty::Hotovo(mb, iops) => (
                    format!(
                        "· {} {} MB/s · {} {}/s",
                        j.t("čítanie", "čtení", "read"),
                        j.cislo(*mb, 1),
                        j.t("náhodne", "náhodně", "random 4K"),
                        iops.round()
                    ),
                    TLMENY,
                ),
                MeranieKarty::Caka => {
                    (j.t("· meriam rýchlosť…", "· měřím rychlost…", "· measuring speed…").into(), TLMENY)
                }
                MeranieKarty::Bezi(mb) => (
                    format!(
                        "{} {mb} MB",
                        j.t("· meriam rýchlosť…", "· měřím rychlost…", "· measuring speed…")
                    ),
                    TLMENY,
                ),
                MeranieKarty::Zaseknute(mb) => (
                    format!(
                        "· {} ({mb} MB)",
                        j.t(
                            "karta pri meraní neodpovedá",
                            "karta při měření neodpovídá",
                            "card stopped responding"
                        )
                    ),
                    JANTAROVA,
                ),
                MeranieKarty::Chyba(e) => (
                    format!("· {} {e}", j.t("meranie zlyhalo:", "měření selhalo:", "test failed:")),
                    JANTAROVA,
                ),
            }
        };
        riadok(&mut p, &mut y, if i == 0 { popisy[5] } else { "" }, &[(&text, TEXT), (&rychlost, farba)]);
    }
    let usb = format!("{} {}", j.t("zariadení:", "zařízení:", "devices:"), s.supis.usb.len());
    riadok(&mut p, &mut y, popisy[6], &[(&usb, TEXT)]);
    // prídavná doska alebo skrinka s identifikačným čipom (napr. HAT+); „–“ = firmvér žiadnu nenašiel
    let doska = s.supis.dosky.first().cloned().unwrap_or_else(|| "–".into());
    riadok(&mut p, &mut y, popisy[7], &[(&doska, if s.supis.dosky.is_empty() { TLMENY } else { TEXT })]);
    // R115: posledné hlásenia jadra o chybách zariadení – aby sa dali odfotiť a opraviť
    for (i, hlasenie) in s.hlasenia.iter().rev().take(hlasenia).rev().enumerate() {
        riadok(&mut p, &mut y, if i == 0 { popisy[10] } else { "" }, &[(hlasenie, JANTAROVA)]);
    }

    // päta – počas samomerania (R122) sa napájanie nemá odpájať, kým sa výsledky nezapíšu na kartu
    let meranie_bezi = s.samomeranie.is_some_and(|m| !matches!(m, Samomeranie::Hotove { .. }));
    let (paticka, farba_paticky) = if meranie_bezi {
        (
            j.t(
                "Prebieha meranie – napájanie odpojte až po nápise HOTOVÉ.",
                "Probíhá měření – napájení odpojte až po nápisu HOTOVO.",
                "Self-test running – unplug the power only after DONE.",
            ),
            JANTAROVA,
        )
    } else {
        (
            j.t(
                "Systém beží z pamäte – napájanie môžete kedykoľvek odpojiť.",
                "Systém běží z paměti – napájení můžete kdykoli odpojit.",
                "The system runs from memory – you can unplug the power at any time.",
            ),
            TLMENY,
        )
    };
    let maly = 0.75 * v;
    let yp = vyska as f32 - mv;
    p.text(pismo, &pismo.skrat(paticka, pravy - m, maly, h * 0.8), m, yp, maly, h * 0.8, farba_paticky);
    let co_dalej = j.t(
        "ORCHIDEA OS beží správne. Programy ORCHIDEA pribudnú v ďalšej verzii.",
        "ORCHIDEA OS běží správně. Programy ORCHIDEA přibudou v další verzi.",
        "ORCHIDEA OS is running. ORCHIDEA programs will come in a later version.",
    );
    p.text(
        pismo,
        &pismo.skrat(co_dalej, pravy - m, maly, h * 0.8),
        m,
        yp - 3.0 * maly,
        maly,
        h * 0.8,
        ZELENA,
    );
    let mut info = match s.pripravene_s {
        Some(t) => format!("{} {} s", j.t("Pripravené za", "Připraveno za", "Ready in"), j.cislo(t, 1)),
        None => String::new(),
    };
    if !info.is_empty() {
        info += " · ";
    }
    info += &format!(
        "{} {} °C, {} {} °C",
        j.t("cieľ", "cíl", "target"),
        j.cislo(s.ciel, 0),
        j.t("strop", "strop", "limit"),
        j.cislo(s.strop, 0)
    );

    if s.strazny_casovac {
        info += &format!(" · {} ✓", j.t("strážny časovač", "hlídací časovač", "watchdog"));
    }
    p.text(pismo, &pismo.skrat(&info, pravy - m, maly, h * 0.8), m, yp - 1.5 * maly, maly, h * 0.8, TLMENY);
    let mut opis = opis.into_inner();
    opis.extend([co_dalej.to_string(), info, paticka.to_string()]);
    (p, opis)
}

/// Časti riadku „Okolie“ (R123, R124, R131) s farbami.
fn riadok_okolia(o: &orchidea_teplo::okolie::StavOkolia, uspora: bool, j: Jazyk) -> Vec<(String, Farba)> {
    use orchidea_teplo::okolie::{Prostredie, Zdroj};
    let mut v = Vec::new();
    match (o.okolie, o.studeny_start) {
        (Some(t), _) => {
            let odkial = match o.zdroj {
                Some(Zdroj::Start) => j.t(" (zo štartu)", " (ze startu)", " (at start)"),
                Some(Zdroj::Izba) => j.t(" (teplomer)", " (teploměr)", " (thermometer)"),
                _ => "",
            };
            v.push((format!("≈ {} °C{odkial}", j.cislo(t, 0)), TEXT));
        }
        (None, Some(false)) => v.push((
            j.t(
                "neznáme – čip bol pri štarte už teplý",
                "neznámé – čip byl při startu už teplý",
                "unknown – chip already warm at start",
            )
            .into(),
            TLMENY,
        )),
        (None, _) => v.push((j.t("zisťuje sa…", "zjišťuje se…", "estimating…").into(), TLMENY)),
    }
    match o.prostredie {
        Prostredie::Utek => v.push((
            j.t(
                "– teplota rastie čoraz rýchlejšie: presuňte zariadenie alebo ho vypnite",
                "– teplota roste čím dál rychleji: přesuňte zařízení nebo ho vypněte",
                "– temperature rising faster and faster: move or switch off the device",
            )
            .into(),
            CERVENA,
        )),
        Prostredie::ZdrojTepla(n) => v.push((
            format!(
                "(+{} °C) {}",
                j.cislo(n, 0),
                j.t(
                    "– zdroj tepla: nie nad radiátor, 10 cm odstup, zvislo",
                    "– zdroj tepla: ne nad radiátor, 10 cm odstup, svisle",
                    "– heat source: not above a radiator, 10 cm away, upright",
                )
            ),
            JANTAROVA,
        )),
        Prostredie::Teple => v.push((
            j.t(
                "– teplé okolie: nie nad radiátor, 10 cm odstup, zvislo",
                "– teplé okolí: ne nad radiátor, 10 cm odstup, svisle",
                "– warm surroundings: not above a radiator, 10 cm away, upright",
            )
            .into(),
            JANTAROVA,
        )),
        Prostredie::Normalne | Prostredie::Nezname => {
            if let Some(sh) = o.samohrev {
                v.push((
                    format!(
                        "· {} +{} °C",
                        j.t("vlastný ohrev", "vlastní ohřev", "self-heating"),
                        j.cislo(sh, 0)
                    ),
                    TLMENY,
                ));
            }
            if let Some(tau) = o.tau_s {
                v.push((format!("· τ {} min", j.cislo(tau / 60.0, 0)), TLMENY));
            }
        }
    }
    if uspora {
        v.push((j.t("· úspora", "· úspora", "· saving").into(), TLMENY));
    }
    v
}

/// Časti riadku „Meranie“ (R122) s farbami.
fn riadok_merania(m: &Samomeranie, j: Jazyk) -> Vec<(String, Farba)> {
    match m {
        Samomeranie::Pripravuje => vec![(
            j.t(
                "začne, keď bude systém pripravený…",
                "začne, až bude systém připraven…",
                "starts when the system is ready…",
            )
            .into(),
            TLMENY,
        )],
        Samomeranie::Bezi { zaznamy, pri_s, poradie, milniky } => {
            let mut v = Vec::new();
            if let Some(c) = poradie {
                v.push((format!("č. {c:02} ·"), TEXT));
            }
            for &minuta in milniky {
                let (znak, farba) = match zaznamy.iter().find(|z| z.0 == minuta) {
                    Some((_, true)) => ("✓", ZELENA),
                    Some((_, false)) => ("✗", JANTAROVA),
                    None => ("…", TLMENY),
                };
                v.push((format!("{minuta} {znak}"), farba));
            }
            // písmo STONKA nemá úvodzovky – čas sa píše ako hodiny vpravo hore („beží 12:05“)
            v.push((
                format!(
                    "· {}: {}",
                    j.t("ďalší záznam", "další záznam", "next record"),
                    hodiny_text(*pri_s, j)
                ),
                TLMENY,
            ));
            v
        }
        Samomeranie::Hotove { poradie, nosic } => vec![
            (
                j.t(
                    "HOTOVÉ – kartu môžete vybrať",
                    "HOTOVO – kartu můžete vyjmout",
                    "DONE – you can remove the card",
                )
                .into(),
                ZELENA,
            ),
            (format!("· č. {poradie:02} · {nosic}"), TLMENY),
        ],
        Samomeranie::Nezapisane(dovod) => vec![
            (format!("{} {dovod}", j.t("nedá sa zapísať:", "nelze zapsat:", "cannot write:")), CERVENA),
            (
                j.t("· zasuňte USB kľúč (FAT32)", "· zasuňte USB disk (FAT32)", "· insert a FAT32 USB stick")
                    .into(),
                JANTAROVA,
            ),
        ],
    }
}

/// Uloženie bodu v pamäti obrazovky (z popisu jadra `fb_var_screeninfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatBodu {
    pub bajtov: usize,
    /// (posun, dĺžka) bitov červenej, zelenej, modrej a priehľadnosti
    pub r: (u32, u32),
    pub g: (u32, u32),
    pub b: (u32, u32),
    pub a: (u32, u32),
}

impl FormatBodu {
    /// Z 32-bitových slov `fb_var_screeninfo` (bitov na bod je slovo 6, farby od slova 8).
    pub fn z_jadra(v: &[u32; 40]) -> Option<FormatBodu> {
        let bajtov = (v[6] / 8) as usize;
        let f =
            FormatBodu { bajtov, r: (v[8], v[9]), g: (v[11], v[12]), b: (v[14], v[15]), a: (v[17], v[18]) };
        let dobre = |(o, l): (u32, u32)| l <= 8 && o + l <= v[6];
        (matches!(bajtov, 2..=4)
            && f.r.1 > 0
            && f.g.1 > 0
            && f.b.1 > 0
            && [f.r, f.g, f.b, f.a].iter().all(|&k| dobre(k)))
        .then_some(f)
    }

    /// Prevedie riadok plátna (0xRRGGBB) na bajty pamäte obrazovky.
    pub fn preved(&self, riadok: &[Farba], ciel: &mut Vec<u8>) {
        ciel.clear();
        let kanal = |c: u32, (o, l): (u32, u32)| -> u32 { if l == 0 { 0 } else { (c >> (8 - l)) << o } };
        let alfa = if self.a.1 > 0 { ((1u32 << self.a.1) - 1) << self.a.0 } else { 0 };
        for &f in riadok {
            let hodnota = kanal((f >> 16) & 0xFF, self.r)
                | kanal((f >> 8) & 0xFF, self.g)
                | kanal(f & 0xFF, self.b)
                | alfa;
            ciel.extend_from_slice(&hodnota.to_le_bytes()[..self.bajtov]);
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::periferie::{Rozhranie, Ulozisko, Vstup};

    fn supis() -> Supis {
        Supis {
            obrazovky: vec!["fb0 1920x1080".into()],
            monitor: None,
            vstupy: vec![Vstup { druh: DruhVstupu::Klavesnica, meno: "k".into() }],
            siet: vec![Rozhranie {
                meno: "eth0".into(),
                adresa: String::new(),
                bezdrotove: false,
                spojenie: Some(true),
                rychlost_mbs: Some(1000),
                cip: None,
            }],
            ulozisko: vec![Ulozisko {
                druh: "SD".into(),
                meno: "mmcblk0".into(),
                popis: "SD32G".into(),
                bajtov: 31_914_983_424,
                vymenitelne: false,
            }],
            zvuk: vec![],
            usb: vec!["x".into()],
            dosky: vec![],
            podpatie: Some(false),
        }
    }

    fn stav<'a>(supis: &'a Supis, teplota: Option<f64>) -> Stav<'a> {
        Stav {
            model: "Raspberry Pi 3 Model B Plus Rev 1.3",
            meno: Some("dielna-1"),
            teplota,
            ciel: 60.0,
            strop: 70.0,
            supis,
            pripravene_s: Some(2.4),
            verzia: "19. 11. 2026 · f26272c",
            bezi_s: 754,
            meranie_karty: &MeranieKarty::Hotovo(22.4, 1830.0),
            merana_karta: "mmcblk0",
            poda: Box::leak(Box::new(ProfilHardveru {
                cip: Some("BCM2837".into()),
                jadro: Some("Cortex-A53"),
                jadier: 4,
                max_mhz: Some(1400),
                ram_dosky_mb: Some(1024),
                pamat_kb: 948_304,
                dostupna_kb: 870_000,
                ..ProfilHardveru::default()
            })),
            zram_mb: Some(474),
            strazny_casovac: true,
            eee: Some(Eee::Aktivny),
            hlasenia: &[],
            test_pamate: Some(Ok(64)),
            siet_chyby: &[],
            siet_spi: &[],
            samomeranie: None,
            okolie: None,
            uspora: false,
            chlad: None,
            vykon: 1.0,
            podpatie: Some(false),
            podpatie_bolo: false,
        }
    }

    fn podiel_textu(p: &Platno) -> f64 {
        p.body.iter().filter(|&&b| b != POZADIE).count() as f64 / p.body.len() as f64
    }

    #[test]
    fn obrazovka_v_roznych_rozliseniach_a_jazykoch() {
        let pismo = Pismo::stonka();
        let s = supis();
        for (w, h) in [(800, 480), (1024, 600), (1920, 1080), (3840, 2160), (320, 240)] {
            for j in [Jazyk::Sk, Jazyk::Cs, Jazyk::En] {
                let p = nakresli(&pismo, w, h, &stav(&s, Some(45.0)), j);
                let podiel = podiel_textu(&p);
                assert!((0.01..0.4).contains(&podiel), "{w}×{h} {j:?}: podiel textu {podiel}");
            }
        }
    }

    #[test]
    fn teplota_meni_farbu_a_obrazovka_je_rovnaka_pri_rovnakom_stave() {
        let pismo = Pismo::stonka();
        let s = supis();
        let a = nakresli(&pismo, 800, 480, &stav(&s, Some(45.0)), Jazyk::Sk);
        let b = nakresli(&pismo, 800, 480, &stav(&s, Some(45.0)), Jazyk::Sk);
        assert_eq!(a, b);
        let ma = |p: &Platno, f: Farba| p.body.contains(&f);
        assert!(ma(&a, ZELENA));
        let horuce = nakresli(&pismo, 800, 480, &stav(&s, Some(69.0)), Jazyk::Sk);
        assert!(ma(&horuce, CERVENA) && !ma(&a, CERVENA));
        // slabé napájanie je červené aj pri chladnom zariadení
        let mut slabe = stav(&s, Some(45.0));
        slabe.podpatie = Some(true);
        assert!(ma(&nakresli(&pismo, 800, 480, &slabe, Jazyk::Sk), CERVENA));
        assert_ne!(a, nakresli(&pismo, 800, 480, &stav(&s, None), Jazyk::Sk));
        assert_eq!(teplota_slovom(62.0, 60.0, 70.0, Jazyk::En).0, "warm – the governor holds it");
        assert_eq!(Jazyk::Cs.cislo(48.54, 1), "48,5");
        assert_eq!(Jazyk::z_kodu(Some("en")), Jazyk::En);
        assert_eq!(Jazyk::z_kodu(None), Jazyk::Sk);
    }

    #[test]
    fn hodiny_sa_prekreslia_samostatne_rovnako_ako_cela_obrazovka() {
        // R109: prekreslenie len hodín musí dať bod po bode ten istý obraz ako nové kreslenie celej obrazovky
        let pismo = Pismo::stonka();
        let s = supis();
        for (w, h) in [(1920, 1080), (1024, 768), (800, 480), (3840, 2160)] {
            for (a, b) in [(59, 60), (3599, 3600), (754, 755), (0, 360_000)] {
                let mut st = stav(&s, Some(45.0));
                st.bezi_s = a;
                let mut p = nakresli(&pismo, w, h, &st, Jazyk::Sk);
                let (x, y, sw, sv) = nakresli_hodiny(&pismo, &mut p, b, Jazyk::Sk);
                st.bezi_s = b;
                let cela = nakresli(&pismo, w, h, &st, Jazyk::Sk);
                assert!(p == cela, "{w}×{h}: {a} → {b} s");
                assert!(sw > 0 && sv > 0 && x + sw <= w && y + sv <= h);
                // obdĺžnik hodín je malý – zlomok obrazovky
                assert!(sw * sv * 100 < w * h, "{w}×{h}: obdĺžnik {sw}×{sv}");
            }
        }
    }

    #[test]
    fn dlhe_meno_a_riadky_sa_zmestia_aj_na_malej_obrazovke() {
        // R116: Odroid ukázal 1024×768 – meno nesmie zasahovať do hodín, riadky sa zmenšia
        let pismo = Pismo::stonka();
        let mut s = supis();
        s.obrazovky = vec!["fb0 1024x768".into(), "HDMI-A-1".into()];
        s.monitor = Some(crate::periferie::Monitor {
            vystup: "HDMI-A-1".into(),
            edid_bajtov: 256,
            najlepsi_rezim: Some("1920x1080".into()),
        });
        let hlasenia = vec!["mmc0: error -110 whilst initialising SD card".to_string()];
        for meno in [Some("testovanie-teploty"), Some("velmi-dlhe-meno-zariadenia-v-dielni-3"), None] {
            let mut st = stav(&s, Some(45.0));
            st.meno = meno;
            st.model = "Hardkernel ODROID-C4";
            st.hlasenia = &hlasenia;
            let p = nakresli(&pismo, 1024, 768, &st, Jazyk::Sk);
            // v obdĺžniku hodín nie je nič iné než hodiny
            let r = Rozmery::pre(1024, 768);
            let mut len_hodiny = Platno::nove(1024, 768, POZADIE);
            kresli_hodiny(&pismo, &mut len_hodiny, &r, st.bezi_s, Jazyk::Sk);
            let (x, y, w, v) = hodiny_obdlznik(&pismo, &r, Jazyk::Sk);
            for yy in y..y + v {
                for xx in x..x + w {
                    let i = yy as usize * 1024 + xx as usize;
                    assert_eq!(p.body[i], len_hodiny.body[i], "{meno:?}: cudzí bod v hodinách [{xx}, {yy}]");
                }
            }
            // hlásenie jadra a upozornenie na monitor sú jantárové
            assert!(p.body.contains(&JANTAROVA));
        }
        assert_eq!(zmesti(&pismo, "krátky", 1000.0, 20.0, 2.0, 0.7), (20.0, 2.0));
        let (v, _) = zmesti(&pismo, "veľmi dlhý text, ktorý sa nezmestí", 50.0, 20.0, 2.0, 0.7);
        assert_eq!(v, 14.0, "najviac na 70 %");
    }

    #[test]
    fn samomeranie_na_obrazovke_a_v_opise() {
        // R122: riadok „Meranie“, päta počas merania a textový opis všetkých riadkov pre správu
        let pismo = Pismo::stonka();
        let s = supis();
        let mut st = stav(&s, Some(45.0));
        let (_, opis) = nakresli_s_opisom(&pismo, 1920, 1080, &st, Jazyk::Sk);
        assert!(
            opis[0].contains("Raspberry Pi 3 Model B Plus Rev 1.3") && opis[0].contains("f26272c"),
            "{opis:?}"
        );
        assert!(opis.iter().any(|r| r.contains("Teplota") && r.contains("45,0 °C – chladné")), "{opis:?}");
        assert!(opis.iter().any(|r| r.contains("Pamäť") && r.contains("overená ✓")), "{opis:?}");
        assert!(!opis.iter().any(|r| r.contains("Meranie")));
        assert!(opis.last().is_some_and(|r| r.contains("kedykoľvek")));
        let bezi = Samomeranie::Bezi {
            zaznamy: vec![(0, true), (5, false)],
            pri_s: 725,
            poradie: Some(3),
            milniky: crate::samomeranie::MILNIKY_MIN.to_vec(),
        };
        st.samomeranie = Some(&bezi);
        let (p, opis) = nakresli_s_opisom(&pismo, 1920, 1080, &st, Jazyk::Sk);
        let r = opis.iter().find(|r| r.contains("Meranie")).cloned().unwrap_or_default();
        for cast in ["č. 03", "0 ✓", "5 ✗", "10 …", "30 …", "ďalší záznam: beží 12:05"] {
            assert!(r.contains(cast), "chýba „{cast}“: {r}");
        }
        assert!(opis.last().is_some_and(|r| r.contains("až po nápise HOTOVÉ")));
        assert!(p.body.contains(&JANTAROVA));
        let hotove = Samomeranie::Hotove { poradie: 3, nosic: "SD karta".into() };
        st.samomeranie = Some(&hotove);
        let (_, opis) = nakresli_s_opisom(&pismo, 1920, 1080, &st, Jazyk::Sk);
        assert!(
            opis.iter().any(|r| r.contains("HOTOVÉ – kartu môžete vybrať · č. 03 · SD karta")),
            "{opis:?}"
        );
        assert!(opis.last().is_some_and(|r| r.contains("kedykoľvek")));
        // aj na malej obrazovke je kresba v poriadku a opis má aj riadky, ktoré sa na ňu nezmestili
        let chyba = Samomeranie::Nezapisane("karta nie je".into());
        st.samomeranie = Some(&chyba);
        for (w, h) in [(320, 240), (800, 480)] {
            for j in [Jazyk::Sk, Jazyk::Cs, Jazyk::En] {
                let (p, o) = nakresli_s_opisom(&pismo, w, h, &st, j);
                assert!((0.01..0.4).contains(&podiel_textu(&p)), "{w}×{h}");
                assert!(o.iter().any(|r| r.contains("USB")) && o.len() >= 16, "{o:?}");
                assert_eq!(p, nakresli(&pismo, w, h, &st, j));
            }
        }
    }

    #[test]
    fn chlad_na_obrazovke() {
        let pismo = Pismo::stonka();
        let s = supis();
        let mut st = stav(&s, Some(52.0));
        let c = Chlad {
            text: "obraz 1920×1080 · 24 Hz · 16 b · kreslí 960×540 · USB kľúč spí 93 %".into(),
            pokus: Some("pokus 3/6 chladný obraz (12/20 min)".into()),
        };
        st.chlad = Some(&c);
        for (w, h) in [(960, 540), (1920, 1080), (640, 480)] {
            let (_, opis) = nakresli_s_opisom(&pismo, w, h, &st, Jazyk::Sk);
            let r = opis.iter().find(|r| r.contains("Chlad")).cloned().unwrap_or_default();
            assert!(r.contains("24 Hz") && r.contains("pokus 3/6"), "{r}");
        }
    }

    #[test]
    fn okolie_na_obrazovke() {
        use orchidea_teplo::okolie::{Prostredie, StavOkolia, Zdroj};
        let pismo = Pismo::stonka();
        let s = supis();
        let mut st = stav(&s, Some(45.0));
        let o = StavOkolia {
            okolie: Some(22.4),
            zdroj: Some(Zdroj::Model),
            okolie_start: Some(21.5),
            samohrev: Some(17.6),
            tau_s: Some(600.0),
            studeny_start: Some(true),
            sklon: Some(0.0),
            ustalena: Some(45.0),
            najnizsie: Some(21.5),
            ohrev_pri_praci: None,
            prostredie: Prostredie::Normalne,
        };
        st.okolie = Some(&o);
        let (p, opis) = nakresli_s_opisom(&pismo, 1920, 1080, &st, Jazyk::Sk);
        let r = opis.iter().find(|r| r.contains("Okolie")).cloned().unwrap_or_default();
        assert!(r.contains("≈ 22 °C") && r.contains("vlastný ohrev +18 °C") && r.contains("τ 10 min"), "{r}");
        assert!(!p.body.contains(&JANTAROVA));
        let zdroj = StavOkolia { okolie: Some(33.0), prostredie: Prostredie::ZdrojTepla(11.5), ..o };
        st.okolie = Some(&zdroj);
        st.uspora = true;
        let (p, opis) = nakresli_s_opisom(&pismo, 1920, 1080, &st, Jazyk::Sk);
        let r = opis.iter().find(|r| r.contains("Okolie")).cloned().unwrap_or_default();
        assert!(
            r.contains("≈ 33 °C")
                && r.contains("(+12 °C)")
                && r.contains("nie nad radiátor")
                && r.contains("· úspora"),
            "{r}"
        );
        assert!(p.body.contains(&JANTAROVA));
        let utek = StavOkolia { prostredie: Prostredie::Utek, ..zdroj };
        st.okolie = Some(&utek);
        assert!(nakresli(&pismo, 1920, 1080, &st, Jazyk::Sk).body.contains(&CERVENA));
        let teply =
            StavOkolia { okolie: None, studeny_start: Some(false), prostredie: Prostredie::Nezname, ..o };
        st.okolie = Some(&teply);
        let (_, opis) = nakresli_s_opisom(&pismo, 800, 480, &st, Jazyk::Cs);
        assert!(opis.iter().any(|r| r.contains("čip byl při startu už teplý")), "{opis:?}");
    }

    #[test]
    fn format_bodov_obrazovky() {
        // 32 bitov: XRGB8888 (modrá najnižšie)
        let mut v = [0u32; 40];
        v[6] = 32;
        (v[8], v[9], v[11], v[12], v[14], v[15]) = (16, 8, 8, 8, 0, 8);
        let f = FormatBodu::z_jadra(&v).unwrap();
        let mut b = Vec::new();
        f.preved(&[0x123456], &mut b);
        assert_eq!(b, [0x56, 0x34, 0x12, 0x00]);
        // 16 bitov: RGB565
        v[6] = 16;
        (v[8], v[9], v[11], v[12], v[14], v[15]) = (11, 5, 5, 6, 0, 5);
        let f = FormatBodu::z_jadra(&v).unwrap();
        f.preved(&[0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF], &mut b);
        assert_eq!(b, [0xFF, 0xFF, 0x00, 0xF8, 0xE0, 0x07, 0x1F, 0x00]);
        // 32 bitov s priehľadnosťou (ABGR)
        v[6] = 32;
        (v[8], v[9], v[11], v[12], v[14], v[15], v[17], v[18]) = (0, 8, 8, 8, 16, 8, 24, 8);
        let f = FormatBodu::z_jadra(&v).unwrap();
        f.preved(&[0x123456], &mut b);
        assert_eq!(b, [0x12, 0x34, 0x56, 0xFF]);
        // nezmysel sa odmietne
        v[6] = 8;
        assert!(FormatBodu::z_jadra(&v).is_none());
    }
}
