//! Zameranie na obrazovke TV krabičky (ovládanie diaľkovým ovládačom z ~3 m): **akcent znamená „tu je
//! zameranie“** (kritika A, 28. 11. 2026). Na každej TV obrazovke je práve jeden rámik zamerania a akcentová plocha
//! veľkosti ovládacieho prvku (výška aspoň 56 bodov, z väčšej časti plná) je najviac jedna – a leží v rámiku.
//! Keď zameranie prejde z Prehrať na Informácie, akcent ide s ním. Tenké ukazovatele (priebeh pozerania) a text sa
//! nepočítajú – nie sú to vyznačené prvky.

#![allow(clippy::too_many_arguments, dead_code)]

#[path = "../examples/navrh/obrazky.rs"]
mod obrazky;
#[path = "../examples/navrh/pohlad.rs"]
mod pohlad;
#[path = "../examples/navrh/tv.rs"]
mod tv;

use orchidea_pismo::{Farba, Platno};
use orchidea_vzhlad::tema::{NOC, TEMY, Tema};

/// Súvislé oblasti bodov presne farby `f` (4-susednosť): (x0, y0, x1, y1, počet bodov).
fn oblasti(p: &Platno, f: Farba) -> Vec<(usize, usize, usize, usize, usize)> {
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
            let mut skus = |j: usize| {
                if !videny[j] && p.body[j] == f {
                    videny[j] = true;
                    zasobnik.push(j);
                }
            };
            if x > 0 {
                skus(i - 1);
            }
            if x + 1 < w {
                skus(i + 1);
            }
            if y > 0 {
                skus(i - w);
            }
            if y + 1 < h {
                skus(i + w);
            }
        }
        v.push((x0, y0, x1, y1, n));
    }
    v
}

fn over(meno: &str, t: &Tema, p: &Platno, akcent_musi_byt: bool) {
    let ramiky: Vec<_> =
        oblasti(p, t.zameranie).into_iter().filter(|o| o.2 - o.0 >= 40 && o.3 - o.1 >= 40).collect();
    assert_eq!(ramiky.len(), 1, "{meno}: rámikov zamerania {}", ramiky.len());
    let r = ramiky[0];
    let plochy: Vec<_> = oblasti(p, t.akcent)
        .into_iter()
        .filter(|o| o.3 - o.1 + 1 >= 56 && o.4 * 2 >= (o.2 - o.0 + 1) * (o.3 - o.1 + 1))
        .collect();
    assert!(plochy.len() <= 1, "{meno}: akcentových plôch {} – {plochy:?}", plochy.len());
    assert!(!akcent_musi_byt || plochy.len() == 1, "{meno}: zamerané tlačidlo nemá akcent");
    for o in plochy {
        assert!(o.0 > r.0 && o.1 > r.1 && o.2 < r.2 && o.3 < r.3, "{meno}: akcent {o:?} mimo rámika {r:?}");
    }
}

#[test]
fn akcent_na_tv_je_len_pri_zamerani() {
    let foto = obrazky::fotka(1200, 800);
    let temy: Vec<&Tema> = if cfg!(target_arch = "x86_64") { TEMY.to_vec() } else { vec![&NOC] };
    for t in temy {
        // mimo x86-64 (ARM v QEMU, balík skúšok pre skutočné Raspberry Pi) len zameranie na Informácie – 1 obrazovka
        let zamerania = if cfg!(target_arch = "x86_64") { 0..3 } else { 1..2 };
        for z in zamerania {
            let k = tv::obrazovka_so_zameranim(t, 'a', 1.0, &foto, z);
            over(&format!("tv_a ({}, zameranie {z})", t.meno), t, &k.p, true);
        }
        if cfg!(target_arch = "x86_64") {
            for v in ['b', 'c'] {
                let k = tv::obrazovka(t, v, 1.0, &foto);
                over(&format!("tv_{v} ({})", t.meno), t, &k.p, false);
            }
        }
    }
}
