//! Cieľ zápisu: fyzický disk, súbor obrazu alebo pamäť (skúšky). Formátovač, napaľovač aj kontrola
//! disku píšu cez jeden spoločný tvar, takže tie isté bajty sa dajú overiť na Linuxe (fsck) aj zapísať
//! na kartu vo Windows.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// Najväčší kus jedného zápisu alebo čítania (rovnaký ako pri zápise obrazov).
pub const KUS: usize = 4 << 20;

/// Miesto, kam sa zapisuje po celých sektoroch.
pub trait Ciel {
    /// Veľkosť sektora v bajtoch (512 alebo 4096).
    fn sektor(&self) -> u32;
    /// Veľkosť v bajtoch.
    fn velkost(&self) -> u64;
    /// Zapíše dáta od posunu; posun je násobok sektora, dĺžka najviac [`KUS`] (doplní sa nulami).
    fn zapis(&mut self, posun: u64, data: &[u8]) -> io::Result<()>;
    /// Prečíta `ciel.len()` bajtov (najviac [`KUS`]) od posunu, ktorý je násobkom sektora.
    fn citaj(&mut self, posun: u64, ciel: &mut [u8]) -> io::Result<()>;
    /// Všetko zapísané je fyzicky na médiu.
    fn dokonci(&mut self) -> io::Result<()>;
}

/// Zapíše ľubovoľne dlhé dáta po kusoch.
pub fn zapis_vsetko(c: &mut dyn Ciel, posun: u64, data: &[u8]) -> io::Result<()> {
    for (i, kus) in data.chunks(KUS).enumerate() {
        c.zapis(posun + (i * KUS) as u64, kus)?;
    }
    Ok(())
}

/// Vynuluje oblasť `[od, od + dlzka)` (zaokrúhlenú na sektory) s priebehom a možnosťou zrušiť.
pub fn vynuluj(
    c: &mut dyn Ciel,
    od: u64,
    dlzka: u64,
    priebeh: &mut dyn FnMut(u64, u64),
    zrus: &AtomicBool,
) -> io::Result<()> {
    let s = c.sektor() as u64;
    let nuly = vec![0u8; KUS];
    let koniec = (od + dlzka).div_ceil(s) * s;
    let mut p = od / s * s;
    while p < koniec {
        if zrus.load(Ordering::Relaxed) {
            return Err(io::Error::other("zrušené"));
        }
        let n = (KUS as u64).min(koniec - p) as usize;
        c.zapis(p, &nuly[..n])?;
        p += n as u64;
        priebeh(p - od.min(p), koniec - od);
    }
    Ok(())
}

/// Cieľ v pamäti – na skúšky.
#[cfg(test)]
pub struct Pamat {
    pub data: Vec<u8>,
    pub sektor: u32,
}

#[cfg(test)]
impl Pamat {
    pub fn nova(velkost: u64, sektor: u32) -> Pamat {
        Pamat { data: vec![0; velkost as usize], sektor }
    }
}

fn mimo(posun: u64, n: usize, velkost: u64) -> io::Error {
    io::Error::other(format!("mimo disku: {posun} + {n} > {velkost}"))
}

#[cfg(test)]
impl Ciel for Pamat {
    fn sektor(&self) -> u32 {
        self.sektor
    }
    fn velkost(&self) -> u64 {
        self.data.len() as u64
    }
    fn zapis(&mut self, posun: u64, data: &[u8]) -> io::Result<()> {
        let s = self.sektor as usize;
        let n = data.len().div_ceil(s) * s;
        if !posun.is_multiple_of(s as u64) || posun + n as u64 > self.velkost() {
            return Err(mimo(posun, n, self.velkost()));
        }
        let p = posun as usize;
        self.data[p..p + data.len()].copy_from_slice(data);
        self.data[p + data.len()..p + n].fill(0);
        Ok(())
    }
    fn citaj(&mut self, posun: u64, ciel: &mut [u8]) -> io::Result<()> {
        if posun + ciel.len() as u64 > self.velkost() {
            return Err(mimo(posun, ciel.len(), self.velkost()));
        }
        let p = posun as usize;
        ciel.copy_from_slice(&self.data[p..p + ciel.len()]);
        Ok(())
    }
    fn dokonci(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Súbor obrazu disku (napr. `karta.img`) – funguje na každom systéme.
pub struct Subor {
    f: File,
    velkost: u64,
    sektor: u32,
}

impl Subor {
    /// Vytvorí (alebo prepíše) súbor danej veľkosti.
    pub fn novy(cesta: &str, velkost: u64, sektor: u32) -> io::Result<Subor> {
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(cesta)?;
        f.set_len(velkost)?;
        Ok(Subor { f, velkost, sektor })
    }
}

impl Ciel for Subor {
    fn sektor(&self) -> u32 {
        self.sektor
    }
    fn velkost(&self) -> u64 {
        self.velkost
    }
    fn zapis(&mut self, posun: u64, data: &[u8]) -> io::Result<()> {
        let s = self.sektor as usize;
        let n = data.len().div_ceil(s) * s;
        if !posun.is_multiple_of(s as u64) || posun + n as u64 > self.velkost {
            return Err(mimo(posun, n, self.velkost));
        }
        self.f.seek(SeekFrom::Start(posun))?;
        self.f.write_all(data)?;
        if n > data.len() {
            self.f.write_all(&vec![0u8; n - data.len()])?;
        }
        Ok(())
    }
    fn citaj(&mut self, posun: u64, ciel: &mut [u8]) -> io::Result<()> {
        self.f.seek(SeekFrom::Start(posun))?;
        self.f.read_exact(ciel)
    }
    fn dokonci(&mut self) -> io::Result<()> {
        self.f.sync_all()
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn pamat_doplni_sektor_nulami_a_strazi_hranice() {
        let mut p = Pamat::nova(4096, 512);
        p.data.fill(0xAA);
        p.zapis(512, &[1, 2, 3]).unwrap();
        assert_eq!(&p.data[512..515], &[1, 2, 3]);
        assert!(p.data[515..1024].iter().all(|&b| b == 0), "zvyšok sektora je nulový");
        assert_eq!(p.data[1024], 0xAA);
        assert!(p.zapis(4096, &[1]).is_err(), "za koncom disku");
        assert!(p.zapis(100, &[1]).is_err(), "nezarovnaný posun");
        let mut nuly = 0;
        vynuluj(&mut p, 0, 4096, &mut |h, _| nuly = h, &AtomicBool::new(false)).unwrap();
        assert!(p.data.iter().all(|&b| b == 0));
        assert_eq!(nuly, 4096);
    }
}
