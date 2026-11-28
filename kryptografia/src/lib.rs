//! Kryptografia ORCHIDEA – vlastné šifry a odtlačky podľa textu noriem (A12, šifrované spojenie s webom).
//!
//! | Modul | Čo | Norma |
//! |---|---|---|
//! | [`sha2`] | odtlačky SHA-256, SHA-384, SHA-512 | FIPS 180-4 |
//! | [`hmac`] | overovací kód správy | RFC 2104, FIPS 198-1 |
//! | [`hkdf`] | odvodenie kľúčov (TLS 1.3) | RFC 5869 |
//!
//! Každý modul má skúšky s vektormi z noriem (nezávislý rozhodca). X25519 a ChaCha20-Poly1305 sú zatiaľ v balíku
//! zabezpečeného spojenia zariadení (KORENE) a presunú sa sem.

#![forbid(unsafe_code)]

pub mod hkdf;
pub mod hmac;
pub mod sha2;

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
