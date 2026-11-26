//! **PCM** – nekomprimované vzorky (etapa L2): celé čísla 8–32 bitov malo- aj veľkokoncové, so znamienkom aj bez
//! (8-bitové), plávajúca čiarka 32 a 64 bitov. Celé čísla idú bitovo presne; vzorky užšie ako ich kontajner
//! (napríklad 20 bitov v 3 bajtoch) sú zarovnané k vyšším bitom a posunú sa na svoju hĺbku.

use crate::paket::Paket;
use crate::stopa::{CisloPcm, ParametreZvuku, PopisPcm, PoradieBajtov};
use crate::zvuk::{BlokZvuku, DekoderZvuku, VzorkyZvuku, orez};
use orchidea_jadro::{Chyba, Vysledok};

/// Dekodér PCM.
#[derive(Debug, Clone)]
pub struct PcmDekoder {
    popis: PopisPcm,
    parametre: ParametreZvuku,
}

impl PcmDekoder {
    pub fn novy(popis: PopisPcm, parametre: ParametreZvuku) -> Vysledok<PcmDekoder> {
        let ok = match popis.cislo {
            CisloPcm::Plavajuce => matches!((popis.bitov, popis.bajtov), (32, 4) | (64, 8)),
            _ => (1..=4).contains(&popis.bajtov) && (1..=popis.bajtov * 8).contains(&popis.bitov),
        };
        if !ok || parametre.kanalov == 0 || parametre.rozlozenie.kanalov() != parametre.kanalov as u32 {
            return Err(Chyba::MimoRozsahu(format!(
                "nepodporené PCM {popis:?}, {} kanálov",
                parametre.kanalov
            )));
        }
        Ok(PcmDekoder { popis, parametre })
    }
}

impl DekoderZvuku for PcmDekoder {
    fn parametre(&self) -> &ParametreZvuku {
        &self.parametre
    }

    fn dekoduj(&mut self, paket: &Paket, vystup: &mut BlokZvuku) -> Vysledok<()> {
        let p = self.popis;
        let k = self.parametre.kanalov as usize;
        let b = p.bajtov as usize;
        let ramec = k * b;
        if !paket.data.len().is_multiple_of(ramec) {
            return Err(Chyba::PoskodeneData(format!(
                "PCM: {} B nie je celý počet rámcov po {ramec} B",
                paket.data.len()
            )));
        }
        let n = paket.data.len() / ramec;
        let precitaj = |s: &[u8]| -> u64 {
            match p.poradie {
                PoradieBajtov::MaloKoncove => s.iter().rev().fold(0u64, |x, &c| (x << 8) | c as u64),
                PoradieBajtov::VelkoKoncove => s.iter().fold(0u64, |x, &c| (x << 8) | c as u64),
            }
        };
        vystup.frekvencia = self.parametre.frekvencia;
        vystup.rozlozenie = self.parametre.rozlozenie;
        vystup.ramcov = n;
        match p.cislo {
            CisloPcm::Plavajuce => {
                let mut data = match std::mem::replace(&mut vystup.vzorky, VzorkyZvuku::F32(Vec::new())) {
                    VzorkyZvuku::F32(d) => d,
                    _ => Vec::new(),
                };
                data.clear();
                data.resize(n * k, 0.0);
                for (i, s) in paket.data.chunks_exact(b).enumerate() {
                    let u = precitaj(s);
                    // f64 sa zaokrúhli na f32 (24 bitov mantisy – viac, než prenesie výstup)
                    let x = if b == 4 { f32::from_bits(u as u32) } else { f64::from_bits(u) as f32 };
                    data[(i % k) * n + i / k] = x;
                }
                vystup.vzorky = VzorkyZvuku::F32(data);
            }
            cislo => {
                let mut data = match std::mem::replace(&mut vystup.vzorky, VzorkyZvuku::F32(Vec::new())) {
                    VzorkyZvuku::Cele { data, .. } => data,
                    _ => Vec::new(),
                };
                data.clear();
                data.resize(n * k, 0);
                let siroke = 8 * b as u32;
                let posun = siroke - p.bitov as u32;
                for (i, s) in paket.data.chunks_exact(b).enumerate() {
                    let u = precitaj(s);
                    let v = match cislo {
                        CisloPcm::BezZnamienka => u as i64 - (1i64 << (siroke - 1)),
                        _ => ((u << (64 - siroke)) as i64) >> (64 - siroke),
                    };
                    data[(i % k) * n + i / k] = (v >> posun) as i32;
                }
                vystup.vzorky = VzorkyZvuku::Cele { bitov: p.bitov, data };
            }
        }
        orez(vystup, paket.orezat_zaciatok as usize, paket.orezat_koniec as usize);
        Ok(())
    }

    fn predbeh_ns(&self) -> i64 {
        0
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::zvuk::RozlozenieKanalov;

    fn dek(bitov: u8, bajtov: u8, poradie: PoradieBajtov, cislo: CisloPcm, kanalov: u8) -> PcmDekoder {
        let par = ParametreZvuku {
            frekvencia: 48_000,
            kanalov,
            rozlozenie: RozlozenieKanalov::predvolene(kanalov).unwrap(),
            bitov: Some(bitov),
        };
        PcmDekoder::novy(PopisPcm { bitov, bajtov, poradie, cislo }, par).unwrap()
    }

    fn cele(d: &mut PcmDekoder, bajty: &[u8]) -> Vec<i32> {
        let mut b = BlokZvuku::cele(48_000, RozlozenieKanalov::STEREO, 16);
        d.dekoduj(&Paket { data: bajty.to_vec(), ..Paket::default() }, &mut b).unwrap();
        match b.vzorky {
            VzorkyZvuku::Cele { data, .. } => data,
            _ => unreachable!(),
        }
    }

    #[test]
    fn varianty() {
        use CisloPcm::*;
        use PoradieBajtov::*;
        // 16 b malokoncové stereo: L = [1, −2], P = [256, −32768] → planárne
        let mut d = dek(16, 2, MaloKoncove, SoZnamienkom, 2);
        assert_eq!(cele(&mut d, &[1, 0, 0, 1, 0xFE, 0xFF, 0, 0x80]), [1, -2, 256, -32768]);
        // 24 b veľkokoncové mono
        let mut d = dek(24, 3, VelkoKoncove, SoZnamienkom, 1);
        assert_eq!(cele(&mut d, &[0x80, 0, 0, 0x7F, 0xFF, 0xFF]), [-8_388_608, 8_388_607]);
        // 8 b bez znamienka
        let mut d = dek(8, 1, MaloKoncove, BezZnamienka, 1);
        assert_eq!(cele(&mut d, &[0, 128, 255]), [-128, 0, 127]);
        // 20 bitov v 3 bajtoch (zarovnané k vyšším bitom)
        let mut d = dek(20, 3, MaloKoncove, SoZnamienkom, 1);
        assert_eq!(cele(&mut d, &[0x10, 0, 0x80]), [-524_287]);
        // 32 b
        let mut d = dek(32, 4, MaloKoncove, SoZnamienkom, 1);
        assert_eq!(cele(&mut d, &[0xFF, 0xFF, 0xFF, 0x7F]), [i32::MAX]);
        // nekompletný rámec je chyba
        let mut b = BlokZvuku::cele(48_000, RozlozenieKanalov::MONO, 32);
        assert!(d.dekoduj(&Paket { data: vec![1, 2, 3], ..Paket::default() }, &mut b).is_err());
        // plávajúce 64 b, orezanie začiatku a konca
        let mut d = dek(64, 8, MaloKoncove, Plavajuce, 1);
        let data: Vec<u8> = [0.5f64, -0.25, 1.0, 0.125].iter().flat_map(|x| x.to_le_bytes()).collect();
        let mut b = BlokZvuku::f32(48_000, RozlozenieKanalov::MONO);
        d.dekoduj(&Paket { data, orezat_zaciatok: 1, orezat_koniec: 1, ..Paket::default() }, &mut b).unwrap();
        assert_eq!(b.vzorky, VzorkyZvuku::F32(vec![-0.25, 1.0]));
        assert_eq!(b.ramcov, 2);
    }
}
