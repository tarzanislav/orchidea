//! Zberač meraní (LOVEC ZBERAČ, R247) – krabička pošle výsledky merania počítaču v miestnej sieti.
//!
//! Vlastník 27. 11. 2026: „všetky krabičky sú pripojené do lokálnej siete … keď program u krabičky dobehne do
//! konca, krabička vyšle signál … LOVEC ZBERAČ prevezme signál a aj konkrétny súbor, v ktorom sú výsledky“.
//!
//! **Hľadanie bez nastavovania siete:** krabička pošle správu [`HLADAM`] na adresu všetkých uzlov miestnej
//! linky IPv6 `ff02::1` (port [`PORT_HLADANIA`]); zberač odpovie [`SOM`] s portom prenosu. Adresy miestnej
//! linky (`fe80::/10`) si jadro aj Windows nastavia samy – netreba router, DHCP ani zadávať IP adresu.
//!
//! **Prenos:** spojenie TCP na [`PORT_PRENOSU`], rámce zašifrované s overením pravosti
//! AEAD_CHACHA20_POLY1305 (RFC 8439) kľúčom domácnosti – 32 náhodných bajtov, ktoré vytvorí ORCHIDEA Zapisovač
//! a zapíše na každú kartu (`zberac=` v SKLENIK.TXT). Krabička bez kľúča nič neposiela; zberač bez kľúča nič
//! neprečíta. Odpoveď zberača je zašifrovaná odvodeným kľúčom ([`kluc_odpovede`]) s nonce požiadavky – žiadna
//! dvojica (kľúč, nonce) sa nepoužije dvakrát.
//!
//! Rámec: `OZB1` · nonce (12 B) · dĺžka šifry (u32 LE) · šifra · značka (16 B); doplnok chránený značkou =
//! `OZB1` + dĺžka. Obsah rámca je [`Sprava`]: riadky `kľúč=hodnota`, prázdny riadok a bajty obsahu.

use std::io::{self, Read};
use std::path::PathBuf;

/// Port UDP, na ktorom zberač počúva hľadanie.
pub const PORT_HLADANIA: u16 = 47_610;
/// Port TCP prenosu výsledkov.
pub const PORT_PRENOSU: u16 = 47_611;
/// Hľadanie zberača (krabička → všetci v miestnej linke). Za ním medzera a identita krabičky.
pub const HLADAM: &str = "ORCHIDEA-ZBERAC-HLADAM 1";
/// Odpoveď zberača; za ňou medzera a port prenosu.
pub const SOM: &str = "ORCHIDEA-ZBERAC-SOM 1";
/// Začiatok každého rámca.
pub const ZNACKA: &[u8; 4] = b"OZB1";
/// Dĺžka hlavičky rámca (značka, nonce, dĺžka).
pub const HLAVICKA_B: usize = 20;
/// Najväčšia správa (záznam merania má desiatky kB).
pub const NAJVIAC_B: usize = 8 << 20;

/// Kľúč odpovedí zberača – odvodený z kľúča domácnosti prvým blokom ChaCha20 s pevným nonce, aby odpoveď
/// s nonce požiadavky nepoužila rovnakú dvojicu (kľúč, nonce) ako požiadavka.
pub fn kluc_odpovede(kluc: &[u8; 32]) -> [u8; 32] {
    let b = crate::chacha20_blok(kluc, 0, b"OZB1-odpoved");
    let mut k = [0u8; 32];
    k.copy_from_slice(&b[..32]);
    k
}

fn doplnok(dlzka: u32) -> [u8; 8] {
    let mut d = [0u8; 8];
    d[..4].copy_from_slice(ZNACKA);
    d[4..].copy_from_slice(&dlzka.to_le_bytes());
    d
}

/// Zabalí `data` do rámca: zašifruje ich kľúčom `kluc` s `nonce` (každý rámec nové, náhodné nonce).
pub fn zabal(kluc: &[u8; 32], nonce: &[u8; 12], data: &[u8]) -> Vec<u8> {
    let dlzka = data.len() as u32;
    let mut sifra = data.to_vec();
    let znacka = crate::zasifruj(kluc, nonce, &doplnok(dlzka), &mut sifra);
    let mut v = Vec::with_capacity(HLAVICKA_B + sifra.len() + 16);
    v.extend_from_slice(ZNACKA);
    v.extend_from_slice(nonce);
    v.extend_from_slice(&dlzka.to_le_bytes());
    v.extend_from_slice(&sifra);
    v.extend_from_slice(&znacka);
    v
}

/// Prečíta hlavičku rámca: nonce a dĺžku šifry.
pub fn hlavicka(h: &[u8; HLAVICKA_B]) -> Result<([u8; 12], usize), String> {
    if &h[..4] != ZNACKA {
        return Err("nie je to rámec zberača ORCHIDEA".into());
    }
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&h[4..16]);
    let dlzka = u32::from_le_bytes([h[16], h[17], h[18], h[19]]) as usize;
    if dlzka > NAJVIAC_B {
        return Err(format!("rámec je príliš veľký ({dlzka} B)"));
    }
    Ok((nonce, dlzka))
}

/// Overí a rozbalí rámec (hlavička + šifra + značka). Pri inom kľúči alebo zmenených údajoch chyba.
pub fn rozbal(kluc: &[u8; 32], ramec: &[u8]) -> Result<([u8; 12], Vec<u8>), String> {
    if ramec.len() < HLAVICKA_B + 16 {
        return Err("rámec je kratší ako hlavička".into());
    }
    let mut h = [0u8; HLAVICKA_B];
    h.copy_from_slice(&ramec[..HLAVICKA_B]);
    let (nonce, dlzka) = hlavicka(&h)?;
    if ramec.len() != HLAVICKA_B + dlzka + 16 {
        return Err("dĺžka rámca nesedí".into());
    }
    let mut data = ramec[HLAVICKA_B..HLAVICKA_B + dlzka].to_vec();
    let mut znacka = [0u8; 16];
    znacka.copy_from_slice(&ramec[HLAVICKA_B + dlzka..]);
    crate::desifruj(kluc, &nonce, &doplnok(dlzka as u32), &mut data, &znacka)
        .map_err(|_| "neplatná značka – iný kľúč domácnosti alebo zmenené údaje".to_string())?;
    Ok((nonce, data))
}

/// Prečíta jeden celý rámec z prúdu (spojenie TCP).
pub fn citaj_ramec(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut h = [0u8; HLAVICKA_B];
    r.read_exact(&mut h)?;
    let (_, dlzka) = hlavicka(&h).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut v = h.to_vec();
    v.resize(HLAVICKA_B + dlzka + 16, 0);
    r.read_exact(&mut v[HLAVICKA_B..])?;
    Ok(v)
}

/// Odpoveď zberača na požiadavku s `nonce` (kľúč odpovedí, rovnaké nonce).
pub fn odpoved(kluc: &[u8; 32], nonce_poziadavky: &[u8; 12], text: &str) -> Vec<u8> {
    zabal(&kluc_odpovede(kluc), nonce_poziadavky, text.as_bytes())
}

/// Overí odpoveď zberača na požiadavku s `nonce` a vráti jej text.
pub fn over_odpoved(kluc: &[u8; 32], nonce_poziadavky: &[u8; 12], ramec: &[u8]) -> Result<String, String> {
    let (nonce, data) = rozbal(&kluc_odpovede(kluc), ramec)?;
    if &nonce != nonce_poziadavky {
        return Err("odpoveď patrí inej požiadavke".into());
    }
    String::from_utf8(data).map_err(|_| "odpoveď nie je text".into())
}

/// Správa v rámci: polia `kľúč=hodnota` (UTF-8, bez nových riadkov) a obsah (bajty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sprava {
    pub polia: Vec<(String, String)>,
    pub obsah: Vec<u8>,
}

impl Sprava {
    pub fn nova() -> Sprava {
        Sprava::default()
    }

    /// Pridá pole (nové riadky a „=“ v kľúči sa nahradia medzerou).
    pub fn s(mut self, kluc: &str, hodnota: &str) -> Sprava {
        let cisty = |t: &str| t.replace(['\n', '\r'], " ");
        self.polia.push((cisty(kluc).replace('=', " "), cisty(hodnota)));
        self
    }

    pub fn s_obsahom(mut self, obsah: Vec<u8>) -> Sprava {
        self.obsah = obsah;
        self
    }

    pub fn pole(&self, kluc: &str) -> Option<&str> {
        self.polia.iter().find(|(k, _)| k == kluc).map(|(_, v)| v.as_str())
    }

    pub fn do_bajtov(&self) -> Vec<u8> {
        let mut v = Vec::new();
        for (k, h) in &self.polia {
            v.extend_from_slice(format!("{k}={h}\n").as_bytes());
        }
        v.push(b'\n');
        v.extend_from_slice(&self.obsah);
        v
    }

    pub fn z_bajtov(b: &[u8]) -> Result<Sprava, String> {
        let koniec = b.windows(2).position(|w| w == b"\n\n").ok_or("správa bez konca hlavičky")?;
        let hlavicka = std::str::from_utf8(&b[..koniec]).map_err(|_| "hlavička správy nie je UTF-8")?;
        let mut s = Sprava::nova();
        for r in hlavicka.lines() {
            let (k, h) = r.split_once('=').ok_or_else(|| format!("pole bez „=“: {r}"))?;
            s.polia.push((k.to_string(), h.to_string()));
        }
        s.obsah = b[koniec + 2..].to_vec();
        Ok(s)
    }
}

/// Kľúč z 64 šestnástkových znakov.
pub fn kluc_z_textu(t: &str) -> Option<[u8; 32]> {
    let t = t.trim();
    if t.len() != 64 || !t.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&t[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(k)
}

/// Kľúč ako 64 šestnástkových znakov.
pub fn kluc_do_textu(k: &[u8; 32]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

/// Priečinok nastavenia ORCHIDEA na počítači: `%APPDATA%\ORCHIDEA` (Windows), inak `~/.config/orchidea`.
/// Premenná `ORCHIDEA_NASTAVENIE` ho prepíše (skúšky).
pub fn priecinok_nastavenia() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ORCHIDEA_NASTAVENIE") {
        return Some(PathBuf::from(p));
    }
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("ORCHIDEA"))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("orchidea"))
    }
}

/// Súbor s kľúčom domácnosti.
pub fn cesta_kluca() -> Option<PathBuf> {
    priecinok_nastavenia().map(|p| p.join("zberac-kluc.txt"))
}

/// Kľúč domácnosti, ak už vznikol.
pub fn nacitaj_kluc() -> Option<[u8; 32]> {
    let t = std::fs::read_to_string(cesta_kluca()?).ok()?;
    t.lines().find_map(kluc_z_textu)
}

/// Uloží kľúč domácnosti, ak ešte žiadny nie je (nikdy neprepíše existujúci). Vráti platný kľúč – nový,
/// alebo ten, ktorý už bol uložený (napríklad ho medzitým vytvorila iná inštancia).
pub fn uloz_kluc_ak_chyba(novy: &[u8; 32]) -> io::Result<[u8; 32]> {
    if let Some(k) = nacitaj_kluc() {
        return Ok(k);
    }
    let cesta = cesta_kluca().ok_or_else(|| io::Error::other("priečinok nastavenia nie je známy"))?;
    if let Some(p) = cesta.parent() {
        std::fs::create_dir_all(p)?;
    }
    let text = format!(
        "# ORCHIDEA – kľúč domácnosti pre zberač meraní (R247). Nemeniť – karty zapísané s týmto kľúčom by\n\
         # potom výsledky neposlali. Zapisovač ho dáva na každú kartu.\n{}\n",
        kluc_do_textu(novy)
    );
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&cesta) {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            Ok(*novy)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            nacitaj_kluc().ok_or_else(|| io::Error::other("súbor kľúča existuje, ale je neplatný"))
        }
        Err(e) => Err(e),
    }
}

/// Pošle hľadanie na adresu `ciel` (krabička: `[ff02::1%rozhranie]:PORT_HLADANIA`) a vráti adresu prenosu
/// zberača, ktorý prvý odpovie do `cakaj`. Odpoveď nesie port prenosu; adresa je odosielateľ odpovede (pri
/// adrese miestnej linky aj s rozhraním).
pub fn najdi(
    ciel: std::net::SocketAddr,
    identita: &str,
    cakaj: std::time::Duration,
) -> io::Result<std::net::SocketAddr> {
    use std::net::{SocketAddr, UdpSocket};
    let s = if ciel.is_ipv6() { UdpSocket::bind("[::]:0")? } else { UdpSocket::bind("0.0.0.0:0")? };
    // ff02::1 je adresa miestnej linky – smerovač ju ďalej nepustí, správa ostane v domácej sieti
    s.set_read_timeout(Some(cakaj))?;
    s.send_to(format!("{HLADAM} {identita}").as_bytes(), ciel)?;
    let koniec = std::time::Instant::now() + cakaj;
    let mut b = [0u8; 256];
    loop {
        let (n, od) = s.recv_from(&mut b)?;
        if let Ok(t) = std::str::from_utf8(&b[..n])
            && let Some(port) = t.strip_prefix(SOM).and_then(|z| z.trim().parse::<u16>().ok())
        {
            let mut a = od;
            a.set_port(port);
            if let (SocketAddr::V6(a6), SocketAddr::V6(c6)) = (&mut a, ciel)
                && a6.scope_id() == 0
            {
                a6.set_scope_id(c6.scope_id());
            }
            return Ok(a);
        }
        if std::time::Instant::now() >= koniec {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "zberač neodpovedal"));
        }
    }
}

/// Pošle správu zberaču na `adresa` a počká na jeho overenú odpoveď. `nonce` musí byť pre každý rámec nové
/// (náhodné). Vráti text odpovede („ok …“).
pub fn posli(
    adresa: std::net::SocketAddr,
    kluc: &[u8; 32],
    nonce: &[u8; 12],
    sprava: &Sprava,
    cakaj: std::time::Duration,
) -> Result<String, String> {
    use std::io::Write;
    let mut t = std::net::TcpStream::connect_timeout(&adresa, cakaj).map_err(|e| format!("spojenie: {e}"))?;
    let _ = t.set_read_timeout(Some(cakaj));
    let _ = t.set_write_timeout(Some(cakaj));
    t.write_all(&zabal(kluc, nonce, &sprava.do_bajtov())).map_err(|e| format!("odoslanie: {e}"))?;
    let r = citaj_ramec(&mut t).map_err(|e| format!("odpoveď: {e}"))?;
    over_odpoved(kluc, nonce, &r)
}

#[cfg(test)]
mod skusky {
    use super::*;

    const K: [u8; 32] = [7; 32];

    /// Dočasný priečinok skúšky – vo WebAssembly (WASI) v pracovnom priečinku (temp_dir a process::id tam nie sú).
    fn docasny(meno: &str) -> std::path::PathBuf {
        let zaklad =
            if cfg!(target_family = "wasm") { std::path::PathBuf::from(".") } else { std::env::temp_dir() };
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d = zaklad.join(format!("orchidea-{meno}-{n}"));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn ramec_tam_a_spat() {
        let s = Sprava::nova()
            .s("zariadenie", "Hardkernel ODROID-C4")
            .s("subor", "01-x.txt")
            .s_obsahom(b"ahoj\n\nsvet".to_vec());
        let r = zabal(&K, &[1; 12], &s.do_bajtov());
        let (n, d) = rozbal(&K, &r).unwrap();
        assert_eq!(n, [1; 12]);
        let s2 = Sprava::z_bajtov(&d).unwrap();
        assert_eq!(s2, s);
        assert_eq!(s2.pole("subor"), Some("01-x.txt"));
        assert_eq!(citaj_ramec(&mut &r[..]).unwrap(), r);
    }

    #[test]
    fn iny_kluc_alebo_zmena_sa_odmietne() {
        let r = zabal(&K, &[2; 12], b"data");
        assert!(rozbal(&[8; 32], &r).is_err(), "iný kľúč");
        let mut z = r.clone();
        z[HLAVICKA_B] ^= 1;
        assert!(rozbal(&K, &z).is_err(), "zmenená šifra");
        let mut z = r.clone();
        z[16] ^= 1;
        assert!(rozbal(&K, &z).is_err(), "zmenená dĺžka");
        assert!(rozbal(&K, &r[..10]).is_err());
    }

    #[test]
    fn odpoved_iny_kluc_rovnake_nonce() {
        let n = [3; 12];
        let o = odpoved(&K, &n, "ok 120");
        assert_eq!(over_odpoved(&K, &n, &o).unwrap(), "ok 120");
        assert!(over_odpoved(&K, &[4; 12], &o).is_err(), "iná požiadavka");
        // odpoveď sa nedá zameniť za požiadavku (iný kľúč)
        assert!(rozbal(&K, &o).is_err());
        assert_ne!(kluc_odpovede(&K), K);
    }

    #[test]
    fn kluc_text_a_subor() {
        let k: [u8; 32] = core::array::from_fn(|i| i as u8 * 7);
        assert_eq!(kluc_z_textu(&kluc_do_textu(&k)), Some(k));
        assert_eq!(kluc_z_textu("abc"), None);
        let d = docasny("zber");
        nastav_priecinok(&d);
        assert_eq!(nacitaj_kluc(), None);
        assert_eq!(uloz_kluc_ak_chyba(&k).unwrap(), k);
        assert_eq!(uloz_kluc_ak_chyba(&[9; 32]).unwrap(), k, "existujúci sa neprepíše");
        assert_eq!(nacitaj_kluc(), Some(k));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn nastav_priecinok(d: &std::path::Path) {
        #[allow(unsafe_code)]
        // SAFETY: premennú ORCHIDEA_NASTAVENIE číta v tomto balíku len táto skúška.
        unsafe {
            std::env::set_var("ORCHIDEA_NASTAVENIE", d)
        };
    }
}
