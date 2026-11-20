//! KLÍČ na Linuxe: štart systému, periférie a sieť, tepelný strážca nad súbormi jadra (`/sys`) a stav na
//! obrazovke.

use crate::obrazovka::{self, FormatBodu, Jazyk, MeranieKarty, Stav};
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
            // úsporný režim Ethernetu (EEE) robí na RPi 3 B+ (lan78xx) a RPi 4/400 (bcmgenet) výpadky
            // spojenia s niektorými prepínačmi – spoľahlivé spojenie s PC (KORENE) má prednosť
            let ovladac = fs::read_link(z.path().join("device/driver"))
                .ok()
                .and_then(|c| c.file_name().map(|m| m.to_string_lossy().into_owned()));
            if matches!(ovladac.as_deref(), Some("lan78xx" | "bcmgenet")) {
                match sys::vypni_eee(meno) {
                    Ok(true) => {
                        hlas(&format!("sieť: {meno} – úsporný režim EEE vypnutý (stabilné spojenie)"))
                    }
                    Ok(false) => {}
                    Err(e) => hlas(&format!("sieť: {meno} – EEE sa nepodarilo vypnúť: {e}")),
                }
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

    let mut posledny = Instant::now();
    let mut zobrazene: Option<(i64, usize)> = None;
    let mut zobrazeny_cas: Option<u64> = None;
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
    let mut bez_teploty_hlasene = false;
    loop {
        let dt = posledny.elapsed().as_secs_f64().max(0.001);
        posledny = Instant::now();
        let teplota = hw.teplota();
        let uroven = match teplota {
            Some(t) => regulator.krok(t, dt),
            // R99: snímač teploty bol, ale nedá sa prečítať (napr. zlyhal firmvér) – nevieme, že je chladno,
            // preto bezpečný režim: strážca znižuje výkon, akoby bolo nad stropom
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
        }
        // pripojená klávesnica, myš, kábel alebo karta sa objaví bez reštartu
        let novy = Supis::nacitaj(Path::new("/"));
        let zmena_periferii = novy != supis;
        if zmena_periferii {
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
        // hodiny od zapnutia sa menia každú sekundu – prekreslí sa len ich riadok
        let bezi_s = uptime().unwrap_or(0.0) as u64;
        let novy_cas = Some(bezi_s) != zobrazeny_cas;
        if zobrazene != Some(stav) || zmena_periferii || nove_meranie || novy_cas {
            match displej.as_mut() {
                Some(d) => {
                    let n = nastavenie.as_ref().map(|(_, n)| n);
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
                    };
                    let jazyk = Jazyk::z_kodu(n.and_then(|n| n.jazyk.as_deref()));
                    let platno = obrazovka::nakresli(&pismo, d.sirka, d.vyska, &stav, jazyk);
                    let riadkov = d.ukaz(platno);
                    if skuska {
                        hlas(&format!(
                            "obrazovka nakreslená: {}×{}, zapísaných riadkov {riadkov}",
                            d.sirka, d.vyska
                        ));
                    }
                }
                None => ukaz_stav(&model, teplota, &regulator, cas_startu, &supis),
            }
            if !pid1 || skuska {
                hlas(&format!(
                    "teplota {}, úroveň {}/{} (jadrá {}, takt {} MHz)",
                    teplota.map_or("–".into(), |t| format!("{t:.1} °C")),
                    regulator.index() + 1,
                    regulator.pocet_urovni(),
                    uroven.jadra,
                    uroven.takt_khz / 1000
                ));
            }
            zobrazene = Some(stav);
            zobrazeny_cas = Some(bezi_s);
            zobrazene_meranie = meranie_karty.clone();
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
        // v pokoji sa strážca prebúdza zriedkavejšie
        let t = teplota.unwrap_or(0.0);
        let pokoj = t < regulator.profil().ciel - 10.0;
        std::thread::sleep(Duration::from_secs(if pokoj { 4 } else { 1 }));
    }
}
