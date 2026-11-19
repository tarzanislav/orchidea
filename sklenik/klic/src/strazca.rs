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
    pub fn pre_model(model: &str) -> Profil {
        if model.contains("Raspberry Pi 4")
            || model.contains("Raspberry Pi 5")
            || model.contains("Raspberry Pi 400")
        {
            Profil { ciel: 65.0, strop: 75.0 }
        } else {
            Profil { ciel: 60.0, strop: 70.0 }
        }
    }
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
}

/// Ako ďaleko dopredu (s) regulátor predpovedá teplotu.
const HORIZONT_S: f64 = 30.0;
/// Rezerva pod stropom, pri ktorej sa okamžite prejde na najnižšiu úroveň.
const NUDZA_POD_STROPOM: f64 = 3.0;

impl Regulator {
    /// Začína opatrne – na najvyššej úrovni s príkonom najviac polovičným; teplotu si najprv overí.
    pub fn novy(urovne: Vec<Uroven>, profil: Profil) -> Regulator {
        assert!(!urovne.is_empty(), "aspoň jedna úroveň výkonu");
        let index = urovne.iter().rposition(|u| u.prikon <= 0.5).unwrap_or(0);
        Regulator { urovne, profil, index, posledna: None, sklon: 0.0, od_zmeny: 0.0 }
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

    /// Jeden krok regulácie: aktuálna teplota a čas od posledného kroku. Vráti zvolenú úroveň.
    pub fn krok(&mut self, teplota: f64, dt_s: f64) -> Uroven {
        let dt = dt_s.max(1e-3);
        if let Some(p) = self.posledna {
            self.sklon = 0.7 * self.sklon + 0.3 * (teplota - p) / dt;
        }
        self.posledna = Some(teplota);
        self.od_zmeny += dt;
        let Profil { ciel, strop } = self.profil;
        let predikcia = teplota + self.sklon.max(0.0) * HORIZONT_S;
        if teplota >= strop - NUDZA_POD_STROPOM {
            self.nastav(0);
        } else if (teplota > ciel || predikcia > strop - NUDZA_POD_STROPOM) && self.od_zmeny >= 2.0 {
            self.nastav(self.index.saturating_sub(1));
        } else if teplota < ciel - 4.0 && predikcia < ciel && self.od_zmeny >= 5.0 {
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

    /// Fyzikálny model dosky s pasívnym chladičom: dT/dt = (P − (T − T_okolia)/R) / C.
    /// Konštanty zodpovedajú Raspberry Pi 3 B+: v nečinnosti okolo 45 °C pri 25 °C v miestnosti, pri plnej
    /// práci bez obmedzenia by vystúpila na ~80 °C (tam firmvér začína škrtiť).
    struct Doska {
        t: f64,
        okolie: f64,
        r: f64,
        c: f64,
        nezatazena_w: f64,
        procesor_max_w: f64,
    }

    impl Doska {
        fn rpi3(okolie: f64) -> Doska {
            Doska { t: okolie, okolie, r: 18.0, c: 8.0, nezatazena_w: 1.2, procesor_max_w: 1.8 }
        }

        /// Krok simulácie; `zataz` 0..1 = podiel času, keď majú povolené jadrá prácu.
        fn krok(&mut self, u: &Uroven, zataz: f64, dt: f64) {
            let p = self.nezatazena_w + self.procesor_max_w * u.prikon * zataz;
            self.t += (p - (self.t - self.okolie) / self.r) / self.c * dt;
        }
    }

    fn simuluj(okolie: f64, zataz: impl Fn(f64) -> f64, sekund: u32) -> (f64, f64, Regulator, u32) {
        let mut d = Doska::rpi3(okolie);
        let mut r = Regulator::novy(
            urovne(&[600_000, 1_400_000], 4),
            Profil::pre_model("Raspberry Pi 3 Model B Plus"),
        );
        let (mut maximum, mut zmeny, mut posledny) = (d.t, 0u32, r.index());
        let mut prace = 0.0;
        for s in 0..sekund {
            let u = r.krok(d.t, 1.0);
            if r.index() != posledny && s > sekund / 2 {
                zmeny += 1;
            }
            posledny = r.index();
            let z = zataz(s as f64);
            prace += u.vykon * z;
            d.krok(&u, z, 1.0);
            maximum = maximum.max(d.t);
        }
        (maximum, prace / sekund as f64, r, zmeny)
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
    fn profily_modelov() {
        assert_eq!(Profil::pre_model("Raspberry Pi 3 Model B Plus Rev 1.3").strop, 70.0);
        assert_eq!(Profil::pre_model("Raspberry Pi 5 Model B Rev 1.0").strop, 75.0);
        assert_eq!(Profil::pre_model("niečo iné").strop, 70.0);
    }
}
