//! Čisté formátovanie karty alebo USB disku vlastným kódom: vymaže staré tabuľky a podpisy (aj zálohu
//! GPT na konci disku a stopy po ISO), zapíše novú tabuľku oddielov a súborový systém zarovnaný podľa
//! pamäte flash. Automatická voľba zodpovedá norme SD karát: do 32 GB FAT32, nad 32 GB exFAT.

use crate::Faza;
use crate::ciel::{self, Ciel};
use crate::exfat::ExFat;
use crate::fat32::Fat32;
use crate::ntfs::Ntfs;
use crate::oddiely::{self, Oddiel, Schema, Typ};
use crate::subory::{Strom, Zvazok};
use std::sync::atomic::AtomicBool;

const MIB: u64 = 1 << 20;

/// Súborový systém.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    /// Podľa veľkosti (ako SD karty: do 32 GB FAT32, väčšie exFAT).
    Automaticky,
    Ntfs,
    Fat32,
    ExFat,
    Ext4,
    Ext3,
    Ext2,
    /// HFS+ (Mac OS Extended) bez žurnálu.
    HfsPlus,
    /// F2FS – linuxový systém pre pamäte flash.
    F2fs,
    /// XFS v5 – linuxový systém pre veľké disky.
    Xfs,
    /// UDF 2.01 na celom disku – Windows, macOS aj Linux bez ovládačov.
    Udf,
}

/// Ponuka systémov v poradí vlastníka: najprv NTFS, FAT32, exFAT a linuxové, potom ostatné.
pub const PONUKA: &[System] = &[
    System::Automaticky,
    System::Ntfs,
    System::Fat32,
    System::ExFat,
    System::Ext4,
    System::Ext3,
    System::Ext2,
    System::HfsPlus,
    System::F2fs,
    System::Udf,
    System::Xfs,
];

impl System {
    /// Text v ponuke okna.
    pub fn v_ponuke(self) -> &'static str {
        match self {
            System::Automaticky => "Automaticky",
            System::HfsPlus => "HFS+ (Mac OS Extended)",
            System::Udf => "UDF (Windows, macOS aj Linux)",
            System::F2fs => "F2FS (Linux, pamäte flash)",
            System::Xfs => "XFS (Linux, veľké disky)",
            s => s.nazov(),
        }
    }

    pub fn z_textu(t: &str) -> Option<System> {
        match t.to_ascii_lowercase().as_str() {
            "auto" | "automaticky" => Some(System::Automaticky),
            "ntfs" => Some(System::Ntfs),
            "fat32" | "fat" => Some(System::Fat32),
            "exfat" => Some(System::ExFat),
            "ext4" => Some(System::Ext4),
            "ext3" => Some(System::Ext3),
            "ext2" => Some(System::Ext2),
            "hfs+" | "hfsplus" | "hfs" | "mac" => Some(System::HfsPlus),
            "udf" => Some(System::Udf),
            "f2fs" => Some(System::F2fs),
            "xfs" => Some(System::Xfs),
            _ => None,
        }
    }

    pub fn nazov(self) -> &'static str {
        match self {
            System::Automaticky => "automaticky",
            System::Ntfs => "NTFS",
            System::Fat32 => "FAT32",
            System::ExFat => "exFAT",
            System::Ext4 => "ext4",
            System::Ext3 => "ext3",
            System::Ext2 => "ext2",
            System::HfsPlus => "HFS+",
            System::Udf => "UDF",
            System::F2fs => "F2FS",
            System::Xfs => "XFS",
        }
    }

    /// Druh oddielu v tabuľke MBR/GPT.
    pub fn typ(self) -> Typ {
        match self {
            System::Fat32 => Typ::Fat32,
            System::Ext4 | System::Ext3 | System::Ext2 | System::F2fs | System::Xfs => Typ::Linux,
            System::HfsPlus => Typ::Hfs,
            _ => Typ::ExFat, // exFAT aj NTFS: 0x07 / základné dáta
        }
    }

    /// Skutočná voľba pre disk danej veľkosti.
    pub fn pre(self, velkost: u64) -> System {
        match self {
            System::Automaticky if velkost <= 32_000_000_000 => System::Fat32,
            System::Automaticky => System::ExFat,
            s => s,
        }
    }
}

/// Navrhne zväzok daného systému (bez zápisu); chyba, ak sa na oddiel nedá.
#[allow(clippy::too_many_arguments)]
pub fn navrhni(
    system: System,
    sektory: u64,
    sektor: u32,
    zaciatok: u64,
    zarovnanie: u64,
    klaster: Option<u32>,
    menovka: &str,
) -> Result<Box<dyn Zvazok>, String> {
    Ok(match system {
        System::Fat32 => Box::new(Fat32::navrhni(sektory, sektor, zaciatok, zarovnanie, klaster, menovka)?),
        System::ExFat | System::Automaticky => {
            Box::new(ExFat::navrhni(sektory, sektor, zaciatok, zarovnanie, klaster, menovka)?)
        }
        System::Ntfs => {
            if klaster.is_some_and(|k| k != 4096) {
                return Err("NTFS zatiaľ len s klastrom 4 KB (ako predvolí Windows)".into());
            }
            Box::new(Ntfs::navrhni(sektory, sektor, zaciatok, menovka)?)
        }
        System::Ext4 | System::Ext3 | System::Ext2 => {
            if klaster.is_some_and(|k| k != 4096) {
                return Err("ext zatiaľ len s blokom 4 KB".into());
            }
            let druh = match system {
                System::Ext4 => crate::ext::Druh::Ext4,
                System::Ext3 => crate::ext::Druh::Ext3,
                _ => crate::ext::Druh::Ext2,
            };
            let _ = zaciatok;
            Box::new(crate::ext::Ext::navrhni(sektory, sektor, druh, menovka)?)
        }
        System::HfsPlus => {
            if klaster.is_some_and(|k| k != 4096) {
                return Err("HFS+ zatiaľ len s blokom 4 KB (ako predvolí macOS)".into());
            }
            Box::new(crate::hfsplus::Hfs::navrhni(sektory, sektor, menovka)?)
        }
        System::F2fs => {
            if klaster.is_some_and(|k| k != 4096) {
                return Err("F2FS má vždy blok 4 KB".into());
            }
            Box::new(crate::f2fs::F2fs::navrhni(sektory, sektor, zaciatok, menovka)?)
        }
        System::Xfs => {
            if klaster.is_some_and(|k| k != 4096) {
                return Err("XFS zatiaľ len s blokom 4 KB (ako predvolí mkfs.xfs)".into());
            }
            Box::new(crate::xfs::Xfs::navrhni(sektory, sektor, menovka)?)
        }
        System::Udf => {
            if klaster.is_some_and(|k| k != sektor) {
                return Err(format!(
                    "UDF má blok rovný sektoru disku ({sektor} B) – inak ho Windows nepripojí"
                ));
            }
            Box::new(crate::udf::Udf::navrhni(sektory, sektor, menovka)?)
        }
    })
}

/// Čo a ako naformátovať.
#[derive(Debug, Clone)]
pub struct Formatovanie {
    pub system: System,
    pub schema: Schema,
    pub menovka: String,
    /// Bajty na klaster (None = podľa veľkosti).
    pub klaster: Option<u32>,
    /// Vynulovať celý disk (pomalé; staré dáta sa nedajú obnoviť).
    pub uplne: bool,
}

impl Default for Formatovanie {
    fn default() -> Self {
        Formatovanie {
            system: System::Automaticky,
            schema: Schema::Mbr,
            menovka: String::new(),
            klaster: None,
            uplne: false,
        }
    }
}

impl Formatovanie {
    /// Z argumentov príkazového riadka (`exfat gpt menovka=FOTKY klaster=128K uplne`).
    pub fn z_argumentov<'a>(arg: impl Iterator<Item = &'a str>) -> Result<Formatovanie, String> {
        let mut f = Formatovanie::default();
        for a in arg {
            let (k, v) = a.split_once('=').unwrap_or((a, ""));
            match k.to_ascii_lowercase().as_str() {
                "mbr" => f.schema = Schema::Mbr,
                "gpt" => f.schema = Schema::Gpt,
                "uplne" | "úplne" => f.uplne = true,
                "menovka" | "meno" => f.menovka = v.to_string(),
                "klaster" => f.klaster = Some(velkost_klastra(v).ok_or(format!("neplatný klaster „{v}“"))?),
                s => f.system = System::z_textu(s).ok_or(format!("neznáma voľba „{a}“"))?,
            }
        }
        Ok(f)
    }
}

/// „32K“, „128k“, „4096“ → bajty.
pub fn velkost_klastra(t: &str) -> Option<u32> {
    let t = t.trim().to_ascii_lowercase();
    let (cislo, nasobok) = match t.strip_suffix(['k', 'm']) {
        Some(c) if t.ends_with('k') => (c, 1024),
        Some(c) => (c, 1 << 20),
        None => (t.as_str(), 1),
    };
    let n: u32 = cislo.trim_end_matches('b').parse().ok()?;
    n.checked_mul(nasobok).filter(|k| k.is_power_of_two())
}

/// Výsledok formátovania.
#[derive(Debug, Clone)]
pub struct Zhrnutie {
    pub system: System,
    pub schema: Schema,
    pub oddiel: Oddiel,
    pub klaster: u64,
    pub zarovnanie: u64,
    pub menovka: String,
    /// Bajty na sektor disku.
    pub sektor: u32,
}

impl Zhrnutie {
    pub fn text(&self) -> String {
        format!(
            "{} na celom disku ({}), tabuľka {}, klaster {}, zarovnanie {} MiB{}",
            self.system.nazov(),
            crate::disk::velkost(self.oddiel.sektory * self.sektor as u64),
            self.schema.nazov(),
            if self.klaster < 1024 {
                format!("{} B", self.klaster)
            } else {
                format!("{} KB", self.klaster / 1024)
            },
            self.zarovnanie / MIB,
            if self.menovka.is_empty() { String::new() } else { format!(", menovka „{}“", self.menovka) }
        )
    }
}

/// Naformátuje celý cieľ. `priebeh` dostáva (fáza, hotovo, celkom).
pub fn formatuj(
    c: &mut dyn Ciel,
    f: &Formatovanie,
    priebeh: &mut dyn FnMut(Faza, u64, u64),
    zrus: &AtomicBool,
) -> Result<Zhrnutie, String> {
    let velkost = c.velkost();
    let sektor = c.sektor();
    let s = sektor as u64;
    let system = f.system.pre(velkost);
    let typ = system.typ();
    // UDF ide na celý disk; pomocná tabuľka MBR má jediný oddiel od sektora 0 (Windows aj macOS)
    let schema = if system == System::Udf { Schema::Mbr } else { f.schema };
    let mut oddiel = if system == System::Udf {
        Oddiel {
            zaciatok: 0,
            sektory: (velkost / s).min(u32::MAX as u64),
            typ,
            meno: f.menovka.clone(),
            aktivny: false,
        }
    } else {
        oddiely::jeden_oddiel(velkost, sektor, schema, typ, &f.menovka)?
    };
    let zarovnanie = oddiely::zarovnanie(velkost);
    // rozvrh súborového systému sa overí ešte pred prvým zápisom
    let zvazok = navrhni(system, oddiel.sektory, sektor, oddiel.zaciatok, zarovnanie, f.klaster, &f.menovka)?;
    if oddiel.meno.is_empty() {
        oddiel.meno = "ORCHIDEA".into();
    }
    let chyba = |e: std::io::Error| format!("zápis na disk: {e}");
    // 1. vymazanie: celý disk, alebo začiatok (staré MBR/GPT/ISO) a posledný MiB (záloha GPT)
    if f.uplne {
        ciel::vynuluj(c, 0, velkost, &mut |h, n| priebeh(Faza::Mazanie, h, n), zrus).map_err(chyba)?;
    } else {
        let zaciatok = oddiel.zaciatok * s;
        ciel::vynuluj(c, 0, zaciatok, &mut |_, _| {}, zrus).map_err(chyba)?;
        let koniec = (velkost.saturating_sub(MIB) / s * s).max(zaciatok);
        ciel::vynuluj(c, koniec, velkost - koniec, &mut |_, _| {}, zrus).map_err(chyba)?;
    }
    // 2. súborový systém
    let posun = oddiel.zaciatok * s;
    zvazok
        .naplnene(
            c,
            posun,
            &Strom::novy(),
            &mut |_, _, _| Ok(()),
            &mut |h, n| priebeh(Faza::Formatovanie, h, n),
            zrus,
        )
        .map_err(chyba)?;
    let klaster = zvazok.klaster_b();
    // 3. tabuľka oddielov až nakoniec: kým nie je, systém nevidí polovičný zväzok
    match schema {
        Schema::Mbr => {
            let podpis = u32::from_le_bytes(oddiely::jedinecne()[..4].try_into().unwrap_or([1; 4]));
            c.zapis(0, &oddiely::mbr(sektor, std::slice::from_ref(&oddiel), podpis)).map_err(chyba)?;
        }
        Schema::Gpt => {
            let (zac, zaloha_od, zaloha) =
                oddiely::gpt(velkost / s, sektor, std::slice::from_ref(&oddiel), oddiely::novy_guid());
            ciel::zapis_vsetko(c, zaloha_od, &zaloha).map_err(chyba)?;
            ciel::zapis_vsetko(c, 0, &zac).map_err(chyba)?;
        }
    }
    c.dokonci().map_err(chyba)?;
    // 4. kontrola: tabuľka a zavádzací sektor zväzku sa prečítajú späť
    let mut b = vec![0u8; s as usize];
    c.citaj(0, &mut b).map_err(chyba)?;
    let z = oddiely::rozloz_mbr(&b).ok_or("kontrola: na disku nie je tabuľka oddielov")?;
    let ocakavany = if schema == Schema::Gpt { 0xEE } else { typ.mbr() };
    if z.first().map(|z| z.typ) != Some(ocakavany) {
        return Err("kontrola: tabuľka oddielov sa nezapísala správne".into());
    }
    let mut z = vec![0u8; 65_536];
    c.citaj(posun, &mut z).map_err(chyba)?;
    if !zvazok.over(&z) {
        return Err("kontrola: súborový systém sa nezapísal správne".into());
    }
    Ok(Zhrnutie { system, schema, oddiel, klaster, zarovnanie, menovka: f.menovka.clone(), sektor })
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::ciel::Pamat;

    #[test]
    fn volby_z_argumentov() {
        let f = Formatovanie::z_argumentov(
            ["exfat", "gpt", "menovka=FOTKY", "klaster=128K", "uplne"].into_iter(),
        )
        .unwrap();
        assert_eq!(f.system, System::ExFat);
        assert_eq!(f.schema, Schema::Gpt);
        assert_eq!(f.menovka, "FOTKY");
        assert_eq!(f.klaster, Some(128 * 1024));
        assert!(f.uplne);
        assert!(Formatovanie::z_argumentov(["ntfx"].into_iter()).is_err());
        assert_eq!(velkost_klastra("4096"), Some(4096));
        assert_eq!(velkost_klastra("3k"), None);
        assert_eq!(System::Automaticky.pre(16_000_000_000), System::Fat32);
        assert_eq!(System::Automaticky.pre(64_000_000_000), System::ExFat);
    }

    #[test]
    fn formatovanie_zmaze_stare_stopy() {
        for (system, schema) in [(System::Fat32, Schema::Mbr), (System::ExFat, Schema::Gpt)] {
            let mut p = Pamat::nova(200 * MIB, 512);
            p.data.fill(0xAB); // staré dáta, napr. predtým napálené ISO
            let f = Formatovanie { system, schema, menovka: "Skúška".into(), ..Formatovanie::default() };
            let z = formatuj(&mut p, &f, &mut |_, _, _| {}, &AtomicBool::new(false)).unwrap();
            assert_eq!(z.oddiel.zaciatok * 512, MIB);
            assert!(
                p.data[512 * 40..MIB as usize].iter().all(|&b| b == 0),
                "medzera pred oddielom vynulovaná"
            );
            assert!(p.data[32_768..32_774].iter().all(|&b| b == 0), "podpis ISO zmizol");
            if schema == Schema::Gpt {
                let n = p.data.len();
                assert_eq!(&p.data[n - 512..n - 504], b"EFI PART", "záloha GPT na konci");
                assert!(oddiely::rozloz_gpt(&p.data[..MIB as usize], 512).is_ok());
            } else {
                assert!(p.data[p.data.len() - 4096..].iter().all(|&b| b == 0), "koniec disku vynulovaný");
            }
        }
    }
}
