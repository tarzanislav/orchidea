//! Veľké ikony aplikácií, zdrojov a systému pre domovskú obrazovku TV krabičky – „pekné veľké ikonky, nech to
//! vyzerá profesionálne“ (vlastník 28. 11. 2026). Jeden štýl: zaoblená dlaždica s uhlopriečnym prechodom farby,
//! biely plný symbol, kreslené naším kódom na mriežke 100 × 100. **Žiadne cudzie logá** – živá TV a mediálny server
//! majú vlastné všeobecné symboly, meno služby je len v popise. Súkromné doplnky vlastníka majú všeobecnú ikonu.

use crate::kreslic::Kreslic;
use orchidea_pismo::{Farba, zmiesaj};

/// Aplikácia, zdroj alebo systémová položka domovskej obrazovky.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aplikacia {
    /// prehrávač médií LÝRA
    Lyra,
    /// živá televízia (prehrávač IPTV)
    ZivaTv,
    /// prehliadač a editor fotografií (Pohľad)
    Fotografie,
    Subory,
    WebovyPrehliadac,
    /// domáci mediálny server (napr. Jellyfin – len ak je server)
    MedialnyServer,
    /// súkromný doplnok vlastníka – všeobecná ikona, meno sa neuvádza
    Doplnok,
    Nastavenia,
    StavKrabicky,
    Aktualizacie,
    UsbKluc,
    Karta,
    DomacaSiet,
    Stiahnute,
    Vypnut,
    /// všetky aplikácie – posledná dlaždica, keď sa aplikácie nezmestia
    VsetkyAplikacie,
}

/// Všetky položky (list ikon a skúšky).
pub const VSETKY: [Aplikacia; 16] = [
    Aplikacia::Lyra,
    Aplikacia::ZivaTv,
    Aplikacia::Fotografie,
    Aplikacia::Subory,
    Aplikacia::WebovyPrehliadac,
    Aplikacia::MedialnyServer,
    Aplikacia::Doplnok,
    Aplikacia::Nastavenia,
    Aplikacia::StavKrabicky,
    Aplikacia::Aktualizacie,
    Aplikacia::UsbKluc,
    Aplikacia::Karta,
    Aplikacia::DomacaSiet,
    Aplikacia::Stiahnute,
    Aplikacia::Vypnut,
    Aplikacia::VsetkyAplikacie,
];

impl Aplikacia {
    /// Popis pod ikonou (tradičný pojem; tvorivý názov len pri produktoch).
    pub fn meno(self) -> &'static str {
        match self {
            Aplikacia::Lyra => "LÝRA",
            Aplikacia::ZivaTv => "Živá TV",
            Aplikacia::Fotografie => "Fotografie",
            Aplikacia::Subory => "Súbory",
            Aplikacia::WebovyPrehliadac => "Web",
            Aplikacia::MedialnyServer => "Server médií",
            Aplikacia::Doplnok => "Doplnok",
            Aplikacia::Nastavenia => "Nastavenia",
            Aplikacia::StavKrabicky => "Stav krabičky",
            Aplikacia::Aktualizacie => "Aktualizácie",
            Aplikacia::UsbKluc => "USB kľúč",
            Aplikacia::Karta => "Karta",
            Aplikacia::DomacaSiet => "Domáca sieť",
            Aplikacia::Stiahnute => "Stiahnuté",
            Aplikacia::Vypnut => "Vypnúť",
            Aplikacia::VsetkyAplikacie => "Všetky",
        }
    }

    /// Farby dlaždice (ľavý horný a pravý dolný roh) – tlmené, rovnako tmavé, aby biely symbol mal ≥ 3 : 1.
    pub fn farby(self) -> (Farba, Farba) {
        match self {
            Aplikacia::Lyra => (0x5A32C2, 0x8156E1),
            Aplikacia::ZivaTv => (0x0B5A63, 0x128085),
            Aplikacia::Fotografie => (0x9C3A24, 0xB65826),
            Aplikacia::Subory => (0x2C4775, 0x46669F),
            Aplikacia::WebovyPrehliadac => (0x1C4AC9, 0x3372D9),
            Aplikacia::MedialnyServer => (0x3F35B8, 0x5C5FDB),
            Aplikacia::Doplnok => (0x355E45, 0x4C7D5D),
            Aplikacia::Nastavenia => (0x353A42, 0x555C67),
            Aplikacia::StavKrabicky => (0x165B31, 0x1E8541),
            Aplikacia::Aktualizacie => (0x5B25B8, 0x7E4FE0),
            Aplikacia::UsbKluc => (0x343B47, 0x4D5563),
            Aplikacia::Karta => (0x7A2E12, 0xA34A0B),
            Aplikacia::DomacaSiet => (0x13566B, 0x0F6E87),
            Aplikacia::Stiahnute => (0x1E3A86, 0x2759D6),
            Aplikacia::Vypnut => (0x7A1D1D, 0xA61B1B),
            Aplikacia::VsetkyAplikacie => (0x3A3550, 0x575078),
        }
    }
}

/// Body elipsy (stred, polosy, uhol v stupňoch) – na tvary symbolov.
fn elipsa(cx: f32, cy: f32, a: f32, b: f32) -> Vec<(f32, f32)> {
    (0..40)
        .map(|i| {
            let u = i as f32 / 40.0 * std::f32::consts::TAU;
            (cx + a * u.cos(), cy + b * u.sin())
        })
        .collect()
}

/// Oblúk (body) so stredom, polomerom a uhlami v stupňoch (0 = doprava, 90 = nadol).
fn obluk(cx: f32, cy: f32, r: f32, od: f32, po: f32) -> Vec<(f32, f32)> {
    let n = (((po - od).abs() / 8.0).ceil() as usize).max(4);
    (0..=n)
        .map(|i| {
            let u = (od + (po - od) * i as f32 / n as f32).to_radians();
            (cx + r * u.cos(), cy + r * u.sin())
        })
        .collect()
}

/// Kreslenie symbolu v súradniciach dlaždice 100 × 100.
struct Symbol<'a> {
    k: &'a mut Kreslic,
    x: f32,
    y: f32,
    q: f32,
    biela: Farba,
    tlmena: Farba,
}

impl Symbol<'_> {
    fn b(&self, p: (f32, f32)) -> (f32, f32) {
        (self.x + p.0 * self.q, self.y + p.1 * self.q)
    }
    fn plocha(&mut self, body: &[(f32, f32)], tlmene: bool) {
        let b: Vec<(f32, f32)> = body.iter().map(|&p| self.b(p)).collect();
        let f = if tlmene { self.tlmena } else { self.biela };
        self.k.mnohouholnik(&b, f);
    }
    fn tah(&mut self, body: &[(f32, f32)], hrubka: f32) {
        for w in body.windows(2) {
            let (a, b) = (self.b(w[0]), self.b(w[1]));
            self.k.ciara(a, b, hrubka * self.q, self.biela);
        }
    }
    fn kruh(&mut self, cx: f32, cy: f32, r: f32) {
        let c = self.b((cx, cy));
        self.k.kruh(c.0, c.1, r * self.q, self.biela);
    }
    fn obdlznik(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, f: Farba) {
        let p = self.b((x, y));
        self.k.obdlznik(p.0, p.1, w * self.q, h * self.q, r * self.q, f);
    }
    fn ram(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, hrubka: f32) {
        let p = self.b((x, y));
        let biela = self.biela;
        self.k.ram_dnu(p.0, p.1, w * self.q, h * self.q, r * self.q, hrubka * self.q, biela);
    }
}

/// Nakreslí ikonu `a` – dlaždicu veľkosti `s` bodov s ľavým horným rohom v (x, y). Hĺbka **len tieňovaním**
/// (vlastník 28. 11. 2026: „3D vzhľad … tieňovaním, jemným, ktoré ikony položí do priestoru“): jedno svetlo zhora
/// zľava – svetlejší horný ľavý roh, tenký svetlý lem hore, mäkký vrhnutý tieň pod dlaždicou a jemný tieň symbolu.
/// `zdvihnuta` (zameraná) má väčší a nižší tieň – „leží vyššie“. V programe sa ikona vykreslí raz a uloží
/// (chladný systém – tiene a prechody sa nepočítajú pri každom snímku).
pub fn kresli(k: &mut Kreslic, a: Aplikacia, x: f32, y: f32, s: f32, zdvihnuta: bool) {
    let (tmava, svetla) = a.farby();
    let r = s * 0.24;
    // mäkký vrhnutý tieň: vrstvy čiernej s malým krytím, posunuté nadol
    let (posun, rozsah, sila) =
        if zdvihnuta { (s * 0.09, s * 0.16, 6.0) } else { (s * 0.045, s * 0.09, 6.0) };
    let n = (rozsah.round() as usize).clamp(3, 24);
    for i in 0..n {
        let e = rozsah * (1.0 - i as f32 / n as f32);
        k.obdlznik_a(x - e / 2.0, y + posun - e / 2.0, s + e, s + e, r + e / 2.0, 0x000000, sila);
    }
    k.obdlznik_s_prechodom(x, y, s, s, r, svetla, tmava);
    k.lem_svetla(x, y, s, s, r, (s * 0.014).max(1.0), 0xFFFFFF, 110);
    let stred = zmiesaj(tmava, svetla, 128);
    // symbol: najprv jemný tieň (tmavší odtieň dlaždice, o kúsok nižšie), potom biely
    for (farba, dy) in [(zmiesaj(tmava, 0x000000, 110), s * 0.016), (0xFFFFFF, 0.0)] {
        let mut sy =
            Symbol { k, x, y: y + dy, q: s / 100.0, biela: farba, tlmena: zmiesaj(stred, farba, 150) };
        symbol(&mut sy, a, stred);
    }
}

/// Symbol ikony `a` (bez dlaždice).
fn symbol(sy: &mut Symbol, a: Aplikacia, stred: Farba) {
    match a {
        Aplikacia::Lyra => {
            // lýra: dve prehnuté ramená, priečka, štyri struny a podstavec
            // ramená sa dole zbiehajú k podstavcu a hore roztvárajú do rohov – tvar lýry, nie koša
            let lave = [
                (45.0, 75.0),
                (36.0, 70.0),
                (30.0, 60.0),
                (31.0, 48.0),
                (35.0, 39.0),
                (33.0, 30.0),
                (27.0, 23.0),
                (22.0, 22.0),
            ];
            let prave: Vec<(f32, f32)> = lave.iter().map(|&(px, py)| (100.0 - px, py)).collect();
            sy.tah(&lave, 6.0);
            sy.tah(&prave, 6.0);
            sy.tah(&[(34.0, 36.0), (66.0, 36.0)], 5.0);
            for sx in [44.0, 50.0, 56.0] {
                sy.tah(&[(sx, 36.0), (sx, 72.0)], 2.6);
            }
            let b = sy.biela;
            sy.obdlznik(38.0, 72.0, 24.0, 8.0, 4.0, b);
        }
        Aplikacia::ZivaTv => {
            sy.ram(20.0, 30.0, 60.0, 40.0, 7.0, 6.0);
            sy.tah(&[(40.0, 79.0), (60.0, 79.0)], 6.0);
            sy.tah(&[(50.0, 71.0), (50.0, 78.0)], 5.0);
            // vysielanie naživo: bod a dve vlny
            sy.kruh(50.0, 50.0, 5.0);
            sy.tah(&obluk(50.0, 50.0, 11.0, 210.0, 330.0), 4.0);
            sy.tah(&obluk(50.0, 50.0, 11.0, 30.0, 150.0), 4.0);
        }
        Aplikacia::Fotografie => {
            sy.ram(20.0, 24.0, 60.0, 52.0, 8.0, 6.0);
            sy.plocha(&[(27.0, 69.0), (43.0, 48.0), (53.0, 60.0), (61.0, 51.0), (73.0, 69.0)], false);
            sy.kruh(63.0, 38.0, 6.0);
        }
        Aplikacia::Subory => {
            sy.plocha(
                &[(20.0, 30.0), (40.0, 30.0), (46.0, 36.0), (80.0, 36.0), (80.0, 74.0), (20.0, 74.0)],
                true,
            );
            sy.plocha(&[(20.0, 44.0), (80.0, 44.0), (80.0, 74.0), (20.0, 74.0)], false);
        }
        Aplikacia::WebovyPrehliadac => {
            sy.tah(&obluk(50.0, 50.0, 28.0, 0.0, 360.0), 5.0);
            sy.tah(
                &elipsa(50.0, 50.0, 12.0, 28.0).into_iter().chain([(62.0, 50.0)]).collect::<Vec<_>>(),
                4.5,
            );
            sy.tah(&[(23.0, 50.0), (77.0, 50.0)], 4.5);
            sy.tah(&[(28.0, 36.0), (72.0, 36.0)], 4.0);
            sy.tah(&[(28.0, 64.0), (72.0, 64.0)], 4.0);
        }
        Aplikacia::MedialnyServer => {
            // domov (strecha a steny) a v ňom prehrať – domáci server s filmami a hudbou
            sy.tah(&[(20.0, 48.0), (50.0, 23.0), (80.0, 48.0)], 6.0);
            sy.tah(&[(28.0, 42.0), (28.0, 77.0), (72.0, 77.0), (72.0, 42.0)], 6.0);
            sy.plocha(&[(44.0, 47.0), (61.0, 58.0), (44.0, 69.0)], false);
        }
        Aplikacia::Doplnok => {
            let b = sy.biela;
            sy.obdlznik(28.0, 38.0, 36.0, 36.0, 5.0, b);
            sy.kruh(46.0, 31.0, 8.5);
            sy.kruh(71.0, 56.0, 8.5);
        }
        Aplikacia::Nastavenia => {
            let (r_paty, r_hlavy) = (20.0, 30.0);
            let mut obrys = Vec::new();
            for i in 0..8 {
                let st = i as f32 * 45.0;
                let bod = |r: f32, u: f32| {
                    let u = (u - 90.0f32).to_radians();
                    (50.0 + r * u.cos(), 50.0 + r * u.sin())
                };
                obrys.extend([bod(r_paty, st - 14.0), bod(r_hlavy, st - 8.0), bod(r_hlavy, st + 8.0)]);
                obrys.push(bod(r_paty, st + 14.0));
            }
            sy.plocha(&obrys, false);
            let c = sy.b((50.0, 50.0));
            let q = sy.q;
            sy.k.kruh(c.0, c.1, 9.0 * q, stred);
        }
        Aplikacia::StavKrabicky => {
            let b = sy.biela;
            sy.obdlznik(22.0, 58.0, 56.0, 16.0, 6.0, b);
            sy.obdlznik(28.0, 64.0, 6.0, 4.0, 2.0, stred);
            sy.tah(
                &[
                    (22.0, 42.0),
                    (36.0, 42.0),
                    (42.0, 28.0),
                    (50.0, 50.0),
                    (56.0, 36.0),
                    (62.0, 42.0),
                    (78.0, 42.0),
                ],
                5.0,
            );
        }
        Aplikacia::Aktualizacie => {
            sy.tah(&obluk(50.0, 50.0, 24.0, -60.0, 225.0), 6.0);
            sy.plocha(&[(56.0, 18.0), (72.0, 26.0), (58.0, 36.0)], false);
            sy.tah(&[(50.0, 38.0), (50.0, 60.0)], 6.0);
            sy.tah(&[(41.0, 52.0), (50.0, 61.0), (59.0, 52.0)], 6.0);
        }
        Aplikacia::UsbKluc => {
            let b = sy.biela;
            sy.obdlznik(37.0, 42.0, 26.0, 38.0, 7.0, b);
            sy.ram(41.0, 20.0, 18.0, 24.0, 3.0, 4.5);
            sy.obdlznik(45.0, 27.0, 4.0, 5.0, 1.0, b);
            sy.obdlznik(51.0, 27.0, 4.0, 5.0, 1.0, b);
        }
        Aplikacia::Karta => {
            sy.plocha(&[(33.0, 20.0), (61.0, 20.0), (69.0, 28.0), (69.0, 80.0), (33.0, 80.0)], false);
            for i in 0..4 {
                sy.obdlznik(38.0 + i as f32 * 7.0, 25.0, 4.0, 11.0, 1.0, stred);
            }
        }
        Aplikacia::DomacaSiet => {
            sy.tah(&[(50.0, 30.0), (30.0, 68.0), (70.0, 68.0), (50.0, 30.0)], 5.0);
            sy.kruh(50.0, 30.0, 10.0);
            sy.kruh(30.0, 68.0, 10.0);
            sy.kruh(70.0, 68.0, 10.0);
        }
        Aplikacia::Stiahnute => {
            sy.tah(&[(50.0, 20.0), (50.0, 58.0)], 6.0);
            sy.tah(&[(37.0, 46.0), (50.0, 59.0), (63.0, 46.0)], 6.0);
            sy.tah(&[(23.0, 62.0), (23.0, 77.0), (77.0, 77.0), (77.0, 62.0)], 6.0);
        }
        Aplikacia::Vypnut => {
            sy.tah(&obluk(50.0, 53.0, 24.0, -50.0, 230.0), 6.5);
            sy.tah(&[(50.0, 22.0), (50.0, 50.0)], 6.5);
        }
        Aplikacia::VsetkyAplikacie => {
            let b = sy.biela;
            for r in 0..3 {
                for c in 0..3 {
                    sy.obdlznik(24.0 + c as f32 * 19.0, 24.0 + r as f32 * 19.0, 14.0, 14.0, 4.0, b);
                }
            }
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::kontrast::pomer;

    #[test]
    fn biely_symbol_ma_kontrast_na_celej_dlazdici() {
        for a in VSETKY {
            let (f1, f2) = a.farby();
            for f in [f1, f2, zmiesaj(f1, f2, 128)] {
                assert!(
                    pomer(0xFFFFFF, f) >= 4.5,
                    "{a:?}: biela na #{f:06X} len {:.2} : 1",
                    pomer(0xFFFFFF, f)
                );
            }
        }
    }

    #[test]
    fn mena_su_jedinecne_a_symbol_sa_nakreslil() {
        for (i, a) in VSETKY.iter().enumerate() {
            assert!(VSETKY[i + 1..].iter().all(|b| b.meno() != a.meno()), "{a:?}");
            let mut k = Kreslic::novy(120.0, 120.0, 1.0, 0x000000);
            kresli(&mut k, *a, 10.0, 10.0, 100.0, false);
            let biele = k.p.body.iter().filter(|&&b| b == 0xFFFFFF).count();
            assert!(biele > 300, "{a:?}: symbol má len {biele} bielych bodov");
        }
    }
}
