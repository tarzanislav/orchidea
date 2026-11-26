//! **FLAC** – bezstratový zvuk podľa RFC 9639 (etapa L2): STREAMINFO, hlavička rámca s CRC-8, podrámce
//! (konštantný, doslovný, pevný predpovedač rádu 0–4, lineárna predikcia rádu 1–32), zvyšky v kódovaní Rice
//! (aj únikové oddiely), zbytočné bity, dekorelácia sterea (ľavý/bočný, bočný/pravý, stredný/bočný), CRC-16 rámca,
//! 32-bitové vzorky (bočný kanál 33 bitov – počíta sa v `i64`).
//!
//! Dôkaz: MD5 dekódovaného zvuku sa rovná MD5 v STREAMINFO (RFC 9639, kap. 8.2) – bit po bite.

use crate::paket::Paket;
use crate::stopa::ParametreZvuku;
use crate::zvuk::{BlokZvuku, DekoderZvuku, RozlozenieKanalov, VzorkyZvuku};
use orchidea_jadro::{Chyba, Vysledok};

fn chyba(s: &str) -> Chyba {
    Chyba::PoskodeneData(format!("FLAC: {s}"))
}

/// CRC-8 hlavičky rámca: mnohočlen x^8 + x^2 + x + 1 (0x07), začiatok 0.
const CRC8: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// CRC-16 rámca: mnohočlen x^16 + x^15 + x^2 + 1 (0x8005), začiatok 0.
const CRC16: [u16; 256] = {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

pub fn crc8(d: &[u8]) -> u8 {
    d.iter().fold(0u8, |c, &b| CRC8[(c ^ b) as usize])
}

/// Priebežný CRC-16 (pridanie jedného bajtu).
pub fn crc16_bajt(c: u16, b: u8) -> u16 {
    (c << 8) ^ CRC16[((c >> 8) as u8 ^ b) as usize]
}

pub fn crc16(d: &[u8]) -> u16 {
    d.iter().fold(0u16, |c, &b| crc16_bajt(c, b))
}

/// Údaje bloku metadát STREAMINFO (RFC 9639, kap. 8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub min_blok: u16,
    pub max_blok: u16,
    pub min_ramec: u32,
    pub max_ramec: u32,
    pub frekvencia: u32,
    pub kanalov: u8,
    pub bitov: u8,
    /// Vzoriek na kanál spolu (0 = neznáme).
    pub vzoriek: u64,
    /// MD5 nekomprimovaného zvuku (samé nuly = neuvedené).
    pub md5: [u8; 16],
}

impl StreamInfo {
    /// Telo bloku STREAMINFO (34 B).
    pub fn z_tela(d: &[u8]) -> Vysledok<StreamInfo> {
        if d.len() < 34 {
            return Err(chyba("STREAMINFO je kratšie ako 34 B"));
        }
        let u = |a: usize, b: usize| d[a..b].iter().fold(0u64, |x, &c| (x << 8) | c as u64);
        let x = u(10, 18);
        let si = StreamInfo {
            min_blok: u(0, 2) as u16,
            max_blok: u(2, 4) as u16,
            min_ramec: u(4, 7) as u32,
            max_ramec: u(7, 10) as u32,
            frekvencia: (x >> 44) as u32,
            kanalov: ((x >> 41) & 7) as u8 + 1,
            bitov: ((x >> 36) & 31) as u8 + 1,
            vzoriek: x & ((1 << 36) - 1),
            md5: d[18..34].try_into().unwrap_or([0; 16]),
        };
        if si.bitov < 4 || si.frekvencia == 0 && si.vzoriek != 0 {
            return Err(chyba("neplatné STREAMINFO"));
        }
        Ok(si)
    }

    /// Z konfigurácie kodeku: „fLaC“ + bloky metadát (Matroska `CodecPrivate`, `dfLa` v MP4 bez „fLaC“),
    /// alebo len telo STREAMINFO.
    pub fn z_konfiguracie(k: &[u8]) -> Vysledok<StreamInfo> {
        let mut d = k.strip_prefix(b"fLaC").unwrap_or(k);
        if d.len() == 34 {
            return StreamInfo::z_tela(d);
        }
        while d.len() >= 4 {
            let typ = d[0] & 0x7F;
            let n = u32::from_be_bytes([0, d[1], d[2], d[3]]) as usize;
            let telo = d.get(4..4 + n).ok_or_else(|| chyba("skrátený blok metadát"))?;
            if typ == 0 {
                return StreamInfo::z_tela(telo);
            }
            if d[0] & 0x80 != 0 {
                break;
            }
            d = &d[4 + n..];
        }
        Err(chyba("konfigurácia bez STREAMINFO"))
    }
}

/// Rozložená hlavička rámca (RFC 9639, kap. 9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlavickaRamca {
    /// Premenlivá veľkosť bloku – `cislo` je číslo prvej vzorky, inak číslo rámca.
    pub premenlivy: bool,
    pub blok: u32,
    pub frekvencia: u32,
    /// Priradenie kanálov: 0–7 nezávislé (počet − 1), 8 ľavý/bočný, 9 bočný/pravý, 10 stredný/bočný.
    pub priradenie: u8,
    pub kanalov: u8,
    pub bitov: u8,
    pub cislo: u64,
    /// Dĺžka hlavičky vrátane CRC-8.
    pub dlzka: usize,
}

/// Rozbor hlavičky rámca zo začiatku `d` vrátane kontroly CRC-8. `None` = nie je to platná hlavička.
pub fn hlavicka_ramca(d: &[u8], si: Option<&StreamInfo>) -> Option<HlavickaRamca> {
    if d.len() < 6 || d[0] != 0xFF || d[1] & 0xFE != 0xF8 {
        return None;
    }
    let premenlivy = d[1] & 1 != 0;
    let (kod_bloku, kod_frekv) = (d[2] >> 4, d[2] & 15);
    let (priradenie, kod_bitov) = (d[3] >> 4, (d[3] >> 1) & 7);
    if kod_bloku == 0 || kod_frekv == 15 || priradenie > 10 || kod_bitov == 3 || d[3] & 1 != 0 {
        return None;
    }
    // číslo rámca alebo vzorky v kódovaní podobnom UTF-8 (najviac 7 B, 36 bitov)
    let a = d[4];
    let n = a.leading_ones() as usize;
    if n == 1 || n > 7 {
        return None;
    }
    let mut cislo = if n == 0 { a as u64 } else { (a & (0x7F >> n)) as u64 };
    let mut p = 5;
    for _ in 1..n.max(1) {
        let b = *d.get(p)?;
        if b & 0xC0 != 0x80 {
            return None;
        }
        cislo = (cislo << 6) | (b & 0x3F) as u64;
        p += 1;
    }
    if (!premenlivy && n > 6) || cislo >= 1 << 36 {
        return None;
    }
    let mut citaj = |k: usize| -> Option<u32> {
        let v = d.get(p..p + k)?.iter().fold(0u32, |x, &b| (x << 8) | b as u32);
        p += k;
        Some(v)
    };
    let blok = match kod_bloku {
        1 => 192,
        2..=5 => 576 << (kod_bloku - 2),
        6 => citaj(1)? + 1,
        7 => citaj(2)? + 1,
        _ => 256 << (kod_bloku - 8),
    };
    let frekvencia = match kod_frekv {
        0 => si?.frekvencia,
        1 => 88_200,
        2 => 176_400,
        3 => 192_000,
        4 => 8_000,
        5 => 16_000,
        6 => 22_050,
        7 => 24_000,
        8 => 32_000,
        9 => 44_100,
        10 => 48_000,
        11 => 96_000,
        12 => citaj(1)? * 1000,
        13 => citaj(2)?,
        _ => citaj(2)? * 10,
    };
    let bitov = match kod_bitov {
        0 => si?.bitov,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        _ => 32,
    };
    let crc = *d.get(p)?;
    if crc8(&d[..p]) != crc || blok > 65_535 {
        return None;
    }
    let kanalov = if priradenie < 8 { priradenie + 1 } else { 2 };
    Some(HlavickaRamca { premenlivy, blok, frekvencia, priradenie, kanalov, bitov, cislo, dlzka: p + 1 })
}

/// Čítač bitov od najvyššieho (RFC 9639 – všetky polia sú big-endian).
struct Bity<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Bity<'a> {
    fn zvysok(&self) -> usize {
        self.d.len() * 8 - self.p
    }

    /// 64 bitov od aktuálnej pozície (za koncom nuly); platných je aspoň 57.
    #[inline]
    fn nahlad(&self) -> u64 {
        let b = self.p >> 3;
        let mut w = [0u8; 8];
        let n = self.d.len().saturating_sub(b).min(8);
        w[..n].copy_from_slice(&self.d[b..b + n]);
        u64::from_be_bytes(w) << (self.p & 7)
    }

    #[inline]
    fn u(&mut self, n: u32) -> Vysledok<u64> {
        if n == 0 {
            return Ok(0);
        }
        if n as usize > self.zvysok() {
            return Err(chyba("rámec je skrátený"));
        }
        let v = if n <= 56 {
            self.nahlad() >> (64 - n)
        } else {
            let h = self.nahlad() >> 32;
            self.p += 32;
            let l = self.nahlad() >> (64 - (n - 32));
            self.p -= 32;
            (h << (n - 32)) | l
        };
        self.p += n as usize;
        Ok(v)
    }

    #[inline]
    fn s(&mut self, n: u32) -> Vysledok<i64> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.u(n)?;
        Ok(((v << (64 - n)) as i64) >> (64 - n))
    }

    /// Počet núl pred jednotkou (unárny kód).
    #[inline]
    fn unarny(&mut self) -> Vysledok<u64> {
        let mut q = 0u64;
        loop {
            let z = self.nahlad().leading_zeros() as usize;
            if z < 57 {
                if z + 1 > self.zvysok() {
                    return Err(chyba("rámec je skrátený"));
                }
                self.p += z + 1;
                return Ok(q + z as u64);
            }
            if 56 > self.zvysok() {
                return Err(chyba("rámec je skrátený"));
            }
            self.p += 56;
            q += 56;
        }
    }

    fn zarovnaj(&mut self) {
        self.p = (self.p + 7) & !7;
    }
}

/// Rozloženie kanálov FLAC podľa počtu (RFC 9639, kap. 9.1.3).
pub fn rozlozenie(kanalov: u8) -> RozlozenieKanalov {
    use RozlozenieKanalov as R;
    let m = match kanalov {
        1 => R::PREDNY_STRED,
        2 => R::STEREO.0,
        3 => R::STEREO.0 | R::PREDNY_STRED,
        4 => R::STEREO.0 | R::ZADNY_LAVY | R::ZADNY_PRAVY,
        5 => R::STEREO.0 | R::PREDNY_STRED | R::ZADNY_LAVY | R::ZADNY_PRAVY,
        6 => R::S5_1_ZADNE.0,
        7 => R::STEREO.0 | R::PREDNY_STRED | R::BASY | R::ZADNY_STRED | R::BOCNY_LAVY | R::BOCNY_PRAVY,
        8 => R::S7_1.0,
        _ => 0,
    };
    RozlozenieKanalov(m)
}

/// Dekodér FLAC.
#[derive(Debug)]
pub struct FlacDekoder {
    si: Option<StreamInfo>,
    parametre: ParametreZvuku,
    /// Pracovné kanály (i64 – bočný kanál 32-bitového zvuku má 33 bitov).
    kanaly: Vec<Vec<i64>>,
}

impl FlacDekoder {
    /// Z konfigurácie kodeku (STREAMINFO); prázdna konfigurácia = parametre len z hlavičiek rámcov.
    pub fn novy(konfiguracia: &[u8], parametre: Option<ParametreZvuku>) -> Vysledok<FlacDekoder> {
        let si = if konfiguracia.is_empty() { None } else { Some(StreamInfo::z_konfiguracie(konfiguracia)?) };
        let parametre = match (si, parametre) {
            (Some(s), _) => ParametreZvuku {
                frekvencia: s.frekvencia,
                kanalov: s.kanalov,
                rozlozenie: rozlozenie(s.kanalov),
                bitov: Some(s.bitov),
            },
            (None, Some(p)) => p,
            (None, None) => {
                ParametreZvuku { frekvencia: 0, kanalov: 0, rozlozenie: RozlozenieKanalov(0), bitov: None }
            }
        };
        Ok(FlacDekoder { si, parametre, kanaly: Vec::new() })
    }

    pub fn streaminfo(&self) -> Option<&StreamInfo> {
        self.si.as_ref()
    }

    /// Dekóduje jeden celý rámec (vrátane kontroly CRC-16) do `vystup`.
    pub fn dekoduj_ramec(&mut self, d: &[u8], vystup: &mut BlokZvuku) -> Vysledok<HlavickaRamca> {
        let h = hlavicka_ramca(d, self.si.as_ref()).ok_or_else(|| chyba("neplatná hlavička rámca"))?;
        if d.len() < h.dlzka + 2 {
            return Err(chyba("rámec je skrátený"));
        }
        let telo = &d[..d.len() - 2];
        if crc16(d) != 0 {
            return Err(chyba("CRC-16 rámca nesedí"));
        }
        let n = h.blok as usize;
        let k = h.kanalov as usize;
        self.kanaly.resize_with(k, Vec::new);
        let mut b = Bity { d: telo, p: h.dlzka * 8 };
        for c in 0..k {
            // bočný kanál má o bit viac
            let bocny = matches!((h.priradenie, c), (8, 1) | (9, 0) | (10, 1));
            let bitov = h.bitov as u32 + bocny as u32;
            let ciel = &mut self.kanaly[c];
            ciel.clear();
            ciel.resize(n, 0);
            podramec(&mut b, bitov, ciel)?;
        }
        b.zarovnaj();
        if b.p != telo.len() * 8 {
            return Err(chyba("rámec má bajty navyše za podrámcami"));
        }
        // dekorelácia sterea (RFC 9639, kap. 9.1.3)
        if h.priradenie >= 8 {
            let (l, r) = self.kanaly.split_at_mut(1);
            let (a, s) = (&mut l[0], &mut r[0]);
            for i in 0..n {
                let (x, y) = (a[i], s[i]);
                (a[i], s[i]) = match h.priradenie {
                    8 => (x, x.wrapping_sub(y)),
                    9 => (x.wrapping_add(y), y),
                    _ => {
                        let m = (x << 1) | (y & 1);
                        (m.wrapping_add(y) >> 1, m.wrapping_sub(y) >> 1)
                    }
                };
            }
        }
        // výstup: planárne celé čísla v rozsahu bitovej hĺbky
        let (min, max) = (-(1i64 << (h.bitov - 1)), (1i64 << (h.bitov - 1)) - 1);
        let mut data = match std::mem::replace(&mut vystup.vzorky, VzorkyZvuku::F32(Vec::new())) {
            VzorkyZvuku::Cele { data, .. } => data,
            _ => Vec::new(),
        };
        data.clear();
        data.reserve(n * k);
        for kanal in &self.kanaly[..k] {
            for &v in kanal {
                if v < min || v > max {
                    return Err(chyba("vzorka mimo bitovej hĺbky"));
                }
                data.push(v as i32);
            }
        }
        vystup.frekvencia = h.frekvencia;
        vystup.rozlozenie = rozlozenie(h.kanalov);
        vystup.ramcov = n;
        vystup.vzorky = VzorkyZvuku::Cele { bitov: h.bitov, data };
        Ok(h)
    }
}

/// Dekóduje podrámec jedného kanála (RFC 9639, kap. 9.2).
fn podramec(b: &mut Bity, bitov: u32, v: &mut [i64]) -> Vysledok<()> {
    if b.u(1)? != 0 {
        return Err(chyba("podrámec – nenulový úvodný bit"));
    }
    let typ = b.u(6)? as u32;
    let zbytocne = if b.u(1)? == 1 { b.unarny()? + 1 } else { 0 };
    if zbytocne >= bitov as u64 {
        return Err(chyba("zbytočných bitov je viac ako bitov vzorky"));
    }
    let bitov = bitov - zbytocne as u32;
    let n = v.len();
    match typ {
        0 => {
            let x = b.s(bitov)?;
            v.fill(x);
        }
        1 => {
            for x in v.iter_mut() {
                *x = b.s(bitov)?;
            }
        }
        8..=12 => {
            let rad = (typ - 8) as usize;
            if rad > n {
                return Err(chyba("rád predpovedača je väčší ako blok"));
            }
            for x in &mut v[..rad] {
                *x = b.s(bitov)?;
            }
            zvysky(b, v, rad)?;
            for i in rad..n {
                let p = match rad {
                    0 => 0,
                    1 => v[i - 1],
                    2 => (2i64.wrapping_mul(v[i - 1])).wrapping_sub(v[i - 2]),
                    3 => 3i64.wrapping_mul(v[i - 1].wrapping_sub(v[i - 2])).wrapping_add(v[i - 3]),
                    _ => 4i64
                        .wrapping_mul(v[i - 1].wrapping_add(v[i - 3]))
                        .wrapping_sub(6i64.wrapping_mul(v[i - 2]))
                        .wrapping_sub(v[i - 4]),
                };
                v[i] = v[i].wrapping_add(p);
            }
        }
        32..=63 => {
            let rad = (typ - 31) as usize;
            if rad > n {
                return Err(chyba("rád predikcie je väčší ako blok"));
            }
            for x in &mut v[..rad] {
                *x = b.s(bitov)?;
            }
            let presnost = b.u(4)? as u32 + 1;
            if presnost == 16 {
                return Err(chyba("neplatná presnosť koeficientov"));
            }
            let posun = b.s(5)?;
            if posun < 0 {
                return Err(chyba("záporný posun koeficientov"));
            }
            let mut koef = [0i64; 32];
            for c in &mut koef[..rad] {
                *c = b.s(presnost)?;
            }
            zvysky(b, v, rad)?;
            for i in rad..n {
                let mut s = 0i64;
                for (j, c) in koef[..rad].iter().enumerate() {
                    s = s.wrapping_add(c.wrapping_mul(v[i - 1 - j]));
                }
                v[i] = v[i].wrapping_add(s >> posun);
            }
        }
        _ => return Err(chyba("vyhradený typ podrámca")),
    }
    if zbytocne > 0 {
        for x in v.iter_mut() {
            *x = x.wrapping_shl(zbytocne as u32);
        }
    }
    Ok(())
}

/// Zvyšky predikcie v kódovaní Rice do `v[rad..]` (RFC 9639, kap. 9.2.7).
fn zvysky(b: &mut Bity, v: &mut [i64], rad: usize) -> Vysledok<()> {
    let sposob = b.u(2)?;
    if sposob > 1 {
        return Err(chyba("vyhradený spôsob kódovania zvyškov"));
    }
    let (bity_par, unik) = if sposob == 0 { (4, 15) } else { (5, 31) };
    let rad_oddielov = b.u(4)? as u32;
    let n = v.len();
    let oddielov = 1usize << rad_oddielov;
    if !n.is_multiple_of(oddielov) || (n >> rad_oddielov) < rad {
        return Err(chyba("neplatný rád oddielov zvyškov"));
    }
    let mut i = rad;
    for o in 0..oddielov {
        let koniec = (o + 1) * (n >> rad_oddielov);
        let k = b.u(bity_par)? as u32;
        if k == unik {
            let bitov = b.u(5)? as u32;
            for x in &mut v[i..koniec] {
                *x = b.s(bitov)?;
            }
        } else {
            for x in &mut v[i..koniec] {
                let q = b.unarny()?;
                if q > (u64::MAX >> 1) >> k {
                    return Err(chyba("príliš veľký zvyšok"));
                }
                let u = (q << k) | b.u(k)?;
                *x = ((u >> 1) as i64) ^ -((u & 1) as i64);
            }
        }
        i = koniec;
    }
    Ok(())
}

impl DekoderZvuku for FlacDekoder {
    fn parametre(&self) -> &ParametreZvuku {
        &self.parametre
    }

    fn dekoduj(&mut self, paket: &Paket, vystup: &mut BlokZvuku) -> Vysledok<()> {
        self.dekoduj_ramec(&paket.data, vystup)?;
        crate::zvuk::orez(vystup, paket.orezat_zaciatok as usize, paket.orezat_koniec as usize);
        Ok(())
    }

    fn predbeh_ns(&self) -> i64 {
        0 // každý rámec je nezávislý
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn crc_podla_normy() {
        // kontrolné hodnoty pre „123456789“: CRC-8 (0x07) = 0xF4, CRC-16/BUYPASS (0x8005) = 0xFEE8
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }

    #[test]
    fn bity_a_rice() {
        let d = [0b1010_0000, 0b0000_0001, 0xFF, 0x00];
        let mut b = Bity { d: &d, p: 0 };
        assert_eq!(b.u(3).unwrap(), 0b101);
        assert_eq!(b.unarny().unwrap(), 12);
        assert_eq!(b.s(4).unwrap(), -1);
        assert_eq!(b.u(12).unwrap(), 0xF00);
        assert!(b.u(1).is_err());
        let z = [0u8; 20];
        let mut b = Bity { d: &z, p: 0 };
        assert!(b.unarny().is_err());
    }

    #[test]
    fn hlavicka_s_crc() {
        // rámec s pevným blokom 4096 (kód 12), 44,1 kHz (9), stereo nezávislé (1), 16 bitov (4), číslo 0
        let mut h = vec![0xFF, 0xF8, 0xC9, 0x18, 0x00];
        h.push(crc8(&h));
        let r = hlavicka_ramca(&h, None).unwrap();
        assert_eq!((r.blok, r.frekvencia, r.kanalov, r.bitov, r.cislo, r.dlzka), (4096, 44_100, 2, 16, 0, 6));
        let mut zla = h.clone();
        zla[5] ^= 1;
        assert!(hlavicka_ramca(&zla, None).is_none());
        // číslo vzorky v 7 bajtoch (premenlivý blok), blok z 16 bitov na konci
        let mut h = vec![0xFF, 0xF9, 0x79, 0x18, 0xFE, 0x83, 0xBF, 0xBF, 0xBF, 0xBF, 0xBF, 0x01, 0x00];
        h.push(crc8(&h));
        let r = hlavicka_ramca(&h, None).unwrap();
        assert_eq!((r.premenlivy, r.cislo, r.blok), (true, (3 << 30) | ((1 << 30) - 1), 257));
        assert!(hlavicka_ramca(&[0xFF, 0xF8, 0x09, 0x18, 0, 0], None).is_none()); // blok 0 vyhradený
    }
}
