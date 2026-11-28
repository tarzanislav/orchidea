//! Domovská obrazovka TV krabičky – tri koncepty na výber vlastníkom (zadanie 28. 11. 2026, 10:33): Aplikácie nad
//! Pokračovať, veľké vlastné ikony, počasie (teraz a na pár dní), dátum + čas + meniny, Zdroje, Systém, miesto pre
//! ďalšie moduly; žiadne reklamy ani „odporúčané“. Plus obrazovka prvého spustenia Počasia a šetrič obrazovky.
//!
//! 1 **Panoráma** – informačný pás hore (veľké hodiny, dátum, meniny, počasie), pod ním rady.
//! 2 **Mozaika** – asymetrická mriežka dlaždíc (aplikácie, dnes, počasie, pokračovať, zdroje, systém).
//! 3 **Scéna** – celá plocha patrí zameranému filmu, informácie vľavo, aplikácie a pokračovanie dole.
//!
//! Rozmery sa počítajú zo stupnice TV (`StupnicaTv`) – pri väčšom písme sa rozloženie prispôsobí (menej dlaždíc
//! v rade, ďalšie sú naznačené). Zameraný je vždy práve jeden prvok; v nečinnosti sa nič nehýbe (hodiny raz za
//! minútu).

use crate::obrazky::{Obr, plagat};
use orchidea_pismo::{Farba, zmiesaj};
use orchidea_vzhlad::ikony::Ikona;
use orchidea_vzhlad::ikony_aplikacii::{self, Aplikacia};
use orchidea_vzhlad::kreslic::{Kreslic, SmerPrechodu, zakladna_v_strede};
use orchidea_vzhlad::miery::*;
use orchidea_vzhlad::tema::{CLONA_OBRAZU, TEXT_NA_OBRAZE, Tema};
use orchidea_vzhlad::znak::{self, Znak};

const W: f32 = 1920.0;
const H: f32 = 1080.0;
const XL: f32 = TV_OKRAJ_X;
const XP: f32 = W - TV_OKRAJ_X;
const YH: f32 = TV_OKRAJ_Y;
const YD: f32 = H - TV_OKRAJ_Y;

/// Ukážkové údaje (vymyslené – skutočné dodá systém: čas zo siete, meniny z tabuľky v systéme, počasie).
const CAS: &str = "20:45";
const DATUM: &str = "pondelok 30. novembra";
const MENINY: &str = "Meniny: Ondrej, Andrej · CZ Ondřej";
const MIESTO: &str = "Bratislava";
const ZDROJ_POCASIA: &str = "Údaje: Open-Meteo.com (CC BY 4.0)";

#[derive(Clone, Copy)]
enum Pocasie {
    Slnko,
    Oblak,
    Dazd,
    Sneh,
}

const PREDPOVED: [(&str, Pocasie, &str); 4] = [
    ("ut", Pocasie::Slnko, "4° / 1°"),
    ("st", Pocasie::Oblak, "6° / 2°"),
    ("št", Pocasie::Dazd, "5° / 3°"),
    ("pi", Pocasie::Sneh, "2° / 0°"),
];

const APLIKACIE: [Aplikacia; 7] = [
    Aplikacia::Lyra,
    Aplikacia::ZivaTv,
    Aplikacia::Fotografie,
    Aplikacia::MedialnyServer,
    Aplikacia::WebovyPrehliadac,
    Aplikacia::Subory,
    Aplikacia::Doplnok,
];
const ZDROJE: [Aplikacia; 4] =
    [Aplikacia::UsbKluc, Aplikacia::Karta, Aplikacia::DomacaSiet, Aplikacia::Stiahnute];
const SYSTEM: [Aplikacia; 4] =
    [Aplikacia::Nastavenia, Aplikacia::StavKrabicky, Aplikacia::Aktualizacie, Aplikacia::Vypnut];

const POKRACOVAT: [(&str, &str, f32); 6] = [
    ("Svetlo nad jazerom", "zostáva 40 min", 0.64),
    ("Mestá v daždi", "zostáva 1 h 12 min", 0.42),
    ("Polárna noc", "zostáva 18 min", 0.81),
    ("Medená hodina", "zostáva 1 h 31 min", 0.15),
    ("Hlboký les", "zostáva 55 min", 0.4),
    ("Stanica 9", "zostáva 2 h 01 min", 0.08),
];

/// Text rozdelený na najviac dva riadky tak, aby sa zmestil do šírky `w` (rozdelí sa pri medzere najbližšie stredu).
fn riadky(k: &Kreslic, text: &str, w: f32, rez: Rez) -> Vec<String> {
    if k.sirka(text, rez) <= w {
        return vec![text.to_string()];
    }
    let medzery: Vec<usize> = text.char_indices().filter(|(_, c)| *c == ' ').map(|(i, _)| i).collect();
    let stred = text.len() / 2;
    match medzery.iter().min_by_key(|&&i| (i as i64 - stred as i64).abs()) {
        Some(&i) => vec![text[..i].to_string(), text[i + 1..].to_string()],
        None => vec![text.to_string()],
    }
}

/// Rámik zamerania na TV (4 body, odstup 4 body).
fn zameranie(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let o = TV_ZAMERANIE_ODSTUP + TV_ZAMERANIE_HRUBKA / 2.0;
    k.ram(x - o, y - o, w + 2.0 * o, h + 2.0 * o, r + o, TV_ZAMERANIE_HRUBKA, t.zameranie);
}

/// Ikona aplikácie s popisom pod ňou (v bunke šírky `bunka`, dlhý popis na dva riadky); zameraná je o 5 % väčšia,
/// s rámikom a popisom v plnej farbe textu.
#[allow(clippy::too_many_arguments)]
fn aplikacia(
    k: &mut Kreslic,
    t: &Tema,
    a: Aplikacia,
    x: f32,
    y: f32,
    s: f32,
    bunka: f32,
    rez: Rez,
    zamerana: bool,
    farba_popisu: Option<Farba>,
) {
    let q = if zamerana { TV_ZAMERANE_ZVACSENIE } else { 1.0 };
    let (zs, zx, zy) = (s * q, x - s * (q - 1.0) / 2.0, y - s * (q - 1.0) / 2.0);
    ikony_aplikacii::kresli(k, a, zx, zy, zs, t.tien.max(3));
    if zamerana {
        zameranie(k, t, zx, zy, zs, zs, zs * 0.24);
    }
    let f = farba_popisu.unwrap_or(if zamerana { t.text } else { t.text_druhy });
    // pod zameranou ikonou je rámik – popis o kúsok nižšie, aby sa ho nedotýkal
    let mut zakladna = y + s + rez.velkost + if zamerana { 28.0 } else { 20.0 };
    for r in riadky(k, a.meno(), bunka - 8.0, rez) {
        let sirka = k.sirka(&r, rez);
        k.text(&r, x + (s - sirka) / 2.0, zakladna, rez, f);
        zakladna += rez.velkost + 12.0;
    }
}

/// Karta „Pokračovať“ – snímka z videa, názov na tmavej clone dopočítanej z obrázka, priebeh pozerania.
#[allow(clippy::too_many_arguments)]
fn karta(k: &mut Kreslic, t: &Tema, i: usize, x: f32, y: f32, w: f32, h: f32, rez: Rez, zamerana: bool) {
    let q = if zamerana { TV_ZAMERANE_ZVACSENIE } else { 1.0 };
    let (zw, zh) = (w * q, h * q);
    let (zx, zy) = (x - (zw - w) / 2.0, y - (zh - h) / 2.0);
    let r = R_VELKE;
    if zamerana {
        k.tien(zx, zy, zw, zh, r, 24.0, t.tien.max(4));
    }
    let mut o = plagat(i + 1, (zw * k.mierka) as usize, (zh * k.mierka) as usize, "");
    k.obraz(&o.body, o.w, o.h, zx, zy, zw, zh, r);
    let (nazov, _, postup) = POKRACOVAT[i % POKRACOVAT.len()];
    let zakladna = zy + zh - 30.0;
    let min = if t.vysoky_kontrast { 7.0 } else { 4.5 };
    let nazvy = riadky(k, nazov, zw - 32.0, rez);
    let vyska_textu = nazvy.len() as f32 * (rez.velkost + 12.0) - 12.0;
    let tw = nazvy.iter().map(|r| k.sirka(r, rez)).fold(0.0, f32::max).min(zw - 32.0);
    let a = k.potrebna_clona(
        zx + 16.0,
        zakladna - vyska_textu,
        tw,
        vyska_textu,
        TEXT_NA_OBRAZE,
        CLONA_OBRAZU,
        min,
    );
    let (m, ow) = (k.mierka, o.w);
    let od = ((zakladna - vyska_textu - 8.0 - zy) * m).max(0.0);
    for py in 0..o.h {
        let v = ((od - py as f32) / (48.0 * m)).clamp(0.0, 1.0);
        let kryt = (a as f32 * (1.0 - v * v * (3.0 - 2.0 * v))).round() as u8;
        if kryt > 0 {
            for b in &mut o.body[py * ow..(py + 1) * ow] {
                *b = zmiesaj(*b, CLONA_OBRAZU, kryt);
            }
        }
    }
    k.obraz(&o.body, o.w, o.h, zx, zy, zw, zh, r);
    let mut z = zakladna - vyska_textu + rez.velkost;
    for r in &nazvy {
        if k.sirka(r, rez) <= zw - 32.0 {
            k.text(r, zx + 16.0, z, rez, TEXT_NA_OBRAZE);
        }
        z += rez.velkost + 12.0;
    }
    k.obdlznik(zx + 16.0, zy + zh - 14.0, zw - 32.0, 6.0, 3.0, zmiesaj(0x000000, 0xFFFFFF, 90));
    k.obdlznik(zx + 16.0, zy + zh - 14.0, (zw - 32.0) * postup, 6.0, 3.0, t.akcent);
    if zamerana {
        zameranie(k, t, zx, zy, zw, zh, r);
    }
}

/// Symbol počasia veľkosti `s` (ľavý horný roh).
fn pocasie(k: &mut Kreslic, p: Pocasie, x: f32, y: f32, s: f32, oblak: Farba, slnko: Farba) {
    let q = s / 100.0;
    let oblak_tvar = |k: &mut Kreslic, dy: f32| {
        k.kruh(x + 34.0 * q, y + (58.0 + dy) * q, 18.0 * q, oblak);
        k.kruh(x + 55.0 * q, y + (46.0 + dy) * q, 24.0 * q, oblak);
        k.kruh(x + 74.0 * q, y + (60.0 + dy) * q, 16.0 * q, oblak);
        k.obdlznik(x + 18.0 * q, y + (58.0 + dy) * q, 72.0 * q, 18.0 * q, 9.0 * q, oblak);
    };
    match p {
        Pocasie::Slnko => {
            let (cx, cy) = (x + 50.0 * q, y + 50.0 * q);
            k.kruh(cx, cy, 18.0 * q, slnko);
            for i in 0..8 {
                let u = (i as f32 * 45.0f32).to_radians();
                k.ciara(
                    (cx + 27.0 * q * u.cos(), cy + 27.0 * q * u.sin()),
                    (cx + 37.0 * q * u.cos(), cy + 37.0 * q * u.sin()),
                    6.0 * q,
                    slnko,
                );
            }
        }
        Pocasie::Oblak => {
            k.kruh(x + 66.0 * q, y + 34.0 * q, 16.0 * q, slnko);
            oblak_tvar(k, 4.0);
        }
        Pocasie::Dazd => {
            oblak_tvar(k, -8.0);
            for dx in [32.0, 50.0, 68.0] {
                k.ciara((x + dx * q, y + 80.0 * q), (x + (dx - 5.0) * q, y + 92.0 * q), 5.0 * q, oblak);
            }
        }
        Pocasie::Sneh => {
            oblak_tvar(k, -8.0);
            for dx in [32.0, 50.0, 68.0] {
                k.kruh(x + dx * q, y + 86.0 * q, 4.5 * q, oblak);
            }
        }
    }
}

/// Veľké hodiny – tenší a vyšší rez ako titul (číslice rovnakej šírky, minúta nehýbe zvyškom).
fn rez_hodin(st: &StupnicaTv, nasobok: f32) -> Rez {
    Rez {
        meno: "hodiny",
        velkost: (st.titul.velkost * nasobok / 2.0).round() * 2.0,
        hrubka: st.titul.hrubka * nasobok * 0.8,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// 1 Panoráma

fn koncept_panorama(k: &mut Kreslic, t: &Tema, st: &StupnicaTv) {
    let r = |b: f32| st.rozmer(b);
    let hodiny = rez_hodin(st, 1.5);
    let zaklad_hodin = YH + r(8.0) + hodiny.velkost;
    let sirka_hodin = k.sirka_cisel(CAS, hodiny);
    // rozmery počasia podľa skutočného textu
    let ik = r(80.0);
    let teraz_w = ik + r(16.0) + k.sirka("2 °C", st.nadpis).max(k.sirka("zamračené", st.popis)) + r(40.0);
    let stlpec = PREDPOVED.iter().map(|p| k.sirka(p.2, st.popis)).fold(0.0, f32::max) + r(28.0);
    let sirka_datumu = k.sirka(MENINY, st.popis).max(k.sirka(DATUM, st.text));
    let kx = XL + sirka_hodin + r(40.0);
    // rozloženie hlavičky: (a) hodiny | dátum | počasie so 4 dňami, (b) hodiny | počasie (koľko dní sa zmestí),
    // dátum a meniny pod hodinami – pri väčšom písme
    let siroke = kx + sirka_datumu + r(48.0) + teraz_w + 4.0 * stlpec <= XP;
    let dni = if siroke { 4 } else { (((XP - kx - teraz_w) / stlpec).floor() as usize).clamp(1, 4) };
    let pocasie_w = teraz_w + dni as f32 * stlpec;
    k.cisla(CAS, XL, zaklad_hodin, hodiny, t.text);
    let (px, py) = if siroke { (XP - pocasie_w, YH) } else { (kx, YH) };
    let mut y;
    if siroke {
        k.text(DATUM, kx, zaklad_hodin - hodiny.velkost + st.text.velkost + r(4.0), st.text, t.text);
        k.text(MENINY, kx, zaklad_hodin, st.popis, t.text_druhy);
        y = zaklad_hodin;
    } else {
        y = zaklad_hodin.max(py + 2.0 * st.popis.velkost + r(88.0)) + r(20.0) + st.popis.velkost;
        k.text(&format!("{DATUM} · {MENINY}"), XL, y, st.popis, t.text);
    }
    pocasie(k, Pocasie::Oblak, px, py + r(4.0), ik, t.text, t.hviezda);
    let tz = py + r(8.0) + st.nadpis.velkost;
    k.cisla("2 °C", px + ik + r(16.0), tz, st.nadpis, t.text);
    k.text("zamračené", px + ik + r(16.0), tz + st.popis.velkost + r(16.0), st.popis, t.text_druhy);
    for (i, (den, p, teploty)) in PREDPOVED.iter().take(dni).enumerate() {
        let x = px + teraz_w + i as f32 * stlpec;
        k.text(den, x, py + st.popis.velkost, st.popis, t.text_druhy);
        pocasie(k, *p, x - r(4.0), py + st.popis.velkost + r(8.0), r(52.0), t.text_druhy, t.hviezda);
        k.cisla(teploty, x, py + st.popis.velkost + r(60.0) + st.popis.velkost + r(4.0), st.popis, t.text);
    }
    let zdroj = format!("{MIESTO} · {ZDROJ_POCASIA}");
    let zdroj_y = if siroke {
        py + 2.0 * st.popis.velkost + r(64.0) + r(16.0) + st.popis.velkost + r(8.0)
    } else {
        y + r(16.0) + st.popis.velkost
    };
    if siroke {
        k.text_vpravo(&zdroj, px + pocasie_w, zdroj_y, st.popis, t.text_druhy);
        // Zdroje a Systém v pravom hornom rohu nad počasím nie sú – pri bežnom písme idú vľavo pod hodiny
    } else {
        k.text(&zdroj, XL, zdroj_y, st.popis, t.text_druhy);
    }
    y = y.max(zdroj_y) + r(48.0);
    // Aplikácie
    let ikona = r(144.0);
    let medzera = r(64.0);
    k.text("Aplikácie", XL, y + st.text.velkost, st.text, t.text);
    y += st.text.velkost + r(28.0);
    let v_rade = (((XP - XL) + medzera) / (ikona + medzera)).floor().max(1.0) as usize;
    for (i, a) in APLIKACIE.iter().enumerate().take(v_rade + 1) {
        let x = XL + i as f32 * (ikona + medzera);
        let naznacena = i == v_rade;
        if naznacena {
            // ďalšie aplikácie – len naznačené (popis by bol mimo bezpečnej plochy)
            ikony_aplikacii::kresli(k, *a, x, y, ikona, t.tien);
            k.obdlznik_a(x - 8.0, y - 8.0, W - x + 8.0, ikona + 16.0, 0.0, t.okolie, 150.0);
        } else {
            aplikacia(k, t, *a, x, y, ikona, ikona + medzera, st.popis, i == 0, None);
        }
    }
    y += ikona + 2.0 * st.popis.velkost + r(32.0) + r(40.0);
    // Pokračovať v pozeraní
    if y + st.text.velkost > YD {
        return;
    }
    k.text("Pokračovať v pozeraní", XL, y + st.text.velkost, st.text, t.text);
    y += st.text.velkost + r(28.0);
    let (kw, kh, km) = (r(320.0), r(180.0), r(32.0));
    let mut x = XL;
    let mut i = 0;
    // rad, ktorý siaha pod bezpečnú plochu, je len naznačený (obsah pokračuje nižšie – posun nadol)
    let cely = y + kh <= YD;
    while x < W && y + kh / 2.0 < H {
        if cely && x + kw <= XP {
            karta(k, t, i, x, y, kw, kh, st.popis, false);
        } else {
            let o = plagat(i + 1, (kw * k.mierka) as usize, (kh * k.mierka) as usize, "");
            k.obraz(&o.body, o.w, o.h, x, y, kw, kh, R_VELKE);
        }
        x += kw + km;
        i += 1;
    }
    y += kh + r(56.0);
    // Zdroje a Systém – rad malých ikon s nadpismi skupín; popis ikony sa ukáže pri zameraní (bublina)
    let mala = r(56.0);
    if y + mala > YD {
        return;
    }
    let mut x = XL;
    for (nadpis, polozky) in [("Zdroje", ZDROJE), ("Systém", SYSTEM)] {
        let w = k.text(nadpis, x, zakladna_v_strede(y, mala, st.popis), st.popis, t.text_druhy) - x;
        x += w + r(24.0);
        for a in polozky {
            if x + mala > XP {
                return;
            }
            ikony_aplikacii::kresli(k, a, x, y, mala, 0);
            x += mala + r(20.0);
        }
        x += r(56.0);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// 2 Mozaika

fn dlazdica(
    k: &mut Kreslic,
    t: &Tema,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    nadpis: Option<&str>,
    st: &StupnicaTv,
) {
    k.obdlznik(x, y, w, h, 24.0, t.plocha_vyssia);
    if t.vysoky_kontrast {
        k.ram_dnu(x, y, w, h, 24.0, 2.0, t.okraj_silny);
    }
    if let Some(n) = nadpis {
        k.text(n, x + 32.0, y + 32.0 + st.text.velkost, st.text, t.text);
    }
}

/// Ikona so zvislým usporiadaním v bunke 2 × 2 (malá ikona vľavo, popis vpravo).
fn polozka_vedla(k: &mut Kreslic, t: &Tema, a: Aplikacia, x: f32, y: f32, s: f32, rez: Rez) {
    ikony_aplikacii::kresli(k, a, x, y, s, 0);
    k.text(a.meno(), x + s + 14.0, zakladna_v_strede(y, s, rez), rez, t.text);
}

fn koncept_mozaika(k: &mut Kreslic, t: &Tema, st: &StupnicaTv) {
    let g = 24.0;
    let pravy_w = 640.0;
    let lavy_w = XP - XL - g - pravy_w;
    let px = XL + lavy_w + g;
    // Aplikácie (vľavo hore): 2 rady po 4, posledná bunka = miesto na ďalší modul
    let ah = 520.0;
    dlazdica(k, t, XL, YH, lavy_w, ah, Some("Aplikácie"), st);
    let ikona = 136.0;
    let medzera = (lavy_w - 64.0 - 4.0 * ikona) / 3.0;
    for i in 0..8 {
        let (rr, c) = (i / 4, i % 4);
        let x = XL + 32.0 + c as f32 * (ikona + medzera);
        let y = YH + 96.0 + rr as f32 * (ikona + 100.0);
        if let Some(a) = APLIKACIE.get(i) {
            // bunka popisu len trochu širšia ako ikona – dlhý popis sa zalomí a neprečnieva z dlaždice
            aplikacia(k, t, *a, x, y, ikona, ikona + 72.0, st.popis, false, None);
        } else {
            // miesto na ďalší modul (dlaždica sa pridá v Nastaveniach)
            let o = 8.0;
            k.ram_dnu(x + o, y + o, ikona - 2.0 * o, ikona - 2.0 * o, ikona * 0.22, 3.0, t.okraj_silny);
            k.ikona(Ikona::Plus, x + ikona / 2.0 - 24.0, y + ikona / 2.0 - 24.0, 48.0, t.text_druhy);
            let mut z = y + ikona + st.popis.velkost + 20.0;
            for r in riadky(k, "Pridať modul", ikona + 64.0, st.popis) {
                let sw = k.sirka(&r, st.popis);
                k.text(&r, x + (ikona - sw) / 2.0, z, st.popis, t.text_druhy);
                z += st.popis.velkost + 12.0;
            }
        }
    }
    // Dnes (vpravo hore)
    let dh = 248.0;
    dlazdica(k, t, px, YH, pravy_w, dh, None, st);
    znak::kresli(k, Znak::Orchidea, t, px + pravy_w - 32.0 - 48.0, YH + 32.0, 48.0);
    let hodiny = rez_hodin(st, 1.4);
    k.cisla(CAS, px + 32.0, YH + 32.0 + hodiny.velkost, hodiny, t.text);
    k.text(DATUM, px + 32.0, YH + 32.0 + hodiny.velkost + 24.0 + st.text.velkost, st.text, t.text);
    k.text(MENINY, px + 32.0, YH + dh - 28.0, st.popis, t.text_druhy);
    // Počasie (vpravo, pod Dnes)
    let py = YH + dh + g;
    let ph = ah - dh - g;
    dlazdica(k, t, px, py, pravy_w, ph, None, st);
    pocasie(k, Pocasie::Oblak, px + 24.0, py + 12.0, 88.0, t.text, t.hviezda);
    k.cisla("2 °C", px + 128.0, py + 24.0 + st.nadpis.velkost, st.nadpis, t.text);
    k.text(
        &format!("zamračené · {MIESTO}"),
        px + 128.0,
        py + 24.0 + st.nadpis.velkost + 40.0,
        st.popis,
        t.text_druhy,
    );
    // predpoveď na 3 dni v jednom riadku: symbol, deň a teploty
    for (i, (den, p, teploty)) in PREDPOVED.iter().take(3).enumerate() {
        let x = px + 32.0 + i as f32 * 196.0;
        let y = py + 128.0;
        pocasie(k, *p, x - 4.0, y, 48.0, t.text_druhy, t.hviezda);
        let z = zakladna_v_strede(y, 48.0, st.popis);
        let x2 = k.text(den, x + 52.0, z, st.popis, t.text_druhy) + 10.0;
        k.cisla(&teploty.replace(" / ", "/"), x2, z, st.popis, t.text);
    }
    k.text(ZDROJ_POCASIA, px + 32.0, py + ph - 24.0, st.popis, t.text_druhy);
    // Pokračovať (vľavo dole): veľká karta zameraná + dve menšie
    let ky = YH + ah + g;
    let kh = YD - ky;
    dlazdica(k, t, XL, ky, lavy_w, kh, Some("Pokračovať v pozeraní"), st);
    let (vw, vh) = (560.0, 315.0);
    let vy = ky + 96.0;
    let mw = lavy_w - 64.0 - vw - 32.0;
    let mh = (vh - 24.0) / 2.0;
    karta(k, t, 0, XL + 32.0, vy, vw, vh, st.text, true);
    karta(k, t, 1, XL + 32.0 + vw + 32.0, vy, mw, mh, st.popis, false);
    karta(k, t, 2, XL + 32.0 + vw + 32.0, vy + mh + 24.0, mw, mh, st.popis, false);
    // Zdroje a Systém (vpravo dole) – 2 × 2
    let zh = (kh - g) / 2.0;
    for (j, (nadpis, polozky)) in [("Zdroje", ZDROJE), ("Systém", SYSTEM)].iter().enumerate() {
        let y = ky + j as f32 * (zh + g);
        dlazdica(k, t, px, y, pravy_w, zh, Some(nadpis), st);
        for (i, a) in polozky.iter().enumerate() {
            let (rr, c) = (i / 2, i % 2);
            polozka_vedla(
                k,
                t,
                *a,
                px + 32.0 + c as f32 * 292.0,
                y + 80.0 + rr as f32 * 60.0,
                48.0,
                st.popis,
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// 3 Scéna

fn koncept_scena(k: &mut Kreslic, t: &Tema, st: &StupnicaTv, foto: &Obr) {
    // celá plocha patrí zameranému filmu; text na ňom biely na tmavej clone dopočítanej z obrázka
    k.obraz(&foto.body, foto.w, foto.h, 0.0, 0.0, W, H, 0.0);
    let min = if t.vysoky_kontrast { 7.0 } else { 4.5 };
    let biela_druha = 0xD8DADF;
    let a = k.potrebna_clona(XL, YH, 900.0, 560.0, biela_druha, CLONA_OBRAZU, min);
    k.clona_s_prechodom(0.0, 0.0, 980.0, H, CLONA_OBRAZU, a.max(110), 480.0, SmerPrechodu::Doprava);
    let a = k.potrebna_clona(0.0, 640.0, W, H - 640.0, biela_druha, CLONA_OBRAZU, min);
    k.clona_s_prechodom(0.0, 640.0, W, H - 640.0, CLONA_OBRAZU, a.max(140), 200.0, SmerPrechodu::Nahor);
    // hore: logo vľavo, zdroje a systém ako malé ikony vpravo
    znak::kresli(k, Znak::Orchidea, t, XL, YH, 40.0);
    let mala = 56.0;
    for (i, a) in ZDROJE.iter().chain(SYSTEM.iter()).enumerate() {
        let x = XP - (8 - i) as f32 * (mala + 20.0) + 20.0 - if i < 4 { 32.0 } else { 0.0 };
        ikony_aplikacii::kresli(k, *a, x, YH - 8.0, mala, 0);
    }
    // vľavo: čas, dátum, meniny, počasie
    let hodiny = rez_hodin(st, 1.7);
    let mut y = YH + 72.0 + hodiny.velkost;
    k.cisla(CAS, XL, y, hodiny, TEXT_NA_OBRAZE);
    y += 24.0 + st.text.velkost;
    k.text(DATUM, XL, y, st.text, TEXT_NA_OBRAZE);
    y += 16.0 + st.popis.velkost;
    k.text(MENINY, XL, y, st.popis, biela_druha);
    y += 28.0;
    pocasie(k, Pocasie::Oblak, XL - 8.0, y, 64.0, TEXT_NA_OBRAZE, 0xF2C14E);
    let x = k.cisla("2 °C", XL + 64.0, y + 12.0 + st.text.velkost, st.text, TEXT_NA_OBRAZE) + 24.0;
    k.text("zamračené · ut 4° · st 6° · št 5°", x, y + 12.0 + st.text.velkost, st.popis, biela_druha);
    k.text(ZDROJ_POCASIA, XL + 64.0, y + 60.0 + st.popis.velkost, st.popis, biela_druha);
    // zameraný film
    let (nazov, zostava, _) = POKRACOVAT[0];
    y = 476.0;
    k.text(nazov, XL, y + st.titul.velkost, st.titul, TEXT_NA_OBRAZE);
    k.text(
        &format!("Pokračovať · {zostava} · OK prehrá"),
        XL,
        y + st.titul.velkost + 48.0,
        st.popis,
        biela_druha,
    );
    // Aplikácie (dock) nad Pokračovať
    let ikona = 96.0;
    let ay = 628.0;
    for (i, a) in APLIKACIE.iter().enumerate() {
        let x = XL + 16.0 + i as f32 * (ikona + 100.0);
        aplikacia(k, t, *a, x, ay, ikona, ikona + 100.0, st.popis, false, Some(biela_druha));
    }
    // Pokračovať – menšie karty, prvá zameraná (pozadie scény patrí jej)
    let (kw, kh) = (288.0, 162.0);
    let ky = YD - kh;
    let mut x = XL;
    let mut i = 0;
    while x + kw <= XP {
        karta(k, t, i, x, ky, kw, kh, st.popis, i == 0);
        x += kw + 32.0;
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Prvé spustenie Počasia a šetrič obrazovky

fn prve_spustenie_pocasia(k: &mut Kreslic, t: &Tema, st: &StupnicaTv) {
    koncept_panorama(k, t, st);
    k.clona(0.0, 0.0, W, H, CLONA_OBRAZU, 200);
    let (w, h) = (1120.0, 760.0);
    let (x, y) = ((W - w) / 2.0, (H - h) / 2.0);
    k.tien(x, y, w, h, R_VELKE, 32.0, t.tien.max(6));
    k.obdlznik(x, y, w, h, R_VELKE, t.plocha_vyssia);
    pocasie(k, Pocasie::Oblak, x + 56.0, y + 40.0, 96.0, t.text, t.hviezda);
    k.text("Počasie", x + 176.0, y + 72.0 + st.nadpis.velkost / 2.0, st.nadpis, t.text);
    let riadky = [
        "Na zobrazenie počasia krabička pošle službe Open-Meteo",
        "len približnú polohu mesta (asi 10 km), raz za hodinu,",
        "bez označenia krabičky. Ak miesto nezadáš, počasie",
        "sa nezobrazí – zapnúť ho môžeš neskôr v Nastaveniach.",
    ];
    let mut ty = y + 196.0;
    for r in riadky {
        k.text(r, x + 56.0, ty, st.popis, t.text);
        ty += st.popis.velkost + 18.0;
    }
    // výber mesta
    let fy = ty + 16.0;
    k.obdlznik(x + 56.0, fy, w - 112.0, 72.0, R, t.prvok);
    k.ram_dnu(x + 56.0, fy, w - 112.0, 72.0, R, 2.0, t.okraj_silny);
    k.ikona(Ikona::Hladat, x + 80.0, fy + 18.0, 36.0, t.text_druhy);
    k.text("Mesto: Brati", x + 136.0, zakladna_v_strede(fy, 72.0, st.text), st.text, t.text);
    let mut sx = x + 56.0;
    for m in ["Bratislava", "Bratislava-Petržalka"] {
        let sw = k.sirka(m, st.popis) + 40.0;
        k.obdlznik(sx, fy + 88.0, sw, 56.0, 28.0, t.prvok_pod_mysou);
        k.text(m, sx + 20.0, zakladna_v_strede(fy + 88.0, 56.0, st.popis), st.popis, t.text);
        sx += sw + 16.0;
    }
    // tlačidlá: Zadať miesto (zamerané) · Bez počasia
    let by = y + h - 56.0 - 72.0;
    let s1 = "Zadať miesto";
    let w1 = k.sirka(s1, st.text) + 80.0;
    let q = TV_ZAMERANE_ZVACSENIE;
    let (zw, zh) = (w1 * q, 72.0 * q);
    let (zx, zy) = (x + 56.0 - (zw - w1) / 2.0, by - (zh - 72.0) / 2.0);
    k.obdlznik(zx, zy, zw, zh, R_VELKE, t.akcent);
    k.text(s1, zx + 40.0 * q, zakladna_v_strede(zy, zh, st.text), st.text, t.akcent_text);
    zameranie(k, t, zx, zy, zw, zh, R_VELKE);
    let s2 = "Bez počasia";
    let w2 = k.sirka(s2, st.text) + 80.0;
    let x2 = x + 56.0 + w1 + 32.0;
    k.obdlznik(x2, by, w2, 72.0, R_VELKE, t.prvok_pod_mysou);
    k.text(s2, x2 + 40.0, zakladna_v_strede(by, 72.0, st.text), st.text, t.text);
    k.text(ZDROJ_POCASIA, x + 56.0, fy + 88.0 + 56.0 + 24.0 + st.popis.velkost, st.popis, t.text_druhy);
}

/// Šetrič obrazovky „Nočná orchidea“ – čierna plocha (OLED nesvieti), veľký tlmený kvet, hodiny a dátum. Prekreslí sa
/// raz za minútu a celý obsah sa pri tom posunie o pár bodov (proti vypáleniu); po 30 min televízor do pohotovosti
/// cez CEC (správca systému (KLÍČ)).
fn setric(k: &mut Kreslic, t: &Tema, st: &StupnicaTv) {
    k.obdlznik(0.0, 0.0, W, H, 0.0, 0x000000);
    // kvet: logo zväčšené a stlmené na 22 % (čierny bod okolo ostane čierny)
    let mut kv = Kreslic::novy(640.0, 640.0, k.mierka, 0x000000);
    znak::kresli(&mut kv, Znak::Orchidea, t, 0.0, 0.0, 640.0);
    let (ox, oy) = ((1180.0 * k.mierka) as usize, (240.0 * k.mierka) as usize);
    for r in 0..kv.p.vyska {
        for c in 0..kv.p.sirka {
            let b = kv.p.body[r * kv.p.sirka + c];
            let (py, px) = (oy + r, ox + c);
            if py < k.p.vyska && px < k.p.sirka {
                k.p.body[py * k.p.sirka + px] = zmiesaj(0x000000, b, 56);
            }
        }
    }
    let hodiny = rez_hodin(st, 2.2);
    // tlmené sivé na čiernej (≥ 4,5 : 1); v téme s vysokým kontrastom farby témy (7 : 1)
    let (tlmena, tlmena2) = if t.vysoky_kontrast { (t.text, t.text_druhy) } else { (0x9A9EA6, 0x8A8E96) };
    k.cisla(CAS, 220.0, 520.0, hodiny, tlmena);
    k.text(DATUM, 224.0, 520.0 + 40.0 + st.text.velkost, st.text, tlmena);
    k.text(MENINY, 224.0, 520.0 + 80.0 + st.text.velkost + st.popis.velkost, st.popis, tlmena2);
}

/// Obrazovka domova: koncept `c` (1 Panoráma, 2 Mozaika, 3 Scéna, 'p' prvé spustenie Počasia, 's' šetrič)
/// v téme `t`, mierke kresliča `m` a so stupnicou TV `st`.
pub fn obrazovka(t: &Tema, c: char, m: f32, st: &StupnicaTv, foto: &Obr) -> Kreslic {
    let mut k = Kreslic::novy(W, H, m, t.okolie);
    match c {
        '2' => koncept_mozaika(&mut k, t, st),
        '3' => koncept_scena(&mut k, t, st, foto),
        'p' => prve_spustenie_pocasia(&mut k, t, st),
        's' => setric(&mut k, t, st),
        _ => koncept_panorama(&mut k, t, st),
    }
    k
}
