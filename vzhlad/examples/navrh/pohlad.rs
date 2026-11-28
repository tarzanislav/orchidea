//! Prehliadač a editor fotografií (Pohľad) – návrh obrazovky v troch variantoch.
//! A: lišta hore, pravý panel úprav, pás náhľadov (schválený smer) · B: nástroje vľavo, panel so záložkami,
//! režim orezania · C: čisté plátno – plávajúca lišta a karta úprav nad fotkou.

use crate::obrazky::{Obr, histogram};
use orchidea_pismo::zmiesaj;
use orchidea_vzhlad::ikony::Ikona;
use orchidea_vzhlad::kreslic::{Kreslic, zakladna_v_strede};
use orchidea_vzhlad::miery::*;
use orchidea_vzhlad::prvky::{self, Druh, Stav};
use orchidea_vzhlad::tema::Tema;
use orchidea_vzhlad::znak::{self, Znak};

pub const W: f32 = 1440.0;
pub const H: f32 = 900.0;

/// Znak programu – orchideová fialová bodka s pyskom kvetu.
pub fn logo(k: &mut Kreslic, t: &Tema, x: f32, y: f32) {
    logo_v(k, t, x, y, 20.0);
}

/// Znak ORCHIDEE vo veľkosti `s` bodov – dočasne návrh 1 (orchidea), kým vlastník nevyberie (`system/logo.png`).
pub fn logo_v(k: &mut Kreslic, t: &Tema, x: f32, y: f32, s: f32) {
    znak::kresli(k, Znak::Orchidea, t, x, y, s);
}

fn oddelovac_zvisly(k: &mut Kreslic, t: &Tema, x: f32, y: f32) {
    k.obdlznik(x, y, 1.0, 20.0, 0.0, t.okraj);
}

/// Fotka v strede plochy s tieňom; vráti jej obdĺžnik.
fn fotka_v_ploche(
    k: &mut Kreslic,
    t: &Tema,
    f: &Obr,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    okraj: f32,
) -> (f32, f32, f32, f32) {
    let mierka = ((w - 2.0 * okraj) / f.w as f32).min((h - 2.0 * okraj) / f.h as f32);
    let (ow, oh) = ((f.w as f32 * mierka).round(), (f.h as f32 * mierka).round());
    let (ox, oy) = ((x + (w - ow) / 2.0).round(), (y + (h - oh) / 2.0).round());
    k.tien(ox, oy, ow, oh, 4.0, 14.0, t.tien.max(3));
    k.obraz(&f.body, f.w, f.h, ox, oy, ow, oh, 2.0);
    (ox, oy, ow, oh)
}

fn histogram_box(k: &mut Kreslic, t: &Tema, f: &Obr, x: f32, y: f32, w: f32, h: f32) {
    k.obdlznik(x, y, w, h, R, t.prvok);
    let hist = histogram(f);
    let max = hist.iter().flat_map(|h| h.iter()).copied().max().unwrap_or(1) as f32;
    let farby = [0xF0605D, 0x4FCB7E, 0x5B9BFF];
    let sb = (w - 16.0) / 64.0;
    for (i, hi) in hist.iter().enumerate() {
        for (j, &n) in hi.iter().enumerate() {
            let v = (n as f32 / max).sqrt() * (h - 16.0);
            k.obdlznik_a(
                x + 8.0 + j as f32 * sb,
                y + h - 8.0 - v,
                sb + 0.3,
                v,
                0.0,
                farby[i],
                if t.tmava { 120.0 } else { 150.0 },
            );
        }
    }
    // štvrtiny
    for q in 1..4 {
        k.obdlznik_a(x + 8.0 + (w - 16.0) * q as f32 / 4.0, y + 8.0, 1.0, h - 16.0, 0.0, t.text_druhy, 40.0);
    }
}

fn oddelovac(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32) -> f32 {
    k.obdlznik(x, y + 16.0, w, 1.0, 0.0, t.okraj);
    y + 33.0
}

fn hlavicka_sekcie(k: &mut Kreslic, t: &Tema, s: &str, vpravo: Option<&str>, x: f32, y: f32, w: f32) -> f32 {
    k.text(s, x, y + NADPIS.velkost, NADPIS, t.text);
    if let Some(v) = vpravo {
        k.text_vpravo(v, x + w, y + NADPIS.velkost, TEXT, t.akcent_v_texte);
    }
    y + NADPIS.velkost + 14.0
}

fn hviezdy(k: &mut Kreslic, t: &Tema, x: f32, y: f32, pocet: usize) {
    for i in 0..5 {
        // neobsadená = obrys v silnom okraji (3 : 1, tvar aj farba) – nie sivá plná hviezda
        let (ik, f) =
            if i < pocet { (Ikona::Hviezda, t.hviezda) } else { (Ikona::HviezdaPrazdna, t.okraj_silny) };
        k.ikona(ik, x + i as f32 * 28.0, y, 20.0, f);
    }
}

fn stitky(k: &mut Kreslic, t: &Tema, x: f32, mut y: f32, w: f32) -> f32 {
    let mut cx = x;
    for s in ["západ slnka", "hory", "jazero", "Tatry"] {
        let sw = k.sirka(s, TEXT) + 24.0;
        if cx + sw > x + w {
            cx = x;
            y += STITOK + 8.0;
        }
        cx += prvky::stitok(k, t, Stav::Bezny, cx, y, s, false, STITOK) + 8.0;
    }
    let s = "+ pridať";
    let sw = k.sirka(s, TEXT) + 24.0;
    if cx + sw > x + w {
        cx = x;
        y += STITOK + 8.0;
    }
    k.ram_dnu(cx, y, sw, STITOK, STITOK / 2.0, 1.0, t.okraj_silny);
    k.text(s, cx + 12.0, zakladna_v_strede(y, STITOK, TEXT), TEXT, t.text_druhy);
    y + STITOK
}

fn udaje(k: &mut Kreslic, t: &Tema, x: f32, mut y: f32, w: f32) -> f32 {
    for (m, v) in [
        ("Rozmer", "6000 × 4000 bodov"),
        ("Formát", "JPEG 4:2:0, 8 bitov"),
        ("Profil farieb", "Display P3"),
        ("Súbor", "8,4 MB"),
        ("Dátum", "12. 10. 2026 18:42"),
    ] {
        k.text(m, x, y + 12.0, POPIS, t.text_druhy);
        k.cisla_vpravo(v, x + w, y + 12.0, TEXT, t.text);
        y += 22.0;
    }
    y
}

/// Posuvníky Svetla v pokojovom stave (stavy pod myšou a zameraný sú na liste prvkov – kritika A, 28. 11.).
fn svetlo(k: &mut Kreslic, t: &Tema, x: f32, mut y: f32, w: f32) -> f32 {
    for (m, h, v) in [("Jas", "+12", 0.12f32), ("Kontrast", "+20", 0.2), ("Gama", "-6", -0.06)] {
        prvky::posuvnik(k, t, Stav::Bezny, x, y, w, m, h, v, true);
        y += 44.0;
    }
    y
}

/// Panel úprav (varianty A a C).
fn panel_uprav(k: &mut Kreslic, t: &Tema, f: &Obr, x: f32, y: f32, w: f32, kompaktny: bool) {
    let mut y = hlavicka_sekcie(k, t, "Histogram", Some("RGB"), x, y, w);
    histogram_box(k, t, f, x, y, w, if kompaktny { 80.0 } else { 96.0 });
    y += if kompaktny { 80.0 } else { 96.0 };
    y = oddelovac(k, t, x, y, w);
    y = hlavicka_sekcie(k, t, "Svetlo", Some("Obnoviť"), x, y, w);
    y = svetlo(k, t, x, y, w) - 8.0;
    y = oddelovac(k, t, x, y, w);
    if !kompaktny {
        y = hlavicka_sekcie(k, t, "Veľkosť", None, x, y, w);
        let pw = (w - 8.0) / 2.0;
        for (i, (m, v)) in [("Šírka", "6000"), ("Výška", "4000")].iter().enumerate() {
            let px = x + i as f32 * (pw + 8.0);
            k.text(m, px, y + 11.0, POPIS, t.text_druhy);
            prvky::pole(k, t, Stav::Bezny, px, y + 18.0, pw, v, "bodov");
        }
        y += 18.0 + POLE + 12.0;
        prvky::tlacidlo(
            k,
            t,
            Druh::Druhe,
            Stav::Bezny,
            false,
            x,
            y,
            TLACIDLO_MALE,
            None,
            Some("Zmenšiť na polovicu"),
        );
        y += TLACIDLO_MALE;
        y = oddelovac(k, t, x, y, w);
    }
    y = hlavicka_sekcie(k, t, "Hodnotenie", None, x, y, w);
    hviezdy(k, t, x, y, 4);
    y += 20.0 + 16.0;
    y = hlavicka_sekcie(k, t, "Kľúčové slová", None, x, y, w);
    y = stitky(k, t, x, y, w);
    if !kompaktny {
        y = oddelovac(k, t, x, y, w);
        y = hlavicka_sekcie(k, t, "Údaje", None, x, y, w);
        udaje(k, t, x, y, w);
    }
}

fn pas_nahladov(k: &mut Kreslic, t: &Tema, f: &Obr, x: f32, y: f32, w: f32, h: f32) {
    k.obdlznik(x, y, w, h, 0.0, t.plocha);
    k.obdlznik(x, y, w, 1.0, 0.0, t.okraj);
    let nh = h - 32.0;
    let nw = (nh * 1.5).round();
    for i in 0..9 {
        let nx = x + 16.0 + i as f32 * (nw + 8.0);
        if nx + nw > x + w - 80.0 {
            break;
        }
        let ny = y + 16.0;
        k.obraz(&f.body, f.w, f.h, nx, ny, nw, nh, R_MALE);
        // iné „fotky“: jemne iný jas a tón
        let tonu = [0, 40, 0, 0, 70, 20, 50, 0, 30][i];
        if tonu > 0 {
            k.obdlznik_a(nx, ny, nw, nh, R_MALE, if i % 2 == 0 { 0x000000 } else { 0x2A3A66 }, tonu as f32);
        }
        if i == 3 {
            k.ram(nx - 3.0, ny - 3.0, nw + 6.0, nh + 6.0, R_MALE + 3.0, 2.0, t.akcent);
        }
    }
    k.cisla_vpravo("4 z 9", x + w - 16.0, zakladna_v_strede(y, h, TEXT), TEXT, t.text_druhy);
}

fn ovladanie_priblizenia(k: &mut Kreslic, t: &Tema, cx: f32, y: f32) {
    let w = 236.0;
    let x = cx - w / 2.0;
    k.tien(x, y, w, 36.0, 18.0, 10.0, t.tien);
    k.obdlznik(x, y, w, 36.0, 18.0, t.plocha_vyssia);
    k.ram_dnu(x, y, w, 36.0, 18.0, 1.0, t.okraj);
    let z = zakladna_v_strede(y, 36.0, TEXT);
    k.text_v_strede("-", x + 4.0, 28.0, z, TEXT_SILNY, t.text);
    k.cisla(" 24 %", x + 36.0, z, TEXT_SILNY, t.text);
    k.text_v_strede("+", x + 96.0, 28.0, z, TEXT_SILNY, t.text);
    k.obdlznik(x + 132.0, y + 8.0, 1.0, 20.0, 0.0, t.okraj);
    k.text("Prispôsobiť", x + 144.0, z, TEXT, t.text_druhy);
}

fn skupiny_nastrojov(orezanie_vybrane: bool) -> Vec<Vec<(Ikona, bool)>> {
    vec![
        vec![(Ikona::Otvorit, false)],
        vec![
            (Ikona::OtocVlavo, false),
            (Ikona::OtocVpravo, false),
            (Ikona::Zrkadlit, false),
            (Ikona::Orezat, orezanie_vybrane),
        ],
        vec![(Ikona::Vratit, false), (Ikona::Znova, false)],
        vec![(Ikona::Porovnat, false), (Ikona::Prezentacia, false), (Ikona::Katalog, false)],
    ]
}

/// Variant A – schválený smer, dotiahnutý: lišta, panel úprav, pás náhľadov.
fn variant_a(k: &mut Kreslic, t: &Tema, f: &Obr) {
    let (pas, px) = (96.0, W - PANEL);
    k.obdlznik(0.0, 0.0, W, LISTA, 0.0, t.plocha);
    k.obdlznik(0.0, LISTA - 1.0, W, 1.0, 0.0, t.okraj);
    logo(k, t, 16.0, 16.0);
    k.text("zapad_slnka.jpg", 48.0, 23.0, TEXT_SILNY, t.text);
    k.cisla("6000 × 4000 · JPEG · 24 %", 48.0, 40.0, POPIS, t.text_druhy);
    // nástroje v strede plochy fotky
    let skupiny = skupiny_nastrojov(false);
    let n: usize = skupiny.iter().map(|s| s.len()).sum();
    let sirka = n as f32 * (TLACIDLO + 4.0) - 4.0 + (skupiny.len() - 1) as f32 * 25.0;
    let mut x = ((px - sirka) / 2.0).round();
    for (g, s) in skupiny.iter().enumerate() {
        if g > 0 {
            oddelovac_zvisly(k, t, x + 8.0, 16.0);
            x += 21.0;
        }
        for &(ik, vybrane) in s {
            prvky::tlacidlo(k, t, Druh::Ikona, Stav::Bezny, vybrane, x, 8.0, TLACIDLO, Some(ik), None);
            x += TLACIDLO + 4.0;
        }
    }
    let uw = k.sirka("Uložiť", TLACIDLO_REZ) + IKONA + 8.0 + 28.0;
    prvky::tlacidlo(
        k,
        t,
        Druh::Hlavne,
        Stav::Bezny,
        false,
        W - 16.0 - uw,
        8.0,
        TLACIDLO,
        Some(Ikona::Ulozit),
        Some("Uložiť"),
    );
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        false,
        W - 16.0 - uw - 8.0 - TLACIDLO,
        8.0,
        TLACIDLO,
        Some(Ikona::Napoveda),
        None,
    );
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        false,
        W - 16.0 - uw - 12.0 - 2.0 * TLACIDLO,
        8.0,
        TLACIDLO,
        Some(Ikona::Nastavenia),
        None,
    );
    // plocha fotky
    fotka_v_ploche(k, t, f, 0.0, LISTA, px, H - LISTA - pas - 40.0, 40.0);
    ovladanie_priblizenia(k, t, px / 2.0, H - pas - 16.0 - 36.0);
    pas_nahladov(k, t, f, 0.0, H - pas, px, pas);
    // panel
    k.obdlznik(px, LISTA, PANEL, H - LISTA, 0.0, t.plocha);
    k.obdlznik(px, LISTA, 1.0, H - LISTA, 0.0, t.okraj);
    panel_uprav(k, t, f, px + 20.0, LISTA + 20.0, PANEL - 40.0, false);
}

/// Variant B – nástroje vo zvislom páse vľavo, panel so záložkami; zobrazený režim orezania.
fn variant_b(k: &mut Kreslic, t: &Tema, f: &Obr) {
    let (lista, pas_l, px) = (48.0, 56.0, W - PANEL);
    k.obdlznik(0.0, 0.0, W, lista, 0.0, t.plocha);
    k.obdlznik(0.0, lista - 1.0, W, 1.0, 0.0, t.okraj);
    logo(k, t, 18.0, 14.0);
    let z = zakladna_v_strede(0.0, lista, TEXT_SILNY);
    let e = k.text("zapad_slnka.jpg", 56.0, z, TEXT_SILNY, t.text);
    k.cisla("6000 × 4000 · JPEG", e + 12.0, z, TEXT, t.text_druhy);
    let uw = k.sirka("Uložiť", TLACIDLO_REZ) + IKONA + 8.0 + 28.0;
    prvky::tlacidlo(
        k,
        t,
        Druh::Hlavne,
        Stav::Bezny,
        false,
        W - 16.0 - uw,
        6.0,
        TLACIDLO,
        Some(Ikona::Ulozit),
        Some("Uložiť"),
    );
    prvky::pole(k, t, Stav::Bezny, W - 16.0 - uw - 16.0 - 96.0, 8.0, 96.0, "24", "%");
    // zvislý pás nástrojov
    k.obdlznik(0.0, lista, pas_l, H - lista, 0.0, t.plocha);
    k.obdlznik(pas_l - 1.0, lista, 1.0, H - lista, 0.0, t.okraj);
    let mut y = lista + 10.0;
    for (g, s) in skupiny_nastrojov(true).iter().enumerate() {
        if g > 0 {
            k.obdlznik(18.0, y + 6.0, 20.0, 1.0, 0.0, t.okraj);
            y += 13.0;
        }
        for &(ik, vybrane) in s {
            prvky::tlacidlo(k, t, Druh::Ikona, Stav::Bezny, vybrane, 10.0, y, TLACIDLO, Some(ik), None);
            y += TLACIDLO + 4.0;
        }
    }
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        false,
        10.0,
        H - 10.0 - 2.0 * TLACIDLO - 4.0,
        TLACIDLO,
        Some(Ikona::Nastavenia),
        None,
    );
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        false,
        10.0,
        H - 10.0 - TLACIDLO,
        TLACIDLO,
        Some(Ikona::Napoveda),
        None,
    );
    // fotka a orezanie
    let (ox, oy, ow, oh) = fotka_v_ploche(k, t, f, pas_l, lista, px - pas_l, H - lista, 56.0);
    let (sx, sy, sw, sh) =
        ((ox + ow * 0.16).round(), (oy + oh * 0.12).round(), (ow * 0.62).round(), (oh * 0.62).round());
    k.clona(ox, oy, ow, sy - oy, 0x000000, 140);
    k.clona(ox, sy + sh, ow, oy + oh - sy - sh, 0x000000, 140);
    k.clona(ox, sy, sx - ox, sh, 0x000000, 140);
    k.clona(sx + sw, sy, ox + ow - sx - sw, sh, 0x000000, 140);
    for j in 1..3 {
        k.obdlznik_a(sx + (sw * j as f32 / 3.0).round(), sy, 1.0, sh, 0.0, 0xFFFFFF, 90.0);
        k.obdlznik_a(sx, sy + (sh * j as f32 / 3.0).round(), sw, 1.0, 0.0, 0xFFFFFF, 90.0);
    }
    k.ram_dnu(sx, sy, sw, sh, 0.0, 1.0, 0xFFFFFF);
    for (cx, cy) in [(sx, sy), (sx + sw, sy), (sx, sy + sh), (sx + sw, sy + sh)] {
        let (dx, dy) = (if cx > sx { -1.0 } else { 1.0 }, if cy > sy { -1.0 } else { 1.0 });
        k.ciara((cx, cy), (cx + dx * 20.0, cy), 4.0, 0xFFFFFF);
        k.ciara((cx, cy), (cx, cy + dy * 20.0), 4.0, 0xFFFFFF);
    }
    let s = "3600 × 2400";
    let tw = k.sirka_cisel(s, TEXT_SILNY) + 20.0;
    k.obdlznik_a(sx + 8.0, sy + 8.0, tw, 24.0, 12.0, 0x000000, 170.0);
    k.cisla(s, sx + 18.0, zakladna_v_strede(sy + 8.0, 24.0, TEXT_SILNY), TEXT_SILNY, 0xFFFFFF);
    // panel so záložkami
    k.obdlznik(px, lista, PANEL, H - lista, 0.0, t.plocha);
    k.obdlznik(px, lista, 1.0, H - lista, 0.0, t.okraj);
    let (x, w) = (px + 20.0, PANEL - 40.0);
    let mut y = lista + 16.0;
    // záložky ako delený prepínač
    k.obdlznik(x, y, w, 32.0, R_MALE + 2.0, t.prvok);
    let zw = (w - 8.0) / 3.0;
    for (i, s) in ["Orezanie", "Úpravy", "Údaje"].iter().enumerate() {
        let zx = x + 4.0 + i as f32 * zw;
        if i == 0 {
            k.obdlznik(zx, y + 4.0, zw, 24.0, R_MALE, t.prvok_stlaceny);
        }
        k.text_v_strede(
            s,
            zx,
            zw,
            zakladna_v_strede(y + 4.0, 24.0, TEXT),
            if i == 0 { TEXT_SILNY } else { TEXT },
            if i == 0 { t.text } else { t.text_druhy },
        );
    }
    y += 32.0 + 20.0;
    y = hlavicka_sekcie(k, t, "Pomer strán", Some("Otočiť"), x, y, w);
    let mut cx = x;
    for (i, p) in ["Voľný", "1:1", "4:3", "3:2", "16:9"].iter().enumerate() {
        cx += prvky::stitok(k, t, Stav::Bezny, cx, y, p, i == 3, STITOK) + 6.0;
    }
    y += STITOK;
    y = oddelovac(k, t, x, y, w);
    y = hlavicka_sekcie(k, t, "Rozmer výberu", None, x, y, w);
    let pw = (w - 8.0) / 2.0;
    for (i, (m, v)) in [("Šírka", "3600"), ("Výška", "2400")].iter().enumerate() {
        let qx = x + i as f32 * (pw + 8.0);
        k.text(m, qx, y + 11.0, POPIS, t.text_druhy);
        prvky::pole(k, t, Stav::Bezny, qx, y + 18.0, pw, v, "bodov");
    }
    y += 18.0 + POLE + 16.0;
    k.obdlznik(x, y, w, 48.0, R, zmiesaj(t.prvok, t.uspech, 28));
    k.kruh(x + 16.0, y + 17.0, 4.0, t.uspech);
    k.text("Bezstratovo", x + 30.0, y + 22.0, TEXT_SILNY, t.text);
    k.text("prichytí sa k mriežke 16 × 16", x + 30.0, y + 39.0, POPIS, t.text_druhy);
    y += 48.0 + 20.0;
    let bw = (w - 8.0) / 2.0;
    k.obdlznik(x, y, bw, TLACIDLO, R_MALE, t.akcent);
    k.text_v_strede(
        "Orezať",
        x,
        bw,
        zakladna_v_strede(y, TLACIDLO, TLACIDLO_REZ),
        TLACIDLO_REZ,
        t.akcent_text,
    );
    k.obdlznik(x + bw + 8.0, y, bw, TLACIDLO, R_MALE, t.prvok);
    k.text_v_strede(
        "Zrušiť",
        x + bw + 8.0,
        bw,
        zakladna_v_strede(y, TLACIDLO, TLACIDLO_REZ),
        TLACIDLO_REZ,
        t.text,
    );
    y += TLACIDLO + 20.0;
    for (r, kl) in [
        ("Nakresliť výber", "ťahať ľavým"),
        ("Posunúť fotku", "ťahať pravým"),
        ("Orezať", "Enter"),
        ("Zrušiť", "Esc"),
    ] {
        k.text(r, x, y + 12.0, POPIS, t.text_druhy);
        let kw = k.sirka(kl, POPIS) + 12.0;
        k.obdlznik(x + w - kw, y - 1.0, kw, 18.0, 4.0, t.prvok);
        k.text(kl, x + w - kw + 6.0, y + 12.0, POPIS, t.text);
        y += 24.0;
    }
}

/// Variant C – čisté plátno: plávajúca lišta nástrojov a karta úprav nad tmavým okolím.
fn variant_c(k: &mut Kreslic, t: &Tema, f: &Obr) {
    let (kw, okraj) = (304.0, 16.0);
    let kx = W - okraj - kw;
    fotka_v_ploche(k, t, f, 0.0, 0.0, kx - okraj, H, 72.0);
    // plávajúca lišta nástrojov
    let skupiny = skupiny_nastrojov(false);
    let n: usize = skupiny.iter().map(|s| s.len()).sum();
    let lw = n as f32 * (TLACIDLO + 4.0) - 4.0 + (skupiny.len() - 1) as f32 * 25.0 + 16.0 + 40.0;
    let lx = ((kx - okraj - lw) / 2.0).round();
    k.tien(lx, okraj, lw, 52.0, 26.0, 14.0, t.tien);
    k.obdlznik(lx, okraj, lw, 52.0, 26.0, t.plocha);
    k.ram_dnu(lx, okraj, lw, 52.0, 26.0, 1.0, t.okraj);
    logo(k, t, lx + 16.0, okraj + 16.0);
    let mut x = lx + 48.0;
    for (g, s) in skupiny.iter().enumerate() {
        if g > 0 {
            oddelovac_zvisly(k, t, x + 8.0, okraj + 16.0);
            x += 21.0;
        }
        for &(ik, v) in s {
            prvky::tlacidlo(k, t, Druh::Ikona, Stav::Bezny, v, x, okraj + 8.0, TLACIDLO, Some(ik), None);
            x += TLACIDLO + 4.0;
        }
    }
    // karta úprav
    let kh = 640.0;
    k.tien(kx, okraj, kw, kh, R_VELKE, 16.0, t.tien);
    k.obdlznik(kx, okraj, kw, kh, R_VELKE, t.plocha);
    k.ram_dnu(kx, okraj, kw, kh, R_VELKE, 1.0, t.okraj);
    let (x, w) = (kx + 20.0, kw - 40.0);
    let mut y = okraj + 20.0;
    k.text("zapad_slnka.jpg", x, y + 12.0, TEXT_SILNY, t.text);
    k.cisla("6000 × 4000 · 8,4 MB", x, y + 30.0, POPIS, t.text_druhy);
    let uw = k.sirka("Uložiť", TLACIDLO_REZ) + 32.0;
    prvky::tlacidlo(
        k,
        t,
        Druh::Hlavne,
        Stav::Bezny,
        false,
        x + w - uw,
        y - 2.0,
        TLACIDLO,
        None,
        Some("Uložiť"),
    );
    y += 52.0;
    k.obdlznik(x - 20.0, y, kw, 1.0, 0.0, t.okraj);
    y += 20.0;
    panel_uprav(k, t, f, x, y, w, true);
    // dole: meno a poradie, priblíženie
    let s = "4 z 9  ·  12. 10. 2026";
    let sw = k.sirka_cisel(s, TEXT) + 32.0;
    k.tien(okraj, H - okraj - 36.0, sw, 36.0, 18.0, 10.0, t.tien);
    k.obdlznik(okraj, H - okraj - 36.0, sw, 36.0, 18.0, t.plocha);
    k.cisla(s, okraj + 16.0, zakladna_v_strede(H - okraj - 36.0, 36.0, TEXT), TEXT, t.text_druhy);
    ovladanie_priblizenia(k, t, (kx - okraj) / 2.0, H - okraj - 36.0);
}

/// Obrazovka Pohľadu v téme `t`, variante `v` ('a', 'b', 'c') a mierke `m`.
pub fn obrazovka(t: &Tema, v: char, m: f32, f: &Obr) -> Kreslic {
    let mut k = Kreslic::novy(W, H, m, t.okolie);
    match v {
        'b' => variant_b(&mut k, t, f),
        'c' => variant_c(&mut k, t, f),
        _ => variant_a(&mut k, t, f),
    }
    k
}
