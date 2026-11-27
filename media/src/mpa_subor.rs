//! **Súbor MPEG audio** (`.mp3`, `.mp2`, `.mpa`) ako kontajner – etapa L8.
//!
//! - Pred prvým rámcom smie byť značka ID3v2, na konci ID3v1 („TAG“, 128 B) a APEv2 („APETAGEX“).
//! - **Prvý rámec** sa prijme, len ak za ním hneď nasleduje ďalšia platná hlavička s rovnakou verziou, vrstvou
//!   a frekvenciou (náhodné 0xFFF v údajoch ID3 alebo v obrázku obalu tak rámec nevytvorí).
//! - **Hlavička Xing / Info** (prvý rámec bez zvuku; vlajky 1 – počet rámcov, 2 – bajtov, 4 – TOC so 100 bodmi,
//!   8 – kvalita) a **VBRI** (Fraunhofer; počet rámcov). Za Xing/Info môže byť **značka LAME** s oneskorením
//!   kodéra a výplňou na konci (12 + 12 bitov na posune 21 od začiatku značky).
//! - **Bezmedzerovo:** dekodér vrstvy III má oneskorenie 529 vzoriek (528 + 1 – konvencia značky LAME, overené
//!   meraním proti FFmpeg); zo začiatku sa oreže `oneskorenie + 529`, z konca `výplň − 529`. Rámec Xing/Info sa
//!   nedekóduje. Dekodéru sa odovzdajú **všetky** zvukové rámce (aj celé orezané) – zásobník bitov a prekrytie.
//! - **Posun:** podľa TOC (VBR) alebo podľa priemernej dĺžky rámca (CBR); poloha sa spresní na platný rámec.
//!   Čas po posune je odhad podľa čísla rámca (formát nemá časové značky) – pri CBR presný, pri VBR ± rámec.

use crate::cas::{na_ns, z_ns, zakladna_z_frekvencie};
use crate::kontajner::{FormatKontajnera, Kontajner, Posun};
use crate::limity::Limity;
use crate::paket::Paket;
use crate::ramce::{HlavickaRamca, mpeg_audio, mpeg_audio_tok};
use crate::stopa::{Kodek, ParametreZvuku, Stopa};
use crate::zdroj::{Citac, ZdrojBajtov};
use crate::znacky::{self, Znacky};
use crate::zvuk::RozlozenieKanalov;
use orchidea_jadro::{Chyba, Vysledok};

/// Koľko bajtov najviac prehľadať pri hľadaní prvého alebo ďalšieho rámca.
const NAJVIAC_HLADANIA: u64 = 4 << 20;
/// Oneskorenie dekodéra vrstvy III (konvencia značky LAME: 528 + 1).
pub const ONESKORENIE_DEKODERA: u64 = 529;

/// Údaje z hlavičky Xing / Info / VBRI a značky LAME.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InfoVbr {
    /// zvukových rámcov (bez rámca Xing/Info)
    pub ramcov: Option<u32>,
    pub bajtov: Option<u32>,
    pub toc: Option<[u8; 100]>,
    /// oneskorenie kodéra a výplň na konci (značka LAME)
    pub oneskorenie: Option<u16>,
    pub vypln: Option<u16>,
    /// `true` – rámec s hlavičkou nie je zvukový (Xing/Info); VBRI je v zvukovom rámci tiež bez zvuku
    pub bez_zvuku: bool,
}

/// Zhodnosť dvoch hlavičiek toku (verzia, vrstva, frekvencia sa v toku nemenia).
fn rovnaky_tok(a: &[u8], b: &[u8]) -> bool {
    a[1] & 0xFE == b[1] & 0xFE && (a[2] >> 2) & 3 == (b[2] >> 2) & 3
}

/// Rozpozná začiatok toku MPEG audio (pre `kontajner::rozpoznaj`): platná hlavička a, ak sa zmestí, aj ďalšia.
pub fn je_tok(zaciatok: &[u8]) -> bool {
    let Some(h) = mpeg_audio(zaciatok) else { return false };
    match zaciatok.get(h.dlzka..h.dlzka + 4) {
        Some(dalsi) => mpeg_audio(dalsi).is_some() && rovnaky_tok(zaciatok, dalsi),
        None => true,
    }
}

/// Rozoberie hlavičku Xing/Info/VBRI v prvom rámci (`r` = celý rámec).
pub fn info_vbr(r: &[u8]) -> Option<InfoVbr> {
    let h = mpeg_audio(r)?;
    let mpeg1 = (r[1] >> 3) & 3 == 3;
    let vedlajsie = match (mpeg1, h.kanalov) {
        (true, 1) => 17,
        (true, _) => 32,
        (false, 1) => 9,
        (false, _) => 17,
    };
    let crc = if r[1] & 1 == 0 { 2 } else { 0 };
    let be32 = |i: usize| r.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    // Xing/Info hneď za vedľajšími informáciami (niektoré kodéry nerátajú CRC – skúsi sa oboje)
    for x in [4 + crc + vedlajsie, 4 + vedlajsie] {
        let Some(znak) = r.get(x..x + 4) else { continue };
        if znak != b"Xing" && znak != b"Info" {
            continue;
        }
        let vlajky = be32(x + 4)?;
        let mut i = x + 8;
        let mut info = InfoVbr { bez_zvuku: true, ..InfoVbr::default() };
        if vlajky & 1 != 0 {
            info.ramcov = be32(i);
            i += 4;
        }
        if vlajky & 2 != 0 {
            info.bajtov = be32(i);
            i += 4;
        }
        if vlajky & 4 != 0 {
            info.toc = r.get(i..i + 100).and_then(|t| t.try_into().ok());
            i += 100;
        }
        if vlajky & 8 != 0 {
            i += 4;
        }
        // značka LAME (alebo Lavf/Lavc – rovnaký tvar): 9 B kodér, …, oneskorenie a výplň na posune 21
        if let Some(l) = r.get(i..i + 24)
            && (l.starts_with(b"LAME")
                || l.starts_with(b"Lavf")
                || l.starts_with(b"Lavc")
                || l.starts_with(b"L3.9"))
        {
            info.oneskorenie = Some(((l[21] as u16) << 4) | (l[22] as u16 >> 4));
            info.vypln = Some((((l[22] & 15) as u16) << 8) | l[23] as u16);
        }
        return Some(info);
    }
    // VBRI (Fraunhofer): 32 B za hlavičkou rámca
    if r.get(36..40) == Some(b"VBRI") {
        return Some(InfoVbr { bajtov: be32(46), ramcov: be32(50), bez_zvuku: true, ..InfoVbr::default() });
    }
    None
}

/// Čítačka súboru MPEG audio.
#[derive(Debug)]
pub struct SuborMpa<Z> {
    c: Citac<Z>,
    stopy: Vec<Stopa>,
    /// prvý zvukový rámec (za Xing/Info)
    prvy: u64,
    /// začiatok rámca s hlavičkou Xing/Info/VBRI (alebo prvý rámec)
    zaciatok_toku: u64,
    koniec: u64,
    info: Option<InfoVbr>,
    vzoriek_ramca: u64,
    /// priemerná dĺžka rámca v bajtoch (CBR podľa toku prvého rámca)
    priemer_ramca: f64,
    /// orezanie zo začiatku (vzorky surového výstupu dekodéra) a celkový počet vzoriek po orezaní
    orez_zaciatok: u64,
    celkom: Option<u64>,
    p: u64,
    /// číslo vzorky (surovej) na začiatku rámca na `p`
    vzorka: u64,
    prerusenie: bool,
    znacky: Option<Znacky>,
}

fn rozlozenie(kanalov: u8) -> RozlozenieKanalov {
    if kanalov == 1 { RozlozenieKanalov::MONO } else { RozlozenieKanalov::STEREO }
}

impl<Z: ZdrojBajtov> SuborMpa<Z> {
    pub fn otvor(zdroj: Z, limity: Limity) -> Vysledok<SuborMpa<Z>> {
        let mut c = Citac::novy(zdroj, Citac::<Z>::PREDVOLENA_DAVKA);
        let dlzka = c.dlzka();
        let mut koniec = dlzka;
        // značky: ID3v2 (začiatok) má prednosť pred APEv2 a ID3v1 (koniec)
        let mut na_konci = Znacky::nove();
        let mut id3v1 = None;
        if koniec >= 128 && c.bajty(koniec - 128, 3)? == b"TAG" {
            id3v1 = znacky::id3v1(c.bajty(koniec - 128, 128)?);
            koniec -= 128;
        }
        // APEv2: päta 32 B „APETAGEX“, veľkosť značky (bez hlavičky) na posune 12, vlajka hlavičky v bite 31
        if koniec >= 32 && c.bajty(koniec - 32, 8)? == b"APETAGEX" {
            let p = c.bajty(koniec - 32, 32)?.to_vec();
            let velkost = u32::from_le_bytes([p[12], p[13], p[14], p[15]]) as u64;
            let s_hlavickou = u32::from_le_bytes([p[20], p[21], p[22], p[23]]) & (1 << 31) != 0;
            if velkost <= koniec.min(limity.prvok)
                && let Ok(d) = c.presne(koniec - velkost, velkost as usize)
                && let Some((z, _)) = znacky::apev2(d)
            {
                na_konci.dopln(z);
            }
            koniec = koniec.saturating_sub(velkost + if s_hlavickou { 32 } else { 0 });
        }
        if let Some(z) = id3v1 {
            na_konci.dopln(z);
        }
        let hlava = c.bajty(0, 10)?.to_vec();
        let za_id3 = if hlava.len() >= 10 && &hlava[..3] == b"ID3" {
            let v = hlava[6..10].iter().fold(0u64, |x, &b| (x << 7) | (b & 0x7F) as u64);
            10 + v + if hlava[5] & 0x10 != 0 { 10 } else { 0 }
        } else {
            0
        };
        let mut vsetky = Znacky::nove();
        if za_id3 > 0
            && let Ok(d) = c.presne(0, za_id3.min(limity.prvok).min(koniec) as usize)
            && let Some((z, _)) = znacky::id3v2(d)
        {
            vsetky.dopln(z);
        }
        vsetky.dopln(na_konci);
        let mut s = SuborMpa {
            c,
            stopy: Vec::new(),
            prvy: 0,
            zaciatok_toku: 0,
            koniec,
            info: None,
            vzoriek_ramca: 1152,
            priemer_ramca: 1.0,
            orez_zaciatok: 0,
            celkom: None,
            p: 0,
            vzorka: 0,
            prerusenie: true,
            znacky: (!vsetky.je_prazdne()).then_some(vsetky),
        };
        let (q, h) = s
            .hladaj_ramec(za_id3, koniec.min(za_id3.saturating_add(NAJVIAC_HLADANIA)))?
            .ok_or_else(|| Chyba::PoskodeneData("MPEG audio: súbor nemá ani jeden platný rámec".into()))?;
        s.zaciatok_toku = q;
        s.prvy = q;
        s.vzoriek_ramca = h.vzoriek as u64;
        let prvy_ramec = s.c.presne(q, h.dlzka)?.to_vec();
        // CBR: presná priemerná dĺžka rámca z toku (výplňové bajty ju dorovnávajú)
        let tok = mpeg_audio_tok(&prvy_ramec).unwrap_or(0) as f64;
        s.priemer_ramca = (h.vzoriek as f64 / 8.0 * tok / h.frekvencia as f64).max(1.0);
        if let Some(info) = info_vbr(&prvy_ramec) {
            if info.bez_zvuku {
                s.prvy = q + h.dlzka as u64;
            }
            if let (Some(n), Some(b)) = (info.ramcov, info.bajtov)
                && n > 0
            {
                s.priemer_ramca = b as f64 / (n as f64 + info.bez_zvuku as u8 as f64);
            }
            if h.vrstva == 3 && info.oneskorenie.is_some() {
                s.orez_zaciatok = info.oneskorenie.unwrap_or(0) as u64 + ONESKORENIE_DEKODERA;
            }
            if let Some(n) = info.ramcov {
                let surove = n as u64 * s.vzoriek_ramca;
                let vypln = info.vypln.map_or(0, |v| (v as u64).saturating_sub(ONESKORENIE_DEKODERA));
                s.celkom = Some(surove.saturating_sub(s.orez_zaciatok + vypln));
            }
            s.info = Some(info);
        }
        s.p = s.prvy;
        let zakladna = zakladna_z_frekvencie(h.frekvencia as u64)
            .ok_or_else(|| Chyba::PoskodeneData("MPEG audio: frekvencia".into()))?;
        let mut stopa = Stopa::nova(1, Kodek::MpegAudio { vrstva: h.vrstva }, zakladna);
        stopa.predvolena = true;
        stopa.zvuk = Some(ParametreZvuku {
            frekvencia: h.frekvencia,
            kanalov: h.kanalov,
            rozlozenie: rozlozenie(h.kanalov),
            bitov: None,
        });
        s.stopy.push(stopa);
        Ok(s)
    }

    /// Údaje Xing/Info/VBRI a LAME, ak sú.
    pub fn info(&self) -> Option<&InfoVbr> {
        self.info.as_ref()
    }

    /// Čítač (štatistika čítaní – chladné čítanie).
    pub fn citac(&self) -> &Citac<Z> {
        &self.c
    }

    /// Najbližší rámec od `od`, za ktorým nasleduje ďalšia zhodná hlavička (alebo koniec súboru).
    fn hladaj_ramec(&mut self, od: u64, do_: u64) -> Vysledok<Option<(u64, HlavickaRamca)>> {
        const KUS: usize = 64 << 10;
        let mut p = od;
        while p < do_ {
            let kus = self.c.bajty(p, KUS + 4)?.to_vec();
            if kus.len() < 4 {
                break;
            }
            let hranica = kus.len().saturating_sub(4).max(1);
            for i in 0..hranica {
                if kus[i] != 0xFF || p + i as u64 >= do_ {
                    continue;
                }
                let Some(h) = mpeg_audio(&kus[i..]) else { continue };
                let q = p + i as u64;
                let dalsi = q + h.dlzka as u64;
                if dalsi + 4 > self.koniec {
                    if dalsi <= self.koniec {
                        return Ok(Some((q, h)));
                    }
                    continue;
                }
                let hl2 = self.c.bajty(dalsi, 4)?.to_vec();
                if mpeg_audio(&hl2).is_some() && rovnaky_tok(&kus[i..i + 4], &hl2) {
                    return Ok(Some((q, h)));
                }
            }
            p += hranica as u64;
        }
        Ok(None)
    }

    fn paket(&self, p: u64, data: Vec<u8>, vzorka: u64) -> Paket {
        let n = self.vzoriek_ramca;
        let z = self.orez_zaciatok;
        let e = self.celkom.map_or(u64::MAX, |c| z + c);
        let od = vzorka.max(z).min(vzorka + n);
        let po = (vzorka + n).min(e).max(od);
        Paket {
            stopa: 0,
            data,
            // čas prvej vzorky rámca pred orezaním (ako v MP4 so zoznamom úprav – prvý rámec má čas záporný)
            pts: Some(vzorka as i64 - z as i64),
            dts: None,
            trvanie: Some(n as i64),
            klucovy: true,
            prerusenie: false,
            orezat_zaciatok: (od - vzorka) as u32,
            orezat_koniec: (vzorka + n - po) as u32,
            pozicia: p,
        }
    }
}

impl<Z: ZdrojBajtov> Kontajner for SuborMpa<Z> {
    fn znacky(&self) -> Option<&Znacky> {
        self.znacky.as_ref()
    }

    fn format(&self) -> FormatKontajnera {
        FormatKontajnera::MpegAudio
    }

    fn stopy(&self) -> &[Stopa] {
        &self.stopy
    }

    fn trvanie_ns(&self) -> Option<i64> {
        let vzoriek = match self.celkom {
            Some(c) => c,
            None => {
                let ramcov = ((self.koniec - self.prvy) as f64 / self.priemer_ramca).round() as u64;
                (ramcov * self.vzoriek_ramca).saturating_sub(self.orez_zaciatok)
            }
        };
        na_ns(vzoriek as i64, self.stopy[0].zakladna)
    }

    fn dalsi_paket(&mut self) -> Vysledok<Option<Paket>> {
        loop {
            if self.p + 4 > self.koniec {
                return Ok(None);
            }
            let p = self.p;
            let hlava = self.c.bajty(p, 4)?.to_vec();
            match mpeg_audio(&hlava) {
                Some(h) if p + h.dlzka as u64 <= self.koniec => {
                    let data = self.c.presne(p, h.dlzka)?.to_vec();
                    self.p = p + h.dlzka as u64;
                    let mut paket = self.paket(p, data, self.vzorka);
                    paket.prerusenie = std::mem::take(&mut self.prerusenie);
                    self.vzorka += h.vzoriek as u64;
                    return Ok(Some(paket));
                }
                _ => {
                    // poškodenie alebo neúplný posledný rámec: ďalší platný rámec
                    self.prerusenie = true;
                    let do_ = self.koniec.min(p.saturating_add(NAJVIAC_HLADANIA));
                    match self.hladaj_ramec(p + 1, do_)? {
                        Some((q, _)) => {
                            // stratené rámce: odhad podľa priemernej dĺžky (časová os ostane súvislá)
                            let stratenych = ((q - p) as f64 / self.priemer_ramca).round() as u64;
                            self.vzorka += stratenych * self.vzoriek_ramca;
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
        let ciel = z_ns(ciel_ns.max(0), zakladna).unwrap_or(0).max(0) as u64 + self.orez_zaciatok;
        let mut ramec = ciel / self.vzoriek_ramca;
        let ramcov = self.info.as_ref().and_then(|i| i.ramcov).map(|n| n as u64);
        if let Some(n) = ramcov {
            ramec = ramec.min(n.saturating_sub(1));
        }
        let dlzka_toku = (self.koniec - self.zaciatok_toku) as f64;
        // poloha: TOC (percentá trvania → podiel bajtov /256), inak priemerná dĺžka rámca
        let poloha = match (self.info.as_ref().and_then(|i| i.toc.as_ref()), ramcov) {
            (Some(toc), Some(n)) if n > 0 => {
                let pct = (ramec as f64 / n as f64 * 100.0).clamp(0.0, 99.999);
                let i = pct as usize;
                let a = toc[i] as f64;
                let b = if i + 1 < 100 { toc[i + 1] as f64 } else { 256.0 };
                let bajtov = self.info.as_ref().and_then(|x| x.bajtov).map_or(dlzka_toku, |b| b as f64);
                self.zaciatok_toku + ((a + (b - a) * (pct - i as f64)) / 256.0 * bajtov) as u64
            }
            // o rámec skôr: nájdený rámec potom nie je za cieľom (posun na kľúčový paket ≤ cieľ)
            _ => self.prvy + (ramec.saturating_sub(1) as f64 * self.priemer_ramca) as u64,
        };
        let poloha = poloha.clamp(self.prvy, self.koniec.saturating_sub(1).max(self.prvy));
        let davka = self.c.davka();
        self.c.nastav_davku(64 << 10);
        let najdeny = self.hladaj_ramec(poloha, self.koniec.min(poloha.saturating_add(NAJVIAC_HLADANIA)))?;
        self.c.nastav_davku(davka);
        let (q, cislo) = match najdeny {
            Some((q, _)) if ramec > 0 => {
                // pri CBR je číslo rámca z polohy presné; pri TOC sa ponechá cieľový rámec (odhad)
                let cislo = if self.info.as_ref().is_some_and(|i| i.toc.is_some()) {
                    ramec
                } else {
                    ((q - self.prvy) as f64 / self.priemer_ramca).round() as u64
                };
                (q, cislo)
            }
            _ => (self.prvy, 0),
        };
        self.p = q;
        self.vzorka = cislo * self.vzoriek_ramca;
        self.prerusenie = true;
        let cas = (self.vzorka).saturating_sub(self.orez_zaciatok);
        Ok(na_ns(cas as i64, zakladna).unwrap_or(0))
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::zdroj::PametovyZdroj;

    /// Tichý rámec MPEG-1 vrstvy III, 128 kb/s, 44,1 kHz, stereo (417 B, s výplňou 418 B).
    fn ramec(vypln: bool) -> Vec<u8> {
        let mut r = vec![0u8; if vypln { 418 } else { 417 }];
        r[..4].copy_from_slice(&[0xFF, 0xFB, 0x90 | if vypln { 2 } else { 0 }, 0x00]);
        r
    }

    fn subor(ramcov: usize, xing: bool) -> Vec<u8> {
        let mut d = b"ID3\x04\x00\x00\x00\x00\x00\x05\xFF\xFB\x00\x00\x00".to_vec();
        if xing {
            let mut x = ramec(false);
            let o = 4 + 32;
            x[o..o + 4].copy_from_slice(b"Info");
            x[o + 4..o + 8].copy_from_slice(&3u32.to_be_bytes());
            x[o + 8..o + 12].copy_from_slice(&(ramcov as u32).to_be_bytes());
            x[o + 12..o + 16].copy_from_slice(&((ramcov * 417 + 417) as u32).to_be_bytes());
            let l = o + 16;
            x[l..l + 9].copy_from_slice(b"LAME3.100");
            // oneskorenie 576 = 0x240, výplň 1440 = 0x5A0 (12 + 12 bitov)
            x[l + 21..l + 24].copy_from_slice(&[0x24, 0x05, 0xA0]);
            d.extend(x);
        }
        for i in 0..ramcov {
            d.extend(ramec(i % 3 == 2));
        }
        d.extend(b"TAG");
        d.extend([0u8; 125]);
        d
    }

    #[test]
    fn bezmedzerove_orezanie() {
        let mut s = SuborMpa::otvor(PametovyZdroj::novy(subor(10, true)), Limity::default()).unwrap();
        let info = s.info().unwrap().clone();
        assert_eq!((info.ramcov, info.oneskorenie, info.vypln), (Some(10), Some(576), Some(1440)));
        let mut pakety = Vec::new();
        while let Some(p) = s.dalsi_paket().unwrap() {
            pakety.push(p);
        }
        // všetky zvukové rámce (bez rámca Info), spolu presne 10·1152 − 1105 − 911 vzoriek
        assert_eq!(pakety.len(), 10);
        let spolu: i64 =
            pakety.iter().map(|p| p.trvanie.unwrap() - (p.orezat_zaciatok + p.orezat_koniec) as i64).sum();
        assert_eq!(spolu, 10 * 1152 - 1105 - 911);
        assert_eq!((pakety[0].orezat_zaciatok, pakety[0].pts), (1105, Some(-1105)));
        assert_eq!(pakety[1].pts, Some(1152 - 1105));
        assert_eq!(pakety[9].orezat_koniec, 911);
        assert_eq!(s.trvanie_ns(), na_ns(spolu, s.stopy()[0].zakladna));
    }

    #[test]
    fn bez_xing_a_posun_cbr() {
        let mut s = SuborMpa::otvor(PametovyZdroj::novy(subor(40, false)), Limity::default()).unwrap();
        assert!(s.info().is_none());
        assert_eq!(s.dalsi_paket().unwrap().unwrap().orezat_zaciatok, 0);
        // posun na rámec 25: čas podľa čísla rámca, ďalší paket je presne ten rámec
        let zakladna = s.stopy()[0].zakladna;
        let t = s.posun(na_ns(25 * 1152 + 100, zakladna).unwrap(), Posun::KlucovyPred).unwrap();
        assert_eq!(t, na_ns(25 * 1152, zakladna).unwrap());
        let p = s.dalsi_paket().unwrap().unwrap();
        assert_eq!(p.pts, Some(25 * 1152));
        assert!(p.prerusenie);
        let n = 1 + std::iter::from_fn(|| s.dalsi_paket().unwrap()).count();
        assert_eq!(n, 15);
    }

    #[test]
    fn rozpoznanie_toku() {
        let d = subor(3, false);
        assert!(je_tok(&d[15..]));
        // za hlavičkou nenasleduje ďalšia (náhodné 0xFFF v údajoch)
        let mut zly = ramec(false);
        zly.extend([0u8; 4]);
        assert!(!je_tok(&zly));
        assert!(!je_tok(b"fLaC"));
    }
}
