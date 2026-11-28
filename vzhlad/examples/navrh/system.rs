//! Listy dizajnového systému: témy s nameraným kontrastom, prvky vo všetkých stavoch, ikony na mriežke, písmo.

use crate::obrazky::{Obr, fotka};
use orchidea_pismo::zmiesaj;
use orchidea_vzhlad::ikony::{Ikona, VSETKY};
use orchidea_vzhlad::kontrast::{Poziadavka, kontrola};
use orchidea_vzhlad::kreslic::{Kreslic, zakladna_v_strede};
use orchidea_vzhlad::miery::*;
use orchidea_vzhlad::prvky::{self, Druh, STAVY, Stav};
use orchidea_vzhlad::tema::{NOC, TEMY, Tema};

/// Meno úlohy farby a jej výber z témy.
type Rola = (&'static str, fn(&Tema) -> u32);

fn desatinne(x: f64) -> String {
    format!("{x:.2}").replace('.', ",")
}

fn hlavicka(k: &mut Kreslic, t: &Tema, nadpis: &str, popis: &str) {
    k.text(nadpis, 40.0, 56.0, NADPIS_VELKY, t.text);
    k.text(popis, 40.0, 80.0, TEXT, t.text_druhy);
}

/// Malá ukážka témy: fotka v okolí, hlavné a druhé tlačidlo, posuvník, štítky.
fn ukazka(k: &mut Kreslic, t: &Tema, f: &Obr, x: f32, y: f32, w: f32) {
    k.obdlznik(x, y, w, 96.0, R, t.okolie);
    k.obraz(&f.body, f.w, f.h, x + 12.0, y + 12.0, 108.0, 72.0, 2.0);
    k.text("Jas", x + 132.0, y + 26.0, POPIS, t.text_druhy);
    k.obdlznik(x + 132.0, y + 38.0, w - 144.0, STOPA, 2.0, t.prvok_stlaceny);
    k.obdlznik(x + 132.0, y + 38.0, (w - 144.0) * 0.6, STOPA, 2.0, t.akcent);
    k.kruh(x + 132.0 + (w - 144.0) * 0.6, y + 40.0, 7.0, t.text);
    k.kruh(x + 132.0 + (w - 144.0) * 0.6, y + 40.0, 4.0, t.akcent);
    k.ikona(Ikona::Hviezda, x + 132.0, y + 60.0, 16.0, t.hviezda);
    k.ikona(Ikona::Hviezda, x + 152.0, y + 60.0, 16.0, t.hviezda);
    k.ikona(Ikona::HviezdaPrazdna, x + 172.0, y + 60.0, 16.0, t.okraj_silny);
    let y = y + 108.0;
    let a =
        prvky::tlacidlo(k, t, Druh::Hlavne, Stav::Bezny, false, x, y, TLACIDLO_MALE, None, Some("Uložiť"));
    prvky::tlacidlo(
        k,
        t,
        Druh::Druhe,
        Stav::Bezny,
        false,
        x + a + 8.0,
        y,
        TLACIDLO_MALE,
        None,
        Some("Zrušiť"),
    );
    let y = y + 40.0;
    let a = prvky::stitok(k, t, Stav::Bezny, x, y, "3:2", true, 24.0);
    let b = prvky::stitok(k, t, Stav::Bezny, x + a + 6.0, y, "4:3", false, 24.0);
    k.text("hrá", x + a + b + 18.0, zakladna_v_strede(y, 24.0, TEXT_SILNY), TEXT_SILNY, t.akcent_v_texte);
}

/// List všetkých tém vedľa seba – každý stĺpec kreslený vo svojej téme, s nameraným kontrastom.
pub fn temy(m: f32) -> Kreslic {
    let (sw, medzera) = (264.0, 16.0);
    let w = TEMY.len() as f32 * sw + (TEMY.len() + 1) as f32 * medzera;
    let h = 1000.0;
    let mut k = Kreslic::novy(w, h, m, 0x26272B);
    let f = fotka(300, 200);
    k.text("Témy vzhľadu (Okvetie) – návrh", medzera + 8.0, 52.0, NADPIS_VELKY, 0xEDEDF0);
    k.text(
        "Každý stĺpec je nakreslený vo svojej téme. Kontrast je nameraný kódom (WCAG 2.2) pre každý pár text/pozadie, ktorý sa v téme stretne.",
        medzera + 8.0,
        76.0,
        TEXT,
        0xB4B8C0,
    );
    let roly: [Rola; 18] = [
        ("okolie obrazu", |t| t.okolie),
        ("plocha", |t| t.plocha),
        ("plocha vyššia", |t| t.plocha_vyssia),
        ("prvok", |t| t.prvok),
        ("prvok pod myšou", |t| t.prvok_pod_mysou),
        ("prvok stlačený", |t| t.prvok_stlaceny),
        ("oddeľovač", |t| t.okraj),
        ("okraj poľa", |t| t.okraj_silny),
        ("text", |t| t.text),
        ("text druhý", |t| t.text_druhy),
        ("text vypnutý", |t| t.text_vypnuty),
        ("akcent", |t| t.akcent),
        ("text na akcente", |t| t.akcent_text),
        ("akcent v texte", |t| t.akcent_v_texte),
        ("rámik zamerania", |t| t.zameranie),
        ("úspech", |t| t.uspech),
        ("hviezda", |t| t.hviezda),
        ("chyba", |t| t.chyba),
    ];
    for (i, t) in TEMY.iter().enumerate() {
        let x = medzera + i as f32 * (sw + medzera);
        let y = 104.0;
        k.obdlznik(x, y, sw, h - y - medzera, R_VELKE, t.plocha);
        k.ram_dnu(x, y, sw, h - y - medzera, R_VELKE, 1.0, t.okraj);
        let (x, w) = (x + 16.0, sw - 32.0);
        let mut y = y + 20.0;
        k.text(t.meno, x, y + NADPIS_VELKY.velkost, NADPIS_VELKY, t.text);
        if i == 0 {
            let s = "predvolená";
            k.text_vpravo(s, x + w, y + NADPIS_VELKY.velkost, POPIS, t.akcent_v_texte);
        }
        y += NADPIS_VELKY.velkost + 20.0;
        k.text(t.druh, x, y, TEXT, t.text_druhy);
        y += 16.0;
        ukazka(&mut k, t, &f, x, y, w);
        y += 108.0 + 40.0 + 24.0 + 20.0;
        for (meno, farba) in roly {
            let c = farba(t);
            k.obdlznik(x, y, 20.0, 20.0, 4.0, c);
            k.ram_dnu(x, y, 20.0, 20.0, 4.0, 1.0, zmiesaj(c, t.text, 50));
            k.text(meno, x + 28.0, y + 14.0, POPIS, t.text_druhy);
            k.cisla_vpravo(&format!("{c:06X}"), x + w, y + 14.0, POPIS, t.text);
            y += 26.0;
        }
        y += 12.0;
        let pary = kontrola(t);
        let min = |p: Poziadavka| {
            pary.iter().filter(|x| x.poziadavka == p).map(|x| x.pomer).fold(f64::MAX, f64::min)
        };
        let vsetko = pary.iter().all(|p| p.splna());
        k.obdlznik(x, y, w, 92.0, R, t.prvok);
        let uroven = if t.vysoky_kontrast { "WCAG 2.2 AAA" } else { "WCAG 2.2 AA" };
        k.text(uroven, x + 12.0, y + 22.0, TEXT_SILNY, if vsetko { t.uspech } else { t.chyba });
        k.text(
            if vsetko { "✓" } else { "×" },
            x + w - 24.0,
            y + 22.0,
            TEXT_SILNY,
            if vsetko { t.uspech } else { t.chyba },
        );
        k.text("text, najmenej", x + 12.0, y + 46.0, POPIS, t.text_druhy);
        k.cisla_vpravo(
            &format!("{} : 1", desatinne(min(Poziadavka::Text))),
            x + w - 12.0,
            y + 46.0,
            TEXT,
            t.text,
        );
        k.text("grafika, najmenej", x + 12.0, y + 66.0, POPIS, t.text_druhy);
        k.cisla_vpravo(
            &format!("{} : 1", desatinne(min(Poziadavka::Grafika))),
            x + w - 12.0,
            y + 66.0,
            TEXT,
            t.text,
        );
        let n = pary.iter().filter(|p| p.poziadavka != Poziadavka::Vynimka).count();
        k.cisla(&format!("{n} párov"), x + 12.0, y + 84.0, POPIS, t.text_druhy);
    }
    k
}

/// List prvkov vo všetkých stavoch v téme `t`.
pub fn prvky_list(t: &Tema, m: f32) -> Kreslic {
    let (w, h) = (1440.0, 900.0);
    let mut k = Kreslic::novy(w, h, m, t.plocha);
    hlavicka(
        &mut k,
        t,
        &format!("Prvky a ich stavy · téma {} ({})", t.druh, t.meno),
        "Bežný, pod myšou, stlačený, zameraný (klávesnica, ovládač), vypnutý. Rámik zamerania 2 body, 2 body od prvku.",
    );
    let (x0, sw, y0) = (240.0, 236.0, 128.0);
    for (i, s) in STAVY.iter().enumerate() {
        k.text(s.meno(), x0 + i as f32 * sw, y0, TEXT_SILNY, t.text_druhy);
    }
    let riadky = [
        "Hlavné tlačidlo",
        "Druhé tlačidlo",
        "Tlačidlo s ikonou",
        "Zapnutý nástroj",
        "Štítok",
        "Vybraný štítok",
        "Prepínač",
        "Pole s hodnotou",
        "Posuvník od stredu",
    ];
    for (r, nazov) in riadky.iter().enumerate() {
        let y = y0 + 32.0 + r as f32 * 68.0;
        k.obdlznik(40.0, y - 14.0, w - 80.0, 1.0, 0.0, t.okraj);
        k.text(nazov, 40.0, y + 24.0, TEXT, t.text);
        for (i, &stav) in STAVY.iter().enumerate() {
            let x = x0 + i as f32 * sw;
            match r {
                0 => {
                    prvky::tlacidlo(
                        &mut k,
                        t,
                        Druh::Hlavne,
                        stav,
                        false,
                        x,
                        y,
                        TLACIDLO,
                        Some(Ikona::Ulozit),
                        Some("Uložiť"),
                    );
                }
                1 => {
                    prvky::tlacidlo(
                        &mut k,
                        t,
                        Druh::Druhe,
                        stav,
                        false,
                        x,
                        y,
                        TLACIDLO,
                        None,
                        Some("Zrušiť"),
                    );
                }
                2 => {
                    prvky::tlacidlo(
                        &mut k,
                        t,
                        Druh::Ikona,
                        stav,
                        false,
                        x,
                        y,
                        TLACIDLO,
                        Some(Ikona::Orezat),
                        None,
                    );
                }
                3 => {
                    prvky::tlacidlo(
                        &mut k,
                        t,
                        Druh::Ikona,
                        stav,
                        true,
                        x,
                        y,
                        TLACIDLO,
                        Some(Ikona::Orezat),
                        None,
                    );
                }
                4 => {
                    prvky::stitok(&mut k, t, stav, x, y + 4.0, "jazero", false, STITOK);
                }
                5 => {
                    prvky::stitok(&mut k, t, stav, x, y + 4.0, "3:2", true, STITOK);
                }
                6 => {
                    prvky::prepinac(&mut k, t, stav, x, y + 6.0, false);
                    prvky::prepinac(&mut k, t, stav, x + 56.0, y + 6.0, true);
                }
                7 => prvky::pole(&mut k, t, stav, x, y + 2.0, 180.0, "6000", "bodov"),
                _ => prvky::posuvnik(&mut k, t, stav, x, y - 2.0, 196.0, "Kontrast", "+20", 0.2, true),
            }
        }
    }
    let y = y0 + 32.0 + riadky.len() as f32 * 68.0 + 8.0;
    k.obdlznik(40.0, y - 14.0, w - 80.0, 1.0, 0.0, t.okraj);
    k.text("Bublina s popisom", 40.0, y + 40.0, TEXT, t.text);
    prvky::bublina(&mut k, t, x0 + 60.0, y + 56.0, "Orezať", Some("C"));
    prvky::bublina(&mut k, t, x0 + sw + 90.0, y + 56.0, "Otočiť doprava", Some("R"));
    k.text(
        "Zapnutý nástroj: farba aj tvar (pásik dole) – rozlíšiteľné aj pri poruche farbocitu.",
        x0 + 2.0 * sw + 40.0,
        y + 32.0,
        POPIS,
        t.text_druhy,
    );
    k.text(
        "Vypnutý prvok je výnimka WCAG 1.4.3 – ostáva viditeľný, ale nevyzýva na kliknutie.",
        x0 + 2.0 * sw + 40.0,
        y + 52.0,
        POPIS,
        t.text_druhy,
    );
    k
}

/// List ikon na mriežke 20 × 20 (v mierke 1 : 1) a zväčšené ikony s mriežkou.
pub fn ikony(m: f32) -> Kreslic {
    let t = &NOC;
    let (w, h) = (1440.0, 760.0);
    let mut k = Kreslic::novy(w, h, m, t.plocha);
    hlavicka(
        &mut k,
        t,
        "Ikony – jedna mriežka 20 × 20 bodov, jeden ťah 1,7 bodu",
        "Živá plocha 2,5–17,5; okrúhle konce; vyplnené len Prehrať, hviezda a polovica témy; neobsadená hviezda je obrys. Popis je zároveň text bubliny.",
    );
    let (stlpce, cw, ch) = (9, 150.0, 92.0);
    for (i, ik) in VSETKY.iter().enumerate() {
        let (r, c) = (i / stlpce, i % stlpce);
        let (x, y) = (40.0 + c as f32 * cw, 116.0 + r as f32 * ch);
        k.obdlznik(x, y, 40.0, 40.0, R_MALE, t.prvok);
        k.ikona(*ik, x + 10.0, y + 10.0, 20.0, t.text);
        k.text(ik.meno(), x, y + 60.0, POPIS, t.text_druhy);
    }
    // zväčšené s mriežkou
    let y = 116.0 + 4.0 * ch + 24.0;
    k.text("Zväčšené 6 × s mriežkou a živou plochou", 40.0, y, TEXT_SILNY, t.text);
    for (i, ik) in
        [Ikona::Orezat, Ikona::Prehrat, Ikona::Nastavenia, Ikona::Fotka, Ikona::Hudba, Ikona::Hviezda]
            .iter()
            .enumerate()
    {
        let (x, yy, s) = (40.0 + i as f32 * 200.0, y + 20.0, 120.0);
        k.obdlznik(x, yy, s, s, 0.0, t.okolie);
        for j in 0..=20 {
            let p = j as f32 * s / 20.0;
            let a = if j % 5 == 0 { 60.0 } else { 24.0 };
            k.obdlznik_a(x + p, yy, 1.0 / m, s, 0.0, t.text_druhy, a);
            k.obdlznik_a(x, yy + p, s, 1.0 / m, 0.0, t.text_druhy, a);
        }
        let z = 2.5 * s / 20.0;
        k.ram(x + z, yy + z, s - 2.0 * z, s - 2.0 * z, 0.0, 1.0, t.akcent);
        k.ikona(*ik, x, yy, s, t.text);
    }
    k
}

/// List typografickej stupnice písma STONKA vrátane obrazovky na diaľku a číslic rovnakej šírky.
pub fn pismo(m: f32) -> Kreslic {
    let t = &NOC;
    let (w, h) = (1440.0, 1000.0);
    let mut k = Kreslic::novy(w, h, m, t.plocha);
    hlavicka(
        &mut k,
        t,
        "Písmo STONKA – typografická stupnica",
        "Veľkosť = výška veľkého písmena v bodoch, hrúbka = ťah. Obrazovka počítača asi 60 cm, TV asi 3 m (veľké písmeno aspoň 16, text 20 oblúkových minút).",
    );
    let mut y = 140.0;
    for r in STUPNICA {
        let tv = r.meno.starts_with("TV");
        let vzor = if tv {
            "Svetlo nad jazerom 20:45"
        } else {
            "Príliš žltučký kôň úpel ďábelské ódy · 0123456789"
        };
        k.text(r.meno, 40.0, y, TEXT_SILNY, t.text);
        let mut opis = format!(
            "{} / {}",
            desatinne(r.velkost as f64).trim_end_matches(",00"),
            desatinne(r.hrubka as f64)
        );
        if tv {
            let u = uhol_znaku(r.velkost, TV_RIADKY, TV_UHLOPRIECKA_PALCE, TV_VZDIALENOST_M);
            opis.push_str(&format!(" · {} oblúk. min.", desatinne(u as f64)));
        }
        k.cisla(&opis, 40.0, y + 18.0, POPIS, t.text_druhy);
        k.text(vzor, 260.0, y + (r.velkost - 12.0).max(0.0) * 0.5, r, t.text);
        y += (r.velkost * 1.9).max(44.0);
    }
    // číslice rovnakej šírky
    let y = y + 8.0;
    k.obdlznik(40.0, y - 20.0, w - 80.0, 1.0, 0.0, t.okraj);
    k.text("Číslice rovnakej šírky – časy a hodnoty sa pri zmene nehýbu", 40.0, y + 6.0, TEXT_SILNY, t.text);
    for (i, s) in ["1:11", "8:48", "10:05", "-0:07"].iter().enumerate() {
        let yy = y + 36.0 + i as f32 * 22.0;
        k.text_vpravo(s, 340.0, yy, TEXT, t.text_druhy);
        k.cisla_vpravo(s, 520.0, yy, TEXT, t.text);
    }
    k.text("bežné", 300.0, y + 128.0, POPIS, t.text_druhy);
    k.text("rovnakej šírky", 448.0, y + 128.0, POPIS, t.text_druhy);
    k.text(
        "Chýba v písme (doplniť): pomlčka mínus, šípky, zvislá čiara, úvodzovky.",
        640.0,
        y + 36.0,
        POPIS,
        t.text_druhy,
    );
    k
}
