//! Dynamické načítanie knižníc operačného systému a ovládačov (dlopen / LoadLibrary).
//!
//! ORCHIDEA sa nezviaže pri preklade so žiadnym ovládačom: knižnica Vulkanu či Direct3D sa hľadá až za behu.
//! Ak chýba, zariadenie sa jednoducho neponúkne a ostanú ostatné (vždy aspoň procesor).

#![allow(unsafe_code)]

use orchidea_jadro::{Chyba, Vysledok};
use std::ffi::{CStr, c_void};

#[cfg(unix)]
// v musl je dlopen priamo v knižnici C (statický program na Raspberry Pi 1 a Zero ho má len ako náhradu)
#[cfg_attr(all(any(target_os = "linux", target_os = "android"), not(target_env = "musl")), link(name = "dl"))]
unsafe extern "C" {
    fn dlopen(subor: *const std::ffi::c_char, priznaky: i32) -> *mut c_void;
    fn dlsym(kniznica: *mut c_void, meno: *const std::ffi::c_char) -> *mut c_void;
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(meno: *const u16) -> *mut c_void;
    fn GetProcAddress(kniznica: *mut c_void, meno: *const std::ffi::c_char) -> *mut c_void;
}

/// Načítaná knižnica. Ostáva načítaná do konca behu programu (ovládače to tak očakávajú).
#[derive(Debug)]
pub struct Kniznica {
    rukovat: *mut c_void,
    pub meno: String,
}

// SAFETY: rukoväť knižnice je len nemenný identifikátor; dlsym/GetProcAddress sú bezpečné z viacerých vlákien.
unsafe impl Send for Kniznica {}
unsafe impl Sync for Kniznica {}

impl Kniznica {
    /// Otvorí prvú knižnicu zo zoznamu, ktorá existuje.
    pub fn otvor(mena: &[&str]) -> Vysledok<Kniznica> {
        for meno in mena {
            #[cfg(unix)]
            {
                let c =
                    std::ffi::CString::new(*meno).map_err(|_| Chyba::MimoRozsahu("meno knižnice".into()))?;
                // SAFETY: platný reťazec ukončený nulou; RTLD_NOW = 2.
                let r = unsafe { dlopen(c.as_ptr(), 2) };
                if !r.is_null() {
                    return Ok(Kniznica { rukovat: r, meno: meno.to_string() });
                }
            }
            #[cfg(windows)]
            {
                let siroke: Vec<u16> = meno.encode_utf16().chain(std::iter::once(0)).collect();
                // SAFETY: platný reťazec UTF-16 ukončený nulou.
                let r = unsafe { LoadLibraryW(siroke.as_ptr()) };
                if !r.is_null() {
                    return Ok(Kniznica { rukovat: r, meno: meno.to_string() });
                }
            }
            #[cfg(not(any(unix, windows)))]
            let _ = meno;
        }
        Err(Chyba::Chybajuce(format!("knižnica {}", mena.join(" ani "))))
    }

    /// Adresa symbolu (None, ak neexistuje).
    pub fn symbol(&self, meno: &CStr) -> Option<*const c_void> {
        #[cfg(unix)]
        // SAFETY: platná rukoväť a reťazec ukončený nulou.
        let p = unsafe { dlsym(self.rukovat, meno.as_ptr()) };
        #[cfg(windows)]
        // SAFETY: platná rukoväť a reťazec ukončený nulou.
        let p = unsafe { GetProcAddress(self.rukovat, meno.as_ptr()) };
        #[cfg(not(any(unix, windows)))]
        let p: *mut c_void = {
            let _ = (meno, self.rukovat);
            std::ptr::null_mut()
        };
        if p.is_null() { None } else { Some(p as *const c_void) }
    }
}
