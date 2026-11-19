//! Súpis periférií: čo je k zariadeniu pripojené – obrazovka, klávesnica, myš, dotyková plocha, sieť,
//! úložisko, zvuk a zariadenia USB.
//!
//! Ovládače sú priamo v jadre SKLENÍKA, takže jadro zariadenia nastaví samo už pri štarte; KLÍČ ich
//! len spíše (z `/sys` a `/proc`), ukáže a zapne sieťové rozhrania. Súpis sa opakuje pri každom
//! prebudení strážcu, preto sa pripojená myš alebo kábel objaví bez reštartu. Koreň súborov sa dá
//! zmeniť – skúšky používajú vymyslený strom `/sys`.

use std::fs;
use std::path::Path;

/// Druh vstupného zariadenia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DruhVstupu {
    Klavesnica,
    Mys,
    /// Dotyková obrazovka (dotyk priamo na obraze).
    Dotyk,
    /// Touchpad, grafický tablet a podobne (ukazovanie mimo obrazu).
    Ukazovatko,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Vstup {
    pub druh: DruhVstupu,
    pub meno: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rozhranie {
    pub meno: String,
    pub adresa: String,
    pub bezdrotove: bool,
    /// Je pripojený kábel (alebo sieť)? `None` – rozhranie ešte nie je zapnuté.
    pub spojenie: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ulozisko {
    pub meno: String,
    pub popis: String,
    pub bajtov: u64,
    pub vymenitelne: bool,
}

/// Všetko, čo KLÍČ našiel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Supis {
    pub obrazovky: Vec<String>,
    pub vstupy: Vec<Vstup>,
    pub siet: Vec<Rozhranie>,
    pub ulozisko: Vec<Ulozisko>,
    pub zvuk: Vec<String>,
    /// Zariadenia USB okrem rozbočovačov (na RPi 1–3 je medzi nimi aj čip Ethernetu).
    pub usb: Vec<String>,
    /// Prídavné dosky a skrinky s identifikačným čipom (HAT), ktoré našiel firmvér Raspberry Pi.
    pub dosky: Vec<String>,
    /// Podpätie (slabý zdroj) podľa firmvéru: `Some(true)` teraz, `Some(false)` v poriadku, `None` bez snímača.
    pub podpatie: Option<bool>,
}

// čísla udalostí a tlačidiel jadra Linux (include/uapi/linux/input-event-codes.h)
const EV_KEY: u32 = 1;
const EV_REL: u32 = 2;
const EV_ABS: u32 = 3;
const KEY_A: usize = 30;
const KEY_SPACE: usize = 57;
const BTN_LEFT: usize = 0x110;
const BTN_TOUCH: usize = 0x14a;
const INPUT_PROP_DIRECT: usize = 1;

fn citaj(cesta: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(cesta).ok().map(|s| s.trim().trim_end_matches('\0').trim().to_string())
}

/// Mená položiek priečinka, zoradené (súpis je potom vždy rovnaký).
fn polozky(cesta: impl AsRef<Path>) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(cesta)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|z| z.file_name().into_string().ok())
        .collect();
    v.sort();
    v
}

/// Je nastavený bit v bitovej mape jadra? Jadro ju píše ako šestnástkové slová typu `long`
/// oddelené medzerou, najvyššie slovo prvé.
pub fn ma_bit(mapa: &str, bit: usize, bitov_slova: usize) -> bool {
    let slova: Vec<&str> = mapa.split_whitespace().collect();
    let i = bit / bitov_slova;
    if i >= slova.len() {
        return false;
    }
    u64::from_str_radix(slova[slova.len() - 1 - i], 16).is_ok_and(|w| (w >> (bit % bitov_slova)) & 1 == 1)
}

/// Druh vstupného zariadenia podľa schopností, ktoré hlási jadro.
pub fn druh_vstupu(ev: &str, kluce: &str, vlastnosti: &str, bitov_slova: usize) -> Option<DruhVstupu> {
    let udalost = |b: u32| ma_bit(ev, b as usize, bitov_slova);
    let kluc = |b: usize| ma_bit(kluce, b, bitov_slova);
    if !udalost(EV_KEY) {
        return None;
    }
    if udalost(EV_ABS) && kluc(BTN_TOUCH) {
        return Some(if ma_bit(vlastnosti, INPUT_PROP_DIRECT, bitov_slova) {
            DruhVstupu::Dotyk
        } else {
            DruhVstupu::Ukazovatko
        });
    }
    if udalost(EV_REL) && kluc(BTN_LEFT) {
        return Some(DruhVstupu::Mys);
    }
    if kluc(KEY_A) && kluc(KEY_SPACE) {
        return Some(DruhVstupu::Klavesnica);
    }
    None
}

fn velkost(bajtov: u64) -> String {
    if bajtov >= 1_000_000_000 {
        format!("{:.1} GB", bajtov as f64 / 1e9)
    } else {
        format!("{} MB", bajtov / 1_000_000)
    }
}

impl Supis {
    /// Súpis zo súborov jadra pod koreňom `koren` (na zariadení `/`).
    pub fn nacitaj(koren: &Path) -> Supis {
        Supis::nacitaj_so_slovom(koren, usize::BITS as usize)
    }

    pub fn nacitaj_so_slovom(koren: &Path, bitov_slova: usize) -> Supis {
        let sys = koren.join("sys");
        let mut s = Supis::default();

        // obrazovka: framebuffer od firmvéru a výstupy grafického ovládača, ak je v jadre
        let grafika = sys.join("class/graphics");
        for fb in polozky(&grafika)
            .into_iter()
            .filter(|n| n.len() > 2 && n.starts_with("fb") && n[2..].bytes().all(|b| b.is_ascii_digit()))
        {
            let rozmer = citaj(grafika.join(&fb).join("virtual_size")).map(|r| r.replace(',', "x"));
            s.obrazovky.push(match rozmer {
                Some(r) => format!("{fb} {r}"),
                None => fb,
            });
        }
        let drm = sys.join("class/drm");
        for vystup in polozky(&drm).into_iter().filter(|n| n.starts_with("card") && n.contains('-')) {
            if citaj(drm.join(&vystup).join("status")).as_deref() == Some("connected") {
                s.obrazovky.push(vystup.split_once('-').map_or(vystup.clone(), |(_, v)| v.to_string()));
            }
        }

        // vstupy: klávesnica, myš, dotyk (jedno zariadenie môže mať viac udalostí – opakovania sa zlúčia)
        let vstupy = sys.join("class/input");
        for udalost in polozky(&vstupy).into_iter().filter(|n| n.starts_with("event")) {
            let z = vstupy.join(&udalost).join("device");
            let ev = citaj(z.join("capabilities/ev")).unwrap_or_default();
            let kluce = citaj(z.join("capabilities/key")).unwrap_or_default();
            let vlastnosti = citaj(z.join("properties")).unwrap_or_default();
            if let Some(druh) = druh_vstupu(&ev, &kluce, &vlastnosti, bitov_slova) {
                let meno = citaj(z.join("name")).unwrap_or_else(|| udalost.clone());
                s.vstupy.push(Vstup { druh, meno });
            }
        }
        s.vstupy.sort();
        s.vstupy.dedup();

        // sieť
        let siet = sys.join("class/net");
        for meno in polozky(&siet).into_iter().filter(|n| n != "lo") {
            let r = siet.join(&meno);
            s.siet.push(Rozhranie {
                adresa: citaj(r.join("address")).unwrap_or_default(),
                bezdrotove: r.join("wireless").exists() || r.join("phy80211").exists(),
                spojenie: citaj(r.join("carrier")).and_then(|c| match c.as_str() {
                    "1" => Some(true),
                    "0" => Some(false),
                    _ => None,
                }),
                meno,
            });
        }

        // úložisko: SD karta, eMMC, USB disky, NVMe (bez pamäťových a pomocných zariadení)
        let bloky = sys.join("block");
        for meno in polozky(&bloky) {
            if ["ram", "loop", "zram"].iter().any(|p| meno.starts_with(p))
                || meno.contains("boot")
                || meno.contains("rpmb")
            {
                continue;
            }
            let b = bloky.join(&meno);
            let bajtov = citaj(b.join("size")).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 512;
            if bajtov == 0 {
                continue;
            }
            let popis = ["device/model", "device/name"]
                .iter()
                .find_map(|f| citaj(b.join(f)).filter(|t| !t.is_empty()))
                .unwrap_or_default();
            s.ulozisko.push(Ulozisko {
                vymenitelne: citaj(b.join("removable")).as_deref() == Some("1"),
                meno,
                popis,
                bajtov,
            });
        }

        // zvuk: zoznam zvukových kariet jadra
        if let Some(karty) = citaj(koren.join("proc/asound/cards")) {
            for riadok in karty.lines() {
                if riadok.trim_start().starts_with(|c: char| c.is_ascii_digit())
                    && let Some((_, popis)) = riadok.split_once(" - ")
                {
                    s.zvuk.push(popis.trim().to_string());
                }
            }
        }

        // prídavné dosky (HAT, HAT+): firmvér ich opis z identifikačného čipu vloží do stromu zariadení
        let strom = koren.join("proc/device-tree");
        for uzol in polozky(&strom).into_iter().filter(|n| n.starts_with("hat")) {
            let u = strom.join(&uzol);
            let casti: Vec<String> = ["vendor", "product"]
                .iter()
                .filter_map(|f| citaj(u.join(f)))
                .filter(|t| !t.is_empty())
                .collect();
            let id = citaj(u.join("product_id")).filter(|t| !t.is_empty());
            if casti.is_empty() && id.is_none() {
                continue;
            }
            let mut opis = casti.join(" ");
            if let Some(id) = id {
                opis = format!("{opis} (výrobok {id})").trim().to_string();
            }
            s.dosky.push(opis);
        }

        // napájanie: snímač podpätia firmvéru Raspberry Pi (hwmon „rpi_volt“)
        let hwmon = sys.join("class/hwmon");
        for h in polozky(&hwmon) {
            if citaj(hwmon.join(&h).join("name")).as_deref() == Some("rpi_volt") {
                s.podpatie = citaj(hwmon.join(&h).join("in0_lcrit_alarm")).map(|v| v == "1");
            }
        }

        // USB: skutočné zariadenia (nie koreňové rozbočovače, rozhrania ani rozbočovače)
        let usb = sys.join("bus/usb/devices");
        for meno in polozky(&usb) {
            let d = usb.join(&meno);
            if meno.starts_with("usb") || meno.contains(':') || !d.join("idVendor").exists() {
                continue;
            }
            if citaj(d.join("bDeviceClass")).as_deref() == Some("09") {
                continue;
            }
            let vyrobca = citaj(d.join("idVendor")).unwrap_or_default();
            let vyrobok = citaj(d.join("idProduct")).unwrap_or_default();
            s.usb.push(citaj(d.join("product")).unwrap_or_else(|| format!("{vyrobca}:{vyrobok}")));
        }
        s
    }

    fn pocet(&self, druh: DruhVstupu) -> usize {
        self.vstupy.iter().filter(|v| v.druh == druh).count()
    }

    /// Jeden riadok do denníka (so slovenskou diakritikou).
    pub fn riadok(&self) -> String {
        let obrazovka = if self.obrazovky.is_empty() { "žiadna".into() } else { self.obrazovky.join(", ") };
        let vstupy = if self.vstupy.is_empty() {
            "žiadne".into()
        } else {
            self.vstupy
                .iter()
                .map(|v| {
                    let d = match v.druh {
                        DruhVstupu::Klavesnica => "klávesnica",
                        DruhVstupu::Mys => "myš",
                        DruhVstupu::Dotyk => "dotyková obrazovka",
                        DruhVstupu::Ukazovatko => "ukazovacie zariadenie",
                    };
                    format!("{d} ({})", v.meno)
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let siet = if self.siet.is_empty() {
            "žiadna".into()
        } else {
            self.siet
                .iter()
                .map(|r| {
                    let druh = if r.bezdrotove { "Wi-Fi" } else { "kábel" };
                    let stav = match (r.spojenie, r.bezdrotove) {
                        (Some(true), _) => "spojené",
                        (Some(false), false) => "kábel nepripojený",
                        (Some(false), true) => "bez siete",
                        (None, _) => "vypnuté",
                    };
                    format!("{} {druh} {} – {stav}", r.meno, r.adresa)
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let ulozisko = if self.ulozisko.is_empty() {
            "žiadne".into()
        } else {
            self.ulozisko
                .iter()
                .map(|u| format!("{} {} {}", u.meno, u.popis, velkost(u.bajtov)).replace("  ", " "))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let zvuk = if self.zvuk.is_empty() { "–".into() } else { self.zvuk.join(", ") };
        let dosky = if self.dosky.is_empty() { "žiadna".into() } else { self.dosky.join(", ") };
        let napajanie = match self.podpatie {
            Some(true) => "PODPÄTIE – slabý zdroj",
            Some(false) => "v poriadku",
            None => "–",
        };
        format!(
            "periférie – obrazovka: {obrazovka}; vstupy: {vstupy}; sieť: {siet}; úložisko: {ulozisko}; zvuk: {zvuk}; USB: {}; prídavná doska: {dosky}; napájanie: {napajanie}",
            self.usb.len()
        )
    }

    /// Riadky pre obrazovku (bez diakritiky – písmo konzoly jadra ju nemusí mať).
    pub fn obrazovka(&self) -> Vec<String> {
        let mut r = Vec::new();
        r.push(format!(
            "obrazovka: {}",
            if self.obrazovky.is_empty() { "-".into() } else { self.obrazovky.join(", ") }
        ));
        r.push(format!(
            "klavesnica: {}  mys: {}  dotyk: {}",
            self.pocet(DruhVstupu::Klavesnica),
            self.pocet(DruhVstupu::Mys),
            self.pocet(DruhVstupu::Dotyk)
        ));
        for i in &self.siet {
            let stav = match (i.spojenie, i.bezdrotove) {
                (Some(true), _) => "spojene",
                (Some(false), false) => "kabel nepripojeny",
                (Some(false), true) => "bez siete",
                (None, _) => "vypnute",
            };
            r.push(format!("siet {}: {}", i.meno, stav));
        }
        if self.siet.is_empty() {
            r.push("siet: -".into());
        }
        for u in &self.ulozisko {
            r.push(format!("ulozisko {}: {}", u.meno, velkost(u.bajtov)));
        }
        r.push(format!("usb zariadeni: {}", self.usb.len()));
        r
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use std::path::PathBuf;

    struct Strom(PathBuf);

    impl Strom {
        fn novy(meno: &str) -> Strom {
            let p = std::env::temp_dir().join(format!("klic-periferie-{meno}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Strom(p)
        }
        fn subor(&self, cesta: &str, obsah: &str) -> &Strom {
            let p = self.0.join(cesta);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, format!("{obsah}\n")).unwrap();
            self
        }
    }

    impl Drop for Strom {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn bitova_mapa_v_slovach_64_aj_32_bitov() {
        // BTN_LEFT = 0x110 = bit 272: 64-bitové slová → 5. slovo odzadu, bit 16
        let mapa64 = "70000 0 0 0 0";
        assert!(ma_bit(mapa64, 272, 64));
        assert!(!ma_bit(mapa64, 271, 64));
        // to isté na 32-bitovom jadre: 9. slovo odzadu
        let mapa32 = "70000 0 0 0 0 0 0 0 0";
        assert!(ma_bit(mapa32, 272, 32));
        assert!(!ma_bit("", 1, 64));
        assert!(!ma_bit("zle", 1, 64));
    }

    #[test]
    fn druhy_vstupov() {
        // klávesnica: EV_SYN, EV_KEY, EV_MSC, EV_LED, EV_REP; klávesy A a medzera
        let kluce = format!("{:x}", (1u64 << KEY_A) | (1u64 << KEY_SPACE));
        assert_eq!(druh_vstupu("120013", &kluce, "0", 64), Some(DruhVstupu::Klavesnica));
        // myš: EV_SYN, EV_KEY, EV_REL, EV_MSC a ľavé tlačidlo
        assert_eq!(druh_vstupu("17", "70000 0 0 0 0", "0", 64), Some(DruhVstupu::Mys));
        // „Consumer Control“ klávesnice má EV_REL, ale nie ľavé tlačidlo – nie je to myš
        assert_eq!(druh_vstupu("1f", "1 0 0 0", "0", 64), None);
        // dotyková obrazovka: EV_ABS, BTN_TOUCH (bit 330 = slovo 5, bit 10), vlastnosť DIRECT
        let dotyk = "400 0 0 0 0 0";
        assert_eq!(druh_vstupu("b", dotyk, "2", 64), Some(DruhVstupu::Dotyk));
        assert_eq!(druh_vstupu("b", dotyk, "0", 64), Some(DruhVstupu::Ukazovatko));
        // tlačidlo napájania a podobne: nič
        assert_eq!(druh_vstupu("3", "10000000000000 0", "0", 64), None);
        assert_eq!(druh_vstupu("", "", "", 64), None);
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore = "WASI nemá dočasný priečinok")]
    fn supis_raspberry_pi_s_klavesnicou_mysou_a_sietou() {
        let s = Strom::novy("rpi");
        let kluce = format!("{:x}", (1u64 << KEY_A) | (1u64 << KEY_SPACE));
        s.subor("sys/class/graphics/fb0/virtual_size", "1920,1080")
            .subor("sys/class/graphics/fbcon/rotate", "0")
            .subor("sys/class/input/event0/device/name", "Klavesnica USB")
            .subor("sys/class/input/event0/device/capabilities/ev", "120013")
            .subor("sys/class/input/event0/device/capabilities/key", &format!("0 0 0 {kluce}"))
            .subor("sys/class/input/event0/device/properties", "0")
            .subor("sys/class/input/event1/device/name", "Klavesnica USB Consumer Control")
            .subor("sys/class/input/event1/device/capabilities/ev", "1f")
            .subor("sys/class/input/event1/device/capabilities/key", "1 0 0 0")
            .subor("sys/class/input/event2/device/name", "Mys USB")
            .subor("sys/class/input/event2/device/capabilities/ev", "17")
            .subor("sys/class/input/event2/device/capabilities/key", "70000 0 0 0 0")
            .subor("sys/class/net/lo/address", "00:00:00:00:00:00")
            .subor("sys/class/net/eth0/address", "b8:27:eb:00:00:01")
            .subor("sys/class/net/eth0/carrier", "1")
            .subor("sys/class/net/wlan0/address", "b8:27:eb:00:00:02")
            .subor("sys/class/net/wlan0/wireless/.x", "")
            .subor("sys/block/mmcblk0/size", "62333952")
            .subor("sys/block/mmcblk0/removable", "0")
            .subor("sys/block/mmcblk0/device/name", "SD32G")
            .subor("sys/block/mmcblk0boot0/size", "8192")
            .subor("sys/block/ram0/size", "8192")
            .subor("sys/block/sda/size", "0")
            .subor("proc/asound/cards", " 0 [Headphones     ]: bcm2835_headpho - bcm2835 Headphones")
            .subor("sys/bus/usb/devices/usb1/idVendor", "1d6b")
            .subor("sys/bus/usb/devices/1-1/idVendor", "0424")
            .subor("sys/bus/usb/devices/1-1/bDeviceClass", "09")
            .subor("sys/bus/usb/devices/1-1.1/idVendor", "0424")
            .subor("sys/bus/usb/devices/1-1.1/idProduct", "ec00")
            .subor("sys/bus/usb/devices/1-1.1/bDeviceClass", "ff")
            .subor("sys/bus/usb/devices/1-1.2/idVendor", "046d")
            .subor("sys/bus/usb/devices/1-1.2/product", "USB Receiver")
            .subor("sys/bus/usb/devices/1-1.2/bDeviceClass", "00")
            .subor("proc/device-tree/hat/vendor", "Vyrobca s.r.o.\0")
            .subor("proc/device-tree/hat/product", "Skrinka 5\0")
            .subor("proc/device-tree/hat/product_id", "0x0316\0")
            .subor("sys/class/hwmon/hwmon0/name", "cpu_thermal")
            .subor("sys/class/hwmon/hwmon1/name", "rpi_volt")
            .subor("sys/class/hwmon/hwmon1/in0_lcrit_alarm", "1");
        // rozhranie zariadenia USB (dvojbodka v mene – na Windows taký súbor nemôže byť)
        if cfg!(unix) {
            s.subor("sys/bus/usb/devices/1-1.1:1.0/idVendor", "0424");
        }
        let p = Supis::nacitaj_so_slovom(&s.0, 64);
        assert_eq!(p.obrazovky, vec!["fb0 1920x1080"]);
        assert_eq!(
            p.vstupy,
            vec![
                Vstup { druh: DruhVstupu::Klavesnica, meno: "Klavesnica USB".into() },
                Vstup { druh: DruhVstupu::Mys, meno: "Mys USB".into() },
            ]
        );
        assert_eq!(p.siet.len(), 2);
        assert_eq!(p.siet[0].meno, "eth0");
        assert_eq!(p.siet[0].spojenie, Some(true));
        assert!(!p.siet[0].bezdrotove);
        assert!(p.siet[1].bezdrotove);
        assert_eq!(p.siet[1].spojenie, None);
        assert_eq!(p.ulozisko.len(), 1);
        assert_eq!(p.ulozisko[0].popis, "SD32G");
        assert_eq!(p.ulozisko[0].bajtov, 62333952 * 512);
        assert_eq!(p.zvuk, vec!["bcm2835 Headphones"]);
        assert_eq!(p.usb, vec!["0424:ec00", "USB Receiver"]);
        assert_eq!(p.dosky, vec!["Vyrobca s.r.o. Skrinka 5 (výrobok 0x0316)"]);
        assert_eq!(p.podpatie, Some(true));
        let r = p.riadok();
        assert!(r.contains("klávesnica (Klavesnica USB)"), "{r}");
        assert!(r.contains("eth0 kábel b8:27:eb:00:00:01 – spojené"), "{r}");
        assert!(r.contains("mmcblk0 SD32G 31.9 GB"), "{r}");
        let o = p.obrazovka();
        assert!(o.iter().all(|r| r.is_ascii()), "{o:?}");
        assert!(o.contains(&"klavesnica: 1  mys: 1  dotyk: 0".to_string()), "{o:?}");
        assert!(o.contains(&"siet eth0: spojene".to_string()), "{o:?}");
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore = "WASI nemá dočasný priečinok")]
    fn prazdne_zariadenie() {
        let s = Strom::novy("prazdne");
        let p = Supis::nacitaj(&s.0);
        assert_eq!(p, Supis::default());
        assert!(p.riadok().contains("obrazovka: žiadna"));
        assert!(p.obrazovka().iter().all(|r| r.is_ascii()));
    }
}
