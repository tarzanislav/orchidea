//! Ovládacie prvky dizajnového systému a ich stavy – bežný, pod myšou, stlačený, zameraný, vypnutý (a vybraný).
//! Každý prvok sa kreslí len z návrhových hodnôt témy a `miery`; program aj návrh používajú tie isté funkcie.

use crate::ikony::Ikona;
use crate::kreslic::{Kreslic, zakladna_v_strede};
use crate::miery::*;
use crate::tema::Tema;
use orchidea_pismo::Farba;

/// Stav prvku.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stav {
    Bezny,
    PodMysou,
    Stlaceny,
    /// zameraný klávesnicou alebo diaľkovým ovládačom – rámik zamerania
    Zamerany,
    Vypnuty,
}

/// Všetky stavy v poradí listu dizajnového systému.
pub const STAVY: [Stav; 5] = [Stav::Bezny, Stav::PodMysou, Stav::Stlaceny, Stav::Zamerany, Stav::Vypnuty];

impl Stav {
    pub fn meno(self) -> &'static str {
        match self {
            Stav::Bezny => "bežný",
            Stav::PodMysou => "pod myšou",
            Stav::Stlaceny => "stlačený",
            Stav::Zamerany => "zameraný",
            Stav::Vypnuty => "vypnutý",
        }
    }
}

/// Druh tlačidla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Druh {
    /// hlavná akcia obrazovky (Uložiť, Orezať, Prehrať) – výplň v akcente, na obrazovke najviac jedna
    Hlavne,
    /// bežná akcia – výplň prvku
    Druhe,
    /// tlačidlo s ikonou na lište – bez výplne, výplň len pod myšou
    Ikona,
}

/// Farba výplne (žiadna = priehľadné) a popredia tlačidla v danom stave; `vybrane` = zapnutý nástroj.
pub fn farby_tlacidla(t: &Tema, druh: Druh, stav: Stav, vybrane: bool) -> (Option<Farba>, Farba) {
    match (druh, stav) {
        (_, Stav::Vypnuty) => (if druh == Druh::Ikona { None } else { Some(t.prvok) }, t.text_vypnuty),
        (Druh::Hlavne, Stav::PodMysou) => (Some(t.akcent_pod_mysou), t.akcent_text),
        (Druh::Hlavne, Stav::Stlaceny) => (Some(t.akcent_stlaceny), t.akcent_text),
        (Druh::Hlavne, _) => (Some(t.akcent), t.akcent_text),
        (_, Stav::Stlaceny) => (Some(t.prvok_stlaceny), t.text),
        (_, Stav::PodMysou) => (Some(t.prvok_pod_mysou), if vybrane { t.akcent_v_texte } else { t.text }),
        (Druh::Ikona, _) if vybrane => (Some(t.prvok_pod_mysou), t.akcent_v_texte),
        (Druh::Ikona, _) => (None, t.text),
        (Druh::Druhe, _) => (Some(t.prvok), t.text),
    }
}

/// Rámik zamerania okolo prvku (2 body, 2 body od okraja) – viditeľný v každej téme (3 : 1).
pub fn zameranie(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let o = ZAMERANIE_ODSTUP + ZAMERANIE_HRUBKA / 2.0;
    k.ram(x - o, y - o, w + 2.0 * o, h + 2.0 * o, r + o, ZAMERANIE_HRUBKA, t.zameranie);
}

/// Tlačidlo s ikonou a/alebo textom. Vráti šírku.
#[allow(clippy::too_many_arguments)]
pub fn tlacidlo(
    k: &mut Kreslic,
    t: &Tema,
    druh: Druh,
    stav: Stav,
    vybrane: bool,
    x: f32,
    y: f32,
    h: f32,
    ikona: Option<Ikona>,
    text: Option<&str>,
) -> f32 {
    let rez = TLACIDLO_REZ;
    let ik = if h >= 32.0 { IKONA } else { 16.0 };
    let w = match (ikona, text) {
        (Some(_), None) => h,
        (None, Some(s)) => k.sirka(s, rez) + 2.0 * 16.0,
        (Some(_), Some(s)) => k.sirka(s, rez) + ik + 8.0 + 2.0 * 14.0,
        (None, None) => h,
    };
    let (vypln, popredie) = farby_tlacidla(t, druh, stav, vybrane);
    if let Some(f) = vypln {
        k.obdlznik(x, y, w, h, R_MALE, f);
    }
    if t.vysoky_kontrast && druh != Druh::Ikona {
        k.ram_dnu(x, y, w, h, R_MALE, 1.0, t.okraj_silny);
    }
    if vybrane && stav != Stav::Vypnuty {
        // zapnutý nástroj: okrem farby aj tvar (pásik dole) – rozlíšiteľné aj pri poruche farbocitu
        k.obdlznik(x + w / 2.0 - 7.0, y + h - 4.0, 14.0, 2.0, 1.0, t.akcent_v_texte);
    }
    match (ikona, text) {
        (Some(i), None) => k.ikona(i, x + (w - ik) / 2.0, y + (h - ik) / 2.0, ik, popredie),
        (None, Some(s)) => k.text_v_strede(s, x, w, zakladna_v_strede(y, h, rez), rez, popredie),
        (Some(i), Some(s)) => {
            k.ikona(i, x + 14.0, y + (h - ik) / 2.0, ik, popredie);
            k.text(s, x + 14.0 + ik + 8.0, zakladna_v_strede(y, h, rez), rez, popredie);
        }
        (None, None) => {}
    }
    if stav == Stav::Zamerany {
        zameranie(k, t, x, y, w, h, R_MALE);
    }
    w
}

/// Posuvník s menom a hodnotou nad stopou; `hodnota` −1…1 (od stredu) alebo 0…1. Výška 36 bodov.
#[allow(clippy::too_many_arguments)]
pub fn posuvnik(
    k: &mut Kreslic,
    t: &Tema,
    stav: Stav,
    x: f32,
    y: f32,
    w: f32,
    meno: &str,
    hodnota_text: &str,
    hodnota: f32,
    od_stredu: bool,
) {
    let vyp = stav == Stav::Vypnuty;
    k.text(meno, x, y + 12.0, TEXT, if vyp { t.text_vypnuty } else { t.text_druhy });
    k.cisla_vpravo(hodnota_text, x + w, y + 12.0, TEXT, if vyp { t.text_vypnuty } else { t.text });
    let sy = y + 24.0;
    k.obdlznik(x, sy, w, STOPA, STOPA / 2.0, if t.vysoky_kontrast { t.okraj } else { t.prvok_stlaceny });
    let (od, poz) = if od_stredu {
        let s = x + w / 2.0;
        (s, s + hodnota.clamp(-1.0, 1.0) * w / 2.0)
    } else {
        (x, x + hodnota.clamp(0.0, 1.0) * w)
    };
    let vypln = if vyp { t.text_vypnuty } else { t.akcent };
    k.obdlznik(od.min(poz), sy, (poz - od).abs(), STOPA, STOPA / 2.0, vypln);
    if od_stredu {
        k.obdlznik(x + w / 2.0 - 0.5, sy - 3.0, 1.0, STOPA + 6.0, 0.5, t.text_druhy);
    }
    let r = UCHOP / 2.0 + if stav == Stav::PodMysou || stav == Stav::Stlaceny { 1.0 } else { 0.0 };
    let cy = sy + STOPA / 2.0;
    if stav == Stav::Stlaceny || stav == Stav::PodMysou {
        k.kruh(poz, cy, r + 6.0, orchidea_pismo::zmiesaj(t.plocha, t.akcent, 50));
    }
    k.kruh(poz, cy, r, if vyp { t.text_vypnuty } else { t.text });
    k.kruh(poz, cy, r - 3.0, vypln);
    if stav == Stav::Zamerany {
        zameranie(k, t, poz - r, cy - r, 2.0 * r, 2.0 * r, r);
    }
}

/// Prepínač zapnuté / vypnuté (40 × 24 bodov).
pub fn prepinac(k: &mut Kreslic, t: &Tema, stav: Stav, x: f32, y: f32, zapnute: bool) {
    let (w, h) = (40.0, 24.0);
    let vyp = stav == Stav::Vypnuty;
    let stopa = match (zapnute, vyp, stav) {
        (_, true, _) => t.prvok,
        (true, _, Stav::PodMysou) => t.akcent_pod_mysou,
        (true, _, Stav::Stlaceny) => t.akcent_stlaceny,
        (true, _, _) => t.akcent,
        (false, _, Stav::PodMysou | Stav::Stlaceny) => t.prvok_stlaceny,
        (false, _, _) => t.prvok_pod_mysou,
    };
    k.obdlznik(x, y, w, h, h / 2.0, stopa);
    if !zapnute {
        k.ram_dnu(x, y, w, h, h / 2.0, 1.0, t.okraj_silny);
    }
    let cx = if zapnute { x + w - h / 2.0 } else { x + h / 2.0 };
    let gulicka = match (zapnute, vyp) {
        (_, true) => t.text_vypnuty,
        (true, _) => t.akcent_text,
        (false, _) => t.text_druhy,
    };
    k.kruh(cx, y + h / 2.0, if zapnute { 8.0 } else { 6.0 }, gulicka);
    if stav == Stav::Zamerany {
        zameranie(k, t, x, y, w, h, h / 2.0);
    }
}

/// Štítok (voľba, kľúčové slovo, formát); vybraný = výplň v akcente. Vráti šírku.
#[allow(clippy::too_many_arguments)]
pub fn stitok(
    k: &mut Kreslic,
    t: &Tema,
    stav: Stav,
    x: f32,
    y: f32,
    text: &str,
    vybrany: bool,
    h: f32,
) -> f32 {
    let w = k.sirka(text, TEXT) + 24.0;
    let (vypln, popredie) = match (vybrany, stav) {
        (_, Stav::Vypnuty) => (t.prvok, t.text_vypnuty),
        (true, Stav::PodMysou) => (t.akcent_pod_mysou, t.akcent_text),
        (true, Stav::Stlaceny) => (t.akcent_stlaceny, t.akcent_text),
        (true, _) => (t.akcent, t.akcent_text),
        (false, Stav::PodMysou) => (t.prvok_pod_mysou, t.text),
        (false, Stav::Stlaceny) => (t.prvok_stlaceny, t.text),
        (false, _) => (t.prvok, t.text),
    };
    k.obdlznik(x, y, w, h, h / 2.0, vypln);
    if t.vysoky_kontrast && !vybrany {
        k.ram_dnu(x, y, w, h, h / 2.0, 1.0, t.okraj_silny);
    }
    k.text(text, x + 12.0, zakladna_v_strede(y, h, TEXT), TEXT, popredie);
    if stav == Stav::Zamerany {
        zameranie(k, t, x, y, w, h, h / 2.0);
    }
    w
}

/// Pole s hodnotou a jednotkou (šírka `w`, výška `POLE`).
#[allow(clippy::too_many_arguments)]
pub fn pole(k: &mut Kreslic, t: &Tema, stav: Stav, x: f32, y: f32, w: f32, hodnota: &str, jednotka: &str) {
    let vyp = stav == Stav::Vypnuty;
    k.obdlznik(x, y, w, POLE, R_MALE, if stav == Stav::PodMysou { t.prvok_pod_mysou } else { t.prvok });
    let okraj = match stav {
        Stav::Zamerany | Stav::Stlaceny => t.akcent,
        Stav::Vypnuty => t.okraj,
        _ => t.okraj_silny,
    };
    k.ram_dnu(x, y, w, POLE, R_MALE, if stav == Stav::Zamerany { 2.0 } else { 1.0 }, okraj);
    let z = zakladna_v_strede(y, POLE, TEXT);
    let koniec = k.cisla(hodnota, x + 10.0, z, TEXT, if vyp { t.text_vypnuty } else { t.text });
    if stav == Stav::Zamerany || stav == Stav::Stlaceny {
        // kurzor na konci textu
        k.obdlznik(koniec + 2.0, y + 8.0, 1.5, POLE - 16.0, 0.0, t.akcent_v_texte);
    }
    k.text_vpravo(jednotka, x + w - 10.0, z, POPIS, if vyp { t.text_vypnuty } else { t.text_druhy });
}

/// Bublina s popisom (a klávesovou skratkou) – nad prvkom so stredom `cx`, spodkom `y`.
pub fn bublina(k: &mut Kreslic, t: &Tema, cx: f32, y: f32, text: &str, klaves: Option<&str>) {
    let tw = k.sirka(text, TEXT);
    let kw = klaves.map(|s| k.sirka(s, POPIS) + 12.0).unwrap_or(0.0);
    let w = tw + 20.0 + if klaves.is_some() { kw + 8.0 } else { 0.0 };
    let h = 28.0;
    let x = cx - w / 2.0;
    k.tien(x, y - h, w, h, R_MALE, 6.0, t.tien);
    k.obdlznik(x, y - h, w, h, R_MALE, t.plocha_vyssia);
    k.ram_dnu(x, y - h, w, h, R_MALE, 1.0, if t.vysoky_kontrast { t.okraj_silny } else { t.okraj });
    k.text(text, x + 10.0, zakladna_v_strede(y - h, h, TEXT), TEXT, t.text);
    if let Some(s) = klaves {
        let kx = x + 10.0 + tw + 8.0;
        k.obdlznik(kx, y - h + 5.0, kw, h - 10.0, 4.0, t.prvok_pod_mysou);
        k.text(s, kx + 6.0, zakladna_v_strede(y - h + 5.0, h - 10.0, POPIS), POPIS, t.text_druhy);
    }
}

/// Nadpis skupiny v paneli (polotučný) – vráti ďalšie y.
pub fn nadpis(k: &mut Kreslic, t: &Tema, s: &str, x: f32, y: f32) -> f32 {
    k.text(s, x, y + NADPIS.velkost, NADPIS, t.text);
    y + NADPIS.velkost + 12.0
}
