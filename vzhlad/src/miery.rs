//! Spoločné návrhové hodnoty všetkých tém: mriežka 4 bodov, rozostupy, zaoblenia, výšky prvkov a typografická
//! stupnica písma STONKA. Hodnoty sú v **bodoch**; na obrazovke s mierkou (HiDPI) sa násobia mierkou
//! (`Kreslic::mierka`) – 100 % = 1 bod na 1 obrazový bod, 200 % = 2 obrazové body.

/// Základ mriežky – každý rozostup a rozmer je jej násobkom.
pub const MRIEZKA: f32 = 4.0;
/// Povolené rozostupy (násobky mriežky).
pub const ROZOSTUPY: [f32; 9] = [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 32.0, 40.0, 48.0];

/// Zaoblenie malých prvkov (tlačidlá, polia).
pub const R_MALE: f32 = 6.0;
/// Zaoblenie kariet, náhľadov, panelov.
pub const R: f32 = 8.0;
/// Zaoblenie veľkých plôch (dialógy, dlaždice TV).
pub const R_VELKE: f32 = 12.0;

/// Výška hornej lišty.
pub const LISTA: f32 = 52.0;
/// Tlačidlo s ikonou (štvorec) a výška tlačidla s textom.
pub const TLACIDLO: f32 = 36.0;
/// Malé tlačidlo (v paneli).
pub const TLACIDLO_MALE: f32 = 28.0;
/// Výška poľa na zadanie hodnoty.
pub const POLE: f32 = 32.0;
/// Výška štítku (zaoblenie = polovica výšky).
pub const STITOK: f32 = 28.0;
/// Šírka pravého panelu úprav.
pub const PANEL: f32 = 320.0;
/// Stopa posuvníka a priemer jeho úchopu.
pub const STOPA: f32 = 4.0;
pub const UCHOP: f32 = 16.0;
/// Rámik zamerania: hrúbka a medzera od prvku (na TV dvojnásobok – viditeľný z gauča).
pub const ZAMERANIE_HRUBKA: f32 = 2.0;
pub const ZAMERANIE_ODSTUP: f32 = 2.0;
pub const TV_ZAMERANIE_HRUBKA: f32 = 4.0;
pub const TV_ZAMERANIE_ODSTUP: f32 = 4.0;
/// Výška tlačidla a riadku zoznamu na TV.
pub const TV_TLACIDLO: f32 = 72.0;
/// Zväčšenie zameraného prvku na TV (okolo stredu; prechod ≤ 150 ms len pri pohybe zamerania)
pub const TV_ZAMERANE_ZVACSENIE: f32 = 1.05;
/// Bezpečná plocha grafiky na TV (EBU R 95: 5 % šírky a výšky od okraja – niektoré televízory obraz orežú).
pub const TV_OKRAJ_X: f32 = 96.0;
pub const TV_OKRAJ_Y: f32 = 56.0;
/// Ikony: mriežka 20 × 20 bodov, ťah 1,7 bodu s okrúhlymi koncami (pri inej veľkosti v pomere).
pub const IKONA: f32 = 20.0;
pub const TAH_IKONY: f32 = 1.7;

/// Rez písma STONKA: výška veľkého písmena a hrúbka ťahu (v bodoch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rez {
    pub meno: &'static str,
    pub velkost: f32,
    pub hrubka: f32,
}

/// Typografická stupnica – obrazovky počítača (vzdialenosť ~60 cm).
pub const POPIS: Rez = Rez { meno: "popis", velkost: 11.0, hrubka: 1.05 };
pub const TEXT: Rez = Rez { meno: "text", velkost: 12.0, hrubka: 1.1 };
pub const TEXT_SILNY: Rez = Rez { meno: "text silný", velkost: 12.0, hrubka: 1.5 };
pub const TLACIDLO_REZ: Rez = Rez { meno: "tlačidlo", velkost: 13.0, hrubka: 1.5 };
pub const NADPIS: Rez = Rez { meno: "nadpis panelu", velkost: 13.0, hrubka: 1.65 };
pub const PODTITUL: Rez = Rez { meno: "podtitul", velkost: 15.0, hrubka: 1.3 };
pub const NADPIS_VELKY: Rez = Rez { meno: "nadpis", velkost: 18.0, hrubka: 1.9 };
pub const TITUL: Rez = Rez { meno: "titul", velkost: 28.0, hrubka: 2.4 };
/// Stupnica pre obrazovku na diaľku (TV krabička, ~3 m) – overená uhlom znaku (`uhol_znaku`).
pub const TV_POPIS: Rez = Rez { meno: "TV popis", velkost: 26.0, hrubka: 2.2 };
pub const TV_TEXT: Rez = Rez { meno: "TV text", velkost: 32.0, hrubka: 2.7 };
pub const TV_NADPIS: Rez = Rez { meno: "TV nadpis", velkost: 44.0, hrubka: 3.6 };
pub const TV_TITUL: Rez = Rez { meno: "TV titul", velkost: 64.0, hrubka: 5.0 };

/// Celá stupnica (na list dizajnového systému).
pub const STUPNICA: [Rez; 12] = [
    POPIS,
    TEXT,
    TEXT_SILNY,
    TLACIDLO_REZ,
    NADPIS,
    PODTITUL,
    NADPIS_VELKY,
    TITUL,
    TV_POPIS,
    TV_TEXT,
    TV_NADPIS,
    TV_TITUL,
];

/// Obrazovka na diaľku: vzdialenosť diváka a referenčný televízor (16 : 9, 1080 riadkov). Pri menšom televízore
/// z tej istej vzdialenosti je uhol menší – 43″: popis 14,8′, text 18,2′ (na výber vlastníkovi).
pub const TV_VZDIALENOST_M: f32 = 3.0;
pub const TV_UHLOPRIECKA_PALCE: f32 = 50.0;
pub const TV_RIADKY: f32 = 1080.0;
/// Výška veľkého písmena v oblúkových minútach: najmenej 16′, odporúčané 20′ a viac (ergonomická zásada pre
/// čítanie z obrazovky, ISO 9241-303 – hodnoty podľa verejných zhrnutí normy, text normy projekt zatiaľ nemá).
pub const UHOL_NAJMENEJ: f32 = 16.0;
pub const UHOL_ODPORUCANY: f32 = 20.0;

/// Uhol (oblúkové minúty), pod ktorým divák vidí veľké písmeno výšky `body` na televízore
/// s uhlopriečkou `palce` (16 : 9, `riadky` obrazových riadkov) zo vzdialenosti `metre`.
pub fn uhol_znaku(body: f32, riadky: f32, palce: f32, metre: f32) -> f32 {
    uhol_znaku_vyska(body, riadky, vyska_obrazu_16_9(palce), metre)
}

/// Uhol (oblúkové minúty) veľkého písmena výšky `body` na obrazovke s výškou obrazu `vyska_obrazu_m` a `riadky`
/// obrazovými riadkami zo vzdialenosti `metre`.
pub fn uhol_znaku_vyska(body: f32, riadky: f32, vyska_obrazu_m: f32, metre: f32) -> f32 {
    let vyska_m = body / riadky * vyska_obrazu_m;
    2.0 * (vyska_m / (2.0 * metre)).atan().to_degrees() * 60.0
}

/// Výška obrazu (m) televízora 16 : 9 s uhlopriečkou `palce`.
pub fn vyska_obrazu_16_9(palce: f32) -> f32 {
    palce * 0.0254 * 9.0 / (16.0f32 * 16.0 + 9.0 * 9.0).sqrt()
}

/// Voľba **„veľkosť písma“** v Nastaveniach obrazovky TV krabičky – vyjadruje vzdialenosť diváka; rýchle
/// prispôsobenie bez znalosti čísel (vlastník 28. 11. 2026: „aby sa to dalo rýchlo prispôsobiť rôznym veľkostiam“).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelkostPisma {
    Mensie,
    Bezne,
    Vacsie,
    Najvacsie,
}

impl VelkostPisma {
    pub const VSETKY: [VelkostPisma; 4] =
        [VelkostPisma::Mensie, VelkostPisma::Bezne, VelkostPisma::Vacsie, VelkostPisma::Najvacsie];

    /// Vzdialenosť diváka (m), ktorej voľba zodpovedá.
    pub fn vzdialenost_m(self) -> f32 {
        match self {
            VelkostPisma::Mensie => 2.0,
            VelkostPisma::Bezne => 3.0,
            VelkostPisma::Vacsie => 4.0,
            VelkostPisma::Najvacsie => 5.0,
        }
    }

    /// Text voľby v Nastaveniach.
    pub fn text(self) -> &'static str {
        match self {
            VelkostPisma::Mensie => "menšie",
            VelkostPisma::Bezne => "bežné",
            VelkostPisma::Vacsie => "väčšie",
            VelkostPisma::Najvacsie => "najväčšie",
        }
    }
}

/// Fyzický rozmer obrazu a vzdialenosť diváka – z nich sa počíta stupnica písma obrazovky TV krabičky.
/// Rozmer dodá správca systému (KLÍČ) z EDID; vzdialenosť voľba „veľkosť písma“.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeraniaObrazovky {
    /// šírka a výška obrazu v cm (EDID, základný blok, bajty 21 a 22); `None` = neznáme
    pub sirka_cm: Option<u16>,
    pub vyska_cm: Option<u16>,
    pub vzdialenost_m: f32,
}

impl MeraniaObrazovky {
    /// Z bajtov 21 a 22 základného bloku EDID (VESA E-EDID 1.4): oba nenulové = rozmer v cm; oba nulové = neurčený
    /// (projektor); **len jeden nulový = druhý je pomer strán, nie rozmer** → tiež neznáme.
    pub fn z_edid(bajt_21: u8, bajt_22: u8, velkost: VelkostPisma) -> MeraniaObrazovky {
        let znamy = bajt_21 != 0 && bajt_22 != 0;
        MeraniaObrazovky {
            sirka_cm: znamy.then_some(bajt_21 as u16),
            vyska_cm: znamy.then_some(bajt_22 as u16),
            vzdialenost_m: velkost.vzdialenost_m(),
        }
    }

    /// Výška obrazu v m; pri neznámom rozmere referenčný televízor 50″ 16 : 9.
    pub fn vyska_obrazu_m(&self) -> f32 {
        match self.vyska_cm {
            Some(v) if v > 0 => v as f32 / 100.0,
            _ => vyska_obrazu_16_9(TV_UHLOPRIECKA_PALCE),
        }
    }
}

/// Stupnica obrazovky TV krabičky pre konkrétny televízor a vzdialenosť: každý rez má **rovnaký uhol znaku ako pri
/// referenčnom 50″ z 3 m** (popis 17,2′, text 21,1′), veľkosti na celé 2 body. `mierka` násobí aj rozmery rozloženia
/// (dlaždice, okraje medzi prvkami) – rozloženie sa prispôsobí (menej dlaždíc v rade). Bezpečná plocha ostáva.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StupnicaTv {
    pub mierka: f32,
    pub popis: Rez,
    pub text: Rez,
    pub nadpis: Rez,
    pub titul: Rez,
}

impl StupnicaTv {
    /// Pri veľkom televízore zblízka sa nezmenšuje pod 3/4 – prázdna plocha by pôsobila stratene.
    pub const NAJMENSIA_MIERKA: f32 = 0.75;
    /// Pri malom televízore zďaleka (32″ z 5 m ≈ 2,6) ešte rozloženie vyjde; nad 3 by sa nezmestil ani názov.
    pub const NAJVACSIA_MIERKA: f32 = 3.0;

    /// Referenčná stupnica (50″, 3 m) – rezy `TV_*`.
    pub const REFERENCNA: StupnicaTv =
        StupnicaTv { mierka: 1.0, popis: TV_POPIS, text: TV_TEXT, nadpis: TV_NADPIS, titul: TV_TITUL };

    /// Stupnica pre obrazovku `m` s `riadky` obrazovými riadkami (návrh sa kreslí v 1080 bodoch; 4K = mierka 2
    /// kresliča, nie stupnice).
    pub fn pre(m: &MeraniaObrazovky) -> StupnicaTv {
        let referencia = vyska_obrazu_16_9(TV_UHLOPRIECKA_PALCE);
        let s = (m.vzdialenost_m / TV_VZDIALENOST_M * referencia / m.vyska_obrazu_m())
            .clamp(Self::NAJMENSIA_MIERKA, Self::NAJVACSIA_MIERKA);
        let rez =
            |r: Rez| Rez { meno: r.meno, velkost: (r.velkost * s / 2.0).round() * 2.0, hrubka: r.hrubka * s };
        StupnicaTv {
            mierka: s,
            popis: rez(TV_POPIS),
            text: rez(TV_TEXT),
            nadpis: rez(TV_NADPIS),
            titul: rez(TV_TITUL),
        }
    }

    /// Rozmer rozloženia (body pri referencii) v tejto stupnici, zaokrúhlený na mriežku 4 bodov.
    pub fn rozmer(&self, body: f32) -> f32 {
        (body * self.mierka / MRIEZKA).round() * MRIEZKA
    }
}

/// Je hodnota na mriežke 4 bodov?
pub fn na_mriezke(x: f32) -> bool {
    (x / MRIEZKA - (x / MRIEZKA).round()).abs() < 1e-4
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn rozmery_su_na_mriezke() {
        for x in ROZOSTUPY.iter().chain(&[
            R,
            R_VELKE,
            LISTA,
            TLACIDLO,
            TLACIDLO_MALE,
            POLE,
            STITOK,
            PANEL,
            STOPA,
            UCHOP,
            IKONA,
        ]) {
            assert!(na_mriezke(*x), "{x} nie je na mriežke 4 bodov");
        }
    }

    #[test]
    fn tv_pismo_je_citatelne_z_gauca() {
        let uhol = |r: Rez| uhol_znaku(r.velkost, TV_RIADKY, TV_UHLOPRIECKA_PALCE, TV_VZDIALENOST_M);
        assert!(uhol(TV_POPIS) >= UHOL_NAJMENEJ, "TV popis {:.1}′", uhol(TV_POPIS));
        for r in [TV_TEXT, TV_NADPIS, TV_TITUL] {
            assert!(uhol(r) >= UHOL_ODPORUCANY, "{} {:.1}′", r.meno, uhol(r));
        }
    }

    #[test]
    fn uhol_znaku_sedi_s_vypoctom_na_papieri() {
        // 43″ 16 : 9 → výška obrazu 0,5355 m; 36 bodov z 1080 = 17,85 mm; z 3 m ≈ 20,5′
        let u = uhol_znaku(36.0, 1080.0, 43.0, 3.0);
        assert!((u - 20.45).abs() < 0.1, "{u}");
        // 50″ → 0,6227 m; 32 bodov = 18,45 mm → 21,1′
        let u = uhol_znaku(32.0, 1080.0, 50.0, 3.0);
        assert!((u - 21.14).abs() < 0.1, "{u}");
    }

    #[test]
    fn tv_obsah_je_v_bezpecnej_ploche() {
        const { assert!(TV_OKRAJ_X >= 0.05 * 1920.0 && TV_OKRAJ_Y >= 0.05 * 1080.0) };
        assert!(na_mriezke(TV_OKRAJ_X) && na_mriezke(TV_OKRAJ_Y) && na_mriezke(TV_TLACIDLO));
    }

    #[test]
    fn stupnica_tv_je_citatelna_na_kazdom_televizore() {
        // 32″–85″ a vzdialenosť 1,5–5 m: rozmer v cm tak, ako ho zapíše EDID (celé cm)
        for palce in [32.0f32, 40.0, 43.0, 50.0, 55.0, 65.0, 75.0, 85.0] {
            for d in [1.5f32, 2.0, 3.0, 4.0, 5.0] {
                let v = vyska_obrazu_16_9(palce);
                let m = MeraniaObrazovky {
                    sirka_cm: Some((v * 16.0 / 9.0 * 100.0).round() as u16),
                    vyska_cm: Some((v * 100.0).round() as u16),
                    vzdialenost_m: d,
                };
                let st = StupnicaTv::pre(&m);
                let u = |r: Rez| uhol_znaku_vyska(r.velkost, TV_RIADKY, m.vyska_obrazu_m(), d);
                assert!(u(st.popis) >= UHOL_NAJMENEJ, "{palce}″ {d} m: popis {:.1}′", u(st.popis));
                for r in [st.text, st.nadpis, st.titul] {
                    assert!(u(r) >= UHOL_ODPORUCANY, "{palce}″ {d} m: {} {:.1}′", r.meno, u(r));
                }
                assert!(st.popis.velkost < st.text.velkost && st.text.velkost < st.nadpis.velkost);
                assert!(na_mriezke(st.rozmer(TV_TLACIDLO)));
            }
        }
    }

    #[test]
    fn stupnica_tv_referencia_a_edid() {
        // 50″ z EDID (111 × 62 cm) a z 3 m = dnešné rezy
        let m = MeraniaObrazovky::z_edid(111, 62, VelkostPisma::Bezne);
        let st = StupnicaTv::pre(&m);
        assert_eq!(
            (st.popis.velkost, st.text.velkost, st.nadpis.velkost, st.titul.velkost),
            (26.0, 32.0, 44.0, 64.0)
        );
        // EDID 1.4: jeden nulový bajt = pomer strán, nie rozmer → neznáme → 50″
        let pomer = MeraniaObrazovky::z_edid(0, 79, VelkostPisma::Bezne);
        assert_eq!((pomer.sirka_cm, pomer.vyska_cm), (None, None));
        assert_eq!(
            StupnicaTv::pre(&pomer),
            StupnicaTv::pre(&MeraniaObrazovky::z_edid(0, 0, VelkostPisma::Bezne))
        );
        // väčšie písmo = väčšia stupnica
        let v = StupnicaTv::pre(&MeraniaObrazovky::z_edid(111, 62, VelkostPisma::Najvacsie));
        assert!(v.text.velkost > st.text.velkost && v.mierka > 1.5);
    }

    #[test]
    fn stupnica_rastie() {
        for w in STUPNICA[..8].windows(2) {
            assert!(
                w[1].velkost >= w[0].velkost && (w[1].velkost > w[0].velkost || w[1].hrubka > w[0].hrubka)
            );
        }
    }
}
