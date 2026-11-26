//! **TOC a balenie rámcov v pakete Opus** (RFC 6716, §3.1–3.4) vrátane kontroly pravidiel [R1]–[R7]
//! a samooddeľovacieho balenia z prílohy B (pre viac prúdov podľa RFC 7845 – každý prúd okrem posledného).

use orchidea_jadro::{Chyba, Vysledok};

/// Režim podľa konfigurácie v TOC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rezim {
    Silk,
    Hybrid,
    Celt,
}

/// Šírka pásma.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pasmo {
    /// úzke pásmo 4 kHz (vzorkovanie 8 kHz)
    Uzke,
    /// stredné 6 kHz (12 kHz)
    Stredne,
    /// široké 8 kHz (16 kHz)
    Siroke,
    /// superširoké 12 kHz (24 kHz)
    Supersiroke,
    /// plné 20 kHz (48 kHz)
    Plne,
}

impl Pasmo {
    /// Vzorkovacia frekvencia zodpovedajúca šírke pásma.
    pub fn frekvencia(self) -> u32 {
        match self {
            Pasmo::Uzke => 8000,
            Pasmo::Stredne => 12000,
            Pasmo::Siroke => 16000,
            Pasmo::Supersiroke => 24000,
            Pasmo::Plne => 48000,
        }
    }
}

/// Rozbor bajtu TOC (tabuľka 2 v §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toc {
    pub konfiguracia: u8,
    pub rezim: Rezim,
    pub pasmo: Pasmo,
    /// Trvanie rámca vo vzorkách pri 48 kHz (120, 240, 480, 960, 1920, 2880).
    pub vzoriek_48k: u32,
    pub stereo: bool,
    pub kod: u8,
}

impl Toc {
    pub fn z_bajtu(b: u8) -> Toc {
        let k = b >> 3;
        let (rezim, pasmo, vzoriek_48k) = match k {
            0..=11 => (
                Rezim::Silk,
                [Pasmo::Uzke, Pasmo::Stredne, Pasmo::Siroke][(k / 4) as usize],
                [480, 960, 1920, 2880][(k % 4) as usize],
            ),
            12..=15 => (
                Rezim::Hybrid,
                if k < 14 { Pasmo::Supersiroke } else { Pasmo::Plne },
                [480, 960][(k % 2) as usize],
            ),
            _ => (
                Rezim::Celt,
                [Pasmo::Uzke, Pasmo::Siroke, Pasmo::Supersiroke, Pasmo::Plne][((k - 16) / 4) as usize],
                [120, 240, 480, 960][(k % 4) as usize],
            ),
        };
        Toc { konfiguracia: k, rezim, pasmo, vzoriek_48k, stereo: b & 4 != 0, kod: b & 3 }
    }
}

/// Rozbalený paket: TOC a rozsahy rámcov v pôvodných bajtoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paket {
    pub toc: Toc,
    pub ramce: Vec<std::ops::Range<usize>>,
    /// Pri samooddeľovacom balení: koľko bajtov paket zabral.
    pub dlzka: usize,
}

fn zly(text: &str) -> Chyba {
    Chyba::PoskodeneData(format!("Opus: {text}"))
}

/// Dĺžka rámca v 1–2 bajtoch (§3.2.1): 0 = bez rámca, 1–251 priamo, 252–255: druhý bajt · 4 + prvý.
fn dlzka_ramca(d: &[u8], i: &mut usize) -> Vysledok<usize> {
    let b0 = *d.get(*i).ok_or_else(|| zly("chýba dĺžka rámca"))? as usize;
    *i += 1;
    if b0 < 252 {
        return Ok(b0);
    }
    let b1 = *d.get(*i).ok_or_else(|| zly("chýba druhý bajt dĺžky rámca"))? as usize;
    *i += 1;
    Ok(b1 * 4 + b0)
}

/// Rozbalí paket podľa §3.2 (`samooddelovaci` = príloha B: navyše dĺžka posledného rámca, resp. dĺžka rámca pri
/// kódoch 0 a 3 CBR, a paket nemusí siahať po koniec `d`).
pub fn rozbal(d: &[u8], samooddelovaci: bool) -> Vysledok<Paket> {
    let toc = Toc::z_bajtu(*d.first().ok_or_else(|| zly("prázdny paket [R1]"))?);
    let mut i = 1usize;
    let mut koniec = d.len();
    let mut vypln = 0usize;
    let mut ramce = Vec::new();
    match toc.kod {
        0 => {
            let n = if samooddelovaci { dlzka_ramca(d, &mut i)? } else { koniec - i };
            ramce.push(n);
        }
        1 => {
            if samooddelovaci {
                let n = dlzka_ramca(d, &mut i)?;
                ramce.extend([n, n]);
            } else {
                let zvysok = koniec - i;
                if !zvysok.is_multiple_of(2) {
                    return Err(zly("kód 1 s nepárnou dĺžkou [R3]"));
                }
                ramce.extend([zvysok / 2, zvysok / 2]);
            }
        }
        2 => {
            let n1 = dlzka_ramca(d, &mut i)?;
            let n2 = if samooddelovaci {
                dlzka_ramca(d, &mut i)?
            } else {
                (koniec - i).checked_sub(n1).ok_or_else(|| zly("kód 2: prvý rámec presahuje paket [R4]"))?
            };
            ramce.extend([n1, n2]);
        }
        _ => {
            let b = *d.get(i).ok_or_else(|| zly("kód 3 bez bajtu počtu rámcov [R6]"))?;
            i += 1;
            let m = (b & 0x3F) as usize;
            if m == 0 || m as u32 * toc.vzoriek_48k > 5760 {
                return Err(zly("počet rámcov 0 alebo viac ako 120 ms [R5]"));
            }
            let vbr = b & 0x80 != 0;
            if b & 0x40 != 0 {
                // Opus padding: 255 = 254 bajtov a pokračuje, inak 0–254
                loop {
                    let p = *d.get(i).ok_or_else(|| zly("chýba dĺžka výplne [R6]"))? as usize;
                    i += 1;
                    if p == 255 {
                        vypln += 254;
                    } else {
                        vypln += p;
                        break;
                    }
                }
                if !samooddelovaci {
                    koniec = koniec
                        .checked_sub(vypln)
                        .filter(|&k| k >= i)
                        .ok_or_else(|| zly("výplň presahuje paket [R6, R7]"))?;
                }
            }
            if vbr {
                for _ in 0..m - 1 {
                    ramce.push(dlzka_ramca(d, &mut i)?);
                }
                if samooddelovaci {
                    ramce.push(dlzka_ramca(d, &mut i)?);
                } else {
                    let sucet: usize = ramce.iter().sum();
                    let posledny = (koniec.checked_sub(i))
                        .and_then(|z| z.checked_sub(sucet))
                        .ok_or_else(|| zly("dĺžky rámcov presahujú paket [R7]"))?;
                    ramce.push(posledny);
                }
            } else {
                let n = if samooddelovaci {
                    dlzka_ramca(d, &mut i)?
                } else {
                    let r = koniec.checked_sub(i).ok_or_else(|| zly("kód 3 CBR presahuje paket [R6]"))?;
                    if !r.is_multiple_of(m) {
                        return Err(zly("kód 3 CBR: zvyšok nie je násobok počtu rámcov [R6]"));
                    }
                    r / m
                };
                ramce.extend(std::iter::repeat_n(n, m));
            }
        }
    }
    let mut rozsahy = Vec::with_capacity(ramce.len());
    for n in ramce {
        if n > 1275 {
            return Err(zly("rámec dlhší ako 1275 B [R2]"));
        }
        let po = i.checked_add(n).filter(|&p| p <= koniec).ok_or_else(|| zly("rámec presahuje paket"))?;
        rozsahy.push(i..po);
        i = po;
    }
    // samooddeľovací paket končí za rámcami a výplňou
    let dlzka = if samooddelovaci {
        i.checked_add(vypln).filter(|&k| k <= d.len()).ok_or_else(|| zly("výplň presahuje paket"))?
    } else {
        d.len()
    };
    Ok(Paket { toc, ramce: rozsahy, dlzka })
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod skusky {
    use super::*;

    #[test]
    fn toc() {
        let t = Toc::z_bajtu(1 << 3);
        assert_eq!(
            (t.rezim, t.pasmo, t.vzoriek_48k, t.stereo, t.kod),
            (Rezim::Silk, Pasmo::Uzke, 960, false, 0)
        );
        let t = Toc::z_bajtu((15 << 3) | 4 | 3);
        assert_eq!(
            (t.rezim, t.pasmo, t.vzoriek_48k, t.stereo, t.kod),
            (Rezim::Hybrid, Pasmo::Plne, 960, true, 3)
        );
        let t = Toc::z_bajtu(20 << 3);
        assert_eq!((t.rezim, t.pasmo, t.vzoriek_48k), (Rezim::Celt, Pasmo::Siroke, 120));
    }

    #[test]
    fn kody_0_az_3_a_pravidla() {
        let p = rozbal(&[1 << 3, 9, 9, 9], false).unwrap();
        assert_eq!(p.ramce, [1..4]);
        assert_eq!(rozbal(&[(29 << 3) | 1, 1, 2, 3, 4], false).unwrap().ramce, vec![1..3, 3..5]);
        assert!(rozbal(&[(29 << 3) | 1, 1, 2, 3], false).is_err(), "R3");
        // kód 2: N1 = 2
        assert_eq!(rozbal(&[(15 << 3) | 2, 2, 7, 7, 8], false).unwrap().ramce, vec![2..4, 4..5]);
        assert!(rozbal(&[(15 << 3) | 2, 5, 1], false).is_err(), "R4");
        assert_eq!(rozbal(&[(15 << 3) | 2, 0], false).unwrap().ramce, vec![2..2, 2..2]);
        assert!(rozbal(&[(15 << 3) | 2, 252], false).is_err());
        // dvojbajtová dĺžka: 253 + 4·1 = 257
        let mut d = vec![(15 << 3) | 2, 253, 1];
        d.resize(3 + 257 + 3, 1);
        assert_eq!(rozbal(&d, false).unwrap().ramce, vec![3..260, 260..263]);
        // kód 3 CBR 4 rámce po 2 B, výplň 3 B (bajt dĺžky 2 + 2 B výplne… P = 1 + 2)
        let p = rozbal(&[(31 << 3) | 3, 0x44, 2, 1, 1, 2, 2, 3, 3, 4, 4, 0, 0], false).unwrap();
        assert_eq!(p.ramce, vec![3..5, 5..7, 7..9, 9..11]);
        // kód 3 VBR: 2 rámce, N1 = 1
        assert_eq!(rozbal(&[(15 << 3) | 3, 0x82, 1, 5, 6, 6], false).unwrap().ramce, vec![3..4, 4..6]);
        // výplň 255 → 254 + ďalší bajt
        let mut d = vec![(31 << 3) | 3, 0x41, 255, 1, 7, 7];
        d.resize(d.len() + 255, 0);
        assert_eq!(rozbal(&d, false).unwrap().ramce, [4..6]);
        assert!(rozbal(&[(31 << 3) | 3, 0x00], false).is_err(), "M = 0");
        assert!(rozbal(&[(3 << 3) | 3, 0x03, 0, 0, 0], false).is_err(), "180 ms");
        assert!(rozbal(&[], false).is_err(), "R1");
        assert!(rozbal(&[(31 << 3) | 3, 0x03, 1, 1], false).is_err(), "CBR nie je násobok");
    }

    #[test]
    fn samooddelovacie() {
        // kód 0 s dĺžkou 2, za ním ďalší paket
        let p = rozbal(&[1 << 3, 2, 7, 7, 0xAA, 0xBB], true).unwrap();
        assert_eq!((p.ramce.as_slice(), p.dlzka), (&[2..4][..], 4));
        let p = rozbal(&[(15 << 3) | 2, 1, 2, 7, 8, 8, 0xAA], true).unwrap();
        assert_eq!((p.ramce.clone(), p.dlzka), (vec![3..4, 4..6], 6));
    }
}
