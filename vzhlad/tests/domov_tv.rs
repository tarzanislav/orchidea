//! Domovská obrazovka TV krabičky: pri každej voľbe „veľkosť písma“ je všetok text v bezpečnej ploche (EBU R 95)
//! a žiadny nie je menší ako popis stupnice (uhol znaku ≥ 16′); na každej obrazovke je práve jeden rámik
//! zamerania a akcentová plocha ovládacieho prvku najviac jedna – v rámiku.

#![allow(clippy::too_many_arguments, dead_code)]

#[path = "../examples/navrh/domov.rs"]
mod domov;
#[path = "../examples/navrh/obrazky.rs"]
mod obrazky;

use orchidea_vzhlad::kreslic::merat_kontrast;
use orchidea_vzhlad::miery::*;
use orchidea_vzhlad::tema::{NOC, TEMY, Tema};

fn over_text(meno: &str, t: &Tema, c: char, st: &StupnicaTv, foto: &obrazky::Obr) {
    merat_kontrast(true);
    let k = domov::obrazovka(t, c, 1.0, st, foto);
    merat_kontrast(false);
    let merania = k.merania.expect("meranie zapnuté");
    assert!(merania.iter().filter(|m| !m.ikona).count() >= 3, "{meno}: málo textov");
    for m in merania.iter().filter(|m| !m.ikona) {
        assert!(
            m.x >= TV_OKRAJ_X - 0.5 && m.x + m.w <= 1920.0 - TV_OKRAJ_X + 0.5,
            "{meno}: „{}“ vodorovne mimo bezpečnej plochy ({:.0} až {:.0})",
            m.co,
            m.x,
            m.x + m.w
        );
        assert!(
            m.y >= TV_OKRAJ_Y - 0.5 && m.y + m.h <= 1080.0 - TV_OKRAJ_Y + 0.5,
            "{meno}: „{}“ zvislo mimo bezpečnej plochy ({:.0} až {:.0})",
            m.co,
            m.y,
            m.y + m.h
        );
        assert!(m.velkost + 1e-3 >= st.popis.velkost, "{meno}: „{}“ menší ako popis ({})", m.co, m.velkost);
    }
}

#[test]
fn text_domova_je_citatelny_a_v_bezpecnej_ploche_pri_kazdej_velkosti_pisma() {
    let foto = obrazky::fotka(1200, 800);
    let velkosti: Vec<VelkostPisma> =
        if cfg!(target_arch = "x86_64") { VelkostPisma::VSETKY.to_vec() } else { vec![VelkostPisma::Bezne] };
    for v in velkosti {
        // 50″ z EDID a zvolená veľkosť písma; Panoráma sa prispôsobuje stupnici
        let st = StupnicaTv::pre(&MeraniaObrazovky::z_edid(111, 62, v));
        over_text(&format!("domov_1 ({})", v.text()), &NOC, '1', &st, &foto);
    }
    if cfg!(target_arch = "x86_64") {
        for c in ['2', '3', 'p', 's'] {
            over_text(&format!("domov_{c}"), &NOC, c, &StupnicaTv::REFERENCNA, &foto);
        }
    }
}

/// Súvislé oblasti bodov presne farby `f` (4-susednosť): (x0, y0, x1, y1, počet).
fn oblasti(p: &orchidea_pismo::Platno, f: u32) -> Vec<(usize, usize, usize, usize, usize)> {
    let (w, h) = (p.sirka, p.vyska);
    let mut videny = vec![false; w * h];
    let mut v = Vec::new();
    for start in 0..w * h {
        if videny[start] || p.body[start] != f {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1, mut n) = (w, h, 0, 0, 0);
        let mut zasobnik = vec![start];
        videny[start] = true;
        while let Some(i) = zasobnik.pop() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1, n) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y), n + 1);
            for j in [i.wrapping_sub(1), i + 1, i.wrapping_sub(w), i + w] {
                let susedny = j < w * h && (j / w == y || j % w == x);
                if susedny && !videny[j] && p.body[j] == f {
                    videny[j] = true;
                    zasobnik.push(j);
                }
            }
        }
        v.push((x0, y0, x1, y1, n));
    }
    v
}

#[test]
fn domov_ma_prave_jedno_zameranie() {
    let foto = obrazky::fotka(1200, 800);
    let temy: Vec<&Tema> = if cfg!(target_arch = "x86_64") { TEMY.to_vec() } else { vec![&NOC] };
    for t in temy {
        for c in ['1', '2', '3', 'p'] {
            let k = domov::obrazovka(t, c, 1.0, &StupnicaTv::REFERENCNA, &foto);
            let meno = format!("domov_{c} ({})", t.meno);
            let ramiky: Vec<_> = oblasti(&k.p, t.zameranie)
                .into_iter()
                .filter(|o| o.2 - o.0 >= 40 && o.3 - o.1 >= 40)
                .collect();
            assert_eq!(ramiky.len(), 1, "{meno}: rámikov zamerania {}", ramiky.len());
            let r = ramiky[0];
            for o in oblasti(&k.p, t.akcent)
                .into_iter()
                .filter(|o| o.3 - o.1 + 1 >= 56 && o.4 * 2 >= (o.2 - o.0 + 1) * (o.3 - o.1 + 1))
            {
                assert!(
                    o.0 > r.0 && o.1 > r.1 && o.2 < r.2 && o.3 < r.3,
                    "{meno}: akcent {o:?} mimo rámika {r:?}"
                );
            }
        }
    }
}
