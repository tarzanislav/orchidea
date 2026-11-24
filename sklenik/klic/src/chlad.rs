//! SPRÁVCA CHLADU pre TV krabičky (R158–R161) – čistá logika bez hardvéru, skúša sa na každom systéme.
//!
//! Meranie X96 Max Plus ukázalo, že v pokoji hreje obraz a periférie, nie procesor. Správca rozhoduje:
//! - **obraz** (R158): chladný režim (najnižšia obnova Full HD, 16 bitov, polovičné rozlíšenie so zväčšením
//!   v obvode obrazu) a **zhasínanie v pokoji** – najprv čierny obraz (nič sa nečíta z pamäte, signál ostáva),
//!   neskôr vypnutý (stojí aj vysielač HDMI); prebudí ho diaľkový ovládač, klávesnica alebo myš,
//! - **USB úložisko spí** medzi zápismi (R159) – kľúč sa zobudí len na zápis merania,
//! - **sieť podľa potreby** (R159): 100 Mb/s, gigabit len pri veľkom prenose – podľa výskumu
//!   *Adaptive Link Rate* (Gunaratne, Christensen, Nordman, Suen; IEEE Trans. Computers 2008): dva prahy
//!   s hysteréziou, aby spojenie nekmitalo,
//! - **tepelný pokus** (R160): po fázach 20 minút porovná pôvodný stav, úsporné periférie, chladný, čierny
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

/// Sieť podľa potreby (R159, R170 – Adaptive Link Rate v troch stupňoch).
///
/// 10BASE-T v pokoji posiela len krátke impulzy (každých 16 ms), 100BASE-TX vysiela nepretržite – preto
/// v hlbokom pokoji 10 Mb/s, pri bežnej práci 100 Mb/s a gigabit len pri veľkom prenose. Každý stupeň má dva
/// prahy (hore rýchlo, dole pomaly), aby spojenie nekmitalo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SietPodlaPotreby {
    /// Najvyššia ponúkaná rýchlosť (Mb/s): 10, 100 alebo 1000.
    pub mbs: u32,
    nad_s: f64,
    pod_s: f64,
}

/// Prenos (Mb/s), pri ktorom 100 Mb/s začína brzdiť – prejde sa na gigabit (po 3 s).
pub const ALR_HORE_MBS: f64 = 60.0;
/// Prenos, pod ktorým gigabit netreba (po 2 min) – späť na 100 Mb/s.
pub const ALR_DOLE_MBS: f64 = 15.0;
/// Prenos, pri ktorom 10 Mb/s nestačí – hneď na 100 Mb/s (po 2 s).
pub const ALR_Z_POKOJA_MBS: f64 = 2.0;
/// Hlboký pokoj: pod týmto prenosom 5 minút → 10 Mb/s.
pub const ALR_POKOJ_MBS: f64 = 0.2;

impl SietPodlaPotreby {
    pub fn nova() -> SietPodlaPotreby {
        SietPodlaPotreby { mbs: 100, nad_s: 0.0, pod_s: 0.0 }
    }

    /// Krok: čas od posledného kroku (s) a prenos (Mb/s – väčší z príjmu a odosielania). Vráti novú najvyššiu
    /// rýchlosť, keď sa má ponuka zmeniť.
    pub fn krok(&mut self, dt_s: f64, mbit_s: f64) -> Option<u32> {
        let (hore_prah, hore_s, dole_prah, dole_s, vyssia, nizsia) = match self.mbs {
            10 => (ALR_Z_POKOJA_MBS, 2.0, f64::MIN, f64::MAX, 100, 10),
            100 => (ALR_HORE_MBS, 3.0, ALR_POKOJ_MBS, 300.0, 1000, 10),
            _ => (f64::MAX, f64::MAX, ALR_DOLE_MBS, 120.0, 1000, 100),
        };
        self.nad_s = if mbit_s >= hore_prah { self.nad_s + dt_s } else { 0.0 };
        self.pod_s = if mbit_s < dole_prah { self.pod_s + dt_s } else { 0.0 };
        let novy = if self.nad_s >= hore_s {
            vyssia
        } else if self.pod_s >= dole_s {
            nizsia
        } else {
            return None;
        };
        *self = SietPodlaPotreby { mbs: novy, nad_s: 0.0, pod_s: 0.0 };
        Some(novy)
    }
}

/// R170: čip siete bez kábla spí. Rozhranie bez spojenia sa po minúte vypne (PHY do úsporného stavu, takty
/// radiča preč) a raz za 30 s sa na pár sekúnd zapne – ak sa medzitým zasunul kábel, spojenie sa dohodne.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kabel {
    /// Rozhranie zapnuté; ako dlho je bez spojenia.
    Zapnute { bez_kabla_s: f64 },
    /// Rozhranie vypnuté (čip spí).
    Spi { s: f64 },
    /// Rozhranie na chvíľu zapnuté – hľadá sa kábel.
    Hlada { s: f64 },
}

/// Čo urobiť s rozhraním po kroku.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrechodKabla {
    /// Prvýkrát bez kábla – vypnúť (oznámiť).
    Uspi,
    /// Krátko zapnúť a hľadať kábel.
    Hladaj,
    /// Kábel sa nenašiel – znova vypnúť.
    Spat,
    /// Kábel sa našiel – rozhranie ostáva zapnuté (oznámiť).
    Najdeny,
    /// R191: kábel je zapojený, ale sieť nič nepotrebuje – čip siete znova spí.
    Overeny,
}

/// Bez spojenia takto dlho → čip siete spí.
pub const BEZ_KABLA_S: f64 = 60.0;
/// Ako dlho čip spí medzi hľadaniami kábla.
pub const SPANOK_PHY_S: f64 = 30.0;
/// Ako dlho sa čaká na spojenie pri hľadaní (dohodnutie 10/100 Mb/s trvá 2–3 s).
pub const HLADANIE_S: f64 = 6.0;
/// R191: ako dlho spí čip siete, keď sieť nič nepotrebuje (kábel sa len občas overí).
pub const SPANOK_NEPOTREBNEJ_S: f64 = 300.0;

impl Kabel {
    pub fn novy() -> Kabel {
        Kabel::Zapnute { bez_kabla_s: 0.0 }
    }

    /// Riadi ho správca (spí alebo hľadá) – stav „bez spojenia“ je zámer, nie zmena.
    pub fn riadeny(&self) -> bool {
        !matches!(self, Kabel::Zapnute { .. })
    }

    /// Krok: čas od posledného kroku (s) a či je spojenie (kábel s dohodnutou rýchlosťou).
    #[cfg(test)]
    pub fn krok(&mut self, dt_s: f64, spojenie: bool) -> Option<PrechodKabla> {
        self.krok_s_potrebou(dt_s, spojenie, true)
    }

    /// R191: krok s ohľadom na to, či sieť niečo potrebuje. SKLENÍK zatiaľ nemá sieťové služby (spojenie s PC
    /// KORENE príde neskôr) – keď sieť nič nepotrebuje, čip siete spí aj so zapojeným káblom a kábel sa overí
    /// raz za 5 minút (gigabitový čip LAN7515 na RPi 3 B+ je najväčší zdroj tepla v pokoji).
    pub fn krok_s_potrebou(&mut self, dt_s: f64, spojenie: bool, potrebna: bool) -> Option<PrechodKabla> {
        if !potrebna {
            let (novy, prechod) = match *self {
                Kabel::Zapnute { bez_kabla_s } if bez_kabla_s + dt_s >= BEZ_KABLA_S => {
                    (Kabel::Spi { s: 0.0 }, Some(PrechodKabla::Uspi))
                }
                Kabel::Zapnute { bez_kabla_s } => (Kabel::Zapnute { bez_kabla_s: bez_kabla_s + dt_s }, None),
                Kabel::Spi { s } if s + dt_s >= SPANOK_NEPOTREBNEJ_S => {
                    (Kabel::Hlada { s: 0.0 }, Some(PrechodKabla::Hladaj))
                }
                Kabel::Spi { s } => (Kabel::Spi { s: s + dt_s }, None),
                Kabel::Hlada { .. } if spojenie => (Kabel::Spi { s: 0.0 }, Some(PrechodKabla::Overeny)),
                Kabel::Hlada { s } if s + dt_s >= HLADANIE_S => {
                    (Kabel::Spi { s: 0.0 }, Some(PrechodKabla::Spat))
                }
                Kabel::Hlada { s } => (Kabel::Hlada { s: s + dt_s }, None),
            };
            *self = novy;
            return prechod;
        }
        let (novy, prechod) = match *self {
            Kabel::Zapnute { .. } if spojenie => (Kabel::novy(), None),
            Kabel::Zapnute { bez_kabla_s } if bez_kabla_s + dt_s >= BEZ_KABLA_S => {
                (Kabel::Spi { s: 0.0 }, Some(PrechodKabla::Uspi))
            }
            Kabel::Zapnute { bez_kabla_s } => (Kabel::Zapnute { bez_kabla_s: bez_kabla_s + dt_s }, None),
            Kabel::Spi { s } if s + dt_s >= SPANOK_PHY_S => {
                (Kabel::Hlada { s: 0.0 }, Some(PrechodKabla::Hladaj))
            }
            Kabel::Spi { s } => (Kabel::Spi { s: s + dt_s }, None),
            Kabel::Hlada { .. } if spojenie => (Kabel::novy(), Some(PrechodKabla::Najdeny)),
            Kabel::Hlada { s } if s + dt_s >= HLADANIE_S => (Kabel::Spi { s: 0.0 }, Some(PrechodKabla::Spat)),
            Kabel::Hlada { s } => (Kabel::Hlada { s: s + dt_s }, None),
        };
        *self = novy;
        prechod
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
    /// Takt obvodu obrazu VPU v MHz (R166; 666 = ako hlavné jadro, 333 = Full HD, 51 = zaparkovaný).
    pub vpu_mhz: u32,
    /// R178: nepoužité radiče kariet odpojené (eMMC Androidu, slot Wi-Fi bez ovládača).
    pub radice: bool,
    /// R184: záťaž počas fázy dlhého testu hardvéru.
    pub zataz: Zataz,
    /// R184: hlboký spánok jadier (R167) smie byť zapnutý.
    pub spanok: bool,
}

/// R184: čo počas fázy dlhého testu pracuje.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zataz {
    Ziadna,
    /// Všetky jadrá počítajú úlohu so známym výsledkom (test_jadier).
    Jadra,
    /// Zápis a kontrola veľkých blokov RAM (test_pamate).
    Pamat,
}

/// Dĺžka fázy (s) – asi 2 časové konštanty krabičky (X96: τ ≈ 5–9 min); zvyšok dopočíta model.
pub const FAZA_S: u64 = 15 * 60;

/// Takt VPU: ako hlavné jadro (aj 4K), dosť pre Full HD, zaparkovaný pri vypnutom obraze (R166).
///
/// R175 (meranie 23. 11. 2026, X96 Max Plus, 1080p @ 60 Hz): 333 MHz stačí na obraz Full HD bez
/// zväčšovania, ale **nie na zväčšovaný chladný obraz** (960×540 → 1920×1080) – rovina OSD nestíha čítať,
/// chýba modrá zložka, obraz sa posunie a vľavo je pás. Pokus 23. 11. popoludní: **500 MHz zväčšovaný obraz
/// zvládne** (fáza 4 bez chýb, −0,7 °C oproti 666 MHz) – zväčšovaný obraz ide na stredný takt.
pub const VPU_PLNY: u32 = 666;
/// Stredný takt (zdroj fclk_div4) – stačí na zväčšovaný obraz 1080p60 (R175, pokus 23. 11. 2026).
pub const VPU_STREDNY: u32 = 500;
pub const VPU_PARKOVANY: u32 = 51;

const fn f(nazov: &'static str, obraz: Ciel, stav: StavObrazu, periferie: bool, vpu_mhz: u32) -> Faza {
    Faza { nazov, obraz, stav, periferie, vpu_mhz, radice: false, zataz: Zataz::Ziadna, spanok: false }
}

/// R184: fáza dlhého testu – vypnutý obraz, úsporné periférie, odpojené radiče, zaparkované VPU.
const fn test_hw(nazov: &'static str, zataz: Zataz, spanok: bool) -> Faza {
    Faza {
        nazov,
        obraz: Ciel::Chladny,
        stav: StavObrazu::Vypnuty,
        periferie: true,
        vpu_mhz: VPU_PARKOVANY,
        radice: true,
        zataz,
        spanok,
    }
}

const fn s_radicmi(mut z: Faza, nazov: &'static str) -> Faza {
    z.nazov = nazov;
    z.radice = true;
    z
}

/// Index kontrolnej fázy „pôvodne – kontrola“ (základ porovnania); po nej nasledujú fázy dlhého testu.
pub const ZAKLAD: usize = 8;
/// Počet fáz bežného tepelného pokusu.
pub const ZAKLADNE_FAZY: usize = ZAKLAD + 1;

/// Počet fáz: bežný pokus 9 (2 h 15 min), dlhý test hardvéru 12 (3 h, R184).
pub fn pocet_faz(dlhy: bool) -> usize {
    if dlhy { FAZY.len() } else { ZAKLADNE_FAZY }
}

pub const FAZY: [Faza; 12] = [
    f("pôvodne (ako doteraz)", Ciel::Povodny, StavObrazu::Zapnuty, false, VPU_PLNY),
    f("úsporné periférie", Ciel::Povodny, StavObrazu::Zapnuty, true, VPU_PLNY),
    f("chladný obraz", Ciel::Chladny, StavObrazu::Zapnuty, true, VPU_PLNY),
    f("chladný + takt VPU 500", Ciel::Chladny, StavObrazu::Zapnuty, true, VPU_STREDNY),
    f("čierny obraz", Ciel::Chladny, StavObrazu::Cierny, true, VPU_PLNY),
    f("obraz vypnutý", Ciel::Chladny, StavObrazu::Vypnuty, true, VPU_PLNY),
    f("vypnutý + VPU zaparkovaný", Ciel::Chladny, StavObrazu::Vypnuty, true, VPU_PARKOVANY),
    s_radicmi(f("", Ciel::Chladny, StavObrazu::Vypnuty, true, VPU_PARKOVANY), "+ odpojené eMMC a Wi-Fi"),
    f("pôvodne – kontrola", Ciel::Povodny, StavObrazu::Zapnuty, false, VPU_PLNY),
    // R184 – dlhý test hardvéru
    test_hw("záťaž – všetky jadrá", Zataz::Jadra, false),
    test_hw("záťaž – pamäť RAM", Zataz::Pamat, false),
    test_hw("chladnutie + spánok jadier", Zataz::Ziadna, true),
];

/// Takt VPU mimo pokusu (R182): **len vypnutý obraz → zaparkovaný**, inak vždy plný takt. Televízor (HDMI
/// s YCbCr) je na takt obvodu obrazu citlivejší ako monitor cez redukciu do VGA – kým sa nižší takt pri
/// zapnutom obraze neoverí na oboch, mení sa len tam, kde obraz aj tak nie je (−1,4 °C z pokusu 23. 11.).
/// Pred zapnutím obrazu sa takt vráti na plný (KLÍČ zvyšuje pred zmenou obrazu, znižuje až po nej).
pub fn takt_vpu(stav: StavObrazu) -> u32 {
    match stav {
        StavObrazu::Vypnuty => VPU_PARKOVANY,
        _ => VPU_PLNY,
    }
}

/// Fáza v danom čase od začiatku pokusu; `None` po skončení.
pub fn faza(s: u64, pocet: usize) -> Option<usize> {
    let i = (s / FAZA_S) as usize;
    (i < pocet.min(FAZY.len())).then_some(i)
}

/// Minúty záznamov samomerania pri pokuse: začiatok a koniec každej fázy.
pub fn milniky_pokusu(pocet: usize) -> Vec<u32> {
    (0..=pocet.min(FAZY.len()) as u32).map(|i| i * (FAZA_S / 60) as u32).collect()
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
    /// Takt VPU, ktorý chce fáza pokusu (mimo pokusu `None` – podľa režimu, `takt_vpu`).
    pub vpu_mhz: Option<u32>,
    /// R178: odpojiť nepoužité radiče kariet.
    pub radice: bool,
    /// R167: hlboký spánok jadier smie byť zapnutý (v pokuse nie – fázy sa porovnávajú s predošlými pokusmi).
    pub jadra_spia: bool,
    /// R184: záťaž dlhého testu hardvéru.
    pub zataz: Zataz,
}

/// Ako dlho po stlačení ostane obraz zapnutý aj vo fáze pokusu, ktorá ho zhasína.
pub const PREBUDENIE_S: u64 = 60;

/// Rozhodnutie podľa pokusu (čas od jeho začiatku) alebo bežnej politiky.
pub fn rozhodni(
    pokus_s: Option<u64>,
    pocet: usize,
    necinnost_s: u64,
    obraz: Ciel,
    z: &Zhasinanie,
) -> Rozhodnutie {
    if let Some(i) = pokus_s.and_then(|s| faza(s, pocet)) {
        let f = FAZY[i];
        let rusi = f.stav != StavObrazu::Zapnuty && necinnost_s < PREBUDENIE_S;
        return Rozhodnutie {
            obraz: f.obraz,
            stav: if rusi { StavObrazu::Zapnuty } else { f.stav },
            periferie: f.periferie,
            faza: Some(i),
            rusi,
            // prebudený obraz vo fáze s vypnutým obrazom potrebuje takt na zobrazenie
            // rušená fáza: zapnutý chladný obraz je zväčšovaný – stredný takt stačí (R175)
            vpu_mhz: Some(if rusi { f.vpu_mhz.max(VPU_STREDNY) } else { f.vpu_mhz }),
            radice: f.radice,
            jadra_spia: f.spanok,
            zataz: f.zataz,
        };
    }
    Rozhodnutie {
        obraz,
        stav: z.stav(necinnost_s),
        periferie: true,
        faza: None,
        rusi: false,
        vpu_mhz: None,
        radice: true,
        jadra_spia: true,
        zataz: Zataz::Ziadna,
    }
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
///
/// Prvé meranie X96 (23. 11. 2026) ukázalo, že fáza 1 začína so studenou skrinkou: popri rýchlom ohreve
/// čipu (τ ≈ 8 min) sa skrinka a podložka zohrievajú oveľa pomalšie, takže ustálená teplota fázy 1 vyjde
/// nižšia ako v rovnakej fáze 6 (+4,4 °C pri izbe stálej 24,5–24,9 °C). Lineárna oprava „posunu okolia“ by
/// preto skresľovala. Základom je **fáza 6** (rovnaký stav ako fáza 1, ale so zohriatou krabičkou ako
/// ostatné fázy) a vedľa je **zmena oproti predošlej fáze** – tá je najspoľahlivejšia.
pub fn vysledok_pokusu(krivka: &[(u64, f64)], rusene: &[bool], pocet: usize, nl: &str) -> String {
    let n = pocet.min(FAZY.len());
    let zakl = ZAKLAD.min(n - 1);
    let odhady: Vec<Option<Odhad>> = (0..n)
        .map(|i| {
            let (od, po) = (i as u64 * FAZA_S, (i as u64 + 1) * FAZA_S);
            let body: Vec<(f64, f64)> =
                krivka.iter().filter(|b| b.0 >= od && b.0 <= po).map(|b| (b.0 as f64, b.1)).collect();
            odhad_ustalenej(&body)
        })
        .collect();
    let zaklad = odhady[zakl].or(odhady[0]).map(|o| o.ustalena);
    let znamienko = |r: f64| format!("{}{} °C", if r > 0.0 { "+" } else { "" }, c(r));
    let mut t = format!(
        "  {n} fáz po {} min – rovnaké miesto, žiadna práca; ustálená teplota odhadnutá modelom ohrevu{nl}",
        FAZA_S / 60
    );
    t += &format!(
        "  {:<26} {:>8} {:>8} {:>12} {:>7} {:>10} {:>12}{nl}",
        "fáza", "začiatok", "koniec", "ustálila by", "τ", "krok", "oproti pôv.*"
    );
    for (i, f) in FAZY.iter().enumerate().take(n) {
        let Some(o) = odhady[i] else {
            t += &format!("  {:<26} {:>8}{nl}", format!("{} {}", i + 1, f.nazov), "nemerané");
            continue;
        };
        let krok = match (i, i.checked_sub(1).and_then(|p| odhady[p])) {
            (0, _) => "–".to_string(),
            (_, _) if i == zakl => "–".to_string(),
            (_, Some(p)) => znamienko(o.ustalena - p.ustalena),
            _ => "–".into(),
        };
        let oproti = match zaklad {
            _ if i == zakl => "základ".to_string(),
            _ if i == 0 => "studený štart".to_string(),
            Some(z) => znamienko(o.ustalena - z),
            None => "–".into(),
        };
        t += &format!(
            "  {:<26} {:>8} {:>8} {:>12} {:>7} {:>10} {:>12}{}{}{nl}",
            format!("{} {}", i + 1, f.nazov),
            format!("{} °C", c(o.zaciatok)),
            format!("{} °C", c(o.koniec)),
            format!("{}{} °C", if o.spolahlivy { "" } else { "≈" }, c(o.ustalena)),
            if o.tau_s > 0.0 { format!("{:.0} min", o.tau_s / 60.0) } else { "–".into() },
            krok,
            oproti,
            if rusene.get(i).copied().unwrap_or(false) { " · rušené (ovládač)" } else { "" },
            if o.odchylka > 0.5 { format!(" · odchýlka {} °C", c(o.odchylka)) } else { String::new() },
        );
    }
    if let (Some(a), Some(z)) = (odhady[0], odhady[zakl]) {
        t += &format!(
            "  * základ = fáza {} (pôvodný stav so zohriatou krabičkou); fáza 1 vyšla o {} – pomalé zohrievanie skrinky{nl}",
            zakl + 1,
            znamienko(a.ustalena - z.ustalena)
        );
    } else {
        t += &format!(
            "  * bez kontrolnej fázy {} – základom je fáza 1 (studený štart, krok je spoľahlivejší){nl}",
            zakl + 1
        );
    }
    t += &format!("  krok = zmena oproti predošlej fáze · ≈ = fáza krátka na spoľahlivý odhad{nl}");
    if n > ZAKLADNE_FAZY {
        t += &format!(
            "  fázy {}–{n}: dlhý test hardvéru (R184) – záťaž jadier a pamäte pri vypnutom obraze, potom chladnutie{nl}",
            ZAKLADNE_FAZY + 1
        );
    }
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

/// R159: USB úložisko spí medzi zápismi (uspanie jadra po `oneskorenie_ms` bez práce) alebo bdie (`spat` =
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

/// R191: všetky zariadenia USB okrem úložísk (tie riadi R159) – (cesta, je HID, je rozbočovač, je koreňový).
fn usb_ostatne(koren: &Path) -> Vec<(PathBuf, bool, bool, bool)> {
    let zar = koren.join("sys/bus/usb/devices");
    let ulozisko = usb_ulozisko(koren);
    let mut v: Vec<(PathBuf, bool, bool, bool)> = fs::read_dir(&zar)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|z| {
            let meno = z.file_name().into_string().ok()?;
            if meno.contains(':') || !z.path().join("power/control").exists() {
                return None;
            }
            let p = zar.join(&meno);
            if ulozisko.contains(&p) {
                return None;
            }
            let predpona = format!("{meno}:");
            let hid = fs::read_dir(&zar).into_iter().flatten().flatten().any(|i| {
                i.file_name().to_string_lossy().starts_with(&predpona)
                    && fs::read_to_string(i.path().join("bInterfaceClass")).is_ok_and(|t| t.trim() == "03")
            });
            let rozbocovac = fs::read_to_string(p.join("bDeviceClass")).is_ok_and(|t| t.trim() == "09");
            Some((p, hid, rozbocovac, meno.starts_with("usb")))
        })
        .collect();
    v.sort();
    v
}

/// Oneskorenie uspania HID (klávesnica, myš, ovládač): dlhšie ako pauza pri písaní; stlačenie ho zobudí.
pub const USB_HID_SPANOK_MS: u32 = 4000;
/// Oneskorenie uspania ostatných zariadení USB (čip siete, rozbočovače).
pub const USB_STROM_SPANOK_MS: u32 = 2000;

/// R191: **celý strom USB spí**, keď sa nič nedeje – klávesnica a myš (zobudí ich stlačenie), čip siete
/// (LAN7515 na RPi 3 B+ – rozbočovač a gigabitový Ethernet v jednom), rozbočovače až po koreň. Keď spia
/// všetky periodické zariadenia, radič USB prestane plánovať mikrorámce – na RPi 1–3 zmizne 8000 budení
/// procesora za sekundu (FIQ radiča dwc_otg). `spat` = false vráti pôvodný stav jadra (rozbočovače auto,
/// ostatné on). Vráti počet nastavených zariadení.
pub fn nastav_spanok_usb_stromu(koren: &Path, spat: bool) -> usize {
    let mut n = 0;
    for (p, hid, rozbocovac, _) in usb_ostatne(koren) {
        let oneskorenie = if hid { USB_HID_SPANOK_MS } else { USB_STROM_SPANOK_MS };
        let _ = fs::write(p.join("power/autosuspend_delay_ms"), oneskorenie.to_string());
        let stav = if spat || rozbocovac { "auto" } else { "on" };
        if fs::write(p.join("power/control"), stav).is_ok() {
            n += 1;
        }
    }
    n
}

/// R191: súčet času v spánku a v činnosti (ms) zariadení stromu USB bez koreňových rozbočovačov a úložísk.
pub fn cas_spanku_usb_stromu(koren: &Path) -> Option<(u64, u64)> {
    let (mut spi, mut bdie, mut nasiel) = (0u64, 0u64, false);
    for (p, _, _, koren_hub) in usb_ostatne(koren) {
        if koren_hub {
            continue;
        }
        let cislo = |s: &str| fs::read_to_string(p.join("power").join(s)).ok()?.trim().parse::<u64>().ok();
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
    fn siet_podla_potreby_v_troch_stupnoch() {
        let mut s = SietPodlaPotreby::nova();
        assert_eq!(s.mbs, 100);
        assert_eq!(s.krok(1.0, 90.0), None);
        assert_eq!(s.krok(1.0, 10.0), None);
        assert_eq!(s.krok(1.0, 80.0), None);
        assert_eq!(s.krok(1.0, 80.0), None);
        assert_eq!(s.krok(1.0, 80.0), Some(1000));
        assert_eq!(s.mbs, 1000);
        for _ in 0..300 {
            assert_eq!(s.krok(1.0, 30.0), None);
        }
        for _ in 0..119 {
            assert_eq!(s.krok(1.0, 1.0), None);
        }
        assert_eq!(s.krok(1.0, 1.0), Some(100));
        // 1 Mb/s nie je hlboký pokoj – 100 ostáva
        for _ in 0..600 {
            assert_eq!(s.krok(1.0, 1.0), None);
        }
        // hlboký pokoj 5 minút → 10 Mb/s
        for _ in 0..299 {
            assert_eq!(s.krok(1.0, 0.01), None);
        }
        assert_eq!(s.krok(1.0, 0.01), Some(10));
        // prenos 2 s nad 2 Mb/s → hneď 100
        assert_eq!(s.krok(1.0, 5.0), None);
        assert_eq!(s.krok(1.0, 5.0), Some(100));
    }

    #[test]
    fn cip_siete_bez_kabla_spi_a_hlada() {
        let mut k = Kabel::novy();
        // so spojením sa nič nedeje
        for _ in 0..100 {
            assert_eq!(k.krok(1.0, true), None);
        }
        // kábel vytiahnutý: po minúte spí
        for _ in 0..59 {
            assert_eq!(k.krok(1.0, false), None);
        }
        assert_eq!(k.krok(1.0, false), Some(PrechodKabla::Uspi));
        assert!(k.riadeny());
        // po 30 s krátko hľadá, bez kábla znova spí
        for _ in 0..29 {
            assert_eq!(k.krok(1.0, false), None);
        }
        assert_eq!(k.krok(1.0, false), Some(PrechodKabla::Hladaj));
        for _ in 0..5 {
            assert_eq!(k.krok(1.0, false), None);
        }
        assert_eq!(k.krok(1.0, false), Some(PrechodKabla::Spat));
        // zapína sa najviac 6 s z 36 s – čip spí 5/6 času
        for _ in 0..29 {
            assert_eq!(k.krok(1.0, false), None);
        }
        assert_eq!(k.krok(1.0, false), Some(PrechodKabla::Hladaj));
        assert_eq!(k.krok(1.0, false), None);
        // kábel zasunutý počas hľadania
        assert_eq!(k.krok(1.0, true), Some(PrechodKabla::Najdeny));
        assert!(!k.riadeny());
        // krátky výpadok spojenia (prepnutie rýchlosti) čip neuspí
        for _ in 0..10 {
            assert_eq!(k.krok(1.0, false), None);
        }
        assert_eq!(k.krok(1.0, true), None);
        assert_eq!(k, Kabel::novy());
    }

    #[test]
    fn pokus_po_fazach_a_rusenie() {
        let z = Zhasinanie::pre_tv(None, None);
        assert_eq!(faza(0, ZAKLADNE_FAZY), Some(0));
        assert_eq!(faza(FAZA_S * 3 + 5, ZAKLADNE_FAZY), Some(3));
        assert_eq!(faza(FAZA_S * 8, ZAKLADNE_FAZY), Some(8));
        assert_eq!(faza(FAZA_S * 9, ZAKLADNE_FAZY), None);
        assert_eq!(faza(FAZA_S * 9, pocet_faz(true)), Some(9), "dlhý test pokračuje záťažou");
        assert_eq!(faza(FAZA_S * 12, pocet_faz(true)), None);
        assert_eq!((FAZY[9].zataz, FAZY[10].zataz, FAZY[11].spanok), (Zataz::Jadra, Zataz::Pamat, true));
        assert!(FAZY[7].radice && !FAZY[6].radice && !FAZY[8].radice, "R178 len vo fáze 8");
        assert_eq!(milniky_pokusu(ZAKLADNE_FAZY), vec![0, 15, 30, 45, 60, 75, 90, 105, 120, 135]);
        assert_eq!(milniky_pokusu(pocet_faz(true)).last(), Some(&180));
        assert_eq!(
            rozhodni(Some(FAZA_S * 6 + 1), ZAKLADNE_FAZY, 10_000, Ciel::Chladny, &z).vpu_mhz,
            Some(VPU_PARKOVANY)
        );
        assert_eq!(
            rozhodni(Some(FAZA_S * 6 + 1), ZAKLADNE_FAZY, 5, Ciel::Chladny, &z).vpu_mhz,
            Some(VPU_STREDNY)
        );
        assert_eq!(
            rozhodni(Some(FAZA_S * 3 + 1), ZAKLADNE_FAZY, 10_000, Ciel::Chladny, &z).vpu_mhz,
            Some(VPU_STREDNY)
        );
        assert_eq!(takt_vpu(StavObrazu::Zapnuty), VPU_PLNY, "pri obraze plný – R182");
        assert_eq!(takt_vpu(StavObrazu::Cierny), VPU_PLNY);
        assert_eq!(takt_vpu(StavObrazu::Vypnuty), VPU_PARKOVANY);
        let r = rozhodni(Some(FAZA_S * 4 + 10), ZAKLADNE_FAZY, 10_000, Ciel::Chladny, &z);
        assert_eq!((r.stav, r.faza, r.rusi), (StavObrazu::Cierny, Some(4), false));
        // ovládač stlačený pred 20 s – obraz sa zapne, fáza je rušená
        let r = rozhodni(Some(FAZA_S * 4 + 10), ZAKLADNE_FAZY, 20, Ciel::Chladny, &z);
        assert_eq!((r.stav, r.rusi), (StavObrazu::Zapnuty, true));
        let r = rozhodni(Some(10), ZAKLADNE_FAZY, 0, Ciel::Chladny, &z);
        assert_eq!((r.obraz, r.periferie, r.rusi), (Ciel::Povodny, false, false));
        // po pokuse bežná politika
        let r = rozhodni(Some(FAZA_S * 9), ZAKLADNE_FAZY, 700, Ciel::Chladny, &z);
        assert_eq!((r.obraz, r.stav, r.faza), (Ciel::Chladny, StavObrazu::Vypnuty, None));
        let r = rozhodni(None, ZAKLADNE_FAZY, 0, Ciel::Chladny, &z);
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
    fn vysledok_pokusu_zaklad_je_kontrolna_faza() {
        // ustálené teploty fáz; každá fáza sa blíži k cieľu s τ = 6 min (fáza 15 min)
        let ciele = [60.0, 58.0, 54.0, 52.0, 50.0, 48.0, 46.0, 45.5, 62.0];
        let mut krivka = Vec::new();
        let mut t0 = 35.0;
        let n = FAZA_S / 30;
        for (i, &ciel) in ciele.iter().enumerate() {
            for k in 0..=n {
                if k == n && i + 1 < ciele.len() {
                    continue;
                }
                krivka.push((
                    i as u64 * FAZA_S + k * 30,
                    ciel + (t0 - ciel) * (-(k as f64 * 30.0) / 360.0).exp(),
                ));
            }
            t0 = ciel + (t0 - ciel) * (-(FAZA_S as f64) / 360.0).exp();
        }
        let r = vysledok_pokusu(
            &krivka,
            &[false, false, false, false, true, false, false, false, false],
            ZAKLADNE_FAZY,
            "\n",
        );
        let riadok = |z: &str| r.lines().find(|l| l.contains(z)).unwrap_or_default().to_string();
        // fáza 3: 54 − 62 = −8 °C oproti základu, krok 54 − 58 = −4 °C
        let f3 = riadok("3 chladný obraz");
        assert!(f3.contains("-4,0 °C") && f3.contains("-8,0 °C"), "{r}");
        assert!(riadok("5 čierny").contains("rušené"), "{r}");
        assert!(riadok("8 + odpojené").contains("-0,5 °C"), "{r}");
        assert!(riadok("9 pôvodne").contains("základ"), "{r}");
        assert!(r.contains("fáza 1 vyšla o"), "{r}");
    }

    // mená v /sys obsahujú dvojbodky (1-2:1.0) – vo Windows nemožné, skúška len na unixových systémoch
    #[cfg(unix)]
    #[test]
    fn siet_spi_ked_ju_nic_nepotrebuje() {
        let mut k = Kabel::novy();
        // kábel zapojený, sieť nepotrebná: po minúte spí
        assert_eq!(k.krok_s_potrebou(30.0, true, false), None);
        assert_eq!(k.krok_s_potrebou(30.0, true, false), Some(PrechodKabla::Uspi));
        // overenie kábla raz za 5 min, potom znova spí
        assert_eq!(k.krok_s_potrebou(299.0, false, false), None);
        assert_eq!(k.krok_s_potrebou(1.0, false, false), Some(PrechodKabla::Hladaj));
        assert_eq!(k.krok_s_potrebou(3.0, true, false), Some(PrechodKabla::Overeny));
        assert!(k.riadeny());
        // bez kábla: hľadanie skončí po 6 s
        assert_eq!(k.krok_s_potrebou(300.0, false, false), Some(PrechodKabla::Hladaj));
        assert_eq!(k.krok_s_potrebou(6.0, false, false), Some(PrechodKabla::Spat));
        // keď sieť zas niečo potrebuje, nájdený kábel ostane zapnutý
        assert_eq!(k.krok_s_potrebou(30.0, false, true), Some(PrechodKabla::Hladaj));
        assert_eq!(k.krok_s_potrebou(2.0, true, true), Some(PrechodKabla::Najdeny));
        assert!(!k.riadeny());
    }

    #[test]
    fn usb_strom_spi() {
        let k = std::env::temp_dir().join(format!("sklenik-usbstrom-{}", std::process::id()));
        let _ = fs::remove_dir_all(&k);
        let z = k.join("sys/bus/usb/devices");
        let zar = |meno: &str, trieda: &str| {
            fs::create_dir_all(z.join(meno).join("power")).unwrap();
            fs::write(z.join(meno).join("power/control"), "on").unwrap();
            fs::write(z.join(meno).join("bDeviceClass"), trieda).unwrap();
            fs::write(z.join(meno).join("power/runtime_suspended_time"), "3000").unwrap();
            fs::write(z.join(meno).join("power/runtime_active_time"), "1000").unwrap();
        };
        let rozhranie = |meno: &str, trieda: &str| {
            fs::create_dir_all(z.join(meno)).unwrap();
            fs::write(z.join(meno).join("bInterfaceClass"), trieda).unwrap();
        };
        zar("usb1", "09");
        zar("1-1", "09");
        zar("1-1.2", "00");
        rozhranie("1-1.2:1.0", "03");
        zar("1-1.3", "00");
        rozhranie("1-1.3:1.0", "08");
        assert_eq!(nastav_spanok_usb_stromu(&k, true), 3);
        assert_eq!(fs::read_to_string(z.join("1-1.2/power/autosuspend_delay_ms")).unwrap(), "4000");
        assert_eq!(fs::read_to_string(z.join("1-1/power/autosuspend_delay_ms")).unwrap(), "2000");
        assert_eq!(fs::read_to_string(z.join("1-1.2/power/control")).unwrap(), "auto");
        // úložisko riadi R159
        assert_eq!(fs::read_to_string(z.join("1-1.3/power/control")).unwrap(), "on");
        // bez koreňového rozbočovača a úložiska: 1-1 a 1-1.2
        assert_eq!(cas_spanku_usb_stromu(&k), Some((6000, 2000)));
        nastav_spanok_usb_stromu(&k, false);
        assert_eq!(fs::read_to_string(z.join("1-1.2/power/control")).unwrap(), "on");
        assert_eq!(fs::read_to_string(z.join("1-1/power/control")).unwrap(), "auto");
        let _ = fs::remove_dir_all(&k);
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
