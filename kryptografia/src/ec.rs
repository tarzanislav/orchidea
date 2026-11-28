//! Overenie podpisu ECDSA na krivkách P-256, P-384 (certifikáty a výmena TLS 1.3) a P-521 (len certifikáty).
//!
//! Podľa **FIPS 186-5** (6.4.2 overenie podpisu ECDSA), parametre kriviek **NIST SP 800-186** (3.2.1.3 P-256,
//! 3.2.1.4 P-384, 3.2.1.5 P-521) a **SEC 1 v2** (2.3.4 bod z bajtov, 3.2.2.1 kontrola verejného kľúča). Vzorce v Jakobiho
//! súradniciach pre a = −3: zdvojenie „dbl-2001-b“ a sčítanie „add-2007-bl“ z Explicit-Formulas Database
//! (Bernstein, Lange). Súčet u₁·G + u₂·Q naraz Shamirovým postupom (ElGamal 1985).
//!
//! Len verejné údaje – výpočet nemusí byť v konštantnom čase.

use crate::velke::{self, Modul, Slova};

/// Krivka y² = x³ − 3x + b nad prvočíselným telesom p, rád bázového bodu n.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Krivka {
    P256,
    P384,
    /// len v certifikátoch (niektoré koreňové autority) – TLS 1.3 ju v ClientHello neponúka
    P521,
}

struct Parametre {
    p: &'static str,
    n: &'static str,
    b: &'static str,
    gx: &'static str,
    gy: &'static str,
}

const P256: Parametre = Parametre {
    p: "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
    n: "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551",
    b: "5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b",
    gx: "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
    gy: "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
};

const P384: Parametre = Parametre {
    p: "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
    n: "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
    b: "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef",
    gx: "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
    gy: "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f",
};

const P521: Parametre = Parametre {
    p: "01ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    n: "01fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa51868783bf2f966b7fcc0148f709a5d03bb5c9b8899c47aebb6fb71e91386409",
    b: "0051953eb9618e1c9a1f929a21a0b68540eea2da725b99b315f3b8b489918ef109e156193951ec7e937b1652c0bd3bb1bf073573df883d2c34f1ef451fd46b503f00",
    gx: "00c6858e06b70404e9cd9e3ecb662395b4429c648139053fb521f828af606b4d3dbaa14b5e77efe75928fe1dc127a2ffa8de3348b3c1856a429bf97e7e31c2e5bd66",
    gy: "011839296a789a3bc0045c8a5fb42c7d1bd998f54449579b446817afbd17273e662c97ee72995ef42640c550b9013fad0761353c7086a272c24088be94769fd16650",
};

fn z_hex(s: &str) -> Slova {
    let b: Vec<u8> =
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0)).collect();
    velke::z_bajtov(&b)
}

impl Krivka {
    /// Dĺžka súradnice v bajtoch.
    pub fn dlzka(self) -> usize {
        match self {
            Krivka::P256 => 32,
            Krivka::P384 => 48,
            Krivka::P521 => 66,
        }
    }

    fn parametre(self) -> &'static Parametre {
        match self {
            Krivka::P256 => &P256,
            Krivka::P384 => &P384,
            Krivka::P521 => &P521,
        }
    }
}

/// Bod v Jakobiho súradniciach (X/Z², Y/Z³), súradnice v Montgomeryho tvare; Z = 0 je nekonečno.
#[derive(Clone, Debug)]
struct Bod {
    x: Slova,
    y: Slova,
    z: Slova,
}

struct Teleso {
    p: Modul,
    b: Slova,
}

impl Teleso {
    fn nekonecno(&self) -> Bod {
        let k = self.p.slov();
        Bod { x: self.p.jedna(), y: self.p.jedna(), z: vec![0; k] }
    }

    fn zdvoj(&self, a: &Bod) -> Bod {
        let p = &self.p;
        if velke::je_nula(&a.z) || velke::je_nula(&a.y) {
            return self.nekonecno();
        }
        let delta = p.nasob(&a.z, &a.z);
        let gama = p.nasob(&a.y, &a.y);
        let beta = p.nasob(&a.x, &gama);
        let t = p.nasob(&p.odcitaj(&a.x, &delta), &p.pricitaj(&a.x, &delta));
        let alfa = p.pricitaj(&p.pricitaj(&t, &t), &t);
        let beta2 = p.pricitaj(&beta, &beta);
        let beta4 = p.pricitaj(&beta2, &beta2);
        let beta8 = p.pricitaj(&beta4, &beta4);
        let x3 = p.odcitaj(&p.nasob(&alfa, &alfa), &beta8);
        let yz = p.pricitaj(&a.y, &a.z);
        let z3 = p.odcitaj(&p.odcitaj(&p.nasob(&yz, &yz), &gama), &delta);
        let g2 = p.nasob(&gama, &gama);
        let g4 = p.pricitaj(&g2, &g2);
        let g8 = p.pricitaj(&p.pricitaj(&g4, &g4), &p.pricitaj(&g4, &g4));
        let y3 = p.odcitaj(&p.nasob(&alfa, &p.odcitaj(&beta4, &x3)), &g8);
        Bod { x: x3, y: y3, z: z3 }
    }

    fn scitaj(&self, a: &Bod, b: &Bod) -> Bod {
        let p = &self.p;
        if velke::je_nula(&a.z) {
            return b.clone();
        }
        if velke::je_nula(&b.z) {
            return a.clone();
        }
        let z1z1 = p.nasob(&a.z, &a.z);
        let z2z2 = p.nasob(&b.z, &b.z);
        let u1 = p.nasob(&a.x, &z2z2);
        let u2 = p.nasob(&b.x, &z1z1);
        let s1 = p.nasob(&p.nasob(&a.y, &b.z), &z2z2);
        let s2 = p.nasob(&p.nasob(&b.y, &a.z), &z1z1);
        let h = p.odcitaj(&u2, &u1);
        let rr = p.odcitaj(&s2, &s1);
        if velke::je_nula(&h) {
            return if velke::je_nula(&rr) { self.zdvoj(a) } else { self.nekonecno() };
        }
        let r = p.pricitaj(&rr, &rr);
        let h2 = p.pricitaj(&h, &h);
        let i = p.nasob(&h2, &h2);
        let j = p.nasob(&h, &i);
        let v = p.nasob(&u1, &i);
        let x3 = p.odcitaj(&p.odcitaj(&p.nasob(&r, &r), &j), &p.pricitaj(&v, &v));
        let s1j = p.nasob(&s1, &j);
        let y3 = p.odcitaj(&p.nasob(&r, &p.odcitaj(&v, &x3)), &p.pricitaj(&s1j, &s1j));
        let zz = p.pricitaj(&a.z, &b.z);
        let z3 = p.nasob(&p.odcitaj(&p.odcitaj(&p.nasob(&zz, &zz), &z1z1), &z2z2), &h);
        Bod { x: x3, y: y3, z: z3 }
    }

    /// Afinná súradnica x (bežný tvar); `None` pre nekonečno.
    fn afinne_x(&self, a: &Bod) -> Option<Slova> {
        if velke::je_nula(&a.z) {
            return None;
        }
        let zi = self.p.inverzia_prvocislo(&a.z);
        Some(self.p.z_tvaru(&self.p.nasob(&a.x, &self.p.nasob(&zi, &zi))))
    }

    /// Je bod (x, y) v Montgomeryho tvare na krivke? y² = x³ − 3x + b.
    fn na_krivke(&self, x: &[u64], y: &[u64]) -> bool {
        let p = &self.p;
        let y2 = p.nasob(y, y);
        let x3 = p.nasob(&p.nasob(x, x), x);
        let tri_x = p.pricitaj(&p.pricitaj(x, x), x);
        let prava = p.pricitaj(&p.odcitaj(&x3, &tri_x), &self.b);
        y2 == prava
    }
}

/// Chyba overenia (dôvod pre záznam).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chyba {
    /// verejný kľúč nie je bod krivky v nekomprimovanom tvare 04‖X‖Y alebo neleží na krivke
    ZlyKluc,
    /// r alebo s mimo 1 … n − 1
    ZlyPodpis,
    /// podpis nesedí
    Nesedi,
}

/// Overí podpis ECDSA (r, s – celé čísla vo veľkom endiane) nad odtlačkom `odtlacok` správy kľúčom
/// `kluc` = 04 ‖ X ‖ Y (SEC 1, 2.3.3, nekomprimovaný bod).
pub fn over(krivka: Krivka, kluc: &[u8], odtlacok: &[u8], r: &[u8], s: &[u8]) -> Result<(), Chyba> {
    let par = krivka.parametre();
    let d = krivka.dlzka();
    let pm = Modul::novy(&z_hex(par.p)).ok_or(Chyba::ZlyKluc)?;
    let nm = Modul::novy(&z_hex(par.n)).ok_or(Chyba::ZlyKluc)?;
    let k = pm.slov();
    let teleso = Teleso { b: pm.do_tvaru(&z_hex(par.b)), p: pm };
    // verejný kľúč: tvar, rozsah súradníc, bod na krivke (SEC 1, 3.2.2.1; rád kofaktor 1 – stačí ≠ nekonečno)
    if kluc.len() != 1 + 2 * d || kluc[0] != 4 {
        return Err(Chyba::ZlyKluc);
    }
    let (qx, qy) = (velke::z_bajtov(&kluc[1..1 + d]), velke::z_bajtov(&kluc[1 + d..]));
    let pn = teleso.p.n().to_vec();
    if velke::porovnaj(&qx, &pn).is_ge() || velke::porovnaj(&qy, &pn).is_ge() {
        return Err(Chyba::ZlyKluc);
    }
    let (qx, qy) = (teleso.p.do_tvaru(&qx), teleso.p.do_tvaru(&qy));
    if !teleso.na_krivke(&qx, &qy) {
        return Err(Chyba::ZlyKluc);
    }
    // r, s v 1 … n − 1
    let nn = nm.n().to_vec();
    let (rv, sv) = (velke::z_bajtov(r), velke::z_bajtov(s));
    for v in [&rv, &sv] {
        if velke::je_nula(v) || velke::porovnaj(v, &nn).is_ge() {
            return Err(Chyba::ZlyPodpis);
        }
    }
    // e = najľavejších bity(n) bitov odtlačku (FIPS 186-5, 6.4.2 krok 3), potom mod n
    let nbity = nm.bity();
    let mut e = velke::z_bajtov(odtlacok);
    if odtlacok.len() * 8 > nbity {
        let posun = odtlacok.len() * 8 - nbity;
        e = posun_vpravo(&e, posun);
    }
    let e = nm.redukuj(&e).ok_or(Chyba::ZlyPodpis)?;
    let w = nm.inverzia_prvocislo(&nm.do_tvaru(&sv));
    let u1 = nm.z_tvaru(&nm.nasob(&nm.do_tvaru(&e), &w));
    let u2 = nm.z_tvaru(&nm.nasob(&nm.do_tvaru(&rv), &w));
    let jedna = teleso.p.jedna();
    let g =
        Bod { x: teleso.p.do_tvaru(&z_hex(par.gx)), y: teleso.p.do_tvaru(&z_hex(par.gy)), z: jedna.clone() };
    let q = Bod { x: qx, y: qy, z: jedna };
    let gq = teleso.scitaj(&g, &q);
    let mut a = teleso.nekonecno();
    for i in (0..velke::bity(&u1).max(velke::bity(&u2))).rev() {
        a = teleso.zdvoj(&a);
        a = match (velke::bit(&u1, i), velke::bit(&u2, i)) {
            (true, true) => teleso.scitaj(&a, &gq),
            (true, false) => teleso.scitaj(&a, &g),
            (false, true) => teleso.scitaj(&a, &q),
            (false, false) => a,
        };
    }
    let x = teleso.afinne_x(&a).ok_or(Chyba::Nesedi)?;
    // x mod n == r
    let mut x = x;
    x.resize(k.max(nm.slov()), 0);
    let x = nm.redukuj(&x).ok_or(Chyba::Nesedi)?;
    let mut rr = rv;
    rr.resize(x.len(), 0);
    if velke::porovnaj(&x, &rr).is_eq() { Ok(()) } else { Err(Chyba::Nesedi) }
}

fn posun_vpravo(a: &[u64], o: usize) -> Slova {
    let (slova, bity) = (o / 64, o % 64);
    let mut r = vec![0u64; a.len().saturating_sub(slova).max(1)];
    for (i, x) in r.iter_mut().enumerate() {
        let nizke = a.get(i + slova).copied().unwrap_or(0);
        let vysoke = a.get(i + slova + 1).copied().unwrap_or(0);
        *x = if bity == 0 { nizke } else { (nizke >> bity) | (vysoke << (64 - bity)) };
    }
    r
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::sha2::z_hex as hx;

    /// Bázový bod leží na krivke a n·G = nekonečno – kontrola prepísaných parametrov (chyba v jedinej číslici
    /// by obe podmienky porušila).
    #[test]
    fn parametre_kriviek() {
        for krivka in [Krivka::P256, Krivka::P384, Krivka::P521] {
            let par = krivka.parametre();
            let pm = Modul::novy(&z_hex(par.p)).unwrap();
            let t = Teleso { b: pm.do_tvaru(&z_hex(par.b)), p: pm };
            let (gx, gy) = (t.p.do_tvaru(&z_hex(par.gx)), t.p.do_tvaru(&z_hex(par.gy)));
            assert!(t.na_krivke(&gx, &gy), "{krivka:?}: G na krivke");
            let g = Bod { x: gx, y: gy, z: t.p.jedna() };
            let n = z_hex(par.n);
            let mut a = t.nekonecno();
            for i in (0..velke::bity(&n)).rev() {
                a = t.zdvoj(&a);
                if velke::bit(&n, i) {
                    a = t.scitaj(&a, &g);
                }
            }
            assert!(velke::je_nula(&a.z), "{krivka:?}: n·G = nekonečno");
        }
    }

    /// Podpisy vytvorené nezávislou knižnicou (Python `cryptography` 50 / OpenSSL – rozhodca, 28. 11. 2026):
    /// (krivka, odtlačok, kľúč 04‖X‖Y, odtlačok správy, r, s). P-256 so SHA-384/512 skracuje odtlačok (6.4.2 krok 3).
    const PODPISY: [(&str, &str, &str, &str, &str, &str); 7] = [
        (
            "P256",
            "S256",
            "04e061ee6f57e49d75e9f076bee598d9cbb242c13c768f3c48fc2c99aa5ea80a0f8d9ef297f9faf97d6d2f4320c49ab34b748af5a6ed838d0a3f39032f66adab7e",
            "88283268dbaa68e70a5cc361b2bd796c9ac8d4bb48360fc908e6ee284760306d",
            "cd5ab24b876b8ef7420d79fd83513ebe4ca62bd9825100151803fcfa47a99007",
            "f47974f446671efdf2ab88646f5a353be358efd09c5a24e9f7ff6a62fbcea522",
        ),
        (
            "P256",
            "S384",
            "048c916f613e0f614b2c805eddfffa91e2d59de4510fbaca64e37aa656fcab2bf27e2120febca7653a804e563e11e1b420c26a5537d4c5cd64a3479c5d47defa4f",
            "bf50df9322767d6fa6b680e18d5839c0cce0fea48053411a3443aedb9fc590dee8a735b37f7624948a0574d75876ae9f",
            "46be94d4eb002e1303796e66d3000315c6491d4137229e05a0e5ef73f851ef7f",
            "5a99d2f46a4572a8e15493018cbbdfb4237da80636fd5bd4d395ebdaa59c5052",
        ),
        (
            "P256",
            "S512",
            "0463b672f8e45de756e4cc2ee7e8c1b84ca4e5ac622e276691b6d5625f4a52dbe182827cc4bd01942ac7663d7cb30903f09d90bf6792585ae59d13aff545d4500f",
            "019cc7dc487bda53ffd247eb6c659f022b69154148ded2fdd7ebf839441605aa50cc8a520c074a79120b6f168230b11a8bfa6ca55c259ac9f6ae4468f63fa4cb",
            "5926114b16a50d261bd5ee6de170270c97602b0f45cd7a7af5dd77550280bdc6",
            "7144cfece78bb7ac330e73fd47d44a90c52c180d4c24fc1502778255c4cca37f",
        ),
        (
            "P384",
            "S256",
            "04224ac102b231fe604fb5d2d2a59d8ec8c09edd4ad6bedf2b7509b84530b863131c740f6df14a325de439fb6ae76ae664853b72502b30f19cc5ea806f4d1cf0c32fbfa6a4047d368c262330f5401cb3eccd4d1d981daa12792da859b5b9d3971e",
            "01bba2cc87ef910eb126014071a13ccae66fe28ae609a547448d6a278450f8d7",
            "0da3113b84881b4e05f540197feafcb0c832ef1146eec754410459bdcbe43f2fad3f8917d69a29f4ac2b65f55c868d9e",
            "fb431b578b464257a6c6a009c85037a495a7cf5e746ac25865d2be43ba7d62521be312b979ec0b6fb18748c046817f08",
        ),
        (
            "P384",
            "S384",
            "0409c406e9d9f37b12b6d708d8e3700eb13afd82a89a93d6cde5dfd7a23fb304ac810d121e7444058072b74ec19818fb6fcb79c97fa572f71ac436d96851aaf405be88a342ff2ccd8db3a27a55e6171bf68527993bf4925ce90600de15f4c673f0",
            "eabecccbd9bd8128b70c712e3b3ba41d9b8a8594606fb80d837d4704ee70f1696702a33ccd160f8c52d85b15b59e43a1",
            "39cf88413ba062029002a8d6ee008b56b06c4b57fb39b7ec63b64b80db8c3bb2a71aa83dfb15055c285adfcc1d28af7c",
            "5e0a48e292cd4433c1f38b32ab3dbcdaf02ea3ba4e4f1deca3c332e84b9dc7e622785a24179fa3556afa300258b660fa",
        ),
        (
            "P384",
            "S512",
            "048491e61977eb41495973b23c333157b1dced196d46cbc44fb04bb6eca74fd98f5ac7ea7410247d702ad896134d89ec10c6569634bcb9a8555c041a51c772d4a0909986e04fe31ed7009846fe2fdd75e76418396c503feb185d0bbe5c742ec917",
            "e07cf456ed9aecabc239e98d3718ccb2c4e5c349cb331d448b7df6010689312e56a3867194721f3f934cb87a3dafd6412d777f8f1fed422e4205253557b99839",
            "768d0b813560787993f368cccd4265379f92a3df80ab9c50edf46ee9e2f2cb96569b93615b680999a25b2cbbf8848e62",
            "854dce53c36b35c59a018d7cab01041bb3dd9801f399e68706e4f6a82b2eec1ab3c0305af1a8e427495b1101eadacd30",
        ),
        (
            "P521",
            "S512",
            "0400d9549e20e6f219827a003a7ba5d0e7b076a5f27f0179f26aa5142553bb65a36b2a50110310caa0839612643c1b1100b9f5e808832dbeee8ea2cec2b66bc832665a00ce03767e96823607c49dbb6f5e74fa50acdab1a4129c291b26631685c317feb4918033828d8e37c444112b063562dc3315efe0a38d31bba17483b01d946ddd83d7",
            "d38e330eabc2a05b35cfd4c8b5fe9f885ce103b485f991b9a4f3479f5593f7abb5e5ca4bebcaf30aaace6a37bd49f387a4c6d732fef56fd65a4b0a987387991e",
            "00d0ad750b0271f05780fd9921354690a94326f50b0f3ab43983d13faad0f4adb912f8396ecddcb8ad0f6f07d0c07c3a3661a7f2fc7a6a831cfa48fcd47de7d70fd2",
            "0165155da3be797d22bbce189d0e80fdbf80892a57fbe97dc80ba0d4deb2faa75dcdc1dac5bb374641d635a3cedb62221ef93c6a90d83801fd53048f7fe14dee6423",
        ),
    ];

    #[test]
    fn podpisy_nezavislej_kniznice() {
        for (k, h, kluc, odt, r, s) in PODPISY {
            let krivka = match k {
                "P256" => Krivka::P256,
                "P384" => Krivka::P384,
                _ => Krivka::P521,
            };
            let (kluc, odt, r, s) = (hx(kluc), hx(odt), hx(r), hx(s));
            assert_eq!(over(krivka, &kluc, &odt, &r, &s), Ok(()), "{k} {h}");
            // zmenený odtlačok, prehodené r a s, iný kľúč
            let mut zly = odt.clone();
            zly[5] ^= 0x10;
            assert_eq!(over(krivka, &kluc, &zly, &r, &s), Err(Chyba::Nesedi), "{k} {h} zmenená správa");
            assert_eq!(over(krivka, &kluc, &odt, &s, &r), Err(Chyba::Nesedi), "{k} {h} r↔s");
            let mut kluc2 = kluc.clone();
            kluc2[10] ^= 1;
            assert_eq!(over(krivka, &kluc2, &odt, &r, &s), Err(Chyba::ZlyKluc), "{k} {h} bod mimo krivky");
            // r = 0, s = n
            assert_eq!(over(krivka, &kluc, &odt, &[0], &s), Err(Chyba::ZlyPodpis));
            let n = hx(krivka.parametre().n);
            assert_eq!(over(krivka, &kluc, &odt, &r, &n), Err(Chyba::ZlyPodpis));
            // komprimovaný tvar a zlá dĺžka sa neprijmú
            assert_eq!(over(krivka, &kluc[..1 + krivka.dlzka()], &odt, &r, &s), Err(Chyba::ZlyKluc));
        }
    }
}
