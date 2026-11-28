//! Klient TLS 1.3 – šifrované spojenie s webom (HTTPS), úloha A12.
//!
//! Podľa **RFC 8446**: záznamová vrstva (5, ochrana záznamu 5.2, nonce 5.3), výmena (4: ClientHello 4.1.2,
//! ServerHello 4.1.3, rozšírenia 4.2, EncryptedExtensions 4.3.1, Certificate 4.4.2, CertificateVerify 4.4.3,
//! Finished 4.4.4, KeyUpdate 4.6.3), odvodenie kľúčov (7.1), kľúče prenosu (7.3), upozornenia (6). Režim
//! kompatibility so sprostredkovateľmi (D.4): neprázdne legacy_session_id a ChangeCipherSpec pred druhým letom.
//!
//! Šifrovacie sady v poradí ponuky: **TLS_CHACHA20_POLY1305_SHA256** (rýchla aj bez inštrukcií AES – RPi 3, 4),
//! TLS_AES_128_GCM_SHA256, TLS_AES_256_GCM_SHA384. Dohoda o kľúči X25519 (RFC 7748, KORENE).
//!
//! Overenie certifikátov a podpisu servera robí [`Overovac`] (krok 4 – X.509, ECDSA, RSA). Bez neho sa spojenie
//! nenadviaže – **neexistuje režim „bez overenia“** mimo skúšok.

use std::io::{self, Read, Write};

use orchidea_kryptografia::aes::AesGcm;
use orchidea_kryptografia::hkdf;
use orchidea_kryptografia::hmac::Hmac;
use orchidea_kryptografia::sha2::{Sha256, Sha384};
use orchidea_kryptografia::{Odtlacok, rovnake};

/// Najväčší obsah záznamu (2¹⁴) a šifrový text (2¹⁴ + 256) – RFC 8446 5.1, 5.2.
const NAJVIAC_OBSAH: usize = 1 << 14;
const NAJVIAC_SIFRA: usize = NAJVIAC_OBSAH + 256;
/// Najväčšia správa výmeny, ktorú klient prijme (reťaz certifikátov).
const NAJVIAC_SPRAVA: usize = 64 * 1024;

const ZAZNAM_CCS: u8 = 20;
const ZAZNAM_UPOZORNENIE: u8 = 21;
const ZAZNAM_VYMENA: u8 = 22;
const ZAZNAM_DATA: u8 = 23;

const CLIENT_HELLO: u8 = 1;
const SERVER_HELLO: u8 = 2;
const NEW_SESSION_TICKET: u8 = 4;
const ENCRYPTED_EXTENSIONS: u8 = 8;
const CERTIFICATE: u8 = 11;
const CERTIFICATE_REQUEST: u8 = 13;
const CERTIFICATE_VERIFY: u8 = 15;
const FINISHED: u8 = 20;
const KEY_UPDATE: u8 = 24;

const ROZ_SERVER_NAME: u16 = 0;
const ROZ_SUPPORTED_GROUPS: u16 = 10;
const ROZ_SIGNATURE_ALGORITHMS: u16 = 13;
const ROZ_ALPN: u16 = 16;
const ROZ_SUPPORTED_VERSIONS: u16 = 43;
const ROZ_KEY_SHARE: u16 = 51;

const SKUPINA_X25519: u16 = 0x001d;
const TLS13: u16 = 0x0304;

/// Náhodná hodnota ServerHello, ktorá znamená HelloRetryRequest (SHA-256 z „HelloRetryRequest“, RFC 8446 4.1.3).
const HRR: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91, 0xc2,
    0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

/// Podpisové schémy, ktoré klient prijme (RFC 8446 4.2.3): ECDSA, RSA-PSS (rsae), v certifikátoch aj PKCS #1 v1.5.
pub const PODPISY: [u16; 8] = [0x0403, 0x0503, 0x0804, 0x0805, 0x0806, 0x0401, 0x0501, 0x0601];

/// Chyba spojenia TLS.
#[derive(Debug)]
pub enum Chyba {
    Spojenie(io::Error),
    /// server porušil protokol (správa, dĺžka, poradie)
    Protokol(&'static str),
    /// server nepodporuje nič z ponuky (TLS 1.3, sada, skupina)
    Nezhoda(&'static str),
    /// overenie certifikátu alebo podpisu zlyhalo
    Overenie(String),
    /// Finished nesedí – niekto upravil výmenu
    ZlaVymena,
    /// server poslal upozornenie (kód podľa RFC 8446 6)
    Upozornenie(u8),
    /// AEAD: zlá značka záznamu
    ZlyZaznam,
}

impl std::fmt::Display for Chyba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Chyba::Spojenie(e) => write!(f, "spojenie: {e}"),
            Chyba::Protokol(c) => write!(f, "server porušil TLS 1.3 ({c})"),
            Chyba::Nezhoda(c) => write!(f, "server nepodporuje {c}"),
            Chyba::Overenie(c) => write!(f, "server sa neoveril: {c}"),
            Chyba::ZlaVymena => write!(f, "výmena kľúčov bola zmenená (Finished nesedí)"),
            Chyba::Upozornenie(k) => write!(f, "server ukončil spojenie (upozornenie TLS {k})"),
            Chyba::ZlyZaznam => write!(f, "zašifrovaný záznam je poškodený"),
        }
    }
}

impl std::error::Error for Chyba {}

impl From<io::Error> for Chyba {
    fn from(e: io::Error) -> Chyba {
        Chyba::Spojenie(e)
    }
}

impl From<Chyba> for io::Error {
    fn from(e: Chyba) -> io::Error {
        match e {
            Chyba::Spojenie(e) => e,
            iny => io::Error::new(io::ErrorKind::InvalidData, iny.to_string()),
        }
    }
}

/// Verejný kľúč servera z certifikátu (vráti [`Overovac::over_retaz`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerejnyKluc {
    /// OID algoritmu (napr. „1.2.840.10045.2.1“ pre EC, „1.2.840.113549.1.1.1“ pre RSA) a parametre
    pub algoritmus: Vec<u8>,
    pub parametre: Vec<u8>,
    /// subjectPublicKey (bez bitu nevyužitých bitov)
    pub kluc: Vec<u8>,
}

/// Overenie servera: reťaz certifikátov pre meno a podpis CertificateVerify.
pub trait Overovac {
    /// `retaz[0]` = certifikát servera (DER), ďalej sprostredkujúce; vráti verejný kľúč servera.
    fn over_retaz(&self, retaz: &[Vec<u8>], meno: &str) -> Result<VerejnyKluc, String>;
    /// Podpis `podpis` schémou `schema` (RFC 8446 4.2.3) nad `sprava` kľúčom `kluc`.
    fn over_podpis(
        &self,
        kluc: &VerejnyKluc,
        schema: u16,
        sprava: &[u8],
        podpis: &[u8],
    ) -> Result<(), String>;
}

/// Odtlačok sady (SHA-256 alebo SHA-384).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hash {
    S256,
    S384,
}

impl Hash {
    fn dlzka(self) -> usize {
        match self {
            Hash::S256 => 32,
            Hash::S384 => 48,
        }
    }
    fn z(self, d: &[u8]) -> Vec<u8> {
        match self {
            Hash::S256 => Sha256::z(d),
            Hash::S384 => Sha384::z(d),
        }
    }
    fn hmac(self, k: &[u8], d: &[u8]) -> Vec<u8> {
        match self {
            Hash::S256 => Hmac::<Sha256>::z(k, d),
            Hash::S384 => Hmac::<Sha384>::z(k, d),
        }
    }
    fn extrahuj(self, sol: &[u8], ikm: &[u8]) -> Vec<u8> {
        match self {
            Hash::S256 => hkdf::extrahuj::<Sha256>(sol, ikm),
            Hash::S384 => hkdf::extrahuj::<Sha384>(sol, ikm),
        }
    }
    fn menovka(self, tajomstvo: &[u8], menovka: &str, kontext: &[u8], dlzka: usize) -> Vec<u8> {
        match self {
            Hash::S256 => hkdf::rozsir_menovku::<Sha256>(tajomstvo, menovka, kontext, dlzka),
            Hash::S384 => hkdf::rozsir_menovku::<Sha384>(tajomstvo, menovka, kontext, dlzka),
        }
    }
    /// Derive-Secret (RFC 8446 7.1) s hotovým odtlačkom prepisu.
    fn odvod(self, tajomstvo: &[u8], menovka: &str, prepis: &[u8]) -> Vec<u8> {
        self.menovka(tajomstvo, menovka, prepis, self.dlzka())
    }
}

/// Prepis výmeny (transcript) – odtlačok všetkých správ výmeny v poradí.
#[derive(Debug, Clone)]
enum Prepis {
    S256(Sha256),
    S384(Sha384),
}

impl Prepis {
    fn novy(h: Hash) -> Prepis {
        match h {
            Hash::S256 => Prepis::S256(Sha256::novy()),
            Hash::S384 => Prepis::S384(Sha384::novy()),
        }
    }
    fn pridaj(&mut self, d: &[u8]) {
        match self {
            Prepis::S256(x) => x.pridaj(d),
            Prepis::S384(x) => x.pridaj(d),
        }
    }
    fn odtlacok(&self) -> Vec<u8> {
        match self {
            Prepis::S256(x) => x.clone().hotovo(),
            Prepis::S384(x) => x.clone().hotovo(),
        }
    }
}

/// Šifrovacia sada TLS 1.3 (RFC 8446 B.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sada {
    Aes128Gcm = 0x1301,
    Aes256Gcm = 0x1302,
    Chacha20Poly1305 = 0x1303,
}

impl Sada {
    fn z(c: u16) -> Option<Sada> {
        match c {
            0x1301 => Some(Sada::Aes128Gcm),
            0x1302 => Some(Sada::Aes256Gcm),
            0x1303 => Some(Sada::Chacha20Poly1305),
            _ => None,
        }
    }
    fn hash(self) -> Hash {
        if self == Sada::Aes256Gcm { Hash::S384 } else { Hash::S256 }
    }
    fn dlzka_kluca(self) -> usize {
        if self == Sada::Aes128Gcm { 16 } else { 32 }
    }
    /// Meno sady pre záznamy.
    pub fn meno(self) -> &'static str {
        match self {
            Sada::Aes128Gcm => "TLS_AES_128_GCM_SHA256",
            Sada::Aes256Gcm => "TLS_AES_256_GCM_SHA384",
            Sada::Chacha20Poly1305 => "TLS_CHACHA20_POLY1305_SHA256",
        }
    }
}

/// Poradie ponuky sád.
const PONUKA: [Sada; 3] = [Sada::Chacha20Poly1305, Sada::Aes128Gcm, Sada::Aes256Gcm];

#[derive(Clone)]
enum Sifra {
    Aes(AesGcm),
    Chacha([u8; 32]),
}

/// Kľúče jedného smeru: šifra, IV a poradové číslo záznamu (RFC 8446 5.3).
#[derive(Clone)]
struct Smer {
    sada: Sada,
    tajomstvo: Vec<u8>,
    sifra: Sifra,
    iv: [u8; 12],
    poradie: u64,
}

impl std::fmt::Debug for Smer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Smer({}, záznam {}, kľúč skrytý)", self.sada.meno(), self.poradie)
    }
}

impl Smer {
    /// Kľúče prenosu z tajomstva (RFC 8446 7.3).
    fn z_tajomstva(sada: Sada, tajomstvo: Vec<u8>) -> Smer {
        let h = sada.hash();
        let kluc = h.menovka(&tajomstvo, "key", &[], sada.dlzka_kluca());
        let iv: [u8; 12] = h.menovka(&tajomstvo, "iv", &[], 12).try_into().unwrap_or([0; 12]);
        let sifra = match sada {
            Sada::Chacha20Poly1305 => Sifra::Chacha(kluc.as_slice().try_into().unwrap_or([0; 32])),
            _ => Sifra::Aes(AesGcm::novy(&kluc).unwrap_or_else(unreachable_aes)),
        };
        Smer { sada, tajomstvo, sifra, iv, poradie: 0 }
    }

    /// KeyUpdate: nové tajomstvo „traffic upd“ (RFC 8446 7.2).
    fn aktualizuj(&mut self) {
        let h = self.sada.hash();
        let nove = h.menovka(&self.tajomstvo, "traffic upd", &[], h.dlzka());
        *self = Smer::z_tajomstva(self.sada, nove);
    }

    fn nonce(&self) -> [u8; 12] {
        let mut n = self.iv;
        for (x, b) in n[4..].iter_mut().zip(self.poradie.to_be_bytes()) {
            *x ^= b;
        }
        n
    }

    /// Zašifruje vnútorný obsah (obsah ‖ typ) do celého záznamu.
    fn zasifruj(&mut self, typ: u8, obsah: &[u8]) -> Vec<u8> {
        let mut vnutro = Vec::with_capacity(obsah.len() + 17);
        vnutro.extend_from_slice(obsah);
        vnutro.push(typ);
        let dlzka = vnutro.len() + 16;
        let hlavicka = [ZAZNAM_DATA, 3, 3, (dlzka >> 8) as u8, dlzka as u8];
        let nonce = self.nonce();
        let znacka = match &self.sifra {
            Sifra::Aes(g) => g.zasifruj(&nonce, &hlavicka, &mut vnutro),
            Sifra::Chacha(k) => sklenik_korene::zasifruj(k, &nonce, &hlavicka, &mut vnutro),
        };
        self.poradie += 1;
        let mut z = hlavicka.to_vec();
        z.extend_from_slice(&vnutro);
        z.extend_from_slice(&znacka);
        z
    }

    /// Dešifruje telo záznamu; vráti (skutočný typ, obsah) – nulová výplň sa odstráni (RFC 8446 5.4).
    fn desifruj(&mut self, hlavicka: &[u8; 5], telo: &[u8]) -> Result<(u8, Vec<u8>), Chyba> {
        if telo.len() < 17 {
            return Err(Chyba::Protokol("príliš krátky zašifrovaný záznam"));
        }
        let (data, znacka) = telo.split_at(telo.len() - 16);
        let mut d = data.to_vec();
        let nonce = self.nonce();
        let ok = match &self.sifra {
            Sifra::Aes(g) => g.desifruj(&nonce, hlavicka, &mut d, znacka).is_ok(),
            Sifra::Chacha(k) => {
                let z: [u8; 16] = znacka.try_into().map_err(|_| Chyba::ZlyZaznam)?;
                sklenik_korene::desifruj(k, &nonce, hlavicka, &mut d, &z).is_ok()
            }
        };
        if !ok {
            return Err(Chyba::ZlyZaznam);
        }
        self.poradie += 1;
        while d.last() == Some(&0) {
            d.pop();
        }
        let typ = d.pop().ok_or(Chyba::Protokol("záznam bez typu"))?;
        Ok((typ, d))
    }
}

fn unreachable_aes() -> AesGcm {
    // dĺžka kľúča je daná sadou (16 alebo 32 B) – AesGcm::novy nezlyhá
    AesGcm::novy(&[0; 16]).unwrap_or_else(|| unreachable!("AES-128 kľúč 16 B"))
}

// ---- pomocné kódovanie (RFC 8446 3: vektory s dĺžkou 1, 2 alebo 3 B) ----

fn s_dlzkou2(v: &mut Vec<u8>, data: &[u8]) {
    v.extend_from_slice(&(data.len() as u16).to_be_bytes());
    v.extend_from_slice(data);
}

fn rozsirenie(v: &mut Vec<u8>, typ: u16, data: &[u8]) {
    v.extend_from_slice(&typ.to_be_bytes());
    s_dlzkou2(v, data);
}

fn sprava_vymeny(typ: u8, telo: &[u8]) -> Vec<u8> {
    let mut v = vec![typ, (telo.len() >> 16) as u8, (telo.len() >> 8) as u8, telo.len() as u8];
    v.extend_from_slice(telo);
    v
}

/// Čítačka bajtov s kontrolou presahu.
struct Citac<'a> {
    d: &'a [u8],
    i: usize,
}

impl<'a> Citac<'a> {
    fn novy(d: &'a [u8]) -> Citac<'a> {
        Citac { d, i: 0 }
    }
    fn bajty(&mut self, n: usize) -> Result<&'a [u8], Chyba> {
        let b = self.d.get(self.i..self.i + n).ok_or(Chyba::Protokol("správa skrátená"))?;
        self.i += n;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8, Chyba> {
        Ok(self.bajty(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Chyba> {
        let b = self.bajty(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u24(&mut self) -> Result<usize, Chyba> {
        let b = self.bajty(3)?;
        Ok(((b[0] as usize) << 16) | ((b[1] as usize) << 8) | b[2] as usize)
    }
    fn vektor1(&mut self) -> Result<&'a [u8], Chyba> {
        let n = self.u8()? as usize;
        self.bajty(n)
    }
    fn vektor2(&mut self) -> Result<&'a [u8], Chyba> {
        let n = self.u16()? as usize;
        self.bajty(n)
    }
    fn vektor3(&mut self) -> Result<&'a [u8], Chyba> {
        let n = self.u24()?;
        self.bajty(n)
    }
    fn koniec(&self) -> bool {
        self.i == self.d.len()
    }
}

/// ClientHello (RFC 8446 4.1.2) – `nahodne` = 32 B náhody, `relacia` = 32 B legacy_session_id (D.4),
/// `verejny` = kľúč X25519. SNI len pre meno (nie číselnú adresu – RFC 6066 3).
pub fn client_hello(meno: &str, nahodne: &[u8; 32], relacia: &[u8; 32], verejny: &[u8; 32]) -> Vec<u8> {
    let mut t = Vec::with_capacity(256);
    t.extend_from_slice(&0x0303u16.to_be_bytes());
    t.extend_from_slice(nahodne);
    t.push(32);
    t.extend_from_slice(relacia);
    let sady: Vec<u8> = PONUKA.iter().flat_map(|s| (*s as u16).to_be_bytes()).collect();
    s_dlzkou2(&mut t, &sady);
    t.extend_from_slice(&[1, 0]);
    let mut r = Vec::with_capacity(128);
    if meno.parse::<std::net::IpAddr>().is_err() {
        let mut zoznam = vec![0u8];
        s_dlzkou2(&mut zoznam, meno.as_bytes());
        let mut sni = Vec::new();
        s_dlzkou2(&mut sni, &zoznam);
        rozsirenie(&mut r, ROZ_SERVER_NAME, &sni);
    }
    let mut skupiny = Vec::new();
    s_dlzkou2(&mut skupiny, &SKUPINA_X25519.to_be_bytes());
    rozsirenie(&mut r, ROZ_SUPPORTED_GROUPS, &skupiny);
    let mut podpisy = Vec::new();
    s_dlzkou2(&mut podpisy, &PODPISY.iter().flat_map(|p| p.to_be_bytes()).collect::<Vec<u8>>());
    rozsirenie(&mut r, ROZ_SIGNATURE_ALGORITHMS, &podpisy);
    let mut alpn = Vec::new();
    let mut protokol = vec![8u8];
    protokol.extend_from_slice(b"http/1.1");
    s_dlzkou2(&mut alpn, &protokol);
    rozsirenie(&mut r, ROZ_ALPN, &alpn);
    rozsirenie(&mut r, ROZ_SUPPORTED_VERSIONS, &[2, 0x03, 0x04]);
    let mut podiel = SKUPINA_X25519.to_be_bytes().to_vec();
    s_dlzkou2(&mut podiel, verejny);
    let mut podiely = Vec::new();
    s_dlzkou2(&mut podiely, &podiel);
    rozsirenie(&mut r, ROZ_KEY_SHARE, &podiely);
    s_dlzkou2(&mut t, &r);
    sprava_vymeny(CLIENT_HELLO, &t)
}

/// Čo klient potrebuje zo ServerHello.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHello {
    pub sada: Sada,
    pub verejny: [u8; 32],
}

/// Rozbor ServerHello (RFC 8446 4.1.3) – telo bez hlavičky správy.
fn precitaj_server_hello(telo: &[u8], relacia: &[u8; 32]) -> Result<ServerHello, Chyba> {
    let mut c = Citac::novy(telo);
    if c.u16()? != 0x0303 {
        return Err(Chyba::Protokol("legacy_version"));
    }
    if c.bajty(32)? == HRR {
        return Err(Chyba::Nezhoda("X25519 (server žiada inú skupinu – HelloRetryRequest)"));
    }
    if c.vektor1()? != relacia {
        return Err(Chyba::Protokol("legacy_session_id_echo"));
    }
    let sada = Sada::z(c.u16()?).ok_or(Chyba::Protokol("sada mimo ponuky"))?;
    if c.u8()? != 0 {
        return Err(Chyba::Protokol("kompresia"));
    }
    let mut r = Citac::novy(c.vektor2()?);
    let (mut verzia, mut kluc) = (None, None);
    while !r.koniec() {
        let typ = r.u16()?;
        let mut d = Citac::novy(r.vektor2()?);
        match typ {
            ROZ_SUPPORTED_VERSIONS => verzia = Some(d.u16()?),
            ROZ_KEY_SHARE => {
                if d.u16()? != SKUPINA_X25519 {
                    return Err(Chyba::Protokol("key_share mimo ponuky"));
                }
                let k: [u8; 32] =
                    d.vektor2()?.try_into().map_err(|_| Chyba::Protokol("dĺžka kľúča X25519"))?;
                kluc = Some(k);
            }
            _ => {}
        }
    }
    if verzia != Some(TLS13) {
        return Err(Chyba::Nezhoda("TLS 1.3"));
    }
    Ok(ServerHello { sada, verejny: kluc.ok_or(Chyba::Protokol("chýba key_share"))? })
}

/// Tajomstvá výmeny (RFC 8446 7.1) po ServerHello.
struct Rozvrh {
    hash: Hash,
    vymena: Vec<u8>,
}

impl Rozvrh {
    /// Handshake Secret = HKDF-Extract(Derive-Secret(Early Secret, „derived“, „“), (EC)DHE); bez PSK.
    fn novy(hash: Hash, ecdhe: &[u8]) -> Rozvrh {
        let nuly = vec![0u8; hash.dlzka()];
        let skore = hash.extrahuj(&[], &nuly);
        let odvodene = hash.odvod(&skore, "derived", &hash.z(b""));
        Rozvrh { hash, vymena: hash.extrahuj(&odvodene, ecdhe) }
    }

    fn hlavne(&self) -> Vec<u8> {
        let h = self.hash;
        let odvodene = h.odvod(&self.vymena, "derived", &h.z(b""));
        h.extrahuj(&odvodene, &vec![0u8; h.dlzka()])
    }
}

/// Spojenie TLS 1.3 nad prúdom `S` – po [`nadviaz`] je to `Read + Write` pre aplikačné dáta.
pub struct Tls<S> {
    prud: S,
    citanie: Smer,
    zapis: Smer,
    /// prijaté, ešte neprečítané aplikačné dáta
    prijate: Vec<u8>,
    precitane: usize,
    /// rozpracované správy výmeny po nadviazaní (NewSessionTicket, KeyUpdate)
    vymena: Vec<u8>,
    koniec: bool,
    pub sada: Sada,
    /// dohodnutý protokol (ALPN), ak ho server zvolil
    pub alpn: Option<String>,
}

impl<S> std::fmt::Debug for Tls<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tls({}, čítanie {:?}, zápis {:?})", self.sada.meno(), self.citanie, self.zapis)
    }
}

/// Prečíta jeden záznam: (typ, hlavička, telo).
fn citaj_zaznam<S: Read>(s: &mut S) -> Result<(u8, [u8; 5], Vec<u8>), Chyba> {
    let mut h = [0u8; 5];
    s.read_exact(&mut h)?;
    let n = u16::from_be_bytes([h[3], h[4]]) as usize;
    if n > NAJVIAC_SIFRA {
        return Err(Chyba::Protokol("záznam dlhší ako 2^14 + 256"));
    }
    let mut telo = vec![0u8; n];
    s.read_exact(&mut telo)?;
    Ok((h[0], h, telo))
}

fn upozornenie(obsah: &[u8]) -> Chyba {
    match obsah {
        [_, 0] => Chyba::Spojenie(io::Error::new(io::ErrorKind::UnexpectedEof, "server zavrel spojenie")),
        [_, kod] => Chyba::Upozornenie(*kod),
        _ => Chyba::Protokol("upozornenie"),
    }
}

/// Nadviaže spojenie TLS 1.3 so serverom `meno` cez hotový prúd (TCP). `nahoda` naplní bajty náhodou z jadra –
/// spolu 96 B (tajný kľúč X25519, náhodná hodnota ClientHello, legacy_session_id).
pub fn nadviaz<S: Read + Write>(
    mut prud: S,
    meno: &str,
    overovac: &dyn Overovac,
    nahoda: &mut dyn FnMut(&mut [u8]),
) -> Result<Tls<S>, Chyba> {
    let mut tajny = [0u8; 32];
    nahoda(&mut tajny);
    let verejny = sklenik_korene::verejny_kluc(&tajny);
    let mut nahodne = [0u8; 32];
    nahoda(&mut nahodne);
    let mut relacia = [0u8; 32];
    nahoda(&mut relacia);
    let ch = client_hello(meno, &nahodne, &relacia, &verejny);
    let mut z = vec![ZAZNAM_VYMENA, 3, 1, (ch.len() >> 8) as u8, ch.len() as u8];
    z.extend_from_slice(&ch);
    prud.write_all(&z)?;
    prud.flush()?;

    // ServerHello (nešifrovaný) – môže prísť aj CCS z režimu kompatibility
    let mut buf = Vec::new();
    let sh_sprava = loop {
        let (typ, _, telo) = citaj_zaznam(&mut prud)?;
        match typ {
            ZAZNAM_VYMENA => buf.extend_from_slice(&telo),
            ZAZNAM_UPOZORNENIE => return Err(upozornenie(&telo)),
            ZAZNAM_CCS => continue,
            _ => return Err(Chyba::Protokol("očakávaný ServerHello")),
        }
        if buf.len() >= 4 {
            let n = ((buf[1] as usize) << 16) | ((buf[2] as usize) << 8) | buf[3] as usize;
            if buf[0] != SERVER_HELLO {
                return Err(Chyba::Protokol("očakávaný ServerHello"));
            }
            if buf.len() >= 4 + n {
                let s: Vec<u8> = buf.drain(..4 + n).collect();
                break s;
            }
        }
    };
    if !buf.is_empty() {
        return Err(Chyba::Protokol("nešifrované údaje po ServerHello"));
    }
    let sh = precitaj_server_hello(&sh_sprava[4..], &relacia)?;
    let hash = sh.sada.hash();
    let mut prepis = Prepis::novy(hash);
    prepis.pridaj(&ch);
    prepis.pridaj(&sh_sprava);
    let spolocne = sklenik_korene::spolocne_tajomstvo(&tajny, &sh.verejny)
        .ok_or(Chyba::Protokol("kľúč X25519 servera je nulový"))?;
    tajny.fill(0);
    let rozvrh = Rozvrh::novy(hash, &spolocne);
    let po_sh = prepis.odtlacok();
    let c_hs = hash.odvod(&rozvrh.vymena, "c hs traffic", &po_sh);
    let s_hs = hash.odvod(&rozvrh.vymena, "s hs traffic", &po_sh);
    let mut citanie = Smer::z_tajomstva(sh.sada, s_hs.clone());
    let mut zapis_hs = Smer::z_tajomstva(sh.sada, c_hs.clone());

    // zašifrované správy servera: EncryptedExtensions, [CertificateRequest], Certificate, CertificateVerify, Finished
    let mut kluc_servera: Option<VerejnyKluc> = None;
    let mut ziada_certifikat: Option<Vec<u8>> = None;
    let mut alpn = None;
    let mut ocakavam = ENCRYPTED_EXTENSIONS;
    let mut vymena: Vec<u8> = Vec::new();
    'vymena: loop {
        let (typ, h, telo) = citaj_zaznam(&mut prud)?;
        match typ {
            ZAZNAM_CCS => continue,
            ZAZNAM_UPOZORNENIE => return Err(upozornenie(&telo)),
            ZAZNAM_DATA => {}
            _ => return Err(Chyba::Protokol("nešifrovaný záznam počas výmeny")),
        }
        let (vnutorny, obsah) = citanie.desifruj(&h, &telo)?;
        match vnutorny {
            ZAZNAM_VYMENA => vymena.extend_from_slice(&obsah),
            ZAZNAM_UPOZORNENIE => return Err(upozornenie(&obsah)),
            _ => return Err(Chyba::Protokol("očakávaná správa výmeny")),
        }
        if vymena.len() > NAJVIAC_SPRAVA + 4 {
            return Err(Chyba::Protokol("správa výmeny je príliš dlhá"));
        }
        while vymena.len() >= 4 {
            let n = ((vymena[1] as usize) << 16) | ((vymena[2] as usize) << 8) | vymena[3] as usize;
            if n > NAJVIAC_SPRAVA {
                return Err(Chyba::Protokol("správa výmeny je príliš dlhá"));
            }
            if vymena.len() < 4 + n {
                break;
            }
            let sprava: Vec<u8> = vymena.drain(..4 + n).collect();
            let (druh, telo) = (sprava[0], &sprava[4..]);
            match (ocakavam, druh) {
                (ENCRYPTED_EXTENSIONS, ENCRYPTED_EXTENSIONS) => {
                    let mut c = Citac::novy(telo);
                    let mut r = Citac::novy(c.vektor2()?);
                    while !r.koniec() {
                        let t = r.u16()?;
                        let d = r.vektor2()?;
                        if t == ROZ_ALPN {
                            let mut a = Citac::novy(d);
                            let mut zoznam = Citac::novy(a.vektor2()?);
                            alpn = Some(String::from_utf8_lossy(zoznam.vektor1()?).into_owned());
                        }
                    }
                    ocakavam = CERTIFICATE;
                }
                (CERTIFICATE, CERTIFICATE_REQUEST) => {
                    let mut c = Citac::novy(telo);
                    ziada_certifikat = Some(c.vektor1()?.to_vec());
                }
                (CERTIFICATE, CERTIFICATE) => {
                    let mut c = Citac::novy(telo);
                    let _kontext = c.vektor1()?;
                    let mut zoznam = Citac::novy(c.vektor3()?);
                    let mut retaz = Vec::new();
                    while !zoznam.koniec() {
                        retaz.push(zoznam.vektor3()?.to_vec());
                        let _rozsirenia = zoznam.vektor2()?;
                    }
                    if retaz.is_empty() {
                        return Err(Chyba::Overenie("server neposlal certifikát".into()));
                    }
                    kluc_servera = Some(overovac.over_retaz(&retaz, meno).map_err(Chyba::Overenie)?);
                    ocakavam = CERTIFICATE_VERIFY;
                }
                (CERTIFICATE_VERIFY, CERTIFICATE_VERIFY) => {
                    let mut c = Citac::novy(telo);
                    let schema = c.u16()?;
                    let podpis = c.vektor2()?;
                    if !PODPISY.contains(&schema) || matches!(schema, 0x0401 | 0x0501 | 0x0601) {
                        // PKCS #1 v1.5 smie byť len v certifikátoch, nie v CertificateVerify (RFC 8446 4.2.3)
                        return Err(Chyba::Overenie(format!("podpisová schéma 0x{schema:04x}")));
                    }
                    let mut obsah = vec![0x20u8; 64];
                    obsah.extend_from_slice(b"TLS 1.3, server CertificateVerify\0");
                    obsah.extend_from_slice(&prepis.odtlacok());
                    let k =
                        kluc_servera.as_ref().ok_or(Chyba::Protokol("CertificateVerify bez certifikátu"))?;
                    overovac.over_podpis(k, schema, &obsah, podpis).map_err(Chyba::Overenie)?;
                    ocakavam = FINISHED;
                }
                (FINISHED, FINISHED) => {
                    let kluc = hash.menovka(&s_hs, "finished", &[], hash.dlzka());
                    let ocakavane = hash.hmac(&kluc, &prepis.odtlacok());
                    if !rovnake(&ocakavane, telo) {
                        return Err(Chyba::ZlaVymena);
                    }
                    prepis.pridaj(&sprava);
                    break 'vymena;
                }
                _ => return Err(Chyba::Protokol("neočakávaná správa výmeny")),
            }
            prepis.pridaj(&sprava);
        }
    }
    if !vymena.is_empty() {
        return Err(Chyba::Protokol("údaje za Finished servera v tom istom kľúči"));
    }
    // aplikačné kľúče z prepisu po Finished servera
    let po_fin = prepis.odtlacok();
    let hlavne = rozvrh.hlavne();
    let c_ap = hash.odvod(&hlavne, "c ap traffic", &po_fin);
    let s_ap = hash.odvod(&hlavne, "s ap traffic", &po_fin);

    // druhý let klienta: CCS (D.4), [prázdny Certificate], Finished – kľúčmi výmeny
    let mut let2 = vec![ZAZNAM_CCS, 3, 3, 0, 1, 1];
    if let Some(kontext) = ziada_certifikat {
        let mut t = Vec::new();
        t.push(kontext.len() as u8);
        t.extend_from_slice(&kontext);
        t.extend_from_slice(&[0, 0, 0]);
        let sprava = sprava_vymeny(CERTIFICATE, &t);
        prepis.pridaj(&sprava);
        let2.extend(zapis_hs.zasifruj(ZAZNAM_VYMENA, &sprava));
    }
    let kluc = hash.menovka(&c_hs, "finished", &[], hash.dlzka());
    let fin = sprava_vymeny(FINISHED, &hash.hmac(&kluc, &prepis.odtlacok()));
    let2.extend(zapis_hs.zasifruj(ZAZNAM_VYMENA, &fin));
    prud.write_all(&let2)?;
    prud.flush()?;

    Ok(Tls {
        prud,
        citanie: Smer::z_tajomstva(sh.sada, s_ap),
        zapis: Smer::z_tajomstva(sh.sada, c_ap),
        prijate: Vec::new(),
        precitane: 0,
        vymena: Vec::new(),
        koniec: false,
        sada: sh.sada,
        alpn,
    })
}

impl<S: Read + Write> Tls<S> {
    /// Spracuje správy výmeny po nadviazaní: NewSessionTicket sa ignoruje, KeyUpdate obnoví kľúč (a odpovie).
    fn po_vymene(&mut self, obsah: &[u8]) -> Result<(), Chyba> {
        self.vymena.extend_from_slice(obsah);
        while self.vymena.len() >= 4 {
            let n = ((self.vymena[1] as usize) << 16)
                | ((self.vymena[2] as usize) << 8)
                | self.vymena[3] as usize;
            if n > NAJVIAC_SPRAVA {
                return Err(Chyba::Protokol("správa po výmene je príliš dlhá"));
            }
            if self.vymena.len() < 4 + n {
                break;
            }
            let sprava: Vec<u8> = self.vymena.drain(..4 + n).collect();
            match sprava[0] {
                NEW_SESSION_TICKET => {}
                KEY_UPDATE => {
                    let ziada = sprava.get(4) == Some(&1);
                    self.citanie.aktualizuj();
                    if ziada {
                        let z = self.zapis.zasifruj(ZAZNAM_VYMENA, &sprava_vymeny(KEY_UPDATE, &[0]));
                        self.prud.write_all(&z)?;
                        self.zapis.aktualizuj();
                    }
                }
                _ => return Err(Chyba::Protokol("neočakávaná správa po výmene")),
            }
        }
        Ok(())
    }

    /// Pošle upozornenie close_notify (RFC 8446 6.1).
    pub fn zavri(&mut self) -> io::Result<()> {
        let z = self.zapis.zasifruj(ZAZNAM_UPOZORNENIE, &[1, 0]);
        self.prud.write_all(&z)?;
        self.prud.flush()
    }
}

impl<S: Read + Write> Read for Tls<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.precitane >= self.prijate.len() {
            if self.koniec {
                return Ok(0);
            }
            let (typ, h, telo) = match citaj_zaznam(&mut self.prud) {
                Ok(z) => z,
                Err(Chyba::Spojenie(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    // spojenie skončilo bez close_notify – pri HTTP s dĺžkou tela to rozhodne vyššia vrstva
                    self.koniec = true;
                    return Ok(0);
                }
                Err(e) => return Err(e.into()),
            };
            if typ == ZAZNAM_CCS {
                continue;
            }
            if typ != ZAZNAM_DATA {
                return Err(Chyba::Protokol("nešifrovaný záznam po výmene").into());
            }
            let (vnutorny, obsah) = self.citanie.desifruj(&h, &telo)?;
            match vnutorny {
                ZAZNAM_DATA => {
                    self.prijate = obsah;
                    self.precitane = 0;
                }
                ZAZNAM_VYMENA => self.po_vymene(&obsah)?,
                ZAZNAM_UPOZORNENIE => match upozornenie(&obsah) {
                    Chyba::Spojenie(_) => {
                        self.koniec = true;
                        return Ok(0);
                    }
                    e => return Err(e.into()),
                },
                _ => return Err(Chyba::Protokol("neznámy typ záznamu").into()),
            }
        }
        let n = buf.len().min(self.prijate.len() - self.precitane);
        buf[..n].copy_from_slice(&self.prijate[self.precitane..self.precitane + n]);
        self.precitane += n;
        Ok(n)
    }
}

impl<S: Read + Write> Write for Tls<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len().min(NAJVIAC_OBSAH);
        let z = self.zapis.zasifruj(ZAZNAM_DATA, &buf[..n]);
        self.prud.write_all(&z)?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.prud.flush()
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_kryptografia::sha2::z_hex;

    // RFC 8448, kap. 3 „Simple 1-RTT Handshake“
    const CH: &str = "010000c00303cb34ecb1e78163ba1c38c6dacb196a6dffa21a8d9912ec18a2ef6283024dece7000006130113031302\
        010000910000000b0009000006736572766572ff01000100000a00140012001d0017001800190100010101020103010400230000\
        003300260024001d002099381de560e4bd43d23d8e435a7dbafeb3c06e51c13cae4d5413691e529aaf2c002b0003020304000d00\
        20001e040305030603020308040805080604010501060102010402050206020202002d00020101001c00024001";
    const SH: &str = "020000560303a6af06a4121860dc5e6e60249cd34c95930c8ac5cb1434dac155772ed3e2692800130100002e0033002400\
        1d0020c9828876112095fe66762bdbf7c672e156d6cc253b833df1dd69b1b04e751f0f002b00020304";
    const KLIENT_TAJNY: &str = "49af42ba7f7994852d713ef2784bcbcaa7911de26adc5642cb634540e7ea5005";

    #[test]
    fn rozvrh_klucov_rfc8448() {
        let ch = z_hex(CH);
        let sh = z_hex(SH);
        assert_eq!((ch.len(), sh.len()), (196, 90), "dĺžky správ podľa RFC 8448");
        let tajny: [u8; 32] = z_hex(KLIENT_TAJNY).try_into().unwrap();
        let relacia = [0u8; 32];
        // ServerHello z RFC 8448 má prázdne legacy_session_id_echo – rozbor tela overí len sadu a kľúč
        let mut telo = sh[4..].to_vec();
        assert_eq!(telo[34], 0, "prázdne session id");
        telo.splice(34..35, std::iter::once(32u8).chain(relacia));
        let s = precitaj_server_hello(&telo, &relacia).unwrap();
        assert_eq!(s.sada, Sada::Aes128Gcm);
        let spolocne = sklenik_korene::spolocne_tajomstvo(&tajny, &s.verejny).unwrap();
        let r = Rozvrh::novy(Hash::S256, &spolocne);
        assert_eq!(
            r.vymena,
            z_hex("1dc826e93606aa6fdc0aadc12f741b01046aa6b99f691ed221a9f0ca043fbeac"),
            "handshake secret"
        );
        let mut p = Prepis::novy(Hash::S256);
        p.pridaj(&ch);
        p.pridaj(&sh);
        let o = p.odtlacok();
        assert_eq!(
            Hash::S256.odvod(&r.vymena, "c hs traffic", &o),
            z_hex("b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21")
        );
        let s_hs = Hash::S256.odvod(&r.vymena, "s hs traffic", &o);
        assert_eq!(s_hs, z_hex("b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38"));
        let k = Hash::S256.menovka(&s_hs, "key", &[], 16);
        let iv = Hash::S256.menovka(&s_hs, "iv", &[], 12);
        assert_eq!((k, iv), (z_hex("3fce516009c21727d0f2e4e86ee403bc"), z_hex("5d313eb2671276ee13000b30")));
        assert_eq!(
            r.hlavne(),
            z_hex("18df06843d13a08bf2a449844c5f8a478001bc4d4c627984d5a41da8d0402919"),
            "master"
        );
    }

    #[test]
    fn client_hello_tvar() {
        let ch = client_hello("example.com", &[1; 32], &[2; 32], &[3; 32]);
        assert_eq!(ch[0], CLIENT_HELLO);
        let n = ((ch[1] as usize) << 16) | ((ch[2] as usize) << 8) | ch[3] as usize;
        assert_eq!(n + 4, ch.len());
        let mut c = Citac::novy(&ch[4..]);
        assert_eq!(c.u16().unwrap(), 0x0303);
        assert_eq!(c.bajty(32).unwrap(), &[1; 32]);
        assert_eq!(c.vektor1().unwrap(), &[2; 32]);
        assert_eq!(c.vektor2().unwrap(), &[0x13, 0x03, 0x13, 0x01, 0x13, 0x02], "ChaCha20 prvá");
        assert_eq!(c.vektor1().unwrap(), &[0]);
        let mut r = Citac::novy(c.vektor2().unwrap());
        assert!(c.koniec());
        let mut typy = Vec::new();
        while !r.koniec() {
            let t = r.u16().unwrap();
            let d = r.vektor2().unwrap();
            if t == ROZ_SERVER_NAME {
                assert_eq!(&d[5..], b"example.com");
            }
            if t == ROZ_KEY_SHARE {
                assert_eq!(&d[2..6], &[0x00, 0x1d, 0x00, 0x20]);
                assert_eq!(&d[6..], &[3; 32]);
            }
            typy.push(t);
        }
        assert_eq!(typy, [0, 10, 13, 16, 43, 51]);
        let bez_sni = client_hello("10.0.0.1", &[1; 32], &[2; 32], &[3; 32]);
        assert!(bez_sni.len() < ch.len(), "číselná adresa bez SNI");
    }

    #[test]
    fn zaznam_tam_a_spat_a_poskodenie() {
        for sada in PONUKA {
            let tajomstvo = vec![7u8; sada.hash().dlzka()];
            let mut a = Smer::z_tajomstva(sada, tajomstvo.clone());
            let mut b = Smer::z_tajomstva(sada, tajomstvo);
            for sprava in [&b"GET / HTTP/1.1\r\n\r\n"[..], b"", &[0xab; 300]] {
                let z = a.zasifruj(ZAZNAM_DATA, sprava);
                let h: [u8; 5] = z[..5].try_into().unwrap();
                assert_eq!(&h[..3], &[23, 3, 3]);
                let (t, o) = b.desifruj(&h, &z[5..]).unwrap();
                assert_eq!((t, o.as_slice()), (ZAZNAM_DATA, sprava));
            }
            let mut z = a.zasifruj(ZAZNAM_DATA, b"tajne");
            let h: [u8; 5] = z[..5].try_into().unwrap();
            z[7] ^= 1;
            assert!(matches!(b.clone().desifruj(&h, &z[5..]), Err(Chyba::ZlyZaznam)));
            // poradové číslo: ten istý záznam druhý raz sa nedešifruje (iný nonce)
            let z = a.zasifruj(ZAZNAM_DATA, b"raz");
            let h: [u8; 5] = z[..5].try_into().unwrap();
            let mut c = b.clone();
            assert!(c.desifruj(&h, &z[5..]).is_err(), "c je o záznam pozadu");
        }
    }

    fn urandom(b: &mut [u8]) {
        std::fs::File::open("/dev/urandom").unwrap().read_exact(b).unwrap();
    }

    /// Spustí program a skončí skúšku, ak zlyhá.
    fn prikaz(d: &std::path::Path, args: &[&str]) {
        let ok = std::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(d)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(ok.success(), "{args:?}");
    }

    /// Reťaz pre živú skúšku vytvorí program openssl (rozhodca): koreň P-384 → sprostredkujúca RSA 2048 → listy
    /// P-256 a RSA 2048 pre skuska.orchidea.
    fn retaz_openssl(d: &std::path::Path) {
        let ca = "basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n";
        let list = "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n\
                    extendedKeyUsage=serverAuth\nsubjectAltName=DNS:skuska.orchidea\n";
        std::fs::write(d.join("ca.ext"), ca).unwrap();
        std::fs::write(d.join("list.ext"), list).unwrap();
        let o = "openssl";
        prikaz(
            d,
            &[
                o,
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-384",
                "-noenc",
                "-days",
                "2",
                "-subj",
                "/CN=Koren skusky",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
                "-addext",
                "keyUsage=critical,keyCertSign",
                "-keyout",
                "koren.key",
                "-out",
                "koren.pem",
            ],
        );
        prikaz(
            d,
            &[
                o,
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-noenc",
                "-subj",
                "/CN=Medzi skusky",
                "-keyout",
                "medzi.key",
                "-out",
                "medzi.csr",
            ],
        );
        prikaz(
            d,
            &[
                o,
                "x509",
                "-req",
                "-in",
                "medzi.csr",
                "-CA",
                "koren.pem",
                "-CAkey",
                "koren.key",
                "-days",
                "2",
                "-sha384",
                "-extfile",
                "ca.ext",
                "-out",
                "medzi.pem",
            ],
        );
        for (meno, kluc) in [("list-ec", "ec"), ("list-rsa", "rsa:2048")] {
            let mut args = vec![o, "req", "-new", "-newkey", kluc];
            if kluc == "ec" {
                args.extend(["-pkeyopt", "ec_paramgen_curve:P-256"]);
            }
            let (k, csr, pem) = (format!("{meno}.key"), format!("{meno}.csr"), format!("{meno}.pem"));
            args.extend(["-noenc", "-subj", "/CN=skuska.orchidea", "-keyout", &k, "-out", &csr]);
            prikaz(d, &args);
            prikaz(
                d,
                &[
                    o,
                    "x509",
                    "-req",
                    "-in",
                    &csr,
                    "-CA",
                    "medzi.pem",
                    "-CAkey",
                    "medzi.key",
                    "-days",
                    "2",
                    "-sha256",
                    "-extfile",
                    "list.ext",
                    "-out",
                    &pem,
                ],
            );
        }
        prikaz(d, &[o, "x509", "-in", "koren.pem", "-outform", "DER", "-out", "koren.der"]);
    }

    #[test]
    #[ignore = "rozhodca: program openssl – v CI `cargo test -p orchidea-siet -- --ignored`"]
    fn proti_openssl_s_server() {
        use std::process::{Command, Stdio};
        let d = std::env::temp_dir().join(format!("orchidea-tls-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        retaz_openssl(&d);
        let teraz =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let (overenie, _) =
            crate::x509::Overenie::nove(&[std::fs::read(d.join("koren.der")).unwrap()], teraz);
        let mut port = 44330 + (std::process::id() % 1000) as u16;
        let mut pripoj = |sada: Sada, list: &str, meno: &str| {
            port += 1;
            let mut server = Command::new("openssl")
                .args(["s_server", "-tls1_3", "-www", "-naccept", "1", "-ciphersuites", sada.meno()])
                .args(["-accept", &format!("127.0.0.1:{port}"), "-cert", &format!("{list}.pem")])
                .args(["-key", &format!("{list}.key"), "-cert_chain", "medzi.pem"])
                .current_dir(&d)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let tcp = (0..50)
                .find_map(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    std::net::TcpStream::connect(("127.0.0.1", port)).ok()
                })
                .expect("s_server neodpovedá");
            tcp.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
            let v = nadviaz(tcp, meno, &overenie, &mut urandom).map(|mut t| {
                t.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
                let mut odpoved = String::new();
                t.read_to_string(&mut odpoved).unwrap();
                let _ = t.zavri();
                (t.sada, odpoved)
            });
            let _ = server.kill();
            let _ = server.wait();
            v
        };
        for sada in [Sada::Chacha20Poly1305, Sada::Aes128Gcm, Sada::Aes256Gcm] {
            for list in ["list-ec", "list-rsa"] {
                let (s, odpoved) =
                    pripoj(sada, list, "skuska.orchidea").unwrap_or_else(|e| panic!("{list}: {e}"));
                assert_eq!(s, sada);
                assert!(odpoved.starts_with("HTTP/1.0 200 ok"), "{odpoved}");
                assert!(odpoved.contains(sada.meno()), "server hlási tú istú sadu: {odpoved}");
            }
        }
        // iné meno → overenie zlyhá ešte pred odoslaním čohokoľvek
        let e = pripoj(Sada::Chacha20Poly1305, "list-ec", "iny.orchidea").unwrap_err();
        assert!(matches!(&e, Chyba::Overenie(t) if t.contains("nepatrí menu")), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn server_hello_odmietnutia() {
        let relacia = [5u8; 32];
        let zostav = |random: &[u8; 32], sada: u16, verzia: Option<u16>| {
            let mut t = 0x0303u16.to_be_bytes().to_vec();
            t.extend_from_slice(random);
            t.push(32);
            t.extend_from_slice(&relacia);
            t.extend_from_slice(&sada.to_be_bytes());
            t.push(0);
            let mut r = Vec::new();
            if let Some(v) = verzia {
                rozsirenie(&mut r, ROZ_SUPPORTED_VERSIONS, &v.to_be_bytes());
            }
            let mut ks = SKUPINA_X25519.to_be_bytes().to_vec();
            s_dlzkou2(&mut ks, &[9; 32]);
            rozsirenie(&mut r, ROZ_KEY_SHARE, &ks);
            s_dlzkou2(&mut t, &r);
            t
        };
        assert!(precitaj_server_hello(&zostav(&[1; 32], 0x1303, Some(TLS13)), &relacia).is_ok());
        assert!(matches!(
            precitaj_server_hello(&zostav(&HRR, 0x1303, Some(TLS13)), &relacia),
            Err(Chyba::Nezhoda(_))
        ));
        assert!(matches!(
            precitaj_server_hello(&zostav(&[1; 32], 0x1303, None), &relacia),
            Err(Chyba::Nezhoda(_))
        ));
        assert!(matches!(
            precitaj_server_hello(&zostav(&[1; 32], 0xc02f, Some(TLS13)), &relacia),
            Err(Chyba::Protokol(_))
        ));
        assert!(matches!(
            precitaj_server_hello(&zostav(&[1; 32], 0x1303, Some(TLS13)), &[6; 32]),
            Err(Chyba::Protokol(_))
        ));
    }
}
