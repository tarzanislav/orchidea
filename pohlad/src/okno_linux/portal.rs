//! Dialóg výberu súboru a priečinka v Linuxe cez službu prostredia xdg-desktop-portal
//! (rozhranie `org.freedesktop.portal.FileChooser`) – dialóg ukáže prostredie (GNOME, KDE …) vo svojom vzhľade.
//!
//! Vlastný klient zbernice D-Bus podľa špecifikácie D-Bus (freedesktop.org): adresa zbernice, overenie
//! EXTERNAL, kódovanie správ (zarovnanie, hlavička, polia), volanie metódy a čakanie na signál `Response`.
//! Beží v pomocnom vlákne – okno medzitým ďalej reaguje.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Hodnota D-Bus (len to, čo dialóg potrebuje; ostatné sa prečíta a preskočí).
#[derive(Debug, Clone, PartialEq)]
pub enum Hodnota {
    Cislo(u64),
    Text(String),
    Pole(Vec<Hodnota>),
    Struktura(Vec<Hodnota>),
    Variant(Box<Hodnota>),
}

impl Hodnota {
    fn text(&self) -> Option<&str> {
        match self {
            Hodnota::Text(t) => Some(t),
            Hodnota::Variant(v) => v.text(),
            _ => None,
        }
    }
}

/// Zápis tela správy so zarovnaním (relatívne k začiatku správy; telo začína na násobku 8).
#[derive(Default)]
struct Zapis {
    b: Vec<u8>,
}

impl Zapis {
    fn zarovnaj(&mut self, n: usize) {
        while !self.b.len().is_multiple_of(n) {
            self.b.push(0);
        }
    }
    fn y(&mut self, v: u8) {
        self.b.push(v);
    }
    fn u(&mut self, v: u32) {
        self.zarovnaj(4);
        self.b.extend_from_slice(&v.to_le_bytes());
    }
    fn s(&mut self, v: &str) {
        self.u(v.len() as u32);
        self.b.extend_from_slice(v.as_bytes());
        self.b.push(0);
    }
    fn g(&mut self, v: &str) {
        self.y(v.len() as u8);
        self.b.extend_from_slice(v.as_bytes());
        self.b.push(0);
    }
    /// Pole: dĺžka, zarovnanie prvkov, prvky.
    fn pole(&mut self, zarovnanie: usize, f: impl FnOnce(&mut Zapis)) {
        self.u(0);
        let miesto = self.b.len() - 4;
        self.zarovnaj(zarovnanie);
        let zaciatok = self.b.len();
        f(self);
        let n = (self.b.len() - zaciatok) as u32;
        self.b[miesto..miesto + 4].copy_from_slice(&n.to_le_bytes());
    }
    /// Položka slovníka a{sv}: kľúč a variant.
    fn polozka(&mut self, kluc: &str, podpis: &str, f: impl FnOnce(&mut Zapis)) {
        self.zarovnaj(8);
        self.s(kluc);
        self.g(podpis);
        f(self);
    }
}

/// Čítanie hodnôt podľa podpisu (zarovnanie relatívne k začiatku správy).
struct Citanie<'a> {
    d: &'a [u8],
    i: usize,
}

impl Citanie<'_> {
    fn zarovnaj(&mut self, n: usize) {
        self.i = self.i.div_ceil(n) * n;
    }
    fn bajty(&mut self, n: usize) -> io::Result<&[u8]> {
        let z = self.d.get(self.i..self.i + n).ok_or_else(|| chyba("správa D-Bus je kratšia"))?;
        self.i += n;
        Ok(z)
    }
    fn u(&mut self) -> io::Result<u32> {
        self.zarovnaj(4);
        let b = self.bajty(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// Jeden úplný typ podpisu od pozície `p`; vráti jeho koniec.
    fn koniec_typu(podpis: &[u8], p: usize) -> io::Result<usize> {
        match podpis.get(p) {
            Some(b'a') => Self::koniec_typu(podpis, p + 1),
            Some(&o @ (b'(' | b'{')) => {
                let z = if o == b'(' { b')' } else { b'}' };
                let mut q = p + 1;
                while podpis.get(q) != Some(&z) {
                    if q >= podpis.len() {
                        return Err(chyba("neuzavretý podpis D-Bus"));
                    }
                    q = Self::koniec_typu(podpis, q)?;
                }
                Ok(q + 1)
            }
            Some(_) => Ok(p + 1),
            None => Err(chyba("prázdny podpis D-Bus")),
        }
    }
    fn hodnota(&mut self, podpis: &[u8], hlbka: usize) -> io::Result<Hodnota> {
        if hlbka > 32 {
            return Err(chyba("príliš vnorená hodnota D-Bus"));
        }
        Ok(match podpis.first() {
            Some(b'y') => Hodnota::Cislo(self.bajty(1)?[0] as u64),
            Some(b'b' | b'u' | b'i' | b'h') => Hodnota::Cislo(self.u()? as u64),
            Some(b'n' | b'q') => {
                self.zarovnaj(2);
                let b = self.bajty(2)?;
                Hodnota::Cislo(u16::from_le_bytes([b[0], b[1]]) as u64)
            }
            Some(b'x' | b't' | b'd') => {
                self.zarovnaj(8);
                let b = self.bajty(8)?;
                Hodnota::Cislo(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
            }
            Some(b's' | b'o') => {
                let n = self.u()? as usize;
                let t = String::from_utf8_lossy(self.bajty(n)?).into_owned();
                self.bajty(1)?;
                Hodnota::Text(t)
            }
            Some(b'g') => {
                let n = self.bajty(1)?[0] as usize;
                let t = String::from_utf8_lossy(self.bajty(n)?).into_owned();
                self.bajty(1)?;
                Hodnota::Text(t)
            }
            Some(b'v') => {
                let n = self.bajty(1)?[0] as usize;
                let p = self.bajty(n)?.to_vec();
                self.bajty(1)?;
                Hodnota::Variant(Box::new(self.hodnota(&p, hlbka + 1)?))
            }
            Some(b'a') => {
                let n = self.u()? as usize;
                let prvok = &podpis[1..Self::koniec_typu(podpis, 1)?];
                self.zarovnaj(zarovnanie(prvok));
                let koniec = self.i + n;
                if koniec > self.d.len() {
                    return Err(chyba("pole D-Bus je za koncom správy"));
                }
                let mut v = Vec::new();
                while self.i < koniec {
                    v.push(self.hodnota(prvok, hlbka + 1)?);
                }
                Hodnota::Pole(v)
            }
            Some(b'(' | b'{') => {
                self.zarovnaj(8);
                let k = Self::koniec_typu(podpis, 0)?;
                let mut q = 1;
                let mut v = Vec::new();
                while q < k - 1 {
                    let e = Self::koniec_typu(podpis, q)?;
                    v.push(self.hodnota(&podpis[q..e], hlbka + 1)?);
                    q = e;
                }
                Hodnota::Struktura(v)
            }
            _ => return Err(chyba("neznámy typ D-Bus")),
        })
    }
}

fn zarovnanie(podpis: &[u8]) -> usize {
    match podpis.first() {
        Some(b'y' | b'g' | b'v') => 1,
        Some(b'n' | b'q') => 2,
        Some(b'x' | b't' | b'd' | b'(' | b'{') => 8,
        _ => 4,
    }
}

fn chyba(t: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, t.to_string())
}

/// Prijatá správa.
#[derive(Debug)]
pub struct Sprava {
    pub typ: u8,
    pub cesta: String,
    pub rozhranie: String,
    pub clen: String,
    pub odpoved_na: u32,
    pub telo: Vec<Hodnota>,
}

/// Správa na odoslanie (typ 1 = volanie metódy).
fn sprava(
    serial: u32,
    ciel: &str,
    cesta: &str,
    rozhranie: &str,
    clen: &str,
    podpis: &str,
    telo: &[u8],
) -> Vec<u8> {
    let mut h = Zapis::default();
    h.b.extend_from_slice(&[b'l', 1, 0, 1]);
    h.u(telo.len() as u32);
    h.u(serial);
    h.pole(8, |h| {
        let mut pole = |kod: u8, typ: &str, f: &dyn Fn(&mut Zapis)| {
            h.zarovnaj(8);
            h.y(kod);
            h.g(typ);
            f(h);
        };
        pole(1, "o", &|h: &mut Zapis| h.s(cesta));
        pole(2, "s", &|h: &mut Zapis| h.s(rozhranie));
        pole(3, "s", &|h: &mut Zapis| h.s(clen));
        pole(6, "s", &|h: &mut Zapis| h.s(ciel));
        if !podpis.is_empty() {
            pole(8, "g", &|h: &mut Zapis| h.g(podpis));
        }
    });
    h.zarovnaj(8);
    h.b.extend_from_slice(telo);
    h.b
}

fn citaj_spravu(r: &mut impl Read) -> io::Result<Sprava> {
    let mut z = [0u8; 16];
    r.read_exact(&mut z)?;
    if z[0] != b'l' {
        return Err(chyba("správa D-Bus nie je little-endian"));
    }
    let telo = u32::from_le_bytes([z[4], z[5], z[6], z[7]]) as usize;
    let polia = u32::from_le_bytes([z[12], z[13], z[14], z[15]]) as usize;
    if telo > 64 << 20 || polia > 1 << 20 {
        return Err(chyba("príliš veľká správa D-Bus"));
    }
    let hlavicka = (16 + polia).div_ceil(8) * 8;
    let mut d = z.to_vec();
    d.resize(hlavicka + telo, 0);
    r.read_exact(&mut d[16..])?;
    let mut c = Citanie { d: &d, i: 12 };
    let polia = c.hodnota(b"a(yv)", 0)?;
    let mut s = Sprava {
        typ: z[1],
        cesta: String::new(),
        rozhranie: String::new(),
        clen: String::new(),
        odpoved_na: 0,
        telo: Vec::new(),
    };
    let mut podpis = String::new();
    if let Hodnota::Pole(v) = polia {
        for p in v {
            if let Hodnota::Struktura(p) = p
                && let [Hodnota::Cislo(kod), h] = &p[..]
            {
                let t = h.text().unwrap_or_default().to_string();
                match kod {
                    1 => s.cesta = t,
                    2 => s.rozhranie = t,
                    3 => s.clen = t,
                    5 => {
                        if let Hodnota::Variant(v) = h
                            && let Hodnota::Cislo(n) = **v
                        {
                            s.odpoved_na = n as u32;
                        }
                    }
                    8 => podpis = t,
                    _ => {}
                }
            }
        }
    }
    let mut c = Citanie { d: &d, i: hlavicka };
    let p = podpis.as_bytes();
    let mut q = 0;
    while q < p.len() {
        let e = Citanie::koniec_typu(p, q)?;
        s.telo.push(c.hodnota(&p[q..e], 0)?);
        q = e;
    }
    Ok(s)
}

/// Cesta k zbernici z adresy (`unix:path=…` alebo `unix:abstract=…`).
fn pripoj_adresu(adresa: &str) -> io::Result<UnixStream> {
    let mut posledna = chyba("adresa zbernice D-Bus nemá podporovaný tvar");
    for a in adresa.split(';') {
        let Some(k) = a.strip_prefix("unix:") else { continue };
        for p in k.split(',') {
            let (kluc, hodnota) = p.split_once('=').unwrap_or((p, ""));
            let hodnota = odescapuj(hodnota);
            let r = match kluc {
                "path" => UnixStream::connect(&hodnota),
                "abstract" => {
                    use std::os::linux::net::SocketAddrExt;
                    std::os::unix::net::SocketAddr::from_abstract_name(hodnota.as_bytes())
                        .and_then(|a| UnixStream::connect_addr(&a))
                }
                _ => continue,
            };
            match r {
                Ok(z) => return Ok(z),
                Err(e) => posledna = e,
            }
        }
    }
    Err(posledna)
}

fn odescapuj(t: &str) -> String {
    let b = t.as_bytes();
    let mut v = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            v.push((h * 16 + l) as u8);
            i += 3;
        } else {
            v.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&v).into_owned()
}

/// Spojenie so zbernicou (po overení a Hello).
pub struct Zbernica {
    r: BufReader<UnixStream>,
    w: UnixStream,
    serial: u32,
    pub meno: String,
}

impl Zbernica {
    /// Pripojí sa na zbernicu relácie (`DBUS_SESSION_BUS_ADDRESS`).
    pub fn relacie() -> io::Result<Zbernica> {
        let adresa =
            std::env::var("DBUS_SESSION_BUS_ADDRESS").map_err(|_| chyba("chýba DBUS_SESSION_BUS_ADDRESS"))?;
        let z = pripoj_adresu(&adresa)?;
        z.set_read_timeout(Some(Duration::from_secs(10)))?;
        Zbernica::na(z)
    }

    /// Overenie EXTERNAL (číslo používateľa z vlastníka `/proc/self`) a Hello.
    pub fn na(z: UnixStream) -> io::Result<Zbernica> {
        use std::os::unix::fs::MetadataExt;
        let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
        let hex: String = uid.to_string().bytes().map(|b| format!("{b:02x}")).collect();
        let mut w = z.try_clone()?;
        w.write_all(b"\0")?;
        w.write_all(format!("AUTH EXTERNAL {hex}\r\n").as_bytes())?;
        let mut r = BufReader::new(z);
        let mut riadok = String::new();
        r.read_line(&mut riadok)?;
        if !riadok.starts_with("OK ") {
            return Err(chyba("zbernica D-Bus odmietla overenie"));
        }
        w.write_all(b"BEGIN\r\n")?;
        let mut b = Zbernica { r, w, serial: 0, meno: String::new() };
        let s = b.volaj(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "Hello",
            "",
            &[],
        )?;
        b.meno = s.telo.first().and_then(Hodnota::text).unwrap_or_default().to_string();
        Ok(b)
    }

    /// Volanie metódy; čaká na odpoveď (signály medzitým zahodí).
    pub fn volaj(
        &mut self,
        ciel: &str,
        cesta: &str,
        rozhranie: &str,
        clen: &str,
        podpis: &str,
        telo: &[u8],
    ) -> io::Result<Sprava> {
        self.serial += 1;
        let serial = self.serial;
        self.w.write_all(&sprava(serial, ciel, cesta, rozhranie, clen, podpis, telo))?;
        loop {
            let s = citaj_spravu(&mut self.r)?;
            if (s.typ == 2 || s.typ == 3) && s.odpoved_na == serial {
                if s.typ == 3 {
                    let t = s.telo.first().and_then(Hodnota::text).unwrap_or("chyba").to_string();
                    return Err(io::Error::other(format!("{clen}: {t}")));
                }
                return Ok(s);
            }
        }
    }

    /// Ďalší signál (blokuje do časového limitu spojenia).
    pub fn signal(&mut self) -> io::Result<Sprava> {
        loop {
            let s = citaj_spravu(&mut self.r)?;
            if s.typ == 4 {
                return Ok(s);
            }
        }
    }
}

/// Je k dispozícii služba dialógov (portál je spustený alebo sa dá spustiť na požiadanie)?
pub fn je_k_dispozicii() -> bool {
    let Ok(mut b) = Zbernica::relacie() else { return false };
    let Ok(s) = b.volaj(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "ListActivatableNames",
        "",
        &[],
    ) else {
        return false;
    };
    let aktivovatelne = matches!(s.telo.first(), Some(Hodnota::Pole(v)) if v.iter().any(|m| m.text() == Some("org.freedesktop.portal.Desktop")));
    if aktivovatelne {
        return true;
    }
    let mut t = Zapis::default();
    t.s("org.freedesktop.portal.Desktop");
    matches!(
        b.volaj(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            "s",
            &t.b
        )
        .map(|s| s.telo.first().cloned()),
        Ok(Some(Hodnota::Cislo(1)))
    )
}

/// Telo volania OpenFile: rodičovské okno, titulok, voľby.
fn telo_otvor(titulok: &str, priecinok: Option<&Path>, adresar: bool, znacka: &str) -> Vec<u8> {
    let mut t = Zapis::default();
    t.s("");
    t.s(titulok);
    t.pole(8, |t| {
        t.polozka("handle_token", "s", |t| t.s(znacka));
        t.polozka("modal", "b", |t| t.u(1));
        if adresar {
            t.polozka("directory", "b", |t| t.u(1));
        } else {
            // filters a(sa(us)): obrázky podľa typu MIME, všetky súbory
            t.polozka("filters", "a(sa(us))", |t| {
                t.pole(8, |t| {
                    for (meno, typ, vzor) in [("Obrázky", 1, "image/*"), ("Všetky súbory", 0, "*")] {
                        t.zarovnaj(8);
                        t.s(meno);
                        t.pole(8, |t| {
                            t.zarovnaj(8);
                            t.u(typ);
                            t.s(vzor);
                        });
                    }
                });
            });
        }
        if let Some(p) = priecinok {
            use std::os::unix::ffi::OsStrExt;
            t.polozka("current_folder", "ay", |t| {
                t.pole(1, |t| {
                    t.b.extend_from_slice(p.as_os_str().as_bytes());
                    t.b.push(0);
                });
            });
        }
    });
    t.b
}

/// Ukáže dialóg výberu súboru (alebo priečinka) a počká na voľbu. `Ok(None)` = zrušené.
pub fn vyber(
    mut b: Zbernica,
    titulok: &str,
    priecinok: Option<&Path>,
    adresar: bool,
) -> io::Result<Option<PathBuf>> {
    let znacka = format!("orchidea{}", std::process::id());
    let odosielatel = b.meno.trim_start_matches(':').replace('.', "_");
    let cesta = format!("/org/freedesktop/portal/desktop/request/{odosielatel}/{znacka}");
    // odber signálu ešte pred volaním (inak by odpoveď mohla prísť skôr)
    let mut t = Zapis::default();
    t.s(&format!(
        "type='signal',interface='org.freedesktop.portal.Request',member='Response',path='{cesta}'"
    ));
    b.volaj("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch", "s", &t.b)?;
    b.volaj(
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.FileChooser",
        "OpenFile",
        "ssa{sv}",
        &telo_otvor(titulok, priecinok, adresar, &znacka),
    )?;
    // dialóg môže byť otvorený dlho
    b.r.get_ref().set_read_timeout(None)?;
    loop {
        let s = b.signal()?;
        if s.clen != "Response" || s.cesta != cesta {
            continue;
        }
        let (Some(Hodnota::Cislo(kod)), Some(Hodnota::Pole(vysledky))) = (s.telo.first(), s.telo.get(1))
        else {
            return Err(chyba("neplatná odpoveď dialógu"));
        };
        if *kod != 0 {
            return Ok(None);
        }
        for v in vysledky {
            if let Hodnota::Struktura(kv) = v
                && let [k, Hodnota::Variant(h)] = &kv[..]
                && k.text() == Some("uris")
                && let Hodnota::Pole(uri) = &**h
                && let Some(u) = uri.first().and_then(Hodnota::text)
            {
                return Ok(super::cesty_z_uri(u).into_iter().next());
            }
        }
        return Ok(None);
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    /// Zdanlivá zbernica: overenie, Hello, AddMatch, OpenFile a signál Response so zvoleným súborom.
    #[test]
    fn dialog_cez_zdanlivu_zbernicu() {
        let (klient, server) = UnixStream::pair().unwrap();
        let s = std::thread::spawn(move || {
            let mut w = server.try_clone().unwrap();
            let mut r = BufReader::new(server);
            let mut nula = [0u8; 1];
            r.read_exact(&mut nula).unwrap();
            let mut riadok = String::new();
            r.read_line(&mut riadok).unwrap();
            assert!(riadok.starts_with("AUTH EXTERNAL "), "{riadok}");
            w.write_all(b"OK 1234deadbeef\r\n").unwrap();
            riadok.clear();
            r.read_line(&mut riadok).unwrap();
            assert_eq!(riadok, "BEGIN\r\n");
            let odpovedz = |w: &mut UnixStream, serial: u32, podpis: &str, telo: &[u8]| {
                let mut h = Zapis::default();
                h.b.extend_from_slice(&[b'l', 2, 0, 1]);
                h.u(telo.len() as u32);
                h.u(900 + serial);
                h.pole(8, |h| {
                    h.zarovnaj(8);
                    h.y(5);
                    h.g("u");
                    h.u(serial);
                    h.zarovnaj(8);
                    h.y(8);
                    h.g("g");
                    h.g(podpis);
                });
                h.zarovnaj(8);
                h.b.extend_from_slice(telo);
                w.write_all(&h.b).unwrap();
            };
            let mut volania = Vec::new();
            for serial in 1..=3 {
                let m = citaj_spravu(&mut r).unwrap();
                volania.push((m.clen.clone(), m.telo.clone()));
                let mut t = Zapis::default();
                match m.clen.as_str() {
                    "Hello" => {
                        t.s(":1.42");
                        odpovedz(&mut w, serial, "s", &t.b);
                    }
                    "AddMatch" => odpovedz(&mut w, serial, "", &[]),
                    "OpenFile" => {
                        t.s("/org/freedesktop/portal/desktop/request/1_42/x");
                        odpovedz(&mut w, serial, "o", &t.b);
                    }
                    _ => panic!("neočakávané volanie {}", m.clen),
                }
            }
            // signál Response: (0, {uris: [..], choices: [] })
            let mut t = Zapis::default();
            t.u(0);
            t.pole(8, |t| {
                t.polozka("choices", "a(ss)", |t| t.pole(8, |_| {}));
                t.polozka("uris", "as", |t| t.pole(4, |t| t.s("file:///tmp/k%C3%BAp%20a.jpg")));
            });
            let mut h = Zapis::default();
            h.b.extend_from_slice(&[b'l', 4, 0, 1]);
            h.u(t.b.len() as u32);
            h.u(999);
            let cesta =
                format!("/org/freedesktop/portal/desktop/request/1_42/orchidea{}", std::process::id());
            h.pole(8, |h| {
                for (kod, typ, hodnota) in [
                    (1u8, "o", cesta.as_str()),
                    (2, "s", "org.freedesktop.portal.Request"),
                    (3, "s", "Response"),
                ] {
                    h.zarovnaj(8);
                    h.y(kod);
                    h.g(typ);
                    h.s(hodnota);
                }
                h.zarovnaj(8);
                h.y(8);
                h.g("g");
                h.g("ua{sv}");
            });
            h.zarovnaj(8);
            h.b.extend_from_slice(&t.b);
            w.write_all(&h.b).unwrap();
            volania
        });
        let b = Zbernica::na(klient).unwrap();
        assert_eq!(b.meno, ":1.42");
        let v = vyber(b, "Otvoriť", Some(Path::new("/home/a")), false).unwrap();
        assert_eq!(v, Some(PathBuf::from("/tmp/kúp a.jpg")));
        let volania = s.join().unwrap();
        // voľby OpenFile: titulok, handle_token, priečinok ako pole bajtov s nulou
        let (meno, telo) = &volania[2];
        assert_eq!(meno, "OpenFile");
        assert_eq!(telo[1], Hodnota::Text("Otvoriť".into()));
        let Hodnota::Pole(volby) = &telo[2] else { panic!() };
        let kluce: Vec<&str> = volby
            .iter()
            .filter_map(|v| match v {
                Hodnota::Struktura(kv) => kv[0].text(),
                _ => None,
            })
            .collect();
        assert_eq!(kluce, ["handle_token", "modal", "filters", "current_folder"]);
    }

    /// Skutočná zbernica relácie (nezávislý rozhodca kódovania): `ORCHIDEA_ZBERNICA=1 dbus-run-session -- cargo test …`.
    #[test]
    fn skutocna_zbernica() {
        if std::env::var_os("ORCHIDEA_ZBERNICA").is_none() {
            return;
        }
        let mut b = Zbernica::relacie().expect("zbernica relácie");
        assert!(b.meno.starts_with(':'), "{}", b.meno);
        // pole slovníka a{ss} a reťazec – démon odmietne zle zakódovanú správu
        let mut t = Zapis::default();
        t.pole(8, |t| {
            t.zarovnaj(8);
            t.s("ORCHIDEA_SKUSKA");
            t.s("ano");
        });
        b.volaj(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "UpdateActivationEnvironment",
            "a{ss}",
            &t.b,
        )
        .unwrap();
        let mut t = Zapis::default();
        t.s("type='signal',interface='org.freedesktop.portal.Request',member='Response'");
        b.volaj(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "AddMatch",
            "s",
            &t.b,
        )
        .unwrap();
        // neexistujúca metóda = chybová odpoveď, nie pád
        assert!(
            b.volaj(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "Nejestvuje",
                "",
                &[]
            )
            .is_err()
        );
        println!("portál k dispozícii: {}", je_k_dispozicii());
    }

    #[test]
    fn adresy_a_poskodene_spravy() {
        assert_eq!(odescapuj("%2ftmp%2Fa%20b"), "/tmp/a b");
        assert_eq!(odescapuj("x%2"), "x%2");
        assert!(pripoj_adresu("tcp:host=x").is_err());
        assert!(pripoj_adresu("unix:path=/neexistuje/zbernica").is_err());
        // skrátená a nezmyselná správa nespadne
        for d in [
            &b"l\x02\x00\x01"[..],
            b"B\x01\x00\x01\0\0\0\0\0\0\0\0\0\0\0\0",
            &[b'l', 2, 0, 1, 255, 255, 255, 255, 1, 0, 0, 0, 8, 0, 0, 0],
        ] {
            assert!(citaj_spravu(&mut &d[..]).is_err());
        }
        let mut c = Citanie { d: &[1, 0, 0, 0], i: 0 };
        assert!(c.hodnota(b"(uu", 0).is_err());
        let mut c = Citanie { d: &[200, 0, 0, 0], i: 0 };
        assert!(c.hodnota(b"a{sv}", 0).is_err());
    }
}
