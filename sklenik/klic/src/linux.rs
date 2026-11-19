//! KLÍČ na Linuxe: štart systému, periférie a sieť, tepelný strážca nad súbormi jadra (`/sys`) a stav na
//! obrazovke.

use crate::obrazovka::{self, FormatBodu, Jazyk, Stav};
use crate::periferie::Supis;
use crate::strazca::{Hardver, Profil, Regulator, Uroven, stupen_ventilatora, urovne};
use crate::sys;
use orchidea_pismo::{Pismo, Platno};
use sklenik_karta::NastavenieZariadenia;
use std::fs;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
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
        if let Some(meno) = z.file_name().to_str() {
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
    hlas(&supis.riadok());
    let pripravene = uptime();

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
        // pripojená klávesnica, myš, kábel alebo karta sa objaví bez reštartu
        let novy = Supis::nacitaj(Path::new("/"));
        let zmena_periferii = novy != supis;
        if zmena_periferii {
            if pid1 && novy.siet.len() != supis.siet.len() {
                zapni_siet();
            }
            hlas(&novy.riadok());
            supis = novy;
            podpatie_bolo |= supis.podpatie == Some(true);
            // karta sa môže objaviť až chvíľu po štarte
            if nastavenie.is_none() {
                nastavenie = nacitaj_nastavenie(&supis);
                nastav_zariadenie(&nastavenie);
            }
        }
        let stav = (teplota.map_or(-1000, |t| (t * 2.0).round() as i64), regulator.index());
        if zobrazene != Some(stav) || zmena_periferii {
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
        }
        krokov += 1;
        if skuska && krokov >= 3 {
            hlas(&format!("SKLENÍK pripravený (štart KLÍČA {:.2} s)", cas_startu.as_secs_f64()));
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
