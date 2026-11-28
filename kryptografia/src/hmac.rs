//! Overovací kód správy HMAC podľa **RFC 2104** a **FIPS 198-1**: H((K ⊕ opad) ‖ H((K ⊕ ipad) ‖ správa)).

use crate::Odtlacok;

/// HMAC s odtlačkom `H`; dá sa plniť po kúskoch.
#[derive(Debug, Clone)]
pub struct Hmac<H: Odtlacok> {
    vnutorny: H,
    vonkajsi: H,
}

impl<H: Odtlacok> Hmac<H> {
    pub fn novy(kluc: &[u8]) -> Hmac<H> {
        // kľúč dlhší ako blok sa najprv zahašuje; kratší sa doplní nulami (RFC 2104 kap. 2)
        let mut k = if kluc.len() > H::BLOK { H::z(kluc) } else { kluc.to_vec() };
        k.resize(H::BLOK, 0);
        let mut vnutorny = H::novy();
        vnutorny.pridaj(&k.iter().map(|b| b ^ 0x36).collect::<Vec<u8>>());
        let mut vonkajsi = H::novy();
        vonkajsi.pridaj(&k.iter().map(|b| b ^ 0x5c).collect::<Vec<u8>>());
        Hmac { vnutorny, vonkajsi }
    }

    pub fn pridaj(&mut self, data: &[u8]) {
        self.vnutorny.pridaj(data);
    }

    pub fn hotovo(self) -> Vec<u8> {
        let mut v = self.vonkajsi;
        v.pridaj(&self.vnutorny.hotovo());
        v.hotovo()
    }

    /// HMAC celých údajov naraz.
    pub fn z(kluc: &[u8], data: &[u8]) -> Vec<u8> {
        let mut h = Hmac::<H>::novy(kluc);
        h.pridaj(data);
        h.hotovo()
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::sha2::{Sha256, Sha384, Sha512, z_hex};

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn rfc4231() {
        // prípad 1: kľúč 20 × 0x0b, „Hi There“
        let k = [0x0bu8; 20];
        assert_eq!(
            hex(&Hmac::<Sha256>::z(&k, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&Hmac::<Sha384>::z(&k, b"Hi There")),
            "afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6"
        );
        assert_eq!(
            hex(&Hmac::<Sha512>::z(&k, b"Hi There")),
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4\
             be9d914eeb61f1702e696c203a126854"
        );
        // prípad 2: kľúč „Jefe“
        assert_eq!(
            hex(&Hmac::<Sha256>::z(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // prípad 3: kľúč 20 × 0xaa, údaje 50 × 0xdd
        assert_eq!(
            hex(&Hmac::<Sha256>::z(&[0xaa; 20], &[0xdd; 50])),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
        // prípad 6: kľúč 131 × 0xaa (dlhší ako blok) – najprv odtlačok kľúča
        assert_eq!(
            hex(&Hmac::<Sha256>::z(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        // prípad 4: kľúč 0x01…0x19, údaje 50 × 0xcd
        let k4 = z_hex("0102030405060708090a0b0c0d0e0f10111213141516171819");
        assert_eq!(
            hex(&Hmac::<Sha256>::z(&k4, &[0xcd; 50])),
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
        );
    }
}
