//! Kontrastný pomer a relatívna svietivosť podľa WCAG 2.2 (W3C, 2023) – meria sa kódom, nie od oka.
//!
//! Zdroj: Web Content Accessibility Guidelines (WCAG) 2.2, definície „relative luminance“ a „contrast ratio“;
//! kritériá 1.4.3 (kontrast textu, úroveň AA: 4,5 : 1), 1.4.6 (úroveň AAA: 7 : 1) a 1.4.11 (kontrast
//! grafických prvkov a hraníc ovládacích prvkov: 3 : 1). Vypnuté prvky sú podľa 1.4.3 výnimkou.

use crate::tema::Tema;
use orchidea_pismo::Farba;

/// Zložka sRGB (0–255) prevedená do lineárneho svetla podľa WCAG 2.2.
fn linearne(c: u32) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Relatívna svietivosť farby 0xRRGGBB (0 = čierna, 1 = biela).
pub fn svietivost(f: Farba) -> f64 {
    0.2126 * linearne((f >> 16) & 0xFF) + 0.7152 * linearne((f >> 8) & 0xFF) + 0.0722 * linearne(f & 0xFF)
}

/// Kontrastný pomer dvoch farieb (1 až 21).
pub fn pomer(a: Farba, b: Farba) -> f64 {
    let (x, y) = (svietivost(a), svietivost(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Čo musí pár farieb splniť.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poziadavka {
    /// text: 4,5 : 1 (1.4.3, AA), v téme s vysokým kontrastom 7 : 1 (1.4.6, AAA)
    Text,
    /// grafický prvok, hranica ovládacieho prvku, rámik zamerania: 3 : 1 (1.4.11)
    Grafika,
    /// vypnutý prvok – výnimka z 1.4.3; meria sa len pre prehľad
    Vynimka,
}

/// Jeden zmeraný pár popredie / pozadie.
#[derive(Debug, Clone, Copy)]
pub struct Par {
    pub popredie: &'static str,
    pub pozadie: &'static str,
    pub farba_popredia: Farba,
    pub farba_pozadia: Farba,
    pub pomer: f64,
    pub minimum: f64,
    pub poziadavka: Poziadavka,
}

impl Par {
    pub fn splna(&self) -> bool {
        self.pomer + 1e-9 >= self.minimum
    }
}

/// Všetky páry farieb, ktoré sa v téme naozaj stretnú, s nameraným pomerom a požadovaným minimom.
pub fn kontrola(t: &Tema) -> Vec<Par> {
    let text_min = if t.vysoky_kontrast { 7.0 } else { 4.5 };
    let mut v = Vec::new();
    let mut pridaj = |p: (&'static str, Farba), z: (&'static str, Farba), poz: Poziadavka| {
        let minimum = match poz {
            Poziadavka::Text => text_min,
            Poziadavka::Grafika => 3.0,
            Poziadavka::Vynimka => 0.0,
        };
        v.push(Par {
            popredie: p.0,
            pozadie: z.0,
            farba_popredia: p.1,
            farba_pozadia: z.1,
            pomer: pomer(p.1, z.1),
            minimum,
            poziadavka: poz,
        });
    };
    let okolie = ("okolie", t.okolie);
    let plocha = ("plocha", t.plocha);
    let vyssia = ("plocha vyššia", t.plocha_vyssia);
    let prvok = ("prvok", t.prvok);
    let pod = ("prvok pod myšou", t.prvok_pod_mysou);
    let stl = ("prvok stlačený", t.prvok_stlaceny);
    use Poziadavka::*;
    for z in [okolie, plocha, vyssia, prvok, pod, stl] {
        pridaj(("text", t.text), z, Text);
    }
    for z in [okolie, plocha, vyssia, prvok, pod] {
        pridaj(("text druhý", t.text_druhy), z, Text);
    }
    for z in [
        ("akcent", t.akcent),
        ("akcent pod myšou", t.akcent_pod_mysou),
        ("akcent stlačený", t.akcent_stlaceny),
    ] {
        pridaj(("text na akcente", t.akcent_text), z, Text);
    }
    for z in [plocha, vyssia, prvok, pod] {
        pridaj(("akcent v texte", t.akcent_v_texte), z, Text);
    }
    for z in [plocha, prvok] {
        pridaj(("chyba", t.chyba), z, Text);
    }
    for z in [okolie, plocha, prvok] {
        pridaj(("rámik zamerania", t.zameranie), z, Grafika);
        pridaj(("akcent", t.akcent), z, Grafika);
    }
    for z in [plocha, prvok] {
        pridaj(("úspech", t.uspech), z, Grafika);
        pridaj(("hviezda", t.hviezda), z, Grafika);
        pridaj(("okraj poľa", t.okraj_silny), z, Grafika);
    }
    for z in [plocha, prvok] {
        pridaj(("text vypnutý", t.text_vypnuty), z, Vynimka);
    }
    v
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::tema::TEMY;

    #[test]
    fn znome_hodnoty_wcag() {
        assert!((pomer(0x000000, 0xFFFFFF) - 21.0).abs() < 1e-9);
        assert!((pomer(0xFFFFFF, 0xFFFFFF) - 1.0).abs() < 1e-9);
        // #767676 je najsvetlejšia sivá, ktorá na bielej ešte splní 4,5 : 1; #777777 už nie
        assert!(pomer(0x767676, 0xFFFFFF) >= 4.5);
        assert!(pomer(0x777777, 0xFFFFFF) < 4.5);
        assert!((svietivost(0xFF0000) - 0.2126).abs() < 1e-9);
    }

    #[test]
    fn kazda_tema_splna_kontrast() {
        for t in TEMY {
            for p in kontrola(t) {
                assert!(
                    p.splna(),
                    "téma {}: {} na {} = {:.2} : 1, treba {:.1} : 1",
                    t.meno,
                    p.popredie,
                    p.pozadie,
                    p.pomer,
                    p.minimum
                );
            }
        }
    }
}
