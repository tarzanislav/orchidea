//! R247: záznamy merania pre zberač meraní (LOVEC ZBERAČ) na počítači vlastníka v miestnej sieti.
//!
//! Po každom zapísanom zázname a pri HOTOVÉ pošle krabička text záznamu zberaču. Zberača nájde sama (adresy
//! IPv6 miestnej linky, `ff02::1` – bez routera a DHCP), prenos je zašifrovaný kľúčom domácnosti z karty
//! (`zberac=` v SKLENIK.TXT). Všetko beží vo vlastnom vlákne s najnižšou prioritou – hlavná slučka nikdy
//! nečaká na sieť (R215). Chladný systém: sieť sa zobudí len vtedy, keď je čo poslať ([`treba_siet`]); keď
//! zberač neodpovedá, krabička to skúsi znova o niekoľko minút a sieť medzitým spí.

use sklenik_korene::zber::{self, Sprava};
use std::collections::VecDeque;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Najviac správ v rade (pri dlhom výpadku zberača sa najstaršie zahodia – na karte ostanú).
const NAJVIAC_V_RADE: usize = 64;
/// Ako dlho sa čaká na sieť (zobudenie čipu siete, dohodnutie spojenia, adresa miestnej linky).
const CAKANIE_NA_SIET: Duration = Duration::from_secs(30);

/// Kto posiela – prvé riadky každej správy.
#[derive(Debug, Clone, Default)]
pub struct Identita {
    pub zariadenie: String,
    pub meno: String,
    pub id: String,
    /// R254: MAC adresa siete LAN – stály identifikátor zariadenia v zozname zariadení zberača (HERBÁR).
    pub mac: String,
    pub verzia: String,
    pub beh: String,
}

#[derive(Debug, Default)]
struct Stav {
    kluc: Option<[u8; 32]>,
    identita: Identita,
    rad: VecDeque<Sprava>,
    zberac: Option<SocketAddr>,
    odoslanych: u32,
    posledna_chyba: Option<String>,
    spusteny: bool,
}

static STAV: Mutex<Stav> = Mutex::new(Stav {
    kluc: None,
    identita: Identita {
        zariadenie: String::new(),
        meno: String::new(),
        id: String::new(),
        mac: String::new(),
        verzia: String::new(),
        beh: String::new(),
    },
    rad: VecDeque::new(),
    zberac: None,
    odoslanych: 0,
    posledna_chyba: None,
    spusteny: false,
});
static NOVE: Condvar = Condvar::new();
static TREBA_SIET: AtomicBool = AtomicBool::new(false);
/// R264: práve sa sťahuje aktualizácia – sieť nesmie zaspať.
static PRENOS_AKTUALIZACIE: AtomicBool = AtomicBool::new(false);
/// R253: koniec merania už išiel spolu s posledným záznamom – samostatná správa HOTOVÉ netreba.
static KONIEC_POSLANY: AtomicBool = AtomicBool::new(false);

/// Potrebuje zberač práve teraz sieť? (správca chladu podľa toho nechá čip siete zapnutý / zobudí ho)
pub fn treba_siet() -> bool {
    TREBA_SIET.load(Ordering::Relaxed) || PRENOS_AKTUALIZACIE.load(Ordering::Relaxed)
}

/// R264: začiatok a koniec sťahovania aktualizácie (sieť ostane zobudená).
pub fn prenos_aktualizacie(bezi: bool) {
    PRENOS_AKTUALIZACIE.store(bezi, Ordering::Relaxed);
}

/// R264: rad správ je prázdny (zberač má všetko, aj HOTOVÉ) – krabička smie reštartovať do novej verzie.
pub fn rad_prazdny() -> bool {
    STAV.lock().map(|s| s.rad.is_empty()).unwrap_or(true)
}

/// R264: identita krabičky pre žiadosti o aktualizáciu.
pub fn identita_spravy() -> Sprava {
    let i = STAV.lock().map(|s| s.identita.clone()).unwrap_or_default();
    s_identitou(&i)
}

/// R264: udalosť aktualizácie pre zberača (riadok v okne a v `_prehlad.txt`).
pub fn hlasenie(text: &str) {
    let i = match STAV.lock() {
        Ok(s) if s.kluc.is_some() => s.identita.clone(),
        _ => return,
    };
    do_radu(s_identitou(&i).s("ziadost", "hlasenie").s("text", text));
}

/// R264: žiadosť, na ktorú zberač odpovie bajtmi (popis balíka, kus súboru). Zberač musí byť už nájdený.
pub fn poziadaj(sprava: Sprava) -> Result<Vec<u8>, String> {
    let (kluc, adresa, mac) = {
        let s = STAV.lock().map_err(|_| "zámok")?;
        (s.kluc.ok_or("bez kľúča")?, s.zberac.ok_or("zberač ešte nie je nájdený")?, s.identita.mac.clone())
    };
    let mut sprava = sprava;
    if !mac.is_empty() {
        doplni_mac(&mut sprava, &mac);
    }
    let n = nonce().ok_or("bez náhodných čísel (/dev/urandom)")?;
    zber::poziadaj(adresa, &kluc, &n, &sprava, Duration::from_secs(30))
}

/// Zapne odosielanie (raz, keď je známe nastavenie s kľúčom domácnosti).
pub fn zapni(kluc: [u8; 32], identita: Identita) {
    let Ok(mut s) = STAV.lock() else { return };
    s.kluc = Some(kluc);
    s.identita = identita;
    if !s.spusteny {
        s.spusteny = true;
        let _ = std::thread::Builder::new().name("zberac".into()).spawn(vlakno);
    }
}

fn do_radu(sprava: Sprava) {
    let Ok(mut s) = STAV.lock() else { return };
    if s.kluc.is_none() {
        return;
    }
    if s.rad.len() >= NAJVIAC_V_RADE {
        s.rad.pop_front();
    }
    s.rad.push_back(sprava);
    TREBA_SIET.store(true, Ordering::Relaxed);
    NOVE.notify_all();
}

/// FNV-1a 64 – odtlačok obsahu, aby zberač opakovaný prenos toho istého záznamu nezdvojil.
fn odtlacok(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

fn s_identitou(i: &Identita) -> Sprava {
    Sprava::nova()
        .s("zariadenie", &i.zariadenie)
        .s("meno", &i.meno)
        .s("id", &i.id)
        .s("mac", &i.mac)
        .s("verzia", &i.verzia)
        .s("beh", &i.beh)
        // R264: krabička vie prijať aktualizáciu po sieti
        .s("aktualizacia", "1")
        .s("arch", crate::aktualizacia::arch())
        .s("cas", &crate::aktualizacia::cas().to_string())
        .s("klic", &crate::aktualizacia::popis_rezimu())
}

/// Záznam zapísaný na kartu (súbor `subor` v priečinku MERANIA, pripojený text `obsah`). R253: `posledny` –
/// záverečný záznam merania nesie aj koniec (HOTOVÉ) v tej istej správe; zberač ho dostane jedným prenosom a
/// nič nezávisí od toho, či druhá správa stihne odísť, kým sieť zaspí.
pub fn zaznam(subor: &str, obsah: &[u8], posledny: bool) {
    let i = match STAV.lock() {
        Ok(s) if s.kluc.is_some() => s.identita.clone(),
        _ => return,
    };
    if posledny {
        KONIEC_POSLANY.store(true, Ordering::Relaxed);
    }
    do_radu(
        s_identitou(&i)
            .s("subor", subor)
            .s("zaznam", &format!("{:016x}", odtlacok(obsah)))
            .s("stav", if posledny { "hotove" } else { "bezi" })
            .s_obsahom(obsah.to_vec()),
    );
}

/// Meranie dobehlo (HOTOVÉ na krabičke) – samostatná správa len vtedy, keď koniec nešiel so záznamom.
pub fn hotove() {
    if KONIEC_POSLANY.load(Ordering::Relaxed) {
        return;
    }
    let i = match STAV.lock() {
        Ok(s) if s.kluc.is_some() => s.identita.clone(),
        _ => return,
    };
    do_radu(s_identitou(&i).s("zaznam", "hotove").s("stav", "hotove"));
}

/// Stav pre záznam merania.
pub fn popis() -> String {
    let Ok(s) = STAV.lock() else { return "?".into() };
    if s.kluc.is_none() {
        return "vypnutý – karta nemá kľúč domácnosti (zapíšte ju novým ORCHIDEA Zapisovačom)".into();
    }
    let mut t = match s.zberac {
        Some(a) => format!("zberač {a}"),
        None => "zberač zatiaľ nenájdený".into(),
    };
    t += &format!(" · odoslané {} · v rade {}", s.odoslanych, s.rad.len());
    if let Some(e) = &s.posledna_chyba {
        t += &format!(" · naposledy: {e}");
    }
    t
}

/// Drôtové rozhranie so spojením a hotovou adresou miestnej linky IPv6: (meno, číslo rozhrania).
fn rozhranie() -> Option<(String, u32)> {
    let if_inet6 = std::fs::read_to_string("/proc/net/if_inet6").unwrap_or_default();
    let mut mena: Vec<String> = std::fs::read_dir("/sys/class/net")
        .ok()?
        .flatten()
        .map(|z| z.file_name().to_string_lossy().to_string())
        .filter(|m| m != "lo" && !m.starts_with("wl"))
        .collect();
    mena.sort();
    for m in mena {
        let zaklad = format!("/sys/class/net/{m}");
        if std::path::Path::new(&format!("{zaklad}/wireless")).exists() {
            continue;
        }
        if !std::fs::read_to_string(format!("{zaklad}/carrier")).is_ok_and(|t| t.trim() == "1") {
            continue;
        }
        // adresa fe80::/10, rozsah linky (20), nie predbežná (príznak 0x40 – overovanie duplicity)
        let hotova = if_inet6.lines().any(|r| {
            let p: Vec<&str> = r.split_whitespace().collect();
            p.len() == 6
                && p[5] == m
                && p[0].starts_with("fe8")
                && p[3] == "20"
                && u32::from_str_radix(p[4], 16).is_ok_and(|f| f & 0x40 == 0)
        });
        if !hotova {
            continue;
        }
        let cislo = std::fs::read_to_string(format!("{zaklad}/ifindex")).ok()?.trim().parse().ok()?;
        return Some((m, cislo));
    }
    None
}

fn nonce() -> Option<[u8; 12]> {
    use std::io::Read;
    let mut n = [0u8; 12];
    std::fs::File::open("/dev/urandom").ok()?.read_exact(&mut n).ok()?;
    Some(n)
}

fn vlakno() {
    let _ = crate::sys::nizka_priorita();
    let mut pauza = Duration::ZERO;
    // R264: nová verzia na skúške sa ozve hneď; po meraní sa krabička pýta na novú verziu každých 5 min
    let mut dalsia_otazka = Instant::now();
    loop {
        // čaká na správu (alebo na koniec pauzy po neúspechu)
        {
            let Ok(mut s) = STAV.lock() else { return };
            let od = Instant::now();
            while s.rad.is_empty() || od.elapsed() < pauza {
                let pytat = crate::aktualizacia::pytat_sa();
                if s.rad.is_empty() && pytat && Instant::now() >= dalsia_otazka {
                    let i = s.identita.clone();
                    s.rad.push_back(s_identitou(&i).s("ziadost", "ponuka"));
                    dalsia_otazka = Instant::now() + crate::aktualizacia::KONTROLA_PONUKY;
                    continue;
                }
                let zvysok = if !s.rad.is_empty() {
                    pauza.saturating_sub(od.elapsed())
                } else if pytat {
                    dalsia_otazka.saturating_duration_since(Instant::now()).max(Duration::from_secs(1))
                } else {
                    // stav sa môže zmeniť (meranie dobehne) – pozrieť sa aspoň raz za minútu
                    Duration::from_secs(60)
                };
                s = match NOVE.wait_timeout(s, zvysok) {
                    Ok((g, _)) => g,
                    Err(_) => return,
                };
            }
        }
        TREBA_SIET.store(true, Ordering::Relaxed);
        let vysledok = posli_rad();
        let prazdny = STAV.lock().map(|s| s.rad.is_empty()).unwrap_or(true);
        match vysledok {
            Ok(()) if prazdny => {
                pauza = Duration::ZERO;
                TREBA_SIET.store(false, Ordering::Relaxed);
                // R264: zberač má všetko – ak čaká nová verzia a meranie dobehlo, reštart
                crate::aktualizacia::restartuj_ak_mozno();
            }
            Ok(()) => pauza = Duration::ZERO,
            Err(e) => {
                if let Ok(mut s) = STAV.lock() {
                    s.posledna_chyba = Some(e);
                }
                // zberač nie je (počítač vypnutý, krabička mimo siete) – sieť nech zatiaľ spí
                pauza = (pauza * 2).clamp(Duration::from_secs(60), Duration::from_secs(600));
                TREBA_SIET.store(false, Ordering::Relaxed);
            }
        }
    }
}

fn posli_rad() -> Result<(), String> {
    let od = Instant::now();
    let (meno, cislo) = loop {
        if let Some(r) = rozhranie() {
            break r;
        }
        if od.elapsed() >= CAKANIE_NA_SIET {
            return Err("sieť bez spojenia alebo bez adresy IPv6".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let (kluc, identita, mut adresa) = {
        let s = STAV.lock().map_err(|_| "zámok")?;
        (s.kluc.ok_or("bez kľúča")?, s.identita.clone(), s.zberac)
    };
    if adresa.is_none() {
        let vsetci = SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1),
            zber::PORT_HLADANIA,
            0,
            cislo,
        ));
        let kto = format!("{} {}", identita.zariadenie, identita.meno).replace(char::is_whitespace, "_");
        for _ in 0..3 {
            if let Ok(a) = zber::najdi(vsetci, &kto, Duration::from_secs(2)) {
                adresa = Some(a);
                break;
            }
        }
        let a = adresa.ok_or_else(|| format!("zberač sa v sieti ({meno}) neozval"))?;
        if let Ok(mut s) = STAV.lock() {
            s.zberac = Some(a);
        }
        crate::linux::hlas(&format!("zberač meraní (R247): nájdený {a} cez {meno}"));
    }
    let a = adresa.ok_or("bez adresy")?;
    // R257: MAC adresa rozhrania, cez ktoré sa posiela. Identita vzniká pri štarte (zapni), keď na RPi 3 B+ ešte
    // nemusí byť čip siete na USB (LAN7515) ohlásený – 27. 11. prišiel RPi 3 B+ bez MAC (zberač ho zaradil podľa
    // sériového čísla, nie do zoznamu zariadení). Doplní sa tu do identity aj do správ, ktoré ju nemajú.
    let mac_rozhrania = std::fs::read_to_string(format!("/sys/class/net/{meno}/address"))
        .ok()
        .map(|m| m.trim().to_string())
        .filter(|m| m.len() == 17 && m != "00:00:00:00:00:00");
    if let (Some(m), Ok(mut s)) = (&mac_rozhrania, STAV.lock())
        && s.identita.mac.is_empty()
    {
        s.identita.mac = m.clone();
    }
    loop {
        let Some(mut sprava) = STAV.lock().ok().and_then(|s| s.rad.front().cloned()) else { return Ok(()) };
        if let Some(m) = &mac_rozhrania {
            doplni_mac(&mut sprava, m);
        }
        let n = nonce().ok_or("bez náhodných čísel (/dev/urandom)")?;
        match zber::posli(a, &kluc, &n, &sprava, Duration::from_secs(15)) {
            // R264: novší zberač môže za „ok“ pridať ponuku aktualizácie („ok nova=<verzia>“)
            Ok(o) if o == "ok" || o.starts_with("ok ") => {
                if let Ok(mut s) = STAV.lock() {
                    s.rad.pop_front();
                    s.odoslanych += 1;
                    s.posledna_chyba = None;
                }
                crate::aktualizacia::po_odpovedi(&o);
            }
            Ok(o) => return Err(format!("zberač: {o}")),
            Err(e) => {
                // nabudúce znova hľadať (počítač mohol dostať inú adresu)
                if let Ok(mut s) = STAV.lock() {
                    s.zberac = None;
                }
                return Err(e);
            }
        }
    }
}

/// R257: prázdne alebo chýbajúce pole `mac` správy doplní (vyplnené nemení).
fn doplni_mac(sprava: &mut Sprava, mac: &str) {
    match sprava.polia.iter_mut().find(|(k, _)| k == "mac") {
        Some((_, v)) if v.is_empty() => *v = mac.to_string(),
        Some(_) => {}
        None => sprava.polia.push(("mac".into(), mac.to_string())),
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn r257_mac_doplnena_pri_odoslani() {
        let i = Identita {
            zariadenie: "Raspberry Pi 3 Model B Plus Rev 1.4".into(),
            id: "00000000d7a60416".into(),
            ..Default::default()
        };
        let mut s = s_identitou(&i).s("zaznam", "0");
        assert_eq!(s.pole("mac"), Some(""));
        doplni_mac(&mut s, "b8:27:eb:a6:04:16");
        assert_eq!(s.pole("mac"), Some("b8:27:eb:a6:04:16"));
        doplni_mac(&mut s, "11:22:33:44:55:66");
        assert_eq!(s.pole("mac"), Some("b8:27:eb:a6:04:16"), "vyplnenú MAC nemení");
        let mut bez = Sprava::nova().s("zaznam", "hotove");
        doplni_mac(&mut bez, "b8:27:eb:a6:04:16");
        assert_eq!(bez.pole("mac"), Some("b8:27:eb:a6:04:16"));
    }

    #[test]
    fn odtlacok_rozlisi_obsah() {
        assert_ne!(odtlacok(b"zaznam 0"), odtlacok(b"zaznam 3"));
        assert_eq!(odtlacok(b""), 0xcbf2_9ce4_8422_2325);
    }
}
