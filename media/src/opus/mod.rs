//! **Opus** (etapa L7, R302) – dekodér podľa textu RFC 6716 a RFC 8251 (bez normatívneho kódu prílohy A RFC 6716;
//! nejasnosti rozhodujú skúšobné vektory a nezávislý rozhodca).
//!
//! Moduly: [`rozsah`] – rozsahový dekodér (§4.1), [`balenie`] – TOC a balenie rámcov v pakete (§3), [`silk`] –
//! vrstva LP (§4.2), [`celt`] – vrstva MDCT (§4.3). CELT podľa rozhodnutia vlastníka A (#15): z prílohy A sa berú
//! iba číselné tabuľky (`celt_tabulky`), čo text neopisuje, je odvodené meraním proti libopus.

use balenie::{Rezim, Toc};
use celt::DekoderCelt;
use orchidea_jadro::Vysledok;
use prevzorkovanie::Prevzorkovac;
use silk::{DekoderSilk, PasmoSilk, VystupSilk};

pub mod balenie;
pub mod celt;
mod celt_pasma;
#[allow(clippy::excessive_precision, clippy::unreadable_literal)]
mod celt_tabulky;
pub mod dekoder;
pub mod mdct;
pub mod prevzorkovanie;
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

/// Pokrytie režimov a prechodov (počty rámcov).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatistikaOpus {
    pub silk: u64,
    pub hybrid: u64,
    pub celt: u64,
    /// nadbytočný rámec CELT na začiatku (prechod CELT → SILK/hybrid) a na konci (SILK/hybrid → CELT)
    pub nadbytocne_zaciatok: u64,
    pub nadbytocne_koniec: u64,
    /// prechody bez nadbytočnosti (nenormatívne doplnenie)
    pub prechody_bez: u64,
    /// dobeh CELT pri prechode hybrid → SILK
    pub dobeh_hybridu: u64,
}

/// Dekodér Opus: SILK, hybrid a CELT s prechodmi (RFC 6716 §4.5); výstup pri 48 kHz (SILK sa prevzorkuje).
#[derive(Debug, Clone, Default)]
pub struct DekoderOpus {
    silk: DekoderSilk,
    celt: Option<DekoderCelt>,
    prevzorkovace: Vec<Prevzorkovac>,
    /// Výstupné kanály dekodéra (z hlavičky toku alebo z prvého paketu).
    kanalov: Option<usize>,
    posledny_rezim: Option<Rezim>,
    /// Predošlý rámec mal nadbytočný rámec CELT na konci (prechod do CELT) – CELT sa pred ďalším nevynuluje.
    posledna_nadbytocnost: bool,
    /// Skúšobný režim: SILK-only pri vnútornej frekvencii bez prevzorkovania (porovnanie s libopus pri 8–16 kHz).
    vnutorna_silk: bool,
    statistika: StatistikaOpus,
    /// Dĺžka posledného paketu (na doplnenie straty).
    posledne_vzoriek: usize,
}

/// Konečný stav rng, ktorý vracia libopus: XOR rozsahových dekodérov rámca a nadbytočného rámca.
struct VystupRamca {
    kanaly: Vec<Vec<f32>>,
    frekvencia: u32,
    rng: u32,
}

fn na_kanaly(mut k: Vec<Vec<f32>>, kanalov: usize) -> Vec<Vec<f32>> {
    if k.len() == kanalov {
        return k;
    }
    if kanalov == 2 {
        let a = k[0].clone();
        k.push(a);
        k
    } else {
        let r = k.pop().unwrap_or_default();
        for (l, r) in k[0].iter_mut().zip(r) {
            *l = 0.5 * (*l + r);
        }
        k
    }
}

/// Prekrytie oknom CELT (výkonovo komplementárne): `von[i] = w²·b[i] + (1 − w²)·a[i]`, w = okno CELT.
fn prekry(a: &[f32], b: &[f32], von: &mut [f32], okno: &[f32]) {
    for i in 0..von.len().min(okno.len()) {
        let w = okno[i] * okno[i];
        von[i] = w * b[i] + (1.0 - w) * a[i];
    }
}

impl DekoderOpus {
    pub fn novy() -> DekoderOpus {
        DekoderOpus::default()
    }

    /// Dekodér s pevným počtom výstupných kanálov (1 alebo 2) – pakety s iným počtom kanálov sa zmiešajú
    /// alebo zdvoja.
    pub fn s_kanalmi(kanalov: usize) -> DekoderOpus {
        DekoderOpus { kanalov: Some(kanalov.clamp(1, 2)), ..DekoderOpus::default() }
    }

    /// Skúšobný režim: rámce SILK-only pri vnútornej frekvencii (8, 12, 16 kHz) bez prevzorkovania.
    pub fn vnutorna_frekvencia_silk(mut self) -> DekoderOpus {
        self.vnutorna_silk = true;
        self
    }

    /// Pokrytie nástrojov CELT (na skúšky a hlásenia rozhodcu).
    pub fn statistika_celt(&self) -> celt::StatistikaCelt {
        self.celt.as_ref().map(|c| c.statistika()).unwrap_or_default()
    }

    /// Pokrytie režimov a prechodov.
    pub fn statistika(&self) -> StatistikaOpus {
        self.statistika
    }

    /// Nový začiatok (posun v súbore): všetky stavy na ticho.
    pub fn reset(&mut self) {
        self.silk.reset();
        if let Some(c) = self.celt.as_mut() {
            c.reset();
        }
        for p in &mut self.prevzorkovace {
            p.reset();
        }
        self.posledny_rezim = None;
        self.posledna_nadbytocnost = false;
    }

    /// Dekóduje paket; výstup pri 48 kHz (v skúšobnom režime SILK pri vnútornej frekvencii).
    pub fn dekoduj_paket(&mut self, data: &[u8]) -> Vysledok<VystupPaketu> {
        if data.is_empty() {
            // stratený paket: ticho v dĺžke predošlého (PLC nie je normatívne, RFC 6716 §4.4)
            let kanalov = self.kanalov.unwrap_or(1);
            let n = self.posledne_vzoriek;
            return Ok(VystupPaketu {
                kanaly: vec![vec![0.0; n]; kanalov],
                frekvencia: 48_000,
                konecny_rng: 0,
            });
        }
        let p = balenie::rozbal(data, false)?;
        let toc: Toc = p.toc;
        let kanalov = *self.kanalov.get_or_insert(1 + toc.stereo as usize);
        let mut vystup = VystupPaketu { kanaly: vec![Vec::new(); kanalov], ..VystupPaketu::default() };
        for r in &p.ramce {
            let v = self.dekoduj_ramec(&data[r.clone()], &toc, kanalov);
            vystup.frekvencia = v.frekvencia;
            for (k, s) in vystup.kanaly.iter_mut().zip(v.kanaly) {
                k.extend(s);
            }
            vystup.konecny_rng = v.rng;
        }
        self.posledne_vzoriek = vystup.kanaly.first().map_or(0, Vec::len);
        Ok(vystup)
    }

    fn celt(&mut self, kanalov: usize) -> &mut DekoderCelt {
        self.celt.get_or_insert_with(|| DekoderCelt::novy(kanalov))
    }

    fn dekoduj_ramec(&mut self, ramec: &[u8], toc: &Toc, kanalov: usize) -> VystupRamca {
        let n = toc.vzoriek_48k as usize;
        let rezim = toc.rezim;
        let stereo = toc.stereo;
        let koniec = match toc.pasmo {
            balenie::Pasmo::Uzke => 13,
            balenie::Pasmo::Stredne | balenie::Pasmo::Siroke => 17,
            balenie::Pasmo::Supersiroke => 19,
            balenie::Pasmo::Plne => 21,
        };
        if ramec.len() <= 1 {
            // prázdny rámec (DTX alebo strata): ticho – PLC nie je normatívne
            let f =
                if self.vnutorna_silk && rezim == Rezim::Silk { toc_silk(toc).frekvencia() } else { 48_000 };
            let m = n * f as usize / 48_000;
            return VystupRamca { kanaly: vec![vec![0.0; m]; kanalov], frekvencia: f, rng: 0 };
        }
        match rezim {
            Rezim::Silk => self.statistika.silk += 1,
            Rezim::Hybrid => self.statistika.hybrid += 1,
            Rezim::Celt => self.statistika.celt += 1,
        }
        let predosly = self.posledny_rezim;
        let pred_nadb = self.posledna_nadbytocnost;
        let okno = mdct::okno_celt();
        // prechod medzi LP a MDCT bez nadbytočnosti (RFC 6716 §4.5: odporúča sa doplnenie – nenormatívne)
        let mut prechod = predosly.is_some()
            && ((rezim == Rezim::Celt && predosly != Some(Rezim::Celt) && !pred_nadb)
                || (rezim != Rezim::Celt && predosly == Some(Rezim::Celt)));
        let mut doplnenie: Option<Vec<Vec<f32>>> = None;
        if prechod && rezim == Rezim::Celt {
            doplnenie = Some(vec![vec![0.0; 240.min(n)]; kanalov]); // bez PLC SILK: ticho, CELT nabehne oknom
        }

        let mut rd = rozsah::RozsahovyDekoder::novy(ramec);
        let mut silk48: Option<Vec<Vec<f32>>> = None;
        let mut silk_vnutorna: Option<(u32, Vec<Vec<f32>>)> = None;
        if rezim != Rezim::Celt {
            if predosly == Some(Rezim::Celt) {
                self.silk.reset();
                for p in &mut self.prevzorkovace {
                    p.reset();
                }
            }
            let pasmo = if rezim == Rezim::Hybrid { PasmoSilk::Wb } else { toc_silk(toc) };
            let VystupSilk { frekvencia, kanaly } =
                self.silk.dekoduj(&mut rd, pasmo, (n / 48) as u32, stereo);
            if self.vnutorna_silk && rezim == Rezim::Silk {
                silk_vnutorna = Some((frekvencia, kanaly));
            } else {
                // prevzorkovanie na 48 kHz; zmena šírky pásma SILK = nový prevzorkovač (s tichom)
                if self.prevzorkovace.len() != 2 || self.prevzorkovace[0].frekvencia() != frekvencia {
                    self.prevzorkovace = vec![Prevzorkovac::novy(frekvencia), Prevzorkovac::novy(frekvencia)];
                }
                let hore: Vec<Vec<f32>> =
                    kanaly.iter().zip(self.prevzorkovace.iter_mut()).map(|(k, p)| p.spracuj(k)).collect();
                silk48 = Some(na_kanaly(hore, kanalov));
            }
        }

        // nadbytočný rámec CELT (§4.5.1)
        let mut dlzka = ramec.len();
        let mut nadb = false;
        let mut na_zaciatku = false;
        let mut nadb_bajtov = 0usize;
        let hybrid = (rezim == Rezim::Hybrid) as i32;
        if rezim != Rezim::Celt && rd.tell() + 17 + 20 * hybrid <= 8 * dlzka as i32 {
            nadb = rezim == Rezim::Silk || rd.bit_logp(12);
            if nadb {
                na_zaciatku = rd.bit_logp(1);
                nadb_bajtov = if rezim == Rezim::Hybrid {
                    rd.uint(256) as usize + 2
                } else {
                    dlzka.saturating_sub(((rd.tell() + 7) >> 3) as usize)
                };
                if nadb_bajtov > dlzka || ((dlzka - nadb_bajtov) as i32) * 8 < rd.tell() {
                    // neplatná veľkosť: zvyšok rámca sa zahodí (RFC: odporúča sa)
                    dlzka = 0;
                    nadb_bajtov = 0;
                    nadb = false;
                } else {
                    dlzka -= nadb_bajtov;
                    rd.skrat(nadb_bajtov);
                }
            }
        }
        if nadb {
            prechod = false;
            if na_zaciatku {
                self.statistika.nadbytocne_zaciatok += 1;
            } else {
                self.statistika.nadbytocne_koniec += 1;
            }
        }
        if prechod {
            self.statistika.prechody_bez += 1;
            if rezim != Rezim::Celt {
                // doplnenie po CELT: dobeh prekrytia (rámec ticha 5 ms) – nenormatívne
                let c = self.celt(kanalov);
                let mut r = rozsah::RozsahovyDekoder::novy(&[0xFF, 0xFF]);
                doplnenie = Some(c.dekoduj(&mut r, 2, 240.min(n).max(120), stereo, 0, koniec));
            }
        }
        let data_nadb = &ramec[dlzka..dlzka + nadb_bajtov];
        let mut rng_nadb = 0u32;
        let mut nadb_zvuk: Option<Vec<Vec<f32>>> = None;
        if nadb && na_zaciatku {
            // CELT → SILK/hybrid: nadbytočný rámec bez vynulovania CELT
            let c = self.celt(kanalov);
            let mut r = rozsah::RozsahovyDekoder::novy(data_nadb);
            nadb_zvuk = Some(c.dekoduj(&mut r, nadb_bajtov, 240, stereo, 0, koniec));
            rng_nadb = r.rng();
        }

        let mut pcm: Vec<Vec<f32>>;
        if rezim != Rezim::Silk {
            let vynuluj = predosly.is_some_and(|p| p != rezim) && !pred_nadb;
            let zaciatok = if rezim == Rezim::Hybrid { 17 } else { 0 };
            let c = self.celt(kanalov);
            if vynuluj {
                c.reset();
            }
            pcm = if dlzka <= 1 {
                vec![vec![0.0; n]; kanalov]
            } else {
                c.dekoduj(&mut rd, dlzka, n.min(960), stereo, zaciatok, koniec)
            };
        } else {
            pcm = vec![vec![0.0; n]; kanalov];
            // hybrid → SILK: dobeh MDCT dekódovaním rámca ticha 2,5 ms
            if predosly == Some(Rezim::Hybrid) && !(nadb && na_zaciatku && pred_nadb) {
                self.statistika.dobeh_hybridu += 1;
                let c = self.celt(kanalov);
                let mut r = rozsah::RozsahovyDekoder::novy(&[0xFF, 0xFF]);
                let dobeh = c.dekoduj(&mut r, 2, 120, stereo, 0, koniec);
                for (p, d) in pcm.iter_mut().zip(dobeh) {
                    for (a, b) in p.iter_mut().zip(d) {
                        *a += b;
                    }
                }
            }
        }
        if let Some(s) = silk48 {
            for (p, k) in pcm.iter_mut().zip(s) {
                for (a, b) in p.iter_mut().zip(k) {
                    *a += b;
                }
            }
        }
        if nadb && !na_zaciatku {
            // SILK/hybrid → CELT: nadbytočný rámec po vynulovaní CELT, druhá polovica sa prekryje s koncom rámca
            let c = self.celt(kanalov);
            c.reset();
            let mut r = rozsah::RozsahovyDekoder::novy(data_nadb);
            let z = c.dekoduj(&mut r, nadb_bajtov, 240, stereo, 0, koniec);
            rng_nadb = r.rng();
            if n >= 120 {
                for (p, k) in pcm.iter_mut().zip(&z) {
                    let a = p[n - 120..].to_vec();
                    prekry(&a, &k[120..], &mut p[n - 120..], &okno);
                }
            }
        }
        if let Some(z) = nadb_zvuk {
            for (p, k) in pcm.iter_mut().zip(&z) {
                p[..120].copy_from_slice(&k[..120]);
                let b = p[120..240].to_vec();
                prekry(&k[120..240], &b, &mut p[120..240], &okno);
            }
        }
        if let Some(d) = doplnenie.filter(|_| prechod) {
            for (p, k) in pcm.iter_mut().zip(&d) {
                if n >= 240 && k.len() >= 240 {
                    p[..120].copy_from_slice(&k[..120]);
                    let b = p[120..240].to_vec();
                    prekry(&k[120..240], &b, &mut p[120..240], &okno);
                } else {
                    let b = p[..120].to_vec();
                    prekry(&k[..120], &b, &mut p[..120], &okno);
                }
            }
        }
        self.posledny_rezim = Some(rezim);
        self.posledna_nadbytocnost = nadb && !na_zaciatku;
        let rng = rd.rng() ^ rng_nadb;
        if let Some((f, k)) = silk_vnutorna {
            return VystupRamca { kanaly: na_kanaly(k, kanalov), frekvencia: f, rng };
        }
        VystupRamca { kanaly: pcm, frekvencia: 48_000, rng }
    }
}

fn toc_silk(toc: &Toc) -> PasmoSilk {
    match toc.pasmo {
        balenie::Pasmo::Uzke => PasmoSilk::Nb,
        balenie::Pasmo::Stredne => PasmoSilk::Mb,
        _ => PasmoSilk::Wb,
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
