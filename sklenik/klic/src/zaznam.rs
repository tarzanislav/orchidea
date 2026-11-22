//! Zápis samomerania na kartu a podrobná identifikácia hardvéru pre správu (R122). Len Linux.
//!
//! Karta je pripojená len na pár sekúnd: pripojí sa oddiel FAT, zapíšu sa súbory, každý sa hneď zapíše
//! na kartu (`fsync`), oddiel sa odpojí a jadro zapíše zvyšok. Ak systém kartu, z ktorej štartoval, nevidí
//! (Odroid C4 so SD kartou), zapíše sa na USB kľúč s FAT32.

use crate::samomeranie::{self, PRIECINOK, SPOLU, ZNACKA_NOSICA, ZNACKA_PORADIA};
use crate::sys;
use std::fs;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// Kam sa oddiel na chvíľu pripojí (v pamäti, `/run` je tmpfs).
const PRIPOJNY_BOD: &str = "/run/karta";

fn citaj(cesta: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(cesta)
        .ok()
        .map(|s| s.trim_matches(|c: char| c.is_whitespace() || c == '\0').to_string())
        .filter(|s| !s.is_empty())
}

fn hex(cesta: impl AsRef<Path>) -> Option<u32> {
    let s = citaj(cesta)?;
    u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

/// Meno ovládača zariadenia (cieľ odkazu `driver`).
fn ovladac(zariadenie: &Path) -> Option<String> {
    fs::read_link(zariadenie.join("driver")).ok()?.file_name().map(|n| n.to_string_lossy().to_string())
}

/// Oddiel FAT, na ktorý sa dá zapísať meranie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nosic {
    /// Oddiel, napr. `mmcblk0p1`.
    pub oddiel: String,
    /// Popis pre človeka: „SD karta“, „vstavaná eMMC“, „USB kľúč“.
    pub popis: String,
    /// Krátke označenie do mena súboru: SD, eMMC, USB.
    pub skratka: String,
}

/// Druh disku (popis, skratka).
pub fn druh_disku(disk: &str) -> (&'static str, &'static str) {
    if disk.starts_with("mmcblk") {
        match citaj(format!("/sys/class/block/{disk}/device/type")).as_deref() {
            Some("MMC") => ("vstavaná eMMC", "eMMC"),
            _ => ("SD karta", "SD"),
        }
    } else if disk.starts_with("sd") {
        ("USB kľúč", "USB")
    } else if disk.starts_with("nvme") {
        ("disk NVMe", "NVMe")
    } else {
        ("disk", "disk")
    }
}

fn je_fat(oddiel: &str) -> bool {
    let Ok(f) = fs::File::open(format!("/dev/{oddiel}")) else { return false };
    let mut s = [0u8; 512];
    f.read_exact_at(&mut s, 0).is_ok() && samomeranie::je_fat(&s)
}

/// Prvé oddiely diskov so súborovým systémom FAT: najprv disk, z ktorého SKLENÍK štartoval, potom USB
/// kľúče, nakoniec ostatné.
pub fn najdi_nosice(spustaci_disk: Option<&str>) -> Vec<Nosic> {
    let mut v: Vec<(u8, Nosic)> = Vec::new();
    for z in fs::read_dir("/sys/class/block").into_iter().flatten().flatten() {
        let oddiel = z.file_name().to_string_lossy().to_string();
        if citaj(z.path().join("partition")).as_deref() != Some("1") {
            continue;
        }
        // disk je priečinok nad oddielom v strome zariadení (…/mmcblk0/mmcblk0p1)
        let Some(disk) = fs::canonicalize(z.path())
            .ok()
            .and_then(|c| c.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()))
        else {
            continue;
        };
        if ["zram", "loop", "ram", "mtd"].iter().any(|p| disk.starts_with(p)) || !je_fat(&oddiel) {
            continue;
        }
        let (popis, skratka) = druh_disku(&disk);
        let prednost = if Some(disk.as_str()) == spustaci_disk {
            0
        } else if disk.starts_with("sd") {
            1
        } else {
            2
        };
        v.push((prednost, Nosic { oddiel, popis: popis.into(), skratka: skratka.into() }));
    }
    v.sort_by(|a, b| (a.0, &a.1.oddiel).cmp(&(b.0, &b.1.oddiel)));
    v.into_iter().map(|x| x.1).collect()
}

/// Súbor na zápis: meno je `NN-<predpona><pripona>`; textové súbory sa pripájajú na koniec.
#[derive(Debug, Clone)]
pub struct Subor {
    pub pripona: String,
    pub obsah: Vec<u8>,
    /// Pripojiť aj do `SPOLU.TXT`.
    pub do_spolu: bool,
}

/// Výsledok zápisu.
#[derive(Debug, Clone)]
pub struct Zapisane {
    pub nosic: Nosic,
    pub poradie: u32,
    /// Zapísané súbory (meno, bajtov).
    pub subory: Vec<(String, usize)>,
}

/// Zapíše súbory do priečinka `MERANIA` na prvý nosič, na ktorý sa dá. `predosle` = (oddiel, poradie)
/// z predošlého zápisu – na ten istý nosič sa pokračuje pod rovnakým číslom. `zaver` dostane trvanie
/// zápisu a vráti posledný riadok správy (pripojí sa k prvému textovému súboru).
pub fn zapis(
    nosice: &[Nosic],
    predpona: &str,
    predosle: Option<&(String, u32)>,
    subory: &[Subor],
    zaver: &dyn Fn(Duration) -> String,
) -> Result<Zapisane, String> {
    let mut chyby = Vec::new();
    for n in nosice {
        let poradie = predosle.filter(|p| p.0 == n.oddiel).map(|p| p.1);
        match zapis_na(n, &format!("{predpona}-{}", n.skratka), poradie, subory, zaver) {
            Ok(z) => return Ok(z),
            Err(e) => chyby.push(format!("{} – {e}", n.oddiel)),
        }
    }
    Err(if chyby.is_empty() {
        "karta ani USB kľúč s FAT nie sú v systéme".into()
    } else {
        chyby.join("; ")
    })
}

fn zapis_na(
    n: &Nosic,
    predpona: &str,
    poradie: Option<u32>,
    subory: &[Subor],
    zaver: &dyn Fn(Duration) -> String,
) -> Result<Zapisane, String> {
    let zaciatok = Instant::now();
    let _ = fs::create_dir_all(PRIPOJNY_BOD);
    sys::pripoj_s_volbami(
        &format!("/dev/{}", n.oddiel),
        PRIPOJNY_BOD,
        "vfat",
        sys::MS_NOSUID | sys::MS_NODEV | sys::MS_NOEXEC | sys::MS_NOATIME,
        "flush",
    )
    .map_err(|e| format!("pripojenie: {e}"))?;
    let vysledok = (|| -> std::io::Result<Zapisane> {
        let priecinok = Path::new(PRIPOJNY_BOD).join(PRIECINOK);
        fs::create_dir_all(&priecinok)?;
        let poradie = match poradie {
            Some(p) => p,
            None => {
                let mena: Vec<String> = fs::read_dir(&priecinok)?
                    .flatten()
                    .map(|z| z.file_name().to_string_lossy().to_string())
                    .collect();
                samomeranie::dalsie_poradie(mena.iter().map(String::as_str))
            }
        };
        let mut zapisane = Vec::new();
        for s in subory {
            let meno = format!("{poradie:02}-{predpona}{}", s.pripona);
            let textovy = s.pripona.ends_with(".txt");
            let obsah = if textovy {
                String::from_utf8_lossy(&s.obsah)
                    .replace(ZNACKA_PORADIA, &format!("{poradie:02}"))
                    .replace(ZNACKA_NOSICA, &format!("{} ({}), súbor {meno}", n.popis, n.oddiel))
                    .into_bytes()
            } else {
                s.obsah.clone()
            };
            let mut f = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(textovy)
                .truncate(!textovy)
                .open(priecinok.join(&meno))?;
            f.write_all(&obsah)?;
            f.sync_all()?;
            if s.do_spolu {
                let mut f = fs::OpenOptions::new().create(true).append(true).open(priecinok.join(SPOLU))?;
                f.write_all(&obsah)?;
                f.sync_all()?;
            }
            zapisane.push((meno, obsah.len()));
        }
        // posledný riadok: ako dlho trval zápis (aby bolo vidieť, že meranie teplotu neovplyvnilo)
        if let Some(s) = subory.iter().find(|s| s.pripona.ends_with(".txt")) {
            let riadok = zaver(zaciatok.elapsed());
            let mut ciele = vec![format!("{poradie:02}-{predpona}{}", s.pripona)];
            if s.do_spolu {
                ciele.push(SPOLU.into());
            }
            for c in ciele {
                let mut f = fs::OpenOptions::new().append(true).open(priecinok.join(c))?;
                f.write_all(riadok.as_bytes())?;
                f.sync_all()?;
            }
        }
        Ok(Zapisane { nosic: n.clone(), poradie, subory: zapisane })
    })();
    // odpojenie zapíše aj adresáre a tabuľku FAT; aj keby zlyhalo, sync zapíše všetko
    let _ = sys::odpoj(PRIPOJNY_BOD);
    sys::zapis_vyrovnavacie();
    vysledok.map_err(|e| format!("zápis: {e}"))
}

/// Sériové číslo dosky (RPi: popis hardvéru; Amlogic/Allwinner: SoC; inak /proc/cpuinfo).
pub fn seriove_cislo() -> Option<String> {
    citaj("/proc/device-tree/serial-number")
        .or_else(|| citaj("/sys/bus/soc/devices/soc0/serial_number"))
        .or_else(|| {
            citaj("/proc/cpuinfo")?
                .lines()
                .find(|r| r.starts_with("Serial"))
                .and_then(|r| r.split(':').nth(1))
                .map(|s| s.trim().to_string())
        })
        .filter(|s| s.chars().any(|c| c != '0'))
}

fn riadok(t: &mut String, text: impl AsRef<str>) {
    t.push_str("  ");
    t.push_str(text.as_ref());
    t.push_str(samomeranie::NL);
}

/// Identifikácia hardvéru: doska, čip, nosiče, sieť, USB, monitor, vstupy, snímače teploty.
pub fn identifikacia() -> String {
    let mut t = String::new();
    if let Some(c) = citaj("/proc/device-tree/compatible") {
        riadok(
            &mut t,
            format!("popis hardvéru (compatible): {}", c.split('\0').collect::<Vec<_>>().join(", ")),
        );
    }
    riadok(&mut t, format!("sériové číslo: {}", seriove_cislo().unwrap_or_else(|| "–".into())));
    let soc = Path::new("/sys/bus/soc/devices/soc0");
    if soc.exists() {
        let casti: Vec<String> = ["family", "machine", "soc_id", "revision"]
            .iter()
            .filter_map(|k| citaj(soc.join(k)).map(|v| format!("{k} {v}")))
            .collect();
        riadok(&mut t, format!("čip (SoC): {}", casti.join(" · ")));
    }
    if let Some(v) = citaj("/proc/device-tree/chosen/bootloader/version") {
        riadok(&mut t, format!("zavádzač EEPROM: {v}"));
    }
    // nosiče: celé disky (nie oddiely)
    let mut disky: Vec<String> = fs::read_dir("/sys/class/block")
        .into_iter()
        .flatten()
        .flatten()
        .map(|z| z.file_name().to_string_lossy().to_string())
        .filter(|m| {
            !["zram", "loop", "ram", "mtd"].iter().any(|p| m.starts_with(p))
                && !Path::new(&format!("/sys/class/block/{m}/partition")).exists()
        })
        .collect();
    disky.sort();
    for d in &disky {
        let z = Path::new("/sys/class/block").join(d);
        let bajtov = citaj(z.join("size")).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 512;
        let mut popis = format!("{d}: {} – {:.1} GB", druh_disku(d).0, bajtov as f64 / 1e9);
        let dev = z.join("device");
        if d.starts_with("mmcblk") {
            let typ = citaj(dev.join("type")).unwrap_or_default();
            let manfid = hex(dev.join("manfid"));
            let vyrobca = manfid.and_then(|m| samomeranie::vyrobca_karty(&typ, m)).unwrap_or("?");
            popis += &format!(
                " · výrobca {vyrobca} ({}) · OEM {} · názov {} · vyrobená {} · rev. {} · sér. č. {}",
                manfid.map_or("?".into(), |m| format!("{m:#04x}")),
                citaj(dev.join("oemid")).unwrap_or_else(|| "?".into()),
                citaj(dev.join("name")).unwrap_or_else(|| "?".into()),
                citaj(dev.join("date")).unwrap_or_else(|| "?".into()),
                citaj(dev.join("hwrev")).unwrap_or_else(|| "?".into()),
                citaj(dev.join("serial")).unwrap_or_else(|| "?".into()),
            );
            if let Some(r) =
                citaj(dev.join("speed_class")).or_else(|| citaj(dev.join("ssr"))).filter(|s| s.len() < 20)
            {
                popis += &format!(" · trieda {r}");
            }
        } else {
            let casti: Vec<String> =
                ["vendor", "model", "rev"].iter().filter_map(|k| citaj(dev.join(k))).collect();
            if !casti.is_empty() {
                popis += &format!(" · {}", casti.join(" "));
            }
        }
        riadok(&mut t, popis);
    }
    // sieť
    let mut siete: Vec<_> =
        fs::read_dir("/sys/class/net").into_iter().flatten().flatten().map(|z| z.path()).collect();
    siete.sort();
    for p in siete.iter().filter(|p| p.join("device").exists()) {
        let meno = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        riadok(
            &mut t,
            format!(
                "sieť {meno}: ovládač {} · MAC {} · stav {} · rýchlosť {} Mb/s · duplex {}",
                ovladac(&p.join("device")).unwrap_or_else(|| "?".into()),
                citaj(p.join("address")).unwrap_or_else(|| "?".into()),
                citaj(p.join("operstate")).unwrap_or_else(|| "?".into()),
                citaj(p.join("speed")).unwrap_or_else(|| "–".into()),
                citaj(p.join("duplex")).unwrap_or_else(|| "–".into()),
            ),
        );
    }
    // USB: každé zariadenie s výrobcom, názvom, rýchlosťou a rozhraniami (a ich ovládačmi)
    let usb = Path::new("/sys/bus/usb/devices");
    let mut zariadenia: Vec<String> = fs::read_dir(usb)
        .into_iter()
        .flatten()
        .flatten()
        .map(|z| z.file_name().to_string_lossy().to_string())
        .filter(|m| !m.contains(':') && usb.join(m).join("idVendor").exists())
        .collect();
    zariadenia.sort();
    for m in &zariadenia {
        let z = usb.join(m);
        let vid = citaj(z.join("idVendor")).unwrap_or_default();
        let mut popis = format!(
            "USB {m}: {vid}:{} {} {} · {} Mb/s",
            citaj(z.join("idProduct")).unwrap_or_default(),
            citaj(z.join("manufacturer")).unwrap_or_default(),
            citaj(z.join("product")).unwrap_or_default(),
            citaj(z.join("speed")).unwrap_or_else(|| "?".into()),
        );
        if vid == "1d6b" {
            popis += " · koreňový rozbočovač";
        }
        let mut rozhrania: Vec<String> = fs::read_dir(&z)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|r| r.file_name().to_string_lossy().starts_with(&format!("{m}:")))
            .map(|r| {
                let p = r.path();
                let cislo = |k: &str| hex(p.join(k)).unwrap_or(0) as u8;
                format!(
                    "{} ({})",
                    samomeranie::rozhranie_usb(
                        cislo("bInterfaceClass"),
                        cislo("bInterfaceSubClass"),
                        cislo("bInterfaceProtocol")
                    ),
                    ovladac(&p).unwrap_or_else(|| "bez ovládača".into())
                )
            })
            .collect();
        rozhrania.sort();
        if !rozhrania.is_empty() {
            popis += &format!(" · rozhrania: {}", rozhrania.join(", "));
        }
        riadok(&mut t, popis);
    }
    // monitor (EDID cez grafický ovládač; na RPi s firmvérovou obrazovkou EDID v jadre nie je)
    let mut monitor = false;
    for z in fs::read_dir("/sys/class/drm").into_iter().flatten().flatten() {
        let p = z.path();
        let Ok(edid) = fs::read(p.join("edid")) else { continue };
        if edid.is_empty() {
            continue;
        }
        monitor = true;
        let vystup = z.file_name().to_string_lossy().to_string();
        let rezim =
            fs::read_to_string(p.join("modes")).ok().and_then(|m| m.lines().next().map(str::to_string));
        match samomeranie::rozloz_edid(&edid) {
            Some(e) => riadok(
                &mut t,
                format!(
                    "monitor {vystup}: výrobca {} · kód {:04x} · {} · sér. č. {} · rok {} · najlepšie {} · jadro nastavilo {} · EDID {} B",
                    e.vyrobca,
                    e.kod_vyrobku,
                    e.nazov.as_deref().unwrap_or("bez názvu"),
                    e.seriove.as_deref().unwrap_or("–"),
                    e.rok,
                    e.rozlisenie.map_or("?".into(), |(w, h)| format!("{w}×{h}")),
                    rezim.as_deref().unwrap_or("?"),
                    edid.len()
                ),
            ),
            None => riadok(&mut t, format!("monitor {vystup}: popis EDID je poškodený ({} B)", edid.len())),
        }
    }
    if !monitor {
        let fb = citaj("/sys/class/graphics/fb0/virtual_size").unwrap_or_else(|| "–".into());
        let meno = citaj("/sys/class/graphics/fb0/name").unwrap_or_else(|| "–".into());
        riadok(
            &mut t,
            format!("obrazovka: {meno} {fb} (popis monitora EDID jadro nemá – obrazovku nastavil firmvér)"),
        );
    }
    // čipy siete na zbernici MDIO (adresa, ID čipu, ovládač) – TV krabičky majú často iný čip, ako udáva
    // výrobca; podľa toho sa nastaví popis hardvéru
    if let Ok(z) = std::fs::read_dir("/sys/bus/mdio_bus/devices") {
        let mut cipy: Vec<String> = z
            .flatten()
            .map(|d| {
                let cesta = d.path();
                let id = citaj(cesta.join("phy_id")).unwrap_or_else(|| "?".into());
                let ovladac = std::fs::read_link(cesta.join("driver"))
                    .ok()
                    .and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "bez ovládača".into());
                format!("{} – ID {id}, {ovladac}", d.file_name().to_string_lossy())
            })
            .collect();
        cipy.sort();
        if cipy.is_empty() {
            riadok(&mut t, "čip siete (MDIO): jadro nenašlo žiadny");
        }
        for c in cipy {
            riadok(&mut t, format!("čip siete (MDIO): {c}"));
        }
    }
    // vstupné zariadenia podľa jadra
    if let Some(v) = citaj("/proc/bus/input/devices") {
        let mena: Vec<&str> = v
            .lines()
            .filter_map(|r| r.strip_prefix("N: Name=\"").and_then(|n| n.strip_suffix('"')))
            .collect();
        if !mena.is_empty() {
            riadok(&mut t, format!("vstupy: {}", mena.join(" · ")));
        }
    }
    let zony = snimace_teploty();
    if !zony.is_empty() {
        riadok(&mut t, format!("snímače teploty: {}", zony.join(" · ")));
    }
    t
}

/// Všetky snímače teploty (zóny jadra) s teplotou.
pub fn snimace_teploty() -> Vec<String> {
    let mut v: Vec<_> = fs::read_dir("/sys/class/thermal")
        .into_iter()
        .flatten()
        .flatten()
        .map(|z| z.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("thermal_zone")))
        .collect();
    v.sort();
    v.iter()
        .map(|p| {
            let teplota = citaj(p.join("temp")).and_then(|s| s.parse::<f64>().ok()).map(|m| m / 1000.0);
            format!(
                "{} {}",
                citaj(p.join("type")).unwrap_or_else(|| "?".into()),
                teplota.map_or("?".into(), |t| format!("{t:.1} °C"))
            )
        })
        .collect()
}

/// Skutočný takt jadra 0 (MHz).
pub fn takt_mhz() -> Option<u32> {
    citaj("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")?.parse::<u32>().ok().map(|k| k / 1000)
}

/// Stav systému: takty jadier, pamäť, jadro Linuxu a jeho príkazový riadok.
pub fn stav_systemu() -> String {
    let mut t = String::new();
    let mut takty = Vec::new();
    for j in 0..16 {
        let p = format!("/sys/devices/system/cpu/cpu{j}");
        if !Path::new(&p).exists() {
            break;
        }
        let zapnute = citaj(format!("{p}/online")).is_none_or(|s| s == "1");
        let takt = citaj(format!("{p}/cpufreq/scaling_cur_freq")).and_then(|s| s.parse::<u32>().ok());
        takty.push(match (zapnute, takt) {
            (false, _) => format!("cpu{j} vypnuté"),
            (true, Some(k)) => format!("cpu{j} {} MHz", k / 1000),
            (true, None) => format!("cpu{j} ?"),
        });
    }
    riadok(&mut t, format!("jadrá: {}", takty.join(" · ")));
    if let Some(g) = citaj("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor") {
        riadok(&mut t, format!("regulácia taktu: {g}"));
    }
    if let Some(m) = citaj("/proc/meminfo") {
        let hodnota = |k: &str| {
            m.lines()
                .find(|r| r.starts_with(k))
                .and_then(|r| r.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                / 1024
        };
        riadok(
            &mut t,
            format!(
                "pamäť: spolu {} MB · dostupná {} MB · voľná {} MB · odkladacia použitá {} MB",
                hodnota("MemTotal:"),
                hodnota("MemAvailable:"),
                hodnota("MemFree:"),
                hodnota("SwapTotal:").saturating_sub(hodnota("SwapFree:"))
            ),
        );
    }
    // R130 (prvý krok – len meranie): napätia regulátorov dosky, základ pre budúcu skúšku podvoltovania
    let mut napatia: Vec<String> = fs::read_dir("/sys/class/regulator")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|z| {
            let p = z.path();
            let uv = citaj(p.join("microvolts"))?.parse::<f64>().ok()?;
            Some(format!("{} {:.3} V", citaj(p.join("name")).unwrap_or_else(|| "?".into()), uv / 1e6))
        })
        .collect();
    napatia.sort();
    if !napatia.is_empty() {
        riadok(&mut t, format!("napätia regulátorov: {}", napatia.join(" · ")));
    }
    if let Some(v) = citaj("/proc/version") {
        riadok(&mut t, format!("jadro: {}", v.split_whitespace().take(3).collect::<Vec<_>>().join(" ")));
    }
    if let Some(c) = citaj("/proc/cmdline") {
        riadok(&mut t, format!("príkazový riadok jadra: {c}"));
    }
    if let Some(u) = citaj("/proc/loadavg") {
        riadok(
            &mut t,
            format!(
                "priemerná záťaž (1/5/15 min): {}",
                u.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
            ),
        );
    }
    t
}

/// Čo má vlákno záznamu urobiť.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uloha {
    /// Záznam danej minúty merania.
    Zaznam(u32),
    /// Opakovanie zápisu, ktorý sa predtým nepodaril (karta alebo kľúč neboli k dispozícii).
    Opakovanie,
}

/// Čo pripraví hlavná slučka (rýchlo, bez čakania) – zvyšok urobí vlákno s najnižšou prioritou.
#[derive(Debug)]
pub struct Priprava {
    /// Hlavička a údaje, ktoré treba odčítať presne v okamihu záznamu (teplota, obrazovka, strážca).
    pub text: String,
    /// Doplnok na koniec správy (hlásenia jadra, krivka teploty).
    pub dovetok: String,
    /// Obraz obrazovky na snímku (PNG sa stlačí až vo vlákne).
    pub platno: Option<orchidea_pismo::Platno>,
    /// Pridať identifikáciu hardvéru (na začiatku a na konci merania).
    pub identifikovat: bool,
    /// Disk, z ktorého SKLENÍK štartoval (napr. `mmcblk0`) – zapisuje sa naň prednostne.
    pub spustaci_disk: Option<String>,
    /// Meno súborov bez poradového čísla a nosiča.
    pub predpona: String,
}

#[derive(Debug)]
struct Vysledok {
    uloha: Uloha,
    zapis: Result<Zapisane, String>,
    /// Súbory, ktoré sa nepodarilo zapísať (pri chybe všetky – skúsia sa znova).
    nezapisane: Vec<Subor>,
}

/// Riadenie samomerania v hlavnej slučke KLÍČA (R122). Hlavná slučka len odčíta hodnoty, ktoré už má
/// (teplotu, výkon a vyťaženie zo strážcu); všetko ostatné robí jedno vlákno s najnižšou prioritou.
#[derive(Debug, Default)]
pub struct Riadenie {
    /// Začiatok merania: okamih a hodiny „beží“ (s).
    zaciatok: Option<(Instant, u64)>,
    /// Index ďalšieho míľnika v `MILNIKY_MIN`.
    dalsi: usize,
    zaznamy: Vec<(u32, bool)>,
    krivka: Vec<samomeranie::BodKrivky>,
    dalsi_bod_s: u64,
    fronta: Vec<Subor>,
    vlakno: Option<std::thread::JoinHandle<Vysledok>>,
    /// (oddiel, poradie) z posledného úspešného zápisu.
    predosle: Option<(String, u32)>,
    nosic: Option<String>,
    chyba: Option<String>,
    dalsi_pokus: Option<Instant>,
}

impl Riadenie {
    /// Začne meranie (raz). Vráti `true`, ak práve začalo.
    pub fn zacni(&mut self, bezi_s: u64) -> bool {
        if self.zaciatok.is_some() {
            return false;
        }
        self.zaciatok = Some((Instant::now(), bezi_s));
        true
    }

    /// Hodiny „beží“ (s) pri začiatku merania.
    pub fn zaciatok_s(&self) -> Option<u64> {
        self.zaciatok.map(|z| z.1)
    }

    /// Bod krivky teploty – raz za 30 s, len prvých 30 minút. Hodnoty sú z posledného kroku strážcu;
    /// navyše sa prečíta len takt (jeden malý súbor jadra).
    pub fn bod(
        &mut self,
        teplota: Option<f64>,
        vykon: f64,
        strop_mhz: u32,
        vytazenie: Option<f64>,
        okolie: Option<f64>,
    ) {
        let Some((z, _)) = self.zaciatok else { return };
        let s = z.elapsed().as_secs();
        let koniec = *samomeranie::MILNIKY_MIN.last().unwrap_or(&30) as u64 * 60;
        if s < self.dalsi_bod_s || self.dalsi_bod_s > koniec {
            return;
        }
        self.krivka.push(samomeranie::BodKrivky {
            s,
            teplota,
            vykon,
            strop_mhz,
            takt_mhz: takt_mhz(),
            vytazenie,
            okolie,
        });
        self.dalsi_bod_s += samomeranie::KROK_KRIVKY_S;
    }

    pub fn krivka(&self) -> &[samomeranie::BodKrivky] {
        &self.krivka
    }

    /// Úloha, ktorá je na rade (ak práve nebeží iná).
    pub fn na_rade(&self) -> Option<Uloha> {
        let (z, _) = self.zaciatok?;
        if self.vlakno.is_some() {
            return None;
        }
        if let Some(&m) = samomeranie::MILNIKY_MIN.get(self.dalsi) {
            return (z.elapsed() >= Duration::from_secs(m as u64 * 60)).then_some(Uloha::Zaznam(m));
        }
        (!self.fronta.is_empty() && self.dalsi_pokus.is_none_or(|t| Instant::now() >= t))
            .then_some(Uloha::Opakovanie)
    }

    /// Spustí úlohu vo vlákne s najnižšou prioritou.
    pub fn spusti(&mut self, uloha: Uloha, p: Priprava) {
        if let Uloha::Zaznam(_) = uloha {
            self.dalsi += 1;
        }
        let mut fronta = std::mem::take(&mut self.fronta);
        let predosle = self.predosle.clone();
        let nl = samomeranie::NL;
        self.vlakno = Some(std::thread::spawn(move || {
            // najnižšia priorita: stláčanie snímky a zápis nepredbehnú strážcu ani nič iné
            let _ = sys::nizka_priorita();
            let t = Instant::now();
            let mut subory = Vec::new();
            if let Uloha::Zaznam(m) = uloha {
                let mut text = p.text;
                // snímače a stav systému sa čítajú hneď – ešte pred stláčaním snímky
                text += &samomeranie::nadpis("SNÍMAČE TEPLOTY A SYSTÉM");
                text += &format!("  {}{nl}", snimace_teploty().join(" · "));
                text += &stav_systemu();
                if p.identifikovat {
                    text += &samomeranie::nadpis("IDENTIFIKÁCIA HARDVÉRU");
                    text += &identifikacia();
                }
                text += &p.dovetok;
                let png = p.platno.map(|pl| (pl.sirka, pl.vyska, pl.png()));
                text += &samomeranie::nadpis("SNÍMKA OBRAZOVKY");
                match &png {
                    Some((w, h, d)) => {
                        text += &format!(
                            "  {ZNACKA_PORADIA}-…-{m:02}min.png · {w}×{h} · {} kB{nl}",
                            d.len().div_ceil(1024)
                        )
                    }
                    None => text += &format!("  bez snímky{nl}"),
                }
                subory.push(Subor { pripona: ".txt".into(), obsah: text.into_bytes(), do_spolu: true });
                if let Some((_, _, d)) = png {
                    subory.push(Subor { pripona: format!("-{m:02}min.png"), obsah: d, do_spolu: false });
                }
            }
            let priprava = t.elapsed();
            // najprv to, čo sa predtým nepodarilo zapísať – správy zostanú v správnom poradí
            fronta.extend(subory);
            let nosice = najdi_nosice(p.spustaci_disk.as_deref());
            let zaver = |zapis: Duration| {
                format!(
                    "  záznam trval: príprava a snímka {} ms · zápis na kartu {} ms · vlákno s najnižšou prioritou{nl}{nl}",
                    priprava.as_millis(),
                    zapis.as_millis()
                )
            };
            match zapis(&nosice, &p.predpona, predosle.as_ref(), &fronta, &zaver) {
                Ok(z) => Vysledok { uloha, zapis: Ok(z), nezapisane: Vec::new() },
                Err(e) => Vysledok { uloha, zapis: Err(e), nezapisane: fronta },
            }
        }));
    }

    /// Prevezme výsledok vlákna, ak skončilo. Vráti správu do denníka.
    pub fn dokonci(&mut self) -> Option<String> {
        if !self.vlakno.as_ref()?.is_finished() {
            return None;
        }
        let v = self.vlakno.take()?.join();
        let co = |u: Uloha| match u {
            Uloha::Zaznam(m) => format!("záznam {m}. minúty"),
            Uloha::Opakovanie => "opakovaný zápis".into(),
        };
        Some(match v {
            Ok(Vysledok { uloha, zapis: Ok(z), .. }) => {
                self.predosle = Some((z.nosic.oddiel.clone(), z.poradie));
                self.nosic = Some(z.nosic.popis.clone());
                self.chyba = None;
                self.dalsi_pokus = None;
                // čo sa predtým nepodarilo, je teraz zapísané
                for zaznam in &mut self.zaznamy {
                    zaznam.1 = true;
                }
                if let Uloha::Zaznam(m) = uloha {
                    self.zaznamy.push((m, true));
                }
                let subory: Vec<String> =
                    z.subory.iter().map(|(m, b)| format!("{m} ({} kB)", b.div_ceil(1024))).collect();
                format!(
                    "{} zapísaný na {} ({}): {}",
                    co(uloha),
                    z.nosic.popis,
                    z.nosic.oddiel,
                    subory.join(", ")
                )
            }
            Ok(Vysledok { uloha, zapis: Err(e), nezapisane }) => {
                self.fronta = nezapisane;
                if let Uloha::Zaznam(m) = uloha {
                    self.zaznamy.push((m, false));
                }
                self.chyba = Some(e.clone());
                self.dalsi_pokus = Some(Instant::now() + Duration::from_secs(60));
                format!("{} sa nepodarilo zapísať: {e} – skúsi sa znova", co(uloha))
            }
            Err(_) => {
                self.chyba = Some("vlákno záznamu zlyhalo".into());
                self.dalsi_pokus = Some(Instant::now() + Duration::from_secs(60));
                "vlákno záznamu zlyhalo".into()
            }
        })
    }

    /// Prvý záznam je vybavený (zapísaný alebo zlyhal) – pre skúšku v emulátore.
    pub fn prvy_vybaveny(&self) -> bool {
        !self.zaznamy.is_empty() || (self.zaciatok.is_some() && self.dalsi > 0 && self.vlakno.is_none())
    }

    /// Čo ukáže riadok „Meranie“.
    pub fn stav(&self) -> crate::obrazovka::Samomeranie {
        use crate::obrazovka::Samomeranie;
        let Some((_, s0)) = self.zaciatok else { return Samomeranie::Pripravuje };
        let poradie = self.predosle.as_ref().map(|p| p.1);
        let n = samomeranie::MILNIKY_MIN.len();
        let vsetko = self.dalsi >= n && self.vlakno.is_none();
        match (vsetko, poradie) {
            (true, Some(p)) if self.fronta.is_empty() => {
                Samomeranie::Hotove { poradie: p, nosic: self.nosic.clone().unwrap_or_default() }
            }
            (true, _) => Samomeranie::Nezapisane(
                self.chyba.as_deref().unwrap_or("neznáma chyba").chars().take(60).collect(),
            ),
            _ => Samomeranie::Bezi {
                zaznamy: self.zaznamy.clone(),
                pri_s: s0 + samomeranie::MILNIKY_MIN[self.dalsi.min(n - 1)] as u64 * 60,
                poradie,
            },
        }
    }
}
