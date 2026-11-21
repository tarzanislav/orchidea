//! KLÍČ na Linuxe: štart systému, periférie a sieť, tepelný strážca nad súbormi jadra (`/sys`) a stav na
//! obrazovke.

use crate::obrazovka::{self, Eee, FormatBodu, Jazyk, MeranieKarty, Stav};
use crate::periferie::{Supis, Ulozisko};
use crate::poda::Poda;
use crate::strazca::{Hardver, Profil, Regulator, Uroven, stupen_ventilatora, urovne};
use crate::sys;
use orchidea_pismo::{Pismo, Platno};
use sklenik_karta::NastavenieZariadenia;
use std::fs;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CPU: &str = "/sys/devices/system/cpu";
/// Súbor, z ktorého ORCHIDEA číta tepelný rozpočet (koľko vlákien smie použiť a ako rýchlo).
const ROZPOCET: &str = "/run/sklenik/teplo";

/// Zápis do sériovej linky, na obrazovku (konzola jadra) aj do denníka jadra.
fn hlas(sprava: &str) {
    println!("KLÍČ: {sprava}");
    // jeden zápis = jeden záznam denníka jadra
    if let Ok(mut k) = fs::OpenOptions::new().write(true).open("/dev/kmsg") {
        let _ = k.write_all(format!("<6>KLÍČ: {sprava}\n").as_bytes());
    }
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
    uplatnena: Option<(u32, u32)>,
    /// Ventilátory jadra (`cooling_device` typu *fan*): cesta a najvyšší stupeň; riadi ich strážca.
    ventilatory: Vec<(PathBuf, u32, u32)>,
}

impl Sysfs {
    fn najdi() -> Sysfs {
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
            .filter(|p| p.exists())
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
        Sysfs { snimace, takty, jadier, vypinatelne, uplatnena: None, ventilatory }
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
        for (i, p) in self.vypinatelne.iter().enumerate() {
            zapis(p, if (i as u32) + 1 < u.jadra { "1" } else { "0" });
        }
        let _ = fs::create_dir_all("/run/sklenik");
        let _ = fs::write(ROZPOCET, format!("vlakna={}\ntakt_khz={}\n", u.jadra, u.takt_khz));
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
fn zapni_siet() {
    for z in fs::read_dir("/sys/class/net").into_iter().flatten().flatten() {
        // lo a skutočné sieťové karty; virtuálne tunely jadra (sit0 …) sa nezapínajú
        if let Some(meno) = z.file_name().to_str()
            && (meno == "lo" || z.path().join("device").exists())
        {
            match sys::zapni_rozhranie(meno) {
                Ok(true) => hlas(&format!("sieť: rozhranie {meno} zapnuté")),
                Ok(false) => {}
                Err(e) => hlas(&format!("sieť: rozhranie {meno} sa nepodarilo zapnúť: {e}")),
            }
        }
    }
}

/// Grafická obrazovka (framebuffer jadra) – zapisujú sa len zmenené riadky.
struct Displej {
    fb: fs::File,
    sirka: usize,
    vyska: usize,
    riadok_bajtov: u64,
    zaciatok: u64,
    format: FormatBodu,
    predosle: Option<Platno>,
    _tty: Option<fs::File>,
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
        // konzola v grafickom režime: jadro do obrazovky nepíše
        let tty = fs::OpenOptions::new().read(true).write(true).open("/dev/tty1").ok();
        if let Some(t) = &tty
            && let Err(e) = sys::graficka_konzola(t)
        {
            hlas(&format!("obrazovka: grafický režim konzoly sa nepodaril: {e}"));
        }
        hlas(&format!("obrazovka: {sirka}×{vyska}, {} bitov na bod", format.bajtov * 8));
        Some(Displej { fb, sirka, vyska, riadok_bajtov, zaciatok, format, predosle: None, _tty: tty })
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
            if self.fb.write_at(&bajty, self.zaciatok + y as u64 * self.riadok_bajtov).is_ok() {
                zapisane += 1;
            }
        }
        self.predosle = Some(p);
        zapisane
    }

    /// Zapíše na obrazovku len obdĺžnik z posledného plátna (R109 – hodiny).
    fn ukaz_obdlznik(&mut self, x: usize, y: usize, w: usize, v: usize) {
        let Some(p) = self.predosle.as_ref() else { return };
        if x >= self.sirka {
            return;
        }
        let mut bajty = Vec::with_capacity(w * self.format.bajtov);
        for yy in y..(y + v).min(self.vyska) {
            let od = yy * self.sirka + x;
            let riadok = &p.body[od..od + w.min(self.sirka - x)];
            self.format.preved(riadok, &mut bajty);
            let _ = self.fb.write_at(
                &bajty,
                self.zaciatok + yy as u64 * self.riadok_bajtov + (x * self.format.bajtov) as u64,
            );
        }
    }
}

/// Nastavenie zariadenia zo súboru SKLENIK.TXT na karte – číta sa priamo z disku, len na čítanie.
fn nacitaj_nastavenie(supis: &Supis) -> Option<(String, NastavenieZariadenia)> {
    for u in &supis.ulozisko {
        let cesta = format!("/dev/{}", u.meno);
        let Ok(mut f) = fs::File::open(&cesta) else { continue };
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
    nastavenie
        .as_ref()
        .and_then(|(cesta, _)| supis.ulozisko.iter().find(|u| format!("/dev/{}", u.meno) == *cesta))
        .or_else(|| supis.ulozisko.iter().find(|u| u.meno.starts_with("mmcblk")))
        .filter(|u| u.meno.starts_with("mmcblk"))
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
fn zapni_zram(poda: &Poda) -> Option<u64> {
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
fn test_jadier(poda: &mut Poda) {
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
        "\x1b[2J\x1b[H  SKLENIK - KLIC\n\n  {model}\n  start: {:.1} s\n\n  teplota: {t}  (ciel {:.0}, strop {:.0})\n  vykon: uroven {}/{}  jadra {}  takt {} MHz\n",
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
    }
    let prikazovy_riadok = citaj("/proc/cmdline").unwrap_or_default();
    let skuska = prikazovy_riadok.split_whitespace().any(|x| x == "sklenik.skuska=1")
        || std::env::args().any(|a| a == "--skuska");
    let len_sledovanie = !pid1 && !std::env::args().any(|a| a == "--riadit");
    let model = citaj("/proc/device-tree/model").unwrap_or_else(|| "neznáme zariadenie".into());
    let mut poda = crate::poda::nacitaj(Path::new("/"));
    test_jadier(&mut poda);
    if pid1 {
        zapni_siet();
    }
    let mut supis = Supis::nacitaj(Path::new("/"));
    let mut podpatie_bolo = supis.podpatie == Some(true);
    let mut nastavenie = nacitaj_nastavenie(&supis);
    let nastav_zariadenie = |n: &Option<(String, NastavenieZariadenia)>| {
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
    let pismo = Pismo::stonka();
    let mut displej = if len_sledovanie { None } else { Displej::otvor() };
    let profil = Profil::pre_model(&model);
    let mut hw = Sysfs::najdi();
    let mut regulator = Regulator::novy(urovne(&hw.takty(), hw.jadier()), profil);
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
    let mut zobrazene_ine: Option<(Option<Eee>, Vec<String>)> = None;
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
    let mut eee = SledovanieEee::nove(&supis);
    let mut vytazenie = Vytazenie::nove();
    let mut kmsg = Hlasenia::otvor(&model);
    kmsg.citaj();
    let mut usb_ozivene = false;
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
                if let Some(t) = teplota {
                    hw.ventiluj(t, profil);
                }
            }
            if pid1 {
                sys::zber_procesov();
                eee.krok();
            }
            // v pokoji sa strážca prebúdza zriedkavejšie
            let pokoj = teplota.unwrap_or(0.0) < regulator.profil().ciel - 10.0;
            dalsi_krok = posledny_krok + Duration::from_secs(if pokoj { 4 } else { 1 });
        }

        // periférie: pri udalosti jadra, inak raz za 10 s (podpätie, ktoré udalosť nemá); bez udalostí 2 s
        let interval = if udalosti.is_some() { 10 } else { 2 };
        let zmena_periferii = if udalost || posledny_supis.elapsed() >= Duration::from_secs(interval) {
            posledny_supis = Instant::now();
            let novy = Supis::nacitaj(Path::new("/"));
            let zmena = novy != supis;
            if zmena {
                if pid1 && novy.siet.len() != supis.siet.len() {
                    zapni_siet();
                }
                // obrazovka sa môže objaviť až po štarte (ovládač grafiky Odroidu) alebo zmeniť rozlíšenie
                if !len_sledovanie && novy.obrazovky != supis.obrazovky {
                    displej = Displej::otvor();
                }
                hlas(&novy.riadok());
                supis = novy;
                podpatie_bolo |= supis.podpatie == Some(true);
                eee.obnov(&supis);
                // karta sa môže objaviť až chvíľu po štarte
                if nastavenie.is_none() {
                    nastavenie = nacitaj_nastavenie(&supis);
                    nastav_zariadenie(&nastavenie);
                }
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
        let ine = (eee.stav(), kmsg.posledne(2));
        let nove_ine = zobrazene_ine.as_ref() != Some(&ine);
        let bezi_s = uptime().unwrap_or(0.0) as u64;
        let novy_cas = Some(bezi_s) != zobrazeny_cas;
        let n = nastavenie.as_ref().map(|(_, n)| n);
        let jazyk = Jazyk::z_kodu(n.and_then(|n| n.jazyk.as_deref()));
        let vsetko = zobrazene != Some(stav) || zmena_periferii || nove_meranie || nove_ine;
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
                    let stav = Stav {
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
                    };
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
        }
        krokov += 1;
        // v skúške sa počká aj na koniec merania karty (najviac 60 s)
        let meranie_bezi =
            matches!(meranie_karty, MeranieKarty::Caka | MeranieKarty::Bezi(_)) && !merana_karta.is_empty();
        if skuska && krokov >= 3 && (!meranie_bezi || start.elapsed() > Duration::from_secs(60)) {
            hlas(&format!(
                "SKLENÍK pripravený (štart KLÍČA {:.2} s, verzia {})",
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
                let bola = u.cakaj(cakat.as_millis() as u64);
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

    fn krok(&mut self) {
        let Some(meno) = self.rozhranie.clone() else { return };
        if self.vypnute {
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
    /// Najvyššia (najmenej závažná) úroveň, ktorá sa ešte ukáže: na RPi len chyby (ovládač USB RPi 1–3
    /// pri každom štarte píše neškodné varovanie), inde aj varovania.
    uroven: u32,
}

impl Hlasenia {
    fn otvor(model: &str) -> Hlasenia {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK – čítanie skončí, keď sú prečítané všetky záznamy
        let subor = fs::OpenOptions::new().read(true).custom_flags(0o4000).open("/dev/kmsg").ok();
        Hlasenia { subor, zoznam: Vec::new(), uroven: if model.contains("Raspberry Pi") { 3 } else { 4 } }
    }

    fn citaj(&mut self) {
        use std::io::Read;
        let Some(f) = self.subor.as_mut() else { return };
        let mut buf = vec![0u8; 8192];
        for _ in 0..10_000 {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(h) = dolezite_hlasenie(&String::from_utf8_lossy(&buf[..n]), self.uroven)
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
}

/// Záznam `/dev/kmsg` (`úroveň,poradie,čas,príznaky;text`) – vráti text, ak je to varovanie alebo chyba
/// zariadenia, ktoré SKLENÍK ukazuje (karta, USB, obrazovka, napájanie zariadení, odložené ovládače).
fn dolezite_hlasenie(zaznam: &str, najvyssia_uroven: u32) -> Option<String> {
    let (hlavicka, text) = zaznam.split_once(';')?;
    let uroven = hlavicka.split(',').next()?.parse::<u32>().ok()? & 7;
    let text = text.lines().next()?.trim();
    if uroven > najvyssia_uroven || text.starts_with("KLÍČ") {
        return None;
    }
    let maly = text.to_lowercase();
    const SLOVA: [&str; 12] =
        ["mmc", "sdhci", "usb", "hub", "dwc", "xhci", "hdmi", "drm", "edid", "regulator", "deferred", "phy"];
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
        let dlhy = format!("3,5,5,-;usb {}", "x".repeat(200));
        assert_eq!(dolezite_hlasenie(&dlhy, 4).map(|t| t.chars().count()), Some(101));
    }
}
