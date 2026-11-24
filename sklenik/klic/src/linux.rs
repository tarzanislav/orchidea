//! KLÍČ na Linuxe: štart systému, periférie a sieť, tepelný strážca nad súbormi jadra (`/sys`) a stav na
//! obrazovke.

use crate::cec;
use crate::chlad::{self, Ciel, SietPodlaPotreby, StavObrazu, Zhasinanie};
use crate::hdmi;
use crate::kms;
use crate::obrazovka::{self, Eee, FormatBodu, Jazyk, MeranieKarty, Stav};
use crate::pamat;
use crate::periferie::{Supis, Ulozisko};
use crate::pokoj;
use crate::radice;
use crate::samomeranie::{self, NL, ZNACKA_NOSICA, ZNACKA_PORADIA, nadpis};
use crate::strazca::{
    Hardver, Profil, Regulator, Uroven, je_tv_krabicka, ma_chladnu_sadu, smie_parkovat, stupen_ventilatora,
    urovne,
};
use crate::sys;
use crate::uloziska;
use crate::zaznam;
use orchidea_pismo::{Pismo, Platno};
use orchidea_teplo::okolie::{self, Okolie};
use orchidea_zariadenie::ProfilHardveru;
use sklenik_karta::NastavenieZariadenia;
use std::fs;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CPU: &str = "/sys/devices/system/cpu";
/// Súbor, z ktorého ORCHIDEA číta tepelný rozpočet (koľko vlákien smie použiť a ako rýchlo).
const ROZPOCET: &str = "/run/sklenik/teplo";
/// Po koľkých ms nečinnosti zaspí USB úložisko (R159).
const USB_SPANOK_MS: u32 = 2000;

/// Cieľ obrazu z nastavenia (R158): `obraz=povodny` – ako pred R158, inak chladný.
fn ciel_obrazu(n: &Option<(String, NastavenieZariadenia)>) -> Ciel {
    match n.as_ref().and_then(|(_, n)| n.obraz.as_deref()) {
        Some("povodny") => Ciel::Povodny,
        _ => Ciel::Chladny,
    }
}

/// `obraz=ostry` – plné rozlíšenie (16 bitov, najnižšia obnova), bez zväčšovania.
fn ostry_obraz(n: &Option<(String, NastavenieZariadenia)>) -> bool {
    n.as_ref().and_then(|(_, n)| n.obraz.as_deref()) == Some("ostry")
}

fn slovo_stavu(s: StavObrazu) -> &'static str {
    match s {
        StavObrazu::Zapnuty => "zapnutý",
        StavObrazu::Cierny => "čierny",
        StavObrazu::Vypnuty => "vypnutý",
    }
}

/// Zápis do sériovej linky, na obrazovku (konzola jadra) aj do denníka jadra.
fn hlas(sprava: &str) {
    println!("KLÍČ: {sprava}");
    // jeden zápis = jeden záznam denníka jadra
    if let Ok(mut k) = fs::OpenOptions::new().write(true).open("/dev/kmsg") {
        let _ = k.write_all(format!("<6>KLÍČ: {sprava}\n").as_bytes());
    }
    // R216: záznamník pádov – pamäť, ktorá prežije reštart (pstore/ramoops), ak ju doska má
    if let Ok(mut p) = fs::OpenOptions::new().write(true).open("/dev/pmsg0") {
        let s = uptime().unwrap_or(0.0) as u64;
        let _ = p.write_all(format!("[{}:{:02}:{:02}] {sprava}\n", s / 3600, s / 60 % 60, s % 60).as_bytes());
    }
}

/// R216: posledné riadky predošlého štartu zo záznamníka pádov (pstore) – po nečakanom reštarte
/// ukážu, čo sa dialo tesne pred ním. Súbory sa potom zmažú, aby sa uvoľnila pamäť pre ďalší záznam.
fn predosly_start() -> Vec<String> {
    const KOREN: &str = "/sys/fs/pstore";
    let _ = fs::create_dir_all(KOREN);
    let _ = sys::pripoj("pstore", KOREN, "pstore", sys::MS_NOSUID | sys::MS_NODEV | sys::MS_NOEXEC);
    let mut subory: Vec<_> = fs::read_dir(KOREN).into_iter().flatten().flatten().map(|z| z.path()).collect();
    subory.sort();
    let mut riadky = Vec::new();
    for cesta in subory {
        let meno = cesta.file_name().and_then(|m| m.to_str()).unwrap_or("").to_string();
        let Ok(obsah) = fs::read(&cesta) else { continue };
        let text = String::from_utf8_lossy(&obsah);
        // z KLÍČA (pmsg) posledných 25 riadkov, z jadra (dmesg pri páde, konzola) posledných 15
        let kolko = if meno.starts_with("pmsg") { 25 } else { 15 };
        let vsetky: Vec<&str> = text.lines().filter(|r| !r.trim().is_empty()).collect();
        for r in &vsetky[vsetky.len().saturating_sub(kolko)..] {
            riadky.push(format!("{meno}: {}", r.chars().take(200).collect::<String>()));
        }
        let _ = fs::remove_file(&cesta);
    }
    riadky
}

/// R183: je novinka zapnutá podľa SKLENIK.TXT? (bez nastavenia áno)
fn novinka(n: &Option<(String, NastavenieZariadenia)>, kluc: &str) -> bool {
    n.as_ref().is_none_or(|(_, n)| n.novinka(kluc))
}

/// R183: vypnuté novinky pre záznam.
fn vypnute_novinky(n: &Option<(String, NastavenieZariadenia)>) -> String {
    let v: Vec<&str> = sklenik_karta::NOVINKY.iter().map(|(k, _)| *k).filter(|k| !novinka(n, k)).collect();
    if v.is_empty() { "všetky zapnuté".into() } else { format!("vypnuté: {}", v.join(", ")) }
}

/// R183: dekodér videa vypnutý – ovládač meson-vdec sa odpojí od zariadenia (ako by v jadre nebol).
fn odpoj_dekoder() -> Vec<String> {
    let d = Path::new("/sys/bus/platform/drivers/meson-vdec");
    let mut v = Vec::new();
    for z in fs::read_dir(d).into_iter().flatten().flatten() {
        let meno = z.file_name().to_string_lossy().to_string();
        if meno.contains('.') && fs::write(d.join("unbind"), &meno).is_ok() {
            v.push(meno);
        }
    }
    v
}

fn citaj(cesta: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(cesta).ok().map(|s| s.trim().trim_end_matches('\0').to_string())
}

fn zapis(cesta: impl AsRef<Path>, hodnota: &str) -> bool {
    fs::write(cesta, hodnota).is_ok()
}

/// Hardvér Raspberry Pi (a každého Linuxu) cez `/sys`.
struct Sysfs {
    snimace: Vec<PathBuf>,
    takty: Vec<u32>,
    jadier: u32,
    vypinatelne: Vec<PathBuf>,
    /// R194: jadrá, ktoré sa po vypnutí nevrátili – už sa neparkujú ani nespúšťajú.
    nefunkcne: Vec<PathBuf>,
    uplatnena: Option<(u32, u32)>,
    /// Ventilátory jadra (`cooling_device` typu *fan*): cesta a najvyšší stupeň; riadi ich strážca.
    ventilatory: Vec<(PathBuf, u32, u32)>,
}

impl Sysfs {
    fn najdi(parkovanie: bool) -> Sysfs {
        let snimace: Vec<PathBuf> = fs::read_dir("/sys/class/thermal")
            .into_iter()
            .flatten()
            .flatten()
            .map(|z| z.path().join("temp"))
            .filter(|p| p.exists())
            .collect();
        let politika = format!("{CPU}/cpufreq/policy0");
        let mut takty: Vec<u32> = citaj(format!("{politika}/scaling_available_frequencies"))
            .map(|s| s.split_whitespace().filter_map(|x| x.parse().ok()).collect())
            .unwrap_or_default();
        if takty.is_empty() {
            takty = ["cpuinfo_min_freq", "cpuinfo_max_freq"]
                .iter()
                .filter_map(|f| citaj(format!("{politika}/{f}"))?.parse().ok())
                .collect();
        }
        let jadier = citaj(format!("{CPU}/present"))
            .and_then(|s| s.rsplit('-').next()?.parse::<u32>().ok())
            .map_or(1, |n| n + 1);
        let vypinatelne = (1..jadier)
            .map(|i| PathBuf::from(format!("{CPU}/cpu{i}/online")))
            .filter(|p| parkovanie && p.exists())
            .collect();
        // regulátor taktu jadra: schedutil (rýchla odozva, úspora v nečinnosti), inak ondemand
        if let Some(dostupne) = citaj(format!("{politika}/scaling_available_governors")) {
            for g in ["schedutil", "ondemand"] {
                if dostupne.split_whitespace().any(|x| x == g) {
                    zapis(format!("{politika}/scaling_governor"), g);
                    break;
                }
            }
        }
        // ventilátory: strážca ich riadi sám (zóny prepne na user_space, aby ich jadro nezapínalo naplno)
        let ventilatory: Vec<(PathBuf, u32, u32)> = fs::read_dir("/sys/class/thermal")
            .into_iter()
            .flatten()
            .flatten()
            .map(|z| z.path())
            .filter(|p| citaj(p.join("type")).is_some_and(|t| t.contains("fan")))
            .filter_map(|p| Some((p.join("cur_state"), citaj(p.join("max_state"))?.parse().ok()?, 0)))
            .collect();
        if !ventilatory.is_empty() {
            for z in &snimace {
                if let Some(zona) = z.parent() {
                    zapis(zona.join("policy"), "user_space");
                }
            }
        }
        Sysfs { snimace, takty, jadier, vypinatelne, nefunkcne: Vec::new(), uplatnena: None, ventilatory }
    }

    fn ventiluj(&mut self, teplota: f64, profil: Profil) {
        for (cesta, najvyssi, terajsi) in &mut self.ventilatory {
            let novy = stupen_ventilatora(teplota, profil, *najvyssi, *terajsi);
            if novy != *terajsi && zapis(&*cesta, &novy.to_string()) {
                *terajsi = novy;
            }
        }
    }
}

impl Hardver for Sysfs {
    fn teplota(&mut self) -> Option<f64> {
        self.snimace.iter().filter_map(|p| citaj(p)?.parse::<f64>().ok()).map(|m| m / 1000.0).reduce(f64::max)
    }

    fn takty(&self) -> Vec<u32> {
        self.takty.clone()
    }

    fn jadier(&self) -> u32 {
        self.jadier
    }

    fn uplatni(&mut self, u: &Uroven) {
        if self.uplatnena == Some((u.jadra, u.takt_khz)) {
            return;
        }
        if u.takt_khz > 0 {
            zapis(format!("{CPU}/cpufreq/policy0/scaling_max_freq"), &u.takt_khz.to_string());
        }
        // jadro 0 zostáva vždy; ďalšie sa vypnú, ak to platforma dovolí (inak platí rozpočet pre ORCHIDEU)
        let mut zlyhane = Vec::new();
        for (i, p) in self.vypinatelne.iter().enumerate() {
            let chce = (i as u32) + 1 < u.jadra;
            let ok = zapis(p, if chce { "1" } else { "0" });
            // R194: jadro sa nevrátilo – firmvér ho nespustí; ďalšie pokusy by len zahlcovali denník jadra
            if chce && (!ok || citaj(p).as_deref() == Some("0")) {
                zlyhane.push(p.clone());
            }
        }
        for p in zlyhane {
            self.vypinatelne.retain(|x| *x != p);
            hlas(&format!(
                "jadro {} sa po vypnutí nevrátilo – už sa nebude parkovať ani spúšťať (R194)",
                p.parent()
                    .and_then(|x| x.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            ));
            self.nefunkcne.push(p);
            self.jadier = self.jadier.saturating_sub(1).max(1);
        }
        // tepelný rozpočet pre ORCHIDEU zapisuje hlavná slučka (úplný, s predpoveďou – R127)
        self.uplatnena = Some((u.jadra, u.takt_khz));
    }
}

fn pripoj_systemove() {
    for (zdroj, ciel, typ, priznaky) in [
        ("proc", "/proc", "proc", sys::MS_NOSUID | sys::MS_NODEV | sys::MS_NOEXEC),
        ("sysfs", "/sys", "sysfs", sys::MS_NOSUID | sys::MS_NODEV | sys::MS_NOEXEC),
        ("devtmpfs", "/dev", "devtmpfs", sys::MS_NOSUID),
        ("tmpfs", "/run", "tmpfs", sys::MS_NOSUID | sys::MS_NODEV),
        ("tmpfs", "/tmp", "tmpfs", sys::MS_NOSUID | sys::MS_NODEV),
        // merač taktov čipu Amlogic (R166) a ďalšie meradlá jadra sú v debugfs
        ("debugfs", "/sys/kernel/debug", "debugfs", sys::MS_NOSUID | sys::MS_NODEV | sys::MS_NOEXEC),
    ] {
        let _ = fs::create_dir_all(ciel);
        if let Err(e) = sys::pripoj(zdroj, ciel, typ, priznaky) {
            // devtmpfs už môže byť pripojený jadrom (CONFIG_DEVTMPFS_MOUNT)
            if e.raw_os_error() != Some(16) {
                hlas(&format!("nepodarilo sa pripojiť {ciel}: {e}"));
            }
        }
    }
}

/// Zapne všetky sieťové rozhrania (ovládače sú v jadre, rozhrania však jadro necháva vypnuté).
/// Zapne sieťové rozhrania. Vráti mená rozhraní, ktoré sa zapnúť nedali (čip siete neodpovedá – R120).
fn zapni_siet() -> Vec<String> {
    let mut chyby = Vec::new();
    for z in fs::read_dir("/sys/class/net").into_iter().flatten().flatten() {
        // lo a skutočné sieťové karty; virtuálne tunely jadra (sit0 …) sa nezapínajú
        if let Some(meno) = z.file_name().to_str()
            && (meno == "lo" || z.path().join("device").exists())
        {
            match sys::zapni_rozhranie(meno) {
                Ok(true) => hlas(&format!("sieť: rozhranie {meno} zapnuté")),
                Ok(false) => {}
                Err(e) => {
                    hlas(&format!("sieť: rozhranie {meno} sa nepodarilo zapnúť: {e} – čip siete neodpovedá"));
                    chyby.push(meno.to_string());
                }
            }
        }
    }
    chyby
}

/// Kam sa kreslí: framebuffer jadra alebo vlastný ovládač KMS (R158).
enum Vystup {
    Fb(fs::File),
    Kms(Box<kms::Kms>),
}

/// Grafická obrazovka – zapisujú sa len zmenené riadky.
struct Displej {
    vystup: Vystup,
    sirka: usize,
    vyska: usize,
    riadok_bajtov: u64,
    zaciatok: u64,
    format: FormatBodu,
    predosle: Option<Platno>,
    _tty: Option<fs::File>,
    /// Vlastný ovládač: režim a kreslenie (pre riadok Chlad a záznam).
    kms_nastavenie: Option<(kms::Rezim, kms::Kreslenie)>,
}

/// Konzola v grafickom režime: jadro do obrazovky nepíše.
fn graficka_konzola() -> Option<fs::File> {
    let tty = fs::OpenOptions::new().read(true).write(true).open("/dev/tty1").ok();
    if let Some(t) = &tty
        && let Err(e) = sys::graficka_konzola(t)
    {
        hlas(&format!("obrazovka: grafický režim konzoly sa nepodaril: {e}"));
    }
    tty
}

fn format_kms(f: u32) -> FormatBodu {
    if f == kms::RGB565 {
        FormatBodu { bajtov: 2, r: (11, 5), g: (5, 6), b: (0, 5), a: (0, 0) }
    } else {
        FormatBodu { bajtov: 4, r: (16, 8), g: (8, 8), b: (0, 8), a: (24, 0) }
    }
}

/// Nastaví režim a kreslenie podľa cieľa; skúša od najchladnejšieho, kým ovládač jadra niečo neprijme.
fn nastav_kms(k: &mut kms::Kms, ciel: Ciel, ostre: bool) -> Option<(kms::Rezim, kms::Kreslenie)> {
    let rezimy = k.rezimy();
    let i = kms::vyber_rezim(&rezimy, ciel)?;
    let r = rezimy[i];
    let plne16 = kms::Kreslenie { sirka: r.sirka as u32, vyska: r.vyska as u32, bitov: 16 };
    let poradie = match ciel {
        Ciel::Povodny => vec![kms::Kreslenie::povodne(&r)],
        Ciel::Chladny if ostre => vec![plne16, kms::Kreslenie::povodne(&r)],
        Ciel::Chladny => vec![kms::Kreslenie::chladne(&r), plne16, kms::Kreslenie::povodne(&r)],
    };
    for kr in poradie {
        match k.nastav(i, kr) {
            Ok(()) => return Some((r, kr)),
            Err(e) => hlas(&format!(
                "obraz: {} s kreslením {}×{} ({} b) ovládač neprijal: {e}",
                r.popis(),
                kr.sirka,
                kr.vyska,
                kr.bitov
            )),
        }
    }
    None
}

impl Displej {
    fn otvor() -> Option<Displej> {
        let fb = fs::OpenOptions::new().read(true).write(true).open("/dev/fb0").ok()?;
        let v = sys::fb_vlastnosti(&fb).ok()?;
        let format = FormatBodu::z_jadra(&v)?;
        let (sirka, vyska) = (v[0] as usize, v[1] as usize);
        if sirka < 160 || vyska < 120 {
            return None;
        }
        let riadok_bajtov = citaj("/sys/class/graphics/fb0/stride")
            .and_then(|s| s.parse().ok())
            .unwrap_or(v[2] as u64 * format.bajtov as u64);
        let zaciatok = v[5] as u64 * riadok_bajtov + v[4] as u64 * format.bajtov as u64;
        let tty = graficka_konzola();
        hlas(&format!("obrazovka: {sirka}×{vyska}, {} bitov na bod", format.bajtov * 8));
        Some(Displej {
            vystup: Vystup::Fb(fb),
            sirka,
            vyska,
            riadok_bajtov,
            zaciatok,
            format,
            predosle: None,
            _tty: tty,
            kms_nastavenie: None,
        })
    }

    /// R158: vlastný ovládač obrazovky cez KMS (TV krabičky). `None` = ostane sa pri `/dev/fb0`.
    fn otvor_kms(ciel: Ciel, ostre: bool) -> Option<Displej> {
        let mut k = match kms::Kms::otvor() {
            Ok(k) => k,
            Err(e) => {
                hlas(&format!("obraz: vlastný ovládač KMS nie je k dispozícii ({e}) – obyčajná obrazovka"));
                return None;
            }
        };
        let nastavenie = nastav_kms(&mut k, ciel, ostre)?;
        let mut d = Displej {
            vystup: Vystup::Kms(Box::new(k)),
            sirka: 0,
            vyska: 0,
            riadok_bajtov: 0,
            zaciatok: 0,
            format: format_kms(kms::XRGB8888),
            predosle: None,
            _tty: graficka_konzola(),
            kms_nastavenie: None,
        };
        d.prevezmi_kms(nastavenie);
        Some(d)
    }

    fn prevezmi_kms(&mut self, nastavenie: (kms::Rezim, kms::Kreslenie)) {
        let Vystup::Kms(k) = &mut self.vystup else { return };
        let Some((_, riadok, w, h, f)) = k.pamat() else { return };
        self.sirka = w as usize;
        self.vyska = h as usize;
        self.riadok_bajtov = riadok as u64;
        self.zaciatok = 0;
        self.format = format_kms(f);
        self.predosle = None;
        let (r, kr) = nastavenie;
        let video = if k.video.is_some() { "áno" } else { "nie" };
        hlas(&format!(
            "obraz: vlastný ovládač KMS – {}, kreslí {}×{} v {} bitoch{}, z pamäte {:.0} MB/s (pôvodne 1080p60 32 b: 498 MB/s); rovina videa NV12: {video}",
            r.popis(),
            kr.sirka,
            kr.vyska,
            kr.bitov,
            if kr.sirka < r.sirka as u32 { " (zväčšuje obvod obrazu)" } else { "" },
            kr.citanie_mb_s(r.obnova)
        ));
        self.kms_nastavenie = Some(nastavenie);
    }

    /// R160: iný cieľ obrazu (fáza tepelného pokusu) bez znovuotvorenia karty.
    fn prestav(&mut self, ciel: Ciel, ostre: bool) -> bool {
        let Vystup::Kms(k) = &mut self.vystup else { return false };
        match nastav_kms(k, ciel, ostre) {
            Some(n) => {
                self.prevezmi_kms(n);
                true
            }
            None => false,
        }
    }

    fn je_kms(&self) -> bool {
        matches!(self.vystup, Vystup::Kms(_))
    }

    /// Takt bodov režimu (kHz) – R185 (nad 340 MHz televízor čaká zakódovaný signál).
    fn takt_khz(&self) -> u32 {
        self.kms_nastavenie.as_ref().map_or(148_500, |(r, _)| r.takt_khz)
    }

    fn stav_obrazu(&self) -> StavObrazu {
        match &self.vystup {
            Vystup::Kms(k) => k.stav,
            Vystup::Fb(_) => StavObrazu::Zapnuty,
        }
    }

    fn nastav_stav_obrazu(&mut self, stav: StavObrazu) {
        let Vystup::Kms(k) = &mut self.vystup else { return };
        let pred = k.stav;
        match k.stav_obrazu(stav) {
            Ok(()) if pred != stav => hlas(&format!(
                "obraz: {}",
                match stav {
                    StavObrazu::Zapnuty => "zapnutý",
                    StavObrazu::Cierny if k.cierny_cez_buffer() =>
                        "čierny (malý čierny obraz zväčšený 5× – ovládač nedovolil vypnúť rovinu)",
                    StavObrazu::Cierny => "čierny – z pamäte sa nečíta nič, signál HDMI beží",
                    StavObrazu::Vypnuty => "vypnutý – stojí kódovač obrazu aj vysielač HDMI",
                }
            )),
            Ok(()) => {}
            Err(e) => hlas(&format!("obraz: zmena stavu sa nepodarila: {e}")),
        }
    }

    /// Zväčšuje obvod obrazu obraz (rovina menšia ako režim) – R175.
    fn zvacsuje(&self, stav: StavObrazu) -> bool {
        match (&self.vystup, &self.kms_nastavenie) {
            (Vystup::Kms(k), _) if stav == StavObrazu::Cierny => k.cierny_cez_buffer(),
            (_, Some((r, kr))) => stav == StavObrazu::Zapnuty && kr.sirka < r.sirka as u32,
            _ => false,
        }
    }

    /// Popis pre riadok Chlad a záznam.
    fn popis(&self) -> String {
        match &self.kms_nastavenie {
            Some((r, k)) if k.sirka < r.sirka as u32 => {
                format!("obraz {} · {} b · kreslí {}×{}", r.popis(), k.bitov, k.sirka, k.vyska)
            }
            Some((r, k)) => format!("obraz {} · {} b", r.popis(), k.bitov),
            None => format!("obraz {}×{} (obyčajná obrazovka)", self.sirka, self.vyska),
        }
    }

    fn zapis(&mut self, posun: u64, bajty: &[u8]) -> bool {
        match &mut self.vystup {
            Vystup::Fb(f) => f.write_at(bajty, posun).is_ok(),
            Vystup::Kms(k) => match k.pamat() {
                Some((pamat, ..)) => {
                    let od = posun as usize;
                    match pamat.get_mut(od..od + bajty.len()) {
                        Some(ciel) => {
                            ciel.copy_from_slice(bajty);
                            true
                        }
                        None => false,
                    }
                }
                None => false,
            },
        }
    }

    /// Zapíše plátno; vráti počet zapísaných riadkov.
    fn ukaz(&mut self, p: Platno) -> usize {
        let mut bajty = Vec::with_capacity(self.sirka * self.format.bajtov);
        let mut zapisane = 0;
        for y in 0..self.vyska {
            let riadok = &p.body[y * self.sirka..(y + 1) * self.sirka];
            if self.predosle.as_ref().is_some_and(|q| &q.body[y * self.sirka..(y + 1) * self.sirka] == riadok)
            {
                continue;
            }
            self.format.preved(riadok, &mut bajty);
            let posun = self.zaciatok + y as u64 * self.riadok_bajtov;
            if self.zapis(posun, &bajty) {
                zapisane += 1;
            }
        }
        self.predosle = Some(p);
        zapisane
    }

    /// Zapíše na obrazovku len obdĺžnik z posledného plátna (R109 – hodiny).
    fn ukaz_obdlznik(&mut self, x: usize, y: usize, w: usize, v: usize) {
        let Some(p) = self.predosle.take() else { return };
        if x < self.sirka {
            let mut bajty = Vec::with_capacity(w * self.format.bajtov);
            for yy in y..(y + v).min(self.vyska) {
                let od = yy * self.sirka + x;
                let riadok = &p.body[od..od + w.min(self.sirka - x)];
                self.format.preved(riadok, &mut bajty);
                let posun = self.zaciatok + yy as u64 * self.riadok_bajtov + (x * self.format.bajtov) as u64;
                self.zapis(posun, &bajty);
            }
        }
        self.predosle = Some(p);
    }
}

/// Takt obvodu obrazu VPU cez vlastný ovládač jadra `sklenik-vpu` (R166).
struct TaktVpu {
    cesta: PathBuf,
    teraz: Option<u32>,
}

impl TaktVpu {
    fn otvor() -> Option<TaktVpu> {
        let cesta = PathBuf::from("/sys/bus/platform/devices/sklenik-vpu/takt_mhz");
        let teraz = citaj(&cesta)?.parse().ok();
        Some(TaktVpu { cesta, teraz })
    }

    /// Nastaví takt; vráti `true`, ak sa zmenil.
    fn nastav(&mut self, mhz: u32) -> bool {
        if self.teraz == Some(mhz) {
            return false;
        }
        match fs::write(&self.cesta, mhz.to_string()) {
            Ok(()) => {
                let skutocny = citaj(&self.cesta).and_then(|v| v.parse().ok());
                hlas(&format!(
                    "takt VPU: {} → {} MHz (vlastný ovládač, prepnutie bez zákmitu)",
                    self.teraz.map_or("?".into(), |t| t.to_string()),
                    skutocny.map_or("?".into(), |t: u32| t.to_string())
                ));
                // pamätá sa želaný takt (skutočný sa líši zaokrúhlením deliteľa)
                self.teraz = Some(mhz);
                true
            }
            Err(e) => {
                hlas(&format!("takt VPU: {mhz} MHz sa nepodaril: {e}"));
                self.teraz = Some(mhz);
                false
            }
        }
    }
}

/// Merané takty čipu Amlogic (merač taktov v čipe, debugfs): VPU, PLL pamäte DDR, bod HDMI.
fn takty_merane() -> Option<String> {
    let d = Path::new("/sys/kernel/debug/meson-clk-msr/clks");
    let mhz = |n: &str| citaj(d.join(n)).and_then(|v| v.parse::<f64>().ok()).map(|hz| hz / 1e6);
    let casti: Vec<String> =
        [("VPU", "vpu"), ("PLL pamäte DDR", "ddr_dpll_pt"), ("bod HDMI", "hdmi_tx_pixel")]
            .iter()
            .filter_map(|(popis, n)| mhz(n).map(|v| format!("{popis} {v:.0} MHz")))
            .collect();
    (!casti.is_empty()).then(|| casti.join(" · "))
}

fn rychlost_siete(mbs: u32) -> String {
    if mbs >= 1000 { "1 Gb/s".into() } else { format!("{mbs} Mb/s") }
}

/// R165: dekodér videa vo vlastnom vlákne – zoznam formátov a (s nastavením `video=ano`) skúška vlastnými
/// tokmi H.264 a HEVC s presne známym obrazom. Každý krok sa hneď oznámi (keby dekodér zamrzol, v denníku
/// ostane, pri ktorom formáte).
/// Poistka skúšky videa na karte: vznikne pred skúškou, zmizne po nej. Ak ju KLÍČ pri štarte nájde, predošlá
/// skúška nedobehla (krabička zamrzla alebo sa reštartovala) – skúška sa vynechá, aby sa štart nezacyklil.
#[cfg(target_pointer_width = "64")]
const POISTKA_VIDEA: &str = "VIDEO-BEZI.TXT";

fn spusti_skusku_videa(skusat: bool, spustaci: Option<String>, riadky: Arc<Mutex<Vec<String>>>) {
    std::thread::spawn(move || {
        let pridaj = |r: String| {
            hlas(&format!("video: {r}"));
            if let Ok(mut v) = riadky.lock() {
                v.push(r);
            }
        };
        #[cfg(target_pointer_width = "64")]
        {
            use crate::{v4l2, video};
            let Some(d) = v4l2::najdi() else {
                pridaj("hardvérový dekodér nenájdený".into());
                return;
            };
            pridaj(format!("dekodér {}", d.popis()));
            if !skusat {
                pridaj("skúška vlastnými tokmi vypnutá (zapne ju riadok video=ano v SKLENIK.TXT)".into());
                return;
            }
            let nosic = zaznam::najdi_nosice(spustaci.as_deref()).into_iter().next();
            match nosic.as_ref().map(|n| {
                zaznam::poistka(
                    n,
                    POISTKA_VIDEA,
                    zaznam::Poistka::Vytvor(
                        "Skúška dekodéra videa práve beží. Ak tento súbor zostal na karte, skúška nedobehla\r\n\
                         (krabička zamrzla alebo sa reštartovala) a KLÍČ ju pri ďalšom štarte vynechá.\r\n\
                         Znova ju zapneš zmazaním tohto súboru.\r\n",
                    ),
                )
            }) {
                Some(Ok(true)) => {
                    pridaj(format!(
                        "predošlá skúška NEDOBEHLA (zamrznutie alebo reštart) – skúška vynechaná; znova ju zapne \
                         zmazanie MERANIA/{POISTKA_VIDEA} na karte"
                    ));
                    return;
                }
                Some(Ok(false)) => {}
                Some(Err(e)) => pridaj(format!("poistka na karte sa nezapísala ({e}) – skúška beží bez nej")),
                None => pridaj("karta s FAT nenájdená – skúška beží bez poistky".into()),
            }
            // 320×240 a 720×576 (šírka nie je násobkom 32 – overí aj zarovnanie riadkov NV12)
            for (sirka, vyska) in [(320, 240), (720, 576)] {
                let snimky: Vec<video::Obraz> = (0..8).map(|n| video::vzor(sirka, vyska, n)).collect();
                // R202: H.264 aj bez PCM (Intra 16×16 s DC) – hlavné jadro I_PCM nevie, overí sa bežná cesta
                let (dc_toky, dc_cakane) = video::h264_dc(sirka, vyska, 8);
                for (f, meno, toky, cakane, druh) in [
                    (v4l2::H264, "H.264", video::h264_pcm(&snimky), &snimky, "bloky PCM"),
                    (v4l2::H264, "H.264 bez PCM", dc_toky, &dc_cakane, "Intra 16×16 s DC"),
                    (v4l2::HEVC, "HEVC", video::hevc_pcm(&snimky), &snimky, "bloky PCM"),
                ] {
                    if !d.vie(f) {
                        if sirka == 320 {
                            pridaj(format!("{meno}: dekodér tento formát nepozná"));
                        }
                        continue;
                    }
                    pridaj(format!(
                        "{meno} {sirka}×{vyska}: skúška – {} snímok ({druh}, presne známy obraz)",
                        cakane.len()
                    ));
                    let mut jadro = kmsg_od_konca();
                    match v4l2::skus(&d.cesta, f, &toky, cakane, Duration::from_secs(10)) {
                        Ok(v) => pridaj(format!("{meno} {sirka}×{vyska}: {}", v.popis(cakane.len()))),
                        Err(e) => pridaj(format!("{meno} {sirka}×{vyska}: chyba – {e}")),
                    }
                    for r in jadro.as_mut().map(kmsg_nove).unwrap_or_default() {
                        pridaj(format!("  jadro: {r}"));
                    }
                }
            }
            // R177: HEVC so skupinami snímok – P-snímky kopírujú predošlú snímku cez referenciu (RPS, zoznam L0,
            // kompenzácia pohybu, vektory, správa snímok); časovanie 25 snímok/s – dekodér znižuje takt
            if d.vie(v4l2::HEVC) {
                let obrazy: Vec<video::Obraz> = (0..96).map(|n| video::vzor(320, 240, n)).collect();
                let (toky, cakane) = video::hevc_gop(&obrazy, 8, Some(25));
                pridaj(format!(
                    "HEVC skupiny 320×240: skúška – {} snímok (každá 8. IDR s PCM, ostatné P cez referenciu)",
                    cakane.len()
                ));
                let mut jadro = kmsg_od_konca();
                match v4l2::skus(&d.cesta, v4l2::HEVC, &toky, &cakane, Duration::from_secs(20)) {
                    Ok(v) => pridaj(format!("HEVC skupiny 320×240: {}", v.popis(cakane.len()))),
                    Err(e) => pridaj(format!("HEVC skupiny 320×240: chyba – {e}")),
                }
                for r in jadro.as_mut().map(kmsg_nove).unwrap_or_default() {
                    pridaj(format!("  jadro: {r}"));
                }
            }
            if let Some(n) = nosic.as_ref() {
                match zaznam::poistka(n, POISTKA_VIDEA, zaznam::Poistka::Zmaz) {
                    Ok(_) => pridaj("skúška dobehla – poistka z karty zmazaná".into()),
                    Err(e) => pridaj(format!("poistku sa nepodarilo zmazať ({e})")),
                }
            }
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            let _ = (skusat, spustaci);
            pridaj("dekodér videa: len s 64-bitovým jadrom".into());
        }
    });
}

/// Popis odpojených radičov kariet (R178).
fn popis_radicov(r: &[radice::Radic], odpojene: bool) -> String {
    match (r.is_empty(), odpojene) {
        (true, _) => "nič na odpojenie".into(),
        (false, true) => {
            format!("odpojené – {}", r.iter().map(radice::Radic::riadok).collect::<Vec<_>>().join(", "))
        }
        (false, false) => "pripojené".into(),
    }
}

/// R184: počty dlhého testu hardvéru (zdieľané s vláknami záťaže).
#[derive(Default)]
struct StatZataze {
    vypoctov: AtomicU64,
    chyb_jadier: AtomicU64,
    mb_pamate: AtomicU64,
    chyb_pamate: AtomicU64,
}

/// R184: bežiaca záťaž – vlákna s najnižšou prioritou (KLÍČ a strážny časovač majú vždy prednosť).
struct Zataznik {
    druh: chlad::Zataz,
    stop: Arc<AtomicBool>,
    vlakna: Vec<std::thread::JoinHandle<()>>,
}

impl Zataznik {
    fn spusti(druh: chlad::Zataz, stat: &Arc<StatZataze>) -> Zataznik {
        let stop = Arc::new(AtomicBool::new(false));
        let pocet = match druh {
            chlad::Zataz::Jadra => std::thread::available_parallelism().map_or(4, |n| n.get()),
            chlad::Zataz::Pamat => 2,
            chlad::Zataz::Ziadna => 0,
        };
        let vlakna = (0..pocet)
            .map(|_| {
                let (stop, stat) = (stop.clone(), stat.clone());
                std::thread::spawn(move || {
                    let _ = sys::nizka_priorita();
                    while !stop.load(Ordering::Relaxed) {
                        match druh {
                            chlad::Zataz::Jadra => {
                                // úloha so známym výsledkom – jadro, ktoré v teple počíta zle, sa prejaví
                                let r = crate::test_jadier::vypocet();
                                stat.vypoctov.fetch_add(1, Ordering::Relaxed);
                                if r != crate::test_jadier::OCAKAVANE {
                                    stat.chyb_jadier.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            chlad::Zataz::Pamat => match crate::test_pamate::otestuj(256) {
                                Ok(_) => {
                                    stat.mb_pamate.fetch_add(256, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    stat.chyb_pamate.fetch_add(1, Ordering::Relaxed);
                                }
                            },
                            chlad::Zataz::Ziadna => break,
                        }
                    }
                })
            })
            .collect();
        Zataznik { druh, stop, vlakna }
    }

    fn zastav(self) {
        self.stop.store(true, Ordering::Relaxed);
        for v in self.vlakna {
            let _ = v.join();
        }
    }
}

fn popis_zataze(s: &StatZataze, max_teplota: Option<f64>) -> String {
    format!(
        "jadrá – výpočtov {} (každý so známym výsledkom), chýb {} · pamäť – {:.1} GB zapísaných a overených, chýb {} · najvyššia teplota pri záťaži {}",
        s.vypoctov.load(Ordering::Relaxed),
        s.chyb_jadier.load(Ordering::Relaxed),
        s.mb_pamate.load(Ordering::Relaxed) as f64 / 1024.0,
        s.chyb_pamate.load(Ordering::Relaxed),
        max_teplota.map_or("–".into(), |t| format!("{t:.1} °C"))
    )
}

/// R190: obsah súboru `sklenik-obraz.txt` na karte RPi (vkladá ho `config.txt`) – obraz cez KMS (vc4) alebo od
/// firmvéru. Spravuje ho KLÍČ podľa prepínača `obraz_kms` v SKLENIK.TXT, platí od ďalšieho štartu.
const OBRAZ_KMS: &str = include_str!("../../boot/sklenik-obraz.txt");
const OBRAZ_FIRMVER: &str = "# ORCHIDEA OS (R190) – obrazovka od firmvéru (prepínač obraz_kms=nie v SKLENIK.TXT). Súbor spravuje KLÍČ –\n# zmena platí od ďalšieho štartu. Ručne neupravovať.\n[all]\n";

/// R199: zariadenia, na ktorých už táto verzia karty urobila tepelný pokus (riadok „sériové číslo verzia model“).
const POKUSY: &str = "POKUSY.TXT";
/// R215: po koľkých sekundách sa úloha s úložiskom ohlási ako zaseknutá.
const ZASEKNUTE_ULOZISKO: Duration = Duration::from_secs(30);

/// Značka na karte: krabička sa po dlhom teste vypla (R184).
const VYPNUTE_PO_TESTE: &str = "VYPNUTE.TXT";

/// Poistka hlbokého spánku jadier na karte (R167) a ako dlho po zapnutí ostane.
const POISTKA_JADIER: &str = "JADRA-SPANOK.TXT";
const POISTKA_JADIER_S: u64 = 300;

/// R167: hlboký spánok jadier – vlastný ovládač jadra `sklenik_jadra` (cpuidle cez PSCI CPU_SUSPEND).
/// Zapína sa len so strážnym časovačom a s poistkou na karte: ak by firmvér pri vypnutí jadra zamrzol,
/// strážny časovač krabičku do 15 s reštartuje, KLÍČ nájde poistku a hlboký spánok už nezapne.
/// Poistka sa zmaže po 5 min bez problému (chyba firmvéru sa prejaví hneď – jadrá spia tisíckrát za sekundu).
enum SpanokJadier {
    Vypnuty,
    Zapnuty { od: Instant, poistka: Option<zaznam::Nosic>, stav: String, zaciatok: radice::Spanok },
    Zakazany(String),
}

impl SpanokJadier {
    /// Jeden krok; vráti `true`, ak sa stav zmenil (zmena tepla).
    fn krok(&mut self, chce: bool, spustaci: Option<&str>, casovac: bool) -> bool {
        let koren = Path::new("/");
        match self {
            SpanokJadier::Vypnuty if chce => {
                *self = SpanokJadier::zapni(koren, spustaci, casovac);
                matches!(self, SpanokJadier::Zapnuty { .. })
            }
            SpanokJadier::Zapnuty { od, poistka, .. } if chce => {
                if poistka.is_some()
                    && od.elapsed() >= Duration::from_secs(POISTKA_JADIER_S)
                    && let Some(n) = poistka.take()
                {
                    match zaznam::poistka(&n, POISTKA_JADIER, zaznam::Poistka::Zmaz) {
                        Ok(_) => hlas("hlboký spánok jadier: 5 min bez problému – poistka z karty zmazaná"),
                        Err(e) => hlas(&format!("hlboký spánok jadier: poistku sa nepodarilo zmazať ({e})")),
                    }
                }
                false
            }
            SpanokJadier::Zapnuty { poistka, .. } => {
                let _ = radice::nastav_spanok_jadier(koren, false);
                if let Some(n) = poistka.take() {
                    let _ = zaznam::poistka(&n, POISTKA_JADIER, zaznam::Poistka::Zmaz);
                }
                hlas("hlboký spánok jadier: vypnutý (tepelný pokus)");
                *self = SpanokJadier::Vypnuty;
                true
            }
            _ => false,
        }
    }

    fn zapni(koren: &Path, spustaci: Option<&str>, casovac: bool) -> SpanokJadier {
        if !radice::spanok_jadier_je(koren) {
            return SpanokJadier::Zakazany("jadro nemá ovládač sklenik_jadra".into());
        }
        const TEXT: &str = "Hlboký spánok jadier (R167) bol zapnutý a ešte neprešlo 5 minút bez problému.\r\n\
                    Ak tento súbor zostal na karte, krabička medzitým zamrzla alebo sa reštartovala\r\n\
                    a KLÍČ hlboký spánok jadier už nezapne. Znova ho zapneš zmazaním tohto súboru.\r\n";
        // R215: poistka na pozadí s limitom – zaseknutý nosič nezastaví hlavnú slučku
        let spustaci = spustaci.map(str::to_string);
        let poistka = uloziska::na_pozadi("poistka-spanku", move || {
            let n = zaznam::najdi_nosice(spustaci.as_deref()).into_iter().next()?;
            let r = zaznam::poistka(&n, POISTKA_JADIER, zaznam::Poistka::Vytvor(TEXT));
            Some((n, r))
        })
        .pockaj(Duration::from_secs(5));
        let Some(Some((n, poistka))) = poistka else {
            return SpanokJadier::Zakazany(
                "nezapnutý – chýba karta s FAT na poistku alebo neodpovedá".into(),
            );
        };
        match poistka {
            Ok(true) => {
                let d = format!(
                    "nezapnutý – predošlý hlboký spánok nedobehol (zamrznutie alebo reštart); zapne ho zmazanie \
                     MERANIA/{POISTKA_JADIER}"
                );
                hlas(&format!("hlboký spánok jadier: {d}"));
                SpanokJadier::Zakazany(d)
            }
            Ok(false) => {
                // R186: spoločný časovač budenia mimo jadier – až s ním smie hlboko spať aj cpu0 (v pokoji jediné
                // bežiace jadro); zapína sa pod tou istou poistkou ako spánok jadier
                if casovac && radice::casovac_budenia_je(koren) {
                    match radice::zapni_casovac_budenia(koren) {
                        Ok(s) | Err(s) => hlas(&format!("časovač budenia (R186): {s}")),
                    }
                }
                SpanokJadier::zapni_spanok(koren, n)
            }
            Err(e) => SpanokJadier::Zakazany(format!("nezapnutý – poistka sa nezapísala ({e})")),
        }
    }

    fn zapni_spanok(koren: &Path, n: zaznam::Nosic) -> SpanokJadier {
        match radice::nastav_spanok_jadier(koren, true) {
            Ok(stav) => {
                hlas(&format!(
                    "hlboký spánok jadier: {stav} – poistka na karte {} min",
                    POISTKA_JADIER_S / 60
                ));
                SpanokJadier::Zapnuty {
                    od: Instant::now(),
                    poistka: Some(n),
                    stav,
                    zaciatok: radice::Spanok::nacitaj(koren).unwrap_or_default(),
                }
            }
            Err(e) => {
                let _ = zaznam::poistka(&n, POISTKA_JADIER, zaznam::Poistka::Zmaz);
                hlas(&format!("hlboký spánok jadier: {e}"));
                SpanokJadier::Zakazany(e)
            }
        }
    }

    fn popis(&self) -> String {
        match self {
            SpanokJadier::Vypnuty => "vypnutý".into(),
            SpanokJadier::Zakazany(d) => d.clone(),
            SpanokJadier::Zapnuty { od, poistka, stav, zaciatok } => {
                let stat = radice::Spanok::nacitaj(Path::new("/"))
                    .map(|s| s.od(zaciatok).popis(od.elapsed().as_secs_f64()))
                    .unwrap_or_default();
                format!("{stav} · {stat}{}", if poistka.is_some() { " · poistka na karte" } else { "" })
            }
        }
    }
}

/// `/dev/kmsg` od tejto chvíle (staršie hlásenia sa preskočia) – na hlásenia dekodéra počas skúšky.
#[cfg(target_pointer_width = "64")]
fn kmsg_od_konca() -> Option<fs::File> {
    use std::io::Seek;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new().read(true).custom_flags(0o4000).open("/dev/kmsg").ok()?;
    f.seek(std::io::SeekFrom::End(0)).ok()?;
    Some(f)
}

/// Nové hlásenia jadra o dekodéri (vlastný kodek HEVC, meson-vdec) od otvorenia.
#[cfg(target_pointer_width = "64")]
fn kmsg_nove(f: &mut fs::File) -> Vec<String> {
    use std::io::Read;
    let mut v = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let z = String::from_utf8_lossy(&buf[..n]);
                let Some((_, text)) = z.split_once(';') else { continue };
                let text = text.lines().next().unwrap_or("").trim();
                if text.contains("sklenik-hevc") || text.contains("vdec") {
                    v.push(text.chars().take(200).collect());
                }
            }
            // EPIPE: záznam medzitým prepísaný – ďalší
            Err(e) if e.raw_os_error() == Some(32) => continue,
            Err(_) => break,
        }
    }
    v
}

/// Teplota snímača pamäte DDR (zóna „ddr-thermal“), ak ho čip má.
fn teplota_pamate() -> Option<f64> {
    for z in fs::read_dir("/sys/class/thermal").ok()?.flatten() {
        if citaj(z.path().join("type")).is_some_and(|t| t.contains("ddr")) {
            return citaj(z.path().join("temp"))?.parse::<f64>().ok().map(|m| m / 1000.0);
        }
    }
    None
}

/// Otvorí obrazovku: na TV krabičke vlastný ovládač KMS (chladný obraz), inak a pri chybe `/dev/fb0`.
fn otvor_displej(chlad: bool, ciel: Ciel, ostre: bool) -> Option<Displej> {
    if chlad && let Some(d) = Displej::otvor_kms(ciel, ostre) {
        return Some(d);
    }
    Displej::otvor()
}

/// Vstupné zariadenia (klávesnica, myš, diaľkový ovládač) – stlačenie prebudí obraz (R158).
struct Vstupy {
    subory: Vec<fs::File>,
}

impl Vstupy {
    fn otvor() -> Vstupy {
        use std::os::unix::fs::OpenOptionsExt;
        let mut subory = Vec::new();
        for z in fs::read_dir("/dev/input").into_iter().flatten().flatten() {
            if z.file_name().to_string_lossy().starts_with("event")
                && let Ok(f) = fs::OpenOptions::new().read(true).custom_flags(0o4000).open(z.path())
            {
                subory.push(f);
            }
        }
        Vstupy { subory }
    }

    fn fds(&self) -> Vec<i32> {
        use std::os::fd::AsRawFd;
        self.subory.iter().map(|f| f.as_raw_fd()).collect()
    }
}

/// R174: firmvér dekodéra videa Amlogic (mikrokód jeho procesora, linux-firmware) je na karte ako samostatné
/// súbory v `amlogic/vdec/` – licencia Amlogic nedovoľuje vložiť ho do jadra. KLÍČ ho pri štarte skopíruje
/// do `/lib/firmware` v pamäti, odkiaľ si ho ovládač meson-vdec načíta pri prvom videu.
fn nahraj_firmver_videa(zariadenie: &str) {
    const SUBORY: [(&str, &str); 4] = [
        ("amlogic/vdec/g12ah264.bin", "g12a_h264.bin"),
        ("amlogic/vdec/sm1vp9.bin", "sm1_vp9_mmu.bin"),
        ("amlogic/vdec/gxlmpg12.bin", "gxl_mpeg12.bin"),
        ("amlogic/vdec/sm1hevc.bin", "sm1_hevc_mmu.bin"),
    ];
    let ciel = Path::new("/lib/firmware/meson/vdec");
    if !Path::new("/sys/bus/platform/drivers/meson-vdec").exists() || ciel.join(SUBORY[3].1).exists() {
        return;
    }
    let Ok(mut f) = fs::File::open(zariadenie) else { return };
    let _ = fs::create_dir_all(ciel);
    let mut nahrane = Vec::new();
    for (na_karte, meno) in SUBORY {
        match sklenik_karta::citaj_subor(&mut f, na_karte) {
            Ok(data) if fs::write(ciel.join(meno), &data).is_ok() => {
                nahrane.push(format!("{meno} ({} KB)", data.len() / 1024))
            }
            Ok(_) => hlas(&format!("video: firmvér {meno} sa nedal zapísať do /lib/firmware")),
            Err(e) => hlas(&format!("video: firmvér {meno}: {e}")),
        }
    }
    if !nahrane.is_empty() {
        hlas(&format!("video: firmvér dekodéra z karty – {}", nahrane.join(", ")));
    }
}

fn nacitaj_nastavenie(supis: &Supis) -> Option<(String, NastavenieZariadenia)> {
    let mena: Vec<String> = supis.ulozisko.iter().map(|u| u.meno.clone()).collect();
    nacitaj_nastavenie_z(&mena)
}

/// R215: nastavenie z diskov podľa mien; disk s chybou vstupu/výstupu sa označí a ďalej neskúša.
fn nacitaj_nastavenie_z(mena: &[String]) -> Option<(String, NastavenieZariadenia)> {
    for meno in mena {
        if zaznam::je_chybny(meno) {
            continue;
        }
        let cesta = format!("/dev/{meno}");
        let Ok(mut f) = fs::File::open(&cesta) else { continue };
        let mut prvy = [0u8; 512];
        if let Err(e) = f.read_exact_at(&mut prvy, 0) {
            if zaznam::oznac_ak_chybny(meno, &e) {
                hlas(&format!(
                    "úložisko {meno}: chyba čítania ({e}) – správca systému ho už neskúša, záznamy idú na iný nosič (R215)"
                ));
            }
            continue;
        }
        if let Ok(text) = sklenik_karta::citaj_nastavenie(&mut f) {
            let (n, upozornenia) = NastavenieZariadenia::z_textu(&text);
            for up in upozornenia {
                hlas(&format!("nastavenie ({cesta}): {up}"));
            }
            return Some((cesta, n));
        }
    }
    None
}

/// Rýchlosť karty: súvislé čítanie (MB/s) a náhodné čítanie po 4 KB (operácií za sekundu).
/// Len číta – z oblasti, ktorú systém pri štarte nečítal, na karte nič nemení.
/// Každá časť má časový strop, takže aj pomalá karta sa zmeria za pár sekúnd.
fn zmeraj_kartu(zariadenie: &str, bajtov: u64, priebeh: &dyn Fn(u64)) -> std::io::Result<(f64, f64)> {
    let f = fs::File::open(zariadenie)?;
    const KUS: usize = 1 << 20;
    let celkom: u64 = (64u64 << 20).min(bajtov / 4);
    let start = (bajtov / 2) & !0xFFFFF;
    let mut buf = vec![0u8; KUS];
    let t = Instant::now();
    let mut precitane = 0u64;
    while precitane < celkom && t.elapsed() < Duration::from_secs(4) {
        let n = f.read_at(&mut buf, start + precitane)?;
        if n == 0 {
            break;
        }
        precitane += n as u64;
        priebeh(precitane >> 20);
    }
    let suvisle = precitane as f64 / 1e6 / t.elapsed().as_secs_f64().max(1e-6);
    // náhodné 4 KB v hornej štvrtine karty
    let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ bajtov;
    let oblast = (bajtov / 4).max(1 << 20) / 4096;
    let zaciatok = bajtov - oblast * 4096;
    let t = Instant::now();
    let mut pocet = 0u32;
    while t.elapsed() < Duration::from_secs(2) && pocet < 4000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        f.read_exact_at(&mut buf[..4096], zaciatok + (x % oblast) * 4096)?;
        pocet += 1;
    }
    Ok((suvisle, pocet as f64 / t.elapsed().as_secs_f64().max(1e-6)))
}

/// Karta, ktorej rýchlosť sa meria: tá, z ktorej SKLENÍK štartoval (má SKLENIK.TXT), inak prvá SD/eMMC.
fn vyber_kartu(supis: &Supis, nastavenie: &Option<(String, NastavenieZariadenia)>) -> Option<Ulozisko> {
    // nosič, z ktorého SKLENÍK naštartoval (karta alebo USB kľúč – X96 Max Plus vie oboje); inak prvá karta
    nastavenie
        .as_ref()
        .and_then(|(cesta, _)| supis.ulozisko.iter().find(|u| format!("/dev/{}", u.meno) == *cesta))
        .filter(|u| u.meno.starts_with("mmcblk") || u.meno.starts_with("sd"))
        .or_else(|| supis.ulozisko.iter().find(|u| u.meno.starts_with("mmcblk")))
        .cloned()
}

/// Spustí meranie karty vo vlastnom vlákne; priebeh a výsledok zapisuje do `meranie`.
fn spusti_meranie(u: Ulozisko, meranie: Arc<Mutex<MeranieKarty>>) {
    let nastav = move |v: MeranieKarty| {
        if let Ok(mut g) = meranie.lock() {
            *g = v;
        }
    };
    std::thread::spawn(move || {
        // pár sekúnd po štarte, aby meranie nebrzdilo štart
        std::thread::sleep(Duration::from_secs(3));
        nastav(MeranieKarty::Bezi(0));
        hlas(&format!("meriam rýchlosť karty {}", u.meno));
        match zmeraj_kartu(&format!("/dev/{}", u.meno), u.bajtov, &|mb| nastav(MeranieKarty::Bezi(mb))) {
            Ok(v) => {
                nastav(MeranieKarty::Hotovo(v.0, v.1));
                hlas(&format!("karta {}: čítanie {:.1} MB/s, náhodne 4 KB {:.0}/s", u.meno, v.0, v.1));
            }
            Err(e) => {
                nastav(MeranieKarty::Chyba(e.to_string()));
                hlas(&format!("meranie karty sa nepodarilo: {e}"));
            }
        }
    });
}

/// Stlačená pamäť zram na zariadeniach s ≤ 1 GB RAM (R94): stránky, ktoré sa práve nepoužívajú, sa stlačia
/// v pamäti (lz4 – rýchly a nenáročný, procesor sa nezohreje) – použiteľnej pamäte je zhruba o tretinu viac.
/// Na kartu sa nič nezapisuje. Vráti veľkosť v MB.
fn zapni_zram(poda: &ProfilHardveru) -> Option<u64> {
    if !poda.mala_pamat() {
        return None;
    }
    let z = Path::new("/sys/block/zram0");
    if !z.exists() {
        hlas("zram: jadro ho nemá");
        return None;
    }
    let _ = fs::write(z.join("comp_algorithm"), "lz4");
    let stranka = sys::velkost_stranky() as u64;
    let velkost = (poda.pamat_kb * 1024 / 2) / stranka * stranka;
    let vysledok = (|| -> std::io::Result<()> {
        fs::write(z.join("disksize"), velkost.to_string())?;
        // hlavička odkladacej pamäte (ako mkswap): verzia 1, posledná stránka, menovka, podpis na konci stránky
        let mut h = vec![0u8; stranka as usize];
        h[1024..1028].copy_from_slice(&1u32.to_ne_bytes());
        h[1028..1032].copy_from_slice(&((velkost / stranka - 1) as u32).to_ne_bytes());
        h[1052..1059].copy_from_slice(b"SKLENIK");
        let n = h.len();
        h[n - 10..].copy_from_slice(b"SWAPSPACE2");
        fs::OpenOptions::new().write(true).open("/dev/zram0")?.write_all_at(&h, 0)?;
        sys::zapni_odkladanie("/dev/zram0", 100)
    })();
    if let Err(e) = vysledok {
        hlas(&format!("zram sa nepodarilo zapnúť: {e}"));
        return None;
    }
    // stlačená pamäť je rýchla – jadro ju smie používať ochotne a bez čítania dopredu
    let _ = fs::write("/proc/sys/vm/swappiness", "100");
    let _ = fs::write("/proc/sys/vm/page-cluster", "0");
    let algoritmus = citaj(z.join("comp_algorithm"))
        .and_then(|a| a.split('[').nth(1).and_then(|x| x.split(']').next()).map(str::to_string))
        .unwrap_or_else(|| "?".into());
    hlas(&format!(
        "zram: {} MB stlačenej pamäte ({algoritmus}, stránka {} kB)",
        velkost >> 20,
        stranka / 1024
    ));
    Some(velkost >> 20)
}

/// Strážca pravdy (R100): každé zapnuté jadro spočíta úlohu so známym výsledkom (súbežne, pár milisekúnd).
fn test_jadier(poda: &mut ProfilHardveru) {
    let zapnute: Vec<usize> = citaj("/sys/devices/system/cpu/online")
        .map(|s| {
            s.split(',')
                .flat_map(|cast| {
                    let (a, b) = cast.split_once('-').unwrap_or((cast, cast));
                    let (a, b) = (a.trim().parse().unwrap_or(0), b.trim().parse().unwrap_or(0));
                    a..=b
                })
                .collect()
        })
        .unwrap_or_else(|| vec![0]);
    let t = Instant::now();
    let vlakna: Vec<_> = zapnute
        .iter()
        .map(|&j| {
            (j, std::thread::spawn(move || sys::pripni_na_jadro(j).map(|_| crate::test_jadier::vypocet())))
        })
        .collect();
    for (j, v) in vlakna {
        match v.join() {
            Ok(Ok(r)) if r == crate::test_jadier::OCAKAVANE => poda.overene_jadra += 1,
            Ok(Ok(r)) => {
                hlas(&format!("test jadier: jadro {j} počíta chybne ({r:#x})"));
                poda.chybne_jadra.push(j);
            }
            Ok(Err(e)) => hlas(&format!("test jadier: jadro {j} sa nedalo vybrať: {e}")),
            Err(_) => poda.chybne_jadra.push(j),
        }
    }
    hlas(&format!(
        "test jadier: {}/{} v poriadku ({:.0} ms)",
        poda.overene_jadra,
        zapnute.len(),
        t.elapsed().as_secs_f64() * 1000.0
    ));
}

fn uptime() -> Option<f64> {
    citaj("/proc/uptime")?.split_whitespace().next()?.parse().ok()
}

/// Stavová obrazovka – prekreslí sa len pri zmene (žiadne zbytočné prebúdzanie).
fn ukaz_stav(model: &str, teplota: Option<f64>, r: &Regulator, cas_startu: Duration, p: &Supis) {
    let t = teplota.map_or("bez snímača".to_string(), |t| format!("{t:.1} °C"));
    let u = r.uroven();
    let text = format!(
        "\x1b[2J\x1b[H  ORCHIDEA OS - spravca systemu\n\n  {model}\n  start: {:.1} s\n\n  teplota: {t}  (ciel {:.0}, strop {:.0})\n  vykon: uroven {}/{}  jadra {}  takt {} MHz\n",
        cas_startu.as_secs_f64(),
        r.profil().ciel,
        r.profil().strop,
        r.index() + 1,
        r.pocet_urovni(),
        u.jadra,
        u.takt_khz / 1000
    );
    let mut text = text;
    text.push('\n');
    for riadok in p.obrazovka() {
        text.push_str(&format!("  {riadok}\n"));
    }
    if let Ok(mut tty) = fs::OpenOptions::new().write(true).open("/dev/tty1") {
        let _ = tty.write_all(text.as_bytes());
    }
}

pub fn spusti() {
    let start = Instant::now();
    let pid1 = std::process::id() == 1;
    if pid1 {
        pripoj_systemove();
        // R216: čo sa dialo pred predošlým (nečakaným) reštartom
        let predosly = predosly_start();
        if !Path::new("/dev/pmsg0").exists() {
            hlas("záznamník pádov (R216): doska ho nemá – po reštarte nebude vidno, čo mu predchádzalo");
        } else if predosly.is_empty() {
            hlas(
                "záznamník pádov (R216): zapnutý, z predošlého štartu nič (prvý štart alebo vypnutie napájania)",
            );
        } else {
            hlas(&format!("záznamník pádov (R216): PREDOŠLÝ ŠTART – posledných {} riadkov:", predosly.len()));
            for r in predosly {
                hlas(&format!("  predošlý štart: {r}"));
            }
        }
    }
    let prikazovy_riadok = citaj("/proc/cmdline").unwrap_or_default();
    let skuska = prikazovy_riadok.split_whitespace().any(|x| x == "sklenik.skuska=1")
        || std::env::args().any(|a| a == "--skuska");
    let len_sledovanie = !pid1 && !std::env::args().any(|a| a == "--riadit");
    let model = citaj("/proc/device-tree/model").unwrap_or_else(|| "neznáme zariadenie".into());
    let mut poda = orchidea_zariadenie::nacitaj(Path::new("/"));
    test_jadier(&mut poda);
    // R120: skúška pamäte vo vlastnom vlákne (zlomok sekundy) – výsledok sa ukáže, keď dobehne
    let test_pamate: Arc<Mutex<Option<Result<usize, usize>>>> = Arc::new(Mutex::new(None));
    {
        let vysledok = test_pamate.clone();
        let mb = crate::test_pamate::velkost_mb(poda.dostupna_kb);
        std::thread::spawn(move || {
            let r = match crate::test_pamate::otestuj(mb) {
                Ok(t) => {
                    hlas(&format!("test pamäte: {mb} MB v poriadku ({:.2} s)", t.as_secs_f64()));
                    Ok(mb)
                }
                Err(c) => {
                    hlas(&format!(
                        "test pamäte: CHYBA na {} MB – zapísané {:#018x}, prečítané {:#018x}",
                        c.bajt >> 20,
                        c.ocakavane,
                        c.precitane
                    ));
                    Err(c.bajt >> 20)
                }
            };
            if let Ok(mut g) = vysledok.lock() {
                *g = Some(r);
            }
        });
    }
    let mut siet_chyby = if pid1 { zapni_siet() } else { Vec::new() };
    let mut supis = Supis::nacitaj(Path::new("/"));
    let mut podpatie_bolo = supis.podpatie == Some(true);
    let mut nastavenie = nacitaj_nastavenie(&supis);
    // R183: prepínače noviniek musia platiť od štartu – TV krabička chvíľu počká na USB kľúč s nastavením
    if pid1 && !skuska && ma_chladnu_sadu(&model) && nastavenie.is_none() {
        let t = Instant::now();
        while nastavenie.is_none() && t.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(250));
            supis = Supis::nacitaj(Path::new("/"));
            nastavenie = nacitaj_nastavenie(&supis);
        }
        hlas(&format!(
            "nastavenie {} po {:.1} s čakania na kľúč",
            if nastavenie.is_some() { "nájdené" } else { "nenájdené" },
            t.elapsed().as_secs_f64()
        ));
    }
    let nastav_zariadenie = |n: &Option<(String, NastavenieZariadenia)>| {
        if let Some((cesta, nz)) = n
            && pid1
            && nz.novinka("dekoder")
        {
            nahraj_firmver_videa(cesta);
        }
        if let Some((cesta, n)) = n {
            hlas(&format!(
                "nastavenie z {cesta}: meno {}, jazyk {}, Wi-Fi {}",
                n.meno.as_deref().unwrap_or("–"),
                n.jazyk.as_deref().unwrap_or("–"),
                n.wifi.as_deref().unwrap_or("–")
            ));
            if pid1
                && let Some(m) = &n.meno
                && let Err(e) = sys::nastav_meno(m)
            {
                hlas(&format!("meno zariadenia sa nepodarilo nastaviť: {e}"));
            }
        }
    };
    nastav_zariadenie(&nastavenie);
    // R187: Full HD bez kódovania signálu (ako RPi) – jadro to má predvolené; prepínač hdmi_bez_kodovania=nie
    // vráti správanie hlavného jadra (kódovanie aj pri Full HD, keď ho televízor podporuje). Pred prvým obrazom KLÍČA.
    if pid1 && !novinka(&nastavenie, "hdmi_bez_kodovania") && hdmi::krabicka_koduje(Path::new("/")).is_some()
    {
        match hdmi::nastav_kodovanie(Path::new("/"), true) {
            Ok(()) => hlas("HDMI (R187): Full HD s kódovaním signálu (prepínač hdmi_bez_kodovania=nie)"),
            Err(e) => hlas(&format!("HDMI (R187): kódovanie sa nepodarilo zapnúť ({e})")),
        }
    }
    // R122: samomeranie – len ako PID 1 (pripája kartu); vypne sa nastavením `meranie=nie` v SKLENIK.TXT
    let mut merania = pid1.then(zaznam::Riadenie::default);
    let predpona_merania = samomeranie::predpona(&model, "", zaznam::seriove_cislo().as_deref());
    let mut vytazenie_teraz: Option<f64> = None;
    let pismo = Pismo::stonka();
    // R158–R161: správca chladu na TV krabičkách – vlastný ovládač obrazu, USB, sieť, jadrá, tepelný pokus
    let tv = ma_chladnu_sadu(&model) && pid1 && !skuska;
    // R198: nižšie napätie v pokoji len na RPi 2, 3 a Zero 2 W (na RPi 4/5 riadi napätie DVFS firmvéru)
    let pokoj_model = pid1
        && !skuska
        && ["Raspberry Pi 2", "Raspberry Pi 3", "Raspberry Pi Zero 2", "Compute Module 3"]
            .iter()
            .any(|m| model.contains(m));
    let mut pokoj_stav: Option<(pokoj::Stav, bool, zaznam::Nosic)> = None;
    // R199: tepelný pokus sám raz na každom zariadení (pre každú verziu karty) – vlastník nič nepíše
    let pokus_kluc =
        format!("{} {}", zaznam::seriove_cislo().unwrap_or_else(|| model.replace(' ', "_")), crate::VERZIA);
    let mut pokus_hotovy_tu = false;
    let mut pokus_hotovy_text: Option<String> = None;
    // R192: fázy tepelného pokusu podľa zariadenia (RPi: spánok siete zvlášť, bez fáz taktu VPU)
    let je_rpi = model.contains("Raspberry Pi");
    let fazy = chlad::fazy(je_rpi);
    // R169: časovače KLÍČA smú meškať až 50 ms – jadro ich zlúči s inými budeniami
    if tv && novinka(&nastavenie, "casovace") {
        sys::tolerancia_casovacov(50_000_000);
    }
    // R183: vypnuté novinky, ktoré pôsobia už pri štarte
    if pid1 && !skuska {
        if !novinka(&nastavenie, "merac_taktov") && sys::odpoj("/sys/kernel/debug").is_ok() {
            hlas("prepínače: merač taktov vypnutý – debugfs odpojený");
        }
        if !novinka(&nastavenie, "dekoder") {
            let d = odpoj_dekoder();
            hlas(&format!(
                "prepínače: dekodér videa vypnutý – odpojené: {}",
                if d.is_empty() { "nič".into() } else { d.join(", ") }
            ));
        }
        hlas(&format!("prepínače (R183): {}", vypnute_novinky(&nastavenie)));
        // R190: obraz RPi cez KMS alebo od firmvéru – súbor sklenik-obraz.txt na karte podľa prepínača obraz_kms
        if model.contains("Raspberry Pi") && nastavenie.is_some() {
            let spustaci = nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
            let obsah = if novinka(&nastavenie, "obraz_kms") { OBRAZ_KMS } else { OBRAZ_FIRMVER };
            if let Some(n) = zaznam::najdi_nosice(spustaci.as_deref()).into_iter().next() {
                match zaznam::subor_v_koreni(&n, "sklenik-obraz.txt", obsah) {
                    Ok(Some(stary)) if stary == obsah => {}
                    Ok(_) => hlas(&format!(
                        "obraz (R190): od ďalšieho štartu {} (prepínač obraz_kms)",
                        if obsah == OBRAZ_KMS { "vlastný ovládač KMS" } else { "obrazovka od firmvéru" }
                    )),
                    Err(e) => {
                        hlas(&format!("obraz (R190): súbor sklenik-obraz.txt sa nepodarilo zapísať ({e})"))
                    }
                }
            }
        }
        // R199: urobila už táto verzia karty tepelný pokus na tomto zariadení?
        if tv && nastavenie.is_some() {
            let spustaci = nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
            if let Some(n) = zaznam::najdi_nosice(spustaci.as_deref()).into_iter().next()
                && let Ok(Some(t)) = zaznam::subor_merani(&n, POKUSY, None)
            {
                pokus_hotovy_tu = t.lines().any(|r| r.starts_with(&format!("{pokus_kluc} ")));
            }
            let p = nastavenie.as_ref().and_then(|(_, n)| n.pokus);
            hlas(&format!(
                "tepelný pokus (R199): {}",
                match (p, pokus_hotovy_tu) {
                    (Some(false), _) => "vypnutý (pokus=nie)".to_string(),
                    (Some(true), _) => "pri každom štarte (pokus=ano)".into(),
                    (None, true) => "na tomto zariadení s touto kartou už hotový – bežný režim".into(),
                    (None, false) => "prvý štart tejto karty na zariadení – urobí sa sám (2 h 15 min) aj so \
                                      skúškou videa"
                        .into(),
                }
            ));
        }
        // R198: chladný pokoj – nižšie napätie v pokoji (RPi 2, 3, Zero 2 W) s poistkou v súbore na karte
        if pokoj_model && nastavenie.is_some() {
            let spustaci = nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
            if let Some(n) = zaznam::najdi_nosice(spustaci.as_deref()).into_iter().next() {
                let zapnute = novinka(&nastavenie, "pokoj_napatie");
                let mut bezi = false;
                match zaznam::uprav_v_koreni(&n, pokoj::SUBOR, |t| {
                    let (novy, b) = pokoj::pri_starte(pokoj::stav(t), zapnute);
                    bezi = b;
                    pokoj::obsah(novy)
                }) {
                    Ok((_, novy)) => {
                        let st = pokoj::stav(Some(&novy));
                        pokoj_stav = Some((st, bezi, n));
                        hlas(&format!(
                            "podpätie v nečinnosti (chladný pokoj, R198): tento štart {} · nastavenie {}",
                            if bezi {
                                "s nižším napätím v pokoji (−50 mV)"
                            } else {
                                "s pôvodným napätím"
                            },
                            st.popis()
                        ));
                    }
                    Err(e) => hlas(&format!(
                        "podpätie v nečinnosti (chladný pokoj, R198): súbor sa nepodarilo upraviť ({e})"
                    )),
                }
            }
        }
        // R184: predošlý beh sa po dlhom teste vypol? (značku zapísal pred vypnutím)
        let spustaci = nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
        if let Some(n) = zaznam::najdi_nosice(spustaci.as_deref()).into_iter().next()
            && zaznam::poistka(&n, VYPNUTE_PO_TESTE, zaznam::Poistka::Zmaz) == Ok(true)
        {
            hlas("predošlý beh: po dlhom teste hardvéru sa krabička sama vypla (R184) ✓");
        }
    }
    let mut displej = if len_sledovanie {
        None
    } else {
        otvor_displej(tv, ciel_obrazu(&nastavenie), ostry_obraz(&nastavenie))
    };
    let mut obraz_teraz = displej.as_ref().filter(|d| d.je_kms()).map(|_| ciel_obrazu(&nastavenie));
    let profil = Profil::pre_model(&model);
    let mut hw = Sysfs::najdi(smie_parkovat(&model));
    let mut regulator = Regulator::novy(urovne(&hw.takty(), hw.jadier()), profil);
    // R194: parkovať jadrá sa dá len tam, kde ich platforma vie vypnúť (RPi 1–3 nie, RPi 5 zle)
    let parkovat_mozno = !hw.vypinatelne.is_empty();
    // R123–R128: odhad okolia, zdroj tepla, predpoveď, úspora v horúcom okolí, tepelný rozpočet
    let mut okolie = if je_tv_krabicka(&model) { Okolie::s_dlhym_startom() } else { Okolie::nove() };
    // R195: teplota v izbe z teplomera (SKLENIK.TXT izba=) – presné okolie a vlastný ohrev
    if let Some(t) = nastavenie.as_ref().and_then(|(_, n)| n.izba).map(|d| d as f64 / 10.0) {
        okolie.nastav_izbu(t);
        hlas(&format!("okolie: izba {t:.1} °C z teplomera (SKLENIK.TXT) – R195"));
    }
    let mut uspora = false;
    let mut posledny_rozpocet: Option<(Instant, u32, u32)> = None;
    let cas_startu = start.elapsed();
    hlas(&format!(
        "{model}; jadier {}, takty {:?} kHz, snímačov teploty {}, vypínateľných jadier {}, ventilátorov {}; cieľ {:.0} °C, strop {:.0} °C{}",
        hw.jadier(),
        hw.takty(),
        hw.snimace.len(),
        hw.vypinatelne.len(),
        hw.ventilatory.len(),
        profil.ciel,
        profil.strop,
        if len_sledovanie { " (len sledovanie)" } else { "" }
    ));
    hlas(&poda.riadok());
    hlas(&supis.riadok());
    let pripravene = uptime();
    let zram_mb = if pid1 { zapni_zram(&poda) } else { None };
    // meranie rýchlosti karty na pozadí (pár sekúnd po štarte, aby nebrzdilo štart)
    // meranie rýchlosti karty na pozadí – spustí sa, keď je karta v systéme (na RPi 3 sa karta objaví
    // až chvíľu po štarte KLÍČA, preto sa to skúša pri každej zmene periférií)
    let meranie = Arc::new(Mutex::new(MeranieKarty::Caka));
    let mut merana_karta = String::new();
    if let Some(u) = vyber_kartu(&supis, &nastavenie) {
        merana_karta = u.meno.clone();
        spusti_meranie(u, meranie.clone());
    }
    let mut zobrazene_meranie = MeranieKarty::Caka;
    let mut zmena_merania = Instant::now();
    let mut posledne_mb = None;

    let mut zobrazene: Option<(i64, usize)> = None;
    let mut zobrazeny_cas: Option<u64> = None;
    #[allow(clippy::type_complexity)]
    let mut zobrazene_ine: Option<(
        Option<Eee>,
        Vec<String>,
        Option<Result<usize, usize>>,
        Vec<String>,
        Option<obrazovka::Samomeranie>,
        (Option<i64>, &'static str, Option<bool>, Option<i64>, Option<i64>, bool),
    )> = None;
    let mut krokov = 0u32;
    // R99: hardvérový strážny časovač – ak sa KLÍČ zasekne na 15 s, doska sa sama reštartuje
    // v emulátore QEMU sa časovač nespráva ako na doske (reštartuje hneď) – v skúške sa vynechá
    let mut strazny = if pid1 && skuska {
        hlas("strážny časovač: v skúške vynechaný");
        None
    } else if pid1 {
        match sys::otvor_strazny_casovac(15) {
            Ok(f) => {
                hlas(&format!(
                    "strážny časovač: 15 s ({})",
                    citaj("/sys/class/watchdog/watchdog0/identity").unwrap_or_else(|| "?".into())
                ));
                Some(f)
            }
            Err(e) => {
                hlas(&format!("strážny časovač nie je k dispozícii: {e}"));
                None
            }
        }
    } else {
        None
    };
    // R113: udalosti jadra – KLÍČ spí, kým sa niečo nepripojí, alebo kým nie je čas na krok strážcu
    let udalosti = match sys::Udalosti::otvor() {
        Ok(u) => {
            hlas("udalosti jadra: sledované (pripojenie zariadení a kábla bez prehľadávania)");
            Some(u)
        }
        Err(e) => {
            hlas(&format!("udalosti jadra nie sú k dispozícii ({e}) – prehľadávanie každé 2 s"));
            None
        }
    };
    // R119: bez obrazovky (Tanix TX68 – jadro zatiaľ nemá HDMI pre H618) hovorí SKLENÍK kontrolkami klávesnice
    let kontrolky = Arc::new(Mutex::new(StavKontroliek {
        slepy: false,
        porucha: false,
        teplota: None,
        ciel: profil.ciel,
        strop: profil.strop,
    }));
    if pid1 && !len_sledovanie {
        spusti_kontrolky(kontrolky.clone());
    }
    let mut slepy_hlaseny = false;
    let mut eee = SledovanieEee::nove(&supis);
    let mut vstupy = Vstupy::otvor();
    let mut posledna_aktivita = Instant::now();
    let mut alr = SietPodlaPotreby::nova();
    let mut alr_vzorka: Option<(String, u64, u64)> = None;
    let mut alr_prepnuti = 0u32;
    // R170: čip siete bez kábla spí (podľa mena rozhrania)
    let mut kable: std::collections::HashMap<String, chlad::Kabel> = std::collections::HashMap::new();
    let mut kabel_hladani = 0u32;
    // R191: či je kábel zapojený, kým čip siete spí (zistené pri poslednom overení)
    let mut kabel_zapojeny: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut periferie_teraz: Option<bool> = None;
    // R178: nepoužité radiče kariet odpojené od ovládača; R167: hlboký spánok jadier
    let mut radice_odpojene: Vec<radice::Radic> = Vec::new();
    let mut radice_stav: Option<bool> = None;
    let mut spanok_jadier = SpanokJadier::Vypnuty;
    // R184: dlhý test hardvéru – záťaž, jej počty, najvyššia teplota a vypnutie po teste
    let stat_zataze = Arc::new(StatZataze::default());
    let mut zataznik: Option<Zataznik> = None;
    let mut max_teplota_zataze: Option<f64> = None;
    let mut vypnutie_po_teste = false;
    // R185: strážca linky HDMI (SCDC televízora) a ručný reštart linky (5× tlačidlo do 4 s)
    let mut ddc: Option<hdmi::Ddc> = None;
    let mut ddc_dalsi_pokus = Instant::now();
    let mut hdmi_stav: Option<hdmi::StavTv> = None;
    let mut hdmi_chyba: Option<String> = None;
    let mut hdmi_kontrola = Instant::now();
    let mut linka_stav = StavObrazu::Vypnuty;
    let mut linka_od = Instant::now();
    let mut restarty_linky: Vec<Instant> = Vec::new();
    let (mut hdmi_restartov, mut hdmi_oprav) = (0u32, 0u32);
    let mut stlacenia: std::collections::VecDeque<Instant> = std::collections::VecDeque::new();
    let mut rucny_restart = false;
    let (mut limit_hlaseny, mut hdmi_zamknuty) = (false, false);
    let straz_hdmi = novinka(&nastavenie, "straz_hdmi");
    // R184: človek sa krabičky dotkol (ovládač, klávesnica, ovládač TV) – až potom smie dlhý test zapnúť TV
    let mut clovek_bol = false;
    let mut rusene = [false; chlad::FAZY.len()];
    let mut usb_spanok_od: Option<(u64, u64)> = None;
    let mut usb_nastavene: Vec<String> = Vec::new();
    let mut usb_spanok_zakazany = false;
    // R191: koľko zariadení stromu USB smie spať a stav času spánku pri zapnutí
    let mut usb_strom = 0usize;
    let mut usb_strom_od: Option<(u64, u64)> = None;
    // R192: zariadenia priamo na koreňovom porte – keď spia, radič USB neposiela mikrorámce
    let mut usb_zbernica_od: Option<(u64, u64)> = None;
    // R201: koreňové rozbočovače (xHCI) – spia, keď stojí celá zbernica
    let mut usb_koren_od: Option<(u64, u64)> = None;
    let mut obrazovka_zmenena = false;
    let mut pokus = false;
    // R192: fáza pokusu dovoľuje spánok siete s káblom (mimo pokusu vždy)
    let mut siet_spi_faza = true;
    // R215: práca s úložiskami na pozadí – hlavná slučka (strážny časovač) nikdy nečaká na kartu ani kľúč
    let mut hladanie_nastavenia: Option<uloziska::Cakanie<Option<(String, NastavenieZariadenia)>>> = None;
    let mut hladanie_nosicov: Option<uloziska::Cakanie<Vec<String>>> = None;
    let mut nosice: Option<Vec<String>> = None;
    let mut pozadie: Vec<uloziska::Cakanie<()>> = Vec::new();
    let mut pokus_zastaveny = false;
    let mut faza_teraz: Option<usize> = None;
    let mut zobrazeny_chlad: Option<obrazovka::Chlad> = None;
    let mut prekreslit = false;
    // R162, R163: merač pamäte (počítadlá radiča DDR) a merač budení
    // merač sa otvára len na krátke okná (R176) – tu sa len zistí, či ho jadro má
    // R183: merač pamäte sa pri štarte neotvára (otvorenie a zatvorenie zapne a vypne monitor radiča pamäte
    // DDR, ktorý ovplyvňuje čítanie obrazu); stačí, že ho jadro má – otvorí sa len v oknách tepelného pokusu
    let merac_pamate_je = novinka(&nastavenie, "merac_pamate")
        && Path::new("/sys/bus/event_source/devices/meson_ddr_bw").exists();
    hlas(&format!(
        "merač pamäte: {}",
        if merac_pamate_je {
            "áno – tok DDR podľa častí čipu, okno 2 s raz za minútu (nie pri plnom obraze)"
        } else {
            "nie je (iný čip alebo jadro bez neho)"
        }
    ));
    let mut okno_pamate: Option<(pamat::MeracPamate, Instant, [u64; 5])> = None;
    let mut posledne_okno: Option<Instant> = None;
    let mut tok_pamate: Option<pamat::Tok> = None;
    let mut tok_zobrazeny: Option<(Instant, pamat::Tok)> = None;
    let mut pamat_pri_zazname = pamat::SumaToku::default();
    let mut pamat_fazy: Vec<(usize, pamat::SumaToku)> = Vec::new();
    let mut budenia_pri_zazname: Option<(Instant, Vec<(String, u64)>)> = None;
    let mut ochrana_pamate = false;
    // R181: vlastný ovládač CEC – televízor a krabička sa dohovoria cez HDMI (otvára sa v slučke, keď je
    // známe nastavenie a jadro má adaptér /dev/cec0)
    let mut cec: Option<cec::Cec> = None;
    let mut cec_stav = String::from("zatiaľ neotvorené");
    let mut cec_dalsi_pokus_s = 5.0;
    let mut cec_obraz: Option<StavObrazu> = None;
    // R203: kedy krabička naposledy budila televízor (kontrola, či sa ozval)
    let mut tv_budenie_od: Option<Instant> = None;
    // televízor išiel spať – obraz krabičky sa vypne, kým sa niekto neozve
    let mut tv_spi = false;
    // R166: takt obvodu obrazu podľa potreby (vlastný ovládač jadra)
    let mut takt_vpu = if tv && novinka(&nastavenie, "takt_obrazu") { TaktVpu::otvor() } else { None };
    if let Some(t) = &takt_vpu {
        hlas(&format!("takt VPU: {} MHz – riadi ho KLÍČ (R166)", t.teraz.unwrap_or(0)));
    }
    let mut vytazenie = Vytazenie::nove();
    let mut kmsg = Hlasenia::otvor(&model);
    let mut straz_sd = radice::StrazSd::default();
    kmsg.citaj();
    let mut usb_ozivene = false;
    // R165: hardvérový dekodér videa – výsledky skúšky (vlákno) a či už bežala
    let video_riadky: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut video_spustene = false;
    let mut bez_teploty_hlasene = false;
    let mut teplota = hw.teplota();
    let mut posledny_krok = Instant::now();
    let mut dalsi_krok = Instant::now();
    let mut posledny_supis = Instant::now();
    let mut udalost = false;
    loop {
        // krok strážcu: v pokoji každé 4 s, inak každú sekundu (udalosti ho neurýchľujú – sklon teploty
        // sa počíta z rovnomerných krokov)
        if Instant::now() >= dalsi_krok {
            let dt = posledny_krok.elapsed().as_secs_f64().max(0.001);
            posledny_krok = Instant::now();
            teplota = hw.teplota();
            let v = vytazenie.krok();
            vytazenie_teraz = v;
            // R123: odhad okolia z minútových priemerov; R125: predpoveď ustálenej teploty pre strážcu
            if let Some(t) = teplota {
                okolie.krok(uptime().unwrap_or(0.0), t, v);
            }
            let stav_okolia = okolie.stav();
            regulator.nastav_predpoved(stav_okolia.ustalena);
            // R126: horúce okolie – šetrí sa aj nečinnosť (raz za beh; nečinný príkon sa zmení → prepočet)
            // počas tepelného pokusu sa nemení nič okrem fáz (R160 – porovnanie musí byť čisté)
            let pokus_bezi = pokus && faza_teraz.is_some();
            if pid1 && !skuska && !uspora && !pokus_bezi && stav_okolia.prostredie.horuce() {
                uspora = true;
                regulator.nastav_usporu(true);
                hlas(&format!(
                    "okolie {}: {} – úsporný režim (nečinnosť na úspornej úrovni, Ethernet bez gigabitu)",
                    stav_okolia.okolie.map_or("?".into(), |o| format!("≈ {o:.0} °C")),
                    stav_okolia.prostredie.slovo()
                ));
                // TV krabička má sieť podľa potreby (R159) – rýchlosť riadi tá
                for r in supis
                    .siet
                    .iter()
                    .filter(|r| !tv && !r.bezdrotove && r.rychlost_mbs.is_some_and(|v| v >= 1000))
                {
                    match sys::ethernet_bez_gigabitu(&r.meno) {
                        Ok(true) => {
                            hlas(&format!("sieť {}: ponúka už len 10/100 Mb/s (menej tepla)", r.meno))
                        }
                        Ok(false) => {}
                        Err(e) => {
                            hlas(&format!("sieť {}: obmedzenie na 100 Mb/s sa nepodarilo: {e}", r.meno))
                        }
                    }
                }
                okolie.prekalibruj();
            }
            let uroven = match teplota {
                Some(t) => regulator.krok_s_vytazenim(t, dt, v),
                // R99: snímač teploty bol, ale nedá sa prečítať (napr. zlyhal firmvér) – nevieme, že je
                // chladno, preto bezpečný režim: strážca znižuje výkon, akoby bolo nad stropom
                None if !hw.snimace.is_empty() => {
                    if !bez_teploty_hlasene {
                        hlas("teplotu sa nepodarilo prečítať – bezpečný režim (nižší výkon)");
                        bez_teploty_hlasene = true;
                    }
                    regulator.krok(regulator.profil().strop + 5.0, dt)
                }
                None => regulator.uroven(),
            };
            // KLÍČ žije – nakŕmiť strážny časovač
            if let Some(f) = strazny.as_mut() {
                let _ = f.write_all(b"\0");
            }
            if !len_sledovanie {
                hw.uplatni(&uroven);
                // R162, R176: tok pamäte v krátkom okne raz za minútu – nie pri obraze s veľkým tokom (pôvodný
                // alebo ostrý obraz na celej ploche), lebo merač by rušil HDMI
                // R177: len počas tepelného pokusu – v bežnom režime sa merač nespúšťa vôbec
                let smie_merat = merac_pamate_je
                    && pokus
                    && faza_teraz.is_some()
                    && displej.as_ref().is_some_and(|d| {
                        d.stav_obrazu() != StavObrazu::Zapnuty || d.zvacsuje(StavObrazu::Zapnuty)
                    });
                match okno_pamate.take() {
                    Some((mut m, t0, s0)) if !smie_merat || t0.elapsed().as_secs_f64() >= pamat::OKNO_S => {
                        if smie_merat && let Some(s1) = m.stav() {
                            let t = pamat::Tok::z_rozdielu(&s0, &s1, t0.elapsed().as_secs_f64());
                            tok_pamate = Some(t);
                            pamat_pri_zazname.pridaj(&t);
                            if let Some((_, f)) = pamat_fazy.last_mut() {
                                f.pridaj(&t);
                            }
                        }
                        drop(m);
                        posledne_okno = Some(Instant::now());
                    }
                    Some(o) => okno_pamate = Some(o),
                    None if smie_merat
                        && posledne_okno
                            .is_none_or(|t| t.elapsed().as_secs_f64() >= pamat::OKNO_KAZDYCH_S) =>
                    {
                        if let Some(mut m) = pamat::MeracPamate::otvor()
                            && let Some(s0) = m.stav()
                        {
                            okno_pamate = Some((m, Instant::now(), s0));
                        }
                    }
                    None => {}
                }
                if !smie_merat {
                    tok_pamate = None;
                }
                // R159: sieť podľa potreby – prenos za posledný krok rozhodne o gigabite
                if tv
                    && periferie_teraz == Some(true)
                    && novinka(&nastavenie, "siet")
                    && let Some(r) = supis.siet.iter().find(|r| !r.bezdrotove && r.spojenie == Some(true))
                {
                    let cesta = format!("/sys/class/net/{}/statistics", r.meno);
                    let bajty = |f: &str| citaj(format!("{cesta}/{f}")).and_then(|v| v.parse::<u64>().ok());
                    if let (Some(rx), Some(tx)) = (bajty("rx_bytes"), bajty("tx_bytes")) {
                        if let Some((_, rx0, tx0)) = alr_vzorka.as_ref().filter(|v| v.0 == r.meno) {
                            let bajtov = rx.saturating_sub(*rx0).max(tx.saturating_sub(*tx0));
                            let mbit = bajtov as f64 * 8.0 / dt / 1e6;
                            if let Some(g) = alr.krok(dt, mbit) {
                                match sys::ethernet_rychlost(&r.meno, g) {
                                    Ok(_) => {
                                        alr_prepnuti += 1;
                                        hlas(&format!(
                                            "sieť {}: najviac {g} Mb/s (prenos {mbit:.1} Mb/s) – sieť podľa potreby",
                                            r.meno
                                        ));
                                    }
                                    Err(e) => {
                                        hlas(&format!("sieť {}: zmena rýchlosti sa nepodarila: {e}", r.meno))
                                    }
                                }
                            }
                        }
                        alr_vzorka = Some((r.meno.clone(), rx, tx));
                    }
                }
                // R170: čip siete bez kábla spí; raz za 30 s sa na chvíľu zobudí a hľadá kábel
                // R191: sieť zatiaľ nič nepotrebuje (SKLENÍK nemá sieťové služby) – spí aj s káblom
                let siet_potrebna = !(novinka(&nastavenie, "siet_spi") && siet_spi_faza);
                if tv && periferie_teraz == Some(true) && novinka(&nastavenie, "siet") {
                    for r in supis.siet.iter().filter(|r| !r.bezdrotove && !siet_chyby.contains(&r.meno)) {
                        let k = kable.entry(r.meno.clone()).or_insert_with(chlad::Kabel::novy);
                        let spojenie =
                            citaj(format!("/sys/class/net/{}/carrier", r.meno)).as_deref() == Some("1");
                        let Some(p) = k.krok_s_potrebou(dt, spojenie, siet_potrebna) else { continue };
                        let zapnut = matches!(p, chlad::PrechodKabla::Hladaj);
                        match p {
                            chlad::PrechodKabla::Najdeny | chlad::PrechodKabla::Overeny => {
                                kabel_zapojeny.insert(r.meno.clone(), true);
                            }
                            chlad::PrechodKabla::Spat => {
                                kabel_zapojeny.insert(r.meno.clone(), false);
                            }
                            chlad::PrechodKabla::Uspi => {
                                kabel_zapojeny.insert(r.meno.clone(), spojenie);
                            }
                            chlad::PrechodKabla::Hladaj => {}
                        }
                        if p != chlad::PrechodKabla::Najdeny
                            && let Err(e) = sys::nastav_rozhranie(&r.meno, zapnut)
                        {
                            hlas(&format!("sieť {}: {e} – čip siete ostáva zapnutý", r.meno));
                            *k = chlad::Kabel::novy();
                            continue;
                        }
                        match p {
                            chlad::PrechodKabla::Uspi if spojenie => hlas(&format!(
                                "sieť {}: kábel zapojený, ale sieť zatiaľ nič nepotrebuje – čip siete spí, kábel sa \
                                 overí raz za {} min (R191)",
                                r.meno,
                                chlad::SPANOK_NEPOTREBNEJ_S as u32 / 60
                            )),
                            chlad::PrechodKabla::Uspi => hlas(&format!(
                                "sieť {}: bez kábla – čip siete spí, kábel sa hľadá raz za {} s",
                                r.meno,
                                if siet_potrebna { chlad::SPANOK_PHY_S } else { chlad::SPANOK_NEPOTREBNEJ_S }
                                    as u32
                            )),
                            chlad::PrechodKabla::Overeny => {}
                            chlad::PrechodKabla::Najdeny => {
                                hlas(&format!("sieť {}: kábel pripojený – čip siete zapnutý", r.meno))
                            }
                            chlad::PrechodKabla::Hladaj => kabel_hladani += 1,
                            chlad::PrechodKabla::Spat => {}
                        }
                    }
                }
                if let Some(t) = teplota {
                    // R129: v horúcom okolí sa ventilátor zapína o 5 °C skôr
                    hw.ventiluj(if stav_okolia.prostredie.horuce() { t + 5.0 } else { t }, profil);
                }
                // R127: tepelný rozpočet pre programy – pri zmene úrovne a raz za 30 s (len zápis do pamäte)
                if posledny_rozpocet.is_none_or(|(kedy, j, f)| {
                    kedy.elapsed() >= Duration::from_secs(30) || (j, f) != (uroven.jadra, uroven.takt_khz)
                }) {
                    let _ = fs::create_dir_all("/run/sklenik");
                    let _ = fs::write(
                        ROZPOCET,
                        okolie::rozpocet(
                            &stav_okolia,
                            teplota,
                            profil.ciel,
                            regulator.predpoved_30s(),
                            uroven.jadra,
                            uroven.takt_khz,
                        ),
                    );
                    posledny_rozpocet = Some((Instant::now(), uroven.jadra, uroven.takt_khz));
                }
            }
            if pid1 {
                sys::zber_procesov();
                // R191: zmeny spojenia, ktoré spôsobuje uspávanie čipu siete, nie sú výpadky
                eee.krok(
                    tv && periferie_teraz == Some(true)
                        && novinka(&nastavenie, "siet")
                        && novinka(&nastavenie, "siet_spi")
                        && siet_spi_faza,
                );
            }
            // R215: výsledky práce s úložiskami na pozadí; zaseknuté úložisko sa len ohlási
            if let Some(c) = hladanie_nastavenia.as_mut() {
                match c.vyzdvihni(ZASEKNUTE_ULOZISKO) {
                    uloziska::Stav::Hotovo(n) => {
                        hladanie_nastavenia = None;
                        if n.is_some() {
                            nastavenie = n;
                            nastav_zariadenie(&nastavenie);
                            // spúšťací disk je teraz známy – poradie nosičov sa zistí znova
                            nosice = None;
                        }
                    }
                    uloziska::Stav::Zaseknuta(t) => hlas(&format!(
                        "úložisko neodpovedá {} s (hľadanie nastavenia) – hlavný program pokračuje (R215)",
                        t.as_secs()
                    )),
                    uloziska::Stav::Stratena => hladanie_nastavenia = None,
                    uloziska::Stav::Bezi => {}
                }
            }
            if nosice.is_none() && hladanie_nosicov.is_none() {
                let spustaci = nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
                hladanie_nosicov = Some(uloziska::na_pozadi("nosice", move || {
                    zaznam::najdi_nosice(spustaci.as_deref()).into_iter().map(|n| n.oddiel).collect()
                }));
            }
            if let Some(c) = hladanie_nosicov.as_mut() {
                match c.vyzdvihni(ZASEKNUTE_ULOZISKO) {
                    uloziska::Stav::Hotovo(v) => {
                        hladanie_nosicov = None;
                        nosice = Some(v);
                    }
                    uloziska::Stav::Zaseknuta(t) => hlas(&format!(
                        "úložisko neodpovedá {} s (zoznam nosičov) – hlavný program pokračuje (R215)",
                        t.as_secs()
                    )),
                    uloziska::Stav::Stratena => hladanie_nosicov = None,
                    uloziska::Stav::Bezi => {}
                }
            }
            pozadie.retain_mut(|c| match c.vyzdvihni(ZASEKNUTE_ULOZISKO) {
                uloziska::Stav::Hotovo(()) | uloziska::Stav::Stratena => false,
                uloziska::Stav::Zaseknuta(t) => {
                    hlas(&format!(
                        "úložisko neodpovedá {} s ({}) – hlavný program pokračuje (R215)",
                        t.as_secs(),
                        c.meno()
                    ));
                    true
                }
                uloziska::Stav::Bezi => true,
            });
            if let Ok(mut k) = kontrolky.lock() {
                k.teplota = teplota;
                k.porucha = !poda.chybne_jadra.is_empty()
                    || matches!(test_pamate.lock().ok().and_then(|g| *g), Some(Err(_)));
                k.slepy = pid1 && displej.is_none();
                if k.slepy && !slepy_hlaseny {
                    hlas(
                        "bez obrazovky – stav na kontrolkách klávesnice (Num Lock bliká = beží; každú minútu teplota: Scroll Lock desiatky, Caps Lock jednotky)",
                    );
                    slepy_hlaseny = true;
                }
            }
            // R122: bod krivky teploty – z hodnôt, ktoré strážca práve odčítal (nič sa nemeria navyše)
            if let Some(m) = merania.as_mut() {
                m.bod(teplota, uroven.vykon, uroven.takt_khz / 1000, v, stav_okolia.okolie);
            }
            // v pokoji sa strážca prebúdza zriedkavejšie
            let pokoj = teplota.unwrap_or(0.0) < regulator.profil().ciel - 10.0;
            dalsi_krok = posledny_krok + Duration::from_secs(if pokoj { 4 } else { 1 });
        }

        // periférie: pri udalosti jadra, inak raz za 10 s (podpätie, ktoré udalosť nemá); bez udalostí 2 s
        let interval = if udalosti.is_some() { 10 } else { 2 };
        let zmena_periferii = if udalost || posledny_supis.elapsed() >= Duration::from_secs(interval) {
            posledny_supis = Instant::now();
            let mut novy = Supis::nacitaj(Path::new("/"));
            // R170: rozhranie, ktoré správca uspal alebo práve hľadá kábel, je „kábel nepripojený“ – nie zmena
            for r in novy.siet.iter_mut() {
                if r.spojenie != Some(true) && kable.get(&r.meno).is_some_and(|k| k.riadeny()) {
                    r.spojenie = Some(false);
                    r.rychlost_mbs = None;
                    if let Some(stary) = supis.siet.iter().find(|x| x.meno == r.meno) {
                        r.cip.clone_from(&stary.cip);
                    }
                }
            }
            let zmena = novy != supis;
            if zmena {
                if pid1 && novy.siet.len() != supis.siet.len() {
                    siet_chyby = zapni_siet();
                }
                // obrazovka sa môže objaviť až po štarte (ovládač grafiky Odroidu) alebo zmeniť rozlíšenie
                if !len_sledovanie && novy.obrazovky != supis.obrazovky {
                    // zhasnutý obraz: televízor v pohotovosti môže hlásiť odpojenie – znova sa otvorí až po prebudení
                    if displej.as_ref().is_some_and(|d| d.je_kms() && d.stav_obrazu() != StavObrazu::Zapnuty)
                    {
                        obrazovka_zmenena = true;
                    } else {
                        // starú kartu treba zatvoriť skôr – vlastníkom obrazu (DRM master) je len prvý otvárajúci
                        drop(displej.take());
                        displej = otvor_displej(tv, ciel_obrazu(&nastavenie), ostry_obraz(&nastavenie));
                        obraz_teraz =
                            displej.as_ref().filter(|d| d.je_kms()).map(|_| ciel_obrazu(&nastavenie));
                        prekreslit = true;
                    }
                }
                // nové vstupné zariadenie sa sleduje hneď; pripojenie zariadenia je činnosť človeka
                if novy.vstupy != supis.vstupy {
                    vstupy = Vstupy::otvor();
                }
                if novy.usb != supis.usb || novy.vstupy != supis.vstupy {
                    posledna_aktivita = Instant::now();
                    tv_spi = false;
                }
                hlas(&novy.riadok());
                supis = novy;
                podpatie_bolo |= supis.podpatie == Some(true);
                // nový USB kľúč tiež spí medzi zápismi (R159); nové zariadenia stromu USB tiež (R191)
                if tv && periferie_teraz == Some(true) {
                    usb_nastavene =
                        chlad::nastav_spanok_usb(Path::new("/"), !usb_spanok_zakazany, USB_SPANOK_MS);
                    if novinka(&nastavenie, "usb_strom") {
                        usb_strom = chlad::nastav_spanok_usb_stromu(Path::new("/"), true);
                    }
                }
                eee.obnov(&supis);
                // karta sa môže objaviť až chvíľu po štarte – hľadá sa na pozadí (R215)
                if nastavenie.is_none() && hladanie_nastavenia.is_none() {
                    let mena: Vec<String> = supis.ulozisko.iter().map(|u| u.meno.clone()).collect();
                    hladanie_nastavenia =
                        Some(uloziska::na_pozadi("nastavenie", move || nacitaj_nastavenie_z(&mena)));
                }
                // nosiče záznamov sa po zmene periférií zistia znova (na pozadí)
                nosice = None;
                if merana_karta.is_empty()
                    && let Some(u) = vyber_kartu(&supis, &nastavenie)
                {
                    merana_karta = u.meno.clone();
                    spusti_meranie(u, meranie.clone());
                }
            }
            kmsg.citaj();
            zmena
        } else {
            false
        };
        // R189: karta SD, ktorú jadro opakovane nevie spustiť – nižší takt, potom odpojený radič
        if pid1 && !skuska {
            for host in std::mem::take(&mut kmsg.sd_zlyhania) {
                let strop = radice::sd_strop_je(Path::new("/"));
                let Some(krok) = straz_sd.zlyhanie(&host, strop) else { continue };
                let Some((rd, _)) = radice::radic_sd(Path::new("/"), &host) else { continue };
                let text = match krok {
                    radice::KrokSd::Pomalsie(_) => {
                        let ok = radice::nastav_sd_strop(Path::new("/"), radice::SD_POMALY_KHZ).is_ok()
                            && radice::odpoj(&rd).is_ok()
                            && radice::pripoj(&rd).is_ok();
                        format!(
                            "karta SD ({host}): {}× zlyhala – skúša sa takt {} MHz (R189){}",
                            radice::SD_ZLYHANI,
                            radice::SD_POMALY_KHZ / 1000,
                            if ok { "" } else { " – nepodarilo sa" }
                        )
                    }
                    radice::KrokSd::Odpoj(_) => match radice::odpoj(&rd) {
                        Ok(()) => {
                            straz_sd.odpojene.push(rd);
                            format!(
                                "karta SD ({host}): jadro ju nevie spustiť ani pomalšie – radič odpojený, už sa \
                                 neskúša (R189); záznamy idú na USB kľúč"
                            )
                        }
                        Err(e) => format!("karta SD ({host}): radič sa nepodarilo odpojiť ({e})"),
                    },
                };
                hlas(&text);
                straz_sd.hlasenie = Some(text);
            }
        }
        // R114: rozbočovač USB Odroidu C4 sa pri štarte občas nezobudí (klávesnica a myš chýbajú, kým sa
        // nevytiahnu a nezasunú) – KLÍČ ho raz „zasunie znova“ sám: odpojí a znova pripojí koreňové rozbočovače
        if pid1 && !skuska && !usb_ozivene && uptime().unwrap_or(0.0) >= 8.0 {
            usb_ozivene = true;
            let cerstvy = Supis::nacitaj(Path::new("/"));
            if cerstvy.usb.is_empty() && cerstvy.vstupy.is_empty() {
                let n = ozivi_usb();
                if n > 0 {
                    hlas(&format!("USB: žiadne zariadenie – rozbočovače ({n}) znova pripojené"));
                    posledny_supis = Instant::now() - Duration::from_secs(60);
                }
            }
        }

        // R165: dekodér videa – zoznam formátov vždy; s nastavením video=ano aj skúška vlastnými tokmi s presne
        // známym obrazom (H.264 a HEVC). Raz, 20 s po štarte (po načítaní nastavenia, najneskôr po minúte).
        let hore = uptime().unwrap_or(0.0);
        // R198: štart s nižším napätím bežal 5 min – overený
        if let Some((st @ pokoj::Stav::Skuska(_), true, n)) = pokoj_stav.as_ref()
            && hore >= pokoj::OVERENIE_S
        {
            let (st, n) = (*st, n.clone());
            let nosic = n.clone();
            // R215: zápis na kartu na pozadí
            pozadie.push(uloziska::na_pozadi("pokoj", move || {
                match zaznam::subor_v_koreni(&nosic, pokoj::SUBOR, &pokoj::obsah(pokoj::Stav::Overene)) {
                    Ok(_) => hlas(&format!(
                        "podpätie v nečinnosti (chladný pokoj, R198): {} min s nižším napätím bez chyby – overený",
                        pokoj::OVERENIE_S as u32 / 60
                    )),
                    Err(e) => hlas(&format!(
                        "podpätie v nečinnosti (chladný pokoj, R198): overenie sa nepodarilo zapísať ({e}) – {}",
                        st.popis()
                    )),
                }
            }));
            pokoj_stav = Some((pokoj::Stav::Overene, true, n));
        }
        if pid1
            && !skuska
            && !video_spustene
            && novinka(&nastavenie, "dekoder")
            && hore >= 20.0
            && (nastavenie.is_some() || hore >= 60.0)
        {
            video_spustene = true;
            let skusat = nastavenie.as_ref().and_then(|(_, n)| n.video) == Some(true)
                || nastavenie.as_ref().and_then(|(_, n)| n.dlhy_test) == Some(true)
                // R199: pri prvom (automatickom) pokuse na zariadení aj skúška videa
                || (tv && !pokus_hotovy_tu && nastavenie.as_ref().is_some_and(|(_, n)| n.pokus.is_none()));
            let spustaci = nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
            spusti_skusku_videa(skusat, spustaci, video_riadky.clone());
        }

        // R158–R161: správca chladu – obraz, periférie a fázy tepelného pokusu
        // bezpečnosť: pokus nesmie krabičku prehriať – pôvodný stav (fázy 1 a 6) v teplej izbe hreje nad
        // strop a strážca procesorom neochladí, čo hreje obraz a periférie (meranie 23. 11.: 71,5 °C)
        if pokus
            && !pokus_zastaveny
            && faza_teraz.is_some()
            && teplota.is_some_and(|t| t >= profil.strop + 5.0)
        {
            pokus_zastaveny = true;
            hlas(&format!(
                "tepelný pokus: zastavený pri {:.1} °C (strop {:.0} °C + 5) – ďalej chladný režim",
                teplota.unwrap_or(0.0),
                profil.strop
            ));
        }
        // R184: dlhý test hardvéru = tepelný pokus s tromi fázami navyše (záťaž jadier, pamäte, chladnutie)
        let dlhy = nastavenie.as_ref().and_then(|(_, n)| n.dlhy_test) == Some(true);
        let pocet_faz = chlad::pocet_faz(dlhy);
        let pokus_s =
            if pokus && !pokus_zastaveny { merania.as_ref().and_then(|m| m.uplynulo_s()) } else { None };
        if tv {
            let n = nastavenie.as_ref().map(|(_, n)| n);
            let zh = Zhasinanie::pre_tv(n.and_then(|n| n.zhasni_min), n.and_then(|n| n.vypni_min));
            let necinnost = posledna_aktivita.elapsed().as_secs();
            let mut r = chlad::rozhodni(pokus_s, pocet_faz, fazy, necinnost, ciel_obrazu(&nastavenie), &zh);
            siet_spi_faza = r.siet_spi;
            // R184: záťaž podľa fázy dlhého testu – vlákna sa spustia a zastavia pri zmene fázy
            let chce_zataz = if pid1 && !skuska { r.zataz } else { chlad::Zataz::Ziadna };
            if zataznik.as_ref().map_or(chlad::Zataz::Ziadna, |z| z.druh) != chce_zataz {
                if let Some(z) = zataznik.take() {
                    z.zastav();
                    hlas(&format!(
                        "dlhý test: záťaž skončila – {}",
                        popis_zataze(&stat_zataze, max_teplota_zataze)
                    ));
                }
                // R184: pri záťaži jadier sa jadrá neparkujú – strážca ich pustí naplno, kým to teplota dovolí
                // (noc 23./24. 11.: s parkovaním počítalo len 1 jadro pri 1 GHz)
                regulator
                    .nastav_parkovanie(parkovat_mozno && chce_zataz != chlad::Zataz::Jadra && r.periferie);
                if chce_zataz != chlad::Zataz::Ziadna {
                    zataznik = Some(Zataznik::spusti(chce_zataz, &stat_zataze));
                    hlas(&format!(
                        "dlhý test: záťaž – {}",
                        if chce_zataz == chlad::Zataz::Jadra { "všetky jadrá" } else { "pamäť RAM" }
                    ));
                }
                okolie.prekalibruj();
            }
            if zataznik.is_some()
                && let Some(t) = teplota
            {
                max_teplota_zataze = Some(max_teplota_zataze.map_or(t, |m: f64| m.max(t)));
            }
            // R181: televízor zaspal (CEC) – krabička vypne obraz hneď, nečaká na nečinnosť (ak ho vypínať smie)
            if tv_spi && r.faza.is_none() && zh.vypnuty_po_s.is_some() {
                r.stav = StavObrazu::Vypnuty;
            }
            // R181: CEC – otvorí sa podľa nastavenia; keď jadro adaptér ešte nemá, skúsi sa znova o 30 s
            // R183: hlavný vypínač noviniek vypne aj CEC (novinka od karty 4edb1db)
            let cec_rezim = if n.is_some_and(|n| n.novinky == Some(false)) {
                cec::Rezim::Nic
            } else {
                cec::Rezim::z_nastavenia(n.and_then(|n| n.cec.as_deref()))
            };
            if cec_rezim == cec::Rezim::Nic {
                if cec.take().is_some() || cec_stav != "vypnuté (cec=nie)" {
                    cec_stav = "vypnuté (cec=nie)".into();
                }
            } else if let Some(c) = cec.as_mut() {
                c.rezim = cec_rezim;
            } else if pid1 && !skuska && hore >= cec_dalsi_pokus_s && (nastavenie.is_some() || hore >= 60.0) {
                match cec::Cec::otvor(cec_rezim) {
                    Ok(c) => {
                        hlas("CEC: adaptér /dev/cec0 otvorený – krabička ovláda televízor cez HDMI (R181)");
                        cec_stav = "otvorené".into();
                        cec = Some(c);
                        cec_obraz = None;
                    }
                    Err(e) => {
                        cec_stav = format!("nedostupné ({e})");
                        cec_dalsi_pokus_s = hore + 30.0;
                    }
                }
            }
            // R164: pamäť pod 85 °C – nad ňou DDR obnovuje 2× častejšie (viac tepla → ešte viac obnovy)
            if r.faza.is_none()
                && let Some(d) = teplota_pamate()
            {
                let chran = d >= 75.0 || (ochrana_pamate && d >= 72.0);
                if chran != ochrana_pamate {
                    ochrana_pamate = chran;
                    hlas(&format!(
                        "pamäť DDR {d:.1} °C: {}",
                        if chran {
                            "ochrana – úsporný displej (chladný obraz), v nečinnosti čierny"
                        } else {
                            "ochrana skončila"
                        }
                    ));
                }
                if chran {
                    r.obraz = Ciel::Chladny;
                    // v pokoji (minúta bez stlačenia) aj čierny obraz – pamäť sa nečíta vôbec
                    if r.stav == StavObrazu::Zapnuty && necinnost >= 60 {
                        r.stav = StavObrazu::Cierny;
                    }
                }
            }
            match r.faza {
                Some(f) => {
                    rusene[f] |= r.rusi;
                    if faza_teraz != Some(f) {
                        faza_teraz = Some(f);
                        pamat_fazy.push((f, pamat::SumaToku::default()));
                        hlas(&format!("tepelný pokus: fáza {}/{} – {}", f + 1, pocet_faz, fazy[f].nazov));
                    }
                }
                None => {
                    if faza_teraz.take().is_some() {
                        hlas("tepelný pokus: hotový – ďalej bežný chladný režim");
                        // R199: zapísať, že toto zariadenie má pokus s touto verziou karty hotový
                        let spustaci =
                            nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
                        if !pokus_hotovy_tu {
                            // R215: zápis značky na pozadí – zaseknutý nosič nezastaví hlavnú slučku
                            let riadok = format!("{pokus_kluc} {model}");
                            pozadie.push(uloziska::na_pozadi("pokus", move || {
                                if let Some(n) = zaznam::najdi_nosice(spustaci.as_deref()).into_iter().next()
                                    && let Err(e) = zaznam::subor_merani(&n, POKUSY, Some(&riadok))
                                {
                                    hlas(&format!("tepelný pokus (R199): značku sa nepodarilo zapísať ({e})"))
                                }
                            }));
                            pokus_hotovy_tu = true;
                        }
                        pokus_hotovy_text = Some(
                            "tepelný pokus hotový – výsledok je na karte, zariadenie môžeš vypnúť".into(),
                        );
                    }
                }
            }
            // R166: takt VPU – zvýšiť pred zapnutím/zmenou obrazu, znížiť až po nej (R182: mimo pokusu len
            // pri vypnutom obraze)
            let ciel_vpu = r.vpu_mhz.unwrap_or_else(|| chlad::takt_vpu(r.stav));
            if let Some(t) = takt_vpu.as_mut()
                && t.teraz.is_none_or(|x| ciel_vpu > x)
                && t.nastav(ciel_vpu)
            {
                okolie.prekalibruj();
            }
            // obraz sa po prebudení znova otvorí, ak sa medzitým zmenil televízor
            if r.stav == StavObrazu::Zapnuty && obrazovka_zmenena {
                obrazovka_zmenena = false;
                drop(displej.take());
                displej = otvor_displej(tv, r.obraz, ostry_obraz(&nastavenie));
                obraz_teraz = displej.as_ref().filter(|d| d.je_kms()).map(|_| r.obraz);
                prekreslit = true;
            }
            if let Some(d) = displej.as_mut().filter(|d| d.je_kms()) {
                if obraz_teraz != Some(r.obraz) {
                    if d.prestav(r.obraz, ostry_obraz(&nastavenie)) {
                        prekreslit = true;
                        okolie.prekalibruj();
                    }
                    obraz_teraz = Some(r.obraz);
                }
                let pred = d.stav_obrazu();
                d.nastav_stav_obrazu(r.stav);
                if d.stav_obrazu() != pred {
                    // nečinný príkon sa zmenil – okolie sa drží, vlastný ohrev sa prepočíta
                    okolie.prekalibruj();
                }
            }
            // R181: televízor ide s obrazom krabičky – zapne sa s ním a zaspí s ním
            if let Some(c) = cec.as_mut()
                && cec_obraz != Some(r.stav)
            {
                // R203: preskočené zapnutie sa nezapíše – keď príde človek, televízor sa zapne (predtým si KLÍČ
                // myslel, že televízor už zapol, a po pokuse ho stlačenie nezobudilo)
                let spracovane = match r.stav {
                    // počas pokusu (aj v noci) krabička televízor sama nezapína – len keď ju človek zobudí
                    StavObrazu::Zapnuty if r.faza.is_some() && !r.rusi && necinnost >= 60 => false,
                    // dlhý test (aj po nečakanom reštarte v noci) zapne televízor len pre človeka
                    StavObrazu::Zapnuty if dlhy && !clovek_bol => false,
                    StavObrazu::Zapnuty => {
                        c.obraz_zapnuty();
                        tv_budenie_od = Some(Instant::now());
                        true
                    }
                    // dlhý test na noc: televízor ide spať vždy, keď krabička vypne obraz (vlastník tak chcel)
                    StavObrazu::Vypnuty if dlhy => {
                        c.uspi_tv_hned();
                        true
                    }
                    StavObrazu::Vypnuty => {
                        c.obraz_vypnuty();
                        true
                    }
                    StavObrazu::Cierny => true,
                };
                if spracovane {
                    cec_obraz = Some(r.stav);
                }
            }
            // R203: televízor sa ani 5 s po prebudení neozval (spí bez HPD, krabička nemá adresu CEC) – reštart
            // linky HDMI, rovnako ako 5× tlačidlo (vlastník 24. 11.: až to televízor zobudilo); raz za prebudenie
            if let Some(t) = tv_budenie_od
                && t.elapsed() >= Duration::from_secs(5)
            {
                tv_budenie_od = None;
                if cec.as_ref().is_some_and(|c| c.caka_na_televizor())
                    && displej.as_ref().is_some_and(|d| d.stav_obrazu() == StavObrazu::Zapnuty)
                {
                    hlas("CEC: televízor sa po prebudení neozval – reštart linky HDMI (R203)");
                    rucny_restart = true;
                }
            }
            // R185: strážca linky HDMI – televízor cez SCDC hlási, či signál prijíma; keď sa zasekne, krabička
            // mu vráti nezakódovaný režim alebo reštartuje linku (vypne a zapne obraz)
            if pid1
                && !skuska
                && straz_hdmi
                && let Some(d) = displej.as_mut()
                && d.je_kms()
            {
                let teraz = Instant::now();
                let signal = d.stav_obrazu() != StavObrazu::Vypnuty;
                if d.stav_obrazu() != linka_stav {
                    linka_stav = d.stav_obrazu();
                    linka_od = teraz;
                }
                if ddc.is_none() && signal && teraz >= ddc_dalsi_pokus {
                    match hdmi::Ddc::otvor(Path::new("/")) {
                        Ok(x) => ddc = Some(x),
                        Err(e) => {
                            hdmi_chyba = Some(e.to_string());
                            ddc_dalsi_pokus = teraz + Duration::from_secs(60);
                        }
                    }
                }
                let rucny = std::mem::take(&mut rucny_restart);
                let mut restart = rucny;
                if signal
                    && hdmi_kontrola.elapsed() >= Duration::from_secs(5)
                    && let Some(x) = ddc.as_ref()
                {
                    hdmi_kontrola = teraz;
                    match x.stav() {
                        Ok(st) => {
                            if st.caka_kodovanie(d.takt_khz(), hdmi::krabicka_koduje(Path::new("/"))) {
                                if x.zrus_kodovanie().is_ok() {
                                    hdmi_oprav += 1;
                                    hlas(&format!(
                                        "HDMI: televízor čakal zakódovaný signál, krabička nekóduje ({}) – vrátený",
                                        st.popis()
                                    ));
                                    restart = true;
                                }
                            } else if !st.zamknuty()
                                && !tv_spi
                                && linka_od.elapsed() >= Duration::from_secs(8)
                            {
                                restart = true;
                                if restarty_linky.len() < 3 {
                                    hlas(&format!(
                                        "HDMI: televízor signál nezamkol ({}) – reštart linky",
                                        st.popis()
                                    ));
                                } else if !limit_hlaseny {
                                    hlas(&format!(
                                        "HDMI: televízor signál nezamkol ({}) – už 3 reštarty za 10 min, strážca čaká \
                                         (ručne 5× tlačidlo; inak televízor na 1 min zo zásuvky)",
                                        st.popis()
                                    ));
                                    limit_hlaseny = true;
                                }
                            } else if st.zamknuty() && !hdmi_zamknuty {
                                hlas(&format!("HDMI: televízor signál prijíma ({})", st.popis()));
                            }
                            hdmi_zamknuty = st.zamknuty();
                            hdmi_stav = Some(st);
                            hdmi_chyba = None;
                        }
                        Err(e) => {
                            // monitor cez redukciu alebo televízor HDMI 1.4 – SCDC nemá, skúsi sa až o minútu
                            hdmi_chyba = Some(format!("televízor bez SCDC alebo neodpovedá ({e})"));
                            hdmi_kontrola = teraz + Duration::from_secs(55);
                        }
                    }
                }
                restarty_linky.retain(|t| t.elapsed() < Duration::from_secs(600));
                if restarty_linky.len() < 3 {
                    limit_hlaseny = false;
                }
                if restart && signal && (rucny || restarty_linky.len() < 3) {
                    let stav = d.stav_obrazu();
                    d.nastav_stav_obrazu(StavObrazu::Vypnuty);
                    std::thread::sleep(Duration::from_millis(1500));
                    d.nastav_stav_obrazu(stav);
                    restarty_linky.push(Instant::now());
                    hdmi_restartov += 1;
                    linka_od = Instant::now();
                    hlas(&format!(
                        "HDMI: linka reštartovaná ({})",
                        if rucny { "ručne – 5× tlačidlo" } else { "strážca" }
                    ));
                }
            }
            if let Some(t) = takt_vpu.as_mut()
                && t.teraz.is_some_and(|x| ciel_vpu < x)
                && t.nastav(ciel_vpu)
            {
                okolie.prekalibruj();
            }
            if periferie_teraz != Some(r.periferie) {
                let p = r.periferie;
                usb_nastavene =
                    chlad::nastav_spanok_usb(Path::new("/"), p && !usb_spanok_zakazany, USB_SPANOK_MS);
                // R191: celý strom USB (klávesnica, myš, čip siete, rozbočovače) spí, keď sa nič nedeje
                if novinka(&nastavenie, "usb_strom") {
                    usb_strom = chlad::nastav_spanok_usb_stromu(Path::new("/"), p);
                    usb_strom_od = if p { chlad::cas_spanku_usb_stromu(Path::new("/")) } else { None };
                    usb_zbernica_od = if p { chlad::cas_spanku_zbernice_usb(Path::new("/")) } else { None };
                    usb_koren_od = if p { chlad::cas_spanku_korenov_usb(Path::new("/")) } else { None };
                }
                regulator.nastav_parkovanie(parkovat_mozno && p);
                alr = SietPodlaPotreby::nova();
                alr_vzorka = None;
                for (meno, k) in kable.drain() {
                    if k.riadeny() {
                        let _ = sys::zapni_rozhranie(&meno);
                    }
                }
                let mut siete = Vec::new();
                for rz in supis.siet.iter().filter(|x| !x.bezdrotove) {
                    if let Ok(true) = sys::ethernet_gigabit(&rz.meno, !p) {
                        siete.push(rz.meno.clone());
                    }
                }
                usb_spanok_od =
                    if p && !usb_spanok_zakazany { chlad::cas_spanku_usb(Path::new("/")) } else { None };
                hlas(&format!(
                    "periférie {}: USB úložisko {} ({}) · jadrá v pokoji {} · sieť {}{}",
                    if p { "úsporné" } else { "pôvodné" },
                    if p { "spí medzi zápismi" } else { "bdie" },
                    if usb_nastavene.is_empty() { "žiadne".to_string() } else { usb_nastavene.join(", ") },
                    if p { "zaparkované" } else { "pracujú" },
                    if p { "100 Mb/s, gigabit podľa potreby" } else { "s gigabitom" },
                    if siete.is_empty() {
                        String::new()
                    } else {
                        format!(" (zmenené: {})", siete.join(", "))
                    }
                ));
                periferie_teraz = Some(p);
                okolie.prekalibruj();
            }
            // R178: nepoužité radiče kariet (eMMC Androidu, Wi-Fi bez ovládača) – odpojené pri úsporných
            // perifériách; až minútu po štarte, keď dobehne meranie úložiska
            let chce_odpojit =
                pid1 && !skuska && r.periferie && r.radice && hore >= 60.0 && novinka(&nastavenie, "radice");
            // R215: nosiče záznamov sa zisťujú na pozadí – kým nie sú známe, nič sa neodpája
            if chce_odpojit
                && radice_stav != Some(true)
                && let Some(zname) = nosice.as_ref()
            {
                // R188: chránené sú aj nosiče záznamov (Odroid zo vstavanej eMMC má na nej nastavenie aj záznamy)
                let mut chranene: Vec<String> =
                    nastavenie.as_ref().map(|(c, _)| c.clone()).into_iter().collect();
                chranene.extend(zname.iter().cloned());
                for rd in radice::nepouzite_radice(Path::new("/"), &chranene) {
                    match radice::odpoj(&rd) {
                        Ok(()) => radice_odpojene.push(rd),
                        Err(e) => hlas(&format!("radič {} sa nepodarilo odpojiť: {e}", rd.riadok())),
                    }
                }
                hlas(&format!("nepoužité radiče kariet (R178): {}", popis_radicov(&radice_odpojene, true)));
                radice_stav = Some(true);
                okolie.prekalibruj();
            } else if radice_stav == Some(true) && !(r.periferie && r.radice) {
                for rd in radice_odpojene.drain(..) {
                    if let Err(e) = radice::pripoj(&rd) {
                        hlas(&format!("radič {} sa nepodarilo znova pripojiť: {e}", rd.riadok()));
                    }
                }
                hlas("nepoužité radiče kariet (R178): znova pripojené");
                radice_stav = Some(false);
                okolie.prekalibruj();
            }
            // R167: hlboký spánok jadier – vlastný ovládač jadra; len pri bežiacom strážnom časovači, s poistkou
            // na karte a nie počas tepelného pokusu (fázy sa porovnávajú s predošlými pokusmi)
            let chce_spanok = pid1
                && !skuska
                && r.jadra_spia
                && strazny.is_some()
                && hore >= 120.0
                && novinka(&nastavenie, "spanok_jadier");
            let spustaci = nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
            if spanok_jadier.krok(chce_spanok, spustaci.as_deref(), novinka(&nastavenie, "casovac_budenia")) {
                okolie.prekalibruj();
            }
        }
        let chlad_stav = tv.then(|| {
            let mut casti = Vec::new();
            if let Some(d) = displej.as_ref() {
                casti.push(d.popis());
            }
            // tok pamäte na obrazovke najviac raz za minútu (prekreslenie by meranie samo ovplyvnilo)
            if let Some(t) = tok_pamate
                && tok_zobrazeny.is_none_or(|(k, _)| k.elapsed() >= Duration::from_secs(60))
            {
                tok_zobrazeny = Some((Instant::now(), t));
            }
            if let Some((_, t)) = tok_zobrazeny {
                casti.push(t.popis());
            }
            if periferie_teraz == Some(true) {
                if let (Some(od), Some(teraz)) = (usb_spanok_od, chlad::cas_spanku_usb(Path::new("/"))) {
                    let (spi, bdie) = (teraz.0.saturating_sub(od.0), teraz.1.saturating_sub(od.1));
                    if spi + bdie > 10_000 {
                        casti.push(format!(
                            "USB kľúč spí {:.0} %",
                            (spi as f64 / (spi + bdie) as f64 * 10.0).round() * 10.0
                        ));
                    }
                }
                if kable.values().any(|k| k.riadeny()) {
                    // R191: čip siete spí (bez kábla alebo sieť zatiaľ nič nepotrebuje)
                    casti.push(if kabel_zapojeny.values().any(|z| *z) {
                        "sieť spí – kábel zapojený, zatiaľ ju nič nepotrebuje".to_string()
                    } else {
                        "čip siete spí – bez kábla".to_string()
                    });
                } else {
                    casti.push(format!("sieť {} podľa potreby", rychlost_siete(alr.mbs)));
                }
                if usb_strom > 0
                    && let (Some(od), Some(t)) = (usb_strom_od, chlad::cas_spanku_usb_stromu(Path::new("/")))
                {
                    let (spi, bdie) = (t.0.saturating_sub(od.0), t.1.saturating_sub(od.1));
                    if spi + bdie > 10_000 {
                        let zbernica = match (usb_zbernica_od, chlad::cas_spanku_zbernice_usb(Path::new("/")))
                        {
                            (Some(od), Some(t)) => {
                                let (s, b) = (t.0.saturating_sub(od.0), t.1.saturating_sub(od.1));
                                format!(" (zbernica {:.0} %)", s as f64 * 100.0 / (s + b).max(1) as f64)
                            }
                            _ => String::new(),
                        };
                        casti.push(format!(
                            "USB spí {:.0} %{zbernica}",
                            (spi as f64 / (spi + bdie) as f64 * 10.0).round() * 10.0
                        ));
                    }
                }
                if regulator.zaparkovane() {
                    casti.push("jadrá zaparkované".into());
                }
            }
            if faza_teraz.is_none() {
                let n = nastavenie.as_ref().map(|(_, n)| n);
                let zh = Zhasinanie::pre_tv(n.and_then(|n| n.zhasni_min), n.and_then(|n| n.vypni_min));
                if let Some(c) = zh.cierny_po_s.or(zh.vypnuty_po_s) {
                    casti.push(format!("zhasne po {} min – prebudí ho ovládač", c / 60));
                }
            }
            obrazovka::Chlad {
                text: casti.join(" · "),
                pokus: faza_teraz
                    .map(|f| {
                        let koniec = merania.as_ref().and_then(|m| m.zaciatok_s()).unwrap_or(0)
                            + (f as u64 + 1) * chlad::FAZA_S;
                        format!(
                            "pokus {}/{} {} – do {}",
                            f + 1,
                            pocet_faz,
                            fazy[f].nazov,
                            samomeranie::cas(koniec)
                        )
                    })
                    .or_else(|| pokus_hotovy_text.clone()),
            }
        });
        let stav = (teplota.map_or(-1000, |t| (t * 2.0).round() as i64), regulator.index());
        let mut meranie_karty = meranie.lock().map(|g| g.clone()).unwrap_or(MeranieKarty::Caka);
        if let MeranieKarty::Bezi(mb) = meranie_karty {
            // ak karta 20 s neposunie meranie ďalej, povieme to otvorene
            if posledne_mb != Some(mb) {
                posledne_mb = Some(mb);
                zmena_merania = Instant::now();
            } else if zmena_merania.elapsed() > Duration::from_secs(20) {
                meranie_karty = MeranieKarty::Zaseknute(mb);
            }
        }
        if let MeranieKarty::Zaseknute(mb) = meranie_karty
            && !matches!(zobrazene_meranie, MeranieKarty::Zaseknute(_))
        {
            hlas(&format!("meranie karty stojí pri {mb} MB – karta neodpovedá"));
        }
        let nove_meranie = meranie_karty != zobrazene_meranie;
        let pamat_vysledok = test_pamate.lock().ok().and_then(|g| *g);
        // v skúške sa počká aj na koniec merania karty (najviac 60 s)
        let meranie_bezi =
            matches!(meranie_karty, MeranieKarty::Caka | MeranieKarty::Bezi(_)) && !merana_karta.is_empty();
        // aj na skúšku pamäte (R120)
        let pamat_bezi = pamat_vysledok.is_none();
        // R122: samomeranie začne, keď je systém pripravený – skúška pamäte a meranie karty (ktoré procesor
        // na chvíľu zaťažia) sú hotové, takže meranie teploty neovplyvnia
        let vypnute = nastavenie.as_ref().is_some_and(|(_, n)| n.meranie == Some(false));
        // R160: na TV krabičke s vlastným ovládačom obrazu sa namiesto 30-minútového merania robí tepelný
        // pokus (9 fáz po 15 min); R199: sám raz na každom zariadení, `pokus=ano` vždy, `pokus=nie` nikdy
        if tv
            && !pokus
            && !vypnute
            && merania.as_ref().is_some_and(|m| m.zaciatok_s().is_none())
            && (nastavenie.as_ref().and_then(|(_, n)| n.pokus).unwrap_or(!pokus_hotovy_tu) || dlhy)
            && displej.as_ref().is_some_and(|d| d.je_kms())
        {
            merania = Some(zaznam::Riadenie::s_milnikmi(chlad::milniky_pokusu(pocet_faz)));
            pokus = true;
        }
        let merania_teraz = merania.as_mut().filter(|_| !vypnute);
        if let Some(m) = merania_teraz {
            if let Some(sprava) = m.dokonci() {
                hlas(&format!("samomeranie: {sprava}"));
                // R159: zápis zlyhal pri spiacom USB kľúči – kľúč sa po prebudení možno nespamätal; do konca
                // behu už nespí (overiť a ustúpiť, nie riskovať merania)
                if sprava.contains("nepodarilo") && periferie_teraz == Some(true) && !usb_spanok_zakazany {
                    usb_spanok_zakazany = true;
                    usb_spanok_od = None;
                    usb_nastavene = chlad::nastav_spanok_usb(Path::new("/"), false, USB_SPANOK_MS);
                    hlas("USB úložisko: zápis zlyhal počas úspory – do konca behu bdie");
                }
            }
            let up = uptime().unwrap_or(0.0);
            if ((!pamat_bezi && !meranie_bezi && (skuska || up >= 10.0)) || up >= 120.0) && m.zacni(up as u64)
            {
                hlas(
                    "samomeranie: začiatok – záznamy v 0., 5., 10. a 30. minúte, krivka teploty každých 30 s",
                );
                let u = regulator.uroven();
                m.bod(teplota, u.vykon, u.takt_khz / 1000, vytazenie_teraz, okolie.stav().okolie);
            }
        }
        let stav_merania = merania.as_ref().filter(|_| !vypnute).map(|m| m.stav());
        let stav_okolia = okolie.stav();
        // riadok Okolie sa prekreslí len pri zmene toho, čo ukazuje (celé stupne, prostredie, úspora)
        let kluc_okolia = (
            stav_okolia.okolie.map(|o| o.round() as i64),
            stav_okolia.prostredie.slovo(),
            stav_okolia.studeny_start,
            stav_okolia.samohrev.map(|x| x.round() as i64),
            stav_okolia.tau_s.map(|t| (t / 60.0).round() as i64),
            uspora,
        );
        let ine =
            (eee.stav(), kmsg.posledne(2), pamat_vysledok, siet_chyby.clone(), stav_merania, kluc_okolia);
        let nove_ine = zobrazene_ine.as_ref() != Some(&ine);
        let bezi_s = uptime().unwrap_or(0.0) as u64;
        let novy_cas = Some(bezi_s) != zobrazeny_cas;
        let n = nastavenie.as_ref().map(|(_, n)| n);
        let jazyk = Jazyk::z_kodu(n.and_then(|n| n.jazyk.as_deref()));
        let nove_chlad = zobrazeny_chlad != chlad_stav;
        let vsetko = zobrazene != Some(stav)
            || zmena_periferii
            || nove_meranie
            || nove_ine
            || nove_chlad
            || prekreslit;
        let postav_stav = || Stav {
            model: &model,
            meno: n.and_then(|n| n.meno.as_deref()),
            teplota,
            ciel: profil.ciel,
            strop: profil.strop,
            vykon: regulator.uroven().vykon,
            podpatie: supis.podpatie,
            podpatie_bolo,
            supis: &supis,
            pripravene_s: pripravene,
            verzia: crate::VERZIA,
            bezi_s,
            meranie_karty: &meranie_karty,
            merana_karta: &merana_karta,
            poda: &poda,
            zram_mb,
            strazny_casovac: strazny.is_some(),
            eee: ine.0,
            hlasenia: &ine.1,
            test_pamate: ine.2,
            siet_chyby: &ine.3,
            samomeranie: ine.4.as_ref(),
            okolie: (!hw.snimace.is_empty()).then_some(&stav_okolia),
            chlad: chlad_stav.as_ref(),
            uspora,
        };
        // R122: záznam samomerania – hlavná slučka len odčíta, čo už má; snímku stlačí a zapíše vlákno s
        // najnižšou prioritou
        if let Some(m) = merania.as_mut().filter(|_| !vypnute)
            && let Some(uloha) = m.na_rade()
        {
            let p = match uloha {
                zaznam::Uloha::Zaznam(minuta) => {
                    // bod krivky v tejto minúte, ak ho krok strážcu ešte nezapísal (prebudenie udalosťou)
                    let u = regulator.uroven();
                    m.bod(teplota, u.vykon, u.takt_khz / 1000, vytazenie_teraz, stav_okolia.okolie);
                    let st = postav_stav();
                    // obraz: presne to, čo je na obrazovke; bez obrazovky (Tanix) sa nakreslí 1280×720
                    let (platno, opis) = match displej.as_ref().and_then(|d| d.predosle.clone()) {
                        Some(p) => (Some(p), obrazovka::nakresli_s_opisom(&pismo, 320, 240, &st, jazyk).1),
                        None => {
                            let (p, o) = obrazovka::nakresli_s_opisom(&pismo, 1280, 720, &st, jazyk);
                            (Some(p), o)
                        }
                    };
                    let posledna = minuta == *m.milniky().last().unwrap_or(&30);
                    let mut text = format!("{}{NL}", "═".repeat(66));
                    text += &format!(
                        "ORCHIDEA OS – samomeranie č. {ZNACKA_PORADIA} · záznam {minuta}. minúty{NL}"
                    );
                    text += &format!(
                        "zariadenie: {model} · meno: {} · verzia ORCHIDEA OS {}{NL}",
                        st.meno.unwrap_or("–"),
                        crate::VERZIA
                    );
                    text += &format!("zapísané na: {ZNACKA_NOSICA}{NL}");
                    text += &format!(
                        "čas: {} od zapnutia · {minuta}. minúta merania (začiatok pri „beží {}“){NL}",
                        samomeranie::cas(bezi_s),
                        samomeranie::cas(m.zaciatok_s().unwrap_or(0))
                    );
                    text += &nadpis("OBRAZOVKA (všetky riadky)");
                    for r in &opis {
                        text += &format!("  {r}{NL}");
                    }
                    text += &nadpis("TEPELNÝ REGULÁTOR (STRÁŽCA)");
                    text += &format!(
                        "  teplota {} · cieľ {:.0} °C, strop {:.0} °C · úroveň {}/{} · jadrá {} · strop taktu {} MHz · výkon {:.0} % · vyťaženie {}{NL}",
                        teplota.map_or("–".into(), |t| format!("{t:.1} °C")),
                        profil.ciel,
                        profil.strop,
                        regulator.index() + 1,
                        regulator.pocet_urovni(),
                        u.jadra,
                        u.takt_khz / 1000,
                        u.vykon * 100.0,
                        vytazenie_teraz.map_or("–".into(), |v| format!("{:.0} %", v * 100.0)),
                    );
                    text += &format!("  {}{NL}", rozpoznanie(&model, &poda, profil));
                    if let Ok(v) = video_riadky.lock()
                        && !v.is_empty()
                    {
                        text += &nadpis("VIDEO – HARDVÉROVÝ DEKODÉR (R165)");
                        for r in v.iter() {
                            text += &format!("  {r}{NL}");
                        }
                    }
                    if tv {
                        text += &nadpis("SPRÁVCA CHLADU (R158–R164)");
                        // R162: priemerný tok pamäte od posledného záznamu
                        if merac_pamate_je {
                            match pamat_pri_zazname.priemer() {
                                Some(t) => {
                                    text += &format!(
                                        "  {} – priemer {} okien po 2 s od posledného záznamu{NL}",
                                        t.popis(),
                                        pamat_pri_zazname.okien
                                    )
                                }
                                None => {
                                    text += &format!(
                                        "  pamäť: nemerala sa – plný obraz (merač by rušil HDMI, R176){NL}"
                                    )
                                }
                            }
                            pamat_pri_zazname = pamat::SumaToku::default();
                        }
                        if novinka(&nastavenie, "merac_taktov")
                            && let Some(t) = takty_merane()
                        {
                            text += &format!(
                                "  takty (merané v čipe): {t} · takt VPU riadi KLÍČ: {}{NL}",
                                takt_vpu
                                    .as_ref()
                                    .and_then(|v| v.teraz)
                                    .map_or("nie".into(), |v| format!("{v} MHz"))
                            );
                        }
                        if let Some(d) = teplota_pamate() {
                            text += &format!(
                                "  pamäť DDR {d:.1} °C (dvojnásobná obnova nad 85 °C) · ochrana {}{NL}",
                                if ochrana_pamate { "zapnutá" } else { "netreba" }
                            );
                        }
                        // R163: čo budí čip
                        if let Some(teraz) = citaj("/proc/interrupts").map(|t| pamat::prerusenia(&t)) {
                            if let Some((t0, p)) = budenia_pri_zazname.as_ref() {
                                let v = pamat::budenia(p, &teraz, t0.elapsed().as_secs_f64(), 8);
                                let spolu: f64 = v.iter().map(|x| x.1).sum();
                                let zoznam: Vec<String> =
                                    v.iter().map(|(m, r)| format!("{m} {r:.1}")).collect();
                                text += &format!("  budenia: {spolu:.0}/s – {}{NL}", zoznam.join(" · "));
                            }
                            budenia_pri_zazname = Some((Instant::now(), teraz));
                        }
                        let d = displej.as_ref();
                        text += &format!(
                            "  {} · stav: {} · vlastný ovládač KMS: {}{NL}",
                            d.map_or("bez obrazovky".into(), |d| d.popis()),
                            slovo_stavu(d.map_or(StavObrazu::Zapnuty, |d| d.stav_obrazu())),
                            if d.is_some_and(|d| d.je_kms()) { "áno" } else { "nie – /dev/fb0" }
                        );
                        if usb_strom > 0 {
                            text += &format!(
                                "  strom USB (R191): {} zariadení smie spať · {}{NL}",
                                usb_strom,
                                match (usb_strom_od, chlad::cas_spanku_usb_stromu(Path::new("/"))) {
                                    (Some(od), Some(t)) => {
                                        let (spi, bdie) =
                                            (t.0.saturating_sub(od.0), t.1.saturating_sub(od.1));
                                        format!(
                                            "spali {:.0} % času",
                                            spi as f64 * 100.0 / (spi + bdie).max(1) as f64
                                        )
                                    }
                                    // R201: v strome nie je nič okrem koreňových rozbočovačov a úložísk
                                    (Some(_), None) | (None, None)
                                        if usb_strom_od.is_none() && periferie_teraz == Some(true) =>
                                    {
                                        "okrem koreňových rozbočovačov a úložísk nič – meria sa zbernica"
                                            .into()
                                    }
                                    _ => "zatiaľ bez merania".into(),
                                }
                            );
                            // R192: zbernica – port na koreni uspaný = bez mikrorámcov (RPi 1–3: bez 8000 FIQ/s)
                            let bdiace = chlad::usb_bdiace(Path::new("/"));
                            let podiel = |od: Option<(u64, u64)>, t: Option<(u64, u64)>| match (od, t) {
                                (Some(od), Some(t)) => {
                                    let (s, b) = (t.0.saturating_sub(od.0), t.1.saturating_sub(od.1));
                                    (s + b > 0).then(|| format!("{:.0} %", s as f64 * 100.0 / (s + b) as f64))
                                }
                                _ => None,
                            };
                            text += &format!(
                                "  zbernica USB (R192, R201): port na koreni uspaný {} · koreňový rozbočovač spal {} · teraz bdejú: {}{NL}",
                                podiel(usb_zbernica_od, chlad::cas_spanku_zbernice_usb(Path::new("/")))
                                    .map_or("– (nič na koreni)".into(), |p| format!(
                                        "{p} času (vtedy port bez mikrorámcov)"
                                    )),
                                podiel(usb_koren_od, chlad::cas_spanku_korenov_usb(Path::new("/")))
                                    .map_or("– (nemeria sa)".into(), |p| format!(
                                        "{p} (vtedy stojí celá zbernica)"
                                    )),
                                if bdiace.is_empty() { "nič".to_string() } else { bdiace.join(", ") }
                            );
                        }
                        if let Some((st, bezi, _)) = pokoj_stav.as_ref() {
                            text += &format!(
                                "  podpätie v nečinnosti (chladný pokoj, R198): tento štart {} · nastavenie {} (napätie jadra v riadku VideoCore)                                 {NL}",
                                if *bezi {
                                    "s nižším napätím v pokoji (−50 mV)"
                                } else {
                                    "s pôvodným napätím"
                                },
                                st.popis()
                            );
                        }
                        let spanok = match (usb_spanok_od, chlad::cas_spanku_usb(Path::new("/"))) {
                            (Some(od), Some(t)) if periferie_teraz == Some(true) => {
                                let (spi, bdie) = (t.0.saturating_sub(od.0), t.1.saturating_sub(od.1));
                                format!(
                                    "spí {:.0} % času ({} s spánku, {} s činnosti)",
                                    spi as f64 * 100.0 / (spi + bdie).max(1) as f64,
                                    spi / 1000,
                                    bdie / 1000
                                )
                            }
                            _ => "bdie (pôvodne)".into(),
                        };
                        text += &format!(
                            "  USB úložisko ({}): {spanok}{NL}",
                            if usb_nastavene.is_empty() {
                                "žiadne".into()
                            } else {
                                usb_nastavene.join(", ")
                            }
                        );
                        text += &format!(
                            "  sieť: {} · prepnutí {alr_prepnuti} · bez kábla: {} · jadrá: {}{NL}",
                            if periferie_teraz == Some(true) {
                                format!("podľa potreby – teraz {}", rychlost_siete(alr.mbs))
                            } else {
                                "s gigabitom (pôvodne)".into()
                            },
                            if kable.values().any(|k| k.riadeny()) {
                                format!(
                                    "čip spí{}, hľadaní kábla {kabel_hladani}",
                                    if kabel_zapojeny.values().any(|z| *z) {
                                        " – kábel zapojený, sieť zatiaľ nič nepotrebuje (R191)"
                                    } else {
                                        ""
                                    }
                                )
                            } else {
                                format!("kábel zapojený, hľadaní {kabel_hladani}")
                            },
                            if regulator.zaparkovane() { "zaparkované (1 jadro)" } else { "pracujú" }
                        );
                        text += &format!(
                            "  radiče kariet (R178): {} · spánok jadier (R167): {}{NL}",
                            match radice_stav {
                                Some(true) => popis_radicov(&radice_odpojene, true),
                                Some(false) => "pripojené (pokus)".into(),
                                None => "zatiaľ pripojené".into(),
                            },
                            spanok_jadier.popis()
                        );
                        if let Some(h) = straz_sd.hlasenie.as_ref() {
                            text += &format!("  {h}{NL}");
                        }
                        if radice::casovac_budenia_je(Path::new("/")) {
                            text += &format!(
                                "  časovač budenia (R186): {}{NL}",
                                radice::stav_casovaca_budenia(Path::new("/"))
                            );
                        }
                        text += &format!("  prepínače (R183): {}{NL}", vypnute_novinky(&nastavenie));
                        text += &format!(
                            "  linka HDMI (R185): {} · krabička kóduje Full HD: {} (R187) · opravy kódovania {} · reštarty linky {}{NL}",
                            match (&hdmi_stav, &hdmi_chyba) {
                                (_, Some(e)) => e.clone(),
                                (Some(st), None) => st.popis(),
                                (None, None) => "zatiaľ nečítaná".into(),
                            },
                            match hdmi::krabicka_koduje(Path::new("/")) {
                                Some(true) => "áno",
                                Some(false) => "nie",
                                None => "?",
                            },
                            hdmi_oprav,
                            hdmi_restartov
                        );
                        if dlhy {
                            text += &format!(
                                "  dlhý test (R184): {}{NL}",
                                popis_zataze(&stat_zataze, max_teplota_zataze)
                            );
                        }
                        text += &format!(
                            "  CEC (R181): {}{NL}",
                            cec.as_ref().map_or_else(|| cec_stav.clone(), |c| c.popis())
                        );
                        if pokus {
                            let r: Vec<String> =
                                (0..pocet_faz).filter(|&i| rusene[i]).map(|i| (i + 1).to_string()).collect();
                            text += &format!(
                                "  tepelný pokus: {} · rušené fázy (ovládač): {}{NL}",
                                match faza_teraz {
                                    Some(f) if minuta > 0 => format!(
                                        "koniec fázy {}, začína {}/{} – {}",
                                        f,
                                        f + 1,
                                        pocet_faz,
                                        fazy[f].nazov
                                    ),
                                    Some(f) => format!("fáza {}/{} – {}", f + 1, pocet_faz, fazy[f].nazov),
                                    None if minuta == 0 => "začína".into(),
                                    None => "hotový".into(),
                                },
                                if r.is_empty() { "žiadne".into() } else { r.join(", ") }
                            );
                        }
                    }
                    text += &format!("  {}{NL}", popis_okolia(&stav_okolia, uspora));
                    text += &format!(
                        "  {}{NL}  Inventár periférií (SÚPIS) – {}{NL}",
                        poda.riadok(),
                        supis.riadok()
                    );
                    let mut dovetok = nadpis(if minuta == 0 {
                        "VAROVANIA A CHYBY JADRA OD ŠTARTU"
                    } else {
                        "NOVÉ VAROVANIA A CHYBY JADRA"
                    });
                    kmsg.citaj();
                    let varovania = kmsg.nenahlasene();
                    if varovania.is_empty() {
                        dovetok += &format!("  žiadne{NL}");
                    }
                    for v in varovania {
                        dovetok += &format!("  {v}{NL}");
                    }
                    // R183: čo sa dialo s obrazom a HDMI a čo robil KLÍČ – na hľadanie chyby
                    dovetok += &nadpis(if minuta == 0 {
                        "OBRAZ, HDMI A KLÍČ OD ŠTARTU (R183)"
                    } else {
                        "OBRAZ, HDMI A KLÍČ – NOVÉ (R183)"
                    });
                    for v in kmsg.obraz_nenahlasene() {
                        dovetok += &format!("  {v}{NL}");
                    }
                    if posledna {
                        dovetok += &nadpis("KRIVKA TEPLOTY (každých 30 s)");
                        dovetok += &samomeranie::tabulka(m.krivka());
                        dovetok += &nadpis("SÚHRN MERANIA");
                        dovetok += &samomeranie::suhrn(m.krivka(), m.milniky());
                        if dlhy {
                            dovetok += &nadpis("VYHODNOTENIE HARDVÉRU (R184)");
                            let (v, cj) = (
                                stat_zataze.vypoctov.load(Ordering::Relaxed),
                                stat_zataze.chyb_jadier.load(Ordering::Relaxed),
                            );
                            let (mb, cp) = (
                                stat_zataze.mb_pamate.load(Ordering::Relaxed),
                                stat_zataze.chyb_pamate.load(Ordering::Relaxed),
                            );
                            dovetok += &format!(
                                "  procesor: {}{NL}",
                                match (v, cj) {
                                    (0, _) => "záťaž jadier neprebehla".to_string(),
                                    (v, 0) => format!("✓ {v} výpočtov pri plnej záťaži, všetky správne"),
                                    (v, c) =>
                                        format!("✗ {c} z {v} výpočtov chybných – jadro v teple počíta zle"),
                                }
                            );
                            dovetok += &format!(
                                "  pamäť RAM: {}{NL}",
                                match (mb, cp) {
                                    (0, 0) => "záťaž pamäte neprebehla".to_string(),
                                    (mb, 0) => format!(
                                        "✓ {:.1} GB zapísaných a overených bez chyby",
                                        mb as f64 / 1024.0
                                    ),
                                    (mb, c) => format!(
                                        "✗ {c} chýb pri {:.1} GB – pamäť v teple chybuje",
                                        mb as f64 / 1024.0
                                    ),
                                }
                            );
                            dovetok += &format!(
                                "  teplota pri plnej záťaži: najviac {} (strop regulátora {:.0} °C){NL}",
                                max_teplota_zataze.map_or("–".into(), |t| format!("{t:.1} °C")),
                                profil.strop
                            );
                            dovetok += &format!(
                                "  dekodér videa: časť VIDEO · obraz, HDMI a CEC: riadky CEC a časť OBRAZ, HDMI A KLÍČ · spánok jadier: riadok R167{NL}"
                            );
                        }
                        if pokus {
                            dovetok += &nadpis("TEPELNÝ POKUS (R160)");
                            let krivka: Vec<(u64, f64)> =
                                m.krivka().iter().filter_map(|b| Some((b.s, b.teplota?))).collect();
                            dovetok += &chlad::vysledok_pokusu(&krivka, &rusene, pocet_faz, je_rpi, NL);
                            // R162: tok pamäte v každej fáze
                            if merac_pamate_je {
                                dovetok += &format!("  tok pamäte po fázach (MB/s, okná 2 s):{NL}");
                                for (f, suma) in &pamat_fazy {
                                    dovetok += &format!(
                                        "    {} {:<26} {}{NL}",
                                        f + 1,
                                        fazy[*f].nazov,
                                        suma.priemer()
                                            .map_or("nemerané – plný obraz (R176)".to_string(), |t| t
                                                .popis())
                                    );
                                }
                            }
                        }
                    }
                    zaznam::Priprava {
                        text,
                        dovetok,
                        platno,
                        identifikovat: minuta == 0,
                        spustaci_disk: nastavenie
                            .as_ref()
                            .map(|(c, _)| c.trim_start_matches("/dev/").to_string()),
                        predpona: predpona_merania.clone(),
                    }
                }
                zaznam::Uloha::Opakovanie => zaznam::Priprava {
                    text: String::new(),
                    dovetok: String::new(),
                    platno: None,
                    identifikovat: false,
                    spustaci_disk: nastavenie
                        .as_ref()
                        .map(|(c, _)| c.trim_start_matches("/dev/").to_string()),
                    predpona: predpona_merania.clone(),
                },
            };
            m.spusti(uloha, p);
        }
        if vsetko || novy_cas {
            match displej.as_mut() {
                // R109: zmenili sa len hodiny – prekreslí sa len ich obdĺžnik (zlomok práce)
                Some(d) if !vsetko && d.predosle.is_some() => {
                    if let Some(platno) = d.predosle.as_mut() {
                        let (x, y, w, v) = obrazovka::nakresli_hodiny(&pismo, platno, bezi_s, jazyk);
                        d.ukaz_obdlznik(x, y, w, v);
                    }
                }
                Some(d) => {
                    let stav = postav_stav();
                    let platno = obrazovka::nakresli(&pismo, d.sirka, d.vyska, &stav, jazyk);
                    let riadkov = d.ukaz(platno);
                    if skuska {
                        hlas(&format!(
                            "obrazovka nakreslená: {}×{}, zapísaných riadkov {riadkov}",
                            d.sirka, d.vyska
                        ));
                    }
                }
                None if vsetko => ukaz_stav(&model, teplota, &regulator, cas_startu, &supis),
                None => {}
            }
            if vsetko && (!pid1 || skuska) {
                let u = regulator.uroven();
                hlas(&format!(
                    "teplota {}, úroveň {}/{} (jadrá {}, takt {} MHz)",
                    teplota.map_or("–".into(), |t| format!("{t:.1} °C")),
                    regulator.index() + 1,
                    regulator.pocet_urovni(),
                    u.jadra,
                    u.takt_khz / 1000
                ));
            }
            zobrazene = Some(stav);
            zobrazeny_cas = Some(bezi_s);
            zobrazene_meranie = meranie_karty.clone();
            zobrazene_ine = Some(ine);
            zobrazeny_chlad = chlad_stav.clone();
            prekreslit = false;
        }
        krokov += 1;
        // R184: po dlhom teste (všetky záznamy zapísané, 10 min bez človeka) uspí televízor a vypne krabičku
        if dlhy
            && pid1
            && !skuska
            && !vypnutie_po_teste
            && posledna_aktivita.elapsed() >= Duration::from_secs(600)
            && merania.as_ref().is_some_and(|m| {
                matches!(
                    m.stav(),
                    obrazovka::Samomeranie::Hotove { .. } | obrazovka::Samomeranie::Nezapisane(_)
                )
            })
        {
            vypnutie_po_teste = true;
            if let Some(z) = zataznik.take() {
                z.zastav();
            }
            hlas("dlhý test hardvéru hotový – televízor ide spať a krabička sa vypína");
            let spustaci = nastavenie.as_ref().map(|(c, _)| c.trim_start_matches("/dev/").to_string());
            // R215: zápis značky na pozadí s limitom (kratším ako strážny časovač)
            let _ = uloziska::na_pozadi("vypnutie-po-teste", move || {
                if let Some(n) = zaznam::najdi_nosice(spustaci.as_deref()).into_iter().next() {
                    let _ = zaznam::poistka(
                        &n,
                        VYPNUTE_PO_TESTE,
                        zaznam::Poistka::Vytvor(
                            "Krabička sa po dlhom teste hardvéru (R184) vypla. KLÍČ to pri ďalšom štarte zapíše do záznamu a súbor zmaže.\r\n",
                        ),
                    );
                }
            })
            .pockaj(Duration::from_secs(10));
            if let Some(c) = cec.as_mut() {
                c.uspi_tv_hned();
            }
            std::thread::sleep(Duration::from_secs(3));
            let e = sys::vypni(false);
            hlas(&format!("vypnutie zlyhalo: {e} – krabička ostáva zapnutá s vypnutým obrazom"));
        }
        // R122: v skúške sa počká aj na prvý záznam samomerania (zápis na kartu emulátora)
        let zaznam_bezi = merania.as_ref().is_some_and(|m| !vypnute && !m.prvy_vybaveny());
        if skuska
            && krokov >= 3
            && (!(meranie_bezi || pamat_bezi || zaznam_bezi) || start.elapsed() > Duration::from_secs(150))
        {
            hlas(&format!(
                "ORCHIDEA OS pripravený (štart správcu systému {:.2} s, verzia {})",
                cas_startu.as_secs_f64(),
                crate::VERZIA
            ));
            if pid1 {
                // čas na snímku obrazovky v skúške (emulátor)
                if displej.is_some() {
                    std::thread::sleep(Duration::from_secs(6));
                }
                let e = sys::vypni(false);
                hlas(&format!("vypnutie zlyhalo: {e}"));
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
            return;
        }

        // spánok do ďalšieho kroku strážcu alebo do udalosti jadra; hodiny na obrazovke sa posunú pri
        // každom prebudení (v pokoji po 4 s – zbytočné prebúdzanie by doskou len hrialo)
        let cakat = dalsi_krok.saturating_duration_since(Instant::now());
        udalost = match &udalosti {
            Some(u) => {
                let ine: Vec<i32> = cec.as_ref().map(|c| vec![c.fd()]).unwrap_or_default();
                let (bola, vstup, _, stlacene) =
                    u.cakaj_so_vstupmi(cakat.as_millis() as u64, &vstupy.fds(), &ine);
                // R158: stlačenie na ovládači, klávesnici alebo pohyb myši prebudí obraz
                if vstup {
                    posledna_aktivita = Instant::now();
                    tv_spi = false;
                    clovek_bol = true;
                    // R185: 5× tlačidlo do 4 s = ručný reštart linky HDMI (keď televízor ukazuje „Nie je signál“);
                    // počítajú sa len stlačenia (nie pohyb myši ani držanie tlačidla)
                    let t = Instant::now();
                    stlacenia.extend(std::iter::repeat_n(t, stlacene as usize));
                    while stlacenia.front().is_some_and(|x| t.duration_since(*x) > Duration::from_secs(4)) {
                        stlacenia.pop_front();
                    }
                    if stlacenia.len() >= 5 {
                        stlacenia.clear();
                        rucny_restart = true;
                    }
                }
                // R181: správy z televízora – ovládač TV a výber vstupu krabičky ju zobudia, vypnutý TV ju uspí
                if let Some(c) = cec.as_mut() {
                    for ud in c.prijmi(cec_obraz == Some(StavObrazu::Zapnuty)) {
                        match ud {
                            cec::Udalost::Tlacidlo(_) | cec::Udalost::ChceMa => {
                                posledna_aktivita = Instant::now();
                                tv_spi = false;
                                clovek_bol = true;
                            }
                            cec::Udalost::TvSpi => {
                                hlas("CEC: televízor zaspal – krabička vypína obraz");
                                tv_spi = true;
                            }
                            cec::Udalost::InyZdroj => {}
                        }
                    }
                }
                if bola {
                    // pripojenie zariadenia je dávka udalostí – počká sa, kým utíchne (najviac 1 s)
                    let t = Instant::now();
                    while t.elapsed() < Duration::from_secs(1) && u.cakaj(200) {}
                }
                bola
            }
            None => {
                std::thread::sleep(cakat);
                false
            }
        };
    }
}

/// Čo ukazujú kontrolky klávesnice v slepom režime (R119).
struct StavKontroliek {
    /// Doska nemá obrazovku – kontrolky hovoria.
    slepy: bool,
    /// Porucha (chybné jadro alebo pamäť, R100/R120) – všetky tri kontrolky rýchlo blikajú.
    porucha: bool,
    teplota: Option<f64>,
    ciel: f64,
    strop: f64,
}

/// Kontrolky (LED) Num/Caps/Scroll Lock.
const LED_NUM: u16 = 0;
const LED_CAPS: u16 = 1;
const LED_SCROLL: u16 = 2;

/// Klávesnice, ktoré majú kontrolky (`/dev/input/eventN` s schopnosťou LED).
fn klavesnice_s_kontrolkami() -> Vec<fs::File> {
    let mut v = Vec::new();
    for z in fs::read_dir("/sys/class/input").into_iter().flatten().flatten() {
        let meno = z.file_name().to_string_lossy().into_owned();
        if !meno.starts_with("event") {
            continue;
        }
        let led = citaj(z.path().join("device/capabilities/led")).unwrap_or_default();
        if led.trim().is_empty() || led.trim() == "0" {
            continue;
        }
        if let Ok(f) = fs::OpenOptions::new().write(true).open(format!("/dev/input/{meno}")) {
            v.push(f);
        }
    }
    v
}

/// Udalosť `struct input_event` (čas, typ, kód, hodnota) v pamäťovom tvare jadra danej architektúry.
fn udalost_vstupu(typ: u16, kod: u16, hodnota: i32) -> Vec<u8> {
    let mut b = Vec::with_capacity(24);
    b.extend_from_slice(&(0 as core::ffi::c_long).to_ne_bytes());
    b.extend_from_slice(&(0 as core::ffi::c_long).to_ne_bytes());
    b.extend_from_slice(&typ.to_ne_bytes());
    b.extend_from_slice(&kod.to_ne_bytes());
    b.extend_from_slice(&hodnota.to_ne_bytes());
    b
}

fn nastav_kontrolky(klavesnice: &mut [fs::File], num: bool, caps: bool, scroll: bool) {
    let mut b = Vec::new();
    for (kod, zapnuta) in [(LED_NUM, num), (LED_CAPS, caps), (LED_SCROLL, scroll)] {
        b.extend(udalost_vstupu(0x11, kod, zapnuta as i32)); // EV_LED
    }
    b.extend(udalost_vstupu(0, 0, 0)); // EV_SYN
    for k in klavesnice.iter_mut() {
        let _ = k.write_all(&b);
    }
}

/// Teplota v bliknutiach: (desiatky, jednotky) zaokrúhlené na celé stupne; 0 jednotiek = jedno dlhé bliknutie.
fn teplota_v_bliknutiach(t: f64) -> (u32, u32) {
    let c = t.round().clamp(0.0, 99.0) as u32;
    (c / 10, c % 10)
}

/// R119: vlákno kontroliek – Num Lock krátko blikne každé 2 s (žijem), Caps Lock svieti nad cieľom, Scroll
/// Lock pri strope; raz za minútu sa ohlási teplota (Scroll Lock desiatky, Caps Lock jednotky).
fn spusti_kontrolky(stav: Arc<Mutex<StavKontroliek>>) {
    std::thread::spawn(move || {
        let pauza = |ms| std::thread::sleep(Duration::from_millis(ms));
        let mut posledna_teplota = Instant::now();
        let mut zhasnute = true;
        loop {
            let (slepy, porucha, teplota, ciel, strop) = match stav.lock() {
                Ok(s) => (s.slepy, s.porucha, s.teplota, s.ciel, s.strop),
                Err(_) => return,
            };
            let mut k = klavesnice_s_kontrolkami();
            if !slepy || k.is_empty() {
                if !zhasnute && !k.is_empty() {
                    nastav_kontrolky(&mut k, false, false, false);
                }
                zhasnute = true;
                pauza(5000);
                continue;
            }
            zhasnute = false;
            // porucha: všetky tri kontrolky rýchlo blikajú (5 s), potom ďalej bežné hlásenia
            if porucha {
                for _ in 0..10 {
                    nastav_kontrolky(&mut k, true, true, true);
                    pauza(250);
                    nastav_kontrolky(&mut k, false, false, false);
                    pauza(250);
                }
            }
            let teple = teplota.is_some_and(|t| t >= ciel);
            let horuce = teplota.is_some_and(|t| t >= strop - 3.0);
            if let Some(t) = teplota
                && posledna_teplota.elapsed() >= Duration::from_secs(60)
            {
                posledna_teplota = Instant::now();
                let (desiatky, jednotky) = teplota_v_bliknutiach(t);
                nastav_kontrolky(&mut k, false, false, false);
                pauza(1500);
                for _ in 0..desiatky {
                    nastav_kontrolky(&mut k, false, false, true);
                    pauza(350);
                    nastav_kontrolky(&mut k, false, false, false);
                    pauza(350);
                }
                pauza(1000);
                if jednotky == 0 {
                    nastav_kontrolky(&mut k, false, true, false);
                    pauza(1500);
                    nastav_kontrolky(&mut k, false, false, false);
                } else {
                    for _ in 0..jednotky {
                        nastav_kontrolky(&mut k, false, true, false);
                        pauza(350);
                        nastav_kontrolky(&mut k, false, false, false);
                        pauza(350);
                    }
                }
                pauza(1500);
            }
            // žijem: Num Lock krátko blikne
            nastav_kontrolky(&mut k, true, teple, horuce);
            pauza(200);
            nastav_kontrolky(&mut k, false, teple, horuce);
            pauza(1800);
        }
    });
}

/// Vyťaženie procesora z `/proc/stat` (podiel času, keď zapnuté jadrá pracovali) – pre strážcu (R110).
struct Vytazenie {
    posledne: Option<(u64, u64)>,
}

impl Vytazenie {
    fn nove() -> Vytazenie {
        Vytazenie { posledne: Self::cas() }
    }

    /// (spolu, nečinne) v tikoch jadra za všetky jadrá.
    fn cas() -> Option<(u64, u64)> {
        let text = fs::read_to_string("/proc/stat").ok()?;
        rozloz_stat(text.lines().next()?)
    }

    fn krok(&mut self) -> Option<f64> {
        let teraz = Self::cas()?;
        let predtym = self.posledne.replace(teraz)?;
        let spolu = teraz.0.saturating_sub(predtym.0);
        let necinne = teraz.1.saturating_sub(predtym.1);
        (spolu > 0).then(|| 1.0 - necinne as f64 / spolu as f64)
    }
}

/// Riadok `cpu  user nice system idle iowait irq softirq steal …` → (spolu, nečinné vrátane čakania na disk).
fn rozloz_stat(riadok: &str) -> Option<(u64, u64)> {
    let mut casti = riadok.split_whitespace();
    if casti.next()? != "cpu" {
        return None;
    }
    let cisla: Vec<u64> = casti.take(8).filter_map(|x| x.parse().ok()).collect();
    if cisla.len() < 5 {
        return None;
    }
    Some((cisla.iter().sum(), cisla[3] + cisla[4]))
}

/// R111: úsporný režim Ethernetu (EEE) zostáva zapnutý (Ethernet v pokoji menej hreje), kým spojenie
/// nezačne vypadávať – vtedy ho KLÍČ vypne (známy problém RPi 3 B+ a 4 s niektorými prepínačmi).
struct SledovanieEee {
    rozhranie: Option<String>,
    posledny_pocet: Option<u64>,
    vypadky: Vec<Instant>,
    vypnute: bool,
    aktivne: bool,
}

/// Koľko zmien spojenia (odpojenie + pripojenie = 2) za 10 minút znamená nestabilné spojenie.
const EEE_ZMIEN: usize = 4;

impl SledovanieEee {
    fn nove(supis: &Supis) -> SledovanieEee {
        let mut s = SledovanieEee {
            rozhranie: None,
            posledny_pocet: None,
            vypadky: Vec::new(),
            vypnute: false,
            aktivne: false,
        };
        s.obnov(supis);
        s
    }

    /// Po zmene periférií: prvé káblové rozhranie a jeho stav EEE.
    fn obnov(&mut self, supis: &Supis) {
        let meno = supis.siet.iter().find(|r| !r.bezdrotove).map(|r| r.meno.clone());
        if meno != self.rozhranie {
            self.rozhranie = meno;
            self.posledny_pocet = None;
            self.vypadky.clear();
        }
        let Some(meno) = &self.rozhranie else { return };
        let aktivne = matches!(sys::eee_stav(meno), Ok(Some((true, true))));
        if aktivne != self.aktivne {
            self.aktivne = aktivne;
            hlas(&format!(
                "sieť: {meno} – úsporný režim EEE {}",
                if aktivne { "aktívny" } else { "neaktívny" }
            ));
        }
    }

    /// `uspavane`: sieť uspáva sám správca systému (R191) – zmeny spojenia sú vtedy zámerné, nie výpadky (R215).
    fn krok(&mut self, uspavane: bool) {
        let Some(meno) = self.rozhranie.clone() else { return };
        if self.vypnute {
            return;
        }
        if uspavane {
            self.posledny_pocet = None;
            self.vypadky.clear();
            return;
        }
        let Some(pocet) =
            citaj(format!("/sys/class/net/{meno}/carrier_changes")).and_then(|c| c.parse::<u64>().ok())
        else {
            return;
        };
        let predtym = self.posledny_pocet.replace(pocet);
        // prvé spojenie po štarte sa nepočíta
        if let Some(p) = predtym
            && pocet > p
            && uptime().unwrap_or(0.0) > 30.0
        {
            let teraz = Instant::now();
            self.vypadky.extend(std::iter::repeat_n(teraz, (pocet - p).min(16) as usize));
        }
        self.vypadky.retain(|t| t.elapsed() < Duration::from_secs(600));
        if self.vypadky.len() >= EEE_ZMIEN {
            match sys::vypni_eee(&meno) {
                Ok(_) => hlas(&format!(
                    "sieť: {meno} – spojenie vypadávalo ({} zmien za 10 min), úsporný režim EEE vypnutý",
                    self.vypadky.len()
                )),
                Err(e) => hlas(&format!("sieť: {meno} – EEE sa nepodarilo vypnúť: {e}")),
            }
            self.vypnute = true;
            self.aktivne = false;
        }
    }

    fn stav(&self) -> Option<Eee> {
        if self.vypnute {
            Some(Eee::VypnutyPreVypadky)
        } else if self.aktivne {
            Some(Eee::Aktivny)
        } else {
            None
        }
    }
}

/// R114: odpojí a znova pripojí koreňové rozbočovače USB (ako vytiahnutie a zasunutie). Vráti ich počet.
fn ozivi_usb() -> usize {
    let mut n = 0;
    for z in fs::read_dir("/sys/bus/usb/devices").into_iter().flatten().flatten() {
        let meno = z.file_name().to_string_lossy().into_owned();
        if meno.starts_with("usb") && zapis(z.path().join("authorized"), "0") {
            std::thread::sleep(Duration::from_millis(200));
            if zapis(z.path().join("authorized"), "1") {
                n += 1;
            }
        }
    }
    n
}

/// R115: hlásenia jadra o chybách zariadení (karta, USB, obrazovka) z `/dev/kmsg` – na obrazovku, aby sa
/// dali odfotiť (doska nemusí mať sériovú linku ani sieť).
struct Hlasenia {
    subor: Option<fs::File>,
    zoznam: Vec<String>,
    /// Všetky varovania a chyby jadra od štartu (aj neškodné a z iných častí) – pre správu samomerania
    /// (R122). Najviac 120: prvých 60 (štart) a najnovšie.
    vsetky: Vec<String>,
    /// Koľko z `vsetky` už bolo v správe samomerania.
    nahlasene: usize,
    /// R183: hlásenia o obraze, HDMI a CEC (všetky úrovne) a hlásenia KLÍČA – najviac 160 (prvých 80 a najnovšie).
    obraz: Vec<String>,
    /// R189: radiče kariet SD, ktoré ohlásili zlyhanie pri rozpoznaní karty (od posledného prevzatia).
    sd_zlyhania: Vec<String>,
    obraz_nahlasene: usize,
    /// Najvyššia (najmenej závažná) úroveň, ktorá sa ešte ukáže: na RPi len chyby (ovládač USB RPi 1–3
    /// pri každom štarte píše neškodné varovanie), inde aj varovania.
    uroven: u32,
}

impl Hlasenia {
    fn otvor(model: &str) -> Hlasenia {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK – čítanie skončí, keď sú prečítané všetky záznamy
        let subor = fs::OpenOptions::new().read(true).custom_flags(0o4000).open("/dev/kmsg").ok();
        Hlasenia {
            subor,
            zoznam: Vec::new(),
            vsetky: Vec::new(),
            nahlasene: 0,
            obraz: Vec::new(),
            obraz_nahlasene: 0,
            sd_zlyhania: Vec::new(),
            uroven: if model.contains("Raspberry Pi") { 3 } else { 4 },
        }
    }

    fn citaj(&mut self) {
        use std::io::Read;
        let Some(f) = self.subor.as_mut() else { return };
        let mut buf = vec![0u8; 8192];
        for _ in 0..10_000 {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let zaznam = String::from_utf8_lossy(&buf[..n]);
                    if let Some(host) =
                        zaznam.split_once(';').and_then(|(_, sprava)| radice::zlyhanie_sd(sprava.trim_end()))
                    {
                        self.sd_zlyhania.push(host);
                    }
                    if let Some(h) = hlasenie_obrazu(&zaznam) {
                        if self.obraz.len() >= 160 {
                            self.obraz.remove(80);
                            if self.obraz_nahlasene > 80 {
                                self.obraz_nahlasene -= 1;
                            }
                        }
                        self.obraz.push(h);
                    }
                    if let Some(h) = varovanie_jadra(&zaznam) {
                        if self.vsetky.len() >= 120 {
                            self.vsetky.remove(60);
                            if self.nahlasene > 60 {
                                self.nahlasene -= 1;
                            }
                        }
                        self.vsetky.push(h);
                    }
                    if let Some(h) = dolezite_hlasenie(&zaznam, self.uroven)
                        && self.zoznam.last() != Some(&h)
                    {
                        self.zoznam.push(h);
                        if self.zoznam.len() > 8 {
                            self.zoznam.remove(0);
                        }
                    }
                }
                // záznamy medzitým prepísané novšími – čítanie pokračuje
                Err(e) if e.raw_os_error() == Some(32) => continue,
                Err(_) => break,
            }
        }
    }

    fn posledne(&self, n: usize) -> Vec<String> {
        self.zoznam[self.zoznam.len().saturating_sub(n)..].to_vec()
    }

    /// R183: hlásenia o obraze a KLÍČA, ktoré ešte neboli v správe samomerania.
    fn obraz_nenahlasene(&mut self) -> Vec<String> {
        let v = self.obraz[self.obraz_nahlasene.min(self.obraz.len())..].to_vec();
        self.obraz_nahlasene = self.obraz.len();
        v
    }

    /// Varovania a chyby, ktoré ešte neboli v správe samomerania (R122).
    fn nenahlasene(&mut self) -> Vec<String> {
        let v = self.vsetky[self.nahlasene.min(self.vsetky.len())..].to_vec();
        self.nahlasene = self.vsetky.len();
        v
    }
}

/// Okolie pre správu samomerania (R123–R126).
fn popis_okolia(o: &okolie::StavOkolia, uspora: bool) -> String {
    let c = |x: Option<f64>, d: usize| x.map_or("?".to_string(), |v| format!("{v:.d$}"));
    format!(
        "okolie ≈ {} °C ({}) · na začiatku {} °C · štart {} · vlastný ohrev {} °C · τ {} s · ustáli sa pri {} °C · ohrev pri práci {} °C/min · prostredie {} · úsporný režim {}",
        c(o.okolie, 1),
        match o.zdroj {
            Some(okolie::Zdroj::Start) => "zo štartu",
            Some(okolie::Zdroj::Model) => "z modelu",
            Some(okolie::Zdroj::Izba) => "z teplomera – izba= v SKLENIK.TXT",
            None => "neznáme",
        },
        c(o.okolie_start, 1),
        match o.studeny_start {
            Some(true) => "studený",
            Some(false) => "teplý (okolie zo štartu neplatí)",
            None => "?",
        },
        c(o.samohrev, 1),
        c(o.tau_s, 0),
        c(o.ustalena, 1),
        c(o.ohrev_pri_praci.map(|r| r * 60.0), 2),
        o.prostredie.slovo(),
        if uspora { "áno" } else { "nie" }
    )
}

/// Rozpoznal KLÍČ zariadenie? (R122 – riadok správy samomerania)
fn rozpoznanie(model: &str, poda: &ProfilHardveru, profil: Profil) -> String {
    let rodina = if model.contains("Raspberry Pi") {
        Some("Raspberry Pi")
    } else if model.contains("ODROID-C4") {
        Some("Odroid C4")
    } else if model.contains("X96") {
        Some("X96 Max Plus (TV krabička)")
    } else if model.contains("Tanix") || model.contains("TX68") {
        Some("Tanix TX68 (TV krabička)")
    } else {
        None
    };
    let strazca = format!(
        "profil tepelného regulátora (strážcu): cieľ {:.0} °C, strop {:.0} °C",
        profil.ciel, profil.strop
    );
    match (rodina, poda.cip.as_deref()) {
        (Some(r), Some(c)) => format!("rozpoznanie: ✓ {r}, čip {c} – {strazca}"),
        (Some(r), None) => format!("rozpoznanie: ⚠ {r}, ale čip nerozpoznaný – {strazca}"),
        (None, c) => format!(
            "rozpoznanie: ⚠ neznáme zariadenie (čip {}) – všeobecný {strazca}",
            c.unwrap_or("nerozpoznaný")
        ),
    }
}

/// Neškodné hlásenia, ktoré sa na obrazovke neukazujú (v správe samomerania zostanú): napr. bezdrôtový
/// prijímač klávesnice a myši má rozhranie bez prerušení, ktoré jadro preskočí – zariadenia fungujú.
const NESKODNE: [&str; 1] = ["couldn't find an input interrupt endpoint"];

/// Záznam `/dev/kmsg` s úrovňou varovanie alebo chyba – pre správu samomerania (čas od štartu, úroveň, text).
fn varovanie_jadra(zaznam: &str) -> Option<String> {
    let (hlavicka, text) = zaznam.split_once(';')?;
    let mut h = hlavicka.split(',');
    let uroven = h.next()?.parse::<u32>().ok()? & 7;
    let cas_us = h.nth(1).and_then(|c| c.parse::<u64>().ok()).unwrap_or(0);
    let text = text.lines().next()?.trim();
    if uroven > 4 || text.starts_with("KLÍČ") {
        return None;
    }
    let druh = match uroven {
        0..=2 => "KRITICKÉ",
        3 => "chyba",
        _ => "varovanie",
    };
    let text: String = text.chars().take(200).collect();
    Some(format!("[{:>8.3} s] {druh}: {text}", cas_us as f64 / 1e6))
}

/// `/dev/kmsg` zapisuje bajty mimo ASCII ako `\xNN` – späť na UTF-8 (diakritika v hláseniach KLÍČA).
fn bez_escape(t: &str) -> String {
    let b = t.as_bytes();
    let mut v = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1] == b'x'
            && let Ok(x) = u8::from_str_radix(&t[i + 2..i + 4], 16)
        {
            v.push(x);
            i += 4;
        } else {
            v.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&v).into_owned()
}

/// R183: hlásenie jadra o obraze, HDMI, CEC, VPU alebo dekodéri (každá úroveň) a každé hlásenie KLÍČA.
fn hlasenie_obrazu(zaznam: &str) -> Option<String> {
    let (hlavicka, text) = zaznam.split_once(';')?;
    let cas_us = hlavicka.split(',').nth(2).and_then(|c| c.parse::<u64>().ok()).unwrap_or(0);
    let text = bez_escape(text.lines().next()?.trim());
    let text = text.as_str();
    let male = text.to_lowercase();
    let o_obraze = ["drm", "hdmi", "cec", "vpu", "fbcon", "fb0", "vdec", "dw-hdmi", "canvas", "edid"]
        .iter()
        .any(|k| male.contains(k));
    if !(o_obraze || text.starts_with("KLÍČ")) {
        return None;
    }
    let text: String = text.chars().take(200).collect();
    Some(format!("[{:>8.3} s] {text}", cas_us as f64 / 1e6))
}

/// Záznam `/dev/kmsg` (`úroveň,poradie,čas,príznaky;text`) – vráti text, ak je to varovanie alebo chyba
/// zariadenia, ktoré SKLENÍK ukazuje (karta, USB, obrazovka, napájanie zariadení, odložené ovládače).
fn dolezite_hlasenie(zaznam: &str, najvyssia_uroven: u32) -> Option<String> {
    let (hlavicka, text) = zaznam.split_once(';')?;
    let uroven = hlavicka.split(',').next()?.parse::<u32>().ok()? & 7;
    let text = text.lines().next()?.trim();
    if uroven > najvyssia_uroven || text.starts_with("KLÍČ") || NESKODNE.iter().any(|n| text.contains(n)) {
        return None;
    }
    let maly = text.to_lowercase();
    const SLOVA: [&str; 16] = [
        "mmc",
        "sdhci",
        "usb",
        "hub",
        "dwc",
        "xhci",
        "hdmi",
        "drm",
        "edid",
        "regulator",
        "deferred",
        "phy",
        "stmmac",
        "ethernet",
        "mdio",
        "eth0",
    ];
    if !SLOVA.iter().any(|s| maly.contains(s)) {
        return None;
    }
    let mut t: String = text.chars().take(100).collect();
    if text.chars().count() > 100 {
        t.push('…');
    }
    Some(t)
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn hlasenia_klica_s_diakritikou() {
        assert_eq!(bez_escape("KL\\xc3\\x8d\\xc4\\x8c: perif\\xc3\\xa9rie"), "KLÍČ: periférie");
        assert!(hlasenie_obrazu("6,10,3039000,-;KL\\xc3\\x8d\\xc4\\x8c: takt").is_some());
        assert!(hlasenie_obrazu("6,11,3039000,-;usb 1-2: new device").is_none());
    }

    #[test]
    fn kontrolky_klavesnice() {
        // 53 °C = 5× Scroll Lock, 3× Caps Lock; 60 °C = 6× a jedno dlhé
        assert_eq!(teplota_v_bliknutiach(53.4), (5, 3));
        assert_eq!(teplota_v_bliknutiach(59.6), (6, 0));
        assert_eq!(teplota_v_bliknutiach(-3.0), (0, 0));
        assert_eq!(teplota_v_bliknutiach(140.0), (9, 9));
        // struct input_event: 2× long (čas) + typ + kód + hodnota
        let u = udalost_vstupu(0x11, LED_CAPS, 1);
        assert_eq!(u.len(), 2 * size_of::<core::ffi::c_long>() + 8);
        let o = 2 * size_of::<core::ffi::c_long>();
        assert_eq!(
            (u16::from_ne_bytes([u[o], u[o + 1]]), u16::from_ne_bytes([u[o + 2], u[o + 3]])),
            (0x11, 1)
        );
    }

    #[test]
    fn ethernet_bez_gigabitu() {
        // R126: 10/100 a gigabit (bity 0–5), samočinné dohodnutie, TP, MII, riadenie toku
        let ponuka = 0x3F | 1 << 6 | 1 << 7 | 1 << 9 | 1 << 13;
        let vsetko = 0x3F | 1 << 6 | 1 << 7 | 1 << 9 | 1 << 13;
        assert_eq!(sys::ponuka_do(vsetko, ponuka, 100), Some(0x0F | 1 << 6 | 1 << 7 | 1 << 9 | 1 << 13));
        assert_eq!(sys::ponuka_do(vsetko, 0x0F | 1 << 6, 100), None, "už bez gigabitu");
        assert_eq!(sys::ponuka_do(0x0F | 1 << 15, 0x0F | 1 << 15, 100), Some(0x0F), "aj 2,5 Gb/s preč");
        // R170: tri stupne
        assert_eq!(
            sys::ponuka_do(vsetko, 0x0F | 1 << 6, 10),
            Some(0x03 | 1 << 6),
            "10 Mb/s v hlbokom pokoji"
        );
        assert_eq!(sys::ponuka_do(vsetko, 0x03 | 1 << 6, 1000), Some(0x3F | 1 << 6), "späť s gigabitom");
        assert_eq!(sys::ponuka_do(0x30, 0x30, 10), None, "bez 10/100 by spojenie zaniklo");
    }

    #[test]
    fn vytazenie_z_proc_stat() {
        assert_eq!(rozloz_stat("cpu  100 0 50 800 50 0 0 0 0 0"), Some((1000, 850)));
        assert_eq!(rozloz_stat("cpu0 1 2 3 4 5"), None);
        assert_eq!(rozloz_stat("cpu  1 2"), None);
    }

    #[test]
    fn hlasenia_jadra_len_o_zariadeniach() {
        assert_eq!(
            dolezite_hlasenie(
                "3,812,4211000,-;mmc0: error -110 whilst initialising SD card\n SUBSYSTEM=mmc",
                4
            ),
            Some("mmc0: error -110 whilst initialising SD card".into())
        );
        assert_eq!(
            dolezite_hlasenie("4,900,9000000,-;platform ffe05000.mmc: deferred probe pending: regulator", 4),
            Some("platform ffe05000.mmc: deferred probe pending: regulator".into())
        );
        // informácie, cudzie témy a vlastné správy KLÍČA sa neukazujú
        assert_eq!(dolezite_hlasenie("6,1,1,-;usb 1-1: new high-speed USB device", 4), None);
        assert_eq!(dolezite_hlasenie("3,2,2,-;random: crng init done", 4), None);
        assert_eq!(dolezite_hlasenie("4,3,3,-;KLÍČ: usb", 4), None);
        // úroveň s príznakom zariadenia (facility) v horných bitoch
        assert!(dolezite_hlasenie("11,4,4,-;usb 1-1: device descriptor read/64, error -71", 4).is_some());
        // na RPi len chyby: varovanie ovládača USB pri štarte sa neukazuje
        assert_eq!(dolezite_hlasenie("4,6,6,-;WARN::dwc_otg_hcd_init:1074: FIQ DMA bounce buffers", 3), None);
        // neškodné hlásenie bezdrôtového prijímača sa na obrazovke neukáže, v správe áno (R122)
        let prijimac = "3,7,1500000,-;usbhid 1-1.3:1.1: couldn't find an input interrupt endpoint";
        assert_eq!(dolezite_hlasenie(prijimac, 4), None);
        assert_eq!(
            varovanie_jadra(prijimac).as_deref(),
            Some("[   1.500 s] chyba: usbhid 1-1.3:1.1: couldn't find an input interrupt endpoint")
        );
        assert_eq!(varovanie_jadra("6,1,1,-;usb 1-1: new high-speed USB device"), None);
        assert!(varovanie_jadra("4,2,2,-;random: warning").is_some_and(|t| t.contains("varovanie")));
        let dlhy = format!("3,5,5,-;usb {}", "x".repeat(200));
        assert_eq!(dolezite_hlasenie(&dlhy, 4).map(|t| t.chars().count()), Some(101));
    }
}
