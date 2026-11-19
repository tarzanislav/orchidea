//! Poistky SADIČA: na ktorý disk sa smie zapisovať. Chyba tu by zničila dáta – pravidlá sú preto čisté
//! funkcie overené skúškami, nezávislé od operačného systému.

/// Zbernica, cez ktorú je disk pripojený.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zbernica {
    Usb,
    Sd,
    Mmc,
    /// Virtuálny disk (súbor VHD) – len pri skúškach.
    Virtualna,
    Nvme,
    Sata,
    Ina(u32),
}

impl Zbernica {
    /// Z hodnoty STORAGE_BUS_TYPE (Windows).
    pub fn z_windows(kod: u32) -> Zbernica {
        match kod {
            7 => Zbernica::Usb,
            0x0C => Zbernica::Sd,
            0x0D => Zbernica::Mmc,
            0x0E | 0x0F => Zbernica::Virtualna,
            0x11 => Zbernica::Nvme,
            0x0B => Zbernica::Sata,
            k => Zbernica::Ina(k),
        }
    }

    pub fn popis(self) -> String {
        match self {
            Zbernica::Usb => "USB".into(),
            Zbernica::Sd => "SD karta".into(),
            Zbernica::Mmc => "MMC".into(),
            Zbernica::Virtualna => "virtuálny disk".into(),
            Zbernica::Nvme => "NVMe (vnútorný)".into(),
            Zbernica::Sata => "SATA (vnútorný)".into(),
            Zbernica::Ina(k) => format!("iná zbernica ({k})"),
        }
    }
}

/// Údaje o disku.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoDisku {
    pub cislo: u32,
    pub model: String,
    pub velkost: u64,
    pub zbernica: Zbernica,
    pub vymenitelny: bool,
    /// Je na disku spustený operačný systém počítača?
    pub systemovy: bool,
}

/// Najväčší disk, ktorý SADIČ ponúkne (tabuľka MBR obrazu aj tak siaha najviac po 2 TiB).
pub const NAJVACSI: u64 = 2 << 40;

/// Smie sa na disk zapisovať? Inak vráti dôvod, ktorý sa ukáže používateľovi.
pub fn posud(d: &InfoDisku, dovol_virtualny: bool) -> Result<(), String> {
    if d.systemovy {
        return Err("je na ňom operačný systém tohto počítača".into());
    }
    match d.zbernica {
        Zbernica::Usb | Zbernica::Sd | Zbernica::Mmc => {}
        Zbernica::Virtualna if dovol_virtualny => {}
        z => return Err(format!("{} – SADIČ zapisuje len na SD karty a USB disky", z.popis())),
    }
    if d.velkost == 0 {
        return Err("nie je vložené médium (veľkosť 0)".into());
    }
    if d.velkost > NAJVACSI {
        return Err("väčší ako 2 TiB – to nie je karta ani USB disk pre Raspberry Pi".into());
    }
    Ok(())
}

/// Ľudsky čitateľná veľkosť.
pub fn velkost(b: u64) -> String {
    let gb = b as f64 / 1e9;
    if gb >= 1.0 { format!("{gb:.1} GB") } else { format!("{:.0} MB", b as f64 / 1e6) }
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn disk(zbernica: Zbernica, velkost: u64, systemovy: bool) -> InfoDisku {
        InfoDisku { cislo: 1, model: "skúška".into(), velkost, zbernica, vymenitelny: true, systemovy }
    }

    #[test]
    fn poistky() {
        let gb = 1_000_000_000;
        assert!(posud(&disk(Zbernica::Usb, 32 * gb, false), false).is_ok());
        assert!(posud(&disk(Zbernica::Sd, 64 * gb, false), false).is_ok());
        assert!(posud(&disk(Zbernica::Usb, 32 * gb, true), true).is_err(), "systémový disk nikdy");
        assert!(posud(&disk(Zbernica::Nvme, 512 * gb, false), true).is_err(), "vnútorný NVMe nikdy");
        assert!(posud(&disk(Zbernica::Sata, 512 * gb, false), true).is_err(), "vnútorný SATA nikdy");
        assert!(posud(&disk(Zbernica::Virtualna, gb, false), false).is_err(), "virtuálny len pri skúške");
        assert!(posud(&disk(Zbernica::Virtualna, gb, false), true).is_ok());
        assert!(posud(&disk(Zbernica::Usb, 0, false), false).is_err(), "bez média");
        assert!(posud(&disk(Zbernica::Usb, 3 << 40, false), false).is_err(), "príliš veľký");
        assert_eq!(Zbernica::z_windows(7), Zbernica::Usb);
        assert_eq!(Zbernica::z_windows(0x11), Zbernica::Nvme);
    }
}
