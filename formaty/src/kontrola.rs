//! Kontrolné súčty – CRC-32 (PNG, zlib v GZIP, ZIP) a Adler-32 (koniec údajov zlib).
//!
//! CRC-32 sa počíta po ôsmich bajtoch naraz (osem tabuliek, spočítaných pri preklade): na jeden bajt pripadne
//! jedno čítanie tabuľky bez závislosti na predchádzajúcom bajte, procesor tak spracuje viac bajtov súbežne.
//! Adler-32 sčítava po blokoch, ktoré nepretečú, a modulo robí raz za blok.

/// Mnohočlen CRC-32 (IEEE 802.3) v obrátenom poradí bitov.
const MNOHOCLEN: u32 = 0xEDB8_8320;

/// Osem tabuliek: `T[0]` je bežná tabuľka po bajtoch, `T[k][n]` je CRC bajtu `n` posunutého o `k` bajtov.
const T: [[u32; 256]; 8] = {
    let mut t = [[0u32; 256]; 8];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { MNOHOCLEN ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[0][n] = c;
        n += 1;
    }
    let mut k = 1;
    while k < 8 {
        let mut n = 0;
        while n < 256 {
            let p = t[k - 1][n];
            t[k][n] = (p >> 8) ^ t[0][(p & 0xFF) as usize];
            n += 1;
        }
        k += 1;
    }
    t
};

/// Priebežný výpočet CRC-32 (pre údaje po častiach, napríklad typ a obsah kúska PNG).
#[derive(Debug, Clone, Copy)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Crc32::novy()
    }
}

impl Crc32 {
    pub const fn novy() -> Crc32 {
        Crc32(0xFFFF_FFFF)
    }

    pub fn pridaj(&mut self, data: &[u8]) {
        let mut c = self.0;
        let mut kusy = data.chunks_exact(8);
        for k in &mut kusy {
            let a = c ^ u32::from_le_bytes([k[0], k[1], k[2], k[3]]);
            c = T[7][(a & 0xFF) as usize]
                ^ T[6][((a >> 8) & 0xFF) as usize]
                ^ T[5][((a >> 16) & 0xFF) as usize]
                ^ T[4][(a >> 24) as usize]
                ^ T[3][k[4] as usize]
                ^ T[2][k[5] as usize]
                ^ T[1][k[6] as usize]
                ^ T[0][k[7] as usize];
        }
        for &b in kusy.remainder() {
            c = T[0][((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        self.0 = c;
    }

    pub fn hodnota(&self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

/// CRC-32 celých údajov.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::novy();
    c.pridaj(data);
    c.hodnota()
}

/// Najväčší počet bajtov, pri ktorom súčty Adler-32 v 32 bitoch ešte nepretečú (z normy zlib).
const ADLER_BLOK: usize = 5552;
const ADLER_MOD: u32 = 65521;

/// Priebežný výpočet Adler-32.
#[derive(Debug, Clone, Copy)]
pub struct Adler32 {
    a: u32,
    b: u32,
}

impl Default for Adler32 {
    fn default() -> Self {
        Adler32::novy()
    }
}

impl Adler32 {
    pub const fn novy() -> Adler32 {
        Adler32 { a: 1, b: 0 }
    }

    pub fn pridaj(&mut self, data: &[u8]) {
        let (mut a, mut b) = (self.a, self.b);
        for kus in data.chunks(ADLER_BLOK) {
            // štyri bajty naraz: b narastie o 4a + 4x0 + 3x1 + 2x2 + x3
            let mut stvorice = kus.chunks_exact(4);
            for s in &mut stvorice {
                let (x0, x1, x2, x3) = (s[0] as u32, s[1] as u32, s[2] as u32, s[3] as u32);
                b += 4 * a + 4 * x0 + 3 * x1 + 2 * x2 + x3;
                a += x0 + x1 + x2 + x3;
            }
            for &x in stvorice.remainder() {
                a += x as u32;
                b += a;
            }
            a %= ADLER_MOD;
            b %= ADLER_MOD;
        }
        self.a = a;
        self.b = b;
    }

    pub fn hodnota(&self) -> u32 {
        (self.b << 16) | self.a
    }
}

/// Adler-32 celých údajov.
pub fn adler32(data: &[u8]) -> u32 {
    let mut s = Adler32::novy();
    s.pridaj(data);
    s.hodnota()
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn znome_hodnoty() {
        // kontrolné hodnoty z normy (reťazec „123456789“)
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn po_castiach_rovnako_ako_naraz() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let mut c = Crc32::novy();
        let mut a = Adler32::novy();
        for kus in data.chunks(777) {
            c.pridaj(kus);
            a.pridaj(kus);
        }
        assert_eq!(c.hodnota(), crc32(&data));
        assert_eq!(a.hodnota(), adler32(&data));
        // pomalý výpočet po bajtoch ako rozhodca
        let mut r = 0xFFFF_FFFFu32;
        for &b in &data {
            r ^= b as u32;
            for _ in 0..8 {
                r = if r & 1 != 0 { MNOHOCLEN ^ (r >> 1) } else { r >> 1 };
            }
        }
        assert_eq!(crc32(&data), !r);
        let (mut x, mut y) = (1u64, 0u64);
        for &b in &data {
            x = (x + b as u64) % 65521;
            y = (y + x) % 65521;
        }
        assert_eq!(adler32(&data), ((y << 16) | x) as u32);
    }

    #[test]
    fn adler_pri_najvacsich_bajtoch_nepretecie() {
        let data = vec![0xFFu8; 3 * ADLER_BLOK + 13];
        let (mut x, mut y) = (1u64, 0u64);
        for &b in &data {
            x = (x + b as u64) % 65521;
            y = (y + x) % 65521;
        }
        assert_eq!(adler32(&data), ((y << 16) | x) as u32);
    }
}
