//! Overenie podpisu RSA – PKCS #1 v1.5 (certifikáty) a RSASSA-PSS (výmena TLS 1.3).
//!
//! Podľa **RFC 8017** (PKCS #1 v2.2): RSAVP1 (5.2.2), EMSA-PKCS1-v1_5 (9.2) s DigestInfo z poznámky 1, EMSA-PSS
//! overenie (9.1.2) s MGF1 (B.2.1). TLS 1.3 (RFC 8446 4.2.3) vyžaduje pri PSS soľ dĺžky odtlačku – overuje sa presne.
//!
//! Len verejné údaje – výpočet nemusí byť v konštantnom čase.

use crate::Odtlacok;
use crate::velke::{self, Modul};

/// Chyba overenia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chyba {
    /// modul párny, príliš malý (< 2048 bitov) alebo exponent mimo 3 … 2³²
    ZlyKluc,
    /// podpis nemá dĺžku modulu alebo je ≥ n
    ZlyPodpis,
    Nesedi,
}

/// Najmenší prijatý modul (CA/Browser Forum Baseline Requirements 6.1.5: 2048 bitov).
pub const NAJMENEJ_BITOV: usize = 2048;

/// Verejný kľúč RSA (n, e) – celé čísla vo veľkom endiane (z certifikátu, RFC 8017 A.1.1).
#[derive(Debug)]
pub struct Kluc {
    modul: Modul,
    e: Vec<u64>,
    /// dĺžka modulu v bajtoch (k)
    k: usize,
}

impl Kluc {
    pub fn novy(n: &[u8], e: &[u8]) -> Result<Kluc, Chyba> {
        let nv = velke::z_bajtov(n);
        let ev = velke::z_bajtov(e);
        let bity = velke::bity(&nv);
        let eb = velke::bity(&ev);
        if !(NAJMENEJ_BITOV..=8192).contains(&bity) || !(2..=33).contains(&eb) || !velke::bit(&ev, 0) {
            return Err(Chyba::ZlyKluc);
        }
        let modul = Modul::novy(&nv).ok_or(Chyba::ZlyKluc)?;
        Ok(Kluc { k: bity.div_ceil(8), modul, e: ev })
    }

    pub fn bity(&self) -> usize {
        self.modul.bity()
    }

    /// RSAVP1: m = s^e mod n, ako `dlzka` bajtov (5.2.2).
    fn rsavp1(&self, podpis: &[u8]) -> Result<Vec<u8>, Chyba> {
        if podpis.len() != self.k {
            return Err(Chyba::ZlyPodpis);
        }
        let s = velke::z_bajtov(podpis);
        if velke::porovnaj(&s, self.modul.n()).is_ge() {
            return Err(Chyba::ZlyPodpis);
        }
        let m = self.modul.z_tvaru(&self.modul.mocnina(&self.modul.do_tvaru(&s), &self.e));
        velke::na_bajty(&m, self.k).ok_or(Chyba::ZlyPodpis)
    }

    /// RSASSA-PKCS1-v1_5 overenie (8.2.2): porovná sa celé zakódovanie EM (nie rozbor – žiadna voľnosť pre útočníka).
    pub fn over_pkcs1<H: Odtlacok>(&self, odtlacok: &[u8], podpis: &[u8]) -> Result<(), Chyba> {
        let predpona = digest_info(H::DLZKA).ok_or(Chyba::ZlyKluc)?;
        if odtlacok.len() != H::DLZKA {
            return Err(Chyba::Nesedi);
        }
        let em = self.rsavp1(podpis)?;
        let t = predpona.len() + odtlacok.len();
        if self.k < t + 11 {
            return Err(Chyba::ZlyKluc);
        }
        let mut ocakavane = vec![0u8, 1];
        ocakavane.resize(self.k - t - 1, 0xff);
        ocakavane.push(0);
        ocakavane.extend_from_slice(predpona);
        ocakavane.extend_from_slice(odtlacok);
        if em == ocakavane { Ok(()) } else { Err(Chyba::Nesedi) }
    }

    /// RSASSA-PSS overenie (8.1.2) s MGF1 tým istým odtlačkom a soľou dĺžky `sol`.
    pub fn over_pss<H: Odtlacok>(&self, odtlacok: &[u8], podpis: &[u8], sol: usize) -> Result<(), Chyba> {
        let h = H::DLZKA;
        if odtlacok.len() != h {
            return Err(Chyba::Nesedi);
        }
        let m = self.rsavp1(podpis)?;
        let em_bity = self.bity() - 1;
        let em_dlzka = em_bity.div_ceil(8);
        // I2OSP(m, emLen): ak emLen < k, prvý bajt m musí byť 0
        let em = if em_dlzka < self.k {
            if m[0] != 0 {
                return Err(Chyba::Nesedi);
            }
            &m[1..]
        } else {
            &m[..]
        };
        if em_dlzka < h + sol + 2 || em[em_dlzka - 1] != 0xbc {
            return Err(Chyba::Nesedi);
        }
        let (maskovane, zvysok) = em.split_at(em_dlzka - h - 1);
        let hh = &zvysok[..h];
        let nevyuzite = 8 * em_dlzka - em_bity;
        let maska_horna = if nevyuzite == 0 { 0 } else { 0xffu8 << (8 - nevyuzite) };
        if maskovane[0] & maska_horna != 0 {
            return Err(Chyba::Nesedi);
        }
        let maska = mgf1::<H>(hh, maskovane.len());
        let mut db: Vec<u8> = maskovane.iter().zip(&maska).map(|(a, b)| a ^ b).collect();
        db[0] &= !maska_horna;
        let ps = em_dlzka - h - sol - 2;
        if db[..ps].iter().any(|&x| x != 0) || db[ps] != 1 {
            return Err(Chyba::Nesedi);
        }
        let salt = &db[ps + 1..];
        let mut m2 = H::novy();
        m2.pridaj(&[0u8; 8]);
        m2.pridaj(odtlacok);
        m2.pridaj(salt);
        if m2.hotovo() == hh { Ok(()) } else { Err(Chyba::Nesedi) }
    }
}

/// DigestInfo pre SHA-256/384/512 (RFC 8017, 9.2, poznámka 1).
fn digest_info(dlzka: usize) -> Option<&'static [u8]> {
    const S256: [u8; 19] = [
        0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00,
        0x04, 0x20,
    ];
    const S384: [u8; 19] = [
        0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00,
        0x04, 0x30,
    ];
    const S512: [u8; 19] = [
        0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00,
        0x04, 0x40,
    ];
    match dlzka {
        32 => Some(&S256),
        48 => Some(&S384),
        64 => Some(&S512),
        _ => None,
    }
}

/// MGF1 (B.2.1).
fn mgf1<H: Odtlacok>(semienko: &[u8], dlzka: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(dlzka + H::DLZKA);
    let mut c: u32 = 0;
    while v.len() < dlzka {
        let mut h = H::novy();
        h.pridaj(semienko);
        h.pridaj(&c.to_be_bytes());
        v.extend(h.hotovo());
        c += 1;
    }
    v.truncate(dlzka);
    v
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::sha2::{Sha256, Sha384, Sha512, z_hex};

    // Kľúče a podpisy z nezávislej knižnice (Python `cryptography` 50 / OpenSSL – rozhodca, 28. 11. 2026), e = 65537.
    const N2048: &str = "cbd0709d0f17600fa3a9890b58c8d02da6d3c324b19d194cb5b100a426552760b1de065f671ff6a9b5cbc3b04ae4fd189f85b46030f3a2e5b12851a736310d7c6f0a1cd5c195e7a84d3dba5b97289bebb250455aa54a583d1c37523d4799f3b29bf2cec1ed2c8b5a7883b49b2c024cf6895abcf8fa78cd05c155203e0a194f65319134be6afa61c966f65b65b337d755d170ac042a5532c28723f7a5a94808b9aa088f8cac326b7d7fbbccedfe2962a38cbc33499397336c61abd7225f4d8a9f7e09a74644d5db19faf8065e047338de903e01eb377a8766bb936671434d65ec1c31d4dffb6dc0d5c11188675c84df7fb83d6c7467ea030df61ce94efdd1ac1b";
    const N4096: &str = "f0e67e2e250c7d123dfbcc544f4ccf497ccd059b6f2cd33ca7d392a7aa822da938de10db2d4f923c651ba6d11f47cc46502b073338c2eaa83b85e343a3f14aff1bf8377b75dcc879c2f5441a07597c2cb20cecf4863af8d625eecd0d5361ed5eab19f9a9e464d1d4fb781ca9daa0a2b6fe7d3423cb4be25cb3dd46bb005fd2abf5a148cc34deccedba5e2d4a9337b7bf792d3276efbcd087b86410b0e94cadc1045bed3291460655e09f77c2739f84233524e6a323d15a265f8863d4f1927dbc5bb94d00884e43109626afc3e172f01489eeb03086e2e4f104a194a806f98d04c0978297e3b396238655c1c3ff7ae582aac3023954786015473c70f97966db85ab5e1e8fa327ebf1f42af55c9531b61d2f8c3b315b74427502fe699a604c138577aa84ce348d8eadb423cd1b26e3716b665eb94332e69ebb0245c506dc6da6762cbe68c225d43c0759ed7891abd233f0ddfde592297e4d82906a8dbbf26251714f40214ac07ff93bbee40f537d3dc257eafa64199ba24703ae152c4cf0d90c32ef37e73278d8d044bcd81a38f3088102c5ed8ef3d606e4311e4eeb7a8797ade9f0c8f28da3cccb04dec39e0c2323190d4efc8497c9dccc7985be3721e4fd7d276f3b144566596ceb370dbb9542cadf3bb6d318db4b566b3797aa8ff4c3a68b21e839c0c28fc7e7e291508a18b915a8658ad6301c25de8dbbf97b3d5bbc659945";
    /// (bity, schéma, odtlačok, odtlačok správy, podpis)
    const PODPISY: [(&str, &str, &str, &str, &str); 8] = [
        (
            "2048",
            "pkcs1",
            "S256",
            "7ed9ca3cfb7a10c74f6f94090c02d5f04d2e5132fec5f76a267b1eba359e0977",
            "a870ccb386a64cdc4d3e481c9e1a16f030ca0d866f2ec5b9e39aad7e04facd8d69e74a616fb92541943734d15d36cb7aad18103287dd030903707a63330a2783c1a03fb50917ff098a2a7f0af8e12129b11011324f6d9009454ac7c6361c7f1f2c56851c33a735f7ae0157b1efb2a99071383aafd1b3f07146371ae37ce9f1a597d71b4db2cde9b6811ed096b955c3e9d821a22759cf1288f8dad63c0333087111b20a5e76d784b7fcdeaff09529c0d624d0e1fd6714cd61d24d1038a08dbd63af6cd76d4abaa9d5dec81628d33eb2c417b5c5b696df6bec85796ef1b4d2bc7b9f05b33b61dc7783f716769b06653bee1f8c3ac75abfbab23014ac598d783186",
        ),
        (
            "2048",
            "pkcs1",
            "S384",
            "b5f21a88fb490230c1520b8a20895ece27262501f29273dae216d5281a315848ed574ddc833a3c64fe1458d942eaed35",
            "522f77e7c2e55a601dc68b457715b9b979e646f557fae4b0bbf300fc4069850ae1a565c75a649d80e576155d8103c203fca458d928aebf4a0ecf02aac2afba858a67c47452b6c069d87f642790b0d144449d052cc2e6416d9d4be1e8d08b4fb964c5969197ebb1e70f20cbca921766b6bd6aa518ae6e55c0f2b76fd5cc6fe166c034178a84b3d5520844b37b46c9d9945d18b9157e5214bf0956b5463d16342158008dd43fffc1cb19e15ff5a15447027f1d38efb1216712a0d7cd8c820d4c409a6f78715381f6d345e56e21cdf6bfcd62a6fe68b51868d87bd1adf55724a9c0b1da8db6b68aad8d205278e58d8067ab98f4269cd6b1929c1fdda7a6e620eff9",
        ),
        (
            "2048",
            "pkcs1",
            "S512",
            "5294bcaf17c777540484bdd575c182dbdc62209613d2161897fe0b298b9ae753c1153b823a134e11708d33d21b65ceb5d52b14a06c987ec13de599a22fc4527d",
            "58044cda2d91e41137da83e0260765072575423a7c50e47b7f668c528805e5dc8c4407b27ff2f86d5b80b2beb75803122b23489e7aeb3393a9741589a7f94a4b94ef75d237becd40a09eb2432cf22ac17f3a9546f2221dc90996e492d43c5e9270bdfa037bc12710c4e469b1a5c739a2ba1b460e6aa33fee9950b3715140c2a078c99a3cbc06c8482b7c2d98950c37a8605087b69864858a8df95b58e618066466ccb27bbf46c4a135a27882cee69030adb47f4083e06d4c8b196c13eeb05ff14fb6f1e330a116a4c3d3968ba2f2891b279aaaa396d6f59959596b7087f0ec7ccc9bafce2c0c9c42a0983a33e19bdc70fc71f1d208dbcb255d48ed4aa267a888",
        ),
        (
            "2048",
            "pss",
            "S256",
            "2b58094a089054f8e809ad2ceccc2d891f7726c38c4f77ac2ce0f058ae4232a7",
            "8453662f60830a950e5f952a24e307d1e9e4be282107efdb4a181c1a81e42315b42753ebc4e2268bd885cb2dae97b7514f9efa1ddc02a0d216306d157af9efc3789dbcbf06a614ef94190836f06e5f0e21800732ba9d2290edc7bda5c975a80340a4ee5c13e827b293ed65d8698f847035d5f5a0a752a7e762d0b123e8217756b14ca4cc38c9c748ddd5d310e70c11d9a580e0120eb77294694bc6617d79fbeb688f526fdd3f67173df081f390823dbe764beac4ffef27701cb7f5f928e77881320d39eb0b7029bb6cfcd46e178098c2f45564a558b16d1a363ad3690c1314d947e291694b41b96d202d6bd84c5de8879a11733b3a1e26e5b78d4e9d0e11a01c",
        ),
        (
            "2048",
            "pss",
            "S384",
            "d0574b66c34be5488ccaad050753fe003cec58569b243d381352d00354147d70ff88d5c22a4e0142560e41ed9a771691",
            "c9421e39305a791d91f8017661e1bf8cf6437014da5134711f778a7713d7ead0141c5f636660ba80d87c1a585db9bcb19b9acb4a422638eeec3da6c5830991539ecd1d20613be36a1e83f9e1704bac78c5f5f40ead6033dbcc17948e81c09acd0922df2a25e4d55ded2ad9882fba21a7990650c1fe7afd0754bf61f20fd1634ba8dbb66ab243f9ef55cb353a6a1432b313b5b021cd07f16d0ae39f4d380919338c59e75ff20e0d7dc4ffad5806a35a93b87ae776eeafccbbb2a06cb1c19f25d736333cd8e122fe49a8c8c857c1d39cc73a0508d57c2b3702feacee9e08402944e40846934700271f2417c93a7aa44b71da318b5cc4de29d65e879fc579b72ced",
        ),
        (
            "2048",
            "pss",
            "S512",
            "f68a51b678c39d80811758a1195a7f178866ee24dc87aedf8f73c4e170be72437b3c15ce8d538512acc21e26147f81c20a956aa516ed66a08976e47e8f315eb3",
            "956d5c7892117537e30518c3c5af8cd254baafe5d890d5e023bcacbf27439293dfb0967fcf705bcbae072caa5e64d21d94f66181d8a9ca6155881f9e9b6d3bb2c84d9ead18a743a02bc9fbb6b59bae78dc366fd9bea580358e8478bdf8730a3875a493986dd80c3cdacf84b2dfcfdb488dea267436eb25d6a140cf257b45b2820b1677764d25619a42d5e26a529ad695989f3d7d871450e71d4f627ded1677f0ec01697a6b24d3702831d29b2104adb3c079c7475a296566701b17c74bb8a108474f86b73eec4ca9eac3997c2c0ed4b5e43f09d00cdab8d9035d8f054547605718131a0fa643cda768ac1e996e45250c338a445fa18376d57bf751235ce79c07",
        ),
        (
            "4096",
            "pkcs1",
            "S256",
            "9fcc8c26e208a5202fa7a05304fa61bd1fa33ca381dda5ef025f0cda0c111d6e",
            "b3bee850b4d7db84fd0953350b64738d7a1afa410f0c7900a23251ecac635eb007765cc0a36151987e71b3b0555f70b615eade4ad2dd87791a4dc1fd29dc696802687918531bb6d7781f4b762f47699f7eb3232d413c00b07420b0ac1925597d4a00e43f9716bd1e7ab71180218b7046ccc3c1c7533c7a959373ab282b4246f93d470c4246fd255d6a5342231eea03e9ac8d18308363693b9ad51e99b95881ea1515bbce6e3faaf6dd6a52718151adcce1218e56e12c16d180a808666a2fcb7978d404fdc3e7107bedccf34c62725110a32285eb78d23d57ebd89e77ac2df8eb42dd6df92d79a4cb642fd26eb8b1c3d515b056a9ec573d5ac109ef923965d82c02c9080bb893c8946a151e107ac8adced35f7f7d231f4af3144928ec874f96eaca23279b5a756fa9b4c2f4e92e8eb56e52071fab6706cd6fa7ba67a6fbfd38c9e10e4a25e24b95a7f04a47bacbe63957daa0496fb628360edaacb96882ab6a1fabac36b89a603202c9766cc6e0940ceeebc041c358d3191b364df5aa10d0c407b1bdb9a9636257c1cddd7fedcb8b725ceb09330c0ad6bb595d350cd0a28b8cd3a7ec950f8f81a8ca4c447ff3dffd40fea6e104cdcb499cec9932f8d2dee34ba8874d29efb06b1f6716312b745f78b75f64f8828c0a93ebb705bbcbb4752566cee65f926b3fb84493206b7ce2178498fc5580ac39e3bf706cf9d9a6ce7060aec8",
        ),
        (
            "4096",
            "pss",
            "S512",
            "b81b2e3120e141ce48020f17c21c96d972e3d7ed6a45cf82ff9b41fecb912ee7d15905766f7aa45891e7c17d593b698fe6e18b94bcd027667eb85653697cb867",
            "107442168f8e6cdeea9a34e6e3c1d55419d8569e8fcfdc394ac3689ab16c06d43938800006a919d3997243644fa66eb7f6cdddd50af556d01c82dead7620e84444634db2e95a25aef3ec7fbb089a68039c9a8401c1644299da6824f22898bca21ba379809ff2f11d23fb112d50e18b20a530b0248e0d5588fc00a80143ad2c217351d13ea75d26be53f509e74cbd41e58f99f2c27571235062c5ac2798b66796e366c3ef1892cdedca299dc1347f96d175fcc5933ab59c7138de2694a56da21e87c277a2d7083575308d698db4154e296f936f681726fc19c1561cc949bc5dabc0d0c03575376bdf390ce2ef9369c45cea415471a341fd29e34070ffa73d439773fbb4f94a89529ef98bac019b00408e7f8c3443ee4dc71b68d23ea050fa4785656038bc54d2bbd96940d8de175dff504240b7cec066b828f71dafc46f8da35b013022782529079400310a65483fb2081d5e730fb64782b259a68097488d5f8a3d33f3c5752e1715287b5a189c1df1687248a9521bde0d45fff22900eae9ed519a8e49075d1296a080e7bd6cc6954a5332b5f1ef566e21ed01eed84cdcfa2643ce777152d0ff566cc472661c94300d372eaf7377f68cba7f5958fb3ed3dafa04eb6544788a352dd7501be34b88d2ca47407d9a298ee8c46144eb34c093f182b80bcd1ed79ec33ef1fc3f3c86bdbcf87ab66eb7424ff93680f236a6d3fec1789d",
        ),
    ];

    fn over(kluc: &Kluc, schema: &str, h: &str, odt: &[u8], podpis: &[u8]) -> Result<(), Chyba> {
        match (schema, h) {
            ("pkcs1", "S256") => kluc.over_pkcs1::<Sha256>(odt, podpis),
            ("pkcs1", "S384") => kluc.over_pkcs1::<Sha384>(odt, podpis),
            ("pkcs1", "S512") => kluc.over_pkcs1::<Sha512>(odt, podpis),
            ("pss", "S256") => kluc.over_pss::<Sha256>(odt, podpis, 32),
            ("pss", "S384") => kluc.over_pss::<Sha384>(odt, podpis, 48),
            _ => kluc.over_pss::<Sha512>(odt, podpis, 64),
        }
    }

    #[test]
    fn podpisy_nezavislej_kniznice() {
        let e = [1u8, 0, 1];
        for (bity, schema, h, odt, podpis) in PODPISY {
            let n = z_hex(if bity == "2048" { N2048 } else { N4096 });
            let kluc = Kluc::novy(&n, &e).unwrap();
            assert_eq!(kluc.bity().to_string(), bity);
            let (odt, podpis) = (z_hex(odt), z_hex(podpis));
            assert_eq!(over(&kluc, schema, h, &odt, &podpis), Ok(()), "{bity} {schema} {h}");
            let mut zly = odt.clone();
            zly[0] ^= 1;
            assert_eq!(
                over(&kluc, schema, h, &zly, &podpis),
                Err(Chyba::Nesedi),
                "{bity} {schema} {h} správa"
            );
            let mut zly = podpis.clone();
            zly[100] ^= 4;
            assert_eq!(over(&kluc, schema, h, &odt, &zly), Err(Chyba::Nesedi), "{bity} {schema} {h} podpis");
            assert_eq!(over(&kluc, schema, h, &odt, &podpis[1..]), Err(Chyba::ZlyPodpis), "dĺžka");
            assert_eq!(over(&kluc, schema, h, &odt, &n), Err(Chyba::ZlyPodpis), "podpis = n");
        }
        // PSS s inou dĺžkou soli a PKCS #1 schéma namiesto PSS sa neprijmú
        let n = z_hex(N2048);
        let kluc = Kluc::novy(&n, &e).unwrap();
        let (_, _, _, odt, podpis) = PODPISY[3];
        assert_eq!(kluc.over_pss::<Sha256>(&z_hex(odt), &z_hex(podpis), 20), Err(Chyba::Nesedi));
        assert_eq!(kluc.over_pkcs1::<Sha256>(&z_hex(odt), &z_hex(podpis)), Err(Chyba::Nesedi));
    }

    #[test]
    fn odmietnute_kluce() {
        let n = z_hex(N2048);
        assert!(Kluc::novy(&n[..128], &[1, 0, 1]).is_err(), "1024 bitov");
        assert!(Kluc::novy(&n, &[1]).is_err(), "e = 1");
        assert!(Kluc::novy(&n, &[1, 0, 0]).is_err(), "párny e");
        let mut parny = n.clone();
        *parny.last_mut().unwrap() &= 0xfe;
        assert!(Kluc::novy(&parny, &[1, 0, 1]).is_err(), "párny modul");
    }
}
