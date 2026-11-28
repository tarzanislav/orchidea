//! Bloková šifra AES-128 a AES-256 podľa **FIPS 197** a autentizované šifrovanie GCM podľa **NIST SP 800-38D**.
//!
//! **Konštantný čas:** žiadna tabuľka indexovaná tajným bajtom (útok cez vyrovnávaciu pamäť procesora). Substitučný
//! box sa počíta: inverzia v GF(2⁸) umocnením na 254 (FIPS 197 5.1.1 – x⁻¹ = x²⁵⁴, 0 ↦ 0) a afinná transformácia;
//! násobenie v GF(2⁸) aj GF(2¹²⁸) (GHASH) je bez vetvenia podľa údajov – len masky. Je to pomalšie ako tabuľky,
//! preto TLS ponúka na doskách bez inštrukcií AES (RPi 3, 4) najprv ChaCha20-Poly1305; inštrukcie ARMv8 (RPi 5,
//! Amlogic S905X3) prídu ako zrýchlenie s rovnakými skúškami.

use crate::rovnake;

/// Násobenie v GF(2⁸) s polynómom x⁸ + x⁴ + x³ + x + 1 (FIPS 197 4.2) – bez vetvenia.
fn gf_nasob(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0u8;
    for _ in 0..8 {
        r ^= a & 0u8.wrapping_sub(b & 1);
        let hore = 0u8.wrapping_sub(a >> 7);
        a = (a << 1) ^ (0x1b & hore);
        b >>= 1;
    }
    r
}

/// Substitučný box: x⁻¹ (x²⁵⁴) a afinná transformácia s konštantou 0x63 (FIPS 197 5.1.1).
fn sbox(x: u8) -> u8 {
    // x^254 = x^(2+4+8+16+32+64+128): x2, x4, … a súčin
    let x2 = gf_nasob(x, x);
    let x3 = gf_nasob(x2, x);
    let x6 = gf_nasob(x3, x3);
    let x12 = gf_nasob(x6, x6);
    let x15 = gf_nasob(x12, x3);
    let x30 = gf_nasob(x15, x15);
    let x60 = gf_nasob(x30, x30);
    let x120 = gf_nasob(x60, x60);
    let x126 = gf_nasob(x120, x6);
    let x127 = gf_nasob(x126, x);
    let inv = gf_nasob(x127, x127); // x^254
    inv ^ inv.rotate_left(1) ^ inv.rotate_left(2) ^ inv.rotate_left(3) ^ inv.rotate_left(4) ^ 0x63
}

/// Rozvrh kľúčov AES (FIPS 197 5.2) – 11 alebo 15 kľúčov kôl.
#[derive(Clone)]
pub struct Aes {
    kola: Vec<[u8; 16]>,
}

impl std::fmt::Debug for Aes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Aes({} kôl, kľúč skrytý)", self.kola.len() - 1)
    }
}

impl Aes {
    /// Kľúč 16 B (AES-128) alebo 32 B (AES-256); iná dĺžka = `None`.
    pub fn novy(kluc: &[u8]) -> Option<Aes> {
        let nk = match kluc.len() {
            16 => 4,
            32 => 8,
            _ => return None,
        };
        let nr = nk + 6;
        let mut w: Vec<[u8; 4]> = kluc.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        let mut rcon = 1u8;
        for i in nk..4 * (nr + 1) {
            let mut t = w[i - 1];
            if i % nk == 0 {
                t = [sbox(t[1]) ^ rcon, sbox(t[2]), sbox(t[3]), sbox(t[0])];
                rcon = gf_nasob(rcon, 2);
            } else if nk > 6 && i % nk == 4 {
                t = [sbox(t[0]), sbox(t[1]), sbox(t[2]), sbox(t[3])];
            }
            let p = w[i - nk];
            w.push([p[0] ^ t[0], p[1] ^ t[1], p[2] ^ t[2], p[3] ^ t[3]]);
        }
        let kola = w
            .chunks_exact(4)
            .map(|c| {
                let mut k = [0u8; 16];
                for (j, s) in c.iter().enumerate() {
                    k[4 * j..4 * j + 4].copy_from_slice(s);
                }
                k
            })
            .collect();
        Some(Aes { kola })
    }

    /// Zašifruje jeden blok (FIPS 197 5.1): stav po stĺpcoch, SubBytes, ShiftRows, MixColumns, AddRoundKey.
    pub fn blok(&self, b: &mut [u8; 16]) {
        let nr = self.kola.len() - 1;
        for (x, k) in b.iter_mut().zip(&self.kola[0]) {
            *x ^= k;
        }
        for kolo in 1..=nr {
            for x in b.iter_mut() {
                *x = sbox(*x);
            }
            // ShiftRows: riadok r (bajty r, r+4, r+8, r+12) posunutý o r doľava
            let s = *b;
            for c in 0..4 {
                for r in 0..4 {
                    b[4 * c + r] = s[4 * ((c + r) % 4) + r];
                }
            }
            if kolo != nr {
                for c in 0..4 {
                    let a = [b[4 * c], b[4 * c + 1], b[4 * c + 2], b[4 * c + 3]];
                    let d = |x: u8| gf_nasob(x, 2);
                    b[4 * c] = d(a[0]) ^ d(a[1]) ^ a[1] ^ a[2] ^ a[3];
                    b[4 * c + 1] = a[0] ^ d(a[1]) ^ d(a[2]) ^ a[2] ^ a[3];
                    b[4 * c + 2] = a[0] ^ a[1] ^ d(a[2]) ^ d(a[3]) ^ a[3];
                    b[4 * c + 3] = d(a[0]) ^ a[0] ^ a[1] ^ a[2] ^ d(a[3]);
                }
            }
            for (x, k) in b.iter_mut().zip(&self.kola[kolo]) {
                *x ^= k;
            }
        }
    }
}

/// Násobenie v GF(2¹²⁸) pre GHASH (NIST SP 800-38D 6.3, algoritmus 1) – bez vetvenia podľa údajov.
fn gf128_nasob(x: u128, y: u128) -> u128 {
    const R: u128 = 0xe1 << 120;
    let mut z = 0u128;
    let mut v = y;
    for i in 0..128 {
        let bit = (x >> (127 - i)) & 1;
        z ^= v & 0u128.wrapping_sub(bit);
        let lsb = v & 1;
        v = (v >> 1) ^ (R & 0u128.wrapping_sub(lsb));
    }
    z
}

/// Autentizované šifrovanie AES-GCM s 12-bajtovým nonce a 16-bajtovou značkou (TLS 1.3).
#[derive(Debug, Clone)]
pub struct AesGcm {
    aes: Aes,
    h: u128,
}

/// Značka nesedí – údaje boli zmenené alebo zlý kľúč.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZlaZnacka;

impl AesGcm {
    pub fn novy(kluc: &[u8]) -> Option<AesGcm> {
        let aes = Aes::novy(kluc)?;
        let mut h = [0u8; 16];
        aes.blok(&mut h);
        Some(AesGcm { aes, h: u128::from_be_bytes(h) })
    }

    fn ghash(&self, aad: &[u8], sifra: &[u8]) -> u128 {
        let mut y = 0u128;
        for cast in [aad, sifra] {
            for kus in cast.chunks(16) {
                let mut b = [0u8; 16];
                b[..kus.len()].copy_from_slice(kus);
                y = gf128_nasob(y ^ u128::from_be_bytes(b), self.h);
            }
        }
        let dlzky = ((aad.len() as u128 * 8) << 64) | (sifra.len() as u128 * 8);
        gf128_nasob(y ^ dlzky, self.h)
    }

    /// CTR so začiatkom J0 + 1 (J0 = nonce ‖ 0x00000001); `od` = prvé počítadlo.
    fn ctr(&self, nonce: &[u8; 12], od: u32, data: &mut [u8]) {
        let mut pocitadlo = od;
        for kus in data.chunks_mut(16) {
            let mut b = [0u8; 16];
            b[..12].copy_from_slice(nonce);
            b[12..].copy_from_slice(&pocitadlo.to_be_bytes());
            self.aes.blok(&mut b);
            for (x, k) in kus.iter_mut().zip(b) {
                *x ^= k;
            }
            pocitadlo = pocitadlo.wrapping_add(1);
        }
    }

    fn znacka(&self, nonce: &[u8; 12], aad: &[u8], sifra: &[u8]) -> [u8; 16] {
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;
        self.aes.blok(&mut j0);
        (u128::from_be_bytes(j0) ^ self.ghash(aad, sifra)).to_be_bytes()
    }

    /// Zašifruje `data` na mieste; vráti značku.
    pub fn zasifruj(&self, nonce: &[u8; 12], aad: &[u8], data: &mut [u8]) -> [u8; 16] {
        self.ctr(nonce, 2, data);
        self.znacka(nonce, aad, data)
    }

    /// Overí značku (v konštantnom čase) a až potom dešifruje na mieste.
    pub fn desifruj(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        data: &mut [u8],
        znacka: &[u8],
    ) -> Result<(), ZlaZnacka> {
        if !rovnake(&self.znacka(nonce, aad, data), znacka) {
            return Err(ZlaZnacka);
        }
        self.ctr(nonce, 2, data);
        Ok(())
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::sha2::z_hex;

    #[test]
    fn substitucny_box_podla_fips197() {
        // FIPS 197 obr. 7: S(0x00) = 0x63, S(0x53) = 0xed, S(0xff) = 0x16, S(0x01) = 0x7c
        assert_eq!([sbox(0x00), sbox(0x53), sbox(0xff), sbox(0x01)], [0x63, 0xed, 0x16, 0x7c]);
        let mut videne = [false; 256];
        for x in 0..=255u8 {
            videne[sbox(x) as usize] = true;
        }
        assert!(videne.iter().all(|&v| v), "permutácia");
    }

    #[test]
    fn fips197_priloha_c() {
        let pt: [u8; 16] = z_hex("00112233445566778899aabbccddeeff").try_into().unwrap();
        let mut b = pt;
        Aes::novy(&z_hex("000102030405060708090a0b0c0d0e0f")).unwrap().blok(&mut b);
        assert_eq!(b.to_vec(), z_hex("69c4e0d86a7b0430d8cdb78070b4c55a"), "C.1 AES-128");
        let mut b = pt;
        Aes::novy(&z_hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"))
            .unwrap()
            .blok(&mut b);
        assert_eq!(b.to_vec(), z_hex("8ea2b7ca516745bfeafc49904b496089"), "C.3 AES-256");
        // príloha B: kľúč 2b7e…, vstup 3243f6a8…
        let mut b: [u8; 16] = z_hex("3243f6a8885a308d313198a2e0370734").try_into().unwrap();
        Aes::novy(&z_hex("2b7e151628aed2a6abf7158809cf4f3c")).unwrap().blok(&mut b);
        assert_eq!(b.to_vec(), z_hex("3925841d02dc09fbdc118597196a0b32"));
        assert!(Aes::novy(&[0; 24]).is_none(), "AES-192 TLS 1.3 nepotrebuje");
    }

    fn gcm(kluc: &str, iv: &str, pt: &str, aad: &str, ct: &str, tag: &str) {
        let g = AesGcm::novy(&z_hex(kluc)).unwrap();
        let nonce: [u8; 12] = z_hex(iv).try_into().unwrap();
        let mut d = z_hex(pt);
        let t = g.zasifruj(&nonce, &z_hex(aad), &mut d);
        assert_eq!(d, z_hex(ct), "šifrový text");
        assert_eq!(t.to_vec(), z_hex(tag), "značka");
        assert_eq!(g.desifruj(&nonce, &z_hex(aad), &mut d, &t), Ok(()));
        assert_eq!(d, z_hex(pt));
        let mut zmenene = z_hex(ct);
        if let Some(b) = zmenene.first_mut() {
            *b ^= 1;
            assert_eq!(g.desifruj(&nonce, &z_hex(aad), &mut zmenene, &t), Err(ZlaZnacka));
        }
        let mut zla = t;
        zla[15] ^= 0x80;
        assert_eq!(g.desifruj(&nonce, &z_hex(aad), &mut z_hex(ct), &zla), Err(ZlaZnacka));
    }

    #[test]
    fn gcm_vektory_specifikacie() {
        // Testovacie prípady 1–4 a 13–16 zo špecifikácie GCM (McGrew a Viega, príloha B; prevzaté NIST)
        gcm(
            "00000000000000000000000000000000",
            "000000000000000000000000",
            "",
            "",
            "",
            "58e2fccefa7e3061367f1d57a4e7455a",
        );
        gcm(
            "00000000000000000000000000000000",
            "000000000000000000000000",
            "00000000000000000000000000000000",
            "",
            "0388dace60b6a392f328c2b971b2fe78",
            "ab6e47d42cec13bdf53a67b21257bddf",
        );
        gcm(
            "feffe9928665731c6d6a8f9467308308",
            "cafebabefacedbaddecaf888",
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525\
             b16aedf5aa0de657ba637b391aafd255",
            "",
            "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa05\
             1ba30b396a0aac973d58e091473f5985",
            "4d5c2af327cd64a62cf35abd2ba6fab4",
        );
        gcm(
            "feffe9928665731c6d6a8f9467308308",
            "cafebabefacedbaddecaf888",
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525\
             b16aedf5aa0de657ba637b39",
            "feedfacedeadbeeffeedfacedeadbeefabaddad2",
            "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa05\
             1ba30b396a0aac973d58e091",
            "5bc94fbc3221a5db94fae95ae7121a47",
        );
        let k256 = "0000000000000000000000000000000000000000000000000000000000000000";
        gcm(k256, "000000000000000000000000", "", "", "", "530f8afbc74536b9a963b4f1c4cb738b");
        gcm(
            k256,
            "000000000000000000000000",
            "00000000000000000000000000000000",
            "",
            "cea7403d4d606b6e074ec5d3baf39d18",
            "d0d1c8a799996bf0265b98b5d48ab919",
        );
        let k = "feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308";
        gcm(
            k,
            "cafebabefacedbaddecaf888",
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525\
             b16aedf5aa0de657ba637b391aafd255",
            "",
            "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838\
             c5f61e6393ba7a0abcc9f662898015ad",
            "b094dac5d93471bdec1a502270e3cc6c",
        );
        gcm(
            k,
            "cafebabefacedbaddecaf888",
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525\
             b16aedf5aa0de657ba637b39",
            "feedfacedeadbeeffeedfacedeadbeefabaddad2",
            "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838\
             c5f61e6393ba7a0abcc9f662",
            "76fc6ece0f4e1768cddf8853bb2d551b",
        );
    }
}
