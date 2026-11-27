//! Verejný dekodér: podľa bitovej hĺbky SPS, na ktoré ukazuje prvý rez obrazu, posiela tok 8-bitovému
//! ([`crate::b8`]) alebo 10-bitovému ([`crate::b16`]) dekodéru. Hĺbka sa môže zmeniť len na začiatku novej
//! sekvencie – vtedy sa doterajší dekodér dokončí (vydá všetky obrazy) a nový dostane uložené sady parametrov.

use crate::b8;
#[cfg(not(test))]
use crate::b16;
use crate::etapy::{self, Etapa};
use crate::statistika::Statistika;
use orchidea_hevc::bity::Citac;
use orchidea_hevc::nal::typ;
use orchidea_hevc::{HlavickaNal, Pps, Sady, Sps, Vps, rbsp};
use std::sync::Arc;

/// Dekódovaný obraz na výstup (8 alebo 10 bitov).
#[derive(Debug, Clone)]
pub enum Vystup {
    B8(Arc<b8::syntax::DekodovanyObraz>),
    #[cfg(not(test))]
    B16(Arc<b16::syntax::DekodovanyObraz>),
}

macro_rules! oba {
    ($s:expr, $o:ident => $e:expr) => {
        match $s {
            Vystup::B8($o) => $e,
            #[cfg(not(test))]
            Vystup::B16($o) => $e,
        }
    };
}

impl Vystup {
    pub fn poc(&self) -> i32 {
        oba!(self, o => o.poc)
    }

    /// Okno orezania v jase: vľavo, vpravo, hore, dole.
    pub fn okno(&self) -> [usize; 4] {
        oba!(self, o => o.okno)
    }

    /// Rozmery dekódovaného (neorezaného) obrazu.
    pub fn sirka(&self) -> usize {
        oba!(self, o => o.obraz.sirka())
    }

    pub fn vyska(&self) -> usize {
        oba!(self, o => o.obraz.vyska())
    }

    /// Bitová hĺbka jasu a farby.
    pub fn bitova_hlbka(&self) -> (u32, u32) {
        oba!(self, o => (o.obraz.bitova_hlbka, o.obraz.bitova_hlbka_farby))
    }

    /// Bajtov na vzorku vo výstupe [`Vystup::zapis_yuv`] (1 pre 8 bitov, 2 pre Main 10).
    pub fn bajtov_na_vzorku(&self) -> usize {
        match self {
            Vystup::B8(_) => 1,
            #[cfg(not(test))]
            Vystup::B16(_) => 2,
        }
    }

    /// Orezaný obraz ako YUV 4:2:0: 8 bitov po bajte, 10 bitov po 2 bajtoch (malý koniec, ako yuv420p10le).
    /// Počká na nehotové riadky.
    pub fn zapis_yuv(&self, ciel: &mut Vec<u8>) {
        oba!(self, o => o.zapis_yuv(ciel))
    }

    /// Celý (neorezaný) obraz v tom istom tvare ako [`Vystup::zapis_yuv`].
    pub fn zapis_yuv_cely(&self, ciel: &mut Vec<u8>) {
        oba!(self, o => o.zapis_yuv_cely(ciel))
    }

    /// 8-bitový obraz (napr. na zobrazenie bez prevodu), inak `None`.
    pub fn ako_8_bitov(&self) -> Option<&Arc<b8::syntax::DekodovanyObraz>> {
        match self {
            Vystup::B8(o) => Some(o),
            #[cfg(not(test))]
            Vystup::B16(_) => None,
        }
    }
}

enum Vnutorny {
    B8(b8::dekoder::Dekoder),
    #[cfg(not(test))]
    B16(b16::dekoder::Dekoder),
}

/// Softvérový dekodér HEVC (Main a Main 10) – pozri [`b8::dekoder::Dekoder`] (súbežnosť, odtlačky).
#[allow(missing_debug_implementations)]
pub struct Dekoder {
    vnutorny: Option<Vnutorny>,
    /// sady parametrov na zistenie hĺbky a na odovzdanie novému dekodéru: posledná jednotka NAL každej sady
    /// (typ, id) v poradí príchodu – najviac 16 VPS, 16 SPS a 64 PPS (VPSSPSPPS_A ich posiela stovky)
    sady: Sady,
    parametre: Vec<((u8, u32), Vec<u8>)>,
    /// jednotky NAL pred prvým rezom (SEI a pod.), kým nie je jasná hĺbka
    cakajuce: Vec<Vec<u8>>,
    predosla: Statistika,
    vlakien: Option<usize>,
    /// overovať odtlačky obrazov zo SEI (skúšky); pri prehrávaní sa vypne
    pub overovat_odtlacky: bool,
    /// výsledky overenia – úplné po [`Dekoder::koniec`]
    pub statistika: Statistika,
}

impl Default for Dekoder {
    fn default() -> Self {
        Self::novy()
    }
}

impl Dekoder {
    pub fn novy() -> Dekoder {
        Dekoder {
            vnutorny: None,
            sady: Sady::nove(),
            parametre: Vec::new(),
            cakajuce: Vec::new(),
            predosla: Statistika::default(),
            vlakien: None,
            overovat_odtlacky: true,
            statistika: Statistika::default(),
        }
    }

    /// Dekodér na prehrávanie: bez overovania odtlačkov.
    pub fn na_prehravanie() -> Dekoder {
        Dekoder { overovat_odtlacky: false, ..Self::novy() }
    }

    /// Počet vlákien dekódovania (predvolene dvojnásobok jadier, najviac 8; 1 = všetko v hlavnom vlákne).
    pub fn s_vlaknami(mut self, n: usize) -> Dekoder {
        self.vlakien = Some(n.max(1));
        self
    }

    /// Spracuje jednotku NAL (bez štartovacieho kódu) a vráti obrazy pripravené na výstup (v poradí zobrazenia).
    pub fn nal(&mut self, nal: &[u8]) -> Vec<Vystup> {
        let _m = etapy::meraj(Etapa::Rozbor);
        let mut vystup = Vec::new();
        let Ok(h) = HlavickaNal::citaj(nal) else {
            return self.posli(nal, vystup);
        };
        match h.typ {
            typ::VPS | typ::SPS | typ::PPS if h.vrstva == 0 => {
                let r = rbsp(&nal[2..]);
                let _ = self.sady.pridaj(&h, &r);
                let id = match h.typ {
                    typ::VPS => Vps::citaj(&r).ok().map(|v| u32::from(v.id)),
                    typ::SPS => Sps::citaj(&r).ok().map(|s| s.id),
                    _ => Pps::citaj(&r).ok().map(|p| p.id),
                };
                if let Some(id) = id {
                    self.parametre.retain(|(k, _)| *k != (h.typ, id));
                    self.parametre.push(((h.typ, id), nal.to_vec()));
                }
                if self.vnutorny.is_none() {
                    return vystup;
                }
            }
            0..=31 if h.vrstva == 0 => {
                if let Some(desat) = self.desat_bitov(&h, nal) {
                    let ina = match &self.vnutorny {
                        None => true,
                        Some(Vnutorny::B8(_)) => desat,
                        #[cfg(not(test))]
                        Some(Vnutorny::B16(_)) => !desat,
                    };
                    if ina {
                        self.prepni(desat, &mut vystup);
                    }
                }
            }
            _ if self.vnutorny.is_none() => {
                if self.cakajuce.len() < 64 {
                    self.cakajuce.push(nal.to_vec());
                }
                return vystup;
            }
            _ => {}
        }
        self.posli(nal, vystup)
    }

    /// Koniec toku: dokončí a vydá všetky obrazy.
    pub fn koniec(&mut self) -> Vec<Vystup> {
        let _m = etapy::meraj(Etapa::Rozbor);
        let mut vystup = Vec::new();
        self.dokonci(&mut vystup);
        self.statistika = self.predosla.clone();
        vystup
    }

    fn posli(&mut self, nal: &[u8], mut vystup: Vec<Vystup>) -> Vec<Vystup> {
        if self.vnutorny.is_none() {
            self.prepni(false, &mut vystup);
        }
        match self.vnutorny.as_mut() {
            Some(Vnutorny::B8(d)) => vystup.extend(d.nal(nal).into_iter().map(Vystup::B8)),
            #[cfg(not(test))]
            Some(Vnutorny::B16(d)) => vystup.extend(d.nal(nal).into_iter().map(Vystup::B16)),
            None => {}
        }
        vystup
    }

    /// Hĺbka SPS prvého rezu obrazu: `Some(true)` = viac ako 8 bitov; `None` = nie je prvý rez alebo sa nedá zistiť.
    fn desat_bitov(&self, h: &HlavickaNal, nal: &[u8]) -> Option<bool> {
        let r = rbsp(&nal[2..nal.len().min(40)]);
        let mut c = Citac::novy(&r);
        if !c.bit().ok()? {
            return None;
        }
        if (16..=23).contains(&h.typ) {
            c.bit().ok()?;
        }
        let pps_id = c.ue().ok()?;
        let (sps, _) = self.sady.pre_rez(pps_id).ok()?;
        Some(sps.bity_jasu.max(sps.bity_farby) > 8)
    }

    fn dokonci(&mut self, vystup: &mut Vec<Vystup>) {
        let stat = match self.vnutorny.take() {
            Some(Vnutorny::B8(mut d)) => {
                vystup.extend(d.koniec().into_iter().map(Vystup::B8));
                d.statistika
            }
            #[cfg(not(test))]
            Some(Vnutorny::B16(mut d)) => {
                vystup.extend(d.koniec().into_iter().map(Vystup::B16));
                d.statistika
            }
            None => return,
        };
        self.predosla.pridaj(stat);
    }

    /// Nový vnútorný dekodér pre danú hĺbku (doterajší sa dokončí); dostane uložené sady parametrov a čakajúce NAL.
    fn prepni(&mut self, desat: bool, vystup: &mut Vec<Vystup>) {
        self.dokonci(vystup);
        macro_rules! novy {
            ($m:ident) => {{
                let mut d = if self.overovat_odtlacky {
                    $m::dekoder::Dekoder::novy()
                } else {
                    $m::dekoder::Dekoder::na_prehravanie()
                };
                if let Some(n) = self.vlakien {
                    d = d.s_vlaknami(n);
                }
                d
            }};
        }
        #[cfg(not(test))]
        let mut n = if desat { Vnutorny::B16(novy!(b16)) } else { Vnutorny::B8(novy!(b8)) };
        #[cfg(test)]
        let mut n = {
            let _ = desat;
            Vnutorny::B8(novy!(b8))
        };
        let predtym: Vec<Vec<u8>> =
            self.parametre.iter().map(|(_, n)| n).chain(&self.cakajuce).cloned().collect();
        self.cakajuce.clear();
        for p in &predtym {
            match &mut n {
                Vnutorny::B8(d) => vystup.extend(d.nal(p).into_iter().map(Vystup::B8)),
                #[cfg(not(test))]
                Vnutorny::B16(d) => vystup.extend(d.nal(p).into_iter().map(Vystup::B16)),
            }
        }
        self.vnutorny = Some(n);
    }
}
