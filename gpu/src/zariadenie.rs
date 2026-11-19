//! Výpočtové zariadenia: spoločné rozhranie pre GPU (Vulkan, Direct3D 12, …) aj referenčný procesor.

use crate::interpret;
use crate::ir::{Kernel, Typ};
use orchidea_jadro::{Chyba, Vysledok};
use std::fmt;
use std::sync::{Arc, OnceLock};

/// Hodnota konštanty kernelu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hodnota32 {
    F32(f32),
    U32(u32),
    I32(i32),
}

impl Hodnota32 {
    pub fn typ(self) -> Typ {
        match self {
            Hodnota32::F32(_) => Typ::F32,
            Hodnota32::U32(_) => Typ::U32,
            Hodnota32::I32(_) => Typ::I32,
        }
    }

    /// 32 bitov hodnoty (pre konštanty odovzdávané zariadeniu).
    pub fn bity(self) -> u32 {
        match self {
            Hodnota32::F32(x) => x.to_bits(),
            Hodnota32::U32(x) => x,
            Hodnota32::I32(x) => x as u32,
        }
    }
}

/// Dáta bufferu pre spustenie kernelu.
#[derive(Debug)]
pub enum DataVazby<'a> {
    F32(&'a mut [f32]),
    U32(&'a mut [u32]),
    I32(&'a mut [i32]),
}

impl DataVazby<'_> {
    pub fn dlzka(&self) -> usize {
        match self {
            DataVazby::F32(d) => d.len(),
            DataVazby::U32(d) => d.len(),
            DataVazby::I32(d) => d.len(),
        }
    }

    pub fn typ(&self) -> Typ {
        match self {
            DataVazby::F32(_) => Typ::F32,
            DataVazby::U32(_) => Typ::U32,
            DataVazby::I32(_) => Typ::I32,
        }
    }

    pub fn bajtov(&self) -> usize {
        self.dlzka() * 4
    }

    /// Dáta ako bajty (malý endian – tak ho používajú všetky podporované GPU).
    pub fn ako_bajty(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.bajtov());
        match self {
            DataVazby::F32(d) => d.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes())),
            DataVazby::U32(d) => d.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes())),
            DataVazby::I32(d) => d.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes())),
        }
        v
    }

    /// Nastaví dáta z bajtov (dĺžka musí sedieť).
    pub fn nastav_z_bajtov(&mut self, b: &[u8]) -> Vysledok<()> {
        if b.len() != self.bajtov() {
            return Err(Chyba::NezhodaTvaru("dĺžka výsledku zo zariadenia nesedí".into()));
        }
        let slova = b.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]);
        match self {
            DataVazby::F32(d) => d.iter_mut().zip(slova).for_each(|(x, s)| *x = f32::from_le_bytes(s)),
            DataVazby::U32(d) => d.iter_mut().zip(slova).for_each(|(x, s)| *x = u32::from_le_bytes(s)),
            DataVazby::I32(d) => d.iter_mut().zip(slova).for_each(|(x, s)| *x = i32::from_le_bytes(s)),
        }
        Ok(())
    }
}

/// Overí, že buffery a konštanty sedia s kernelom.
pub fn over_argumenty(kernel: &Kernel, vazby: &[DataVazby<'_>], konstanty: &[Hodnota32]) -> Vysledok<()> {
    kernel.over()?;
    if vazby.len() != kernel.vazby.len() {
        return Err(Chyba::NezhodaTvaru(format!(
            "kernel {} má {} bufferov, dostal {}",
            kernel.meno,
            kernel.vazby.len(),
            vazby.len()
        )));
    }
    for (v, d) in kernel.vazby.iter().zip(vazby) {
        if v.typ != d.typ() {
            return Err(Chyba::NezhodaTypov {
                popis: format!("buffer {} má byť {}", v.meno, v.typ),
                navrh: None,
            });
        }
        if d.dlzka() == 0 || d.dlzka() > (u32::MAX / 4) as usize {
            return Err(Chyba::MimoRozsahu(format!("buffer {} musí mať 1 až 2³⁰ prvkov", v.meno)));
        }
    }
    if konstanty.len() != kernel.konstanty.len() {
        return Err(Chyba::NezhodaTvaru(format!(
            "kernel {} má {} konštánt",
            kernel.meno,
            kernel.konstanty.len()
        )));
    }
    for (k, h) in kernel.konstanty.iter().zip(konstanty) {
        if k.typ != h.typ() {
            return Err(Chyba::NezhodaTypov {
                popis: format!("konštanta {} má byť {}", k.meno, k.typ),
                navrh: None,
            });
        }
    }
    Ok(())
}

/// Rozhranie zariadenia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Druh {
    Procesor,
    Vulkan,
    D3d12,
}

/// Druh hardvéru.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypZariadenia {
    /// Samostatná grafická karta.
    Diskretne,
    /// Grafika v procesore.
    Integrovane,
    Virtualne,
    /// Softvérový ovládač bežiaci na procesore (lavapipe, WARP) – správny, ale pomalší.
    Softverove,
    /// Referenčný interpret ORCHIDEE.
    Referencne,
}

/// Popis zariadenia.
#[derive(Debug, Clone, PartialEq)]
pub struct PopisZariadenia {
    pub druh: Druh,
    pub typ: TypZariadenia,
    pub meno: String,
    pub ovladac: String,
}

impl fmt::Display for PopisZariadenia {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} – {} ({:?}, {})", self.druh, self.meno, self.typ, self.ovladac)
    }
}

/// Výpočtové zariadenie.
pub trait Zariadenie: Send + Sync + fmt::Debug {
    fn popis(&self) -> &PopisZariadenia;

    /// Spustí kernel: nahrá buffery, vykoná `skupiny` skupín vlákien a vráti obsah bufferov, do ktorých
    /// kernel zapisuje.
    fn spusti(
        &self,
        kernel: &Kernel,
        vazby: &mut [DataVazby<'_>],
        konstanty: &[Hodnota32],
        skupiny: [u32; 3],
    ) -> Vysledok<()>;
}

/// Referenčné zariadenie – interpret na procesore. Je vždy k dispozícii.
#[derive(Debug)]
pub struct Procesor {
    popis: PopisZariadenia,
}

impl Default for Procesor {
    fn default() -> Self {
        Procesor {
            popis: PopisZariadenia {
                druh: Druh::Procesor,
                typ: TypZariadenia::Referencne,
                meno: "referenčný interpret ORCHIDEA".into(),
                ovladac: env!("CARGO_PKG_VERSION").into(),
            },
        }
    }
}

impl Zariadenie for Procesor {
    fn popis(&self) -> &PopisZariadenia {
        &self.popis
    }

    fn spusti(
        &self,
        kernel: &Kernel,
        vazby: &mut [DataVazby<'_>],
        konstanty: &[Hodnota32],
        skupiny: [u32; 3],
    ) -> Vysledok<()> {
        interpret::vykonaj(kernel, vazby, konstanty, skupiny)
    }
}

/// Zariadenia a dôvody, prečo sa niektoré nepodarilo otvoriť.
type ZoznamZariadeni = (Vec<Arc<dyn Zariadenie>>, Vec<String>);

/// Všetky dostupné zariadenia, od najvhodnejšieho. Referenčný procesor je vždy posledný.
/// Zariadenia, ktoré sa nepodarilo otvoriť, sa vrátia ako zoznam dôvodov – nič sa nezamlčí.
///
/// Zariadenia sa otvoria raz za beh programu a zdieľajú sa (ovládače neznášajú opakované vytváranie
/// a rušenie inštancií z viacerých vlákien naraz; každé zariadenie má vlastný zámok).
pub fn vsetky_zariadenia() -> (Vec<Arc<dyn Zariadenie>>, Vec<String>) {
    static ZARIADENIA: OnceLock<ZoznamZariadeni> = OnceLock::new();
    let (z, d) = ZARIADENIA.get_or_init(|| {
        let mut z: Vec<Arc<dyn Zariadenie>> = Vec::new();
        let mut dovody = Vec::new();
        if std::env::var("ORCHIDEA_BEZ_GPU").is_ok_and(|v| !v.is_empty()) {
            dovody.push("GPU vypnuté premennou ORCHIDEA_BEZ_GPU".into());
        } else {
            match crate::vulkan::Vulkan::otvor_vsetky() {
                Ok(v) => z.extend(v.into_iter().map(|x| Arc::new(x) as Arc<dyn Zariadenie>)),
                Err(e) => dovody.push(format!("Vulkan: {e}")),
            }
            #[cfg(windows)]
            match crate::d3d12::D3d12::otvor_vsetky() {
                Ok(v) => z.extend(v.into_iter().map(|x| Arc::new(x) as Arc<dyn Zariadenie>)),
                Err(e) => dovody.push(format!("Direct3D 12: {e}")),
            }
        }
        z.sort_by_key(|x| x.popis().typ);
        z.push(Arc::new(Procesor::default()));
        (z, dovody)
    });
    (z.clone(), d.clone())
}
