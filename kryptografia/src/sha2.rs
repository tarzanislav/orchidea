//! Odtlačky SHA-256, SHA-384 a SHA-512 podľa **FIPS 180-4** (kap. 4.1.2, 4.1.3 funkcie, 4.2.2 a 4.2.3 konštanty,
//! 5.1 doplnenie, 5.3 počiatočné hodnoty, 6.2 a 6.4 výpočet; SHA-384 = SHA-512 s inými počiatočnými hodnotami a
//! skrátením – 6.5).

use crate::Odtlacok;

/// Konštanty SHA-256 – prvých 32 bitov zlomkovej časti tretích odmocnín prvých 64 prvočísel (FIPS 180-4 4.2.2).
const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Konštanty SHA-384/512 – 64 bitov zlomkovej časti tretích odmocnín prvých 80 prvočísel (FIPS 180-4 4.2.3).
const K512: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

/// SHA-256.
#[derive(Debug, Clone)]
pub struct Sha256 {
    h: [u32; 8],
    blok: [u8; 64],
    v_bloku: usize,
    dlzka: u64,
}

impl Sha256 {
    fn spracuj(h: &mut [u32; 8], b: &[u8]) {
        let mut w = [0u32; 64];
        for (t, s) in b.chunks_exact(4).enumerate() {
            w[t] = u32::from_be_bytes([s[0], s[1], s[2], s[3]]);
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16].wrapping_add(s0).wrapping_add(w[t - 7]).wrapping_add(s1);
        }
        let [mut a, mut bb, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for t in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K256[t]).wrapping_add(w[t]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & bb) ^ (a & c) ^ (bb & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = bb;
            bb = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, bb, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
}

impl Odtlacok for Sha256 {
    const DLZKA: usize = 32;
    const BLOK: usize = 64;

    fn novy() -> Sha256 {
        Sha256 {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            blok: [0; 64],
            v_bloku: 0,
            dlzka: 0,
        }
    }

    fn pridaj(&mut self, mut d: &[u8]) {
        self.dlzka = self.dlzka.wrapping_add(d.len() as u64);
        if self.v_bloku > 0 {
            let n = (64 - self.v_bloku).min(d.len());
            self.blok[self.v_bloku..self.v_bloku + n].copy_from_slice(&d[..n]);
            self.v_bloku += n;
            d = &d[n..];
            if self.v_bloku < 64 {
                return;
            }
            let b = self.blok;
            Self::spracuj(&mut self.h, &b);
            self.v_bloku = 0;
        }
        let mut kusy = d.chunks_exact(64);
        for b in &mut kusy {
            Self::spracuj(&mut self.h, b);
        }
        let z = kusy.remainder();
        self.blok[..z.len()].copy_from_slice(z);
        self.v_bloku = z.len();
    }

    fn hotovo(mut self) -> Vec<u8> {
        let bity = self.dlzka.wrapping_mul(8);
        self.pridaj(&[0x80]);
        while self.v_bloku != 56 {
            self.pridaj(&[0]);
        }
        self.pridaj(&bity.to_be_bytes());
        self.h.iter().flat_map(|x| x.to_be_bytes()).collect()
    }
}

/// Spoločné jadro SHA-512 a SHA-384.
#[derive(Debug, Clone)]
struct Jadro512 {
    h: [u64; 8],
    blok: [u8; 128],
    v_bloku: usize,
    dlzka: u128,
}

impl Jadro512 {
    fn nove(h: [u64; 8]) -> Jadro512 {
        Jadro512 { h, blok: [0; 128], v_bloku: 0, dlzka: 0 }
    }

    fn spracuj(h: &mut [u64; 8], b: &[u8]) {
        let mut w = [0u64; 80];
        for (t, s) in b.chunks_exact(8).enumerate() {
            w[t] = u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]);
        }
        for t in 16..80 {
            let s0 = w[t - 15].rotate_right(1) ^ w[t - 15].rotate_right(8) ^ (w[t - 15] >> 7);
            let s1 = w[t - 2].rotate_right(19) ^ w[t - 2].rotate_right(61) ^ (w[t - 2] >> 6);
            w[t] = w[t - 16].wrapping_add(s0).wrapping_add(w[t - 7]).wrapping_add(s1);
        }
        let [mut a, mut bb, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for t in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K512[t]).wrapping_add(w[t]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & bb) ^ (a & c) ^ (bb & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = bb;
            bb = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, bb, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }

    fn pridaj(&mut self, mut d: &[u8]) {
        self.dlzka = self.dlzka.wrapping_add(d.len() as u128);
        if self.v_bloku > 0 {
            let n = (128 - self.v_bloku).min(d.len());
            self.blok[self.v_bloku..self.v_bloku + n].copy_from_slice(&d[..n]);
            self.v_bloku += n;
            d = &d[n..];
            if self.v_bloku < 128 {
                return;
            }
            let b = self.blok;
            Self::spracuj(&mut self.h, &b);
            self.v_bloku = 0;
        }
        let mut kusy = d.chunks_exact(128);
        for b in &mut kusy {
            Self::spracuj(&mut self.h, b);
        }
        let z = kusy.remainder();
        self.blok[..z.len()].copy_from_slice(z);
        self.v_bloku = z.len();
    }

    fn hotovo(mut self, dlzka: usize) -> Vec<u8> {
        let bity = self.dlzka.wrapping_mul(8);
        self.pridaj(&[0x80]);
        while self.v_bloku != 112 {
            self.pridaj(&[0]);
        }
        self.pridaj(&bity.to_be_bytes());
        let mut v: Vec<u8> = self.h.iter().flat_map(|x| x.to_be_bytes()).collect();
        v.truncate(dlzka);
        v
    }
}

/// SHA-512.
#[derive(Debug, Clone)]
pub struct Sha512(Jadro512);

impl Odtlacok for Sha512 {
    const DLZKA: usize = 64;
    const BLOK: usize = 128;

    fn novy() -> Sha512 {
        Sha512(Jadro512::nove([
            0x6a09e667f3bcc908,
            0xbb67ae8584caa73b,
            0x3c6ef372fe94f82b,
            0xa54ff53a5f1d36f1,
            0x510e527fade682d1,
            0x9b05688c2b3e6c1f,
            0x1f83d9abfb41bd6b,
            0x5be0cd19137e2179,
        ]))
    }

    fn pridaj(&mut self, d: &[u8]) {
        self.0.pridaj(d);
    }

    fn hotovo(self) -> Vec<u8> {
        self.0.hotovo(64)
    }
}

/// SHA-384.
#[derive(Debug, Clone)]
pub struct Sha384(Jadro512);

impl Odtlacok for Sha384 {
    const DLZKA: usize = 48;
    const BLOK: usize = 128;

    fn novy() -> Sha384 {
        Sha384(Jadro512::nove([
            0xcbbb9d5dc1059ed8,
            0x629a292a367cd507,
            0x9159015a3070dd17,
            0x152fecd8f70e5939,
            0x67332667ffc00b31,
            0x8eb44a8768581511,
            0xdb0c2e0d64f98fa7,
            0x47b5481dbefa4fa4,
        ]))
    }

    fn pridaj(&mut self, d: &[u8]) {
        self.0.pridaj(d);
    }

    fn hotovo(self) -> Vec<u8> {
        self.0.hotovo(48)
    }
}

/// Bajty z šestnástkového zápisu (pre skúšky a vektory).
pub fn z_hex(t: &str) -> Vec<u8> {
    let t: Vec<u8> = t.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    t.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap_or("00"), 16).unwrap_or(0)).collect()
}

#[cfg(test)]
mod testy {
    use super::*;

    fn hex<H: Odtlacok>(d: &[u8]) -> String {
        H::z(d).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_vektory_fips() {
        // FIPS 180-4, príklady NIST (CSRC – SHA256.pdf): „abc“, 448-bitová správa, prázdna, milión „a“
        assert_eq!(hex::<Sha256>(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            hex::<Sha256>(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(hex::<Sha256>(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(
            hex::<Sha256>(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn sha512_a_sha384_vektory_fips() {
        assert_eq!(
            hex::<Sha512>(b"abc"),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd\
             454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hex::<Sha384>(b"abc"),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
        let m = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";
        assert_eq!(
            hex::<Sha512>(m),
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433a\
             c7d329eeb6dd26545e96e55b874be909"
        );
        assert_eq!(
            hex::<Sha384>(m),
            "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039"
        );
        assert_eq!(
            hex::<Sha384>(b""),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
        );
    }

    #[test]
    fn po_kuskoch_rovnako_ako_naraz() {
        let d: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        for kus in [1, 3, 55, 56, 63, 64, 65, 111, 112, 127, 128, 129, 300] {
            let mut a = Sha256::novy();
            let mut b = Sha384::novy();
            for c in d.chunks(kus) {
                a.pridaj(c);
                b.pridaj(c);
            }
            assert_eq!(a.hotovo(), Sha256::z(&d), "kus {kus}");
            assert_eq!(b.hotovo(), Sha384::z(&d), "kus {kus}");
        }
        // hranice doplnenia: 55, 56, 64 B (SHA-256) a 111, 112, 128 B (SHA-512)
        for n in [55usize, 56, 64, 111, 112, 128] {
            let d = vec![0x61u8; n];
            let mut h = Sha512::novy();
            h.pridaj(&d[..n / 2]);
            let k = h.clone();
            h.pridaj(&d[n / 2..]);
            let mut k2 = k;
            k2.pridaj(&d[n / 2..]);
            assert_eq!(h.hotovo(), k2.hotovo(), "klon pokračuje rovnako");
        }
    }
}
