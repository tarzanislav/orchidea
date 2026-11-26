//! Dekodér zvukovej stopy Opus pre prehrávač (`DekoderZvuku`): hlavička a mapovanie kanálov podľa RFC 7845,
//! viac prúdov v jednom pakete (RFC 6716 príloha B – všetky prúdy okrem posledného sú samooddeľovacie),
//! výstupný zisk, pre-skip a prevod poradia kanálov Vorbis na spoločné poradie (`RozlozenieKanalov`).

use super::{DekoderOpus, balenie};
use crate::paket::Paket;
use crate::stopa::ParametreZvuku;
use crate::zvuk::{BlokZvuku, DekoderZvuku, RozlozenieKanalov, VzorkyZvuku};
use orchidea_jadro::{Chyba, Vysledok};

/// Hlavička identifikácie Opus (RFC 7845 §5.1; v MP4 box `dOps` – rovnaké polia vo veľkom endiáne).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlavickaOpus {
    pub kanalov: u8,
    /// Počet vzoriek pri 48 kHz, ktoré sa zo začiatku toku zahodia.
    pub predskok: u16,
    pub frekvencia_vstupu: u32,
    /// Výstupný zisk v dB · 256 (Q7.8).
    pub zisk_q8: i16,
    pub rodina: u8,
    pub prudov: u8,
    pub spojenych: u8,
    /// Pre každý výstupný kanál (poradie Vorbis pri rodine 1): index dekódovaného kanála alebo 255 (ticho).
    pub mapovanie: Vec<u8>,
}

fn zla(s: &str) -> Chyba {
    Chyba::PoskodeneData(format!("OpusHead: {s}"))
}

impl HlavickaOpus {
    /// Z konfigurácie stopy: `OpusHead` (Matroska, Ogg, TS) alebo obsah boxu `dOps` (MP4); bez konfigurácie
    /// rodina 0 s `kanalov` (1 alebo 2).
    pub fn z_konfiguracie(k: &[u8], kanalov: u8) -> Vysledok<HlavickaOpus> {
        if k.is_empty() {
            if !(1..=2).contains(&kanalov) {
                return Err(zla("bez hlavičky pre viac ako 2 kanály"));
            }
            return Ok(HlavickaOpus {
                kanalov,
                predskok: 0,
                frekvencia_vstupu: 48_000,
                zisk_q8: 0,
                rodina: 0,
                prudov: 1,
                spojenych: kanalov - 1,
                mapovanie: (0..kanalov).collect(),
            });
        }
        let (d, velky) = match k.strip_prefix(b"OpusHead") {
            Some(d) => (d, false),
            None => (k, true), // dOps
        };
        if d.len() < 11 {
            return Err(zla("krátka"));
        }
        let verzia = d[0];
        if (!velky && verzia >> 4 != 0) || (velky && verzia != 0) {
            return Err(zla("nepodporovaná verzia"));
        }
        let u16_ = |a: u8, b: u8| if velky { u16::from_be_bytes([a, b]) } else { u16::from_le_bytes([a, b]) };
        let frekvencia = if velky {
            u32::from_be_bytes([d[4], d[5], d[6], d[7]])
        } else {
            u32::from_le_bytes([d[4], d[5], d[6], d[7]])
        };
        let mut h = HlavickaOpus {
            kanalov: d[1],
            predskok: u16_(d[2], d[3]),
            frekvencia_vstupu: frekvencia,
            zisk_q8: u16_(d[8], d[9]) as i16,
            rodina: d[10],
            prudov: 1,
            spojenych: 0,
            mapovanie: Vec::new(),
        };
        if h.kanalov == 0 {
            return Err(zla("0 kanálov"));
        }
        if h.rodina == 0 {
            if h.kanalov > 2 {
                return Err(zla("rodina 0 s viac ako 2 kanálmi"));
            }
            h.spojenych = h.kanalov - 1;
            h.mapovanie = (0..h.kanalov).collect();
        } else {
            let t = d.get(11..13 + h.kanalov as usize).ok_or_else(|| zla("chýba tabuľka mapovania"))?;
            h.prudov = t[0];
            h.spojenych = t[1];
            h.mapovanie = t[2..].to_vec();
            if h.prudov == 0 || h.spojenych > h.prudov || h.prudov as u16 + h.spojenych as u16 > 255 {
                return Err(zla("neplatný počet prúdov"));
            }
            let dekodovanych = h.prudov + h.spojenych;
            if h.mapovanie.iter().any(|&m| m != 255 && m >= dekodovanych) {
                return Err(zla("mapovanie mimo prúdov"));
            }
            if h.rodina == 1 && h.kanalov > 8 {
                return Err(zla("rodina 1 s viac ako 8 kanálmi"));
            }
        }
        Ok(h)
    }

    /// Rozloženie a poradie: `poradie[c]` = index spoločného poradia pre výstupný kanál `c` hlavičky.
    fn rozlozenie(&self) -> (RozlozenieKanalov, Vec<usize>) {
        let n = self.kanalov;
        // poradie Vorbis (RFC 7845 §5.1.1.2) → poradie bitov masky (FL FR FC LFE BL BR … BC SL SR)
        let poradie: Vec<usize> = match (self.rodina, n) {
            (1, 3) => vec![0, 2, 1],
            (1, 5) => vec![0, 2, 1, 3, 4],
            (1, 6) => vec![0, 2, 1, 4, 5, 3],
            (1, 7) => vec![0, 2, 1, 5, 6, 4, 3],
            (1, 8) => vec![0, 2, 1, 6, 7, 4, 5, 3],
            _ => (0..n as usize).collect(),
        };
        let r = if self.rodina <= 1 {
            RozlozenieKanalov::predvolene(n).unwrap_or(RozlozenieKanalov::STEREO)
        } else {
            // iné rodiny (ambisonika, nedefinované): kanály bez polôh – prvých n bitov masky
            RozlozenieKanalov(((1u64 << n.min(18)) - 1) as u32)
        };
        (r, poradie)
    }
}

/// Dekodér stopy Opus: výstup pri 48 kHz v `f32`, kanály v spoločnom poradí.
#[derive(Debug)]
pub struct OpusDekoder {
    hlavicka: HlavickaOpus,
    prudy: Vec<DekoderOpus>,
    parametre: ParametreZvuku,
    poradie: Vec<usize>,
    zisk: f32,
    /// Zostávajúci pre-skip (len od začiatku toku; po posune sa nepoužije – presnosť dá predbeh 80 ms).
    predskok: usize,
    /// XOR konečných stavov rozsahových dekodérov všetkých prúdov posledného paketu (ako libopus).
    rng: u32,
}

impl OpusDekoder {
    pub fn novy(konfiguracia: &[u8], parametre: Option<ParametreZvuku>) -> Vysledok<OpusDekoder> {
        let kanalov = parametre.map_or(2, |p| p.kanalov);
        let h = HlavickaOpus::z_konfiguracie(konfiguracia, kanalov)?;
        let (rozlozenie, poradie) = h.rozlozenie();
        if rozlozenie.kanalov() != h.kanalov as u32 {
            return Err(zla("rozloženie kanálov"));
        }
        let prudy =
            (0..h.prudov).map(|s| DekoderOpus::s_kanalmi(if s < h.spojenych { 2 } else { 1 })).collect();
        let zisk = orchidea_jadro::matematika::exp(h.zisk_q8 as f64 / 256.0 / 20.0 * core::f64::consts::LN_10)
            as f32;
        Ok(OpusDekoder {
            parametre: ParametreZvuku { frekvencia: 48_000, kanalov: h.kanalov, rozlozenie, bitov: None },
            predskok: h.predskok as usize,
            hlavicka: h,
            prudy,
            poradie,
            zisk,
            rng: 0,
        })
    }

    pub fn hlavicka(&self) -> &HlavickaOpus {
        &self.hlavicka
    }

    /// Konečný stav posledného paketu (XOR prúdov) – na porovnanie s rozhodcom.
    pub fn konecny_rng(&self) -> u32 {
        self.rng
    }
}

impl DekoderZvuku for OpusDekoder {
    fn parametre(&self) -> &ParametreZvuku {
        &self.parametre
    }

    fn dekoduj(&mut self, paket: &Paket, vystup: &mut BlokZvuku) -> Vysledok<()> {
        let d = &paket.data;
        let mut poloha = 0usize;
        let mut kanaly: Vec<Vec<f32>> =
            Vec::with_capacity((self.hlavicka.prudov + self.hlavicka.spojenych) as usize);
        let prudov = self.prudy.len();
        self.rng = 0;
        for (s, dek) in self.prudy.iter_mut().enumerate() {
            let zvysok = d.get(poloha..).unwrap_or_default();
            let dlzka = if s + 1 < prudov { balenie::rozbal(zvysok, true)?.dlzka } else { zvysok.len() };
            let jeden = &zvysok[..dlzka];
            let p = if s + 1 < prudov {
                // samooddeľovací paket → bežný (bez dodatočnej dĺžky): dekóduje sa cez rozbalené rámce
                prepis_na_bezny(jeden)?
            } else {
                jeden.to_vec()
            };
            let v = dek.dekoduj_paket(&p)?;
            self.rng ^= v.konecny_rng;
            kanaly.extend(v.kanaly);
            poloha += dlzka;
        }
        let ramcov = kanaly.first().map_or(0, Vec::len);
        if kanaly.iter().any(|k| k.len() != ramcov) {
            return Err(Chyba::PoskodeneData("Opus: prúdy v pakete majú rôzne trvanie".into()));
        }
        let n = self.hlavicka.kanalov as usize;
        let mut data = vec![0.0f32; n * ramcov];
        for c in 0..n {
            let m = self.hlavicka.mapovanie[c];
            if m == 255 {
                continue;
            }
            let ciel = self.poradie[c];
            for (o, &x) in data[ciel * ramcov..(ciel + 1) * ramcov].iter_mut().zip(&kanaly[m as usize]) {
                *o = x * self.zisk;
            }
        }
        vystup.frekvencia = 48_000;
        vystup.rozlozenie = self.parametre.rozlozenie;
        vystup.ramcov = ramcov;
        vystup.vzorky = VzorkyZvuku::F32(data);
        let preskok = self.predskok.min(ramcov);
        self.predskok -= preskok;
        crate::zvuk::orez(vystup, paket.orezat_zaciatok as usize + preskok, paket.orezat_koniec as usize);
        Ok(())
    }

    fn predbeh_ns(&self) -> i64 {
        80_000_000 // RFC 7845 §4.6: najmenej 80 ms pred cieľom posunu
    }

    fn reset(&mut self) {
        for p in &mut self.prudy {
            p.reset();
        }
        self.predskok = 0;
    }
}

/// Samooddeľovací paket (príloha B) prepíše na bežný (§3.2) s tými istými rámcami.
fn prepis_na_bezny(d: &[u8]) -> Vysledok<Vec<u8>> {
    let p = balenie::rozbal(d, true)?;
    let ramce: Vec<&[u8]> = p.ramce.iter().map(|r| &d[r.clone()]).collect();
    let mut von = vec![d[0]];
    let dlzka = |n: usize, v: &mut Vec<u8>| {
        if n < 252 {
            v.push(n as u8);
        } else {
            let a = 252 + (n & 3);
            v.push(a as u8);
            v.push(((n - a) / 4) as u8);
        }
    };
    match p.toc.kod {
        0 | 1 => {}
        2 => dlzka(ramce[0].len(), &mut von),
        _ => {
            // kód 3: VBR (bez výplne) – dĺžky všetkých rámcov okrem posledného
            von.push(0x80 | ramce.len() as u8);
            for r in &ramce[..ramce.len() - 1] {
                dlzka(r.len(), &mut von);
            }
        }
    }
    for r in &ramce {
        von.extend_from_slice(r);
    }
    Ok(von)
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn hlavicka_opushead_a_dops() {
        // OpusHead 5.1: 4 prúdy, 2 spojené, mapovanie Vorbis
        let mut h = b"OpusHead".to_vec();
        h.extend_from_slice(&[1, 6, 0x38, 0x01, 0x80, 0xBB, 0, 0, 0x00, 0x01, 1, 4, 2, 0, 4, 1, 2, 3, 5]);
        let o = HlavickaOpus::z_konfiguracie(&h, 0).unwrap();
        assert_eq!((o.kanalov, o.predskok, o.frekvencia_vstupu, o.zisk_q8), (6, 312, 48_000, 256));
        assert_eq!((o.rodina, o.prudov, o.spojenych), (1, 4, 2));
        assert_eq!(o.mapovanie, [0, 4, 1, 2, 3, 5]);
        // dOps (MP4) – stereo, veľký endián
        let dops = [0, 2, 0x01, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 0];
        let o = HlavickaOpus::z_konfiguracie(&dops, 0).unwrap();
        assert_eq!((o.kanalov, o.predskok, o.spojenych, o.mapovanie.clone()), (2, 312, 1, vec![0, 1]));
        assert!(HlavickaOpus::z_konfiguracie(b"OpusHead\x01", 2).is_err());
        assert!(HlavickaOpus::z_konfiguracie(&[], 6).is_err());
    }

    #[test]
    fn samooddelovaci_na_bezny() {
        // kód 2: rámce 3 a 2 bajty; samooddeľovací má navyše dĺžku druhého rámca
        let samo = [0x02 | (1 << 3), 3, 2, 1, 2, 3, 4, 5];
        let b = prepis_na_bezny(&samo).unwrap();
        assert_eq!(b, [0x02 | (1 << 3), 3, 1, 2, 3, 4, 5]);
        let p = balenie::rozbal(&b, false).unwrap();
        assert_eq!(p.ramce, vec![2..5, 5..7]);
    }
}
