//! R230: cesta zvuku na kartách ASoC s DPCM (Amlogic G12A/SM1 – Odroid C4, X96): vlastný klient ovládacích prvkov
//! ALSA (`/dev/snd/controlCN`) bez knižníc.
//!
//! Zvuková karta „axg-sound-card“ má prednú časť (FRDDR – čítanie z pamäte) a zadnú (TDM → TOHDMITX → HDMI)
//! spojené len cez prepínače v jadre. Kým ich nikto nenastaví, otvorenie PCM skončí „no backend DAIs enabled“
//! (EINVAL) – presne to ukázal Odroid C4 26. 11. Cestu KLÍČ odvodí z popisu hardvéru (`audio-routing`:
//! „TDMOUT_B IN 0“ ← „FRDDR_A OUT 1“) a nastaví: FRDDR_A SRC 1 EN, FRDDR_A SINK 1 SEL = OUT n, TDMOUT_x SRC SEL =
//! IN m, TOHDMITX I2S SRC = I2S x, TOHDMITX I2S OUT EN.

use std::ffi::{c_long, c_ulong};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

use crate::sys;

const fn io_u(smer: u32, nr: u32, velkost: usize) -> c_ulong {
    ((smer << 30) | ((velkost as u32) << 16) | ((b'U' as u32) << 8) | nr) as c_ulong
}

/// `struct snd_ctl_elem_id`
#[repr(C)]
#[derive(Clone, Copy)]
struct ElemId {
    numid: u32,
    iface: i32,
    device: u32,
    subdevice: u32,
    meno: [u8; 44],
    index: u32,
}

/// `struct snd_ctl_elem_list`
#[repr(C)]
#[derive(Clone, Copy)]
struct ElemList {
    offset: u32,
    space: u32,
    used: u32,
    count: u32,
    pids: *mut ElemId,
    _vyhradene: [u8; 50],
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct Unia128([u8; 128]);

/// `struct snd_ctl_elem_info`
#[repr(C)]
#[derive(Clone, Copy)]
struct ElemInfo {
    id: ElemId,
    typ: i32,
    access: u32,
    count: u32,
    owner: i32,
    hodnota: Unia128,
    _vyhradene: [u8; 64],
}

const HODNOTA_B: usize = 128 * size_of::<c_long>();

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct UniaHodnoty([u8; HODNOTA_B]);

/// `struct snd_ctl_elem_value`
#[repr(C)]
#[derive(Clone, Copy)]
struct ElemValue {
    id: ElemId,
    indirect: u32,
    hodnota: UniaHodnoty,
    _vyhradene: [u8; 128],
}

const _: () = assert!(size_of::<ElemId>() == 64);
const _: () = assert!(size_of::<ElemInfo>() == 272);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<ElemList>() == 80 && size_of::<ElemValue>() == 1224);
#[cfg(target_pointer_width = "32")]
const _: () = assert!(size_of::<ElemList>() == 72 && size_of::<ElemValue>() == 712);

const ELEM_LIST: c_ulong = io_u(3, 0x10, size_of::<ElemList>());
const ELEM_INFO: c_ulong = io_u(3, 0x11, size_of::<ElemInfo>());
const ELEM_WRITE: c_ulong = io_u(3, 0x13, size_of::<ElemValue>());

const TYP_BOOL: i32 = 1;
const TYP_CELE: i32 = 2;
const TYP_VYCET: i32 = 3;

fn prazdne_id() -> ElemId {
    ElemId { numid: 0, iface: 0, device: 0, subdevice: 0, meno: [0; 44], index: 0 }
}

fn meno(id: &ElemId) -> String {
    let n = id.meno.iter().position(|&b| b == 0).unwrap_or(44);
    String::from_utf8_lossy(&id.meno[..n]).into_owned()
}

/// Požadovaná hodnota prvku.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hodnota {
    Zapnute,
    Polozka(String),
}

/// R230: cesta z popisu hardvéru – dvojice (cieľ, zdroj) z `audio-routing` (reťazce oddelené nulou).
pub fn dvojice_cesty(surove: &[u8]) -> Vec<(String, String)> {
    let r: Vec<String> = surove
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    r.chunks_exact(2).map(|p| (p[0].clone(), p[1].clone())).collect()
}

/// R230: pravidlá pre prednú časť `predna` (napr. „FRDDR_A“) podľa cesty: zdroj „FRDDR_A OUT n“ → cieľ
/// „TDMOUT_x IN m“ → TOHDMITX „I2S x“.
pub fn pravidla(cesta: &[(String, String)], predna: &str) -> Vec<(String, Hodnota)> {
    let mut v = Vec::new();
    let predpona = format!("{predna} OUT ");
    let Some((ciel, zdroj)) =
        cesta.iter().find(|(c, z)| z.starts_with(&predpona) && c.starts_with("TDMOUT_"))
    else {
        return v;
    };
    let vystup = zdroj.trim_start_matches(&predpona).trim();
    let Some((tdm, vstup)) = ciel.split_once(' ') else { return v };
    let pismeno = tdm.trim_start_matches("TDMOUT_");
    v.push((format!("{predna} SRC 1 EN Switch"), Hodnota::Zapnute));
    v.push((format!("{predna} SINK 1 SEL"), Hodnota::Polozka(format!("OUT {vystup}"))));
    v.push((format!("{tdm} SRC SEL"), Hodnota::Polozka(vstup.trim().to_string())));
    v.push(("TOHDMITX I2S SRC".into(), Hodnota::Polozka(format!("I2S {pismeno}"))));
    v.push(("TOHDMITX I2S OUT EN Switch".into(), Hodnota::Zapnute));
    v
}

/// R230: nastaví cestu zvuku pre zariadenie `pcmCkDdp` (predná časť FRDDR_A/B/C podľa čísla zariadenia), ak karta
/// takú cestu má. Vráti popis do záznamu alebo None (karta bez prepínačov cesty, napr. vc4 na RPi).
pub fn nastav_cestu(karta: &str, zariadenie: u32) -> Option<String> {
    let surove = std::fs::read("/proc/device-tree/sound/audio-routing").ok()?;
    let predna = format!("FRDDR_{}", (b'A' + zariadenie.min(2) as u8) as char);
    let pravidla = pravidla(&dvojice_cesty(&surove), &predna);
    if pravidla.is_empty() {
        return Some(format!("cesta zvuku: popis hardvéru nemá cestu z {predna} do TDMOUT"));
    }
    let f = match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(0o2000000)
        .open(format!("/dev/snd/controlC{karta}"))
    {
        Ok(f) => f,
        Err(e) => return Some(format!("cesta zvuku: ovládanie karty sa nedá otvoriť ({e})")),
    };
    let fd = f.as_raw_fd();
    // zoznam prvkov
    let mut l =
        ElemList { offset: 0, space: 0, used: 0, count: 0, pids: std::ptr::null_mut(), _vyhradene: [0; 50] };
    if let Err(e) = sys::ioctl_raz(fd, ELEM_LIST, &mut l) {
        return Some(format!("cesta zvuku: zoznam prvkov zlyhal ({e})"));
    }
    let mut idy = vec![prazdne_id(); l.count as usize];
    l.space = l.count;
    l.pids = idy.as_mut_ptr();
    if let Err(e) = sys::ioctl_raz(fd, ELEM_LIST, &mut l) {
        return Some(format!("cesta zvuku: zoznam prvkov zlyhal ({e})"));
    }
    idy.truncate(l.used as usize);
    let mut casti = Vec::new();
    for (nazov, chcem) in &pravidla {
        // X96 26. 11.: prepínač výstupu TOHDMITX sa volá „TOHDMITX Switch“ (nie „… I2S OUT EN Switch“) – bez
        // neho cesta nie je celá, zadná časť nedostane takt a zápis po 10 s skončí EIO
        let nahrady: &[&str] = if nazov == "TOHDMITX I2S OUT EN Switch" { &["TOHDMITX Switch"] } else { &[] };
        let Some(id) = idy
            .iter()
            .find(|i| meno(i) == *nazov)
            .or_else(|| nahrady.iter().find_map(|n| idy.iter().find(|i| meno(i) == *n)))
        else {
            casti.push(format!("{nazov}: chýba"));
            continue;
        };
        let skutocne = meno(id);
        casti.push(match nastav(fd, id, chcem) {
            Ok(t) => format!("{skutocne} = {t}"),
            Err(e) => format!("{skutocne}: {e}"),
        });
    }
    let chybaju = casti.iter().filter(|c| c.ends_with(": chýba")).count();
    let mut t = format!("cesta zvuku (R230): {}", casti.join(" · "));
    if chybaju > 0 {
        // pre rozbor: aké prvky karta má
        let mena: Vec<String> = idy.iter().map(meno).filter(|m| !m.is_empty()).take(40).collect();
        t += &format!(" · prvky karty: {}", mena.join(", "));
    }
    Some(t)
}

fn nastav(fd: i32, id: &ElemId, chcem: &Hodnota) -> Result<String, String> {
    let mut info = ElemInfo {
        id: *id,
        typ: 0,
        access: 0,
        count: 0,
        owner: 0,
        hodnota: Unia128([0; 128]),
        _vyhradene: [0; 64],
    };
    sys::ioctl_raz(fd, ELEM_INFO, &mut info).map_err(|e| format!("informácie zlyhali ({e})"))?;
    let mut v =
        ElemValue { id: *id, indirect: 0, hodnota: UniaHodnoty([0; HODNOTA_B]), _vyhradene: [0; 128] };
    let pocet = (info.count as usize).clamp(1, 128);
    let popis = match (info.typ, chcem) {
        (TYP_BOOL | TYP_CELE, Hodnota::Zapnute) => {
            let l = size_of::<c_long>();
            for i in 0..pocet {
                v.hodnota.0[i * l..(i + 1) * l].copy_from_slice(&(1 as c_long).to_ne_bytes());
            }
            "zapnuté".to_string()
        }
        (TYP_VYCET, Hodnota::Polozka(text)) => {
            let poloziek = u32::from_ne_bytes(info.hodnota.0[0..4].try_into().unwrap_or([0; 4]));
            let mut najdena = None;
            let mut mena = Vec::new();
            for k in 0..poloziek.min(64) {
                let mut i2 = info;
                i2.hodnota.0[4..8].copy_from_slice(&k.to_ne_bytes());
                if sys::ioctl_raz(fd, ELEM_INFO, &mut i2).is_ok() {
                    let n = &i2.hodnota.0[8..72];
                    let d = n.iter().position(|&b| b == 0).unwrap_or(64);
                    let m = String::from_utf8_lossy(&n[..d]).into_owned();
                    if m == *text {
                        najdena = Some(k);
                    }
                    mena.push(m);
                }
            }
            let k = najdena.ok_or_else(|| format!("položka „{text}“ nie je (má {})", mena.join("/")))?;
            for i in 0..pocet {
                v.hodnota.0[i * 4..(i + 1) * 4].copy_from_slice(&k.to_ne_bytes());
            }
            text.clone()
        }
        (t, _) => return Err(format!("nečakaný typ prvku {t}")),
    };
    sys::ioctl_raz(fd, ELEM_WRITE, &mut v).map_err(|e| format!("zápis zlyhal ({e})"))?;
    Ok(popis)
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn cesta_odroid_c4() {
        // audio-routing z meson-sm1-odroid.dtsi
        let mut s = Vec::new();
        for r in [
            "TDMOUT_B IN 0",
            "FRDDR_A OUT 1",
            "TDMOUT_B IN 1",
            "FRDDR_B OUT 1",
            "TDMOUT_B IN 2",
            "FRDDR_C OUT 1",
            "TDM_B Playback",
            "TDMOUT_B OUT",
        ] {
            s.extend_from_slice(r.as_bytes());
            s.push(0);
        }
        let c = dvojice_cesty(&s);
        assert_eq!(c.len(), 4);
        let p = pravidla(&c, "FRDDR_A");
        assert_eq!(
            p,
            vec![
                ("FRDDR_A SRC 1 EN Switch".to_string(), Hodnota::Zapnute),
                ("FRDDR_A SINK 1 SEL".to_string(), Hodnota::Polozka("OUT 1".into())),
                ("TDMOUT_B SRC SEL".to_string(), Hodnota::Polozka("IN 0".into())),
                ("TOHDMITX I2S SRC".to_string(), Hodnota::Polozka("I2S B".into())),
                ("TOHDMITX I2S OUT EN Switch".to_string(), Hodnota::Zapnute),
            ]
        );
        let p = pravidla(&c, "FRDDR_C");
        assert_eq!(p[2].1, Hodnota::Polozka("IN 2".into()));
        assert!(pravidla(&c, "FRDDR_D").is_empty());
    }
}
