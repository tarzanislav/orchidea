//! Disky na Windows cez vlastné väzby na kernel32 (CreateFileW, DeviceIoControl, ReadFile, WriteFile).
//! Jediné miesto SADIČA s nebezpečným kódom.

#![allow(unsafe_code)]

use crate::disk::{InfoDisku, Zbernica};
use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::BTreeSet;
use std::ffi::c_void;
use std::io;

type Handle = *mut c_void;
const NEPLATNY: Handle = -1isize as Handle;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        meno: *const u16,
        pristup: u32,
        zdielanie: u32,
        sa: *mut c_void,
        vytvorenie: u32,
        priznaky: u32,
        vzor: Handle,
    ) -> Handle;
    fn DeviceIoControl(
        h: Handle,
        kod: u32,
        vstup: *const c_void,
        vstup_dlzka: u32,
        vystup: *mut c_void,
        vystup_dlzka: u32,
        vratene: *mut u32,
        prekryv: *mut c_void,
    ) -> i32;
    fn ReadFile(h: Handle, buf: *mut c_void, n: u32, precitane: *mut u32, prekryv: *mut c_void) -> i32;
    fn WriteFile(h: Handle, buf: *const c_void, n: u32, zapisane: *mut u32, prekryv: *mut c_void) -> i32;
    fn SetFilePointerEx(h: Handle, vzdialenost: i64, nova: *mut i64, sposob: u32) -> i32;
    fn FlushFileBuffers(h: Handle) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn FindFirstVolumeW(meno: *mut u16, dlzka: u32) -> Handle;
    fn FindNextVolumeW(h: Handle, meno: *mut u16, dlzka: u32) -> i32;
    fn FindVolumeClose(h: Handle) -> i32;
    fn GetSystemWindowsDirectoryW(buf: *mut u16, dlzka: u32) -> u32;
}

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(algoritmus: *mut c_void, buf: *mut u8, dlzka: u32, priznaky: u32) -> i32;
}

/// R247: náhodné bajty zo systémového generátora Windows (BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG).
pub fn nahodne(b: &mut [u8]) -> io::Result<()> {
    // SAFETY: buffer `b` je platný a zapisovateľný v celej dĺžke; algoritmus NULL so systémovým generátorom.
    let r = unsafe { BCryptGenRandom(std::ptr::null_mut(), b.as_mut_ptr(), b.len() as u32, 2) };
    if r == 0 { Ok(()) } else { Err(io::Error::other(format!("BCryptGenRandom zlyhal (0x{r:08x})"))) }
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const ZDIELAJ_VSETKO: u32 = 1 | 2;
const OPEN_EXISTING: u32 = 3;
const BEZ_VYROVNAVANIA: u32 = 0x2000_0000 | 0x8000_0000; // NO_BUFFERING | WRITE_THROUGH

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00A0;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
const FSCTL_LOCK_VOLUME: u32 = 0x0009_0018;
const FSCTL_DISMOUNT_VOLUME: u32 = 0x0009_0020;
const IOCTL_DISK_UPDATE_PROPERTIES: u32 = 0x0007_0140;
const IOCTL_DISK_CREATE_DISK: u32 = 0x0007_C058;

/// Najväčší kus jedného zápisu alebo čítania.
pub const KUS: usize = 4 << 20;

fn siroky(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

struct Rukovat(Handle);

impl Drop for Rukovat {
    fn drop(&mut self) {
        // SAFETY: platná rukoväť z CreateFileW, zatvára sa práve raz.
        unsafe { CloseHandle(self.0) };
    }
}

fn otvor(meno: &str, pristup: u32, priznaky: u32) -> io::Result<Rukovat> {
    let w = siroky(meno);
    // SAFETY: meno je ukončený reťazec UTF-16; ostatné parametre sú hodnoty alebo NULL.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            pristup,
            ZDIELAJ_VSETKO,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            priznaky,
            std::ptr::null_mut(),
        )
    };
    if h == NEPLATNY { Err(io::Error::last_os_error()) } else { Ok(Rukovat(h)) }
}

fn ioctl(h: &Rukovat, kod: u32, vstup: &[u8], vystup: &mut [u8]) -> io::Result<usize> {
    let mut vratene = 0u32;
    // SAFETY: vstupný aj výstupný buffer sú platné po celej uvedenej dĺžke.
    let ok = unsafe {
        DeviceIoControl(
            h.0,
            kod,
            vstup.as_ptr().cast(),
            vstup.len() as u32,
            vystup.as_mut_ptr().cast(),
            vystup.len() as u32,
            &mut vratene,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(vratene as usize) }
}

fn u32_na(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_na(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bajtov"))
}

/// Čísla diskov, na ktorých leží zväzok (zväzok môže siahať na viac diskov).
fn disky_zvazku(meno: &str) -> BTreeSet<u32> {
    let Ok(h) = otvor(meno, 0, 0) else { return BTreeSet::new() };
    let mut out = vec![0u8; 1024];
    if ioctl(&h, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], &mut out).is_err() {
        return BTreeSet::new();
    }
    let n = u32_na(&out, 0) as usize;
    (0..n.min(40)).map(|i| u32_na(&out, 8 + 24 * i)).collect()
}

/// Disky, na ktorých je Windows (zväzok so systémovým priečinkom).
fn systemove_disky() -> BTreeSet<u32> {
    let mut buf = [0u16; 260];
    // SAFETY: buffer má uvedenú dĺžku.
    let n = unsafe { GetSystemWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let priecinok = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    match priecinok.get(..2) {
        Some(pismeno) => disky_zvazku(&format!(r"\\.\{pismeno}")),
        // ak sa systémový disk nedá zistiť, za systémový sa považuje každý (nič sa neponúkne)
        None => (0..64).collect(),
    }
}

/// Všetky zväzky (oddiely s písmenom aj bez neho) na disku.
fn zvazky_na_disku(cislo: u32) -> Vec<String> {
    let mut out = Vec::new();
    let mut meno = [0u16; 260];
    // SAFETY: buffer má uvedenú dĺžku; rukoväť hľadania sa zatvorí nižšie.
    let h = unsafe { FindFirstVolumeW(meno.as_mut_ptr(), meno.len() as u32) };
    if h == NEPLATNY {
        return out;
    }
    loop {
        let dlzka = meno.iter().position(|&c| c == 0).unwrap_or(meno.len());
        let m = String::from_utf16_lossy(&meno[..dlzka]);
        let bez_lomky = m.trim_end_matches('\\').to_string();
        if disky_zvazku(&bez_lomky).contains(&cislo) {
            out.push(bez_lomky);
        }
        // SAFETY: platná rukoväť hľadania a buffer s uvedenou dĺžkou.
        if unsafe { FindNextVolumeW(h, meno.as_mut_ptr(), meno.len() as u32) } == 0 {
            break;
        }
    }
    // SAFETY: rukoväť z FindFirstVolumeW.
    unsafe { FindVolumeClose(h) };
    out
}

fn citaj_info(cislo: u32, h: &Rukovat, systemove: &BTreeSet<u32>) -> io::Result<(InfoDisku, u32)> {
    let dotaz = [0u8; 12]; // StorageDeviceProperty, PropertyStandardQuery
    let mut opis = vec![0u8; 1024];
    let n = ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, &dotaz, &mut opis)?;
    let text = |posun: u32| -> String {
        let p = posun as usize;
        if p == 0 || p >= n {
            return String::new();
        }
        let koniec = opis[p..n].iter().position(|&c| c == 0).map_or(n, |k| p + k);
        String::from_utf8_lossy(&opis[p..koniec]).trim().to_string()
    };
    let model = [text(u32_na(&opis, 12)), text(u32_na(&opis, 16))]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut geo = vec![0u8; 256];
    let (velkost, sektor) = match ioctl(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut geo) {
        Ok(_) => (u64_na(&geo, 24), u32_na(&geo, 20).max(512)),
        Err(_) => (0, 512), // médium nie je vložené
    };
    Ok((
        InfoDisku {
            cislo,
            model: if model.is_empty() { "neznámy disk".into() } else { model },
            velkost,
            zbernica: Zbernica::z_windows(u32_na(&opis, 28)),
            vymenitelny: opis[10] != 0,
            systemovy: systemove.contains(&cislo),
        },
        sektor,
    ))
}

/// Všetky fyzické disky počítača.
pub fn disky() -> Vec<InfoDisku> {
    let systemove = systemove_disky();
    (0..64)
        .filter_map(|c| {
            let h = otvor(&format!(r"\\.\PhysicalDrive{c}"), 0, 0).ok()?;
            citaj_info(c, &h, &systemove).ok().map(|(i, _)| i)
        })
        .collect()
}

/// Buffer zarovnaný na stránku (zápis bez vyrovnávacej pamäte to vyžaduje).
struct Zarovnany {
    ptr: *mut u8,
    vrstva: Layout,
}

impl Zarovnany {
    fn novy() -> Zarovnany {
        let vrstva = Layout::from_size_align(KUS, 4096).expect("platné rozloženie");
        // SAFETY: rozloženie má nenulovú veľkosť.
        let ptr = unsafe { alloc_zeroed(vrstva) };
        assert!(!ptr.is_null(), "nedostatok pamäte");
        Zarovnany { ptr, vrstva }
    }

    fn kus(&mut self, n: usize) -> &mut [u8] {
        assert!(n <= KUS);
        // SAFETY: ptr ukazuje na KUS inicializovaných bajtov, ktoré vlastní tento buffer.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, n) }
    }
}

impl Drop for Zarovnany {
    fn drop(&mut self) {
        // SAFETY: ptr bol alokovaný s týmto rozložením.
        unsafe { dealloc(self.ptr, self.vrstva) };
    }
}

/// Zopakuje operáciu, kým disk hlási, že nie je pripravený (ERROR_NOT_READY 21, ERROR_IO_DEVICE 1117) –
/// najviac asi 10 s.
fn znova(mut f: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut pokus = 0;
    loop {
        match f() {
            Err(e) if pokus < 40 && matches!(e.raw_os_error(), Some(21) | Some(1117)) => {
                pokus += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            r => return r,
        }
    }
}

/// Otvorený fyzický disk. Pri zápise drží uzamknuté a odpojené všetky jeho zväzky.
pub struct Disk {
    h: Rukovat,
    pub info: InfoDisku,
    pub sektor: u32,
    _zamky: Vec<Rukovat>,
    buf: Zarovnany,
    cislo: u32,
    na_zapis: bool,
}

impl Disk {
    /// Otvorí disk; pri `na_zapis` najprv uzamkne a odpojí jeho zväzky (Windows do nich potom nezapisuje).
    /// Ak Windows ešte znovu číta oddiely disku (napr. hneď po predchádzajúcom napálení), rukoväť otvorená
    /// v tej chvíli ostane „nepripravená“ – preto sa disk najprv skúšobne prečíta (pri zápise sa prvý
    /// sektor aj zapíše späť nezmenený) a pri chybe sa všetko zavrie a otvorí znova (najviac asi 20 s).
    pub fn otvor(cislo: u32, na_zapis: bool) -> io::Result<Disk> {
        let mut pokus = 0;
        loop {
            let mut d = Disk::otvor_raz(cislo, na_zapis)?;
            let n = d.sektor as usize;
            let skuska = d.citaj_buf(0, n, n).and_then(|_| if na_zapis { d.zapis_buf(0, n) } else { Ok(()) });
            match skuska {
                Err(e) if pokus < 40 && matches!(e.raw_os_error(), Some(21) | Some(1117) | Some(1167)) => {
                    drop(d);
                    pokus += 1;
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                _ => return Ok(d),
            }
        }
    }

    fn otvor_raz(cislo: u32, na_zapis: bool) -> io::Result<Disk> {
        let systemove = systemove_disky();
        let mut zamky = Vec::new();
        if na_zapis {
            // hneď po predchádzajúcom zápise Windows oddiely ešte len pripája – počká sa, kým je zoznam
            // zväzkov rovnaký dvakrát po sebe (najviac asi 3 s)
            let mut zvazky = zvazky_na_disku(cislo);
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(300));
                let znova = zvazky_na_disku(cislo);
                if znova == zvazky {
                    break;
                }
                zvazky = znova;
            }
            for z in zvazky {
                let h = otvor(&z, GENERIC_READ | GENERIC_WRITE, 0)?;
                let mut zamknute = false;
                for _ in 0..20 {
                    if ioctl(&h, FSCTL_LOCK_VOLUME, &[], &mut []).is_ok() {
                        zamknute = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                if !zamknute {
                    return Err(io::Error::other(format!(
                        "oddiel {z} sa používa (zatvorte okná a programy, ktoré z karty čítajú)"
                    )));
                }
                ioctl(&h, FSCTL_DISMOUNT_VOLUME, &[], &mut [])?;
                zamky.push(h);
            }
        }
        let pristup = if na_zapis { GENERIC_READ | GENERIC_WRITE } else { GENERIC_READ };
        let h = otvor(&format!(r"\\.\PhysicalDrive{cislo}"), pristup, BEZ_VYROVNAVANIA)?;
        let (info, sektor) = citaj_info(cislo, &h, &systemove)?;
        Ok(Disk { h, info, sektor, _zamky: zamky, buf: Zarovnany::novy(), cislo, na_zapis })
    }

    fn posun(&self, posun: u64) -> io::Result<()> {
        // SAFETY: platná rukoväť; nová poloha sa nevracia (NULL).
        if unsafe { SetFilePointerEx(self.h.0, posun as i64, std::ptr::null_mut(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Zapíše dáta od posunu (dĺžka sa doplní nulami na celé sektory). Ak Windows práve znovu číta tabuľku
    /// oddielov (napr. hneď po predchádzajúcom zápise), disk chvíľu hlási „nie je pripravené“ – vtedy sa
    /// pokus zopakuje.
    pub fn zapis(&mut self, posun: u64, data: &[u8]) -> io::Result<()> {
        let s = self.sektor as usize;
        let n = data.len().div_ceil(s) * s;
        let kus = self.buf.kus(n);
        kus[..data.len()].copy_from_slice(data);
        kus[data.len()..].fill(0);
        match znova(|| self.zapis_buf(posun, n)) {
            Err(e) if matches!(e.raw_os_error(), Some(21) | Some(1117)) => {
                self.otvor_znova()?;
                let zvazkov = zvazky_na_disku(self.cislo).len();
                znova(|| self.zapis_buf(posun, n)).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!(
                            "{e} (na {} MiB od začiatku disku; zväzkov na disku: {zvazkov})",
                            posun >> 20
                        ),
                    )
                })
            }
            r => r,
        }
    }

    /// Zruší tabuľku oddielov tak, ako ju vidí Windows (ako Rufus pred zápisom), a počká, kým zmiznú
    /// zväzky disku: počas zápisu potom Windows nič nepripája – ani oddiely, ktoré našiel na disku po
    /// predchádzajúcom napálení. Volá sa až po potvrdení a kontrole, že ide o ten istý disk.
    pub fn uvolni_oddiely(&mut self) {
        let mut vstup = [0u8; 24];
        vstup[0..4].copy_from_slice(&2u32.to_le_bytes()); // PARTITION_STYLE_RAW
        if ioctl(&self.h, IOCTL_DISK_CREATE_DISK, &vstup, &mut []).is_err() {
            return;
        }
        let _ = ioctl(&self.h, IOCTL_DISK_UPDATE_PROPERTIES, &[], &mut []);
        for _ in 0..20 {
            if zvazky_na_disku(self.cislo).is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    /// Zavrie disk aj zámky zväzkov a otvorí ho znova (Windows medzitým mohol pripojiť nové oddiely).
    fn otvor_znova(&mut self) -> io::Result<()> {
        self._zamky.clear();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let d = Disk::otvor_raz(self.cislo, self.na_zapis)?;
        self.h = d.h;
        self._zamky = d._zamky;
        Ok(())
    }

    fn zapis_buf(&mut self, posun: u64, n: usize) -> io::Result<()> {
        self.posun(posun)?;
        let mut zapisane = 0u32;
        // SAFETY: buffer je zarovnaný a má n bajtov.
        if unsafe { WriteFile(self.h.0, self.buf.ptr.cast(), n as u32, &mut zapisane, std::ptr::null_mut()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        if zapisane as usize != n {
            return Err(io::Error::other(format!("zapísaných {zapisane} z {n} bajtov")));
        }
        Ok(())
    }

    /// Prečíta `ciel.len()` bajtov od posunu.
    pub fn citaj(&mut self, posun: u64, ciel: &mut [u8]) -> io::Result<()> {
        let s = self.sektor as usize;
        let n = ciel.len().div_ceil(s) * s;
        znova(|| self.citaj_buf(posun, n, ciel.len()))?;
        let len = ciel.len();
        ciel.copy_from_slice(&self.buf.kus(n)[..len]);
        Ok(())
    }

    fn citaj_buf(&mut self, posun: u64, n: usize, treba: usize) -> io::Result<()> {
        self.posun(posun)?;
        let mut precitane = 0u32;
        // SAFETY: buffer je zarovnaný a má aspoň n bajtov.
        if unsafe { ReadFile(self.h.0, self.buf.ptr.cast(), n as u32, &mut precitane, std::ptr::null_mut()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        if (precitane as usize) < treba {
            return Err(io::Error::other(format!("prečítaných {precitane} z {treba} bajtov")));
        }
        Ok(())
    }

    /// Dokončí zápis (všetko je fyzicky na médiu).
    pub fn dokonci(&mut self) -> io::Result<()> {
        // SAFETY: platná rukoväť.
        if unsafe { FlushFileBuffers(self.h.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Povie Windows, aby znovu prečítal tabuľku oddielov – až po overení, lebo Windows po pripojení
    /// oddielu FAT32 doň sám zapisuje (napr. priečinok System Volume Information).
    pub fn ohlas_nove_oddiely(&mut self) {
        let _ = ioctl(&self.h, IOCTL_DISK_UPDATE_PROPERTIES, &[], &mut []);
    }
}

impl crate::ciel::Ciel for Disk {
    fn sektor(&self) -> u32 {
        self.sektor
    }
    fn velkost(&self) -> u64 {
        self.info.velkost
    }
    fn zapis(&mut self, posun: u64, data: &[u8]) -> io::Result<()> {
        Disk::zapis(self, posun, data)
    }
    fn citaj(&mut self, posun: u64, ciel: &mut [u8]) -> io::Result<()> {
        Disk::citaj(self, posun, ciel)
    }
    fn dokonci(&mut self) -> io::Result<()> {
        Disk::dokonci(self)
    }
}
