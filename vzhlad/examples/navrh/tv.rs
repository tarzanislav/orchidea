//! Obrazovka TV krabičky pre diaľkový ovládač (1920 × 1080 bodov, ~3 m od obrazovky) – tri varianty.
//! A: veľký náhľad a pásy („Pokračovať v pozeraní“) · B: zoznam vľavo a podrobnosti vpravo · C: mriežka plagátov.
//! Pohyb šípkami, OK, Späť; zameraný je vždy práve jeden prvok – rámik zamerania 4 body v akcente.

use crate::obrazky::{Obr, plagat};
use crate::pohlad::logo_v;
use orchidea_pismo::zmiesaj;
use orchidea_vzhlad::ikony::Ikona;
use orchidea_vzhlad::kreslic::{Kreslic, zakladna_v_strede};
use orchidea_vzhlad::miery::*;
use orchidea_vzhlad::tema::Tema;

const W: f32 = 1920.0;
const H: f32 = 1080.0;

const FILMY: [(&str, &str); 7] = [
    ("Svetlo nad jazerom", "2024 · 1 h 52 min"),
    ("Mestá v daždi", "2023 · 2 h 05 min"),
    ("Polárna noc", "2025 · 1 h 38 min"),
    ("Medená hodina", "2022 · 1 h 47 min"),
    ("Hlboký les", "2021 · 1 h 31 min"),
    ("Stanica 9", "2026 · 2 h 12 min"),
    ("Dunaj", "2020 · 1 h 59 min"),
];

fn zameranie_tv(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let o = TV_ZAMERANIE_ODSTUP + TV_ZAMERANIE_HRUBKA / 2.0;
    k.ram(x - o, y - o, w + 2.0 * o, h + 2.0 * o, r + o, TV_ZAMERANIE_HRUBKA, t.zameranie);
}

/// Tlačidlo pre TV (výška 72 bodov); vráti šírku.
#[allow(clippy::too_many_arguments)]
fn tlacidlo_tv(
    k: &mut Kreslic,
    t: &Tema,
    x: f32,
    y: f32,
    ik: Ikona,
    s: &str,
    hlavne: bool,
    zamerane: bool,
) -> f32 {
    let w = k.sirka(s, TV_TEXT) + 40.0 + 36.0 + 40.0;
    let (vypln, popredie) = if hlavne { (t.akcent, t.akcent_text) } else { (t.prvok_pod_mysou, t.text) };
    k.obdlznik(x, y, w, TV_TLACIDLO, R_VELKE, vypln);
    k.ikona(ik, x + 36.0, y + 18.0, 36.0, popredie);
    k.text(s, x + 36.0 + 36.0 + 16.0, zakladna_v_strede(y, TV_TLACIDLO, TV_TEXT), TV_TEXT, popredie);
    if zamerane {
        zameranie_tv(k, t, x, y, w, TV_TLACIDLO, R_VELKE);
    }
    w
}

fn stitok_tv(k: &mut Kreslic, t: &Tema, x: f32, y: f32, s: &str) -> f32 {
    let w = k.sirka(s, TV_POPIS) + 40.0;
    k.obdlznik(x, y, w, 48.0, 24.0, t.prvok);
    if t.vysoky_kontrast {
        k.ram_dnu(x, y, w, 48.0, 24.0, 2.0, t.okraj_silny);
    }
    k.text(s, x + 20.0, zakladna_v_strede(y, 48.0, TV_POPIS), TV_POPIS, t.text);
    w
}

/// Horné záložky a hodiny.
fn zalozky(k: &mut Kreslic, t: &Tema, vybrana: usize) {
    let z = TV_OKRAJ_Y + 24.0 + TV_TEXT.velkost / 2.0;
    logo_v(k, t, TV_OKRAJ_X, z - TV_TEXT.velkost / 2.0 - 20.0, 40.0);
    let mut x = TV_OKRAJ_X + 72.0;
    for (i, s) in ["Domov", "Filmy", "Hudba", "Fotky"].iter().enumerate() {
        let w = k.sirka(s, TV_TEXT);
        k.text(s, x, z, TV_TEXT, if i == vybrana { t.text } else { t.text_druhy });
        if i == vybrana {
            k.obdlznik(x, z + 14.0, w, 4.0, 2.0, t.akcent);
        }
        x += w + 56.0;
    }
    k.cisla_vpravo("20:45", W - TV_OKRAJ_X, z, TV_TEXT, t.text);
    k.ikona(
        Ikona::Nastavenia,
        W - TV_OKRAJ_X - k.sirka_cisel("20:45", TV_TEXT) - 40.0 - 36.0,
        z - TV_TEXT.velkost / 2.0 - 18.0,
        36.0,
        t.text_druhy,
    );
}

/// Nápoveda ovládača dole vpravo – nad prechodom do okolia, aby nič neprekryla.
fn napoveda(k: &mut Kreslic, t: &Tema) {
    let h0 = 156.0;
    let (m, pw) = (k.mierka, k.p.sirka);
    let od = ((H - h0) * m) as usize;
    for py in od..k.p.vyska {
        let v = (py as f32 / m - (H - h0)) / h0;
        let a = (v.min(0.6) / 0.6 * 255.0) as u8;
        for px in 0..pw {
            let i = py * pw + px;
            k.p.body[i] = zmiesaj(k.p.body[i], t.okolie, a);
        }
    }
    let y = H - TV_OKRAJ_Y - 40.0;
    let mut x = W - TV_OKRAJ_X;
    for (kl, s) in [("Späť", "Domov"), ("šípky", "Pohyb"), ("OK", "Prehrať")] {
        let sw = k.sirka(s, TV_POPIS);
        x -= sw;
        k.text(s, x, zakladna_v_strede(y, 40.0, TV_POPIS), TV_POPIS, t.text_druhy);
        let kw = k.sirka(kl, TV_POPIS) + 24.0;
        x -= 12.0 + kw;
        k.obdlznik(x, y, kw, 40.0, 8.0, t.prvok_pod_mysou);
        k.text(kl, x + 12.0, zakladna_v_strede(y, 40.0, TV_POPIS), TV_POPIS, t.text);
        x -= 40.0;
    }
}

fn pozadie_s_nahladom(k: &mut Kreslic, t: &Tema, o: &Obr) {
    // náhľad vpravo hore, doľava a nadol prechádza do okolia témy
    let (x0, h0) = (W * 0.38, H * 0.72);
    k.obraz(&o.body, o.w, o.h, x0, 0.0, W - x0, h0, 0.0);
    let (m, pw) = (k.mierka, k.p.sirka);
    for py in 0..(h0 * m) as usize {
        for px in (x0 * m) as usize..pw {
            let u = (px as f32 / m - x0) / (W - x0);
            let v = py as f32 / m / h0;
            let a = ((1.0 - u * 2.2).max(0.0) + (v - 0.55).max(0.0) * 2.4).min(1.0);
            let a = (a * a * (3.0 - 2.0 * a) * 255.0) as u8;
            let i = py * pw + px;
            k.p.body[i] =
                zmiesaj(zmiesaj(k.p.body[i], t.okolie, if t.tmava { 70 } else { 120 }), t.okolie, a);
        }
    }
}

fn karta(
    k: &mut Kreslic,
    t: &Tema,
    i: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    postup: Option<f32>,
    zamerana: bool,
) {
    let o = plagat(i, (w * k.mierka) as usize / 2, (h * k.mierka) as usize / 2, FILMY[i % FILMY.len()].0);
    k.obraz(&o.body, o.w, o.h, x, y, w, h, R_VELKE);
    if let Some(p) = postup {
        k.obdlznik(x + 16.0, y + h - 14.0, w - 32.0, 6.0, 3.0, zmiesaj(0x000000, 0xFFFFFF, 90));
        k.obdlznik(x + 16.0, y + h - 14.0, (w - 32.0) * p, 6.0, 3.0, t.akcent);
    }
    if zamerana {
        zameranie_tv(k, t, x, y, w, h, R_VELKE);
    }
}

fn variant_a(k: &mut Kreslic, t: &Tema, foto: &Obr) {
    pozadie_s_nahladom(k, t, foto);
    zalozky(k, t, 1);
    let x = TV_OKRAJ_X;
    k.text("Svetlo nad jazerom", x, 316.0, TV_TITUL, t.text);
    k.cisla("2024 · 1 h 52 min · 12+", x, 376.0, TV_POPIS, t.text_druhy);
    let mut sx = x;
    for s in ["1080p HEVC", "Dolby Digital 5.1", "Titulky SK"] {
        sx += stitok_tv(k, t, sx, 404.0, s) + 16.0;
    }
    let mut y = 508.0;
    for r in
        ["Dvaja bratia sa po rokoch vrátia k horskému jazeru,", "kde ich otec postavil prvý most v doline."]
    {
        k.text(r, x, y, TV_POPIS, t.text_druhy);
        y += 44.0;
    }
    let y = 580.0;
    let w = tlacidlo_tv(k, t, x, y, Ikona::Prehrat, "Prehrať", true, true);
    let w2 = tlacidlo_tv(k, t, x + w + 24.0, y, Ikona::Info, "Informácie", false, false);
    tlacidlo_tv(k, t, x + w + w2 + 48.0, y, Ikona::Titulky, "Titulky", false, false);
    // pás
    let py = 744.0;
    k.text("Pokračovať v pozeraní", x, py, TV_TEXT, t.text);
    let (kw, kh) = (320.0, 180.0);
    for i in 0..6 {
        karta(
            k,
            t,
            i + 1,
            x + i as f32 * (kw + 32.0),
            py + 28.0,
            kw,
            kh,
            Some([0.62, 0.2, 0.85, 0.4, 0.1, 0.5][i]),
            false,
        );
    }
    napoveda(k, t);
}

fn variant_b(k: &mut Kreslic, t: &Tema) {
    zalozky(k, t, 1);
    let (x, lw) = (TV_OKRAJ_X, 760.0);
    let mut y = 184.0;
    for (i, (n, m)) in FILMY.iter().enumerate().take(6) {
        let rh = 112.0;
        let zamerany = i == 1;
        if zamerany {
            k.obdlznik(x, y, lw, rh, R_VELKE, t.prvok_pod_mysou);
            zameranie_tv(k, t, x, y, lw, rh, R_VELKE);
        }
        let o = plagat(i, 192, 108, "");
        k.obraz(&o.body, o.w, o.h, x + 16.0, y + 16.0, 142.0, 80.0, R);
        k.text(n, x + 182.0, y + 50.0, TV_TEXT, t.text);
        k.cisla(m, x + 182.0, y + 88.0, TV_POPIS, t.text_druhy);
        y += rh + 16.0;
    }
    // podrobnosti vpravo
    let (dx, dw) = (x + lw + 64.0, W - TV_OKRAJ_X - (x + lw + 64.0));
    let o = plagat(1, 640, 360, "");
    let dh = (dw * 9.0 / 16.0).round().min(470.0);
    k.tien(dx, 184.0, dw, dh, R_VELKE, 24.0, t.tien.max(3));
    k.obraz(&o.body, o.w, o.h, dx, 184.0, dw, dh, R_VELKE);
    let mut y = 184.0 + dh + 64.0;
    k.text("Mestá v daždi", dx, y, TV_NADPIS, t.text);
    y += 48.0;
    k.cisla("2023 · 2 h 05 min · 15+", dx, y, TV_POPIS, t.text_druhy);
    y += 28.0;
    let mut sx = dx;
    for s in ["4K HEVC 10 bitov", "AC-3 5.1 priamo do TV"] {
        sx += stitok_tv(k, t, sx, y, s) + 16.0;
    }
    y += 48.0 + 48.0;
    k.text("Tri noci, jedno mesto a list, ktorý nikto neodoslal.", dx, y, TV_POPIS, t.text_druhy);
    napoveda(k, t);
}

fn variant_c(k: &mut Kreslic, t: &Tema) {
    zalozky(k, t, 1);
    let x = TV_OKRAJ_X;
    k.text("Filmy", x, 228.0, TV_NADPIS, t.text);
    k.cisla(
        "48 filmov · podľa pridania",
        x + k.sirka("Filmy", TV_NADPIS) + 24.0,
        228.0,
        TV_POPIS,
        t.text_druhy,
    );
    let (pw, ph, medzera) = (224.0, 336.0, 40.0);
    let y0 = 292.0;
    for i in 0..7 {
        let px = x + i as f32 * (pw + medzera);
        if px + pw > W - TV_OKRAJ_X + pw * 0.6 {
            break;
        }
        let zamerany = i == 2;
        let (zx, zy, zw, zh) =
            if zamerany { (px - 12.0, y0 - 18.0, pw + 24.0, ph + 36.0) } else { (px, y0, pw, ph) };
        if zamerany {
            k.tien(zx, zy, zw, zh, R_VELKE, 24.0, t.tien.max(4));
        }
        let o = plagat(
            i + 2,
            (zw * k.mierka) as usize / 2,
            (zh * k.mierka) as usize / 2,
            FILMY[(i + 2) % FILMY.len()].0,
        );
        k.obraz(&o.body, o.w, o.h, zx, zy, zw, zh, R_VELKE);
        if zamerany {
            zameranie_tv(k, t, zx, zy, zw, zh, R_VELKE);
            k.text(FILMY[(i + 2) % FILMY.len()].0, zx, zy + zh + 56.0, TV_TEXT, t.text);
            k.cisla(FILMY[(i + 2) % FILMY.len()].1, zx, zy + zh + 96.0, TV_POPIS, t.text_druhy);
        } else {
            k.obdlznik_a(zx, zy, zw, zh, R_VELKE, t.okolie, 60.0);
        }
    }
    // druhý riadok – len začiatok (naznačí ďalší obsah pod)
    for i in 0..7 {
        let px = x + i as f32 * (pw + medzera);
        let o = plagat(i + 5, 116, 174, "");
        k.obraz(&o.body, o.w, o.h, px, 836.0, pw, ph, R_VELKE);
    }
    napoveda(k, t);
}

/// Obrazovka TV krabičky v téme `t`, variante `v` a mierke `m` (1 = 1080p, 2 = 4K).
pub fn obrazovka(t: &Tema, v: char, m: f32, foto: &Obr) -> Kreslic {
    let mut k = Kreslic::novy(W, H, m, t.okolie);
    match v {
        'b' => variant_b(&mut k, t),
        'c' => variant_c(&mut k, t),
        _ => variant_a(&mut k, t, foto),
    }
    k
}
