//! Strážca linky HDMI (R185) – náš kód, bez knižníc. Televízor (HDMI 2.0) má v sebe registre SCDC (adresa
//! 0x54 na zbernici DDC kábla HDMI), v ktorých hlási, či **vidí takt** a či má **zamknuté všetky tri kanály**
//! obrazu, a či čaká **zakódovaný signál** (scrambling – potrebný len nad 340 MHz, napr. 4K @ 60 Hz).
//!
//! Prečo: televízor sa dnes viackrát „zasekol“ (napr. po krabičke RPi so 4K) – krabička posielala signál,
//! televízor hlásil „Nie je signál“ a pomohlo len odpojenie zo zásuvky. Strážca:
//! - pravidelne číta stav televízora (pri zapnutom obraze raz za 5 s – krátka správa na DDC),
//! - ak televízor čaká zakódovaný signál, hoci krabička posiela Full HD bez kódovania (R187), **vráti mu to**
//!   (zápis do registra televízora, nie do krabičky) – keď krabička kóduje (televízor to pri Full HD podporuje
//!   a jadro tak bolo nastavené), register televízora sa nemení (chyba prvej verzie R185, 24. 11.),
//! - ak televízor kanály nezamkne do 8 s, **reštartuje linku HDMI** (obraz vypne a znova zapne),
//!   najviac 3× za 10 min,
//! - reštart linky sa dá spustiť aj ručne: **5× stlačiť tlačidlo ovládača do 4 s**.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::sys;

const SCDC: u16 = 0x54;
const SINK_VERSION: u8 = 0x01;
const TMDS_CONFIG: u8 = 0x20;
const SCRAMBLER_STATUS: u8 = 0x21;
const STATUS_FLAGS_0: u8 = 0x40;

/// Stav linky z registrov SCDC televízora.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StavTv {
    pub verzia: u8,
    /// Televízor čaká zakódovaný signál (Scrambling_Enable).
    pub kodovanie: bool,
    /// Televízor čaká vysoký pomer taktu (TMDS_Bit_Clock_Ratio 1/40 – len nad 340 MHz).
    pub vysoky_pomer: bool,
    /// Televízor vidí zakódovaný signál.
    pub vidi_kodovanie: bool,
    /// Televízor vidí takt linky.
    pub takt: bool,
    /// Kanály 0, 1, 2 zamknuté.
    pub kanaly: [bool; 3],
    /// Surové registre 0x20, 0x21, 0x40 (do záznamu).
    pub surove: [u8; 3],
}

impl StavTv {
    pub fn z_registrov(verzia: u8, config: u8, scrambler: u8, flags: u8) -> StavTv {
        StavTv {
            verzia,
            kodovanie: config & 1 != 0,
            vysoky_pomer: config & 2 != 0,
            vidi_kodovanie: scrambler & 1 != 0,
            takt: flags & 1 != 0,
            kanaly: [flags & 2 != 0, flags & 4 != 0, flags & 8 != 0],
            surove: [config, scrambler, flags],
        }
    }

    /// Televízor signál prijíma (takt aj všetky kanály).
    pub fn zamknuty(&self) -> bool {
        self.takt && self.kanaly.iter().all(|&k| k)
    }

    /// Televízor čaká kódovanie, ktoré krabička pri takte `takt_khz` neposiela (zaseknutý stav). Len keď je
    /// isté, že krabička pri Full HD nekóduje (R187 zapnuté) – inak by zásah linku rozbil.
    pub fn caka_kodovanie(&self, takt_khz: u32, krabicka_koduje: Option<bool>) -> bool {
        krabicka_koduje == Some(false) && takt_khz <= 340_000 && (self.kodovanie || self.vysoky_pomer)
    }

    pub fn popis(&self) -> String {
        let k = |b: bool| if b { "✓" } else { "✗" };
        let a = |b: bool| if b { "áno" } else { "nie" };
        format!(
            "TV vidí takt {} · kanály {}{}{} · TV čaká kódovanie {}, vidí ho {} · SCDC v{} ({:02x} {:02x} {:02x})",
            k(self.takt),
            k(self.kanaly[0]),
            k(self.kanaly[1]),
            k(self.kanaly[2]),
            a(self.kodovanie || self.vysoky_pomer),
            a(self.vidi_kodovanie),
            self.verzia,
            self.surove[0],
            self.surove[1],
            self.surove[2]
        )
    }
}

const BEZ_KODOVANIA: &str = "sys/module/dw_hdmi/parameters/sklenik_bez_kodovania";

/// R187: kóduje krabička signál pri Full HD? `None` = jadro bez úpravy R187 (nevie sa – strážca nezasahuje).
pub fn krabicka_koduje(koren: &Path) -> Option<bool> {
    match fs::read_to_string(koren.join(BEZ_KODOVANIA)).ok()?.trim() {
        "Y" | "1" => Some(false),
        "N" | "0" => Some(true),
        _ => None,
    }
}

/// R187: Full HD bez kódovania (ako RPi) alebo s kódovaním (hlavné jadro); platí od ďalšieho nastavenia obrazu.
pub fn nastav_kodovanie(koren: &Path, kodovat: bool) -> io::Result<()> {
    fs::write(koren.join(BEZ_KODOVANIA), if kodovat { "N" } else { "Y" })
}

#[repr(C)]
#[derive(Clone, Copy)]
struct I2cMsg {
    addr: u16,
    flags: u16,
    len: u16,
    buf: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct I2cRdwr {
    msgs: *mut I2cMsg,
    nmsgs: u32,
}

const I2C_RDWR: std::ffi::c_ulong = 0x0707;
const I2C_M_RD: u16 = 1;

/// Zbernica DDC vysielača HDMI (adaptér „DesignWare HDMI“ jadra) cez `/dev/i2c-N`.
pub struct Ddc {
    subor: fs::File,
}

impl Ddc {
    pub fn otvor(koren: &Path) -> io::Result<Ddc> {
        for z in fs::read_dir(koren.join("sys/bus/i2c/devices"))?.flatten() {
            let meno = z.file_name().to_string_lossy().to_string();
            let nazov = fs::read_to_string(z.path().join("name")).unwrap_or_default();
            if meno.starts_with("i2c-") && nazov.trim() == "DesignWare HDMI" {
                let subor =
                    fs::OpenOptions::new().read(true).write(true).open(koren.join("dev").join(&meno))?;
                return Ok(Ddc { subor });
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "zbernica DDC HDMI nenájdená"))
    }

    fn prenos(&self, spravy: &mut [I2cMsg]) -> io::Result<()> {
        let mut d = I2cRdwr { msgs: spravy.as_mut_ptr(), nmsgs: spravy.len() as u32 };
        sys::ioctl_raz(self.subor.as_raw_fd(), I2C_RDWR, &mut d)
    }

    fn citaj(&self, reg: u8) -> io::Result<u8> {
        let (mut r, mut v) = (reg, 0u8);
        let mut m = [
            I2cMsg { addr: SCDC, flags: 0, len: 1, buf: &mut r },
            I2cMsg { addr: SCDC, flags: I2C_M_RD, len: 1, buf: &mut v },
        ];
        self.prenos(&mut m)?;
        Ok(v)
    }

    fn zapis(&self, reg: u8, hodnota: u8) -> io::Result<()> {
        let mut b = [reg, hodnota];
        let mut m = [I2cMsg { addr: SCDC, flags: 0, len: 2, buf: b.as_mut_ptr() }];
        self.prenos(&mut m)
    }

    /// Stav linky z televízora; chyba = televízor SCDC nemá (HDMI 1.4) alebo neodpovedá.
    pub fn stav(&self) -> io::Result<StavTv> {
        Ok(StavTv::z_registrov(
            self.citaj(SINK_VERSION)?,
            self.citaj(TMDS_CONFIG)?,
            self.citaj(SCRAMBLER_STATUS)?,
            self.citaj(STATUS_FLAGS_0)?,
        ))
    }

    /// Vráti televízor do nezakódovaného režimu (Full HD a menej).
    pub fn zrus_kodovanie(&self) -> io::Result<()> {
        self.zapis(TMDS_CONFIG, 0)
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn kodovanie_krabicky() {
        let k = std::env::temp_dir().join(format!("sklenik-hdmi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&k);
        assert_eq!(krabicka_koduje(&k), None);
        fs::create_dir_all(k.join("sys/module/dw_hdmi/parameters")).unwrap();
        fs::write(k.join(BEZ_KODOVANIA), "Y\n").unwrap();
        assert_eq!(krabicka_koduje(&k), Some(false));
        nastav_kodovanie(&k, true).unwrap();
        assert_eq!(krabicka_koduje(&k), Some(true));
        let _ = fs::remove_dir_all(&k);
    }

    #[test]
    fn stav_z_registrov() {
        let s = StavTv::z_registrov(1, 0, 0, 0x0f);
        assert!(s.zamknuty() && !s.caka_kodovanie(74_250, Some(false)));
        assert_eq!(
            s.popis(),
            "TV vidí takt ✓ · kanály ✓✓✓ · TV čaká kódovanie nie, vidí ho nie · SCDC v1 (00 00 0f)"
        );
        // zaseknutý po 4K: čaká kódovanie a vysoký pomer, nič nezamkne
        let z = StavTv::z_registrov(1, 0x03, 0, 0x01);
        assert!(
            !z.zamknuty() && z.caka_kodovanie(74_250, Some(false)) && !z.caka_kodovanie(594_000, Some(false))
        );
        assert_eq!(z.kanaly, [false, false, false]);
        // 24. 11.: krabička kóduje aj Full HD (televízor to podporuje) – strážca nesmie kódovanie televízoru vypnúť
        let k = StavTv::z_registrov(1, 0x01, 0x01, 0x03);
        assert!(!k.caka_kodovanie(74_250, Some(true)) && !k.caka_kodovanie(74_250, None));
    }
}
