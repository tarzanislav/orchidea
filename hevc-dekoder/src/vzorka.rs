//! Operácie, ktoré závisia od typu vzorky (`u8` pre 8 bitov, `u16` pre 9–10 bitov): odtlačky obrazu (D.3.19)
//! a zápis do bajtov. Hĺbka roviny môže byť pri `u16` aj 8 (rôzna hĺbka jasu a farby) – vtedy 1 bajt na vzorku.

use orchidea_hevc::odtlacok::{Md5, crc_roviny, kontrolny_sucet_roviny, md5_roviny};

/// Typ vzorky dekodéra.
pub trait TypVzorky: Copy + Send + Sync + 'static {
    /// MD5 roviny (riadky po `krok`), vzorky po 1 bajte pri hĺbke ≤ 8, inak 2 bajty v poradí malého konca.
    fn md5(d: &[Self], w: usize, h: usize, krok: usize, hlbka: u32) -> [u8; 16];
    /// CRC roviny podľa D.3.19 (pri hĺbke > 8 najprv nižší, potom vyšší bajt vzorky).
    fn crc(d: &[Self], w: usize, h: usize, krok: usize, hlbka: u32) -> u16;
    /// Kontrolný súčet roviny podľa D.3.19.
    fn sucet(d: &[Self], w: usize, h: usize, krok: usize, hlbka: u32) -> u32;
    /// Pridá vzorky ako bajty (1 bajt pre `u8`, 2 bajty – malý koniec – pre `u16`, ako yuv420p10le).
    fn pridaj_bajty(ciel: &mut Vec<u8>, d: &[Self]);
}

impl TypVzorky for u8 {
    fn md5(d: &[u8], w: usize, h: usize, krok: usize, _hlbka: u32) -> [u8; 16] {
        md5_roviny(d, w, h, krok)
    }
    fn crc(d: &[u8], w: usize, h: usize, krok: usize, _hlbka: u32) -> u16 {
        crc_roviny(d, w, h, krok)
    }
    fn sucet(d: &[u8], w: usize, h: usize, krok: usize, _hlbka: u32) -> u32 {
        kontrolny_sucet_roviny(d, w, h, krok)
    }
    fn pridaj_bajty(ciel: &mut Vec<u8>, d: &[u8]) {
        ciel.extend_from_slice(d);
    }
}

impl TypVzorky for u16 {
    fn md5(d: &[u16], w: usize, h: usize, krok: usize, hlbka: u32) -> [u8; 16] {
        let mut m = Md5::novy();
        let mut riadok = Vec::with_capacity(2 * w);
        for y in 0..h {
            riadok.clear();
            let r = &d[y * krok..y * krok + w];
            if hlbka > 8 {
                r.iter().for_each(|&v| riadok.extend_from_slice(&v.to_le_bytes()));
            } else {
                r.iter().for_each(|&v| riadok.push(v as u8));
            }
            m.pridaj(&riadok);
        }
        m.vysledok()
    }
    fn crc(d: &[u16], w: usize, h: usize, krok: usize, hlbka: u32) -> u16 {
        let mut crc: u32 = 0xffff;
        let bajt = |crc: &mut u32, b: u32| {
            for i in 0..8 {
                let msb = (*crc >> 15) & 1;
                let bit = (b >> (7 - i)) & 1;
                *crc = (((*crc << 1) + bit) & 0xffff) ^ (msb * 0x1021);
            }
        };
        for y in 0..h {
            for &v in &d[y * krok..y * krok + w] {
                bajt(&mut crc, u32::from(v) & 0xff);
                if hlbka > 8 {
                    bajt(&mut crc, u32::from(v) >> 8);
                }
            }
        }
        for _ in 0..16 {
            let msb = (crc >> 15) & 1;
            crc = ((crc << 1) & 0xffff) ^ (msb * 0x1021);
        }
        crc as u16
    }
    fn sucet(d: &[u16], w: usize, h: usize, krok: usize, hlbka: u32) -> u32 {
        let mut s: u32 = 0;
        for y in 0..h {
            for (x, &v) in d[y * krok..y * krok + w].iter().enumerate() {
                let maska = ((x & 0xff) ^ (y & 0xff) ^ (x >> 8) ^ (y >> 8)) as u32;
                s = s.wrapping_add((u32::from(v) & 0xff) ^ maska);
                if hlbka > 8 {
                    s = s.wrapping_add((u32::from(v) >> 8) ^ maska);
                }
            }
        }
        s
    }
    fn pridaj_bajty(ciel: &mut Vec<u8>, d: &[u16]) {
        d.iter().for_each(|&v| ciel.extend_from_slice(&v.to_le_bytes()));
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    /// 16-bitové odtlačky pri hĺbke 8 sa rovnajú 8-bitovým (rovnaké bajty).
    #[test]
    fn odtlacky_16_pri_hlbke_8_ako_8_bitove() {
        let d8: Vec<u8> = (0..12 * 7).map(|i| (i * 37 % 251) as u8).collect();
        let d16: Vec<u16> = d8.iter().map(|&v| u16::from(v)).collect();
        assert_eq!(u16::md5(&d16, 10, 7, 12, 8), u8::md5(&d8, 10, 7, 12, 8));
        assert_eq!(u16::crc(&d16, 10, 7, 12, 8), u8::crc(&d8, 10, 7, 12, 8));
        assert_eq!(u16::sucet(&d16, 10, 7, 12, 8), u8::sucet(&d8, 10, 7, 12, 8));
    }
}
