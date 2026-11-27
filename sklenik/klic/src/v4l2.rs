//! Vlastný klient hardvérového dekodéra videa cez V4L2 mem2mem (R165) – surové ioctl, žiadne knižnice.
//!
//! Dekodér v jadre (Amlogic meson-vdec; HEVC vlastným kódom SKLENÍKA) má dve fronty: **OUTPUT** berie
//! komprimovaný tok, **CAPTURE** vracia dekódované snímky NV12 (rovina Y a rovina striedavých Cb/Cr). KLÍČ
//! ním overuje celú cestu – firmvér, prerušenia, pamäť, poradie snímok – na tokoch s presne známym obrazom
//! (`video.rs`) a meria čas. Rovnaké rozhranie neskôr použije ORCHIDEA na prehrávanie (snímky NV12 idú
//! cez dmabuf priamo do roviny VD1 obvodu obrazu, bez kopírovania).
//!
//! Rozloženie štruktúr je podľa `include/uapi/linux/videodev2.h` pre 64-bitové jadro (aarch64); na 32-bitových
//! systémoch sa modul neprekladá.

use crate::sys;
use crate::video::{Obraz, porovnaj};
use std::ffi::c_ulong;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

const fn io(smer: u32, nr: u32, velkost: usize) -> c_ulong {
    ((smer << 30) | ((velkost as u32) << 16) | (0x56 << 8) | nr) as c_ulong
}
const R: u32 = 2;
const W: u32 = 1;
const RW: u32 = 3;

/// `struct v4l2_capability`
#[repr(C)]
#[derive(Clone, Copy)]
struct Schopnosti {
    ovladac: [u8; 16],
    karta: [u8; 32],
    zbernica: [u8; 32],
    verzia: u32,
    schopnosti: u32,
    schopnosti_zariadenia: u32,
    rezerva: [u32; 3],
}

/// `struct v4l2_fmtdesc`
#[repr(C)]
#[derive(Clone, Copy)]
struct PopisFormatu {
    index: u32,
    typ: u32,
    priznaky: u32,
    popis: [u8; 32],
    fourcc: u32,
    mbus: u32,
    rezerva: [u32; 3],
}

/// `struct v4l2_frmsizeenum` (spojitý rozsah: min šírka, max šírka, krok, min výška, max výška, krok)
#[repr(C)]
#[derive(Clone, Copy)]
struct Velkosti {
    index: u32,
    fourcc: u32,
    typ: u32,
    hodnoty: [u32; 6],
    rezerva: [u32; 2],
}

/// `struct v4l2_plane_pix_format`
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FormatRoviny {
    velkost: u32,
    riadok: u32,
    rezerva: [u16; 6],
}

/// `struct v4l2_pix_format_mplane`
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FormatObrazu {
    sirka: u32,
    vyska: u32,
    fourcc: u32,
    pole: u32,
    farby: u32,
    roviny: [FormatRoviny; 8],
    pocet_rovin: u8,
    priznaky: u8,
    ycbcr: u8,
    kvantovanie: u8,
    prenos: u8,
    rezerva: [u8; 7],
}

/// `struct v4l2_format` (únia formátov má 200 bajtov a zarovnanie 8)
#[repr(C)]
#[derive(Clone, Copy)]
struct Format {
    typ: u32,
    _vypln: u32,
    mp: FormatObrazu,
    zvysok: [u8; 8],
}

/// `struct v4l2_requestbuffers`
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Ziadost {
    pocet: u32,
    typ: u32,
    pamat: u32,
    schopnosti: u32,
    priznaky: u8,
    rezerva: [u8; 3],
}

/// `struct v4l2_plane`
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rovina {
    pouzite: u32,
    dlzka: u32,
    posun: u64,
    posun_dat: u32,
    rezerva: [u32; 11],
}

/// `struct v4l2_buffer` (64-bitové jadro – čas ako dve 64-bitové čísla)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Buffer {
    index: u32,
    typ: u32,
    pouzite: u32,
    priznaky: u32,
    pole: u32,
    _vypln: u32,
    cas_s: i64,
    cas_us: i64,
    casovy_kod: [u32; 4],
    poradie: u32,
    pamat: u32,
    roviny: u64,
    pocet_rovin: u32,
    rezerva2: u32,
    poziadavka: u32,
    _vypln2: u32,
}

/// `struct v4l2_event_subscription`
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Odber {
    typ: u32,
    id: u32,
    priznaky: u32,
    rezerva: [u32; 5],
}

/// `struct v4l2_event`
#[repr(C)]
#[derive(Clone, Copy)]
struct Udalost {
    typ: u32,
    _vypln: u32,
    data: [u64; 8],
    cakajuce: u32,
    poradie: u32,
    cas_s: i64,
    cas_ns: i64,
    id: u32,
    rezerva: [u32; 8],
    _vypln2: u32,
}

/// `struct v4l2_decoder_cmd`
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PrikazDekodera {
    prikaz: u32,
    priznaky: u32,
    data: [u32; 16],
}

/// `struct v4l2_control`
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Ovladanie {
    id: u32,
    hodnota: i32,
}

const QUERYCAP: c_ulong = io(R, 0, size_of::<Schopnosti>());
const ENUM_FMT: c_ulong = io(RW, 2, size_of::<PopisFormatu>());
const G_FMT: c_ulong = io(RW, 4, size_of::<Format>());
const S_FMT: c_ulong = io(RW, 5, size_of::<Format>());
const REQBUFS: c_ulong = io(RW, 8, size_of::<Ziadost>());
const QUERYBUF: c_ulong = io(RW, 9, size_of::<Buffer>());
const QBUF: c_ulong = io(RW, 15, size_of::<Buffer>());
const DQBUF: c_ulong = io(RW, 17, size_of::<Buffer>());
const STREAMON: c_ulong = io(W, 18, size_of::<i32>());
const STREAMOFF: c_ulong = io(W, 19, size_of::<i32>());
const G_CTRL: c_ulong = io(RW, 27, size_of::<Ovladanie>());
const ENUM_FRAMESIZES: c_ulong = io(RW, 74, size_of::<Velkosti>());
const DQEVENT: c_ulong = io(R, 89, size_of::<Udalost>());
const SUBSCRIBE_EVENT: c_ulong = io(W, 90, size_of::<Odber>());
const DECODER_CMD: c_ulong = io(RW, 96, size_of::<PrikazDekodera>());

const VYSTUP: u32 = 9; // V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE – snímky z dekodéra
const VSTUP: u32 = 10; // V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE – tok do dekodéra
const MMAP: u32 = 1;
const POLE_ZIADNE: u32 = 1;
const CAP_M2M_MPLANE: u32 = 0x4000;
const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
const UDALOST_EOS: u32 = 2;
const UDALOST_ZMENA_ZDROJA: u32 = 5;
const BUF_POSLEDNY: u32 = 0x0010_0000;
const BUF_CHYBA: u32 = 0x40;
const MIN_BUFFROV_VYSTUPU: u32 = 0x0098_0927;
const PRIKAZ_STOP: u32 = 1;

/// Kód formátu (fourcc) zo štyroch znakov.
pub const fn fourcc(k: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*k)
}
pub const H264: u32 = fourcc(b"H264");
pub const HEVC: u32 = fourcc(b"HEVC");
pub const NV12M: u32 = fourcc(b"NM12");

/// Kód formátu ako text (napr. „H264“, „VP90“).
pub fn nazov(f: u32) -> String {
    f.to_le_bytes().iter().map(|&c| if c.is_ascii_graphic() { c as char } else { '?' }).collect()
}

fn text(b: &[u8]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).into_owned()
}

/// Formát, ktorý dekodér prijíma.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatDekodera {
    pub fourcc: u32,
    pub popis: String,
    /// najväčšie rozlíšenie (šírka, výška)
    pub max: Option<(u32, u32)>,
}

/// Nájdený hardvérový dekodér.
#[derive(Debug, Clone, PartialEq)]
pub struct Dekoder {
    pub cesta: String,
    pub ovladac: String,
    pub karta: String,
    pub formaty: Vec<FormatDekodera>,
}

impl Dekoder {
    pub fn vie(&self, f: u32) -> bool {
        self.formaty.iter().any(|x| x.fourcc == f)
    }

    /// Riadok do záznamu: „meson-vdec: H.264 do 3840×2160 · VP9 do 3840×2160 · …“
    pub fn popis(&self) -> String {
        let f: Vec<String> = self
            .formaty
            .iter()
            .map(|f| match f.max {
                Some((s, v)) => format!("{} do {s}×{v}", nazov(f.fourcc)),
                None => nazov(f.fourcc),
            })
            .collect();
        format!("{} ({}): {}", self.ovladac, self.cesta, f.join(" · "))
    }
}

fn otvor(cesta: &str) -> io::Result<File> {
    // O_NONBLOCK – výber hotového buffra nečaká (čaká sa cez poll)
    OpenOptions::new().read(true).write(true).custom_flags(0o4000).open(cesta)
}

/// Nájde dekodér videa (zariadenie mem2mem s komprimovanými formátmi na vstupe) medzi `/dev/video*`.
pub fn najdi() -> Option<Dekoder> {
    for i in 0..16 {
        let cesta = format!("/dev/video{i}");
        let Ok(f) = otvor(&cesta) else { continue };
        let fd = f.as_raw_fd();
        let mut s = Schopnosti {
            ovladac: [0; 16],
            karta: [0; 32],
            zbernica: [0; 32],
            verzia: 0,
            schopnosti: 0,
            schopnosti_zariadenia: 0,
            rezerva: [0; 3],
        };
        if sys::ioctl_struct(fd, QUERYCAP, &mut s).is_err() {
            continue;
        }
        let caps = if s.schopnosti & CAP_DEVICE_CAPS != 0 { s.schopnosti_zariadenia } else { s.schopnosti };
        if caps & CAP_M2M_MPLANE == 0 {
            continue;
        }
        let mut formaty = Vec::new();
        for index in 0..32 {
            let mut p = PopisFormatu {
                index,
                typ: VSTUP,
                priznaky: 0,
                popis: [0; 32],
                fourcc: 0,
                mbus: 0,
                rezerva: [0; 3],
            };
            if sys::ioctl_struct(fd, ENUM_FMT, &mut p).is_err() {
                break;
            }
            // len komprimované formáty (príznak V4L2_FMT_FLAG_COMPRESSED) – to je dekodér
            if p.priznaky & 1 == 0 {
                continue;
            }
            let mut v = Velkosti { index: 0, fourcc: p.fourcc, typ: 0, hodnoty: [0; 6], rezerva: [0; 2] };
            let max = match sys::ioctl_struct(fd, ENUM_FRAMESIZES, &mut v) {
                // 1 = pevné rozmery (šírka, výška), 2/3 = rozsah (min, max, krok …)
                Ok(()) if v.typ == 1 => Some((v.hodnoty[0], v.hodnoty[1])),
                Ok(()) => Some((v.hodnoty[1], v.hodnoty[4])),
                Err(_) => None,
            };
            formaty.push(FormatDekodera { fourcc: p.fourcc, popis: text(&p.popis), max });
        }
        if !formaty.is_empty() {
            return Some(Dekoder { cesta, ovladac: text(&s.ovladac), karta: text(&s.karta), formaty });
        }
    }
    None
}

/// Buffer s namapovanými rovinami.
struct Buf {
    mapy: Vec<sys::Mapa>,
    dlzky: Vec<u32>,
}

fn ziadaj(fd: i32, typ: u32, pocet: u32) -> io::Result<u32> {
    let mut z = Ziadost { pocet, typ, pamat: MMAP, ..Default::default() };
    sys::ioctl_struct(fd, REQBUFS, &mut z)?;
    Ok(z.pocet)
}

fn mapuj(fd: i32, typ: u32, pocet: u32, rovin: usize) -> io::Result<Vec<Buf>> {
    (0..pocet)
        .map(|index| {
            let mut roviny = [Rovina::default(); 4];
            let mut b = Buffer {
                index,
                typ,
                pamat: MMAP,
                roviny: sys::adresa(&mut roviny[..rovin]),
                pocet_rovin: rovin as u32,
                ..Default::default()
            };
            sys::ioctl_struct(fd, QUERYBUF, &mut b)?;
            let mut mapy = Vec::new();
            let mut dlzky = Vec::new();
            for r in &roviny[..rovin] {
                mapy.push(sys::Mapa::otvor(fd, r.dlzka as usize, r.posun & 0xFFFF_FFFF)?);
                dlzky.push(r.dlzka);
            }
            Ok(Buf { mapy, dlzky })
        })
        .collect()
}

fn zarad(fd: i32, typ: u32, index: u32, buf: &Buf, pouzite: &[u32], cas_us: i64) -> io::Result<()> {
    let mut roviny = [Rovina::default(); 4];
    for (i, r) in roviny.iter_mut().enumerate().take(buf.dlzky.len()) {
        r.dlzka = buf.dlzky[i];
        r.pouzite = pouzite.get(i).copied().unwrap_or(0);
    }
    let n = buf.dlzky.len();
    let mut b = Buffer {
        index,
        typ,
        pamat: MMAP,
        pole: POLE_ZIADNE,
        cas_us,
        roviny: sys::adresa(&mut roviny[..n]),
        pocet_rovin: n as u32,
        ..Default::default()
    };
    sys::ioctl_struct(fd, QBUF, &mut b)
}

/// Vyberie hotový buffer: (index, príznaky, použité bajty rovín); `None`, keď ešte nie je.
/// Vybraný buffer: (index, príznaky, použité bajty rovín, časová značka v µs).
type Vybrany = (u32, u32, [u32; 4], i64);

fn vyber(fd: i32, typ: u32, rovin: usize) -> io::Result<Option<Vybrany>> {
    let mut roviny = [Rovina::default(); 4];
    let mut b = Buffer {
        typ,
        pamat: MMAP,
        roviny: sys::adresa(&mut roviny[..rovin]),
        pocet_rovin: rovin as u32,
        ..Default::default()
    };
    match sys::ioctl_raz(fd, DQBUF, &mut b) {
        Ok(()) => {
            let mut p = [0u32; 4];
            for (i, r) in roviny.iter().enumerate().take(rovin) {
                p[i] = r.pouzite;
            }
            Ok(Some((b.index, b.priznaky, p, b.cas_s * 1_000_000 + b.cas_us)))
        }
        Err(e) if e.raw_os_error() == Some(11) => Ok(None),
        // EPIPE: po poslednom buffri (dekodér zastavený)
        Err(e) if e.raw_os_error() == Some(32) => Ok(None),
        Err(e) => Err(e),
    }
}

fn prud(fd: i32, typ: u32, zapni: bool) -> io::Result<()> {
    let mut t = typ as i32;
    sys::ioctl_struct(fd, if zapni { STREAMON } else { STREAMOFF }, &mut t)
}

fn format(fd: i32, typ: u32) -> io::Result<Format> {
    let mut f = Format { typ, _vypln: 0, mp: FormatObrazu::default(), zvysok: [0; 8] };
    sys::ioctl_struct(fd, G_FMT, &mut f)?;
    Ok(f)
}

/// Výsledok skúšky dekodéra.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vysledok {
    /// dekódovaných snímok
    pub snimok: usize,
    /// z nich presných (bit po bite)
    pub presnych: usize,
    /// prvá odchýlka (snímka, rovina, riadok, stĺpec, čakané → dekódované)
    pub prva_chyba: Option<String>,
    /// čas od spustenia po poslednú snímku
    pub ms: u64,
    /// rozmery hlásené dekodérom
    pub rozmer: (u32, u32),
    /// zmien zdroja (udalosť SOURCE_CHANGE)
    pub zmien: u32,
    /// R225: ku každému výstupu najbližší vzor (index, počet odlišných bodov) – ukáže vynechané, zdvojené alebo
    /// prehodené snímky a posun, nielen „0/8 presne“
    pub poradie: Vec<(usize, usize, i64)>,
}

impl Vysledok {
    pub fn popis(&self, cakanych: usize) -> String {
        let mut t = format!(
            "{}/{} snímok presne, {} ms, {}×{}",
            self.presnych, cakanych, self.ms, self.rozmer.0, self.rozmer.1
        );
        if self.snimok != cakanych {
            t += &format!(" (dekódovaných {})", self.snimok);
        }
        if let Some(c) = &self.prva_chyba {
            t += &format!(" – prvá chyba: {c}");
        }
        if self.presnych < self.snimok && !self.poradie.is_empty() {
            let p: Vec<String> = self
                .poradie
                .iter()
                .enumerate()
                .map(|(i, &(vzor, chyb, jednotka))| {
                    let zhoda = if chyb == 0 { "✓".to_string() } else { format!("({chyb})") };
                    format!("{}[tok {jednotka}]→{}{zhoda}", i + 1, vzor + 1)
                })
                .collect();
            t += &format!(" · výstup[časová značka = číslo jednotky toku]→najbližší vzor: {}", p.join(" "));
        }
        t
    }
}

/// Snímky NV12 z dekodéra.
struct Vystupy {
    bufy: Vec<Buf>,
    sirka: u32,
    vyska: u32,
    riadok_y: u32,
    riadok_uv: u32,
}

fn priprav_vystupy(fd: i32) -> io::Result<Vystupy> {
    let f = format(fd, VYSTUP)?;
    let mut min = Ovladanie { id: MIN_BUFFROV_VYSTUPU, hodnota: 0 };
    let min = if sys::ioctl_struct(fd, G_CTRL, &mut min).is_ok() { min.hodnota.max(1) as u32 } else { 2 };
    let pocet = ziadaj(fd, VYSTUP, (min + 2).max(4))?;
    let rovin = (f.mp.pocet_rovin as usize).clamp(1, 2);
    let bufy = mapuj(fd, VYSTUP, pocet, rovin)?;
    for (i, b) in bufy.iter().enumerate() {
        zarad(fd, VYSTUP, i as u32, b, &[], 0)?;
    }
    prud(fd, VYSTUP, true)?;
    Ok(Vystupy {
        bufy,
        sirka: f.mp.sirka,
        vyska: f.mp.vyska,
        riadok_y: f.mp.roviny[0].riadok.max(f.mp.sirka),
        riadok_uv: f.mp.roviny[1].riadok.max(f.mp.sirka),
    })
}

/// Skúška dekodéra: pošle prístupové jednotky `jednotky` (formát `fourcc`) a porovná vrátené snímky so
/// vzorom `cakane` bit po bite. Najviac `limit` času; dekodér sa vždy zastaví a uvoľní.
pub fn skus(
    cesta: &str,
    fourcc: u32,
    jednotky: &[Vec<u8>],
    cakane: &[Obraz],
    limit: Duration,
) -> io::Result<Vysledok> {
    let subor = otvor(cesta)?;
    let fd = subor.as_raw_fd();
    let (sirka, vyska) = (cakane[0].sirka as u32, cakane[0].vyska as u32);
    let najvacsia = jednotky.iter().map(|j| j.len()).max().unwrap_or(0) as u32;

    // vstup: komprimovaný tok, jedna rovina
    let mut f = Format { typ: VSTUP, _vypln: 0, mp: FormatObrazu::default(), zvysok: [0; 8] };
    f.mp.sirka = sirka;
    f.mp.vyska = vyska;
    f.mp.fourcc = fourcc;
    f.mp.pocet_rovin = 1;
    f.mp.roviny[0].velkost = (najvacsia + 4096).max(1 << 20);
    sys::ioctl_struct(fd, S_FMT, &mut f)?;
    // výstup: NV12 v dvoch rovinách
    let mut f = Format { typ: VYSTUP, _vypln: 0, mp: FormatObrazu::default(), zvysok: [0; 8] };
    f.mp.sirka = sirka;
    f.mp.vyska = vyska;
    f.mp.fourcc = NV12M;
    f.mp.pocet_rovin = 2;
    sys::ioctl_struct(fd, S_FMT, &mut f)?;
    for typ in [UDALOST_ZMENA_ZDROJA, UDALOST_EOS] {
        let mut o = Odber { typ, ..Default::default() };
        sys::ioctl_struct(fd, SUBSCRIBE_EVENT, &mut o)?;
    }

    let pocet_vstupov = ziadaj(fd, VSTUP, 2)?;
    let mut vstupy = mapuj(fd, VSTUP, pocet_vstupov, 1)?;
    let mut volne: Vec<u32> = (0..pocet_vstupov).collect();
    let mut vystupy = Some(priprav_vystupy(fd)?);

    let start = Instant::now();
    let mut v = Vysledok::default();
    let mut dalsia = 0usize;
    let mut zastaveny = false;
    let mut prud_vstupu = false;
    let mut chyba: Option<io::Error> = None;

    'hlavna: while start.elapsed() < limit && v.snimok < cakane.len() {
        // tok do dekodéra – kým sú voľné buffre
        while dalsia < jednotky.len() {
            let Some(i) = volne.pop() else { break };
            let tok = &jednotky[dalsia];
            let buf = &mut vstupy[i as usize];
            if tok.len() > buf.dlzky[0] as usize {
                chyba = Some(io::Error::new(io::ErrorKind::InvalidInput, "tok je väčší ako buffer dekodéra"));
                break 'hlavna;
            }
            buf.mapy[0].bajty()[..tok.len()].copy_from_slice(tok);
            if let Err(e) = zarad(fd, VSTUP, i, buf, &[tok.len() as u32], dalsia as i64 + 1) {
                chyba = Some(e);
                break 'hlavna;
            }
            dalsia += 1;
            if !prud_vstupu {
                if let Err(e) = prud(fd, VSTUP, true) {
                    chyba = Some(e);
                    break 'hlavna;
                }
                prud_vstupu = true;
            }
        }
        // všetko odoslané → dekodér nech vydá aj zvyšné snímky
        if dalsia == jednotky.len() && !zastaveny && prud_vstupu {
            let mut p = PrikazDekodera { prikaz: PRIKAZ_STOP, ..Default::default() };
            let _ = sys::ioctl_struct(fd, DECODER_CMD, &mut p);
            zastaveny = true;
        }
        let zostava = limit.saturating_sub(start.elapsed()).as_millis() as u64;
        let r = match sys::cakaj_na(fd, 1 | 2 | 4, zostava.min(500)) {
            Ok(r) => r,
            Err(e) => {
                chyba = Some(e);
                break;
            }
        };
        // udalosti: zmena zdroja (rozmery / počet snímok, ktoré dekodér potrebuje)
        if r & 2 != 0 {
            let mut u = Udalost {
                typ: 0,
                _vypln: 0,
                data: [0; 8],
                cakajuce: 0,
                poradie: 0,
                cas_s: 0,
                cas_ns: 0,
                id: 0,
                rezerva: [0; 8],
                _vypln2: 0,
            };
            while sys::ioctl_raz(fd, DQEVENT, &mut u).is_ok() {
                if u.typ == UDALOST_ZMENA_ZDROJA {
                    v.zmien += 1;
                    let _ = prud(fd, VYSTUP, false);
                    drop(vystupy.take());
                    let _ = ziadaj(fd, VYSTUP, 0);
                    match priprav_vystupy(fd) {
                        Ok(n) => vystupy = Some(n),
                        Err(e) => {
                            chyba = Some(e);
                            break 'hlavna;
                        }
                    }
                }
                if u.typ == UDALOST_EOS && v.snimok >= cakane.len() {
                    break 'hlavna;
                }
            }
        }
        // vrátené buffre toku
        while let Ok(Some((i, _, _, _))) = vyber(fd, VSTUP, 1) {
            volne.push(i);
        }
        // hotové snímky
        if let Some(vy) = vystupy.as_mut() {
            loop {
                let (i, priznaky, _, cas) = match vyber(fd, VYSTUP, vy.bufy[0].mapy.len()) {
                    Ok(Some(x)) => x,
                    Ok(None) => break,
                    Err(e) => {
                        chyba = Some(e);
                        break 'hlavna;
                    }
                };
                let posledny = priznaky & BUF_POSLEDNY != 0;
                let buf = &mut vy.bufy[i as usize];
                let obsah = buf.dlzky[0] > 0 && priznaky & BUF_CHYBA == 0;
                if obsah && v.snimok < cakane.len() {
                    v.rozmer = (vy.sirka, vy.vyska);
                    let c = &cakane[v.snimok];
                    let (y, cb, cr) =
                        rozbal_nv12(buf, c.sirka, c.vyska, vy.riadok_y as usize, vy.riadok_uv as usize);
                    let (chyb, prva) = porovnaj(c, &y, &cb, &cr);
                    // R225: najbližší vzor (pri zhode vlastný index)
                    let najblizsi = if chyb == 0 {
                        (v.snimok, 0)
                    } else {
                        cakane
                            .iter()
                            .enumerate()
                            .filter(|(_, o)| o.sirka == c.sirka && o.vyska == c.vyska)
                            .map(|(k, o)| (k, porovnaj(o, &y, &cb, &cr).0))
                            .min_by_key(|&(_, n)| n)
                            .unwrap_or((v.snimok, chyb))
                    };
                    if v.poradie.len() < 16 {
                        v.poradie.push((najblizsi.0, najblizsi.1, cas));
                    }
                    if chyb == 0 {
                        v.presnych += 1;
                    } else if v.prva_chyba.is_none() {
                        // R225: ako vyzerá chybná snímka – nezapísaná (samé 0/255) alebo iný obsah
                        let n = y.len().max(1) as f64;
                        let priemer = y.iter().map(|&b| b as f64).sum::<f64>() / n;
                        let nuly = y.iter().filter(|&&b| b == 0).count() as f64 / n * 100.0;
                        let plne = y.iter().filter(|&&b| b == 255).count() as f64 / n * 100.0;
                        let riadkov_ok = (0..c.vyska)
                            .take_while(|&r| {
                                y[r * c.sirka..(r + 1) * c.sirka] == c.y[r * c.sirka..(r + 1) * c.sirka]
                            })
                            .count();
                        // R237: ktoré riadky v rámci makrobloku sú chybné (Y 0–15, Cb 0–7)
                        let zle_y = crate::video::chybne_riadky_bloku(&c.y, &y, c.sirka, 16);
                        let zle_cb = crate::video::chybne_riadky_bloku(&c.cb, &cb, c.sirka / 2, 8);
                        // R237: pôvod prvých chybných riadkov jasu (stará snímka? posunutý riadok? stála hodnota?)
                        let povod: Vec<String> = (0..c.vyska.min(y.len() / c.sirka.max(1)))
                            .filter(|&r| {
                                y[r * c.sirka..(r + 1) * c.sirka] != c.y[r * c.sirka..(r + 1) * c.sirka]
                            })
                            .take(6)
                            .map(|r| {
                                let p =
                                    crate::video::povod_riadku(cakane, &y[r * c.sirka..(r + 1) * c.sirka]);
                                format!("r{r}={p}")
                            })
                            .collect();
                        // R239: ukážka prvého chybného riadka jasu – 8 bodov od prvého rozdielu (čakané → prišlo)
                        let ukazka = (0..c.vyska.min(y.len() / c.sirka.max(1)))
                            .find_map(|r| {
                                let (zr, sr) = (
                                    &c.y[r * c.sirka..(r + 1) * c.sirka],
                                    &y[r * c.sirka..(r + 1) * c.sirka],
                                );
                                let st = (0..c.sirka).find(|&i| zr[i] != sr[i])?;
                                let k = (st + 8).min(c.sirka);
                                let cisla =
                                    |v: &[u8]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
                                Some(format!("r{r} s{st}: {} → {}", cisla(&zr[st..k]), cisla(&sr[st..k])))
                            })
                            .unwrap_or_default();
                        v.prva_chyba = prva.map(|(rov, r, s, a, b)| {
                            format!(
                                "snímka {} {rov} riadok {r} stĺpec {s}: {a} → {b} ({chyb} bodov; Y priemer {priemer:.0}, \
                                 nuly {nuly:.0} %, 255 {plne:.0} %, presných riadkov zhora {riadkov_ok}; chybné riadky \
                                 makroblokov Y {zle_y}, Cb {zle_cb}; pôvod riadkov (táto = s{}) {}; ukážka {ukazka}; riadok Y {} B, UV {} B)",
                                v.snimok + 1,
                                v.snimok + 1,
                                povod.join(" "),
                                vy.riadok_y,
                                vy.riadok_uv
                            )
                        });
                    }
                    v.snimok += 1;
                    v.ms = start.elapsed().as_millis() as u64;
                }
                if posledny {
                    break 'hlavna;
                }
                let _ = zarad(fd, VYSTUP, i, buf, &[], 0);
            }
        }
    }
    let _ = prud(fd, VSTUP, false);
    let _ = prud(fd, VYSTUP, false);
    drop(vystupy);
    drop(vstupy);
    drop(subor);
    match chyba {
        Some(e) if v.snimok == 0 => Err(e),
        _ => Ok(v),
    }
}

/// NV12 (Y a striedavé Cb/Cr) s dĺžkou riadkov `riadok_*` → tri roviny bez zarovnania.
fn rozbal_nv12(
    buf: &mut Buf,
    sirka: usize,
    vyska: usize,
    riadok_y: usize,
    riadok_uv: usize,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = Vec::with_capacity(sirka * vyska);
    let mut cb = Vec::with_capacity(sirka * vyska / 4);
    let mut cr = Vec::with_capacity(sirka * vyska / 4);
    let jedna_rovina = buf.mapy.len() == 1;
    let (rov_y, rov_uv) = if jedna_rovina {
        let vsetko = buf.mapy[0].bajty();
        let (a, b) = vsetko.split_at(riadok_y * vyska);
        (a.to_vec(), b.to_vec())
    } else {
        (buf.mapy[0].bajty().to_vec(), buf.mapy[1].bajty().to_vec())
    };
    for r in 0..vyska {
        if let Some(riadok) = rov_y.get(r * riadok_y..r * riadok_y + sirka) {
            y.extend_from_slice(riadok);
        }
    }
    for r in 0..vyska / 2 {
        if let Some(riadok) = rov_uv.get(r * riadok_uv..r * riadok_uv + sirka) {
            for p in riadok.chunks_exact(2) {
                cb.push(p[0]);
                cr.push(p[1]);
            }
        }
    }
    (y, cb, cr)
}
