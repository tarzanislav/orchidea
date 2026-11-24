//! Tepelný strážca SKLENÍKA – zariadenie s pasívnym chladením (alebo bez chladiča) sa nesmie prehriať.
//!
//! Strážca drží teplotu pri **cieli** (napr. 60 °C na Raspberry Pi 3) a **strop** (70 °C) nikdy
//! neprekročí: podľa teploty a rýchlosti jej rastu volí **úroveň výkonu** – kombináciu najvyššieho taktu
//! a počtu pracujúcich jadier. Úrovne sú zoradené podľa odhadovaného príkonu a ponechané sú len tie, ktoré
//! za daný príkon dajú najviac práce. Znižuje rýchlo (bezpečnosť), zvyšuje pomaly (bez kmitania).
//!
//! Regulátor je čistá logika bez hardvéru – overuje sa na fyzikálnom modeli ohrevu (skúšky nižšie).

/// Úroveň výkonu: koľko jadier smie pracovať a najvyšší takt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uroven {
    pub jadra: u32,
    pub takt_khz: u32,
    /// Odhad príkonu procesora pri plnej záťaži (pomerne, najvyššia úroveň = 1).
    pub prikon: f64,
    /// Odhad množstva práce za sekundu (pomerne, najvyššia úroveň = 1).
    pub vykon: f64,
}

/// Teplotný profil zariadenia (°C).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profil {
    /// Teplota, pri ktorej sa má zariadenie dlhodobo držať aj pri plnej práci.
    pub ciel: f64,
    /// Teplota, ktorá sa nesmie prekročiť nikdy.
    pub strop: f64,
}

impl Profil {
    /// Profil podľa modelu (text z `/proc/device-tree/model`). Schválené R81: RPi 3 – strop 70 °C,
    /// RPi 4 a 5 – 75 °C; cieľ je o 10 °C nižšie (vlastník: minimalizovať teplotu).
    /// Teplotný profil podľa modelu. Odroid C4 (Amlogic S905X3, 12 nm) má rovnaký opatrný profil ako
    /// RPi 3 – na doske bez chladiča je to bezpečná voľba, kým sa nezmeria na skutočnom zariadení.
    /// TV krabičky (X96 Max Plus, Tanix TX68 – R117, R118) sú v uzavretej plastovej krabičke bez vetrania
    /// a známe prehrievaním (vlastník: „zvlášť X96“) – o 5 °C prísnejší profil.
    pub fn pre_model(model: &str) -> Profil {
        if je_tv_krabicka(model) {
            Profil { ciel: 55.0, strop: 65.0 }
        } else if model.contains("Raspberry Pi 4")
            || model.contains("Raspberry Pi 5")
            || model.contains("Raspberry Pi 400")
        {
            Profil { ciel: 65.0, strop: 75.0 }
        } else {
            Profil { ciel: 60.0, strop: 70.0 }
        }
    }
}

/// TV krabička (podľa mena modelu v popise hardvéru, ktoré dáva SKLENÍK na karte).
pub fn je_tv_krabicka(model: &str) -> bool {
    ["X96", "Tanix", "TANIX", "TX68"].iter().any(|m| model.contains(m))
}

/// R188, R190: dosky s chladnými ovládačmi KLÍČA (R158–R191: vlastný obraz cez KMS, USB spí, sieť v troch
/// stupňoch, takt VPU, CEC, strážca HDMI, odpojené radiče, spánok jadier, tepelný pokus) – TV krabičky,
/// Odroid C4 (ten istý čip Amlogic S905X3 ako X96 Max Plus) a Raspberry Pi (obraz cez KMS – jadro vc4).
/// Časti, ktoré doska nemá (takt VPU, spánok jadier), sa vynechajú samy.
pub fn ma_chladnu_sadu(model: &str) -> bool {
    je_tv_krabicka(model) || model.contains("ODROID-C4") || model.contains("Raspberry Pi")
}

/// R194: smú sa jadrá vypínať (CPU hotplug)? RPi 5: firmvér jadro po vypnutí už nespustí („CPU3: failed to
/// come online“, potom „psci: failed to boot CPU3 (-22)“ – 2. kolo meraní 24. 11.). RPi 1–3 a Zero 2 W
/// štartujú jadrá cez „spin-table“ – vypnúť sa nedajú vôbec. Inde (RPi 4, Amlogic, Allwinner) áno.
pub fn smie_parkovat(model: &str) -> bool {
    !model.contains("Raspberry Pi 5")
        && !model.contains("Raspberry Pi 500")
        && !model.contains("Compute Module 5")
}

/// Úrovne výkonu z dostupných taktov a počtu jadier. Napätie rastie s taktom, takže príkon jadra sa
/// odhaduje ako f · V², V ≈ 0,8 + 0,2 · f/fmax. Zostanú len úrovne, ktoré pri vyššom príkone dajú aj viac
/// práce (Paretova hranica).
pub fn urovne(takty_khz: &[u32], jadier: u32) -> Vec<Uroven> {
    let mut takty: Vec<u32> = takty_khz.iter().copied().filter(|t| *t > 0).collect();
    takty.sort_unstable();
    takty.dedup();
    let jadier = jadier.max(1);
    let fmax = *takty.last().unwrap_or(&1) as f64;
    let mut vsetky: Vec<Uroven> = Vec::new();
    for &t in &takty {
        let f = t as f64 / fmax;
        let v = 0.8 + 0.2 * f;
        for j in 1..=jadier {
            vsetky.push(Uroven {
                jadra: j,
                takt_khz: t,
                prikon: j as f64 * f * v * v / jadier as f64,
                vykon: j as f64 * f / jadier as f64,
            });
        }
    }
    if vsetky.is_empty() {
        vsetky = (1..=jadier)
            .map(|j| Uroven {
                jadra: j,
                takt_khz: 0,
                prikon: j as f64 / jadier as f64,
                vykon: j as f64 / jadier as f64,
            })
            .collect();
    }
    vsetky.sort_by(|a, b| a.prikon.total_cmp(&b.prikon).then(b.vykon.total_cmp(&a.vykon)));
    let mut hranica: Vec<Uroven> = Vec::new();
    for u in vsetky {
        if hranica.last().is_none_or(|p| u.vykon > p.vykon + 1e-9) {
            hranica.push(u);
        }
    }
    hranica
}

/// Regulátor teploty.
#[derive(Debug, Clone)]
pub struct Regulator {
    urovne: Vec<Uroven>,
    profil: Profil,
    index: usize,
    posledna: Option<f64>,
    /// Vyhladená rýchlosť zmeny teploty (°C/s).
    sklon: f64,
    od_zmeny: f64,
    /// Predpoveď z modelu okolia (R125): kam sa teplota pri terajšej záťaži ustáli.
    ustalena: Option<f64>,
    /// Úspora v nečinnosti pri horúcom okolí (R126): v nečinnosti najviac úsporná úroveň.
    uspora: bool,
    /// Parkovanie jadier v pokoji (R161, TV krabičky).
    parkovanie: bool,
    necinny_s: f64,
    /// Úroveň pred zaparkovaním – keď príde práca, vráti sa na ňu hneď.
    pred_parkovanim: Option<usize>,
}

/// Po koľkých sekundách nečinnosti sa jadrá zaparkujú.
const PARKOVANIE_PO_S: f64 = 20.0;

/// Ako ďaleko dopredu (s) regulátor predpovedá teplotu.
const HORIZONT_S: f64 = 30.0;
/// Rezerva pod stropom, pri ktorej sa okamžite prejde na najnižšiu úroveň.
const NUDZA_POD_STROPOM: f64 = 3.0;
/// Vyťaženie, pod ktorým je procesor nečinný (strážca KLÍČ a obrazovka spotrebujú okolo 1 %).
const NECINNOST: f64 = 0.15;

impl Regulator {
    /// Začína opatrne – na najvyššej úrovni s príkonom najviac polovičným; teplotu si najprv overí.
    pub fn novy(urovne: Vec<Uroven>, profil: Profil) -> Regulator {
        assert!(!urovne.is_empty(), "aspoň jedna úroveň výkonu");
        let index = urovne.iter().rposition(|u| u.prikon <= 0.5).unwrap_or(0);
        Regulator {
            urovne,
            profil,
            index,
            posledna: None,
            sklon: 0.0,
            od_zmeny: 0.0,
            ustalena: None,
            uspora: false,
            parkovanie: false,
            necinny_s: 0.0,
            pred_parkovanim: None,
        }
    }

    /// R161: parkovanie jadier v pokoji (TV krabičky). Nečinné, ale zapnuté jadrá stále žerú prúd únikom
    /// (rastie s teplotou exponenciálne); vypnuté jadro odpojí firmvér (PSCI CPU_OFF) od napájania. Po 20 s
    /// nečinnosti ostane jedno jadro na najnižšom takte; keď príde práca, strážca sa **hneď** vráti na
    /// predošlú úroveň (nie po krokoch) – odozva ostáva, teplo v pokoji klesne.
    pub fn nastav_parkovanie(&mut self, parkovanie: bool) {
        self.parkovanie = parkovanie;
        self.necinny_s = 0.0;
        if !parkovanie && let Some(p) = self.pred_parkovanim.take() {
            self.nastav(p.min(self.urovne.len() - 1));
        }
    }

    pub fn zaparkovane(&self) -> bool {
        self.pred_parkovanim.is_some()
    }

    /// R126: pri horúcom okolí sa aj nečinnosť šetrí – každý watt navyše zostáva v doske (poučenie z vrtov
    /// a Venuše: keď je okolie horúce, jediná páka je vlastný ohrev). V nečinnosti strážca klesne na úspornú
    /// úroveň (najviac polovičný príkon); keď príde práca, zvyšuje ako obvykle.
    pub fn nastav_usporu(&mut self, uspora: bool) {
        self.uspora = uspora;
    }

    /// Najvyššia úsporná úroveň (príkon najviac polovičný).
    fn usporna(&self) -> usize {
        self.urovne.iter().rposition(|u| u.prikon <= 0.5).unwrap_or(0)
    }

    /// Predpoveď ustálenej teploty z modelu okolia (R125, `okolie.rs`): teplota + sklon · τ z minútových
    /// priemerov. Strážca podľa nej zníži výkon skôr, než teplota cieľ prekročí, a nezvýši ho, ak by sa
    /// teplota ustálila nad cieľom (poučenie z automobilov a prediktívneho riadenia MPC).
    pub fn nastav_predpoved(&mut self, ustalena: Option<f64>) {
        self.ustalena = ustalena;
    }

    pub fn profil(&self) -> Profil {
        self.profil
    }

    pub fn uroven(&self) -> Uroven {
        self.urovne[self.index]
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn pocet_urovni(&self) -> usize {
        self.urovne.len()
    }

    /// Teplota o 30 s pri terajšom raste (pre tepelný rozpočet programov – R127).
    pub fn predpoved_30s(&self) -> Option<f64> {
        self.posledna.map(|t| t + self.sklon.max(0.0) * HORIZONT_S)
    }

    /// Jeden krok regulácie: aktuálna teplota a čas od posledného kroku. Vráti zvolenú úroveň.
    /// Vyťaženie procesora nie je známe – strážca sa správa opatrne, akoby procesor pracoval.
    pub fn krok(&mut self, teplota: f64, dt_s: f64) -> Uroven {
        self.krok_s_vytazenim(teplota, dt_s, None)
    }

    /// Krok regulácie s vyťažením procesora (R110): podiel času, keď povolené jadrá pracovali (0–1).
    ///
    /// Zníženie výkonu chladí len vtedy, keď procesor pracuje. V nečinnosti (napr. RPi 3 v plastovej
    /// krabičke, ktorá sa sama od seba ustáli okolo 60 °C) by znižovanie nič neochladilo, len by zobralo výkon
    /// pre prácu, ktorá príde – strážca preto v nečinnosti výkon drží a postupne vracia. Pri strope a pri
    /// rýchlom raste teploty znižuje vždy (bezpečnosť má prednosť).
    pub fn krok_s_vytazenim(&mut self, teplota: f64, dt_s: f64, vytazenie: Option<f64>) -> Uroven {
        let dt = dt_s.max(1e-3);
        if let Some(p) = self.posledna {
            self.sklon = 0.7 * self.sklon + 0.3 * (teplota - p) / dt;
        }
        self.posledna = Some(teplota);
        self.od_zmeny += dt;
        let Profil { ciel, strop } = self.profil;
        let predikcia = teplota + self.sklon.max(0.0) * HORIZONT_S;
        let necinny = vytazenie.is_some_and(|v| v < NECINNOST);
        // R125: pri práci sa teplota ustáli nad cieľom – znížiť už teraz (pomalšie, raz za 20 s)
        let ustali_sa_nad = !necinny && self.ustalena.is_some_and(|u| u > ciel + 2.0) && teplota > ciel - 8.0;
        let ustali_sa_pod = self.ustalena.is_none_or(|u| u < ciel - 1.0);
        if teplota >= strop - NUDZA_POD_STROPOM {
            self.pred_parkovanim = None;
            self.nastav(0);
            return self.uroven();
        }
        if self.parkovanie
            && let Some(v) = vytazenie
        {
            if v < NECINNOST {
                self.necinny_s += dt;
                if self.necinny_s >= PARKOVANIE_PO_S && self.pred_parkovanim.is_none() && self.index > 0 {
                    self.pred_parkovanim = Some(self.index);
                    self.nastav(0);
                }
                if self.pred_parkovanim.is_some() {
                    return self.uroven();
                }
            } else {
                self.necinny_s = 0.0;
                if let Some(p) = self.pred_parkovanim.take() {
                    // práca prišla – späť hneď, ak to teplota dovolí (inak bežné zvyšovanie po krokoch)
                    if predikcia < ciel {
                        self.nastav(p.min(self.urovne.len() - 1));
                    }
                    return self.uroven();
                }
            }
        }
        if self.uspora && necinny && self.index > self.usporna() && self.od_zmeny >= 5.0 {
            self.nastav(self.index - 1);
        } else if ((predikcia > strop - NUDZA_POD_STROPOM || (teplota > ciel && !necinny))
            && self.od_zmeny >= 2.0)
            || (ustali_sa_nad && self.od_zmeny >= 20.0)
        {
            self.nastav(self.index.saturating_sub(1));
        } else if self.od_zmeny >= 5.0
            && ((teplota < ciel - 4.0 && predikcia < ciel && (necinny || ustali_sa_pod))
                || (necinny && teplota <= ciel + 2.0 && self.sklon <= 0.005))
            && !(self.uspora && necinny && self.index >= self.usporna())
        {
            self.nastav((self.index + 1).min(self.urovne.len() - 1));
        }
        self.uroven()
    }

    fn nastav(&mut self, index: usize) {
        if index != self.index {
            self.index = index;
            self.od_zmeny = 0.0;
        }
    }
}

/// Krivka ventilátora (ak zariadenie nejaký má, napr. skrinka Pironman 5 alebo oficiálny chladič RPi 5).
/// Šetrí energiu a predlžuje životnosť: pod `ciel − 5 °C` je ventilátor vypnutý, potom sa po krokoch
/// zrýchľuje až po plný výkon 5 °C pod stropom. Vypína sa až o 4 °C nižšie, než sa zapol (bez cvakania).
/// Vráti stupeň 0..=`najvyssi`.
pub fn stupen_ventilatora(teplota: f64, profil: Profil, najvyssi: u32, terajsi: u32) -> u32 {
    if najvyssi == 0 {
        return 0;
    }
    let zaciatok = profil.ciel - 5.0;
    let plny = profil.strop - 5.0;
    let ziadany = |t: f64| -> u32 {
        if t < zaciatok {
            0
        } else if t >= plny {
            najvyssi
        } else {
            1 + ((t - zaciatok) / (plny - zaciatok) * (najvyssi - 1) as f64).floor() as u32
        }
    };
    let novy = ziadany(teplota);
    if novy >= terajsi { novy } else { ziadany(teplota + 4.0).min(terajsi) }
}

/// Rozhranie hardvéru, ktorý strážca riadi.
pub trait Hardver {
    /// Najvyššia teplota zo všetkých snímačov (°C), ak nejaký je.
    fn teplota(&mut self) -> Option<f64>;
    /// Dostupné takty (kHz); prázdne, ak sa takt nedá riadiť.
    fn takty(&self) -> Vec<u32>;
    fn jadier(&self) -> u32;
    /// Uplatní úroveň (najvyšší takt, počet jadier). Čo sa nedá, ticho preskočí.
    fn uplatni(&mut self, uroven: &Uroven);
}

#[cfg(test)]
mod skusky {
    use super::*;

    /// Fyzikálny model dosky: doska (veľká tepelná kapacita) a čip nad ňou (snímač je v čipe).
    /// Doska: dT/dt = (P − (T − T_okolia)/R) / C. Čip je o príkon procesora × R_čipu teplejší než doska
    /// a nasleduje ho s oneskorením niekoľkých sekúnd – pri náraze práce teplota v čipe vyskočí hneď.
    struct Doska {
        t: f64,
        cip: f64,
        okolie: f64,
        r: f64,
        c: f64,
        r_cipu: f64,
        nezatazena_w: f64,
        procesor_max_w: f64,
    }

    impl Doska {
        /// RPi 3 B+ s chladičom alebo voľne na stole: v nečinnosti okolo 45 °C pri 25 °C v miestnosti,
        /// pri plnej práci bez obmedzenia by vystúpila na ~80 °C (tam firmvér začína škrtiť). Pôvodná
        /// kalibrácia – čip bez vlastného ohrevu nad doskou.
        fn rpi3(okolie: f64) -> Doska {
            Doska {
                t: okolie,
                cip: okolie,
                okolie,
                r: 18.0,
                c: 8.0,
                r_cipu: 0.0,
                nezatazena_w: 1.2,
                procesor_max_w: 1.8,
            }
        }

        /// RPi 3 B+ v plastovej krabičke bez chladiča – zmerané 21. 11. 2026 (R102, SKLENÍK 0dd8e61):
        /// v nečinnosti 40,8 °C po 2:16, 47,2 °C po 6:01, 55,3 °C po 16:15 a 59,6 °C po 33:03 min.
        /// Preložená krivka: ustálená teplota 60,5 °C, časová konštanta 610 s; izba okolo 22 °C.
        fn rpi3_plast(okolie: f64) -> Doska {
            let r = 20.0;
            Doska {
                t: 36.1,
                cip: 36.1,
                okolie,
                r,
                c: 610.0 / r,
                r_cipu: 3.0,
                nezatazena_w: (60.5 - 22.0) / r,
                procesor_max_w: 1.8,
            }
        }

        /// Krok simulácie; `zataz` 0..1 = podiel času, keď majú povolené jadrá prácu.
        fn krok(&mut self, u: &Uroven, zataz: f64, dt: f64) {
            let procesor = self.procesor_max_w * u.prikon * zataz;
            let p = self.nezatazena_w + procesor;
            self.t += (p - (self.t - self.okolie) / self.r) / self.c * dt;
            // čip dobieha dosku s časovou konštantou 3 s
            self.cip += (self.t + procesor * self.r_cipu - self.cip) * (dt / 3.0).min(1.0);
        }
    }

    fn simuluj_dosku(mut d: Doska, zataz: impl Fn(f64) -> f64, sekund: u32) -> (f64, f64, Regulator, u32) {
        let mut r = Regulator::novy(
            urovne(&[600_000, 1_400_000], 4),
            Profil::pre_model("Raspberry Pi 3 Model B Plus"),
        );
        let (mut maximum, mut zmeny, mut posledny) = (d.cip, 0u32, r.index());
        let mut prace = 0.0;
        for s in 0..sekund {
            let z = zataz(s as f64);
            let u = r.krok_s_vytazenim(d.cip, 1.0, Some(z));
            if r.index() != posledny && s > sekund / 2 {
                zmeny += 1;
            }
            posledny = r.index();
            prace += u.vykon * z;
            d.krok(&u, z, 1.0);
            maximum = maximum.max(d.cip);
        }
        (maximum, prace / sekund as f64, r, zmeny)
    }

    fn simuluj(okolie: f64, zataz: impl Fn(f64) -> f64, sekund: u32) -> (f64, f64, Regulator, u32) {
        simuluj_dosku(Doska::rpi3(okolie), zataz, sekund)
    }

    #[test]
    fn urovne_su_paretova_hranica() {
        let u = urovne(&[600_000, 1_400_000], 4);
        assert!(u.windows(2).all(|w| w[0].prikon < w[1].prikon && w[0].vykon < w[1].vykon), "{u:?}");
        assert_eq!(u.last().map(|x| (x.jadra, x.takt_khz)), Some((4, 1_400_000)));
        assert_eq!(u[0].jadra, 1);
        assert!(urovne(&[], 4).len() == 4, "bez riadenia taktu zostanú len jadrá");
    }

    #[test]
    fn plna_praca_pri_25_stupnoch_drzi_ciel_a_nikdy_neprekroci_strop() {
        let (maximum, vykon, r, zmeny) = simuluj(25.0, |_| 1.0, 3 * 3600);
        eprintln!("plná práca, 25 °C: maximum {maximum:.1} °C, výkon {vykon:.2}, zmeny {zmeny}");
        assert!(maximum < r.profil().strop, "maximum {maximum:.1} °C");
        assert!(maximum < r.profil().ciel + 3.0, "teplota má zostať pri cieli, maximum {maximum:.1} °C");
        assert!(vykon > 0.35, "pri cieli 60 °C má zostať rozumný výkon, je {vykon:.2}");
        assert!(zmeny <= 120, "regulátor kmitá: {zmeny} zmien za 1,5 h");
    }

    #[test]
    fn horuca_miestnost_40_stupnov_strop_neprekroci() {
        let (maximum, vykon, r, _) = simuluj(40.0, |_| 1.0, 3 * 3600);
        eprintln!("plná práca, 40 °C: maximum {maximum:.1} °C, výkon {vykon:.2}");
        assert!(maximum < r.profil().strop, "maximum {maximum:.1} °C");
    }

    #[test]
    fn narazova_praca_vyuzije_plny_vykon_kym_je_chladno() {
        // 20 s práca, 40 s ticho – typická interaktívna práca (úprava, náhľad, export malého súboru)
        let (maximum, vykon, r, _) = simuluj(25.0, |s| if s % 60.0 < 20.0 { 1.0 } else { 0.0 }, 3600);
        eprintln!("nárazová práca, 25 °C: maximum {maximum:.1} °C, výkon {vykon:.2} z 0,33");
        assert!(maximum < r.profil().ciel + 1.0, "maximum {maximum:.1} °C");
        assert!(vykon > 0.25, "nárazová práca má dostať takmer plný výkon, priemer {vykon:.2} z 0,33");
    }

    /// Strážca s modelom okolia (R123, R125): 40 min nečinnosti po studenom štarte (model sa naučí τ
    /// a vlastný ohrev), potom práca. Vráti (maximum, priemerný výkon a priemerná teplota pri práci, stav
    /// okolia).
    fn s_okolim(
        doska: fn(f64) -> Doska,
        okolie: impl Fn(f64) -> f64,
        zataz: impl Fn(f64) -> f64,
        sekund: u32,
        predpoved: bool,
    ) -> (f64, f64, f64, orchidea_teplo::okolie::StavOkolia) {
        let mut d = doska(okolie(0.0));
        (d.t, d.cip) = (okolie(0.0), okolie(0.0));
        let mut r = Regulator::novy(
            urovne(&[600_000, 1_400_000], 4),
            Profil::pre_model("Raspberry Pi 3 Model B Plus"),
        );
        let mut o = orchidea_teplo::okolie::Okolie::nove();
        let (mut maximum, mut prace, mut pracovnych, mut sucet_t) = (0.0f64, 0.0, 0u32, 0.0);
        for s in 0..sekund {
            let sf = s as f64;
            let z = if sf < 2400.0 { 0.01 } else { zataz(sf) };
            // snímač s rozlíšením 0,5 °C
            let t = (d.cip * 2.0).round() / 2.0;
            o.krok(sf + 2.0, t, Some(z));
            if predpoved {
                r.nastav_predpoved(o.stav().ustalena);
            }
            let u = r.krok_s_vytazenim(t, 1.0, Some(z));
            d.okolie = okolie(sf);
            d.krok(&u, z, 1.0);
            if sf >= 2400.0 {
                maximum = maximum.max(d.cip);
                prace += u.vykon * z;
                pracovnych += 1;
                sucet_t += d.cip;
            }
        }
        let n = pracovnych.max(1) as f64;
        (maximum, prace / n, sucet_t / n, o.stav())
    }

    #[test]
    fn predpoved_z_modelu_okolia_znizi_prekmit_a_drzi_vykon() {
        // R125: po 40 min nečinnosti 2 h plnej práce pri 22 °C – doska s chladičom aj plastová krabička
        // (v plaste je už nečinnosť nad cieľom – fyzika, nie softvér; predpoveď tam nesmie nič pokaziť)
        for (meno, doska) in [("chladič", Doska::rpi3 as fn(f64) -> Doska), ("plast", Doska::rpi3_plast)] {
            let (max_bez, vykon_bez, t_bez, _) = s_okolim(doska, |_| 22.0, |_| 1.0, 2400 + 7200, false);
            let (max_s, vykon_s, t_s, stav) = s_okolim(doska, |_| 22.0, |_| 1.0, 2400 + 7200, true);
            eprintln!(
                "{meno}: bez predpovede max {max_bez:.1} °C, priemer {t_bez:.1} °C, výkon {vykon_bez:.2}; s predpoveďou max {max_s:.1} °C, priemer {t_s:.1} °C, výkon {vykon_s:.2}; τ {:?}, vlastný ohrev {:?}",
                stav.tau_s, stav.samohrev
            );
            // chlad je dominanta: s predpoveďou je doska pri práci chladnejšia (ak je čo chladiť)
            assert!(t_s <= t_bez + 0.05, "{meno}: priemer {t_s:.1} °C oproti {t_bez:.1} °C");
            if meno == "chladič" {
                assert!(t_s < t_bez - 1.0, "{meno}: predpoveď má dosku pri práci ochladiť");
            }
            let p = Profil::pre_model("Raspberry Pi 3 Model B Plus");
            assert!(stav.tau_s.is_some(), "{meno}: model sa naučil τ: {stav:?}");
            assert!(max_s <= max_bez + 0.05, "{meno}: predpoveď nesmie zvýšiť maximum");
            assert!(max_s < p.strop, "{meno}: maximum {max_s:.1} °C");
            assert!(vykon_s > 0.8 * vykon_bez, "{meno}: výkon {vykon_s:.2} oproti {vykon_bez:.2}");
        }
    }

    #[test]
    fn radiator_pri_praci_strop_neprekroci() {
        // pri práci sa vedľa zapne radiátor: okolie 22 → 40 °C za 20 minút
        let okolie = |s: f64| 22.0 + ((s - 3000.0) / 1200.0).clamp(0.0, 1.0) * 18.0;
        let (maximum, vykon, _, stav) = s_okolim(Doska::rpi3, okolie, |_| 1.0, 2400 + 3 * 3600, true);
        let p = Profil::pre_model("Raspberry Pi 3 Model B Plus");
        eprintln!("radiátor pri práci: maximum {maximum:.1} °C, výkon {vykon:.2}, {stav:?}");
        // pri 40 °C je už nečinnosť nad cieľom (40 + 22 °C vlastného ohrevu) – strážca drží strop s rezervou
        assert!(maximum < p.strop - NUDZA_POD_STROPOM, "maximum {maximum:.1} °C");
        assert!(vykon > 0.1, "výkon {vykon:.2}");
    }

    #[test]
    fn uspora_v_necinnosti_pri_horucom_okoli() {
        // R126: v nečinnosti úsporná úroveň, pri práci znova vyššia
        let mut r = Regulator::novy(
            urovne(&[600_000, 1_400_000], 4),
            Profil::pre_model("Raspberry Pi 3 Model B Plus"),
        );
        r.nastav_usporu(true);
        for _ in 0..120 {
            r.krok_s_vytazenim(45.0, 1.0, Some(0.02));
        }
        assert!(r.uroven().prikon <= 0.5, "{:?}", r.uroven());
        for _ in 0..120 {
            r.krok_s_vytazenim(45.0, 1.0, Some(0.9));
        }
        assert!(r.uroven().prikon > 0.5, "pri práci a chlade sa výkon vráti: {:?}", r.uroven());
        r.nastav_usporu(false);
        for _ in 0..120 {
            r.krok_s_vytazenim(45.0, 1.0, Some(0.02));
        }
        assert_eq!(r.index(), r.pocet_urovni() - 1, "bez úspory nečinnosť výkon neberie (R110)");
    }

    #[test]
    fn necinnost_je_chladna() {
        let (maximum, _, _, _) = simuluj(25.0, |_| 0.0, 3600);
        eprintln!("nečinnosť, 25 °C: maximum {maximum:.1} °C");
        assert!(maximum < 48.0, "v nečinnosti {maximum:.1} °C");
    }

    #[test]
    fn ventilator_setri_energiu_a_necvaka() {
        let p = Profil::pre_model("Raspberry Pi 5 Model B");
        assert_eq!(stupen_ventilatora(50.0, p, 4, 0), 0, "v pokoji vypnutý");
        assert_eq!(stupen_ventilatora(61.0, p, 4, 0), 1);
        assert_eq!(stupen_ventilatora(70.0, p, 4, 1), 4, "5 °C pod stropom naplno");
        // zapol sa pri 61 °C, pri 59 °C ešte beží, pod 56 °C sa vypne
        assert_eq!(stupen_ventilatora(59.0, p, 4, 1), 1);
        assert_eq!(stupen_ventilatora(55.5, p, 4, 1), 0);
        assert_eq!(stupen_ventilatora(80.0, p, 0, 0), 0, "bez ventilátora nič");
    }

    #[test]
    fn model_plastovej_krabicky_sedi_s_meranim() {
        // R102: model musí zopakovať skutočné meranie RPi 3 B+ v nečinnosti (±1,5 °C)
        let mut d = Doska::rpi3_plast(22.0);
        let u = urovne(&[600_000, 1_400_000], 4)[0];
        let mut namerane = vec![(136, 40.8), (361, 47.2), (975, 55.3), (1983, 59.6)].into_iter().peekable();
        for s in 0..=1983 {
            if let Some(&(cas, t)) = namerane.peek()
                && cas == s
            {
                assert!((d.cip - t).abs() < 1.5, "po {s} s model {:.1} °C, namerané {t} °C", d.cip);
                namerane.next();
            }
            d.krok(&u, 0.0, 1.0);
        }
        assert!(namerane.next().is_none());
    }

    #[test]
    fn necinnost_v_plaste_neberie_vykon() {
        // R110: RPi 3 v plaste sa sám od seba ustáli nad cieľom 60 °C – znižovanie by nič neochladilo,
        // preto má strážca v nečinnosti nechať plný výkon pre prácu, ktorá príde
        let (maximum, _, r, _) = simuluj_dosku(Doska::rpi3_plast(22.0), |_| 0.01, 2 * 3600);
        eprintln!(
            "nečinnosť v plaste: maximum {maximum:.1} °C, úroveň {}/{}",
            r.index() + 1,
            r.pocet_urovni()
        );
        assert_eq!(r.index(), r.pocet_urovni() - 1, "v nečinnosti plný výkon");
        assert!(maximum < r.profil().strop);
    }

    #[test]
    fn plna_praca_v_plaste_pri_22_stupnoch_nikdy_neprekroci_strop() {
        let (maximum, vykon, r, zmeny) = simuluj_dosku(Doska::rpi3_plast(22.0), |_| 1.0, 3 * 3600);
        eprintln!("plná práca v plaste, 22 °C: maximum {maximum:.1} °C, výkon {vykon:.2}, zmeny {zmeny}");
        assert!(maximum < r.profil().strop, "maximum {maximum:.1} °C");
        assert!(zmeny <= 200, "regulátor kmitá: {zmeny} zmien");
    }

    #[test]
    fn plast_v_teplej_miestnosti_je_za_hranicou_softveru() {
        // Pri 30 °C v miestnosti má RPi 3 v plaste už v nečinnosti ~68 °C (zmerané 38,5 °C nad izbou) –
        // strážca dá najnižší výkon, viac softvér nezmôže; treba chladič alebo kovovú/vetranú krabičku.
        // Poistka firmvéru (temp_limit=75) zostáva nad tým.
        let (maximum, vykon, r, _) = simuluj_dosku(Doska::rpi3_plast(30.0), |_| 1.0, 3 * 3600);
        eprintln!("plná práca v plaste, 30 °C: maximum {maximum:.1} °C, výkon {vykon:.2}");
        assert_eq!(r.index(), 0, "najnižší výkon");
        assert!(maximum < r.profil().strop + 3.0, "maximum {maximum:.1} °C");
    }

    #[test]
    fn narazova_praca_v_plaste_dostane_vykon_a_strop_neprekroci() {
        // 20 s práca, 40 s ticho; po skončení práce sa výkon vráti (R110)
        let (maximum, vykon, r, _) =
            simuluj_dosku(Doska::rpi3_plast(22.0), |s| if s % 60.0 < 20.0 { 1.0 } else { 0.01 }, 2 * 3600);
        eprintln!("nárazová práca v plaste: maximum {maximum:.1} °C, výkon {vykon:.2} z 0,33");
        assert!(maximum < r.profil().strop - NUDZA_POD_STROPOM, "maximum {maximum:.1} °C");
        assert!(vykon > 0.05, "priemer {vykon:.2}");
    }

    #[test]
    fn bez_vytazenia_sa_strazca_sprava_opatrne() {
        // neznáme vyťaženie = ako pri práci: nad cieľom znižuje
        let mut r = Regulator::novy(urovne(&[600_000, 1_400_000], 4), Profil::pre_model("Raspberry Pi 3"));
        let zaciatok = r.index();
        for _ in 0..10 {
            r.krok(62.0, 1.0);
        }
        assert!(r.index() < zaciatok);
        let mut n = Regulator::novy(urovne(&[600_000, 1_400_000], 4), Profil::pre_model("Raspberry Pi 3"));
        for _ in 0..10 {
            n.krok_s_vytazenim(62.0, 1.0, Some(0.01));
        }
        assert!(n.index() >= zaciatok, "v nečinnosti pri 62 °C bez zníženia");
    }

    #[test]
    fn profily_modelov() {
        assert_eq!(Profil::pre_model("Raspberry Pi 3 Model B Plus Rev 1.3").strop, 70.0);
        assert_eq!(Profil::pre_model("Raspberry Pi 5 Model B Rev 1.0").strop, 75.0);
        assert_eq!(Profil::pre_model("niečo iné").strop, 70.0);
        assert_eq!(Profil::pre_model("Hardkernel ODROID-C4").strop, 70.0);
        let x96 = Profil::pre_model("Amediatech X96 Max Plus");
        assert_eq!((x96.ciel, x96.strop), (55.0, 65.0));
        assert_eq!(Profil::pre_model("Tanix TX68").strop, 65.0);
    }

    #[test]
    fn parkovanie_jadier_v_pokoji_a_hned_spat_pri_praci() {
        let mut r =
            Regulator::novy(urovne(&[100_000, 1_000_000, 1_908_000], 4), Profil { ciel: 55.0, strop: 65.0 });
        r.nastav_parkovanie(true);
        let horna = r.pocet_urovni() - 1;
        r.index = horna;
        // 19 s pokoja – ešte nie
        for _ in 0..19 {
            r.krok_s_vytazenim(45.0, 1.0, Some(0.02));
        }
        assert_eq!(r.index(), horna);
        r.krok_s_vytazenim(45.0, 1.0, Some(0.02));
        assert_eq!(r.index(), 0, "zaparkované");
        assert!(r.zaparkovane());
        assert_eq!(r.uroven().jadra, 1);
        // dlhý pokoj – ostáva zaparkované (nezvyšuje sa)
        for _ in 0..100 {
            r.krok_s_vytazenim(44.0, 4.0, Some(0.01));
        }
        assert_eq!(r.index(), 0);
        // práca – hneď späť na pôvodnú úroveň
        r.krok_s_vytazenim(45.0, 1.0, Some(0.9));
        assert_eq!(r.index(), horna);
        assert!(!r.zaparkovane());
        // pri strope sa parkovanie zruší a ide sa na najnižšiu úroveň
        for _ in 0..25 {
            r.krok_s_vytazenim(45.0, 1.0, Some(0.0));
        }
        assert!(r.zaparkovane());
        r.krok_s_vytazenim(63.0, 1.0, Some(0.0));
        assert!(!r.zaparkovane());
        // vypnutie parkovania vráti úroveň
        let mut r2 = Regulator::novy(urovne(&[100_000, 1_908_000], 4), Profil { ciel: 55.0, strop: 65.0 });
        r2.nastav_parkovanie(true);
        let p = r2.index();
        for _ in 0..21 {
            r2.krok_s_vytazenim(40.0, 1.0, Some(0.0));
        }
        assert_eq!(r2.index(), 0);
        r2.nastav_parkovanie(false);
        // vráti sa na úroveň pred zaparkovaním (v pokoji ju strážca medzitým zvyšoval)
        assert!(r2.index() >= p && r2.index() > 0);
    }
}
