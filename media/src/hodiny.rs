//! **Hodiny prehrávania a synchronizácia obrazu** (R305, etapa L6).
//!
//! **Zdroj času je zvukové zariadenie** – zvuk hrá svojím kryštálom bez prevzorkovania a obraz sa mu prispôsobí
//! opakovaním alebo vynechaním snímky.
//!
//! [`HodinyPrehravania`]: z meraní zariadenia ([`MeranieZvuku`] – odovzdané rámce `n`, ešte nezahrané `d`,
//! monotónny čas `t`) vychádza čas média `z = začiatok + (n − d) · 10⁹ / f` (presne v `i128`). Priamka `z(t)` sa
//! odhaduje **váženými najmenšími štvorcami s exponenciálnym zabúdaním (τ = 120 s)**; sklon (pomer kryštálu zvuku
//! a monotónnych hodín) je ohraničený na 1 ± 500 ppm. Dlhé okno je nutné: pri chvení časových značiek ±10 ms by
//! okno 8 s dalo šum sklonu ≈ ±450 ppm (simulácia L6), τ = 120 s dá ≈ ±8 ppm a chybu času pod 2 ms. Náhlu
//! **zmenu posunu** (8 po sebe idúcich odchýlok rovnakého znamienka, priemer nad 15 ms) zachytí posun priamky.
//! **Zverejnený čas je spojitý a monotónny:** rozdiel medzi zverejnenou priamkou a odhadom sa dorovná zmenou
//! sklonu najviac o 0,5 % (≈ 5 ms za sekundu); skokom len pri dvoch po sebe idúcich odchýlkach nad 100 ms
//! (podtečenie, chyba) s hlásením – jedno odľahlé meranie sa zahodí. Pauza čas zastaví; bez zvuku bežia monotónne
//! hodiny.
//!
//! [`Synchronizacia`]: pri každom obnovení displeja ([`Obnova`]) je cieľ `c = hodiny(t_v) + oneskorenie obrazu`;
//! zobrazí sa snímka s najväčším `pts ≤ c + P/2`, staršie sa vynechajú, inak ostáva predošlá. **Hysterézia ±P/8**
//! platí len proti udalosti opačnej k poslednej (po opakovaní treba na skrátenie či vynechanie rezervu +P/8, po
//! skrátení sa opakovanie odvráti už pri rezerve −P/8) – kadencia pri hranici nebliká a okno fázy ostáva súmerné,
//! priemerná odchýlka ≈ 0 (súmerná hysterézia pri pomalom posune fázy posúva priemer o P/8 – zistené simuláciou).
//! Odchýlka nástupu snímky voči zvuku je najviac P/2 + P/8 + chyba hodín.

use crate::r5::{MeranieZvuku, Obnova};
use std::collections::VecDeque;

/// Časová konštanta zabúdania pri odhade priamky.
pub const TAU_NS: f64 = 120e9;
/// Zmena posunu: počet odchýlok rovnakého znamienka a najmenší priemer.
pub const ZMENA_MERANI: usize = 8;
pub const ZMENA_NS: f64 = 15e6;
/// Najväčšia odchýlka sklonu od 1 (kryštály bývajú ±50–100 ppm).
pub const NAJVIAC_PPM: f64 = 500.0;
/// Najväčšia zmena sklonu pri dorovnávaní (0,5 %).
pub const DOROVNANIE: f64 = 0.005;
/// Rozdiel, nad ktorým sa čas nastaví skokom.
pub const SKOK_NS: i64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Rezim {
    /// Čaká sa na prvé meranie zvuku.
    Caka,
    /// Čas podľa zvuku.
    Zvuk,
    /// Bez zvuku: monotónne hodiny.
    Monotonne,
}

/// Hodiny prehrávania (iné meno ako `jadro::Hodiny` denníka zmien).
#[derive(Debug, Clone)]
pub struct HodinyPrehravania {
    frekvencia: u32,
    zaciatok_ns: i64,
    rezim: Rezim,
    /// Odhad priamky: kotva (t, z) prvého merania, vážený počet, priemery a kovariancie voči kotve.
    kotva: (i64, i64),
    vaha: f64,
    priemer_t: f64,
    priemer_z: f64,
    ctt: f64,
    ctz: f64,
    posledne_t: i64,
    /// Odchýlky posledných meraní od priamky (zmena posunu) a čakajúci skok.
    odchylky: VecDeque<f64>,
    cakajuci_skok: Option<f64>,
    /// Zverejnená priamka: v čase `ref_t` hodnota `ref_m`, sklon `sklon`.
    ref_t: i64,
    ref_m: i64,
    sklon: f64,
    /// Odhadnutý pomer kryštálov (sklon priamky meraní).
    pomer: f64,
    /// Pauza: zastavený čas média.
    zastavene: Option<i64>,
    skokov: u64,
}

impl HodinyPrehravania {
    /// Hodiny podľa zvukového zariadenia s frekvenciou `frekvencia`; `zaciatok_ns` = čas média prvého rámca, ktorý
    /// sa zariadeniu odovzdal (po posune nový začiatok cez [`HodinyPrehravania::posun`]).
    pub fn zo_zvuku(frekvencia: u32, zaciatok_ns: i64) -> HodinyPrehravania {
        HodinyPrehravania {
            frekvencia: frekvencia.max(1),
            zaciatok_ns,
            rezim: Rezim::Caka,
            kotva: (0, 0),
            vaha: 0.0,
            priemer_t: 0.0,
            priemer_z: 0.0,
            ctt: 0.0,
            ctz: 0.0,
            posledne_t: 0,
            odchylky: VecDeque::new(),
            cakajuci_skok: None,
            ref_t: 0,
            ref_m: zaciatok_ns,
            sklon: 1.0,
            pomer: 1.0,
            zastavene: None,
            skokov: 0,
        }
    }

    /// Hodiny bez zvuku: čas média `zaciatok_ns` v monotónnom čase `t`, ďalej rýchlosťou 1.
    pub fn monotonne(zaciatok_ns: i64, t: i64) -> HodinyPrehravania {
        let mut h = HodinyPrehravania::zo_zvuku(1, zaciatok_ns);
        h.rezim = Rezim::Monotonne;
        h.ref_t = t;
        h
    }

    /// Nový začiatok po posune v súbore (počítadlo rámcov zariadenia začína znova od 0).
    pub fn posun(&mut self, zaciatok_ns: i64, t: i64) {
        self.zaciatok_ns = zaciatok_ns;
        self.vaha = 0.0;
        self.ref_t = t;
        self.ref_m = zaciatok_ns;
        self.sklon = 1.0;
        if self.zastavene.is_some() {
            self.zastavene = Some(zaciatok_ns);
        }
        if self.rezim == Rezim::Zvuk {
            self.rezim = Rezim::Caka;
        }
    }

    /// Čas média z merania zariadenia.
    pub fn cas_merania(&self, m: &MeranieZvuku) -> i64 {
        let hrane = m.odovzdanych_ramcov.saturating_sub(m.oneskorenie_ramcov) as i128;
        let f = self.frekvencia as i128;
        (self.zaciatok_ns as i128 + (hrane * 1_000_000_000 + f / 2) / f)
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// Začne odhad priamky od merania (t, z); pomer kryštálov ostáva ako počiatočný odhad.
    fn zacni_odhad(&mut self, t: i64, z: i64) {
        self.kotva = (t, z);
        self.vaha = 1.0;
        self.priemer_t = 0.0;
        self.priemer_z = 0.0;
        self.ctt = 0.0;
        self.ctz = 0.0;
        self.posledne_t = t;
        self.odchylky.clear();
        self.cakajuci_skok = None;
    }

    /// Odhad času média v čase `t` z priamky.
    fn odhad(&self, t: i64) -> f64 {
        self.kotva.1 as f64
            + self.priemer_z
            + self.pomer * (t.saturating_sub(self.kotva.0) as f64 - self.priemer_t)
    }

    /// Spracuje meranie zvukového zariadenia; vráti veľkosť skoku (ns), ak sa čas musel nastaviť skokom.
    pub fn meranie(&mut self, m: &MeranieZvuku) -> Option<i64> {
        if self.zastavene.is_some() || self.rezim == Rezim::Monotonne {
            return None;
        }
        let t = m.monotonny_ns;
        let z = self.cas_merania(m);
        if self.rezim == Rezim::Caka || self.vaha == 0.0 {
            self.rezim = Rezim::Zvuk;
            self.zacni_odhad(t, z);
            self.ref_t = t;
            self.ref_m = z;
            self.sklon = self.pomer;
            return None;
        }
        if t <= self.posledne_t {
            return None; // meranie mimo poradia
        }
        let odchylka = z as f64 - self.odhad(t);
        if odchylka.abs() > SKOK_NS as f64 {
            // skok len pri dvoch po sebe idúcich odchýlkach s rovnakým posunom (±20 ms), inak odľahlé meranie
            match self.cakajuci_skok {
                Some(o) if (o - odchylka).abs() < 20e6 => {
                    let teraz = self.zverejneny(t);
                    self.skokov += 1;
                    self.zacni_odhad(t, z);
                    self.ref_t = t;
                    self.ref_m = z;
                    self.sklon = self.pomer;
                    return Some(z.saturating_sub(teraz));
                }
                _ => {
                    self.cakajuci_skok = Some(odchylka);
                    return None;
                }
            }
        }
        self.cakajuci_skok = None;
        // vážené najmenšie štvorce s exponenciálnym zabúdaním (numericky stabilná priebežná aktualizácia)
        let x = t.saturating_sub(self.kotva.0) as f64;
        let y = z.saturating_sub(self.kotva.1) as f64;
        let lambda = (-(t.saturating_sub(self.posledne_t) as f64) / TAU_NS).exp();
        self.posledne_t = t;
        self.vaha = lambda * self.vaha + 1.0;
        let dx = x - self.priemer_t;
        let dy = y - self.priemer_z;
        self.priemer_t += dx / self.vaha;
        self.priemer_z += dy / self.vaha;
        self.ctt = lambda * self.ctt + dx * (x - self.priemer_t);
        self.ctz = lambda * self.ctz + dx * (y - self.priemer_z);
        // sklon až z rozpätia aspoň 1 s (smerodajná odchýlka času meraní)
        let r = self.ctz / self.ctt;
        if self.vaha >= 3.0 && self.ctt / self.vaha >= 1e18 && r.is_finite() {
            self.pomer = r.clamp(1.0 - NAJVIAC_PPM * 1e-6, 1.0 + NAJVIAC_PPM * 1e-6);
        }
        // zmena posunu: posledné odchýlky rovnakého znamienka s veľkým priemerom → posun priamky
        self.odchylky.push_back(odchylka);
        if self.odchylky.len() > ZMENA_MERANI {
            self.odchylky.pop_front();
        }
        if self.odchylky.len() == ZMENA_MERANI {
            let priemer = self.odchylky.iter().sum::<f64>() / ZMENA_MERANI as f64;
            let rovnake = self.odchylky.iter().all(|o| o.signum() == priemer.signum());
            if rovnake && priemer.abs() > ZMENA_NS {
                self.priemer_z += priemer;
                self.odchylky.clear();
            }
        }
        let odhad = self.odhad(t).round() as i64;
        let teraz = self.zverejneny(t);
        // spojité dorovnanie: rozdiel sa vyrovná za ≈ 1 s, sklon sa zmení najviac o 0,5 %
        self.ref_t = t;
        self.ref_m = teraz;
        self.sklon = self.pomer + (odhad.saturating_sub(teraz) as f64 / 1e9).clamp(-DOROVNANIE, DOROVNANIE);
        None
    }

    fn zverejneny(&self, t: i64) -> i64 {
        self.ref_m.saturating_add((self.sklon * t.saturating_sub(self.ref_t) as f64).round() as i64)
    }

    /// Čas média v monotónnom čase `t` (`None`, kým nepríde prvé meranie zvuku).
    pub fn cas_ns(&self, t: i64) -> Option<i64> {
        if let Some(z) = self.zastavene {
            return Some(z);
        }
        match self.rezim {
            Rezim::Caka => None,
            Rezim::Monotonne => Some(self.ref_m.saturating_add(t.saturating_sub(self.ref_t))),
            Rezim::Zvuk => Some(self.zverejneny(t)),
        }
    }

    /// Pauza v monotónnom čase `t`: čas média stojí (zariadenie sa zastaví tiež).
    pub fn pauza(&mut self, t: i64) {
        if self.zastavene.is_none() {
            self.zastavene = Some(self.cas_ns(t).unwrap_or(self.zaciatok_ns));
        }
    }

    /// Pokračovanie po pauze: čas ide ďalej od zastavenej hodnoty; okno meraní sa začne znova (pomer ostáva).
    pub fn pokracuj(&mut self, t: i64) {
        if let Some(z) = self.zastavene.take() {
            self.ref_t = t;
            self.ref_m = z;
            self.sklon = if self.rezim == Rezim::Monotonne { 1.0 } else { self.pomer };
            // odhad začne znova pri ďalšom meraní (zariadenie stálo; pomer kryštálov ostáva)
            self.vaha = 0.0;
        }
    }

    /// Odhadnutý pomer kryštálu zvuku a monotónnych hodín (1 + ppm · 10⁻⁶).
    pub fn pomer(&self) -> f64 {
        self.pomer
    }

    /// Počet skokov času (podtečenie zvuku, chyba).
    pub fn skokov(&self) -> u64 {
        self.skokov
    }
}

/// Rozhodnutie pri obnovení displeja (indexy do frontu čakajúcich snímok).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rozhodnutie {
    /// Zobraziť snímku `fronta[i]`; `None` = ostáva predošlá.
    pub zobraz: Option<usize>,
    /// Snímky `fronta[..vynechat]` sa uvoľnia bez zobrazenia.
    pub vynechat: usize,
}

/// Štatistika synchronizácie.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatistikaSynchronizacie {
    pub obnoveni: u64,
    pub zobrazenych: u64,
    /// Snímky uvoľnené bez zobrazenia.
    pub vynechanych: u64,
    /// Snímky zobrazené kratšie ako ich trvanie − P/2 (vrátane vynechaných).
    pub skratenych: u64,
    /// Snímky zobrazené dlhšie ako ich trvanie + P/2 (opakovanie navyše).
    pub opakovanych: u64,
    /// Odchýlka nástupu snímky voči hodinám (pts − c): súčet, najväčšia absolútna.
    pub sucet_odchylok_ns: i128,
    pub najvacsia_odchylka_ns: i64,
}

impl StatistikaSynchronizacie {
    pub fn priemerna_odchylka_ns(&self) -> i64 {
        if self.zobrazenych == 0 { 0 } else { (self.sucet_odchylok_ns / self.zobrazenych as i128) as i64 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strana {
    Nevie,
    /// Posledná udalosť bola skrátenie alebo vynechanie (obraz dobiehal zvuk).
    Skoro,
    /// Posledná udalosť bolo opakovanie (obraz čakal na zvuk).
    Neskoro,
}

/// Výber snímky pri každom obnovení displeja.
#[derive(Debug, Clone)]
pub struct Synchronizacia {
    /// Oneskorenie obrazu voči zvuku na výstupe (latencia obrazu − latencia zvuku televízora a roviny), ns.
    pub oneskorenie_obrazu_ns: i64,
    strana: Strana,
    zobrazena: Option<i64>,
    zobrazeni: u32,
    statistika: StatistikaSynchronizacie,
}

impl Synchronizacia {
    pub fn nova(oneskorenie_obrazu_ns: i64) -> Synchronizacia {
        Synchronizacia {
            oneskorenie_obrazu_ns,
            strana: Strana::Nevie,
            zobrazena: None,
            zobrazeni: 0,
            statistika: StatistikaSynchronizacie::default(),
        }
    }

    /// Po posune: zabudne zobrazenú snímku a stranu hysterézie.
    pub fn prerusenie(&mut self) {
        self.strana = Strana::Nevie;
        self.zobrazena = None;
        self.zobrazeni = 0;
    }

    pub fn statistika(&self) -> &StatistikaSynchronizacie {
        &self.statistika
    }

    /// Rozhodne pri obnovení `o`: `fronta` = PTS (ns) čakajúcich snímok vzostupne (bez zobrazenej).
    pub fn obnova(&mut self, hodiny: &HodinyPrehravania, o: Obnova, fronta: &[i64]) -> Rozhodnutie {
        self.statistika.obnoveni += 1;
        let p = o.perioda_ns.max(1);
        let Some(h) = hodiny.cas_ns(o.monotonny_ns) else { return Rozhodnutie::default() };
        let c = h.saturating_add(self.oneskorenie_obrazu_ns);
        // rezerva snímky: c + P/2 − pts; zobrazí sa najneskoršia s rezervou ≥ 0
        let rezerva = |pts: i64| c.saturating_add(p / 2).saturating_sub(pts);
        let posledna_s = |prah: i64| fronta.iter().rposition(|&pts| rezerva(pts) >= prah);
        // udalosti kadencie: zobrazená snímka by ostala dlhšie ako F + 3/4 P (opakovanie), alebo by skončila
        // skôr ako F − 3/4 P či by sa vynechala snímka, ktorá mala dostať obnovenie (skrátenie)
        // v i128 – hodnoty zo súboru nesmú pretiecť
        let q = p as i128;
        let trvanie = fronta.first().zip(self.zobrazena).map(|(&a, z)| a as i128 - z as i128);
        let opakovanie = |zobrazeni: u32| trvanie.is_some_and(|f| 4 * zobrazeni as i128 * q > 4 * f + 3 * q);
        let skratenie = |i: usize| {
            trvanie.is_some_and(|f| 4 * self.zobrazeni as i128 * q < 4 * f - 3 * q)
                || (i > 0 && 4 * (fronta[i] as i128 - fronta[0] as i128) > 3 * q * i as i128)
        };
        let mut vybrana = posledna_s(0);
        // hysterézia ±P/8 len proti opačnej udalosti, než bola posledná – kadencia pri hranici nebliká a okno
        // fázy ostáva súmerné (priemerná odchýlka ≈ 0)
        match (vybrana, self.strana) {
            (Some(i), Strana::Neskoro) if skratenie(i) => {
                vybrana = posledna_s(p / 8).filter(|&j| j <= i);
            }
            (None, Strana::Skoro) if opakovanie(self.zobrazeni + 1) => {
                vybrana = posledna_s(-p / 8).filter(|&j| j == 0);
            }
            _ => {}
        }
        match vybrana {
            Some(i) if skratenie(i) => self.strana = Strana::Skoro,
            None if opakovanie(self.zobrazeni + 1) => self.strana = Strana::Neskoro,
            _ => {}
        }
        match vybrana {
            Some(i) => {
                let s = &mut self.statistika;
                // predošlá snímka končí: porovnanie počtu obnovení s jej trvaním (do ďalšej snímky)
                // udalosť = počet obnovení mimo F/P ± 3/4 (kadencia 3:2 má 2 alebo 3 pri F/P = 2,5)
                if let Some(f) = trvanie {
                    let zobrazena = 4 * self.zobrazeni as i128 * q;
                    s.opakovanych += (zobrazena > 4 * f + 3 * q) as u64;
                    s.skratenych += (zobrazena < 4 * f - 3 * q) as u64;
                }
                s.vynechanych += i as u64;
                // vynechaná snímka je skrátená, ak mala dostať aspoň jedno obnovenie (F > 3/4 P)
                if 4 * (fronta[i] as i128 - fronta[0] as i128) > 3 * q * i as i128 {
                    s.skratenych += i as u64;
                }
                // odchýlka nástupu len v ustálenom stave (prvá snímka po začiatku alebo posune ide hneď)
                if self.zobrazena.is_some() {
                    let odchylka = fronta[i].saturating_sub(c);
                    s.zobrazenych += 1;
                    s.sucet_odchylok_ns += odchylka as i128;
                    s.najvacsia_odchylka_ns = s.najvacsia_odchylka_ns.max(odchylka.saturating_abs());
                }
                self.zobrazena = Some(fronta[i]);
                self.zobrazeni = 1;
                Rozhodnutie { zobraz: Some(i), vynechat: i }
            }
            None => {
                self.zobrazeni = self.zobrazeni.saturating_add(1);
                Rozhodnutie::default()
            }
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn meranie(n: u64, d: u64, t: i64) -> MeranieZvuku {
        MeranieZvuku { odovzdanych_ramcov: n, oneskorenie_ramcov: d, monotonny_ns: t }
    }

    #[test]
    fn cas_z_merania_a_pauza() {
        let mut h = HodinyPrehravania::zo_zvuku(48_000, 1_000_000_000);
        assert_eq!(h.cas_ns(0), None);
        assert_eq!(h.cas_merania(&meranie(48_000 + 4800, 4800, 0)), 2_000_000_000);
        h.meranie(&meranie(4800, 4800, 5_000_000));
        assert_eq!(h.cas_ns(5_000_000), Some(1_000_000_000));
        assert_eq!(h.cas_ns(505_000_000), Some(1_500_000_000));
        h.pauza(505_000_000);
        assert_eq!(h.cas_ns(9_000_000_000), Some(1_500_000_000));
        h.pokracuj(9_000_000_000);
        assert_eq!(h.cas_ns(9_100_000_000), Some(1_600_000_000));
        // bez zvuku
        let h = HodinyPrehravania::monotonne(7, 100);
        assert_eq!(h.cas_ns(1100), Some(1007));
    }

    #[test]
    fn podtecenie_je_skok_s_hlasenim() {
        let mut h = HodinyPrehravania::zo_zvuku(48_000, 0);
        for k in 0..20u64 {
            assert_eq!(h.meranie(&meranie(k * 12_000 + 2000, 2000, k as i64 * 250_000_000)), None);
        }
        // jedno odľahlé meranie sa zahodí
        assert_eq!(h.meranie(&meranie(20 * 12_000 + 2000 + 48_000, 2000, 20 * 250_000_000)), None);
        assert_eq!(h.skokov(), 0);
        // zvuk 0,5 s nehral (podtečenie): dve merania po sebe zaostávajú → skok −0,5 s
        for k in 21..23u64 {
            let r = h.meranie(&meranie(k * 12_000 + 2000 - 24_000, 2000, k as i64 * 250_000_000));
            if k == 22 {
                let s = r.unwrap();
                assert!((s + 500_000_000).abs() < 1_000_000, "skok {s}");
            } else {
                assert_eq!(r, None);
            }
        }
        assert_eq!(h.skokov(), 1);
        // po skoku čas pokračuje spojito od nového stavu
        let t = h.cas_ns(23 * 250_000_000).unwrap();
        assert!((t - (23 * 250_000_000 - 500_000_000)).abs() < 1_000_000);
    }

    #[test]
    fn hysterezia_bez_blikania() {
        // snímky po P, hodiny presne na hranici a chvenie ±P/16: bez hysterézie by sa striedalo opakovanie a vynechanie
        let p = 41_666_667i64;
        let mut s = Synchronizacia::nova(0);
        let mut zobrazena = -1i64;
        let mut x = 12345u32;
        for k in 0..2000i64 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let chvenie = (x % (p as u32 / 8)) as i64 - p / 16;
            let h = HodinyPrehravania::monotonne(k * p - p / 2 + chvenie, 0);
            let fronta: Vec<i64> = (zobrazena + 1..zobrazena + 4).map(|j| j * p).collect();
            let r = s.obnova(&h, Obnova { monotonny_ns: 0, perioda_ns: p }, &fronta);
            if let Some(i) = r.zobraz {
                zobrazena += 1 + i as i64;
            }
        }
        let st = s.statistika();
        assert!(st.opakovanych + st.skratenych <= 1, "{st:?}");
    }
}
