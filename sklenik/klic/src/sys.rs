//! Volania jadra Linux, ktoré štandardná knižnica Rustu nemá (pripojenie súborových systémov, vypnutie,
//! zber ukončených procesov, zapnutie sieťového rozhrania). Jediné miesto KLÍČA s nebezpečným kódom.

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
    fn reboot(prikaz: c_int) -> c_int;
    fn sync();
    fn waitpid(pid: c_int, stav: *mut c_int, volby: c_int) -> c_int;
    fn socket(domena: c_int, typ: c_int, protokol: c_int) -> c_int;
    fn ioctl(fd: c_int, poziadavka: c_ulong, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
}

pub const MS_NOSUID: c_ulong = 2;
pub const MS_NODEV: c_ulong = 4;
pub const MS_NOEXEC: c_ulong = 8;

const RB_POWER_OFF: c_int = 0x4321_fedc_u32 as c_int;
const RB_AUTOBOOT: c_int = 0x0123_4567;
const WNOHANG: c_int = 1;

const AF_INET: c_int = 2;
const SOCK_DGRAM: c_int = 2;
const SOCK_CLOEXEC: c_int = 0o2000000;
const SIOCGIFFLAGS: c_ulong = 0x8913;
const SIOCSIFFLAGS: c_ulong = 0x8914;
const IFF_UP: i16 = 1;
/// `struct ifreq`: meno (16 B) + spoločná časť; 40 B stačí na 32- aj 64-bitovom jadre.
const IFREQ: usize = 40;

/// Pripojí súborový systém.
pub fn pripoj(zdroj: &str, ciel: &str, typ: &str, priznaky: c_ulong) -> io::Result<()> {
    let (z, c, t) = (cstr(zdroj)?, cstr(ciel)?, cstr(typ)?);
    // SAFETY: všetky reťazce sú platné C reťazce žijúce počas volania, data je NULL.
    let r = unsafe { mount(z.as_ptr(), c.as_ptr(), t.as_ptr(), priznaky, std::ptr::null()) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
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
        if priznaky & IFF_UP != 0 {
            return Ok(false);
        }
        ifr[16..18].copy_from_slice(&(priznaky | IFF_UP).to_ne_bytes());
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

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "reťazec obsahuje nulový znak"))
}
