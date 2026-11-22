//! Kontrola karty alebo USB disku: falošná kapacita, chybné miesta a skutočná rýchlosť.
//!
//! Falošné karty hlásia väčšiu kapacitu, než majú – zápis za skutočnou hranicou sa „zabalí“ na začiatok
//! alebo stratí. Rýchla kontrola (podobne ako f3probe) zapíše jedinečné bloky na stovky miest celej
//! kapacity – rovnomerne aj tesne okolo mocnín dvoch, kde falošné radiče kapacitu zvyčajne lámu – a až
//! potom ich všetky prečíta. Úplná kontrola (ako H2testw, ale priamo na disk) zapíše a overí celý povrch.
//! Po kontrole sa médium naformátuje; falošné len na skutočnú kapacitu, takže už nestratí dáta.

use crate::Faza;
use crate::ciel::{Ciel, KUS};
use orchidea_jadro::nahoda::Nahoda;
use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const MIB: u64 = 1 << 20;
const ZNACKA: &[u8; 16] = b"SADIC-KONTROLA\0\0";

/// Výsledok kontroly.
#[derive(Debug, Clone, PartialEq)]
pub struct Vysledok {
    /// Kapacita, ktorú médium hlási.
    pub hlasena: u64,
    /// Kapacita, do ktorej všetko sedí (pri falošnom médiu menšia).
    pub skutocna: u64,
    pub skusanych: u64,
    pub chybnych: u64,
    /// Blok sa prečítal z iného miesta (typické pre falošnú kapacitu).
    pub zabalenych: u64,
    pub zapis_mb_s: f64,
    pub citanie_mb_s: f64,
    pub uplna: bool,
}

impl Vysledok {
    pub fn falosna(&self) -> bool {
        self.skutocna + 16 * MIB <= self.hlasena
    }

    pub fn v_poriadku(&self) -> bool {
        self.chybnych == 0
    }

    /// Trieda rýchlosti SD podľa nameraného zápisu (C = Class, U = UHS).
    pub fn trieda(&self) -> &'static str {
        match self.zapis_mb_s {
            r if r >= 30.0 => "zodpovedá triede U3 / V30 (≥ 30 MB/s)",
            r if r >= 10.0 => "zodpovedá triede C10 / U1 (≥ 10 MB/s)",
            r if r >= 6.0 => "zodpovedá triede C6",
            r if r >= 4.0 => "zodpovedá triede C4",
            _ => "pomalšie ako trieda C4 – na systém nevhodné",
        }
    }

    pub fn text(&self) -> String {
        let gb = |b: u64| format!("{:.1} GB", b as f64 / 1e9);
        let mut t = if self.falosna() {
            format!(
                "⚠ FALOŠNÁ KAPACITA: hlási {}, ale spoľahlivo uloží len {}.",
                gb(self.hlasena),
                gb(self.skutocna)
            )
        } else if !self.v_poriadku() {
            format!("⚠ Médium má chybné miesta ({} z {} skúšaných blokov).", self.chybnych, self.skusanych)
        } else {
            format!(
                "✓ Kapacita {} je skutočná, bez chýb ({} skúšaných miest).",
                gb(self.hlasena),
                self.skusanych
            )
        };
        t += &format!(
            "\nZápis {:.1} MB/s, čítanie {:.1} MB/s – {}.",
            self.zapis_mb_s,
            self.citanie_mb_s,
            self.trieda()
        );
        if self.zabalenych > 0 {
            t += &format!(
                "\n{} blokov sa prečítalo z iného miesta – radič klame o veľkosti.",
                self.zabalenych
            );
        }
        t
    }
}

/// Blok vzoru pre miesto `posun`: značka, posun, semienko a pseudonáhodné bajty.
fn vzor(semienko: u64, posun: u64, ciel: &mut [u8]) {
    let mut n = Nahoda::zo_semienka(semienko ^ posun.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    for c in ciel.chunks_mut(8) {
        let v = n.dalsie().to_le_bytes();
        c.copy_from_slice(&v[..c.len()]);
    }
    ciel[..16].copy_from_slice(ZNACKA);
    ciel[16..24].copy_from_slice(&posun.to_le_bytes());
    ciel[24..32].copy_from_slice(&semienko.to_le_bytes());
}

/// Miesta rýchlej kontroly: rovnomerne po celej kapacite, okolo mocnín dvoch a posledný blok.
pub fn miesta(velkost: u64, blok: u64, rovnomerne: u64) -> Vec<u64> {
    let mut v = Vec::new();
    let posledny = (velkost / blok).saturating_sub(1) * blok;
    for i in 0..rovnomerne {
        v.push((velkost / rovnomerne * i) / blok * blok);
    }
    let mut m = 16 * MIB;
    while m < velkost {
        for p in [m.saturating_sub(blok), m, m + blok] {
            if p <= posledny {
                v.push(p);
            }
        }
        m *= 2;
    }
    v.push(posledny);
    v.sort_unstable();
    v.dedup();
    v
}

/// Zber pozorovaní pri čítaní (adresy idú vzostupne).
#[derive(Default)]
struct Zber {
    skusanych: u64,
    chybnych: u64,
    zabalenych: u64,
    /// Zjednotenie adries, ktoré zdieľajú jednu bunku pamäte (prečítaný blok patril inej adrese).
    rodic: BTreeMap<u64, u64>,
    /// Prvý blok so smetím (ani náš vzor) a koľko blokov odvtedy bolo skúšaných / so smetím.
    prvy_odpad: Option<u64>,
    za_odpadom: u64,
    odpadu: u64,
}

impl Zber {
    fn koren(&mut self, x: u64) -> u64 {
        let mut r = x;
        while let Some(&p) = self.rodic.get(&r) {
            if p == r {
                break;
            }
            r = p;
        }
        let mut y = x;
        while let Some(&p) = self.rodic.get(&y) {
            if p == r {
                break;
            }
            self.rodic.insert(y, r);
            y = p;
        }
        r
    }

    fn spoj(&mut self, a: u64, b: u64) {
        self.rodic.entry(a).or_insert(a);
        self.rodic.entry(b).or_insert(b);
        let (ra, rb) = (self.koren(a), self.koren(b));
        if ra != rb {
            self.rodic.insert(ra.max(rb), ra.min(rb));
        }
    }

    fn posud(&mut self, m: u64, b: &[u8], ocakavany: &[u8]) {
        self.skusanych += 1;
        if self.prvy_odpad.is_some() {
            self.za_odpadom += 1;
        }
        if b == ocakavany {
            return;
        }
        self.chybnych += 1;
        let q = u64::from_le_bytes(b[16..24].try_into().unwrap_or([0; 8]));
        if &b[..16] == ZNACKA && q != m && b[24..32] == ocakavany[24..32] {
            self.zabalenych += 1;
            if self.rodic.len() < 2_000_000 {
                self.spoj(m, q);
            }
        } else {
            if self.prvy_odpad.is_none() {
                self.prvy_odpad = Some(m);
                self.za_odpadom = 1;
            }
            self.odpadu += 1;
        }
    }

    /// Najnižšia adresa, ktorá nemá vlastnú bunku: v každej skupine zdieľajúcej bunku je skutočná len
    /// najnižšia adresa. Smetie od istej adresy po koniec (≥ 90 % blokov) = stratená kapacita.
    fn prva_falosna(&mut self) -> Option<u64> {
        let kluce: Vec<u64> = self.rodic.keys().copied().collect();
        let mut skupiny: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for k in kluce {
            let r = self.koren(k);
            skupiny.entry(r).or_default().push(k);
        }
        let zo_zabalenia = skupiny
            .values_mut()
            .filter(|s| s.len() > 1)
            .map(|s| {
                s.sort_unstable();
                s[1]
            })
            .min();
        let zo_straty =
            self.prvy_odpad.filter(|_| self.odpadu >= 3 && self.odpadu * 10 >= self.za_odpadom * 9);
        match (zo_zabalenia, zo_straty) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Skontroluje médium. Prepíše jeho obsah!
pub fn skontroluj(
    c: &mut dyn Ciel,
    uplna: bool,
    priebeh: &mut dyn FnMut(Faza, u64, u64),
    zrus: &AtomicBool,
) -> io::Result<Vysledok> {
    let hlasena = c.velkost();
    let blok = (c.sektor() as u64).max(4096);
    let semienko = u64::from_le_bytes(crate::oddiely::jedinecne()[..8].try_into().unwrap_or([7; 8]));
    let zrusene = || io::Error::other("zrušené");
    let mut kus = vec![0u8; KUS];
    let mut precitane = vec![0u8; KUS];
    // 1. rýchlosť: súvislý zápis na začiatok (súčasť úplnej kontroly, inak 1 % disku, 32–256 MiB)
    let suvisle = if uplna {
        hlasena / KUS as u64 * KUS as u64
    } else {
        (hlasena / 100).clamp(32 * MIB, 256 * MIB).min(hlasena / 2) / MIB * MIB
    };
    let t = Instant::now();
    let mut p = 0;
    while p < suvisle {
        if zrus.load(Ordering::Relaxed) {
            return Err(zrusene());
        }
        let n = (KUS as u64).min(suvisle - p) as usize;
        for (i, b) in kus[..n].chunks_mut(blok as usize).enumerate() {
            vzor(semienko, p + i as u64 * blok, b);
        }
        c.zapis(p, &kus[..n])?;
        p += n as u64;
        priebeh(Faza::SkuskaZapis, p, suvisle);
    }
    // 2. rozptýlené bloky po celej kapacite (aj za skúšaným začiatkom)
    let body: Vec<u64> = if uplna {
        Vec::new()
    } else {
        miesta(hlasena, blok, 256).into_iter().filter(|&m| m >= suvisle).collect()
    };
    for (i, &m) in body.iter().enumerate() {
        if zrus.load(Ordering::Relaxed) {
            return Err(zrusene());
        }
        vzor(semienko, m, &mut kus[..blok as usize]);
        c.zapis(m, &kus[..blok as usize])?;
        priebeh(Faza::SkuskaZapis, i as u64 + 1, body.len() as u64);
    }
    c.dokonci()?;
    let zapis_mb_s = suvisle as f64 / 1e6 / t.elapsed().as_secs_f64().max(0.001);
    // 3. čítanie a porovnanie
    let mut v = Vysledok {
        hlasena,
        skutocna: hlasena,
        skusanych: 0,
        chybnych: 0,
        zabalenych: 0,
        zapis_mb_s,
        citanie_mb_s: 0.0,
        uplna,
    };
    let mut z = Zber::default();
    let t = Instant::now();
    let mut p = 0;
    let mut ocakavany = vec![0u8; blok as usize];
    while p < suvisle {
        if zrus.load(Ordering::Relaxed) {
            return Err(zrusene());
        }
        let n = (KUS as u64).min(suvisle - p) as usize;
        c.citaj(p, &mut precitane[..n])?;
        for (i, b) in precitane[..n].chunks(blok as usize).enumerate() {
            let m = p + i as u64 * blok;
            vzor(semienko, m, &mut ocakavany[..b.len()]);
            z.posud(m, b, &ocakavany[..b.len()]);
        }
        p += n as u64;
        priebeh(Faza::SkuskaCitanie, p, suvisle);
    }
    v.citanie_mb_s = suvisle as f64 / 1e6 / t.elapsed().as_secs_f64().max(0.001);
    for (i, &m) in body.iter().enumerate() {
        if zrus.load(Ordering::Relaxed) {
            return Err(zrusene());
        }
        c.citaj(m, &mut precitane[..blok as usize])?;
        vzor(semienko, m, &mut ocakavany);
        z.posud(m, &precitane[..blok as usize], &ocakavany);
        priebeh(Faza::SkuskaCitanie, i as u64 + 1, body.len() as u64);
    }
    v.skusanych = z.skusanych;
    v.chybnych = z.chybnych;
    v.zabalenych = z.zabalenych;
    if let Some(m) = z.prva_falosna() {
        // skutočná kapacita: po prvú falošnú adresu, zaokrúhlené nadol na 16 MiB
        v.skutocna = m / (16 * MIB) * (16 * MIB);
    }
    Ok(v)
}

/// Cieľ obmedzený na prvých `velkost` bajtov (na naformátovanie falošnej karty na skutočnú kapacitu).
pub struct Obmedzeny<'a> {
    pub c: &'a mut dyn Ciel,
    pub velkost: u64,
}

impl Ciel for Obmedzeny<'_> {
    fn sektor(&self) -> u32 {
        self.c.sektor()
    }
    fn velkost(&self) -> u64 {
        self.velkost.min(self.c.velkost())
    }
    fn zapis(&mut self, posun: u64, data: &[u8]) -> io::Result<()> {
        if posun + data.len() as u64 > self.velkost() {
            return Err(io::Error::other("za skutočnou kapacitou"));
        }
        self.c.zapis(posun, data)
    }
    fn citaj(&mut self, posun: u64, ciel: &mut [u8]) -> io::Result<()> {
        self.c.citaj(posun, ciel)
    }
    fn dokonci(&mut self) -> io::Result<()> {
        self.c.dokonci()
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::ciel::Pamat;

    /// Falošná karta: hlási `hlasena`, ale adresy sa zabalia po `skutocna` bajtoch.
    struct Falosna {
        p: Pamat,
        hlasena: u64,
    }

    impl Ciel for Falosna {
        fn sektor(&self) -> u32 {
            512
        }
        fn velkost(&self) -> u64 {
            self.hlasena
        }
        fn zapis(&mut self, posun: u64, data: &[u8]) -> io::Result<()> {
            let n = self.p.data.len() as u64;
            for (i, k) in data.chunks(512).enumerate() {
                self.p.zapis((posun + i as u64 * 512) % n, k)?;
            }
            Ok(())
        }
        fn citaj(&mut self, posun: u64, ciel: &mut [u8]) -> io::Result<()> {
            let n = self.p.data.len() as u64;
            for (i, k) in ciel.chunks_mut(512).enumerate() {
                self.p.citaj((posun + i as u64 * 512) % n, k)?;
            }
            Ok(())
        }
        fn dokonci(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn miesta_pokryju_celu_kapacitu_a_hranice() {
        let m = miesta(64 << 30, 4096, 256);
        assert!(m.len() > 256);
        assert_eq!(*m.last().unwrap(), (64 << 30) - 4096);
        assert!(m.contains(&(32 << 30)) && m.contains(&((32 << 30) - 4096)));
        assert!(m.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn spravna_karta_prejde() {
        let mut p = Pamat::nova(256 * MIB, 512);
        let v = skontroluj(&mut p, false, &mut |_, _, _| {}, &AtomicBool::new(false)).unwrap();
        assert!(v.v_poriadku() && !v.falosna(), "{}", v.text());
        assert_eq!(v.skutocna, 256 * MIB);
        assert!(v.text().contains("je skutočná"));
    }

    #[test]
    fn falosna_karta_sa_odhali() {
        // hlási 1 GiB, má 256 MiB
        let mut f = Falosna { p: Pamat::nova(256 * MIB, 512), hlasena: 1 << 30 };
        let v = skontroluj(&mut f, false, &mut |_, _, _| {}, &AtomicBool::new(false)).unwrap();
        assert!(v.falosna(), "{}", v.text());
        assert!(v.zabalenych > 0);
        assert!(v.skutocna <= 256 * MIB && v.skutocna >= 128 * MIB, "skutočná {}", v.skutocna);
        assert!(v.text().contains("FALOŠNÁ KAPACITA"));
        // úplná kontrola nájde to isté
        let v = skontroluj(&mut f, true, &mut |_, _, _| {}, &AtomicBool::new(false)).unwrap();
        assert!(v.falosna());
        assert!(v.skutocna <= 256 * MIB);
    }
}
