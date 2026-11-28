//! Odvodenie kľúčov HKDF podľa **RFC 5869** (extrakcia 2.2, rozšírenie 2.3) a pomocná funkcia TLS 1.3
//! HKDF-Expand-Label (RFC 8446 7.1).

use crate::Odtlacok;
use crate::hmac::Hmac;

/// HKDF-Extract: PRK = HMAC(soľ, vstupný kľúčový materiál); prázdna soľ = nuly dĺžky odtlačku.
pub fn extrahuj<H: Odtlacok>(sol: &[u8], ikm: &[u8]) -> Vec<u8> {
    let nuly = vec![0u8; H::DLZKA];
    Hmac::<H>::z(if sol.is_empty() { &nuly } else { sol }, ikm)
}

/// HKDF-Expand: T(1) ‖ T(2) ‖ … skrátené na `dlzka` (najviac 255 × dĺžka odtlačku).
pub fn rozsir<H: Odtlacok>(prk: &[u8], info: &[u8], dlzka: usize) -> Vec<u8> {
    assert!(dlzka <= 255 * H::DLZKA, "HKDF: najviac 255 blokov");
    let mut v = Vec::with_capacity(dlzka + H::DLZKA);
    let mut t: Vec<u8> = Vec::new();
    let mut i = 1u8;
    while v.len() < dlzka {
        let mut h = Hmac::<H>::novy(prk);
        h.pridaj(&t);
        h.pridaj(info);
        h.pridaj(&[i]);
        t = h.hotovo();
        v.extend_from_slice(&t);
        i = i.wrapping_add(1);
    }
    v.truncate(dlzka);
    v
}

/// HKDF-Expand-Label (RFC 8446 7.1): HkdfLabel = dĺžka (2 B) ‖ „tls13 “ + menovka (1 B dĺžka) ‖ kontext (1 B dĺžka).
pub fn rozsir_menovku<H: Odtlacok>(tajomstvo: &[u8], menovka: &str, kontext: &[u8], dlzka: usize) -> Vec<u8> {
    let plna = format!("tls13 {menovka}");
    let mut info = Vec::with_capacity(4 + plna.len() + kontext.len());
    info.extend_from_slice(&(dlzka as u16).to_be_bytes());
    info.push(plna.len() as u8);
    info.extend_from_slice(plna.as_bytes());
    info.push(kontext.len() as u8);
    info.extend_from_slice(kontext);
    rozsir::<H>(tajomstvo, &info, dlzka)
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::sha2::{Sha256, z_hex};

    #[test]
    fn rfc5869_pripad_1() {
        let ikm = [0x0bu8; 22];
        let sol = z_hex("000102030405060708090a0b0c");
        let info = z_hex("f0f1f2f3f4f5f6f7f8f9");
        let prk = extrahuj::<Sha256>(&sol, &ikm);
        assert_eq!(prk, z_hex("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"));
        assert_eq!(
            rozsir::<Sha256>(&prk, &info, 42),
            z_hex("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865")
        );
    }

    #[test]
    fn rfc5869_pripad_2_dlhe_vstupy() {
        let ikm: Vec<u8> = (0x00..=0x4f).collect();
        let sol: Vec<u8> = (0x60..=0xaf).collect();
        let info: Vec<u8> = (0xb0..=0xff).collect();
        let prk = extrahuj::<Sha256>(&sol, &ikm);
        assert_eq!(prk, z_hex("06a6b88c5853361a06104c9ceb35b45cef760014904671014a193f40c15fc244"));
        assert_eq!(
            rozsir::<Sha256>(&prk, &info, 82),
            z_hex(
                "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09\
                 da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87"
            )
        );
    }

    #[test]
    fn rfc5869_pripad_3_bez_soli() {
        let prk = extrahuj::<Sha256>(&[], &[0x0bu8; 22]);
        assert_eq!(prk, z_hex("19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04"));
        assert_eq!(
            rozsir::<Sha256>(&prk, &[], 42),
            z_hex("8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8")
        );
    }

    #[test]
    fn tls13_menovka_rfc8448() {
        // RFC 8448 3 (jednoduchá výmena 1-RTT): early secret = HKDF-Extract(0, 0…0); „derived“ s prázdnym prepisom
        let skore = extrahuj::<Sha256>(&[], &[0u8; 32]);
        assert_eq!(skore, z_hex("33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a"));
        let prazdny = Sha256::z(b"");
        let odvodene = rozsir_menovku::<Sha256>(&skore, "derived", &prazdny, 32);
        assert_eq!(odvodene, z_hex("6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba"));
    }
}
