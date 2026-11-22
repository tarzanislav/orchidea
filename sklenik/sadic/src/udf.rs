//! UDF 2.01 vlastným kódom – systém, ktorý bez ďalších ovládačov číta aj zapisuje Windows, macOS
//! aj Linux. Rozvrh ako mkudffs pre pevné disky: rozpoznávacia postupnosť (BEA01, NSR03, TEA01) na
//! 32 KB, hlavná a záložná postupnosť popisov zväzku, záznam celistvosti, kotvy na bloku 256, N − 256
//! a N, oddiel s bitmapou voľného miesta, popisom množiny súborov a koreňovým priečinkom (rozšírený
//! záznam súboru s údajmi priamo v ICB). Blok = sektor disku (inak ho Windows nepripojí); zapisuje
//! sa na celý disk s pomocnou tabuľkou MBR, ktorej oddiel začína na sektore 0 (tak UDF na USB
//! pripojí aj Windows aj macOS).

use crate::ciel::{self, Ciel};
use crate::oddiely::{jedinecne, obciansky, teraz};
use std::io;
use std::sync::atomic::AtomicBool;

/// Verzia popisov (ECMA-167 3. vydanie, NSR03 – vyžaduje UDF 2.00 a novšie).
const VERZIA: u16 = 3;
const MVDS: u64 = 96;
const LVID: u64 = 128;
const KOTVA: u64 = 256;
const ODDIEL: u64 = 257;
/// Bloky za oddielom: medzera, kotva N − 256, medzera, záložná postupnosť (N − 160), medzera, kotva N.
const ZA_ODDIELOM: u64 = 263;
const IDENT: &str = "*SADIC";

/// Navrhnutý zväzok UDF.
#[derive(Debug, Clone)]
pub struct Udf {
    blok: u64,
    blokov: u64,
    menovka: String,
    uuid: String,
}

/// CRC-ITU-T (CCITT, polynóm 0x1021, začiatok 0) – kontrola popisov UDF.
fn crc(d: &[u8]) -> u16 {
    let mut c: u16 = 0;
    for &b in d {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

fn le16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn le32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn le64(d: &mut [u8], o: usize, v: u64) {
    d[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// Doplní značku popisu (16 B): identifikátor, verzia, sériové číslo, CRC obsahu a kontrolný súčet.
fn znacka(d: &mut [u8], id: u16, poloha: u32, dlzka_crc: usize) {
    le16(d, 0, id);
    le16(d, 2, VERZIA);
    le16(d, 6, 1);
    let c = crc(&d[16..16 + dlzka_crc]);
    le16(d, 8, c);
    le16(d, 10, dlzka_crc as u16);
    le32(d, 12, poloha);
    d[4] = 0;
    d[4] = d[..16].iter().enumerate().filter(|&(i, _)| i != 4).fold(0u8, |s, (_, &b)| s.wrapping_add(b));
}

/// Reťazec d-string: kompresia 8 (Latin-1) alebo 16 (UTF-16BE), na konci dĺžka.
fn dstring(d: &mut [u8], o: usize, n: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    let siroky = text.chars().any(|c| c as u32 > 0xFF);
    let mut b = vec![if siroky { 16u8 } else { 8 }];
    for c in text.chars() {
        let mut j = [0u16; 2];
        let kus: Vec<u8> = if siroky {
            c.encode_utf16(&mut j).iter().flat_map(|u| u.to_be_bytes()).collect()
        } else {
            vec![c as u8]
        };
        if b.len() + kus.len() > n - 1 {
            break;
        }
        b.extend(kus);
    }
    d[o..o + b.len()].copy_from_slice(&b);
    d[o + n - 1] = b.len() as u8;
}

/// Sada znakov CS0 „OSTA Compressed Unicode“ (64 B).
fn sada_znakov(d: &mut [u8], o: usize) {
    d[o] = 0;
    d[o + 1..o + 24].copy_from_slice(b"OSTA Compressed Unicode");
}

/// Identifikátor (regid, 32 B): príznaky, text a prípona.
fn regid(d: &mut [u8], o: usize, text: &str, pripona: &[u8]) {
    d[o + 1..o + 1 + text.len()].copy_from_slice(text.as_bytes());
    d[o + 24..o + 24 + pripona.len()].copy_from_slice(pripona);
}

/// Prípona domény a identifikátorov UDF: revízia 2.01.
const REVIZIA: [u8; 2] = [0x01, 0x02];

/// Časová pečiatka (12 B) v UTC.
fn peciatka(d: &mut [u8], o: usize, unix: u64) {
    let (rok, mesiac, den, h, m, s) = obciansky(unix);
    le16(d, o, 0x1000); // typ 1 (miestny čas), pásmo +0 min
    le16(d, o + 2, rok as u16);
    d[o + 4..o + 9].copy_from_slice(&[mesiac, den, h, m, s]);
}

/// long_ad: dĺžka, blok v oddiele, číslo oddielu.
fn long_ad(d: &mut [u8], o: usize, dlzka: u32, blok: u32) {
    le32(d, o, dlzka);
    le32(d, o + 4, blok);
}

impl Udf {
    /// Navrhne UDF pre celý disk (`sektory` × `sektor`).
    pub fn navrhni(sektory: u64, sektor: u32, menovka: &str) -> Result<Udf, String> {
        let blok = sektor as u64;
        if blok != 512 && blok != 4096 {
            return Err(format!("UDF: nepodporovaná veľkosť sektora {sektor} B"));
        }
        if sektory * blok < 8 << 20 {
            return Err("disk je primalý pre UDF (aspoň 8 MB)".into());
        }
        if sektory > u32::MAX as u64 {
            return Err("disk je priveľký pre UDF (čísla blokov majú 32 bitov)".into());
        }
        let m = menovka.trim();
        let n = jedinecne();
        let uuid: String = n[..8].iter().map(|b| format!("{b:02x}")).collect();
        Ok(Udf {
            blok,
            blokov: sektory,
            menovka: if m.is_empty() { "SADIC".into() } else { m.to_string() },
            uuid,
        })
    }

    fn dlzka_oddielu(&self) -> u64 {
        self.blokov - ODDIEL - ZA_ODDIELOM
    }

    /// Bloky bitmapy voľného miesta (popis 24 B + bit na blok oddielu).
    fn blokov_bitmapy(&self) -> u64 {
        (24 + self.dlzka_oddielu().div_ceil(8)).div_ceil(self.blok)
    }

    /// Obsadené bloky oddielu: bitmapa, popis množiny súborov, koreňový priečinok.
    fn obsadenych(&self) -> u64 {
        self.blokov_bitmapy() + 2
    }

    fn popis(&self) -> Vec<u8> {
        vec![0u8; 512]
    }

    /// Hlavná (aj záložná) postupnosť popisov zväzku: PVD, LVD, PD, USD, IUVD, TD.
    fn postupnost(&self, zac: u64, cas: u64) -> Vec<u8> {
        let b = self.blok as usize;
        let mut v = vec![0u8; 16 * b];
        let poloha = |i: u64| (zac + i) as u32;
        // PVD
        let mut d = self.popis();
        le32(&mut d, 16, 1);
        dstring(&mut d, 24, 32, &self.menovka);
        le16(&mut d, 56, 1);
        le16(&mut d, 58, 1);
        le16(&mut d, 60, 2);
        le16(&mut d, 62, 3);
        le32(&mut d, 64, 1);
        le32(&mut d, 68, 1);
        dstring(&mut d, 72, 128, &format!("{}SADIC", self.uuid));
        sada_znakov(&mut d, 200);
        sada_znakov(&mut d, 264);
        regid(&mut d, 344, IDENT, &[]);
        peciatka(&mut d, 376, cas);
        regid(&mut d, 388, IDENT, &[]);
        le16(&mut d, 488, 1);
        znacka(&mut d, 1, poloha(0), 496);
        v[..512].copy_from_slice(&d);
        // LVD
        let mut d = self.popis();
        le32(&mut d, 16, 2);
        sada_znakov(&mut d, 20);
        dstring(&mut d, 84, 128, &self.menovka);
        le32(&mut d, 212, self.blok as u32);
        regid(&mut d, 216, "*OSTA UDF Compliant", &REVIZIA);
        long_ad(&mut d, 248, self.blok as u32, self.blokov_bitmapy() as u32);
        le32(&mut d, 264, 6);
        le32(&mut d, 268, 1);
        regid(&mut d, 272, IDENT, &[]);
        le32(&mut d, 432, (2 * self.blok).max(8192) as u32);
        le32(&mut d, 436, LVID as u32);
        d[440..446].copy_from_slice(&[1, 6, 1, 0, 0, 0]); // mapa oddielu typu 1: zväzok 1, oddiel 0
        znacka(&mut d, 6, poloha(1), 430);
        v[b..b + 512].copy_from_slice(&d);
        // PD
        let mut d = self.popis();
        le32(&mut d, 16, 3);
        le16(&mut d, 20, 1);
        regid(&mut d, 24, "+NSR03", &[]);
        le32(&mut d, 64, (self.blokov_bitmapy() * self.blok) as u32); // bitmapa voľných blokov na začiatku oddielu
        le32(&mut d, 184, 4); // prepisovateľný
        le32(&mut d, 188, ODDIEL as u32);
        le32(&mut d, 192, self.dlzka_oddielu() as u32);
        regid(&mut d, 196, IDENT, &[]);
        znacka(&mut d, 5, poloha(2), 496);
        v[2 * b..2 * b + 512].copy_from_slice(&d);
        // USD (žiadne nepridelené miesto mimo oddielu)
        let mut d = self.popis();
        le32(&mut d, 16, 4);
        znacka(&mut d, 7, poloha(3), 8);
        v[3 * b..3 * b + 512].copy_from_slice(&d);
        // IUVD s informáciami o logickom zväzku
        let mut d = self.popis();
        le32(&mut d, 16, 5);
        regid(&mut d, 20, "*UDF LV Info", &REVIZIA);
        sada_znakov(&mut d, 52);
        dstring(&mut d, 116, 128, &self.menovka);
        regid(&mut d, 352, IDENT, &[]);
        znacka(&mut d, 4, poloha(4), 496);
        v[4 * b..4 * b + 512].copy_from_slice(&d);
        // TD
        let mut d = self.popis();
        znacka(&mut d, 8, poloha(5), 496);
        v[5 * b..5 * b + 512].copy_from_slice(&d);
        v
    }

    /// Záznam celistvosti logického zväzku (zatvorený) a ukončenie.
    fn celistvost(&self, cas: u64) -> Vec<u8> {
        let b = self.blok as usize;
        let mut v = vec![0u8; 2 * b];
        let mut d = vec![0u8; 134];
        peciatka(&mut d, 16, cas);
        le32(&mut d, 28, 1);
        le64(&mut d, 40, 16); // ďalšie jedinečné číslo
        le32(&mut d, 72, 1);
        le32(&mut d, 76, 46);
        le32(&mut d, 80, (self.dlzka_oddielu() - self.obsadenych()) as u32);
        le32(&mut d, 84, self.dlzka_oddielu() as u32);
        regid(&mut d, 88, IDENT, &[]);
        le32(&mut d, 124, 1); // priečinkov: koreň
        for o in [128, 130, 132] {
            d[o..o + 2].copy_from_slice(&REVIZIA);
        }
        znacka(&mut d, 9, LVID as u32, 118);
        v[..134].copy_from_slice(&d);
        let mut t = self.popis();
        znacka(&mut t, 8, LVID as u32 + 1, 496);
        v[b..b + 512].copy_from_slice(&t);
        v
    }

    fn kotva(&self, poloha: u64) -> Vec<u8> {
        let mut d = vec![0u8; self.blok as usize];
        let dlzka = (16 * self.blok) as u32;
        le32(&mut d, 16, dlzka);
        le32(&mut d, 20, MVDS as u32);
        le32(&mut d, 24, dlzka);
        le32(&mut d, 28, (self.blokov - 160) as u32);
        znacka(&mut d, 2, poloha as u32, 496);
        d
    }

    /// Začiatok oddielu: bitmapa voľného miesta, popis množiny súborov, koreňový priečinok.
    fn obsah_oddielu(&self, cas: u64) -> Vec<u8> {
        let b = self.blok as usize;
        let n = self.blokov_bitmapy() as usize;
        let mut v = vec![0u8; (n + 2) * b];
        // bitmapa: 1 = voľný blok, bit 0 bajtu 0 = blok 0
        let bitov = self.dlzka_oddielu();
        le32(&mut v, 16, bitov as u32);
        le32(&mut v, 20, bitov.div_ceil(8) as u32);
        let obsadene = self.obsadenych();
        let cele_od = obsadene.div_ceil(8);
        for blok in obsadene..(cele_od * 8).min(bitov) {
            v[24 + (blok / 8) as usize] |= 1 << (blok % 8);
        }
        let cele_do = bitov / 8;
        if cele_do > cele_od {
            v[24 + cele_od as usize..24 + cele_do as usize].fill(0xFF);
        }
        for blok in (cele_do * 8).max(obsadene)..bitov {
            v[24 + (blok / 8) as usize] |= 1 << (blok % 8);
        }
        znacka(&mut v[..24], 264, 0, 8);
        // popis množiny súborov
        let fsd = self.blokov_bitmapy() as u32;
        let koren = fsd + 1;
        let mut d = vec![0u8; 512];
        peciatka(&mut d, 16, cas);
        le16(&mut d, 28, 3);
        le16(&mut d, 30, 3);
        le32(&mut d, 32, 1);
        le32(&mut d, 36, 1);
        sada_znakov(&mut d, 48);
        dstring(&mut d, 112, 128, &self.menovka);
        sada_znakov(&mut d, 240);
        dstring(&mut d, 304, 32, &self.menovka);
        long_ad(&mut d, 400, self.blok as u32, koren);
        regid(&mut d, 416, "*OSTA UDF Compliant", &REVIZIA);
        znacka(&mut d, 256, fsd, 496);
        v[n * b..n * b + 512].copy_from_slice(&d);
        // koreňový priečinok: rozšírený záznam súboru, obsah (odkaz na rodiča) priamo v ICB
        let mut e = vec![0u8; 256];
        le16(&mut e, 20, 4); // stratégia 4
        le16(&mut e, 24, 1);
        e[27] = 4; // priečinok
        le16(&mut e, 34, 3); // údaje v ICB
        // vlastník a skupina neurčení (každý systém dosadí toho, kto disk pripojil), práva rwx pre všetkých
        le32(&mut e, 36, u32::MAX);
        le32(&mut e, 40, u32::MAX);
        le32(&mut e, 44, 0x7CE7);
        le16(&mut e, 48, 1);
        le64(&mut e, 56, 40);
        le64(&mut e, 64, 40);
        for o in [80, 92, 104, 116] {
            peciatka(&mut e, o, cas);
        }
        le32(&mut e, 128, 1);
        regid(&mut e, 168, IDENT, &[]);
        le32(&mut e, 212, 40);
        // FID rodiča
        let mut f = vec![0u8; 40];
        le16(&mut f, 16, 1);
        f[18] = 0x0A; // priečinok + rodič
        long_ad(&mut f, 20, self.blok as u32, koren);
        znacka(&mut f, 257, koren, 24);
        e[216..256].copy_from_slice(&f);
        znacka(&mut e, 266, koren, 240);
        v[(n + 1) * b..(n + 1) * b + 256].copy_from_slice(&e);
        v
    }

    /// Zapíše prázdny zväzok UDF od bajtu `posun` (celý disk).
    pub fn naformatuj(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<()> {
        let b = self.blok;
        let cas = teraz();
        // bloky 0 – 256: zavádzacia oblasť (nuly), rozpoznávacia postupnosť, popisy, celistvosť, kotva
        let mut zac = vec![0u8; ((ODDIEL) * b) as usize];
        let krok = b.max(2048) as usize;
        for (i, id) in [b"BEA01", b"NSR03", b"TEA01"].iter().enumerate() {
            let o = 32_768 + i * krok;
            zac[o + 1..o + 6].copy_from_slice(*id);
            zac[o + 6] = 1;
        }
        let mvds = self.postupnost(MVDS, cas);
        zac[(MVDS * b) as usize..(MVDS * b) as usize + mvds.len()].copy_from_slice(&mvds);
        let lvid = self.celistvost(cas);
        zac[(LVID * b) as usize..(LVID * b) as usize + lvid.len()].copy_from_slice(&lvid);
        zac[(KOTVA * b) as usize..].copy_from_slice(&self.kotva(KOTVA));
        ciel::zapis_vsetko(c, posun, &zac)?;
        // oddiel: bitmapa, popis množiny súborov, koreň
        let obsah = self.obsah_oddielu(cas);
        let celkom = zac.len() as u64 + obsah.len() as u64 + ZA_ODDIELOM * b;
        priebeh(zac.len() as u64, celkom);
        let hotovo = zac.len() as u64;
        for (i, kus) in obsah.chunks(1 << 20).enumerate() {
            if zrus.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(io::Error::other("zrušené"));
            }
            c.zapis(posun + ODDIEL * b + (i << 20) as u64, kus)?;
            priebeh(hotovo + ((i as u64 + 1) << 20).min(obsah.len() as u64), celkom);
        }
        // za oddielom: kotva N − 256, záložná postupnosť na N − 160, kotva N
        let od = self.blokov - ZA_ODDIELOM;
        let mut kon = vec![0u8; (ZA_ODDIELOM * b) as usize];
        let at = |blok: u64| ((blok - od) * b) as usize;
        kon[at(self.blokov - 257)..at(self.blokov - 256)].copy_from_slice(&self.kotva(self.blokov - 257));
        let rvds = self.postupnost(self.blokov - 160, cas);
        kon[at(self.blokov - 160)..at(self.blokov - 160) + rvds.len()].copy_from_slice(&rvds);
        kon[at(self.blokov - 1)..].copy_from_slice(&self.kotva(self.blokov - 1));
        ciel::zapis_vsetko(c, posun + od * b, &kon)?;
        priebeh(celkom, celkom);
        Ok(())
    }
}

impl crate::subory::Zvazok for Udf {
    fn klaster_b(&self) -> u64 {
        self.blok
    }
    fn naplnene(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        strom: &crate::subory::Strom,
        _citaj: &mut crate::subory::Citaj,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<Vec<crate::subory::Zapisany>> {
        if !strom.subory().is_empty() || strom.uzly.len() > 1 {
            return Err(io::Error::other("zápis súborov na UDF zatiaľ nie je – len formátovanie"));
        }
        self.naformatuj(c, posun, priebeh, zrus)?;
        Ok(Vec::new())
    }
    fn over(&self, zaciatok: &[u8]) -> bool {
        zaciatok.len() >= 32_774 && &zaciatok[32_769..32_774] == b"BEA01"
    }
    fn zmesti_sa(&self, strom: &crate::subory::Strom) -> bool {
        strom.uzly.len() <= 1
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::ciel::Pamat;

    #[test]
    fn crc_ako_v_norme() {
        // CRC-16/XMODEM (= CRC-ITU-T zo začiatku 0) z „123456789“
        assert_eq!(crc(b"123456789"), 0x31C3);
    }

    #[test]
    fn rozvrh_a_kotvy() {
        for sektor in [512u32, 4096] {
            let n = (64u64 << 20) / sektor as u64;
            let mut p = Pamat::nova(64 << 20, sektor);
            let u = Udf::navrhni(n, sektor, "Skúška").unwrap();
            u.naformatuj(&mut p, 0, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
            let mut z = vec![0u8; 65_536];
            p.citaj(0, &mut z).unwrap();
            assert!(crate::subory::Zvazok::over(&u, &z));
            let b = sektor as u64;
            for kotva in [256, n - 257, n - 1] {
                let mut k = vec![0u8; b as usize];
                p.citaj(kotva * b, &mut k).unwrap();
                assert_eq!(u16::from_le_bytes([k[0], k[1]]), 2, "kotva na {kotva}");
                assert_eq!(u32::from_le_bytes(k[12..16].try_into().unwrap()), kotva as u32);
                assert_eq!(u16::from_le_bytes([k[8], k[9]]), crc(&k[16..512]));
            }
        }
    }
}
