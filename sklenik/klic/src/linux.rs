//! KLÍČ na Linuxe: štart systému, tepelný strážca nad súbormi jadra (`/sys`) a stav na obrazovke.

use crate::strazca::{Hardver, Profil, Regulator, Uroven, stupen_ventilatora, urovne};
use crate::sys;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CPU: &str = "/sys/devices/system/cpu";
/// Súbor, z ktorého ORCHIDEA číta tepelný rozpočet (koľko vlákien smie použiť a ako rýchlo).
const ROZPOCET: &str = "/run/sklenik/teplo";

/// Zápis do sériovej linky, na obrazovku (konzola jadra) aj do denníka jadra.
fn hlas(sprava: &str) {
    println!("KLÍČ: {sprava}");
    if let Ok(mut k) = fs::OpenOptions::new().write(true).open("/dev/kmsg") {
        let _ = writeln!(k, "<6>KLÍČ: {sprava}");
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

/// Stavová obrazovka – prekreslí sa len pri zmene (žiadne zbytočné prebúdzanie).
fn ukaz_stav(model: &str, teplota: Option<f64>, r: &Regulator, cas_startu: Duration) {
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

    let mut posledny = Instant::now();
    let mut zobrazene: Option<(i64, usize)> = None;
    let mut krokov = 0u32;
    loop {
        let dt = posledny.elapsed().as_secs_f64().max(0.001);
        posledny = Instant::now();
        let teplota = hw.teplota();
        let uroven = match teplota {
            Some(t) => regulator.krok(t, dt),
            None => regulator.uroven(),
        };
        if !len_sledovanie {
            hw.uplatni(&uroven);
            if let Some(t) = teplota {
                hw.ventiluj(t, profil);
            }
        }
        if pid1 {
            sys::zber_procesov();
        }
        let stav = (teplota.map_or(-1000, |t| (t * 2.0).round() as i64), regulator.index());
        if zobrazene != Some(stav) {
            ukaz_stav(&model, teplota, &regulator, cas_startu);
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
        }
        krokov += 1;
        if skuska && krokov >= 3 {
            hlas(&format!("SKLENÍK pripravený (štart KLÍČA {:.2} s)", cas_startu.as_secs_f64()));
            if pid1 {
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
