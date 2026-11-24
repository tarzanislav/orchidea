//! Jednotky NAL – rozdelenie toku Annex B, hlavička a odstránenie ochrany proti emulácii (H.265, kap. 7.3.1, B.2).

use orchidea_jadro::{Chyba, Vysledok};

/// Typy jednotiek NAL (H.265, tab. 7-1), ktoré rozbor potrebuje.
pub mod typ {
    pub const TRAIL_N: u8 = 0;
    pub const RASL_R: u8 = 9;
    pub const BLA_W_LP: u8 = 16;
    pub const IDR_W_RADL: u8 = 19;
    pub const IDR_N_LP: u8 = 20;
    pub const CRA_NUT: u8 = 21;
    pub const RSV_IRAP_23: u8 = 23;
    pub const VPS: u8 = 32;
    pub const SPS: u8 = 33;
    pub const PPS: u8 = 34;
    pub const AUD: u8 = 35;
    pub const EOS: u8 = 36;
    pub const EOB: u8 = 37;
    pub const FD: u8 = 38;
    pub const PREFIX_SEI: u8 = 39;
    pub const SUFFIX_SEI: u8 = 40;
}

/// Hlavička jednotky NAL (2 bajty).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlavickaNal {
    pub typ: u8,
    pub vrstva: u8,
    pub casova_vrstva: u8,
}

impl HlavickaNal {
    pub fn citaj(nal: &[u8]) -> Vysledok<HlavickaNal> {
        if nal.len() < 2 {
            return Err(Chyba::PoskodeneData("HEVC: jednotka NAL kratšia ako hlavička".into()));
        }
        if nal[0] & 0x80 != 0 {
            return Err(Chyba::PoskodeneData("HEVC: forbidden_zero_bit = 1".into()));
        }
        let casova = nal[1] & 7;
        if casova == 0 {
            return Err(Chyba::PoskodeneData("HEVC: nuh_temporal_id_plus1 = 0".into()));
        }
        Ok(HlavickaNal {
            typ: (nal[0] >> 1) & 0x3f,
            vrstva: ((nal[0] & 1) << 5) | (nal[1] >> 3),
            casova_vrstva: casova - 1,
        })
    }

    /// Kódovaný rez obrazu (VCL)?
    pub fn je_vcl(&self) -> bool {
        self.typ < 32
    }

    /// Obraz s náhodným prístupom (IRAP: BLA, IDR, CRA)?
    pub fn je_irap(&self) -> bool {
        (typ::BLA_W_LP..=typ::RSV_IRAP_23).contains(&self.typ)
    }

    pub fn je_idr(&self) -> bool {
        self.typ == typ::IDR_W_RADL || self.typ == typ::IDR_N_LP
    }
}

/// Jednotky NAL toku Annex B (bez štartovacích kódov, s hlavičkou). Nuly na konci jednotky
/// (trailing_zero_8bits) sa odstránia.
pub fn jednotky_annex_b(tok: &[u8]) -> Vec<&[u8]> {
    let mut zaciatky = Vec::new();
    let mut i = 0;
    while i + 3 <= tok.len() {
        if tok[i] == 0 && tok[i + 1] == 0 && tok[i + 2] == 1 {
            zaciatky.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut jednotky = Vec::with_capacity(zaciatky.len());
    for (k, &z) in zaciatky.iter().enumerate() {
        let mut koniec = zaciatky.get(k + 1).map_or(tok.len(), |&d| d - 3);
        while koniec > z && tok[koniec - 1] == 0 {
            koniec -= 1;
        }
        if koniec > z {
            jednotky.push(&tok[z..koniec]);
        }
    }
    jednotky
}

/// RBSP z obsahu jednotky NAL za hlavičkou: odstráni bajty emulation_prevention_three_byte (00 00 03).
pub fn rbsp(telo: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(telo.len());
    let mut nuly = 0;
    for &b in telo {
        if nuly >= 2 && b == 3 {
            nuly = 0;
            continue;
        }
        nuly = if b == 0 { nuly + 1 } else { 0 };
        v.push(b);
    }
    v
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn annex_b_a_ochrana() {
        let tok = [0, 0, 0, 1, 0x40, 0x01, 0xAA, 0, 0, 3, 1, 0, 0, 1, 0x42, 0x01, 0xBB, 0, 0];
        let j = jednotky_annex_b(&tok);
        assert_eq!(j.len(), 2);
        assert_eq!(j[0], &[0x40, 0x01, 0xAA, 0, 0, 3, 1]);
        assert_eq!(j[1], &[0x42, 0x01, 0xBB]);
        let h = HlavickaNal::citaj(j[0]).unwrap();
        assert_eq!((h.typ, h.vrstva, h.casova_vrstva), (typ::VPS, 0, 0));
        assert_eq!(rbsp(&j[0][2..]), vec![0xAA, 0, 0, 1]);
        assert!(HlavickaNal::citaj(&[0x40, 0x00]).is_err());
        assert!(HlavickaNal::citaj(&[0x26, 0x01]).unwrap().je_idr());
        assert!(HlavickaNal::citaj(&[0x2A, 0x01]).unwrap().je_irap());
    }
}
