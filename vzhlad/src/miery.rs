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
    let vyska_obrazu_m = palce * 0.0254 * 9.0 / (16.0f32 * 16.0 + 9.0 * 9.0).sqrt();
    let vyska_m = body / riadky * vyska_obrazu_m;
    2.0 * (vyska_m / (2.0 * metre)).atan().to_degrees() * 60.0
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
    fn stupnica_rastie() {
        for w in STUPNICA[..8].windows(2) {
            assert!(
                w[1].velkost >= w[0].velkost && (w[1].velkost > w[0].velkost || w[1].hrubka > w[0].hrubka)
            );
        }
    }
}
