//! DEFLATE (RFC 1951) a obal zlib (RFC 1950) – vlastný dekodér aj kodér.
//!
//! Na DEFLATE stojí PNG, TIFF (kompresia Deflate) aj OpenEXR (ZIP, ZIPS) a profily ICC v PNG.
//!
//! - **Dekodér** ([`rozbal`]) číta dvojúrovňovou tabuľkou Huffmanových kódov (prvých 10 bitov priamo, dlhšie
//!   kódy cez malé podtabuľky) a stráži limit výstupu – „kompresná bomba“ skončí chybou, nie vyčerpaním pamäte.
//! - **Kodér** ([`zbal`]) hľadá opakovania (LZ77) reťazou rozptylovej tabuľky s odloženým výberom („lazy
//!   matching“); každý blok dostane vlastné Huffmanove kódy a z troch spôsobov (uložený, pevný, dynamický)
//!   sa vyberie najmenší. Úroveň 0–9 mení len hĺbku hľadania, nikdy výsledok rozbalenia.

mod rozbal;
mod zbal;

pub use rozbal::{rozbal, rozbal_do};
pub use zbal::zbal;

use crate::kontrola::{Adler32, adler32};
use orchidea_jadro::{Chyba, Vysledok};

/// Základ dĺžky pre kódy 257–285 a počet doplnkových bitov (RFC 1951, 3.2.5).
pub(crate) const ZAKLAD_DLZKY: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
pub(crate) const BITY_DLZKY: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
/// Základ vzdialenosti pre kódy 0–29 a počet doplnkových bitov.
pub(crate) const ZAKLAD_VZDIALENOSTI: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
pub(crate) const BITY_VZDIALENOSTI: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
/// Poradie dĺžok kódov pre kódovanie dĺžok v dynamickom bloku.
pub(crate) const PORADIE_DLZOK: [usize; 19] =
    [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Pevné dĺžky kódov (RFC 1951, 3.2.6).
pub(crate) fn pevne_dlzky() -> ([u8; 288], [u8; 32]) {
    let mut l = [0u8; 288];
    for (i, x) in l.iter_mut().enumerate() {
        *x = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    (l, [5u8; 32])
}

/// Rozbalí údaje zlib (RFC 1950): hlavička, DEFLATE, kontrolný súčet Adler-32. `limit` je najväčšia dovolená
/// dĺžka výsledku.
pub fn zlib_rozbal(data: &[u8], limit: usize) -> Vysledok<Vec<u8>> {
    let mut von = Vec::new();
    zlib_rozbal_do(data, limit, &mut von)?;
    Ok(von)
}

/// Ako [`zlib_rozbal`], výsledok sa pridá na koniec `von`.
pub fn zlib_rozbal_do(data: &[u8], limit: usize, von: &mut Vec<u8>) -> Vysledok<usize> {
    if data.len() < 6 {
        return Err(Chyba::PoskodeneData("zlib: príliš krátke údaje".into()));
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0F != 8 || cmf >> 4 > 7 {
        return Err(Chyba::PoskodeneData(format!("zlib: neznáma metóda {cmf:#04x}")));
    }
    if !((cmf as u16) << 8 | flg as u16).is_multiple_of(31) {
        return Err(Chyba::PoskodeneData("zlib: chybná kontrola hlavičky".into()));
    }
    if flg & 0x20 != 0 {
        return Err(Chyba::PoskodeneData("zlib: preddefinovaný slovník nie je dovolený".into()));
    }
    let zaciatok = von.len();
    let precitane = rozbal_do(&data[2..], limit, von)?;
    let koniec = 2 + precitane;
    if data.len() < koniec + 4 {
        return Err(Chyba::PoskodeneData("zlib: chýba kontrolný súčet".into()));
    }
    let ocakavany = u32::from_be_bytes([data[koniec], data[koniec + 1], data[koniec + 2], data[koniec + 3]]);
    let mut a = Adler32::novy();
    a.pridaj(&von[zaciatok..]);
    if a.hodnota() != ocakavany {
        return Err(Chyba::PoskodeneData("zlib: kontrolný súčet Adler-32 nesedí".into()));
    }
    Ok(koniec + 4)
}

/// Zbalí údaje do formátu zlib s úrovňou 0–9 (6 je bežná rovnováha rýchlosti a veľkosti).
pub fn zlib_zbal(data: &[u8], uroven: u8) -> Vec<u8> {
    let uroven = uroven.min(9);
    let flevel = match uroven {
        0 | 1 => 0,
        2..=5 => 1,
        6 => 2,
        _ => 3,
    };
    let cmf = 0x78u8; // metóda 8, okno 32 kB
    let mut flg = flevel << 6;
    flg += (31 - ((cmf as u16) << 8 | flg as u16) % 31) as u8 % 31;
    let mut von = vec![cmf, flg];
    von.extend_from_slice(&zbal(data, uroven));
    von.extend_from_slice(&adler32(data).to_be_bytes());
    von
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    fn vzorky() -> Vec<Vec<u8>> {
        let mut n = Nahoda::zo_semienka(1951);
        let mut v =
            vec![Vec::new(), vec![0], b"a".to_vec(), b"abcabcabcabcabcabcabc".to_vec(), vec![7u8; 100_000]];
        v.push((0..70_000).map(|_| n.dalsie() as u8).collect());
        // text s opakovaniami a dlhými vzdialenosťami
        let slova = ["orchidea ", "skleník ", "lýra ", "dlaždica ", "pyramída ", "\n"];
        let mut t = Vec::new();
        for _ in 0..40_000 {
            t.extend_from_slice(slova[n.mensie_ako(6) as usize].as_bytes());
        }
        v.push(t);
        // obrazové riadky s malými zmenami
        let mut o = Vec::new();
        for y in 0..300u32 {
            for x in 0..900u32 {
                o.push(((x / 7 + y / 5) % 256) as u8 ^ (n.mensie_ako(16) == 0) as u8);
            }
        }
        v.push(o);
        v
    }

    #[test]
    fn tam_a_spat_kazda_uroven() {
        for data in vzorky() {
            for uroven in 0..=9 {
                let z = zlib_zbal(&data, uroven);
                assert_eq!(zlib_rozbal(&z, data.len()).unwrap(), data, "úroveň {uroven}, {} B", data.len());
            }
        }
    }

    #[test]
    fn vyssia_uroven_nie_je_vacsia() {
        let data = &vzorky()[6];
        let z1 = zlib_zbal(data, 1).len();
        let z9 = zlib_zbal(data, 9).len();
        assert!(z9 <= z1, "{z9} > {z1}");
        assert!(z9 * 4 < data.len(), "text sa má stlačiť aspoň 4×: {z9} z {}", data.len());
    }

    #[test]
    fn limit_vystupu_zastavi_bombu() {
        let data = vec![0u8; 10_000_000];
        let z = zlib_zbal(&data, 6);
        assert!(z.len() < 20_000);
        assert!(matches!(zlib_rozbal(&z, 1_000_000), Err(Chyba::Limit(_))));
        assert_eq!(zlib_rozbal(&z, 10_000_000).unwrap().len(), 10_000_000);
    }

    #[test]
    fn poskodene_udaje_su_chyba() {
        let z = zlib_zbal(b"orchidea orchidea orchidea", 6);
        let mut zle = z.clone();
        let k = zle.len() - 1;
        zle[k] ^= 1;
        assert!(zlib_rozbal(&zle, 1000).is_err());
        assert!(zlib_rozbal(&z[..z.len() - 5], 1000).is_err());
        assert!(zlib_rozbal(&[0x78, 0x9C], 1000).is_err());
    }

    #[test]
    fn znamy_tok_zo_zlib() {
        // „hello hello hello“ zbalené nástrojom zlib (úroveň 9) – nezávislý kodér
        let z =
            [0x78, 0xDA, 0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x57, 0xC8, 0x40, 0x90, 0x00, 0x3A, 0x2E, 0x06, 0x7D];
        assert_eq!(zlib_rozbal(&z, 100).unwrap(), b"hello hello hello");
    }
}
