//! **R198 – chladný pokoj**: nižšie napätie čipu RPi 2/3 a Zero 2 W **len v pokoji** (−50 mV pri 600 MHz)
//! s poistkou.
//!
//! Firmvér RPi drží v pokoji (ARM 600 MHz) napätie jadra 1,2 V – to isté, ktoré stačí aj na 1,2–1,4 GHz.
//! Pri 600 MHz má čip veľkú rezervu; `over_voltage_min=-2` zníži napätie v pokoji na 1,15 V. Výkon pri práci
//! sa nemení (firmvér pred zvýšením taktu zvýši napätie). Úniková aj spínacia strata čipu klesá s napätím
//! (zhruba s jeho druhou mocninou) – v pokoji to je niekoľko desiatok mW, teda desatiny °C.
//!
//! Na RPi 4/400/5 je `over_voltage_min` podľa dokumentácie zastarané (napätie riadi DVFS firmvéru) – tam sa nemení.
//!
//! **Poistka**: súbor `sklenik-pokoj.txt` (vkladá ho `config.txt`) nesie stav. Nové nastavenie sa skúša – ak sa
//! štyrikrát za sebou neoverí 5 min behu bez pádu, KLÍČ ho sám vypne (`stav=zlyhalo`) a znova ho neskúša, kým
//! vlastník súbor nezmaže.

/// Ako dlho musí štart s nižším napätím bežať, aby sa overil.
pub const OVERENIE_S: f64 = 300.0;
/// Koľko neoverených štartov za sebou vypne nastavenie.
pub const POKUSOV: u32 = 4;
/// Meno súboru na karte (vedľa `config.txt`).
pub const SUBOR: &str = "sklenik-pokoj.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stav {
    /// Bez nižšieho napätia (prepínač `pokoj_napatie=nie` alebo súbor chýba).
    Vypnute,
    /// Nastavenie zapísané – platí od najbližšieho štartu.
    Nove,
    /// Štart s nižším napätím ešte neoverený (n-tý pokus).
    Skuska(u32),
    Overene,
    /// Poistka: nastavenie sa neoverilo – vypnuté natrvalo.
    Zlyhalo,
}

impl Stav {
    /// Platí v tomto súbore nižšie napätie (firmvér ho pri štarte použil)?
    pub fn aktivne(self) -> bool {
        matches!(self, Stav::Nove | Stav::Skuska(_) | Stav::Overene)
    }

    pub fn popis(self) -> String {
        match self {
            Stav::Vypnute => "vypnutý".into(),
            Stav::Nove => "zapísaný – platí od ďalšieho štartu".into(),
            Stav::Skuska(n) => format!("skúša sa ({n}. štart z {POKUSOV})"),
            Stav::Overene => "overený".into(),
            Stav::Zlyhalo => {
                format!("vypnutý poistkou ({POKUSOV} štarty sa neoverili) – zmaž {SUBOR} na karte")
            }
        }
    }
}

/// Stav zo súboru (riadok `# stav=…`); chýbajúci alebo neznámy súbor = vypnuté.
pub fn stav(text: Option<&str>) -> Stav {
    let Some(riadok) = text.and_then(|t| t.lines().find_map(|r| r.trim().strip_prefix("# stav="))) else {
        return Stav::Vypnute;
    };
    let mut casti = riadok.split_whitespace();
    match (casti.next(), casti.next().and_then(|n| n.parse::<u32>().ok())) {
        (Some("nove"), _) => Stav::Nove,
        (Some("skuska"), n) => Stav::Skuska(n.unwrap_or(1).max(1)),
        (Some("overene"), _) => Stav::Overene,
        (Some("zlyhalo"), _) => Stav::Zlyhalo,
        _ => Stav::Vypnute,
    }
}

/// Obsah súboru pre stav.
pub fn obsah(s: Stav) -> String {
    let stav = match s {
        Stav::Vypnute => "vypnute".to_string(),
        Stav::Nove => "nove".into(),
        Stav::Skuska(n) => format!("skuska {n}"),
        Stav::Overene => "overene".into(),
        Stav::Zlyhalo => "zlyhalo".into(),
    };
    let mut t = format!(
        "# SKLENÍK – chladný pokoj (R198): nižšie napätie čipu len v pokoji (RPi 2, 3, Zero 2 W). Súbor spravuje\n\
         # KLÍČ podľa prepínača pokoj_napatie v SKLENIK.TXT – zmena platí od ďalšieho štartu. Ručne neupravovať;\n\
         # zmazanie súboru poistku vynuluje.\n\
         # stav={stav}\n"
    );
    if s.aktivne() {
        // −2 × 25 mV: 1,2 V → 1,15 V pri 600 MHz; pri práci napätie ako doteraz
        t += "[pi2]\nover_voltage_min=-2\n[pi3]\nover_voltage_min=-2\n[pi02]\nover_voltage_min=-2\n";
    }
    t += "[all]\n";
    t
}

/// Rozhodnutie pri štarte: stav súboru teraz a prepínač → (nový stav súboru, beží tento štart s nižším
/// napätím).
pub fn pri_starte(teraz: Stav, zapnute: bool) -> (Stav, bool) {
    let bezi = teraz.aktivne();
    let novy = match (zapnute, teraz) {
        (false, _) => Stav::Vypnute,
        (true, Stav::Vypnute) => Stav::Nove,
        (true, Stav::Nove) => Stav::Skuska(1),
        // predošlý štart s nižším napätím nevydržal 5 min (pád alebo vypnutie) – ďalší pokus alebo koniec
        (true, Stav::Skuska(n)) if n >= POKUSOV => Stav::Zlyhalo,
        (true, Stav::Skuska(n)) => Stav::Skuska(n + 1),
        (true, s) => s,
    };
    (novy, bezi)
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn poistka_po_stavoch() {
        // čistá karta: zapíše sa, platí od ďalšieho štartu
        let (s, bezi) = pri_starte(stav(None), true);
        assert_eq!((s, bezi), (Stav::Nove, false));
        let t = obsah(s);
        assert!(t.contains("[pi3]\nover_voltage_min=-2") && t.ends_with("[all]\n"));
        assert!(!t.contains("[pi4]"), "RPi 4/5 bez zmeny");
        // ďalší štart beží s nižším napätím – skúška 1
        let (s, bezi) = pri_starte(stav(Some(&t)), true);
        assert_eq!((s, bezi), (Stav::Skuska(1), true));
        // štart nevydržal 5 min – ďalšie pokusy, po štvrtom poistka
        let (s, _) = pri_starte(stav(Some(&obsah(s))), true);
        assert_eq!(s, Stav::Skuska(2));
        let (s, _) = pri_starte(stav(Some(&obsah(s))), true);
        assert_eq!(s, Stav::Skuska(3));
        let (s, _) = pri_starte(stav(Some(&obsah(s))), true);
        assert_eq!(s, Stav::Skuska(4));
        let (s, bezi) = pri_starte(stav(Some(&obsah(s))), true);
        assert_eq!((s, bezi), (Stav::Zlyhalo, true));
        assert!(!obsah(s).contains("over_voltage"));
        // poistka drží, kým sa súbor nezmaže
        assert_eq!(pri_starte(stav(Some(&obsah(Stav::Zlyhalo))), true), (Stav::Zlyhalo, false));
        // overený ostáva overený; vypnutý prepínač vypne
        assert_eq!(pri_starte(stav(Some(&obsah(Stav::Overene))), true), (Stav::Overene, true));
        assert_eq!(pri_starte(stav(Some(&obsah(Stav::Overene))), false), (Stav::Vypnute, true));
        assert_eq!(stav(Some("niečo iné\n")), Stav::Vypnute);
    }

    #[test]
    fn subor_na_karte_je_nove_nastavenie() {
        // karta prichádza so stavom „nové“ – prvý štart už beží s nižším napätím v pokoji a skúša sa
        assert_eq!(include_str!("../../boot/sklenik-pokoj.txt"), obsah(Stav::Nove));
    }
}
