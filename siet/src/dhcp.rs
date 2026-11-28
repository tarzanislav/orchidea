//! Klient DHCPv4 – adresa IPv4, maska, brána a servery DNS a NTP z domáceho routera.
//!
//! Podľa **RFC 2131** (protokol, stavy klienta – obr. 5, polia správ – tab. 5, opakovanie – kap. 4.1, obnova –
//! kap. 4.4.5) a **RFC 2132** (voľby). Opakované voľby sa spájajú podľa **RFC 3396**. Pevná časť správy je
//! z BOOTP (**RFC 951**).
//!
//! Automat je čistý: [`Klient::tik`] a [`Klient::prijmi`] vrátia [`Akcia`] – čo poslať a čo nastaviť v jadre.
//! Posielanie (UDP 68 → 67) a nastavenie adresy (rtnetlink) robí volajúci. Čas je monotónny
//! ([`Duration`] od štartu volajúceho), náhoda (číslo transakcie, rozptyl opakovania) prichádza zvonka.
//!
//! Vedomé zjednodušenia (zapísané, nie zabudnuté): nekontroluje sa kolízia adresy cez ARP (RFC 2131 4.4.1 –
//! „SHOULD“), neposiela sa DHCPRELEASE (krabička sa vypína odpojením napájania) a meno zariadenia (voľba 12) sa
//! neposiela – router sa nedozvie nič navyše o domácnosti.

use std::net::Ipv4Addr;
use std::time::Duration;

/// Port servera (RFC 2131, kap. 4.1).
pub const PORT_SERVERA: u16 = 67;
/// Port klienta.
pub const PORT_KLIENTA: u16 = 68;
/// Najväčšia správa, ktorú klient prijme (voľba 57; zásuvka volajúceho má aspoň takú vyrovnávaciu pamäť).
pub const NAJVACSIA_SPRAVA: u16 = 1500;

const COOKIE: [u8; 4] = [99, 130, 83, 99];
/// Pevná časť správy po pole `file` vrátane (RFC 2131, obr. 1).
const PEVNA: usize = 236;
/// Najkratšia správa BOOTP (RFC 951) – niektoré servery kratšie zahodia.
const NAJKRATSIA: usize = 300;
const BOOTREQUEST: u8 = 1;
const BOOTREPLY: u8 = 2;
/// Príznak „odpovedz všetkým“ (broadcast) – klient bez adresy nevie prijať odpoveď na jednu adresu.
const PRIZNAK_VSETKYM: u16 = 0x8000;

/// Kódy volieb (RFC 2132).
pub mod volba {
    pub const VYPLN: u8 = 0;
    pub const MASKA: u8 = 1;
    pub const BRANA: u8 = 3;
    pub const DNS: u8 = 6;
    pub const DOMENA: u8 = 15;
    pub const MTU: u8 = 26;
    pub const NTP: u8 = 42;
    pub const ZIADANA_ADRESA: u8 = 50;
    pub const PRENAJOM: u8 = 51;
    pub const PRETAZENIE: u8 = 52;
    pub const TYP: u8 = 53;
    pub const SERVER: u8 = 54;
    pub const ZOZNAM: u8 = 55;
    pub const SPRAVA: u8 = 56;
    pub const NAJVACSIA: u8 = 57;
    pub const T1: u8 = 58;
    pub const T2: u8 = 59;
    pub const ID_KLIENTA: u8 = 61;
    pub const KONIEC: u8 = 255;
}

/// Voľby, o ktoré klient žiada (voľba 55): maska, brána, DNS, doména, MTU, NTP, prenájom, T1, T2.
const ZIADANE: [u8; 9] = [
    volba::MASKA,
    volba::BRANA,
    volba::DNS,
    volba::DOMENA,
    volba::MTU,
    volba::NTP,
    volba::PRENAJOM,
    volba::T1,
    volba::T2,
];

/// Typ správy DHCP (voľba 53).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typ {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

impl Typ {
    fn z(cislo: u8) -> Option<Typ> {
        Some(match cislo {
            1 => Typ::Discover,
            2 => Typ::Offer,
            3 => Typ::Request,
            4 => Typ::Decline,
            5 => Typ::Ack,
            6 => Typ::Nak,
            7 => Typ::Release,
            8 => Typ::Inform,
            _ => return None,
        })
    }
}

/// Chyba pri čítaní správy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chyba {
    PrilisKratka(usize),
    ZlyHardver { htype: u8, hlen: u8 },
    BezCookie,
    PoskodenaVolba(u8),
}

impl std::fmt::Display for Chyba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Chyba::PrilisKratka(n) => write!(f, "správa DHCP je príliš krátka ({n} B)"),
            Chyba::ZlyHardver { htype, hlen } => {
                write!(f, "nie je pre Ethernet (htype {htype}, hlen {hlen})")
            }
            Chyba::BezCookie => write!(f, "chýba magické číslo DHCP (99.130.83.99)"),
            Chyba::PoskodenaVolba(k) => write!(f, "voľba {k} presahuje koniec správy"),
        }
    }
}

impl std::error::Error for Chyba {}

/// Správa DHCP (RFC 2131, obr. 1) pre Ethernet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sprava {
    /// 1 = požiadavka klienta, 2 = odpoveď servera
    pub op: u8,
    /// číslo transakcie
    pub xid: u32,
    /// sekundy od začiatku získavania adresy
    pub secs: u16,
    /// príznak „odpovedz všetkým“
    pub vsetkym: bool,
    pub ciaddr: Ipv4Addr,
    pub yiaddr: Ipv4Addr,
    pub siaddr: Ipv4Addr,
    pub giaddr: Ipv4Addr,
    /// adresa MAC klienta
    pub chaddr: [u8; 6],
    /// voľby v poradí prvého výskytu; opakované kódy sú spojené (RFC 3396)
    pub volby: Vec<(u8, Vec<u8>)>,
}

impl Sprava {
    fn ziadost(mac: [u8; 6], xid: u32, secs: u16) -> Sprava {
        Sprava {
            op: BOOTREQUEST,
            xid,
            secs,
            vsetkym: false,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: mac,
            volby: Vec::new(),
        }
    }

    /// Pridá voľbu (dlhšiu ako 255 B rozdelí na viac volieb s tým istým kódom – RFC 3396).
    pub fn s_volbou(mut self, kod: u8, data: &[u8]) -> Sprava {
        self.volby.push((kod, data.to_vec()));
        self
    }

    /// Hodnota voľby.
    pub fn volba(&self, kod: u8) -> Option<&[u8]> {
        self.volby.iter().find(|(k, _)| *k == kod).map(|(_, d)| d.as_slice())
    }

    /// Typ správy (voľba 53).
    pub fn typ(&self) -> Option<Typ> {
        match self.volba(volba::TYP) {
            Some([t]) => Typ::z(*t),
            _ => None,
        }
    }

    /// Jedna adresa IPv4 z voľby (presne 4 bajty).
    pub fn adresa(&self, kod: u8) -> Option<Ipv4Addr> {
        match self.volba(kod) {
            Some(&[a, b, c, d]) => Some(Ipv4Addr::new(a, b, c, d)),
            _ => None,
        }
    }

    /// Zoznam adries IPv4 z voľby (násobok 4 bajtov, inak prázdny).
    pub fn adresy(&self, kod: u8) -> Vec<Ipv4Addr> {
        match self.volba(kod) {
            Some(d) if !d.is_empty() && d.len() % 4 == 0 => {
                d.chunks_exact(4).map(|c| Ipv4Addr::new(c[0], c[1], c[2], c[3])).collect()
            }
            _ => Vec::new(),
        }
    }

    /// 32-bitové číslo z voľby (časy v sekundách).
    pub fn cislo(&self, kod: u8) -> Option<u32> {
        match self.volba(kod) {
            Some(&[a, b, c, d]) => Some(u32::from_be_bytes([a, b, c, d])),
            _ => None,
        }
    }

    /// Zakóduje správu; doplní na najmenej 300 B (RFC 951).
    pub fn zakoduj(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(NAJKRATSIA + 64);
        v.extend_from_slice(&[self.op, 1, 6, 0]);
        v.extend_from_slice(&self.xid.to_be_bytes());
        v.extend_from_slice(&self.secs.to_be_bytes());
        v.extend_from_slice(&(if self.vsetkym { PRIZNAK_VSETKYM } else { 0 }).to_be_bytes());
        for a in [self.ciaddr, self.yiaddr, self.siaddr, self.giaddr] {
            v.extend_from_slice(&a.octets());
        }
        v.extend_from_slice(&self.chaddr);
        v.resize(PEVNA, 0); // zvyšok chaddr (10 B), sname (64 B), file (128 B)
        v.extend_from_slice(&COOKIE);
        for (kod, data) in &self.volby {
            if data.is_empty() {
                v.extend_from_slice(&[*kod, 0]);
            }
            for kus in data.chunks(255) {
                v.push(*kod);
                v.push(kus.len() as u8);
                v.extend_from_slice(kus);
            }
        }
        v.push(volba::KONIEC);
        if v.len() < NAJKRATSIA {
            v.resize(NAJKRATSIA, volba::VYPLN);
        }
        v
    }

    /// Prečíta správu pre Ethernet. Voľby z polí `file` a `sname` podľa voľby 52 (pretaženie) sa pripoja.
    pub fn precitaj(d: &[u8]) -> Result<Sprava, Chyba> {
        if d.len() < PEVNA + 4 {
            return Err(Chyba::PrilisKratka(d.len()));
        }
        if d[1] != 1 || d[2] != 6 {
            return Err(Chyba::ZlyHardver { htype: d[1], hlen: d[2] });
        }
        if d[PEVNA..PEVNA + 4] != COOKIE {
            return Err(Chyba::BezCookie);
        }
        let a = |i: usize| Ipv4Addr::new(d[i], d[i + 1], d[i + 2], d[i + 3]);
        let mut chaddr = [0u8; 6];
        chaddr.copy_from_slice(&d[28..34]);
        let mut s = Sprava {
            op: d[0],
            xid: u32::from_be_bytes([d[4], d[5], d[6], d[7]]),
            secs: u16::from_be_bytes([d[8], d[9]]),
            vsetkym: u16::from_be_bytes([d[10], d[11]]) & PRIZNAK_VSETKYM != 0,
            ciaddr: a(12),
            yiaddr: a(16),
            siaddr: a(20),
            giaddr: a(24),
            chaddr,
            volby: Vec::new(),
        };
        citaj_volby(&d[PEVNA + 4..], &mut s.volby)?;
        // RFC 2132 9.3: pretaženie – 1 = aj pole file, 2 = aj sname, 3 = obe (najprv file, potom sname)
        if let Some(&[p]) = s.volba(volba::PRETAZENIE) {
            if p & 1 != 0 {
                citaj_volby(&d[108..236], &mut s.volby)?;
            }
            if p & 2 != 0 {
                citaj_volby(&d[44..108], &mut s.volby)?;
            }
        }
        Ok(s)
    }
}

/// Prečíta voľby do konca alebo po voľbu 255; opakovaný kód pripojí k prvému (RFC 3396).
fn citaj_volby(d: &[u8], volby: &mut Vec<(u8, Vec<u8>)>) -> Result<(), Chyba> {
    let mut i = 0;
    while i < d.len() {
        let kod = d[i];
        match kod {
            volba::VYPLN => i += 1,
            volba::KONIEC => break,
            _ => {
                let dlzka = *d.get(i + 1).ok_or(Chyba::PoskodenaVolba(kod))? as usize;
                let data = d.get(i + 2..i + 2 + dlzka).ok_or(Chyba::PoskodenaVolba(kod))?;
                match volby.iter_mut().find(|(k, _)| *k == kod) {
                    Some((_, v)) => v.extend_from_slice(data),
                    None => volby.push((kod, data.to_vec())),
                }
                i += 2 + dlzka;
            }
        }
    }
    Ok(())
}

/// Prenájom adresy od servera DHCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prenajom {
    pub adresa: Ipv4Addr,
    /// dĺžka predpony siete (z masky; bez masky podľa triedy adresy)
    pub predpona: u8,
    pub brany: Vec<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    pub ntp: Vec<Ipv4Addr>,
    pub domena: Option<String>,
    pub mtu: Option<u16>,
    /// identifikátor servera (voľba 54)
    pub server: Ipv4Addr,
    /// začiatok prenájmu – čas odoslania žiadosti (RFC 2131 4.4.1), na hodinách volajúceho
    pub od: Duration,
    /// trvanie prenájmu (`None` = neobmedzený, 0xffffffff)
    pub trvanie: Option<Duration>,
    /// obnova u toho istého servera (T1) a u ktoréhokoľvek (T2), od začiatku prenájmu
    pub t1: Option<Duration>,
    pub t2: Option<Duration>,
}

impl Prenajom {
    /// Prenájom z potvrdenia (DHCPACK). `None`, ak adresa nie je použiteľná pre zariadenie.
    pub fn z_potvrdenia(s: &Sprava, od: Duration, server: Ipv4Addr) -> Option<Prenajom> {
        let a = s.yiaddr;
        if a.is_unspecified() || a.is_broadcast() || a.is_multicast() || a.is_loopback() {
            return None;
        }
        let predpona =
            s.adresa(volba::MASKA).and_then(predpona_z_masky).unwrap_or_else(|| predpona_podla_triedy(a));
        let sekundy = |k| s.cislo(k).map(|c| (c != u32::MAX).then(|| Duration::from_secs(c as u64)));
        // bez voľby 51 sa prenájom nedá obnoviť správne – berie sa ako 1 hodina (opatrne, nie neobmedzene)
        let trvanie = sekundy(volba::PRENAJOM).unwrap_or(Some(Duration::from_secs(3600)));
        let (t1, t2) = match trvanie {
            None => (None, None),
            Some(l) => {
                // RFC 2131 4.4.5: predvolene T1 = 0,5 a T2 = 0,875 prenájmu; musí platiť T1 < T2 < prenájom
                let mut t1 = sekundy(volba::T1).flatten().unwrap_or(l / 2);
                let mut t2 = sekundy(volba::T2).flatten().unwrap_or(l * 7 / 8);
                if t2 >= l || t2 == Duration::ZERO {
                    t2 = l * 7 / 8;
                }
                if t1 >= t2 || t1 == Duration::ZERO {
                    t1 = t2.min(l / 2);
                    if t1 >= t2 {
                        t1 = t2 / 2;
                    }
                }
                (Some(t1), Some(t2))
            }
        };
        let domena = s
            .volba(volba::DOMENA)
            .and_then(|d| std::str::from_utf8(d).ok())
            .map(|d| d.trim_end_matches('\0').to_string())
            .filter(|d| !d.is_empty());
        Some(Prenajom {
            adresa: a,
            predpona,
            brany: s.adresy(volba::BRANA),
            dns: s.adresy(volba::DNS),
            ntp: s.adresy(volba::NTP),
            domena,
            mtu: match s.volba(volba::MTU) {
                Some(&[a, b]) => Some(u16::from_be_bytes([a, b])).filter(|m| *m >= 68),
                _ => None,
            },
            server,
            od,
            trvanie,
            t1,
            t2,
        })
    }

    /// Koniec prenájmu (`None` = neobmedzený).
    pub fn koniec(&self) -> Option<Duration> {
        self.trvanie.map(|t| self.od.saturating_add(t))
    }

    /// Maska siete ako adresa (napr. 255.255.255.0).
    pub fn maska(&self) -> Ipv4Addr {
        Ipv4Addr::from(if self.predpona == 0 { 0 } else { u32::MAX << (32 - self.predpona as u32) })
    }
}

/// Dĺžka predpony z masky; `None`, ak maska nie je súvislá (napr. 255.0.255.0).
pub fn predpona_z_masky(m: Ipv4Addr) -> Option<u8> {
    let c = u32::from(m);
    let n = c.leading_ones();
    (c.checked_shl(n).unwrap_or(0) == 0).then_some(n as u8)
}

/// Predpona podľa triedy adresy (RFC 791) – len keď server masku nepošle.
fn predpona_podla_triedy(a: Ipv4Addr) -> u8 {
    match a.octets()[0] {
        0..=127 => 8,
        128..=191 => 16,
        _ => 24,
    }
}

/// Kam poslať správu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ciel {
    /// 255.255.255.255:67 cez zvolené rozhranie
    Vsetkym,
    /// server na adrese (obnova prenájmu, RENEWING)
    Server(Ipv4Addr),
}

/// Čo má volajúci urobiť.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Akcia {
    Posli {
        data: Vec<u8>,
        ciel: Ciel,
    },
    /// nastaviť adresu, predponu, trasu cez bránu (alebo predĺžiť platnosť rovnakého prenájmu)
    Nastav(Prenajom),
    /// odobrať adresu a trasu (prenájom skončil, server odmietol, iná adresa)
    Odober(Prenajom),
}

/// Stav klienta (RFC 2131, obr. 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stav {
    /// INIT – treba poslať DHCPDISCOVER
    Zaciatok,
    /// SELECTING – čaká sa na ponuku
    Vyber,
    /// REQUESTING – žiadosť o ponúknutú adresu
    Ziadost,
    /// INIT-REBOOT / REBOOTING – po znovupripojení kábla žiadosť o doterajšiu adresu
    Znovu,
    /// BOUND – adresa je platná
    Prenajate,
    /// RENEWING – obnova u toho istého servera
    Obnova,
    /// REBINDING – obnova u ktoréhokoľvek servera
    ObnovaVsetkym,
    /// kábel odpojený – nič sa neposiela
    Odpojene,
}

/// Klient DHCP pre jedno rozhranie.
#[derive(Debug, Clone)]
pub struct Klient {
    mac: [u8; 6],
    stav: Stav,
    xid: u32,
    /// začiatok súčasnej výmeny (pole secs)
    zaciatok: Duration,
    /// čas odoslania poslednej žiadosti (začiatok prenájmu po potvrdení)
    ziadost_od: Duration,
    pokus: u32,
    dalsi: Duration,
    ponuka: Option<(Ipv4Addr, Ipv4Addr)>,
    prenajom: Option<Prenajom>,
    posledna_odmietnuta: Option<String>,
}

/// Najviac žiadostí v stave REQUESTING a REBOOTING, potom sa začína odznova (RFC 2131 3.1, 3.2).
const POKUSOV_ZIADOSTI: u32 = 4;
const POKUSOV_ZNOVU: u32 = 2;
/// Najkratšia pauza pri obnove (RFC 2131 4.4.5: „down to a minimum of 60 seconds“).
const NAJMENEJ_PRI_OBNOVE: Duration = Duration::from_secs(60);

impl Klient {
    /// Nový klient pre rozhranie s adresou MAC; prvú správu pošle pri prvom [`Klient::tik`].
    pub fn novy(mac: [u8; 6]) -> Klient {
        Klient {
            mac,
            stav: Stav::Zaciatok,
            xid: 0,
            zaciatok: Duration::ZERO,
            ziadost_od: Duration::ZERO,
            pokus: 0,
            dalsi: Duration::ZERO,
            ponuka: None,
            prenajom: None,
            posledna_odmietnuta: None,
        }
    }

    pub fn stav(&self) -> Stav {
        self.stav
    }

    pub fn prenajom(&self) -> Option<&Prenajom> {
        self.prenajom.as_ref()
    }

    /// Kedy treba klienta znova zobudiť ([`Klient::tik`]); `None` = netreba (neobmedzený prenájom, kábel dole
    /// bez prenájmu).
    pub fn kedy(&self) -> Option<Duration> {
        match self.stav {
            Stav::Prenajate if self.prenajom.as_ref().is_some_and(|p| p.t1.is_none()) => None,
            Stav::Odpojene => self.prenajom.as_ref().and_then(|p| p.koniec()),
            _ => Some(self.dalsi),
        }
    }

    /// Krátky popis stavu pre záznam a obrazovku.
    pub fn popis(&self) -> String {
        let p = |p: &Prenajom| format!("{}/{}", p.adresa, p.predpona);
        match (self.stav, &self.prenajom) {
            (Stav::Zaciatok | Stav::Vyber, _) => {
                format!("hľadá server DHCP (pokus {})", self.pokus.max(1))
            }
            (Stav::Ziadost, _) => match self.ponuka {
                Some((a, s)) => format!("žiada o adresu {a} od {s}"),
                None => "žiada o adresu".into(),
            },
            (Stav::Znovu, Some(pr)) => format!("overuje doterajšiu adresu {}", p(pr)),
            (Stav::Prenajate, Some(pr)) => format!("adresa {}", p(pr)),
            (Stav::Obnova, Some(pr)) => format!("adresa {} – obnovuje prenájom", p(pr)),
            (Stav::ObnovaVsetkym, Some(pr)) => format!("adresa {} – obnovuje u ktoréhokoľvek servera", p(pr)),
            (Stav::Odpojene, _) => "kábel odpojený".into(),
            (_, None) => "bez adresy".into(),
        }
    }

    /// Posledné odmietnutie servera (DHCPNAK, text z voľby 56).
    pub fn odmietnutie(&self) -> Option<&str> {
        self.posledna_odmietnuta.as_deref()
    }

    /// Kábel pripojený alebo odpojený (RFC 2131 3.2: po znovupripojení klient overí doterajšiu adresu).
    pub fn linka(&mut self, hore: bool, teraz: Duration, nahoda: u32) -> Vec<Akcia> {
        let mut akcie = Vec::new();
        if !hore {
            self.stav = Stav::Odpojene;
            return akcie;
        }
        if self.stav != Stav::Odpojene {
            return akcie;
        }
        match &self.prenajom {
            Some(p) if p.koniec().is_none_or(|k| teraz < k) => {
                let a = p.adresa;
                self.xid = nahoda;
                self.zaciatok = teraz;
                self.pokus = 0;
                self.stav = Stav::Znovu;
                self.posli_znovu(a, teraz, nahoda, &mut akcie);
            }
            _ => {
                if let Some(p) = self.prenajom.take() {
                    akcie.push(Akcia::Odober(p));
                }
                self.stav = Stav::Zaciatok;
                self.dalsi = teraz;
                akcie.extend(self.tik(teraz, nahoda));
            }
        }
        akcie
    }

    /// Uplynul čas z [`Klient::kedy`] (alebo volajúci len kontroluje) – odošle, čo treba.
    pub fn tik(&mut self, teraz: Duration, nahoda: u32) -> Vec<Akcia> {
        let mut akcie = Vec::new();
        if self.stav == Stav::Odpojene {
            if let Some(p) = self.prenajom.take_if(|p| p.koniec().is_some_and(|k| teraz >= k)) {
                akcie.push(Akcia::Odober(p));
            }
            return akcie;
        }
        if teraz < self.dalsi {
            return akcie;
        }
        match self.stav {
            Stav::Zaciatok => {
                self.xid = nahoda;
                self.zaciatok = teraz;
                self.pokus = 0;
                self.ponuka = None;
                self.stav = Stav::Vyber;
                self.posli_objavenie(teraz, nahoda, &mut akcie);
            }
            Stav::Vyber => self.posli_objavenie(teraz, nahoda, &mut akcie),
            Stav::Ziadost => {
                if self.pokus >= POKUSOV_ZIADOSTI {
                    self.stav = Stav::Zaciatok;
                    self.dalsi = teraz;
                    return self.tik(teraz, nahoda);
                }
                if let Some((a, s)) = self.ponuka {
                    self.posli_ziadost(a, s, teraz, nahoda, &mut akcie);
                }
            }
            Stav::Znovu => {
                let a = self.prenajom.as_ref().map(|p| p.adresa);
                match a {
                    Some(a) if self.pokus < POKUSOV_ZNOVU => self.posli_znovu(a, teraz, nahoda, &mut akcie),
                    Some(_) => {
                        // server mlčí – RFC 2131 3.2 dovoľuje používať doterajšiu adresu do konca prenájmu;
                        // obnova pri T1 (alebo hneď, ak T1 už prešlo) potom zistí, či server ešte platí
                        self.stav = Stav::Prenajate;
                        let t1 = self.prenajom.as_ref().and_then(|p| p.t1.map(|t| p.od.saturating_add(t)));
                        self.dalsi = t1.unwrap_or(Duration::MAX);
                        akcie.extend(self.tik(teraz, nahoda));
                    }
                    None => {
                        self.stav = Stav::Zaciatok;
                        self.dalsi = teraz;
                        akcie.extend(self.tik(teraz, nahoda));
                    }
                }
            }
            Stav::Prenajate | Stav::Obnova | Stav::ObnovaVsetkym => {
                let Some(p) = self.prenajom.clone() else {
                    self.stav = Stav::Zaciatok;
                    self.dalsi = teraz;
                    return self.tik(teraz, nahoda);
                };
                let (Some(t1), Some(t2)) = (p.t1, p.t2) else { return akcie };
                let koniec = p.koniec().unwrap_or(Duration::MAX);
                if teraz >= koniec {
                    // RFC 2131 4.4.5: po konci prenájmu klient adresu nesmie používať a začína odznova
                    self.prenajom = None;
                    akcie.push(Akcia::Odober(p));
                    self.stav = Stav::Zaciatok;
                    self.dalsi = teraz;
                    akcie.extend(self.tik(teraz, nahoda));
                } else if teraz >= p.od.saturating_add(t2) {
                    if self.stav != Stav::ObnovaVsetkym {
                        self.stav = Stav::ObnovaVsetkym;
                        self.xid = nahoda;
                        self.zaciatok = teraz;
                    }
                    self.posli_obnovu(&p, Ciel::Vsetkym, teraz, koniec, &mut akcie);
                } else if teraz >= p.od.saturating_add(t1) {
                    if self.stav != Stav::Obnova {
                        self.stav = Stav::Obnova;
                        self.xid = nahoda;
                        self.zaciatok = teraz;
                    }
                    self.posli_obnovu(&p, Ciel::Server(p.server), teraz, p.od.saturating_add(t2), &mut akcie);
                } else {
                    self.dalsi = p.od.saturating_add(t1);
                }
            }
            Stav::Odpojene => {}
        }
        akcie
    }

    /// Prijatá správa z portu 68. Správy pre iného klienta alebo inú transakciu sa ticho ignorujú.
    pub fn prijmi(&mut self, data: &[u8], teraz: Duration, nahoda: u32) -> Vec<Akcia> {
        let mut akcie = Vec::new();
        let Ok(s) = Sprava::precitaj(data) else { return akcie };
        if s.op != BOOTREPLY || s.xid != self.xid || s.chaddr != self.mac {
            return akcie;
        }
        match (self.stav, s.typ()) {
            (Stav::Vyber, Some(Typ::Offer)) => {
                let Some(server) = s.adresa(volba::SERVER) else { return akcie };
                let a = s.yiaddr;
                if a.is_unspecified() || a.is_broadcast() || a.is_multicast() || a.is_loopback() {
                    return akcie;
                }
                self.ponuka = Some((a, server));
                self.stav = Stav::Ziadost;
                self.pokus = 0;
                self.posli_ziadost(a, server, teraz, nahoda, &mut akcie);
            }
            (Stav::Ziadost | Stav::Znovu | Stav::Obnova | Stav::ObnovaVsetkym, Some(Typ::Ack)) => {
                let server = s
                    .adresa(volba::SERVER)
                    .or(self.ponuka.map(|(_, s)| s))
                    .or(self.prenajom.as_ref().map(|p| p.server))
                    .unwrap_or(Ipv4Addr::UNSPECIFIED);
                if self.stav == Stav::Ziadost && self.ponuka.is_some_and(|(_, sv)| sv != server) {
                    return akcie; // potvrdenie od iného servera, než ktorého ponuku sme prijali
                }
                let Some(novy) = Prenajom::z_potvrdenia(&s, self.ziadost_od, server) else { return akcie };
                if let Some(stary) = self.prenajom.take()
                    && stary.adresa != novy.adresa
                {
                    akcie.push(Akcia::Odober(stary));
                }
                self.dalsi = novy.t1.map_or(Duration::MAX, |t| novy.od.saturating_add(t));
                self.prenajom = Some(novy.clone());
                self.ponuka = None;
                self.posledna_odmietnuta = None;
                self.stav = Stav::Prenajate;
                akcie.push(Akcia::Nastav(novy));
            }
            (Stav::Ziadost | Stav::Znovu | Stav::Obnova | Stav::ObnovaVsetkym, Some(Typ::Nak)) => {
                self.posledna_odmietnuta = Some(
                    s.volba(volba::SPRAVA)
                        .map(|t| String::from_utf8_lossy(t).into_owned())
                        .unwrap_or_else(|| "server odmietol adresu".into()),
                );
                if let Some(p) = self.prenajom.take() {
                    akcie.push(Akcia::Odober(p));
                }
                self.ponuka = None;
                self.stav = Stav::Zaciatok;
                // RFC 2131 3.1 bod 5: začať odznova; 1 s pauza, aby sa dva servery nehádali v slučke
                self.dalsi = teraz + Duration::from_secs(1);
            }
            _ => {}
        }
        akcie
    }

    /// Spoločné voľby žiadosti: typ, identifikátor klienta, najväčšia správa, zoznam žiadaných volieb.
    fn sprava(&self, typ: Typ, teraz: Duration) -> Sprava {
        let secs = teraz.saturating_sub(self.zaciatok).as_secs().min(u16::MAX as u64) as u16;
        let mut id = vec![1u8];
        id.extend_from_slice(&self.mac);
        Sprava::ziadost(self.mac, self.xid, secs)
            .s_volbou(volba::TYP, &[typ as u8])
            .s_volbou(volba::ID_KLIENTA, &id)
            .s_volbou(volba::NAJVACSIA, &NAJVACSIA_SPRAVA.to_be_bytes())
            .s_volbou(volba::ZOZNAM, &ZIADANE)
    }

    fn posli_objavenie(&mut self, teraz: Duration, nahoda: u32, akcie: &mut Vec<Akcia>) {
        self.pokus += 1;
        let mut s = self.sprava(Typ::Discover, teraz);
        s.vsetkym = true;
        akcie.push(Akcia::Posli { data: s.zakoduj(), ciel: Ciel::Vsetkym });
        self.dalsi = teraz + interval(self.pokus, nahoda);
    }

    fn posli_ziadost(
        &mut self,
        a: Ipv4Addr,
        server: Ipv4Addr,
        teraz: Duration,
        nahoda: u32,
        akcie: &mut Vec<Akcia>,
    ) {
        self.pokus += 1;
        self.ziadost_od = teraz;
        let mut s = self
            .sprava(Typ::Request, teraz)
            .s_volbou(volba::ZIADANA_ADRESA, &a.octets())
            .s_volbou(volba::SERVER, &server.octets());
        s.vsetkym = true;
        akcie.push(Akcia::Posli { data: s.zakoduj(), ciel: Ciel::Vsetkym });
        self.dalsi = teraz + interval(self.pokus, nahoda);
    }

    fn posli_znovu(&mut self, a: Ipv4Addr, teraz: Duration, nahoda: u32, akcie: &mut Vec<Akcia>) {
        self.pokus += 1;
        self.ziadost_od = teraz;
        let mut s = self.sprava(Typ::Request, teraz).s_volbou(volba::ZIADANA_ADRESA, &a.octets());
        s.vsetkym = true;
        akcie.push(Akcia::Posli { data: s.zakoduj(), ciel: Ciel::Vsetkym });
        self.dalsi = teraz + interval(self.pokus, nahoda);
    }

    /// RENEWING a REBINDING: ciaddr = vlastná adresa, bez volieb 50 a 54 (RFC 2131, tab. 5); ďalší pokus o polovicu
    /// zvyšného času do `hranica`, najmenej 60 s.
    fn posli_obnovu(
        &mut self,
        p: &Prenajom,
        ciel: Ciel,
        teraz: Duration,
        hranica: Duration,
        akcie: &mut Vec<Akcia>,
    ) {
        self.ziadost_od = teraz;
        let mut s = self.sprava(Typ::Request, teraz);
        s.ciaddr = p.adresa;
        akcie.push(Akcia::Posli { data: s.zakoduj(), ciel });
        let zvysok = hranica.saturating_sub(teraz) / 2;
        self.dalsi =
            (teraz + zvysok.max(NAJMENEJ_PRI_OBNOVE)).min(hranica.max(teraz + Duration::from_secs(1)));
    }
}

/// Opakovanie: 4, 8, 16, 32, 64 s (ďalej 64 s), náhodne ±1 s (RFC 2131 4.1).
pub fn interval(pokus: u32, nahoda: u32) -> Duration {
    let zaklad = 4u64 << pokus.saturating_sub(1).min(4);
    let posun = (nahoda % 2001) as i64 - 1000;
    Duration::from_millis((zaklad as i64 * 1000 + posun) as u64)
}

#[cfg(test)]
mod testy {
    use super::*;

    const MAC: [u8; 6] = [0xb8, 0x27, 0xeb, 0x12, 0x34, 0x56];
    const SERVER: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
    const ADRESA: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 42);

    fn s(sek: u64) -> Duration {
        Duration::from_secs(sek)
    }

    /// Odpoveď servera ako by ju poslal router.
    fn odpoved(typ: Typ, xid: u32, volby: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut sp = Sprava::ziadost(MAC, xid, 0);
        sp.op = BOOTREPLY;
        sp.yiaddr = if typ == Typ::Nak { Ipv4Addr::UNSPECIFIED } else { ADRESA };
        sp = sp.s_volbou(volba::TYP, &[typ as u8]).s_volbou(volba::SERVER, &SERVER.octets());
        for (k, d) in volby {
            sp = sp.s_volbou(*k, d);
        }
        sp.zakoduj()
    }

    fn bezne_volby(prenajom: u32) -> Vec<(u8, Vec<u8>)> {
        vec![
            (volba::MASKA, vec![255, 255, 255, 0]),
            (volba::BRANA, SERVER.octets().to_vec()),
            (volba::DNS, [SERVER.octets(), [9, 9, 9, 9]].concat()),
            (volba::PRENAJOM, prenajom.to_be_bytes().to_vec()),
            (volba::DOMENA, b"doma".to_vec()),
        ]
    }

    fn odoslane(a: &[Akcia]) -> Vec<(Sprava, Ciel)> {
        a.iter()
            .filter_map(|x| match x {
                Akcia::Posli { data, ciel } => Some((Sprava::precitaj(data).unwrap(), *ciel)),
                _ => None,
            })
            .collect()
    }

    /// Klient s platným prenájmom (prenájom 3600 s od času 10 s).
    fn prenajaty(prenajom: u32) -> (Klient, u32) {
        let mut k = Klient::novy(MAC);
        let a = k.tik(s(10), 0x1234_5678);
        let xid = odoslane(&a)[0].0.xid;
        k.prijmi(&odpoved(Typ::Offer, xid, &bezne_volby(prenajom)), s(10), 7);
        let a = k.prijmi(&odpoved(Typ::Ack, xid, &bezne_volby(prenajom)), s(10), 7);
        assert!(matches!(a.as_slice(), [Akcia::Nastav(_)]), "{a:?}");
        (k, xid)
    }

    #[test]
    fn objavenie_bajt_po_bajte() {
        let mut k = Klient::novy(MAC);
        let a = k.tik(Duration::ZERO, 0xdead_beef);
        let [Akcia::Posli { data, ciel: Ciel::Vsetkym }] = a.as_slice() else { panic!("{a:?}") };
        assert_eq!(data.len(), 300, "doplnené na dĺžku BOOTP");
        assert_eq!(&data[..4], &[1, 1, 6, 0], "op, htype Ethernet, hlen 6, hops");
        assert_eq!(&data[4..8], &0xdead_beefu32.to_be_bytes(), "xid");
        assert_eq!(&data[8..12], &[0, 0, 0x80, 0], "secs 0, príznak broadcast");
        assert!(data[12..28].iter().all(|&b| b == 0), "ciaddr, yiaddr, siaddr, giaddr");
        assert_eq!(&data[28..34], &MAC);
        assert!(data[34..236].iter().all(|&b| b == 0), "chaddr doplnok, sname, file");
        assert_eq!(&data[236..240], &COOKIE);
        let ocakavane: Vec<u8> = [
            &[53, 1, 1][..],
            &[61, 7, 1],
            &MAC,
            &[57, 2, 0x05, 0xdc],
            &[55, 9, 1, 3, 6, 15, 26, 42, 51, 58, 59],
            &[255],
        ]
        .concat();
        assert_eq!(&data[240..240 + ocakavane.len()], ocakavane.as_slice());
        assert!(data[240 + ocakavane.len()..].iter().all(|&b| b == 0), "výplň");
        assert_eq!(k.stav(), Stav::Vyber);
        assert_eq!(k.kedy(), Some(Duration::from_millis(4000 + (0xdead_beefu32 % 2001) as u64 - 1000)));
    }

    #[test]
    fn cela_vymena_a_prenajom() {
        let mut k = Klient::novy(MAC);
        let a = k.tik(s(5), 99);
        let (disc, _) = &odoslane(&a)[0];
        assert_eq!(disc.typ(), Some(Typ::Discover));
        let a = k.prijmi(&odpoved(Typ::Offer, disc.xid, &bezne_volby(86_400)), s(6), 1);
        let o = odoslane(&a);
        let [(req, Ciel::Vsetkym)] = o.as_slice() else { panic!("{a:?}") };
        assert_eq!(req.typ(), Some(Typ::Request));
        assert_eq!(req.xid, disc.xid, "REQUEST v stave SELECTING nesie xid z ponuky");
        assert_eq!(req.adresa(volba::ZIADANA_ADRESA), Some(ADRESA));
        assert_eq!(req.adresa(volba::SERVER), Some(SERVER));
        assert_eq!(req.ciaddr, Ipv4Addr::UNSPECIFIED);
        assert!(req.vsetkym);
        assert_eq!(req.secs, 1);
        let a = k.prijmi(&odpoved(Typ::Ack, disc.xid, &bezne_volby(86_400)), s(7), 1);
        let [Akcia::Nastav(p)] = a.as_slice() else { panic!("{a:?}") };
        assert_eq!(p.adresa, ADRESA);
        assert_eq!(p.predpona, 24);
        assert_eq!(p.maska(), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(p.brany, vec![SERVER]);
        assert_eq!(p.dns, vec![SERVER, Ipv4Addr::new(9, 9, 9, 9)]);
        assert_eq!(p.domena.as_deref(), Some("doma"));
        assert_eq!(p.od, s(6), "prenájom platí od odoslania žiadosti");
        assert_eq!((p.t1, p.t2, p.trvanie), (Some(s(43_200)), Some(s(75_600)), Some(s(86_400))));
        assert_eq!(k.stav(), Stav::Prenajate);
        assert_eq!(k.kedy(), Some(s(6 + 43_200)), "zobudiť až pri T1 – medzi tým nič nebeží");
        assert!(k.tik(s(1000), 5).is_empty(), "pred T1 sa nič neposiela");
        assert_eq!(k.popis(), "adresa 192.168.1.42/24");
    }

    #[test]
    fn obnova_u_servera_potom_u_vsetkych_potom_koniec() {
        let (mut k, _) = prenajaty(3600); // od 10 s: T1 = 1810, T2 = 3160, koniec 3610
        let a = k.tik(s(1810), 42);
        let o = odoslane(&a);
        let [(r, Ciel::Server(sv))] = o.as_slice() else { panic!("{a:?}") };
        assert_eq!(*sv, SERVER, "RENEWING ide priamo serveru");
        assert_eq!(r.ciaddr, ADRESA);
        assert_eq!((r.adresa(volba::ZIADANA_ADRESA), r.adresa(volba::SERVER)), (None, None));
        assert!(!r.vsetkym);
        assert_eq!(k.stav(), Stav::Obnova);
        assert_eq!(k.kedy(), Some(s(1810 + 675)), "polovica zvyšku do T2");
        // server mlčí až do T2 → REBINDING všetkým
        let mut t = 1810;
        while let Some(d) = k.kedy().filter(|d| *d < s(3160)) {
            t = d.as_secs();
            assert!(odoslane(&k.tik(d, 1)).iter().all(|(_, c)| matches!(c, Ciel::Server(_))));
        }
        assert!(t >= 3100, "pokusy sa zhusťujú k T2, najmenej po 60 s");
        let a = k.tik(s(3160), 1);
        let o = odoslane(&a);
        let [(r, Ciel::Vsetkym)] = o.as_slice() else { panic!("{a:?}") };
        assert_eq!(r.ciaddr, ADRESA);
        assert_eq!(k.stav(), Stav::ObnovaVsetkym);
        // a do konca prenájmu nič → adresa preč, hľadá sa odznova
        while let Some(d) = k.kedy().filter(|d| *d < s(3610)) {
            k.tik(d, 1);
        }
        let a = k.tik(s(3610), 77);
        assert!(matches!(a[0], Akcia::Odober(ref p) if p.adresa == ADRESA), "{a:?}");
        assert_eq!(odoslane(&a)[0].0.typ(), Some(Typ::Discover));
        assert_eq!(k.stav(), Stav::Vyber);
        assert!(k.prenajom().is_none());
    }

    #[test]
    fn obnova_potvrdena_predlzi_prenajom() {
        let (mut k, _) = prenajaty(3600);
        let a = k.tik(s(1810), 42);
        let xid = odoslane(&a)[0].0.xid;
        assert_eq!(xid, 42, "nová transakcia pri obnove");
        let a = k.prijmi(&odpoved(Typ::Ack, xid, &bezne_volby(3600)), s(1811), 3);
        let [Akcia::Nastav(p)] = a.as_slice() else { panic!("{a:?}") };
        assert_eq!(p.od, s(1810));
        assert_eq!(k.kedy(), Some(s(1810 + 1800)));
        assert_eq!(k.stav(), Stav::Prenajate);
    }

    #[test]
    fn odmietnutie_zrusi_adresu() {
        let (mut k, _) = prenajaty(3600);
        let xid = odoslane(&k.tik(s(1810), 9))[0].0.xid;
        let a = k.prijmi(&odpoved(Typ::Nak, xid, &[(volba::SPRAVA, b"zla siet".to_vec())]), s(1811), 3);
        assert!(matches!(a.as_slice(), [Akcia::Odober(p)] if p.adresa == ADRESA), "{a:?}");
        assert_eq!(k.odmietnutie(), Some("zla siet"));
        assert_eq!(k.stav(), Stav::Zaciatok);
        assert_eq!(k.kedy(), Some(s(1812)), "1 s pauza pred novým hľadaním");
    }

    #[test]
    fn cudzia_odpoved_sa_ignoruje() {
        let mut k = Klient::novy(MAC);
        let xid = odoslane(&k.tik(s(0), 5))[0].0.xid;
        assert!(k.prijmi(&odpoved(Typ::Offer, xid + 1, &bezne_volby(60)), s(1), 1).is_empty(), "iný xid");
        let mut ina = Sprava::precitaj(&odpoved(Typ::Offer, xid, &bezne_volby(60))).unwrap();
        ina.chaddr = [2, 0, 0, 0, 0, 1];
        assert!(k.prijmi(&ina.zakoduj(), s(1), 1).is_empty(), "iná MAC");
        let mut bez_servera = Sprava::precitaj(&odpoved(Typ::Offer, xid, &[])).unwrap();
        bez_servera.volby.retain(|(c, _)| *c != volba::SERVER);
        assert!(k.prijmi(&bez_servera.zakoduj(), s(1), 1).is_empty(), "ponuka bez identifikátora servera");
        assert!(k.prijmi(&[0u8; 100], s(1), 1).is_empty(), "poškodená správa");
        assert_eq!(k.stav(), Stav::Vyber);
    }

    #[test]
    fn opakovanie_4_8_16_32_64() {
        let stred: Vec<u64> = (1..=7).map(|p| interval(p, 1000).as_secs()).collect();
        assert_eq!(stred, [4, 8, 16, 32, 64, 64, 64]);
        assert_eq!(interval(1, 0), Duration::from_millis(3000));
        assert_eq!(interval(1, 2000), Duration::from_millis(5000));
        // bez odpovede klient opakuje DISCOVER s tým istým xid
        let mut k = Klient::novy(MAC);
        let prvy = odoslane(&k.tik(s(0), 1000))[0].0.xid;
        for _ in 0..5 {
            let d = k.kedy().unwrap();
            let a = k.tik(d, 1000);
            assert_eq!(odoslane(&a)[0].0.xid, prvy);
        }
    }

    #[test]
    fn ziadost_bez_odpovede_zacne_odznova() {
        let mut k = Klient::novy(MAC);
        let xid = odoslane(&k.tik(s(0), 1000))[0].0.xid;
        k.prijmi(&odpoved(Typ::Offer, xid, &bezne_volby(600)), s(1), 1000);
        for _ in 0..POKUSOV_ZIADOSTI - 1 {
            let d = k.kedy().unwrap();
            assert_eq!(odoslane(&k.tik(d, 1000))[0].0.typ(), Some(Typ::Request));
        }
        let d = k.kedy().unwrap();
        let a = k.tik(d, 555);
        assert_eq!(odoslane(&a)[0].0.typ(), Some(Typ::Discover));
        assert_eq!(odoslane(&a)[0].0.xid, 555, "nová transakcia");
    }

    #[test]
    fn kabel_odpojeny_a_znova_pripojeny() {
        let (mut k, _) = prenajaty(3600);
        assert!(k.linka(false, s(100), 1).is_empty());
        assert_eq!(k.stav(), Stav::Odpojene);
        assert_eq!(k.kedy(), Some(s(3610)), "zobudiť len na koniec prenájmu");
        let a = k.linka(true, s(200), 31);
        let o = odoslane(&a);
        let [(r, Ciel::Vsetkym)] = o.as_slice() else { panic!("{a:?}") };
        assert_eq!(r.adresa(volba::ZIADANA_ADRESA), Some(ADRESA), "INIT-REBOOT žiada doterajšiu adresu");
        assert_eq!(r.adresa(volba::SERVER), None);
        assert_eq!(r.ciaddr, Ipv4Addr::UNSPECIFIED);
        assert_eq!(k.stav(), Stav::Znovu);
        let a = k.prijmi(&odpoved(Typ::Ack, 31, &bezne_volby(3600)), s(201), 1);
        assert!(matches!(a.as_slice(), [Akcia::Nastav(p)] if p.adresa == ADRESA));
        // kábel dole až po konci prenájmu → adresa preč a po pripojení hľadanie odznova
        k.linka(false, s(300), 1);
        assert!(matches!(k.tik(s(3800), 1).as_slice(), [Akcia::Odober(_)]));
        let a = k.linka(true, s(4000), 8);
        assert_eq!(odoslane(&a)[0].0.typ(), Some(Typ::Discover));
    }

    #[test]
    fn po_znovupripojeni_server_mlci_adresa_ostava() {
        // RFC 2131 3.2: bez odpovede na INIT-REBOOT smie klient používať doterajšiu adresu do konca prenájmu
        let (mut k, _) = prenajaty(3600);
        k.linka(false, s(100), 1);
        k.linka(true, s(200), 1000);
        let d = k.kedy().unwrap();
        assert_eq!(odoslane(&k.tik(d, 1000)).len(), 1, "druhý pokus INIT-REBOOT");
        let d = k.kedy().unwrap();
        let a = k.tik(d, 1000);
        assert!(a.iter().all(|x| !matches!(x, Akcia::Odober(_))), "adresa sa neodoberá: {a:?}");
        assert_eq!(k.stav(), Stav::Prenajate);
        assert_eq!(k.prenajom().map(|p| p.adresa), Some(ADRESA));
        assert_eq!(k.kedy(), Some(s(1810)), "ďalej obnova pri T1 doterajšieho prenájmu");
    }

    #[test]
    fn neobmedzeny_prenajom_sa_neobnovuje() {
        let (k, _) = prenajaty(u32::MAX);
        assert_eq!(k.prenajom().unwrap().trvanie, None);
        assert_eq!(k.kedy(), None);
    }

    #[test]
    fn zle_t1_t2_sa_opravia() {
        let mut v = bezne_volby(1000);
        v.push((volba::T1, 900u32.to_be_bytes().to_vec()));
        v.push((volba::T2, 800u32.to_be_bytes().to_vec()));
        let sp = Sprava::precitaj(&odpoved(Typ::Ack, 1, &v)).unwrap();
        let p = Prenajom::z_potvrdenia(&sp, s(0), SERVER).unwrap();
        assert_eq!((p.t1, p.t2), (Some(s(500)), Some(s(800))), "T1 < T2 < prenájom");
        let mut v = bezne_volby(1000);
        v.push((volba::T2, 5000u32.to_be_bytes().to_vec()));
        let p = Prenajom::z_potvrdenia(&Sprava::precitaj(&odpoved(Typ::Ack, 1, &v)).unwrap(), s(0), SERVER);
        assert_eq!(p.unwrap().t2, Some(s(875)));
    }

    #[test]
    fn maska_a_trieda() {
        assert_eq!(predpona_z_masky(Ipv4Addr::new(255, 255, 255, 0)), Some(24));
        assert_eq!(predpona_z_masky(Ipv4Addr::new(255, 255, 255, 255)), Some(32));
        assert_eq!(predpona_z_masky(Ipv4Addr::new(0, 0, 0, 0)), Some(0));
        assert_eq!(predpona_z_masky(Ipv4Addr::new(255, 255, 240, 0)), Some(20));
        assert_eq!(predpona_z_masky(Ipv4Addr::new(255, 0, 255, 0)), None);
        assert_eq!(predpona_podla_triedy(Ipv4Addr::new(10, 1, 2, 3)), 8);
        assert_eq!(predpona_podla_triedy(Ipv4Addr::new(172, 16, 0, 9)), 16);
        assert_eq!(predpona_podla_triedy(Ipv4Addr::new(192, 168, 0, 9)), 24);
    }

    #[test]
    fn pretazenie_a_spajanie_volieb() {
        // RFC 2132 9.3 a RFC 3396: voľby v poli file a opakovaná voľba DNS rozdelená na dve
        let mut d = odpoved(Typ::Ack, 1, &[]);
        d.truncate(240);
        d.extend_from_slice(&[52, 1, 1, 6, 4, 1, 1, 1, 1, 255]);
        d[108..118].copy_from_slice(&[6, 4, 8, 8, 8, 8, 1, 4, 255, 255]);
        d[118..122].copy_from_slice(&[255, 0, 255, 0]);
        d[122] = 255;
        let s = Sprava::precitaj(&d).unwrap();
        assert_eq!(s.adresy(volba::DNS), vec![Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(8, 8, 8, 8)]);
        assert_eq!(s.adresa(volba::MASKA), Some(Ipv4Addr::new(255, 255, 255, 0)));
        // dlhá voľba sa pri kódovaní rozdelí a pri čítaní znova spojí
        let dlha: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let sp = Sprava::ziadost(MAC, 1, 0).s_volbou(200, &dlha);
        assert_eq!(Sprava::precitaj(&sp.zakoduj()).unwrap().volba(200), Some(dlha.as_slice()));
    }

    #[test]
    fn chybne_spravy() {
        assert_eq!(Sprava::precitaj(&[0; 10]), Err(Chyba::PrilisKratka(10)));
        let mut d = odpoved(Typ::Ack, 1, &[]);
        d[236] = 0;
        assert_eq!(Sprava::precitaj(&d), Err(Chyba::BezCookie));
        let mut d = odpoved(Typ::Ack, 1, &[]);
        d[1] = 6;
        assert!(matches!(Sprava::precitaj(&d), Err(Chyba::ZlyHardver { .. })));
        let mut d = odpoved(Typ::Ack, 1, &[]);
        d.truncate(240);
        d.extend_from_slice(&[6, 8, 1, 2]);
        assert_eq!(Sprava::precitaj(&d), Err(Chyba::PoskodenaVolba(6)));
    }
}
