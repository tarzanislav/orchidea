//! Vlastný ovládač obrazovky cez DRM/KMS jadra Linux – **chladný obraz** (R158).
//!
//! Meranie na X96 Max Plus (22. 11. 2026) ukázalo, že krabičku v pokoji nezohrieva procesor, ale obraz:
//! obvod obrazu (VPU) číta obraz z pamäte DDR stále dokola, aj keď sa nemení, a vysielač HDMI beží na takte
//! bodov. 4K → Full HD znížilo ustálenú teplotu o 23 °C. Tento ovládač ide ďalej – rovnakým smerom, akým
//! idú vedecké práce o energii (energiu žerie **pohyb dát**, nie výpočet – Horowitz, ISSCC 2014):
//!
//! 1. **najnižšia bežná obnova, ktorú televízor sám ponúka** (1080p24/25/30 – takt bodov a vysielača HDMI
//!    74,25 MHz namiesto 148,5 MHz; režim sa berie len zo zoznamu televízora, nič sa nevymýšľa),
//! 2. **16 bitov na bod (RGB565) a polovičné rozlíšenie** – rovina OSD obvodu obrazu má vlastný zväčšovač,
//!    takže z pamäte sa číta 960 × 540 × 2 B × 24 Hz ≈ 25 MB/s namiesto 1920 × 1080 × 4 B × 60 Hz ≈ 500 MB/s
//!    (20× menej); zväčšenie robí obvod obrazu za letu, bez práce procesora,
//! 3. **čierny obraz** (rovina vypnutá – z pamäte sa nečíta nič, televízor má stále signál, prebudenie
//!    okamžité) a **vypnutý obraz** (CRTC vypnuté – stojí kódovač obrazu aj vysielač HDMI).
//!
//! Používa atómové nastavenie (atomic KMS) – každá zmena sa najprv **vyskúša nanečisto** (TEST_ONLY), až
//! potom sa použije; keď ovládač jadra niečo nedovolí, KLÍČ ostane pri obyčajnom `/dev/fb0`. To isté
//! rozhranie neskôr použije ORCHIDEA na video: rovina VD1 prijíma NV12 priamo z hardvérového dekodéra.
//!
//! Žiadny nebezpečný kód – volania jadra sú v `sys.rs`.

use crate::chlad::{Ciel, StavObrazu};
use crate::sys;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;

const fn iowr(nr: u32, velkost: usize) -> std::ffi::c_ulong {
    ((3u32 << 30) | ((velkost as u32) << 16) | (0x64 << 8) | nr) as std::ffi::c_ulong
}
const fn iow(nr: u32, velkost: usize) -> std::ffi::c_ulong {
    ((1u32 << 30) | ((velkost as u32) << 16) | (0x64 << 8) | nr) as std::ffi::c_ulong
}

// štruktúry presne podľa include/uapi/drm/drm.h a drm_mode.h (veľkosti overuje skúška nižšie)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Schopnost {
    schopnost: u64,
    hodnota: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Zdroje {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Konektor {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

/// `struct drm_mode_modeinfo` – režim obrazu tak, ako ho hlási televízor (EDID) cez ovládač jadra.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    pub htotal: u16,
    hskew: u16,
    pub vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    pub vtotal: u16,
    vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub typ: u32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Koder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ZdrojeRovin {
    plane_id_ptr: u64,
    count_planes: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rovina {
    plane_id: u32,
    crtc_id: u32,
    fb_id: u32,
    possible_crtcs: u32,
    gamma_size: u32,
    count_format_types: u32,
    format_type_ptr: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VlastnostiObjektu {
    props_ptr: u64,
    prop_values_ptr: u64,
    count_props: u32,
    obj_id: u32,
    obj_type: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vlastnost {
    values_ptr: u64,
    enum_blob_ptr: u64,
    prop_id: u32,
    flags: u32,
    name: [u8; 32],
    count_values: u32,
    count_enum_blobs: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HlupyBuffer {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MapovanieBuffra {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rukovat {
    handle: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Snimka2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    pad: u32,
    modifier: [u64; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Atomicky {
    flags: u32,
    count_objs: u32,
    objs_ptr: u64,
    count_props_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    reserved: u64,
    user_data: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NovyBlob {
    data: u64,
    length: u32,
    blob_id: u32,
}

const SET_CLIENT_CAP: std::ffi::c_ulong = iow(0x0D, 16);
const GET_CAP: std::ffi::c_ulong = iowr(0x0C, 16);
const GETRESOURCES: std::ffi::c_ulong = iowr(0xA0, 64);
const GETENCODER: std::ffi::c_ulong = iowr(0xA6, 20);
const GETCONNECTOR: std::ffi::c_ulong = iowr(0xA7, 80);
const GETPROPERTY: std::ffi::c_ulong = iowr(0xAA, 64);
const RMFB: std::ffi::c_ulong = iowr(0xAF, 4);
const CREATE_DUMB: std::ffi::c_ulong = iowr(0xB2, 32);
const MAP_DUMB: std::ffi::c_ulong = iowr(0xB3, 16);
const DESTROY_DUMB: std::ffi::c_ulong = iowr(0xB4, 4);
const GETPLANERESOURCES: std::ffi::c_ulong = iowr(0xB5, 16);
const GETPLANE: std::ffi::c_ulong = iowr(0xB6, 32);
const ADDFB2: std::ffi::c_ulong = iowr(0xB8, 104);
const OBJ_GETPROPERTIES: std::ffi::c_ulong = iowr(0xB9, 32);
const ATOMIC: std::ffi::c_ulong = iowr(0xBC, 56);
const CREATEPROPBLOB: std::ffi::c_ulong = iowr(0xBD, 16);
const DESTROYPROPBLOB: std::ffi::c_ulong = iowr(0xBE, 4);

const CAP_DUMB_BUFFER: u64 = 1;
const CLIENT_CAP_UNIVERSAL_PLANES: u64 = 2;
const CLIENT_CAP_ATOMIC: u64 = 3;
const OBJ_CRTC: u32 = 0xcccc_cccc;
const OBJ_CONNECTOR: u32 = 0xc0c0_c0c0;
const OBJ_PLANE: u32 = 0xeeee_eeee;
const ATOMIC_TEST_ONLY: u32 = 0x0100;
const ATOMIC_ALLOW_MODESET: u32 = 0x0400;
const MODE_TYPE_PREFERRED: u32 = 1 << 3;
const MODE_FLAG_INTERLACE: u32 = 1 << 4;
const MODE_FLAG_3D_MASK: u32 = 0x1f << 14;
const PLANE_TYPE_PRIMARY: u64 = 1;

/// Formáty bodov (fourcc).
pub const RGB565: u32 = u32::from_le_bytes(*b"RG16");
pub const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
pub const NV12: u32 = u32::from_le_bytes(*b"NV12");

/// Režim obrazu bez technických podrobností – na výber a na obrazovku.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rezim {
    pub sirka: u16,
    pub vyska: u16,
    /// Obnova v Hz (presná, napr. 23,976).
    pub obnova: f64,
    /// Takt bodov (kHz) = takt vysielača HDMI.
    pub takt_khz: u32,
    pub prekladany: bool,
    pub preferovany: bool,
}

impl Rezim {
    pub fn z(m: &ModeInfo) -> Rezim {
        let body = m.htotal as f64 * m.vtotal as f64;
        let mut obnova = if body > 0.0 { m.clock as f64 * 1000.0 / body } else { m.vrefresh as f64 };
        if m.flags & MODE_FLAG_INTERLACE != 0 {
            obnova *= 2.0;
        }
        Rezim {
            sirka: m.hdisplay,
            vyska: m.vdisplay,
            obnova,
            takt_khz: m.clock,
            prekladany: m.flags & (MODE_FLAG_INTERLACE | MODE_FLAG_3D_MASK) != 0,
            preferovany: m.typ & MODE_TYPE_PREFERRED != 0,
        }
    }

    pub fn popis(&self) -> String {
        let obnova = if (self.obnova - self.obnova.round()).abs() < 0.01 {
            format!("{:.0}", self.obnova)
        } else {
            format!("{:.3}", self.obnova).replace('.', ",")
        };
        format!("{}×{} @ {obnova} Hz", self.sirka, self.vyska)
    }
}

/// Vyberie režim zo zoznamu televízora. Nikdy si režim nevymýšľa – iba vyberá z toho, čo televízor ponúkol;
/// 4K sa nevyberie nikdy (meranie: +23 °C).
pub fn vyber_rezim(rezimy: &[Rezim], ciel: Ciel) -> Option<usize> {
    let bezne = |r: &Rezim| !r.prekladany && r.obnova >= 23.0 && r.obnova <= 61.0;
    let s_rozmerom = |w: u16, h: u16| {
        rezimy.iter().enumerate().filter(move |(_, r)| bezne(r) && r.sirka == w && r.vyska == h)
    };
    // pri rovnakej obnove radšej presná (24,000 pred 23,976 – vyšší takt o 1/1000)
    let najnizsia = |w: u16, h: u16| {
        s_rozmerom(w, h).min_by(|a, b| {
            a.1.obnova.round().total_cmp(&b.1.obnova.round()).then(b.1.takt_khz.cmp(&a.1.takt_khz))
        })
    };
    let najvyssia = |w: u16, h: u16| {
        s_rozmerom(w, h).filter(|(_, r)| r.obnova >= 49.0).max_by(|a, b| {
            a.1.obnova.round().total_cmp(&b.1.obnova.round()).then(a.1.takt_khz.cmp(&b.1.takt_khz))
        })
    };
    let vyber = match ciel {
        Ciel::Chladny => najnizsia(1920, 1080).or_else(|| najnizsia(1280, 720)),
        Ciel::Povodny => najvyssia(1920, 1080).or_else(|| najvyssia(1280, 720)),
    };
    vyber.map(|(i, _)| i).or_else(|| {
        // neznámy televízor: preferovaný režim, ak nie je väčší ako Full HD, inak najmenší takt od 720 riadkov
        rezimy
            .iter()
            .position(|r| r.preferovany && bezne(r) && r.vyska <= 1080)
            .or_else(|| {
                rezimy
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| bezne(r) && r.vyska >= 720 && r.vyska <= 1080)
                    .min_by_key(|(_, r)| r.takt_khz)
                    .map(|(i, _)| i)
            })
            .or_else(|| (!rezimy.is_empty()).then_some(0))
    })
}

/// Ako sa kreslí: rozmery plátna a bity na bod. Plátno menšie ako režim zväčší obvod obrazu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kreslenie {
    pub sirka: u32,
    pub vyska: u32,
    /// 16 (RGB565) alebo 32 (XRGB8888).
    pub bitov: u32,
}

impl Kreslenie {
    /// Chladné kreslenie: polovičné rozlíšenie (najviac 2× zväčšenie – písmo ostane čitateľné, obvod obrazu
    /// dovolí až 5×) a 16 bitov. Pri malých režimoch plné rozlíšenie.
    pub fn chladne(r: &Rezim) -> Kreslenie {
        let (w, h) = (r.sirka as u32, r.vyska as u32);
        if w >= 1600 && h >= 900 {
            Kreslenie { sirka: w / 2, vyska: h / 2, bitov: 16 }
        } else {
            Kreslenie { sirka: w, vyska: h, bitov: 16 }
        }
    }

    pub fn povodne(r: &Rezim) -> Kreslenie {
        Kreslenie { sirka: r.sirka as u32, vyska: r.vyska as u32, bitov: 32 }
    }

    pub fn format(&self) -> u32 {
        if self.bitov == 16 { RGB565 } else { XRGB8888 }
    }

    /// Koľko bajtov za sekundu prečíta obvod obrazu z pamäte pri danej obnove.
    pub fn citanie_mb_s(&self, obnova: f64) -> f64 {
        self.sirka as f64 * self.vyska as f64 * (self.bitov / 8) as f64 * obnova / 1e6
    }
}

struct Buffer {
    rukovat: u32,
    snimka: u32,
    sirka: u32,
    vyska: u32,
    riadok: u32,
    format: u32,
    mapa: sys::Mapa,
}

/// Identifikátory vlastností, ktoré ovládač mení.
#[derive(Default, Clone, Copy)]
struct Ids {
    mode_id: u32,
    active: u32,
    kon_crtc: u32,
    fb_id: u32,
    crtc_id: u32,
    src: [u32; 4],
    dst: [u32; 4],
}

/// Obrazovka riadená priamo cez KMS.
pub struct Kms {
    karta: fs::File,
    crtc: u32,
    konektor: u32,
    rovina: u32,
    ids: Ids,
    rezimy: Vec<ModeInfo>,
    /// Formáty, ktoré prijme hlavná rovina (OSD).
    pub formaty: Vec<u32>,
    /// Rovina pre video NV12 (VD1) – pre ORCHIDEU; KLÍČ ju len hlási.
    pub video: Option<u32>,
    rezim: Option<usize>,
    blob: u32,
    buffer: Option<Buffer>,
    cierny: Option<Buffer>,
    pub stav: StavObrazu,
    /// Čierny obraz bez roviny ovládač nedovolil – použije sa malý čierny buffer (5× zväčšený).
    cierny_cez_buffer: bool,
}

fn meno(b: &[u8]) -> String {
    String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[])).into_owned()
}

impl Kms {
    /// Otvorí prvú grafickú kartu s pripojeným výstupom (card0 – card3: grafický procesor môže mať vlastnú
    /// kartu bez výstupov). Chyba = KLÍČ ostane pri `/dev/fb0`.
    pub fn otvor() -> io::Result<Kms> {
        let mut chyba = io::Error::other("žiadna grafická karta");
        for i in 0..4 {
            match fs::OpenOptions::new().read(true).write(true).open(format!("/dev/dri/card{i}")) {
                Ok(karta) => match Kms::otvor_kartu(karta) {
                    Ok(k) => return Ok(k),
                    Err(e) => chyba = e,
                },
                Err(e) if i == 0 => chyba = e,
                Err(_) => break,
            }
        }
        Err(chyba)
    }

    fn otvor_kartu(karta: fs::File) -> io::Result<Kms> {
        let fd = karta.as_raw_fd();
        let mut c = Schopnost { schopnost: CAP_DUMB_BUFFER, hodnota: 0 };
        sys::ioctl_struct(fd, GET_CAP, &mut c)?;
        if c.hodnota == 0 {
            return Err(io::Error::other("ovládač nemá jednoduché buffery"));
        }
        for s in [CLIENT_CAP_UNIVERSAL_PLANES, CLIENT_CAP_ATOMIC] {
            sys::ioctl_struct(fd, SET_CLIENT_CAP, &mut Schopnost { schopnost: s, hodnota: 1 })?;
        }
        // zdroje (dvakrát: počty, potom polia)
        let mut z = Zdroje::default();
        sys::ioctl_struct(fd, GETRESOURCES, &mut z)?;
        let mut crtcs = vec![0u32; z.count_crtcs as usize];
        let mut konektory = vec![0u32; z.count_connectors as usize];
        let mut z2 = Zdroje {
            crtc_id_ptr: sys::adresa(&mut crtcs),
            connector_id_ptr: sys::adresa(&mut konektory),
            count_crtcs: crtcs.len() as u32,
            count_connectors: konektory.len() as u32,
            ..Zdroje::default()
        };
        sys::ioctl_struct(fd, GETRESOURCES, &mut z2)?;
        for &k in konektory.iter().take(z2.count_connectors as usize) {
            let mut kon = Konektor { connector_id: k, ..Konektor::default() };
            sys::ioctl_struct(fd, GETCONNECTOR, &mut kon)?;
            if kon.connection != 1 || kon.count_modes == 0 {
                continue;
            }
            let mut rezimy = vec![ModeInfo::default(); kon.count_modes as usize];
            let mut kodery = vec![0u32; kon.count_encoders as usize];
            let mut kon2 = Konektor {
                connector_id: k,
                modes_ptr: sys::adresa(&mut rezimy),
                count_modes: rezimy.len() as u32,
                encoders_ptr: sys::adresa(&mut kodery),
                count_encoders: kodery.len() as u32,
                ..Konektor::default()
            };
            sys::ioctl_struct(fd, GETCONNECTOR, &mut kon2)?;
            rezimy.truncate(kon2.count_modes.min(rezimy.len() as u32) as usize);
            // CRTC: ktorý kóder ho môže použiť
            let mut maska = 0u32;
            for &e in kodery.iter().take(kon2.count_encoders as usize) {
                let mut kod = Koder { encoder_id: e, ..Koder::default() };
                if sys::ioctl_struct(fd, GETENCODER, &mut kod).is_ok() {
                    maska |= kod.possible_crtcs;
                }
            }
            let Some(ci) = (0..crtcs.len()).find(|i| maska & (1 << i) != 0) else { continue };
            let crtc = crtcs[ci];
            let mut kms = Kms {
                karta,
                crtc,
                konektor: k,
                rovina: 0,
                ids: Ids::default(),
                rezimy,
                formaty: Vec::new(),
                video: None,
                rezim: None,
                blob: 0,
                buffer: None,
                cierny: None,
                stav: StavObrazu::Zapnuty,
                cierny_cez_buffer: false,
            };
            kms.najdi_roviny(1 << ci)?;
            kms.najdi_vlastnosti()?;
            return Ok(kms);
        }
        Err(io::Error::other("žiadny pripojený výstup"))
    }

    fn fd(&self) -> i32 {
        self.karta.as_raw_fd()
    }

    /// Vlastnosti objektu: meno → (id, hodnota).
    fn vlastnosti(&self, obj: u32, typ: u32) -> io::Result<HashMap<String, (u32, u64)>> {
        let mut v = VlastnostiObjektu { obj_id: obj, obj_type: typ, ..VlastnostiObjektu::default() };
        sys::ioctl_struct(self.fd(), OBJ_GETPROPERTIES, &mut v)?;
        let mut ids = vec![0u32; v.count_props as usize];
        let mut hodnoty = vec![0u64; v.count_props as usize];
        let mut v2 = VlastnostiObjektu {
            props_ptr: sys::adresa(&mut ids),
            prop_values_ptr: sys::adresa(&mut hodnoty),
            count_props: ids.len() as u32,
            obj_id: obj,
            obj_type: typ,
            pad: 0,
        };
        sys::ioctl_struct(self.fd(), OBJ_GETPROPERTIES, &mut v2)?;
        let n = (v2.count_props as usize).min(ids.len());
        let mut mapa = HashMap::new();
        for i in 0..n {
            let mut p = Vlastnost { prop_id: ids[i], ..Vlastnost::default() };
            if sys::ioctl_struct(self.fd(), GETPROPERTY, &mut p).is_ok() {
                mapa.insert(meno(&p.name), (ids[i], hodnoty[i]));
            }
        }
        Ok(mapa)
    }

    fn najdi_roviny(&mut self, maska_crtc: u32) -> io::Result<()> {
        let fd = self.fd();
        let mut r = ZdrojeRovin::default();
        sys::ioctl_struct(fd, GETPLANERESOURCES, &mut r)?;
        let mut roviny = vec![0u32; r.count_planes as usize];
        let mut r2 =
            ZdrojeRovin { plane_id_ptr: sys::adresa(&mut roviny), count_planes: roviny.len() as u32, pad: 0 };
        sys::ioctl_struct(fd, GETPLANERESOURCES, &mut r2)?;
        for &p in roviny.iter().take(r2.count_planes as usize) {
            let mut g = Rovina { plane_id: p, ..Rovina::default() };
            sys::ioctl_struct(fd, GETPLANE, &mut g)?;
            if g.possible_crtcs & maska_crtc == 0 {
                continue;
            }
            let mut formaty = vec![0u32; g.count_format_types as usize];
            let mut g2 = Rovina {
                plane_id: p,
                format_type_ptr: sys::adresa(&mut formaty),
                count_format_types: formaty.len() as u32,
                ..Rovina::default()
            };
            sys::ioctl_struct(fd, GETPLANE, &mut g2)?;
            let typ = self.vlastnosti(p, OBJ_PLANE)?.get("type").map(|v| v.1);
            if typ == Some(PLANE_TYPE_PRIMARY) && self.rovina == 0 {
                self.rovina = p;
                self.formaty = formaty;
            } else if formaty.contains(&NV12) && self.video.is_none() {
                self.video = Some(p);
            }
        }
        if self.rovina == 0 {
            return Err(io::Error::other("bez hlavnej roviny"));
        }
        Ok(())
    }

    fn najdi_vlastnosti(&mut self) -> io::Result<()> {
        let c = self.vlastnosti(self.crtc, OBJ_CRTC)?;
        let k = self.vlastnosti(self.konektor, OBJ_CONNECTOR)?;
        let r = self.vlastnosti(self.rovina, OBJ_PLANE)?;
        let id = |m: &HashMap<String, (u32, u64)>, n: &str| {
            m.get(n).map(|v| v.0).ok_or_else(|| io::Error::other(format!("chýba vlastnosť {n}")))
        };
        self.ids = Ids {
            mode_id: id(&c, "MODE_ID")?,
            active: id(&c, "ACTIVE")?,
            kon_crtc: id(&k, "CRTC_ID")?,
            fb_id: id(&r, "FB_ID")?,
            crtc_id: id(&r, "CRTC_ID")?,
            src: [id(&r, "SRC_X")?, id(&r, "SRC_Y")?, id(&r, "SRC_W")?, id(&r, "SRC_H")?],
            dst: [id(&r, "CRTC_X")?, id(&r, "CRTC_Y")?, id(&r, "CRTC_W")?, id(&r, "CRTC_H")?],
        };
        Ok(())
    }

    /// Režimy, ktoré ponúka televízor.
    pub fn rezimy(&self) -> Vec<Rezim> {
        self.rezimy.iter().map(Rezim::z).collect()
    }

    fn novy_buffer(&self, k: Kreslenie) -> io::Result<Buffer> {
        let fd = self.fd();
        let mut d = HlupyBuffer { height: k.vyska, width: k.sirka, bpp: k.bitov, ..HlupyBuffer::default() };
        sys::ioctl_struct(fd, CREATE_DUMB, &mut d)?;
        let zrus = |e: io::Error| {
            let _ = sys::ioctl_struct(fd, DESTROY_DUMB, &mut Rukovat { handle: d.handle });
            e
        };
        let mut f = Snimka2 {
            width: k.sirka,
            height: k.vyska,
            pixel_format: k.format(),
            handles: [d.handle, 0, 0, 0],
            pitches: [d.pitch, 0, 0, 0],
            ..Snimka2::default()
        };
        sys::ioctl_struct(fd, ADDFB2, &mut f).map_err(zrus)?;
        let mut m = MapovanieBuffra { handle: d.handle, ..MapovanieBuffra::default() };
        let mapa = sys::ioctl_struct(fd, MAP_DUMB, &mut m)
            .and_then(|_| sys::Mapa::otvor(fd, d.size as usize, m.offset))
            .map_err(|e| {
                let _ = sys::ioctl_struct(fd, RMFB, &mut Rukovat { handle: f.fb_id });
                zrus(e)
            })?;
        let mut b = Buffer {
            rukovat: d.handle,
            snimka: f.fb_id,
            sirka: k.sirka,
            vyska: k.vyska,
            riadok: d.pitch,
            format: k.format(),
            mapa,
        };
        b.mapa.bajty().fill(0);
        Ok(b)
    }

    fn zrus_buffer(&self, b: Buffer) {
        let fd = self.fd();
        let (snimka, rukovat) = (b.snimka, b.rukovat);
        drop(b.mapa);
        let _ = sys::ioctl_struct(fd, RMFB, &mut Rukovat { handle: snimka });
        let _ = sys::ioctl_struct(fd, DESTROY_DUMB, &mut Rukovat { handle: rukovat });
    }

    fn novy_blob(&self, m: &ModeInfo) -> io::Result<u32> {
        let mut kopia = [*m];
        let mut b =
            NovyBlob { data: sys::adresa(&mut kopia), length: size_of::<ModeInfo>() as u32, blob_id: 0 };
        sys::ioctl_struct(self.fd(), CREATEPROPBLOB, &mut b)?;
        Ok(b.blob_id)
    }

    /// Jedno atómové nastavenie: najprv nanečisto, potom naozaj.
    fn uplatni(&self, zmeny: &[(u32, Vec<(u32, u64)>)], modeset: bool) -> io::Result<()> {
        let mut objekty: Vec<u32> = zmeny.iter().map(|z| z.0).collect();
        let mut pocty: Vec<u32> = zmeny.iter().map(|z| z.1.len() as u32).collect();
        let mut ids: Vec<u32> = zmeny.iter().flat_map(|z| z.1.iter().map(|v| v.0)).collect();
        let mut hodnoty: Vec<u64> = zmeny.iter().flat_map(|z| z.1.iter().map(|v| v.1)).collect();
        for test in [true, false] {
            let mut a = Atomicky {
                flags: if test { ATOMIC_TEST_ONLY } else { 0 }
                    | if modeset { ATOMIC_ALLOW_MODESET } else { 0 },
                count_objs: objekty.len() as u32,
                objs_ptr: sys::adresa(&mut objekty),
                count_props_ptr: sys::adresa(&mut pocty),
                props_ptr: sys::adresa(&mut ids),
                prop_values_ptr: sys::adresa(&mut hodnoty),
                reserved: 0,
                user_data: 0,
            };
            sys::ioctl_struct(self.fd(), ATOMIC, &mut a)?;
        }
        Ok(())
    }

    fn rovina_s(&self, b: &Buffer, r: &ModeInfo) -> Vec<(u32, u64)> {
        self.rovina_z((b.snimka, b.sirka, b.vyska), r)
    }

    /// Hlavná rovina so snímkou (id, šírka, výška) roztiahnutou na celý režim.
    fn rovina_z(&self, (snimka, sirka, vyska): (u32, u32, u32), r: &ModeInfo) -> Vec<(u32, u64)> {
        let i = &self.ids;
        vec![
            (i.fb_id, snimka as u64),
            (i.crtc_id, self.crtc as u64),
            (i.src[0], 0),
            (i.src[1], 0),
            (i.src[2], (sirka as u64) << 16),
            (i.src[3], (vyska as u64) << 16),
            (i.dst[0], 0),
            (i.dst[1], 0),
            (i.dst[2], r.hdisplay as u64),
            (i.dst[3], r.vdisplay as u64),
        ]
    }

    /// Nastaví režim a kreslenie (nový buffer; starý sa zruší až po úspešnej zmene). Obraz sa zapne.
    pub fn nastav(&mut self, rezim: usize, k: Kreslenie) -> io::Result<()> {
        let m = *self.rezimy.get(rezim).ok_or_else(|| io::Error::other("neznámy režim"))?;
        if !self.formaty.contains(&k.format()) {
            return Err(io::Error::other("rovina nemá tento formát"));
        }
        let b = self.novy_buffer(k)?;
        let blob = match self.novy_blob(&m) {
            Ok(x) => x,
            Err(e) => {
                self.zrus_buffer(b);
                return Err(e);
            }
        };
        let i = &self.ids;
        let zmeny = vec![
            (self.crtc, vec![(i.mode_id, blob as u64), (i.active, 1)]),
            (self.konektor, vec![(i.kon_crtc, self.crtc as u64)]),
            (self.rovina, self.rovina_s(&b, &m)),
        ];
        if let Err(e) = self.uplatni(&zmeny, true) {
            self.zrus_buffer(b);
            let _ = sys::ioctl_struct(self.fd(), DESTROYPROPBLOB, &mut Rukovat { handle: blob });
            return Err(e);
        }
        if let Some(stary) = self.buffer.replace(b) {
            self.zrus_buffer(stary);
        }
        if let Some(c) = self.cierny.take() {
            self.zrus_buffer(c);
        }
        if self.blob != 0 {
            let _ = sys::ioctl_struct(self.fd(), DESTROYPROPBLOB, &mut Rukovat { handle: self.blob });
        }
        self.blob = blob;
        self.rezim = Some(rezim);
        self.stav = StavObrazu::Zapnuty;
        Ok(())
    }

    /// Zapne, začierni alebo vypne obraz (obsah buffra ostáva – po zapnutí je obraz hneď späť).
    pub fn stav_obrazu(&mut self, stav: StavObrazu) -> io::Result<()> {
        if stav == self.stav {
            return Ok(());
        }
        let (Some(ri), Some(b)) = (self.rezim, self.buffer.as_ref()) else {
            return Err(io::Error::other("obraz nie je nastavený"));
        };
        let m = self.rezimy[ri];
        let i = self.ids;
        let hlavny = (b.snimka, b.sirka, b.vyska);
        let s_buffrom = |kms: &Kms, snimka: (u32, u32, u32), aktivny: u64| {
            vec![(kms.crtc, vec![(i.active, aktivny)]), (kms.rovina, kms.rovina_z(snimka, &m))]
        };
        let vysledok = match stav {
            StavObrazu::Zapnuty => self.uplatni(&s_buffrom(self, hlavny, 1), true),
            StavObrazu::Vypnuty => self.uplatni(&s_buffrom(self, hlavny, 0), true),
            StavObrazu::Cierny => {
                let bez_roviny =
                    vec![(self.crtc, vec![(i.active, 1)]), (self.rovina, vec![(i.fb_id, 0), (i.crtc_id, 0)])];
                if !self.cierny_cez_buffer && self.uplatni(&bez_roviny, true).is_ok() {
                    Ok(())
                } else {
                    // ovládač chce rovinu stále – malý čierny buffer zväčšený 5× (číta sa 25× menej)
                    self.cierny_cez_buffer = true;
                    if self.cierny.is_none() {
                        let k = Kreslenie {
                            sirka: (m.hdisplay as u32).div_ceil(5),
                            vyska: (m.vdisplay as u32).div_ceil(5),
                            bitov: 16,
                        };
                        self.cierny = Some(self.novy_buffer(k)?);
                    }
                    let c = self.cierny.as_ref().map(|c| (c.snimka, c.sirka, c.vyska)).unwrap_or(hlavny);
                    self.uplatni(&s_buffrom(self, c, 1), true)
                }
            }
        };
        vysledok?;
        self.stav = stav;
        Ok(())
    }

    /// Pamäť buffra na kreslenie: (bajty, riadok v bajtoch, šírka, výška, formát).
    pub fn pamat(&mut self) -> Option<(&mut [u8], usize, u32, u32, u32)> {
        let b = self.buffer.as_mut()?;
        let (riadok, w, h, f) = (b.riadok as usize, b.sirka, b.vyska, b.format);
        Some((b.mapa.bajty(), riadok, w, h, f))
    }

    pub fn cierny_cez_buffer(&self) -> bool {
        self.cierny_cez_buffer
    }
}

impl Drop for Kms {
    fn drop(&mut self) {
        if let Some(b) = self.buffer.take() {
            self.zrus_buffer(b);
        }
        if let Some(b) = self.cierny.take() {
            self.zrus_buffer(b);
        }
        // po zatvorení karty obnoví jadro svoju obrazovku (fbdev) samo
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn velkosti_struktur_ako_v_jadre() {
        assert_eq!(size_of::<Zdroje>(), 64);
        assert_eq!(size_of::<Konektor>(), 80);
        assert_eq!(size_of::<ModeInfo>(), 68);
        assert_eq!(size_of::<Koder>(), 20);
        assert_eq!(size_of::<ZdrojeRovin>(), 16);
        assert_eq!(size_of::<Rovina>(), 32);
        assert_eq!(size_of::<VlastnostiObjektu>(), 32);
        assert_eq!(size_of::<Vlastnost>(), 64);
        assert_eq!(size_of::<HlupyBuffer>(), 32);
        assert_eq!(size_of::<MapovanieBuffra>(), 16);
        assert_eq!(size_of::<Snimka2>(), 104);
        assert_eq!(size_of::<Atomicky>(), 56);
        assert_eq!(size_of::<NovyBlob>(), 16);
        // DRM_IOCTL_MODE_ATOMIC a DRM_IOCTL_MODE_ADDFB2 z hlavičky jadra
        assert_eq!(ATOMIC, 0xC038_64BC);
        assert_eq!(ADDFB2, 0xC068_64B8);
        assert_eq!(SET_CLIENT_CAP, 0x4010_640D);
        assert_eq!(RGB565, 0x3631_4752);
        assert_eq!(XRGB8888, 0x3432_5258);
    }

    fn r(w: u16, h: u16, hz: f64, takt: u32) -> Rezim {
        Rezim { sirka: w, vyska: h, obnova: hz, takt_khz: takt, prekladany: false, preferovany: false }
    }

    #[test]
    fn chladny_rezim_z_ponuky_televizora() {
        // typický 4K televízor: preferuje 4K, ponúka Full HD 24/25/30/50/60 aj 720p
        let mut tv = vec![
            r(3840, 2160, 60.0, 594000),
            r(1920, 1080, 60.0, 148500),
            r(1920, 1080, 59.94, 148352),
            r(1920, 1080, 50.0, 148500),
            r(1920, 1080, 30.0, 74250),
            r(1920, 1080, 24.0, 74250),
            r(1920, 1080, 23.976, 74176),
            r(1280, 720, 60.0, 74250),
        ];
        tv[0].preferovany = true;
        assert_eq!(vyber_rezim(&tv, Ciel::Chladny), Some(5), "1080p24 presne");
        assert_eq!(vyber_rezim(&tv, Ciel::Povodny), Some(1), "1080p60 ako doteraz");
        // televízor bez 24/30: Full HD 50
        let stary = vec![r(1920, 1080, 60.0, 148500), r(1920, 1080, 50.0, 148500), r(1280, 720, 50.0, 74250)];
        assert_eq!(vyber_rezim(&stary, Ciel::Chladny), Some(1));
        // monitor len 720p a 4K: nikdy 4K
        let mut m = vec![r(3840, 2160, 30.0, 297000), r(1280, 720, 60.0, 74250)];
        m[0].preferovany = true;
        assert_eq!(vyber_rezim(&m, Ciel::Chladny), Some(1));
        // prekladané režimy (1080i) sa nevyberú
        let mut i = vec![r(1920, 1080, 25.0, 74250), r(1920, 1080, 60.0, 148500)];
        i[0].prekladany = true;
        assert_eq!(vyber_rezim(&i, Ciel::Chladny), Some(1));
        assert_eq!(vyber_rezim(&[], Ciel::Chladny), None);
    }

    #[test]
    fn chladne_kreslenie_cita_dvadsatkrat_menej() {
        let fhd24 = r(1920, 1080, 24.0, 74250);
        let fhd60 = r(1920, 1080, 60.0, 148500);
        let k = Kreslenie::chladne(&fhd24);
        assert_eq!(k, Kreslenie { sirka: 960, vyska: 540, bitov: 16 });
        let p = Kreslenie::povodne(&fhd60);
        let pomer = p.citanie_mb_s(60.0) / k.citanie_mb_s(24.0);
        assert!((pomer - 20.0).abs() < 0.01, "{pomer}");
        assert_eq!(Kreslenie::chladne(&r(1280, 720, 60.0, 74250)).sirka, 1280);
        assert_eq!(k.format(), RGB565);
        assert_eq!(
            Rezim::z(&ModeInfo {
                clock: 74250,
                hdisplay: 1920,
                htotal: 2750,
                vdisplay: 1080,
                vtotal: 1125,
                ..ModeInfo::default()
            })
            .popis(),
            "1920×1080 @ 24 Hz"
        );
    }
}
