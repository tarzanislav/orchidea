//! Klient HTTP/1.1 – sťahovanie zoznamov staníc IPTV, súborov a živých prúdov (rozhranie R6).
//!
//! Podľa **RFC 9112** (správa HTTP/1.1 – stavový riadok 4, polia 5, dĺžka tela 6.3, kúsky „chunked“ 7.1),
//! **RFC 9110** (sémantika – kódy 1xx 15.2, presmerovania 15.4) a **RFC 3986** (adresa URL – 3, spájanie
//! relatívnej adresy 5.2, bodkové úseky 5.2.4).
//!
//! Telo sa číta **prúdom** ([`Telo`] je `Read`) – živý prúd MPEG-TS nikdy nekončí a nesmie sa celý načítať do pamäte.
//! Klient žiada `Accept-Encoding: identity` (bez kompresie) a `Connection: close` (jedno spojenie = jedna
//! požiadavka – jednoduché a bez skrytých stavov).
//!
//! **HTTPS zatiaľ nie** (vlastné TLS 1.3 je úloha A12): adresa `https://` aj presmerovanie na ňu vráti
//! [`Chyba::Zabezpecene`] – nikdy sa potichu nepošle nezabezpečená požiadavka namiesto zabezpečenej.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::time::Duration;

use crate::dns;

/// Najdlhšia hlavička odpovede (všetky riadky spolu) a jeden riadok.
const NAJDLHSIA_HLAVICKA: usize = 64 * 1024;
const NAJDLHSI_RIADOK: usize = 8 * 1024;
const NAJVIAC_POLI: usize = 128;
/// Najviac presmerovaní (RFC 9110 15.4 – klient má zistiť slučku; 5 ako bežné prehliadače odporúčali v RFC 2068).
pub const NAJVIAC_PRESMEROVANI: usize = 5;
/// Ako sa klient predstaví serveru.
pub const MENO_KLIENTA: &str = "ORCHIDEA/0.1";

/// Chyba sťahovania.
#[derive(Debug)]
pub enum Chyba {
    ZlaAdresa(String),
    /// `https://` – zabezpečené spojenie ešte nie je (A12)
    Zabezpecene(String),
    Preklad(dns::ChybaPrekladu),
    Spojenie(io::Error),
    /// odpoveď porušuje RFC 9112 (stavový riadok, pole, dĺžka, kúsky)
    Poskodena(&'static str),
    /// kódovanie prenosu alebo obsahu, ktoré klient nežiadal (gzip, br …)
    Kodovanie(String),
    PrilisVelaPresmerovani,
}

impl std::fmt::Display for Chyba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Chyba::ZlaAdresa(a) => write!(f, "neplatná adresa „{a}“"),
            Chyba::Zabezpecene(a) => write!(f, "zabezpečené spojenie (HTTPS) zatiaľ nie je: {a}"),
            Chyba::Preklad(e) => write!(f, "{e}"),
            Chyba::Spojenie(e) => write!(f, "spojenie: {e}"),
            Chyba::Poskodena(c) => write!(f, "poškodená odpoveď HTTP ({c})"),
            Chyba::Kodovanie(k) => write!(f, "nepodporované kódovanie „{k}“"),
            Chyba::PrilisVelaPresmerovani => write!(f, "priveľa presmerovaní"),
        }
    }
}

impl std::error::Error for Chyba {}

impl From<io::Error> for Chyba {
    fn from(e: io::Error) -> Chyba {
        Chyba::Spojenie(e)
    }
}

/// Adresa `http://hostiteľ[:port]/cesta?otázka` (RFC 3986). Úlomok (`#…`) sa zahodí – na server nejde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    /// meno alebo číselná adresa (IPv6 bez hranatých zátvoriek)
    pub hostitel: String,
    pub port: u16,
    /// cesta a otázka, vždy začína „/“
    pub cesta: String,
}

impl Url {
    /// Rozoberie absolútnu adresu `http://…`.
    pub fn rozober(t: &str) -> Result<Url, Chyba> {
        let t = t.trim();
        let zla = || Chyba::ZlaAdresa(t.to_string());
        let (schema, zvysok) = t.split_once("://").ok_or_else(zla)?;
        if schema.eq_ignore_ascii_case("https") {
            return Err(Chyba::Zabezpecene(t.to_string()));
        }
        if !schema.eq_ignore_ascii_case("http") {
            return Err(zla());
        }
        let koniec = zvysok.find(['/', '?', '#']).unwrap_or(zvysok.len());
        let (autorita, cesta) = zvysok.split_at(koniec);
        if autorita.contains('@') || autorita.is_empty() {
            return Err(zla()); // meno a heslo v adrese sa neposielajú (RFC 3986 3.2.1 – zastarané)
        }
        let (hostitel, port) = if let Some(z) = autorita.strip_prefix('[') {
            let (h, za) = z.split_once(']').ok_or_else(zla)?;
            h.parse::<std::net::Ipv6Addr>().map_err(|_| zla())?;
            (h.to_string(), za.strip_prefix(':'))
        } else {
            match autorita.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (autorita.to_string(), None),
            }
        };
        let port = match port {
            None | Some("") => 80,
            Some(p) => p.parse::<u16>().ok().filter(|&p| p > 0).ok_or_else(zla)?,
        };
        if hostitel.is_empty() || !hostitel.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_:".contains(&b))
        {
            return Err(zla());
        }
        Ok(Url {
            hostitel: hostitel.to_ascii_lowercase(),
            port,
            cesta: normalizuj_cestu(cesta).ok_or_else(zla)?,
        })
    }

    /// Spojí adresu z poľa `Location` s touto (RFC 3986 5.2): absolútna, bez schémy (`//…`), od koreňa (`/…`)
    /// alebo relatívna k priečinku tejto cesty.
    pub fn spoj(&self, odkaz: &str) -> Result<Url, Chyba> {
        let o = odkaz.trim();
        if o.contains("://") {
            return Url::rozober(o);
        }
        if let Some(z) = o.strip_prefix("//") {
            return Url::rozober(&format!("http://{z}"));
        }
        let cesta = if o.starts_with('/') {
            o.to_string()
        } else if o.starts_with('?') {
            format!("{}{o}", self.cesta.split('?').next().unwrap_or("/"))
        } else {
            let zaklad = self.cesta.split('?').next().unwrap_or("/");
            format!("{}{o}", &zaklad[..zaklad.rfind('/').map_or(0, |i| i + 1)])
        };
        Ok(Url {
            hostitel: self.hostitel.clone(),
            port: self.port,
            cesta: normalizuj_cestu(&cesta).ok_or_else(|| Chyba::ZlaAdresa(o.to_string()))?,
        })
    }

    /// Hodnota poľa `Host` (port len ak nie je 80; IPv6 v hranatých zátvorkách).
    pub fn host(&self) -> String {
        let h =
            if self.hostitel.contains(':') { format!("[{}]", self.hostitel) } else { self.hostitel.clone() };
        if self.port == 80 { h } else { format!("{h}:{}", self.port) }
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "http://{}{}", self.host(), self.cesta)
    }
}

/// Cesta bez úlomku, s odstránenými úsekmi „.“ a „..“ (RFC 3986 5.2.4), medzery ako %20; riadiace znaky nie.
fn normalizuj_cestu(c: &str) -> Option<String> {
    let c = c.split('#').next().unwrap_or("");
    if c.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return None;
    }
    let c = c.replace(' ', "%20");
    let (cesta, otazka) = match c.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (c, None),
    };
    let mut useky: Vec<&str> = Vec::new();
    let casti: Vec<&str> = cesta.split('/').collect();
    for (i, u) in casti.iter().enumerate() {
        let posledny = i + 1 == casti.len();
        match *u {
            "." => {
                if posledny {
                    useky.push("");
                }
            }
            ".." => {
                if useky.len() > 1 {
                    useky.pop();
                }
                if posledny {
                    useky.push("");
                }
            }
            _ => useky.push(u),
        }
    }
    let mut v = useky.join("/");
    if !v.starts_with('/') {
        v.insert(0, '/');
    }
    if let Some(q) = otazka {
        v.push('?');
        v.push_str(&q);
    }
    Some(v)
}

/// Požiadavka GET (RFC 9112 3): bez kompresie, spojenie sa po odpovedi zavrie. `navyse` = ďalšie polia
/// (napr. `Range` na pokračovanie sťahovania).
pub fn ziadost(url: &Url, navyse: &[(&str, &str)]) -> Vec<u8> {
    let mut t = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {MENO_KLIENTA}\r\nAccept: */*\r\nAccept-Encoding: identity\r\n\
         Connection: close\r\n",
        url.cesta,
        url.host()
    );
    for (k, v) in navyse {
        if !k.bytes().any(|b| b < 0x21) && !v.bytes().any(|b| b == b'\r' || b == b'\n') {
            t += &format!("{k}: {v}\r\n");
        }
    }
    t += "\r\n";
    t.into_bytes()
}

/// Stavový riadok a polia odpovede.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hlavicka {
    pub stav: u16,
    pub dovod: String,
    /// polia v poradí; mená malými písmenami
    pub polia: Vec<(String, String)>,
}

impl Hlavicka {
    /// Prvé pole s menom (bez ohľadu na veľkosť písmen).
    pub fn pole(&self, meno: &str) -> Option<&str> {
        let m = meno.to_ascii_lowercase();
        self.polia.iter().find(|(k, _)| *k == m).map(|(_, v)| v.as_str())
    }

    /// Všetky hodnoty poľa spojené čiarkou (RFC 9110 5.3).
    pub fn pole_vsetky(&self, meno: &str) -> Option<String> {
        let m = meno.to_ascii_lowercase();
        let v: Vec<&str> = self.polia.iter().filter(|(k, _)| *k == m).map(|(_, v)| v.as_str()).collect();
        (!v.is_empty()).then(|| v.join(", "))
    }
}

fn citaj_riadok<R: BufRead>(r: &mut R, spolu: &mut usize) -> Result<String, Chyba> {
    let mut v = Vec::new();
    loop {
        let b = r.fill_buf()?;
        if b.is_empty() {
            return Err(Chyba::Poskodena("spojenie skončilo uprostred hlavičky"));
        }
        let (kus, nasiel) = match b.iter().position(|&x| x == b'\n') {
            Some(i) => (i + 1, true),
            None => (b.len(), false),
        };
        v.extend_from_slice(&b[..kus]);
        r.consume(kus);
        *spolu += kus;
        if v.len() > NAJDLHSI_RIADOK || *spolu > NAJDLHSIA_HLAVICKA {
            return Err(Chyba::Poskodena("príliš dlhá hlavička"));
        }
        if nasiel {
            break;
        }
    }
    v.pop();
    if v.last() == Some(&b'\r') {
        v.pop(); // RFC 9112 2.2: riadok končí CRLF, samotné LF klient smie prijať
    }
    String::from_utf8(v).map_err(|_| Chyba::Poskodena("hlavička nie je text"))
}

/// Prečíta stavový riadok a polia; medziodpovede 1xx (okrem 101) preskočí (RFC 9110 15.2).
pub fn precitaj_hlavicku<R: BufRead>(r: &mut R) -> Result<Hlavicka, Chyba> {
    let mut spolu = 0;
    loop {
        let riadok = citaj_riadok(r, &mut spolu)?;
        let (verzia, zvysok) = riadok.split_once(' ').ok_or(Chyba::Poskodena("stavový riadok"))?;
        if !verzia.starts_with("HTTP/1.") || verzia.len() != 8 {
            return Err(Chyba::Poskodena("nie je HTTP/1.x"));
        }
        let (kod, dovod) = zvysok.split_once(' ').unwrap_or((zvysok, ""));
        if kod.len() != 3 {
            return Err(Chyba::Poskodena("stavový kód"));
        }
        let stav: u16 = kod.parse().map_err(|_| Chyba::Poskodena("stavový kód"))?;
        let mut polia = Vec::new();
        loop {
            let p = citaj_riadok(r, &mut spolu)?;
            if p.is_empty() {
                break;
            }
            if p.starts_with([' ', '\t']) {
                return Err(Chyba::Poskodena("zložené pole (obs-fold)")); // RFC 9112 5.2
            }
            let (k, v) = p.split_once(':').ok_or(Chyba::Poskodena("pole bez dvojbodky"))?;
            if k.is_empty()
                || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            {
                return Err(Chyba::Poskodena("meno poľa"));
            }
            polia.push((k.to_ascii_lowercase(), v.trim_matches([' ', '\t']).to_string()));
            if polia.len() > NAJVIAC_POLI {
                return Err(Chyba::Poskodena("priveľa polí"));
            }
        }
        if (100..200).contains(&stav) && stav != 101 {
            continue;
        }
        return Ok(Hlavicka { stav, dovod: dovod.to_string(), polia });
    }
}

/// Ako je ohraničené telo (RFC 9112 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ramec {
    /// bez tela (HEAD, 1xx, 204, 304)
    Ziadne,
    /// presne toľko bajtov (Content-Length)
    Dlzka(u64),
    /// kúsky (Transfer-Encoding: chunked)
    Kusy,
    /// do konca spojenia
    DoKonca,
}

/// Ohraničenie tela odpovede na GET.
pub fn ramec(h: &Hlavicka) -> Result<Ramec, Chyba> {
    if (100..200).contains(&h.stav) || h.stav == 204 || h.stav == 304 {
        return Ok(Ramec::Ziadne);
    }
    if let Some(k) = h.pole_vsetky("content-encoding")
        && !k.split(',').all(|x| x.trim().eq_ignore_ascii_case("identity"))
    {
        return Err(Chyba::Kodovanie(k));
    }
    if let Some(te) = h.pole_vsetky("transfer-encoding") {
        let kodovania: Vec<String> = te.split(',').map(|x| x.trim().to_ascii_lowercase()).collect();
        return match kodovania.as_slice() {
            [.., p] if p == "chunked" && kodovania[..kodovania.len() - 1].iter().all(|k| k == "identity") => {
                Ok(Ramec::Kusy)
            }
            _ if kodovania.iter().all(|k| k == "identity") => Ok(Ramec::DoKonca),
            _ => Err(Chyba::Kodovanie(te)),
        };
    }
    if let Some(d) = h.pole_vsetky("content-length") {
        // RFC 9112 6.3 bod 5: viac rôznych hodnôt = chyba; rovnaké sa smú opakovať
        let mut hodnoty = d.split(',').map(|x| x.trim());
        let prva = hodnoty.next().unwrap_or("");
        if !prva.bytes().all(|b| b.is_ascii_digit()) || prva.is_empty() || hodnoty.any(|x| x != prva) {
            return Err(Chyba::Poskodena("Content-Length"));
        }
        return prva.parse().map(Ramec::Dlzka).map_err(|_| Chyba::Poskodena("Content-Length"));
    }
    Ok(Ramec::DoKonca)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StavKusov {
    Velkost,
    Data(u64),
    KoniecKusa,
    Hotovo,
}

/// Telo odpovede čítané prúdom podľa [`Ramec`]. Predčasný koniec spojenia je chyba `UnexpectedEof`.
#[derive(Debug)]
pub struct Telo<R> {
    r: R,
    ramec: Ramec,
    zostava: u64,
    kusy: StavKusov,
}

impl<R: BufRead> Telo<R> {
    pub fn nove(r: R, ramec: Ramec) -> Telo<R> {
        let zostava = if let Ramec::Dlzka(n) = ramec { n } else { 0 };
        Telo { r, ramec, zostava, kusy: StavKusov::Velkost }
    }

    pub fn ramec(&self) -> Ramec {
        self.ramec
    }

    /// Prečíta celé telo, najviac `najviac` bajtov (zoznam staníc, malý súbor) – viac je chyba.
    pub fn cele(&mut self, najviac: usize) -> io::Result<Vec<u8>> {
        let mut v = Vec::new();
        (&mut *self).take(najviac as u64 + 1).read_to_end(&mut v)?;
        if v.len() > najviac {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "telo je väčšie, ako je dovolené"));
        }
        Ok(v)
    }

    fn riadok_kusov(&mut self) -> io::Result<String> {
        let mut spolu = 0;
        citaj_riadok(&mut self.r, &mut spolu).map_err(|e| match e {
            Chyba::Spojenie(e) => e,
            _ => io::Error::new(io::ErrorKind::UnexpectedEof, "poškodený kúsok tela"),
        })
    }
}

fn zle(c: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, c)
}

impl<R: BufRead> Read for Telo<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.ramec {
            Ramec::Ziadne => Ok(0),
            Ramec::DoKonca => self.r.read(buf),
            Ramec::Dlzka(_) => {
                if self.zostava == 0 {
                    return Ok(0);
                }
                let n = buf.len().min(self.zostava.min(usize::MAX as u64) as usize);
                let n = self.r.read(&mut buf[..n])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "telo kratšie ako Content-Length",
                    ));
                }
                self.zostava -= n as u64;
                Ok(n)
            }
            Ramec::Kusy => loop {
                match self.kusy {
                    StavKusov::Hotovo => return Ok(0),
                    StavKusov::Velkost => {
                        let r = self.riadok_kusov()?;
                        let hex = r.split(';').next().unwrap_or("").trim_matches([' ', '\t']);
                        if hex.is_empty() || hex.len() > 16 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return Err(zle("veľkosť kúska"));
                        }
                        let n = u64::from_str_radix(hex, 16).map_err(|_| zle("veľkosť kúska"))?;
                        if n == 0 {
                            // záverečné polia (trailer) až po prázdny riadok – zahodia sa
                            while !self.riadok_kusov()?.is_empty() {}
                            self.kusy = StavKusov::Hotovo;
                            return Ok(0);
                        }
                        self.kusy = StavKusov::Data(n);
                    }
                    StavKusov::Data(zvysok) => {
                        let n = buf.len().min(zvysok.min(usize::MAX as u64) as usize);
                        let n = self.r.read(&mut buf[..n])?;
                        if n == 0 {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "kúsok tela nedokončený",
                            ));
                        }
                        self.kusy = if zvysok == n as u64 {
                            StavKusov::KoniecKusa
                        } else {
                            StavKusov::Data(zvysok - n as u64)
                        };
                        return Ok(n);
                    }
                    StavKusov::KoniecKusa => {
                        if !self.riadok_kusov()?.is_empty() {
                            return Err(zle("kúsok bez CRLF na konci"));
                        }
                        self.kusy = StavKusov::Velkost;
                    }
                }
            },
        }
    }
}

/// Odpoveď: hlavička a telo čítané prúdom.
#[derive(Debug)]
pub struct Odpoved<R> {
    pub hlavicka: Hlavicka,
    pub telo: Telo<R>,
    /// adresa, z ktorej odpoveď naozaj prišla (po presmerovaniach)
    pub url: Url,
}

/// Pošle požiadavku cez hotové spojenie a prečíta hlavičku odpovede.
pub fn ziadaj<S: Read + Write>(
    mut spojenie: S,
    url: &Url,
    navyse: &[(&str, &str)],
) -> Result<Odpoved<BufReader<S>>, Chyba> {
    spojenie.write_all(&ziadost(url, navyse))?;
    spojenie.flush()?;
    let mut r = BufReader::with_capacity(16 * 1024, spojenie);
    let hlavicka = precitaj_hlavicku(&mut r)?;
    let ramec = ramec(&hlavicka)?;
    Ok(Odpoved { hlavicka, telo: Telo::nove(r, ramec), url: url.clone() })
}

/// Nastavenie sťahovania cez sieť.
#[derive(Debug, Clone, Copy)]
pub struct Nastavenie {
    /// najdlhšie čakanie na spojenie a na každé čítanie
    pub cakanie: Duration,
    pub presmerovani: usize,
}

impl Default for Nastavenie {
    fn default() -> Nastavenie {
        Nastavenie { cakanie: Duration::from_secs(10), presmerovani: NAJVIAC_PRESMEROVANI }
    }
}

/// Otázka DNS cez UDP so zásuvkou spojenou len so serverom (odpoveď od iného sa neprijme).
pub fn otazka_udp(kam: SocketAddr, data: &[u8]) -> io::Result<Vec<u8>> {
    let s = UdpSocket::bind(if kam.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    s.connect(kam)?;
    s.send(data)?;
    let mut b = vec![0u8; 2048];
    let n = s.recv(&mut b)?;
    b.truncate(n);
    Ok(b)
}

/// Stiahne adresu: preklad mena (servery z `prekladac`), spojenie TCP, požiadavka GET, presmerovania 301, 302,
/// 303, 307, 308 v rámci HTTP. Vráti odpoveď s telom na čítanie prúdom (aj chybové kódy 4xx a 5xx – rozhodne
/// volajúci). `teraz` = monotónny čas volajúceho (platnosť pamäte prekladu), `nahoda` = čísla otázok DNS.
pub fn stiahni(
    adresa: &str,
    navyse: &[(&str, &str)],
    prekladac: &mut dns::Prekladac,
    teraz: Duration,
    nahoda: &mut dyn FnMut() -> u16,
    nastavenie: Nastavenie,
) -> Result<Odpoved<BufReader<TcpStream>>, Chyba> {
    let mut url = Url::rozober(adresa)?;
    for _ in 0..=nastavenie.presmerovani {
        let adresy: Vec<IpAddr> = match url.hostitel.parse::<IpAddr>() {
            Ok(a) => vec![a],
            Err(_) => prekladac
                .preloz(&url.hostitel, dns::Typ::A, teraz, &mut otazka_udp, nahoda)
                .map_err(Chyba::Preklad)?,
        };
        let mut posledna = io::Error::new(io::ErrorKind::NotFound, "bez adresy");
        let mut spojenie = None;
        for a in adresy {
            match TcpStream::connect_timeout(&SocketAddr::new(a, url.port), nastavenie.cakanie) {
                Ok(s) => {
                    spojenie = Some(s);
                    break;
                }
                Err(e) => posledna = e,
            }
        }
        let s = spojenie.ok_or(Chyba::Spojenie(posledna))?;
        s.set_read_timeout(Some(nastavenie.cakanie))?;
        s.set_write_timeout(Some(nastavenie.cakanie))?;
        let o = ziadaj(s, &url, navyse)?;
        match (o.hlavicka.stav, o.hlavicka.pole("location")) {
            (301 | 302 | 303 | 307 | 308, Some(kam)) => url = url.spoj(kam)?,
            _ => return Ok(o),
        }
    }
    Err(Chyba::PrilisVelaPresmerovani)
}

#[cfg(test)]
mod testy {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn adresy() {
        let u = Url::rozober("http://Example.COM:8080/a/b?x=1&y=2#cast").unwrap();
        assert_eq!(u, Url { hostitel: "example.com".into(), port: 8080, cesta: "/a/b?x=1&y=2".into() });
        assert_eq!(u.host(), "example.com:8080");
        assert_eq!(u.to_string(), "http://example.com:8080/a/b?x=1&y=2");
        let u = Url::rozober("http://10.0.0.1").unwrap();
        assert_eq!((u.port, u.cesta.as_str(), u.host()), (80, "/", "10.0.0.1".to_string()));
        let u = Url::rozober("http://[2001:db8::1]:81/zoznam.m3u").unwrap();
        assert_eq!((u.hostitel.as_str(), u.host()), ("2001:db8::1", "[2001:db8::1]:81".to_string()));
        assert_eq!(Url::rozober("http://h/a b.m3u").unwrap().cesta, "/a%20b.m3u");
        assert_eq!(Url::rozober("http://h/a/./b/../c").unwrap().cesta, "/a/c");
        assert!(matches!(Url::rozober("https://h/"), Err(Chyba::Zabezpecene(_))));
        assert!(matches!(Url::rozober("HTTPS://h/"), Err(Chyba::Zabezpecene(_))));
        for zla in [
            "ftp://h/",
            "http://",
            "http://meno:heslo@h/",
            "http://h:0/",
            "http://h:99999/",
            "h/",
            "http://h/a\tb",
        ] {
            assert!(matches!(Url::rozober(zla), Err(Chyba::ZlaAdresa(_))), "{zla}");
        }
    }

    #[test]
    fn spajanie_adries() {
        let b = Url::rozober("http://h.sk/a/b/c?q=1").unwrap();
        assert_eq!(b.spoj("d").unwrap().cesta, "/a/b/d");
        assert_eq!(b.spoj("../d").unwrap().cesta, "/a/d");
        assert_eq!(b.spoj("/x").unwrap().cesta, "/x");
        assert_eq!(b.spoj("?z=2").unwrap().cesta, "/a/b/c?z=2");
        assert_eq!(b.spoj("//iny.sk/y").unwrap(), Url::rozober("http://iny.sk/y").unwrap());
        assert_eq!(b.spoj("http://iny.sk:81/").unwrap().port, 81);
        assert!(
            matches!(b.spoj("https://iny.sk/"), Err(Chyba::Zabezpecene(_))),
            "nikdy potichu bez zabezpečenia"
        );
        assert_eq!(b.spoj("../../../../x").unwrap().cesta, "/x", "nad koreň sa nedá");
    }

    #[test]
    fn poziadavka_bajt_po_bajte() {
        let u = Url::rozober("http://iptv.priklad.sk:8000/get.php?u=a&p=b").unwrap();
        let z = ziadost(&u, &[("Range", "bytes=100-"), ("Zle\r\nPole", "x"), ("X", "a\r\nInjekcia: 1")]);
        assert_eq!(
            String::from_utf8(z).unwrap(),
            "GET /get.php?u=a&p=b HTTP/1.1\r\nHost: iptv.priklad.sk:8000\r\nUser-Agent: ORCHIDEA/0.1\r\nAccept: */*\r\n\
             Accept-Encoding: identity\r\nConnection: close\r\nRange: bytes=100-\r\n\r\n"
        );
    }

    fn hlavicka(t: &str) -> Result<Hlavicka, Chyba> {
        precitaj_hlavicku(&mut Cursor::new(t.as_bytes().to_vec()))
    }

    #[test]
    fn hlavicky() {
        let h = hlavicka("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: audio/x-mpegurl\r\nSet-Cookie: a\r\nset-cookie: b\r\n\r\n")
            .unwrap();
        assert_eq!((h.stav, h.dovod.as_str()), (200, "OK"), "100 Continue sa preskočí");
        assert_eq!(h.pole("CONTENT-TYPE"), Some("audio/x-mpegurl"));
        assert_eq!(h.pole_vsetky("set-cookie").as_deref(), Some("a, b"));
        assert_eq!(
            hlavicka("HTTP/1.0 404\nX:  y \n\n").unwrap().pole("x"),
            Some("y"),
            "samotné LF, bez dôvodu"
        );
        assert!(
            matches!(hlavicka("HTTP/1.1 200 OK\r\nA: b\r\n c\r\n\r\n"), Err(Chyba::Poskodena(_))),
            "obs-fold"
        );
        assert!(matches!(hlavicka("HTTP/2 200 OK\r\n\r\n"), Err(Chyba::Poskodena(_))));
        assert!(matches!(hlavicka("HTTP/1.1 20 OK\r\n\r\n"), Err(Chyba::Poskodena(_))));
        assert!(matches!(hlavicka("HTTP/1.1 200 OK\r\nZle meno: x\r\n\r\n"), Err(Chyba::Poskodena(_))));
        assert!(matches!(hlavicka("HTTP/1.1 200 OK\r\n"), Err(Chyba::Poskodena(_))), "koniec uprostred");
        let dlha = format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "a".repeat(NAJDLHSI_RIADOK));
        assert!(matches!(hlavicka(&dlha), Err(Chyba::Poskodena(_))));
    }

    fn odpoved(t: &[u8]) -> (Hlavicka, Telo<Cursor<Vec<u8>>>) {
        let mut c = Cursor::new(t.to_vec());
        let h = precitaj_hlavicku(&mut c).unwrap();
        let r = ramec(&h).unwrap();
        (h, Telo::nove(c, r))
    }

    #[test]
    fn telo_podla_dlzky_a_do_konca() {
        let (_, mut t) = odpoved(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nahoj!navyse");
        assert_eq!(t.cele(100).unwrap(), b"ahoj!", "za dĺžkou sa nečíta");
        let (_, mut t) = odpoved(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nkratke");
        assert_eq!(t.cele(100).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        let (_, mut t) = odpoved(b"HTTP/1.1 200 OK\r\n\r\nvsetko do konca");
        assert_eq!(t.cele(100).unwrap(), b"vsetko do konca");
        let (_, mut t) = odpoved(b"HTTP/1.1 200 OK\r\n\r\n0123456789");
        assert_eq!(t.cele(5).unwrap_err().kind(), io::ErrorKind::InvalidData, "limit");
        let (_, mut t) = odpoved(b"HTTP/1.1 304 Not Modified\r\nContent-Length: 50\r\n\r\n");
        assert_eq!(t.ramec(), Ramec::Ziadne);
        assert_eq!(t.cele(10).unwrap(), b"");
    }

    #[test]
    fn telo_po_kuskoch() {
        let (_, mut t) = odpoved(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 999\r\n\r\n\
              5;meno=hodnota\r\n#EXTM\r\n3\r\nU\n\n\r\n0\r\nZaver: x\r\n\r\nZA KONCOM",
        );
        assert_eq!(t.ramec(), Ramec::Kusy, "Transfer-Encoding má prednosť pred Content-Length");
        assert_eq!(t.cele(100).unwrap(), b"#EXTMU\n\n");
        let (_, mut t) = odpoved(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n");
        assert_eq!(t.cele(100).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let (_, mut t) = odpoved(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabc");
        assert_eq!(t.cele(100).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        let (_, mut t) =
            odpoved(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabcX\r\n0\r\n\r\n");
        assert_eq!(t.cele(100).unwrap_err().kind(), io::ErrorKind::InvalidData, "chýba CRLF za kúskom");
        let (_, mut t) =
            odpoved(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n11111111111111111\r\n");
        assert_eq!(t.cele(100).unwrap_err().kind(), io::ErrorKind::InvalidData, "pretečenie veľkosti");
    }

    #[test]
    fn nepodporovane_a_zle_dlzky() {
        let r = |t: &str| ramec(&hlavicka(t).unwrap());
        assert!(matches!(r("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n\r\n"), Err(Chyba::Kodovanie(_))));
        assert!(matches!(
            r("HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n"),
            Err(Chyba::Kodovanie(_))
        ));
        assert!(matches!(r("HTTP/1.1 200 OK\r\nContent-Length: 5, 6\r\n\r\n"), Err(Chyba::Poskodena(_))));
        assert!(matches!(r("HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n"), Err(Chyba::Poskodena(_))));
        assert_eq!(
            r("HTTP/1.1 200 OK\r\nContent-Length: 7\r\nContent-Length: 7\r\n\r\n").unwrap(),
            Ramec::Dlzka(7)
        );
        assert_eq!(r("HTTP/1.1 204 No Content\r\n\r\n").unwrap(), Ramec::Ziadne);
    }

    /// Spojenie v pamäti: čo klient zapíše, sa uloží; čítanie vráti pripravenú odpoveď.
    struct Pamat {
        odpoved: Cursor<Vec<u8>>,
        zapisane: Vec<u8>,
    }
    impl Read for Pamat {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.odpoved.read(b)
        }
    }
    impl Write for Pamat {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.zapisane.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn ziadost_cez_spojenie() {
        let p = Pamat {
            odpoved: Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()),
            zapisane: vec![],
        };
        let u = Url::rozober("http://h/z").unwrap();
        let mut o = ziadaj(p, &u, &[]).unwrap();
        assert_eq!(o.hlavicka.stav, 200);
        assert_eq!(o.telo.cele(10).unwrap(), b"ok");
    }

    #[test]
    #[cfg_attr(target_family = "wasm", ignore)]
    fn stiahnutie_s_presmerovanim_cez_tcp() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut ziadosti = Vec::new();
            for (i, s) in l.incoming().take(2).enumerate() {
                let mut s = s.unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut prva = String::new();
                r.read_line(&mut prva).unwrap();
                ziadosti.push(prva.trim_end().to_string());
                let mut riadok = String::new();
                while r.read_line(&mut riadok).unwrap() > 2 {
                    riadok.clear();
                }
                let o = if i == 0 {
                    "HTTP/1.1 302 Found\r\nLocation: /nove/zoznam.m3u\r\nContent-Length: 0\r\n\r\n"
                        .to_string()
                } else {
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n#EXTM3U\r\n0\r\n\r\n"
                        .to_string()
                };
                s.write_all(o.as_bytes()).unwrap();
            }
            ziadosti
        });
        let mut k = dns::Prekladac::novy(vec![]);
        let mut n = || 1u16;
        let mut o = stiahni(
            &format!("http://127.0.0.1:{port}/stare.m3u"),
            &[],
            &mut k,
            Duration::ZERO,
            &mut n,
            Nastavenie::default(),
        )
        .unwrap();
        assert_eq!(o.telo.cele(1000).unwrap(), b"#EXTM3U");
        assert_eq!(o.url.cesta, "/nove/zoznam.m3u");
        assert_eq!(server.join().unwrap(), ["GET /stare.m3u HTTP/1.1", "GET /nove/zoznam.m3u HTTP/1.1"]);
    }
}
