//! Napálenie ISO, ktoré nie je hybridné (inštalačné ISO Windows a iné): súbory sa rozbalia na nový
//! zväzok FAT32, z ktorého štartuje každý počítač s UEFI. Súbory nad 4 GB (install.wim) sa na FAT32
//! nezmestia – vtedy vznikne druhý oddiel **NTFS** (alebo exFAT) s priečinkom `sources` (osvedčený postup
//! „FAT32 na štart + NTFS na inštaláciu“). Všetko sa po zápise overí spätným čítaním.

use crate::Faza;
use crate::ciel::{self, Ciel, KUS};
use crate::fat32::Fat32;
use crate::formatuj::System;
use crate::iso::{self, Rozbor};
use crate::oddiely::{self, Oddiel, Schema, Typ};
use crate::subory::{Strom, Zapisany};
use orchidea_jadro::Hasovac;
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, Ordering};

const MIB: u64 = 1 << 20;

/// Hranica súboru pre FAT32 (pri skúškach sa dá znížiť cez SADIC_SKUSKA_HRANICA, aby sa rozdelenie
/// na dva oddiely dalo overiť aj s malým ISO).
fn hranica() -> u64 {
    if std::env::var("SADIC_SKUSKA").as_deref() == Ok("1")
        && let Some(h) = std::env::var("SADIC_SKUSKA_HRANICA").ok().and_then(|v| v.parse().ok())
    {
        return h;
    }
    iso::HRANICA_FAT32
}

/// Rozdelenie súborov: prvý oddiel FAT32 (štart) a voliteľne druhý (NTFS/exFAT, veľké súbory).
pub fn rozdelenie(r: &Rozbor) -> (Strom, Option<Strom>) {
    let hranica = hranica();
    if !r.polozky.iter().any(|p| !p.priecinok && p.velkost > hranica) {
        return (Strom::z_poloziek(&r.polozky, |_| true), None);
    }
    let na_druhy = |p: &iso::Polozka| -> bool {
        let c = p.cesta.to_ascii_lowercase();
        if r.windows() {
            // inštalácia Windows: celý priečinok sources okrem boot.wim (ten štartuje inštalátor)
            c.starts_with("sources/") && c != "sources/boot.wim"
        } else {
            !p.priecinok && p.velkost > hranica
        }
    };
    let prvy = Strom::z_poloziek(&r.polozky, |p| {
        !na_druhy(p) && !(r.windows() && p.cesta.eq_ignore_ascii_case("sources"))
    });
    let mut prvy = prvy;
    if r.windows() && r.ma("sources/boot.wim") {
        // priečinok sources musí na prvom oddiele existovať aj s boot.wim
        prvy.pridaj("sources", true, 0, None);
    }
    let druhy = Strom::z_poloziek(&r.polozky, |p| {
        na_druhy(p) || r.windows() && p.cesta.eq_ignore_ascii_case("sources")
    });
    (prvy, Some(druhy))
}

/// `h / n` z `z` bez pretečenia (priebeh pri súboroch nad 4 GB).
fn podiel(h: u64, n: u64, z: u64) -> u64 {
    (h.min(n) as u128 * z as u128 / n.max(1) as u128) as u64
}

/// Menovka zväzku z menovky ISO.
fn menovka_z(r: &Rozbor, nahradna: &str) -> String {
    if r.menovka.trim().is_empty() { nahradna.into() } else { r.menovka.clone() }
}

/// Výsledok napálenia v režime súborov.
#[derive(Debug, Clone)]
pub struct Napalene {
    pub suborov: usize,
    pub bajtov: u64,
    pub dva_oddiely: bool,
    pub druhy: System,
    pub schema: Schema,
}

impl Napalene {
    pub fn text(&self) -> String {
        format!(
            "Rozbalené: {} ({}) na {}, tabuľka {}{}. Štart cez UEFI (ponuka štartu býva na F12, F11, F8 alebo Esc).",
            crate::disk::pocet(self.suborov, "súbor", "súbory", "súborov"),
            crate::disk::velkost(self.bajtov),
            if self.dva_oddiely {
                format!("FAT32 + {} (súbory nad 4 GB)", self.druhy.nazov())
            } else {
                "FAT32".into()
            },
            self.schema.nazov(),
            if self.dva_oddiely && self.druhy == System::ExFat {
                " – s exFAT je to zatiaľ skúšobné"
            } else {
                ""
            }
        )
    }
}

/// Napáli ISO rozbalením súborov na cieľ.
pub fn napal_subory<F: Read + Seek>(
    c: &mut dyn Ciel,
    zdroj: &mut F,
    r: &Rozbor,
    schema: Schema,
    druhy: System,
    priebeh: &mut dyn FnMut(Faza, u64, u64),
    zrus: &AtomicBool,
) -> Result<Napalene, String> {
    if !r.efi() {
        return Err(
            "toto ISO nemá štart cez UEFI – po rozbalení na USB by sa nespustilo (štartuje len z CD/DVD)"
                .into(),
        );
    }
    let velkost = c.velkost();
    let sektor = c.sektor();
    let s = sektor as u64;
    let zarovnanie = oddiely::zarovnanie(velkost);
    let (strom1, strom2) = rozdelenie(r);
    let koniec_disku = velkost / s - if schema == Schema::Gpt { oddiely::gpt_koniec(sektor) } else { 0 };
    let zaciatok = zarovnanie / s;
    let chyba = |e: std::io::Error| format!("zápis na disk: {e}");
    let potrebne = strom1.velkost_suborov() + strom2.as_ref().map_or(0, Strom::velkost_suborov);
    if potrebne + 64 * MIB > velkost {
        return Err(format!(
            "ISO sa nezmestí: súbory majú {}, disk {}",
            crate::disk::velkost(potrebne),
            crate::disk::velkost(velkost)
        ));
    }
    // rozvrh oddielov
    let menovka1 = menovka_z(r, "SADIC");
    let (fat, ex, oddiel1, oddiel2) = match &strom2 {
        None => {
            let o = oddiely::jeden_oddiel(velkost, sektor, schema, Typ::Fat32, &menovka1)?;
            let f = Fat32::navrhni(o.sektory, sektor, o.zaciatok, zarovnanie, None, &menovka1)?;
            (f, None, o, None)
        }
        Some(_) => {
            // prvý oddiel: súbory + rezerva, zarovnaný; rastie, kým sa súbory nezmestia
            let z = zarovnanie / s;
            let mut dlzka = (strom1.velkost_suborov() * 21 / 20 + 64 * MIB)
                .max(if velkost >= 1 << 30 { 100 * MIB } else { 0 })
                .div_ceil(zarovnanie)
                * z;
            let f = loop {
                let f = Fat32::navrhni(dlzka, sektor, zaciatok, zarovnanie, None, &menovka1)?;
                if f.potrebne_klastre(&strom1) + 16 <= f.klastre as u64 {
                    break f;
                }
                dlzka += z.max(64 * MIB / s);
            };
            let zac2 = zaciatok + dlzka;
            let na_mib = (MIB / s).max(1);
            let dlzka2 = koniec_disku.saturating_sub(zac2).min(if schema == Schema::Mbr {
                u32::MAX as u64
            } else {
                u64::MAX
            }) / na_mib
                * na_mib;
            let e = crate::formatuj::navrhni(druhy, dlzka2, sektor, zac2, zarovnanie, None, "INSTALL")?;
            if !e.zmesti_sa(strom2.as_ref().unwrap_or(&Strom::novy())) {
                return Err("súbory sa na druhý oddiel nezmestia – disk je primalý".into());
            }
            let o1 =
                Oddiel { zaciatok, sektory: dlzka, typ: Typ::Fat32, meno: menovka1.clone(), aktivny: true };
            let o2 = Oddiel {
                zaciatok: zac2,
                sektory: dlzka2,
                typ: druhy.typ(),
                meno: "INSTALL".into(),
                aktivny: false,
            };
            (f, Some(e), o1, Some(o2))
        }
    };
    // vymazanie starých stôp: začiatok disku a posledný MiB
    ciel::vynuluj(c, 0, zaciatok * s, &mut |_, _| {}, zrus).map_err(chyba)?;
    let koniec = (velkost.saturating_sub(MIB) / s * s).max(zaciatok * s);
    ciel::vynuluj(c, koniec, velkost - koniec, &mut |_, _| {}, zrus).map_err(chyba)?;
    // zápis súborov
    let celkom = potrebne.max(1);
    let mut zaklad = 0u64;
    let mut zapisane: Vec<Zapisany> = Vec::new();
    let mut chyba_citania: Option<String> = None;
    {
        let polozky = &r.polozky;
        let mut citaj_strom = |strom: &Strom, u: usize, posun: u64, buf: &mut [u8]| -> std::io::Result<()> {
            let i = strom.uzly[u].polozka.ok_or_else(|| std::io::Error::other("uzol bez zdroja"))?;
            iso::citaj_z(zdroj, &polozky[i], posun, buf)
        };
        let data1 = strom1.velkost_suborov();
        let mut p1 = |h: u64, n: u64| priebeh(Faza::Zapis, podiel(h, n, data1), celkom);
        match fat.naplnene(
            c,
            oddiel1.zaciatok * s,
            &strom1,
            &mut |u, p, b| citaj_strom(&strom1, u, p, b),
            &mut p1,
            zrus,
        ) {
            Ok(z) => zapisane.extend(z),
            Err(e) => chyba_citania = Some(e.to_string()),
        }
        zaklad += data1;
        if chyba_citania.is_none()
            && let (Some(e), Some(o2), Some(strom2)) = (&ex, &oddiel2, &strom2)
        {
            let data2 = strom2.velkost_suborov();
            let mut p2 = |h: u64, n: u64| priebeh(Faza::Zapis, zaklad + podiel(h, n, data2), celkom);
            match e.naplnene(
                c,
                o2.zaciatok * s,
                strom2,
                &mut |u, p, b| citaj_strom(strom2, u, p, b),
                &mut p2,
                zrus,
            ) {
                Ok(z) => zapisane.extend(z),
                Err(e) => chyba_citania = Some(e.to_string()),
            }
        }
    }
    if let Some(e) = chyba_citania {
        return Err(format!("zápis súborov: {e}"));
    }
    // tabuľka oddielov nakoniec
    let oddiely: Vec<Oddiel> = std::iter::once(oddiel1.clone()).chain(oddiel2.clone()).collect();
    match schema {
        Schema::Mbr => {
            let podpis = u32::from_le_bytes(oddiely::jedinecne()[..4].try_into().unwrap_or([1; 4]));
            c.zapis(0, &oddiely::mbr(sektor, &oddiely, podpis)).map_err(chyba)?;
        }
        Schema::Gpt => {
            let (zac, zaloha_od, zaloha) = oddiely::gpt(velkost / s, sektor, &oddiely, oddiely::novy_guid());
            ciel::zapis_vsetko(c, zaloha_od, &zaloha).map_err(chyba)?;
            ciel::zapis_vsetko(c, 0, &zac).map_err(chyba)?;
        }
    }
    c.dokonci().map_err(chyba)?;
    // overenie: každý súbor sa prečíta z disku a porovná s odtlačkom zo zápisu
    let mut buf = vec![0u8; KUS];
    let mut hotovo = 0u64;
    for z in &zapisane {
        let mut h = Hasovac::novy();
        let mut v = 0u64;
        while v < z.velkost {
            if zrus.load(Ordering::Relaxed) {
                return Err("zrušené – disk nie je úplný, napáľte ho znova".into());
            }
            let n = (KUS as u64).min(z.velkost - v) as usize;
            c.citaj(z.posun + v, &mut buf[..n]).map_err(chyba)?;
            h.pridaj(&buf[..n]);
            v += n as u64;
            hotovo += n as u64;
            priebeh(Faza::Overenie, hotovo, celkom);
        }
        if h.dokonci().0 != z.odtlacok {
            return Err(format!(
                "OVERENIE ZLYHALO pri súbore v oblasti {} MB – disk môže byť poškodený alebo falošný",
                z.posun / 1_000_000
            ));
        }
    }
    Ok(Napalene { suborov: zapisane.len(), bajtov: potrebne, dva_oddiely: ex.is_some(), druhy, schema })
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::ciel::Pamat;
    use std::io::Cursor;

    #[test]
    fn windows_iso_sa_rozbali_na_fat32() {
        let obraz = crate::iso::skusky::skusobne_iso(false, true);
        let r = iso::rozober(&mut Cursor::new(&obraz)).unwrap();
        let (p1, p2) = rozdelenie(&r);
        assert!(p2.is_none(), "malé ISO: jeden oddiel");
        assert_eq!(p1.subory().len(), 4);
        let mut c = Pamat::nova(300 * MIB, 512);
        let n = napal_subory(
            &mut c,
            &mut Cursor::new(&obraz),
            &r,
            Schema::Mbr,
            System::Ntfs,
            &mut |_, _, _| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(n.suborov, 4);
        let m = oddiely::rozloz_mbr(&c.data[..512]).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].typ, 0x0C);
    }

    #[test]
    fn velky_subor_ide_na_druhy_oddiel() {
        let obraz = crate::iso::skusky::skusobne_iso(false, true);
        let mut r = iso::rozober(&mut Cursor::new(&obraz)).unwrap();
        // predstierame install.wim nad 4 GB (len pre rozdelenie)
        for p in r.polozky.iter_mut().filter(|p| p.cesta.eq_ignore_ascii_case("sources/install.wim")) {
            p.velkost = 5 << 30;
        }
        let (p1, p2) = rozdelenie(&r);
        let p2 = p2.unwrap();
        let mena =
            |s: &Strom| s.subory().iter().map(|&u| s.uzly[u].meno.to_ascii_lowercase()).collect::<Vec<_>>();
        assert!(mena(&p2).contains(&"install.wim".to_string()));
        assert!(!mena(&p1).contains(&"install.wim".to_string()));
        assert!(mena(&p1).contains(&"bootx64.efi".to_string()));
    }
}
