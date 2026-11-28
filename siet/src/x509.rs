//! Overenie servera HTTPS: certifikáty X.509, reťaz k dôveryhodnej autorite a meno servera (A12, krok 4).
//!
//! Podľa **RFC 5280** (4.1 štruktúra certifikátu, 4.2 rozšírenia, 6.1 overenie cesty – zjednodušené tak, ako ho
//! robia prehliadače: bez politík), **X.690** (DER – presné kódovanie, odmietnu sa neminimálne dĺžky), **RFC 9525**
//! (meno servera len z subjectAltName, hviezdička len ako celý ľavý štítok), **RFC 8446 4.2.3** (podpisové schémy
//! TLS 1.3) a **RFC 4055** (parametre RSASSA-PSS v certifikátoch). Podpisy počíta `orchidea-kryptografia`.
//!
//! Čo sa odmietne: SHA-1 a MD5 v podpisoch, RSA pod 2048 bitov, neznáme **kritické** rozšírenie, sprostredkujúca
//! autorita bez `cA`, prekročená dĺžka cesty, certifikát mimo platnosti, meno mimo subjectAltName, zakázaný priestor
//! mien (nameConstraints pre mená DNS a adresy IP).

use std::net::IpAddr;

use orchidea_kryptografia::Odtlacok;
use orchidea_kryptografia::ec::{self, Krivka};
use orchidea_kryptografia::rsa;
use orchidea_kryptografia::sha2::{Sha256, Sha384, Sha512};

use crate::tls::{Overovac, VerejnyKluc};

// ---- identifikátory objektov (obsah OID v DER) ----
const OID_EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const OID_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_RSA_PSS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];
const OID_MGF1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x08];
const OID_RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_RSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const OID_RSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const OID_ECDSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
const OID_SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
const OID_SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
const OID_SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
const OID_SERVER_AUTH: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
const OID_ANY_EKU: &[u8] = &[0x55, 0x1d, 0x25, 0x00];
/// rozšírenia 2.5.29.x
const ROZ_KEY_USAGE: u8 = 15;
const ROZ_SAN: u8 = 17;
const ROZ_BASIC: u8 = 19;
const ROZ_NAME_CONSTRAINTS: u8 = 30;
const ROZ_POLICIES: u8 = 32;
const ROZ_POLICY_CONSTRAINTS: u8 = 36;
const ROZ_EKU: u8 = 37;
const ROZ_INHIBIT_ANY: u8 = 54;

/// Najdlhšia cesta (list + sprostredkujúce), ktorú klient skúsi.
const NAJVIAC_CESTA: usize = 8;

// ---- DER (X.690) ----

/// Čítačka DER: (značka, obsah, celé TLV).
struct Der<'a> {
    d: &'a [u8],
}

impl<'a> Der<'a> {
    fn novy(d: &'a [u8]) -> Der<'a> {
        Der { d }
    }
    fn koniec(&self) -> bool {
        self.d.is_empty()
    }
    fn znacka(&self) -> Option<u8> {
        self.d.first().copied()
    }
    fn dalsi(&mut self) -> Result<(u8, &'a [u8], &'a [u8]), String> {
        let d = self.d;
        let (&znacka, zvysok) = d.split_first().ok_or("DER: koniec údajov")?;
        if znacka & 0x1f == 0x1f {
            return Err("DER: dlhá značka".into());
        }
        let (&prvy, mut zvysok) = zvysok.split_first().ok_or("DER: chýba dĺžka")?;
        let dlzka = if prvy < 0x80 {
            prvy as usize
        } else {
            let n = (prvy & 0x7f) as usize;
            if n == 0 || n > 4 || zvysok.len() < n {
                return Err("DER: neplatná dĺžka".into());
            }
            let mut v = 0usize;
            for &b in &zvysok[..n] {
                v = (v << 8) | b as usize;
            }
            if zvysok[0] == 0 || v < 0x80 {
                return Err("DER: neminimálna dĺžka".into());
            }
            zvysok = &zvysok[n..];
            v
        };
        if zvysok.len() < dlzka {
            return Err("DER: skrátené údaje".into());
        }
        let hlavicka = d.len() - zvysok.len();
        let cele = &d[..hlavicka + dlzka];
        self.d = &zvysok[dlzka..];
        Ok((znacka, &zvysok[..dlzka], cele))
    }
    fn ocakavaj(&mut self, znacka: u8) -> Result<&'a [u8], String> {
        let (z, obsah, _) = self.dalsi()?;
        if z != znacka {
            return Err(format!("DER: očakávaná značka 0x{znacka:02x}, je 0x{z:02x}"));
        }
        Ok(obsah)
    }
    fn ocakavaj_cele(&mut self, znacka: u8) -> Result<(&'a [u8], &'a [u8]), String> {
        let (z, obsah, cele) = self.dalsi()?;
        if z != znacka {
            return Err(format!("DER: očakávaná značka 0x{znacka:02x}, je 0x{z:02x}"));
        }
        Ok((obsah, cele))
    }
    fn volitelny(&mut self, znacka: u8) -> Result<Option<&'a [u8]>, String> {
        if self.znacka() == Some(znacka) { self.ocakavaj(znacka).map(Some) } else { Ok(None) }
    }
}

/// Kladné celé číslo (INTEGER) bez úvodnej nuly.
fn kladne(obsah: &[u8]) -> Result<&[u8], String> {
    match obsah {
        [] => Err("DER: prázdne celé číslo".into()),
        [b, ..] if b & 0x80 != 0 => Err("DER: záporné číslo".into()),
        [0, b, ..] if b & 0x80 == 0 => Err("DER: neminimálne celé číslo".into()),
        [0, zvysok @ ..] if !zvysok.is_empty() => Ok(zvysok),
        _ => Ok(obsah),
    }
}

/// BIT STRING bez nevyužitých bitov.
fn bitovy_retazec(obsah: &[u8]) -> Result<&[u8], String> {
    match obsah {
        [0, zvysok @ ..] => Ok(zvysok),
        _ => Err("DER: BIT STRING s nevyužitými bitmi".into()),
    }
}

/// Čas (UTCTime alebo GeneralizedTime, RFC 5280 4.1.2.5) → sekundy od 1. 1. 1970.
fn cas(znacka: u8, obsah: &[u8]) -> Result<i64, String> {
    let t = std::str::from_utf8(obsah).map_err(|_| "čas: nie je text")?;
    let (rok, zvysok) = match (znacka, t.len()) {
        (0x17, 13) => {
            let yy: i64 = t[..2].parse().map_err(|_| "čas: rok")?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &t[2..])
        }
        (0x18, 15) => (t[..4].parse().map_err(|_| "čas: rok")?, &t[4..]),
        _ => return Err(format!("čas: neplatný tvar {t}")),
    };
    if !zvysok.ends_with('Z') || !zvysok[..10].bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("čas: neplatný tvar {t}"));
    }
    let c = |i: usize| zvysok[i..i + 2].parse::<i64>().unwrap_or(-1);
    let (mesiac, den, h, m, s) = (c(0), c(2), c(4), c(6), c(8));
    if !(1..=12).contains(&mesiac) || !(1..=31).contains(&den) || h > 23 || m > 59 || s > 59 {
        return Err(format!("čas: neplatná hodnota {t}"));
    }
    // dni od 1. 1. 1970 (občiansky kalendár, H. Hinnant: chrono-Compatible Low-Level Date Algorithms)
    let y = if mesiac <= 2 { rok - 1 } else { rok };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mesiac + 9) % 12;
    let doy = (153 * mp + 2) / 5 + den - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let dni = era * 146_097 + doe - 719_468;
    Ok(dni * 86_400 + h * 3600 + m * 60 + s)
}

// ---- certifikát ----

/// Algoritmus podpisu certifikátu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algoritmus {
    Ecdsa(Hash),
    RsaPkcs1(Hash),
    RsaPss(Hash),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hash {
    S256,
    S384,
    S512,
}

impl Hash {
    fn z(self, d: &[u8]) -> Vec<u8> {
        match self {
            Hash::S256 => Sha256::z(d),
            Hash::S384 => Sha384::z(d),
            Hash::S512 => Sha512::z(d),
        }
    }
    fn z_oid(oid: &[u8]) -> Option<Hash> {
        match oid {
            OID_SHA256 => Some(Hash::S256),
            OID_SHA384 => Some(Hash::S384),
            OID_SHA512 => Some(Hash::S512),
            _ => None,
        }
    }
    fn dlzka(self) -> usize {
        match self {
            Hash::S256 => 32,
            Hash::S384 => 48,
            Hash::S512 => 64,
        }
    }
}

/// AlgorithmIdentifier → algoritmus podpisu (RFC 5280 4.1.1.2, RFC 5758, RFC 4055 3.1 pre PSS).
fn algoritmus(obsah: &[u8]) -> Result<Algoritmus, String> {
    let mut d = Der::novy(obsah);
    let oid = d.ocakavaj(0x06)?;
    let bez_parametrov = |d: &mut Der| -> Result<(), String> {
        // pri RSA je NULL povinný, pri ECDSA musí chýbať – prijmeme oboje len tam, kde norma dovoľuje
        if d.volitelny(0x05)?.is_some_and(|n| !n.is_empty()) || !d.koniec() {
            return Err("algoritmus: nečakané parametre".into());
        }
        Ok(())
    };
    let a = match oid {
        OID_ECDSA_SHA256 | OID_ECDSA_SHA384 | OID_ECDSA_SHA512 => {
            if !d.koniec() {
                return Err("ECDSA: parametre musia chýbať".into());
            }
            Algoritmus::Ecdsa(match oid {
                OID_ECDSA_SHA256 => Hash::S256,
                OID_ECDSA_SHA384 => Hash::S384,
                _ => Hash::S512,
            })
        }
        OID_RSA_SHA256 | OID_RSA_SHA384 | OID_RSA_SHA512 => {
            bez_parametrov(&mut d)?;
            Algoritmus::RsaPkcs1(match oid {
                OID_RSA_SHA256 => Hash::S256,
                OID_RSA_SHA384 => Hash::S384,
                _ => Hash::S512,
            })
        }
        OID_RSA_PSS => Algoritmus::RsaPss(pss_parametre(d.ocakavaj(0x30)?)?),
        _ => return Err(format!("nepodporovaný algoritmus podpisu {}", oid_text(oid))),
    };
    Ok(a)
}

/// RSASSA-PSS-params (RFC 4055 3.1): prijmeme len hash ∈ {SHA-256, 384, 512}, MGF1 s tým istým hashom, soľ = dĺžka
/// hashu, trailerField 1 (to dovoľuje aj RFC 8446 pre certifikáty).
fn pss_parametre(obsah: &[u8]) -> Result<Hash, String> {
    let mut d = Der::novy(obsah);
    let hash_alg = d.volitelny(0xa0)?.ok_or("PSS: chýba hash (SHA-1 sa neprijíma)")?;
    let mut h = Der::novy(Der::novy(hash_alg).ocakavaj(0x30)?);
    let hash = Hash::z_oid(h.ocakavaj(0x06)?).ok_or("PSS: nepodporovaný hash")?;
    let mgf = d.volitelny(0xa1)?.ok_or("PSS: chýba MGF")?;
    let mut m = Der::novy(Der::novy(mgf).ocakavaj(0x30)?);
    if m.ocakavaj(0x06)? != OID_MGF1 {
        return Err("PSS: MGF nie je MGF1".into());
    }
    let mut mh = Der::novy(m.ocakavaj(0x30)?);
    if Hash::z_oid(mh.ocakavaj(0x06)?) != Some(hash) {
        return Err("PSS: MGF1 s iným hashom".into());
    }
    let sol = d.volitelny(0xa2)?.ok_or("PSS: chýba dĺžka soli")?;
    let sol = kladne(Der::novy(sol).ocakavaj(0x02)?)?;
    if sol.len() != 1 || sol[0] as usize != hash.dlzka() {
        return Err("PSS: soľ inej dĺžky ako hash".into());
    }
    if let Some(t) = d.volitelny(0xa3)?
        && Der::novy(t).ocakavaj(0x02)? != [1]
    {
        return Err("PSS: trailerField".into());
    }
    Ok(hash)
}

fn oid_text(oid: &[u8]) -> String {
    let mut casti = Vec::new();
    if let Some(&prvy) = oid.first() {
        casti.push((prvy / 40).min(2) as u64);
        casti.push(prvy as u64 - 40 * casti[0]);
    }
    let mut v = 0u64;
    for &b in oid.iter().skip(1) {
        v = (v << 7) | (b & 0x7f) as u64;
        if b & 0x80 == 0 {
            casti.push(v);
            v = 0;
        }
    }
    casti.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(".")
}

/// Verejný kľúč z SubjectPublicKeyInfo.
fn kluc_z_spki(obsah: &[u8]) -> Result<VerejnyKluc, String> {
    let mut d = Der::novy(obsah);
    let mut alg = Der::novy(d.ocakavaj(0x30)?);
    let oid = alg.ocakavaj(0x06)?.to_vec();
    let parametre = if alg.koniec() { Vec::new() } else { alg.dalsi()?.2.to_vec() };
    let kluc = bitovy_retazec(d.ocakavaj(0x03)?)?.to_vec();
    let k = VerejnyKluc { algoritmus: oid, parametre, kluc };
    // kontrola tvaru hneď (zlý kľúč = zlý certifikát)
    match k.algoritmus.as_slice() {
        OID_EC => {
            krivka(&k)?;
        }
        OID_RSA => {
            rsa_kluc(&k)?;
        }
        iny => return Err(format!("nepodporovaný typ kľúča {}", oid_text(iny))),
    }
    Ok(k)
}

fn krivka(k: &VerejnyKluc) -> Result<Krivka, String> {
    let mut p = Der::novy(&k.parametre);
    match p.ocakavaj(0x06)? {
        OID_P256 => Ok(Krivka::P256),
        OID_P384 => Ok(Krivka::P384),
        iny => Err(format!("nepodporovaná krivka {}", oid_text(iny))),
    }
}

fn rsa_kluc(k: &VerejnyKluc) -> Result<rsa::Kluc, String> {
    if !(k.parametre.is_empty() || k.parametre == [0x05, 0x00]) {
        return Err("RSA: parametre kľúča".into());
    }
    let mut d = Der::novy(Der::novy(&k.kluc).ocakavaj(0x30)?);
    let n = kladne(d.ocakavaj(0x02)?)?;
    let e = kladne(d.ocakavaj(0x02)?)?;
    rsa::Kluc::novy(n, e).map_err(|e| format!("RSA kľúč: {e:?}"))
}

/// Podpis ECDSA v DER (ECDSA-Sig-Value, RFC 5480 / RFC 8446 4.2.3) → (r, s).
fn ecdsa_podpis(podpis: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let mut d = Der::novy(podpis);
    let mut s = Der::novy(d.ocakavaj(0x30)?);
    if !d.koniec() {
        return Err("ECDSA: údaje za podpisom".into());
    }
    let r = kladne(s.ocakavaj(0x02)?)?;
    let ss = kladne(s.ocakavaj(0x02)?)?;
    if !s.koniec() {
        return Err("ECDSA: údaje v podpise".into());
    }
    Ok((r, ss))
}

/// Overí podpis `podpis` nad `data` kľúčom `k` algoritmom certifikátu.
fn over_podpis_certifikatu(
    k: &VerejnyKluc,
    alg: Algoritmus,
    data: &[u8],
    podpis: &[u8],
) -> Result<(), String> {
    match (alg, k.algoritmus.as_slice()) {
        (Algoritmus::Ecdsa(h), OID_EC) => {
            let (r, s) = ecdsa_podpis(podpis)?;
            ec::over(krivka(k)?, &k.kluc, &h.z(data), r, s).map_err(|e| format!("ECDSA: {e:?}"))
        }
        (Algoritmus::RsaPkcs1(h), OID_RSA) => {
            let kluc = rsa_kluc(k)?;
            let o = h.z(data);
            match h {
                Hash::S256 => kluc.over_pkcs1::<Sha256>(&o, podpis),
                Hash::S384 => kluc.over_pkcs1::<Sha384>(&o, podpis),
                Hash::S512 => kluc.over_pkcs1::<Sha512>(&o, podpis),
            }
            .map_err(|e| format!("RSA: {e:?}"))
        }
        (Algoritmus::RsaPss(h), OID_RSA) => over_pss(&rsa_kluc(k)?, h, data, podpis),
        _ => Err("algoritmus podpisu nesedí s typom kľúča".into()),
    }
}

fn over_pss(kluc: &rsa::Kluc, h: Hash, data: &[u8], podpis: &[u8]) -> Result<(), String> {
    let o = h.z(data);
    match h {
        Hash::S256 => kluc.over_pss::<Sha256>(&o, podpis, 32),
        Hash::S384 => kluc.over_pss::<Sha384>(&o, podpis, 48),
        Hash::S512 => kluc.over_pss::<Sha512>(&o, podpis, 64),
    }
    .map_err(|e| format!("RSA-PSS: {e:?}"))
}

/// Obmedzenia priestoru mien (nameConstraints) – len mená DNS a adresy IP (iné typy sa pri overení servera
/// nepoužívajú).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Obmedzenia {
    povolene_dns: Vec<String>,
    zakazane_dns: Vec<String>,
    povolene_ip: Vec<Vec<u8>>,
    zakazane_ip: Vec<Vec<u8>>,
    /// povolené podstromy existujú pre daný typ (prázdny zoznam typu = bez obmedzenia toho typu)
    ma_povolene_dns: bool,
    ma_povolene_ip: bool,
}

/// Rozobraný certifikát.
#[derive(Debug, Clone)]
pub struct Certifikat {
    der: Vec<u8>,
    tbs: (usize, usize),
    algoritmus: Algoritmus,
    podpis: Vec<u8>,
    vydavatel: Vec<u8>,
    subjekt: Vec<u8>,
    od: i64,
    do_: i64,
    pub kluc: VerejnyKluc,
    /// basicConstraints cA = true → Some(pathLenConstraint)
    autorita: Option<Option<u32>>,
    ma_basic: bool,
    /// keyUsage (bity 0–8), ak je
    pouzitie: Option<u16>,
    /// extKeyUsage, ak je
    ucely: Option<Vec<Vec<u8>>>,
    dns: Vec<String>,
    ip: Vec<Vec<u8>>,
    obmedzenia: Option<Obmedzenia>,
}

impl Certifikat {
    /// Rozbor certifikátu v DER (RFC 5280 4.1).
    pub fn precitaj(der: &[u8]) -> Result<Certifikat, String> {
        let mut vonku = Der::novy(der);
        let cert = vonku.ocakavaj(0x30)?;
        if !vonku.koniec() {
            return Err("certifikát: údaje za koncom".into());
        }
        let mut c = Der::novy(cert);
        let (tbs, tbs_cele) = c.ocakavaj_cele(0x30)?;
        let zaciatok = tbs_cele.as_ptr() as usize - der.as_ptr() as usize;
        let alg_vonku = c.ocakavaj_cele(0x30)?;
        let algoritmus = algoritmus(alg_vonku.0)?;
        let podpis = bitovy_retazec(c.ocakavaj(0x03)?)?.to_vec();
        if !c.koniec() {
            return Err("certifikát: údaje za podpisom".into());
        }
        let mut t = Der::novy(tbs);
        let verzia = match t.volitelny(0xa0)? {
            Some(v) => Der::novy(v).ocakavaj(0x02)?.to_vec(),
            None => vec![0],
        };
        let _seriove = t.ocakavaj(0x02)?;
        let (_, alg_vnutri) = t.ocakavaj_cele(0x30)?;
        if alg_vnutri != alg_vonku.1 {
            return Err("certifikát: algoritmus v tbs a vonku sa líši (RFC 5280 4.1.1.2)".into());
        }
        let (_, vydavatel) = t.ocakavaj_cele(0x30)?;
        let mut platnost = Der::novy(t.ocakavaj(0x30)?);
        let (z1, o1, _) = platnost.dalsi()?;
        let (z2, o2, _) = platnost.dalsi()?;
        let (od, do_) = (cas(z1, o1)?, cas(z2, o2)?);
        let (_, subjekt) = t.ocakavaj_cele(0x30)?;
        let kluc = kluc_z_spki(t.ocakavaj(0x30)?)?;
        let _ = t.volitelny(0x81)?;
        let _ = t.volitelny(0x82)?;
        let mut cert = Certifikat {
            der: der.to_vec(),
            tbs: (zaciatok, tbs_cele.len()),
            algoritmus,
            podpis,
            vydavatel: vydavatel.to_vec(),
            subjekt: subjekt.to_vec(),
            od,
            do_,
            kluc,
            autorita: None,
            ma_basic: false,
            pouzitie: None,
            ucely: None,
            dns: Vec::new(),
            ip: Vec::new(),
            obmedzenia: None,
        };
        if let Some(roz) = t.volitelny(0xa3)? {
            if verzia != [2] {
                return Err("certifikát: rozšírenia mimo verzie 3".into());
            }
            cert.rozsirenia(Der::novy(roz).ocakavaj(0x30)?)?;
        }
        if !t.koniec() {
            return Err("certifikát: údaje za rozšíreniami".into());
        }
        Ok(cert)
    }

    fn rozsirenia(&mut self, obsah: &[u8]) -> Result<(), String> {
        let mut d = Der::novy(obsah);
        let mut videne = Vec::new();
        while !d.koniec() {
            let mut e = Der::novy(d.ocakavaj(0x30)?);
            let oid = e.ocakavaj(0x06)?;
            let kriticke = match e.volitelny(0x01)? {
                Some([0xff]) => true,
                Some([0x00]) => false,
                Some(_) => return Err("rozšírenie: neplatná hodnota critical".into()),
                None => false,
            };
            let hodnota = e.ocakavaj(0x04)?;
            if videne.contains(&oid) {
                return Err(format!("rozšírenie {} dvakrát", oid_text(oid)));
            }
            videne.push(oid);
            let typ = match oid {
                [0x55, 0x1d, t] => Some(*t),
                _ => None,
            };
            match typ {
                Some(ROZ_BASIC) => {
                    let mut b = Der::novy(Der::novy(hodnota).ocakavaj(0x30)?);
                    let ca = matches!(b.volitelny(0x01)?, Some([0xff]));
                    let dlzka = match b.volitelny(0x02)? {
                        Some(n) => {
                            let n = kladne(n)?;
                            Some(n.iter().fold(0u32, |a, &x| a.saturating_mul(256).saturating_add(x as u32)))
                        }
                        None => None,
                    };
                    self.ma_basic = true;
                    self.autorita = if ca { Some(dlzka) } else { None };
                }
                Some(ROZ_KEY_USAGE) => {
                    let mut b = Der::novy(hodnota);
                    let bs = b.ocakavaj(0x03)?;
                    let (&nevyuzite, bajty) = bs.split_first().ok_or("keyUsage prázdne")?;
                    if nevyuzite > 7 {
                        return Err("keyUsage".into());
                    }
                    let mut v = 0u16;
                    for (i, &x) in bajty.iter().take(2).enumerate() {
                        for j in 0..8 {
                            if x & (0x80 >> j) != 0 {
                                v |= 1 << (i * 8 + j);
                            }
                        }
                    }
                    self.pouzitie = Some(v);
                }
                Some(ROZ_EKU) => {
                    let mut b = Der::novy(Der::novy(hodnota).ocakavaj(0x30)?);
                    let mut u = Vec::new();
                    while !b.koniec() {
                        u.push(b.ocakavaj(0x06)?.to_vec());
                    }
                    self.ucely = Some(u);
                }
                Some(ROZ_SAN) => {
                    let mut b = Der::novy(Der::novy(hodnota).ocakavaj(0x30)?);
                    while !b.koniec() {
                        let (z, o, _) = b.dalsi()?;
                        match z {
                            0x82 => self.dns.push(
                                std::str::from_utf8(o)
                                    .ok()
                                    .filter(|s| s.is_ascii())
                                    .ok_or("subjectAltName: meno DNS nie je ASCII")?
                                    .to_ascii_lowercase(),
                            ),
                            0x87 => self.ip.push(o.to_vec()),
                            _ => {}
                        }
                    }
                }
                Some(ROZ_NAME_CONSTRAINTS) => self.obmedzenia = Some(obmedzenia(hodnota)?),
                // politiky certifikátov sa nevyhodnocujú (ako v prehliadačoch); kritické nevadí
                Some(ROZ_POLICIES | ROZ_POLICY_CONSTRAINTS | ROZ_INHIBIT_ANY) => {}
                _ if kriticke => return Err(format!("neznáme kritické rozšírenie {}", oid_text(oid))),
                _ => {}
            }
        }
        Ok(())
    }

    fn tbs(&self) -> &[u8] {
        &self.der[self.tbs.0..self.tbs.0 + self.tbs.1]
    }

    /// Mená DNS z subjectAltName (malými písmenami).
    pub fn mena(&self) -> &[String] {
        &self.dns
    }

    /// Platnosť (od, do) v sekundách od 1. 1. 1970.
    pub fn platnost(&self) -> (i64, i64) {
        (self.od, self.do_)
    }

    /// Bity keyUsage: 0 digitalSignature, 5 keyCertSign.
    fn smie(&self, bit: u16) -> bool {
        self.pouzitie.is_none_or(|v| v & (1 << bit) != 0)
    }

    fn sluzi_serveru(&self) -> bool {
        self.ucely.as_ref().is_none_or(|u| u.iter().any(|o| o == OID_SERVER_AUTH || o == OID_ANY_EKU))
    }
}

fn obmedzenia(hodnota: &[u8]) -> Result<Obmedzenia, String> {
    let mut o = Obmedzenia::default();
    let mut d = Der::novy(Der::novy(hodnota).ocakavaj(0x30)?);
    while !d.koniec() {
        let (z, obsah, _) = d.dalsi()?;
        let povolene = match z {
            0xa0 => true,
            0xa1 => false,
            _ => return Err("nameConstraints".into()),
        };
        let mut stromy = Der::novy(obsah);
        while !stromy.koniec() {
            let mut s = Der::novy(stromy.ocakavaj(0x30)?);
            let (typ, zaklad, _) = s.dalsi()?;
            match (typ, povolene) {
                (0x82, true) => {
                    o.ma_povolene_dns = true;
                    o.povolene_dns.push(String::from_utf8_lossy(zaklad).to_ascii_lowercase())
                }
                (0x82, false) => o.zakazane_dns.push(String::from_utf8_lossy(zaklad).to_ascii_lowercase()),
                (0x87, true) => {
                    o.ma_povolene_ip = true;
                    o.povolene_ip.push(zaklad.to_vec())
                }
                (0x87, false) => o.zakazane_ip.push(zaklad.to_vec()),
                _ => {}
            }
        }
    }
    Ok(o)
}

/// Meno patrí do podstromu DNS (RFC 5280 4.2.1.10): rovnaké alebo poddoména; „.x“ len poddoména.
fn v_strome_dns(meno: &str, strom: &str) -> bool {
    let meno = meno.strip_prefix("*.").unwrap_or(meno);
    if strom.is_empty() {
        return true;
    }
    if let Some(s) = strom.strip_prefix('.') {
        return meno.ends_with(&format!(".{s}"));
    }
    meno == strom || meno.ends_with(&format!(".{strom}"))
}

fn v_strome_ip(ip: &[u8], strom: &[u8]) -> bool {
    let n = ip.len();
    strom.len() == 2 * n && (0..n).all(|i| ip[i] & strom[n + i] == strom[i] & strom[n + i])
}

impl Obmedzenia {
    fn dovoli(&self, c: &Certifikat) -> bool {
        let dns_ok = c.dns.iter().all(|m| {
            (!self.ma_povolene_dns || self.povolene_dns.iter().any(|s| v_strome_dns(m, s)))
                && !self.zakazane_dns.iter().any(|s| v_strome_dns(m, s))
        });
        let ip_ok = c.ip.iter().all(|a| {
            (!self.ma_povolene_ip || self.povolene_ip.iter().any(|s| v_strome_ip(a, s)))
                && !self.zakazane_ip.iter().any(|s| v_strome_ip(a, s))
        });
        dns_ok && ip_ok
    }
}

/// Zodpovedá meno servera menu v certifikáte (RFC 9525 6.3)? Hviezdička len ako celý ľavý štítok, pokrýva presne
/// jeden štítok a nie verejnú príponu tvaru „*.com“ (aspoň dve bodky za ňou nevyžadujeme – stačí jedna).
fn meno_sedi(server: &str, vzor: &str) -> bool {
    let server = server.trim_end_matches('.').to_ascii_lowercase();
    let vzor = vzor.trim_end_matches('.');
    if let Some(zvysok) = vzor.strip_prefix("*.") {
        if !zvysok.contains('.') || zvysok.contains('*') {
            return false;
        }
        return match server.split_once('.') {
            Some((stitok, z)) => !stitok.is_empty() && z == zvysok,
            None => false,
        };
    }
    !vzor.contains('*') && server == vzor
}

/// Overovač servera: dôveryhodné koreňové autority a aktuálny čas.
pub struct Overenie {
    korene: Vec<Certifikat>,
    /// sekundy od 1. 1. 1970 – čas zo siete (SNTP); bez neho sa nič neoverí
    teraz: i64,
}

impl std::fmt::Debug for Overenie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Overenie({} koreňových autorít, čas {})", self.korene.len(), self.teraz)
    }
}

impl Overenie {
    /// Koreňové autority v DER; chybné sa preskočia (vráti aj ich počet).
    pub fn nove(korene: &[Vec<u8>], teraz: i64) -> (Overenie, usize) {
        let mut zle = 0;
        let korene =
            korene.iter().filter_map(|d| Certifikat::precitaj(d).map_err(|_| zle += 1).ok()).collect();
        (Overenie { korene, teraz }, zle)
    }

    pub fn pocet_korenov(&self) -> usize {
        self.korene.len()
    }

    fn platny(&self, c: &Certifikat, kto: &str) -> Result<(), String> {
        if self.teraz < c.od {
            return Err(format!("{kto}: ešte neplatí (čas zariadenia {}; od {})", self.teraz, c.od));
        }
        if self.teraz > c.do_ {
            return Err(format!("{kto}: platnosť skončila ({})", c.do_));
        }
        Ok(())
    }

    /// Hľadá cestu od `cesta.last()` ku koreňu; `sprostredkujuce` = ďalšie certifikáty od servera.
    fn cesta(&self, cesta: &mut Vec<Certifikat>, sprostredkujuce: &[Certifikat]) -> Result<(), String> {
        let posledny = cesta.last().ok_or("prázdna cesta")?.clone();
        // koreň, ktorý ho vydal (alebo je ním samotný)
        let mut chyba = String::from("chýba dôveryhodná autorita vydavateľa");
        for koren in &self.korene {
            if koren.der == posledny.der {
                return self.skontroluj_cestu(cesta, None);
            }
            if koren.subjekt == posledny.vydavatel {
                match over_podpis_certifikatu(
                    &koren.kluc,
                    posledny.algoritmus,
                    posledny.tbs(),
                    &posledny.podpis,
                ) {
                    Ok(()) => match self.skontroluj_cestu(cesta, Some(koren)) {
                        Ok(()) => return Ok(()),
                        Err(e) => chyba = e,
                    },
                    Err(e) => chyba = format!("podpis autority: {e}"),
                }
            }
        }
        if cesta.len() >= NAJVIAC_CESTA {
            return Err("príliš dlhá cesta certifikátov".into());
        }
        for s in sprostredkujuce {
            if s.subjekt != posledny.vydavatel || cesta.iter().any(|c| c.der == s.der) {
                continue;
            }
            if over_podpis_certifikatu(&s.kluc, posledny.algoritmus, posledny.tbs(), &posledny.podpis)
                .is_err()
            {
                continue;
            }
            cesta.push(s.clone());
            match self.cesta(cesta, sprostredkujuce) {
                Ok(()) => return Ok(()),
                Err(e) => chyba = e,
            }
            cesta.pop();
        }
        Err(chyba)
    }

    /// Kontroly celej cesty: platnosť, autority (cA, keyUsage, dĺžka cesty, účel), obmedzenia mien.
    fn skontroluj_cestu(&self, cesta: &[Certifikat], koren: Option<&Certifikat>) -> Result<(), String> {
        let list = &cesta[0];
        for (i, c) in cesta.iter().enumerate() {
            self.platny(c, if i == 0 { "certifikát servera" } else { "sprostredkujúca autorita" })?;
        }
        if let Some(k) = koren {
            self.platny(k, "koreňová autorita")?;
        }
        // autority: všetky okrem listu (posledný v ceste môže byť sám koreň)
        let autority: Vec<&Certifikat> = cesta[1..].iter().chain(koren).collect();
        for (i, a) in autority.iter().enumerate() {
            let je_koren =
                koren.is_some_and(|k| std::ptr::eq(*a, k)) || self.korene.iter().any(|k| k.der == a.der);
            match a.autorita {
                Some(dlzka) => {
                    // pod autoritou je i sprostredkujúcich autorít (bez listu)
                    if dlzka.is_some_and(|d| (i as u32) > d) {
                        return Err("prekročená dĺžka cesty (pathLenConstraint)".into());
                    }
                }
                None if je_koren && !a.ma_basic => {} // starý koreň bez basicConstraints – dôvera je v zozname
                None => return Err("vydavateľ nie je certifikačná autorita (basicConstraints)".into()),
            }
            if !a.smie(5) {
                return Err("autorita nesmie podpisovať certifikáty (keyUsage)".into());
            }
            if !je_koren && !a.sluzi_serveru() {
                return Err("autorita nie je určená pre servery (extKeyUsage)".into());
            }
            if let Some(o) = &a.obmedzenia
                && !o.dovoli(list)
            {
                return Err("meno servera mimo povoleného priestoru autority (nameConstraints)".into());
            }
        }
        if !list.sluzi_serveru() {
            return Err("certifikát nie je určený pre server (extKeyUsage)".into());
        }
        if !list.smie(0) {
            return Err("kľúč servera nesmie podpisovať (keyUsage)".into());
        }
        Ok(())
    }
}

impl Overovac for Overenie {
    fn over_retaz(&self, retaz: &[Vec<u8>], meno: &str) -> Result<VerejnyKluc, String> {
        let list = Certifikat::precitaj(retaz.first().ok_or("prázdna reťaz")?)
            .map_err(|e| format!("certifikát servera: {e}"))?;
        // meno: adresa IP proti iPAddress, inak meno DNS proti dNSName (RFC 9525 – nie CN)
        let sedi = match meno.parse::<IpAddr>() {
            Ok(IpAddr::V4(a)) => list.ip.iter().any(|x| x == &a.octets()),
            Ok(IpAddr::V6(a)) => list.ip.iter().any(|x| x == &a.octets()),
            Err(_) => list.dns.iter().any(|v| meno_sedi(meno, v)),
        };
        if !sedi {
            return Err(format!("certifikát nepatrí menu {meno} (má {})", list.dns.join(", ")));
        }
        let ostatne: Vec<Certifikat> =
            retaz[1..].iter().filter_map(|d| Certifikat::precitaj(d).ok()).collect();
        let kluc = list.kluc.clone();
        let mut cesta = vec![list];
        self.cesta(&mut cesta, &ostatne)?;
        Ok(kluc)
    }

    fn over_podpis(
        &self,
        kluc: &VerejnyKluc,
        schema: u16,
        sprava: &[u8],
        podpis: &[u8],
    ) -> Result<(), String> {
        match (schema, kluc.algoritmus.as_slice()) {
            (0x0403 | 0x0503, OID_EC) => {
                let (k, h) =
                    if schema == 0x0403 { (Krivka::P256, Hash::S256) } else { (Krivka::P384, Hash::S384) };
                if krivka(kluc)? != k {
                    return Err("schéma ECDSA nesedí s krivkou kľúča (RFC 8446 4.2.3)".into());
                }
                let (r, s) = ecdsa_podpis(podpis)?;
                ec::over(k, &kluc.kluc, &h.z(sprava), r, s).map_err(|e| format!("ECDSA: {e:?}"))
            }
            (0x0804..=0x0806, OID_RSA) => {
                let h = [Hash::S256, Hash::S384, Hash::S512][(schema - 0x0804) as usize];
                over_pss(&rsa_kluc(kluc)?, h, sprava, podpis)
            }
            _ => Err(format!("podpisová schéma 0x{schema:04x} nesedí s kľúčom servera")),
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    /// 28. 11. 2026 12:00 UTC – čas skúšok (skúšobné certifikáty platia 2026–2036, `stary` 2020–2025).
    const TERAZ: i64 = 1_795_867_200;

    macro_rules! der {
        ($m:literal) => {
            include_bytes!(concat!("../skusky/", $m, ".der")).to_vec()
        };
    }

    fn overenie() -> Overenie {
        let (o, zle) = Overenie::nove(&[der!("koren")], TERAZ);
        assert_eq!((o.pocet_korenov(), zle), (1, 0));
        o
    }

    /// Reťaze z nezávislej knižnice (`nastroje/skusobne_certifikaty.py`): koreň P-384 → sprostredkujúca RSA 2048
    /// (podpis ECDSA SHA-384) → list P-256 (podpis RSA PKCS #1 SHA-256) alebo list RSA (podpis RSA-PSS).
    #[test]
    fn platne_retaze() {
        let o = overenie();
        let medzi = der!("medzi");
        for meno in ["skuska.orchidea", "SKUSKA.orchidea.", "a.web.orchidea", "127.0.0.1"] {
            let k =
                o.over_retaz(&[der!("list"), medzi.clone()], meno).unwrap_or_else(|e| panic!("{meno}: {e}"));
            assert_eq!(k.algoritmus, OID_EC);
        }
        let k = o.over_retaz(&[der!("list-pss"), medzi.clone()], "pss.orchidea").unwrap();
        assert_eq!(k.algoritmus, OID_RSA);
        // poradie sprostredkujúcich nevadí, navyše poslaný koreň tiež nie
        o.over_retaz(&[der!("list"), der!("koren"), medzi.clone()], "skuska.orchidea").unwrap();
        // autorita s obmedzením mien – meno vnútri
        o.over_retaz(&[der!("v-obmedzeni"), der!("obmedzena")], "a.povolene.orchidea").unwrap();
        let c = Certifikat::precitaj(&der!("list")).unwrap();
        assert_eq!(c.mena(), ["skuska.orchidea", "*.web.orchidea"]);
        assert_eq!(c.platnost(), (1_767_225_600, 2_082_758_400));
    }

    #[test]
    fn odmietnute_retaze() {
        let o = overenie();
        let medzi = der!("medzi");
        let chyba = |retaz: &[Vec<u8>], meno: &str| o.over_retaz(retaz, meno).err().unwrap_or_default();
        let e = chyba(&[der!("list"), medzi.clone()], "iny.orchidea");
        assert!(e.contains("nepatrí menu"), "{e}");
        let e = chyba(&[der!("list"), medzi.clone()], "a.b.web.orchidea");
        assert!(e.contains("nepatrí menu"), "{e}");
        let e = chyba(&[der!("list")], "skuska.orchidea");
        assert!(e.contains("autorita"), "chýba sprostredkujúca: {e}");
        let e = chyba(&[der!("stary"), medzi.clone()], "stary.orchidea");
        assert!(e.contains("platnosť skončila"), "{e}");
        let e = chyba(&[der!("klient"), medzi.clone()], "klient.orchidea");
        assert!(e.contains("extKeyUsage"), "{e}");
        let e = chyba(&[der!("falosny"), der!("list"), medzi.clone()], "falosny.orchidea");
        assert!(e.contains("basicConstraints") || e.contains("autorita"), "list ako vydavateľ: {e}");
        let e = chyba(&[der!("mimo-obmedzenia"), der!("obmedzena")], "zakazane.orchidea");
        assert!(e.contains("nameConstraints"), "{e}");
        let e = chyba(&[der!("nezname"), medzi.clone()], "nezname.orchidea");
        assert!(e.contains("kritické"), "{e}");
        let e = chyba(&[der!("cudzi-list"), der!("cudzi-koren")], "skuska.orchidea");
        assert!(e.contains("autorita") || e.contains("podpis"), "rovnaké meno, iný kľúč: {e}");
        // čas zariadenia pred platnosťou (napr. bez SNTP: 1. 1. 1970)
        let (bez_casu, _) = Overenie::nove(&[der!("koren")], 0);
        let e = bez_casu.over_retaz(&[der!("list"), medzi.clone()], "skuska.orchidea").unwrap_err();
        assert!(e.contains("ešte neplatí"), "{e}");
        // poškodený podpis listu
        let mut zly = der!("list");
        let n = zly.len();
        zly[n - 10] ^= 1;
        assert!(o.over_retaz(&[zly, medzi], "skuska.orchidea").is_err());
    }

    #[test]
    fn der_dlzky() {
        assert_eq!(Der::novy(&[0x04, 0x02, 1, 2]).dalsi().unwrap().1, &[1, 2]);
        assert!(Der::novy(&[0x04, 0x81, 0x02, 1, 2]).dalsi().is_err(), "neminimálna dĺžka");
        assert!(Der::novy(&[0x04, 0x80, 1, 2, 0, 0]).dalsi().is_err(), "neurčitá dĺžka");
        assert!(Der::novy(&[0x04, 0x05, 1, 2]).dalsi().is_err(), "skrátené");
        let mut dlhe = vec![0x04, 0x82, 0x01, 0x00];
        dlhe.extend([7u8; 256]);
        assert_eq!(Der::novy(&dlhe).dalsi().unwrap().1.len(), 256);
        assert_eq!(kladne(&[0x00, 0x80]).unwrap(), &[0x80]);
        assert!(kladne(&[0x00, 0x7f]).is_err() && kladne(&[0x80]).is_err());
    }

    #[test]
    fn casy() {
        assert_eq!(cas(0x17, b"700101000000Z").unwrap(), 0);
        assert_eq!(cas(0x17, b"491231235959Z").unwrap(), 2_524_607_999);
        assert_eq!(cas(0x18, b"20261128103000Z").unwrap(), 1_795_861_800);
        assert_eq!(cas(0x18, b"20240229000000Z").unwrap(), 1_709_164_800, "priestupný deň");
        assert!(cas(0x17, b"261328103000Z").is_err());
        assert!(cas(0x18, b"20261128103000+0100").is_err());
    }

    #[test]
    fn mena() {
        assert!(meno_sedi("www.example.com", "www.example.com"));
        assert!(meno_sedi("WWW.Example.COM.", "www.example.com"));
        assert!(meno_sedi("a.example.com", "*.example.com"));
        assert!(!meno_sedi("a.b.example.com", "*.example.com"), "hviezdička = jeden štítok");
        assert!(!meno_sedi("example.com", "*.example.com"));
        assert!(!meno_sedi("example.com", "*.com"), "nie celá prípona");
        assert!(!meno_sedi("wwww.example.com", "w*.example.com"), "len celý štítok");
        assert!(v_strome_dns("a.orchidea.sk", "orchidea.sk") && v_strome_dns("orchidea.sk", "orchidea.sk"));
        assert!(
            !v_strome_dns("zlaorchidea.sk", "orchidea.sk") && !v_strome_dns("orchidea.sk", ".orchidea.sk")
        );
        assert!(v_strome_ip(&[10, 1, 2, 3], &[10, 0, 0, 0, 255, 0, 0, 0]));
        assert!(!v_strome_ip(&[11, 1, 2, 3], &[10, 0, 0, 0, 255, 0, 0, 0]));
        assert_eq!(oid_text(OID_SERVER_AUTH), "1.3.6.1.5.5.7.3.1");
        assert_eq!(oid_text(OID_P384), "1.3.132.0.34");
    }
}
