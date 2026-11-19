//! STONKA – vlastné ťahové písmo ORCHIDEE a SKLENÍKA (pracovný názov).
//!
//! Každý znak je opísaný niekoľkými ťahmi (úsečky a oblúky) v jednotkách výšky veľkého písmena, takže
//! písmo sa dá nakresliť **v akejkoľvek veľkosti** – od malej dotykovej obrazovky po 4K – a stále je ostré.
//! Kreslí sa vyhladene: každý bod obrazu dostane krytie podľa vzdialenosti od najbližšieho ťahu (okrúhle
//! konce a spoje vzniknú samy). Nakreslený znak sa uloží do pamäte, opakovaný text sa už len skopíruje.
//!
//! Znaky: veľké a malé písmená A–Z, slovenské a české písmená s diakritikou (dĺžeň, mäkčeň, vokáň,
//! dve bodky, krúžok), číslice, interpunkcia a niekoľko značiek (°, ×, ✓, …). Diakritika sa skladá
//! zo základného písmena a znamienka – rovnaký tvar znamienka na všetkých písmenách.
//!
//! Súradnice: základná čiara 0, výška malého písmena 0,7, výška veľkého písmena 1, spodný dotiahnutý ťah
//! −0,3; os y smeruje nahor. Celé písmo je pôvodný kód projektu, nevychádza zo žiadneho cudzieho písma.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Výška malého písmena (k výške veľkého písmena 1).
pub const VYSKA_MALEHO: f32 = 0.7;
/// Najvyšší bod znakov vrátane diakritiky nad veľkými písmenami.
pub const HORE: f32 = 1.3;
/// Najnižší bod znakov (g, j, p, q, y, zátvorky).
pub const DOLE: f32 = -0.32;

/// Základné znaky: (znak, šírka kresby, ťahy). Ťahy: `M x y` začne lomenú čiaru, `L x y` ju predĺži,
/// `O sx sy rx ry od do` je oblúk elipsy (uhly v stupňoch, proti smeru hodinových ručičiek).
const ZNAKY: &[(char, f32, &str)] = &[
    (' ', 0.3, ""),
    // veľké písmená
    ('A', 0.72, "M 0 0 L 0.36 1 L 0.72 0 M 0.13 0.34 L 0.59 0.34"),
    (
        'B',
        0.62,
        "M 0.38 0.53 L 0 0.53 M 0 0 L 0 1 L 0.36 1 O 0.36 0.765 0.22 0.235 90 -90 M 0 0 L 0.38 0 O 0.38 0.265 0.24 0.265 -90 90",
    ),
    ('C', 0.74, "O 0.42 0.5 0.42 0.5 42 318"),
    ('D', 0.7, "M 0.25 0 L 0 0 L 0 1 L 0.25 1 O 0.25 0.5 0.45 0.5 90 -90"),
    ('E', 0.56, "M 0.56 1 L 0 1 L 0 0 L 0.56 0 M 0 0.52 L 0.48 0.52"),
    ('F', 0.54, "M 0.54 1 L 0 1 L 0 0 M 0 0.52 L 0.46 0.52"),
    ('G', 0.84, "O 0.42 0.5 0.42 0.5 40 360 M 0.84 0.5 L 0.5 0.5"),
    ('H', 0.66, "M 0 0 L 0 1 M 0.66 0 L 0.66 1 M 0 0.52 L 0.66 0.52"),
    ('I', 0.0, "M 0 0 L 0 1"),
    ('J', 0.5, "M 0.5 1 L 0.5 0.3 O 0.25 0.3 0.25 0.3 0 -180"),
    ('K', 0.64, "M 0 0 L 0 1 M 0.6 1 L 0 0.36 M 0.21 0.58 L 0.64 0"),
    ('L', 0.52, "M 0 1 L 0 0 L 0.52 0"),
    ('M', 0.84, "M 0 0 L 0 1 L 0.42 0.28 L 0.84 1 L 0.84 0"),
    ('N', 0.68, "M 0 0 L 0 1 L 0.68 0 L 0.68 1"),
    ('O', 0.88, "O 0.44 0.5 0.44 0.5 0 360"),
    ('P', 0.6, "M 0 0 L 0 1 L 0.34 1 O 0.34 0.73 0.26 0.27 90 -90 M 0.34 0.46 L 0 0.46"),
    ('Q', 0.9, "O 0.44 0.5 0.44 0.5 0 360 M 0.52 0.26 L 0.9 -0.08"),
    ('R', 0.64, "M 0 0 L 0 1 L 0.34 1 O 0.34 0.73 0.26 0.27 90 -90 M 0.34 0.46 L 0 0.46 M 0.3 0.46 L 0.64 0"),
    ('S', 0.6, "O 0.3 0.75 0.28 0.25 25 270 O 0.3 0.25 0.3 0.25 90 -155"),
    ('T', 0.66, "M 0 1 L 0.66 1 M 0.33 1 L 0.33 0"),
    ('U', 0.66, "M 0 1 L 0 0.33 O 0.33 0.33 0.33 0.33 180 360 M 0.66 0.33 L 0.66 1"),
    ('V', 0.7, "M 0 1 L 0.35 0 L 0.7 1"),
    ('W', 0.98, "M 0 1 L 0.25 0 L 0.49 0.72 L 0.73 0 L 0.98 1"),
    ('X', 0.66, "M 0 1 L 0.66 0 M 0.66 1 L 0 0"),
    ('Y', 0.68, "M 0 1 L 0.34 0.48 L 0.68 1 M 0.34 0.48 L 0.34 0"),
    ('Z', 0.6, "M 0 1 L 0.6 1 L 0 0 L 0.6 0"),
    // malé písmená
    ('a', 0.56, "O 0.28 0.35 0.28 0.35 0 360 M 0.56 0.7 L 0.56 0"),
    ('b', 0.56, "M 0 1 L 0 0 O 0.28 0.35 0.28 0.35 0 360"),
    ('c', 0.52, "O 0.3 0.35 0.3 0.35 48 312"),
    ('d', 0.56, "O 0.28 0.35 0.28 0.35 0 360 M 0.56 1 L 0.56 0"),
    ('e', 0.56, "M 0 0.36 L 0.56 0.36 O 0.28 0.35 0.28 0.35 0 318"),
    ('f', 0.43, "M 0.12 0 L 0.12 0.8 O 0.31 0.8 0.19 0.2 180 50 M 0 0.68 L 0.34 0.68"),
    ('g', 0.56, "O 0.28 0.38 0.28 0.32 0 360 M 0.56 0.7 L 0.56 -0.05 O 0.28 -0.05 0.28 0.25 0 -160"),
    ('h', 0.54, "M 0 1 L 0 0 O 0.27 0.43 0.27 0.27 180 0 M 0.54 0.43 L 0.54 0"),
    ('i', 0.0, "M 0 0 L 0 0.7 M 0 0.93 L 0 0.935"),
    ('ı', 0.0, "M 0 0 L 0 0.7"),
    ('j', 0.12, "M 0.12 0.7 L 0.12 -0.1 O -0.06 -0.1 0.18 0.2 0 -125 M 0.12 0.93 L 0.12 0.935"),
    ('k', 0.54, "M 0 1 L 0 0 M 0.5 0.7 L 0 0.26 M 0.17 0.41 L 0.54 0"),
    ('l', 0.0, "M 0 1 L 0 0"),
    (
        'm',
        0.86,
        "M 0 0.7 L 0 0 O 0.215 0.47 0.215 0.23 180 0 M 0.43 0.47 L 0.43 0 O 0.645 0.47 0.215 0.23 180 0 M 0.86 0.47 L 0.86 0",
    ),
    ('n', 0.54, "M 0 0.7 L 0 0 O 0.27 0.43 0.27 0.27 180 0 M 0.54 0.43 L 0.54 0"),
    ('o', 0.62, "O 0.31 0.35 0.31 0.35 0 360"),
    ('p', 0.56, "M 0 0.7 L 0 -0.3 O 0.28 0.35 0.28 0.35 0 360"),
    ('q', 0.56, "O 0.28 0.35 0.28 0.35 0 360 M 0.56 0.7 L 0.56 -0.3"),
    ('r', 0.36, "M 0 0.7 L 0 0 O 0.3 0.42 0.3 0.28 180 80"),
    ('s', 0.46, "O 0.23 0.53 0.21 0.17 25 270 O 0.23 0.18 0.23 0.18 90 -155"),
    ('t', 0.36, "M 0.12 0.92 L 0.12 0.14 O 0.28 0.14 0.16 0.14 180 285 M 0 0.7 L 0.34 0.7"),
    ('u', 0.54, "M 0 0.7 L 0 0.27 O 0.27 0.27 0.27 0.27 180 360 M 0.54 0.7 L 0.54 0"),
    ('v', 0.54, "M 0 0.7 L 0.27 0 L 0.54 0.7"),
    ('w', 0.82, "M 0 0.7 L 0.2 0 L 0.41 0.52 L 0.62 0 L 0.82 0.7"),
    ('x', 0.52, "M 0 0.7 L 0.52 0 M 0.52 0.7 L 0 0"),
    ('y', 0.54, "M 0 0.7 L 0.246 0 M 0.54 0.7 L 0.12 -0.3"),
    ('z', 0.5, "M 0 0.7 L 0.5 0.7 L 0 0 L 0.5 0"),
    // číslice
    ('0', 0.56, "O 0.28 0.5 0.28 0.5 0 360"),
    ('1', 0.56, "M 0.1 0.78 L 0.34 1 L 0.34 0"),
    ('2', 0.56, "O 0.28 0.72 0.27 0.28 160 -40 M 0.487 0.54 L 0 0 L 0.56 0"),
    ('3', 0.56, "O 0.27 0.75 0.25 0.25 150 -90 O 0.27 0.27 0.28 0.27 90 -150"),
    ('4', 0.58, "M 0.42 0 L 0.42 1 L 0 0.3 L 0.58 0.3"),
    ('5', 0.56, "M 0.52 1 L 0.08 1 L 0.06 0.57 O 0.27 0.33 0.29 0.33 135 -150"),
    ('6', 0.56, "M 0.46 1 L 0.07 0.42 O 0.29 0.31 0.27 0.31 0 360"),
    ('7', 0.56, "M 0 1 L 0.56 1 L 0.2 0"),
    ('8', 0.56, "O 0.28 0.765 0.23 0.235 0 360 O 0.28 0.265 0.28 0.265 0 360"),
    ('9', 0.56, "O 0.27 0.69 0.27 0.31 0 360 M 0.49 0.58 L 0.1 0"),
    // interpunkcia a značky
    ('.', 0.0, "M 0 0 L 0 0.005"),
    (',', 0.08, "M 0.08 0.03 L 0 -0.16"),
    (':', 0.0, "M 0 0 L 0 0.005 M 0 0.52 L 0 0.525"),
    (';', 0.08, "M 0.08 0.03 L 0 -0.16 M 0.08 0.52 L 0.08 0.525"),
    ('-', 0.36, "M 0 0.4 L 0.36 0.4"),
    ('–', 0.56, "M 0 0.4 L 0.56 0.4"),
    ('+', 0.56, "M 0 0.4 L 0.56 0.4 M 0.28 0.12 L 0.28 0.68"),
    ('=', 0.56, "M 0 0.28 L 0.56 0.28 M 0 0.52 L 0.56 0.52"),
    ('/', 0.44, "M 0 -0.1 L 0.44 1.05"),
    ('(', 0.22, "O 0.5 0.4 0.5 0.7 125 235"),
    (')', 0.22, "O -0.28 0.4 0.5 0.7 55 -55"),
    ('[', 0.2, "M 0.2 1.02 L 0 1.02 L 0 -0.18 L 0.2 -0.18"),
    (']', 0.2, "M 0 1.02 L 0.2 1.02 L 0.2 -0.18 L 0 -0.18"),
    ('<', 0.5, "M 0.5 0.68 L 0 0.4 L 0.5 0.12"),
    ('>', 0.5, "M 0 0.68 L 0.5 0.4 L 0 0.12"),
    ('%', 0.72, "M 0.06 0 L 0.66 1 O 0.15 0.82 0.13 0.17 0 360 O 0.57 0.18 0.13 0.17 0 360"),
    ('°', 0.3, "O 0.15 0.85 0.13 0.13 0 360"),
    ('!', 0.0, "M 0 1 L 0 0.3 M 0 0 L 0 0.005"),
    ('?', 0.5, "O 0.25 0.74 0.25 0.26 160 -70 M 0.336 0.496 L 0.25 0.4 L 0.25 0.27 M 0.25 0 L 0.25 0.005"),
    ('"', 0.2, "M 0 1 L 0 0.76 M 0.2 1 L 0.2 0.76"),
    ('\'', 0.0, "M 0 1 L 0 0.76"),
    ('_', 0.56, "M 0 -0.15 L 0.56 -0.15"),
    ('×', 0.46, "M 0 0.17 L 0.46 0.63 M 0.46 0.17 L 0 0.63"),
    ('·', 0.0, "M 0 0.42 L 0 0.425"),
    ('•', 0.3, "O 0.15 0.4 0.07 0.07 0 360"),
    ('*', 0.44, "M 0.22 1 L 0.22 0.56 M 0.02 0.89 L 0.42 0.67 M 0.02 0.67 L 0.42 0.89"),
    ('#', 0.66, "M 0.2 0 L 0.3 1 M 0.42 0 L 0.52 1 M 0.02 0.33 L 0.62 0.33 M 0.06 0.67 L 0.66 0.67"),
    ('…', 0.56, "M 0 0 L 0 0.005 M 0.28 0 L 0.28 0.005 M 0.56 0 L 0.56 0.005"),
    ('✓', 0.62, "M 0 0.36 L 0.22 0.08 L 0.62 0.72"),
];

/// Znamienka diakritiky (súradnice voči kotve znamienka).
const AKUT: &str = "M -0.06 0 L 0.1 0.17";
const MAKCEN: &str = "M -0.14 0.16 L 0 0.03 L 0.14 0.16";
const VOKAN: &str = "M -0.14 0.03 L 0 0.16 L 0.14 0.03";
const BODKY: &str = "M -0.13 0.08 L -0.13 0.085 M 0.13 0.08 L 0.13 0.085";
const KRUZOK: &str = "O 0 0.09 0.08 0.08 0 360";
const APOSTROF: &str = "M 0.03 0.2 L -0.02 0";

/// Kotva znamienka nad malým písmenom, nad vysokým písmenom (b, d, l …) a nad veľkým písmenom.
const NAD_MALYM: f32 = 0.84;
const NAD_VYSOKYM: f32 = 1.12;

/// Písmeno s diakritikou: (znak, základ, znamienko, x kotvy – `None` = stred, y kotvy, šírka navyše).
type Skladane = (char, char, &'static str, Option<f32>, f32, f32);

const SKLADANE: &[Skladane] = &[
    ('á', 'a', AKUT, None, NAD_MALYM, 0.0),
    ('ä', 'a', BODKY, None, NAD_MALYM, 0.0),
    ('č', 'c', MAKCEN, None, NAD_MALYM, 0.0),
    ('ď', 'd', APOSTROF, Some(0.7), 0.8, 0.14),
    ('é', 'e', AKUT, None, NAD_MALYM, 0.0),
    ('ě', 'e', MAKCEN, None, NAD_MALYM, 0.0),
    ('í', 'ı', AKUT, Some(0.0), NAD_MALYM, 0.0),
    ('ĺ', 'l', AKUT, Some(0.04), NAD_VYSOKYM, 0.04),
    ('ľ', 'l', APOSTROF, Some(0.16), 0.8, 0.16),
    ('ň', 'n', MAKCEN, None, NAD_MALYM, 0.0),
    ('ó', 'o', AKUT, None, NAD_MALYM, 0.0),
    ('ô', 'o', VOKAN, None, NAD_MALYM, 0.0),
    ('ö', 'o', BODKY, None, NAD_MALYM, 0.0),
    ('ŕ', 'r', AKUT, Some(0.18), NAD_MALYM, 0.0),
    ('ř', 'r', MAKCEN, Some(0.18), NAD_MALYM, 0.0),
    ('š', 's', MAKCEN, None, NAD_MALYM, 0.0),
    ('ť', 't', APOSTROF, Some(0.3), 0.76, 0.0),
    ('ú', 'u', AKUT, None, NAD_MALYM, 0.0),
    ('ů', 'u', KRUZOK, None, NAD_MALYM, 0.0),
    ('ü', 'u', BODKY, None, NAD_MALYM, 0.0),
    ('ý', 'y', AKUT, None, NAD_MALYM, 0.0),
    ('ž', 'z', MAKCEN, None, NAD_MALYM, 0.0),
    ('Á', 'A', AKUT, None, NAD_VYSOKYM, 0.0),
    ('Ä', 'A', BODKY, None, NAD_VYSOKYM, 0.0),
    ('Č', 'C', MAKCEN, None, NAD_VYSOKYM, 0.0),
    ('Ď', 'D', MAKCEN, Some(0.32), NAD_VYSOKYM, 0.0),
    ('É', 'E', AKUT, None, NAD_VYSOKYM, 0.0),
    ('Ě', 'E', MAKCEN, None, NAD_VYSOKYM, 0.0),
    ('Í', 'I', AKUT, Some(0.0), NAD_VYSOKYM, 0.0),
    ('Ĺ', 'L', AKUT, Some(0.1), NAD_VYSOKYM, 0.0),
    ('Ľ', 'L', APOSTROF, Some(0.24), 0.8, 0.0),
    ('Ň', 'N', MAKCEN, None, NAD_VYSOKYM, 0.0),
    ('Ó', 'O', AKUT, None, NAD_VYSOKYM, 0.0),
    ('Ô', 'O', VOKAN, None, NAD_VYSOKYM, 0.0),
    ('Ö', 'O', BODKY, None, NAD_VYSOKYM, 0.0),
    ('Ŕ', 'R', AKUT, Some(0.3), NAD_VYSOKYM, 0.0),
    ('Ř', 'R', MAKCEN, Some(0.3), NAD_VYSOKYM, 0.0),
    ('Š', 'S', MAKCEN, None, NAD_VYSOKYM, 0.0),
    ('Ť', 'T', MAKCEN, None, NAD_VYSOKYM, 0.0),
    ('Ú', 'U', AKUT, None, NAD_VYSOKYM, 0.0),
    ('Ů', 'U', KRUZOK, None, NAD_VYSOKYM, 0.0),
    ('Ü', 'U', BODKY, None, NAD_VYSOKYM, 0.0),
    ('Ý', 'Y', AKUT, None, NAD_VYSOKYM, 0.0),
    ('Ž', 'Z', MAKCEN, None, NAD_VYSOKYM, 0.0),
];

type Bod = [f32; 2];

/// Znak: šírka kresby a ťahy (lomené čiary) v jednotkách výšky veľkého písmena.
#[derive(Debug, Clone)]
struct Znak {
    sirka: f32,
    tahy: Vec<Vec<Bod>>,
}

/// Prevedie zápis ťahov na lomené čiary; oblúky sa rozložia na krátke úsečky.
fn tahy(zapis: &str, posun: Bod) -> Vec<Vec<Bod>> {
    let t: Vec<&str> = zapis.split_whitespace().collect();
    let cislo = |i: usize| -> f32 { t.get(i).and_then(|s| s.parse().ok()).unwrap_or(0.0) };
    let mut out: Vec<Vec<Bod>> = Vec::new();
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            "M" => {
                out.push(vec![[cislo(i + 1) + posun[0], cislo(i + 2) + posun[1]]]);
                i += 3;
            }
            "L" => {
                let b = [cislo(i + 1) + posun[0], cislo(i + 2) + posun[1]];
                match out.last_mut() {
                    Some(c) => c.push(b),
                    None => out.push(vec![b]),
                }
                i += 3;
            }
            "O" => {
                let (sx, sy, rx, ry) = (cislo(i + 1), cislo(i + 2), cislo(i + 3), cislo(i + 4));
                let (od, po) = (cislo(i + 5), cislo(i + 6));
                let n = (((po - od).abs() / 7.5).ceil() as usize).max(2);
                out.push(
                    (0..=n)
                        .map(|k| {
                            let u = (od + (po - od) * k as f32 / n as f32).to_radians();
                            [sx + rx * u.cos() + posun[0], sy + ry * u.sin() + posun[1]]
                        })
                        .collect(),
                );
                i += 7;
            }
            _ => i += 1,
        }
    }
    out
}

/// Vykreslený znak: krytie 0–255, počiatok znaku (ľavý koniec základnej čiary) je v bode (`ox`, `oy`).
#[derive(Debug)]
struct Maska {
    sirka: usize,
    vyska: usize,
    ox: i32,
    oy: i32,
    kryt: Vec<u8>,
}

/// Písmo STONKA s pamäťou vykreslených znakov.
#[derive(Debug)]
pub struct Pismo {
    znaky: HashMap<char, Znak>,
    pamat: RefCell<HashMap<(char, u32, u32), Rc<Maska>>>,
}

impl Default for Pismo {
    fn default() -> Self {
        Pismo::stonka()
    }
}

impl Pismo {
    /// Písmo STONKA so všetkými znakmi.
    pub fn stonka() -> Pismo {
        let mut znaky: HashMap<char, Znak> =
            ZNAKY.iter().map(|&(c, sirka, z)| (c, Znak { sirka, tahy: tahy(z, [0.0, 0.0]) })).collect();
        for &(c, zaklad, znamienko, x, y, navyse) in SKLADANE {
            let z = &znaky[&zaklad];
            let kotva = [x.unwrap_or(z.sirka / 2.0), y];
            let mut tahy_znaku = z.tahy.clone();
            tahy_znaku.extend(tahy(znamienko, kotva));
            let sirka = z.sirka + navyse;
            znaky.insert(c, Znak { sirka, tahy: tahy_znaku });
        }
        Pismo { znaky, pamat: RefCell::new(HashMap::new()) }
    }

    /// Má písmo tento znak? (Neznámy znak sa nakreslí ako „?“.)
    pub fn ma(&self, c: char) -> bool {
        self.znaky.contains_key(&c)
    }

    fn znak(&self, c: char) -> &Znak {
        self.znaky.get(&c).unwrap_or(&self.znaky[&'?'])
    }

    /// Posun pera za znakom (v bodoch obrazu) pri výške veľkého písmena `velkost` a hrúbke ťahu `hrubka`.
    pub fn posun(&self, c: char, velkost: f32, hrubka: f32) -> f32 {
        self.znak(c).sirka * velkost + hrubka + 0.14 * velkost
    }

    /// Šírka textu v bodoch obrazu.
    pub fn sirka(&self, text: &str, velkost: f32, hrubka: f32) -> f32 {
        let s: f32 = text.chars().map(|c| self.posun(c, velkost, hrubka)).sum();
        if text.is_empty() { 0.0 } else { s - 0.14 * velkost }
    }

    /// Skráti text tak, aby sa zmestil do šírky `max` (na konci pridá „…“).
    pub fn skrat(&self, text: &str, max: f32, velkost: f32, hrubka: f32) -> String {
        if self.sirka(text, velkost, hrubka) <= max {
            return text.to_string();
        }
        let tri_bodky = self.posun('…', velkost, hrubka);
        let mut s = String::new();
        let mut w = 0.0;
        for c in text.chars() {
            let p = self.posun(c, velkost, hrubka);
            if w + p + tri_bodky > max {
                break;
            }
            w += p;
            s.push(c);
        }
        s.trim_end().to_string() + "…"
    }

    fn maska(&self, c: char, velkost: f32, hrubka: f32) -> Rc<Maska> {
        let kluc = (c, velkost.to_bits(), hrubka.to_bits());
        if let Some(m) = self.pamat.borrow().get(&kluc) {
            return m.clone();
        }
        let m = Rc::new(vykresli(self.znak(c), velkost, hrubka));
        self.pamat.borrow_mut().insert(kluc, m.clone());
        m
    }

    /// Počet znakov v pamäti (na skúšky a meranie).
    pub fn v_pamati(&self) -> usize {
        self.pamat.borrow().len()
    }
}

/// Vzdialenosť bodu od úsečky.
fn vzdialenost(p: Bod, a: Bod, b: Bod) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let d2 = dx * dx + dy * dy;
    let t = if d2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / d2).clamp(0.0, 1.0) } else { 0.0 };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (x * x + y * y).sqrt()
}

fn vykresli(z: &Znak, velkost: f32, hrubka: f32) -> Maska {
    let r = hrubka / 2.0;
    let body = z.tahy.iter().flatten();
    let (mut x0, mut x1, mut y0, mut y1) = (0.0f32, z.sirka, 0.0f32, 0.0f32);
    for b in body {
        x0 = x0.min(b[0]);
        x1 = x1.max(b[0]);
        y0 = y0.min(b[1]);
        y1 = y1.max(b[1]);
    }
    let ox = (-x0 * velkost + r + 1.0).ceil() as i32;
    let oy = (y1 * velkost + r + 1.0).ceil() as i32;
    let sirka = (ox as f32 + x1 * velkost + r + 1.0).ceil() as usize;
    let vyska = (oy as f32 - y0 * velkost + r + 1.0).ceil() as usize;
    // úsečky v bodoch masky (os y nadol)
    let mut usecky: Vec<(Bod, Bod)> = Vec::new();
    for tah in &z.tahy {
        let m: Vec<Bod> =
            tah.iter().map(|b| [ox as f32 + b[0] * velkost, oy as f32 - b[1] * velkost]).collect();
        if m.len() == 1 {
            usecky.push((m[0], m[0]));
        }
        usecky.extend(m.windows(2).map(|w| (w[0], w[1])));
    }
    let mut kryt = vec![0u8; sirka * vyska];
    for (a, b) in &usecky {
        // len body v okolí úsečky
        let bx0 = (a[0].min(b[0]) - r - 1.0).floor().max(0.0) as usize;
        let bx1 = ((a[0].max(b[0]) + r + 1.0).ceil() as usize).min(sirka);
        let by0 = (a[1].min(b[1]) - r - 1.0).floor().max(0.0) as usize;
        let by1 = ((a[1].max(b[1]) + r + 1.0).ceil() as usize).min(vyska);
        for y in by0..by1 {
            for x in bx0..bx1 {
                let d = vzdialenost([x as f32 + 0.5, y as f32 + 0.5], *a, *b);
                let k = ((r + 0.5 - d).clamp(0.0, 1.0) * 255.0).round() as u8;
                let bunka = &mut kryt[y * sirka + x];
                *bunka = (*bunka).max(k);
            }
        }
    }
    Maska { sirka, vyska, ox, oy, kryt }
}

/// Farba 0xRRGGBB.
pub type Farba = u32;

/// Zmieša dve farby v pomere `a` (0–255 = 0–100 % farby `f`).
pub fn zmiesaj(p: Farba, f: Farba, a: u8) -> Farba {
    if a == 255 {
        return f;
    }
    let a = a as u32;
    let kanal = |s: u32| -> u32 {
        let (x, y) = ((p >> s) & 0xFF, (f >> s) & 0xFF);
        ((x * (255 - a) + y * a + 127) / 255) << s
    };
    kanal(16) | kanal(8) | kanal(0)
}

/// Plátno: obraz v pamäti (0xRRGGBB po riadkoch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platno {
    pub sirka: usize,
    pub vyska: usize,
    pub body: Vec<Farba>,
}

impl Platno {
    pub fn nove(sirka: usize, vyska: usize, farba: Farba) -> Platno {
        Platno { sirka, vyska, body: vec![farba; sirka * vyska] }
    }

    /// Vyplní obdĺžnik (orezaný na plátno).
    pub fn vypln(&mut self, x: i32, y: i32, w: i32, h: i32, farba: Farba) {
        let (x0, y0) = (x.max(0) as usize, y.max(0) as usize);
        let x1 = ((x + w).max(0) as usize).min(self.sirka);
        let y1 = ((y + h).max(0) as usize).min(self.vyska);
        for r in y0..y1 {
            if x0 < x1 {
                self.body[r * self.sirka + x0..r * self.sirka + x1].fill(farba);
            }
        }
    }

    /// Nakreslí text; `x` je začiatok, `zakladna` je základná čiara. Vráti koniec textu.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        pismo: &Pismo,
        text: &str,
        x: f32,
        zakladna: f32,
        velkost: f32,
        hrubka: f32,
        farba: Farba,
    ) -> f32 {
        let mut pero = x;
        let y = zakladna.round() as i32;
        for c in text.chars() {
            if c != ' ' {
                let m = pismo.maska(c, velkost, hrubka);
                let (dx, dy) = (pero.round() as i32 - m.ox, y - m.oy);
                for my in 0..m.vyska {
                    let py = dy + my as i32;
                    if py < 0 || py >= self.vyska as i32 {
                        continue;
                    }
                    for mx in 0..m.sirka {
                        let a = m.kryt[my * m.sirka + mx];
                        let px = dx + mx as i32;
                        if a == 0 || px < 0 || px >= self.sirka as i32 {
                            continue;
                        }
                        let i = py as usize * self.sirka + px as usize;
                        self.body[i] = zmiesaj(self.body[i], farba, a);
                    }
                }
            }
            pero += pismo.posun(c, velkost, hrubka);
        }
        pero - 0.14 * velkost
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    const ABECEDA: &str = "AÁÄBCČDĎEÉĚFGHIÍJKLĹĽMNŇOÓÔÖPQRŔŘSŠTŤUÚŮÜVWXYÝZŽ\
        aáäbcčdďeéěfghiíjklĺľmnňoóôöpqrŕřsštťuúůüvwxyýzž0123456789.,:;-–+=/()[]<>%°!?\"'_×·•*#…✓";

    fn plocha(p: &Platno, pozadie: Farba) -> usize {
        p.body.iter().filter(|&&b| b != pozadie).count()
    }

    #[test]
    fn vsetky_znaky_slovenciny_a_cestiny_su_nakreslene() {
        let pismo = Pismo::stonka();
        for c in ABECEDA.chars() {
            assert!(pismo.ma(c), "chýba znak {c}");
            let mut p = Platno::nove(60, 60, 0);
            p.text(&pismo, &c.to_string(), 10.0, 40.0, 24.0, 3.0, 0xFFFFFF);
            assert!(plocha(&p, 0) > 10, "znak {c} je prázdny");
        }
        assert!(!pismo.ma('€'));
    }

    #[test]
    fn diakritika_je_nad_pismenom_a_dolny_tah_pod_ciarou() {
        let pismo = Pismo::stonka();
        // body nad výškou malého písmena (riadky nad základnou čiarou viac ako 0,78 výšky)
        let nad = |c: char, hranica: f32| {
            let mut p = Platno::nove(60, 80, 0);
            p.text(&pismo, &c.to_string(), 10.0, 60.0, 30.0, 3.0, 0xFFFFFF);
            let riadok = (60.0 - hranica * 30.0) as usize;
            (0..riadok)
                .flat_map(|y| (0..60).map(move |x| (x, y)))
                .filter(|&(x, y)| p.body[y * 60 + x] != 0)
                .count()
        };
        assert_eq!(nad('c', 0.8), 0);
        assert!(nad('č', 0.8) > 5);
        assert_eq!(nad('C', 1.07), 0);
        assert!(nad('Č', 1.07) > 5);
        assert!(nad('Ó', 1.07) > 5 && nad('ů', 0.8) > 5 && nad('ô', 0.8) > 5);
        // „g“ a „p“ siahajú pod základnú čiaru, „a“ nie
        let pod = |c: char| {
            let mut p = Platno::nove(60, 80, 0);
            p.text(&pismo, &c.to_string(), 10.0, 40.0, 30.0, 3.0, 0xFFFFFF);
            (44..80)
                .flat_map(|y| (0..60).map(move |x| (x, y)))
                .filter(|&(x, y)| p.body[y * 60 + x] != 0)
                .count()
        };
        assert_eq!(pod('a'), 0);
        assert!(pod('g') > 5 && pod('p') > 5 && pod('j') > 5);
    }

    #[test]
    fn sirka_skratenie_a_pamat() {
        let pismo = Pismo::stonka();
        let a = pismo.sirka("SKLENÍK", 20.0, 2.0);
        assert!(a > 0.0 && pismo.sirka("SKLENÍK pripravený", 20.0, 2.0) > a);
        assert!((pismo.sirka("SKLENÍK", 40.0, 4.0) - 2.0 * a).abs() < 0.5, "šírka rastie s veľkosťou");
        let s = pismo.skrat("Raspberry Pi 3 Model B Plus Rev 1.3", 120.0, 20.0, 2.0);
        assert!(s.ends_with('…') && pismo.sirka(&s, 20.0, 2.0) <= 120.0, "{s}");
        assert_eq!(pismo.skrat("krátke", 500.0, 20.0, 2.0), "krátke");
        let mut p = Platno::nove(400, 60, 0);
        p.text(&pismo, "aaaa", 0.0, 40.0, 20.0, 2.0, 0xFFFFFF);
        assert_eq!(pismo.v_pamati(), 1, "rovnaký znak sa kreslí len raz");
    }

    #[test]
    fn kreslenie_je_rovnake_a_miesa_farby() {
        let pismo = Pismo::stonka();
        let mut a = Platno::nove(200, 50, 0x102018);
        let mut b = a.clone();
        a.text(&pismo, "Teplota 48,5 °C", 4.0, 36.0, 18.0, 2.2, 0xE8F0EA);
        b.text(&Pismo::stonka(), "Teplota 48,5 °C", 4.0, 36.0, 18.0, 2.2, 0xE8F0EA);
        assert_eq!(a, b);
        assert_eq!(zmiesaj(0x000000, 0xFFFFFF, 255), 0xFFFFFF);
        assert_eq!(zmiesaj(0x000000, 0xFFFFFF, 0), 0);
        assert_eq!(zmiesaj(0x000000, 0xFF8000, 128), 0x804000);
        let mut p = Platno::nove(10, 10, 0);
        p.vypln(-5, 8, 100, 100, 7);
        assert_eq!(p.body.iter().filter(|&&x| x == 7).count(), 20);
    }
}
