//! Prehrávač médií LÝRA – návrh obrazovky v troch variantoch.
//! A: obal, údaje o médiu a zoznam skladieb (prijatý smer) · B: knižnica albumov s malým prehrávačom ·
//! C: „teraz hrá“ na celú plochu s farbou z obalu.

use crate::obrazky::{Obr, obal, priemer, stvorec};
use crate::pohlad::logo;
use orchidea_pismo::zmiesaj;
use orchidea_vzhlad::ikony::Ikona;
use orchidea_vzhlad::kontrast::{farba_so_svietivostou, najsilnejsie_vmiesanie, svietivost};
use orchidea_vzhlad::kreslic::{Kreslic, zakladna_v_strede};
use orchidea_vzhlad::miery::*;
use orchidea_vzhlad::prvky::{self, Druh, Stav};
use orchidea_vzhlad::tema::Tema;

const W: f32 = 1440.0;
const H: f32 = 900.0;

/// Ukážkový album – jeden album, jeden (vymyslený) interpret, jeden obal pri všetkých jeho skladbách.
const ALBUM: &str = "Horské obrazy";
const INTERPRET: &str = "Komorný orchester Javor";

const SKLADBY: [(&str, &str); 8] = [
    ("Ranná hmla", "3:41"),
    ("Svetlo nad jazerom", "5:12"),
    ("Horský vietor", "4:05"),
    ("Odraz", "6:30"),
    ("Nočné ticho", "4:48"),
    ("Návrat", "3:57"),
    ("Hrebeň", "5:20"),
    ("Posledný lúč", "7:02"),
];

fn horna_lista(k: &mut Kreslic, t: &Tema) {
    k.obdlznik(0.0, 0.0, W, LISTA, 0.0, t.plocha);
    k.obdlznik(0.0, LISTA - 1.0, W, 1.0, 0.0, t.okraj);
    logo(k, t, 16.0, 16.0);
    k.text("prehrávač médií LÝRA", 48.0, zakladna_v_strede(0.0, LISTA, TEXT_SILNY), TEXT_SILNY, t.text);
    let (hx, hw) = (W / 2.0 - 220.0, 440.0);
    k.obdlznik(hx, 10.0, hw, 32.0, 16.0, t.prvok);
    if t.vysoky_kontrast {
        k.ram_dnu(hx, 10.0, hw, 32.0, 16.0, 1.0, t.okraj_silny);
    }
    k.ikona(Ikona::Hladat, hx + 12.0, 18.0, 16.0, t.text_druhy);
    k.text(
        "Hľadať skladbu, album, interpreta",
        hx + 36.0,
        zakladna_v_strede(10.0, 32.0, TEXT),
        TEXT,
        t.text_druhy,
    );
    k.obdlznik(hx + hw - 48.0, 16.0, 36.0, 20.0, 4.0, t.prvok_pod_mysou);
    k.text_v_strede(
        "Ctrl F",
        hx + hw - 48.0,
        36.0,
        zakladna_v_strede(16.0, 20.0, POPIS),
        POPIS,
        t.text_druhy,
    );
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        false,
        W - 16.0 - TLACIDLO,
        8.0,
        TLACIDLO,
        Some(Ikona::Nastavenia),
        None,
    );
}

/// Časová os s kapitolami a načítanou časťou; `hraje` 0–1.
fn casova_os(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32, hraje: f32, cas: (&str, &str)) {
    k.obdlznik(x, y, w, STOPA, 2.0, t.prvok_stlaceny);
    k.obdlznik(x, y, w * 0.64, STOPA, 2.0, zmiesaj(t.prvok_stlaceny, t.text_druhy, 90));
    k.obdlznik(x, y, w * hraje, STOPA, 2.0, t.akcent);
    for z in [0.21f32, 0.55, 0.8] {
        k.obdlznik(x + (w * z).round() - 1.0, y - 3.0, 2.0, STOPA + 6.0, 1.0, t.okolie);
    }
    k.kruh(x + w * hraje, y + 2.0, 7.0, t.text);
    k.cisla(cas.0, x, y + 24.0, POPIS, t.text_druhy);
    k.cisla_vpravo(cas.1, x + w, y + 24.0, POPIS, t.text_druhy);
}

fn ovladanie(k: &mut Kreslic, t: &Tema, cx: f32, cy: f32, velke: f32) {
    let mal = TLACIDLO;
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        false,
        cx - velke / 2.0 - 16.0 - 2.0 * mal - 12.0,
        cy - mal / 2.0,
        mal,
        Some(Ikona::Nahodne),
        None,
    );
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        false,
        cx - velke / 2.0 - 16.0 - mal,
        cy - mal / 2.0,
        mal,
        Some(Ikona::Predosla),
        None,
    );
    k.kruh(cx, cy, velke / 2.0, t.akcent);
    let s = velke * 0.46;
    k.ikona(Ikona::Pauza, cx - s / 2.0, cy - s / 2.0, s, t.akcent_text);
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        false,
        cx + velke / 2.0 + 16.0,
        cy - mal / 2.0,
        mal,
        Some(Ikona::Dalsia),
        None,
    );
    prvky::tlacidlo(
        k,
        t,
        Druh::Ikona,
        Stav::Bezny,
        true,
        cx + velke / 2.0 + 16.0 + mal + 12.0,
        cy - mal / 2.0,
        mal,
        Some(Ikona::Opakovat),
        None,
    );
}

fn hlasitost(k: &mut Kreslic, t: &Tema, x: f32, cy: f32) {
    k.ikona(Ikona::Hlasitost, x, cy - 10.0, 20.0, t.text);
    k.obdlznik(x + 32.0, cy - 2.0, 120.0, STOPA, 2.0, t.prvok_stlaceny);
    k.obdlznik(x + 32.0, cy - 2.0, 84.0, STOPA, 2.0, t.akcent);
    k.kruh(x + 116.0, cy, 7.0, t.text);
    k.cisla("-6 dB", x + 164.0, cy + 5.0, POPIS, t.text_druhy);
    for (i, v) in [0.72f32, 0.64].iter().enumerate() {
        let yy = cy + 14.0 + i as f32 * 5.0;
        k.obdlznik(x + 32.0, yy, 120.0, 3.0, 1.5, t.prvok);
        k.obdlznik(x + 32.0, yy, 120.0 * v, 3.0, 1.5, t.uspech);
    }
}

/// Štítky formátu; zalomia sa do šírky `w`. Vráti spodok posledného riadku.
fn stitky_formatu(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32) -> f32 {
    let (mut cx, mut cy) = (x, y);
    for s in ["FLAC", "24 bitov / 96 kHz", "ReplayGain -6,2 dB", "bezmedzerovo"] {
        let sw = k.sirka(s, TEXT) + 24.0;
        if cx + sw > x + w {
            cx = x;
            cy += STITOK + 8.0;
        }
        cx += prvky::stitok(k, t, Stav::Bezny, cx, cy, s, false, STITOK) + 8.0;
    }
    cy + STITOK
}

fn spektrum(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32, h: f32, pozadie: bool) {
    if pozadie {
        k.obdlznik(x, y, w, h, R, t.plocha);
    }
    let n = 56;
    let bw = (w - 24.0) / n as f32;
    for i in 0..n {
        let u = i as f32 / n as f32;
        let v = ((u * 9.0).sin() * 0.3 + 0.55 - u * 0.35 + (u * 31.0).sin() * 0.08).clamp(0.05, 0.95);
        let bh = (v * (h - 24.0)).round();
        let f = zmiesaj(t.akcent, t.akcent_v_texte, (u * 255.0) as u8);
        k.obdlznik(x + 12.0 + i as f32 * bw, y + h - 12.0 - bh, bw - 3.0, bh, 2.0, f);
    }
}

/// Zoznam skladieb v pokojovom stave – zvýraznená len práve hrajúca skladba.
fn zoznam(k: &mut Kreslic, t: &Tema, x: f32, y: f32, w: f32, hraje: usize) {
    for (i, (n, d)) in SKLADBY.iter().enumerate() {
        let ry = y + i as f32 * 48.0;
        if i == hraje {
            k.obdlznik(x, ry, w, 44.0, R, t.prvok_pod_mysou);
        }
        if i == hraje {
            k.obdlznik(x, ry + 10.0, 3.0, 24.0, 1.5, t.akcent);
            k.ikona(Ikona::Prehrat, x + 14.0, ry + 14.0, 16.0, t.akcent_v_texte);
        } else {
            k.cisla_vpravo(
                &format!("{}", i + 1),
                x + 28.0,
                zakladna_v_strede(ry, 44.0, TEXT),
                TEXT,
                t.text_druhy,
            );
        }
        let rez = if i == hraje { TEXT_SILNY } else { TEXT };
        k.text(
            n,
            x + 44.0,
            zakladna_v_strede(ry, 44.0, rez),
            rez,
            if i == hraje { t.akcent_v_texte } else { t.text },
        );
        k.cisla_vpravo(d, x + w - 12.0, zakladna_v_strede(ry, 44.0, TEXT), TEXT, t.text_druhy);
    }
}

fn obal_s_tienom(k: &mut Kreslic, t: &Tema, o: &Obr, x: f32, y: f32, s: f32) {
    k.tien(x, y, s, s, R_VELKE, 18.0, t.tien.max(3));
    k.obraz(&o.body, o.w, o.h, x, y, s, s, R_VELKE);
}

fn variant_a(k: &mut Kreslic, t: &Tema, o: &Obr) {
    horna_lista(k, t);
    let lx = W - 360.0;
    let (ox, oy, os) = (48.0, LISTA + 48.0, 500.0);
    obal_s_tienom(k, t, o, ox, oy, os);
    let x = ox + os + 48.0;
    let pw = lx - 48.0 - x;
    k.text("Svetlo nad jazerom", x, oy + 28.0, TITUL, t.text);
    k.text(&format!("{INTERPRET} · 2026"), x, oy + 58.0, PODTITUL, t.text_druhy);
    let y = stitky_formatu(k, t, x, oy + 80.0, pw);
    let (sy, sh) = (y + 20.0, oy + 312.0 - y - 20.0);
    spektrum(k, t, x, sy, pw, sh, true);
    let iy = oy + 328.0;
    let kw = ((pw - 3.0 * 8.0) / 4.0).floor();
    for (i, (m, v)) in
        [("Kontajner", "FLAC"), ("Kanály", "2.0 stereo"), ("Tok", "2 304 kb/s"), ("Výstup", "priamo")]
            .iter()
            .enumerate()
    {
        let ix = x + i as f32 * (kw + 8.0);
        k.obdlznik(ix, iy, kw, 60.0, R, t.plocha);
        k.text(m, ix + 12.0, iy + 23.0, POPIS, t.text_druhy);
        k.cisla(v, ix + 12.0, iy + 44.0, TEXT_SILNY, t.text);
    }
    // značky albumu (údaje, ktoré balík media číta už dnes)
    let zy = oy + 404.0;
    k.obdlznik(x, zy, pw, os - 404.0, R, t.plocha);
    let mut y = zy + 26.0;
    for (m, v) in [("Album", ALBUM), ("Žáner", "klasická hudba"), ("Poradie", "skladba 2 z 8 · disk 1 z 1")]
    {
        k.text(m, x + 12.0, y, POPIS, t.text_druhy);
        k.cisla(v, x + 124.0, y, TEXT, t.text);
        y += 22.0;
    }
    // zoznam vpravo
    k.obdlznik(lx, LISTA, 360.0, H - LISTA, 0.0, t.plocha);
    k.obdlznik(lx, LISTA, 1.0, H - LISTA, 0.0, t.okraj);
    k.text("Zoznam skladieb", lx + 24.0, LISTA + 36.0, NADPIS, t.text);
    k.cisla_vpravo("8 skladieb · 40:35", lx + 336.0, LISTA + 36.0, POPIS, t.text_druhy);
    zoznam(k, t, lx + 12.0, LISTA + 56.0, 336.0, 1);
    // ovládanie dole
    let oy2 = H - 136.0;
    k.obdlznik(0.0, oy2, lx, 136.0, 0.0, t.plocha);
    k.obdlznik(0.0, oy2, lx, 1.0, 0.0, t.okraj);
    casova_os(k, t, 48.0, oy2 + 28.0, lx - 96.0, 0.38, ("1:58", "-3:14"));
    ovladanie(k, t, (lx / 2.0).round(), oy2 + 92.0, 56.0);
    hlasitost(k, t, lx - 48.0 - 200.0, oy2 + 92.0);
}

fn variant_b(k: &mut Kreslic, t: &Tema, foto: &Obr) {
    horna_lista(k, t);
    let (sw, mini) = (240.0, 80.0);
    k.obdlznik(0.0, LISTA, sw, H - LISTA - mini, 0.0, t.plocha);
    k.obdlznik(sw - 1.0, LISTA, 1.0, H - LISTA - mini, 0.0, t.okraj);
    let mut y = LISTA + 20.0;
    k.text("KNIŽNICA", 24.0, y + 11.0, POPIS, t.text_druhy);
    y += 24.0;
    for (i, (ik, s)) in [
        (Ikona::Katalog, "Albumy"),
        (Ikona::Hudba, "Interpreti"),
        (Ikona::Zoznam, "Skladby"),
        (Ikona::Hviezda, "Obľúbené"),
    ]
    .iter()
    .enumerate()
    {
        if i == 0 {
            k.obdlznik(12.0, y, sw - 24.0, 36.0, R_MALE, t.prvok_pod_mysou);
            k.obdlznik(12.0, y + 9.0, 3.0, 18.0, 1.5, t.akcent);
        }
        k.ikona(*ik, 28.0, y + 8.0, 20.0, if i == 0 { t.akcent_v_texte } else { t.text_druhy });
        k.text(s, 60.0, zakladna_v_strede(y, 36.0, TEXT), if i == 0 { TEXT_SILNY } else { TEXT }, t.text);
        y += 40.0;
    }
    y += 16.0;
    k.text("ZOZNAMY", 24.0, y + 11.0, POPIS, t.text_druhy);
    y += 24.0;
    for s in ["Na cestu", "Večer", "Sústredenie"] {
        k.ikona(Ikona::Zoznam, 28.0, y + 8.0, 20.0, t.text_druhy);
        k.text(s, 60.0, zakladna_v_strede(y, 36.0, TEXT), TEXT, t.text);
        y += 40.0;
    }
    prvky::tlacidlo(
        k,
        t,
        Druh::Druhe,
        Stav::Bezny,
        false,
        24.0,
        H - mini - 24.0 - TLACIDLO_MALE,
        TLACIDLO_MALE,
        Some(Ikona::Plus),
        Some("Pridať priečinok"),
    );
    // mriežka albumov
    let (gx, gw) = (sw + 32.0, W - sw - 64.0);
    let gy = LISTA + 28.0;
    k.text("Albumy", gx, gy + NADPIS_VELKY.velkost, NADPIS_VELKY, t.text);
    k.cisla(
        "124 albumov",
        gx + k.sirka("Albumy", NADPIS_VELKY) + 12.0,
        gy + NADPIS_VELKY.velkost,
        TEXT,
        t.text_druhy,
    );
    let mut cx = gx + gw;
    for (i, s) in ["Podľa roku", "Podľa interpreta", "Naposledy pridané"].iter().enumerate().rev() {
        let w = k.sirka(s, TEXT) + 24.0;
        cx -= w;
        prvky::stitok(k, t, Stav::Bezny, cx, gy - 4.0, s, i == 2, STITOK);
        cx -= 8.0;
    }
    let stlpce = 5;
    let medzera = 24.0;
    let s = ((gw - medzera * (stlpce - 1) as f32) / stlpce as f32).floor();
    let mena = [
        (ALBUM, INTERPRET),
        ("Mestá v daždi", "Kvarteto Brezno"),
        ("Púšť", "Ema Lúčna"),
        ("Hlboký les", "Tichý zbor"),
        ("Medená hodina", "Trio Kremeň"),
        ("Polárna noc", "Severka"),
        ("Jarné trhy", "Ľudová kapela Breza"),
        ("Stanica 9", "Elektrické ovce"),
        ("Dunaj", "Komorní hráči"),
        ("Sivá zóna", "Hmlovina"),
    ];
    for (i, (n, a)) in mena.iter().enumerate() {
        let (r, c) = (i / stlpce, i % stlpce);
        let x = gx + c as f32 * (s + medzera);
        let y = gy + 48.0 + r as f32 * (s + 64.0);
        if y + s > H - mini {
            break;
        }
        let o = if i == 0 { stvorec(foto) } else { obal(i, 256) };
        k.obraz(&o.body, o.w, o.h, x, y, s, s, R);
        if i == 0 {
            k.ram(x - 3.0, y - 3.0, s + 6.0, s + 6.0, R + 3.0, 2.0, t.akcent);
        }
        k.text(
            &k.pismo.skrat(n, s * k.mierka, TEXT_SILNY.velkost * k.mierka, TEXT_SILNY.hrubka * k.mierka),
            x,
            y + s + 20.0,
            TEXT_SILNY,
            t.text,
        );
        k.text(a, x, y + s + 38.0, POPIS, t.text_druhy);
    }
    // malý prehrávač dole
    let my = H - mini;
    k.obdlznik(0.0, my, W, mini, 0.0, t.plocha_vyssia);
    k.obdlznik(0.0, my, W, 1.0, 0.0, t.okraj);
    let o = stvorec(foto);
    k.obraz(&o.body, o.w, o.h, 16.0, my + 12.0, 56.0, 56.0, R_MALE);
    k.text("Svetlo nad jazerom", 88.0, my + 34.0, TEXT_SILNY, t.text);
    k.text(INTERPRET, 88.0, my + 52.0, POPIS, t.text_druhy);
    ovladanie(k, t, W / 2.0, my + 32.0, 44.0);
    casova_os(k, t, W / 2.0 - 260.0, my + 64.0, 520.0, 0.38, ("", ""));
    k.cisla_vpravo("1:58", W / 2.0 - 272.0, my + 69.0, POPIS, t.text_druhy);
    k.cisla("5:12", W / 2.0 + 272.0, my + 69.0, POPIS, t.text_druhy);
    hlasitost(k, t, W - 24.0 - 200.0, my + 36.0);
}

fn variant_c(k: &mut Kreslic, t: &Tema, o: &Obr) {
    // pozadie: farba obalu vmiešaná do okolia témy – vo svetlej téme ako pastel so svietivosťou okolia (odtieň ostane,
    // text nestratí kontrast); najsilnejšie vmiešanie sa aj tak dopočíta, aby text, druhý text a akcent v texte
    // mali 4,5 : 1 (7 : 1) pri každom obale
    let farba = if t.tmava { priemer(o) } else { farba_so_svietivostou(priemer(o), svietivost(t.okolie)) };
    let min = if t.vysoky_kontrast { 7.0 } else { 4.5 };
    let najviac = najsilnejsie_vmiesanie(
        t.okolie,
        farba,
        if t.tmava { 70 } else { 55 },
        &[t.text, t.text_druhy, t.akcent_v_texte],
        min,
    ) as f32;
    let (pw, ph) = (k.p.sirka, k.p.vyska);
    for y in 0..ph {
        let v = y as f32 / ph as f32;
        for x in 0..pw {
            let u = x as f32 / pw as f32;
            let sila = ((1.0 - (u - 0.25).hypot(v - 0.45) * 1.3).clamp(0.0, 1.0) * najviac) as u8;
            k.p.body[y * pw + x] = zmiesaj(t.okolie, farba, sila);
        }
    }
    prvky::tlacidlo(k, t, Druh::Ikona, Stav::Bezny, false, 24.0, 20.0, TLACIDLO, Some(Ikona::Dozadu), None);
    k.text("Knižnica", 68.0, zakladna_v_strede(20.0, TLACIDLO, TEXT), TEXT, t.text_druhy);
    let (ox, os) = (72.0, 440.0);
    let oy = ((H - os) / 2.0).round();
    obal_s_tienom(k, t, o, ox, oy, os);
    let x = ox + os + 56.0;
    let cw = W - 48.0 - 288.0 - 48.0 - x;
    k.text("TERAZ HRÁ · 2 / 8", x, oy + 14.0, POPIS, t.akcent_v_texte);
    k.text(
        "Svetlo nad jazerom",
        x,
        oy + 64.0,
        Rez { meno: "titul veľký", velkost: 34.0, hrubka: 2.9 },
        t.text,
    );
    k.text(&format!("{INTERPRET} · {ALBUM}"), x, oy + 98.0, PODTITUL, t.text_druhy);
    let y = stitky_formatu(k, t, x, oy + 120.0, cw);
    spektrum(k, t, x - 12.0, y + 8.0, cw + 24.0, oy + 300.0 - y - 8.0, false);
    casova_os(k, t, x, oy + 328.0, cw, 0.38, ("1:58", "-3:14"));
    ovladanie(k, t, x + cw / 2.0, oy + os - 32.0, 64.0);
    // ďalej v poradí
    let (cx, kw) = (W - 48.0 - 288.0, 288.0);
    let cy = oy;
    k.obdlznik_a(cx, cy, kw, 356.0, R_VELKE, t.plocha, 230.0);
    k.text("Ďalej", cx + 20.0, cy + 34.0, NADPIS, t.text);
    for (i, (n, d)) in SKLADBY[2..7].iter().enumerate() {
        let ry = cy + 56.0 + i as f32 * 58.0;
        // skladby toho istého albumu – ten istý obal
        k.obraz(&o.body, o.w, o.h, cx + 20.0, ry, 44.0, 44.0, R_MALE);
        k.text(n, cx + 76.0, ry + 18.0, TEXT_SILNY, t.text);
        k.cisla(d, cx + 76.0, ry + 36.0, POPIS, t.text_druhy);
    }
    hlasitost(k, t, W - 48.0 - 200.0, oy + os - 32.0);
}

/// Obrazovka LÝRY v téme `t`, variante `v` a mierke `m`.
pub fn obrazovka(t: &Tema, v: char, m: f32, foto: &Obr) -> Kreslic {
    let mut k = Kreslic::novy(W, H, m, t.okolie);
    let o = stvorec(foto);
    match v {
        'b' => variant_b(&mut k, t, foto),
        'c' => variant_c(&mut k, t, &o),
        _ => variant_a(&mut k, t, &o),
    }
    k
}
