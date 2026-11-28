//! Témy vzhľadu (Okvetie) – výmenné sady farieb. Všetko ostatné (rozostupy, písmo, tvary) je spoločné v `miery`.
//!
//! Zásady: okolie obrazu je takmer neutrálne (nefarbí vnímanie fotky), jediný výrazný akcent je orchideová fialová
//! (odtieň okolo 260°), v každej téme upravená tak, aby splnila kontrast (`kontrast::kontrola`). Svetlé témy nie sú
//! biele – žiadna plocha nie je svetlejšia ako `SVETLA_HRANICA` (požiadavka vlastníka 28. 11. 2026).

use orchidea_pismo::Farba;

/// Najsvetlejšia povolená plocha svetlej témy (vlastník: „svetlá varianta, ale nie biela“).
pub const SVETLA_HRANICA: Farba = 0xEDEAE5;

/// Farby jednej témy vzhľadu podľa úlohy (nie podľa odtieňa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tema {
    /// tvorivé meno témy (v nastaveniach za tradičným opisom v zátvorke)
    pub meno: &'static str,
    /// tradičný opis: „tmavá“, „svetlá teplá“ …
    pub druh: &'static str,
    pub tmava: bool,
    /// téma pre prístupnosť – text 7 : 1 (WCAG 2.2 AAA)
    pub vysoky_kontrast: bool,
    /// plocha okolo fotky a videa
    pub okolie: Farba,
    /// lišty a panely
    pub plocha: Farba,
    /// vyššia vrstva: bubliny, ponuky, karty nad plochou
    pub plocha_vyssia: Farba,
    /// tlačidlá, polia, karty
    pub prvok: Farba,
    pub prvok_pod_mysou: Farba,
    pub prvok_stlaceny: Farba,
    /// jemné oddeľovače (1 bod) – nie sú jedinou hranicou ovládacieho prvku
    pub okraj: Farba,
    /// hranica poľa a výrazný okraj (3 : 1 voči ploche)
    pub okraj_silny: Farba,
    pub text: Farba,
    pub text_druhy: Farba,
    pub text_vypnuty: Farba,
    /// výplň hlavnej akcie, posuvníka, vybraného prvku
    pub akcent: Farba,
    pub akcent_pod_mysou: Farba,
    pub akcent_stlaceny: Farba,
    /// text a ikona na akcente
    pub akcent_text: Farba,
    /// akcent použitý ako farba textu alebo ikony (hrajúca skladba, zapnutý nástroj)
    pub akcent_v_texte: Farba,
    /// rámik zamerania klávesnicou a diaľkovým ovládačom
    pub zameranie: Farba,
    pub uspech: Farba,
    pub hviezda: Farba,
    pub chyba: Farba,
    /// sila tieňa pod plávajúcimi prvkami (krytie jednej vrstvy 0–255)
    pub tien: u8,
}

/// Tmavá – predvolená (schválená vlastníkom 28. 11. 2026).
pub const NOC: Tema = Tema {
    meno: "Noc",
    druh: "tmavá",
    tmava: true,
    vysoky_kontrast: false,
    okolie: 0x131417,
    plocha: 0x1B1D21,
    plocha_vyssia: 0x24272C,
    prvok: 0x262930,
    prvok_pod_mysou: 0x31353D,
    prvok_stlaceny: 0x3B4049,
    okraj: 0x2C3037,
    okraj_silny: 0x767C87,
    text: 0xE9EAEE,
    text_druhy: 0x9DA3AD,
    text_vypnuty: 0x60666F,
    akcent: 0xA77BFF,
    akcent_pod_mysou: 0xB590FF,
    akcent_stlaceny: 0x9566F0,
    akcent_text: 0x15101F,
    akcent_v_texte: 0xBC9BFF,
    zameranie: 0xC9AEFF,
    uspech: 0x4CC38A,
    hviezda: 0xF2C14E,
    chyba: 0xFF7A70,
    tien: 7,
};

/// Svetlá teplá – plátno, teplá sivobéžová, nie biela.
pub const LAN: Tema = Tema {
    meno: "Ľan",
    druh: "svetlá teplá",
    tmava: false,
    vysoky_kontrast: false,
    okolie: 0xC4BFB8,
    plocha: 0xE4DFD7,
    plocha_vyssia: 0xEDEAE5,
    prvok: 0xD7D1C8,
    prvok_pod_mysou: 0xCCC5BB,
    prvok_stlaceny: 0xC0B8AD,
    okraj: 0xCBC4B9,
    okraj_silny: 0x786F64,
    text: 0x26221E,
    text_druhy: 0x504942,
    text_vypnuty: 0x9A9288,
    akcent: 0x6638C0,
    akcent_pod_mysou: 0x5A2EB0,
    akcent_stlaceny: 0x4F279C,
    akcent_text: 0xF6F2FF,
    akcent_v_texte: 0x5A2FB2,
    zameranie: 0x5A2EB0,
    uspech: 0x1C7D4E,
    hviezda: 0x8F5E06,
    chyba: 0xA3221B,
    tien: 4,
};

/// Svetlá chladná – sivá ako hmla, nie biela.
pub const HMLA: Tema = Tema {
    meno: "Hmla",
    druh: "svetlá chladná",
    tmava: false,
    vysoky_kontrast: false,
    okolie: 0xBEC2C8,
    plocha: 0xDDE0E5,
    plocha_vyssia: 0xE8EAED,
    prvok: 0xD0D4DA,
    prvok_pod_mysou: 0xC4C9D0,
    prvok_stlaceny: 0xB8BEC6,
    okraj: 0xC3C8CF,
    okraj_silny: 0x6C7480,
    text: 0x1D2127,
    text_druhy: 0x434A53,
    text_vypnuty: 0x939AA4,
    akcent: 0x6A3FD0,
    akcent_pod_mysou: 0x5D33BE,
    akcent_stlaceny: 0x512AA8,
    akcent_text: 0xF5F2FF,
    akcent_v_texte: 0x5530B8,
    zameranie: 0x5D33BE,
    uspech: 0x1A7A4C,
    hviezda: 0x94630A,
    chyba: 0xA3221B,
    tien: 4,
};

/// Polotmavá – stredná sivá na denné svetlo (obývačka s TV cez deň).
pub const SUMRAK: Tema = Tema {
    meno: "Súmrak",
    druh: "polotmavá",
    tmava: true,
    vysoky_kontrast: false,
    okolie: 0x34373D,
    plocha: 0x3E4249,
    plocha_vyssia: 0x484C54,
    prvok: 0x4A4F57,
    prvok_pod_mysou: 0x555A63,
    prvok_stlaceny: 0x60666F,
    okraj: 0x50555D,
    okraj_silny: 0x9AA0AA,
    text: 0xF3F4F6,
    text_druhy: 0xD5D9DE,
    text_vypnuty: 0x8A9099,
    akcent: 0xC3A3FF,
    akcent_pod_mysou: 0xD0B6FF,
    akcent_stlaceny: 0xB28CFF,
    akcent_text: 0x1A1226,
    akcent_v_texte: 0xDCC9FF,
    zameranie: 0xD9C4FF,
    uspech: 0x6FD9A4,
    hviezda: 0xF5CC66,
    chyba: 0xFFB0A8,
    tien: 8,
};

/// Vysoký kontrast – prístupnosť (text aspoň 7 : 1, WCAG 2.2 AAA), výrazné okraje a žltý rámik zamerania.
pub const TRN: Tema = Tema {
    meno: "Tŕň",
    druh: "vysoký kontrast",
    tmava: true,
    vysoky_kontrast: true,
    okolie: 0x000000,
    plocha: 0x000000,
    plocha_vyssia: 0x141414,
    prvok: 0x1C1C1C,
    prvok_pod_mysou: 0x2E2E2E,
    prvok_stlaceny: 0x404040,
    okraj: 0xA0A0A0,
    okraj_silny: 0xFFFFFF,
    text: 0xFFFFFF,
    text_druhy: 0xE6E6E6,
    text_vypnuty: 0x9C9C9C,
    akcent: 0xD2B6FF,
    akcent_pod_mysou: 0xE0CCFF,
    akcent_stlaceny: 0xC3A0FF,
    akcent_text: 0x000000,
    akcent_v_texte: 0xD2B6FF,
    zameranie: 0xFFE14D,
    uspech: 0x7CF0B4,
    hviezda: 0xFFD24D,
    chyba: 0xFF9C94,
    tien: 0,
};

/// Čierna – pre OLED televízory a obrazovky (čierny bod nesvieti: menej energie a tepla – chladný systém).
pub const POLNOC: Tema = Tema {
    meno: "Polnoc",
    druh: "čierna pre OLED",
    tmava: true,
    vysoky_kontrast: false,
    okolie: 0x000000,
    plocha: 0x000000,
    plocha_vyssia: 0x121212,
    prvok: 0x181818,
    prvok_pod_mysou: 0x242424,
    prvok_stlaceny: 0x303030,
    okraj: 0x1F1F1F,
    okraj_silny: 0x6C6C6C,
    text: 0xE4E4E4,
    text_druhy: 0x9C9C9C,
    text_vypnuty: 0x5A5A5A,
    akcent: 0xA77BFF,
    akcent_pod_mysou: 0xB590FF,
    akcent_stlaceny: 0x9566F0,
    akcent_text: 0x0F0A1A,
    akcent_v_texte: 0xBC9BFF,
    zameranie: 0xC9AEFF,
    uspech: 0x4CC38A,
    hviezda: 0xF2C14E,
    chyba: 0xFF7A70,
    tien: 0,
};

/// Všetky témy v poradí ponuky (prvá je predvolená).
pub const TEMY: [&Tema; 6] = [&NOC, &LAN, &HMLA, &SUMRAK, &TRN, &POLNOC];

/// Téma podľa tvorivého mena (zapamätaná voľba v nastaveniach); neznáme meno = predvolená.
pub fn podla_mena(meno: &str) -> &'static Tema {
    TEMY.iter().copied().find(|t| t.meno == meno).unwrap_or(&NOC)
}

/// Odtieň farby v stupňoch (0–360) – na skúšku, že akcent ostáva orchideovou fialovou.
pub fn odtien(f: Farba) -> f32 {
    let (r, g, b) = (((f >> 16) & 0xFF) as f32, ((f >> 8) & 0xFF) as f32, (f & 0xFF) as f32);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max == min {
        return 0.0;
    }
    let d = max - min;
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::kontrast::svietivost;

    #[test]
    fn svetle_temy_nie_su_biele() {
        let hranica = svietivost(SVETLA_HRANICA);
        for t in TEMY.iter().filter(|t| !t.tmava) {
            for (meno, f) in [
                ("okolie", t.okolie),
                ("plocha", t.plocha),
                ("plocha vyššia", t.plocha_vyssia),
                ("prvok", t.prvok),
                ("prvok pod myšou", t.prvok_pod_mysou),
            ] {
                assert!(
                    svietivost(f) <= hranica + 1e-9,
                    "téma {}: {meno} #{f:06X} je svetlejšia ako hranica",
                    t.meno
                );
            }
        }
    }

    #[test]
    fn akcent_ostava_orchideovou_fialovou() {
        for t in TEMY {
            for f in [t.akcent, t.akcent_pod_mysou, t.akcent_stlaceny, t.akcent_v_texte] {
                let h = odtien(f);
                assert!((252.0..=268.0).contains(&h), "téma {}: akcent #{f:06X} má odtieň {h:.0}°", t.meno);
            }
        }
    }

    #[test]
    fn okolie_obrazu_je_takmer_neutralne() {
        // rozdiel kanálov okolia najviac 12 z 255 – fotka sa nevníma farebne posunutá
        for t in TEMY {
            let c = [(t.okolie >> 16) & 0xFF, (t.okolie >> 8) & 0xFF, t.okolie & 0xFF];
            let rozdiel = c.iter().max().unwrap() - c.iter().min().unwrap();
            assert!(rozdiel <= 12, "téma {}: okolie #{:06X} je príliš farebné", t.meno, t.okolie);
        }
    }

    #[test]
    fn mena_tem_su_jedinecne_a_najdu_sa() {
        for (i, t) in TEMY.iter().enumerate() {
            assert_eq!(podla_mena(t.meno), *t);
            assert!(TEMY[i + 1..].iter().all(|u| u.meno != t.meno));
        }
        assert_eq!(podla_mena("neznáma"), &NOC);
    }
}
