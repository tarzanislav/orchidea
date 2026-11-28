//! R268 (A9): internet v ORCHIDEA OS – adresa z routera (DHCP), preklad mien (DNS), čas zo siete (SNTP).
//!
//! Protokoly sú v balíku `orchidea-siet` (čisté, overené skúškami bajt po bajte). Tu je len vstup a výstup:
//! zásuvka UDP na porte 68 viazaná na káblové rozhranie, rtnetlink na adresu a trasu cez bránu a nastavenie
//! hodín. Vlákno spí v `poll` – zobudí ho len paket, zmena kábla alebo čas z automatu DHCP (obnova prenájmu).
//!
//! **Bezpečnosť:** KLÍČ nepočúva na žiadnom porte – adresa IPv4 slúži len na odchádzajúce spojenia (čas,
//! preklad mien, neskôr sťahovanie). Zberač meraní ostáva na adrese miestnej linky IPv6 so šifrovaním (KORENE).
//! Adresa dostane v jadre platnosť do konca prenájmu – keby vlákno zlyhalo, jadro ju po prenájme samo odoberie.
//!
//! **Chladný systém:** keď je prenájom platný a čas nastavený, vlákno nič nerobí a sieť nežiada – čip siete
//! smie spať (R191). Po zobudení kábla overí doterajšiu adresu jednou správou (INIT-REBOOT, RFC 2131 3.2).

use orchidea_siet::{dhcp, dns, sntp};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::os::fd::AsRawFd;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Meno, ktorým sa overuje preklad a hľadá čas, keď router server NTP neposlal (verejná skupina serverov NTP).
pub const SERVER_CASU: &str = "pool.ntp.org";
/// Čas sa znova overí najskôr po tomto čase (pri obnove prenájmu).
const OBNOVA_CASU: Duration = Duration::from_secs(6 * 3600);
/// Posun menší ako toto sa nenastavuje (hodiny bežia správne, nastavenie by len rušilo záznamy).
const PRESNOST_CASU_NS: i128 = 100_000_000;
/// Bez servera DHCP sa sieť žiada najviac takto dlho od pripojenia kábla – potom čip siete smie spať.
const HLADANIE_SERVERA: Duration = Duration::from_secs(300);
const CAKANIE_NA_ODPOVED: Duration = Duration::from_secs(2);

/// Stav pre záznam merania (SPOLU.TXT) a obrazovku.
#[derive(Debug, Default, Clone)]
struct Stav {
    rozhranie: Option<String>,
    kabel: Option<bool>,
    dhcp: String,
    prenajom: Option<dhcp::Prenajom>,
    preklad: Option<String>,
    cas: Option<String>,
    chyba: Option<String>,
    ziada_siet: bool,
}

static STAV: LazyLock<Mutex<Stav>> = LazyLock::new(|| Mutex::new(Stav::default()));

/// Potrebuje internet práve teraz sieť (čip siete nesmie spať)? Pri hľadaní adresy a jej obnove áno.
pub fn treba_siet() -> bool {
    STAV.lock().is_ok_and(|s| s.ziada_siet)
}

/// Jeden riadok pre záznam merania.
pub fn popis() -> String {
    let Ok(s) = STAV.lock() else { return "?".into() };
    let Some(r) = &s.rozhranie else { return "čaká na káblové rozhranie".into() };
    let mut t = format!("{r}: ");
    t += match s.kabel {
        Some(true) => "kábel ✓",
        Some(false) => "kábel odpojený (alebo čip siete spí)",
        None => "kábel ?",
    };
    t += &format!(" · {}", s.dhcp);
    if let Some(p) = &s.prenajom {
        let zoznam = |v: &[Ipv4Addr]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ");
        t += &format!(
            " · brána {} · DNS {}{}",
            if p.brany.is_empty() { "žiadna".into() } else { zoznam(&p.brany) },
            if p.dns.is_empty() { "žiadny".into() } else { zoznam(&p.dns) },
            p.trvanie.map(|d| format!(" · prenájom {} min", d.as_secs() / 60)).unwrap_or_default()
        );
    }
    if let Some(p) = &s.preklad {
        t += &format!(" · preklad mien {p}");
    }
    if let Some(c) = &s.cas {
        t += &format!(" · čas {c}");
    }
    if let Some(e) = &s.chyba {
        t += &format!(" · naposledy: {e}");
    }
    t
}

fn zmen(f: impl FnOnce(&mut Stav)) {
    if let Ok(mut s) = STAV.lock() {
        f(&mut s);
    }
}

fn nahoda_bajty<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        // bez /dev/urandom aspoň čas (číslo transakcie DHCP nemusí byť tajné, len rôzne)
        let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for (i, x) in b.iter_mut().enumerate() {
            *x = (n >> (8 * (i % 16))) as u8;
        }
    }
    b
}

fn nahoda32() -> u32 {
    u32::from_ne_bytes(nahoda_bajty())
}

/// Káblové rozhranie (nie Wi-Fi, nie lo): (meno, číslo rozhrania, adresa MAC).
fn kablove_rozhranie() -> Option<(String, u32, [u8; 6])> {
    let mut mena: Vec<String> = std::fs::read_dir("/sys/class/net")
        .ok()?
        .flatten()
        .map(|z| z.file_name().to_string_lossy().to_string())
        .filter(|m| m != "lo" && !m.starts_with("wl"))
        .collect();
    mena.sort();
    for m in mena {
        let z = format!("/sys/class/net/{m}");
        if std::path::Path::new(&format!("{z}/wireless")).exists() {
            continue;
        }
        // len Ethernet (typ 1) – nie tunely, mosty bez adresy a pod.
        if std::fs::read_to_string(format!("{z}/type")).map(|t| t.trim() != "1").unwrap_or(true) {
            continue;
        }
        let Some(mac) =
            std::fs::read_to_string(format!("{z}/address")).ok().and_then(|a| mac_z_textu(a.trim()))
        else {
            continue;
        };
        let Some(cislo) =
            std::fs::read_to_string(format!("{z}/ifindex")).ok().and_then(|c| c.trim().parse().ok())
        else {
            continue;
        };
        return Some((m, cislo, mac));
    }
    None
}

fn mac_z_textu(t: &str) -> Option<[u8; 6]> {
    let mut m = [0u8; 6];
    let mut casti = t.split(':');
    for b in m.iter_mut() {
        *b = u8::from_str_radix(casti.next()?, 16).ok()?;
    }
    (casti.next().is_none() && m != [0; 6]).then_some(m)
}

fn kabel(meno: &str) -> bool {
    std::fs::read_to_string(format!("/sys/class/net/{meno}/carrier")).is_ok_and(|t| t.trim() == "1")
}

// ---- rtnetlink (RFC 3549; uapi linux/netlink.h, rtnetlink.h, if_addr.h) – kódovanie je čisté a skúšané ----

const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const NLMSG_ERROR: u16 = 2;
const NLM_F_REQUEST: u16 = 1;
const NLM_F_ACK: u16 = 4;
const NLM_F_REPLACE: u16 = 0x100;
const NLM_F_CREATE: u16 = 0x400;
const AF_INET: u8 = 2;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_BROADCAST: u16 = 4;
const IFA_CACHEINFO: u16 = 6;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PREFSRC: u16 = 7;
const RT_TABLE_MAIN: u8 = 254;
const RTPROT_DHCP: u8 = 16;
const RT_SCOPE_UNIVERSE: u8 = 0;
const RTN_UNICAST: u8 = 1;

fn netlink_sprava(typ: u16, priznaky: u16, poradie: u32, telo: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(16 + telo.len());
    v.extend_from_slice(&((16 + telo.len()) as u32).to_ne_bytes());
    v.extend_from_slice(&typ.to_ne_bytes());
    v.extend_from_slice(&priznaky.to_ne_bytes());
    v.extend_from_slice(&poradie.to_ne_bytes());
    v.extend_from_slice(&0u32.to_ne_bytes());
    v.extend_from_slice(telo);
    v
}

fn atribut(v: &mut Vec<u8>, typ: u16, data: &[u8]) {
    v.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
    v.extend_from_slice(&typ.to_ne_bytes());
    v.extend_from_slice(data);
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

/// Pridať (alebo predĺžiť) či odobrať adresu z prenájmu. `platnost_s` = zvyšok prenájmu v sekundách
/// (`u32::MAX` = navždy) – jadro adresu po nej odoberie samo.
fn sprava_adresy(nova: bool, poradie: u32, rozhranie: u32, p: &dhcp::Prenajom, platnost_s: u32) -> Vec<u8> {
    let mut t = vec![AF_INET, p.predpona, 0, RT_SCOPE_UNIVERSE];
    t.extend_from_slice(&rozhranie.to_ne_bytes());
    atribut(&mut t, IFA_LOCAL, &p.adresa.octets());
    atribut(&mut t, IFA_ADDRESS, &p.adresa.octets());
    if p.predpona < 31 {
        let vsetkym = u32::from(p.adresa) | !u32::from(p.maska());
        atribut(&mut t, IFA_BROADCAST, &vsetkym.to_be_bytes());
    }
    let priznaky = if nova {
        // struct ifa_cacheinfo: preferovaná a platná doba (s), časy vzniku a zmeny (vyplní jadro)
        let mut c = Vec::with_capacity(16);
        for x in [platnost_s, platnost_s, 0, 0] {
            c.extend_from_slice(&x.to_ne_bytes());
        }
        atribut(&mut t, IFA_CACHEINFO, &c);
        NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE
    } else {
        NLM_F_REQUEST | NLM_F_ACK
    };
    netlink_sprava(if nova { RTM_NEWADDR } else { RTM_DELADDR }, priznaky, poradie, &t)
}

/// Predvolená trasa cez bránu (0.0.0.0/0), zdrojová adresa z prenájmu, protokol „DHCP“.
fn sprava_trasy(nova: bool, poradie: u32, rozhranie: u32, brana: Ipv4Addr, zdroj: Ipv4Addr) -> Vec<u8> {
    let mut t = vec![AF_INET, 0, 0, 0, RT_TABLE_MAIN, RTPROT_DHCP, RT_SCOPE_UNIVERSE, RTN_UNICAST];
    t.extend_from_slice(&0u32.to_ne_bytes());
    atribut(&mut t, RTA_GATEWAY, &brana.octets());
    atribut(&mut t, RTA_OIF, &rozhranie.to_ne_bytes());
    atribut(&mut t, RTA_PREFSRC, &zdroj.octets());
    let priznaky = if nova {
        NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE
    } else {
        NLM_F_REQUEST | NLM_F_ACK
    };
    netlink_sprava(if nova { RTM_NEWROUTE } else { RTM_DELROUTE }, priznaky, poradie, &t)
}

/// Odpoveď jadra na požiadavku s `poradie`: NLMSG_ERROR s kódom 0 = hotovo, záporný = chyba errno.
fn potvrdenie(d: &[u8], poradie: u32) -> Option<io::Result<()>> {
    let mut i = 0;
    while i + 16 <= d.len() {
        let dlzka = u32::from_ne_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
        let typ = u16::from_ne_bytes([d[i + 4], d[i + 5]]);
        let seq = u32::from_ne_bytes([d[i + 8], d[i + 9], d[i + 10], d[i + 11]]);
        if dlzka < 16 || i + dlzka > d.len() {
            return None;
        }
        if typ == NLMSG_ERROR && seq == poradie && dlzka >= 20 {
            let e = i32::from_ne_bytes([d[i + 16], d[i + 17], d[i + 18], d[i + 19]]);
            return Some(if e == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(-e)) });
        }
        i += (dlzka + 3) & !3;
    }
    None
}

/// Pošle požiadavku jadru a počká na potvrdenie.
fn ziadost_jadru(nl: &mut std::fs::File, sprava: &[u8], poradie: u32) -> io::Result<()> {
    nl.write_all(sprava)?;
    let mut b = vec![0u8; 8192];
    loop {
        let n = nl.read(&mut b)?;
        if let Some(v) = potvrdenie(&b[..n], poradie) {
            return v;
        }
    }
}

/// Vstup a výstup jedného rozhrania pre klienta DHCP.
struct Rozhranie {
    meno: String,
    cislo: u32,
    udp: UdpSocket,
    nl: std::fs::File,
    poradie: u32,
    zaciatok: Instant,
}

impl Rozhranie {
    fn otvor(meno: &str, cislo: u32) -> io::Result<Rozhranie> {
        let udp = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, dhcp::PORT_KLIENTA))?;
        udp.set_broadcast(true)?;
        crate::sys::viaz_na_rozhranie(udp.as_raw_fd(), meno)?;
        udp.set_nonblocking(true)?;
        Ok(Rozhranie {
            meno: meno.to_string(),
            cislo,
            udp,
            nl: crate::sys::netlink_route(0)?,
            poradie: 1,
            zaciatok: Instant::now(),
        })
    }

    fn teraz(&self) -> Duration {
        self.zaciatok.elapsed()
    }

    /// Vykoná akcie automatu; vráti nový prenájom, ak sa adresa práve nastavila.
    fn vykonaj(&mut self, akcie: Vec<dhcp::Akcia>, hlas: fn(&str)) -> Option<dhcp::Prenajom> {
        let mut novy = None;
        for a in akcie {
            match a {
                dhcp::Akcia::Posli { data, ciel } => {
                    let kam = match ciel {
                        dhcp::Ciel::Vsetkym => SocketAddrV4::new(Ipv4Addr::BROADCAST, dhcp::PORT_SERVERA),
                        dhcp::Ciel::Server(s) => SocketAddrV4::new(s, dhcp::PORT_SERVERA),
                    };
                    if let Err(e) = self.udp.send_to(&data, kam) {
                        zmen(|s| s.chyba = Some(format!("DHCP sa neodoslal ({e})")));
                    }
                }
                dhcp::Akcia::Nastav(p) => {
                    let zvysok = p.koniec().map_or(u32::MAX, |k| {
                        k.saturating_sub(self.teraz()).as_secs().min(u32::MAX as u64 - 1) as u32
                    });
                    self.poradie += 1;
                    let r = ziadost_jadru(
                        &mut self.nl,
                        &sprava_adresy(true, self.poradie, self.cislo, &p, zvysok),
                        self.poradie,
                    );
                    if let Err(e) = r {
                        hlas(&format!("internet (R268): adresu {} sa nepodarilo nastaviť ({e})", p.adresa));
                        zmen(|s| s.chyba = Some(format!("adresa sa nenastavila ({e})")));
                        continue;
                    }
                    if let Some(&brana) = p.brany.first() {
                        self.poradie += 1;
                        let r = ziadost_jadru(
                            &mut self.nl,
                            &sprava_trasy(true, self.poradie, self.cislo, brana, p.adresa),
                            self.poradie,
                        );
                        if let Err(e) = r {
                            hlas(&format!("internet (R268): trasa cez bránu {brana} sa nenastavila ({e})"));
                            zmen(|s| s.chyba = Some(format!("trasa sa nenastavila ({e})")));
                        }
                    }
                    novy = Some(p);
                }
                dhcp::Akcia::Odober(p) => {
                    if let Some(&brana) = p.brany.first() {
                        self.poradie += 1;
                        let _ = ziadost_jadru(
                            &mut self.nl,
                            &sprava_trasy(false, self.poradie, self.cislo, brana, p.adresa),
                            self.poradie,
                        );
                    }
                    self.poradie += 1;
                    let _ = ziadost_jadru(
                        &mut self.nl,
                        &sprava_adresy(false, self.poradie, self.cislo, &p, 0),
                        self.poradie,
                    );
                    hlas(&format!("internet (R268): adresa {}/{} odobratá", p.adresa, p.predpona));
                    novy = None;
                }
            }
        }
        novy
    }

    /// Prečíta všetky čakajúce odpovede servera DHCP.
    fn prijmi(&mut self, klient: &mut dhcp::Klient, hlas: fn(&str)) -> Option<dhcp::Prenajom> {
        let mut novy = None;
        let mut b = [0u8; dhcp::NAJVACSIA_SPRAVA as usize];
        loop {
            match self.udp.recv_from(&mut b) {
                Ok((n, _)) => {
                    let akcie = klient.prijmi(&b[..n], self.teraz(), nahoda32());
                    if let Some(p) = self.vykonaj(akcie, hlas) {
                        novy = Some(p);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return novy,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return novy,
            }
        }
    }
}

/// Pošle otázku na server (DNS, NTP) a počká na jednu odpoveď len od neho (connect filtruje cudzích).
fn otazka_udp(kam: SocketAddr, data: &[u8]) -> io::Result<Vec<u8>> {
    let s = UdpSocket::bind(if kam.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
    s.set_read_timeout(Some(CAKANIE_NA_ODPOVED))?;
    s.connect(kam)?;
    s.send(data)?;
    let mut b = vec![0u8; 2048];
    let n = s.recv(&mut b)?;
    b.truncate(n);
    Ok(b)
}

fn nahoda16() -> u16 {
    u16::from_ne_bytes(nahoda_bajty())
}

/// Preklad mena cez servery DNS z prenájmu.
pub fn preloz(
    prekladac: &mut dns::Prekladac,
    meno: &str,
    od: Instant,
) -> Result<Vec<IpAddr>, dns::ChybaPrekladu> {
    prekladac.preloz(meno, dns::Typ::A, od.elapsed(), &mut otazka_udp, &mut nahoda16)
}

/// Výsledok overenia času.
struct Cas {
    server: IpAddr,
    vysledok: sntp::Vysledok,
}

/// Opýta sa serverov NTP (najviac 4 otázky) a vyberie odpoveď s najmenším oneskorením (RFC 4330 5).
fn zisti_cas(servery: &[IpAddr]) -> Result<Cas, String> {
    let mut najlepsi: Option<Cas> = None;
    let mut chyba = String::from("žiadny server času");
    for &server in servery.iter().take(4) {
        let znacka = u64::from_ne_bytes(nahoda_bajty());
        let t1 = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let odpoved = otazka_udp(SocketAddr::new(server, sntp::PORT), &sntp::ziadost(znacka));
        let t4 = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        match odpoved
            .map_err(|e| e.to_string())
            .and_then(|d| sntp::spracuj(&d, znacka, t1, t4).map_err(|e| e.to_string()))
        {
            Ok(v) => {
                if najlepsi.as_ref().is_none_or(|n| v.oneskorenie < n.vysledok.oneskorenie) {
                    najlepsi = Some(Cas { server, vysledok: v });
                }
            }
            Err(e) => chyba = format!("{server}: {e}"),
        }
    }
    najlepsi.ok_or(chyba)
}

fn posun_text(ns: i128) -> String {
    let ms = ns as f64 / 1e6;
    if ms.abs() >= 86_400_000.0 {
        format!("{:+.1} dňa", ms / 86_400_000.0).replace('.', ",")
    } else if ms.abs() >= 1000.0 {
        format!("{:+.1} s", ms / 1000.0).replace('.', ",")
    } else {
        format!("{ms:+.1} ms").replace('.', ",")
    }
}

/// Po získaní adresy: overí preklad mien a čas; čas nastaví, ak `nastavit` a posun je väčší ako 0,1 s.
fn over_internet(p: &dhcp::Prenajom, nastavit: bool, hlas: fn(&str)) -> (Option<String>, Option<String>) {
    let od = Instant::now();
    let mut prekladac = dns::Prekladac::novy(p.dns.iter().map(|a| IpAddr::V4(*a)).collect());
    let (preklad, ntp_mena) = match preloz(&mut prekladac, SERVER_CASU, od) {
        Ok(a) => (format!("✓ ({SERVER_CASU} → {}, {} ms)", a[0], od.elapsed().as_millis()), a),
        Err(e) => (format!("✗ ({e})"), Vec::new()),
    };
    hlas(&format!("internet (R268): preklad mien {preklad}"));
    let servery: Vec<IpAddr> =
        if p.ntp.is_empty() { ntp_mena } else { p.ntp.iter().map(|a| IpAddr::V4(*a)).collect() };
    let cas = match zisti_cas(&servery) {
        Ok(c) => {
            let v = c.vysledok;
            let mut t = format!(
                "✓ ({}, stratum {}, oneskorenie {} ms, posun {}",
                c.server,
                v.stratum,
                v.oneskorenie.as_millis(),
                posun_text(v.posun_ns)
            );
            if nastavit && v.posun_ns.abs() > PRESNOST_CASU_NS {
                let teraz = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
                match crate::sys::nastav_cas(v.cas_pri_prijati(teraz)) {
                    Ok(()) => t += " – hodiny nastavené",
                    Err(e) => t += &format!(" – hodiny sa nenastavili: {e}"),
                }
            }
            t + ")"
        }
        Err(e) => format!("✗ ({e})"),
    };
    hlas(&format!("internet (R268): čas zo siete {cas}"));
    (Some(preklad), Some(cas))
}

/// Spustí vlákno internetu (raz za beh KLÍČA).
pub fn spusti(hlas: fn(&str)) {
    let _ = std::thread::Builder::new().name("internet".into()).spawn(move || vlakno(hlas));
}

fn vlakno(hlas: fn(&str)) {
    let _ = crate::sys::nizka_priorita();
    let udalosti = match crate::sys::netlink_route(1) {
        Ok(u) => u,
        Err(e) => {
            zmen(|s| s.chyba = Some(format!("udalosti siete nedostupné ({e})")));
            return;
        }
    };
    // rozhranie sa môže objaviť neskôr (USB adaptér, ovládač) – čaká sa na udalosť, nie v slučke
    let (meno, cislo, mac) = loop {
        if let Some(r) = kablove_rozhranie() {
            break r;
        }
        zmen(|s| s.dhcp = "bez káblového rozhrania".into());
        let _ = crate::sys::cakaj_na_citanie(&[udalosti.as_raw_fd()], Some(600_000));
        vyber_udalosti(&udalosti);
    };
    let mut r = match Rozhranie::otvor(&meno, cislo) {
        Ok(r) => r,
        Err(e) => {
            hlas(&format!("internet (R268): rozhranie {meno} sa nedá použiť ({e})"));
            zmen(|s| s.chyba = Some(format!("{meno}: {e}")));
            return;
        }
    };
    hlas(&format!("internet (R268): klient DHCP na {meno} (vlastný, RFC 2131)"));
    let mut klient = dhcp::Klient::novy(mac);
    let mut pripojene = kabel(&meno);
    let mut kabel_od = r.teraz();
    if !pripojene {
        klient.linka(false, r.teraz(), 0);
    }
    let mut cas_overeny: Option<Duration> = None;
    loop {
        let akcie = klient.tik(r.teraz(), nahoda32());
        let mut novy = r.vykonaj(akcie, hlas);
        // spánok do času z automatu, paketu alebo udalosti kábla
        let caka = klient.kedy().map(|k| k.saturating_sub(r.teraz()).as_millis() as u64 + 1);
        let pripravene = crate::sys::cakaj_na_citanie(&[r.udp.as_raw_fd(), udalosti.as_raw_fd()], caka)
            .unwrap_or_else(|_| vec![false, false]);
        if pripravene.first() == Some(&true)
            && let Some(p) = r.prijmi(&mut klient, hlas)
        {
            novy = Some(p);
        }
        if pripravene.get(1) == Some(&true) {
            vyber_udalosti(&udalosti);
            let k = kabel(&meno);
            if k != pripojene {
                pripojene = k;
                kabel_od = r.teraz();
                let akcie = klient.linka(k, r.teraz(), nahoda32());
                if let Some(p) = r.vykonaj(akcie, hlas) {
                    novy = Some(p);
                }
            }
        }
        if let Some(p) = &novy {
            hlas(&format!(
                "internet (R268): adresa {}/{} od {} (prenájom {})",
                p.adresa,
                p.predpona,
                p.server,
                p.trvanie.map_or("neobmedzený".into(), |d| format!("{} min", d.as_secs() / 60))
            ));
            if cas_overeny.is_none_or(|c| r.teraz().saturating_sub(c) >= OBNOVA_CASU) {
                let (preklad, cas) = over_internet(p, true, hlas);
                if cas.as_deref().is_some_and(|c| c.starts_with('✓')) {
                    cas_overeny = Some(r.teraz());
                }
                zmen(|s| {
                    s.preklad = preklad;
                    s.cas = cas;
                });
            }
        }
        let stav = klient.stav();
        let hlada = matches!(
            stav,
            dhcp::Stav::Zaciatok | dhcp::Stav::Vyber | dhcp::Stav::Ziadost | dhcp::Stav::Znovu
        );
        let obnovuje = matches!(stav, dhcp::Stav::Obnova | dhcp::Stav::ObnovaVsetkym);
        let ziada = obnovuje || (hlada && r.teraz().saturating_sub(kabel_od) < HLADANIE_SERVERA);
        zmen(|s| {
            s.rozhranie = Some(r.meno.clone());
            s.kabel = Some(pripojene);
            s.dhcp = klient.popis();
            s.prenajom = klient.prenajom().cloned();
            s.ziada_siet = ziada;
            if let Some(o) = klient.odmietnutie() {
                s.chyba = Some(format!("server DHCP odmietol: {o}"));
            }
        });
    }
}

/// Vyberie čakajúce udalosti rtnetlink (obsah netreba – stav kábla sa číta zo /sys).
fn vyber_udalosti(u: &std::fs::File) {
    let mut b = [0u8; 4096];
    while crate::sys::cakaj_na_citanie(&[u.as_raw_fd()], Some(0)).is_ok_and(|v| v[0]) {
        if (&*u).read(&mut b).is_err() {
            break;
        }
    }
}

/// Skúška pre CI a ručné overenie: `klic --internet <rozhranie> [--meno <meno>]… [--stiahni <url>]… [--uloz <priečinok>] [--nastav-cas]`.
/// Získa adresu (najviac 60 s), nastaví ju, preloží mená a opýta sa na čas; vypíše riadky `INTERNET …`.
/// Návratový kód 0 = adresa, všetky mená aj čas (ak server času existuje) v poriadku.
pub fn skuska(argumenty: &[String]) -> i32 {
    let hlas = |t: &str| println!("{t}");
    let meno = argumenty.iter().position(|a| a == "--internet").and_then(|i| argumenty.get(i + 1));
    let Some(meno) = meno else {
        eprintln!("použitie: klic --internet <rozhranie> [--meno <meno>]… [--nastav-cas]");
        return 2;
    };
    let mena: Vec<&String> = argumenty.windows(2).filter(|w| w[0] == "--meno").map(|w| &w[1]).collect();
    let nastavit = argumenty.iter().any(|a| a == "--nastav-cas");
    let z = format!("/sys/class/net/{meno}");
    let (Some(mac), Some(cislo)) = (
        std::fs::read_to_string(format!("{z}/address")).ok().and_then(|a| mac_z_textu(a.trim())),
        std::fs::read_to_string(format!("{z}/ifindex")).ok().and_then(|c| c.trim().parse::<u32>().ok()),
    ) else {
        println!("INTERNET CHYBA rozhranie {meno} neexistuje");
        return 1;
    };
    let _ = crate::sys::zapni_rozhranie(meno);
    let mut r = match Rozhranie::otvor(meno, cislo) {
        Ok(r) => r,
        Err(e) => {
            println!("INTERNET CHYBA zásuvka DHCP: {e}");
            return 1;
        }
    };
    let mut klient = dhcp::Klient::novy(mac);
    let mut prenajom = None;
    while r.teraz() < Duration::from_secs(60) && prenajom.is_none() {
        let akcie = klient.tik(r.teraz(), nahoda32());
        prenajom = r.vykonaj(akcie, hlas);
        let caka =
            klient.kedy().map_or(1000, |k| k.saturating_sub(r.teraz()).as_millis() as u64 + 1).min(1000);
        if crate::sys::cakaj_na_citanie(&[r.udp.as_raw_fd()], Some(caka)).is_ok_and(|v| v[0])
            && let Some(p) = r.prijmi(&mut klient, hlas)
        {
            prenajom = Some(p);
        }
    }
    let Some(p) = prenajom else {
        println!("INTERNET CHYBA bez adresy po 60 s – {}", klient.popis());
        return 1;
    };
    println!(
        "INTERNET ADRESA {}/{} brana={} dns={} ntp={} server={} prenajom_s={} t1_s={} t2_s={}",
        p.adresa,
        p.predpona,
        p.brany.first().map_or("-".into(), |a| a.to_string()),
        p.dns.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(","),
        p.ntp.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(","),
        p.server,
        p.trvanie.map_or("-".into(), |d| d.as_secs().to_string()),
        p.t1.map_or("-".into(), |d| d.as_secs().to_string()),
        p.t2.map_or("-".into(), |d| d.as_secs().to_string()),
    );
    let mut ok = true;
    let od = Instant::now();
    let mut prekladac = dns::Prekladac::novy(p.dns.iter().map(|a| IpAddr::V4(*a)).collect());
    for m in &mena {
        match preloz(&mut prekladac, m, od) {
            Ok(a) => println!(
                "INTERNET MENO {m} {}",
                a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
            ),
            Err(e) => {
                ok = false;
                println!("INTERNET CHYBA meno {m}: {e}");
            }
        }
    }
    if !p.ntp.is_empty() {
        let servery: Vec<IpAddr> = p.ntp.iter().map(|a| IpAddr::V4(*a)).collect();
        match zisti_cas(&servery) {
            Ok(c) => {
                let v = c.vysledok;
                println!(
                    "INTERNET CAS server={} stratum={} oneskorenie_us={} posun_ms={:.3}",
                    c.server,
                    v.stratum,
                    v.oneskorenie.as_micros(),
                    v.posun_ns as f64 / 1e6
                );
                if nastavit && v.posun_ns.abs() > PRESNOST_CASU_NS {
                    let teraz = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
                    if let Err(e) = crate::sys::nastav_cas(v.cas_pri_prijati(teraz)) {
                        println!("INTERNET CHYBA hodiny: {e}");
                        ok = false;
                    }
                }
            }
            Err(e) => {
                ok = false;
                println!("INTERNET CHYBA čas: {e}");
            }
        }
    }
    // sťahovanie cez HTTP (rozhranie R6) – dĺžka a odtlačok BLAKE3 na porovnanie s rozhodcom
    let adresy: Vec<&String> = argumenty.windows(2).filter(|w| w[0] == "--stiahni").map(|w| &w[1]).collect();
    for a in adresy {
        let mut n = nahoda16;
        match orchidea_siet::http::stiahni(a, &[], &mut prekladac, od.elapsed(), &mut n, Default::default()) {
            Ok(mut o) => match o.telo.cele(64 << 20) {
                Ok(t) => {
                    let odtlacok: String = sklenik_korene::aktualizacia::odtlacok(&t)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    println!(
                        "INTERNET STIAHNUTE {a} stav={} bajtov={} blake3={odtlacok} z={}",
                        o.hlavicka.stav,
                        t.len(),
                        o.url
                    );
                    if o.hlavicka.stav != 200 {
                        ok = false;
                    }
                    // `--uloz <priečinok>`: telo do súboru s menom posledného úseku cesty (rozhodca ho porovná `cmp`)
                    if let Some(kam) = argumenty.windows(2).find(|w| w[0] == "--uloz").map(|w| &w[1]) {
                        let meno =
                            o.url.cesta.split('?').next().unwrap_or("").rsplit('/').next().unwrap_or("");
                        let meno = if meno.is_empty() { "index" } else { meno };
                        if let Err(e) = std::fs::write(std::path::Path::new(kam).join(meno), &t) {
                            ok = false;
                            println!("INTERNET CHYBA uloženie {meno}: {e}");
                        }
                    }
                }
                Err(e) => {
                    ok = false;
                    println!("INTERNET CHYBA telo {a}: {e}");
                }
            },
            Err(e) => {
                ok = false;
                println!("INTERNET CHYBA stiahni {a}: {e}");
            }
        }
    }
    // obnova prenájmu u servera (RENEWING) – overí unicast cez nastavenú adresu
    if p.t1.is_some() {
        let t1 = p.od + p.t1.unwrap_or_default();
        let mut obnoveny = None;
        let koniec = r.teraz() + Duration::from_secs(20);
        // posun hodín automatu na T1 netreba čakať – klient sa spýta hneď s časom T1
        let akcie = klient.tik(t1.max(r.teraz()), nahoda32());
        r.vykonaj(akcie, hlas);
        while r.teraz() < koniec && obnoveny.is_none() {
            if crate::sys::cakaj_na_citanie(&[r.udp.as_raw_fd()], Some(500)).is_ok_and(|v| v[0]) {
                obnoveny = r.prijmi(&mut klient, hlas);
            }
        }
        match obnoveny {
            Some(n) if n.adresa == p.adresa => println!("INTERNET OBNOVA {} ✓", n.adresa),
            _ => {
                ok = false;
                println!("INTERNET CHYBA obnova prenájmu – {}", klient.popis());
            }
        }
    }
    println!("INTERNET VYSLEDOK {}", if ok { "ok" } else { "chyba" });
    i32::from(!ok)
}

#[cfg(test)]
mod testy {
    use super::*;

    fn prenajom() -> dhcp::Prenajom {
        dhcp::Prenajom {
            adresa: Ipv4Addr::new(192, 168, 1, 42),
            predpona: 24,
            brany: vec![Ipv4Addr::new(192, 168, 1, 1)],
            dns: vec![],
            ntp: vec![],
            domena: None,
            mtu: None,
            server: Ipv4Addr::new(192, 168, 1, 1),
            od: Duration::ZERO,
            trvanie: Some(Duration::from_secs(3600)),
            t1: None,
            t2: None,
        }
    }

    #[test]
    fn netlink_adresa_bajt_po_bajte() {
        let m = sprava_adresy(true, 7, 2, &prenajom(), 3600);
        assert_eq!(
            m.len(),
            16 + 8 + 8 + 8 + 8 + 20,
            "hlavička, ifaddrmsg, LOCAL, ADDRESS, BROADCAST, CACHEINFO"
        );
        assert_eq!(u32::from_ne_bytes(m[0..4].try_into().unwrap()) as usize, m.len());
        assert_eq!(u16::from_ne_bytes([m[4], m[5]]), RTM_NEWADDR);
        assert_eq!(u16::from_ne_bytes([m[6], m[7]]), 0x0505, "REQUEST, ACK, CREATE, REPLACE");
        assert_eq!(u32::from_ne_bytes(m[8..12].try_into().unwrap()), 7);
        assert_eq!(&m[16..20], &[AF_INET, 24, 0, RT_SCOPE_UNIVERSE]);
        assert_eq!(u32::from_ne_bytes(m[20..24].try_into().unwrap()), 2, "číslo rozhrania");
        assert_eq!(u16::from_ne_bytes([m[26], m[27]]), IFA_LOCAL);
        assert_eq!(&m[28..32], &[192, 168, 1, 42]);
        assert_eq!(&m[44..48], &[192, 168, 1, 255], "adresa všetkým v sieti /24");
        assert_eq!(u16::from_ne_bytes([m[48], m[49]]), 20, "dĺžka CACHEINFO");
        assert_eq!(u32::from_ne_bytes(m[52..56].try_into().unwrap()), 3600, "preferovaná");
        assert_eq!(u32::from_ne_bytes(m[56..60].try_into().unwrap()), 3600, "platná");
        let z = sprava_adresy(false, 8, 2, &prenajom(), 0);
        assert_eq!(u16::from_ne_bytes([z[4], z[5]]), RTM_DELADDR);
        assert_eq!(z.len(), 16 + 8 + 8 + 8 + 8, "bez CACHEINFO");
    }

    #[test]
    fn netlink_trasa_bajt_po_bajte() {
        let m = sprava_trasy(true, 3, 2, Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(192, 168, 1, 42));
        assert_eq!(m.len(), 16 + 12 + 8 + 8 + 8);
        assert_eq!(u16::from_ne_bytes([m[4], m[5]]), RTM_NEWROUTE);
        assert_eq!(
            &m[16..24],
            &[AF_INET, 0, 0, 0, RT_TABLE_MAIN, RTPROT_DHCP, RT_SCOPE_UNIVERSE, RTN_UNICAST]
        );
        assert_eq!(u16::from_ne_bytes([m[30], m[31]]), RTA_GATEWAY);
        assert_eq!(&m[32..36], &[192, 168, 1, 1]);
        assert_eq!(u16::from_ne_bytes([m[38], m[39]]), RTA_OIF);
        assert_eq!(&m[48..52], &[192, 168, 1, 42], "zdrojová adresa");
    }

    #[test]
    fn potvrdenie_jadra() {
        let mut ok = netlink_sprava(NLMSG_ERROR, 0, 9, &0i32.to_ne_bytes());
        ok.extend_from_slice(&[0; 16]); // pôvodná hlavička
        assert!(matches!(potvrdenie(&ok, 9), Some(Ok(()))));
        assert!(potvrdenie(&ok, 10).is_none(), "iné poradie");
        let zle = netlink_sprava(NLMSG_ERROR, 0, 9, &(-17i32).to_ne_bytes());
        let e = potvrdenie(&zle, 9).unwrap().unwrap_err();
        assert_eq!(e.raw_os_error(), Some(17), "EEXIST");
        assert!(potvrdenie(&[1, 2, 3], 9).is_none());
    }

    #[test]
    fn mac_a_posun() {
        assert_eq!(mac_z_textu("b8:27:eb:12:34:56"), Some([0xb8, 0x27, 0xeb, 0x12, 0x34, 0x56]));
        assert_eq!(mac_z_textu("00:00:00:00:00:00"), None);
        assert_eq!(mac_z_textu("b8:27:eb:12:34"), None);
        assert_eq!(posun_text(1_500_000), "+1,5 ms");
        assert_eq!(posun_text(-2_500_000_000), "-2,5 s");
        assert_eq!(posun_text(1_795_000_000_000_000_000), "+20775,5 dňa");
    }
}
