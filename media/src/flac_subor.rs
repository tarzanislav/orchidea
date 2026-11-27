//! **Súbor FLAC** (`.flac`) ako kontajner – RFC 9639, kap. 6–9 (etapa L2).
//!
//! - Pred „fLaC“ smie byť značka ID3v2 (časté v hudobných knižniciach), na konci ID3v1 („TAG“, 128 B).
//! - Bloky metadát: STREAMINFO, SEEKTABLE, VORBIS_COMMENT a PICTURE (značky a obal; aj ID3v2 a ID3v1, ak ich
//!   súbor má).
//! - **Hranica rámca:** rámec končí tam, kde začína ďalšia platná hlavička (CRC-8) **a** CRC-16 doterajších
//!   bajtov je 0 – náhodná zhoda bajtov synchronizácie v údajoch rámca tak nerozdelí rámec zle.
//! - Súbor bez „fLaC“ (začína rámcom alebo neparsovateľnými údajmi) sa prečíta od prvého platného rámca.
//! - Posun podľa SEEKTABLE, inak delením intervalu podľa čísla vzorky v hlavičke rámca.

use crate::cas::{na_ns, z_ns, zakladna_z_frekvencie};
use crate::flac::{HlavickaRamca, StreamInfo, crc16_bajt, hlavicka_ramca, rozlozenie};
use crate::kontajner::{FormatKontajnera, Kontajner, Posun};
use crate::limity::Limity;
use crate::paket::Paket;
use crate::stopa::{Kodek, ParametreZvuku, Stopa};
use crate::zdroj::{Citac, ZdrojBajtov};
use crate::znacky::{self, Znacky};
use orchidea_jadro::{Chyba, Vysledok};

/// Koľko bajtov najviac prehľadať pri hľadaní prvého alebo ďalšieho rámca.
const NAJVIAC_HLADANIA: u64 = 16 << 20;

/// Čítačka súboru FLAC.
#[derive(Debug)]
pub struct FlacSubor<Z> {
    c: Citac<Z>,
    stopy: Vec<Stopa>,
    si: Option<StreamInfo>,
    prvy_ramec: u64,
    koniec: u64,
    /// Body SEEKTABLE: (číslo vzorky, poloha od prvého rámca).
    body: Vec<(u64, u64)>,
    /// Pevná veľkosť bloku (číslo rámca × blok = číslo vzorky).
    pevny_blok: u64,
    /// Najväčšia dĺžka rámca, ktorú hľadanie konca prehľadá.
    najviac_ramec: usize,
    p: u64,
    prerusenie: bool,
    znacky: Option<Znacky>,
}

/// Preskočí značku ID3v2 na začiatku (veľkosť v 4 × 7 bitoch, voliteľná päta).
fn za_id3v2(d: &[u8]) -> u64 {
    if d.len() < 10 || &d[..3] != b"ID3" {
        return 0;
    }
    let v = d[6..10].iter().fold(0u64, |x, &b| (x << 7) | (b & 0x7F) as u64);
    10 + v + if d[5] & 0x10 != 0 { 10 } else { 0 }
}

impl<Z: ZdrojBajtov> FlacSubor<Z> {
    pub fn otvor(zdroj: Z, limity: Limity) -> Vysledok<FlacSubor<Z>> {
        let mut c = Citac::novy(zdroj, Citac::<Z>::PREDVOLENA_DAVKA);
        let dlzka = c.dlzka();
        let mut koniec = dlzka;
        if dlzka >= 128 && c.bajty(dlzka - 128, 3)? == b"TAG" {
            koniec -= 128;
        }
        let mut p = za_id3v2(c.bajty(0, 10)?);
        // značky: VORBIS_COMMENT; ID3v2 na začiatku a ID3v1 na konci (niektoré zapisovače) ich doplnia
        let mut znacky: Option<Znacky> = None;
        let mut vorbis = false;
        let mut ine_znacky = Znacky::nove();
        if p > 0
            && let Ok(d) = c.presne(0, p.min(limity.prvok) as usize)
            && let Some((z, _)) = znacky::id3v2(d)
        {
            ine_znacky.dopln(z);
        }
        if koniec < dlzka
            && let Some(z) = znacky::id3v1(c.bajty(koniec, 128)?)
        {
            ine_znacky.dopln(z);
        }
        let mut si = None;
        let mut body = Vec::new();
        let mut konfiguracia = Vec::new();
        if c.bajty(p, 4)? == b"fLaC" {
            p += 4;
            // poškodené metadáta (zlá dĺžka bloku) nezastavia prehrávanie: rámce sa nájdu hľadaním
            // s kontrolou CRC-16 od tohto miesta
            while let Ok(h) = c.presne(p, 4).map(|h| h.to_vec()) {
                let n = u32::from_be_bytes([0, h[1], h[2], h[3]]) as u64;
                let Ok(dlzka) = limity.over_prvok(n, koniec.saturating_sub(p + 4)) else { break };
                let telo = c.presne(p + 4, dlzka)?.to_vec();
                match h[0] & 0x7F {
                    0 if si.is_none() => {
                        let Ok(s) = StreamInfo::z_tela(&telo) else { break };
                        si = Some(s);
                        konfiguracia = b"fLaC\x80\x00\x00\x22".to_vec();
                        konfiguracia.extend_from_slice(&telo[..34]);
                    }
                    3 => {
                        for bod in telo.chunks_exact(18) {
                            let vzorka = u64::from_be_bytes(bod[..8].try_into().unwrap_or([0xFF; 8]));
                            let poloha = u64::from_be_bytes(bod[8..16].try_into().unwrap_or([0; 8]));
                            if vzorka != u64::MAX {
                                limity.over_index(body.len() as u64 * 16, 16)?;
                                body.push((vzorka, poloha));
                            }
                        }
                    }
                    // prvý VORBIS_COMMENT (RFC 9639: najviac jeden); PICTURE môže byť pred ním aj za ním
                    4 if !vorbis => {
                        vorbis = true;
                        if let Some(z) = znacky::vorbis_comment(&telo) {
                            znacky.get_or_insert_default().pripoj(z);
                        }
                    }
                    6 => znacky::flac_picture(znacky.get_or_insert_default(), &telo),
                    127 => break, // neplatný typ – ďalej len hľadanie rámcov
                    _ => {}
                }
                p += 4 + n;
                if h[0] & 0x80 != 0 {
                    break;
                }
            }
        }
        body.sort_unstable();
        let mut znacky = znacky.unwrap_or_default();
        znacky.dopln(ine_znacky);
        let znacky = (!znacky.je_prazdne()).then_some(znacky);
        // najväčšej dĺžke rámca zo STREAMINFO sa neverí (poškodené súbory ju majú zlú) – hľadanie konca
        // číta po kusoch, takže veľký strop nestojí nič navyše
        let najviac_ramec = (limity.paket as usize).min(16 << 20);
        let mut f = FlacSubor {
            c,
            stopy: Vec::new(),
            si,
            prvy_ramec: p,
            koniec,
            body,
            pevny_blok: 0,
            najviac_ramec,
            p,
            prerusenie: true,
            znacky,
        };
        // prvý platný rámec (súbor bez fLaC alebo s neparsovateľnými údajmi pred rámcami)
        let (q, h) = f
            .hladaj_ramec(p, f.koniec.min(p.saturating_add(NAJVIAC_HLADANIA)))?
            .ok_or_else(|| Chyba::PoskodeneData("FLAC: súbor nemá ani jeden platný rámec".into()))?;
        f.prvy_ramec = q;
        f.p = q;
        f.pevny_blok = match si {
            Some(s) if s.min_blok == s.max_blok && s.max_blok > 0 => s.max_blok as u64,
            _ => h.blok as u64,
        };
        let (frekvencia, kanalov, bitov) = match si {
            Some(s) => (s.frekvencia, s.kanalov, s.bitov),
            None => (h.frekvencia, h.kanalov, h.bitov),
        };
        let zakladna = zakladna_z_frekvencie(frekvencia as u64)
            .ok_or_else(|| Chyba::PoskodeneData("FLAC: vzorkovacia frekvencia 0".into()))?;
        let mut stopa = Stopa::nova(1, Kodek::Flac, zakladna);
        stopa.konfiguracia = konfiguracia;
        stopa.predvolena = true;
        stopa.zvuk =
            Some(ParametreZvuku { frekvencia, kanalov, rozlozenie: rozlozenie(kanalov), bitov: Some(bitov) });
        f.stopy.push(stopa);
        Ok(f)
    }

    /// Čítač (štatistika čítaní – chladné čítanie).
    pub fn citac(&self) -> &Citac<Z> {
        &self.c
    }

    pub fn streaminfo(&self) -> Option<&StreamInfo> {
        self.si.as_ref()
    }

    /// Číslo prvej vzorky rámca.
    fn vzorka(&self, h: &HlavickaRamca) -> u64 {
        if h.premenlivy { h.cislo } else { h.cislo.saturating_mul(self.pevny_blok) }
    }

    /// Dĺžka rámca od `p`: po ďalšiu platnú hlavičku, pred ktorou je CRC-16 rovné 0 (alebo po koniec súboru).
    /// Číta po kusoch (64 KiB, potom štvornásobne) – bežný rámec má 2–20 KiB, viac sa neprečíta.
    fn dlzka_ramca(&mut self, p: u64, h: &HlavickaRamca) -> Vysledok<Option<usize>> {
        let zvysok = (self.koniec - p).min(self.najviac_ramec as u64) as usize;
        let si = self.si;
        let mut crc = 0u16;
        let mut i = 0usize;
        let mut n = zvysok.min(64 << 10);
        loop {
            let d = self.c.bajty(p, n)?.to_vec();
            while i < d.len() {
                let b = d[i];
                // hlavička ďalšieho rámca potrebuje až 16 B – pri konci kusu sa najprv dočíta
                if i >= h.dlzka + 2 && crc == 0 && b == 0xFF {
                    if i + 16 > d.len() && d.len() < zvysok {
                        break;
                    }
                    if hlavicka_ramca(&d[i..], si.as_ref()).is_some() {
                        return Ok(Some(i));
                    }
                }
                crc = crc16_bajt(crc, b);
                i += 1;
            }
            if d.len() >= zvysok {
                // posledný rámec súboru
                let posledny = crc == 0 && p + d.len() as u64 == self.koniec && d.len() > h.dlzka + 2;
                return Ok(posledny.then_some(d.len()));
            }
            n = (n * 4).min(zvysok);
        }
    }

    /// Najbližší platný rámec od `od` (hlavička s CRC-8 a celý rámec s CRC-16).
    fn hladaj_ramec(&mut self, od: u64, do_: u64) -> Vysledok<Option<(u64, HlavickaRamca)>> {
        const KUS: usize = 64 << 10;
        let mut p = od;
        while p < do_ {
            let kus = self.c.bajty(p, KUS + 16)?.to_vec();
            if kus.len() < 6 {
                break;
            }
            let hranica = kus.len().saturating_sub(16).max(1);
            for i in 0..hranica {
                if kus[i] != 0xFF || p + i as u64 >= do_ {
                    continue;
                }
                let Some(h) = hlavicka_ramca(&kus[i..], self.si.as_ref()) else { continue };
                let q = p + i as u64;
                if self.dlzka_ramca(q, &h)?.is_some() {
                    return Ok(Some((q, h)));
                }
            }
            p += hranica as u64;
        }
        Ok(None)
    }
}

impl<Z: ZdrojBajtov> Kontajner for FlacSubor<Z> {
    fn znacky(&self) -> Option<&Znacky> {
        self.znacky.as_ref()
    }

    fn format(&self) -> FormatKontajnera {
        FormatKontajnera::Flac
    }

    fn stopy(&self) -> &[Stopa] {
        &self.stopy
    }

    fn trvanie_ns(&self) -> Option<i64> {
        let s = self.si.filter(|s| s.vzoriek > 0)?;
        na_ns(s.vzoriek as i64, self.stopy[0].zakladna)
    }

    fn dalsi_paket(&mut self) -> Vysledok<Option<Paket>> {
        loop {
            if self.p >= self.koniec {
                return Ok(None);
            }
            let p = self.p;
            let hlava = self.c.bajty(p, 16)?.to_vec();
            let h = hlavicka_ramca(&hlava, self.si.as_ref());
            let dlzka = match &h {
                Some(h) => self.dlzka_ramca(p, h)?,
                None => None,
            };
            let (Some(h), Some(n)) = (h, dlzka) else {
                // poškodenie: pokračuj ďalším platným rámcom
                self.prerusenie = true;
                let do_ = self.koniec.min(p.saturating_add(NAJVIAC_HLADANIA));
                match self.hladaj_ramec(p + 1, do_)? {
                    Some((q, _)) => {
                        self.p = q;
                        continue;
                    }
                    None => {
                        self.p = self.koniec;
                        return Ok(None);
                    }
                }
            };
            let data = self.c.presne(p, n)?.to_vec();
            self.p = p + n as u64;
            return Ok(Some(Paket {
                stopa: 0,
                data,
                pts: i64::try_from(self.vzorka(&h)).ok(),
                dts: None,
                trvanie: Some(h.blok as i64),
                klucovy: true,
                prerusenie: std::mem::take(&mut self.prerusenie),
                orezat_zaciatok: 0,
                orezat_koniec: 0,
                pozicia: p,
            }));
        }
    }

    fn posun(&mut self, ciel_ns: i64, _sposob: Posun) -> Vysledok<i64> {
        // rámce FLAC sú nezávislé – presný posun nepotrebuje predbeh
        let zakladna = self.stopy[0].zakladna;
        let ciel = z_ns(ciel_ns.max(0), zakladna).unwrap_or(0).max(0) as u64;
        let davka = self.c.davka();
        self.c.nastav_davku(64 << 10);
        let mut najlepsi = (self.prvy_ramec, 0u64);
        // 1. SEEKTABLE
        let i = self.body.partition_point(|&(v, _)| v <= ciel);
        let mut lo = self.prvy_ramec;
        if i > 0 {
            let (v, o) = self.body[i - 1];
            let q = self.prvy_ramec.saturating_add(o);
            if q < self.koniec
                && let Some(h) = hlavicka_ramca(self.c.bajty(q, 16)?, self.si.as_ref())
                && self.vzorka(&h) == v
            {
                najlepsi = (q, v);
                lo = q;
            }
        }
        // 2. delenie intervalu (aj spresnenie za bodom SEEKTABLE)
        let mut hi = self.koniec;
        let mut pokusov = 0;
        while hi > lo + 1 && pokusov < 64 {
            pokusov += 1;
            let stred = lo + (hi - lo) / 2;
            match self.hladaj_ramec(stred, hi)? {
                Some((q, h)) if self.vzorka(&h) <= ciel => {
                    if self.vzorka(&h) >= najlepsi.1 {
                        najlepsi = (q, self.vzorka(&h));
                    }
                    lo = q.max(stred);
                }
                _ => hi = stred,
            }
        }
        self.c.nastav_davku(davka);
        self.p = najlepsi.0;
        self.prerusenie = true;
        Ok(na_ns(najlepsi.1 as i64, zakladna).unwrap_or(0))
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn id3v2() {
        assert_eq!(za_id3v2(b"ID3\x04\x00\x00\x00\x00\x02\x01rest"), 10 + 257);
        assert_eq!(za_id3v2(b"ID3\x04\x00\x10\x00\x00\x00\x05"), 25);
        assert_eq!(za_id3v2(b"fLaC"), 0);
    }
}
