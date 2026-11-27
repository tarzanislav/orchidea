//! R226: zvuk cez HDMI – vlastný zápis do ALSA (rozhranie jadra `/dev/snd/pcmC*D*p`) bez knižníc.
//!
//! Prvý krok k prehrávaču LÝRA na krabičkách: KLÍČ pri štarte zahrá krátky tón (1 kHz, −20 dBFS, 1,5 s
//! s nábehom a dobehom) a do záznamu zapíše, čo zvuková karta prijala – formát, takt vzoriek, veľkosť buffra,
//! zapísané rámce, čas a podtečenia. Čas zápisu ≈ 1,5 s dokazuje, že zvuková karta naozaj odoberá vzorky
//! taktom 48 kHz (DMA beží); či tón počuť, potvrdí človek pri televízore. Vypína ho prepínač `zvuk_hdmi=nie`.
//!
//! Formáty podľa toho, čo ovládač dovolí: 16-bit, 32-bit alebo rámce IEC 60958 (`IEC958_SUBFRAME_LE` – vyžaduje
//! ho HDMI ovládač vc4 na RPi): 24-bit vzorka v bitoch 4–27, bit platnosti, používateľský bit, bit stavu kanála
//! a parita; stav kanála podľa IEC 60958-3 (spotrebiteľský formát, lineárne PCM, 48 kHz, 24 bitov), preambula
//! bloku na každom 192. rámci.

use std::ffi::c_ulong;
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

use crate::sys;

const fn io(smer: u32, nr: u32, velkost: usize) -> c_ulong {
    ((smer << 30) | ((velkost as u32) << 16) | ((b'A' as u32) << 8) | nr) as c_ulong
}
const PIS: u32 = 1;
const CITAJ: u32 = 2;
const CITAJ_PIS: u32 = 3;

/// Maska parametra (256 bitov).
#[repr(C)]
#[derive(Clone, Copy)]
struct Maska {
    bity: [u32; 8],
}

/// Interval parametra; príznaky: openmin, openmax, integer, empty (bity 0–3).
#[repr(C)]
#[derive(Clone, Copy)]
struct Interval {
    min: u32,
    max: u32,
    priznaky: u32,
}

/// `struct snd_pcm_hw_params` (include/uapi/sound/asound.h).
#[repr(C)]
#[derive(Clone, Copy)]
struct HwParams {
    flags: u32,
    masky: [Maska; 3],
    _mres: [Maska; 5],
    intervaly: [Interval; 12],
    _ires: [Interval; 9],
    rmask: u32,
    cmask: u32,
    info: u32,
    msbits: u32,
    rate_num: u32,
    rate_den: u32,
    fifo_size: std::ffi::c_ulong,
    _vyhradene: [u8; 64],
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<HwParams>() == 608);
#[cfg(target_pointer_width = "32")]
const _: () = assert!(size_of::<HwParams>() == 604);

/// `struct snd_xferi`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Xferi {
    vysledok: std::ffi::c_long,
    buf: *mut std::ffi::c_void,
    ramcov: std::ffi::c_ulong,
}

/// `struct snd_pcm_sw_params` – R229: štart až pri plnom buffri (nie po prvom rámci).
#[repr(C)]
#[derive(Clone, Copy)]
struct SwParams {
    tstamp_mode: i32,
    period_step: u32,
    sleep_min: u32,
    avail_min: c_ulong,
    xfer_align: c_ulong,
    start_threshold: c_ulong,
    stop_threshold: c_ulong,
    silence_threshold: c_ulong,
    silence_size: c_ulong,
    boundary: c_ulong,
    proto: u32,
    tstamp_type: u32,
    _vyhradene: [u8; 56],
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<SwParams>() == 136);
#[cfg(target_pointer_width = "32")]
const _: () = assert!(size_of::<SwParams>() == 104);

const HW_PARAMS: c_ulong = io(CITAJ_PIS, 0x11, size_of::<HwParams>());
const SW_PARAMS: c_ulong = io(CITAJ_PIS, 0x13, size_of::<SwParams>());
const PREPARE: c_ulong = io(0, 0x40, 0);
const DRAIN: c_ulong = io(0, 0x44, 0);
const WRITEI: c_ulong = io(PIS, 0x50, size_of::<Xferi>());
const DELAY: c_ulong = io(CITAJ, 0x21, size_of::<std::ffi::c_long>());

// parametre (SNDRV_PCM_HW_PARAM_*)
const P_PRISTUP: usize = 0;
const P_FORMAT: usize = 1;
const P_PODFORMAT: usize = 2;
const P_PRVY_INTERVAL: usize = 8;
const P_KANALY: usize = 10;
const P_TAKT: usize = 11;
const P_PERIODA_CAS: usize = 12;
const P_PERIODA: usize = 13;
const P_BUFFER_CAS: usize = 16;
const P_BUFFER: usize = 17;

const PRISTUP_PREKLADANY: u32 = 3; // RW_INTERLEAVED
/// Formáty (SNDRV_PCM_FORMAT_*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    S16Le = 2,
    S32Le = 10,
    Iec958 = 18,
}

impl Format {
    fn bajtov(self) -> usize {
        match self {
            Format::S16Le => 2,
            _ => 4,
        }
    }
    fn popis(self) -> &'static str {
        match self {
            Format::S16Le => "16 bitov",
            Format::S32Le => "32 bitov",
            Format::Iec958 => "rámce IEC 60958",
        }
    }
}

pub const TAKT: u32 = 48_000;
const KANALY: u32 = 2;

/// Parametre zvukovej karty; `s_casom` = R229: perióda 10–50 ms a buffer 40–250 ms (inak vyberie ovládač –
/// pri vc4 to bola najmenšia perióda a hneď podtečenie).
fn hw_params(format: Format, s_casom: bool) -> HwParams {
    let plna = Maska { bity: [u32::MAX; 8] };
    let mut p = HwParams {
        flags: 0,
        masky: [plna; 3],
        _mres: [plna; 5],
        intervaly: [Interval { min: 0, max: u32::MAX, priznaky: 0 }; 12],
        _ires: [Interval { min: 0, max: u32::MAX, priznaky: 0 }; 9],
        rmask: u32::MAX,
        cmask: 0,
        info: 0,
        msbits: 0,
        rate_num: 0,
        rate_den: 0,
        fifo_size: 0,
        _vyhradene: [0; 64],
    };
    let jedna = |bit: u32| {
        let mut m = Maska { bity: [0; 8] };
        m.bity[(bit / 32) as usize] = 1 << (bit % 32);
        m
    };
    p.masky[P_PRISTUP] = jedna(PRISTUP_PREKLADANY);
    p.masky[P_FORMAT] = jedna(format as u32);
    p.masky[P_PODFORMAT] = jedna(0);
    p.intervaly[P_KANALY - P_PRVY_INTERVAL] = Interval { min: KANALY, max: KANALY, priznaky: 0b100 };
    p.intervaly[P_TAKT - P_PRVY_INTERVAL] = Interval { min: TAKT, max: TAKT, priznaky: 0b100 };
    if s_casom {
        // perióda okolo 10 ms a buffer do 250 ms – dosť na plynulosť, málo na oneskorenie (µs)
        p.intervaly[P_PERIODA_CAS - P_PRVY_INTERVAL] = Interval { min: 10_000, max: 50_000, priznaky: 0 };
        p.intervaly[P_BUFFER_CAS - P_PRVY_INTERVAL] = Interval { min: 40_000, max: 250_000, priznaky: 0 };
    }
    p
}

/// R229: čo hlási jadro o prúde (`/proc/asound/cardN/pcmDp/sub0/status`, `hw_params`) – pri chybe do záznamu.
fn stav_prudu(zariadenie: &str) -> String {
    let Some((karta, dev)) =
        zariadenie.strip_prefix("pcmC").and_then(|z| z.strip_suffix('p')).and_then(|z| z.split_once('D'))
    else {
        return String::new();
    };
    let cesta = format!("/proc/asound/card{karta}/pcm{dev}p/sub0");
    let mut casti = Vec::new();
    for (subor, kluce) in [
        ("status", &["state", "hw_ptr", "appl_ptr", "avail", "delay"][..]),
        ("hw_params", &["format", "period_size", "buffer_size"][..]),
    ] {
        let t = std::fs::read_to_string(format!("{cesta}/{subor}")).unwrap_or_default();
        for r in t.lines() {
            if let Some((k, v)) = r.split_once(':')
                && kluce.contains(&k.trim())
            {
                casti.push(format!("{}={}", k.trim(), v.trim()));
            }
        }
    }
    casti.join(" ")
}

/// R229: nové hlásenia jadra o zvuku a HDMI počas skúšky (vlastné čítanie `/dev/kmsg` od konca).
struct HlaseniaJadra(Option<std::fs::File>);

impl HlaseniaJadra {
    fn od_teraz() -> Self {
        use std::io::Seek;
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(0o4000) // O_NONBLOCK – na konci záznamu vráti EAGAIN
            .open("/dev/kmsg")
            .ok()
            .and_then(|mut f| f.seek(std::io::SeekFrom::End(0)).ok().map(|_| f));
        HlaseniaJadra(f)
    }

    fn nove(&mut self) -> Vec<String> {
        use std::io::Read;
        const KLUCE: [&str; 10] =
            ["hdmi", "vc4", "audio", "snd", "asoc", "dma", "tdm", "frddr", "i2s", "pcm"];
        let mut v = Vec::new();
        let Some(f) = self.0.as_mut() else { return v };
        let mut buf = [0u8; 8192];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let z = String::from_utf8_lossy(&buf[..n]);
                    let Some((_, text)) = z.split_once(';') else { continue };
                    let text = text.lines().next().unwrap_or("").trim();
                    let mala = text.to_lowercase();
                    // R245: vlastné hlásenia KLÍČA (napr. skúšky videa v tom istom čase) nie sú hlásenia zvuku
                    let klic = text.starts_with("KLÍČ") || text.starts_with("KL\\x");
                    if !klic && KLUCE.iter().any(|k| mala.contains(k)) && v.len() < 12 {
                        v.push(text.chars().take(160).collect());
                    }
                }
                Err(e) if e.raw_os_error() == Some(32) => continue,
                Err(_) => break,
            }
        }
        v
    }
}

/// Stav kanála IEC 60958-3 (spotrebiteľský): lineárne PCM, bez ochrany kopírovania, 48 kHz, 24-bit slovo.
pub fn stav_kanala() -> [u8; 24] {
    let mut c = [0u8; 24];
    c[0] = 0b0000_0100; // spotrebiteľský, zvuk (PCM), kopírovanie povolené, bez predzdôraznenia
    c[1] = 0; // kategória: všeobecná
    c[2] = 0; // zdroj a číslo kanála neuvedené
    c[3] = 0b0000_0010; // vzorkovanie 48 kHz, presnosť taktu úroveň II
    c[4] = 0b0000_1011; // dĺžka slova: najviac 24 bitov, 24 bitov
    c
}

/// Jeden podrámec IEC 60958 (formát ALSA IEC958_SUBFRAME_LE) zo 24-bit vzorky.
pub fn podramec(vzorka24: i32, ramec: usize, pravy: bool, stav: &[u8; 24]) -> u32 {
    let preambula: u32 = match (ramec.is_multiple_of(192), pravy) {
        (_, true) => 0x4,      // W (Y) – pravý kanál
        (true, false) => 0x8,  // B (Z) – začiatok bloku
        (false, false) => 0x2, // M (X)
    };
    let bit_stavu = ((stav[(ramec % 192) / 8] >> (ramec % 8)) & 1) as u32;
    let mut s = preambula | ((vzorka24 as u32 & 0x00FF_FFFF) << 4) | (bit_stavu << 30);
    // parita: bity 4–31 spolu párne
    if !(s >> 4).count_ones().is_multiple_of(2) {
        s |= 1 << 31;
    }
    s
}

/// Tón 1 kHz, −20 dBFS, s nábehom a dobehom 20 ms (bez cvaknutia), ako 24-bit vzorky (rovnaké v oboch kanáloch).
pub fn ton(ramcov: usize) -> Vec<i32> {
    let amplituda = 0.1 * 8_388_607.0; // −20 dBFS
    let nabeh = (TAKT as usize) / 50;
    (0..ramcov)
        .map(|i| {
            let obal = (i.min(ramcov - 1 - i) as f64 / nabeh as f64).min(1.0);
            let t = i as f64 / TAKT as f64;
            (amplituda * obal * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()).round() as i32
        })
        .collect()
}

fn bajty(format: Format, vzorky: &[i32]) -> Vec<u8> {
    let stav = stav_kanala();
    let mut v = Vec::with_capacity(vzorky.len() * KANALY as usize * format.bajtov());
    for (i, &s) in vzorky.iter().enumerate() {
        for pravy in [false, true] {
            match format {
                Format::S16Le => v.extend_from_slice(&((s >> 8) as i16).to_le_bytes()),
                Format::S32Le => v.extend_from_slice(&(s << 8).to_le_bytes()),
                Format::Iec958 => v.extend_from_slice(&podramec(s, i, pravy, &stav).to_le_bytes()),
            }
        }
    }
    v
}

/// Výstupné zariadenia PCM (karty zvuku).
pub fn zariadenia() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir("/dev/snd")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|z| {
            let m = z.file_name().to_string_lossy().to_string();
            let karta = m.strip_prefix("pcmC")?.split('D').next()?.to_string();
            m.ends_with('p').then_some((m.clone(), karta))
        })
        .map(|(m, karta)| {
            let meno = std::fs::read_to_string(format!("/proc/asound/card{karta}/id")).unwrap_or_default();
            (m, meno.trim().to_string())
        })
        .collect();
    v.sort();
    v
}

/// Zahrá skúšobný tón; vráti popis pre záznam (pri chybe aj stav prúdu a hlásenia jadra – R229).
pub fn skuska_tonu(zariadenie: &str) -> Result<String, String> {
    let mut jadro = HlaseniaJadra::od_teraz();
    let v = skuska_tonu_vnutro(zariadenie);
    let hlasenia = jadro.nove();
    let pridaj = |t: String| {
        if hlasenia.is_empty() { t } else { format!("{t} · jadro: {}", hlasenia.join(" | ")) }
    };
    v.map(pridaj).map_err(pridaj)
}

fn skuska_tonu_vnutro(zariadenie: &str) -> Result<String, String> {
    // blokujúci režim: zápis čaká, kým zvuková karta vezme dáta (zariadenie nikto iný nepoužíva)
    let f = OpenOptions::new()
        .write(true)
        .custom_flags(0o2000000) // O_CLOEXEC
        .open(format!("/dev/snd/{zariadenie}"))
        .map_err(|e| format!("{zariadenie} sa nedá otvoriť ({e})"))?;
    let fd = f.as_raw_fd();
    let mut zvoleny = None;
    let mut chyby = Vec::new();
    // najprv s časmi periódy a buffra (R229), potom bez nich – nech rozhodne ovládač
    'hladanie: for s_casom in [true, false] {
        for format in [Format::S16Le, Format::S32Le, Format::Iec958] {
            let mut p = hw_params(format, s_casom);
            match sys::ioctl_raz(fd, HW_PARAMS, &mut p) {
                Ok(()) => {
                    zvoleny = Some((format, p, s_casom));
                    break 'hladanie;
                }
                Err(e) => {
                    chyby.push(format!("{}{}: {e}", format.popis(), if s_casom { " s časmi" } else { "" }))
                }
            }
        }
    }
    let (format, p, s_casom) =
        zvoleny.ok_or_else(|| format!("ovládač neprijal formát ({})", chyby.join(", ")))?;
    let iv = |i: usize| p.intervaly[i - P_PRVY_INTERVAL].min;
    let buffer = iv(P_BUFFER);
    let perioda = iv(P_PERIODA);
    // R229: štart až pri plnom buffri, zastavenie až pri prázdnom
    let mut sw = SwParams {
        tstamp_mode: 0,
        period_step: 1,
        sleep_min: 0,
        avail_min: perioda.max(1) as c_ulong,
        xfer_align: 1,
        start_threshold: buffer as c_ulong,
        stop_threshold: buffer as c_ulong,
        silence_threshold: 0,
        silence_size: 0,
        boundary: 0,
        proto: 0,
        tstamp_type: 0,
        _vyhradene: [0; 56],
    };
    let sw_ok = sys::ioctl_raz(fd, SW_PARAMS, &mut sw).map_err(|e| e.to_string());
    let nastavenie = format!(
        "{} · {} Hz · 2 kanály · buffer {buffer} rámcov ({:.0} ms), perióda {perioda}{} · štart {}",
        format.popis(),
        TAKT,
        buffer as f64 * 1000.0 / TAKT as f64,
        if s_casom { "" } else { " (časy podľa ovládača)" },
        match &sw_ok {
            Ok(()) => "pri plnom buffri".to_string(),
            Err(e) => format!("po prvom rámci (sw_params: {e})"),
        }
    );
    let mut nic = 0u32;
    sys::ioctl_raz(fd, PREPARE, &mut nic).map_err(|e| format!("{nastavenie} · príprava zlyhala ({e})"))?;
    let ramcov = (TAKT as usize) * 3 / 2;
    let mut data = bajty(format, &ton(ramcov));
    let na_ramec = KANALY as usize * format.bajtov();
    let mut zapisanych = 0usize;
    let mut podtecenia = 0u32;
    let mut prve_podtecenie: Option<(u128, usize, String)> = None;
    // R5c: po každej perióde (čas, prehraté rámce = zapísané − oneskorenie v zariadení) → skutočný takt zvuku
    let mut body: Vec<(f64, f64)> = Vec::new();
    let mut oneskorenie_max = 0i64;
    let kus = (perioda as usize).clamp(64, 4096);
    let start = std::time::Instant::now();
    while zapisanych < ramcov {
        let zvysok = &mut data[zapisanych * na_ramec..];
        let mut x = Xferi {
            vysledok: 0,
            buf: zvysok.as_mut_ptr() as *mut std::ffi::c_void,
            ramcov: (ramcov - zapisanych).min(kus) as std::ffi::c_ulong,
        };
        match sys::ioctl_struct(fd, WRITEI, &mut x) {
            Ok(()) if x.vysledok > 0 => {
                zapisanych += x.vysledok as usize;
                let mut d: std::ffi::c_long = 0;
                if sys::ioctl_raz(fd, DELAY, &mut d).is_ok() && podtecenia == 0 {
                    let d = d as i64;
                    oneskorenie_max = oneskorenie_max.max(d);
                    body.push((start.elapsed().as_secs_f64(), zapisanych as f64 - d as f64));
                }
            }
            Ok(()) => break,
            Err(e) if e.raw_os_error() == Some(32) => {
                // EPIPE = podtečenie – pripraviť znova a pokračovať
                podtecenia += 1;
                if prve_podtecenie.is_none() {
                    prve_podtecenie = Some((start.elapsed().as_millis(), zapisanych, stav_prudu(zariadenie)));
                }
                let pripravene = sys::ioctl_raz(fd, PREPARE, &mut nic);
                if podtecenia > 5 {
                    let (ms, ram, stav) = prve_podtecenie.take().unwrap_or_default();
                    return Err(format!(
                        "{nastavenie} · opakované podtečenie po {zapisanych} rámcoch (prvé po {ms} ms a {ram} rámcoch; \
                         stav prúdu: {stav}; nová príprava: {}; teraz: {})",
                        pripravene.map_or_else(|e| format!("zlyhala ({e})"), |_| "ok".into()),
                        stav_prudu(zariadenie)
                    ));
                }
            }
            Err(e) => {
                return Err(format!(
                    "{nastavenie} · zápis zlyhal po {zapisanych} rámcoch ({e}; stav prúdu: {})",
                    stav_prudu(zariadenie)
                ));
            }
        }
    }
    let _ = sys::ioctl_raz(fd, DRAIN, &mut nic);
    let takt = namerany_takt(&body)
        .map_or("takt nezmeraný".into(), |(t, n)| format!("nameraný takt {t:.0} Hz ({n} bodov)"));
    let prve = prve_podtecenie
        .map(|(ms, ram, stav)| format!(" (prvé po {ms} ms a {ram} rámcoch; {stav})"))
        .unwrap_or_default();
    Ok(format!(
        "tón 1 kHz −20 dBFS 1,5 s: {nastavenie} · zapísaných {zapisanych} rámcov za {} ms · podtečení \
         {podtecenia}{prve} · {takt} · oneskorenie v zariadení najviac {oneskorenie_max} rámcov ({:.0} ms)",
        start.elapsed().as_millis(),
        oneskorenie_max as f64 * 1000.0 / TAKT as f64
    ))
}

/// Takt zvuku zo smernice priamky (čas → prehraté rámce), metóda najmenších štvorcov; prvá štvrtina bodov
/// sa vynechá (rozbeh, plnenie buffra). Vráti (rámce/s, počet bodov).
pub fn namerany_takt(body: &[(f64, f64)]) -> Option<(f64, usize)> {
    let b = &body[body.len() / 4..];
    if b.len() < 8 {
        return None;
    }
    let n = b.len() as f64;
    let (st, sr) = b.iter().fold((0.0, 0.0), |(a, c), &(t, r)| (a + t, c + r));
    let (mt, mr) = (st / n, sr / n);
    let (cit, men) =
        b.iter().fold((0.0, 0.0), |(c, m), &(t, r)| (c + (t - mt) * (r - mr), m + (t - mt) * (t - mt)));
    (men > 0.0).then(|| (cit / men, b.len()))
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn podramec_iec60958() {
        let st = stav_kanala();
        // začiatok bloku: preambula B, prvý bit stavu kanála (spotrebiteľský = 0)
        let s = podramec(0, 0, false, &st);
        assert_eq!(s & 0xF, 0x8);
        assert_eq!((s >> 30) & 1, 0);
        // pravý kanál: preambula W
        assert_eq!(podramec(0, 0, true, &st) & 0xF, 0x4);
        assert_eq!(podramec(0, 1, false, &st) & 0xF, 0x2);
        // bit 2 stavu kanála (kopírovanie povolené) = 1 v rámci 2
        assert_eq!((podramec(0, 2, false, &st) >> 30) & 1, 1);
        // parita: bity 4–31 párne
        for v in [0, 1, -1, 0x7FFFFF, -0x800000, 12345] {
            for r in 0..200 {
                let s = podramec(v, r, r % 2 == 1, &st);
                assert_eq!((s >> 4).count_ones() % 2, 0, "{v} {r}");
                assert_eq!(((s >> 4) & 0xFF_FFFF) as i32, v & 0xFF_FFFF);
            }
        }
        // 48 kHz a 24 bitov v stave kanála
        assert_eq!(st[3] & 0x0F, 0b0010);
        assert_eq!(st[4] & 0x0F, 0b1011);
    }

    #[test]
    fn takt_zo_smernice() {
        // 48 kHz s periódami po 1024 rámcoch (prehraté skáču po perióde) a rozbehom na začiatku
        let mut body = vec![(0.001, 0.0), (0.002, 0.0)];
        for i in 1..70 {
            let t = i as f64 * 1024.0 / 48_000.0 + 0.0003;
            body.push((t, (i as f64 * 1024.0).floor()));
        }
        let (takt, n) = namerany_takt(&body).unwrap();
        assert!((takt - 48_000.0).abs() < 5.0, "{takt}");
        assert!(n > 40);
        assert_eq!(namerany_takt(&body[..6]), None);
    }

    #[test]
    fn ton_bez_cvaknutia() {
        let t = ton(TAKT as usize);
        assert_eq!(t[0], 0);
        assert!(t[t.len() - 1].abs() < 2000);
        let max = t.iter().map(|v| v.abs()).max().unwrap();
        // −20 dBFS z 24 bitov
        assert!((838_000..=838_861).contains(&max), "{max}");
        // 16-bit: horných 16 bitov
        let b = bajty(Format::S16Le, &t[..10]);
        assert_eq!(b.len(), 10 * 2 * 2);
    }
}
