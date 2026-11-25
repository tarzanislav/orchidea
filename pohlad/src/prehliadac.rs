//! Jadro okna Pohľadu – nezávislé od systému okien, bez `unsafe`.
//!
//! **Chladný návrh (R208):** jadro nič nerobí samo od seba. Okno mu odovzdá vstup (`vstup`) alebo ho zobudí
//! pracovné vlákno (`spracuj_pozadie`); každá z týchto funkcií vráti, či treba prekresliť. Okno potom zavolá
//! `vykresli` – len vtedy. Bez vstupu a bez dokončenej práce na pozadí sa nepočíta nič (žiadny časovač ani
//! animácia), procesor spí v `GetMessage`.
//!
//! - **Načítanie na pozadí:** jedno pracovné vlákno s frontom (najprv aktuálny obraz, potom susedné, potom
//!   náhľady viditeľnej časti pásu); po každom výsledku zavolá `zobud` (okno si pošle správu).
//! - **Zobrazenie:** pyramída zmenšenín po polovici (priemer v lineárnom svetle), vykreslí sa len okno
//!   obrazovky: pri zmenšení z najbližšej väčšej úrovne bilineárne, pri zväčšení od 2× najbližší bod (ostré
//!   pixely). Priehľadnosť na šachovnici.
//! - **Porovnanie:** obraz A sa označí (`Oznac`), aktuálny je B; režimy delený (posuvná deliaca čiara) a rozdiel
//!   (|A − B| × 8), oba s rovnakým priblížením a posunom; PSNR a SSIM sa vypočítajú na pozadí.

use crate::nahlady::{self, Polozka};
use crate::porovnanie;
use orchidea_formaty::obraz::{Limity, Obraz, Vzorky};
use orchidea_jadro::paralel;
use orchidea_pismo::{Farba, Pismo, Platno};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

/// Povel z klávesnice (mapovanie kláves robí okno).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Povel {
    Dalsi,
    Predosly,
    Prvy,
    Posledny,
    Priblizit,
    Oddialit,
    /// Zmestiť do okna.
    Prisposobit,
    /// 1 bod obrazu = 1 bod obrazovky.
    Skutocna,
    /// Označiť aktuálny obraz ako A na porovnanie (opakovane: zrušiť).
    Oznac,
    /// Prepnúť režim: jeden obraz → delený → rozdiel.
    Rezim,
    /// Ukázať / skryť pás náhľadov.
    Pas,
    /// Ukázať / skryť riadok s údajmi.
    Udaje,
}

/// Vstup od okna.
#[derive(Debug, Clone, PartialEq)]
pub enum Vstup {
    Povel(Povel),
    /// Nová veľkosť plochy okna v bodoch.
    Velkost {
        sirka: usize,
        vyska: usize,
    },
    /// Koliesko: kladné = priblížiť, okolo bodu (x, y).
    Koliesko {
        krokov: f32,
        x: f32,
        y: f32,
    },
    /// Ťahanie ľavým tlačidlom (posun o dx, dy bodov).
    Tahaj {
        dx: f32,
        dy: f32,
    },
    /// Kliknutie ľavým tlačidlom.
    Klik {
        x: f32,
        y: f32,
    },
    /// Pohyb myši (v delenom režime posúva deliacu čiaru, ak je stlačené pravé tlačidlo).
    Delic {
        x: f32,
    },
    /// Otvoriť súbor alebo priečinok.
    Otvor(PathBuf),
}

/// Režim zobrazenia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rezim {
    Jeden,
    Deleny,
    Rozdiel,
}

/// Obraz pripravený na zobrazenie: pyramída RGBA 8 b (úroveň 0 = plné rozlíšenie).
#[derive(Debug)]
pub struct Snimka {
    pub sirka: usize,
    pub vyska: usize,
    urovne: Vec<(usize, usize, Vec<[u8; 4]>)>,
    /// Pôvodný obraz na zobrazenie (8 b sRGB, natočený) – na porovnanie.
    obraz: Obraz,
    /// Krátky popis (rozmery, hĺbka, formát).
    pub popis: String,
}

fn rgba(o: &Obraz) -> Vec<[u8; 4]> {
    let Vzorky::U8(d) = &o.vzorky else { return Vec::new() };
    let k = o.kanaly();
    d.chunks_exact(k)
        .map(|p| match (k, o.alfa) {
            (1, _) => [p[0], p[0], p[0], 255],
            (2, true) => [p[0], p[0], p[0], p[1]],
            (3, _) => [p[0], p[1], p[2], 255],
            _ => [p[0], p[1], p[2], if o.alfa { p[3] } else { 255 }],
        })
        .collect()
}

/// Tabuľky sRGB ↔ lineárne (pre polovičné zmenšenie pyramídy).
fn tabulky() -> &'static (Vec<f32>, Vec<u8>) {
    static T: std::sync::OnceLock<(Vec<f32>, Vec<u8>)> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let s = orchidea_farba::Prenos::Srgb;
        let dek = (0..256).map(|i| s.do_linearneho(i as f64 / 255.0) as f32).collect();
        let kod = (0..4096).map(|i| (s.z_linearneho(i as f64 / 4095.0) * 255.0).round() as u8).collect();
        (dek, kod)
    })
}

fn polovica(w: usize, h: usize, d: &[[u8; 4]]) -> (usize, usize, Vec<[u8; 4]>) {
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let (dek, kod) = tabulky();
    let mut v = vec![[0u8; 4]; nw * nh];
    paralel::pre_kusy(&mut v, nw, |y, riadok| {
        for (x, o) in riadok.iter_mut().enumerate() {
            let body = [(2 * x, 2 * y), (2 * x + 1, 2 * y), (2 * x, 2 * y + 1), (2 * x + 1, 2 * y + 1)];
            let mut s = [0f32; 4];
            let mut n = 0.0;
            for (bx, by) in body {
                if bx < w && by < h {
                    let p = d[by * w + bx];
                    let a = p[3] as f32 / 255.0;
                    for c in 0..3 {
                        s[c] += dek[p[c] as usize] * a;
                    }
                    s[3] += a;
                    n += 1.0;
                }
            }
            let a = s[3] / n;
            for c in 0..3 {
                let l = if s[3] > 0.0 { s[c] / s[3] } else { 0.0 };
                o[c] = kod[(l.clamp(0.0, 1.0) * 4095.0).round() as usize];
            }
            o[3] = (a * 255.0).round() as u8;
        }
    });
    (nw, nh, v)
}

impl Snimka {
    /// Pripraví obraz na zobrazenie (prevod do sRGB, natočenie, pyramída).
    pub fn z_obrazu(o: Obraz, popis: String) -> orchidea_jadro::Vysledok<Snimka> {
        let o = nahlady::na_zobrazenie(o)?;
        let (w, h) = (o.sirka, o.vyska);
        let mut urovne = vec![(w, h, rgba(&o))];
        while urovne.last().is_some_and(|u| u.0 > 1 || u.1 > 1) && urovne.len() < 16 {
            let (uw, uh, d) = urovne.last().unwrap();
            let p = polovica(*uw, *uh, d);
            urovne.push(p);
        }
        Ok(Snimka { sirka: w, vyska: h, urovne, obraz: o, popis })
    }

    /// Farba bodu v súradniciach plného rozlíšenia pri mierke `z` (bodov obrazovky na bod obrazu).
    fn bod(&self, x: f32, y: f32, z: f32) -> [u8; 4] {
        if z >= 2.0 {
            let (xi, yi) = (x.floor() as isize, y.floor() as isize);
            if xi < 0 || yi < 0 || xi >= self.sirka as isize || yi >= self.vyska as isize {
                return [0; 4];
            }
            return self.urovne[0].2[yi as usize * self.sirka + xi as usize];
        }
        // úroveň, ktorá je ešte aspoň taká jemná ako obrazovka
        let mut l = 0;
        while l + 1 < self.urovne.len() && z * (1 << (l + 1)) as f32 <= 1.0 {
            l += 1;
        }
        let k = (1 << l) as f32;
        let (uw, uh, d) = &self.urovne[l];
        let (fx, fy) = (x / k - 0.5, y / k - 0.5);
        if x < 0.0 || y < 0.0 || x >= self.sirka as f32 || y >= self.vyska as f32 {
            return [0; 4];
        }
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let cl = |v: f32, m: usize| (v.max(0.0) as usize).min(m - 1);
        let (xa, xb, ya, yb) = (cl(x0, *uw), cl(x0 + 1.0, *uw), cl(y0, *uh), cl(y0 + 1.0, *uh));
        let (p, q, r, s) = (d[ya * uw + xa], d[ya * uw + xb], d[yb * uw + xa], d[yb * uw + xb]);
        let mut o = [0u8; 4];
        for c in 0..4 {
            let hore = p[c] as f32 * (1.0 - tx) + q[c] as f32 * tx;
            let dole = r[c] as f32 * (1.0 - tx) + s[c] as f32 * tx;
            o[c] = (hore * (1.0 - ty) + dole * ty).round() as u8;
        }
        o
    }
}

// ---------------------------------------------------------------- práca na pozadí

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Ziadost {
    Snimka(usize),
    Nahlad(usize),
    Porovnanie(usize, usize),
}

enum Hotove {
    Snimka(usize, Result<Arc<Snimka>, String>),
    Nahlad(usize, Option<Arc<Snimka>>),
    Porovnanie(usize, usize, Result<porovnanie::Miery, String>),
}

/// Front práce pracovného vlákna (nové žiadosti nahradia staré – bez zbytočnej práce).
struct Front {
    ziadosti: Mutex<(VecDeque<Ziadost>, bool)>,
    signal: Condvar,
}

/// Rozmery pásu náhľadov.
const PAS: usize = 96;
const NAHLAD: usize = 80;
const POZADIE: Farba = 0x1C1D1F;
const TEXT: Farba = 0xD8D8D8;
const ZVYRAZNENIE: Farba = 0x5A8DEE;

/// Stav prehliadača.
pub struct Prehliadac {
    polozky: Vec<Polozka>,
    aktualny: usize,
    sirka: usize,
    vyska: usize,
    /// Priblíženie (bodov obrazovky na bod obrazu); `None` = zmestiť do okna.
    priblizenie: Option<f32>,
    /// Posun stredu obrazu od stredu plochy (v bodoch obrazovky).
    posun: (f32, f32),
    rezim: Rezim,
    oznaceny: Option<usize>,
    delic: f32,
    pas: bool,
    udaje: bool,
    snimky: HashMap<usize, Result<Arc<Snimka>, String>>,
    nahlady: HashMap<usize, Option<Arc<Snimka>>>,
    miery: Option<(usize, usize, Result<porovnanie::Miery, String>)>,
    front: Arc<Front>,
    vysledky: Receiver<Hotove>,
    cakajuce: std::collections::HashSet<Ziadost>,
    pismo: Pismo,
    /// Počet vykreslení (na meranie chladu).
    pub vykresleni: u64,
    priecinok: Option<PathBuf>,
    zmena: bool,
    /// Cesty zdieľané s pracovným vláknom.
    zoznam_ciest: Arc<Mutex<Vec<PathBuf>>>,
}

impl std::fmt::Debug for Prehliadac {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prehliadac")
            .field("polozky", &self.polozky.len())
            .field("aktualny", &self.aktualny)
            .finish()
    }
}

fn pracuj(
    front: Arc<Front>,
    polozky: Arc<Mutex<Vec<PathBuf>>>,
    tx: Sender<Hotove>,
    zobud: Arc<dyn Fn() + Send + Sync>,
) {
    let limity = Limity::default();
    let snimka = |i: usize| -> Result<Arc<Snimka>, String> {
        let cesta = polozky.lock().map_err(|_| "zámok")?.get(i).cloned().ok_or("mimo zoznamu")?;
        let d = std::fs::read(&cesta).map_err(|e| e.to_string())?;
        let format = orchidea_formaty::rozpoznaj(&d).map(|f| format!("{f:?}")).unwrap_or_default();
        let o = orchidea_formaty::citaj(&d, &limity).map_err(|e| e.to_string())?;
        let popis = format!("{} × {} · {} b · {format}", o.sirka, o.vyska, o.bitov);
        Snimka::z_obrazu(o, popis).map(Arc::new).map_err(|e| e.to_string())
    };
    let mut posledne: HashMap<usize, Arc<Snimka>> = HashMap::new();
    loop {
        let z = {
            let Ok(mut f) = front.ziadosti.lock() else { return };
            loop {
                if f.1 {
                    return;
                }
                if let Some(z) = f.0.pop_front() {
                    break z;
                }
                f = match front.signal.wait(f) {
                    Ok(f) => f,
                    Err(_) => return,
                };
            }
        };
        let v = match z {
            Ziadost::Snimka(i) => {
                let r = snimka(i);
                if let Ok(s) = &r {
                    posledne.insert(i, s.clone());
                    if posledne.len() > 4 {
                        let k = *posledne.keys().find(|&&k| k != i).unwrap();
                        posledne.remove(&k);
                    }
                }
                Hotove::Snimka(i, r)
            }
            Ziadost::Nahlad(i) => {
                let cesta = polozky.lock().ok().and_then(|p| p.get(i).cloned());
                let n = cesta
                    .and_then(|c| nahlady::nahlad(&c, NAHLAD, &limity).ok())
                    .and_then(|o| Snimka::z_obrazu(o, String::new()).ok())
                    .map(Arc::new);
                Hotove::Nahlad(i, n)
            }
            Ziadost::Porovnanie(a, b) => {
                let ziskaj = |i: usize| posledne.get(&i).cloned().map(Ok).unwrap_or_else(|| snimka(i));
                let r = match (ziskaj(a), ziskaj(b)) {
                    (Ok(x), Ok(y)) => porovnanie::porovnaj(&x.obraz, &y.obraz).map_err(|e| e.to_string()),
                    (Err(e), _) | (_, Err(e)) => Err(e),
                };
                Hotove::Porovnanie(a, b, r)
            }
        };
        if tx.send(v).is_err() {
            return;
        }
        zobud();
    }
}

impl Drop for Prehliadac {
    fn drop(&mut self) {
        if let Ok(mut f) = self.front.ziadosti.lock() {
            f.1 = true;
        }
        self.front.signal.notify_all();
    }
}

impl Prehliadac {
    /// Nový prehliadač; `zobud` sa zavolá z pracovného vlákna, keď je hotová práca (okno si má poslať správu
    /// a potom zavolať `spracuj_pozadie`).
    pub fn novy(zobud: Arc<dyn Fn() + Send + Sync>) -> Prehliadac {
        // vnútorná súbežnosť dekódovania: polovica jadier (chladne)
        paralel::obmedz_vlakna((paralel::pocet_vlakien() / 2).max(1));
        let front =
            Arc::new(Front { ziadosti: Mutex::new((VecDeque::new(), false)), signal: Condvar::new() });
        let (tx, rx) = channel();
        let polozky = Arc::new(Mutex::new(Vec::new()));
        let f = front.clone();
        let p = polozky.clone();
        std::thread::spawn(move || pracuj(f, p, tx, zobud));
        Prehliadac {
            polozky: Vec::new(),
            aktualny: 0,
            sirka: 1,
            vyska: 1,
            priblizenie: None,
            posun: (0.0, 0.0),
            rezim: Rezim::Jeden,
            oznaceny: None,
            delic: 0.5,
            pas: true,
            udaje: true,
            snimky: HashMap::new(),
            nahlady: HashMap::new(),
            miery: None,
            front,
            vysledky: rx,
            cakajuce: Default::default(),
            pismo: Pismo::stonka(),
            vykresleni: 0,
            priecinok: None,
            zmena: true,
            zoznam_ciest: polozky,
        }
    }

    /// Otvorí súbor (a jeho priečinok) alebo priečinok.
    pub fn otvor(&mut self, cesta: &Path) {
        let (priecinok, vybrany) = if cesta.is_dir() {
            (cesta.to_path_buf(), None)
        } else {
            (cesta.parent().map(Path::to_path_buf).unwrap_or_default(), Some(cesta.to_path_buf()))
        };
        self.polozky = nahlady::zoznam(&priecinok).unwrap_or_default();
        if let Ok(mut z) = self.zoznam_ciest.lock() {
            *z = self.polozky.iter().map(|p| p.cesta.clone()).collect();
        }
        self.aktualny = vybrany.and_then(|v| self.polozky.iter().position(|p| p.cesta == v)).unwrap_or(0);
        self.snimky.clear();
        self.nahlady.clear();
        self.oznaceny = None;
        self.miery = None;
        self.cakajuce.clear();
        self.priecinok = Some(priecinok);
        self.prisposob();
        self.naplanuj();
    }

    fn prisposob(&mut self) {
        self.priblizenie = None;
        self.posun = (0.0, 0.0);
        self.zmena = true;
    }

    /// Naplánuje prácu: aktuálny obraz, A pri porovnaní, susedné, porovnanie, viditeľné náhľady.
    fn naplanuj(&mut self) {
        let n = self.polozky.len();
        if n == 0 {
            return;
        }
        let mut chcem = vec![Ziadost::Snimka(self.aktualny)];
        if let Some(a) = self.oznaceny {
            chcem.push(Ziadost::Snimka(a));
        }
        if let (Some(a), true) = (self.oznaceny, self.rezim != Rezim::Jeden)
            && self.miery.as_ref().is_none_or(|m| (m.0, m.1) != (a, self.aktualny))
        {
            chcem.push(Ziadost::Porovnanie(a, self.aktualny));
        }
        chcem.push(Ziadost::Snimka((self.aktualny + 1) % n));
        chcem.push(Ziadost::Snimka((self.aktualny + n - 1) % n));
        if self.pas {
            let (od, po) = self.viditelne_nahlady();
            // od stredu pásu von
            let mut idx: Vec<usize> = (od..po).collect();
            idx.sort_by_key(|&i| (i as isize - self.aktualny as isize).abs());
            chcem.extend(idx.into_iter().map(Ziadost::Nahlad));
        }
        // zahodiť staré snímky ďaleko od aktuálnej (pamäť)
        let drz: Vec<usize> = [
            Some(self.aktualny),
            self.oznaceny,
            Some((self.aktualny + 1) % n),
            Some((self.aktualny + n - 1) % n),
        ]
        .into_iter()
        .flatten()
        .collect();
        self.snimky.retain(|k, _| drz.contains(k));
        let chcem: Vec<Ziadost> = chcem
            .into_iter()
            .filter(|z| match z {
                Ziadost::Snimka(i) => !self.snimky.contains_key(i),
                Ziadost::Nahlad(i) => !self.nahlady.contains_key(i),
                Ziadost::Porovnanie(..) => true,
            })
            .collect();
        if let Ok(mut f) = self.front.ziadosti.lock() {
            f.0.clear();
            let mut videne = std::collections::HashSet::new();
            for z in chcem {
                if videne.insert(z.clone()) {
                    f.0.push_back(z.clone());
                    self.cakajuce.insert(z);
                }
            }
        }
        self.front.signal.notify_one();
    }

    /// Spracuje hotovú prácu z pozadia; vráti, či treba prekresliť.
    pub fn spracuj_pozadie(&mut self) -> bool {
        let mut zmena = false;
        while let Ok(v) = self.vysledky.try_recv() {
            match v {
                Hotove::Snimka(i, r) => {
                    self.cakajuce.remove(&Ziadost::Snimka(i));
                    let viditelna = i == self.aktualny || Some(i) == self.oznaceny;
                    self.snimky.insert(i, r);
                    zmena |= viditelna;
                }
                Hotove::Nahlad(i, n) => {
                    self.cakajuce.remove(&Ziadost::Nahlad(i));
                    self.nahlady.insert(i, n);
                    while self.nahlady.len() > 1024 {
                        let k = *self
                            .nahlady
                            .keys()
                            .max_by_key(|&&k| (k as isize - self.aktualny as isize).abs())
                            .unwrap();
                        self.nahlady.remove(&k);
                    }
                    zmena |= self.pas;
                }
                Hotove::Porovnanie(a, b, r) => {
                    self.cakajuce.remove(&Ziadost::Porovnanie(a, b));
                    self.miery = Some((a, b, r));
                    zmena |= self.rezim != Rezim::Jeden;
                }
            }
        }
        self.zmena |= zmena;
        zmena
    }

    /// Čaká sa ešte na prácu na pozadí?
    pub fn pracuje(&self) -> bool {
        !self.cakajuce.is_empty()
    }

    /// Treba prekresliť?
    pub fn potrebuje_prekreslit(&self) -> bool {
        self.zmena
    }

    fn plocha(&self) -> (usize, usize) {
        let pas = if self.pas && self.polozky.len() > 1 { PAS } else { 0 };
        let udaje = if self.udaje { 24 } else { 0 };
        (self.sirka, self.vyska.saturating_sub(pas + udaje).max(1))
    }

    fn viditelne_nahlady(&self) -> (usize, usize) {
        let n = self.polozky.len();
        let vidno = (self.sirka / PAS).max(1) + 2;
        let od = self.aktualny.saturating_sub(vidno / 2);
        (od, (od + vidno).min(n))
    }

    fn mierka(&self, s: &Snimka) -> f32 {
        let (w, h) = self.plocha();
        self.priblizenie
            .unwrap_or_else(|| (w as f32 / s.sirka as f32).min(h as f32 / s.vyska as f32).min(1.0))
    }

    fn aktualna(&self) -> Option<Arc<Snimka>> {
        self.snimky.get(&self.aktualny).and_then(|r| r.as_ref().ok().cloned())
    }

    /// Spracuje vstup; vráti, či treba prekresliť.
    pub fn vstup(&mut self, v: Vstup) -> bool {
        let n = self.polozky.len();
        let povodny = self.aktualny;
        match v {
            Vstup::Otvor(c) => self.otvor(&c),
            Vstup::Velkost { sirka, vyska } => {
                if (sirka, vyska) == (self.sirka, self.vyska) {
                    return false;
                }
                self.sirka = sirka.max(1);
                self.vyska = vyska.max(1);
                self.zmena = true;
                self.naplanuj();
            }
            Vstup::Povel(p) => match p {
                Povel::Dalsi if n > 0 => self.aktualny = (self.aktualny + 1) % n,
                Povel::Predosly if n > 0 => self.aktualny = (self.aktualny + n - 1) % n,
                Povel::Prvy => self.aktualny = 0,
                Povel::Posledny => self.aktualny = n.saturating_sub(1),
                Povel::Priblizit | Povel::Oddialit => {
                    let (w, h) = self.plocha();
                    let k = if p == Povel::Priblizit { 1.0 } else { -1.0 };
                    return self.vstup(Vstup::Koliesko { krokov: k, x: w as f32 / 2.0, y: h as f32 / 2.0 });
                }
                Povel::Prisposobit => self.prisposob(),
                Povel::Skutocna => {
                    self.priblizenie = Some(1.0);
                    self.posun = (0.0, 0.0);
                    self.zmena = true;
                }
                Povel::Oznac => {
                    self.oznaceny =
                        if self.oznaceny == Some(self.aktualny) { None } else { Some(self.aktualny) };
                    if self.oznaceny.is_none() {
                        self.rezim = Rezim::Jeden;
                    }
                    self.miery = None;
                    self.zmena = true;
                    self.naplanuj();
                }
                Povel::Rezim => {
                    self.rezim = match (self.rezim, self.oznaceny) {
                        (_, None) => Rezim::Jeden,
                        (Rezim::Jeden, _) => Rezim::Deleny,
                        (Rezim::Deleny, _) => Rezim::Rozdiel,
                        (Rezim::Rozdiel, _) => Rezim::Jeden,
                    };
                    self.zmena = true;
                    self.naplanuj();
                }
                Povel::Pas => {
                    self.pas = !self.pas;
                    self.zmena = true;
                    self.naplanuj();
                }
                Povel::Udaje => {
                    self.udaje = !self.udaje;
                    self.zmena = true;
                }
                _ => {}
            },
            Vstup::Koliesko { krokov, x, y } => {
                let Some(s) = self.aktualna() else { return false };
                let stara = self.mierka(&s);
                let nova = (stara * 1.25f32.powf(krokov)).clamp(1.0 / 64.0, 64.0);
                // bod pod kurzorom ostane na mieste
                let (w, h) = self.plocha();
                let (cx, cy) = (w as f32 / 2.0 + self.posun.0, h as f32 / 2.0 + self.posun.1);
                let k = nova / stara;
                self.posun = (x - (x - cx) * k - w as f32 / 2.0, y - (y - cy) * k - h as f32 / 2.0);
                self.priblizenie = Some(nova);
                self.zmena = true;
            }
            Vstup::Tahaj { dx, dy } => {
                if dx == 0.0 && dy == 0.0 {
                    return false;
                }
                self.posun.0 += dx;
                self.posun.1 += dy;
                self.zmena = true;
            }
            Vstup::Delic { x } => {
                let d = (x / self.sirka as f32).clamp(0.0, 1.0);
                if self.rezim != Rezim::Deleny || d == self.delic {
                    return false;
                }
                self.delic = d;
                self.zmena = true;
            }
            Vstup::Klik { x, y } => {
                let (_, h) = self.plocha();
                if self.pas && y >= (h + if self.udaje { 24 } else { 0 }) as f32 {
                    let (od, po) = self.viditelne_nahlady();
                    let (pod, _) = self.pozicia_pasu(od);
                    let i = od as isize + ((x - pod) / PAS as f32).floor() as isize;
                    if i >= od as isize && (i as usize) < po {
                        self.aktualny = i as usize;
                    }
                }
            }
        }
        if self.aktualny != povodny {
            if self.priblizenie.is_some() && self.rezim == Rezim::Jeden {
                self.prisposob();
            }
            self.miery = None;
            self.zmena = true;
            self.naplanuj();
        }
        self.zmena
    }

    fn pozicia_pasu(&self, od: usize) -> (f32, f32) {
        // aktuálny náhľad v strede pásu
        let stred = self.sirka as f32 / 2.0 - PAS as f32 / 2.0;
        (stred - (self.aktualny - od) as f32 * PAS as f32, 0.0)
    }

    /// Titulok okna.
    pub fn titulok(&self) -> String {
        match self.polozky.get(self.aktualny) {
            Some(p) => format!(
                "{} – Pohľad ({}/{})",
                p.cesta.file_name().map(|m| m.to_string_lossy().into_owned()).unwrap_or_default(),
                self.aktualny + 1,
                self.polozky.len()
            ),
            None => "Pohľad – pretiahnite sem obrázok alebo priečinok".into(),
        }
    }

    /// Vykreslí stav do plátna (veľkosť plátna = veľkosť okna).
    pub fn vykresli(&mut self, p: &mut Platno) {
        self.vykresleni += 1;
        self.zmena = false;
        if p.sirka != self.sirka || p.vyska != self.vyska {
            *p = Platno::nove(self.sirka, self.vyska, POZADIE);
        } else {
            p.body.fill(POZADIE);
        }
        let (w, h) = self.plocha();
        let b = self.aktualna();
        let a = self.oznaceny.and_then(|i| self.snimky.get(&i)).and_then(|r| r.as_ref().ok().cloned());
        if let Some(b) = &b {
            let z = self.mierka(b);
            // ľavý horný roh obrazu na obrazovke
            let ox = w as f32 / 2.0 + self.posun.0 - b.sirka as f32 * z / 2.0;
            let oy = h as f32 / 2.0 + self.posun.1 - b.vyska as f32 * z / 2.0;
            let delic = (self.delic * w as f32) as usize;
            let rezim = if a.is_some() { self.rezim } else { Rezim::Jeden };
            let (a_ref, b_ref) = (a.as_deref(), b.as_ref());
            paralel::pre_kusy(&mut p.body[..w * h], w, |y, riadok| {
                let iy = (y as f32 + 0.5 - oy) / z;
                for (x, o) in riadok.iter_mut().enumerate() {
                    let ix = (x as f32 + 0.5 - ox) / z;
                    if ix < 0.0 || iy < 0.0 || ix >= b_ref.sirka as f32 || iy >= b_ref.vyska as f32 {
                        continue;
                    }
                    // A sa mapuje na rozmer B (rôzne veľké obrazy sa porovnajú v rovnakej polohe)
                    let z_a = |s: &Snimka| {
                        s.bod(
                            ix * s.sirka as f32 / b_ref.sirka as f32,
                            iy * s.vyska as f32 / b_ref.vyska as f32,
                            z,
                        )
                    };
                    let bod = match (rezim, a_ref) {
                        (Rezim::Deleny, Some(sa)) if x < delic => z_a(sa),
                        (Rezim::Rozdiel, Some(sa)) => {
                            let (u, v) = (z_a(sa), b_ref.bod(ix, iy, z));
                            let r =
                                |c: usize| ((u[c] as i32 - v[c] as i32).unsigned_abs() * 8).min(255) as u8;
                            [r(0), r(1), r(2), 255]
                        }
                        _ => b_ref.bod(ix, iy, z),
                    };
                    *o = na_pozadi(bod, x, y);
                }
            });
            if rezim == Rezim::Deleny {
                p.vypln(delic as i32, 0, 1, h as i32, ZVYRAZNENIE);
            }
        } else if let Some(Err(e)) = self.snimky.get(&self.aktualny) {
            let t = format!("Obrázok sa nedá zobraziť: {e}");
            p.text(&self.pismo, &t, 16.0, h as f32 / 2.0, 15.0, 1.0, TEXT);
        } else if self.polozky.is_empty() {
            let t = "Pretiahnite sem obrázok alebo priečinok";
            let x = (w as f32 - self.pismo.sirka(t, 18.0, 1.0)) / 2.0;
            p.text(&self.pismo, t, x, h as f32 / 2.0, 18.0, 1.0, TEXT);
        }
        // riadok s údajmi
        let mut y = h;
        if self.udaje {
            p.vypln(0, y as i32, self.sirka as i32, 24, 0x26282B);
            let t = self.text_udajov(b.as_deref(), a.as_deref());
            let t = self.pismo.skrat(&t, self.sirka as f32 - 16.0, 13.0, 1.0);
            p.text(&self.pismo, &t, 8.0, y as f32 + 17.0, 13.0, 1.0, TEXT);
            y += 24;
        }
        // pás náhľadov
        if self.pas && self.polozky.len() > 1 {
            let (od, po) = self.viditelne_nahlady();
            let (x0, _) = self.pozicia_pasu(od);
            for i in od..po {
                let x = x0 + (i - od) as f32 * PAS as f32;
                if i == self.aktualny || Some(i) == self.oznaceny {
                    let f = if i == self.aktualny { ZVYRAZNENIE } else { 0xE0A040 };
                    p.vypln(x as i32 + 2, y as i32 + 2, PAS as i32 - 4, PAS as i32 - 4, f);
                }
                if let Some(Some(n)) = self.nahlady.get(&i) {
                    let (nx, ny) =
                        (x as i32 + (PAS - n.sirka) as i32 / 2, y as i32 + (PAS - n.vyska) as i32 / 2);
                    let d = &n.urovne[0].2;
                    for r in 0..n.vyska {
                        for s in 0..n.sirka {
                            let (px, py) = (nx + s as i32, ny + r as i32);
                            if px >= 0 && (px as usize) < p.sirka && py >= 0 && (py as usize) < p.vyska {
                                p.body[py as usize * p.sirka + px as usize] =
                                    na_pozadi(d[r * n.sirka + s], px as usize, py as usize);
                            }
                        }
                    }
                }
            }
        }
    }

    fn text_udajov(&self, b: Option<&Snimka>, a: Option<&Snimka>) -> String {
        let meno = |i: usize| {
            self.polozky
                .get(i)
                .and_then(|p| p.cesta.file_name())
                .map(|m| m.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut t = String::new();
        if let Some(b) = b {
            t = format!("{} · {} · {:.0} %", meno(self.aktualny), b.popis, self.mierka(b) * 100.0);
        }
        if let (Some(ia), Some(b)) = (self.oznaceny, a.and(b)) {
            // písmo STONKA nemá „|“ ani „−“ – použijú sa „/“ a „-“
            let rezim = match self.rezim {
                Rezim::Jeden => "A označený (C = porovnať)",
                Rezim::Deleny => "delený A / B",
                Rezim::Rozdiel => "rozdiel A-B × 8",
            };
            t = format!(
                "A: {} · B: {} · {:.0} % · {rezim}",
                meno(ia),
                meno(self.aktualny),
                self.mierka(b) * 100.0
            );
            match &self.miery {
                Some((x, y, Ok(m))) if (*x, *y) == (ia, self.aktualny) => {
                    let ssim = m.ssim.map_or("–".into(), |s| format!("{s:.4}"));
                    let psnr = if m.psnr.is_finite() { format!("{:.2} dB", m.psnr) } else { "zhoda".into() };
                    t += &format!(" · PSNR {psnr} · SSIM {ssim} · najviac {}", m.najviac);
                }
                Some((x, y, Err(e))) if (*x, *y) == (ia, self.aktualny) => t += &format!(" · {e}"),
                _ if self.rezim != Rezim::Jeden => t += " · počítam…",
                _ => {}
            }
        }
        if self.polozky.len() > 1 {
            t += &format!(" · {}/{}", self.aktualny + 1, self.polozky.len());
        }
        t
    }

    /// Počet obrázkov v priečinku.
    pub fn pocet(&self) -> usize {
        self.polozky.len()
    }
}

/// Zloží bod s alfou na šachovnicu (8 × 8).
fn na_pozadi(b: [u8; 4], x: usize, y: usize) -> Farba {
    let s = if ((x / 8) + (y / 8)).is_multiple_of(2) { 0x3A } else { 0x2E };
    let a = b[3] as u32;
    let c = |v: u8| (v as u32 * a + s * (255 - a) + 127) / 255;
    (c(b[0]) << 16) | (c(b[1]) << 8) | c(b[2])
}
