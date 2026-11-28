//! Presný čas zo siete – klient SNTP.
//!
//! Podľa **RFC 4330** (SNTPv4: formát – kap. 4, klient – kap. 5, kontrola odpovede – kap. 5 body 1–4 a „Kiss-o'-Death“
//! – kap. 8) a **RFC 5905** (časová značka NTP a éry – kap. 6, výpočet posunu a oneskorenia – kap. 8).
//! Krabičky Raspberry Pi a TV krabičky nemajú hodiny s batériou – po štarte začínajú v roku 1970, preto čas zo
//! siete potrebujú záznamy aj (neskôr) overovanie certifikátov.
//!
//! Do poľa „čas odoslania“ klient nedáva svoj čas, ale **náhodné číslo** (ochrana súkromia a proti podvrhu – server
//! ho musí vrátiť v poli „čas pôvodu“); vlastné časy odoslania a prijatia si drží volajúci.

use std::time::Duration;

/// Port servera NTP.
pub const PORT: u16 = 123;
/// Dĺžka správy bez rozšírení.
pub const DLZKA: usize = 48;
/// Sekundy medzi 1. 1. 1900 (NTP) a 1. 1. 1970 (Unix).
const POSUN_1970: u64 = 2_208_988_800;
/// Najväčšie oneskorenie cesty, pri ktorom sa odpoveď ešte prijme.
const NAJVACSIE_ONESKORENIE: Duration = Duration::from_secs(4);
/// Najväčšia nepresnosť servera (root dispersion) – RFC 5905 MAXDISP 16 s.
const NAJVACSIA_NEPRESNOST_MS: u64 = 16_000;

/// Časová značka NTP (64 bitov: sekundy od 1900 a zlomok) z času Unix; éra 1 (od 7. 2. 2036) sa zalomí.
pub fn z_unix(t: Duration) -> u64 {
    let sek = (t.as_secs() + POSUN_1970) & 0xFFFF_FFFF;
    let zlomok = ((t.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (sek << 32) | zlomok
}

/// Čas Unix z časovej značky NTP. Éru určuje najvyšší bit sekúnd (RFC 4330 kap. 3): 1 = éra 0 (1968–2036),
/// 0 = éra 1 (2036–2104). Značka 0 nie je čas (server nie je synchronizovaný).
pub fn na_unix(ts: u64) -> Option<Duration> {
    if ts == 0 {
        return None;
    }
    let sek = ts >> 32;
    let zlomok = ts & 0xFFFF_FFFF;
    let unix =
        if sek & 0x8000_0000 != 0 { sek.checked_sub(POSUN_1970)? } else { sek + (1 << 32) - POSUN_1970 };
    let nanos = (zlomok * 1_000_000_000 + (1 << 31)) >> 32;
    Some(Duration::new(unix, 0) + Duration::from_nanos(nanos.min(999_999_999)))
}

/// Žiadosť klienta: LI 0, verzia 4, režim 3 (klient), „čas odoslania“ = `znacka` (náhodné číslo).
pub fn ziadost(znacka: u64) -> [u8; DLZKA] {
    let mut d = [0u8; DLZKA];
    d[0] = (4 << 3) | 3;
    d[40..48].copy_from_slice(&znacka.to_be_bytes());
    d
}

/// Prečo sa odpoveď neprijala.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chyba {
    PrilisKratka(usize),
    /// iný režim než 4 (server) alebo nepodporovaná verzia
    ZlyRezim {
        verzia: u8,
        rezim: u8,
    },
    /// server sám nemá čas (LI = 3, stratum > 15 alebo nulové časové značky)
    Nesynchronizovany,
    /// „Kiss-o'-Death“ (stratum 0): server žiada prestať alebo spomaliť – kód (napr. RATE, DENY)
    Bozk(String),
    /// „čas pôvodu“ nie je naše náhodné číslo – odpoveď na inú otázku alebo podvrh
    CudziaOdpoved,
    /// nezmyselné časy (server prijal po odoslaní, záporné alebo priveľké oneskorenie, veľká nepresnosť)
    ZleCasy(&'static str),
}

impl std::fmt::Display for Chyba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Chyba::PrilisKratka(n) => write!(f, "odpoveď NTP má {n} B (treba 48)"),
            Chyba::ZlyRezim { verzia, rezim } => {
                write!(f, "nie je odpoveď servera (verzia {verzia}, režim {rezim})")
            }
            Chyba::Nesynchronizovany => write!(f, "server nemá platný čas"),
            Chyba::Bozk(k) => write!(f, "server odmietol („{k}“)"),
            Chyba::CudziaOdpoved => write!(f, "odpoveď nepatrí k našej žiadosti"),
            Chyba::ZleCasy(c) => write!(f, "nezmyselné časy ({c})"),
        }
    }
}

impl std::error::Error for Chyba {}

/// Výsledok jednej výmeny.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vysledok {
    /// o koľko treba posunúť miestne hodiny (kladné = dopredu), v nanosekundách – θ z RFC 5905 kap. 8
    pub posun_ns: i128,
    /// oneskorenie cesty tam a späť bez času spracovania na serveri – δ
    pub oneskorenie: Duration,
    pub stratum: u8,
}

impl Vysledok {
    /// Presný čas Unix v okamihu prijatia odpovede (`prijate` = miestny čas prijatia).
    pub fn cas_pri_prijati(&self, prijate: Duration) -> Duration {
        let ns = prijate.as_nanos() as i128 + self.posun_ns;
        Duration::from_nanos(ns.clamp(0, u64::MAX as i128) as u64)
    }
}

/// Spracuje odpoveď. `znacka` = náhodné číslo zo žiadosti, `odoslane` a `prijate` = miestne časy Unix (T1, T4).
pub fn spracuj(d: &[u8], znacka: u64, odoslane: Duration, prijate: Duration) -> Result<Vysledok, Chyba> {
    if d.len() < DLZKA {
        return Err(Chyba::PrilisKratka(d.len()));
    }
    let li = d[0] >> 6;
    let verzia = (d[0] >> 3) & 7;
    let rezim = d[0] & 7;
    if rezim != 4 || !(3..=4).contains(&verzia) {
        return Err(Chyba::ZlyRezim { verzia, rezim });
    }
    let stratum = d[1];
    let u32_ = |i: usize| u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    let ts = |i: usize| {
        u64::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3], d[i + 4], d[i + 5], d[i + 6], d[i + 7]])
    };
    if stratum == 0 {
        let kod: String = d[12..16].iter().filter(|b| b.is_ascii_graphic()).map(|&b| b as char).collect();
        return Err(Chyba::Bozk(kod));
    }
    if ts(24) != znacka {
        return Err(Chyba::CudziaOdpoved);
    }
    if li == 3 || stratum > 15 {
        return Err(Chyba::Nesynchronizovany);
    }
    let (Some(t2), Some(t3)) = (na_unix(ts(32)), na_unix(ts(40))) else {
        return Err(Chyba::Nesynchronizovany);
    };
    if t3 < t2 {
        return Err(Chyba::ZleCasy("server odoslal skôr, ako prijal"));
    }
    // root dispersion – 16.16 sekúnd
    if (u32_(8) as u64 * 1000) >> 16 > NAJVACSIA_NEPRESNOST_MS {
        return Err(Chyba::ZleCasy("server má veľkú nepresnosť"));
    }
    let n = |t: Duration| t.as_nanos() as i128;
    let (t1, t2, t3, t4) = (n(odoslane), n(t2), n(t3), n(prijate));
    let posun_ns = ((t2 - t1) + (t3 - t4)) / 2;
    let oneskorenie = (t4 - t1) - (t3 - t2);
    if oneskorenie < -1_000_000 {
        return Err(Chyba::ZleCasy("záporné oneskorenie"));
    }
    let oneskorenie = Duration::from_nanos(oneskorenie.max(0) as u64);
    if oneskorenie > NAJVACSIE_ONESKORENIE {
        return Err(Chyba::ZleCasy("priveľké oneskorenie"));
    }
    Ok(Vysledok { posun_ns, oneskorenie, stratum })
}

#[cfg(test)]
mod testy {
    use super::*;

    fn odpoved(znacka: u64, t2: Duration, t3: Duration, stratum: u8, li: u8) -> [u8; DLZKA] {
        let mut d = [0u8; DLZKA];
        d[0] = (li << 6) | (4 << 3) | 4;
        d[1] = stratum;
        d[3] = 0xEC; // presnosť 2^-20
        d[12..16].copy_from_slice(b"GPS\0");
        d[24..32].copy_from_slice(&znacka.to_be_bytes());
        d[32..40].copy_from_slice(&z_unix(t2).to_be_bytes());
        d[40..48].copy_from_slice(&z_unix(t3).to_be_bytes());
        d
    }

    #[test]
    fn casove_znacky_a_ery() {
        assert_eq!(z_unix(Duration::ZERO), 0x83AA_7E80_0000_0000, "1. 1. 1970 = 2 208 988 800 s od 1900");
        assert_eq!(na_unix(0x83AA_7E80_0000_0000), Some(Duration::ZERO));
        assert_eq!(z_unix(Duration::from_millis(500)) & 0xFFFF_FFFF, 0x8000_0000, "pol sekundy");
        // 28. 11. 2026 09:00:00 UTC
        let dnes = Duration::from_secs(1_795_856_400) + Duration::from_nanos(123_456_789);
        let spat = na_unix(z_unix(dnes)).unwrap();
        assert!(spat.abs_diff(dnes) < Duration::from_nanos(2), "{spat:?}");
        // 7. 2. 2036 06:28:16 UTC – sekundy NTP pretečú na 0 (éra 1)
        let prelom = Duration::from_secs((1u64 << 32) - POSUN_1970);
        assert_eq!(z_unix(prelom) >> 32, 0);
        assert_eq!(na_unix(z_unix(prelom + Duration::from_secs(5))), Some(prelom + Duration::from_secs(5)));
        assert_eq!(na_unix(0), None);
    }

    #[test]
    fn ziadost_bajt_po_bajte() {
        let z = ziadost(0x0123_4567_89AB_CDEF);
        assert_eq!(z[0], 0b00_100_011, "LI 0, verzia 4, režim 3");
        assert!(z[1..40].iter().all(|&b| b == 0), "klient nič o sebe neprezradí");
        assert_eq!(&z[40..], &[0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]);
    }

    #[test]
    fn posun_a_oneskorenie() {
        // hodiny krabičky sú v roku 1970 (bez batérie): odoslané 10 s, prijaté 10,040 s;
        // server prijal 1 795 856 400,015 a odoslal 1 795 856 400,017 (2 ms spracovanie)
        let skutocny = Duration::from_secs(1_795_856_400);
        let t1 = Duration::from_secs(10);
        let t4 = Duration::from_millis(10_040);
        let t2 = skutocny + Duration::from_millis(15);
        let t3 = skutocny + Duration::from_millis(17);
        let v = spracuj(&odpoved(77, t2, t3, 2, 0), 77, t1, t4).unwrap();
        assert_eq!(v.oneskorenie, Duration::from_millis(38));
        assert_eq!(v.stratum, 2);
        // čas pri prijatí = T3 + polovica cesty späť (cesta symetrická: 19 ms tam aj späť)
        let cas = v.cas_pri_prijati(t4);
        assert!(cas.abs_diff(skutocny + Duration::from_millis(36)) < Duration::from_micros(1), "{cas:?}");
    }

    #[test]
    fn odmietnute_odpovede() {
        let t = Duration::from_secs(1_795_856_400);
        let (t1, t4) = (Duration::from_secs(1), Duration::from_secs(1) + Duration::from_millis(20));
        assert_eq!(spracuj(&[0; 20], 1, t1, t4), Err(Chyba::PrilisKratka(20)));
        assert_eq!(spracuj(&odpoved(1, t, t, 2, 0), 2, t1, t4), Err(Chyba::CudziaOdpoved));
        assert_eq!(spracuj(&odpoved(1, t, t, 2, 3), 1, t1, t4), Err(Chyba::Nesynchronizovany), "LI 3");
        assert_eq!(spracuj(&odpoved(1, t, t, 16, 0), 1, t1, t4), Err(Chyba::Nesynchronizovany));
        let mut bozk = odpoved(1, t, t, 0, 3);
        bozk[12..16].copy_from_slice(b"RATE");
        assert_eq!(spracuj(&bozk, 1, t1, t4), Err(Chyba::Bozk("RATE".into())));
        let mut klient = odpoved(1, t, t, 2, 0);
        klient[0] = (4 << 3) | 3;
        assert_eq!(spracuj(&klient, 1, t1, t4), Err(Chyba::ZlyRezim { verzia: 4, rezim: 3 }));
        assert!(matches!(
            spracuj(&odpoved(1, t + Duration::from_secs(1), t, 2, 0), 1, t1, t4),
            Err(Chyba::ZleCasy(_))
        ));
        assert!(matches!(
            spracuj(&odpoved(1, t, t, 2, 0), 1, t1, t1 + Duration::from_secs(10)),
            Err(Chyba::ZleCasy("priveľké oneskorenie"))
        ));
        let mut nepresny = odpoved(1, t, t, 2, 0);
        nepresny[8..12].copy_from_slice(&(20u32 << 16).to_be_bytes());
        assert!(matches!(spracuj(&nepresny, 1, t1, t4), Err(Chyba::ZleCasy(_))));
        let mut nulovy = odpoved(1, t, t, 2, 0);
        nulovy[40..48].fill(0);
        assert_eq!(spracuj(&nulovy, 1, t1, t4), Err(Chyba::Nesynchronizovany));
    }
}
