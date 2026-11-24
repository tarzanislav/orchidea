//! Vlastný ovládač CEC (R181) – televízor a krabička sa dohovoria cez HDMI (rozhranie jadra `/dev/cec0`,
//! náš kód bez knižníc).
//!
//! - **Krabička zobudí obraz → televízor sa zapne a prepne na jej vstup** (Image View On + Active Source).
//! - **Krabička vypne obraz (nečinnosť) → televízor ide spať** (Standby) – len ak práve ukazuje krabičku;
//!   keď sa pozerá iný vstup, nechá ho tak.
//! - **Televízor sa vypne → krabička hneď vypne obraz** (stojí kódovač aj vysielač HDMI – chladnejšie).
//! - **Ovládač televízora** (tlačidlá cez CEC) zobudí krabičku ako jej vlastný ovládač.
//! - Odpovedá na otázky televízora (stav napájania, aktívny zdroj, ponuka); meno „SKLENIK“, typ prehrávač.
//!
//! Nastavenie `cec=` v SKLENIK.TXT: `ano` (predvolené – zapína aj uspáva televízor), `zapni` (len zapína),
//! `nie` (CEC nepoužíva). Správy sa tvoria a čítajú čistými funkciami – skúšky bežia aj bez televízora.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

use crate::sys;

// operačné kódy CEC 1.4 (HDMI 1.4b, dodatok CEC)
const IMAGE_VIEW_ON: u8 = 0x04;
const STANDBY: u8 = 0x36;
const USER_CONTROL_PRESSED: u8 = 0x44;
const ROUTING_CHANGE: u8 = 0x80;
const ACTIVE_SOURCE: u8 = 0x82;
const REQUEST_ACTIVE_SOURCE: u8 = 0x85;
const SET_STREAM_PATH: u8 = 0x86;
const MENU_REQUEST: u8 = 0x8D;
const MENU_STATUS: u8 = 0x8E;
const GIVE_DEVICE_POWER_STATUS: u8 = 0x8F;
const REPORT_POWER_STATUS: u8 = 0x90;
const INACTIVE_SOURCE: u8 = 0x9D;

const TV: u8 = 0;
const VSETKYM: u8 = 0xF;

/// Čo KLÍČ s televízorom robí (SKLENIK.TXT `cec=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rezim {
    /// Zapína aj uspáva televízor (predvolené).
    Vsetko,
    /// Len zapína televízor; uspávať ho nebude.
    LenZapni,
    /// CEC sa nepoužíva.
    Nic,
}

impl Rezim {
    pub fn z_nastavenia(v: Option<&str>) -> Rezim {
        match v {
            Some("nie") => Rezim::Nic,
            Some("zapni") => Rezim::LenZapni,
            _ => Rezim::Vsetko,
        }
    }
}

/// Čo televízor (alebo iné zariadenie na HDMI) oznámil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Udalost {
    /// Tlačidlo ovládača televízora (kód CEC UI command).
    Tlacidlo(u8),
    /// Televízor ide spať.
    TvSpi,
    /// Televízor chce ukazovať krabičku (zvolil jej vstup).
    ChceMa,
    /// Aktívny je iný zdroj (pozerá sa iný vstup).
    InyZdroj,
}

fn adresa(pa: u16) -> [u8; 2] {
    pa.to_be_bytes()
}

fn pa_z(b: &[u8]) -> Option<u16> {
    (b.len() >= 2).then(|| u16::from_be_bytes([b[0], b[1]]))
}

/// R203: „zapni sa“ televízoru **bez vlastnej adresy** (od neregistrovaného zariadenia 15). Televízor v
/// pohotovosti často stiahne HPD – jadro potom krabičke zruší adresu a bežnú správu poslať nedovolí; túto
/// jedinú (Image View On od 15 pre televízor) dovolí práve na zobudenie takého televízora.
pub fn zapni_tv_bez_adresy() -> Vec<u8> {
    vec![0xF << 4 | TV, IMAGE_VIEW_ON]
}

/// Správa „zapni sa“ televízoru.
pub fn zapni_tv(ja: u8) -> Vec<u8> {
    vec![ja << 4 | TV, IMAGE_VIEW_ON]
}

/// Správa všetkým „ukazuje sa môj vstup“.
pub fn aktivny_zdroj(ja: u8, pa: u16) -> Vec<u8> {
    let a = adresa(pa);
    vec![ja << 4 | VSETKYM, ACTIVE_SOURCE, a[0], a[1]]
}

/// Správa televízoru „choď spať“.
pub fn uspi_tv(ja: u8) -> Vec<u8> {
    vec![ja << 4 | TV, STANDBY]
}

/// Správa televízoru „môj vstup už nič neukazuje“ (keď ho krabička neuspáva).
pub fn neaktivny_zdroj(ja: u8, pa: u16) -> Vec<u8> {
    let a = adresa(pa);
    vec![ja << 4 | TV, INACTIVE_SOURCE, a[0], a[1]]
}

/// Spracuje prijatú správu: vráti udalosť a prípadnú odpoveď. `obraz` = krabička má zapnutý obraz,
/// `aktivny` = televízor práve ukazuje krabičku.
pub fn spracuj(
    sprava: &[u8],
    ja: u8,
    pa: u16,
    obraz: bool,
    aktivny: bool,
) -> (Option<Udalost>, Option<Vec<u8>>) {
    if sprava.len() < 2 {
        return (None, None);
    }
    let od = sprava[0] >> 4;
    let komu = sprava[0] & 0xF;
    if od == ja || (komu != ja && komu != VSETKYM) {
        return (None, None);
    }
    let data = &sprava[2..];
    match sprava[1] {
        USER_CONTROL_PRESSED if !data.is_empty() => (Some(Udalost::Tlacidlo(data[0])), None),
        STANDBY if od == TV => (Some(Udalost::TvSpi), None),
        ACTIVE_SOURCE => match pa_z(data) {
            Some(p) if p != pa => (Some(Udalost::InyZdroj), None),
            _ => (None, None),
        },
        ROUTING_CHANGE => match pa_z(data.get(2..).unwrap_or(&[])) {
            Some(p) if p == pa => (Some(Udalost::ChceMa), Some(aktivny_zdroj(ja, pa))),
            Some(_) => (Some(Udalost::InyZdroj), None),
            None => (None, None),
        },
        SET_STREAM_PATH => match pa_z(data) {
            Some(p) if p == pa => (Some(Udalost::ChceMa), Some(aktivny_zdroj(ja, pa))),
            Some(_) => (Some(Udalost::InyZdroj), None),
            None => (None, None),
        },
        REQUEST_ACTIVE_SOURCE if aktivny && obraz => (None, Some(aktivny_zdroj(ja, pa))),
        GIVE_DEVICE_POWER_STATUS if komu == ja => {
            // 0 = zapnutý, 1 = pohotovosť (obraz vypnutý)
            (None, Some(vec![ja << 4 | od, REPORT_POWER_STATUS, if obraz { 0 } else { 1 }]))
        }
        MENU_REQUEST if komu == ja => (None, Some(vec![ja << 4 | od, MENU_STATUS, 0])),
        _ => (None, None),
    }
}

// ------------------------------------------------------------------- rozhranie jadra (linux/cec.h)

#[repr(C)]
#[derive(Clone, Copy)]
struct Sprava {
    tx_ts: u64,
    rx_ts: u64,
    len: u32,
    timeout: u32,
    sequence: u32,
    flags: u32,
    msg: [u8; 16],
    reply: u8,
    rx_status: u8,
    tx_status: u8,
    tx_arb_lost_cnt: u8,
    tx_nack_cnt: u8,
    tx_low_drive_cnt: u8,
    tx_error_cnt: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LogickeAdresy {
    log_addr: [u8; 4],
    log_addr_mask: u16,
    cec_version: u8,
    num_log_addrs: u8,
    vendor_id: u32,
    flags: u32,
    osd_name: [u8; 15],
    primary_device_type: [u8; 4],
    log_addr_type: [u8; 4],
    all_device_types: [u8; 4],
    features: [[u8; 12]; 4],
}

const _: () = assert!(size_of::<Sprava>() == 56);
const _: () = assert!(size_of::<LogickeAdresy>() == 92);

const fn io(smer: u32, nr: u32, velkost: usize) -> std::ffi::c_ulong {
    ((smer << 30) | ((velkost as u32) << 16) | ((b'a' as u32) << 8) | nr) as std::ffi::c_ulong
}
const CITAJ: u32 = 2;
const PIS: u32 = 1;
const CITAJ_PIS: u32 = 3;
const G_PHYS_ADDR: std::ffi::c_ulong = io(CITAJ, 1, 2);
const G_LOG_ADDRS: std::ffi::c_ulong = io(CITAJ, 3, size_of::<LogickeAdresy>());
const S_LOG_ADDRS: std::ffi::c_ulong = io(CITAJ_PIS, 4, size_of::<LogickeAdresy>());
const TRANSMIT: std::ffi::c_ulong = io(CITAJ_PIS, 5, size_of::<Sprava>());
const RECEIVE: std::ffi::c_ulong = io(CITAJ_PIS, 6, size_of::<Sprava>());
const S_MODE: std::ffi::c_ulong = io(PIS, 9, 4);

const MODE_INITIATOR: u32 = 1;
const MODE_FOLLOWER: u32 = 1 << 4;
const LOG_ADDR_INVALID: u8 = 0xff;
/// Neregistrované zariadenie (žiadna voľná adresa prehrávača).
const LOG_ADDR_UNREGISTERED: u8 = 15;
const PHYS_ADDR_INVALID: u16 = 0xffff;
const RX_STATUS_OK: u8 = 1;
const TX_STATUS_OK: u8 = 1;

/// Adaptér CEC jadra.
pub struct Cec {
    subor: fs::File,
    pub rezim: Rezim,
    /// Televízor práve ukazuje krabičku.
    pub aktivny: bool,
    /// Obraz sa zapol skôr, než jadro získalo adresu – televízor sa zapne hneď, ako ju bude mať.
    zapnutie_caka: bool,
    /// R203: zobúdzanie televízora bez adresy – kedy naposledy a koľkokrát pri tomto prebudení.
    bez_adresy_naposledy: Option<std::time::Instant>,
    bez_adresy_pokusov: u8,
    pub zobudeni_bez_adresy: u32,
    /// Počty pre záznam.
    pub zapnuti_tv: u32,
    pub uspati_tv: u32,
    pub tlacidiel: u32,
    pub tv_spal: u32,
    pub odoslanych_ok: u32,
    pub odoslanych_chyba: u32,
    /// R220: posledné nové prihlásenie a ich počet.
    preprihlasenie: Option<std::time::Instant>,
    pub preprihlaseni: u32,
}

impl Cec {
    /// Otvorí `/dev/cec0`, prihlási krabičku ako prehrávač „SKLENIK“ (adresu si jadro vybojuje samo, keď je
    /// pripojený televízor) a zapne príjem správ.
    pub fn otvor(rezim: Rezim) -> io::Result<Cec> {
        if rezim == Rezim::Nic {
            return Err(io::Error::other("vypnuté v SKLENIK.TXT (cec=nie)"));
        }
        let subor = fs::OpenOptions::new().read(true).write(true).custom_flags(0o4000).open("/dev/cec0")?;
        let fd = subor.as_raw_fd();
        let mut mode = MODE_INITIATOR | MODE_FOLLOWER;
        sys::ioctl_raz(fd, S_MODE, &mut mode)?;
        prihlas(fd)?;
        Ok(Cec {
            subor,
            rezim,
            aktivny: false,
            zapnutie_caka: false,
            bez_adresy_naposledy: None,
            bez_adresy_pokusov: 0,
            zobudeni_bez_adresy: 0,
            zapnuti_tv: 0,
            uspati_tv: 0,
            tlacidiel: 0,
            tv_spal: 0,
            odoslanych_ok: 0,
            odoslanych_chyba: 0,
            preprihlasenie: None,
            preprihlaseni: 0,
        })
    }

    /// R220: krabička ostala neregistrovaná (logická adresa 15 – Odroid C4 24. 11. po uspatí televízora: pri
    /// boji o adresu bola zbernica v zvláštnom stave a jadro to už neskúsi znova). Kým je televízor pripojený,
    /// odhlási sa a prihlási znova – najviac raz za minútu.
    fn oprav_adresu(&mut self, adresy: Option<(u8, u16)>) {
        if adresy.map(|(la, _)| la) != Some(LOG_ADDR_UNREGISTERED)
            || self.preprihlasenie.is_some_and(|t| t.elapsed().as_secs() < 60)
        {
            return;
        }
        self.preprihlasenie = Some(std::time::Instant::now());
        let fd = self.subor.as_raw_fd();
        let mut prazdne = prazdne_adresy();
        if sys::ioctl_raz(fd, S_LOG_ADDRS, &mut prazdne).is_ok() && prihlas(fd).is_ok() {
            self.preprihlaseni += 1;
        }
    }
}

/// Prázdne logické adresy (odhlásenie).
fn prazdne_adresy() -> LogickeAdresy {
    LogickeAdresy {
        log_addr: [LOG_ADDR_INVALID; 4],
        log_addr_mask: 0,
        cec_version: 0,
        num_log_addrs: 0,
        vendor_id: 0,
        flags: 0,
        osd_name: [0; 15],
        primary_device_type: [0; 4],
        log_addr_type: [0; 4],
        all_device_types: [0; 4],
        features: [[0; 12]; 4],
    }
}

/// Prihlási krabičku ako prehrávač „SKLENIK“ – adresu si jadro vybojuje samo, keď je pripojený televízor.
fn prihlas(fd: i32) -> io::Result<()> {
    {
        let mut meno = [0u8; 15];
        meno[..7].copy_from_slice(b"SKLENIK");
        let mut la = LogickeAdresy {
            log_addr: [LOG_ADDR_INVALID; 4],
            log_addr_mask: 0,
            cec_version: 5, // CEC 1.4 – rozumejú mu všetky televízory
            num_log_addrs: 1,
            vendor_id: 0xffff_ffff,
            flags: 1, // ak nie je voľná adresa prehrávača, ostane neregistrovaný
            osd_name: meno,
            primary_device_type: [4, 0, 0, 0], // prehrávač
            log_addr_type: [3, 0, 0, 0],       // prehrávač
            all_device_types: [0x10, 0, 0, 0],
            features: [[0; 12]; 4],
        };
        // EBUSY = adresy už nastavené (napr. po reštarte KLÍČA) – v poriadku
        match sys::ioctl_raz(fd, S_LOG_ADDRS, &mut la) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(16) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl Cec {
    pub fn fd(&self) -> i32 {
        self.subor.as_raw_fd()
    }

    /// Logická adresa (ak ju už jadro získalo) a fyzická adresa (z EDID televízora).
    pub fn adresy(&self) -> Option<(u8, u16)> {
        let fd = self.subor.as_raw_fd();
        let mut pa: u16 = PHYS_ADDR_INVALID;
        sys::ioctl_raz(fd, G_PHYS_ADDR, &mut pa).ok()?;
        let mut la = LogickeAdresy {
            log_addr: [LOG_ADDR_INVALID; 4],
            log_addr_mask: 0,
            cec_version: 0,
            num_log_addrs: 0,
            vendor_id: 0,
            flags: 0,
            osd_name: [0; 15],
            primary_device_type: [0; 4],
            log_addr_type: [0; 4],
            all_device_types: [0; 4],
            features: [[0; 12]; 4],
        };
        sys::ioctl_raz(fd, G_LOG_ADDRS, &mut la).ok()?;
        (pa != PHYS_ADDR_INVALID && la.num_log_addrs > 0 && la.log_addr[0] != LOG_ADDR_INVALID)
            .then_some((la.log_addr[0], pa))
    }

    fn posli(&mut self, sprava: &[u8]) -> bool {
        let mut s = prazdna();
        s.len = sprava.len().min(16) as u32;
        s.msg[..s.len as usize].copy_from_slice(&sprava[..s.len as usize]);
        // neblokujúce zariadenie: odoslanie sa zaradí, výsledok príde ako správa s tx_status
        let ok = sys::ioctl_raz(self.subor.as_raw_fd(), TRANSMIT, &mut s).is_ok();
        if !ok {
            self.odoslanych_chyba += 1;
        }
        ok
    }

    /// Krabička zobudila obraz: zapni televízor a prepni ho na krabičku.
    pub fn obraz_zapnuty(&mut self) {
        let Some((ja, pa)) = self.adresy() else {
            // R203: televízor spí a nemá HPD – zobudiť ho bez adresy, Active Source pošle, keď adresa príde
            if !self.zapnutie_caka {
                self.bez_adresy_pokusov = 0;
            }
            self.zapnutie_caka = true;
            self.zobud_bez_adresy();
            return;
        };
        self.zapnutie_caka = false;
        if self.posli(&zapni_tv(ja)) && self.posli(&aktivny_zdroj(ja, pa)) {
            self.zapnuti_tv += 1;
            self.aktivny = true;
        }
    }

    /// R203: Image View On od neregistrovaného zariadenia (najviac 5× za prebudenie, každé 3 s).
    fn zobud_bez_adresy(&mut self) {
        if self.bez_adresy_pokusov >= 5
            || self.bez_adresy_naposledy.is_some_and(|t| t.elapsed().as_secs() < 3)
        {
            return;
        }
        self.bez_adresy_pokusov += 1;
        self.bez_adresy_naposledy = Some(std::time::Instant::now());
        if self.posli(&zapni_tv_bez_adresy()) {
            self.zobudeni_bez_adresy += 1;
        }
    }

    /// R203: obraz je zapnutý, ale televízor sa ešte neozval (krabička nemá adresu) – treba pomôcť linke.
    pub fn caka_na_televizor(&self) -> bool {
        self.zapnutie_caka
    }

    /// Krabička vypla obraz: uspi televízor, ak ukazuje krabičku (inak len oznám, že vstup nič neukazuje).
    pub fn obraz_vypnuty(&mut self) {
        self.zapnutie_caka = false;
        let Some((ja, pa)) = self.adresy() else { return };
        if !self.aktivny {
            return;
        }
        if self.rezim == Rezim::Vsetko {
            if self.posli(&uspi_tv(ja)) {
                self.uspati_tv += 1;
            }
        } else {
            self.posli(&neaktivny_zdroj(ja, pa));
        }
        self.aktivny = false;
    }

    /// Uspi televízor hneď (koniec dlhého testu v noci) – aj keď práve neukazuje krabičku.
    pub fn uspi_tv_hned(&mut self) {
        if let Some((ja, _)) = self.adresy()
            && self.posli(&uspi_tv(ja))
        {
            self.uspati_tv += 1;
            self.aktivny = false;
        }
    }

    /// Prečíta všetky čakajúce správy, odpovie na otázky a vráti udalosti pre KLÍČ.
    pub fn prijmi(&mut self, obraz: bool) -> Vec<Udalost> {
        let mut udalosti = Vec::new();
        self.oprav_adresu(self.adresy());
        let adresy = self.adresy();
        if self.zapnutie_caka && obraz {
            if adresy.is_some() {
                self.obraz_zapnuty();
            } else {
                self.zobud_bez_adresy();
            }
        }
        for _ in 0..32 {
            let mut s = prazdna();
            if sys::ioctl_raz(self.subor.as_raw_fd(), RECEIVE, &mut s).is_err() {
                break;
            }
            // výsledok nášho odoslania
            if s.tx_status != 0 {
                if s.tx_status & TX_STATUS_OK != 0 {
                    self.odoslanych_ok += 1;
                } else {
                    self.odoslanych_chyba += 1;
                }
                continue;
            }
            if s.rx_status & RX_STATUS_OK == 0 {
                continue;
            }
            let Some((ja, pa)) = adresy else { continue };
            let n = (s.len as usize).min(16);
            let (u, odpoved) = spracuj(&s.msg[..n], ja, pa, obraz, self.aktivny);
            match u {
                Some(Udalost::Tlacidlo(_)) => self.tlacidiel += 1,
                Some(Udalost::TvSpi) => {
                    self.tv_spal += 1;
                    self.aktivny = false;
                }
                Some(Udalost::ChceMa) => self.aktivny = true,
                Some(Udalost::InyZdroj) => self.aktivny = false,
                None => {}
            }
            if let Some(u) = u {
                udalosti.push(u);
            }
            if let Some(o) = odpoved {
                self.posli(&o);
            }
        }
        udalosti
    }

    /// Riadok do záznamu.
    pub fn popis(&self) -> String {
        let adr = match self.adresy() {
            Some((la, pa)) => format!(
                "adresa {}.{}.{}.{} · logická {la}",
                pa >> 12,
                (pa >> 8) & 0xF,
                (pa >> 4) & 0xF,
                pa & 0xF
            ),
            None => "televízor nepripojený alebo bez CEC".into(),
        };
        let adr = if self.preprihlaseni > 0 {
            format!("{adr} · nové prihlásenia po strate adresy (R220) {}×", self.preprihlaseni)
        } else {
            adr
        };
        format!(
            "{adr} · {} · televízor zapnutý {}×, uspatý {}×, sám zaspal {}×{} · tlačidlá ovládača TV {} · správy ok {}, chyby {}",
            match self.rezim {
                Rezim::Vsetko => "zapína aj uspáva TV",
                Rezim::LenZapni => "len zapína TV",
                Rezim::Nic => "vypnuté",
            },
            self.zapnuti_tv,
            self.uspati_tv,
            self.tv_spal,
            if self.zobudeni_bez_adresy > 0 {
                format!(" (z toho bez adresy {}×, R203)", self.zobudeni_bez_adresy)
            } else {
                String::new()
            },
            self.tlacidiel,
            self.odoslanych_ok,
            self.odoslanych_chyba
        )
    }
}

fn prazdna() -> Sprava {
    Sprava {
        tx_ts: 0,
        rx_ts: 0,
        len: 0,
        timeout: 0,
        sequence: 0,
        flags: 0,
        msg: [0; 16],
        reply: 0,
        rx_status: 0,
        tx_status: 0,
        tx_arb_lost_cnt: 0,
        tx_nack_cnt: 0,
        tx_low_drive_cnt: 0,
        tx_error_cnt: 0,
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    const JA: u8 = 4; // prehrávač 1
    const PA: u16 = 0x1000; // HDMI 1

    #[test]
    fn spravy_pre_televizor() {
        assert_eq!(zapni_tv(JA), [0x40, 0x04]);
        // R203: od neregistrovaného (15) pre televízor (0) – jediná správa, ktorú jadro pošle bez adresy
        assert_eq!(zapni_tv_bez_adresy(), [0xF0, 0x04]);
        assert_eq!(aktivny_zdroj(JA, PA), [0x4F, 0x82, 0x10, 0x00]);
        assert_eq!(uspi_tv(JA), [0x40, 0x36]);
        assert_eq!(neaktivny_zdroj(JA, PA), [0x40, 0x9D, 0x10, 0x00]);
        assert_eq!(Rezim::z_nastavenia(None), Rezim::Vsetko);
        assert_eq!(Rezim::z_nastavenia(Some("zapni")), Rezim::LenZapni);
        assert_eq!(Rezim::z_nastavenia(Some("nie")), Rezim::Nic);
    }

    #[test]
    fn udalosti_od_televizora() {
        // tlačidlo „OK“ (0x00) z ovládača televízora
        assert_eq!(spracuj(&[0x04, 0x44, 0x00], JA, PA, true, true), (Some(Udalost::Tlacidlo(0)), None));
        // televízor ide spať (všetkým)
        assert_eq!(spracuj(&[0x0F, 0x36], JA, PA, true, true), (Some(Udalost::TvSpi), None));
        // iný prehrávač je aktívny / naša správa sa ignoruje
        assert_eq!(spracuj(&[0x8F, 0x82, 0x20, 0x00], JA, PA, true, true), (Some(Udalost::InyZdroj), None));
        assert_eq!(spracuj(&[0x4F, 0x82, 0x10, 0x00], JA, PA, true, true), (None, None));
        // televízor zvolil vstup krabičky
        assert_eq!(
            spracuj(&[0x0F, 0x86, 0x10, 0x00], JA, PA, false, false),
            (Some(Udalost::ChceMa), Some(vec![0x4F, 0x82, 0x10, 0x00]))
        );
        assert_eq!(
            spracuj(&[0x0F, 0x80, 0x20, 0x00, 0x10, 0x00], JA, PA, true, false),
            (Some(Udalost::ChceMa), Some(vec![0x4F, 0x82, 0x10, 0x00]))
        );
        // otázky televízora
        assert_eq!(spracuj(&[0x04, 0x8F], JA, PA, false, false), (None, Some(vec![0x40, 0x90, 1])));
        assert_eq!(spracuj(&[0x04, 0x8F], JA, PA, true, false), (None, Some(vec![0x40, 0x90, 0])));
        assert_eq!(spracuj(&[0x0F, 0x85], JA, PA, true, true), (None, Some(vec![0x4F, 0x82, 0x10, 0x00])));
        assert_eq!(spracuj(&[0x0F, 0x85], JA, PA, true, false), (None, None));
        assert_eq!(spracuj(&[0x04, 0x8D, 0x00], JA, PA, true, true), (None, Some(vec![0x40, 0x8E, 0])));
        // správa pre iné zariadenie
        assert_eq!(spracuj(&[0x08, 0x8F], JA, PA, true, true), (None, None));
    }
}
