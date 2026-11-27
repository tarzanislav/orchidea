//! KORENE – spojenie zariadení SKLENÍKA s počítačom (R77).
//!
//! Prvá časť: **kryptografia vlastným kódom podľa noriem** – nikdy vymyslené šifry:
//! - [`x25519`] – dohoda o kľúči na krivke Curve25519 (RFC 7748),
//! - [`chacha20`], [`Poly1305`] a [`zasifruj`] / [`desifruj`] – šifrovanie s overením pravosti
//!   AEAD_CHACHA20_POLY1305 (RFC 8439).
//!
//! Všetko je overené oficiálnymi testovacími vektormi z noriem. Výpočty s tajnými údajmi nevetvia podľa
//! tajných hodnôt (výmena v Montgomeryho rebríku cez masku, porovnanie značky v konštantnom čase).
//! Beží rovnako na 32- aj 64-bitových procesoroch (od Raspberry Pi 1 po PC).

#![deny(unsafe_code)]

pub mod aktualizacia;
mod pole;
pub mod zber;

pub use pole::x25519;

/// Základný bod krivky Curve25519 (u = 9).
pub const ZAKLADNY_BOD: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 9;
    b
};

/// Verejný kľúč k tajnému kľúču (32 náhodných bajtov).
pub fn verejny_kluc(tajny: &[u8; 32]) -> [u8; 32] {
    x25519(tajny, &ZAKLADNY_BOD)
}

/// Spoločné tajomstvo s druhou stranou. `None`, ak druhá strana poslala bod malého rádu (výsledok samé
/// nuly) – taký kľúč sa nesmie použiť.
pub fn spolocne_tajomstvo(tajny: &[u8; 32], cudzi_verejny: &[u8; 32]) -> Option<[u8; 32]> {
    let k = x25519(tajny, cudzi_verejny);
    let nula = k.iter().fold(0u8, |a, &b| a | b) == 0;
    (!nula).then_some(k)
}

// ---- ChaCha20 (RFC 8439, kap. 2.1–2.4) ----

fn stvrtkolo(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// Jeden 64-bajtový blok prúdu ChaCha20.
pub fn chacha20_blok(kluc: &[u8; 32], pocitadlo: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let slovo = |b: &[u8], i: usize| u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
    let mut s = [0u32; 16];
    s[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    for i in 0..8 {
        s[4 + i] = slovo(kluc, i);
    }
    s[12] = pocitadlo;
    for i in 0..3 {
        s[13 + i] = slovo(nonce, i);
    }
    let mut x = s;
    for _ in 0..10 {
        stvrtkolo(&mut x, 0, 4, 8, 12);
        stvrtkolo(&mut x, 1, 5, 9, 13);
        stvrtkolo(&mut x, 2, 6, 10, 14);
        stvrtkolo(&mut x, 3, 7, 11, 15);
        stvrtkolo(&mut x, 0, 5, 10, 15);
        stvrtkolo(&mut x, 1, 6, 11, 12);
        stvrtkolo(&mut x, 2, 7, 8, 13);
        stvrtkolo(&mut x, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[4 * i..4 * i + 4].copy_from_slice(&x[i].wrapping_add(s[i]).to_le_bytes());
    }
    out
}

/// Zašifruje alebo dešifruje `data` na mieste prúdom ChaCha20 od bloku `pocitadlo`.
pub fn chacha20(kluc: &[u8; 32], pocitadlo: u32, nonce: &[u8; 12], data: &mut [u8]) {
    for (i, kus) in data.chunks_mut(64).enumerate() {
        let blok = chacha20_blok(kluc, pocitadlo.wrapping_add(i as u32), nonce);
        for (d, k) in kus.iter_mut().zip(blok) {
            *d ^= k;
        }
    }
}

// ---- Poly1305 (RFC 8439, kap. 2.5) – 26-bitové časti, rýchle aj na 32-bitovom procesore ----

/// Overenie pravosti Poly1305 (jednorazový 32-bajtový kľúč), postupné pridávanie údajov.
#[derive(Debug, Clone)]
pub struct Poly1305 {
    r: [u64; 5],
    s: [u32; 4],
    h: [u64; 5],
    zvysok: [u8; 16],
    v_zvysku: usize,
}

const M26: u64 = 0x3ff_ffff;

fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl Poly1305 {
    pub fn novy(kluc: &[u8; 32]) -> Poly1305 {
        // r so zakázanými bitmi vynulovanými („clamp“) rozložené na 26-bitové časti
        let r = [
            (le32(kluc, 0) & 0x3ff_ffff) as u64,
            ((le32(kluc, 3) >> 2) & 0x3ff_ff03) as u64,
            ((le32(kluc, 6) >> 4) & 0x3ff_c0ff) as u64,
            ((le32(kluc, 9) >> 6) & 0x3f0_3fff) as u64,
            ((le32(kluc, 12) >> 8) & 0x00f_ffff) as u64,
        ];
        let s = [le32(kluc, 16), le32(kluc, 20), le32(kluc, 24), le32(kluc, 28)];
        Poly1305 { r, s, h: [0; 5], zvysok: [0; 16], v_zvysku: 0 }
    }

    fn blok(&mut self, m: &[u8; 16], horny_bit: u64) {
        let [r0, r1, r2, r3, r4] = self.r;
        let (s1, s2, s3, s4) = (r1 * 5, r2 * 5, r3 * 5, r4 * 5);
        let h = &mut self.h;
        h[0] += le32(m, 0) as u64 & M26;
        h[1] += (le32(m, 3) >> 2) as u64 & M26;
        h[2] += (le32(m, 6) >> 4) as u64 & M26;
        h[3] += (le32(m, 9) >> 6) as u64 & M26;
        h[4] += (le32(m, 12) >> 8) as u64 | horny_bit;
        let d0 = h[0] * r0 + h[1] * s4 + h[2] * s3 + h[3] * s2 + h[4] * s1;
        let mut d1 = h[0] * r1 + h[1] * r0 + h[2] * s4 + h[3] * s3 + h[4] * s2;
        let mut d2 = h[0] * r2 + h[1] * r1 + h[2] * r0 + h[3] * s4 + h[4] * s3;
        let mut d3 = h[0] * r3 + h[1] * r2 + h[2] * r1 + h[3] * r0 + h[4] * s4;
        let mut d4 = h[0] * r4 + h[1] * r3 + h[2] * r2 + h[3] * r1 + h[4] * r0;
        d1 += d0 >> 26;
        h[0] = d0 & M26;
        d2 += d1 >> 26;
        h[1] = d1 & M26;
        d3 += d2 >> 26;
        h[2] = d2 & M26;
        d4 += d3 >> 26;
        h[3] = d3 & M26;
        h[0] += (d4 >> 26) * 5;
        h[4] = d4 & M26;
        h[1] += h[0] >> 26;
        h[0] &= M26;
    }

    /// Pridá ďalšie údaje.
    pub fn pridaj(&mut self, mut data: &[u8]) {
        if self.v_zvysku > 0 {
            let n = (16 - self.v_zvysku).min(data.len());
            self.zvysok[self.v_zvysku..self.v_zvysku + n].copy_from_slice(&data[..n]);
            self.v_zvysku += n;
            data = &data[n..];
            if self.v_zvysku < 16 {
                return;
            }
            let b = self.zvysok;
            self.blok(&b, 1 << 24);
            self.v_zvysku = 0;
        }
        let mut kusy = data.chunks_exact(16);
        for kus in &mut kusy {
            self.blok(kus.try_into().expect("16 bajtov"), 1 << 24);
        }
        let z = kusy.remainder();
        self.zvysok[..z.len()].copy_from_slice(z);
        self.v_zvysku = z.len();
    }

    /// Značka pravosti (16 bajtov).
    pub fn dokonci(mut self) -> [u8; 16] {
        if self.v_zvysku > 0 {
            let mut b = [0u8; 16];
            b[..self.v_zvysku].copy_from_slice(&self.zvysok[..self.v_zvysku]);
            b[self.v_zvysku] = 1;
            self.blok(&b, 0);
        }
        let mut h = self.h;
        // úplné prenesenie
        for i in 1..5 {
            h[i] += h[i - 1] >> 26;
            h[i - 1] &= M26;
        }
        h[0] += (h[4] >> 26) * 5;
        h[4] &= M26;
        h[1] += h[0] >> 26;
        h[0] &= M26;
        // g = h + 5 − 2^130; ak nie je záporné, výsledok je g (výber maskou, bez vetvenia)
        let mut g = [0u64; 5];
        let mut c = 5u64;
        for i in 0..4 {
            g[i] = h[i] + c;
            c = g[i] >> 26;
            g[i] &= M26;
        }
        g[4] = h[4].wrapping_add(c).wrapping_sub(1 << 26);
        let maska = (g[4] >> 63).wrapping_sub(1);
        for i in 0..5 {
            h[i] = (h[i] & !maska) | (g[i] & maska & if i == 4 { M26 } else { u64::MAX });
        }
        let slova = [
            (h[0] | (h[1] << 26)) & 0xffff_ffff,
            ((h[1] >> 6) | (h[2] << 20)) & 0xffff_ffff,
            ((h[2] >> 12) | (h[3] << 14)) & 0xffff_ffff,
            ((h[3] >> 18) | (h[4] << 8)) & 0xffff_ffff,
        ];
        let mut out = [0u8; 16];
        let mut prenos = 0u64;
        for i in 0..4 {
            let f = slova[i] + self.s[i] as u64 + prenos;
            out[4 * i..4 * i + 4].copy_from_slice(&(f as u32).to_le_bytes());
            prenos = f >> 32;
        }
        out
    }
}

/// Poly1305 celej správy naraz.
pub fn poly1305(kluc: &[u8; 32], sprava: &[u8]) -> [u8; 16] {
    let mut p = Poly1305::novy(kluc);
    p.pridaj(sprava);
    p.dokonci()
}

// ---- AEAD_CHACHA20_POLY1305 (RFC 8439, kap. 2.8) ----

fn znacka(kluc: &[u8; 32], nonce: &[u8; 12], doplnok: &[u8], sifra: &[u8]) -> [u8; 16] {
    let jednorazovy: [u8; 32] = chacha20_blok(kluc, 0, nonce)[..32].try_into().expect("32 bajtov");
    let mut p = Poly1305::novy(&jednorazovy);
    let nuly = [0u8; 16];
    p.pridaj(doplnok);
    p.pridaj(&nuly[..(16 - doplnok.len() % 16) % 16]);
    p.pridaj(sifra);
    p.pridaj(&nuly[..(16 - sifra.len() % 16) % 16]);
    p.pridaj(&(doplnok.len() as u64).to_le_bytes());
    p.pridaj(&(sifra.len() as u64).to_le_bytes());
    p.dokonci()
}

/// Zašifruje `data` na mieste a vráti značku pravosti. `doplnok` sa nešifruje, ale je chránený značkou.
/// Rovnaké `nonce` sa s tým istým kľúčom nesmie použiť dvakrát.
pub fn zasifruj(kluc: &[u8; 32], nonce: &[u8; 12], doplnok: &[u8], data: &mut [u8]) -> [u8; 16] {
    chacha20(kluc, 1, nonce, data);
    znacka(kluc, nonce, doplnok, data)
}

/// Overí značku a až potom dešifruje `data` na mieste. Pri neplatnej značke vráti chybu a údaje nechá
/// zašifrované.
pub fn desifruj(
    kluc: &[u8; 32],
    nonce: &[u8; 12],
    doplnok: &[u8],
    data: &mut [u8],
    ocakavana: &[u8; 16],
) -> Result<(), NeplatnaZnacka> {
    let z = znacka(kluc, nonce, doplnok, data);
    // porovnanie v konštantnom čase
    if z.iter().zip(ocakavana).fold(0u8, |a, (x, y)| a | (x ^ y)) != 0 {
        return Err(NeplatnaZnacka);
    }
    chacha20(kluc, 1, nonce, data);
    Ok(())
}

/// Údaje boli zmenené alebo kľúč nesedí.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeplatnaZnacka;

impl std::fmt::Display for NeplatnaZnacka {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("neplatná značka pravosti – údaje boli zmenené alebo kľúč nesedí")
    }
}

impl std::error::Error for NeplatnaZnacka {}

#[cfg(test)]
mod skusky {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn pole32(s: &str) -> [u8; 32] {
        hex(s).try_into().unwrap()
    }

    #[test]
    fn x25519_podla_rfc7748_kap_5_2() {
        let k = pole32("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
        let u = pole32("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c");
        assert_eq!(
            x25519(&k, &u),
            pole32("c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552")
        );
        let k = pole32("4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d");
        let u = pole32("e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493");
        assert_eq!(
            x25519(&k, &u),
            pole32("95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957")
        );
    }

    #[test]
    fn x25519_opakovane_1_a_1000_krat() {
        let (mut k, mut u) = (ZAKLADNY_BOD, ZAKLADNY_BOD);
        for i in 1..=1000 {
            let novy = x25519(&k, &u);
            u = k;
            k = novy;
            if i == 1 {
                assert_eq!(k, pole32("422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079"));
            }
        }
        assert_eq!(k, pole32("684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51"));
    }

    #[test]
    fn dohoda_o_kluci_podla_rfc7748_kap_6_1() {
        let a = pole32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let b = pole32("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let pa = verejny_kluc(&a);
        let pb = verejny_kluc(&b);
        assert_eq!(pa, pole32("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"));
        assert_eq!(pb, pole32("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f"));
        let k = pole32("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
        assert_eq!(spolocne_tajomstvo(&a, &pb), Some(k));
        assert_eq!(spolocne_tajomstvo(&b, &pa), Some(k));
        // bod malého rádu (u = 0) dá samé nuly – odmietne sa
        assert_eq!(spolocne_tajomstvo(&a, &[0; 32]), None);
    }

    const VETA: &[u8] = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

    #[test]
    fn chacha20_podla_rfc8439_kap_2_4_2() {
        let kluc: [u8; 32] = std::array::from_fn(|i| i as u8);
        let nonce: [u8; 12] = hex("000000000000004a00000000").try_into().unwrap();
        let mut d = VETA.to_vec();
        chacha20(&kluc, 1, &nonce, &mut d);
        assert_eq!(
            d,
            hex("6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0bf91b65c5524733ab8f593dabcd62b357
                 1639d624e65152ab8f530c359f0861d807ca0dbf500d6a6156a38e088a22b65e52bc514d16ccf806818ce91ab7793736
                 5af90bbf74a35be6b40b8eedf2785e42874d")
        );
        chacha20(&kluc, 1, &nonce, &mut d);
        assert_eq!(d, VETA);
    }

    #[test]
    fn poly1305_podla_rfc8439_kap_2_5_2_aj_po_kuskoch() {
        let kluc = pole32("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
        let sprava = b"Cryptographic Forum Research Group";
        let ocakavana = hex("a8061dc1305136c6c22b8baf0c0127a9");
        assert_eq!(poly1305(&kluc, sprava).to_vec(), ocakavana);
        for delenie in [1, 3, 15, 16, 17] {
            let mut p = Poly1305::novy(&kluc);
            for kus in sprava.chunks(delenie) {
                p.pridaj(kus);
            }
            assert_eq!(p.dokonci().to_vec(), ocakavana, "po {delenie} bajtoch");
        }
    }

    #[test]
    fn jednorazovy_kluc_poly1305_podla_rfc8439_kap_2_6_2() {
        let kluc: [u8; 32] = std::array::from_fn(|i| 0x80 + i as u8);
        let nonce: [u8; 12] = hex("000000000001020304050607").try_into().unwrap();
        assert_eq!(
            chacha20_blok(&kluc, 0, &nonce)[..32].to_vec(),
            hex("8ad5a08b905f81cc815040274ab29471a833b637e3fd0da508dbb8e2fdd1a646")
        );
    }

    #[test]
    fn aead_podla_rfc8439_kap_2_8_2_a_odhalenie_zmeny() {
        let kluc: [u8; 32] = std::array::from_fn(|i| 0x80 + i as u8);
        let nonce: [u8; 12] = hex("070000004041424344454647").try_into().unwrap();
        let doplnok = hex("50515253c0c1c2c3c4c5c6c7");
        let mut d = VETA.to_vec();
        let z = zasifruj(&kluc, &nonce, &doplnok, &mut d);
        assert_eq!(
            d,
            hex("d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b
                 1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc
                 3ff4def08e4b7a9de576d26586cec64b6116")
        );
        assert_eq!(z.to_vec(), hex("1ae10b594f09e26a7e902ecbd0600691"));
        // zmenený bajt alebo doplnok sa odhalí a údaje zostanú zašifrované
        let mut zmenene = d.clone();
        zmenene[10] ^= 1;
        assert_eq!(desifruj(&kluc, &nonce, &doplnok, &mut zmenene, &z), Err(NeplatnaZnacka));
        assert_eq!(desifruj(&kluc, &nonce, b"iny", &mut d.clone(), &z), Err(NeplatnaZnacka));
        desifruj(&kluc, &nonce, &doplnok, &mut d, &z).unwrap();
        assert_eq!(d, VETA);
    }

    #[test]
    fn dohoda_sedi_pre_mnoho_klucov() {
        // jednoduchý generátor (len na skúšku – skutočné kľúče sú z náhodného zdroja systému)
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut kluc = || -> [u8; 32] {
            std::array::from_fn(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
        };
        for _ in 0..24 {
            let (a, b) = (kluc(), kluc());
            assert_eq!(spolocne_tajomstvo(&a, &verejny_kluc(&b)), spolocne_tajomstvo(&b, &verejny_kluc(&a)));
        }
    }
}
