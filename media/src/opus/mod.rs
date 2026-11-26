//! **Opus** (etapa L7, R302) – dekodér podľa textu RFC 6716 a RFC 8251 (bez normatívneho kódu prílohy A RFC 6716;
//! nejasnosti rozhodujú skúšobné vektory a nezávislý rozhodca).
//!
//! Moduly: [`rozsah`] – rozsahový dekodér (§4.1), [`balenie`] – TOC a balenie rámcov v pakete (§3), [`silk`] –
//! vrstva LP (§4.2).

use balenie::{Rezim, Toc};
use orchidea_jadro::{Chyba, Vysledok};
use silk::{DekoderSilk, PasmoSilk, VystupSilk};

pub mod balenie;
pub mod rozsah;
pub mod silk;
mod silk_tabulky;

/// Trvanie jedného rámca podľa konfigurácie v TOC (stotiny ms → ns): SILK 10/20/40/60 ms, hybrid 10/20 ms,
/// CELT 2,5/5/10/20 ms (RFC 6716, tabuľka 2).
fn trvanie_ramca_ns(konfiguracia: u8) -> u64 {
    match konfiguracia {
        0..=11 => [10_000_000, 20_000_000, 40_000_000, 60_000_000][(konfiguracia % 4) as usize],
        12..=15 => [10_000_000, 20_000_000][(konfiguracia % 2) as usize],
        _ => [2_500_000, 5_000_000, 10_000_000, 20_000_000][(konfiguracia % 4) as usize],
    }
}

/// Trvanie paketu Opus v ns z TOC a počtu rámcov (kód 0: 1 rámec, 1 a 2: dva, 3: počet v druhom bajte).
/// `None` pri prázdnom alebo neplatnom pakete (paket dlhší ako 120 ms je podľa RFC 6716 neplatný).
pub fn trvanie_paketu_ns(paket: &[u8]) -> Option<u64> {
    let toc = *paket.first()?;
    let ramcov = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*paket.get(1)? & 0x3F) as u64,
    };
    let t = ramcov * trvanie_ramca_ns(toc >> 3);
    (ramcov > 0 && t <= 120_000_000).then_some(t)
}

/// Výsledok dekódovania paketu.
#[derive(Debug, Clone, Default)]
pub struct VystupPaketu {
    /// Vzorky po kanáloch pri frekvencii `frekvencia`.
    pub kanaly: Vec<Vec<f32>>,
    pub frekvencia: u32,
    /// Konečný stav rozsahového dekodéra posledného rámca (zhoda s kodérom = bitovo správne dekódovanie).
    pub konecny_rng: u32,
}

/// Dekodér Opus (zatiaľ vrstva SILK – režim SILK-only; CELT a hybrid pribudnú).
#[derive(Debug, Clone, Default)]
pub struct DekoderOpus {
    silk: DekoderSilk,
    posledny_rezim: Option<Rezim>,
}

impl DekoderOpus {
    pub fn novy() -> DekoderOpus {
        DekoderOpus::default()
    }

    /// Dekóduje paket; výstup SILK pri vnútornej frekvencii (8, 12 alebo 16 kHz).
    pub fn dekoduj_paket(&mut self, data: &[u8]) -> Vysledok<VystupPaketu> {
        let p = balenie::rozbal(data, false)?;
        let toc: Toc = p.toc;
        let mut vystup =
            VystupPaketu { kanaly: vec![Vec::new(); 1 + toc.stereo as usize], ..VystupPaketu::default() };
        for r in &p.ramce {
            let ramec = &data[r.clone()];
            if toc.rezim != Rezim::Silk {
                return Err(Chyba::MimoRozsahu("Opus: režim CELT a hybrid ešte nie je (etapa L7)".into()));
            }
            if self.posledny_rezim == Some(Rezim::Celt) {
                self.silk.reset();
            }
            self.posledny_rezim = Some(toc.rezim);
            let pasmo = match toc.pasmo {
                balenie::Pasmo::Uzke => PasmoSilk::Nb,
                balenie::Pasmo::Stredne => PasmoSilk::Mb,
                _ => PasmoSilk::Wb,
            };
            let ms = toc.vzoriek_48k / 48;
            let mut rd = rozsah::RozsahovyDekoder::novy(ramec);
            let VystupSilk { frekvencia, kanaly } = if ramec.len() <= 1 {
                // prázdny rámec (DTX alebo strata): ticho – PLC nie je normatívne
                let n = (pasmo.frekvencia() * ms / 1000) as usize;
                VystupSilk {
                    frekvencia: pasmo.frekvencia(),
                    kanaly: vec![vec![0.0; n]; 1 + toc.stereo as usize],
                }
            } else {
                self.silk.dekoduj(&mut rd, pasmo, ms, toc.stereo)
            };
            // nadbytočný rámec CELT (§4.5.1): pri SILK-only aspoň 17 zvyšných bitov
            if ramec.len() > 1 && rd.zostava_bitov() >= 17 {
                let _ = rd.pdf(&[1, 1]);
            }
            vystup.frekvencia = frekvencia;
            for (v, k) in vystup.kanaly.iter_mut().zip(kanaly) {
                v.extend(k);
            }
            vystup.konecny_rng = rd.rng();
        }
        Ok(vystup)
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn trvanie_z_toc() {
        assert_eq!(trvanie_paketu_ns(&[12 << 3]), Some(10_000_000)); // hybrid 10 ms
        assert_eq!(trvanie_paketu_ns(&[15 << 3]), Some(20_000_000));
        assert_eq!(trvanie_paketu_ns(&[(31 << 3) | 1]), Some(40_000_000)); // CELT 20 ms × 2
        assert_eq!(trvanie_paketu_ns(&[(16 << 3) | 3, 0x05]), Some(12_500_000)); // CELT 2,5 ms × 5
        assert_eq!(trvanie_paketu_ns(&[(3 << 3) | 3, 0x03]), None); // 3 × 60 ms > 120 ms
        assert_eq!(trvanie_paketu_ns(&[(3 << 3) | 3, 0x00]), None);
        assert_eq!(trvanie_paketu_ns(&[(3 << 3) | 3]), None);
        assert_eq!(trvanie_paketu_ns(&[]), None);
    }
}
