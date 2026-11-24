//! OKOLIE (R123, R124) – odhad teploty vzduchu okolo zariadenia len zo snímača v čipe a zistenie,
//! že je zariadenie pri zdroji tepla (radiátor, slnko, iné zariadenie) alebo že sa teplota vymyká.
//!
//! Vychádza z poznatkov automobilov (teplota okolia riadiacej jednotky sa modeluje ako porucha, ktorá sa
//! odhaduje a predvída) a telefónov (okolie sa odhaduje z vnútorných snímačov len v nečinnosti):
//!
//! 1. **Studený štart:** zariadenie, ktoré bolo vypnuté, má na začiatku teplotu okolia. Prvé meranie KLÍČA
//!    (pár sekúnd po zapnutí) je teda odhad okolia – ak teplota potom v nečinnosti rastie (doska sa
//!    zohrieva). Ak nerastie, bol to teplý štart (reštart zohriatej dosky) a okolie zo štartu neplatí.
//! 2. **Vlastný ohrev:** z priebehu nečinnej teploty po studenom štarte (exponenciála, Aitkenovo
//!    zrýchlenie z troch minútových priemerov) sa zistí časová konštanta dosky τ a ustálená teplota –
//!    rozdiel oproti okoliu je vlastný ohrev dosky v nečinnosti (v plastovej krabičke viac, v kovovej menej).
//! 3. **Okolie počas behu:** okolie ≈ nečinná teplota + sklon · τ − vlastný ohrev (sklon · τ predpovedá,
//!    kam sa teplota ustáli). Odhad sa mení len v nečinnosti – pri práci sa drží.
//! 4. **Prostredie:** teplé okolie (≥ 30 °C), okolie stúplo o 5 °C a viac (zdroj tepla nablízku), alebo
//!    tepelný útek – pri stálej záťaži sa teplota zvyšuje čoraz rýchlejšie (druhá derivácia kladná).
//!
//! Čistá logika bez hardvéru – skúša sa na fyzikálnom modeli dosky (skúšky nižšie a v `strazca.rs`).

/// Vyťaženie, pod ktorým je procesor nečinný (rovnaké ako v strážcovi).
pub const NECINNOST: f64 = 0.15;
/// Okolie od tejto teploty je teplé (izba má 20–25 °C; pri radiátore 30–45 °C).
pub const TEPLE_OKOLIE: f64 = 30.0;
/// O koľko musí okolie stúpnuť, aby to bol zdroj tepla nablízku.
pub const NARAST_ZDROJA: f64 = 5.0;
/// Najvyššia prvá teplota čipu, ktorú ešte možno brať ako teplotu okolia (studený štart).
pub const STUDENY_START_NAJVIAC: f64 = 30.0;
/// Najviac minútových priemerov v pamäti (24 h).
const NAJVIAC_MINUT: usize = 24 * 60;

/// Minútový priemer.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Minuta {
    /// Poradie minúty od zapnutia (podľa hodín „beží“).
    index: u64,
    teplota: f64,
    /// Priemerné vyťaženie, ak bolo známe.
    vytazenie: Option<f64>,
}

impl Minuta {
    fn necinna(&self) -> bool {
        self.vytazenie.is_some_and(|v| v < NECINNOST)
    }
}

/// Odkiaľ je odhad okolia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zdroj {
    /// Z prvého merania po studenom štarte.
    Start,
    /// Z modelu: nečinná teplota, sklon a vlastný ohrev.
    Model,
    /// R195: z teplomera – vlastník zapísal teplotu v izbe do SKLENIK.TXT (`izba=`).
    Izba,
}

/// Prostredie zariadenia.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Prostredie {
    /// Ešte nevieme (prvé minúty, teplý štart bez modelu).
    Nezname,
    Normalne,
    /// Teplé okolie (≥ 30 °C).
    Teple,
    /// Okolie stúplo – zdroj tepla nablízku (o koľko °C).
    ZdrojTepla(f64),
    /// Pri stálej záťaži teplota rastie čoraz rýchlejšie.
    Utek,
}

impl Prostredie {
    /// Je okolie horúce natoľko, že sa oplatí šetriť aj v nečinnosti (R126)?
    pub fn horuce(&self) -> bool {
        matches!(self, Prostredie::Teple | Prostredie::ZdrojTepla(_) | Prostredie::Utek)
    }
}

/// Čo o okolí vieme – pre obrazovku, správu, strážcu a programy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StavOkolia {
    pub okolie: Option<f64>,
    pub zdroj: Option<Zdroj>,
    /// Okolie na začiatku (studený štart).
    pub okolie_start: Option<f64>,
    /// Vlastný ohrev dosky v nečinnosti (°C nad okolím).
    pub samohrev: Option<f64>,
    /// Časová konštanta dosky (s).
    pub tau_s: Option<f64>,
    /// Bol štart studený (`Some(false)` = teplý, okolie zo štartu neplatí)?
    pub studeny_start: Option<bool>,
    /// Sklon teploty z minútových priemerov (°C/s).
    pub sklon: Option<f64>,
    /// Kam sa teplota pri terajšej záťaži ustáli (teplota + sklon · τ).
    pub ustalena: Option<f64>,
    /// Najnižšie okolie od štartu (najchladnejšie obdobie – R128).
    pub najnizsie: Option<f64>,
    /// Najrýchlejší ohrev pri plnej práci (°C/s) – ako dlho môže trvať šprint (R127).
    pub ohrev_pri_praci: Option<f64>,
    pub prostredie: Prostredie,
}

/// Odhad okolia – volá sa pri každom kroku strážcu.
#[derive(Debug, Clone, Default)]
pub struct Okolie {
    /// TV krabička: dlhý štart (pôvodný zavádzač, logo, náš U-Boot, čítanie z USB) zohreje čip – prvé
    /// meranie nad `STUDENY_START_NAJVIAC` nie je teplota izby.
    dlhy_start: bool,
    /// Prvé meranie: (hodiny „beží“ v s, teplota).
    prve: Option<(f64, f64)>,
    minuty: Vec<Minuta>,
    /// Rozpracovaná minúta: index, súčet teplôt, počet, súčet vyťaženia, počet s vyťažením.
    rozpracovana: Option<(u64, f64, u32, f64, u32)>,
    studeny: Option<bool>,
    okolie_start: Option<f64>,
    samohrev: Option<f64>,
    tau: Option<f64>,
    odhad: Option<f64>,
    zdroj: Option<Zdroj>,
    najnizsie: Option<f64>,
    /// Okolie v okamihu, keď sa zmenil nečinný príkon (R126), a minúta zmeny – vlastný ohrev sa
    /// prepočíta, až keď sa teplota znova ustáli (najskôr po 3 τ).
    prekalibrovat: Option<(f64, u64)>,
    prostredie: Option<Prostredie>,
    ohrev_pri_praci: Option<f64>,
    /// R195: teplota v izbe z teplomera (SKLENIK.TXT) – okolie sa neodhaduje.
    izba: Option<f64>,
}

impl Okolie {
    pub fn nove() -> Okolie {
        Okolie::default()
    }

    /// Pre TV krabičky (dlhý štart zohreje čip skôr, než KLÍČ prvý raz zmeria).
    pub fn s_dlhym_startom() -> Okolie {
        Okolie { dlhy_start: true, ..Okolie::default() }
    }

    /// R195: teplota v izbe z teplomera – okolie je známe, vlastný ohrev = ustálená teplota − izba (aj pri
    /// teplom štarte, keď by odhad zo štartu bol zlý – 2. kolo meraní, plastová krabička: +5 °C).
    pub fn nastav_izbu(&mut self, t: f64) {
        self.izba = Some(t);
        self.okolie_start = Some(t);
        self.odhad = Some(t);
        self.zdroj = Some(Zdroj::Izba);
    }

    /// Krok: hodiny „beží“ (s), teplota čipu, vyťaženie (0–1).
    pub fn krok(&mut self, s: f64, teplota: f64, vytazenie: Option<f64>) {
        if self.prve.is_none() {
            self.prve = Some((s, teplota));
        }
        let index = (s / 60.0).floor().max(0.0) as u64;
        match &mut self.rozpracovana {
            Some((i, st, n, sv, nv)) if *i == index => {
                *st += teplota;
                *n += 1;
                if let Some(v) = vytazenie {
                    *sv += v;
                    *nv += 1;
                }
            }
            _ => {
                if let Some((i, st, n, sv, nv)) = self.rozpracovana.take()
                    && n > 0
                {
                    let vytazenie = (nv > 0).then(|| sv / nv as f64);
                    self.pridaj(Minuta { index: i, teplota: st / n as f64, vytazenie });
                }
                let (sv, nv) = vytazenie.map_or((0.0, 0), |v| (v, 1));
                self.rozpracovana = Some((index, teplota, 1, sv, nv));
            }
        }
    }

    /// Nečinný príkon sa zmenil (napr. Ethernet 100 Mb/s – R126): okolie sa drží, vlastný ohrev sa
    /// prepočíta pri najbližšom ustálení.
    pub fn prekalibruj(&mut self) {
        if let Some(o) = self.odhad {
            let minuta = self.rozpracovana.map_or(0, |r| r.0);
            self.prekalibrovat = Some((o, minuta));
        }
    }

    fn pridaj(&mut self, m: Minuta) {
        // medzera v hodinách (napr. zaseknutie) – nesúvislé minúty sa nemiešajú
        if self.minuty.last().is_some_and(|p| m.index != p.index + 1) {
            self.minuty.clear();
        }
        // ohrev pri plnej práci: prírastok za minútu, keď procesor pracoval aspoň na 80 %
        if let Some(p) = self.minuty.last()
            && m.vytazenie.is_some_and(|v| v >= 0.8)
            && p.vytazenie.is_some_and(|v| v >= 0.8)
        {
            let r = (m.teplota - p.teplota) / 60.0;
            if r > 0.0 {
                self.ohrev_pri_praci = Some(self.ohrev_pri_praci.map_or(r, |x| x.max(r)));
            }
        }
        self.minuty.push(m);
        if self.minuty.len() > NAJVIAC_MINUT {
            self.minuty.remove(0);
        }
        self.analyzuj();
    }

    fn analyzuj(&mut self) {
        let Some((_, t0)) = self.prve else { return };
        // 1. studený alebo teplý štart – rozhodne sa po troch celých minútach
        if self.studeny.is_none()
            && let Some(izba) = self.izba
        {
            // čip je po pár sekundách práce o niekoľko °C nad izbou; viac = doska ešte teplá z predošlého behu
            self.studeny = Some(t0 - izba <= 6.0);
        }
        if self.studeny.is_none() && self.minuty.len() >= 3 {
            let priemer = self.minuty[..3].iter().map(|m| m.teplota).sum::<f64>() / 3.0;
            let rastie = self.minuty[2].teplota > self.minuty[0].teplota;
            // TV krabička: nad 30 °C pri prvom meraní to nie je vzduch v izbe, ale čip zohriaty zavádzačom
            // (pôvodný zavádzač, logo, náš U-Boot a čítanie z USB trvá ~20 s) – okolie zo štartu by bolo
            // o 10 °C vyššie (X96: 36 °C pri 24,6 °C v izbe)
            let studeny = priemer - t0 >= 1.0 && rastie && !(self.dlhy_start && t0 >= STUDENY_START_NAJVIAC);
            self.studeny = Some(studeny);
            if studeny {
                // čip je pri prvom meraní o kúsok teplejší než doska (pár sekúnd práce zavádzača)
                let o = t0 - 0.5;
                self.okolie_start = Some(o);
                self.odhad = Some(o);
                self.zdroj = Some(Zdroj::Start);
            }
        }
        // 2. vlastný ohrev a τ z priebehu nečinného ohrevu po studenom štarte
        if (self.studeny == Some(true) || self.izba.is_some()) && self.samohrev.is_none() {
            self.prelozenie_ohrevu();
        }
        // 3. ustálená nečinnosť: vlastný ohrev (prvý raz / po prekalibrovaní)
        if let Some(t_ust) = self.ustalena_necinnost() {
            let teraz = self.minuty.last().map_or(0, |m| m.index);
            let cakat = self.tau.map_or(20, |t| (3.0 * t / 60.0).ceil() as u64).max(10);
            if let Some((o, od)) = self.prekalibrovat {
                if teraz >= od + cakat {
                    self.samohrev = Some((t_ust - o).clamp(0.3, 45.0));
                    self.prekalibrovat = None;
                }
            } else if self.samohrev.is_none()
                && let Some(o) = self.okolie_start
            {
                self.samohrev = Some((t_ust - o).clamp(0.3, 45.0));
            }
        }
        // 4. okolie počas behu (len v nečinnosti a keď poznáme vlastný ohrev)
        if let (Some(sh), Some(posledna)) = (self.samohrev, self.minuty.last().copied())
            && posledna.necinna()
            && self.prekalibrovat.is_none()
            && self.izba.is_none()
        {
            let predpoved = self.ustalena_teplota().unwrap_or(posledna.teplota);
            let novy = predpoved - sh;
            // vyhladenie – minútový šum snímača sa neprenesie do odhadu
            self.odhad = Some(match (self.odhad, self.zdroj) {
                (Some(o), Some(Zdroj::Model)) => o + 0.3 * (novy - o),
                _ => novy,
            });
            self.zdroj = Some(Zdroj::Model);
        }
        if let Some(o) = self.odhad {
            self.najnizsie = Some(self.najnizsie.map_or(o, |n| n.min(o)));
        }
        self.prostredie = Some(self.urci_prostredie());
    }

    /// Exponenciálny ohrev po studenom štarte: tri rovnomerne vzdialené nečinné minúty (Aitken) dajú τ
    /// a ustálenú teplotu; vlastný ohrev = ustálená − okolie na začiatku.
    fn prelozenie_ohrevu(&mut self) {
        let Some(o) = self.okolie_start else { return };
        // od 2. minúty (prvá je ovplyvnená skúškou pamäte a meraním karty), len súvislá nečinnosť
        let necinne = self.minuty.iter().skip(1).take_while(|m| m.necinna()).count();
        if necinne < 7 {
            return;
        }
        let h = (necinne - 1) / 2;
        let (a, b, c) = (self.minuty[1].teplota, self.minuty[1 + h].teplota, self.minuty[1 + 2 * h].teplota);
        let (d1, d2) = (b - a, c - b);
        if d1 < 1.0 || d2 < 0.0 || d2 >= 0.95 * d1 {
            return;
        }
        let q = (d2 / d1).max(1e-3);
        let tau = -(h as f64 * 60.0) / q.ln();
        let ustalena = c + d2 * q / (1.0 - q);
        // spoľahlivé, až keď sa ohrev aspoň z polovice prejavil (c je za ~0,7 τ)
        if (2 * h) as f64 * 60.0 >= 0.7 * tau && (60.0..=7200.0).contains(&tau) {
            self.tau = Some(tau);
            self.samohrev = Some((ustalena - o).clamp(0.3, 45.0));
        }
    }

    /// Posledných 5 minút nečinnosti s teplotou v rozpätí 0,6 °C → ustálená teplota.
    fn ustalena_necinnost(&self) -> Option<f64> {
        let n = self.minuty.len();
        if n < 5 {
            return None;
        }
        let posledne = &self.minuty[n - 5..];
        let (min, max) =
            posledne.iter().fold((f64::MAX, f64::MIN), |a, m| (a.0.min(m.teplota), a.1.max(m.teplota)));
        (posledne.iter().all(Minuta::necinna) && max - min <= 0.6)
            .then(|| posledne.iter().map(|m| m.teplota).sum::<f64>() / 5.0)
    }

    /// Sklon z posledných troch minút (°C/s), ak boli súvislé.
    fn sklon(&self) -> Option<f64> {
        let n = self.minuty.len();
        (n >= 3).then(|| (self.minuty[n - 1].teplota - self.minuty[n - 3].teplota) / 120.0)
    }

    /// Kam sa teplota ustáli: posledný priemer + sklon · τ (jednoduchý tepelný model).
    fn ustalena_teplota(&self) -> Option<f64> {
        let t = self.minuty.last()?.teplota;
        Some(t + self.sklon()? * self.tau?)
    }

    fn urci_prostredie(&self) -> Prostredie {
        let n = self.minuty.len();
        // tepelný útek: pri stálej záťaži 4 minútové prírastky rastú a posledný je väčší než 0,3 °C/min
        if n >= 5 {
            let m = &self.minuty[n - 5..];
            let prirastky: Vec<f64> = m.windows(2).map(|w| w[1].teplota - w[0].teplota).collect();
            let stala_zataz = m.windows(2).all(|w| match (w[0].vytazenie, w[1].vytazenie) {
                (Some(a), Some(b)) => (a - b).abs() < 0.1,
                _ => false,
            });
            if stala_zataz && prirastky.windows(2).all(|w| w[1] > w[0] + 0.05) && prirastky[3] > 0.3 {
                return Prostredie::Utek;
            }
        }
        let predosle = self.prostredie.unwrap_or(Prostredie::Nezname);
        let Some(o) = self.odhad else {
            // teplý štart bez modelu: nečinná teplota dlho po štarte stále rastie → stúpa okolie
            if n >= 45 {
                let m = &self.minuty[n - 10..];
                let rast = m[9].teplota - m[0].teplota;
                if m.iter().all(Minuta::necinna)
                    && m.windows(2).all(|w| w[1].teplota >= w[0].teplota - 0.1)
                    && rast >= 2.0
                {
                    return Prostredie::ZdrojTepla(rast);
                }
            }
            return Prostredie::Nezname;
        };
        // hysteréza: von z teplého okolia až pod 28 °C, zo zdroja tepla až pod 3 °C nárastu
        let zaklad = self.okolie_start.or(self.najnizsie).unwrap_or(o);
        let narast = o - zaklad;
        let bol_zdroj = matches!(predosle, Prostredie::ZdrojTepla(_));
        // zdroj tepla zohreje vzduch aspoň na 26 °C (inak je to len zariadenie prinesené z chladu do izby)
        if (narast >= NARAST_ZDROJA || (bol_zdroj && narast >= 3.0)) && o >= 26.0 {
            Prostredie::ZdrojTepla(narast)
        } else if o >= TEPLE_OKOLIE || (predosle == Prostredie::Teple && o >= TEPLE_OKOLIE - 2.0) {
            Prostredie::Teple
        } else {
            Prostredie::Normalne
        }
    }

    pub fn stav(&self) -> StavOkolia {
        StavOkolia {
            okolie: self.odhad,
            zdroj: self.zdroj,
            okolie_start: self.okolie_start,
            samohrev: self.samohrev,
            tau_s: self.tau,
            studeny_start: self.studeny,
            sklon: self.sklon(),
            ustalena: self.ustalena_teplota(),
            najnizsie: self.najnizsie,
            ohrev_pri_praci: self.ohrev_pri_praci,
            prostredie: self.prostredie.unwrap_or(Prostredie::Nezname),
        }
    }
}

impl Prostredie {
    /// Slovo do súboru pre programy a do správy.
    pub fn slovo(&self) -> &'static str {
        match self {
            Prostredie::Nezname => "nezname",
            Prostredie::Normalne => "normalne",
            Prostredie::Teple => "teple",
            Prostredie::ZdrojTepla(_) => "zdroj_tepla",
            Prostredie::Utek => "utek",
        }
    }
}

/// Tepelný rozpočet pre programy ORCHIDEA v `/run/sklenik/teplo` (R127, R128) – riadky `kľúč=hodnota`.
///
/// Programy podľa neho samy uberú prácu skôr, než strážca začne brzdiť (ako Android ADPF), a ťažkú prácu,
/// ktorá počká, nechajú na chladné obdobie (ako dátové centrá a budova Eastgate):
/// - `rezerva` 0–1: koľko zostáva po cieľ (1 = chladné, 0 = pri cieli),
/// - `predpoved_30s`: teplota o 30 s pri terajšej práci,
/// - `sprint_s`: koľko sekúnd plnej práce zostáva po cieľ (z nameraného ohrevu pri práci),
/// - `oddych_s`: oddych po šprinte – polovica nadbytočného tepla odíde za ~0,7 τ,
/// - `odlozit_tazku_pracu`, `chladne_okno`: kedy robiť prácu, ktorá počká.
pub fn rozpocet(
    stav: &StavOkolia,
    teplota: Option<f64>,
    ciel: f64,
    predpoved_30s: Option<f64>,
    vlakna: u32,
    takt_khz: u32,
) -> String {
    let mut r = format!("vlakna={vlakna}\ntakt_khz={takt_khz}\nciel={ciel:.0}\n");
    let c = |x: Option<f64>| x.map_or("nezname".to_string(), |v| format!("{v:.1}"));
    r += &format!(
        "teplota={}\nokolie={}\nprostredie={}\n",
        c(teplota),
        c(stav.okolie),
        stav.prostredie.slovo()
    );
    if let Some(t) = teplota {
        // rezerva voči okoliu (ak ho poznáme), inak voči bežnej izbe 25 °C
        let dno = stav.okolie.unwrap_or(25.0).min(ciel - 1.0);
        r += &format!("rezerva={:.2}\n", ((ciel - t) / (ciel - dno)).clamp(0.0, 1.0));
    }
    r += &format!("predpoved_30s={}\n", c(predpoved_30s));
    r += &format!("tau_s={}\n", stav.tau_s.map_or("nezname".into(), |t| format!("{t:.0}")));
    match (teplota, stav.ohrev_pri_praci) {
        (Some(t), Some(o)) if o > 1e-4 => r += &format!("sprint_s={:.0}\n", ((ciel - t) / o).max(0.0)),
        _ => r += "sprint_s=nezname\n",
    }
    r += &format!("oddych_s={}\n", stav.tau_s.map_or("nezname".into(), |t| format!("{:.0}", 0.7 * t)));
    // R128: ťažká práca, ktorá počká – nie v horúcom okolí, nie pri cieli, nie keď je teplejšie než zvyčajne
    let teplejsie_ako_zvycajne = matches!((stav.okolie, stav.najnizsie), (Some(o), Some(n)) if o > n + 3.0);
    let odlozit =
        stav.prostredie.horuce() || teplota.is_none_or(|t| t > ciel - 5.0) || teplejsie_ako_zvycajne;
    let chladne = !odlozit
        && teplota.is_some_and(|t| t < ciel - 10.0)
        && matches!((stav.okolie, stav.najnizsie), (Some(o), Some(n)) if o <= n + 1.5);
    let ano = |b: bool| if b { "ano" } else { "nie" };
    r += &format!("odlozit_tazku_pracu={}\nchladne_okno={}\n", ano(odlozit), ano(chladne));
    r
}

#[cfg(test)]
mod skusky {
    use super::*;

    /// Jednoduchá doska: T' = (P − (T − okolie)/R)/C, čip = doska (snímač).
    struct Doska {
        t: f64,
        r: f64,
        c: f64,
        p_necinna: f64,
    }

    impl Doska {
        fn krok(&mut self, okolie: f64, p: f64, dt: f64) {
            self.t += (p - (self.t - okolie) / self.r) / self.c * dt;
        }
    }

    /// Simulácia po 4 s (ako strážca v pokoji); `okolie(s)` a `zataz(s)` ako funkcie času.
    fn simuluj(
        sekund: u32,
        okolie: impl Fn(f64) -> f64,
        zataz: impl Fn(f64) -> f64,
        doska: &mut Doska,
        o: &mut Okolie,
    ) {
        let mut s = 2.0;
        while s < sekund as f64 {
            let z = zataz(s);
            // snímač s rozlíšením 0,5 °C (ako starší RPi)
            let nameraná = (doska.t * 2.0).round() / 2.0;
            o.krok(s, nameraná, Some(z));
            doska.krok(okolie(s), doska.p_necinna + 2.0 * z, 4.0);
            s += 4.0;
        }
    }

    fn plastova_rpi3(okolie: f64) -> Doska {
        // ako zmerané 21. 11.: ustálenie 60,5 °C pri ~22 °C, τ ≈ 610 s
        Doska { t: okolie, r: 20.0, c: 610.0 / 20.0, p_necinna: 38.5 / 20.0 }
    }

    #[test]
    fn izba_z_teplomera_aj_pri_teplom_starte() {
        // plastová krabička, doska ešte teplá z predošlého merania: štart 33,8 °C, izba 25 °C
        let mut o = Okolie::nove();
        o.nastav_izbu(25.0);
        let mut s = 0.0;
        while s < 40.0 * 60.0 {
            // ohrev k 46 °C (vlastný ohrev 21 °C), τ 5 min, od 33,8 °C
            let t = 46.0 - (46.0 - 33.8) * (-s / 300.0f64).exp();
            o.krok(s, t, Some(0.01));
            s += 30.0;
        }
        let st = o.stav();
        assert_eq!(st.zdroj, Some(Zdroj::Izba));
        assert_eq!(st.okolie, Some(25.0));
        assert_eq!(st.studeny_start, Some(false));
        let sh = st.samohrev.expect("vlastný ohrev");
        assert!((sh - 21.0).abs() < 1.0, "vlastný ohrev {sh}");
    }

    #[test]
    fn studeny_start_da_okolie_samohrev_a_tau() {
        let mut d = plastova_rpi3(22.0);
        let mut o = Okolie::nove();
        simuluj(4 * 60, |_| 22.0, |_| 0.02, &mut d, &mut o);
        let s = o.stav();
        assert_eq!(s.studeny_start, Some(true));
        assert!((s.okolie.unwrap() - 22.0).abs() < 1.2, "{s:?}");
        assert_eq!(s.zdroj, Some(Zdroj::Start));
        simuluj(40 * 60, |_| 22.0, |_| 0.02, &mut d, &mut o);
        let s = o.stav();
        let tau = s.tau_s.expect("τ");
        assert!((400.0..900.0).contains(&tau), "τ {tau:.0} s");
        let sh = s.samohrev.expect("vlastný ohrev");
        assert!((sh - 38.5).abs() < 4.0, "vlastný ohrev {sh:.1}");
        assert!((s.okolie.unwrap() - 22.0).abs() < 2.5, "{s:?}");
        assert_eq!(s.prostredie, Prostredie::Normalne);
    }

    #[test]
    fn teply_start_okolie_nepozna() {
        let mut d = plastova_rpi3(22.0);
        d.t = 60.5; // reštart zohriatej dosky
        let mut o = Okolie::nove();
        simuluj(10 * 60, |_| 22.0, |_| 0.02, &mut d, &mut o);
        let s = o.stav();
        assert_eq!(s.studeny_start, Some(false));
        assert_eq!(s.okolie, None);
        assert_eq!(s.prostredie, Prostredie::Nezname);
    }

    #[test]
    fn radiator_zapnuty_po_hodine_je_zdroj_tepla() {
        // kovová krabička: menší vlastný ohrev, τ 900 s; po hodine sa vedľa zapne radiátor (+12 °C za 20 min)
        let mut d = Doska { t: 21.0, r: 9.0, c: 100.0, p_necinna: 2.0 };
        let mut o = Okolie::nove();
        let okolie = |s: f64| 21.0 + (((s - 3600.0) / 1200.0).clamp(0.0, 1.0)) * 12.0;
        simuluj(3600, okolie, |_| 0.02, &mut d, &mut o);
        let pred = o.stav();
        assert_eq!(pred.prostredie, Prostredie::Normalne, "{pred:?}");
        assert!((pred.okolie.unwrap() - 21.0).abs() < 2.0, "{pred:?}");
        simuluj(3 * 3600, okolie, |_| 0.02, &mut d, &mut o);
        let po = o.stav();
        assert!(matches!(po.prostredie, Prostredie::ZdrojTepla(n) if n >= 5.0), "{po:?}");
        assert!((po.okolie.unwrap() - 33.0).abs() < 3.0, "{po:?}");
    }

    #[test]
    fn zariadenie_prinesene_z_chladu_nie_je_zdroj_tepla() {
        // doska vychladnutá na 12 °C (prinesená zvonka) v izbe s 21 °C – okolie „stúpne“ o 9 °C, ale to nie je
        // zdroj tepla
        let mut d = Doska { t: 12.0, r: 9.0, c: 100.0, p_necinna: 2.0 };
        let mut o = Okolie::nove();
        simuluj(3 * 3600, |_| 21.0, |_| 0.02, &mut d, &mut o);
        assert!(!o.stav().prostredie.horuce(), "{:?}", o.stav());
    }

    #[test]
    fn teple_okolie_od_zaciatku() {
        // zariadenie zapnuté rovno pri radiátore: okolie 34 °C
        let mut d = plastova_rpi3(34.0);
        let mut o = Okolie::nove();
        simuluj(5 * 60, |_| 34.0, |_| 0.02, &mut d, &mut o);
        assert_eq!(o.stav().prostredie, Prostredie::Teple);
        assert!(o.stav().prostredie.horuce());
    }

    #[test]
    fn prekalibrovanie_po_zmene_necinneho_prikonu() {
        let mut d = Doska { t: 22.0, r: 9.0, c: 60.0, p_necinna: 2.0 };
        let mut o = Okolie::nove();
        simuluj(60 * 60, |_| 22.0, |_| 0.02, &mut d, &mut o);
        let pred = o.stav();
        // úspora (R126): nečinný príkon o 0,4 W nižší – okolie sa nesmie „ochladiť“
        o.prekalibruj();
        d.p_necinna -= 0.4;
        simuluj(2 * 3600, |_| 22.0, |_| 0.02, &mut d, &mut o);
        let po = o.stav();
        assert!((po.okolie.unwrap() - 22.0).abs() < 1.5, "pred {pred:?}, po {po:?}");
        assert!(po.samohrev.unwrap() < pred.samohrev.unwrap() - 2.0, "pred {pred:?}, po {po:?}");
    }

    #[test]
    fn rozpocet_pre_programy() {
        let mut d = plastova_rpi3(22.0);
        d.r = 9.0; // doska s chladičom
        d.p_necinna = 2.0;
        let mut o = Okolie::nove();
        simuluj(30 * 60, |_| 22.0, |_| 0.02, &mut d, &mut o);
        // 5 minút plnej práce – ohrev pri práci
        simuluj(35 * 60, |_| 22.0, |s| if s >= 30.0 * 60.0 { 1.0 } else { 0.02 }, &mut d, &mut o);
        let s = o.stav();
        assert!(s.ohrev_pri_praci.is_some(), "{s:?}");
        let r = rozpocet(&s, Some(45.0), 60.0, Some(46.0), 4, 1_400_000);
        for k in [
            "vlakna=4",
            "takt_khz=1400000",
            "ciel=60",
            "teplota=45.0",
            "prostredie=normalne",
            "predpoved_30s=46.0",
        ] {
            assert!(r.lines().any(|l| l == k), "chýba {k}: {r}");
        }
        assert!(
            r.contains("rezerva=0.") && !r.contains("sprint_s=nezname") && !r.contains("tau_s=nezname"),
            "{r}"
        );
        assert!(r.contains("odlozit_tazku_pracu=nie") && r.contains("chladne_okno=ano"), "{r}");
        // pri cieli a v horúcom okolí sa ťažká práca odkladá
        let r = rozpocet(&s, Some(58.0), 60.0, None, 4, 1_400_000);
        assert!(r.contains("odlozit_tazku_pracu=ano") && r.contains("chladne_okno=nie"), "{r}");
        let horuce = StavOkolia { prostredie: Prostredie::Teple, ..s };
        assert!(rozpocet(&horuce, Some(40.0), 60.0, None, 1, 600_000).contains("odlozit_tazku_pracu=ano"));
        let nic = rozpocet(&Okolie::nove().stav(), None, 60.0, None, 1, 0);
        assert!(
            nic.contains("okolie=nezname") && nic.contains("sprint_s=nezname") && !nic.contains("rezerva"),
            "{nic}"
        );
    }

    #[test]
    fn tepelny_utek_pri_stalej_zatazi() {
        let mut o = Okolie::nove();
        // čoraz rýchlejší rast pri stálej záťaži 0,9
        let mut t = 50.0;
        let mut prirastok = 0.2;
        for m in 0..10u64 {
            for k in 0..15 {
                o.krok((m * 60 + k * 4) as f64, t, Some(0.9));
            }
            t += prirastok;
            prirastok *= 1.6;
        }
        o.krok(600.0, t, Some(0.9));
        assert_eq!(o.stav().prostredie, Prostredie::Utek);
        // obyčajný ohrev (prírastky klesajú) nie je útek
        let mut o = Okolie::nove();
        let mut t = 50.0;
        for m in 0..10u64 {
            for k in 0..15 {
                o.krok((m * 60 + k * 4) as f64, t, Some(0.9));
            }
            t += 2.0 * 0.7f64.powi(m as i32);
        }
        o.krok(600.0, t, Some(0.9));
        assert_ne!(o.stav().prostredie, Prostredie::Utek);
    }

    #[test]
    fn cip_zohriaty_zavadzacom_nie_je_teplota_okolia() {
        // X96: prvé meranie 36 °C (zavádzač a logo zohriali čip), potom ohrev k 61 °C – izba má 24,6 °C
        let mut o = Okolie::s_dlhym_startom();
        for k in 0..=1200 {
            let s = k as f64;
            o.krok(s, 61.0 - 25.0 * (-s / 480.0).exp(), Some(0.02));
        }
        let st = o.stav();
        assert_eq!(st.studeny_start, Some(false), "{st:?}");
        assert_eq!(st.okolie, None);
        assert!(!st.prostredie.horuce(), "{st:?}");
    }
}
