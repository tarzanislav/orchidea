//! **Surový tok AC-3 a E-AC-3** (`.ac3`, `.eac3`, `.ec3`) ako kontajner – LÝRA otvorí súbor priamo.
//!
//! - Rámce podľa syncinfo a bsi (ATSC A/52 §5.4.1, príloha E – `ramce::ac3`). **Prvý rámec** sa prijme, len ak za
//!   ním hneď nasleduje ďalšia platná hlavička toho istého druhu (AC-3 alebo E-AC-3) a frekvencie (náhodné 0x0B77
//!   v údajoch tak rámec nevytvorí); na začiatku smie byť značka ID3v2 alebo iné údaje (hľadá sa najviac 4 MB).
//! - **E-AC-3:** paket = nezávislý rámec programu 0 a závislé podtoky za ním (rovnako ako vo vyrezávaní z MPEG-TS
//!   a v dávkach priechodu IEC 61937); nezávislé podtoky iných programov (substreamid ≠ 0) sa preskočia.
//! - Formát nemá časové značky – čas je počet vzoriek (základňa 1/frekvencia, presne). Posun podľa priemernej dĺžky
//!   paketu s novou synchronizáciou na platný rámec; pri stálej dĺžke rámca (CBR) je čas po posune presný.

use crate::cas::{na_ns, z_ns, zakladna_z_frekvencie};
use crate::kontajner::{FormatKontajnera, Kontajner, Posun};
use crate::limity::Limity;
use crate::paket::Paket;
use crate::ramce::{self, HlavickaRamca};
use crate::stopa::{Kodek, ParametreZvuku, Stopa};
use crate::zdroj::{Citac, ZdrojBajtov};
use orchidea_jadro::{Chyba, Vysledok};

/// Koľko bajtov najviac prehľadať pri hľadaní prvého alebo ďalšieho rámca.
const NAJVIAC_HLADANIA: u64 = 4 << 20;
/// Najmenšia rozumná dĺžka rámca (AC-3 má najmenej 128 B; E-AC-3 s jedným blokom pri 32 kb/s ≈ 22 B).
const NAJKRATSI_RAMEC: usize = 16;

/// Hlavička rámca s druhom toku.
#[derive(Debug, Clone, Copy)]
struct Ramec {
    hl: HlavickaRamca,
    eac3: bool,
    /// substreamid (E-AC-3; pri AC-3 0)
    podtok: u8,
}

impl Ramec {
    fn z(b: &[u8]) -> Option<Ramec> {
        let hl = ramce::ac3(b)?;
        if hl.dlzka < NAJKRATSI_RAMEC {
            return None;
        }
        let eac3 = b[5] >> 3 > 10;
        Some(Ramec { hl, eac3, podtok: if eac3 { (b[2] >> 3) & 7 } else { 0 } })
    }

    /// Dva rámce patria do toho istého toku (druh a frekvencia sa v toku nemenia).
    fn rovnaky_tok(&self, b: &Ramec) -> bool {
        self.eac3 == b.eac3 && self.hl.frekvencia == b.hl.frekvencia
    }

    /// Rámec, ktorým začína paket: AC-3, alebo nezávislý rámec E-AC-3 programu 0.
    fn zaciatok_paketu(&self) -> bool {
        !self.hl.zavisly && self.podtok == 0
    }
}

/// Rozpozná začiatok surového toku AC-3/E-AC-3 (pre `kontajner::rozpoznaj`): `Some(e_ac3)`.
pub fn je_tok(zaciatok: &[u8]) -> Option<bool> {
    let h = Ramec::z(zaciatok)?;
    match zaciatok.get(h.hl.dlzka..h.hl.dlzka + ramce::NAJMENEJ_HLAVICKY) {
        Some(dalsi) => Ramec::z(dalsi).filter(|d| h.rovnaky_tok(d)).map(|_| h.eac3),
        None => Some(h.eac3),
    }
}

/// Čítačka surového toku AC-3 / E-AC-3.
#[derive(Debug)]
pub struct SuborAc3<Z> {
    c: Citac<Z>,
    stopy: Vec<Stopa>,
    eac3: bool,
    prvy: u64,
    koniec: u64,
    /// vzoriek na kanál v pakete (AC-3 1536, E-AC-3 256 × počet blokov nezávislého rámca)
    vzoriek_paketu: u64,
    /// priemerná dĺžka paketu v bajtoch (z prvého paketu)
    priemer_paketu: f64,
    p: u64,
    vzorka: u64,
    prerusenie: bool,
}

impl<Z: ZdrojBajtov> SuborAc3<Z> {
    pub fn otvor(zdroj: Z, limity: Limity) -> Vysledok<SuborAc3<Z>> {
        let _ = limity;
        let mut c = Citac::novy(zdroj, Citac::<Z>::PREDVOLENA_DAVKA);
        let koniec = c.dlzka();
        let hlava = c.bajty(0, 10)?.to_vec();
        let za_id3 = if hlava.len() >= 10 && &hlava[..3] == b"ID3" {
            let v = hlava[6..10].iter().fold(0u64, |x, &b| (x << 7) | (b & 0x7F) as u64);
            10 + v + if hlava[5] & 0x10 != 0 { 10 } else { 0 }
        } else {
            0
        };
        let mut s = SuborAc3 {
            c,
            stopy: Vec::new(),
            eac3: false,
            prvy: 0,
            koniec,
            vzoriek_paketu: 1536,
            priemer_paketu: 1.0,
            p: 0,
            vzorka: 0,
            prerusenie: true,
        };
        let (q, h) = s
            .hladaj_ramec(za_id3, koniec.min(za_id3.saturating_add(NAJVIAC_HLADANIA)))?
            .ok_or_else(|| Chyba::PoskodeneData("AC-3: súbor nemá ani jeden platný rámec".into()))?;
        s.prvy = q;
        s.p = q;
        s.eac3 = h.eac3;
        s.vzoriek_paketu = h.hl.vzoriek as u64;
        let dlzka = s.dlzka_paketu(q, &h)?.unwrap_or(h.hl.dlzka);
        s.priemer_paketu = dlzka as f64;
        let zakladna = zakladna_z_frekvencie(h.hl.frekvencia as u64)
            .ok_or_else(|| Chyba::PoskodeneData("AC-3: frekvencia".into()))?;
        let mut stopa = Stopa::nova(1, if h.eac3 { Kodek::Eac3 } else { Kodek::Ac3 }, zakladna);
        stopa.predvolena = true;
        let rozlozenie =
            h.hl.rozlozenie.ok_or_else(|| Chyba::PoskodeneData("AC-3: rozloženie kanálov".into()))?;
        stopa.zvuk = Some(ParametreZvuku {
            frekvencia: h.hl.frekvencia,
            kanalov: h.hl.kanalov,
            rozlozenie,
            bitov: None,
        });
        s.stopy.push(stopa);
        Ok(s)
    }

    /// Čítač (štatistika čítaní – chladné čítanie).
    pub fn citac(&self) -> &Citac<Z> {
        &self.c
    }

    /// Dĺžka paketu od rámca na `p`: rámec a závislé podtoky E-AC-3 za ním; `None` = neúplný na konci súboru.
    fn dlzka_paketu(&mut self, p: u64, h: &Ramec) -> Vysledok<Option<usize>> {
        let mut dlzka = h.hl.dlzka;
        if p + dlzka as u64 > self.koniec {
            return Ok(None);
        }
        if h.eac3 {
            // najviac 8 závislých podtokov (substreamid 0–7, A/52 príloha E)
            for _ in 0..8 {
                let q = p + dlzka as u64;
                let hl = self.c.bajty(q, ramce::NAJMENEJ_HLAVICKY)?.to_vec();
                match Ramec::z(&hl) {
                    Some(z) if z.hl.zavisly && h.rovnaky_tok(&z) && q + z.hl.dlzka as u64 <= self.koniec => {
                        dlzka += z.hl.dlzka;
                    }
                    _ => break,
                }
            }
        }
        Ok(Some(dlzka))
    }

    /// Najbližší začiatok paketu od `od`, za ktorým nasleduje ďalšia zhodná hlavička (alebo koniec súboru).
    fn hladaj_ramec(&mut self, od: u64, do_: u64) -> Vysledok<Option<(u64, Ramec)>> {
        const KUS: usize = 64 << 10;
        let mut p = od;
        while p < do_ {
            let kus = self.c.bajty(p, KUS + ramce::NAJMENEJ_HLAVICKY)?.to_vec();
            if kus.len() < ramce::NAJMENEJ_HLAVICKY {
                break;
            }
            let hranica = kus.len().saturating_sub(ramce::NAJMENEJ_HLAVICKY).max(1);
            for i in 0..hranica {
                if kus[i] != 0x0B || kus[i + 1] != 0x77 || p + i as u64 >= do_ {
                    continue;
                }
                let Some(h) = Ramec::z(&kus[i..]) else { continue };
                if !h.zaciatok_paketu() {
                    continue;
                }
                let q = p + i as u64;
                let dalsi = q + h.hl.dlzka as u64;
                if dalsi + ramce::NAJMENEJ_HLAVICKY as u64 > self.koniec {
                    if dalsi <= self.koniec {
                        return Ok(Some((q, h)));
                    }
                    continue;
                }
                let hl2 = self.c.bajty(dalsi, ramce::NAJMENEJ_HLAVICKY)?.to_vec();
                if Ramec::z(&hl2).is_some_and(|d| h.rovnaky_tok(&d)) {
                    return Ok(Some((q, h)));
                }
            }
            p += hranica as u64;
        }
        Ok(None)
    }
}

impl<Z: ZdrojBajtov> Kontajner for SuborAc3<Z> {
    fn format(&self) -> FormatKontajnera {
        FormatKontajnera::Ac3 { eac3: self.eac3 }
    }

    fn stopy(&self) -> &[Stopa] {
        &self.stopy
    }

    fn trvanie_ns(&self) -> Option<i64> {
        let paketov = ((self.koniec - self.prvy) as f64 / self.priemer_paketu).round() as u64;
        na_ns((paketov * self.vzoriek_paketu) as i64, self.stopy[0].zakladna)
    }

    fn dalsi_paket(&mut self) -> Vysledok<Option<Paket>> {
        loop {
            if self.p + ramce::NAJMENEJ_HLAVICKY as u64 > self.koniec {
                return Ok(None);
            }
            let p = self.p;
            let hl = self.c.bajty(p, ramce::NAJMENEJ_HLAVICKY)?.to_vec();
            let h =
                Ramec::z(&hl).filter(|h| h.eac3 == self.eac3 && h.hl.frekvencia == self.zvuk_frekvencia());
            match h {
                Some(h) if h.zaciatok_paketu() => {
                    let Some(dlzka) = self.dlzka_paketu(p, &h)? else {
                        // neúplný posledný rámec
                        self.p = self.koniec;
                        return Ok(None);
                    };
                    let data = self.c.presne(p, dlzka)?.to_vec();
                    self.p = p + dlzka as u64;
                    let paket = Paket {
                        stopa: 0,
                        data,
                        pts: Some(self.vzorka as i64),
                        dts: None,
                        trvanie: Some(h.hl.vzoriek as i64),
                        klucovy: true,
                        prerusenie: std::mem::take(&mut self.prerusenie),
                        orezat_zaciatok: 0,
                        orezat_koniec: 0,
                        pozicia: p,
                    };
                    self.vzorka += h.hl.vzoriek as u64;
                    return Ok(Some(paket));
                }
                // rámec iného programu E-AC-3 alebo osamotený závislý podtok: preskočí sa celý
                Some(h) if p + h.hl.dlzka as u64 <= self.koniec => self.p = p + h.hl.dlzka as u64,
                _ => {
                    // poškodenie: ďalší platný rámec; čas podľa priemernej dĺžky (časová os ostane súvislá)
                    self.prerusenie = true;
                    let do_ = self.koniec.min(p.saturating_add(NAJVIAC_HLADANIA));
                    match self.hladaj_ramec(p + 1, do_)? {
                        Some((q, _)) => {
                            let stratenych = ((q - p) as f64 / self.priemer_paketu).round() as u64;
                            self.vzorka += stratenych * self.vzoriek_paketu;
                            self.p = q;
                        }
                        None => {
                            self.p = self.koniec;
                            return Ok(None);
                        }
                    }
                }
            }
        }
    }

    fn posun(&mut self, ciel_ns: i64, _sposob: Posun) -> Vysledok<i64> {
        let zakladna = self.stopy[0].zakladna;
        let ciel = z_ns(ciel_ns.max(0), zakladna).unwrap_or(0).max(0) as u64;
        // o paket skôr: nájdený paket potom nie je za cieľom (každý rámec je kľúčový; prekrytie MDCT dobehne
        // dekodér pri predbehu)
        let paket = (ciel / self.vzoriek_paketu).saturating_sub(1);
        let poloha = (self.prvy + (paket as f64 * self.priemer_paketu) as u64)
            .clamp(self.prvy, self.koniec.saturating_sub(1).max(self.prvy));
        let davka = self.c.davka();
        self.c.nastav_davku(64 << 10);
        let najdeny = self.hladaj_ramec(poloha, self.koniec.min(poloha.saturating_add(NAJVIAC_HLADANIA)))?;
        self.c.nastav_davku(davka);
        let (q, cislo) = match najdeny {
            Some((q, _)) if paket > 0 => (q, ((q - self.prvy) as f64 / self.priemer_paketu).round() as u64),
            _ => (self.prvy, 0),
        };
        self.p = q;
        self.vzorka = cislo * self.vzoriek_paketu;
        self.prerusenie = true;
        Ok(na_ns(self.vzorka as i64, zakladna).unwrap_or(0))
    }
}

impl<Z> SuborAc3<Z> {
    fn zvuk_frekvencia(&self) -> u32 {
        self.stopy.first().and_then(|s| s.zvuk).map_or(0, |z| z.frekvencia)
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::zdroj::PametovyZdroj;

    /// Rámec AC-3 so správnou hlavičkou (obsah za bsi je ľubovoľný – čítačka ho nedekóduje).
    /// 48 kHz, frmsizecod 16 (128 kb/s → 256 slov = 512 B), acmod 2 (2/0), bez LFE.
    fn ramec_ac3(znak: u8) -> Vec<u8> {
        let mut r = vec![znak; 512];
        r[..8].copy_from_slice(&[0x0B, 0x77, 0, 0, 0x10, 8 << 3, 0x40, 0]);
        r
    }

    /// Rámec E-AC-3 (bsid 16): strmtyp, substreamid, frmsiz (slov − 1), 48 kHz, 6 blokov, acmod 7, LFE.
    fn ramec_eac3(zavisly: bool, substreamid: u8, slov: usize, znak: u8) -> Vec<u8> {
        let mut r = vec![znak; slov * 2];
        let frmsiz = slov - 1;
        r[0] = 0x0B;
        r[1] = 0x77;
        r[2] = (if zavisly { 1 } else { 0 }) << 6 | (substreamid & 7) << 3 | (frmsiz >> 8) as u8 & 7;
        r[3] = frmsiz as u8;
        r[4] = 0b0011_1111; // fscod 0 (2 b), numblkscod 3 (2 b), acmod 7 (3 b), lfeon 1
        r[5] = 16 << 3;
        r
    }

    #[test]
    fn surovy_ac3_s_id3_a_odpadom() {
        let mut d = b"ID3\x03\0\0\0\0\0\x05ABCDE".to_vec();
        d.extend_from_slice(&[0x0B, 0x77, 1, 2, 3]); // falošná synchronizácia v odpade
        for k in 0..10 {
            d.extend(ramec_ac3(k));
        }
        let mut s = SuborAc3::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap();
        assert_eq!(s.format(), FormatKontajnera::Ac3 { eac3: false });
        assert_eq!(s.stopy()[0].kodek, Kodek::Ac3);
        assert_eq!(s.stopy()[0].zvuk.unwrap().kanalov, 2);
        let mut casy = Vec::new();
        while let Some(p) = s.dalsi_paket().unwrap() {
            assert_eq!(p.data.len(), 512);
            casy.push(p.pts.unwrap());
        }
        assert_eq!(casy, (0..10).map(|k| k * 1536).collect::<Vec<_>>());
        assert_eq!(s.trvanie_ns(), Some(320_000_000));
        // posun na 100 ms: o paket skôr ako paket 3 (96 ms) – paket 2 (64 ms), prekrytie MDCT dobehne predbeh
        let t = s.posun(100_000_000, Posun::KlucovyPred).unwrap();
        assert_eq!(t, 64_000_000);
        assert_eq!(s.dalsi_paket().unwrap().unwrap().data[100], 2);
    }

    #[test]
    fn eac3_zavisle_podtoky_a_iny_program() {
        let mut d = Vec::new();
        for k in 0..6u8 {
            d.extend(ramec_eac3(false, 0, 300, k));
            d.extend(ramec_eac3(true, 0, 100, 100 + k)); // 7.1: závislý podtok
            d.extend(ramec_eac3(false, 1, 50, 200)); // iný program – preskočí sa
        }
        assert_eq!(je_tok(&d), Some(true));
        let mut s = SuborAc3::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap();
        assert_eq!(s.stopy()[0].kodek, Kodek::Eac3);
        let mut n = 0;
        while let Some(p) = s.dalsi_paket().unwrap() {
            assert_eq!(p.data.len(), 800);
            assert_eq!(p.data[700], 100 + n as u8);
            assert_eq!(p.pts, Some(n * 1536));
            n += 1;
        }
        assert_eq!(n, 6);
    }

    #[test]
    fn poskodenie_a_neuplny_koniec() {
        let mut d = Vec::new();
        for k in 0..4 {
            d.extend(ramec_ac3(k));
        }
        d[512 * 2] = 0; // zničená synchronizácia tretieho rámca
        d.extend(&ramec_ac3(9)[..100]); // neúplný posledný rámec
        let mut s = SuborAc3::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap();
        let mut pakety = Vec::new();
        while let Some(p) = s.dalsi_paket().unwrap() {
            pakety.push((p.pts.unwrap(), p.prerusenie));
        }
        assert_eq!(pakety, vec![(0, true), (1536, false), (4608, true)]);
    }

    #[test]
    fn rozpoznanie() {
        let mut d = ramec_ac3(0);
        d.extend(ramec_ac3(1));
        assert_eq!(je_tok(&d), Some(false));
        assert_eq!(je_tok(&[0x0B, 0x77, 0, 0, 0xFF, 0, 0, 0]), None);
        let mut zly = ramec_ac3(0);
        zly.extend([0u8; 16]);
        assert_eq!(je_tok(&zly), None, "za rámcom nie je ďalšia hlavička");
    }
}
