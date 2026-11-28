//! Domovská obrazovka TV krabičky – tri koncepty na výber vlastníkom (zadanie 28. 11. 2026, 10:33): Aplikácie nad
//! Pokračovať, veľké vlastné ikony, počasie (teraz a na pár dní), dátum + čas + meniny, Zdroje, Systém, miesto pre
//! ďalšie moduly; žiadne reklamy ani „odporúčané“. Plus obrazovka prvého spustenia Počasia a šetrič obrazovky.
//!
//! 1 **Panoráma** – informačný pás hore (veľké hodiny, dátum, meniny, počasie), pod ním rady.
//! 2 **Mozaika** – asymetrická mriežka panelov (aplikácie, dnes, počasie, pokračovať, zdroje, systém) – **vybraná
//!   vlastníkom** (28. 11. 2026, 11:47): bez sivej, farebné pozadie (2 varianty), ikony s hĺbkou tieňovaním.
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
    ikony_aplikacii::kresli(k, a, zx, zy, zs, zamerana);
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
            ikony_aplikacii::kresli(k, *a, x, y, ikona, false);
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
            ikony_aplikacii::kresli(k, a, x, y, mala, false);
            x += mala + r(20.0);
        }
        x += r(56.0);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// 2 Mozaika

/// Pozadie domova bez sivej (vlastník 28. 11. 2026: „nepáči sa mi to sivé pozadie“) – statické, vykreslí sa raz.
/// `'f'` farebné: uhlopriečny prechod z okolia témy s nádychom akcentu a mäkká žiara zľava hore;
/// `'a'` atmosféra: zameraný film s nízkou sýtosťou pod clonou dopočítanou z obrázka.
fn pozadie_domova(k: &mut Kreslic, t: &Tema, variant: char, foto: &Obr) {
    let polnoc = t.okolie == 0x000000 && !t.vysoky_kontrast;
    let zaklad = if t.tmava {
        zmiesaj(t.okolie, t.akcent, if polnoc { 10 } else { 34 })
    } else {
        zmiesaj(t.okolie, t.akcent, 26)
    };
    let tmavsi = if t.tmava { zmiesaj(t.okolie, 0x000000, 90) } else { zmiesaj(t.okolie, t.akcent, 8) };
    if variant == 'a' {
        k.obraz(&foto.body, foto.w, foto.h, 0.0, 0.0, W, H, 0.0);
        // nízka sýtosť: polovica k sivej rovnakej svietivosti
        for b in k.p.body.iter_mut() {
            let (r, g, bb) = ((*b >> 16) & 0xFF, (*b >> 8) & 0xFF, *b & 0xFF);
            let y = (r * 54 + g * 183 + bb * 19) >> 8;
            *b = zmiesaj(*b, (y << 16) | (y << 8) | y, 128);
        }
        // rezerva 20 % na sklo panelov, ktoré pozadie trochu zosvetlí (skúška meria výsledok)
        let min = if t.vysoky_kontrast { 7.0 } else { 4.5 } * 1.2;
        let a = k.potrebna_clona(0.0, 0.0, W, H, t.text_druhy, zaklad, min);
        k.clona(0.0, 0.0, W, H, zaklad, a.max(150));
        return;
    }
    k.obdlznik_s_prechodom(0.0, 0.0, W, H, 0.0, zaklad, tmavsi);
    // mäkká žiara akcentu zľava hore (kruhový prechod, slabne do stratena)
    let (m, pw) = (k.mierka, k.p.sirka);
    let (cx, cy, rr) = (W * 0.18, H * 0.05, W * 0.55);
    let ziara = if t.tmava { t.akcent } else { 0xFFFFFF };
    let sila = if polnoc {
        26.0
    } else if t.tmava {
        46.0
    } else {
        120.0
    };
    for py in 0..k.p.vyska {
        for px in 0..pw {
            let d = ((px as f32 / m - cx).hypot(py as f32 / m - cy) / rr).min(1.0);
            let a = (sila * (1.0 - d) * (1.0 - d)).round() as u8;
            if a > 0 {
                let i = py * pw + px;
                k.p.body[i] = zmiesaj(k.p.body[i], ziara, a);
            }
        }
    }
}

/// Panel ako jemné sklo (nie sivá plocha): mierne svetlejšia výplň s nádychom pozadia a svetlý lem hore.
fn panel(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32, h: f32, nadpis: Option<&str>, st: &StupnicaTv) {
    let r = 24.0;
    if t.tmava {
        k.obdlznik_a(x, y, w, h, r, 0xFFFFFF, if t.vysoky_kontrast { 14.0 } else { 12.0 });
    } else {
        k.obdlznik_a(x, y, w, h, r, 0xFFFFFF, 70.0);
    }
    k.lem_svetla(x, y, w, h, r, 1.5, 0xFFFFFF, if t.tmava { 60 } else { 150 });
    if t.vysoky_kontrast {
        k.ram_dnu(x, y, w, h, r, 2.0, t.okraj_silny);
    }
    if let Some(n) = nadpis {
        k.text(n, x + 32.0, y + 32.0 + st.text.velkost, st.text, t.text);
    }
}

/// Ikona s popisom vpravo (Zdroje, Systém).
fn polozka_vedla(k: &mut Kreslic, t: &Tema, a: Aplikacia, x: f32, y: f32, s: f32, rez: Rez) {
    ikony_aplikacii::kresli(k, a, x, y, s, false);
    k.text(a.meno(), x + s + 14.0, zakladna_v_strede(y, s, rez), rez, t.text);
}

/// Rozloženie Mozaiky (body): panely Aplikácie, Dnes, Počasie, Pokračovať, Zdroje, Systém – jeden zdroj pre
/// kreslenie aj skúšku „text vnútri panelov“.
pub fn panely_mozaiky() -> [(f32, f32, f32, f32); 6] {
    let g = 24.0;
    let pravy_w = 640.0;
    let lavy_w = XP - XL - g - pravy_w;
    let px = XL + lavy_w + g;
    let (ah, dh) = (528.0, 248.0);
    let ky = YH + ah + g;
    let kh = YD - ky;
    let zh = (kh - g) / 2.0;
    [
        (XL, YH, lavy_w, ah),
        (px, YH, pravy_w, dh),
        (px, YH + dh + g, pravy_w, ah - dh - g),
        (XL, ky, lavy_w, kh),
        (px, ky, pravy_w, zh),
        (px, ky + zh + g, pravy_w, zh),
    ]
}

const APLIKACIE_MOZAIKY: [Aplikacia; 7] = [
    Aplikacia::Lyra,
    Aplikacia::ZivaTv,
    Aplikacia::Fotografie,
    Aplikacia::MedialnyServer,
    Aplikacia::WebovyPrehliadac,
    Aplikacia::Subory,
    Aplikacia::VsetkyAplikacie,
];

fn koncept_mozaika(k: &mut Kreslic, t: &Tema, st: &StupnicaTv, s_pocasim: bool) {
    let [ap, dn, po, pk, zd, sy] = panely_mozaiky();
    // Aplikácie: 2 rady po 4 – posledná aplikácia „Všetky“, potom miesto na ďalší modul
    panel(k, t, ap.0, ap.1, ap.2, ap.3, Some("Aplikácie"), st);
    // štyri rovnaké bunky v rade – ikona aj popis v strede bunky, popis nikdy nepresiahne panel
    let ikona = 128.0;
    let bunka = (ap.2 - 64.0) / 4.0;
    for i in 0..8 {
        let (rr, c) = (i / 4, i % 4);
        let x = ap.0 + 32.0 + c as f32 * bunka + (bunka - ikona) / 2.0;
        let y = ap.1 + 92.0 + rr as f32 * (ikona + 88.0);
        if let Some(a) = APLIKACIE_MOZAIKY.get(i) {
            aplikacia(k, t, *a, x, y, ikona, bunka, st.popis, false, None);
        } else {
            let o = 8.0;
            k.ram_dnu(x + o, y + o, ikona - 2.0 * o, ikona - 2.0 * o, ikona * 0.22, 3.0, t.okraj_silny);
            k.ikona(Ikona::Plus, x + ikona / 2.0 - 24.0, y + ikona / 2.0 - 24.0, 48.0, t.text_druhy);
            let s = "Pridať modul";
            let sw = k.sirka(s, st.popis);
            k.text(s, x + (ikona - sw) / 2.0, y + ikona + st.popis.velkost + 20.0, st.popis, t.text_druhy);
        }
    }
    // Dnes
    panel(k, t, dn.0, dn.1, dn.2, dn.3, None, st);
    znak::kresli(k, Znak::Orchidea, t, dn.0 + dn.2 - 32.0 - 48.0, dn.1 + 32.0, 48.0);
    let hodiny = rez_hodin(st, 1.4);
    k.cisla(CAS, dn.0 + 32.0, dn.1 + 32.0 + hodiny.velkost, hodiny, t.text);
    k.text(DATUM, dn.0 + 32.0, dn.1 + 32.0 + hodiny.velkost + 24.0 + st.text.velkost, st.text, t.text);
    k.text(MENINY, dn.0 + 32.0, dn.1 + dn.3 - 28.0, st.popis, t.text_druhy);
    // Počasie (len ak je zadané miesto – inak nenápadná výzva; nič sa neposiela)
    panel(k, t, po.0, po.1, po.2, po.3, None, st);
    if s_pocasim {
        pocasie(k, Pocasie::Oblak, po.0 + 24.0, po.1 + 12.0, 88.0, t.text, t.hviezda);
        k.cisla("2 °C", po.0 + 128.0, po.1 + 24.0 + st.nadpis.velkost, st.nadpis, t.text);
        k.text(
            &format!("zamračené · {MIESTO}"),
            po.0 + 128.0,
            po.1 + 24.0 + st.nadpis.velkost + 40.0,
            st.popis,
            t.text_druhy,
        );
        for (i, (den, p, teploty)) in PREDPOVED.iter().take(3).enumerate() {
            let x = po.0 + 32.0 + i as f32 * 196.0;
            let y = po.1 + 128.0;
            pocasie(k, *p, x - 4.0, y, 48.0, t.text_druhy, t.hviezda);
            let z = zakladna_v_strede(y, 48.0, st.popis);
            let x2 = k.text(den, x + 52.0, z, st.popis, t.text_druhy) + 10.0;
            k.cisla(&teploty.replace(" / ", "/"), x2, z, st.popis, t.text);
        }
        k.text("Open-Meteo.com · CC BY 4.0", po.0 + 32.0, po.1 + po.3 - 28.0, st.popis, t.text_druhy);
    } else {
        pocasie(k, Pocasie::Oblak, po.0 + 24.0, po.1 + 12.0, 88.0, t.text_druhy, t.text_druhy);
        k.text("Počasie", po.0 + 128.0, po.1 + 24.0 + st.nadpis.velkost, st.nadpis, t.text);
        k.text(
            "nastaví sa výberom miesta",
            po.0 + 128.0,
            po.1 + 24.0 + st.nadpis.velkost + 40.0,
            st.popis,
            t.text_druhy,
        );
    }
    // Pokračovať: veľká karta zameraná + dve menšie
    panel(k, t, pk.0, pk.1, pk.2, pk.3, Some("Pokračovať v pozeraní"), st);
    let (vw, vh) = (532.0, 300.0);
    let vy = pk.1 + 92.0;
    let mw = pk.2 - 64.0 - vw - 32.0;
    let mh = (vh - 24.0) / 2.0;
    karta(k, t, 0, pk.0 + 32.0, vy, vw, vh, st.text, true);
    karta(k, t, 1, pk.0 + 32.0 + vw + 32.0, vy, mw, mh, st.popis, false);
    karta(k, t, 2, pk.0 + 32.0 + vw + 32.0, vy + mh + 24.0, mw, mh, st.popis, false);
    // Zdroje 2 × 2
    panel(k, t, zd.0, zd.1, zd.2, zd.3, Some("Zdroje"), st);
    for (i, a) in ZDROJE.iter().enumerate() {
        let (rr, c) = (i / 2, i % 2);
        polozka_vedla(
            k,
            t,
            *a,
            zd.0 + 32.0 + c as f32 * 292.0,
            zd.1 + 84.0 + rr as f32 * 56.0,
            44.0,
            st.popis,
        );
    }
    // Systém: Nastavenia, Stav krabičky, Aktualizácie; Vypnúť samostatne vpravo hore – ďaleko od Aktualizácií
    panel(k, t, sy.0, sy.1, sy.2, sy.3, Some("Systém"), st);
    for (i, a) in [Aplikacia::Nastavenia, Aplikacia::StavKrabicky, Aplikacia::Aktualizacie].iter().enumerate()
    {
        let (rr, c) = (i / 2, i % 2);
        polozka_vedla(
            k,
            t,
            *a,
            sy.0 + 32.0 + c as f32 * 292.0,
            sy.1 + 84.0 + rr as f32 * 56.0,
            44.0,
            st.popis,
        );
    }
    let vw2 = k.sirka("Vypnúť", st.popis) + 40.0 + 14.0;
    polozka_vedla(k, t, Aplikacia::Vypnut, sy.0 + sy.2 - 32.0 - vw2, sy.1 + 26.0, 40.0, st.popis);
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
        ikony_aplikacii::kresli(k, *a, x, YH - 8.0, mala, false);
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

fn prve_spustenie_pocasia(k: &mut Kreslic, t: &Tema, st: &StupnicaTv, foto: &Obr) {
    // pozadie: Domov ešte bez počasia (údaje ešte neboli stiahnuté)
    pozadie_domova(k, t, 'f', foto);
    koncept_mozaika(k, t, st, false);
    k.clona(0.0, 0.0, W, H, CLONA_OBRAZU, 200);
    let (w, h) = (1120.0, 900.0);
    let (x, y) = ((W - w) / 2.0, 90.0);
    k.tien(x, y, w, h, R_VELKE, 32.0, t.tien.max(6));
    k.obdlznik(x, y, w, h, R_VELKE, t.plocha_vyssia);
    pocasie(k, Pocasie::Oblak, x + 48.0, y + 28.0, 88.0, t.text, t.hviezda);
    k.text("Počasie", x + 160.0, y + 72.0 + st.nadpis.velkost / 2.0, st.nadpis, t.text);
    let riadky = [
        "Pre počasie krabička pošle službe Open-Meteo len",
        "približnú polohu obce (asi 10 km), raz za hodinu,",
        "bez označenia krabičky. Zoznam obcí je v krabičke –",
        "pri výbere sa nič neposiela. Bez miesta sa počasie",
        "nezobrazí; zapnúť ho môžeš neskôr v Nastaveniach.",
    ];
    let mut ty = y + 176.0;
    for r in riadky {
        k.text(r, x + 56.0, ty, st.popis, t.text);
        ty += st.popis.velkost + 18.0;
    }
    // výber kraj → obec z miestneho zoznamu (ovládačom pohodlnejší ako písanie)
    let (lx, lw, ox, ow) = (x + 56.0, 440.0, x + 56.0 + 440.0 + 32.0, w - 112.0 - 440.0 - 32.0);
    let hy = y + 412.0;
    k.text("Kraj", lx, hy, st.popis, t.text_druhy);
    k.text("Obec", ox, hy, st.popis, t.text_druhy);
    let kraje = ["Bratislavský", "Trnavský", "Trenčiansky", "Nitriansky", "Žilinský"];
    let obce = ["Bratislava", "Bernolákovo", "Malacky", "Pezinok", "Senec"];
    let rh = 52.0;
    for (i, (kr, ob)) in kraje.iter().zip(obce.iter()).enumerate() {
        let ry = hy + 20.0 + i as f32 * rh;
        if i == 0 {
            // vybraný kraj – neutrálne; zameraná obec – akcent a rámik (akcent = zameranie)
            k.obdlznik(lx, ry, lw, rh - 8.0, R, t.prvok_pod_mysou);
            k.obdlznik(ox, ry, ow, rh - 8.0, R, t.akcent);
            zameranie(k, t, ox, ry, ow, rh - 8.0, R);
        }
        k.text(kr, lx + 20.0, zakladna_v_strede(ry, rh - 8.0, st.popis), st.popis, t.text);
        let f = if i == 0 { t.akcent_text } else { t.text };
        k.text(ob, ox + 20.0, zakladna_v_strede(ry, rh - 8.0, st.popis), st.popis, f);
    }
    k.text("Údaje: Open-Meteo.com (CC BY 4.0)", x + 56.0, y + h - 56.0 - 72.0 - 24.0, st.popis, t.text_druhy);
    // tlačidlá – neutrálne (zameranie je v zozname obcí)
    let by = y + h - 56.0 - 72.0;
    let mut bx = x + 56.0;
    for s in ["Zadať miesto", "Bez počasia"] {
        let bw = k.sirka(s, st.text) + 80.0;
        k.obdlznik(bx, by, bw, 72.0, R_VELKE, t.prvok_pod_mysou);
        k.text(s, bx + 40.0, zakladna_v_strede(by, 72.0, st.text), st.text, t.text);
        bx += bw + 32.0;
    }
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
        // Mozaika (vybraná vlastníkom 28. 11. 11:47): '2' farebné pozadie, '4' atmosféra zameraného filmu;
        // pri väčšom písme (mierka nad 1,2) usporiadanie po radoch ako Panoráma
        '2' | '4' => {
            pozadie_domova(&mut k, t, if c == '4' { 'a' } else { 'f' }, foto);
            if st.mierka > 1.2 {
                koncept_panorama(&mut k, t, st);
            } else {
                koncept_mozaika(&mut k, t, st, true);
            }
        }
        '3' => koncept_scena(&mut k, t, st, foto),
        'p' => prve_spustenie_pocasia(&mut k, t, st, foto),
        's' => setric(&mut k, t, st),
        _ => koncept_panorama(&mut k, t, st),
    }
    k
}
