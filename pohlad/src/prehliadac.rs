//! Jadro okna prehliadača a editora fotografií (Pohľad) – nezávislé od systému okien, bez `unsafe`.
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

use crate::herbar::{self, Herbar, Zaznam, ZmenaFotky};
use crate::nahlady::{self, Polozka};
use crate::orezanie::{Orezanie, Pomer, Uchop};
use crate::panel::{self, PrvokMeta};
use crate::porovnanie;
use crate::upravy::{self, Historia, Uprava};
use orchidea_formaty::jpeg::bezstratovo::{self, Mriezka, Transformacia};
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
    /// Úpravy (história späť / znova, uloženie vedľa pôvodného súboru).
    OtocVpravo,
    OtocVlavo,
    Zrkadlo,
    /// Orezať na zobrazený výrez.
    Orezat,
    /// Režim orezania: pomer strán (znova ten istý = na výšku / na šírku).
    OrezPomer(Pomer),
    /// Režim orezania: vymeniť na šírku / na výšku.
    OrezOtocPomer,
    /// Režim orezania: orezať na výber (Enter).
    OrezPotvrd,
    /// Režim orezania: zrušiť (Esc).
    OrezZrus,
    JasViac,
    JasMenej,
    KontrastViac,
    KontrastMenej,
    GamaViac,
    GamaMenej,
    /// Kvalitné zmenšenie na polovicu.
    Zmensit,
    Spat,
    Znova,
    Ulozit,
    /// Prezentácia na celú obrazovku (zapnúť / vypnúť).
    Prezentacia,
    /// Prezentácia: pozastaviť / pokračovať v automatickom posune.
    Pauza,
    /// Nápoveda klávesov (zapnúť / vypnúť).
    Napoveda,
    /// Ukázať / skryť panel nástrojov.
    Panel,
    /// Výber v mriežke katalógu o riadok hore / dole.
    Hore,
    Dole,
    /// Katalóg fotografií (Herbár): otvoriť / zavrieť.
    Katalog,
    /// Pridať otvorený priečinok do katalógu.
    KatalogPridaj,
    /// Obnoviť katalóg (nové a zmenené súbory).
    KatalogObnov,
    /// Otvoriť obrázok dialógom okna.
    Otvorit,
    /// Hodnotenie aktuálnej fotky (0 = bez hodnotenia, 1–5) – zapíše sa do súboru XMP vedľa fotky.
    Hodnotenie(i8),
    /// Pole na nové kľúčové slovo.
    PridajKluc,
}

/// Žiadosť jadra na okno – systémový dialóg (jadro ho nevie ukázať samo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZiadostOkna {
    /// Dialóg „Otvoriť“ – vybraný súbor sa pošle ako `Vstup::Otvor` (otvorí sa celý jeho priečinok).
    OtvorSubor { priecinok: Option<PathBuf> },
    /// Výber priečinka do katalógu – vybraný priečinok sa pošle ako `Vstup::Otvor` (v katalógu sa pridá).
    VyberPriecinok { priecinok: Option<PathBuf> },
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
    /// Posun priblíženého obrazu ťahaním pravým (`pravym`) alebo stredným tlačidlom. Ťahanie ľavým tlačidlom
    /// po obraze kreslí výber na orezanie (pri porovnaní a v prezentácii posúva).
    Posun {
        dx: f32,
        dy: f32,
        pravym: bool,
    },
    /// Otvoriť súbor alebo priečinok.
    Otvor(PathBuf),
    /// Časovač prezentácie (okno ho pošle len počas prezentácie).
    Tik,
    /// Napísaný znak (len keď `pise()` – pole hľadania katalógu); `\u{8}` zmaže, `\r` otvorí vybranú fotku.
    Znak(char),
    /// Pohyb myši bez stlačeného tlačidla (zvýraznenie tlačidla panela; prekreslí sa len pri zmene).
    Mys {
        x: f32,
        y: f32,
    },
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
    /// Náhľad úprav (obraz, generácia úprav).
    Uprav(usize, u64),
    /// Uloženie úprav (obraz, generácia úprav).
    Uloz(usize, u64),
    /// Práca s katalógom fotografií.
    Katalog(UlohaKatalogu),
    /// Hodnotenie a kľúčové slová fotky (fotka + súbory XMP vedľa nej).
    Meta(PathBuf),
    /// Zápis do súboru XMP vedľa fotky.
    ZapisXmp(PathBuf, ZmenaFotky),
}

/// Práca s katalógom na pozadí (súbor katalógu, ...).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum UlohaKatalogu {
    Nacitaj(PathBuf),
    Pridaj(PathBuf, PathBuf),
    Obnov(PathBuf),
}

enum Hotove {
    /// Výsledky nesú cestu – po zmene zoznamu sa staré výsledky zahodia.
    Snimka(usize, PathBuf, Result<Arc<Snimka>, String>),
    Nahlad(usize, PathBuf, Option<Arc<Snimka>>),
    Katalog(UlohaKatalogu, Result<(Herbar, Option<String>), String>),
    Meta(PathBuf, Result<Zaznam, String>),
    Zapisane(PathBuf, ZmenaFotky, Result<Zaznam, String>),
    Porovnanie(usize, usize, Result<porovnanie::Miery, String>),
    Upravena(usize, u64, Result<Arc<Snimka>, String>),
    Ulozene(usize, u64, Result<upravy::Ulozene, String>),
}

/// Front práce pracovného vlákna (nové žiadosti nahradia staré – bez zbytočnej práce).
struct Front {
    ziadosti: Mutex<(VecDeque<Ziadost>, bool)>,
    signal: Condvar,
}

/// Rozmery pásu náhľadov.
const PAS: usize = 96;
/// Riadok hľadania a dlaždice mriežky katalógu.
const HLADANIE: usize = 40;
const DLAZDICA_W: usize = 112;
const DLAZDICA_H: usize = 122;
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
    /// História úprav každého obrazu.
    historie: HashMap<usize, Historia>,
    /// Upravené zobrazenie (obraz → generácia, snímka).
    upravene: HashMap<usize, (u64, Arc<Snimka>)>,
    /// Kroky úprav podľa generácie (zdieľané s pracovným vláknom).
    ulohy: Arc<Mutex<HashMap<u64, Vec<Uprava>>>>,
    generacia: u64,
    posledna_uprava: HashMap<usize, u64>,
    /// Krátka správa v riadku s údajmi (uloženie, chyba úpravy).
    sprava: Option<String>,
    /// Prezentácia: (interval v ms, pozastavená).
    prezentacia: Option<(u32, bool)>,
    panel: bool,
    napoveda: bool,
    /// Tlačidlo panela pod kurzorom.
    pod_mysou: Option<&'static str>,
    /// Kliknutie začalo na paneli – ťahanie neposúva obraz.
    klik_v_paneli: bool,
    /// Režim orezania výberom (tlačidlo Orezať, X).
    orezanie: Option<Orezanie>,
    /// Poloha kurzora pri ťahaní výberu (body okna).
    orez_kurzor: (f32, f32),
    /// Ľavé tlačidlo stlačené na obraze: odtiaľ sa ťahaním nakreslí nový výber (body okna).
    vyber_od: Option<(f32, f32)>,
    /// Súbor katalógu fotografií (Herbár).
    katalog_subor: Option<PathBuf>,
    herbar: Option<Herbar>,
    /// Zobrazená mriežka katalógu s hľadaním.
    katalog: bool,
    dotaz: String,
    /// Čísla záznamov katalógu pre položky (keď zoznam pochádza z katalógu).
    zaznamy_vysledkov: Vec<usize>,
    prvy_riadok: usize,
    /// Zoznam priečinka pred otvorením katalógu (položky, aktuálny, priečinok).
    predosle: Option<(Vec<Polozka>, usize, Option<PathBuf>)>,
    katalog_sprava: Option<String>,
    /// Okno vie ukázať systémové dialógy.
    dialogy: bool,
    /// Hodnotenie a kľúčové slová fotiek (fotka + súbory XMP vedľa nej), podľa cesty.
    meta: HashMap<PathBuf, Zaznam>,
    /// Chyba posledného zápisu do súboru XMP.
    chyba_xmp: Option<String>,
    /// Píše sa nové kľúčové slovo.
    kluc_vstup: Option<String>,
    pod_mysou_meta: Option<PrvokMeta>,
    /// Katalóg v pamäti sa zmenil (uloží sa pri zatvorení katalógu alebo okna).
    katalog_zmeneny: bool,
    ziadost_okna: Option<ZiadostOkna>,
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
    ulohy: Arc<Mutex<HashMap<u64, Vec<Uprava>>>>,
    tx: Sender<Hotove>,
    zobud: Arc<dyn Fn() + Send + Sync>,
) {
    let limity = Limity::default();
    let cesta_c = |i: usize| polozky.lock().ok().and_then(|p| p.get(i).cloned()).unwrap_or_default();
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
                let c = cesta_c(i);
                let r = snimka(i);
                if let Ok(s) = &r {
                    posledne.insert(i, s.clone());
                    if posledne.len() > 4 {
                        let k = *posledne.keys().find(|&&k| k != i).unwrap();
                        posledne.remove(&k);
                    }
                }
                Hotove::Snimka(i, c, r)
            }
            Ziadost::Nahlad(i) => {
                let c = cesta_c(i);
                let n = nahlady::nahlad(&c, NAHLAD, &limity)
                    .ok()
                    .and_then(|o| Snimka::z_obrazu(o, String::new()).ok())
                    .map(Arc::new);
                Hotove::Nahlad(i, c, n)
            }
            Ziadost::Katalog(u) => {
                let r = (|| -> orchidea_jadro::Vysledok<(Herbar, Option<String>)> {
                    let (subor, pridaj, obnov) = match &u {
                        UlohaKatalogu::Nacitaj(s) => (s, None, false),
                        UlohaKatalogu::Pridaj(s, p) => (s, Some(p), true),
                        UlohaKatalogu::Obnov(s) => (s, None, true),
                    };
                    let mut h = Herbar::nacitaj(subor)?;
                    if let Some(p) = pridaj {
                        h.pridaj_priecinok(p, true);
                    }
                    if !obnov {
                        return Ok((h, None));
                    }
                    let (n, p, z) = h.obnov(0, &limity);
                    h.uloz(subor)?;
                    Ok((h, Some(format!("prečítaných {n}, bez zmeny {p}, vynechaných {z}"))))
                })()
                .map_err(|e| e.to_string());
                Hotove::Katalog(u, r)
            }
            Ziadost::Porovnanie(a, b) => {
                let ziskaj = |i: usize| posledne.get(&i).cloned().map(Ok).unwrap_or_else(|| snimka(i));
                let r = match (ziskaj(a), ziskaj(b)) {
                    (Ok(x), Ok(y)) => porovnanie::porovnaj(&x.obraz, &y.obraz).map_err(|e| e.to_string()),
                    (Err(e), _) | (_, Err(e)) => Err(e),
                };
                Hotove::Porovnanie(a, b, r)
            }
            Ziadost::Uprav(i, g) => {
                let kroky = ulohy.lock().ok().and_then(|u| u.get(&g).cloned()).unwrap_or_default();
                let zaklad = posledne.get(&i).cloned().map(Ok).unwrap_or_else(|| snimka(i));
                let r = zaklad.and_then(|z| {
                    let o = upravy::aplikuj_vsetky(&z.obraz, &kroky).map_err(|e| e.to_string())?;
                    let popis = format!("{} × {} · upravené", o.sirka, o.vyska);
                    Snimka::z_obrazu(o, popis).map(Arc::new).map_err(|e| e.to_string())
                });
                Hotove::Upravena(i, g, r)
            }
            Ziadost::Uloz(i, g) => {
                let kroky = ulohy.lock().ok().and_then(|u| u.get(&g).cloned()).unwrap_or_default();
                let cesta = polozky.lock().ok().and_then(|p| p.get(i).cloned());
                let r = match cesta {
                    Some(c) => upravy::uloz(&c, &kroky, &limity).map_err(|e| e.to_string()),
                    None => Err("mimo zoznamu".into()),
                };
                Hotove::Ulozene(i, g, r)
            }
            Ziadost::Meta(c) => {
                let r = herbar::zaznam_suboru(&c, &limity).map_err(|e| e.to_string());
                Hotove::Meta(c, r)
            }
            Ziadost::ZapisXmp(c, z) => {
                let r = herbar::zapis_xmp(&c, &z, &limity).map_err(|e| e.to_string());
                Hotove::Zapisane(c, z, r)
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
        let ulohy = Arc::new(Mutex::new(HashMap::new()));
        let f = front.clone();
        let p = polozky.clone();
        let u = ulohy.clone();
        std::thread::spawn(move || pracuj(f, p, u, tx, zobud));
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
            historie: HashMap::new(),
            upravene: HashMap::new(),
            ulohy,
            generacia: 0,
            posledna_uprava: HashMap::new(),
            sprava: None,
            prezentacia: None,
            panel: true,
            napoveda: false,
            pod_mysou: None,
            klik_v_paneli: false,
            orezanie: None,
            orez_kurzor: (0.0, 0.0),
            vyber_od: None,
            katalog_subor: None,
            herbar: None,
            katalog: false,
            dotaz: String::new(),
            zaznamy_vysledkov: Vec::new(),
            prvy_riadok: 0,
            predosle: None,
            katalog_sprava: None,
            dialogy: false,
            ziadost_okna: None,
            meta: HashMap::new(),
            chyba_xmp: None,
            kluc_vstup: None,
            pod_mysou_meta: None,
            katalog_zmeneny: false,
        }
    }

    /// Otvorí súbor (a jeho priečinok) alebo priečinok.
    pub fn otvor(&mut self, cesta: &Path) {
        // relatívna cesta („foto.jpg“) by mala prázdny priečinok – vždy úplná cesta
        let cesta = &std::path::absolute(cesta).unwrap_or_else(|_| cesta.to_path_buf());
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
        self.historie.clear();
        self.upravene.clear();
        self.sprava = None;
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
        if self.katalog {
            // mriežka katalógu: len náhľady viditeľných dlaždíc (zhora)
            let (_, _, stlpcov, riadkov) = self.mriezka();
            let od = self.prvy_riadok * stlpcov;
            let po = (od + stlpcov * (riadkov + 1)).min(n);
            let chcem: Vec<Ziadost> =
                (od..po).filter(|i| !self.nahlady.contains_key(i)).map(Ziadost::Nahlad).collect();
            if let Ok(mut f) = self.front.ziadosti.lock() {
                f.0.retain(|z| {
                    matches!(
                        z,
                        Ziadost::Uprav(..) | Ziadost::Uloz(..) | Ziadost::Katalog(..) | Ziadost::ZapisXmp(..)
                    )
                });
                self.cakajuce.retain(|z| f.0.contains(z));
                for z in chcem {
                    f.0.push_back(z.clone());
                    self.cakajuce.insert(z);
                }
            }
            self.front.signal.notify_one();
            return;
        }
        let mut chcem = vec![Ziadost::Snimka(self.aktualny)];
        if let Some(p) = self.polozky.get(self.aktualny) {
            chcem.push(Ziadost::Meta(p.cesta.clone()));
        }
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
        if self.pas_viditelny() {
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
        self.upravene.retain(|k, _| drz.contains(k));
        let chcem: Vec<Ziadost> = chcem
            .into_iter()
            .filter(|z| match z {
                Ziadost::Snimka(i) => !self.snimky.contains_key(i),
                Ziadost::Nahlad(i) => !self.nahlady.contains_key(i),
                Ziadost::Meta(c) => !self.meta.contains_key(c),
                Ziadost::Porovnanie(..)
                | Ziadost::Uprav(..)
                | Ziadost::Uloz(..)
                | Ziadost::Katalog(..)
                | Ziadost::ZapisXmp(..) => true,
            })
            .collect();
        if let Ok(mut f) = self.front.ziadosti.lock() {
            // úpravy a uloženie sa nezahadzujú
            f.0.retain(|z| {
                matches!(
                    z,
                    Ziadost::Uprav(..) | Ziadost::Uloz(..) | Ziadost::Katalog(..) | Ziadost::ZapisXmp(..)
                )
            });
            self.cakajuce.retain(|z| f.0.contains(z));
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
                Hotove::Snimka(i, c, r) => {
                    self.cakajuce.remove(&Ziadost::Snimka(i));
                    if self.polozky.get(i).map(|p| &p.cesta) != Some(&c) {
                        continue;
                    }
                    let viditelna = i == self.aktualny || Some(i) == self.oznaceny;
                    self.snimky.insert(i, r);
                    zmena |= viditelna;
                }
                Hotove::Nahlad(i, c, n) => {
                    self.cakajuce.remove(&Ziadost::Nahlad(i));
                    if self.polozky.get(i).map(|p| &p.cesta) != Some(&c) {
                        continue;
                    }
                    self.nahlady.insert(i, n);
                    let stred =
                        if self.katalog { self.prvy_riadok * self.mriezka().2 } else { self.aktualny };
                    while self.nahlady.len() > 1024 {
                        let k = *self
                            .nahlady
                            .keys()
                            .max_by_key(|&&k| (k as isize - stred as isize).abs())
                            .unwrap();
                        self.nahlady.remove(&k);
                    }
                    zmena |= self.pas || self.katalog;
                }
                Hotove::Katalog(u, r) => {
                    let nacitane = matches!(u, UlohaKatalogu::Nacitaj(_));
                    self.cakajuce.remove(&Ziadost::Katalog(u));
                    match r {
                        Ok((h, sprava)) => {
                            // po načítaní hneď obnova na pozadí (len zmenené fotky a súbory XMP, aj od iných programov)
                            if nacitane
                                && !h.priecinky.is_empty()
                                && let Some(s) = self.katalog_subor.clone()
                            {
                                self.posli(Ziadost::Katalog(UlohaKatalogu::Obnov(s)));
                            } else {
                                self.katalog_zmeneny = false;
                            }
                            self.herbar = Some(h);
                            self.katalog_sprava = sprava;
                            if self.katalog {
                                self.hladaj_v_katalogu();
                            }
                        }
                        Err(e) => self.katalog_sprava = Some(format!("katalóg: {e}")),
                    }
                    zmena = true;
                }
                Hotove::Upravena(i, g, r) => {
                    self.cakajuce.remove(&Ziadost::Uprav(i, g));
                    if let Ok(mut u) = self.ulohy.lock() {
                        u.remove(&g);
                    }
                    // platí len najnovšia generácia úprav daného obrazu
                    let aktivna = self.historie.get(&i).is_some_and(|h| !h.aktivne().is_empty());
                    if aktivna && Some(&g) == self.posledna_uprava.get(&i) {
                        match r {
                            Ok(s) => {
                                self.upravene.insert(i, (g, s));
                            }
                            Err(e) => self.sprava = Some(format!("úprava zlyhala: {e}")),
                        }
                        zmena |= i == self.aktualny;
                    }
                }
                Hotove::Ulozene(i, g, r) => {
                    self.cakajuce.remove(&Ziadost::Uloz(i, g));
                    if let Ok(mut u) = self.ulohy.lock() {
                        u.remove(&g);
                    }
                    self.sprava = Some(match r {
                        Ok(u) => format!(
                            "uložené{}: {}",
                            if u.bezstratovo { " bezstratovo" } else { "" },
                            u.cesta.file_name().map(|m| m.to_string_lossy().into_owned()).unwrap_or_default()
                        ),
                        Err(e) => format!("uloženie zlyhalo: {e}"),
                    });
                    zmena = true;
                }
                Hotove::Meta(c, r) => {
                    self.cakajuce.remove(&Ziadost::Meta(c.clone()));
                    if let Ok(z) = r {
                        // čakajúci zápis tej istej fotky má prednosť (zobrazenie ostane podľa zmeny)
                        if !self.cakajuce.iter().any(|q| matches!(q, Ziadost::ZapisXmp(d, _) if *d == c)) {
                            self.meta.insert(c.clone(), z);
                        }
                    }
                    zmena |= self.polozky.get(self.aktualny).is_some_and(|p| p.cesta == c);
                }
                Hotove::Zapisane(c, zm, r) => {
                    self.cakajuce.remove(&Ziadost::ZapisXmp(c.clone(), zm));
                    match r {
                        Ok(z) => {
                            if let Some(h) = &mut self.herbar
                                && h.aktualizuj(z.clone())
                            {
                                self.katalog_zmeneny = true;
                            }
                            if !self.cakajuce.iter().any(|q| matches!(q, Ziadost::ZapisXmp(d, _) if *d == c))
                            {
                                self.meta.insert(c.clone(), z);
                            }
                        }
                        Err(e) => {
                            self.chyba_xmp = Some(format!("nezapísané: {e}"));
                            self.meta.remove(&c);
                            self.posli(Ziadost::Meta(c.clone()));
                        }
                    }
                    zmena = true;
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

    /// Interval časovača prezentácie v ms, ak beží a nie je pozastavená (okno podľa neho nastaví časovač –
    /// mimo prezentácie žiadny časovač nebeží).
    pub fn casovac(&self) -> Option<u32> {
        self.prezentacia.filter(|p| !p.1 && self.polozky.len() > 1).map(|p| p.0)
    }

    /// Beží prezentácia (okno na celú obrazovku)?
    pub fn prezentuje(&self) -> bool {
        self.prezentacia.is_some()
    }

    fn pas_viditelny(&self) -> bool {
        self.pas && self.polozky.len() > 1 && self.prezentacia.is_none()
    }

    fn udaje_viditelne(&self) -> bool {
        self.udaje && self.prezentacia.is_none()
    }

    fn panel_viditelny(&self) -> bool {
        self.panel && self.prezentacia.is_none()
    }

    /// Výška panela nástrojov nad obrazom.
    fn hore(&self) -> usize {
        if self.panel_viditelny() { panel::VYSKA.min(self.vyska.saturating_sub(1)) } else { 0 }
    }

    fn plocha(&self) -> (usize, usize) {
        let pas = if self.pas_viditelny() { PAS } else { 0 };
        let udaje = if self.udaje_viditelne() { 24 } else { 0 };
        let meta = if self.meta_viditelna() && !self.katalog { panel::VYSKA_META } else { 0 };
        (self.sirka, self.vyska.saturating_sub(pas + udaje + meta + self.hore()).max(1))
    }

    /// Riadok hodnotenia a kľúčových slov je vidieť (s panelom nástrojov, keď je vybraná fotka).
    fn meta_viditelna(&self) -> bool {
        self.panel_viditelny() && !self.polozky.is_empty()
    }

    /// Výška, na ktorej začína riadok hodnotenia.
    fn y_meta(&self) -> usize {
        if self.katalog {
            let udaje = if self.udaje_viditelne() { 24 } else { 0 };
            self.vyska.saturating_sub(udaje + panel::VYSKA_META)
        } else {
            self.hore() + self.plocha().1
        }
    }

    fn v_meta(&self, y: f32) -> bool {
        let y0 = self.y_meta() as f32;
        self.meta_viditelna() && y >= y0 && y < y0 + panel::VYSKA_META as f32
    }

    /// Hodnotenie a kľúčové slová vybranej fotky (`None`, kým sa nenačítajú).
    pub fn hodnotenie_a_kluce(&self) -> Option<(Option<i8>, Vec<String>)> {
        let p = self.polozky.get(self.aktualny)?;
        if let Some(z) = self.meta.get(&p.cesta) {
            return Some((z.hodnotenie, z.kluce.clone()));
        }
        if self.katalog
            && let (Some(h), Some(&i)) = (&self.herbar, self.zaznamy_vysledkov.get(self.aktualny))
            && let Some(z) = h.zaznamy.get(i)
            && z.cesta == p.cesta
        {
            return Some((z.hodnotenie, z.kluce.clone()));
        }
        None
    }

    /// Kľúčové slová na zobrazenie (vnútorné značky darktable „darktable|…“ sa neukazujú, ale ostávajú v súbore).
    fn zobrazene_kluce(&self) -> Vec<String> {
        self.hodnotenie_a_kluce()
            .map(|(_, k)| k.into_iter().filter(|k| !k.starts_with("darktable|")).collect())
            .unwrap_or_default()
    }

    fn rozlozenie_meta(&self) -> Vec<panel::UmiestnenieMeta> {
        if !self.meta_viditelna() {
            return Vec::new();
        }
        panel::rozloz_meta(&self.pismo, self.sirka, &self.zobrazene_kluce(), self.kluc_vstup.as_deref())
    }

    /// Stred prvku riadku hodnotenia (na skúšky a samotest okna).
    pub fn prvok_meta(&self, prvok: PrvokMeta) -> Option<(f32, f32)> {
        let y = self.y_meta() as f32 + panel::VYSKA_META as f32 / 2.0;
        self.rozlozenie_meta().iter().find(|u| u.prvok == prvok).map(|u| (u.x + u.sirka / 2.0, y))
    }

    /// Zmena hodnotenia alebo kľúčových slov vybranej fotky: hneď sa ukáže, zápis beží na pozadí.
    fn zmen(&mut self, z: ZmenaFotky) -> bool {
        let Some(c) = self.polozky.get(self.aktualny).map(|p| p.cesta.clone()) else { return false };
        let mut zaznam = self.meta.get(&c).cloned().or_else(|| {
            let h = self.herbar.as_ref()?;
            h.zaznamy.iter().find(|z| z.cesta == c).cloned()
        });
        if let Some(zz) = &mut zaznam {
            match &z {
                ZmenaFotky::Hodnotenie(h) => zz.hodnotenie = Some(*h),
                ZmenaFotky::PridajKluc(k) if !zz.kluce.contains(k) => zz.kluce.push(k.clone()),
                ZmenaFotky::PridajKluc(_) => {}
                ZmenaFotky::OdoberKluc(k) => zz.kluce.retain(|x| x != k),
            }
            self.meta.insert(c.clone(), zz.clone());
        }
        self.chyba_xmp = None;
        self.posli(Ziadost::ZapisXmp(c, z));
        self.zmena = true;
        true
    }

    /// Písanie nového kľúčového slova.
    fn pis_kluc(&mut self, c: char) -> bool {
        let Some(t) = &mut self.kluc_vstup else { return false };
        match c {
            '\u{8}' => {
                t.pop();
            }
            '\r' | '\n' => {
                let k = t.trim().to_string();
                self.kluc_vstup = None;
                self.zmena = true;
                if !k.is_empty() {
                    return self.zmen(ZmenaFotky::PridajKluc(k));
                }
            }
            c if c.is_control() => return false,
            c if t.chars().count() < 100 => t.push(c),
            _ => return false,
        }
        self.zmena = true;
        true
    }

    /// Vstupy riadku hodnotenia (`None` – vstup patrí inam).
    fn vstup_meta(&mut self, v: &Vstup) -> Option<bool> {
        match v {
            Vstup::Znak(c) if self.kluc_vstup.is_some() => Some(self.pis_kluc(*c)),
            Vstup::Povel(Povel::Hodnotenie(h)) => Some(
                self.polozky.get(self.aktualny).is_some()
                    && self.zmen(ZmenaFotky::Hodnotenie((*h).clamp(-1, 5))),
            ),
            Vstup::Povel(Povel::PridajKluc) => {
                if !self.meta_viditelna() {
                    return Some(false);
                }
                self.kluc_vstup = Some(String::new());
                self.zmena = true;
                Some(true)
            }
            Vstup::Mys { x, y } if self.v_meta(*y) => {
                let nove = panel::na_bode_meta(&self.rozlozenie_meta(), *x);
                let panel_zmena = self.pod_mysou.take().is_some();
                if nove == self.pod_mysou_meta && !panel_zmena {
                    return Some(false);
                }
                self.pod_mysou_meta = nove;
                self.zmena = true;
                Some(true)
            }
            Vstup::Klik { x, y } if self.v_meta(*y) && !self.napoveda => {
                self.klik_v_paneli = true;
                let hodnotenie = self.hodnotenie_a_kluce().and_then(|m| m.0);
                let kluce = self.zobrazene_kluce();
                match panel::na_bode_meta(&self.rozlozenie_meta(), *x) {
                    Some(PrvokMeta::Hviezda(n)) => {
                        let h = if hodnotenie == Some(n) { 0 } else { n };
                        Some(self.zmen(ZmenaFotky::Hodnotenie(h)))
                    }
                    Some(PrvokMeta::Odober(i)) => match kluce.get(i) {
                        Some(k) => Some(self.zmen(ZmenaFotky::OdoberKluc(k.clone()))),
                        None => Some(false),
                    },
                    Some(PrvokMeta::Pridaj) => {
                        self.kluc_vstup = Some(String::new());
                        self.zmena = true;
                        Some(true)
                    }
                    _ => Some(false),
                }
            }
            Vstup::Klik { .. } if self.kluc_vstup.is_some() => {
                // kliknutie mimo poľa zruší písanie a ide ďalej
                self.kluc_vstup = None;
                self.zmena = true;
                None
            }
            _ => None,
        }
    }

    /// Uloží katalóg, ak sa v pamäti zmenil (pri zatvorení katalógu alebo okna).
    pub fn pri_zatvoreni(&mut self) {
        if self.katalog_zmeneny
            && let (Some(h), Some(s)) = (&self.herbar, &self.katalog_subor)
            && h.uloz(s).is_ok()
        {
            self.katalog_zmeneny = false;
        }
    }

    fn rozlozenie(&self) -> Vec<panel::Umiestnenie> {
        let prvky = if self.katalog {
            panel::KATALOG
        } else if self.orezanie.is_some() {
            panel::OREZANIE
        } else {
            panel::PREHLIADAC
        };
        if self.panel_viditelny() { panel::rozloz(prvky, &self.pismo, self.sirka) } else { Vec::new() }
    }

    /// Nastaví súbor katalógu fotografií (Herbár); načíta sa, až keď sa katalóg otvorí.
    pub fn nastav_katalog(&mut self, cesta: PathBuf) {
        self.katalog_subor = Some(cesta);
        self.herbar = None;
        self.zmena = true;
    }

    /// Okno vie ukázať dialógy „Otvoriť“ a výber priečinka (bez nich pridá katalóg otvorený priečinok).
    pub fn povol_dialogy(&mut self) {
        self.dialogy = true;
        self.zmena = true;
    }

    /// Vezme žiadosť o dialóg (okno ho ukáže a výsledok pošle ako `Vstup::Otvor`).
    pub fn vezmi_ziadost_okna(&mut self) -> Option<ZiadostOkna> {
        self.ziadost_okna.take()
    }

    /// Píše sa do poľa hľadania (okno má posielať znaky ako `Vstup::Znak`, nie ako povely)?
    pub fn pise(&self) -> bool {
        self.katalog || self.kluc_vstup.is_some()
    }

    /// Mriežka katalógu: (x začiatku, y začiatku, stĺpcov, celých riadkov).
    fn mriezka(&self) -> (usize, usize, usize, usize) {
        let y0 = self.hore() + HLADANIE;
        let udaje = if self.udaje_viditelne() { 24 } else { 0 };
        let meta = if self.meta_viditelna() { panel::VYSKA_META } else { 0 };
        let h = self.vyska.saturating_sub(y0 + udaje + meta);
        let stlpcov = (self.sirka / DLAZDICA_W).max(1);
        let x0 = self.sirka.saturating_sub(stlpcov * DLAZDICA_W) / 2;
        (x0, y0, stlpcov, (h / DLAZDICA_H).max(1))
    }

    /// Vymení zoznam položiek; úpravy ostanú pri súboroch (podľa cesty).
    fn nastav_polozky(&mut self, nove: Vec<Polozka>, aktualny: usize) {
        let mut podla_cesty: HashMap<PathBuf, Historia> = HashMap::new();
        for (i, h) in self.historie.drain() {
            if let Some(p) = self.polozky.get(i) {
                podla_cesty.insert(p.cesta.clone(), h);
            }
        }
        self.polozky = nove;
        for (i, p) in self.polozky.iter().enumerate() {
            if let Some(h) = podla_cesty.remove(&p.cesta) {
                self.historie.insert(i, h);
            }
        }
        if let Ok(mut z) = self.zoznam_ciest.lock() {
            *z = self.polozky.iter().map(|p| p.cesta.clone()).collect();
        }
        self.aktualny = aktualny.min(self.polozky.len().saturating_sub(1));
        self.snimky.clear();
        self.nahlady.clear();
        self.upravene.clear();
        self.posledna_uprava.clear();
        self.oznaceny = None;
        self.rezim = Rezim::Jeden;
        self.miery = None;
        self.zmena = true;
    }

    /// Hľadá v katalógu podľa `dotaz` a výsledky dá do zoznamu.
    fn hladaj_v_katalogu(&mut self) {
        let Some(h) = &self.herbar else {
            self.nastav_polozky(Vec::new(), 0);
            self.zaznamy_vysledkov.clear();
            return;
        };
        let mut vysledky = Vec::new();
        let mut polozky = Vec::new();
        for i in h.hladaj_indexy(&self.dotaz) {
            if let Some(p) = h.zaznamy[i].polozka() {
                vysledky.push(i);
                polozky.push(p);
            }
        }
        // rovnaké výsledky (návrat z fotky) – výber ostane
        let rovnake = vysledky == self.zaznamy_vysledkov && polozky.len() == self.polozky.len();
        let vyber = if rovnake { self.aktualny } else { 0 };
        if !rovnake {
            self.nastav_polozky(polozky, 0);
            self.prvy_riadok = 0;
        }
        self.zaznamy_vysledkov = vysledky;
        self.aktualny = vyber;
        self.ukaz_vyber();
        self.zmena = true;
        self.naplanuj();
    }

    /// Posunie mriežku tak, aby bol vybraný záznam vidieť.
    fn ukaz_vyber(&mut self) {
        let (_, _, stlpcov, riadkov) = self.mriezka();
        let r = self.aktualny / stlpcov;
        if r < self.prvy_riadok {
            self.prvy_riadok = r;
        } else if r >= self.prvy_riadok + riadkov {
            self.prvy_riadok = r + 1 - riadkov;
        }
    }

    fn otvor_katalog(&mut self) {
        if self.predosle.is_none() {
            self.predosle = Some((self.polozky.clone(), self.aktualny, self.priecinok.clone()));
        }
        self.katalog = true;
        self.napoveda = false;
        self.prezentacia = None;
        self.pod_mysou = None;
        if self.herbar.is_none()
            && let Some(c) = self.katalog_subor.clone()
        {
            self.posli(Ziadost::Katalog(UlohaKatalogu::Nacitaj(c)));
        }
        self.hladaj_v_katalogu();
    }

    /// Zavrie katalóg a vráti zoznam priečinka spred otvorenia.
    fn zavri_katalog(&mut self) {
        self.pri_zatvoreni();
        self.katalog = false;
        self.pod_mysou = None;
        if let Some((polozky, aktualny, priecinok)) = self.predosle.take() {
            self.nastav_polozky(polozky, aktualny);
            self.priecinok = priecinok;
        }
        self.zaznamy_vysledkov.clear();
        self.prisposob();
        self.naplanuj();
    }

    /// Otvorí vybranú fotku z výsledkov (zoznam ostane – listuje sa vo výsledkoch).
    fn otvor_vysledok(&mut self, i: usize) {
        if i >= self.polozky.len() {
            return;
        }
        self.katalog = false;
        self.pod_mysou = None;
        self.aktualny = i;
        self.prisposob();
        self.naplanuj();
    }

    /// Priečinok, ktorý tlačidlo „Pridať priečinok“ pridá do katalógu.
    fn priecinok_na_pridanie(&self) -> Option<PathBuf> {
        self.predosle.as_ref().and_then(|p| p.2.clone()).or_else(|| self.priecinok.clone())
    }

    fn katalog_pracuje(&self) -> bool {
        self.cakajuce.iter().any(|z| matches!(z, Ziadost::Katalog(..)))
    }

    /// Urobí povel tlačidla panela niečo? (Neaktívne tlačidlá sú sivé.)
    fn povel_aktivny(&self, p: Povel) -> bool {
        let obraz = self.aktualna().is_some();
        let h = self.historie.get(&self.aktualny);
        match p {
            Povel::Katalog => self.katalog_subor.is_some(),
            Povel::KatalogPridaj => {
                self.katalog_subor.is_some()
                    && !self.katalog_pracuje()
                    && (self.dialogy || self.priecinok_na_pridanie().is_some())
            }
            Povel::Otvorit => self.dialogy,
            Povel::Hodnotenie(_) | Povel::PridajKluc => obraz || (self.katalog && !self.polozky.is_empty()),
            Povel::KatalogObnov => {
                !self.katalog_pracuje() && self.herbar.as_ref().is_some_and(|h| !h.priecinky.is_empty())
            }
            Povel::Spat => h.is_some_and(|h| h.moze_spat()),
            Povel::Znova => h.is_some_and(|h| h.moze_znova()),
            Povel::Ulozit => obraz && h.is_some_and(|h| !h.aktivne().is_empty()),
            Povel::Rezim => self.oznaceny.is_some() && self.polozky.len() > 1,
            Povel::Prezentacia => !self.polozky.is_empty(),
            Povel::Napoveda => true,
            Povel::OrezPomer(_) | Povel::OrezOtocPomer | Povel::OrezZrus => self.orezanie.is_some(),
            Povel::OrezPotvrd => self.orezanie.as_ref().is_some_and(|o| !o.cely_obraz()),
            _ => obraz,
        }
    }

    /// Stred tlačidla panela podľa mena (na skúšky a samotest okna).
    pub fn tlacidlo(&self, id: &str) -> Option<(f32, f32)> {
        self.rozlozenie().iter().find_map(|u| match u.prvok {
            panel::Prvok::Tlacidlo(t) if t.id == id => Some((u.x + u.sirka / 2.0, panel::VYSKA as f32 / 2.0)),
            _ => None,
        })
    }

    /// Esc: zavrie nápovedu alebo prezentáciu; vráti `false`, ak nie je čo zavrieť (okno sa má zavrieť).
    pub fn esc(&mut self) -> bool {
        if self.orezanie.take().is_some() {
            self.zmena = true;
            true
        } else if self.napoveda {
            self.napoveda = false;
            self.zmena = true;
            true
        } else if self.kluc_vstup.is_some() {
            self.kluc_vstup = None;
            self.zmena = true;
            true
        } else if self.katalog {
            self.zavri_katalog();
            true
        } else if self.prezentacia.is_some() {
            self.vstup(Vstup::Povel(Povel::Prezentacia));
            true
        } else {
            false
        }
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
        if self.historie.get(&self.aktualny).is_some_and(|h| !h.aktivne().is_empty())
            && let Some((_, s)) = self.upravene.get(&self.aktualny)
        {
            return Some(s.clone());
        }
        self.snimky.get(&self.aktualny).and_then(|r| r.as_ref().ok().cloned())
    }

    /// Po zmene histórie: naplánuje náhľad úprav na pozadí.
    fn po_uprave(&mut self) {
        let i = self.aktualny;
        let kroky = self.historie.get(&i).map(|h| h.aktivne().to_vec()).unwrap_or_default();
        self.sprava = None;
        self.zmena = true;
        if kroky.is_empty() {
            self.upravene.remove(&i);
            self.posledna_uprava.remove(&i);
            return;
        }
        self.generacia += 1;
        let g = self.generacia;
        self.posledna_uprava.insert(i, g);
        if let Ok(mut u) = self.ulohy.lock() {
            u.insert(g, kroky);
        }
        self.posli(Ziadost::Uprav(i, g));
    }

    fn posli(&mut self, z: Ziadost) {
        if let Ok(mut f) = self.front.ziadosti.lock() {
            // úpravy majú prednosť (používateľ čaká na výsledok)
            f.0.push_front(z.clone());
            self.cakajuce.insert(z);
        }
        self.front.signal.notify_one();
    }

    /// Zobrazený výrez v súradniciach obrazu (ak nie je vidno celý obraz).
    fn zobrazeny_vyrez(&self, s: &Snimka) -> Option<(usize, usize, usize, usize)> {
        let (w, h) = self.plocha();
        let z = self.mierka(s);
        let ox = w as f32 / 2.0 + self.posun.0 - s.sirka as f32 * z / 2.0;
        let oy = h as f32 / 2.0 + self.posun.1 - s.vyska as f32 * z / 2.0;
        let x0 = ((-ox) / z).max(0.0).floor() as usize;
        let y0 = ((-oy) / z).max(0.0).floor() as usize;
        let x1 = (((w as f32 - ox) / z).ceil().max(0.0) as usize).min(s.sirka);
        let y1 = (((h as f32 - oy) / z).ceil().max(0.0) as usize).min(s.vyska);
        (x1 > x0 && y1 > y0 && (x0, y0, x1, y1) != (0, 0, s.sirka, s.vyska)).then_some((
            x0,
            y0,
            x1 - x0,
            y1 - y0,
        ))
    }

    /// Zapne režim orezania: výber = zobrazený výrez (ak je obraz priblížený), inak celý obraz.
    fn zacni_orezanie(&mut self) {
        let Some(s) = self.aktualna() else { return };
        let vyrez = self.zobrazeny_vyrez(&s);
        let mriezka = self.mriezka_bezstratovo();
        self.orezanie = Some(Orezanie::novy(s.sirka, s.vyska, vyrez, mriezka));
        self.sprava = None;
        self.zmena = true;
    }

    /// Dá sa ťahaním nakresliť výber? (jeden obraz, nie porovnanie ani prezentácia)
    fn mozno_kreslit_vyber(&self) -> bool {
        let porovnanie = self.oznaceny.is_some() && self.rezim != Rezim::Jeden;
        !porovnanie && self.prezentacia.is_none() && !self.napoveda && self.aktualna().is_some()
    }

    /// Prvé ťahanie po stlačení na obraze: nový výber od bodu stlačenia (aj bez režimu orezania).
    fn zacni_kreslenie(&mut self) {
        let Some((x, y)) = self.vyber_od.take() else { return };
        let Some(s) = self.aktualna() else { return };
        let z = self.mierka(&s);
        let (w, h) = self.plocha();
        let ox = w as f32 / 2.0 + self.posun.0 - s.sirka as f32 * z / 2.0;
        let oy = h as f32 / 2.0 + self.posun.1 - s.vyska as f32 * z / 2.0;
        let (bx, by) = ((x - ox) / z, (y - self.hore() as f32 - oy) / z);
        match self.orezanie.as_mut() {
            Some(o) => o.nakresli_od(bx, by),
            None => {
                let m = self.mriezka_bezstratovo();
                self.orezanie = Some(Orezanie::nakreslene(s.sirka, s.vyska, bx, by, m));
                self.sprava = None;
            }
        }
        self.orez_kurzor = (x, y);
        self.zmena = true;
    }

    /// Mriežka bezstratového orezania aktuálneho JPEG (len ak doterajšie úpravy sú geometrické a bez orezania).
    fn mriezka_bezstratovo(&self) -> Option<Mriezka> {
        use std::io::Read;
        let t = match self.historie.get(&self.aktualny).map(|h| h.bezstratova()) {
            None => Transformacia::Ziadna,
            Some(Some((t, None))) => t,
            _ => return None,
        };
        let cesta = &self.polozky.get(self.aktualny)?.cesta;
        // stačí hlavička (Exif s natočením a SOF)
        let mut d = Vec::new();
        std::fs::File::open(cesta).ok()?.take(1 << 20).read_to_end(&mut d).ok()?;
        if orchidea_formaty::rozpoznaj(&d) != Some(orchidea_formaty::Format::Jpeg) {
            return None;
        }
        let n = orchidea_formaty::citaj_popis(&d, &Limity::default()).map_or(1, |p| p.metadata.natocenie);
        bezstratovo::mriezka(&d, Transformacia::z_natocenia(n).potom(t))
    }

    /// Potvrdí orezanie (Enter, tlačidlo Orezať); výber celého obrazu nič neoreže.
    fn potvrd_orezanie(&mut self) -> bool {
        let Some(o) = self.orezanie.take() else { return false };
        self.zmena = true;
        if o.cely_obraz() {
            return true;
        }
        let (x, y, sirka, vyska) = o.vysledok();
        self.historie.entry(self.aktualny).or_default().pridaj(Uprava::Orez { x, y, sirka, vyska });
        self.prisposob();
        self.po_uprave();
        true
    }

    /// Vstup v režime orezania; `None` = spracuje sa ako obvykle (panel, priblíženie …).
    fn vstup_orezania(&mut self, v: &Vstup) -> Option<bool> {
        let s = self.aktualna()?;
        let z = self.mierka(&s);
        let (w, h) = self.plocha();
        let hore = self.hore() as f32;
        let ox = w as f32 / 2.0 + self.posun.0 - s.sirka as f32 * z / 2.0;
        let oy = h as f32 / 2.0 + self.posun.1 - s.vyska as f32 * z / 2.0;
        let do_obrazu = |x: f32, y: f32| ((x - ox) / z, (y - hore - oy) / z);
        // úchop 8 bodov obrazovky
        let tolerancia = 8.0 / z;
        let v_ploche = |y: f32| y >= hore && y < hore + h as f32;
        match v {
            Vstup::Povel(p) => match p {
                Povel::Orezat | Povel::OrezPotvrd => Some(self.potvrd_orezanie()),
                Povel::OrezZrus => {
                    self.orezanie = None;
                    self.zmena = true;
                    Some(true)
                }
                Povel::OrezPomer(pomer) => {
                    self.orezanie.as_mut()?.nastav_pomer(*pomer);
                    self.zmena = true;
                    Some(true)
                }
                Povel::OrezOtocPomer => {
                    self.orezanie.as_mut()?.otoc_pomer();
                    self.zmena = true;
                    Some(true)
                }
                // Uložiť: najprv orezať na výber, potom uložiť
                Povel::Ulozit => {
                    self.potvrd_orezanie();
                    None
                }
                Povel::Napoveda
                | Povel::Panel
                | Povel::Priblizit
                | Povel::Oddialit
                | Povel::Prisposobit
                | Povel::Skutocna
                | Povel::Udaje
                | Povel::Pas => None,
                // iný povel (ďalší obraz, otočenie …) orezanie zruší
                _ => {
                    self.orezanie = None;
                    self.zmena = true;
                    None
                }
            },
            Vstup::Klik { x, y } if v_ploche(*y) && !self.napoveda => {
                let (bx, by) = do_obrazu(*x, *y);
                self.orez_kurzor = (*x, *y);
                self.klik_v_paneli = false;
                // mimo výberu: ťahaním sa nakreslí nový, samotné kliknutie výber zruší
                if !self.orezanie.as_mut()?.zacni(bx, by, tolerancia) {
                    self.vyber_od = Some((*x, *y));
                }
                self.zmena = true;
                Some(true)
            }
            Vstup::Klik { .. } => {
                self.orezanie.as_mut()?.pusti();
                None
            }
            Vstup::Tahaj { dx, dy } if self.orezanie.as_ref().is_some_and(Orezanie::taha) => {
                self.orez_kurzor = (self.orez_kurzor.0 + dx, self.orez_kurzor.1 + dy);
                let (bx, by) = do_obrazu(self.orez_kurzor.0, self.orez_kurzor.1);
                let zmena = self.orezanie.as_mut()?.tahaj(bx, by);
                self.zmena |= zmena;
                Some(zmena)
            }
            Vstup::Mys { x, y } => {
                // kliknutie mimo výberu bez ťahania = zrušiť výber
                if self.vyber_od.take().is_some() {
                    self.orezanie = None;
                    self.zmena = true;
                    return None;
                }
                // pohyb bez tlačidla = ťahanie skončilo; zvýrazní sa časť výberu pod myšou
                let nove = if v_ploche(*y) {
                    let (bx, by) = do_obrazu(*x, *y);
                    self.orezanie.as_ref()?.uchop_na(bx, by, tolerancia)
                } else {
                    None
                };
                let o = self.orezanie.as_mut()?;
                o.pusti();
                if nove != o.pod_mysou {
                    o.pod_mysou = nove;
                    self.zmena = true;
                }
                None
            }
            Vstup::Otvor(_) => {
                self.orezanie = None;
                self.vyber_od = None;
                None
            }
            _ => None,
        }
    }

    /// Spracuje vstup; vráti, či treba prekresliť.
    pub fn vstup(&mut self, v: Vstup) -> bool {
        let pred = self.polozky.get(self.aktualny).map(|p| p.cesta.clone());
        let mut prekreslit = false;
        if let Vstup::Mys { y, .. } = &v
            && !self.v_meta(*y)
            && self.pod_mysou_meta.take().is_some()
        {
            self.zmena = true;
            prekreslit = true;
        }
        let r = match self.vstup_meta(&v) {
            Some(r) => r,
            None if self.katalog => self.vstup_katalogu(v),
            None => self.vstup_prehliadaca(v),
        };
        // iná fotka: písanie slova a chyba zápisu sa zrušia
        if self.polozky.get(self.aktualny).map(|p| &p.cesta) != pred.as_ref() {
            self.kluc_vstup = None;
            self.chyba_xmp = None;
            self.pod_mysou_meta = None;
        }
        r || prekreslit
    }

    fn vstup_prehliadaca(&mut self, v: Vstup) -> bool {
        if let Vstup::Tahaj { dx, dy } = v
            && (dx != 0.0 || dy != 0.0)
            && self.vyber_od.is_some()
            && self.mozno_kreslit_vyber()
        {
            self.zacni_kreslenie();
        }
        if self.orezanie.is_some()
            && let Some(r) = self.vstup_orezania(&v)
        {
            return r;
        }
        let n = self.polozky.len();
        let povodny = self.aktualny;
        match v {
            Vstup::Znak(_) => return false,
            Vstup::Otvor(c) => {
                self.predosle = None;
                self.zaznamy_vysledkov.clear();
                // nové otvorenie: hodnotenia a kľúčové slová sa prečítajú znova (mohol ich zmeniť iný program)
                let cakajuce: Vec<PathBuf> = self
                    .cakajuce
                    .iter()
                    .filter_map(|q| match q {
                        Ziadost::ZapisXmp(d, _) => Some(d.clone()),
                        _ => None,
                    })
                    .collect();
                self.meta.retain(|c, _| cakajuce.contains(c));
                self.otvor(&c);
            }
            Vstup::Tik => {
                if self.casovac().is_none() || n == 0 {
                    return false;
                }
                // ďalší obraz je už načítaný vopred – pri tiku sa len prekreslí
                self.aktualny = (self.aktualny + 1) % n;
            }
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
                Povel::Prezentacia => {
                    self.prezentacia = match self.prezentacia {
                        Some(_) => None,
                        None => Some((5000, false)),
                    };
                    self.prisposob();
                    self.naplanuj();
                }
                Povel::Napoveda => {
                    self.napoveda = !self.napoveda;
                    self.zmena = true;
                }
                Povel::Katalog if self.katalog_subor.is_some() => self.otvor_katalog(),
                Povel::Otvorit if self.dialogy => {
                    self.ziadost_okna = Some(ZiadostOkna::OtvorSubor { priecinok: self.priecinok.clone() });
                    return false;
                }
                Povel::Panel => {
                    self.panel = !self.panel;
                    self.pod_mysou = None;
                    self.zmena = true;
                }
                Povel::Pauza => {
                    if let Some(p) = self.prezentacia.as_mut() {
                        p.1 = !p.1;
                        self.zmena = true;
                    }
                }
                Povel::Spat | Povel::Znova => {
                    let h = self.historie.entry(self.aktualny).or_default();
                    let zmena = if p == Povel::Spat { h.spat() } else { h.znova() };
                    if zmena {
                        self.prisposob();
                        self.po_uprave();
                    }
                }
                Povel::Ulozit => {
                    let kroky =
                        self.historie.get(&self.aktualny).map(|h| h.aktivne().to_vec()).unwrap_or_default();
                    self.generacia += 1;
                    let g = self.generacia;
                    if let Ok(mut u) = self.ulohy.lock() {
                        u.insert(g, kroky);
                    }
                    self.sprava = Some("ukladám…".into());
                    self.zmena = true;
                    self.posli(Ziadost::Uloz(self.aktualny, g));
                }
                Povel::OtocVpravo
                | Povel::OtocVlavo
                | Povel::Zrkadlo
                | Povel::Orezat
                | Povel::JasViac
                | Povel::JasMenej
                | Povel::KontrastViac
                | Povel::KontrastMenej
                | Povel::GamaViac
                | Povel::GamaMenej
                | Povel::Zmensit => {
                    let Some(s) = self.aktualna() else { return false };
                    let b = |jas: f32, kontrast: f32, gama: f32| Uprava::Bodova { jas, kontrast, gama };
                    let u = match p {
                        Povel::OtocVpravo => Uprava::Transformacia(Transformacia::Otoc90),
                        Povel::OtocVlavo => Uprava::Transformacia(Transformacia::Otoc270),
                        Povel::Zrkadlo => Uprava::Transformacia(Transformacia::ZrkadloVodorovne),
                        Povel::Orezat => {
                            self.zacni_orezanie();
                            return true;
                        }
                        Povel::JasViac => b(0.05, 1.0, 1.0),
                        Povel::JasMenej => b(-0.05, 1.0, 1.0),
                        Povel::KontrastViac => b(0.0, 1.1, 1.0),
                        Povel::KontrastMenej => b(0.0, 1.0 / 1.1, 1.0),
                        Povel::GamaViac => b(0.0, 1.0, 1.1),
                        Povel::GamaMenej => b(0.0, 1.0, 1.0 / 1.1),
                        _ => Uprava::Zmensi { max: (s.sirka.max(s.vyska) / 2).max(1) },
                    };
                    self.historie.entry(self.aktualny).or_default().pridaj(u);
                    if matches!(p, Povel::OtocVpravo | Povel::OtocVlavo | Povel::Orezat | Povel::Zmensit) {
                        self.prisposob();
                    }
                    self.po_uprave();
                }
                _ => {}
            },
            Vstup::Koliesko { krokov, x, y } => {
                let y = y - self.hore() as f32;
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
                // ťahanie ľavým po obraze kreslí výber (vyššie); posúva len pri porovnaní a v prezentácii
                if (dx == 0.0 && dy == 0.0)
                    || self.klik_v_paneli
                    || self.napoveda
                    || self.mozno_kreslit_vyber()
                {
                    return false;
                }
                self.posun.0 += dx;
                self.posun.1 += dy;
                self.zmena = true;
            }
            Vstup::Posun { dx, dy, pravym } => {
                // pravým tlačidlom sa v delenom porovnaní posúva deliaca čiara, nie obraz
                if (dx == 0.0 && dy == 0.0)
                    || (pravym && self.rezim == Rezim::Deleny && self.oznaceny.is_some())
                {
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
            Vstup::Mys { x, y } => {
                self.vyber_od = None;
                let nove = panel::na_bode(&self.rozlozenie(), x, y)
                    .filter(|t| self.povel_aktivny(t.povel))
                    .map(|t| t.id);
                if nove == self.pod_mysou {
                    return false;
                }
                self.pod_mysou = nove;
                self.zmena = true;
            }
            Vstup::Klik { x, y } => {
                self.klik_v_paneli = false;
                if self.napoveda {
                    self.napoveda = false;
                    self.zmena = true;
                    return true;
                }
                if self.panel_viditelny() && y < self.hore() as f32 {
                    self.klik_v_paneli = true;
                    if let Some(t) = panel::na_bode(&self.rozlozenie(), x, y)
                        && self.povel_aktivny(t.povel)
                    {
                        return self.vstup(Vstup::Povel(t.povel));
                    }
                    return false;
                }
                let (_, h) = self.plocha();
                // stlačenie na obraze: ťahaním sa nakreslí výber na orezanie
                if y >= self.hore() as f32 && y < (self.hore() + h) as f32 {
                    self.vyber_od = Some((x, y));
                }
                let h = h + self.hore();
                let h = h + if self.meta_viditelna() { panel::VYSKA_META } else { 0 };
                if self.pas_viditelny() && y >= (h + if self.udaje_viditelne() { 24 } else { 0 }) as f32 {
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
            self.sprava = None;
            self.naplanuj();
            if self.historie.get(&self.aktualny).is_some_and(|h| !h.aktivne().is_empty())
                && !self.upravene.contains_key(&self.aktualny)
            {
                self.po_uprave();
            }
        }
        self.zmena
    }

    /// Vstup v mriežke katalógu.
    fn vstup_katalogu(&mut self, v: Vstup) -> bool {
        let n = self.polozky.len();
        let (x0, y0, stlpcov, riadkov) = self.mriezka();
        let povodny = (self.aktualny, self.prvy_riadok);
        match v {
            Vstup::Znak(c) => {
                match c {
                    '\u{8}' => {
                        if self.dotaz.pop().is_none() {
                            return false;
                        }
                    }
                    '\r' | '\n' => {
                        self.otvor_vysledok(self.aktualny);
                        return true;
                    }
                    c if c.is_control() => return false,
                    c => {
                        if self.dotaz.chars().count() >= 200 {
                            return false;
                        }
                        self.dotaz.push(c);
                    }
                }
                self.hladaj_v_katalogu();
                return true;
            }
            Vstup::Otvor(c) => {
                let priecinok = if c.is_dir() { Some(c) } else { c.parent().map(Path::to_path_buf) };
                if let (Some(s), Some(p)) = (self.katalog_subor.clone(), priecinok) {
                    self.posli(Ziadost::Katalog(UlohaKatalogu::Pridaj(s, p)));
                    self.katalog_sprava = Some("pridávam priečinok…".into());
                    self.zmena = true;
                }
                return true;
            }
            Vstup::Velkost { sirka, vyska } => {
                if (sirka, vyska) == (self.sirka, self.vyska) {
                    return false;
                }
                self.sirka = sirka.max(1);
                self.vyska = vyska.max(1);
                self.ukaz_vyber();
                self.zmena = true;
                self.naplanuj();
                return true;
            }
            Vstup::Povel(p) => match p {
                Povel::Dalsi if n > 0 => self.aktualny = (self.aktualny + 1).min(n - 1),
                Povel::Predosly => self.aktualny = self.aktualny.saturating_sub(1),
                Povel::Dole if n > 0 => self.aktualny = (self.aktualny + stlpcov).min(n - 1),
                Povel::Hore => self.aktualny = self.aktualny.saturating_sub(stlpcov),
                Povel::Prvy => self.aktualny = 0,
                Povel::Posledny => self.aktualny = n.saturating_sub(1),
                Povel::Katalog => {
                    self.zavri_katalog();
                    return true;
                }
                Povel::KatalogPridaj if self.dialogy => {
                    self.ziadost_okna =
                        Some(ZiadostOkna::VyberPriecinok { priecinok: self.priecinok_na_pridanie() });
                    return false;
                }
                Povel::KatalogPridaj => {
                    if let (Some(s), Some(p)) = (self.katalog_subor.clone(), self.priecinok_na_pridanie()) {
                        self.posli(Ziadost::Katalog(UlohaKatalogu::Pridaj(s, p)));
                        self.katalog_sprava = Some("pridávam priečinok…".into());
                        self.zmena = true;
                    }
                    return true;
                }
                Povel::KatalogObnov => {
                    if let Some(s) = self.katalog_subor.clone() {
                        self.posli(Ziadost::Katalog(UlohaKatalogu::Obnov(s)));
                        self.katalog_sprava = Some("obnovujem…".into());
                        self.zmena = true;
                    }
                    return true;
                }
                Povel::Napoveda => {
                    self.napoveda = !self.napoveda;
                    self.zmena = true;
                    return true;
                }
                Povel::Panel => {
                    self.panel = !self.panel;
                    self.zmena = true;
                    self.naplanuj();
                    return true;
                }
                Povel::Udaje => {
                    self.udaje = !self.udaje;
                    self.zmena = true;
                    self.naplanuj();
                    return true;
                }
                _ => return false,
            },
            Vstup::Koliesko { krokov, .. } => {
                let riadkov_spolu = n.div_ceil(stlpcov);
                let max = riadkov_spolu.saturating_sub(riadkov);
                let k = (-krokov * 2.0).round() as isize;
                self.prvy_riadok = (self.prvy_riadok as isize + k).clamp(0, max as isize) as usize;
            }
            Vstup::Mys { x, y } => {
                let nove = panel::na_bode(&self.rozlozenie(), x, y)
                    .filter(|t| self.povel_aktivny(t.povel))
                    .map(|t| t.id);
                if nove == self.pod_mysou {
                    return false;
                }
                self.pod_mysou = nove;
                self.zmena = true;
                return true;
            }
            Vstup::Klik { x, y } => {
                if self.napoveda {
                    self.napoveda = false;
                    self.zmena = true;
                    return true;
                }
                if self.panel_viditelny() && y < self.hore() as f32 {
                    if let Some(t) = panel::na_bode(&self.rozlozenie(), x, y)
                        && self.povel_aktivny(t.povel)
                    {
                        return self.vstup_katalogu(Vstup::Povel(t.povel));
                    }
                    return false;
                }
                if y < y0 as f32 || x < x0 as f32 {
                    return false;
                }
                let s = ((x as usize) - x0) / DLAZDICA_W;
                let r = ((y as usize) - y0) / DLAZDICA_H + self.prvy_riadok;
                if s < stlpcov && r * stlpcov + s < n {
                    self.otvor_vysledok(r * stlpcov + s);
                    return true;
                }
                return false;
            }
            Vstup::Tahaj { .. } | Vstup::Delic { .. } | Vstup::Posun { .. } | Vstup::Tik => return false,
        }
        if (self.aktualny, self.prvy_riadok) == povodny {
            return false;
        }
        if self.aktualny != povodny.0 {
            self.ukaz_vyber();
        }
        self.zmena = true;
        self.naplanuj();
        true
    }

    fn pozicia_pasu(&self, od: usize) -> (f32, f32) {
        // aktuálny náhľad v strede pásu
        let stred = self.sirka as f32 / 2.0 - PAS as f32 / 2.0;
        (stred - (self.aktualny - od) as f32 * PAS as f32, 0.0)
    }

    /// Titulok okna.
    pub fn titulok(&self) -> String {
        if self.katalog {
            let spolu = self.herbar.as_ref().map_or(0, |h| h.zaznamy.len());
            return format!(
                "katalóg fotografií (Herbár) – nájdených {} z {spolu} – prehliadač a editor fotografií (Pohľad)",
                self.polozky.len()
            );
        }
        match self.polozky.get(self.aktualny) {
            Some(p) => format!(
                "{} ({}/{}) – prehliadač a editor fotografií (Pohľad)",
                p.cesta.file_name().map(|m| m.to_string_lossy().into_owned()).unwrap_or_default(),
                self.aktualny + 1,
                self.polozky.len()
            ),
            None => {
                "prehliadač a editor fotografií (Pohľad) – pretiahnite sem obrázok alebo priečinok".into()
            }
        }
    }

    /// Vykreslí stav do plátna (veľkosť plátna = veľkosť okna).
    pub fn vykresli(&mut self, p: &mut Platno) {
        self.vykresleni += 1;
        self.zmena = false;
        let pozadie = if self.prezentacia.is_some() { 0 } else { POZADIE };
        if p.sirka != self.sirka || p.vyska != self.vyska {
            *p = Platno::nove(self.sirka, self.vyska, pozadie);
        } else {
            p.body.fill(pozadie);
        }
        if self.katalog {
            self.kresli_katalog(p);
            self.kresli_panel(p);
            if self.napoveda {
                panel::kresli_napovedu(p, &self.pismo, panel::NAPOVEDA);
            }
            return;
        }
        let (w, h) = self.plocha();
        let hore = self.hore();
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
            paralel::pre_kusy(&mut p.body[hore * w..(hore + h) * w], w, |y, riadok| {
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
                    *o = na_pozadi(bod, x, y + hore);
                }
            });
            if rezim == Rezim::Deleny {
                p.vypln(delic as i32, hore as i32, 1, h as i32, ZVYRAZNENIE);
            }
            if let Some(o) = &self.orezanie {
                self.kresli_orezanie(p, o, b, (ox, oy + hore as f32), z, (hore, h));
            }
        } else if let Some(Err(e)) = self.snimky.get(&self.aktualny) {
            let t = format!("Obrázok sa nedá zobraziť: {e}");
            p.text(&self.pismo, &t, 16.0, (hore + h / 2) as f32, 15.0, 1.0, TEXT);
        } else if self.polozky.is_empty() {
            let t = "Pretiahnite sem obrázok alebo priečinok";
            let x = (w as f32 - self.pismo.sirka(t, 18.0, 1.0)) / 2.0;
            p.text(&self.pismo, t, x, (hore + h / 2) as f32, 18.0, 1.0, TEXT);
            let t = "F1 alebo ? – nápoveda klávesov";
            let x = (w as f32 - self.pismo.sirka(t, 14.0, 1.0)) / 2.0;
            p.text(&self.pismo, t, x, (hore + h / 2) as f32 + 30.0, 14.0, 1.0, 0x9A9DA1);
        }
        // riadok hodnotenia a kľúčových slov
        let mut y = h + hore;
        if self.meta_viditelna() {
            self.kresli_meta(p, y);
            y += panel::VYSKA_META;
        }
        // riadok s údajmi
        if self.udaje_viditelne() {
            p.vypln(0, y as i32, self.sirka as i32, 24, 0x26282B);
            let t = self.text_udajov(b.as_deref(), a.as_deref());
            let t = self.pismo.skrat(&t, self.sirka as f32 - 16.0, 13.0, 1.0);
            p.text(&self.pismo, &t, 8.0, y as f32 + 17.0, 13.0, 1.0, TEXT);
            y += 24;
        }
        // pás náhľadov
        if self.pas_viditelny() {
            let (od, po) = self.viditelne_nahlady();
            let (x0, _) = self.pozicia_pasu(od);
            for i in od..po {
                let x = x0 + (i - od) as f32 * PAS as f32;
                if i == self.aktualny || Some(i) == self.oznaceny {
                    let f = if i == self.aktualny { ZVYRAZNENIE } else { 0xE0A040 };
                    p.vypln(x as i32 + 2, y as i32 + 2, PAS as i32 - 4, PAS as i32 - 4, f);
                }
                if let Some(Some(n)) = self.nahlady.get(&i) {
                    kresli_nahlad(p, n, x as i32 + PAS as i32 / 2, y as i32 + PAS as i32 / 2);
                }
            }
        }
        self.kresli_panel(p);
        if self.napoveda {
            panel::kresli_napovedu(p, &self.pismo, panel::NAPOVEDA);
        }
    }

    /// Katalóg: riadok hľadania, mriežka výsledkov s náhľadmi, riadok s údajmi o vybranej fotke.
    fn kresli_katalog(&self, p: &mut Platno) {
        let hore = self.hore() as i32;
        let (x0, y0, stlpcov, riadkov) = self.mriezka();
        // riadok hľadania
        p.vypln(0, hore, self.sirka as i32, HLADANIE as i32, 0x232528);
        let z = hore as f32 + 26.0;
        let x = p.text(&self.pismo, "Hľadať:", 12.0, z, 14.0, 1.0, TEXT) + 10.0;
        let spolu = self.herbar.as_ref().map_or(0, |h| h.zaznamy.len());
        let info = if self.katalog_pracuje() {
            "pracujem…".to_string()
        } else if self.herbar.is_some() {
            format!("nájdených {} z {spolu}", self.polozky.len())
        } else {
            String::new()
        };
        let iw = self.pismo.sirka(&info, 13.0, 1.0);
        let pole = (self.sirka as f32 - x - iw - 30.0).max(80.0);
        p.vypln(x as i32, hore + 7, pole as i32, HLADANIE as i32 - 14, 0x141517);
        p.vypln(x as i32, hore + HLADANIE as i32 - 8, pole as i32, 1, ZVYRAZNENIE);
        let t = self.pismo.skrat(&format!("{}_", self.dotaz), pole - 12.0, 14.0, 1.0);
        p.text(&self.pismo, &t, x + 6.0, z, 14.0, 1.0, 0xFFFFFF);
        p.text(&self.pismo, &info, self.sirka as f32 - iw - 12.0, z, 13.0, 1.0, 0x9A9DA1);
        // mriežka
        let n = self.polozky.len();
        if n == 0 {
            let (riadok1, riadok2) = match (&self.herbar, self.katalog_pracuje()) {
                (_, true) => ("Pracujem na katalógu…", String::new()),
                (None, _) => ("Katalóg sa načítava.", String::new()),
                (Some(h), _) if h.zaznamy.is_empty() => (
                    "Katalóg je prázdny. Pretiahnite sem priečinok s fotografiami alebo stlačte „Pridať priečinok“.",
                    "Fotky ostanú na svojom mieste – katalóg si pamätá len ich údaje.".to_string(),
                ),
                _ => (
                    "Nič sa nenašlo.",
                    "Príklady: kvet -vianoce   datum:2024-05   fotoaparat:canon   iso>=800   gps:ano"
                        .to_string(),
                ),
            };
            for (k, (t, v)) in [(riadok1.to_string(), 16.0), (riadok2, 14.0)].into_iter().enumerate() {
                let t = self.pismo.skrat(&t, self.sirka as f32 - 20.0, v, 1.0);
                let tx = (self.sirka as f32 - self.pismo.sirka(&t, v, 1.0)) / 2.0;
                p.text(&self.pismo, &t, tx.max(10.0), y0 as f32 + 60.0 + k as f32 * 28.0, v, 1.0, TEXT);
            }
        }
        let od = self.prvy_riadok * stlpcov;
        let po = (od + stlpcov * (riadkov + 1)).min(n);
        for i in od..po {
            let (s, r) = ((i - od) % stlpcov, (i - od) / stlpcov);
            let (x, y) = ((x0 + s * DLAZDICA_W) as i32, (y0 + r * DLAZDICA_H) as i32);
            if i == self.aktualny {
                p.vypln(x + 2, y + 2, DLAZDICA_W as i32 - 4, DLAZDICA_H as i32 - 4, 0x2D3E5C);
                p.vypln(x + 2, y + 2, DLAZDICA_W as i32 - 4, 2, ZVYRAZNENIE);
            }
            match self.nahlady.get(&i) {
                Some(Some(nh)) => kresli_nahlad(p, nh, x + DLAZDICA_W as i32 / 2, y + 8 + NAHLAD as i32 / 2),
                Some(None) => {
                    p.text(
                        &self.pismo,
                        "?",
                        (x + DLAZDICA_W as i32 / 2 - 4) as f32,
                        (y + 52) as f32,
                        16.0,
                        1.0,
                        TEXT,
                    );
                }
                None => p.vypln(x + 16, y + 8, NAHLAD as i32, NAHLAD as i32, 0x26282B),
            }
            let meno = self.polozky[i]
                .cesta
                .file_name()
                .map(|m| m.to_string_lossy().into_owned())
                .unwrap_or_default();
            let meno = self.pismo.skrat(&meno, DLAZDICA_W as f32 - 10.0, 11.0, 1.0);
            let mx = x as f32 + (DLAZDICA_W as f32 - self.pismo.sirka(&meno, 11.0, 1.0)) / 2.0;
            p.text(&self.pismo, &meno, mx, (y + 8 + NAHLAD as i32 + 18) as f32, 11.0, 1.0, TEXT);
            // hviezdičky pod menom
            let hodnotenie = self.meta.get(&self.polozky[i].cesta).map(|z| z.hodnotenie).or_else(|| {
                let h = self.herbar.as_ref()?;
                let &j = self.zaznamy_vysledkov.get(i)?;
                h.zaznamy.get(j).map(|z| z.hodnotenie)
            });
            if let Some(Some(h)) = hodnotenie
                && h > 0
            {
                let sx = x as f32 + DLAZDICA_W as f32 / 2.0 - h as f32 * 5.0 + 5.0;
                for k in 0..h {
                    panel::hviezda(
                        p,
                        sx + k as f32 * 10.0,
                        (y + 8 + NAHLAD as i32 + 28) as f32,
                        4.5,
                        0xF2C14E,
                    );
                }
            }
        }
        if self.meta_viditelna() && n > 0 {
            self.kresli_meta(p, self.y_meta());
        }
        // riadok s údajmi o vybranej fotke
        if self.udaje_viditelne() {
            let y = self.vyska as i32 - 24;
            p.vypln(0, y, self.sirka as i32, 24, 0x26282B);
            let t = self.text_udajov_katalogu();
            let t = self.pismo.skrat(&t, self.sirka as f32 - 16.0, 13.0, 1.0);
            p.text(&self.pismo, &t, 8.0, y as f32 + 17.0, 13.0, 1.0, TEXT);
        }
    }

    fn text_udajov_katalogu(&self) -> String {
        let mut t = Vec::new();
        if let (Some(h), Some(&i)) = (&self.herbar, self.zaznamy_vysledkov.get(self.aktualny))
            && let Some(z) = h.zaznamy.get(i)
            && !self.polozky.is_empty()
        {
            let z = self.meta.get(&z.cesta).unwrap_or(z);
            t.push(z.cesta.file_name().map(|m| m.to_string_lossy().into_owned()).unwrap_or_default());
            if !z.datum.is_empty() {
                t.push(z.datum.clone());
            }
            if !z.fotoaparat.is_empty() {
                t.push(z.fotoaparat.clone());
            }
            t.push(format!("{} × {}", z.sirka, z.vyska));
            if let Some(iso) = z.iso {
                t.push(format!("ISO {iso}"));
            }
            if !z.kluce.is_empty() {
                t.push(format!("kľúčové slová: {}", z.kluce.join(", ")));
            }
            if !z.nazov.is_empty() {
                t.push(z.nazov.clone());
            }
            if let Some(r) = z.hodnotenie {
                t.push(format!("hodnotenie {r}/5"));
            }
            if let Some(p) = z.cesta.parent() {
                t.push(p.display().to_string());
            }
        }
        if let Some(m) = &self.katalog_sprava {
            t.push(m.clone());
        }
        if t.is_empty() {
            t.push("Enter alebo kliknutie otvorí fotku · Esc zavrie katalóg".into());
        }
        t.join(" · ")
    }

    fn kresli_meta(&self, p: &mut Platno, y: usize) {
        let u = self.rozlozenie_meta();
        let hodnotenie = self.hodnotenie_a_kluce().and_then(|m| m.0);
        panel::kresli_meta(
            p,
            &self.pismo,
            y as i32,
            &u,
            hodnotenie,
            self.pod_mysou_meta,
            self.chyba_xmp.as_deref(),
        );
        if let Some(m) = self.pod_mysou_meta {
            panel::kresli_tip_meta(p, &self.pismo, &u, m, y as i32, &panel::tip_meta(m, hodnotenie));
        }
    }

    /// Výber orezania: stmavené okolie, rám, tretiny, úchopy a rozmer v bodoch.
    fn kresli_orezanie(
        &self,
        p: &mut Platno,
        o: &Orezanie,
        s: &Snimka,
        (ox, oy): (f32, f32),
        z: f32,
        (hore, h): (usize, usize),
    ) {
        let (x, y, w, vy) = o.vysledok();
        let na_obrazovku = |bx: f32, by: f32| ((ox + bx * z).round() as i32, (oy + by * z).round() as i32);
        let (ix0, iy0) = na_obrazovku(0.0, 0.0);
        let (ix1, iy1) = na_obrazovku(s.sirka as f32, s.vyska as f32);
        let (sx0, sy0) = na_obrazovku(x as f32, y as f32);
        let (sx1, sy1) = na_obrazovku((x + w) as f32, (y + vy) as f32);
        let (pw, ph) = (p.sirka as i32, p.vyska as i32);
        let (r0, r1) = (iy0.max(hore as i32), iy1.min((hore + h) as i32).min(ph));
        for r in r0.max(0)..r1 {
            for c in ix0.max(0)..ix1.min(pw) {
                if c < sx0 || c >= sx1 || r < sy0 || r >= sy1 {
                    let i = r as usize * p.sirka + c as usize;
                    p.body[i] = orchidea_pismo::zmiesaj(p.body[i], 0, 150);
                }
            }
        }
        // obdĺžnik zmiešaný s farbou, orezaný na plochu obrazu (nezasiahne panel ani riadky pod obrazom)
        let dolu = ((hore + h) as i32).min(ph);
        let ciara = |p: &mut Platno, x: i32, y: i32, cw: i32, ch: i32, f: Farba, a: u8| {
            for r in y.max(hore as i32)..(y + ch).min(dolu) {
                for c in x.max(0)..(x + cw).min(pw) {
                    let i = r as usize * p.sirka + c as usize;
                    p.body[i] = orchidea_pismo::zmiesaj(p.body[i], f, a);
                }
            }
        };
        // tretiny (pravidlo tretín) a rám
        for k in 1..3 {
            let tx = sx0 + (sx1 - sx0) * k / 3;
            let ty = sy0 + (sy1 - sy0) * k / 3;
            ciara(p, tx, sy0, 1, sy1 - sy0, 0xFFFFFF, 70);
            ciara(p, sx0, ty, sx1 - sx0, 1, 0xFFFFFF, 70);
        }
        ciara(p, sx0, sy0, sx1 - sx0, 1, 0xFFFFFF, 255);
        ciara(p, sx0, sy1 - 1, sx1 - sx0, 1, 0xFFFFFF, 255);
        ciara(p, sx0, sy0, 1, sy1 - sy0, 0xFFFFFF, 255);
        ciara(p, sx1 - 1, sy0, 1, sy1 - sy0, 0xFFFFFF, 255);
        // úchopy v rohoch a v strede okrajov
        let (cx, cy) = ((sx0 + sx1) / 2, (sy0 + sy1) / 2);
        for (u, hx, hy) in [
            (Uchop::LavyHorny, sx0, sy0),
            (Uchop::PravyHorny, sx1, sy0),
            (Uchop::LavyDolny, sx0, sy1),
            (Uchop::PravyDolny, sx1, sy1),
            (Uchop::Horny, cx, sy0),
            (Uchop::Dolny, cx, sy1),
            (Uchop::Lavy, sx0, cy),
            (Uchop::Pravy, sx1, cy),
        ] {
            let f = if o.pod_mysou == Some(u) { ZVYRAZNENIE } else { 0xFFFFFF };
            ciara(p, hx - 5, hy - 5, 10, 10, 0x101113, 255);
            ciara(p, hx - 4, hy - 4, 8, 8, f, 255);
        }
        // rozmer v bodoch
        let mut t = format!("{w} × {vy}");
        if o.mriezka.is_some() {
            t += " · bezstratovo";
        }
        let tw = self.pismo.sirka(&t, 13.0, 1.0) as i32 + 12;
        let (lx, mut ly) = (sx0 + 6, sy0 + 8);
        if sy1 - sy0 < 40 {
            ly = sy1 + 8;
        }
        ly = ly.clamp(hore as i32 + 2, (dolu - 22).max(hore as i32 + 2));
        ciara(p, lx, ly, tw, 20, 0x101113, 200);
        p.text(&self.pismo, &t, (lx + 6) as f32, (ly + 15) as f32, 13.0, 1.0, 0xFFFFFF);
    }

    fn kresli_panel(&self, p: &mut Platno) {
        if !self.panel_viditelny() {
            return;
        }
        let u = self.rozlozenie();
        let zapnute = |pv: Povel| match (pv, &self.orezanie) {
            (Povel::OrezPomer(x), Some(o)) => o.pomer == x,
            _ => false,
        };
        panel::kresli(p, &self.pismo, &u, self.pod_mysou, |pv| self.povel_aktivny(pv), zapnute);
        if let Some(id) = self.pod_mysou {
            panel::kresli_tip(p, &self.pismo, &u, id);
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
        if let Some(o) = &self.orezanie {
            let (_, _, w, h) = o.vysledok();
            t = format!(
                "Orezanie {w} × {h} · ťahajte rohy a okraje, vnútri posun · Enter orezať, Esc zrušiť"
            );
            if let Some(m) = o.mriezka {
                t += &format!(" · JPEG bezstratovo (začiatok na mriežke {} × {})", m.mcu.0, m.mcu.1);
            }
            return t;
        }
        if let Some(h) = self.historie.get(&self.aktualny).filter(|h| !h.aktivne().is_empty()) {
            t += &format!(" · úprav: {} (Ctrl+Z späť, Ctrl+S uložiť)", h.aktivne().len());
        }
        if let Some(m) = &self.sprava {
            t += &format!(" · {m}");
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

/// Nakreslí náhľad so stredom v (cx, cy).
fn kresli_nahlad(p: &mut Platno, n: &Snimka, cx: i32, cy: i32) {
    let (nx, ny) = (cx - n.sirka as i32 / 2, cy - n.vyska as i32 / 2);
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

/// Zloží bod s alfou na šachovnicu (8 × 8).
fn na_pozadi(b: [u8; 4], x: usize, y: usize) -> Farba {
    let s = if ((x / 8) + (y / 8)).is_multiple_of(2) { 0x3A } else { 0x2E };
    let a = b[3] as u32;
    let c = |v: u8| (v as u32 * a + s * (255 - a) + 127) / 255;
    (c(b[0]) << 16) | (c(b[1]) << 8) | c(b[2])
}
