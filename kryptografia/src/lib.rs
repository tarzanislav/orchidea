//! Kryptografia ORCHIDEA – vlastné šifry a odtlačky podľa textu noriem (A12, šifrované spojenie s webom).
//!
//! | Modul | Čo | Norma |
//! |---|---|---|
//! | [`sha2`] | odtlačky SHA-256, SHA-384, SHA-512 | FIPS 180-4 |
//! | [`hmac`] | overovací kód správy | RFC 2104, FIPS 198-1 |
//! | [`hkdf`] | odvodenie kľúčov (TLS 1.3) | RFC 5869 |
//! | [`ec`] | overenie podpisu ECDSA na P-256 a P-384 | FIPS 186-5, SP 800-186, SEC 1 |
//! | [`rsa`] | overenie podpisu RSA (PKCS #1 v1.5, PSS) | RFC 8017 |
//! | [`velke`] | veľké čísla, Montgomeryho násobenie (RSA, ECDSA – len verejné údaje) | Montgomery 1985 |
//! | [`aes`] | bloková šifra AES-128/256 a autentizované šifrovanie GCM, v konštantnom čase | FIPS 197, NIST SP 800-38D |
//!
//! Každý modul má skúšky s vektormi z noriem (nezávislý rozhodca). Overenie podpisov (ECDSA, RSA) pracuje len s verejnými údajmi. X25519 a ChaCha20-Poly1305 sú zatiaľ v balíku
//! zabezpečeného spojenia zariadení (KORENE) a presunú sa sem.

#![forbid(unsafe_code)]

pub mod aes;
pub mod ec;
pub mod hkdf;
pub mod hmac;
pub mod rsa;
pub mod sha2;
pub mod velke;

/// Odtlačok (hash) so spoločným rozhraním pre HMAC, HKDF a prepis výmeny TLS.
pub trait Odtlacok: Clone {
    /// dĺžka výsledku v bajtoch
    const DLZKA: usize;
    /// dĺžka bloku v bajtoch (HMAC)
    const BLOK: usize;
    fn novy() -> Self;
    fn pridaj(&mut self, data: &[u8]);
    fn hotovo(self) -> Vec<u8>;

    /// Odtlačok celých údajov naraz.
    fn z(data: &[u8]) -> Vec<u8> {
        let mut h = Self::novy();
        h.pridaj(data);
        h.hotovo()
    }
}

/// Porovnanie v konštantnom čase (overovacie kódy, značky) – čas nezávisí od toho, kde sa líšia.
pub fn rovnake(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |r, (x, y)| r | (x ^ y)) == 0
}
