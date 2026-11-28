//! Preklad mien na adresy (DNS) – klient bez rekurzie, ktorý sa pýta serverov z DHCP.
//!
//! Podľa **RFC 1035** (správa – kap. 4.1, kompresia mien – 4.1.4, UDP 512 B – 4.2.1), **RFC 3596** (záznam AAAA),
//! **RFC 2308** (negatívne odpovede a ich TTL zo záznamu SOA) a **RFC 6891** (EDNS(0) – väčšia odpoveď cez UDP).
//! Ochrana pred podvrhnutím podľa **RFC 5452**: náhodné číslo otázky (a náhodný zdrojový port volí jadro),
//! odpoveď musí niesť presne tú istú otázku, berú sa len záznamy z časti odpovede a len pre reťaz od pýtaného mena.
//!
//! Vstup a výstup robí volajúci (funkcia „pošli a čakaj“), preto sa celý preklad dá overiť bez siete.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// Port servera DNS.
pub const PORT: u16 = 53;
/// Veľkosť odpovede cez UDP, ktorú klient ohlási v EDNS(0) (odporúčanie DNS Flag Day 2020 – bez delenia paketov).
pub const EDNS_VELKOST: u16 = 1232;
/// Najdlhšie meno v kódovaní DNS (RFC 1035 2.3.4).
const NAJDLHSIE_MENO: usize = 255;
/// Najviac skokov pri kompresii a najdlhšia reťaz CNAME – ochrana pred slučkou.
const NAJVIAC_SKOKOV: usize = 32;
const NAJDLHSIA_RETAZ: usize = 8;
/// Najdlhšie podržanie výsledku: kladný 1 deň, negatívny 3 h (RFC 2308 5).
const NAJDLHSIE_TTL: u32 = 86_400;
const NAJDLHSIE_NEGATIVNE_TTL: u32 = 10_800;
/// Negatívna odpoveď bez SOA sa drží krátko.
const NEGATIVNE_BEZ_SOA: u32 = 60;

/// Typ záznamu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Typ {
    /// adresa IPv4
    A = 1,
    /// adresa IPv6 (RFC 3596)
    Aaaa = 28,
}

const TYP_CNAME: u16 = 5;
const TYP_SOA: u16 = 6;
const TYP_OPT: u16 = 41;
const TRIEDA_IN: u16 = 1;

/// Chyba pri tvorbe otázky alebo čítaní odpovede.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chyba {
    /// meno nie je platné meno hostiteľa (prázdne, dlhé, zlé znaky)
    ZleMeno(String),
    PrilisKratka,
    /// odpoveď na inú otázku (iné číslo, meno, typ) – možný podvrh, zahodí sa
    InaOtazka,
    NieJeOdpoved,
    /// poškodené meno alebo záznam (zlý ukazovateľ kompresie, slučka, presah)
    Poskodena(&'static str),
}

impl std::fmt::Display for Chyba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Chyba::ZleMeno(m) => write!(f, "neplatné meno „{m}“"),
            Chyba::PrilisKratka => write!(f, "odpoveď DNS je príliš krátka"),
            Chyba::InaOtazka => write!(f, "odpoveď nepatrí k otázke"),
            Chyba::NieJeOdpoved => write!(f, "správa nie je odpoveď"),
            Chyba::Poskodena(c) => write!(f, "poškodená odpoveď DNS ({c})"),
        }
    }
}

impl std::error::Error for Chyba {}

/// Výsledok jednej otázky.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Odpoved {
    /// adresy pre meno (po reťazi CNAME), najmenší TTL z použitých záznamov
    Adresy { adresy: Vec<IpAddr>, ttl: u32, kanonicke: String },
    /// meno neexistuje (NXDOMAIN); TTL negatívnej odpovede
    Neexistuje { ttl: u32 },
    /// meno existuje, ale záznam tohto typu nemá (NODATA)
    BezZaznamu { ttl: u32 },
    /// odpoveď končí prezývkou (CNAME) bez adries cieľa – treba sa spýtať na `kanonicke` (RFC 1034 5.3.3)
    Prezyvka { kanonicke: String, ttl: u32 },
    /// odpoveď sa nezmestila do UDP (TC) – treba TCP alebo iný server
    Skratena,
    /// server odpovedal chybou (RCODE: 1 zlá otázka, 2 zlyhanie servera, 5 odmietnutie …)
    ChybaServera(u8),
}

/// Normalizuje meno (malé písmená, bez bodky na konci) a overí ho: menovky 1–63 znakov, spolu najviac 253,
/// písmená, číslice, pomlčka a podčiarkovník (RFC 1035 2.3.1, RFC 1123 2.1).
pub fn normalizuj(meno: &str) -> Result<String, Chyba> {
    let m = meno.strip_suffix('.').unwrap_or(meno).to_ascii_lowercase();
    let zle = || Chyba::ZleMeno(meno.to_string());
    if m.is_empty() || m.len() > NAJDLHSIE_MENO - 2 {
        return Err(zle());
    }
    for menovka in m.split('.') {
        if menovka.is_empty()
            || menovka.len() > 63
            || !menovka.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(zle());
        }
    }
    Ok(m)
}

fn zapis_meno(v: &mut Vec<u8>, meno: &str) {
    for menovka in meno.split('.') {
        v.push(menovka.len() as u8);
        v.extend_from_slice(menovka.as_bytes());
    }
    v.push(0);
}

/// Otázka na záznam `typ` pre `meno` s číslom `id`; s rekurziou (RD) a voliteľne s EDNS(0).
pub fn otazka(id: u16, meno: &str, typ: Typ, edns: bool) -> Result<Vec<u8>, Chyba> {
    let m = normalizuj(meno)?;
    let mut v = Vec::with_capacity(32 + m.len());
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(&0x0100u16.to_be_bytes()); // QR=0, OPCODE=0, RD=1
    v.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    v.extend_from_slice(&[0, 0, 0, 0]); // ANCOUNT, NSCOUNT
    v.extend_from_slice(&(edns as u16).to_be_bytes()); // ARCOUNT
    zapis_meno(&mut v, &m);
    v.extend_from_slice(&(typ as u16).to_be_bytes());
    v.extend_from_slice(&TRIEDA_IN.to_be_bytes());
    if edns {
        // RFC 6891 6.1.2: koreňové meno, typ OPT, trieda = veľkosť UDP, TTL = rozšírený RCODE, verzia 0, príznaky 0
        v.push(0);
        v.extend_from_slice(&TYP_OPT.to_be_bytes());
        v.extend_from_slice(&EDNS_VELKOST.to_be_bytes());
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    }
    Ok(v)
}

fn u16_na(d: &[u8], i: usize) -> Result<u16, Chyba> {
    d.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).ok_or(Chyba::Poskodena("presah"))
}

fn u32_na(d: &[u8], i: usize) -> Result<u32, Chyba> {
    d.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or(Chyba::Poskodena("presah"))
}

/// Prečíta (aj skomprimované) meno od `i`; vráti meno malými písmenami a pozíciu za ním.
/// Ukazovateľ kompresie musí smerovať dozadu (nie na seba ani dopredu) – slučka sa nedá zostaviť.
fn citaj_meno(d: &[u8], mut i: usize) -> Result<(String, usize), Chyba> {
    let mut meno = String::new();
    let mut koniec: Option<usize> = None;
    let mut dlzka = 1usize;
    let mut skokov = 0;
    loop {
        let b = *d.get(i).ok_or(Chyba::Poskodena("meno presahuje správu"))?;
        match b & 0xC0 {
            0x00 if b == 0 => {
                return Ok((meno, koniec.unwrap_or(i + 1)));
            }
            0x00 => {
                let n = b as usize;
                let menovka = d.get(i + 1..i + 1 + n).ok_or(Chyba::Poskodena("menovka presahuje správu"))?;
                dlzka += n + 1;
                if dlzka > NAJDLHSIE_MENO {
                    return Err(Chyba::Poskodena("meno dlhšie ako 255 B"));
                }
                if !meno.is_empty() {
                    meno.push('.');
                }
                for &c in menovka {
                    if c.is_ascii_graphic() && c != b'.' && c != b'\\' {
                        meno.push(c.to_ascii_lowercase() as char);
                    } else {
                        meno.push_str(&format!("\\{c:03}"));
                    }
                }
                i += 1 + n;
            }
            0xC0 => {
                let ciel = (u16_na(d, i)? & 0x3FFF) as usize;
                if ciel >= i {
                    return Err(Chyba::Poskodena("ukazovateľ kompresie dopredu"));
                }
                skokov += 1;
                if skokov > NAJVIAC_SKOKOV {
                    return Err(Chyba::Poskodena("priveľa skokov kompresie"));
                }
                koniec.get_or_insert(i + 2);
                i = ciel;
            }
            _ => return Err(Chyba::Poskodena("vyhradený typ menovky")),
        }
    }
}

/// Jeden záznam zo správy.
struct Zaznam {
    meno: String,
    typ: u16,
    trieda: u16,
    ttl: u32,
    data: std::ops::Range<usize>,
}

fn citaj_zaznam(d: &[u8], i: usize) -> Result<(Zaznam, usize), Chyba> {
    let (meno, i) = citaj_meno(d, i)?;
    let typ = u16_na(d, i)?;
    let trieda = u16_na(d, i + 2)?;
    // RFC 2181 8: TTL s najvyšším bitom sa berie ako 0
    let ttl = u32_na(d, i + 4)?;
    let ttl = if ttl > i32::MAX as u32 { 0 } else { ttl };
    let n = u16_na(d, i + 8)? as usize;
    let od = i + 10;
    if od + n > d.len() {
        return Err(Chyba::Poskodena("údaje záznamu presahujú správu"));
    }
    Ok((Zaznam { meno, typ, trieda, ttl, data: od..od + n }, od + n))
}

/// TTL negatívnej odpovede: min(TTL SOA, SOA MINIMUM) z časti autorít (RFC 2308 5).
fn negativne_ttl(d: &[u8], autority: &[Zaznam]) -> u32 {
    autority
        .iter()
        .filter(|z| z.typ == TYP_SOA)
        .filter_map(|z| {
            // RDATA SOA: MNAME, RNAME, SERIAL, REFRESH, RETRY, EXPIRE, MINIMUM
            let (_, i) = citaj_meno(d, z.data.start).ok()?;
            let (_, i) = citaj_meno(d, i).ok()?;
            let minimum = u32_na(d, i + 16).ok()?;
            Some(z.ttl.min(minimum))
        })
        .min()
        .unwrap_or(NEGATIVNE_BEZ_SOA)
        .min(NAJDLHSIE_NEGATIVNE_TTL)
}

/// Prečíta odpoveď na otázku (`id`, `meno`, `typ`). Chyba = odpoveď sa zahodí a čaká sa ďalej (alebo iný server).
pub fn precitaj(d: &[u8], id: u16, meno: &str, typ: Typ) -> Result<Odpoved, Chyba> {
    let pytane = normalizuj(meno)?;
    if d.len() < 12 {
        return Err(Chyba::PrilisKratka);
    }
    let priznaky = u16_na(d, 2)?;
    if u16_na(d, 0)? != id {
        return Err(Chyba::InaOtazka);
    }
    if priznaky & 0x8000 == 0 || (priznaky >> 11) & 0xF != 0 {
        return Err(Chyba::NieJeOdpoved);
    }
    let (qd, an, ns) = (u16_na(d, 4)?, u16_na(d, 6)?, u16_na(d, 8)?);
    if qd != 1 {
        return Err(Chyba::InaOtazka);
    }
    let (q, i) = citaj_meno(d, 12)?;
    if q != pytane || u16_na(d, i)? != typ as u16 || u16_na(d, i + 2)? != TRIEDA_IN {
        return Err(Chyba::InaOtazka);
    }
    if priznaky & 0x0200 != 0 {
        return Ok(Odpoved::Skratena);
    }
    let mut i = i + 4;
    let mut odpovede = Vec::with_capacity(an as usize);
    for _ in 0..an {
        let (z, dalej) = citaj_zaznam(d, i)?;
        odpovede.push(z);
        i = dalej;
    }
    let mut autority = Vec::with_capacity(ns as usize);
    for _ in 0..ns {
        let (z, dalej) = citaj_zaznam(d, i)?;
        autority.push(z);
        i = dalej;
    }
    match (priznaky & 0xF) as u8 {
        0 => {}
        3 => return Ok(Odpoved::Neexistuje { ttl: negativne_ttl(d, &autority) }),
        r => return Ok(Odpoved::ChybaServera(r)),
    }
    // reťaz od pýtaného mena: CNAME → … → adresy (RFC 1034 3.6.2); iné mená sa ignorujú (RFC 5452 6)
    let mut sucasne = pytane.clone();
    let mut ttl = u32::MAX;
    for _ in 0..=NAJDLHSIA_RETAZ {
        let adresy: Vec<(IpAddr, u32)> = odpovede
            .iter()
            .filter(|z| z.meno == sucasne && z.trieda == TRIEDA_IN && z.typ == typ as u16)
            .filter_map(|z| {
                let b = &d[z.data.clone()];
                let a = match (typ, b.len()) {
                    (Typ::A, 4) => IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3])),
                    (Typ::Aaaa, 16) => {
                        let mut o = [0u8; 16];
                        o.copy_from_slice(b);
                        IpAddr::V6(Ipv6Addr::from(o))
                    }
                    _ => return None,
                };
                Some((a, z.ttl))
            })
            .collect();
        if !adresy.is_empty() {
            let ttl = adresy.iter().map(|(_, t)| *t).min().unwrap_or(0).min(ttl).min(NAJDLHSIE_TTL);
            return Ok(Odpoved::Adresy {
                adresy: adresy.into_iter().map(|(a, _)| a).collect(),
                ttl,
                kanonicke: sucasne,
            });
        }
        let dalsie = odpovede
            .iter()
            .find(|z| z.meno == sucasne && z.trieda == TRIEDA_IN && z.typ == TYP_CNAME)
            .map(|z| (citaj_meno(d, z.data.start), z.ttl));
        match dalsie {
            Some((Ok((ciel, _)), t)) => {
                ttl = ttl.min(t);
                sucasne = ciel;
            }
            Some((Err(e), _)) => return Err(e),
            None => break,
        }
    }
    if sucasne != pytane && !autority.iter().any(|z| z.typ == TYP_SOA) {
        return Ok(Odpoved::Prezyvka { kanonicke: sucasne, ttl: ttl.min(NAJDLHSIE_TTL) });
    }
    Ok(Odpoved::BezZaznamu { ttl: negativne_ttl(d, &autority) })
}

/// Výsledok v pamäti prekladača.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vysledok {
    Adresy(Vec<IpAddr>),
    /// meno neexistuje alebo nemá záznam tohto typu
    Nic,
}

/// Vyrovnávacia pamäť výsledkov podľa TTL (malá – krabička sa pýta na pár mien).
#[derive(Debug, Clone, Default)]
pub struct Pamat {
    zaznamy: Vec<(String, Typ, Vysledok, Duration)>,
}

/// Najviac mien v pamäti; potom sa vyhodí to, čo vyprší najskôr.
const NAJVIAC_V_PAMATI: usize = 128;

impl Pamat {
    pub fn najdi(&self, meno: &str, typ: Typ, teraz: Duration) -> Option<&Vysledok> {
        self.zaznamy
            .iter()
            .find(|(m, t, _, do_)| m == meno && *t == typ && teraz < *do_)
            .map(|(_, _, v, _)| v)
    }

    pub fn uloz(&mut self, meno: &str, typ: Typ, v: Vysledok, ttl: u32, teraz: Duration) {
        self.zaznamy.retain(|(m, t, _, do_)| !(m == meno && *t == typ) && teraz < *do_);
        if ttl == 0 {
            return;
        }
        if self.zaznamy.len() >= NAJVIAC_V_PAMATI
            && let Some(i) = self.zaznamy.iter().enumerate().min_by_key(|(_, z)| z.3).map(|(i, _)| i)
        {
            self.zaznamy.swap_remove(i);
        }
        self.zaznamy.push((meno.to_string(), typ, v, teraz + Duration::from_secs(ttl as u64)));
    }

    pub fn dlzka(&self) -> usize {
        self.zaznamy.len()
    }
}

/// Prečo sa meno nepreložilo.
#[derive(Debug)]
pub enum ChybaPrekladu {
    ZleMeno(String),
    /// meno neexistuje (NXDOMAIN) alebo nemá adresu
    Neexistuje(String),
    /// nie je známy žiadny server DNS (DHCP ho neposlal)
    BezServera,
    /// žiadny server neodpovedal použiteľne; posledná chyba
    Zlyhanie(String),
}

impl std::fmt::Display for ChybaPrekladu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChybaPrekladu::ZleMeno(m) => write!(f, "neplatné meno „{m}“"),
            ChybaPrekladu::Neexistuje(m) => write!(f, "meno {m} neexistuje alebo nemá adresu"),
            ChybaPrekladu::BezServera => write!(f, "nie je známy server DNS"),
            ChybaPrekladu::Zlyhanie(e) => write!(f, "preklad zlyhal: {e}"),
        }
    }
}

impl std::error::Error for ChybaPrekladu {}

/// Prenos jednej otázky: (server, otázka) → odpoveď; chyba = server neodpovedal včas.
pub type Prenos<'a> = dyn FnMut(SocketAddr, &[u8]) -> io::Result<Vec<u8>> + 'a;

/// Prekladač: servery z DHCP a pamäť. Posielanie robí volajúci cez `prenos(server, otázka) → odpoveď`
/// (UDP s časovým limitom; chyba = server neodpovedal).
#[derive(Debug, Clone, Default)]
pub struct Prekladac {
    pub servery: Vec<IpAddr>,
    pamat: Pamat,
}

impl Prekladac {
    pub fn novy(servery: Vec<IpAddr>) -> Prekladac {
        Prekladac { servery, pamat: Pamat::default() }
    }

    pub fn pamat(&self) -> &Pamat {
        &self.pamat
    }

    /// Adresy pre meno (`typ` A alebo AAAA). Číselná adresa sa vráti hneď. Servery sa skúšajú postupne; na
    /// chybu RCODE 1 (server nepozná EDNS) sa otázka zopakuje bez EDNS.
    pub fn preloz(
        &mut self,
        meno: &str,
        typ: Typ,
        teraz: Duration,
        prenos: &mut Prenos<'_>,
        nahoda: &mut dyn FnMut() -> u16,
    ) -> Result<Vec<IpAddr>, ChybaPrekladu> {
        if let Ok(a) = meno.parse::<IpAddr>() {
            return Ok(vec![a]);
        }
        let m = normalizuj(meno).map_err(|_| ChybaPrekladu::ZleMeno(meno.to_string()))?;
        match self.pamat.najdi(&m, typ, teraz) {
            Some(Vysledok::Adresy(a)) => return Ok(a.clone()),
            Some(Vysledok::Nic) => return Err(ChybaPrekladu::Neexistuje(m)),
            None => {}
        }
        if self.servery.is_empty() {
            return Err(ChybaPrekladu::BezServera);
        }
        let mut posledna = String::from("bez odpovede");
        let mut pytane = m.clone();
        let mut prezyvok = 0;
        'servery: for &server in &self.servery.clone() {
            let mut edns = true;
            loop {
                let id = nahoda();
                let Ok(q) = otazka(id, &pytane, typ, edns) else { break };
                let adresa = SocketAddr::new(server, PORT);
                let odpoved = match prenos(adresa, &q) {
                    Ok(o) => o,
                    Err(e) => {
                        posledna = format!("{server}: {e}");
                        break;
                    }
                };
                match precitaj(&odpoved, id, &pytane, typ) {
                    Ok(Odpoved::Adresy { adresy, ttl, .. }) => {
                        self.pamat.uloz(&m, typ, Vysledok::Adresy(adresy.clone()), ttl, teraz);
                        return Ok(adresy);
                    }
                    Ok(Odpoved::Neexistuje { ttl }) | Ok(Odpoved::BezZaznamu { ttl }) => {
                        self.pamat.uloz(&m, typ, Vysledok::Nic, ttl, teraz);
                        return Err(ChybaPrekladu::Neexistuje(m));
                    }
                    Ok(Odpoved::Prezyvka { kanonicke, .. }) if prezyvok < NAJDLHSIA_RETAZ => {
                        // server poslal len prezývku – ten istý server sa spýta na cieľové meno
                        prezyvok += 1;
                        pytane = kanonicke;
                        continue;
                    }
                    Ok(Odpoved::Prezyvka { .. }) => {
                        posledna = format!("{server}: priveľa prezývok (CNAME)");
                        break 'servery;
                    }
                    Ok(Odpoved::ChybaServera(1)) if edns => {
                        posledna = format!("{server}: nepozná EDNS");
                        edns = false;
                        continue;
                    }
                    Ok(Odpoved::ChybaServera(r)) => {
                        posledna = format!("{server}: chyba servera (RCODE {r})");
                        break;
                    }
                    Ok(Odpoved::Skratena) => {
                        posledna = format!("{server}: odpoveď skrátená (TC)");
                        break;
                    }
                    Err(e) => {
                        posledna = format!("{server}: {e}");
                        break;
                    }
                }
            }
        }
        Err(ChybaPrekladu::Zlyhanie(posledna))
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    /// Odpoveď servera: hlavička, otázka (z `q`), záznamy ako surové bajty.
    fn odpoved(q: &[u8], rcode: u8, tc: bool, an: u16, ns: u16, zaznamy: &[u8]) -> Vec<u8> {
        let mut v = q[..2].to_vec();
        let priznaky: u16 = 0x8180 | rcode as u16 | if tc { 0x0200 } else { 0 };
        v.extend_from_slice(&priznaky.to_be_bytes());
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&an.to_be_bytes());
        v.extend_from_slice(&ns.to_be_bytes());
        v.extend_from_slice(&0u16.to_be_bytes());
        // otázka bez OPT
        let (_, koniec) = citaj_meno(q, 12).unwrap();
        v.extend_from_slice(&q[12..koniec + 4]);
        v.extend_from_slice(zaznamy);
        v
    }

    /// Záznam: meno ako ukazovateľ na otázku (0xC00C) alebo surové meno, typ, TTL, údaje.
    fn zaznam(meno: &[u8], typ: u16, ttl: u32, data: &[u8]) -> Vec<u8> {
        let mut v = meno.to_vec();
        v.extend_from_slice(&typ.to_be_bytes());
        v.extend_from_slice(&TRIEDA_IN.to_be_bytes());
        v.extend_from_slice(&ttl.to_be_bytes());
        v.extend_from_slice(&(data.len() as u16).to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    const OTAZKA: [u8; 2] = [0xC0, 0x0C];

    #[test]
    fn otazka_bajt_po_bajte() {
        let q = otazka(0xBEEF, "Example.COM.", Typ::A, false).unwrap();
        let ocakavane = [
            0xBE, 0xEF, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0, // hlavička: RD, 1 otázka
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm',
            0, // meno malými písmenami
            0, 1, 0, 1, // A, IN
        ];
        assert_eq!(q, ocakavane);
        let e = otazka(1, "a.sk", Typ::Aaaa, true).unwrap();
        assert_eq!(&e[10..12], &[0, 1], "ARCOUNT 1 – OPT");
        assert_eq!(&e[e.len() - 11..], &[0, 0, 41, 0x04, 0xD0, 0, 0, 0, 0, 0, 0], "OPT, 1232 B");
        assert_eq!(&e[18..20], &[0, 28], "typ AAAA");
    }

    #[test]
    fn neplatne_mena() {
        for m in [
            "",
            ".",
            "a..b",
            "medzera v mene.sk",
            &"a".repeat(64),
            &format!("{}.sk", "a.".repeat(130)),
            "ž.sk",
        ] {
            assert!(matches!(otazka(1, m, Typ::A, false), Err(Chyba::ZleMeno(_))), "{m}");
        }
        assert_eq!(normalizuj("_dmarc.Orchidea-OS.sk.").unwrap(), "_dmarc.orchidea-os.sk");
    }

    #[test]
    fn adresa_a_kompresia() {
        let q = otazka(7, "github.com", Typ::A, false).unwrap();
        let z =
            [zaznam(&OTAZKA, 1, 60, &[140, 82, 121, 4]), zaznam(&OTAZKA, 1, 30, &[140, 82, 121, 3])].concat();
        let o = precitaj(&odpoved(&q, 0, false, 2, 0, &z), 7, "github.com", Typ::A).unwrap();
        assert_eq!(
            o,
            Odpoved::Adresy {
                adresy: vec!["140.82.121.4".parse().unwrap(), "140.82.121.3".parse().unwrap()],
                ttl: 30,
                kanonicke: "github.com".into()
            }
        );
    }

    #[test]
    fn retaz_cname_a_cudzie_zaznamy() {
        let q = otazka(9, "www.priklad.sk", Typ::A, false).unwrap();
        // www.priklad.sk CNAME cdn.priklad.sk (meno cieľa: „cdn“ + ukazovateľ na „priklad.sk“ v otázke na 16)
        let cname = zaznam(&OTAZKA, TYP_CNAME, 300, &[3, b'c', b'd', b'n', 0xC0, 16]);
        let koniec_cname = 12 + q.len() - 12 + cname.len();
        // cdn.priklad.sk A 1.2.3.4 – meno ukazuje na údaje CNAME
        let cname_data = koniec_cname - 6;
        let a = zaznam(&[0xC0 | (cname_data >> 8) as u8, cname_data as u8], 1, 120, &[1, 2, 3, 4]);
        // podvrhnutý záznam pre iné meno sa nesmie použiť
        let cudzi = zaznam(&[4, b'b', b'a', b'n', b'k', 2, b's', b'k', 0], 1, 9999, &[6, 6, 6, 6]);
        let d = odpoved(&q, 0, false, 3, 0, &[cname, cudzi, a].concat());
        let o = precitaj(&d, 9, "www.priklad.sk", Typ::A).unwrap();
        assert_eq!(
            o,
            Odpoved::Adresy {
                adresy: vec!["1.2.3.4".parse().unwrap()],
                ttl: 120,
                kanonicke: "cdn.priklad.sk".into()
            }
        );
    }

    #[test]
    fn neexistuje_s_ttl_zo_soa() {
        let q = otazka(3, "nic.priklad.sk", Typ::A, false).unwrap();
        // SOA: MNAME ns (ukazovateľ), RNAME, SERIAL, REFRESH, RETRY, EXPIRE, MINIMUM 900
        let mut soa = vec![2, b'n', b's', 0xC0, 16, 4, b'r', b'o', b'o', b't', 0xC0, 16];
        for c in [2026112801u32, 3600, 600, 86400, 900] {
            soa.extend_from_slice(&c.to_be_bytes());
        }
        let z = zaznam(&[0xC0, 16], TYP_SOA, 3600, &soa);
        let o = precitaj(&odpoved(&q, 3, false, 0, 1, &z), 3, "nic.priklad.sk", Typ::A).unwrap();
        assert_eq!(o, Odpoved::Neexistuje { ttl: 900 });
        let o = precitaj(&odpoved(&q, 0, false, 0, 1, &z), 3, "nic.priklad.sk", Typ::A).unwrap();
        assert_eq!(o, Odpoved::BezZaznamu { ttl: 900 }, "NODATA");
        let o = precitaj(&odpoved(&q, 3, false, 0, 0, &[]), 3, "nic.priklad.sk", Typ::A).unwrap();
        assert_eq!(o, Odpoved::Neexistuje { ttl: 60 }, "bez SOA krátko");
    }

    #[test]
    fn podvrhy_a_poskodene_odpovede() {
        let q = otazka(5, "a.sk", Typ::A, false).unwrap();
        let ok = odpoved(&q, 0, false, 1, 0, &zaznam(&OTAZKA, 1, 60, &[1, 1, 1, 1]));
        assert_eq!(precitaj(&ok, 6, "a.sk", Typ::A), Err(Chyba::InaOtazka), "iné číslo");
        assert_eq!(precitaj(&ok, 5, "b.sk", Typ::A), Err(Chyba::InaOtazka), "iné meno");
        assert_eq!(precitaj(&ok, 5, "a.sk", Typ::Aaaa), Err(Chyba::InaOtazka), "iný typ");
        assert_eq!(precitaj(&q, 5, "a.sk", Typ::A), Err(Chyba::NieJeOdpoved), "otázka namiesto odpovede");
        assert_eq!(precitaj(&ok[..8], 5, "a.sk", Typ::A), Err(Chyba::PrilisKratka));
        let mut skratena = ok.clone();
        skratena.truncate(ok.len() - 2);
        assert!(matches!(precitaj(&skratena, 5, "a.sk", Typ::A), Err(Chyba::Poskodena(_))));
        // ukazovateľ sám na seba (slučka) a dopredu
        let slucka = odpoved(&q, 0, false, 1, 0, &zaznam(&[0xC0, 22], 1, 60, &[1, 1, 1, 1]));
        assert!(matches!(precitaj(&slucka, 5, "a.sk", Typ::A), Err(Chyba::Poskodena(_))));
        assert_eq!(
            precitaj(&odpoved(&q, 0, true, 0, 0, &[]), 5, "a.sk", Typ::A),
            Ok(Odpoved::Skratena),
            "TC"
        );
        assert_eq!(
            precitaj(&odpoved(&q, 2, false, 0, 0, &[]), 5, "a.sk", Typ::A),
            Ok(Odpoved::ChybaServera(2))
        );
        // záznam A so zlou dĺžkou sa ignoruje
        let zla = odpoved(&q, 0, false, 1, 0, &zaznam(&OTAZKA, 1, 60, &[1, 1, 1]));
        assert_eq!(precitaj(&zla, 5, "a.sk", Typ::A), Ok(Odpoved::BezZaznamu { ttl: 60 }));
    }

    #[test]
    fn len_prezyvka_sa_dopyta() {
        // dnsmasq s --cname na meno z --address vráti len CNAME bez adresy (zmerané 28. 11. 2026)
        let q = otazka(4, "alias.orchidea", Typ::A, false).unwrap();
        let cname = zaznam(&OTAZKA, TYP_CNAME, 0, &[6, b's', b'k', b'u', b's', b'k', b'a', 0xC0, 18]);
        let o = precitaj(&odpoved(&q, 0, false, 1, 0, &cname), 4, "alias.orchidea", Typ::A).unwrap();
        assert_eq!(o, Odpoved::Prezyvka { kanonicke: "skuska.orchidea".into(), ttl: 0 });
        let mut k = Prekladac::novy(vec!["10.77.0.1".parse().unwrap()]);
        let mut pytane = Vec::new();
        let mut prenos = |_: SocketAddr, q: &[u8]| -> io::Result<Vec<u8>> {
            let bez_opt = &q[..q.len() - 11];
            let (meno, _) = citaj_meno(bez_opt, 12).unwrap();
            pytane.push(meno.clone());
            Ok(if meno == "alias.orchidea" {
                odpoved(
                    bez_opt,
                    0,
                    false,
                    1,
                    0,
                    &zaznam(&OTAZKA, TYP_CNAME, 0, &[6, b's', b'k', b'u', b's', b'k', b'a', 0xC0, 18]),
                )
            } else {
                odpoved(bez_opt, 0, false, 1, 0, &zaznam(&OTAZKA, 1, 0, &[10, 77, 0, 7]))
            })
        };
        let mut n = || 9u16;
        let a = k.preloz("alias.orchidea", Typ::A, Duration::ZERO, &mut prenos, &mut n).unwrap();
        assert_eq!(a, vec!["10.77.0.7".parse::<IpAddr>().unwrap()]);
        assert_eq!(pytane, ["alias.orchidea", "skuska.orchidea"]);
    }

    #[test]
    fn aaaa() {
        let q = otazka(2, "ipv6.sk", Typ::Aaaa, false).unwrap();
        let a: Ipv6Addr = "2001:db8::1".parse().unwrap();
        let o = precitaj(
            &odpoved(&q, 0, false, 1, 0, &zaznam(&OTAZKA, 28, 10, &a.octets())),
            2,
            "ipv6.sk",
            Typ::Aaaa,
        );
        assert_eq!(
            o,
            Ok(Odpoved::Adresy { adresy: vec![IpAddr::V6(a)], ttl: 10, kanonicke: "ipv6.sk".into() })
        );
    }

    #[test]
    fn pamat_podla_ttl() {
        let mut p = Pamat::default();
        let a = Vysledok::Adresy(vec!["1.1.1.1".parse().unwrap()]);
        p.uloz("a.sk", Typ::A, a.clone(), 60, Duration::from_secs(100));
        assert_eq!(p.najdi("a.sk", Typ::A, Duration::from_secs(159)), Some(&a));
        assert_eq!(p.najdi("a.sk", Typ::A, Duration::from_secs(160)), None, "TTL vypršalo");
        assert_eq!(p.najdi("a.sk", Typ::Aaaa, Duration::from_secs(100)), None);
        p.uloz("b.sk", Typ::A, Vysledok::Nic, 0, Duration::from_secs(100));
        assert_eq!(p.najdi("b.sk", Typ::A, Duration::from_secs(100)), None, "TTL 0 sa neukladá");
        for i in 0..200u32 {
            p.uloz(&format!("m{i}.sk"), Typ::A, Vysledok::Nic, 1000 + i, Duration::from_secs(100));
        }
        assert!(p.dlzka() <= NAJVIAC_V_PAMATI);
        assert!(p.najdi("m199.sk", Typ::A, Duration::from_secs(100)).is_some(), "najnovšie ostáva");
    }

    #[test]
    fn prekladac_skusa_servery_a_pamata_si() {
        let s1: IpAddr = "192.168.1.1".parse().unwrap();
        let s2: IpAddr = "9.9.9.9".parse().unwrap();
        let mut k = Prekladac::novy(vec![s1, s2]);
        let mut id = 100u16;
        let mut nahoda = || {
            id += 1;
            id
        };
        let mut otazky = Vec::new();
        let mut prenos = |kam: SocketAddr, q: &[u8]| -> io::Result<Vec<u8>> {
            otazky.push((kam, q.len()));
            if kam.ip() == "192.168.1.1".parse::<IpAddr>().unwrap() {
                if q[11] == 1 {
                    // starý server: na EDNS odpovie FORMERR, potom zlyhanie servera
                    return Ok(odpoved(&q[..q.len() - 11], 1, false, 0, 0, &[]));
                }
                return Ok(odpoved(q, 2, false, 0, 0, &[]));
            }
            let bez_opt = &q[..q.len() - 11];
            Ok(odpoved(bez_opt, 0, false, 1, 0, &zaznam(&OTAZKA, 1, 300, &[93, 184, 215, 14])))
        };
        let a = k.preloz("example.org", Typ::A, Duration::from_secs(1), &mut prenos, &mut nahoda).unwrap();
        assert_eq!(a, vec!["93.184.215.14".parse::<IpAddr>().unwrap()]);
        assert_eq!(otazky.len(), 3, "s1 s EDNS → FORMERR, s1 bez EDNS → SERVFAIL, s2 s EDNS → adresa");
        assert_eq!(otazky[2].0, SocketAddr::new(s2, PORT));
        let mut bez_siete =
            |_: SocketAddr, _: &[u8]| -> io::Result<Vec<u8>> { panic!("má sa vziať z pamäte") };
        let mut n = || 1u16;
        let b = k.preloz("Example.org.", Typ::A, Duration::from_secs(200), &mut bez_siete, &mut n).unwrap();
        assert_eq!(b, a);
        assert_eq!(k.preloz("10.0.0.1", Typ::A, Duration::ZERO, &mut bez_siete, &mut n).unwrap().len(), 1);
        let mut ticho =
            |_: SocketAddr, _: &[u8]| -> io::Result<Vec<u8>> { Err(io::ErrorKind::TimedOut.into()) };
        assert!(matches!(
            k.preloz("iny.sk", Typ::A, Duration::ZERO, &mut ticho, &mut n),
            Err(ChybaPrekladu::Zlyhanie(_))
        ));
        assert!(matches!(
            Prekladac::novy(vec![]).preloz("iny.sk", Typ::A, Duration::ZERO, &mut ticho, &mut n),
            Err(ChybaPrekladu::BezServera)
        ));
    }
}
