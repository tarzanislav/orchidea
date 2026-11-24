//! Úložisko parametrických sád VPS, SPS a PPS podľa identifikátora – rezy sa na ne odvolávajú cez `pps_id`
//! a PPS cez `sps_id`. Nová sada s rovnakým identifikátorom nahradí starú (platí pre ďalšie obrazy, 7.4.2.4.2).

use crate::nal::{HlavickaNal, typ};
use crate::parametre::{Pps, Sps, Vps};
use orchidea_jadro::{Chyba, Vysledok};

/// Parametrické sady toku.
#[derive(Debug, Clone, Default)]
pub struct Sady {
    pub vps: Vec<Option<Vps>>,
    pub sps: Vec<Option<Sps>>,
    pub pps: Vec<Option<Pps>>,
}

impl Sady {
    pub fn nove() -> Sady {
        Sady { vps: vec![None; 16], sps: vec![None; 16], pps: vec![None; 64] }
    }

    /// Prečíta a uloží parametrickú sadu z jednotky NAL (`rbsp` bez hlavičky NAL). Iné typy jednotiek ignoruje.
    /// Vráti `true`, ak išlo o parametrickú sadu.
    pub fn pridaj(&mut self, nal: &HlavickaNal, rbsp: &[u8]) -> Vysledok<bool> {
        if self.vps.is_empty() {
            *self = Sady::nove();
        }
        match nal.typ {
            typ::VPS => {
                let v = Vps::citaj(rbsp)?;
                let id = v.id as usize;
                self.vps[id] = Some(v);
            }
            typ::SPS => {
                let s = Sps::citaj(rbsp)?;
                let id = s.id as usize;
                self.sps[id] = Some(s);
            }
            typ::PPS => {
                let p = Pps::citaj(rbsp)?;
                let id = p.id as usize;
                self.pps[id] = Some(p);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// PPS s identifikátorom `id` a jeho SPS.
    pub fn pre_rez(&self, pps_id: u32) -> Vysledok<(&Sps, &Pps)> {
        let pps = self
            .pps
            .get(pps_id as usize)
            .and_then(|p| p.as_ref())
            .ok_or_else(|| Chyba::Chybajuce(format!("HEVC: PPS {pps_id} nebolo prijaté")))?;
        let sps = self
            .sps
            .get(pps.sps_id as usize)
            .and_then(|s| s.as_ref())
            .ok_or_else(|| Chyba::Chybajuce(format!("HEVC: SPS {} nebolo prijaté", pps.sps_id)))?;
        Ok((sps, pps))
    }
}
