//! Kontrast **na nakreslených obrazovkách** návrhu (WCAG 2.2) – každý text a každá ikona sa zmeria voči tomu, čo
//! bolo pod nimi naozaj nakreslené (plocha, fotka, plagát, farba z obalu, priesvitná clona). Tabuľka v `kontrast`
//! meria páry farieb témy; táto skúška stráži, že obrazovky tie páry naozaj použili a že text nad obrázkom má
//! dosť silnú clonu.
//!
//! Požiadavky: text 4,5 : 1 (1.4.3), v téme s vysokým kontrastom 7 : 1 (1.4.6); veľký text 3 : 1 (4,5 : 1);
//! ikona 3 : 1 (1.4.11); vypnuté prvky sú výnimka. **Veľký text** podľa WCAG je od 18 pt (24 obrazových bodov
//! CSS) – pri písme STONKA výška veľkých písmen od 17 bodov. Na TV sa výnimka pre veľký text počíta podľa uhla
//! znaku z 3 m: TV text (32 bodov) má rovnaký uhol ako bežný text na počítači, preto platí až od TV nadpisu
//! (44 bodov).
//!
//! Meria sa **najhorší bod** pod textom (pás od základnej čiary po výšku veľkých písmen) – nad fotkou rozhoduje
//! najsvetlejšie (najtmavšie) miesto, nie priemer.
//!
//! Na x86-64 (CI): 6 tém × 3 varianty × 3 obrazovky (prehliadač a editor fotografií (Pohľad), prehrávač médií LÝRA,
//! obrazovka TV krabičky) = 54 obrazoviek.

#![allow(clippy::too_many_arguments, dead_code)]

#[path = "../examples/navrh/lyra.rs"]
mod lyra;
#[path = "../examples/navrh/obrazky.rs"]
mod obrazky;
#[path = "../examples/navrh/pohlad.rs"]
mod pohlad;
#[path = "../examples/navrh/tv.rs"]
mod tv;

use orchidea_vzhlad::kreslic::{Kreslic, Meranie, merat_kontrast};
use orchidea_vzhlad::miery::TV_NADPIS;
use orchidea_vzhlad::tema::{LAN, NOC, TEMY, Tema};

/// Požadované minimum pre jedno meranie (`None` = výnimka: vypnutý prvok).
fn minimum(t: &Tema, m: &Meranie, tv: bool) -> Option<f64> {
    if m.farba == t.text_vypnuty {
        return None;
    }
    if m.ikona {
        return Some(3.0);
    }
    let velky = if tv { m.velkost >= TV_NADPIS.velkost } else { m.velkost >= 17.0 };
    Some(match (t.vysoky_kontrast, velky) {
        (false, false) => 4.5,
        (false, true) => 3.0,
        (true, false) => 7.0,
        (true, true) => 4.5,
    })
}

/// Jedna obrazovka na meranie: meno, téma, či je to TV, kreslenie.
type Pripad<'a> = (String, &'a Tema, bool, Box<dyn Fn() -> Kreslic + 'a>);

/// Zmeria jednu obrazovku; vráti počet meraní a zoznam nedostatkov.
fn zmeraj(meno: &str, t: &Tema, tv: bool, kresli: impl Fn() -> Kreslic) -> (usize, Vec<String>) {
    merat_kontrast(true);
    let k = kresli();
    merat_kontrast(false);
    let merania = k.merania.expect("meranie zapnuté");
    let mut zle = Vec::new();
    for m in &merania {
        if let Some(min) = minimum(t, m, tv)
            && m.najmensi + 1e-9 < min
        {
            zle.push(format!(
                "{meno}: {} „{}“ #{:06X} {:.0}×{:.0} na ({:.0}, {:.0}): {:.2} : 1 (98 % bodov {:.2}), treba {min}",
                if m.ikona { "ikona" } else { "text" },
                m.co,
                m.farba,
                m.w,
                m.h,
                m.x,
                m.y,
                m.najmensi,
                m.percentil_2
            ));
        }
    }
    (merania.len(), zle)
}

#[test]
fn text_a_ikony_na_obrazovkach_splnaju_kontrast() {
    let foto = obrazky::fotka(1200, 800);
    let mut pripady: Vec<Pripad> = Vec::new();
    // každá téma s každým variantom – v programe sa dajú zvoliť nezávisle. Mimo x86-64 (ARM v QEMU, balík skúšok
    // pre skutočné Raspberry Pi vrátane RPi Zero, WebAssembly) len dve menšie obrazovky – overí sa kód merania,
    // plné pokrytie beží v CI na x86-64 (chladný systém aj pri skúškach)
    if cfg!(target_arch = "x86_64") {
        for t in TEMY {
            for v in ['a', 'b', 'c'] {
                let f = &foto;
                pripady.push((
                    format!("pohlad_{v} ({})", t.meno),
                    t,
                    false,
                    Box::new(move || pohlad::obrazovka(t, v, 1.0, f)),
                ));
                pripady.push((
                    format!("lyra_{v} ({})", t.meno),
                    t,
                    false,
                    Box::new(move || lyra::obrazovka(t, v, 1.0, f)),
                ));
                pripady.push((
                    format!("tv_{v} ({})", t.meno),
                    t,
                    true,
                    Box::new(move || tv::obrazovka(t, v, 1.0, f)),
                ));
            }
        }
    } else {
        let f = &foto;
        pripady.push((
            "pohlad_a (Noc)".into(),
            &NOC,
            false,
            Box::new(move || pohlad::obrazovka(&NOC, 'a', 1.0, f)),
        ));
        pripady.push((
            "lyra_c (Ľan)".into(),
            &LAN,
            false,
            Box::new(move || lyra::obrazovka(&LAN, 'c', 1.0, f)),
        ));
    }
    let mut vsetky_zle = Vec::new();
    let mut spolu = 0;
    for (meno, t, je_tv, kresli) in &pripady {
        let (n, zle) = zmeraj(meno, t, *je_tv, kresli);
        assert!(n >= 10, "{meno}: zmeraných len {n} textov a ikon");
        spolu += n;
        vsetky_zle.extend(zle);
    }
    assert!(
        vsetky_zle.is_empty(),
        "{} z {spolu} meraní nesplní kontrast:\n{}",
        vsetky_zle.len(),
        vsetky_zle.join("\n")
    );
}

#[test]
fn meranie_je_vypnute_bez_ziadosti() {
    let k = Kreslic::novy(10.0, 10.0, 1.0, 0);
    assert!(k.merania.is_none());
}

#[test]
fn meranie_vidi_pozadie_pod_textom() {
    use orchidea_vzhlad::miery::TEXT;
    merat_kontrast(true);
    let mut k = Kreslic::novy(200.0, 40.0, 1.0, 0x000000);
    merat_kontrast(false);
    // ľavá polovica čierna, pravá biela – biely text na nej nemá kontrast
    k.obdlznik(100.0, 0.0, 100.0, 40.0, 0.0, 0xFFFFFF);
    k.text("Čierne", 10.0, 25.0, TEXT, 0xFFFFFF);
    k.text("Biele", 110.0, 25.0, TEXT, 0xFFFFFF);
    let m = k.merania.unwrap();
    assert_eq!(m.len(), 2);
    assert!((m[0].najmensi - 21.0).abs() < 1e-6, "{}", m[0].najmensi);
    assert!((m[1].najmensi - 1.0).abs() < 1e-6, "{}", m[1].najmensi);
}
