//! **Profil po funkciách priamo na doske** (`hevc_meranie --profil`): vzorkovanie adresy inštrukcie hlavného
//! vlákna softvérovými hodinami jadra Linux (`perf_event_open`, PERF_COUNT_SW_CPU_CLOCK – netreba hardvérové
//! počítadlá ani program perf) a pomenovanie adries podľa tabuľky symbolov vlastného programu (`/proc/self/exe`).
//!
//! Bez knižníc: systémové volania priamo (aarch64 `svc 0`, x86-64 `syscall`), ELF a `/proc` čítané vlastným
//! kódom. Mená ostávajú zakódované (Rust v0, `_R…`) – čitateľné sa urobia mimo dosky (`c++filt`), na doske by
//! dekódovanie mien bolo zbytočne veľa kódu. Profiluje sa len volajúce vlákno – meria sa s `--vlakna 1`.
//! Inde (iný systém či procesor, jadro bez udalostí) [`Profil::spusti`] vráti dôvod a meranie ide ďalej bez profilu.

// Mimo Linuxu na aarch64 a x86-64 sa profil nespustí (vráti dôvod) – pomocné časti tam ostanú nepoužité.
#![cfg_attr(
    not(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64"))),
    allow(dead_code)
)]

use std::collections::HashMap;

/// Vzorky za sekundu (1 kHz: pri 15 s behu ≈ 15 000 vzoriek, chyba podielu < 1 %).
const FREKVENCIA: u64 = 1000;
/// Stránok vyrovnávacej pamäte vzoriek (mocnina 2; 16 B na vzorku → pri 4 KiB stránkach 32 000 vzoriek).
const STRANOK: usize = 128;

#[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64")))]
mod sys {
    #[cfg(target_arch = "aarch64")]
    pub const PERF_EVENT_OPEN: usize = 241;
    #[cfg(target_arch = "aarch64")]
    pub const MMAP: usize = 222;
    #[cfg(target_arch = "aarch64")]
    pub const IOCTL: usize = 29;
    #[cfg(target_arch = "x86_64")]
    pub const PERF_EVENT_OPEN: usize = 298;
    #[cfg(target_arch = "x86_64")]
    pub const MMAP: usize = 9;
    #[cfg(target_arch = "x86_64")]
    pub const IOCTL: usize = 16;

    /// Systémové volanie so 6 argumentmi; záporný výsledok = −errno.
    ///
    /// # Safety
    /// Argumenty musia byť platné pre dané volanie (ukazovatele na pamäť správnej veľkosti).
    #[cfg(target_arch = "aarch64")]
    pub unsafe fn volanie(nr: usize, a: [usize; 6]) -> isize {
        let ret: isize;
        // SAFETY: platnosť argumentov zaručuje volajúci; svc 0 mení len x0.
        unsafe {
            core::arch::asm!("svc 0", inlateout("x0") a[0] => ret, in("x1") a[1], in("x2") a[2], in("x3") a[3],
                in("x4") a[4], in("x5") a[5], in("x8") nr, options(nostack));
        }
        ret
    }

    /// Systémové volanie so 6 argumentmi; záporný výsledok = −errno.
    ///
    /// # Safety
    /// Argumenty musia byť platné pre dané volanie (ukazovatele na pamäť správnej veľkosti).
    #[cfg(target_arch = "x86_64")]
    pub unsafe fn volanie(nr: usize, a: [usize; 6]) -> isize {
        let ret: isize;
        // SAFETY: platnosť argumentov zaručuje volajúci; syscall mení rax, rcx, r11.
        unsafe {
            core::arch::asm!("syscall", inlateout("rax") nr as isize => ret, in("rdi") a[0], in("rsi") a[1],
                in("rdx") a[2], in("r10") a[3], in("r8") a[4], in("r9") a[5], lateout("rcx") _, lateout("r11") _,
                options(nostack));
        }
        ret
    }
}

/// Bežiace vzorkovanie.
pub struct Profil {
    #[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64")))]
    fd: usize,
    #[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64")))]
    pamat: *const u8,
    #[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64")))]
    stranka: usize,
}

/// Výsledok: počet vzoriek po symboloch (zakódované meno) a súhrn.
pub struct Vysledok {
    pub po_symboloch: Vec<(String, u64)>,
    pub vzoriek: u64,
    pub stratenych: u64,
}

fn le_u16(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?)))
}
fn le_u32(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?)))
}
fn le_u64(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

/// Funkcia z tabuľky symbolov: (adresa, dĺžka, meno).
type Symbol = (u64, u64, String);

/// Funkcie z tabuľky symbolov ELF64 (little endian) zoradené podľa adresy a virtuálna adresa prvého segmentu
/// PT_LOAD s posunom 0 v súbore.
fn symboly(elf: &[u8]) -> Option<(Vec<Symbol>, u64)> {
    if elf.get(..6)? != b"\x7fELF\x02\x01" {
        return None;
    }
    let (phoff, phent, phnum) = (le_u64(elf, 0x20)?, le_u16(elf, 0x36)?, le_u16(elf, 0x38)?);
    let mut zaklad = 0;
    for i in 0..phnum {
        let p = (phoff + i * phent) as usize;
        if le_u32(elf, p)? == 1 && le_u64(elf, p + 8)? == 0 {
            zaklad = le_u64(elf, p + 16)?;
            break;
        }
    }
    let (shoff, shent, shnum) = (le_u64(elf, 0x28)?, le_u16(elf, 0x3a)?, le_u16(elf, 0x3c)?);
    let sekcia = |i: u64| -> Option<(u64, u64, u64, u64)> {
        let s = (shoff + i * shent) as usize;
        Some((le_u32(elf, s + 4)?, le_u64(elf, s + 0x18)?, le_u64(elf, s + 0x20)?, le_u32(elf, s + 0x28)?))
    };
    let mut vysledok = Vec::new();
    for i in 0..shnum {
        let (typ, off, dlzka, link) = sekcia(i)?;
        if typ != 2 {
            continue; // SHT_SYMTAB
        }
        let (_, str_off, str_dlzka, _) = sekcia(link)?;
        let retazce = elf.get(str_off as usize..(str_off + str_dlzka) as usize)?;
        for s in (off..off + dlzka).step_by(24) {
            let s = s as usize;
            let (meno, info, adresa, velkost) =
                (le_u32(elf, s)?, *elf.get(s + 4)?, le_u64(elf, s + 8)?, le_u64(elf, s + 16)?);
            if info & 0xf != 2 || velkost == 0 {
                continue; // len STT_FUNC s dĺžkou
            }
            let m = retazce.get(meno as usize..)?;
            let m = &m[..m.iter().position(|&b| b == 0).unwrap_or(m.len())];
            vysledok.push((adresa, velkost, String::from_utf8_lossy(m).into_owned()));
        }
    }
    vysledok.sort_unstable_by_key(|s| s.0);
    Some((vysledok, zaklad))
}

/// Začiatok mapovania vlastného programu s posunom 0 (z `/proc/self/maps`).
fn zaciatok_programu() -> Option<u64> {
    let cesta = std::fs::read_link("/proc/self/exe").ok()?;
    let cesta = cesta.to_string_lossy();
    let mapy = std::fs::read_to_string("/proc/self/maps").ok()?;
    for riadok in mapy.lines() {
        let polia: Vec<&str> = riadok.split_whitespace().collect();
        if polia.len() >= 6 && polia[5] == cesta && u64::from_str_radix(polia[2], 16).ok()? == 0 {
            return u64::from_str_radix(polia[0].split('-').next()?, 16).ok();
        }
    }
    None
}

/// Veľkosť stránky z pomocného vektora (AT_PAGESZ = 6); RPi 5 má 16 KiB.
fn velkost_stranky() -> usize {
    std::fs::read("/proc/self/auxv")
        .ok()
        .and_then(|a| {
            a.chunks_exact(16)
                .find(|p| le_u64(p, 0) == Some(6))
                .and_then(|p| le_u64(p, 8))
                .map(|v| v as usize)
        })
        .unwrap_or(4096)
}

impl Profil {
    /// Začne vzorkovať volajúce vlákno; pri neúspechu vráti dôvod.
    #[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64")))]
    pub fn spusti() -> Result<Profil, String> {
        // perf_event_attr (128 B): typ SOFTWARE, CPU_CLOCK, frekvencia, PERF_SAMPLE_IP,
        // disabled | exclude_kernel | exclude_hv | freq
        let mut attr = [0u8; 128];
        attr[0..4].copy_from_slice(&1u32.to_le_bytes());
        attr[4..8].copy_from_slice(&128u32.to_le_bytes());
        attr[16..24].copy_from_slice(&FREKVENCIA.to_le_bytes());
        attr[24..32].copy_from_slice(&1u64.to_le_bytes());
        attr[40..48].copy_from_slice(&((1u64 << 0) | (1 << 5) | (1 << 6) | (1 << 10)).to_le_bytes());
        // SAFETY: attr je platný perf_event_attr s veľkosťou 128; pid 0, cpu −1, skupina −1, FD_CLOEXEC.
        let fd = unsafe {
            sys::volanie(sys::PERF_EVENT_OPEN, [attr.as_ptr() as usize, 0, usize::MAX, usize::MAX, 8, 0])
        };
        if fd < 0 {
            return Err(format!("perf_event_open_errno_{}", -fd));
        }
        let stranka = velkost_stranky();
        let dlzka = (1 + STRANOK) * stranka;
        // SAFETY: mmap novej oblasti (adresa 0), PROT_READ | PROT_WRITE, MAP_SHARED nad udalosťou fd.
        let p = unsafe { sys::volanie(sys::MMAP, [0, dlzka, 3, 1, fd as usize, 0]) };
        if p < 0 && p > -4096 {
            return Err(format!("mmap_errno_{}", -p));
        }
        // SAFETY: PERF_EVENT_IOC_ENABLE nad platnou udalosťou.
        let r = unsafe { sys::volanie(sys::IOCTL, [fd as usize, 0x2400, 0, 0, 0, 0]) };
        if r < 0 {
            return Err(format!("ioctl_errno_{}", -r));
        }
        Ok(Profil { fd: fd as usize, pamat: p as *const u8, stranka })
    }

    #[cfg(not(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64"))))]
    pub fn spusti() -> Result<Profil, String> {
        Err("len_linux_aarch64_x86_64".into())
    }

    /// Zastaví vzorkovanie a spočíta vzorky po funkciách.
    #[cfg(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64")))]
    pub fn zastav(self) -> Vysledok {
        // SAFETY: PERF_EVENT_IOC_DISABLE nad platnou udalosťou.
        unsafe { sys::volanie(sys::IOCTL, [self.fd, 0x2401, 0, 0, 0, 0]) };
        let meta = self.pamat;
        // SAFETY: prvá stránka mapovania je perf_event_mmap_page: data_head na 1024, data_offset 1040,
        // data_size 1048 (staršie jadrá 0 → údaje hneď za prvou stránkou).
        let (hlava, posun, velkost) = unsafe {
            let h = std::ptr::read_volatile(meta.add(1024) as *const u64);
            std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
            (
                h,
                std::ptr::read_volatile(meta.add(1040) as *const u64),
                std::ptr::read_volatile(meta.add(1048) as *const u64),
            )
        };
        let (posun, velkost) = if velkost == 0 {
            (self.stranka as u64, (STRANOK * self.stranka) as u64)
        } else {
            (posun, velkost)
        };
        // kruhová pamäť bez posúvania data_tail: jadro nič neprepíše, po zaplnení ohlási stratené (PERF_RECORD_LOST)
        let platne = hlava.min(velkost) as usize;
        // SAFETY: oblasť [posun, posun + velkost) leží v mapovaní (1 + STRANOK) stránok.
        let data = unsafe { std::slice::from_raw_parts(meta.add(posun as usize), platne) };
        let (mut adresy, mut stratenych, mut i) = (Vec::new(), 0u64, 0usize);
        while i + 8 <= data.len() {
            let (typ, dlzka) = (le_u32(data, i).unwrap_or(0), le_u16(data, i + 6).unwrap_or(0) as usize);
            if dlzka < 8 {
                break;
            }
            match typ {
                9 => adresy.extend(le_u64(data, i + 8)), // PERF_RECORD_SAMPLE: ip
                2 => stratenych += le_u64(data, i + 16).unwrap_or(0), // PERF_RECORD_LOST: id, lost
                _ => {}
            }
            i += dlzka;
        }
        Vysledok::z_adries(&adresy, stratenych)
    }

    #[cfg(not(all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64"))))]
    pub fn zastav(self) -> Vysledok {
        Vysledok { po_symboloch: Vec::new(), vzoriek: 0, stratenych: 0 }
    }
}

/// Mapovania procesu (začiatok, koniec, krátke meno súboru) – na pomenovanie adries mimo programu.
fn mapovania() -> Vec<(u64, u64, String)> {
    let mapy = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
    mapy.lines()
        .filter_map(|r| {
            let polia: Vec<&str> = r.split_whitespace().collect();
            let (od, po) = polia.first()?.split_once('-')?;
            let meno = polia.get(5).map_or("[anon]", |m| m.rsplit('/').next().unwrap_or(m));
            Some((u64::from_str_radix(od, 16).ok()?, u64::from_str_radix(po, 16).ok()?, format!("?{meno}")))
        })
        .collect()
}

impl Vysledok {
    fn z_adries(adresy: &[u64], stratenych: u64) -> Vysledok {
        let mapy = mapovania();
        let elf = std::fs::read("/proc/self/exe").unwrap_or_default();
        let (tabulka, zaklad) = symboly(&elf).unwrap_or_default();
        let posun = zaciatok_programu().map_or(0, |z| z.wrapping_sub(zaklad & !0xfff));
        let mut pocty: HashMap<&str, u64> = HashMap::new();
        for &ip in adresy {
            let a = ip.wrapping_sub(posun);
            let i = tabulka.partition_point(|s| s.0 <= a);
            let meno = match i.checked_sub(1).map(|i| &tabulka[i]) {
                Some((zac, dlzka, meno)) if a < zac + dlzka => meno.as_str(),
                // mimo funkcií programu: meno mapovania (napr. ?libc.so.6 – memset, memcpy)
                _ => mapy.iter().find(|m| m.0 <= ip && ip < m.1).map_or("?", |m| m.2.as_str()),
            };
            *pocty.entry(meno).or_default() += 1;
        }
        let mut po_symboloch: Vec<(String, u64)> =
            pocty.into_iter().map(|(m, n)| (m.to_owned(), n)).collect();
        po_symboloch.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Vysledok { po_symboloch, vzoriek: adresy.len() as u64, stratenych }
    }
}
