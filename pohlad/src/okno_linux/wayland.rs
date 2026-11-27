//! Protokol Wayland na úrovni správ (vlastný kód podľa opisu protokolu `wayland.xml`):
//! pripojenie na kompozitor, kódovanie žiadostí a čítanie udalostí.
//!
//! Správa: id objektu (u32), potom (dĺžka << 16 | číslo správy) (u32), potom argumenty po 4 bajtoch.
//! Reťazec = dĺžka vrátane nuly + bajty + nula, zarovnané na 4; pole = dĺžka + bajty, zarovnané na 4;
//! popisovače súborov idú mimo bajtov (SCM_RIGHTS).

use super::sys;
use std::io;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// Pripojí sa na kompozitor podľa `WAYLAND_DISPLAY` (a `XDG_RUNTIME_DIR`).
pub fn pripoj() -> io::Result<UnixStream> {
    let displej = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into());
    let cesta = PathBuf::from(&displej);
    let cesta = if cesta.is_absolute() {
        cesta
    } else {
        let beh = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "chýba XDG_RUNTIME_DIR"))?;
        PathBuf::from(beh).join(cesta)
    };
    UnixStream::connect(cesta)
}

/// Je k dispozícii Wayland (premenná prostredia alebo zásuvka)?
pub fn je_k_dispozicii() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Žiadosť na kompozitor.
pub struct Sprava {
    data: Vec<u8>,
}

impl Sprava {
    pub fn nova(objekt: u32, cislo: u16) -> Sprava {
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(&objekt.to_ne_bytes());
        data.extend_from_slice(&(cislo as u32).to_ne_bytes());
        Sprava { data }
    }
    pub fn u(mut self, v: u32) -> Sprava {
        self.data.extend_from_slice(&v.to_ne_bytes());
        self
    }
    pub fn i(mut self, v: i32) -> Sprava {
        self.data.extend_from_slice(&v.to_ne_bytes());
        self
    }
    pub fn s(mut self, v: &str) -> Sprava {
        // reťazec bez nulových bajtov vnútri
        let v: String = v.chars().filter(|&c| c != '\0').collect();
        self.data.extend_from_slice(&(v.len() as u32 + 1).to_ne_bytes());
        self.data.extend_from_slice(v.as_bytes());
        self.data.push(0);
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        self
    }
    /// Pole bajtov (stavy okna, stlačené klávesy – posiela kompozitor; tu pre skúšky).
    #[cfg(test)]
    pub fn a(mut self, v: &[u8]) -> Sprava {
        self.data.extend_from_slice(&(v.len() as u32).to_ne_bytes());
        self.data.extend_from_slice(v);
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        self
    }
    /// Hotové bajty (s dĺžkou v hlavičke).
    pub fn bajty(mut self) -> Vec<u8> {
        let dlzka = self.data.len() as u32;
        let slovo = u32::from_ne_bytes([self.data[4], self.data[5], self.data[6], self.data[7]]);
        self.data[4..8].copy_from_slice(&((dlzka << 16) | (slovo & 0xffff)).to_ne_bytes());
        self.data
    }
}

/// Zapisovacia strana spojenia a prideľovanie čísel objektov.
pub struct Spojenie {
    z: UnixStream,
    dalsie_id: u32,
}

impl Spojenie {
    pub fn nove(z: UnixStream) -> Spojenie {
        // 1 je wl_display
        Spojenie { z, dalsie_id: 2 }
    }
    pub fn nove_id(&mut self) -> u32 {
        let id = self.dalsie_id;
        self.dalsie_id += 1;
        id
    }
    pub fn posli(&self, s: Sprava) -> io::Result<()> {
        sys::posli(&self.z, &s.bajty(), &[])
    }
    pub fn posli_s_fd(&self, s: Sprava, fds: &[BorrowedFd<'_>]) -> io::Result<()> {
        sys::posli(&self.z, &s.bajty(), fds)
    }
}

/// Udalosť od kompozitora (ešte nerozložená podľa rozhrania).
#[derive(Debug)]
pub struct Udalost {
    pub objekt: u32,
    pub cislo: u16,
    pub data: Vec<u8>,
}

/// Skladá udalosti z prijatých bajtov (správa môže prísť po častiach).
#[derive(Default)]
pub struct Skladac {
    buf: Vec<u8>,
}

impl Skladac {
    pub fn pridaj(&mut self, d: &[u8]) {
        self.buf.extend_from_slice(d);
    }
    pub fn dalsia(&mut self) -> Option<io::Result<Udalost>> {
        if self.buf.len() < 8 {
            return None;
        }
        let objekt = u32::from_ne_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        let slovo = u32::from_ne_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]);
        let dlzka = (slovo >> 16) as usize;
        if dlzka < 8 || !dlzka.is_multiple_of(4) {
            return Some(Err(io::Error::new(io::ErrorKind::InvalidData, "chybná dĺžka správy Wayland")));
        }
        if self.buf.len() < dlzka {
            return None;
        }
        let data = self.buf[8..dlzka].to_vec();
        self.buf.drain(..dlzka);
        Some(Ok(Udalost { objekt, cislo: (slovo & 0xffff) as u16, data }))
    }
}

/// Čítanie argumentov udalosti (pri chybe vracia nuly a prázdne reťazce, nikdy nespadne).
pub struct Citac<'a> {
    d: &'a [u8],
    i: usize,
}

impl<'a> Citac<'a> {
    pub fn novy(d: &'a [u8]) -> Citac<'a> {
        Citac { d, i: 0 }
    }
    pub fn u(&mut self) -> u32 {
        let v = self.d.get(self.i..self.i + 4).map_or(0, |b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
        self.i += 4;
        v
    }
    pub fn i(&mut self) -> i32 {
        self.u() as i32
    }
    /// Číslo s pevnou desatinnou čiarkou (24.8).
    pub fn f(&mut self) -> f64 {
        self.i() as f64 / 256.0
    }
    pub fn pole(&mut self) -> &'a [u8] {
        let n = self.u() as usize;
        let z = self.i.min(self.d.len());
        let k = (z + n).min(self.d.len());
        self.i += n.div_ceil(4) * 4;
        &self.d[z..k]
    }
    pub fn s(&mut self) -> String {
        let b = self.pole();
        let b = b.strip_suffix(&[0]).unwrap_or(b);
        String::from_utf8_lossy(b).into_owned()
    }
}

/// Popisovače prijaté so správami (v poradí, v akom prišli).
pub type Popisovace = std::collections::VecDeque<OwnedFd>;

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn kodovanie_a_citanie_sprav() {
        let b = Sprava::nova(3, 2).u(7).s("wl_shm").i(-5).bajty();
        assert_eq!(b.len(), 8 + 4 + 4 + 8 + 4);
        assert_eq!(u32::from_ne_bytes([b[4], b[5], b[6], b[7]]), (28 << 16) | 2);
        let mut sk = Skladac::default();
        sk.pridaj(&b[..10]);
        assert!(sk.dalsia().is_none());
        sk.pridaj(&b[10..]);
        let u = sk.dalsia().unwrap().unwrap();
        assert_eq!((u.objekt, u.cislo), (3, 2));
        let mut c = Citac::novy(&u.data);
        assert_eq!(c.u(), 7);
        assert_eq!(c.s(), "wl_shm");
        assert_eq!(c.i(), -5);
        // za koncom: nuly, žiadna chyba
        assert_eq!(c.u(), 0);
        assert_eq!(c.s(), "");
        assert!(sk.dalsia().is_none());
        // poškodená dĺžka
        sk.pridaj(&[1, 0, 0, 0, 0, 0, 3, 0]);
        assert!(sk.dalsia().unwrap().is_err());
        // reťazec s nulou vnútri sa nerozbije
        let b = Sprava::nova(1, 0).s("a\0b").bajty();
        let mut sk = Skladac::default();
        sk.pridaj(&b);
        assert_eq!(Citac::novy(&sk.dalsia().unwrap().unwrap().data).s(), "ab");
    }
}
