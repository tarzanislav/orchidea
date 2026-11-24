//! Správy SEI (H.265, kap. 7.3.5, D.2) – zatiaľ odtlačok dekódovaného obrazu (decoded picture hash), ktorým
//! skúšobné toky normy dokazujú správnosť dekodéra snímku po snímke.

use crate::bity::Citac;
use orchidea_jadro::{Chyba, Vysledok};

/// Typ správy SEI: odtlačok dekódovaného obrazu.
pub const ODTLACOK_OBRAZU: u32 = 132;

/// Správy SEI v RBSP jednotky SEI: (typ, obsah).
pub fn spravy(rbsp: &[u8]) -> Vysledok<Vec<(u32, &[u8])>> {
    let mut v = Vec::new();
    let mut i = 0;
    let citaj = |i: &mut usize| -> Vysledok<u32> {
        let mut x = 0u32;
        loop {
            let b = *rbsp.get(*i).ok_or_else(|| Chyba::PoskodeneData("HEVC: SEI sa skončila".into()))?;
            *i += 1;
            x = x
                .checked_add(b as u32)
                .ok_or_else(|| Chyba::PoskodeneData("HEVC: SEI – pretečenie".into()))?;
            if b != 0xff {
                return Ok(x);
            }
        }
    };
    // posledný bajt je rbsp_trailing_bits (0x80)
    while i < rbsp.len() && !(i == rbsp.len() - 1 && rbsp[i] == 0x80) {
        let typ = citaj(&mut i)?;
        let dlzka = citaj(&mut i)? as usize;
        let obsah =
            rbsp.get(i..i + dlzka).ok_or_else(|| Chyba::PoskodeneData("HEVC: SEI dlhšia ako dáta".into()))?;
        v.push((typ, obsah));
        i += dlzka;
    }
    Ok(v)
}

/// Odtlačok dekódovaného obrazu pre každú zložku (Y, Cb, Cr).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OdtlacokObrazu {
    Md5(Vec<[u8; 16]>),
    Crc(Vec<u16>),
    Kontrolny(Vec<u32>),
}

impl OdtlacokObrazu {
    /// decoded_picture_hash(): `zloziek` = 1 pre obraz len s jasom, inak 3.
    pub fn citaj(obsah: &[u8], zloziek: usize) -> Vysledok<OdtlacokObrazu> {
        let mut c = Citac::novy(obsah);
        let typ = c.u(8)?;
        Ok(match typ {
            0 => OdtlacokObrazu::Md5(
                (0..zloziek)
                    .map(|_| {
                        let mut m = [0u8; 16];
                        for b in m.iter_mut() {
                            *b = c.u(8)? as u8;
                        }
                        Ok(m)
                    })
                    .collect::<Vysledok<_>>()?,
            ),
            1 => OdtlacokObrazu::Crc((0..zloziek).map(|_| Ok(c.u(16)? as u16)).collect::<Vysledok<_>>()?),
            2 => OdtlacokObrazu::Kontrolny((0..zloziek).map(|_| c.u(32)).collect::<Vysledok<_>>()?),
            t => return Err(Chyba::PoskodeneData(format!("HEVC: neznámy typ odtlačku obrazu {t}"))),
        })
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn odtlacok_md5() {
        let mut rbsp = vec![132, 49, 0];
        rbsp.extend((0..48).map(|i| i as u8));
        rbsp.push(0x80);
        let s = spravy(&rbsp).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, ODTLACOK_OBRAZU);
        match OdtlacokObrazu::citaj(s[0].1, 3).unwrap() {
            OdtlacokObrazu::Md5(m) => {
                assert_eq!(m.len(), 3);
                assert_eq!(m[2][15], 47);
            }
            o => panic!("{o:?}"),
        }
        // typ nad 255 cez 0xFF
        let s = spravy(&[0xff, 0x05, 1, 9, 0x80]).unwrap();
        assert_eq!(s, vec![(260, &[9u8][..])]);
        assert!(spravy(&[5, 10, 1]).is_err());
    }
}
