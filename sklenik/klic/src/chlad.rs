//! SPRÁVCA CHLADU pre TV krabičky (R132–R135) – čistá logika bez hardvéru, skúša sa na každom systéme.
//!
//! Meranie X96 Max Plus ukázalo, že v pokoji hreje obraz a periférie, nie procesor. Správca rozhoduje:
//! - **obraz** (R132): chladný režim (najnižšia obnova Full HD, 16 bitov, polovičné rozlíšenie so zväčšením
//!   v obvode obrazu) a **zhasínanie v pokoji** – najprv čierny obraz (nič sa nečíta z pamäte, signál ostáva),
//!   neskôr vypnutý (stojí aj vysielač HDMI); prebudí ho diaľkový ovládač, klávesnica alebo myš,
//! - **USB úložisko spí** medzi zápismi (R133) – kľúč sa zobudí len na zápis merania,
//! - **sieť podľa potreby** (R133): 100 Mb/s, gigabit len pri veľkom prenose – podľa výskumu
//!   *Adaptive Link Rate* (Gunaratne, Christensen, Nordman, Suen; IEEE Trans. Computers 2008): dva prahy
//!   s hysteréziou, aby spojenie nekmitalo,
//! - **tepelný pokus** (R134): po fázach 20 minút porovná pôvodný stav, úsporné periférie, chladný, čierny
//!   a vypnutý obraz a na konci znova pôvodný stav (návrh A–B–…–A: posun teploty miestnosti sa odčíta).
//!   Ustálenú teplotu každej fázy odhadne z krivky modelom ohrevu T(t) = T∞ + (T₀ − T∞)·e^(−t/τ)
//!   (najmenšie štvorce, τ hľadané po mriežke) – netreba čakať hodiny, kým sa teplota ustáli.

use std::fs;
use std::path::{Path, PathBuf};

/// Čo sa od obrazu chce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ciel {
    /// Pôvodné správanie SKLENÍKA: Full HD @ 60 Hz (na porovnanie v tepelnom pokuse).
    Povodny,
    /// Najnižšia bežná obnova Full HD, ktorú televízor ponúka (24/25/30 Hz), inak 720p.
    Chladny,
}

/// Stav obrazu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StavObrazu {
    Zapnuty,
    /// Rovina vypnutá – z pamäte sa nečíta nič, signál HDMI beží (prebudenie okamžité).
    Cierny,
    /// CRTC vypnuté – stojí kódovač obrazu aj vysielač HDMI (televízor ukáže „bez signálu“).
    Vypnuty,
}

/// Zhasínanie obrazu v pokoji (bez stlačenia na vstupe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zhasinanie {
    /// Po koľkých sekundách pokoja sa obraz začierni (`None` = nikdy).
    pub cierny_po_s: Option<u64>,
    /// Po koľkých sekundách pokoja sa obraz vypne (`None` = nikdy).
    pub vypnuty_po_s: Option<u64>,
}

impl Zhasinanie {
    /// Predvolené pre TV krabičky: čierny po 3 min, vypnutý po 10 min. Nastavenie `zhasni=` a `vypni=`
    /// (minúty, 0 = nikdy) v SKLENIK.TXT.
    pub fn pre_tv(zhasni_min: Option<u32>, vypni_min: Option<u32>) -> Zhasinanie {
        let s = |m: Option<u32>, predvolene: u64| match m {
            Some(0) => None,
            Some(m) => Some(m as u64 * 60),
            None => Some(predvolene),
        };
        Zhasinanie { cierny_po_s: s(zhasni_min, 180), vypnuty_po_s: s(vypni_min, 600) }
    }

    pub fn stav(&self, necinnost_s: u64) -> StavObrazu {
        if self.vypnuty_po_s.is_some_and(|v| necinnost_s >= v) {
            StavObrazu::Vypnuty
        } else if self.cierny_po_s.is_some_and(|c| necinnost_s >= c) {
            StavObrazu::Cierny
        } else {
            StavObrazu::Zapnuty
        }
    }
}

/// Sieť podľa potreby (R133, Adaptive Link Rate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SietPodlaPotreby {
    pub gigabit: bool,
    nad_s: f64,
    pod_s: f64,
}

/// Prenos (Mb/s), pri ktorom 100 Mb/s začína brzdiť – prejde sa na gigabit (po 3 s).
pub const ALR_HORE_MBS: f64 = 60.0;
/// Prenos, pod ktorým gigabit netreba (po 2 min) – späť na 100 Mb/s.
pub const ALR_DOLE_MBS: f64 = 15.0;

impl SietPodlaPotreby {
    pub fn nova() -> SietPodlaPotreby {
        SietPodlaPotreby { gigabit: false, nad_s: 0.0, pod_s: 0.0 }
    }

    /// Krok: čas od posledného kroku (s) a prenos (Mb/s – väčší z príjmu a odosielania). Vráti nový stav,
    /// keď sa má ponuka zmeniť.
    pub fn krok(&mut self, dt_s: f64, mbit_s: f64) -> Option<bool> {
        if self.gigabit {
            self.pod_s = if mbit_s < ALR_DOLE_MBS { self.pod_s + dt_s } else { 0.0 };
            if self.pod_s >= 120.0 {
                *self = SietPodlaPotreby::nova();
                return Some(false);
            }
        } else {
            self.nad_s = if mbit_s >= ALR_HORE_MBS { self.nad_s + dt_s } else { 0.0 };
            if self.nad_s >= 3.0 {
                *self = SietPodlaPotreby { gigabit: true, nad_s: 0.0, pod_s: 0.0 };
                return Some(true);
            }
        }
        None
    }
}

/// Fáza tepelného pokusu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Faza {
    pub nazov: &'static str,
    pub obraz: Ciel,
    pub stav: StavObrazu,
    /// Úsporné periférie: USB kľúč spí, jadrá v pokoji zaparkované, sieť podľa potreby.
    pub periferie: bool,
}

/// Dĺžka fázy (s) – asi 2,5 časovej konštanty krabičky (X96: τ ≈ 8 min); zvyšok dopočíta model.
pub const FAZA_S: u64 = 20 * 60;

pub const FAZY: [Faza; 6] = [
    Faza {
        nazov: "pôvodne (ako doteraz)",
        obraz: Ciel::Povodny,
        stav: StavObrazu::Zapnuty,
        periferie: false,
    },
    Faza { nazov: "úsporné periférie", obraz: Ciel::Povodny, stav: StavObrazu::Zapnuty, periferie: true },
    Faza { nazov: "chladný obraz", obraz: Ciel::Chladny, stav: StavObrazu::Zapnuty, periferie: true },
    Faza { nazov: "čierny obraz", obraz: Ciel::Chladny, stav: StavObrazu::Cierny, periferie: true },
    Faza { nazov: "obraz vypnutý", obraz: Ciel::Chladny, stav: StavObrazu::Vypnuty, periferie: true },
    Faza {
        nazov: "pôvodne – kontrola", obraz: Ciel::Povodny, stav: StavObrazu::Zapnuty, periferie: false
    },
];

/// Fáza v danom čase od začiatku pokusu; `None` po skončení.
pub fn faza(s: u64) -> Option<usize> {
    let i = (s / FAZA_S) as usize;
    (i < FAZY.len()).then_some(i)
}

/// Minúty záznamov samomerania pri pokuse: začiatok a koniec každej fázy.
pub fn milniky_pokusu() -> Vec<u32> {
    (0..=FAZY.len() as u32).map(|i| i * (FAZA_S / 60) as u32).collect()
}

/// Čo má byť teraz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rozhodnutie {
    pub obraz: Ciel,
    pub stav: StavObrazu,
    pub periferie: bool,
    pub faza: Option<usize>,
    /// Človek práve používa ovládač, hoci fáza chce obraz zhasnutý – fáza je rušená.
    pub rusi: bool,
}

/// Ako dlho po stlačení ostane obraz zapnutý aj vo fáze pokusu, ktorá ho zhasína.
pub const PREBUDENIE_S: u64 = 60;

/// Rozhodnutie podľa pokusu (čas od jeho začiatku) alebo bežnej politiky.
pub fn rozhodni(pokus_s: Option<u64>, necinnost_s: u64, obraz: Ciel, z: &Zhasinanie) -> Rozhodnutie {
    if let Some(i) = pokus_s.and_then(faza) {
        let f = FAZY[i];
        let rusi = f.stav != StavObrazu::Zapnuty && necinnost_s < PREBUDENIE_S;
        return Rozhodnutie {
            obraz: f.obraz,
            stav: if rusi { StavObrazu::Zapnuty } else { f.stav },
            periferie: f.periferie,
            faza: Some(i),
            rusi,
        };
    }
    Rozhodnutie { obraz, stav: z.stav(necinnost_s), periferie: true, faza: None, rusi: false }
}

/// Odhad ustálenej teploty fázy z jej krivky.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Odhad {
    pub zaciatok: f64,
    pub koniec: f64,
    /// Kam by sa teplota ustálila (T∞).
    pub ustalena: f64,
    /// Časová konštanta (s).
    pub tau_s: f64,
    /// Stredná odchýlka bodov od modelu (°C).
    pub odchylka: f64,
    /// τ je dosť krátka voči dĺžke fázy, aby odhad T∞ platil.
    pub spolahlivy: bool,
}

/// Odhad T∞ z bodov (čas od začiatku fázy v s, teplota °C): pre každé τ z mriežky (30 s – 2 h, logaritmicky)
/// lineárne najmenšie štvorce pre T = a + b·e^(−t/τ); vyhrá τ s najmenšou odchýlkou.
pub fn odhad_ustalenej(body: &[(f64, f64)]) -> Option<Odhad> {
    if body.len() < 4 {
        return None;
    }
    let (zaciatok, koniec) = (body[0].1, body[body.len() - 1].1);
    let dlzka = body[body.len() - 1].0 - body[0].0;
    let rozsah =
        body.iter().map(|b| b.1).fold(f64::MIN, f64::max) - body.iter().map(|b| b.1).fold(f64::MAX, f64::min);
    let n = body.len() as f64;
    let priemer = body.iter().map(|b| b.1).sum::<f64>() / n;
    if rozsah < 0.3 {
        // teplota sa nemenila – je ustálená
        return Some(Odhad {
            zaciatok,
            koniec,
            ustalena: priemer,
            tau_s: 0.0,
            odchylka: 0.0,
            spolahlivy: true,
        });
    }
    let t0 = body[0].0;
    // pre dané τ: lineárne najmenšie štvorce pre T = a + b·e^(−t/τ) → (súčet štvorcov, a)
    let prisposob = |tau: f64| -> Option<(f64, f64)> {
        let (mut se, mut see, mut st, mut set) = (0.0, 0.0, 0.0, 0.0);
        for &(t, v) in body {
            let e = (-(t - t0) / tau).exp();
            se += e;
            see += e * e;
            st += v;
            set += e * v;
        }
        let det = n * see - se * se;
        if det.abs() < 1e-9 {
            return None;
        }
        let b = (n * set - se * st) / det;
        let a = (st - b * se) / n;
        let ss: f64 = body.iter().map(|&(t, v)| (v - a - b * (-(t - t0) / tau).exp()).powi(2)).sum();
        Some((ss, a))
    };
    let mriezka = |k: f64| 30.0 * (7200.0f64 / 30.0).powf(k / 120.0);
    let mut najlepsi: Option<(f64, f64, f64, f64)> = None; // (súčet štvorcov, a, τ, k)
    for k in 0..=120 {
        let tau = mriezka(k as f64);
        if let Some((ss, a)) = prisposob(tau)
            && najlepsi.is_none_or(|x| ss < x.0)
        {
            najlepsi = Some((ss, a, tau, k as f64));
        }
    }
    // spresnenie medzi susedmi mriežky (zlatý rez)
    let (_, _, _, k) = najlepsi?;
    let (mut lo, mut hi) = ((k - 1.0).max(0.0), (k + 1.0).min(120.0));
    let g = (5f64.sqrt() - 1.0) / 2.0;
    for _ in 0..40 {
        let (k1, k2) = (hi - g * (hi - lo), lo + g * (hi - lo));
        let s1 = prisposob(mriezka(k1)).map_or(f64::MAX, |x| x.0);
        let s2 = prisposob(mriezka(k2)).map_or(f64::MAX, |x| x.0);
        if s1 < s2 {
            hi = k2;
        } else {
            lo = k1;
        }
    }
    let tau = mriezka((lo + hi) / 2.0);
    if let Some((ss, a)) = prisposob(tau)
        && najlepsi.is_some_and(|x| ss <= x.0)
    {
        najlepsi = Some((ss, a, tau, k));
    }
    let (ss, a, tau, _) = najlepsi?;
    Some(Odhad {
        zaciatok,
        koniec,
        ustalena: a,
        tau_s: tau,
        odchylka: (ss / n).sqrt(),
        spolahlivy: tau <= 1.5 * dlzka.max(1.0),
    })
}

fn c(x: f64) -> String {
    format!("{x:.1}").replace('.', ",")
}

/// Výsledok pokusu do správy: krivka (s od začiatku pokusu, teplota) a rušené fázy.
pub fn vysledok_pokusu(krivka: &[(u64, f64)], rusene: &[bool], nl: &str) -> String {
    let odhady: Vec<Option<Odhad>> = (0..FAZY.len())
        .map(|i| {
            let (od, po) = (i as u64 * FAZA_S, (i as u64 + 1) * FAZA_S);
            let body: Vec<(f64, f64)> =
                krivka.iter().filter(|b| b.0 >= od && b.0 <= po).map(|b| (b.0 as f64, b.1)).collect();
            odhad_ustalenej(&body)
        })
        .collect();
    // posun okolia: rozdiel medzi prvou a poslednou (rovnakou) fázou, rozložený lineárne v čase
    let zaklad = odhady[0].map(|o| o.ustalena);
    let posun = match (zaklad, odhady[FAZY.len() - 1]) {
        (Some(a), Some(z)) => Some(z.ustalena - a),
        _ => None,
    };
    let mut t = format!(
        "  {} fáz po {} min – rovnaké miesto, žiadna práca; ustálená teplota odhadnutá modelom ohrevu{nl}",
        FAZY.len(),
        FAZA_S / 60
    );
    t += &format!(
        "  {:<26} {:>8} {:>8} {:>12} {:>7} {:>10}{nl}",
        "fáza", "začiatok", "koniec", "ustálila by", "τ", "oproti 1.*"
    );
    for (i, f) in FAZY.iter().enumerate() {
        let Some(o) = odhady[i] else {
            t += &format!("  {:<26} {:>8}{nl}", format!("{} {}", i + 1, f.nazov), "nemerané");
            continue;
        };
        let rozdiel = match (zaklad, posun) {
            (Some(a), Some(p)) => Some(o.ustalena - (a + p * i as f64 / (FAZY.len() - 1) as f64)),
            (Some(a), None) => Some(o.ustalena - a),
            _ => None,
        };
        let rozdiel = match rozdiel {
            _ if i == 0 => "základ".to_string(),
            _ if i == FAZY.len() - 1 => "kontrola".to_string(),
            Some(r) => format!("{}{} °C", if r > 0.0 { "+" } else { "" }, c(r)),
            None => "–".into(),
        };
        t += &format!(
            "  {:<26} {:>8} {:>8} {:>12} {:>7} {:>10}{}{}{nl}",
            format!("{} {}", i + 1, f.nazov),
            format!("{} °C", c(o.zaciatok)),
            format!("{} °C", c(o.koniec)),
            format!("{}{} °C", if o.spolahlivy { "" } else { "≈" }, c(o.ustalena)),
            if o.tau_s > 0.0 { format!("{:.0} min", o.tau_s / 60.0) } else { "–".into() },
            rozdiel,
            if rusene.get(i).copied().unwrap_or(false) { " · rušené (ovládač)" } else { "" },
            if o.odchylka > 0.5 { format!(" · odchýlka {} °C", c(o.odchylka)) } else { String::new() },
        );
    }
    match posun {
        Some(p) => {
            t += &format!(
                "  * opravené o posun okolia {}{} °C medzi 1. a {}. fázou (lineárne v čase){nl}",
                if p > 0.0 { "+" } else { "" },
                c(p),
                FAZY.len()
            )
        }
        None => t += &format!("  * bez kontrolnej fázy – posun teploty miestnosti nie je odčítaný{nl}"),
    }
    t += &format!("  ≈ = fáza bola krátka na spoľahlivý odhad (teplota sa ešte výrazne menila){nl}");
    t
}

/// Zariadenia USB s rozhraním úložiska (trieda 08) – adresáre v `/sys/bus/usb/devices`.
pub fn usb_ulozisko(koren: &Path) -> Vec<PathBuf> {
    let zar = koren.join("sys/bus/usb/devices");
    let mut v: Vec<PathBuf> = fs::read_dir(&zar)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|z| {
            let meno = z.file_name().into_string().ok()?;
            let (zariadenie, _) = meno.split_once(':')?;
            let trieda = fs::read_to_string(z.path().join("bInterfaceClass")).ok()?;
            (trieda.trim() == "08").then(|| zar.join(zariadenie))
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Adresáre zariadení SCSI (disk „0:0:0:0“) pod zariadením USB.
fn scsi_pod(adresar: &Path, hlbka: u32, v: &mut Vec<PathBuf>) {
    if hlbka == 0 {
        return;
    }
    for z in fs::read_dir(adresar).into_iter().flatten().flatten() {
        let p = z.path();
        let meno = z.file_name().to_string_lossy().into_owned();
        let casti: Vec<&str> = meno.split(':').collect();
        if casti.len() == 4 && casti.iter().all(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit())) {
            v.push(p);
        } else if (meno.contains(':') || meno.starts_with("host") || meno.starts_with("target"))
            && z.file_type().is_ok_and(|t| t.is_dir())
        {
            scsi_pod(&p, hlbka - 1, v);
        }
    }
}

/// R133: USB úložisko spí medzi zápismi (uspanie jadra po `oneskorenie_ms` bez práce) alebo bdie (`spat` =
/// false – pôvodne). Nastaví zariadenie USB aj disk SCSI na ňom. Vráti mená nastavených zariadení USB.
pub fn nastav_spanok_usb(koren: &Path, spat: bool, oneskorenie_ms: u32) -> Vec<String> {
    let mut mena = Vec::new();
    for z in usb_ulozisko(koren) {
        let mut ciele = vec![z.clone()];
        scsi_pod(&z, 4, &mut ciele);
        let mut ok = true;
        for c in &ciele {
            let _ = fs::write(c.join("power/autosuspend_delay_ms"), oneskorenie_ms.to_string());
            ok &= fs::write(c.join("power/control"), if spat { "auto" } else { "on" }).is_ok();
        }
        if ok && let Some(m) = z.file_name() {
            mena.push(m.to_string_lossy().into_owned());
        }
    }
    mena
}

/// Súčet času v spánku a v činnosti (ms) všetkých USB úložísk – podiel spánku sa počíta z rozdielov.
pub fn cas_spanku_usb(koren: &Path) -> Option<(u64, u64)> {
    let mut spi = 0u64;
    let mut bdie = 0u64;
    let mut nasiel = false;
    for z in usb_ulozisko(koren) {
        let cislo = |s: &str| fs::read_to_string(z.join("power").join(s)).ok()?.trim().parse::<u64>().ok();
        if let (Some(s), Some(a)) = (cislo("runtime_suspended_time"), cislo("runtime_active_time")) {
            spi += s;
            bdie += a;
            nasiel = true;
        }
    }
    nasiel.then_some((spi, bdie))
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn zhasinanie_po_krokoch() {
        let z = Zhasinanie::pre_tv(None, None);
        assert_eq!(z.stav(0), StavObrazu::Zapnuty);
        assert_eq!(z.stav(179), StavObrazu::Zapnuty);
        assert_eq!(z.stav(180), StavObrazu::Cierny);
        assert_eq!(z.stav(600), StavObrazu::Vypnuty);
        let nikdy = Zhasinanie::pre_tv(Some(0), Some(0));
        assert_eq!(nikdy.stav(1_000_000), StavObrazu::Zapnuty);
        let len_vypni = Zhasinanie::pre_tv(Some(0), Some(5));
        assert_eq!(len_vypni.stav(299), StavObrazu::Zapnuty);
        assert_eq!(len_vypni.stav(300), StavObrazu::Vypnuty);
    }

    #[test]
    fn siet_podla_potreby_s_hysterezou() {
        let mut s = SietPodlaPotreby::nova();
        // krátky nárast neprepne
        assert_eq!(s.krok(1.0, 90.0), None);
        assert_eq!(s.krok(1.0, 10.0), None);
        // trvalý prenos nad 60 Mb/s 3 s → gigabit
        assert_eq!(s.krok(1.0, 80.0), None);
        assert_eq!(s.krok(1.0, 80.0), None);
        assert_eq!(s.krok(1.0, 80.0), Some(true));
        assert!(s.gigabit);
        // 30 Mb/s je medzi prahmi – gigabit ostáva (žiadne kmitanie)
        for _ in 0..300 {
            assert_eq!(s.krok(1.0, 30.0), None);
        }
        // pod 15 Mb/s 2 minúty → späť na 100
        for _ in 0..119 {
            assert_eq!(s.krok(1.0, 1.0), None);
        }
        assert_eq!(s.krok(1.0, 1.0), Some(false));
        assert!(!s.gigabit);
    }

    #[test]
    fn pokus_po_fazach_a_rusenie() {
        let z = Zhasinanie::pre_tv(None, None);
        assert_eq!(faza(0), Some(0));
        assert_eq!(faza(FAZA_S * 3 + 5), Some(3));
        assert_eq!(faza(FAZA_S * 6), None);
        assert_eq!(milniky_pokusu(), vec![0, 20, 40, 60, 80, 100, 120]);
        let r = rozhodni(Some(FAZA_S * 3 + 10), 10_000, Ciel::Chladny, &z);
        assert_eq!((r.stav, r.faza, r.rusi), (StavObrazu::Cierny, Some(3), false));
        // ovládač stlačený pred 20 s – obraz sa zapne, fáza je rušená
        let r = rozhodni(Some(FAZA_S * 3 + 10), 20, Ciel::Chladny, &z);
        assert_eq!((r.stav, r.rusi), (StavObrazu::Zapnuty, true));
        let r = rozhodni(Some(10), 0, Ciel::Chladny, &z);
        assert_eq!((r.obraz, r.periferie, r.rusi), (Ciel::Povodny, false, false));
        // po pokuse bežná politika
        let r = rozhodni(Some(FAZA_S * 7), 700, Ciel::Chladny, &z);
        assert_eq!((r.obraz, r.stav, r.faza), (Ciel::Chladny, StavObrazu::Vypnuty, None));
        let r = rozhodni(None, 0, Ciel::Chladny, &z);
        assert!(r.periferie);
    }

    #[test]
    fn odhad_ustalenej_teploty_z_casti_krivky() {
        // ohrev 36 → 61 °C s τ = 8 min, meraný 20 min po 30 s (ako X96 vo Full HD) s kvantovaním 0,1 °C
        let body: Vec<(f64, f64)> = (0..=40)
            .map(|k| {
                let t = k as f64 * 30.0;
                (t, ((61.0 - 25.0 * (-t / 480.0).exp()) * 10.0).round() / 10.0)
            })
            .collect();
        let o = odhad_ustalenej(&body).expect("odhad");
        assert!((o.ustalena - 61.0).abs() < 0.3, "{o:?}");
        assert!((o.tau_s - 480.0).abs() < 60.0, "{o:?}");
        assert!(o.spolahlivy);
        // chladnutie 61 → 52 °C
        let body: Vec<(f64, f64)> =
            (0..=40).map(|k| (k as f64 * 30.0, 52.0 + 9.0 * (-(k as f64 * 30.0) / 400.0).exp())).collect();
        let o = odhad_ustalenej(&body).expect("odhad");
        assert!((o.ustalena - 52.0).abs() < 0.2, "{o:?}");
        // ustálená teplota
        let body: Vec<(f64, f64)> = (0..10).map(|k| (k as f64 * 30.0, 50.0 + 0.1 * (k % 2) as f64)).collect();
        assert!((odhad_ustalenej(&body).expect("odhad").ustalena - 50.05).abs() < 0.01);
        assert_eq!(odhad_ustalenej(&body[..3]), None);
    }

    #[test]
    fn vysledok_pokusu_odcita_posun_okolia() {
        // ustálené teploty fáz 60, 58, 54, 50, 47, 61 (okolie stúplo o 1 °C), každá fáza sa blíži k cieľu
        let ciele = [60.0, 58.0, 54.0, 50.0, 47.0, 61.0];
        let mut krivka = Vec::new();
        let mut t0 = 35.0;
        for (i, &ciel) in ciele.iter().enumerate() {
            for k in 0..=40u64 {
                let s = i as u64 * FAZA_S + k * 30;
                if k == 40 && i + 1 < ciele.len() {
                    continue;
                }
                krivka.push((s, ciel + (t0 - ciel) * (-(k as f64 * 30.0) / 480.0).exp()));
            }
            t0 = ciel + (t0 - ciel) * (-(1200.0f64) / 480.0).exp();
        }
        let r = vysledok_pokusu(&krivka, &[false, false, false, true, false, false], "\n");
        assert!(r.contains("3 chladný obraz"), "{r}");
        // fáza 3: 54 − (60 + 1·2/5) = −6,4 °C (model odhadne ustálené teploty na ±0,1 °C)
        assert!(r.contains("-6,4 °C") || r.contains("-6,5 °C") || r.contains("-6,3 °C"), "{r}");
        assert!(r.contains("rušené"), "{r}");
        assert!(r.contains("posun okolia +1,0 °C") || r.contains("posun okolia +0,9 °C"), "{r}");
    }

    #[test]
    fn usb_ulozisko_spi_a_meria_sa_spanok() {
        let k = std::env::temp_dir().join(format!("sklenik-usb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&k);
        let d = k.join("sys/bus/usb/devices");
        let disk = d.join("1-2/1-2:1.0/host0/target0:0:0/0:0:0:0");
        fs::create_dir_all(disk.join("power")).unwrap();
        fs::create_dir_all(d.join("1-2/power")).unwrap();
        // rozhranie úložiska (v skutočnom /sys odkaz na adresár rozhrania)
        fs::create_dir_all(d.join("1-2:1.0")).ok();
        fs::write(d.join("1-2:1.0/bInterfaceClass"), "08\n").unwrap();
        fs::create_dir_all(d.join("1-3:1.0")).unwrap();
        fs::write(d.join("1-3:1.0/bInterfaceClass"), "03\n").unwrap();
        fs::write(d.join("1-2/power/runtime_suspended_time"), "900\n").unwrap();
        fs::write(d.join("1-2/power/runtime_active_time"), "100\n").unwrap();
        assert_eq!(usb_ulozisko(&k), vec![d.join("1-2")]);
        assert_eq!(nastav_spanok_usb(&k, true, 2000), vec!["1-2".to_string()]);
        assert_eq!(fs::read_to_string(d.join("1-2/power/control")).unwrap(), "auto");
        assert_eq!(fs::read_to_string(disk.join("power/control")).unwrap(), "auto");
        assert_eq!(fs::read_to_string(disk.join("power/autosuspend_delay_ms")).unwrap(), "2000");
        nastav_spanok_usb(&k, false, 2000);
        assert_eq!(fs::read_to_string(d.join("1-2/power/control")).unwrap(), "on");
        assert_eq!(cas_spanku_usb(&k), Some((900, 100)));
        let _ = fs::remove_dir_all(&k);
    }
}
