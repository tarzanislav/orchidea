//! Volania jadra Linux, ktoré štandardná knižnica Rustu nemá (pripojenie súborových systémov, vypnutie,
//! zber ukončených procesov, zapnutie sieťového rozhrania, obrazovka, meno zariadenia). Jediné miesto KLÍČA s nebezpečným kódom.

#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_int, c_ulong, c_void};
use std::io;

unsafe extern "C" {
    fn mount(
        zdroj: *const c_char,
        ciel: *const c_char,
        typ: *const c_char,
        priznaky: c_ulong,
        data: *const c_void,
    ) -> c_int;
    fn umount2(ciel: *const c_char, priznaky: c_int) -> c_int;
    fn reboot(prikaz: c_int) -> c_int;
    fn sync();
    fn waitpid(pid: c_int, stav: *mut c_int, volby: c_int) -> c_int;
    fn socket(domena: c_int, typ: c_int, protokol: c_int) -> c_int;
    fn ioctl(fd: c_int, poziadavka: c_ulong, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn sethostname(meno: *const u8, dlzka: usize) -> c_int;
    fn swapon(cesta: *const c_char, priznaky: c_int) -> c_int;
    fn sched_setaffinity(pid: c_int, velkost: usize, maska: *const u64) -> c_int;
    fn sysconf(meno: c_int) -> core::ffi::c_long;
    fn setpriority(druh: c_int, kto: u32, priorita: c_int) -> c_int;
    fn bind(fd: c_int, adresa: *const c_void, dlzka: u32) -> c_int;
    fn poll(fds: *mut PollFd, pocet: c_ulong, cakanie_ms: c_int) -> c_int;
    fn recv(fd: c_int, buf: *mut c_void, dlzka: usize, priznaky: c_int) -> isize;
    fn read(fd: c_int, buf: *mut c_void, dlzka: usize) -> isize;
    fn mmap(
        adresa: *mut c_void,
        dlzka: usize,
        ochrana: c_int,
        priznaky: c_int,
        fd: c_int,
        posun: i64,
    ) -> *mut c_void;
    fn munmap(adresa: *mut c_void, dlzka: usize) -> c_int;
    fn syscall(cislo: std::ffi::c_long, ...) -> std::ffi::c_long;
    fn prctl(volba: c_int, ...) -> c_int;
    fn fcntl(fd: c_int, prikaz: c_int, ...) -> c_int;
    fn posix_fadvise(fd: c_int, posun: i64, dlzka: i64, rada: c_int) -> c_int;
}

/// `struct pollfd`
#[repr(C)]
struct PollFd {
    fd: c_int,
    udalosti: i16,
    vratene: i16,
}

/// R264: len na čítanie – hľadanie aktualizácie nič nezapíše (ani príznak „použitý“ oddielu FAT).
pub const MS_RDONLY: c_ulong = 1;
pub const MS_NOSUID: c_ulong = 2;
pub const MS_NODEV: c_ulong = 4;
pub const MS_NOEXEC: c_ulong = 8;
pub const MS_NOATIME: c_ulong = 1024;

const RB_POWER_OFF: c_int = 0x4321_fedc_u32 as c_int;
const RB_AUTOBOOT: c_int = 0x0123_4567;
const WNOHANG: c_int = 1;

const AF_INET: c_int = 2;
const SOCK_DGRAM: c_int = 2;
const SOCK_CLOEXEC: c_int = 0o2000000;
const SIOCGIFFLAGS: c_ulong = 0x8913;
const SIOCSIFFLAGS: c_ulong = 0x8914;
const IFF_UP: i16 = 1;
const FBIOGET_VSCREENINFO: c_ulong = 0x4600;
const KDSETMODE: c_ulong = 0x4B3A;
const KD_GRAPHICS: c_ulong = 1;

/// `struct ifreq`: meno (16 B) + spoločná časť; 40 B stačí na 32- aj 64-bitovom jadre.
const IFREQ: usize = 40;
const SIOCETHTOOL: c_ulong = 0x8946;
const ETHTOOL_GEEE: u32 = 0x44;
const ETHTOOL_SEEE: u32 = 0x45;
const SC_PAGESIZE: c_int = 30;
const SWAP_FLAG_PREFER: c_int = 0x8000;
const WDIOC_SETTIMEOUT: c_ulong = 0xC004_5706;
const AF_NETLINK: u16 = 16;
const SOCK_RAW: c_int = 3;
const SOCK_NONBLOCK: c_int = 0o4000;
const NETLINK_ROUTE: c_int = 0;
const NETLINK_KOBJECT_UEVENT: c_int = 15;
/// Skupina správ o sieťových rozhraniach (kábel pripojený/odpojený).
const RTMGRP_LINK: u32 = 1;
/// Skupina udalostí jadra o zariadeniach (pripojené/odpojené USB, karta, obrazovka …).
const UEVENT_JADRA: u32 = 1;
const POLLIN: i16 = 1;
const MSG_DONTWAIT: c_int = 0x40;

/// Pripojí súborový systém.
pub fn pripoj(zdroj: &str, ciel: &str, typ: &str, priznaky: c_ulong) -> io::Result<()> {
    let (z, c, t) = (cstr(zdroj)?, cstr(ciel)?, cstr(typ)?);
    // SAFETY: všetky reťazce sú platné C reťazce žijúce počas volania, data je NULL.
    let r = unsafe { mount(z.as_ptr(), c.as_ptr(), t.as_ptr(), priznaky, std::ptr::null()) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Pripojí súborový systém s voľbami ovládača (napr. `flush` pre FAT – R122).
pub fn pripoj_s_volbami(
    zdroj: &str,
    ciel: &str,
    typ: &str,
    priznaky: c_ulong,
    volby: &str,
) -> io::Result<()> {
    let (z, c, t, v) = (cstr(zdroj)?, cstr(ciel)?, cstr(typ)?, cstr(volby)?);
    // SAFETY: všetky reťazce sú platné C reťazce žijúce počas volania; volby sú text ukončený nulou.
    let r = unsafe { mount(z.as_ptr(), c.as_ptr(), t.as_ptr(), priznaky, v.as_ptr() as *const c_void) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Odpojí súborový systém (jadro pritom zapíše všetko, čo ešte nebolo zapísané).
pub fn odpoj(ciel: &str) -> io::Result<()> {
    let c = cstr(ciel)?;
    // SAFETY: platný C reťazec žijúci počas volania; bez príznakov.
    if unsafe { umount2(c.as_ptr(), 0) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Najnižšia priorita pre volajúce vlákno (nice 19) – práca na pozadí nepredbehne nič iné (R122).
pub fn nizka_priorita() -> io::Result<()> {
    // SAFETY: PRIO_PROCESS (0) s kto = 0 je na Linuxe volajúce vlákno; len celé čísla.
    if unsafe { setpriority(0, 0, 19) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Zapíše všetky vyrovnávacie pamäte jadra na disky.
pub fn zapis_vyrovnavacie() {
    // SAFETY: sync nemá parametre ani výsledok.
    unsafe { sync() }
}

/// Zapíše vyrovnávacie pamäte na disk a vypne zariadenie (alebo reštartuje).
pub fn vypni(restart: bool) -> io::Error {
    // SAFETY: sync nemá parametre; reboot sa volá s platnou konštantou jadra.
    unsafe {
        sync();
        reboot(if restart { RB_AUTOBOOT } else { RB_POWER_OFF });
    }
    io::Error::last_os_error()
}

/// Zozbiera všetky ukončené procesy (úloha PID 1). Vráti ich počet.
pub fn zber_procesov() -> usize {
    let mut n = 0;
    loop {
        let mut stav: c_int = 0;
        // SAFETY: stav je platná premenná; WNOHANG – volanie neblokuje.
        let pid = unsafe { waitpid(-1, &mut stav, WNOHANG) };
        if pid <= 0 {
            return n;
        }
        n += 1;
    }
}

/// Zapne sieťové rozhranie (ako `ip link set <meno> up`). Vráti `true`, ak bolo vypnuté a zaplo sa.
pub fn zapni_rozhranie(meno: &str) -> io::Result<bool> {
    nastav_rozhranie(meno, true)
}

/// Zapne alebo vypne sieťové rozhranie (`ip link set <meno> up/down`). Vypnuté rozhranie odpojí čip siete
/// (PHY prejde do úsporného stavu) a vypne takty radiča. Vráti `true`, ak sa stav zmenil.
pub fn nastav_rozhranie(meno: &str, zapnute: bool) -> io::Result<bool> {
    if meno.is_empty() || meno.len() >= 16 || meno.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "neplatné meno rozhrania"));
    }
    // SAFETY: obyčajné volanie jadra s konštantami; výsledok sa kontroluje.
    let fd = unsafe { socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut ifr = [0u8; IFREQ];
    ifr[..meno.len()].copy_from_slice(meno.as_bytes());
    let vysledok = (|| {
        // SAFETY: ifr je platná vyrovnávacia pamäť veľkosti struct ifreq, žije počas volania.
        if unsafe { ioctl(fd, SIOCGIFFLAGS, ifr.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let priznaky = i16::from_ne_bytes([ifr[16], ifr[17]]);
        if (priznaky & IFF_UP != 0) == zapnute {
            return Ok(false);
        }
        let nove = if zapnute { priznaky | IFF_UP } else { priznaky & !IFF_UP };
        ifr[16..18].copy_from_slice(&nove.to_ne_bytes());
        // SAFETY: ako vyššie.
        if unsafe { ioctl(fd, SIOCSIFFLAGS, ifr.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(true)
    })();
    // SAFETY: fd je náš otvorený deskriptor, zatvára sa raz.
    unsafe { close(fd) };
    vysledok
}

/// Popis obrazovky od jadra (`struct fb_var_screeninfo` ako 40 slov po 32 bitoch).
pub fn fb_vlastnosti(fb: &std::fs::File) -> io::Result<[u32; 40]> {
    use std::os::fd::AsRawFd;
    let mut v = [0u32; 40];
    // SAFETY: v má presne veľkosť fb_var_screeninfo (160 B), deskriptor je platný počas volania.
    if unsafe { ioctl(fb.as_raw_fd(), FBIOGET_VSCREENINFO, v.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(v)
}

/// Prepne konzolu do grafického režimu – jadro potom do obrazovky nekreslí text.
pub fn graficka_konzola(tty: &std::fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: KDSETMODE berie celé číslo hodnotou; deskriptor je platný.
    if unsafe { ioctl(tty.as_raw_fd(), KDSETMODE, KD_GRAPHICS) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Nastaví meno zariadenia v sieti (ako `hostname`).
pub fn nastav_meno(meno: &str) -> io::Result<()> {
    // SAFETY: ukazovateľ a dĺžka pochádzajú z platného reťazca.
    if unsafe { sethostname(meno.as_ptr(), meno.len()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "reťazec obsahuje nulový znak"))
}

/// Veľkosť stránky pamäte (4 kB; RPi 5 má 16 kB).
pub fn velkost_stranky() -> usize {
    // SAFETY: sysconf len číta nastavenie systému.
    let v = unsafe { sysconf(SC_PAGESIZE) };
    if v > 0 { v as usize } else { 4096 }
}

/// Zapne odkladaciu pamäť (zram) s prednosťou `priorita`.
pub fn zapni_odkladanie(cesta: &str, priorita: c_int) -> io::Result<()> {
    let c = CString::new(cesta).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: platný reťazec ukončený nulou, žije počas volania.
    if unsafe { swapon(c.as_ptr(), SWAP_FLAG_PREFER | (priorita & 0x7fff)) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

const ETHTOOL_GSET: u32 = 0x1;
const ETHTOOL_SSET: u32 = 0x2;
/// Ponúkané režimy, ktoré ostanú bez gigabitu: 10/100 Mb/s (bity 0–3), samočinné dohodnutie, druhy
/// rozhraní a riadenie toku (bity 6–11, 13, 14).
pub const BEZ_GIGABITU: u32 = 0x0000_6FCF;

/// R126: Ethernet ponúkne len 10/100 Mb/s (gigabitový čip siete berie v pokoji viac energie). Spojenie sa
/// znova dohodne (asi 2–3 s). Vráti `Ok(true)`, ak sa ponuka zmenila.
pub fn ethernet_bez_gigabitu(meno: &str) -> io::Result<bool> {
    // len gigabit preč, 10/100 ostáva (ako pred R170)
    ethernet_rychlost(meno, 100)
}

/// Sieť podľa potreby (R159): ponuka gigabitu zapnutá alebo vypnutá.
pub fn ethernet_gigabit(meno: &str, gigabit: bool) -> io::Result<bool> {
    ethernet_rychlost(meno, if gigabit { 1000 } else { 100 })
}

/// Ponúkané režimy najviac do `max_mbs` (10, 100, 1000) z podporovaných; ostatné bity (autoneg, pauza)
/// ostávajú. `None`, ak sa nič nemení alebo by nezostal žiadny režim.
pub fn ponuka_do(podporovane: u32, ponuka: u32, max_mbs: u32) -> Option<u32> {
    let rychlosti = match max_mbs {
        0..=10 => 0x03,
        11..=100 => 0x0F,
        _ => 0x3F,
    };
    let nova = if max_mbs > 100 {
        (ponuka & !0x3F) | (podporovane & rychlosti)
    } else {
        // bez gigabitu aj bez rýchlejších režimov (2,5/10 Gb/s)
        (ponuka & BEZ_GIGABITU & !0x0F) | (podporovane & rychlosti)
    };
    (nova != ponuka && nova & 0x3F != 0).then_some(nova)
}

/// R170: sieť v troch stupňoch – ponuka najviac 10, 100 alebo 1000 Mb/s. Vráti `Ok(true)`, ak sa zmenila
/// (spojenie sa znova dohodne, asi 2–3 s).
pub fn ethernet_rychlost(meno: &str, max_mbs: u32) -> io::Result<bool> {
    if meno.is_empty() || meno.len() >= 16 || meno.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "neplatné meno rozhrania"));
    }
    // struct ethtool_cmd: 44 bajtov – cmd, supported, advertising (posun 8), …, autoneg (posun 18)
    let mut cmd = [0u32; 11];
    cmd[0] = ETHTOOL_GSET;
    // SAFETY: obyčajné volanie jadra s konštantami; výsledok sa kontroluje.
    let fd = unsafe { socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut ifr = [0u8; IFREQ];
    ifr[..meno.len()].copy_from_slice(meno.as_bytes());
    let vysledok = (|| {
        let mut volaj = |cmd: &mut [u32; 11]| {
            let ukazovatel = cmd.as_mut_ptr() as usize;
            ifr[16..16 + size_of::<usize>()].copy_from_slice(&ukazovatel.to_ne_bytes());
            // SAFETY: ifr obsahuje meno a ukazovateľ na cmd (44 B); obe žijú počas volania.
            if unsafe { ioctl(fd, SIOCETHTOOL, ifr.as_mut_ptr()) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        };
        volaj(&mut cmd)?;
        let autoneg = cmd[4].to_ne_bytes()[2];
        let nova = ponuka_do(cmd[1], cmd[2], max_mbs);
        let Some(nova) = nova.filter(|_| autoneg == 1) else {
            return Ok(false);
        };
        cmd[0] = ETHTOOL_SSET;
        cmd[2] = nova;
        volaj(&mut cmd)?;
        Ok(true)
    })();
    // SAFETY: fd je náš otvorený deskriptor, zatvára sa raz.
    unsafe { close(fd) };
    vysledok
}

/// Vypne úsporný režim Ethernetu (EEE) na rozhraní – na RPi 3 B+, 4 a 400 známa príčina výpadkov spojenia.
/// Vráti `Ok(false)`, ak EEE nebolo zapnuté.
pub fn vypni_eee(meno: &str) -> io::Result<bool> {
    if meno.is_empty() || meno.len() >= 16 || meno.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "neplatné meno rozhrania"));
    }
    // struct ethtool_eee: cmd, supported, advertised, lp_advertised, eee_active, eee_enabled,
    // tx_lpi_enabled, tx_lpi_timer, reserved[2] – 10 slov po 32 bitoch
    let mut eee = [0u32; 10];
    eee[0] = ETHTOOL_GEEE;
    // SAFETY: obyčajné volanie jadra s konštantami; výsledok sa kontroluje.
    let fd = unsafe { socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut ifr = [0u8; IFREQ];
    ifr[..meno.len()].copy_from_slice(meno.as_bytes());
    let vysledok = (|| {
        let mut volaj = |eee: &mut [u32; 10]| {
            let ukazovatel = eee.as_mut_ptr() as usize;
            ifr[16..16 + size_of::<usize>()].copy_from_slice(&ukazovatel.to_ne_bytes());
            // SAFETY: ifr obsahuje meno a ukazovateľ na eee; obe žijú počas volania.
            if unsafe { ioctl(fd, SIOCETHTOOL, ifr.as_mut_ptr()) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        };
        volaj(&mut eee)?;
        if eee[5] == 0 && eee[6] == 0 {
            return Ok(false);
        }
        eee[0] = ETHTOOL_SEEE;
        eee[5] = 0;
        eee[6] = 0;
        volaj(&mut eee)?;
        Ok(true)
    })();
    // SAFETY: fd je náš otvorený deskriptor, zatvára sa raz.
    unsafe { close(fd) };
    vysledok
}

/// Pripne aktuálne vlákno na jedno jadro procesora.
pub fn pripni_na_jadro(jadro: usize) -> io::Result<()> {
    let mut maska = [0u64; 16];
    if jadro >= 1024 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    maska[jadro / 64] = 1 << (jadro % 64);
    // SAFETY: maska je platné pole 128 bajtov, žije počas volania.
    if unsafe { sched_setaffinity(0, size_of_val(&maska), maska.as_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Hardvérový strážny časovač (R99): ak ho KLÍČ do `sekund` nenakŕmi, doska sa sama reštartuje.
pub fn otvor_strazny_casovac(sekund: c_int) -> io::Result<std::fs::File> {
    let f = std::fs::OpenOptions::new().write(true).open("/dev/watchdog")?;
    nastav_strazny_casovac(&f, sekund)?;
    Ok(f)
}

/// Čas strážneho časovača otvoreného `f` (R264: aj prevzatého po spustení novej verzie KLÍČA).
pub fn nastav_strazny_casovac(f: &std::fs::File, sekund: c_int) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut t = sekund;
    // SAFETY: ukazovateľ na c_int, žije počas volania; deskriptor patrí f.
    if unsafe { ioctl(f.as_raw_fd(), WDIOC_SETTIMEOUT, &mut t as *mut c_int) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// R264: má deskriptor prežiť spustenie iného programu (exec)? Strážny časovač sa takto odovzdá novej verzii
/// KLÍČA – zariadenie /dev/watchdog sa nedá otvoriť dvakrát a zavretý bez „V“ by dosku reštartoval.
pub fn dedi_pri_exec(fd: c_int, dedi: bool) -> io::Result<()> {
    const F_SETFD: c_int = 2;
    const FD_CLOEXEC: c_int = 1;
    // SAFETY: fcntl s číselným argumentom, deskriptor kontroluje jadro (neplatný → EBADF).
    if unsafe { fcntl(fd, F_SETFD, if dedi { 0 } else { FD_CLOEXEC }) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// R264: súbor zdedený cez exec podľa čísla deskriptora (`None`, ak taký deskriptor nie je otvorený).
pub fn zdedeny_subor(fd: c_int) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    const F_GETFD: c_int = 1;
    // SAFETY: F_GETFD len overí, že deskriptor je otvorený.
    if fd < 3 || unsafe { fcntl(fd, F_GETFD) } < 0 {
        return None;
    }
    // SAFETY: deskriptor je otvorený a odovzdaný práve tomuto programu (premenná prostredia od štartéra);
    // nikto iný ho nevlastní.
    Some(unsafe { std::fs::File::from_raw_fd(fd) })
}

/// R264: zahodí stránky súboru z vyrovnávacej pamäte (po `sync_all`) – ďalšie čítanie ide z karty, takže
/// overenie odtlačku po zápise naozaj overí kartu.
pub fn zabudni_v_pamati(f: &std::fs::File) {
    use std::os::fd::AsRawFd;
    const POSIX_FADV_DONTNEED: c_int = 4;
    // SAFETY: obyčajné volanie s platným deskriptorom; výsledok je len rada jadru.
    unsafe { posix_fadvise(f.as_raw_fd(), 0, 0, POSIX_FADV_DONTNEED) };
}

/// Stav úsporného režimu Ethernetu (EEE): (zapnutý, práve aktívny so súperom na druhej strane kábla).
/// `Ok(None)`, ak ho rozhranie nepodporuje.
pub fn eee_stav(meno: &str) -> io::Result<Option<(bool, bool)>> {
    if meno.is_empty() || meno.len() >= 16 || meno.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "neplatné meno rozhrania"));
    }
    let mut eee = [0u32; 10];
    eee[0] = ETHTOOL_GEEE;
    // SAFETY: obyčajné volanie jadra s konštantami; výsledok sa kontroluje.
    let fd = unsafe { socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut ifr = [0u8; IFREQ];
    ifr[..meno.len()].copy_from_slice(meno.as_bytes());
    let ukazovatel = eee.as_mut_ptr() as usize;
    ifr[16..16 + size_of::<usize>()].copy_from_slice(&ukazovatel.to_ne_bytes());
    // SAFETY: ifr obsahuje meno a ukazovateľ na eee; obe žijú počas volania.
    let r = unsafe { ioctl(fd, SIOCETHTOOL, ifr.as_mut_ptr()) };
    let chyba = io::Error::last_os_error();
    // SAFETY: fd je náš otvorený deskriptor, zatvára sa raz.
    unsafe { close(fd) };
    if r < 0 {
        // EOPNOTSUPP – ovládač EEE nepozná
        return if chyba.raw_os_error() == Some(95) { Ok(None) } else { Err(chyba) };
    }
    if eee[1] == 0 {
        return Ok(None);
    }
    Ok(Some((eee[5] != 0, eee[4] != 0)))
}

/// Udalosti jadra (R113): pripojené a odpojené zariadenia (uevent) a zmeny sieťových rozhraní (kábel).
/// KLÍČ na ne čaká v spánku – nemusí sa sám prebúdzať a prehľadávať `/sys`.
pub struct Udalosti {
    fds: Vec<c_int>,
}

impl Udalosti {
    pub fn otvor() -> io::Result<Udalosti> {
        let mut u = Udalosti { fds: Vec::new() };
        for (protokol, skupiny) in [(NETLINK_KOBJECT_UEVENT, UEVENT_JADRA), (NETLINK_ROUTE, RTMGRP_LINK)] {
            // SAFETY: obyčajné volanie jadra s konštantami; výsledok sa kontroluje.
            let fd =
                unsafe { socket(AF_NETLINK as c_int, SOCK_RAW | SOCK_CLOEXEC | SOCK_NONBLOCK, protokol) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            u.fds.push(fd);
            // struct sockaddr_nl: rodina, výplň, pid (0 = pridelí jadro), skupiny
            let mut adresa = [0u8; 12];
            adresa[..2].copy_from_slice(&AF_NETLINK.to_ne_bytes());
            adresa[8..12].copy_from_slice(&skupiny.to_ne_bytes());
            // SAFETY: adresa je platná struct sockaddr_nl (12 B), žije počas volania.
            if unsafe { bind(fd, adresa.as_ptr() as *const c_void, 12) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(u)
    }

    /// Čaká najviac `ms` milisekúnd. Vráti `true`, ak prišla aspoň jedna udalosť (všetky sa vyberú).
    pub fn cakaj(&self, ms: u64) -> bool {
        self.cakaj_so_vstupmi(ms, &[], &[]).0
    }

    /// Čaká na udalosti jadra aj na vstupné zariadenia (`/dev/input/event*` – klávesnica, myš, diaľkový
    /// ovládač; R158 – prebudenie obrazu). Vráti (udalosť jadra, stlačenie/pohyb na vstupe); všetko sa vyberie.
    /// `ine` sa len sledujú (napr. CEC) – čítanie je na volajúcom; tretia hodnota = niektorý je pripravený.
    /// Vráti (udalosť jadra, činnosť človeka, iný popisovač pripravený, počet stlačení tlačidiel – R185).
    pub fn cakaj_so_vstupmi(&self, ms: u64, vstupy: &[c_int], ine: &[c_int]) -> (bool, bool, bool, u32) {
        let mut p: Vec<PollFd> = self
            .fds
            .iter()
            .chain(vstupy)
            .chain(ine)
            .map(|&fd| PollFd { fd, udalosti: POLLIN, vratene: 0 })
            .collect();
        let ms = ms.min(i32::MAX as u64) as c_int;
        // SAFETY: p je platné pole struct pollfd s dĺžkou p.len(), žije počas volania.
        let n = unsafe { poll(p.as_mut_ptr(), p.len() as c_ulong, ms) };
        if n <= 0 {
            return (false, false, false, 0);
        }
        let ine_pripravene = p[self.fds.len() + vstupy.len()..].iter().any(|x| x.vratene != 0);
        let mut buf = [0u8; 8192];
        let mut bolo = false;
        for fd in &self.fds {
            // SAFETY: buf je platná vyrovnávacia pamäť; MSG_DONTWAIT – nečaká sa.
            while unsafe { recv(*fd, buf.as_mut_ptr() as *mut c_void, buf.len(), MSG_DONTWAIT) } > 0 {
                bolo = true;
            }
        }
        let mut vstup = false;
        let mut stlacene = 0u32;
        for (i, &fd) in vstupy.iter().enumerate() {
            if p[self.fds.len() + i].vratene == 0 {
                continue;
            }
            // struct input_event: čas (16 B na 64-bitovom, 8 B na 32-bitovom jadre), typ, kód, hodnota
            let dlzka = 2 * size_of::<usize>() + 8;
            loop {
                // SAFETY: buf je platná vyrovnávacia pamäť; zariadenie je otvorené bez čakania (O_NONBLOCK).
                let n = unsafe { read(fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
                if n <= 0 {
                    break;
                }
                for u in buf[..n as usize].chunks_exact(dlzka) {
                    let typ = u16::from_ne_bytes([u[dlzka - 8], u[dlzka - 7]]);
                    // EV_SYN (0) len oddeľuje dávky; EV_KEY, EV_REL, EV_ABS, EV_MSC (ovládač) sú činnosť
                    if typ != 0 {
                        vstup = true;
                    }
                    // EV_KEY s hodnotou 1 = stlačenie (0 pustenie, 2 opakovanie pri držaní sa nepočíta)
                    let hodnota =
                        i32::from_ne_bytes([u[dlzka - 4], u[dlzka - 3], u[dlzka - 2], u[dlzka - 1]]);
                    if typ == 1 && hodnota == 1 {
                        stlacene += 1;
                    }
                }
            }
        }
        (bolo, vstup, ine_pripravene, stlacene)
    }
}

impl Drop for Udalosti {
    fn drop(&mut self) {
        for &fd in &self.fds {
            // SAFETY: fd sú naše otvorené deskriptory, každý sa zatvára raz.
            unsafe { close(fd) };
        }
    }
}

/// Volanie ioctl s jednou štruktúrou (DRM/KMS – vlastný ovládač obrazovky, R158).
///
/// `T` musí byť štruktúra `#[repr(C)]` presne podľa hlavičky jadra (veľkosť je zakódovaná v `poziadavka`)
/// a polia s ukazovateľmi (`*_ptr`) musia ukazovať na pamäť, ktorá žije počas volania. Jadro môže volanie
/// prerušiť signálom alebo požiadať o opakovanie – opakuje sa ako v knižnici libdrm.
pub fn ioctl_struct<T: Copy>(fd: c_int, poziadavka: c_ulong, s: &mut T) -> io::Result<()> {
    loop {
        // SAFETY: s je platná štruktúra s veľkosťou z požiadavky; ukazovatele v nej zaručuje volajúci.
        if unsafe { ioctl(fd, poziadavka, s as *mut T as *mut c_void) } >= 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if !matches!(e.raw_os_error(), Some(4) | Some(11)) {
            return Err(e);
        }
    }
}

/// Adresa poľa pre polia `*_ptr` v štruktúrach jadra.
pub fn adresa<T>(pole: &mut [T]) -> u64 {
    pole.as_mut_ptr() as usize as u64
}

/// Pamäť zariadenia namapovaná do procesu (obrazový buffer KMS) – uvoľní sa pri zániku.
pub struct Mapa {
    ptr: *mut u8,
    dlzka: usize,
}

// SAFETY: mapa patrí jednému vlastníkovi; prístup len cez &mut self.
unsafe impl Send for Mapa {}

impl Mapa {
    /// Namapuje `dlzka` bajtov zariadenia `fd` od posunu `posun` na čítanie aj zápis (zdieľane).
    pub fn otvor(fd: c_int, dlzka: usize, posun: u64) -> io::Result<Mapa> {
        // SAFETY: jadro vráti novú oblasť alebo MAP_FAILED; dĺžka a posun sú od ovládača.
        let p = unsafe { mmap(std::ptr::null_mut(), dlzka, 3, 1, fd, posun as i64) };
        if p as isize == -1 || p.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapa { ptr: p as *mut u8, dlzka })
    }

    pub fn bajty(&mut self) -> &mut [u8] {
        // SAFETY: oblasť je namapovaná s dĺžkou `dlzka` a žije, kým žije Mapa.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.dlzka) }
    }
}

impl Drop for Mapa {
    fn drop(&mut self) {
        // SAFETY: odmapuje sa presne oblasť, ktorú vrátil mmap, raz.
        unsafe { munmap(self.ptr as *mut c_void, self.dlzka) };
    }
}

/// Číslo volania perf_event_open (R162 – merač pamäte).
#[cfg(target_arch = "aarch64")]
const PERF_EVENT_OPEN: std::ffi::c_long = 241;
#[cfg(target_arch = "arm")]
const PERF_EVENT_OPEN: std::ffi::c_long = 364;
#[cfg(not(any(target_arch = "aarch64", target_arch = "arm")))]
const PERF_EVENT_OPEN: std::ffi::c_long = 298;

/// Otvorí počítadlo jadra (perf) typu `typ` s udalosťou `config` a filtrom `config1` na procesore `cpu`
/// (počítadlá radiča pamäte sú „uncore“ – jedno pre celý čip). Vráti deskriptor; čítanie dá 8 bajtov.
pub fn perf_pocitadlo(typ: u32, config: u64, config1: u64, cpu: i32) -> io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    // struct perf_event_attr vo verzii 1 (72 bajtov): typ, veľkosť, config, …, config1 (posun 56), config2
    let mut attr = [0u8; 72];
    attr[0..4].copy_from_slice(&typ.to_ne_bytes());
    attr[4..8].copy_from_slice(&72u32.to_ne_bytes());
    attr[8..16].copy_from_slice(&config.to_ne_bytes());
    attr[56..64].copy_from_slice(&config1.to_ne_bytes());
    // SAFETY: attr je platná štruktúra s uvedenou veľkosťou; pid −1 = celý systém, bez skupiny, bez príznakov.
    let fd = unsafe {
        syscall(PERF_EVENT_OPEN, attr.as_mut_ptr(), -1 as c_int, cpu as c_int, -1 as c_int, 8 as c_ulong)
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: jadro vrátilo nový deskriptor, ktorý odteraz vlastní File (O_CLOEXEC = príznak 8).
    Ok(unsafe { std::fs::File::from_raw_fd(fd as c_int) })
}

/// R169: väčšia tolerancia časovačov procesu (jadro môže budenia zlúčiť s inými) – menej prebudení čipu.
/// Dedia ju nové vlákna aj procesy: prehrávač videa spustený z KLÍČA si musí nastaviť späť 50 µs.
pub fn tolerancia_casovacov(ns: u64) {
    // SAFETY: PR_SET_TIMERSLACK (29) s číselným argumentom; výsledok netreba.
    unsafe { prctl(29, ns as c_ulong) };
}

#[cfg(target_pointer_width = "64")]
/// Čaká na udalosti `udalosti` (POLLIN 1, POLLPRI 2, POLLOUT 4) jedného deskriptora najviac `ms` milisekúnd;
/// vráti, čo nastalo (0 = vypršal čas).
pub fn cakaj_na(fd: c_int, udalosti: i16, ms: u64) -> io::Result<i16> {
    let mut p = [PollFd { fd, udalosti, vratene: 0 }];
    let ms = ms.min(i32::MAX as u64) as c_int;
    // SAFETY: p je platné pole struct pollfd s dĺžkou 1, žije počas volania.
    let n = unsafe { poll(p.as_mut_ptr(), 1, ms) };
    if n < 0 {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(4) { Ok(0) } else { Err(e) };
    }
    Ok(p[0].vratene)
}

/// Ako [`ioctl_struct`], ale „skús znova“ (EAGAIN) vráti volajúcemu – pre neblokujúce zariadenia (V4L2: ešte
/// nie je hotový buffer). Opakuje sa len prerušenie signálom.
pub fn ioctl_raz<T: Copy>(fd: c_int, poziadavka: c_ulong, s: &mut T) -> io::Result<()> {
    loop {
        // SAFETY: ako pri ioctl_struct – štruktúra s veľkosťou z požiadavky, ukazovatele zaručuje volajúci.
        if unsafe { ioctl(fd, poziadavka, s as *mut T as *mut c_void) } >= 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(4) {
            return Err(e);
        }
    }
}
