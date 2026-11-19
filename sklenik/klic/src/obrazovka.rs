//! Stavová obrazovka SKLENÍKA: model, meno zariadenia, teplota vo farbe, výkon a pripojené zariadenia –
//! po slovensky, česky alebo anglicky, vlastným písmom STONKA s diakritikou.
//!
//! Rozloženie sa počíta z rozlíšenia obrazovky (od malej dotykovej 800×480 po 4K). Kreslí sa do plátna
//! v pamäti; na obrazovku sa zapíšu len riadky, ktoré sa zmenili (časť pre Linux v `linux.rs`).

use crate::periferie::{DruhVstupu, Supis};
use orchidea_pismo::{Farba, Pismo, Platno};

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
}

/// Teplota slovom a farbou.
fn teplota_slovom(t: f64, ciel: f64, strop: f64, j: Jazyk) -> (&'static str, Farba) {
    if t < ciel - 5.0 {
        (j.t("chladné", "chladné", "cool"), ZELENA)
    } else if t < strop - 3.0 {
        (
            j.t("teplé – strážca drží teplotu", "teplé – strážce drží teplotu", "warm – the guard holds it"),
            JANTAROVA,
        )
    } else {
        (
            j.t(
                "horúce – strážca znižuje výkon",
                "horké – strážce snižuje výkon",
                "hot – the guard lowers power",
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
    let mut p = Platno::nove(sirka, vyska, POZADIE);
    let v = (vyska as f32 / 30.0).clamp(10.0, 64.0);
    let h = (v * 0.11).max(1.3);
    // okraj aj pre televízory, ktoré obraz na krajoch orezávajú (asi 4–5 %)
    let m = (sirka as f32 * 0.045).max(1.2 * v).max(6.0);
    let mv = (vyska as f32 * 0.04).max(v).max(6.0);
    let pravy = sirka as f32 - m;

    // hlavička: SKLENÍK a model
    let mut y = mv + 1.6 * v;
    let koniec = p.text(pismo, "SKLENÍK", m, y, 1.6 * v, (v * 0.17).max(2.0), ZELENA);
    let model = pismo.skrat(s.model, pravy - koniec - 2.0 * v, 0.8 * v, h * 0.8);
    let w = pismo.sirka(&model, 0.8 * v, h * 0.8);
    p.text(pismo, &model, pravy - w, y, 0.8 * v, h * 0.8, TLMENY);
    // verzia karty pod modelom – aby bolo hneď vidieť, ktorá karta beží
    let verzia = format!("{} {}", j.t("verzia", "verze", "version"), s.verzia);
    let wv = pismo.sirka(&verzia, 0.65 * v, h * 0.7);
    p.text(pismo, &verzia, pravy - wv, y + 1.2 * v, 0.65 * v, h * 0.7, TLMENY);

    // meno zariadenia
    y += 3.0 * v;
    match s.meno {
        Some(meno) => {
            let meno = pismo.skrat(meno, pravy - m, 2.2 * v, v * 0.16);
            p.text(pismo, &meno, m, y, 2.2 * v, v * 0.16, TEXT);
        }
        None => {
            let t = j.t(
                "bez mena – meno nastavíte SADIČOM",
                "bez jména – jméno nastavíte SADIČEM",
                "no name – set it with SADIČ",
            );
            p.text(pismo, t, m, y - 0.6 * v, v, h, TLMENY);
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
    ];
    let stlpec = m + popisy.iter().map(|t| pismo.sirka(t, v, h)).fold(0.0, f32::max) + 1.5 * v;
    let krok = 1.8 * v;
    let riadok = |p: &mut Platno, y: &mut f32, popis: &str, casti: &[(&str, Farba)]| {
        *y += krok;
        if *y > vyska as f32 - mv - 3.4 * v {
            return;
        }
        p.text(pismo, popis, m, *y, v, h, TLMENY);
        let mut x = stlpec;
        for (text, farba) in casti {
            let text = pismo.skrat(text, pravy - x, v, h);
            x = p.text(pismo, &text, x, *y, v, h, *farba) + pismo.posun(' ', v, h);
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
        (None, _) => ("–", TLMENY),
    };
    riadok(&mut p, &mut y, popisy[1], &[napajanie]);

    let obrazovka = s
        .supis
        .obrazovky
        .first()
        .map(|o| o.split_once(' ').map_or(o.as_str(), |(_, r)| r).replace('x', "×"))
        .unwrap_or_else(|| "–".into());
    riadok(&mut p, &mut y, popisy[2], &[(&obrazovka, TEXT)]);

    let ma = |d: DruhVstupu| s.supis.vstupy.iter().any(|i| i.druh == d);
    let znak = |b: bool| if b { ("✓", ZELENA) } else { ("–", TLMENY) };
    let (k, mys, dotyk) =
        (znak(ma(DruhVstupu::Klavesnica)), znak(ma(DruhVstupu::Mys)), znak(ma(DruhVstupu::Dotyk)));
    riadok(
        &mut p,
        &mut y,
        popisy[3],
        &[
            (j.t("klávesnica", "klávesnice", "keyboard"), TEXT),
            k,
            (j.t("  myš", "  myš", "  mouse"), TEXT),
            mys,
            (j.t("  dotyk", "  dotyk", "  touch"), TEXT),
            dotyk,
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
            (false, Some(true)) => (j.t("kábel pripojený", "kabel připojen", "cable connected"), ZELENA),
            (false, Some(false)) => (j.t("bez kábla", "bez kabelu", "no cable"), TLMENY),
            (false, None) => (j.t("vypnuté", "vypnuto", "off"), TLMENY),
        };
        let meno = format!("{} –", r.meno);
        riadok(&mut p, &mut y, if i == 0 { popisy[4] } else { "" }, &[(&meno, TEXT), (stav, farba)]);
    }

    if s.supis.ulozisko.is_empty() {
        riadok(&mut p, &mut y, popisy[5], &[("–", TLMENY)]);
    }
    for (i, u) in s.supis.ulozisko.iter().take(2).enumerate() {
        let druh = if u.meno.starts_with("mmcblk") {
            j.t("karta SD/eMMC", "karta SD/eMMC", "SD/eMMC card")
        } else if u.meno.starts_with("nvme") {
            "NVMe"
        } else {
            j.t("disk USB", "disk USB", "USB disk")
        };
        let text = format!("{druh} {}", velkost_disku(u.bajtov, j));
        riadok(&mut p, &mut y, if i == 0 { popisy[5] } else { "" }, &[(&text, TEXT)]);
    }
    let usb = format!("{} {}", j.t("zariadení:", "zařízení:", "devices:"), s.supis.usb.len());
    riadok(&mut p, &mut y, popisy[6], &[(&usb, TEXT)]);
    // prídavná doska alebo skrinka s identifikačným čipom (napr. HAT+); „–“ = firmvér žiadnu nenašiel
    let doska = s.supis.dosky.first().cloned().unwrap_or_else(|| "–".into());
    riadok(&mut p, &mut y, popisy[7], &[(&doska, if s.supis.dosky.is_empty() { TLMENY } else { TEXT })]);

    // päta
    let paticka = j.t(
        "Systém beží z pamäte – napájanie môžete kedykoľvek odpojiť.",
        "Systém běží z paměti – napájení můžete kdykoli odpojit.",
        "The system runs from memory – you can unplug the power at any time.",
    );
    let maly = 0.75 * v;
    let yp = vyska as f32 - mv;
    p.text(pismo, &pismo.skrat(paticka, pravy - m, maly, h * 0.8), m, yp, maly, h * 0.8, TLMENY);
    let co_dalej = j.t(
        "SKLENÍK beží správne. Programy ORCHIDEA pribudnú v ďalšej verzii.",
        "SKLENÍK běží správně. Programy ORCHIDEA přibudou v další verzi.",
        "SKLENÍK is running. ORCHIDEA programs will come in a later version.",
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

    p.text(pismo, &pismo.skrat(&info, pravy - m, maly, h * 0.8), m, yp - 1.5 * maly, maly, h * 0.8, TLMENY);
    p
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
            vstupy: vec![Vstup { druh: DruhVstupu::Klavesnica, meno: "k".into() }],
            siet: vec![Rozhranie {
                meno: "eth0".into(),
                adresa: String::new(),
                bezdrotove: false,
                spojenie: Some(true),
            }],
            ulozisko: vec![Ulozisko {
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
        assert_eq!(teplota_slovom(62.0, 60.0, 70.0, Jazyk::En).0, "warm – the guard holds it");
        assert_eq!(Jazyk::Cs.cislo(48.54, 1), "48,5");
        assert_eq!(Jazyk::z_kodu(Some("en")), Jazyk::En);
        assert_eq!(Jazyk::z_kodu(None), Jazyk::Sk);
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
