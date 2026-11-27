//! Vlastné úsporné ovládanie častí čipu cez /sys:
//! - **R178** – nepoužité radiče kariet: vstavaná eMMC Androidu a slot Wi-Fi bez ovládača bežia stále (eMMC
//!   s taktom karty až 200 MHz, radič sa zastaví len pri zmene rýchlosti). Odpojením od ovládača jadro zastaví
//!   takt karty aj radiča a čip Wi-Fi drží v resete. Navyše **istota, že sa do krabičky nič nezapíše**.
//!   Karta SD sa ako „nepoužitá“ neodpája nikdy (môže byť spúšťacia).
//! - **R189** – karta SD, ktorú jadro opakovane nevie spustiť (Odroid C4 + lacná karta, chyby CRC): najprv
//!   nižší takt (strop v jadre SKLENÍKA), potom sa radič odpojí – jadro by ju inak skúšalo každú sekundu.
//! - **R167** – hlboký spánok jadier: vlastný ovládač jadra `sklenik_jadra` (cpuidle cez PSCI), ktorý zapína
//!   KLÍČ až pri bežiacom strážnom časovači a s poistkou na karte.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn citaj(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Patrí zariadenie `meno` (napr. `mmcblk1p2`, `/dev/mmcblk1boot0`) k disku `disk` (`mmcblk1`)?
fn patri(meno: &str, disk: &str) -> bool {
    let meno = meno.trim_start_matches("/dev/");
    meno == disk
        || meno
            .strip_prefix(disk)
            .is_some_and(|z| z.starts_with('p') || z.starts_with("boot") || z.starts_with("rpmb"))
}

/// Radič kariet (platformové zariadenie), ktorý sa dá odpojiť od ovládača a znova pripojiť.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Radic {
    /// Meno platformového zariadenia (napr. `ffe07000.mmc`).
    pub zariadenie: String,
    /// Priečinok ovládača v /sys (v ňom `unbind` a `bind`).
    pub ovladac: PathBuf,
    /// Čo na ňom je (napr. „eMMC 032GE4“).
    pub popis: String,
}

impl Radic {
    pub fn riadok(&self) -> String {
        format!("{} ({})", self.popis, self.zariadenie)
    }
}

/// Nepoužité radiče kariet: vstavaná eMMC, ktorej žiadna časť nie je pripojená ani chránená (spúšťací disk,
/// nosič záznamov), a slot SDIO, ktorého funkcie nemajú ovládač. Karty SD sa nevracajú nikdy.
pub fn nepouzite_radice(koren: &Path, chranene: &[String]) -> Vec<Radic> {
    let pripojene: Vec<String> = citaj(koren.join("proc/mounts"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    let mut karty: Vec<_> =
        fs::read_dir(koren.join("sys/bus/mmc/devices")).into_iter().flatten().flatten().collect();
    karty.sort_by_key(|z| z.file_name());
    let mut v = Vec::new();
    for z in karty {
        let karta = z.file_name().to_string_lossy().to_string();
        let Ok(cesta) = fs::canonicalize(z.path()) else { continue };
        let popis = match citaj(cesta.join("type")).as_deref() {
            Some("MMC") => {
                let bloky: Vec<String> = fs::read_dir(cesta.join("block"))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|b| b.file_name().to_string_lossy().to_string())
                    .collect();
                let pouzita = bloky.is_empty()
                    || bloky.iter().any(|b| {
                        chranene.iter().any(|c| patri(c, b)) || pripojene.iter().any(|p| patri(p, b))
                    });
                if pouzita {
                    continue;
                }
                format!("eMMC {}", citaj(cesta.join("name")).unwrap_or_default())
            }
            Some("SDIO") => {
                let predpona = format!("{karta}:");
                let s_ovladacom = fs::read_dir(&cesta).into_iter().flatten().flatten().any(|f| {
                    f.file_name().to_string_lossy().starts_with(&predpona) && f.path().join("driver").exists()
                });
                if s_ovladacom {
                    continue;
                }
                format!(
                    "Wi-Fi SDIO {}:{} bez ovládača",
                    citaj(cesta.join("vendor")).unwrap_or_default().trim_start_matches("0x"),
                    citaj(cesta.join("device")).unwrap_or_default().trim_start_matches("0x")
                )
            }
            _ => continue,
        };
        // …/ffe07000.mmc/mmc_host/mmc1/mmc1:0001 – radič je o tri úrovne vyššie
        let Some(radic) = cesta.parent().and_then(Path::parent).and_then(Path::parent) else { continue };
        let Ok(ovladac) = fs::canonicalize(radic.join("driver")) else { continue };
        let Some(zariadenie) = radic.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
        v.push(Radic { zariadenie, ovladac, popis });
    }
    v
}

/// Odpojí radič od ovládača – jadro zastaví takty a kartu vypne (eMMC ostane napájaná, len stojí).
pub fn odpoj(r: &Radic) -> io::Result<()> {
    fs::write(r.ovladac.join("unbind"), &r.zariadenie)
}

/// Znova pripojí radič (jadro kartu nanovo rozpozná).
pub fn pripoj(r: &Radic) -> io::Result<()> {
    fs::write(r.ovladac.join("bind"), &r.zariadenie)
}

/// R189: z textu hlásenia jadra „mmc0: error -84 whilst initialising SD card“ vráti meno radiča (`mmc0`).
pub fn zlyhanie_sd(sprava: &str) -> Option<String> {
    let (host, zvysok) = sprava.split_once(": ")?;
    (host.starts_with("mmc") && zvysok.contains("whilst initialising SD card")).then(|| host.to_string())
}

/// R235: z hlásenia jadra „I/O error, dev mmcblk0, sector …“ vráti blokové zariadenie (`mmcblk0`).
pub fn chyba_citania_mmc(sprava: &str) -> Option<String> {
    let zvysok = sprava.split_once("I/O error, dev ")?.1;
    let disk = zvysok.split([',', ' ']).next()?;
    (disk.starts_with("mmcblk") && !disk.contains("boot") && !disk.contains("rpmb")).then(|| disk.to_string())
}

/// R235: radič (`mmcN`), na ktorom je blokové zariadenie `mmcblkN` (cez sysfs, nie podľa čísla).
pub fn host_disku(koren: &Path, disk: &str) -> Option<String> {
    let cesta = fs::canonicalize(koren.join("sys/block").join(disk).join("device")).ok()?;
    let host = cesta.parent()?.file_name()?.to_string_lossy().to_string();
    host.starts_with("mmc").then_some(host)
}

/// R235: stupne taktu karty pri chybách čítania (karta sa rozpozná, ale čítanie dát má chyby CRC – Odroid C4
/// 26. 11.: 50 MHz, chyby takmer pri každom čítaní). Radič sa neodpojí – karta nesie záznamy.
pub const SD_STUPNE_KHZ: [u32; 2] = [25_000, 12_500];
/// Koľko chýb čítania od posledného kroku (a najskôr po 30 s) spustí ďalší, nižší takt.
pub const SD_CHYB_NA_KROK: u32 = 3;

#[derive(Debug, Default)]
pub struct StrazCitania {
    /// mmcN → (chyby od posledného kroku, stupeň 0 = pôvodný takt, čas posledného kroku)
    stav: std::collections::HashMap<String, (u32, usize, Option<std::time::Instant>)>,
}

impl StrazCitania {
    /// Zapíše chybu čítania na radiči; vráti nový strop taktu v kHz, ak treba ďalší krok.
    pub fn chyba(&mut self, host: &str) -> Option<u32> {
        let (n, stupen, od) = self.stav.entry(host.to_string()).or_insert((0, 0, None));
        *n += 1;
        let pokoj = od.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(30));
        if *n < SD_CHYB_NA_KROK || !pokoj || *stupen >= SD_STUPNE_KHZ.len() {
            return None;
        }
        let khz = SD_STUPNE_KHZ[*stupen];
        *stupen += 1;
        *n = 0;
        *od = Some(std::time::Instant::now());
        Some(khz)
    }

    /// Súčasný strop taktu radiča (kHz), ak sa už znížil.
    pub fn strop(&self, host: &str) -> Option<u32> {
        let (_, stupen, _) = self.stav.get(host)?;
        (*stupen > 0).then(|| SD_STUPNE_KHZ[*stupen - 1])
    }
}

/// R189: radič karty `mmcN` (platformové zariadenie a jeho ovládač) a či je v ňom rozpoznaná karta.
pub fn radic_sd(koren: &Path, host: &str) -> Option<(Radic, bool)> {
    let h = koren.join("sys/class/mmc_host").join(host);
    let zariadenie_cesta = fs::canonicalize(h.join("device")).ok()?;
    let ovladac = fs::canonicalize(zariadenie_cesta.join("driver")).ok()?;
    let zariadenie = zariadenie_cesta.file_name()?.to_string_lossy().to_string();
    let predpona = format!("{host}:");
    let ma_kartu =
        fs::read_dir(&h).ok()?.flatten().any(|z| z.file_name().to_string_lossy().starts_with(&predpona));
    Some((Radic { zariadenie, ovladac, popis: format!("karta SD {host}") }, ma_kartu))
}

const SD_STROP: &str = "sys/module/mmc_core/parameters/sklenik_sd_max_khz";

/// R240: je karta radiča v popise hardvéru nevymeniteľná (jadro ju nekontroluje každú sekundu)?
pub fn sd_nevymenna(koren: &Path, host: &str) -> bool {
    koren.join("sys/class/mmc_host").join(host).join("device/of_node/non-removable").exists()
}

/// R189: má jadro strop taktu karty SD (úprava jadra SKLENÍKA)?
pub fn sd_strop_je(koren: &Path) -> bool {
    koren.join(SD_STROP).exists()
}

/// R189: strop taktu karty SD v kHz (0 = bez stropu); platí od ďalšieho rozpoznania karty.
pub fn nastav_sd_strop(koren: &Path, khz: u32) -> io::Result<()> {
    fs::write(koren.join(SD_STROP), khz.to_string())
}

/// R189: strážca karty SD – po opakovaných zlyhaniach najprv nižší takt, potom odpojený radič (jadro by
/// kartu inak skúšalo každú sekundu až do vypnutia – teplo a budenie).
#[derive(Debug, Default)]
pub struct StrazSd {
    zlyhania: std::collections::HashMap<String, u32>,
    /// mmcN → 1 = skúša sa nižší takt, 2 = radič odpojený
    stupen: std::collections::HashMap<String, u8>,
    /// R240: mmcN → čas posledného zlyhania (s od štartu) – nevymeniteľná karta hlási jedno spustenie dávkou
    posledne: std::collections::HashMap<String, f64>,
    pub odpojene: Vec<Radic>,
    pub hlasenie: Option<String>,
}

/// Čo urobiť s radičom karty SD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KrokSd {
    Pomalsie(String),
    Odpoj(String),
}

pub const SD_ZLYHANI: u32 = 5;
pub const SD_POMALY_KHZ: u32 = 25_000;

impl StrazSd {
    /// Zapíše zlyhanie radiča; vráti krok, ak ich je dosť (`strop` = jadro má strop taktu).
    #[cfg(test)]
    pub fn zlyhanie(&mut self, host: &str, strop: bool) -> Option<KrokSd> {
        self.zlyhanie_karty(host, strop, false, 0.0)
    }

    /// R240: ako `zlyhanie`, s ohľadom na nevymeniteľnú kartu – jadro ju spúšťa len raz (pri pripojení radiča),
    /// nie každú sekundu, a jedno spustenie hlási dávkou (až 4 frekvencie). Dávka (hlásenia do 5 s od
    /// predošlého) = jedno zlyhanie a na krok stačí jedno.
    pub fn zlyhanie_karty(
        &mut self,
        host: &str,
        strop: bool,
        nevymenna: bool,
        teraz_s: f64,
    ) -> Option<KrokSd> {
        if nevymenna {
            let predosle = self.posledne.insert(host.to_string(), teraz_s);
            if predosle.is_some_and(|t| teraz_s - t < 5.0) {
                return None;
            }
        }
        let prah = if nevymenna { 1 } else { SD_ZLYHANI };
        let n = self.zlyhania.entry(host.to_string()).or_insert(0);
        *n += 1;
        if *n < prah {
            return None;
        }
        *n = 0;
        let stupen = self.stupen.entry(host.to_string()).or_insert(0);
        match *stupen {
            0 if strop => {
                *stupen = 1;
                Some(KrokSd::Pomalsie(host.to_string()))
            }
            0 | 1 => {
                *stupen = 2;
                Some(KrokSd::Odpoj(host.to_string()))
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn stupen(&self, host: &str) -> u8 {
        self.stupen.get(host).copied().unwrap_or(0)
    }
}

/// Parametre vlastného ovládača spánku jadier v jadre (R167).
const SPANOK: &str = "sys/module/sklenik_jadra/parameters";

/// Má jadro vlastný ovládač spánku jadier?
pub fn spanok_jadier_je(koren: &Path) -> bool {
    koren.join(SPANOK).join("zapnute").exists()
}

/// Zapne alebo vypne hlboký spánok jadier; vráti hlásenie ovládača (výsledok overenia firmvéru).
/// Pri odmietnutí (firmvér bez CPU_SUSPEND, rozšírený formát PSCI) vráti chybu s hlásením ovládača.
pub fn nastav_spanok_jadier(koren: &Path, zapnuty: bool) -> Result<String, String> {
    let stav = || citaj(koren.join(SPANOK).join("stav")).unwrap_or_default();
    match fs::write(koren.join(SPANOK).join("zapnute"), if zapnuty { "1" } else { "0" }) {
        Ok(()) => Ok(stav()),
        Err(e) => Err(format!("{} ({e})", stav())),
    }
}

const CASOVAC: &str = "sys/module/sklenik_casovac/parameters";
const SPOLOCNY: &str = "sys/devices/system/clockevents/broadcast/current_device";

/// R186: jadro má vlastný spoločný časovač budenia (Timer F)?
pub fn casovac_budenia_je(koren: &Path) -> bool {
    koren.join(CASOVAC).join("zapnute").exists()
}

/// R186: zapne spoločný časovač budenia (ovládač ho najprv vyskúša; zapnutý ostane do reštartu).
pub fn zapni_casovac_budenia(koren: &Path) -> Result<String, String> {
    match fs::write(koren.join(CASOVAC).join("zapnute"), "1") {
        Ok(()) => Ok(stav_casovaca_budenia(koren)),
        Err(e) => Err(format!("{} ({e})", stav_casovaca_budenia(koren))),
    }
}

/// R186: stav ovládača a ktorý časovač jadro naozaj používa ako spoločný.
pub fn stav_casovaca_budenia(koren: &Path) -> String {
    let stav = citaj(koren.join(CASOVAC).join("stav"))
        .unwrap_or_else(|| "jadro nemá ovládač sklenik_casovac".into());
    match citaj(koren.join(SPOLOCNY)) {
        Some(c) => format!("{stav} · jadro budí cez {c}"),
        None => stav,
    }
}

/// Štatistika hlbokého stavu za všetky jadrá: vstupy, čas v ňom (µs), odmietnuté firmvérom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spanok {
    pub vstupov: u64,
    pub cas_us: u64,
    pub odmietnutych: u64,
}

impl Spanok {
    pub fn nacitaj(koren: &Path) -> Option<Spanok> {
        let mut s = Spanok::default();
        let mut najdene = false;
        let cpu = koren.join("sys/devices/system/cpu");
        for z in fs::read_dir(&cpu).into_iter().flatten().flatten() {
            let meno = z.file_name().to_string_lossy().to_string();
            if !meno
                .strip_prefix("cpu")
                .is_some_and(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            let stav = z.path().join("cpuidle/state1");
            if citaj(stav.join("name")).as_deref() != Some("jadro-vyp") {
                continue;
            }
            let cislo = |f: &str| citaj(stav.join(f)).and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
            s.vstupov += cislo("usage");
            s.cas_us += cislo("time");
            s.odmietnutych += cislo("rejected");
            najdene = true;
        }
        najdene.then_some(s)
    }

    /// Rozdiel od skoršieho stavu.
    pub fn od(&self, skor: &Spanok) -> Spanok {
        Spanok {
            vstupov: self.vstupov.saturating_sub(skor.vstupov),
            cas_us: self.cas_us.saturating_sub(skor.cas_us),
            odmietnutych: self.odmietnutych.saturating_sub(skor.odmietnutych),
        }
    }

    /// Popis: koľko času spolu boli jadrá vypnuté (súčet za všetky jadrá) za `za_s` sekúnd.
    pub fn popis(&self, za_s: f64) -> String {
        format!(
            "jadrá vypnuté spolu {:.0} s za {:.0} min · vstupov {} · odmietnutých firmvérom {}",
            self.cas_us as f64 / 1e6,
            za_s / 60.0,
            self.vstupov,
            self.odmietnutych
        )
    }
}

#[cfg(all(test, unix))]
mod skusky {
    use super::*;
    #[test]
    fn r240_nevymenna_karta_davka_je_jedno_zlyhanie() {
        let mut z = StrazSd::default();
        // jedno spustenie pri štarte: 4 hlásenia (400, 300, 200, 100 kHz) do 2 s → jeden krok
        assert_eq!(z.zlyhanie_karty("mmc0", true, true, 3.0), Some(KrokSd::Pomalsie("mmc0".into())));
        for t in [3.4, 3.9, 4.6] {
            assert_eq!(z.zlyhanie_karty("mmc0", true, true, t), None);
        }
        // znovu pripojený radič zlyhá o chvíľu (ešte v dávke) – nič; neskoršie zlyhanie → odpojiť
        assert_eq!(z.zlyhanie_karty("mmc0", true, true, 6.0), None);
        assert_eq!(z.zlyhanie_karty("mmc0", true, true, 30.0), Some(KrokSd::Odpoj("mmc0".into())));
        // vymeniteľná karta: po starom 5 hlásení
        let mut v = StrazSd::default();
        for _ in 0..SD_ZLYHANI - 1 {
            assert_eq!(v.zlyhanie_karty("mmc1", true, false, 1.0), None);
        }
        assert_eq!(v.zlyhanie_karty("mmc1", true, false, 1.0), Some(KrokSd::Pomalsie("mmc1".into())));
    }

    #[test]
    fn r240_nevymenna_podla_popisu() {
        let d = std::env::temp_dir().join(format!("sklenik-nevym-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let uzol = d.join("sys/class/mmc_host/mmc0/device/of_node");
        fs::create_dir_all(&uzol).unwrap();
        assert!(!sd_nevymenna(&d, "mmc0"));
        fs::write(uzol.join("non-removable"), b"").unwrap();
        assert!(sd_nevymenna(&d, "mmc0"));
        let _ = fs::remove_dir_all(&d);
    }

    use std::os::unix::fs::symlink;

    struct Strom(PathBuf);

    impl Strom {
        fn novy(meno: &str) -> Strom {
            let p = std::env::temp_dir().join(format!("sklenik-radice-{meno}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Strom(p)
        }
        fn subor(&self, cesta: &str, obsah: &str) -> &Strom {
            let p = self.0.join(cesta);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, obsah).unwrap();
            self
        }
        fn odkaz(&self, cesta: &str, ciel: &str) -> &Strom {
            let p = self.0.join(cesta);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            symlink(self.0.join(ciel), p).unwrap();
            self
        }
    }

    impl Drop for Strom {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Krabička X96: SDIO (bez ovládača), karta SD a eMMC – ako v /sys jadra 6.18.
    fn x96(s: &Strom, emmc_pripojena: bool, wifi_ovladac: bool) {
        let plat = "sys/devices/platform/soc";
        s.subor("sys/bus/platform/drivers/meson-gx-mmc/bind", "")
            .subor(&format!("{plat}/ffe03000.mmc/mmc_host/mmc0/mmc0:0001/type"), "SDIO\n")
            .subor(&format!("{plat}/ffe03000.mmc/mmc_host/mmc0/mmc0:0001/vendor"), "0x024c\n")
            .subor(&format!("{plat}/ffe03000.mmc/mmc_host/mmc0/mmc0:0001/device"), "0xc822\n")
            .subor(&format!("{plat}/ffe03000.mmc/mmc_host/mmc0/mmc0:0001/mmc0:0001:1/class"), "0x07\n")
            .subor(&format!("{plat}/ffe05000.mmc/mmc_host/mmc2/mmc2:aaaa/type"), "SD\n")
            .subor(&format!("{plat}/ffe05000.mmc/mmc_host/mmc2/mmc2:aaaa/block/mmcblk2/size"), "1\n")
            .subor(&format!("{plat}/ffe07000.mmc/mmc_host/mmc1/mmc1:0001/type"), "MMC\n")
            .subor(&format!("{plat}/ffe07000.mmc/mmc_host/mmc1/mmc1:0001/name"), "032GE4\n")
            .subor(&format!("{plat}/ffe07000.mmc/mmc_host/mmc1/mmc1:0001/block/mmcblk1/size"), "1\n")
            .subor(&format!("{plat}/ffe07000.mmc/mmc_host/mmc1/mmc1:0001/block/mmcblk1boot0/size"), "1\n")
            .subor(
                "proc/mounts",
                if emmc_pripojena {
                    "/dev/sda1 /run/karta vfat rw 0 0\n/dev/mmcblk1p20 /mnt ext4 ro 0 0\n"
                } else {
                    "/dev/sda1 /run/karta vfat rw 0 0\n"
                },
            );
        for (radic, karta) in [
            ("ffe03000.mmc", "mmc0/mmc0:0001"),
            ("ffe05000.mmc", "mmc2/mmc2:aaaa"),
            ("ffe07000.mmc", "mmc1/mmc1:0001"),
        ] {
            s.odkaz(&format!("{plat}/{radic}/driver"), "sys/bus/platform/drivers/meson-gx-mmc");
            let meno = karta.split('/').nth(1).unwrap();
            s.odkaz(&format!("sys/bus/mmc/devices/{meno}"), &format!("{plat}/{radic}/mmc_host/{karta}"));
        }
        if wifi_ovladac {
            s.odkaz(
                &format!("{plat}/ffe03000.mmc/mmc_host/mmc0/mmc0:0001/mmc0:0001:1/driver"),
                "sys/bus/platform/drivers/meson-gx-mmc",
            );
        }
    }

    #[test]
    fn odpoji_emmc_a_wifi_bez_ovladaca_nikdy_kartu_sd() {
        let s = Strom::novy("x96");
        x96(&s, false, false);
        let r = nepouzite_radice(&s.0, &["sda1".into()]);
        let mena: Vec<&str> = r.iter().map(|x| x.zariadenie.as_str()).collect();
        assert_eq!(mena, ["ffe03000.mmc", "ffe07000.mmc"], "{r:?}");
        assert_eq!(r[0].popis, "Wi-Fi SDIO 024c:c822 bez ovládača");
        assert_eq!(r[1].riadok(), "eMMC 032GE4 (ffe07000.mmc)");
        odpoj(&r[1]).unwrap();
        assert_eq!(fs::read_to_string(r[1].ovladac.join("unbind")).unwrap(), "ffe07000.mmc");
        pripoj(&r[1]).unwrap();
        assert_eq!(fs::read_to_string(r[1].ovladac.join("bind")).unwrap(), "ffe07000.mmc");
    }

    #[test]
    fn pouzita_emmc_a_wifi_s_ovladacom_ostanu() {
        let s = Strom::novy("pouzite");
        x96(&s, true, true);
        assert!(nepouzite_radice(&s.0, &[]).is_empty());
        let s = Strom::novy("chranena");
        x96(&s, false, false);
        // eMMC je spúšťacia alebo nosič záznamov
        let r = nepouzite_radice(&s.0, &["/dev/mmcblk1p1".into()]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].zariadenie, "ffe03000.mmc");
        assert!(patri("mmcblk1boot0", "mmcblk1") && !patri("mmcblk10p1", "mmcblk1"));
    }

    #[test]
    fn spanok_jadier_zo_sys() {
        let s = Strom::novy("spanok");
        for c in 0..4 {
            s.subor(&format!("sys/devices/system/cpu/cpu{c}/cpuidle/state1/name"), "jadro-vyp\n")
                .subor(&format!("sys/devices/system/cpu/cpu{c}/cpuidle/state1/usage"), "100\n")
                .subor(&format!("sys/devices/system/cpu/cpu{c}/cpuidle/state1/time"), "5000000\n")
                .subor(&format!("sys/devices/system/cpu/cpu{c}/cpuidle/state1/rejected"), "0\n");
        }
        s.subor("sys/devices/system/cpu/cpufreq/policy0/x", "");
        let z = Spanok::nacitaj(&s.0).expect("spánok");
        assert_eq!(z, Spanok { vstupov: 400, cas_us: 20_000_000, odmietnutych: 0 });
        assert_eq!(
            z.od(&Spanok::default()).popis(600.0),
            "jadrá vypnuté spolu 20 s za 10 min · vstupov 400 · odmietnutých firmvérom 0"
        );
        assert!(!spanok_jadier_je(&s.0));
        s.subor("sys/module/sklenik_jadra/parameters/zapnute", "0\n")
            .subor("sys/module/sklenik_jadra/parameters/stav", "zapnutý – PSCI 1.0\n");
        assert!(spanok_jadier_je(&s.0));
        assert_eq!(nastav_spanok_jadier(&s.0, true), Ok("zapnutý – PSCI 1.0".into()));
        // R186
        assert!(!casovac_budenia_je(&s.0));
        assert_eq!(stav_casovaca_budenia(&s.0), "jadro nemá ovládač sklenik_casovac");
        s.subor("sys/module/sklenik_casovac/parameters/zapnute", "0\n")
            .subor("sys/module/sklenik_casovac/parameters/stav", "beží · prerušení 5\n")
            .subor("sys/devices/system/clockevents/broadcast/current_device", "sklenik-casovac-f\n");
        assert!(casovac_budenia_je(&s.0));
        assert_eq!(
            zapni_casovac_budenia(&s.0),
            Ok("beží · prerušení 5 · jadro budí cez sklenik-casovac-f".into())
        );
    }

    #[test]
    fn straz_karty_sd() {
        assert_eq!(zlyhanie_sd("mmc0: error -84 whilst initialising SD card"), Some("mmc0".into()));
        assert_eq!(zlyhanie_sd("mmc0: error -84 reading general info of SD ext reg"), None);
        assert_eq!(zlyhanie_sd("usb 1-1: new device"), None);
        let mut z = StrazSd::default();
        for _ in 0..SD_ZLYHANI - 1 {
            assert_eq!(z.zlyhanie("mmc0", true), None);
        }
        assert_eq!(z.zlyhanie("mmc0", true), Some(KrokSd::Pomalsie("mmc0".into())));
        for _ in 0..SD_ZLYHANI - 1 {
            assert_eq!(z.zlyhanie("mmc0", true), None);
        }
        assert_eq!(z.zlyhanie("mmc0", true), Some(KrokSd::Odpoj("mmc0".into())));
        assert_eq!(z.stupen("mmc0"), 2);
        for _ in 0..2 * SD_ZLYHANI {
            assert_eq!(z.zlyhanie("mmc0", true), None);
        }
        // jadro bez stropu taktu: hneď odpojiť
        let mut b = StrazSd::default();
        for _ in 0..SD_ZLYHANI - 1 {
            b.zlyhanie("mmc1", false);
        }
        assert_eq!(b.zlyhanie("mmc1", false), Some(KrokSd::Odpoj("mmc1".into())));
    }

    #[test]
    fn r235_chyby_citania_karty() {
        assert_eq!(
            chyba_citania_mmc("I/O error, dev mmcblk0, sector 60831744 op 0x0:(READ) flags 0x80700"),
            Some("mmcblk0".into())
        );
        assert_eq!(chyba_citania_mmc("Buffer I/O error on dev mmcblk0, logical block 0"), None);
        assert_eq!(chyba_citania_mmc("I/O error, dev sda, sector 0"), None);
        assert_eq!(chyba_citania_mmc("I/O error, dev mmcblk1boot0, sector 0"), None);
        let mut s = StrazCitania::default();
        assert_eq!(s.chyba("mmc0"), None);
        assert_eq!(s.chyba("mmc0"), None);
        assert_eq!(s.chyba("mmc0"), Some(25_000));
        assert_eq!(s.strop("mmc0"), Some(25_000));
        // ďalší krok najskôr po 30 s – dávka chýb hneď po prepnutí nič nespraví
        for _ in 0..5 {
            assert_eq!(s.chyba("mmc0"), None);
        }
        assert_eq!(s.strop("mmc1"), None);
    }
}
