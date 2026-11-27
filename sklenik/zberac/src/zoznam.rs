//! Zoznam zariadení (HERBÁR, R254) – stále číslo a názov každej krabičky podľa MAC adresy siete LAN.
//!
//! Súbor `_zariadenia.txt` v koreni priečinka MERANIA (riadok `číslo | MAC adresa | názov`). Pri prvom spustení
//! ho zberač vytvorí zo zoznamu známeho zariadení vlastníka (`sklenik/zariadenia.txt`); nové zariadenie pridá
//! sám s ďalším číslom a názvom podľa modelu – názov potom môže vlastník v súbore prepísať (číta sa pri každej
//! správe, zberač netreba reštartovať).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Súbor zoznamu v koreni priečinka MERANIA.
pub const SUBOR: &str = "_zariadenia.txt";

/// Známe zariadenia vlastníka (27. 11. 2026) – základ nového zoznamu.
const ZAKLAD: &str = include_str!("../../zariadenia.txt");

const HLAVICKA: &str = "# Zoznam zariadení (HERBÁR) – číslo | MAC adresa siete LAN | názov.\r\n\
# Nové zariadenie pridá zberač sám s ďalším číslom. Názov môžete prepísať (číslo a MAC nechajte).\r\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zariadenie {
    pub cislo: u32,
    /// Malé písmená, dvojbodky (`00:1e:06:48:4d:60`).
    pub mac: String,
    pub nazov: String,
}

impl Zariadenie {
    /// Meno priečinka zariadenia: `01 ODROID C4 (00-1e-06-48-4d-60)` (dvojbodku Windows v mene nedovolí).
    pub fn priecinok(&self) -> String {
        crate::sklad::bezpecne_meno(&format!(
            "{:02} {} ({})",
            self.cislo,
            self.nazov,
            self.mac.replace(':', "-")
        ))
    }
}

/// MAC adresa v ľubovoľnom zápise (`:`, `-`, `_` alebo bez oddeľovačov) → `aa:bb:cc:dd:ee:ff`; nuly a iné texty
/// (sériové číslo, meno) vrátia `None`.
pub fn mac(t: &str) -> Option<String> {
    let hex: String = t.trim().chars().filter(|c| !matches!(c, ':' | '-' | '_')).collect();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) || hex.chars().all(|c| c == '0') {
        return None;
    }
    let h = hex.to_ascii_lowercase();
    Some((0..6).map(|i| &h[i * 2..i * 2 + 2]).collect::<Vec<_>>().join(":"))
}

/// Riadky zoznamu (komentáre `#` a prázdne riadky sa preskočia, chybné tiež).
pub fn citaj(text: &str) -> Vec<Zariadenie> {
    text.lines()
        .filter_map(|r| {
            let r = r.trim();
            if r.is_empty() || r.starts_with('#') {
                return None;
            }
            let mut c = r.splitn(3, '|').map(str::trim);
            let cislo = c.next()?.parse().ok()?;
            let mac = mac(c.next()?)?;
            let nazov = c.next().unwrap_or("").to_string();
            Some(Zariadenie {
                cislo,
                mac,
                nazov: if nazov.is_empty() { format!("zariadenie {cislo}") } else { nazov },
            })
        })
        .collect()
}

fn riadok(z: &Zariadenie) -> String {
    format!("{} | {} | {}\r\n", z.cislo, z.mac, z.nazov)
}

#[derive(Debug)]
pub struct Zoznam {
    cesta: PathBuf,
    zapis: Mutex<()>,
}

impl Zoznam {
    /// Otvorí zoznam v priečinku MERANIA; chýbajúci vytvorí zo základu, známe zariadenia, ktoré v ňom chýbajú,
    /// doplní (číslo zo základu, ak je voľné, inak ďalšie).
    pub fn otvor(koren: &Path) -> io::Result<Zoznam> {
        let z = Zoznam { cesta: koren.join(SUBOR), zapis: Mutex::new(()) };
        let _g = z.zapis.lock();
        let mut text = fs::read_to_string(&z.cesta).unwrap_or_default();
        if text.trim().is_empty() {
            text = HLAVICKA.to_string();
        }
        let mut polozky = citaj(&text);
        let mut doplnok = String::new();
        for zaklad in citaj(ZAKLAD) {
            if polozky.iter().any(|p| p.mac == zaklad.mac) {
                continue;
            }
            let cislo = if polozky.iter().any(|p| p.cislo == zaklad.cislo) {
                polozky.iter().map(|p| p.cislo).max().unwrap_or(0) + 1
            } else {
                zaklad.cislo
            };
            let nove = Zariadenie { cislo, ..zaklad };
            doplnok += &riadok(&nove);
            polozky.push(nove);
        }
        if !doplnok.is_empty() || !z.cesta.exists() {
            if !text.ends_with('\n') {
                text += "\r\n";
            }
            fs::write(&z.cesta, text + &doplnok)?;
        }
        drop(_g);
        Ok(z)
    }

    pub fn polozky(&self) -> Vec<Zariadenie> {
        citaj(&fs::read_to_string(&self.cesta).unwrap_or_default())
    }

    /// Zariadenie s touto MAC adresou; neznáme pridá s ďalším číslom a názvom `model`. Vráti (zariadenie, nové?).
    pub fn najdi_alebo_pridaj(&self, mac: &str, model: &str) -> io::Result<(Zariadenie, bool)> {
        let _g = self.zapis.lock();
        let polozky = self.polozky();
        if let Some(z) = polozky.iter().find(|z| z.mac == mac) {
            return Ok((z.clone(), false));
        }
        let nove = Zariadenie {
            cislo: polozky.iter().map(|p| p.cislo).max().unwrap_or(0) + 1,
            mac: mac.to_string(),
            nazov: if model.trim().is_empty() { "zariadenie".into() } else { model.trim().to_string() },
        };
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&self.cesta)?;
        if fs::metadata(&self.cesta).map(|m| m.len()).unwrap_or(0) == 0 {
            f.write_all(HLAVICKA.as_bytes())?;
        }
        f.write_all(riadok(&nove).as_bytes())?;
        Ok((nove, true))
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn mac_v_roznych_zapisoch() {
        assert_eq!(mac("00:1E:06:48:4D:60").as_deref(), Some("00:1e:06:48:4d:60"));
        assert_eq!(mac("00_1e_06_48_4d_60").as_deref(), Some("00:1e:06:48:4d:60"));
        assert_eq!(mac("00-1e-06-48-4d-60").as_deref(), Some("00:1e:06:48:4d:60"));
        assert_eq!(mac("00:00:00:00:00:00"), None);
        assert_eq!(mac("odroid-tv"), None);
        assert_eq!(mac("100000001234abcd"), None, "sériové číslo RPi nie je MAC");
    }

    #[test]
    fn zaklad_vlastnika() {
        let z = citaj(ZAKLAD);
        assert_eq!(z.len(), 6);
        assert_eq!(z[0], Zariadenie { cislo: 1, mac: "00:1e:06:48:4d:60".into(), nazov: "ODROID C4".into() });
        assert_eq!(z[0].priecinok(), "01 ODROID C4 (00-1e-06-48-4d-60)");
        assert_eq!(z[2].nazov, "RPi 3 B+ kovový chladič");
        let mut macy: Vec<&str> = z.iter().map(|x| x.mac.as_str()).collect();
        macy.sort();
        macy.dedup();
        assert_eq!(macy.len(), 6, "MAC adresy sa neopakujú");
    }
}
