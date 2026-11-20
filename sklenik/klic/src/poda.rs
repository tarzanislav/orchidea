//! PÔDA – presný profil hardvéru (R92). KLÍČ ho prečíta pri štarte len zo súborov jadra (/proc, /sys,
//! strom popisu hardvéru) a podľa neho sa nastaví: koľko pamäte, aký čip a jeho verzia, aké jadrá procesora
//! a ich inštrukcie, úseky pamäte NUMA, zdroj napájania. Nič sa nemeria ani nezapisuje – trvá to milisekundy.

use std::fs;
use std::path::Path;

/// Profil hardvéru.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Poda {
    /// Kód revízie dosky Raspberry Pi (napr. 0xc04170); iné dosky ho nemajú.
    pub revizia: Option<u32>,
    /// RAM dosky podľa kódu revízie (MB).
    pub ram_dosky_mb: Option<u32>,
    /// Revízia plošného spoja (napr. „1.4“).
    pub revizia_dosky: Option<String>,
    /// Výrobca dosky podľa kódu revízie.
    pub vyrobca: Option<&'static str>,
    /// Čip (napr. „BCM2712 D0“, „BCM2837“, „Amlogic S905X3“).
    pub cip: Option<String>,
    /// Jadro procesora (napr. „Cortex-A76“).
    pub jadro: Option<&'static str>,
    /// Počet jadier procesora.
    pub jadier: usize,
    /// Najvyšší takt procesora (MHz).
    pub max_mhz: Option<u32>,
    /// Užitočné inštrukcie procesora (NEON, súčin bodov, FP16, AES, …).
    pub instrukcie: Vec<&'static str>,
    /// Pamäť, ktorú vidí jadro systému (kB), a koľko je hneď voľnej.
    pub pamat_kb: u64,
    pub dostupna_kb: u64,
    /// Úseky pamäte NUMA (RPi 4/5 – pamäť rozdelená na úseky pre rýchlejší prístup viacerých jadier).
    pub uzly_numa: usize,
    /// Zdroj napájania podľa firmvéru (RPi 5): najväčší prúd zdroja (mA) a či má USB vysoký limit (1,6 A).
    pub zdroj_ma: Option<u32>,
    pub usb_vysoky_prud: Option<bool>,
    /// Firmvér zaznamenal, že doska bola niekedy pretaktovaná nad povolené napätie (bit záruky).
    pub zaruka_porusena: bool,
    /// Strážca pravdy (R100): koľko jadier prešlo testom so známym výsledkom a ktoré počítali chybne.
    pub overene_jadra: usize,
    pub chybne_jadra: Vec<usize>,
}

fn citaj(cesta: &Path) -> Option<String> {
    fs::read_to_string(cesta).ok().map(|s| s.trim_end_matches(['\n', '\0']).trim().to_string())
}

fn cislo_be(cesta: &Path) -> Option<u32> {
    let b = fs::read(cesta).ok()?;
    (b.len() >= 4).then(|| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Hodnota riadku `/proc/cpuinfo` (prvý výskyt).
fn z_cpuinfo<'a>(cpuinfo: &'a str, kluc: &str) -> Option<&'a str> {
    cpuinfo.lines().find_map(|r| {
        let (k, v) = r.split_once(':')?;
        (k.trim() == kluc).then(|| v.trim())
    })
}

/// Jadro procesora podľa „CPU part“ (výrobca ARM).
fn jadro_procesora(cast: &str) -> Option<&'static str> {
    Some(match u32::from_str_radix(cast.trim_start_matches("0x"), 16).ok()? {
        0xb76 => "ARM1176",
        0xc07 => "Cortex-A7",
        0xd03 => "Cortex-A53",
        0xd05 => "Cortex-A55",
        0xd08 => "Cortex-A72",
        0xd0b => "Cortex-A76",
        _ => return None,
    })
}

/// Užitočné inštrukcie z riadku „Features“ (jadro ich hlási na 32 aj 64 bitoch inými menami).
fn instrukcie(priznaky: &str) -> Vec<&'static str> {
    let ma = |m: &str| priznaky.split_whitespace().any(|p| p == m);
    let mut v = Vec::new();
    if ma("asimd") || ma("neon") {
        v.push("NEON");
    } else if ma("vfp") {
        v.push("VFP");
    }
    if ma("asimddp") {
        v.push("súčin bodov");
    }
    if ma("asimdhp") || ma("fphp") {
        v.push("FP16");
    }
    if ma("aes") {
        v.push("AES");
    }
    if ma("sha2") {
        v.push("SHA2");
    }
    if ma("crc32") {
        v.push("CRC32");
    }
    if ma("atomics") {
        v.push("LSE");
    }
    v
}

/// Rozloží kód revízie Raspberry Pi: (RAM v MB, revízia dosky, výrobca, čip, bit záruky).
pub fn rozloz_reviziu(
    kod: u32,
) -> (Option<u32>, Option<String>, Option<&'static str>, Option<&'static str>, bool) {
    let zaruka = kod & (1 << 25) != 0 || kod & 0xFF00_0000 == 0x1000_0000;
    if kod & (1 << 23) == 0 {
        // starý kód (RPi 1): RAM podľa tabuľky
        let ram = match kod & 0xFFFF {
            0x2..=0x9 | 0x12 => Some(256),
            0xd..=0x11 | 0x13 | 0x14 => Some(512),
            0x15 => Some(256),
            _ => None,
        };
        return (ram, None, None, Some("BCM2835"), zaruka);
    }
    let ram = Some(256u32 << ((kod >> 20) & 7));
    let revizia = Some(format!("1.{}", kod & 0xF));
    let vyrobca = match (kod >> 16) & 0xF {
        0 => Some("Sony UK"),
        1 => Some("Egoman"),
        2 | 4 => Some("Embest"),
        3 => Some("Sony Japan"),
        5 => Some("Stadium"),
        _ => None,
    };
    let cip = match (kod >> 12) & 0xF {
        0 => Some("BCM2835"),
        1 => Some("BCM2836"),
        2 => Some("BCM2837"),
        3 => Some("BCM2711"),
        4 => Some("BCM2712"),
        _ => None,
    };
    (ram, revizia, vyrobca, cip, zaruka)
}

/// BCM2712 vo verzii D0 (menší a úspornejší čip) má vlastný ovládač pinov v strome popisu hardvéru.
fn bcm2712_d0(dt: &Path) -> bool {
    let Ok(polozky) = fs::read_dir(dt) else { return false };
    polozky.flatten().filter(|p| p.file_name().to_string_lossy().starts_with("soc")).any(|soc| {
        fs::read_dir(soc.path()).into_iter().flatten().flatten().any(|n| {
            n.file_name().to_string_lossy().starts_with("pinctrl@")
                && fs::read(n.path().join("compatible"))
                    .map(|c| c.windows(10).any(|w| w == b"bcm2712d0-"))
                    .unwrap_or(false)
        })
    })
}

/// Prečíta profil hardvéru zo súborov jadra pod `koren` (na zariadení „/“).
pub fn nacitaj(koren: &Path) -> Poda {
    let mut p = Poda::default();
    let dt = koren.join("proc/device-tree");
    let cpuinfo = citaj(&koren.join("proc/cpuinfo")).unwrap_or_default();

    // kód 0 nie je platný (napr. emulátor QEMU) – vtedy sa čip zistí z popisu hardvéru
    p.revizia = cislo_be(&dt.join("system/linux,revision"))
        .or_else(|| z_cpuinfo(&cpuinfo, "Revision").and_then(|r| u32::from_str_radix(r, 16).ok()))
        .filter(|&k| k != 0);
    if let Some(kod) = p.revizia {
        let (ram, rev, vyrobca, cip, zaruka) = rozloz_reviziu(kod);
        p.ram_dosky_mb = ram;
        p.revizia_dosky = rev;
        p.vyrobca = vyrobca;
        p.zaruka_porusena = zaruka;
        p.cip = cip.map(|c| if c == "BCM2712" && bcm2712_d0(&dt) { "BCM2712 D0".into() } else { c.into() });
    }
    if p.cip.is_none() {
        let kompat = fs::read(dt.join("compatible")).unwrap_or_default();
        let ma = |s: &[u8]| kompat.split(|&b| b == 0).any(|c| c == s);
        p.cip = if ma(b"hardkernel,odroid-c4") {
            Some("Amlogic S905X3".into())
        } else if ma(b"amlogic,sm1") {
            Some("Amlogic SM1".into())
        } else {
            kompat.split(|&b| b == 0).rfind(|c| !c.is_empty()).map(|c| {
                let c = String::from_utf8_lossy(c);
                // „brcm,bcm2837“ → „BCM2837“
                match c.strip_prefix("brcm,") {
                    Some(b) => b.to_uppercase(),
                    None => c.into_owned(),
                }
            })
        };
    }

    p.jadier = cpuinfo.lines().filter(|r| r.starts_with("processor")).count();
    p.jadro = z_cpuinfo(&cpuinfo, "CPU part").and_then(jadro_procesora);
    p.instrukcie = instrukcie(z_cpuinfo(&cpuinfo, "Features").unwrap_or(""));
    p.max_mhz = citaj(&koren.join("sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"))
        .and_then(|k| k.parse::<u32>().ok())
        .map(|khz| khz / 1000);

    let meminfo = citaj(&koren.join("proc/meminfo")).unwrap_or_default();
    let kb = |kluc: &str| {
        z_cpuinfo(&meminfo, kluc)
            .and_then(|v| v.trim_end_matches("kB").trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    p.pamat_kb = kb("MemTotal");
    p.dostupna_kb = kb("MemAvailable");

    p.uzly_numa = fs::read_dir(koren.join("sys/devices/system/node"))
        .map(|d| {
            d.flatten()
                .filter(|n| {
                    let m = n.file_name().to_string_lossy().into_owned();
                    m.len() > 4 && m.starts_with("node") && m[4..].bytes().all(|b| b.is_ascii_digit())
                })
                .count()
        })
        .unwrap_or(0);

    let zdroj = dt.join("chosen/power");
    p.zdroj_ma = cislo_be(&zdroj.join("max_current"));
    p.usb_vysoky_prud = cislo_be(&zdroj.join("usb_max_current_enable")).map(|v| v != 0);
    p
}

impl Poda {
    /// Celková pamäť na zobrazenie (MB): podľa kódu dosky, inak podľa jadra.
    pub fn ram_mb(&self) -> u64 {
        self.ram_dosky_mb.map(u64::from).unwrap_or(self.pamat_kb / 1024)
    }

    /// Malá pamäť (≤ 1 GB) – zapne sa stlačená pamäť zram (R94).
    pub fn mala_pamat(&self) -> bool {
        self.pamat_kb > 0 && self.pamat_kb <= 1_100_000
    }

    /// Jeden riadok do denníka.
    pub fn riadok(&self) -> String {
        let mut casti = Vec::new();
        if let Some(r) = self.revizia {
            casti.push(format!(
                "kód dosky {r:06x} (rev. {}, {}){}",
                self.revizia_dosky.as_deref().unwrap_or("?"),
                self.vyrobca.unwrap_or("?"),
                if self.zaruka_porusena { ", bit záruky nastavený" } else { "" }
            ));
        }
        casti.push(format!("čip {}", self.cip.as_deref().unwrap_or("?")));
        casti.push(format!(
            "{}× {}{}",
            self.jadier,
            self.jadro.unwrap_or("?"),
            self.max_mhz.map(|m| format!(" {m} MHz")).unwrap_or_default()
        ));
        if !self.instrukcie.is_empty() {
            casti.push(self.instrukcie.join(" "));
        }
        casti.push(format!(
            "RAM {} MB (jadro {} MB, voľné {} MB)",
            self.ram_mb(),
            self.pamat_kb / 1024,
            self.dostupna_kb / 1024
        ));
        casti.push(format!("NUMA {}", self.uzly_numa));
        if let Some(ma) = self.zdroj_ma {
            casti.push(format!(
                "zdroj {ma} mA, USB {}",
                match self.usb_vysoky_prud {
                    Some(true) => "1,6 A",
                    Some(false) => "0,6 A",
                    None => "?",
                }
            ));
        }
        format!("PÔDA – {}", casti.join("; "))
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use std::path::PathBuf;

    struct Strom(PathBuf);
    impl Strom {
        fn novy(meno: &str) -> Strom {
            let d = std::env::temp_dir().join(format!("poda-{meno}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            Strom(d)
        }
        fn subor(self, cesta: &str, obsah: &[u8]) -> Strom {
            let c = self.0.join(cesta);
            fs::create_dir_all(c.parent().unwrap()).unwrap();
            fs::write(c, obsah).unwrap();
            self
        }
    }
    impl Drop for Strom {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn kod_revizie() {
        // RPi 5 4 GB, Sony UK, rev. 1.0
        let (ram, rev, vyr, cip, z) = rozloz_reviziu(0xc04170);
        assert_eq!(
            (ram, rev.as_deref(), vyr, cip, z),
            (Some(4096), Some("1.0"), Some("Sony UK"), Some("BCM2712"), false)
        );
        // RPi 3 B+ 1 GB, Sony UK, rev. 1.3
        assert_eq!(rozloz_reviziu(0xa020d3).0, Some(1024));
        assert_eq!(rozloz_reviziu(0xa020d3).3, Some("BCM2837"));
        // RPi 4 B 2 GB rev. 1.4
        assert_eq!(rozloz_reviziu(0xb03114).1.as_deref(), Some("1.4"));
        // RPi 400 4 GB
        assert_eq!(rozloz_reviziu(0xc03130).0, Some(4096));
        // RPi 2 v1.2 má čip BCM2837 (64-bitový)
        assert_eq!(rozloz_reviziu(0xa22042).3, Some("BCM2837"));
        // starý kód RPi 1 B rev. 2 – 512 MB; bit záruky v starom kóde
        assert_eq!(rozloz_reviziu(0x000e).0, Some(512));
        assert!(rozloz_reviziu(0x1000_000e).4);
        // RPi 5 8 GB so zdvihnutým bitom záruky
        assert!(rozloz_reviziu(0xc04170 | 1 << 25).4);
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore)]
    fn rpi5_d0() {
        let s = Strom::novy("rpi5")
            .subor("proc/device-tree/system/linux,revision", &0xd04171u32.to_be_bytes())
            .subor("proc/device-tree/soc@107c000000/pinctrl@7d504100/compatible", b"brcm,bcm2712d0-pinctrl\0")
            .subor("proc/device-tree/chosen/power/max_current", &5000u32.to_be_bytes())
            .subor("proc/device-tree/chosen/power/usb_max_current_enable", &1u32.to_be_bytes())
            .subor(
                "proc/cpuinfo",
                b"processor\t: 0\nFeatures\t: fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics fphp asimdhp \
cpuid asimdrdm lrcpc dcpop asimddp\nCPU part\t: 0xd0b\n\nprocessor\t: 1\n\nprocessor\t: 2\n\nprocessor\t: 3\n",
            )
            .subor("proc/meminfo", b"MemTotal:        8245632 kB\nMemFree:  8000000 kB\nMemAvailable:    7980000 kB\n")
            .subor("sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq", b"2400000\n")
            .subor("sys/devices/system/node/node0/.x", b"")
            .subor("sys/devices/system/node/node7/.x", b"")
            .subor("sys/devices/system/node/possible", b"0-7\n");
        let p = nacitaj(&s.0);
        assert_eq!(p.cip.as_deref(), Some("BCM2712 D0"));
        assert_eq!(p.ram_mb(), 8192);
        assert_eq!(p.revizia_dosky.as_deref(), Some("1.1"));
        assert_eq!(p.jadier, 4);
        assert_eq!(p.jadro, Some("Cortex-A76"));
        assert_eq!(p.max_mhz, Some(2400));
        assert_eq!(p.instrukcie, vec!["NEON", "súčin bodov", "FP16", "AES", "SHA2", "CRC32", "LSE"]);
        assert_eq!(p.uzly_numa, 2);
        assert_eq!((p.zdroj_ma, p.usb_vysoky_prud), (Some(5000), Some(true)));
        assert!(!p.mala_pamat());
        assert!(p.riadok().contains("čip BCM2712 D0"));
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore)]
    fn rpi3_a_odroid() {
        // RPi 3 B+: kód revízie len v /proc/cpuinfo, 1 GB → zram
        let s = Strom::novy("rpi3")
            .subor("proc/cpuinfo", b"processor\t: 0\nFeatures\t: fp asimd evtstrm crc32 cpuid\nCPU part\t: 0xd03\nRevision\t: a020d3\n")
            .subor("proc/meminfo", b"MemTotal:         948304 kB\nMemAvailable:     870000 kB\n");
        let p = nacitaj(&s.0);
        assert_eq!(p.cip.as_deref(), Some("BCM2837"));
        assert_eq!(p.instrukcie, vec!["NEON", "CRC32"]);
        assert!(p.mala_pamat());
        assert_eq!(p.zdroj_ma, None);
        // emulátor RPi 2: kód revízie 0 – čip podľa popisu hardvéru
        let s = Strom::novy("qemu")
            .subor("proc/device-tree/compatible", b"raspberrypi,2-model-b\0brcm,bcm2836\0")
            .subor("proc/cpuinfo", b"processor\t: 0\nCPU part\t: 0xc07\nRevision\t: 0000\n");
        let p = nacitaj(&s.0);
        assert_eq!((p.revizia, p.cip.as_deref()), (None, Some("BCM2836")));
        // Odroid C4: bez kódu revízie, čip podľa popisu hardvéru
        let s = Strom::novy("odroid")
            .subor("proc/device-tree/compatible", b"hardkernel,odroid-c4\0amlogic,sm1\0")
            .subor("proc/cpuinfo", b"processor\t: 0\nCPU part\t: 0xd05\n");
        let p = nacitaj(&s.0);
        assert_eq!(
            (p.cip.as_deref(), p.jadro, p.revizia),
            (Some("Amlogic S905X3"), Some("Cortex-A55"), None)
        );
    }
}
