//! # SADIČ – zapisovač obrazov SKLENÍKA
//!
//! Zapíše obraz na SD kartu alebo USB disk, overí ho spätným čítaním (odtlačok BLAKE3), pripraví
//! nastavenie zariadenia (meno, Wi-Fi, jazyk) a vie kartu zálohovať a obnoviť. Ponúkne len vymeniteľné
//! disky a nikdy disk s operačným systémom počítača.

#![deny(unsafe_code)]

#[cfg_attr(not(windows), allow(dead_code))]
mod disk;
#[cfg(windows)]
mod windows;

use sklenik_karta as obraz;
use sklenik_karta::Nastavenie;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::ExitCode;

const POMOC: &str = "SADIČ – zapisovač obrazov SKLENÍKA

  sadic zoznam                                   disky, na ktoré sa smie zapisovať
  sadic zapis <obraz.img> <disk> [nastavenie]    zapíše obraz a overí ho
  sadic over <obraz.img> <disk> [nastavenie]     len overí, či je na disku obraz
  sadic zaloha <disk> <súbor.img>                uloží celý disk do súboru
  sadic obnov <súbor.img> <disk>                 vráti zálohu na disk
  sadic priprav <obraz.img> [nastavenie]         zapíše nastavenie priamo do súboru obrazu
  sadic skusobny-obraz <súbor.img> [MB]          malý obraz na skúšky

  nastavenie: meno=… wifi=… wifi_heslo=… jazyk=…   (heslo sa neukladá, len kľúč WPA)
  --potvrdzujem <disk>   potvrdenie bez otázky (napr. v skriptoch)";

/// Chyba s návratovým kódom: 1 = chyba, 2 = zápis odmietnutý poistkou.
struct Chyba(u8, String);

impl<E: std::fmt::Display> From<E> for Chyba {
    fn from(e: E) -> Chyba {
        Chyba(1, e.to_string())
    }
}

type Vysledok<T> = Result<T, Chyba>;

struct Argumenty {
    volne: Vec<String>,
    potvrdenie: Option<String>,
    dovol_virtualny: bool,
}

fn argumenty() -> Vysledok<Argumenty> {
    let mut a = Argumenty { volne: Vec::new(), potvrdenie: None, dovol_virtualny: false };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--potvrdzujem" => {
                a.potvrdenie = Some(it.next().ok_or(Chyba(1, "--potvrdzujem potrebuje číslo disku".into()))?)
            }
            "--dovol-virtualny" => {
                if std::env::var("SADIC_SKUSKA").as_deref() != Ok("1") {
                    return Err(Chyba(2, "--dovol-virtualny je len pre skúšky (SADIC_SKUSKA=1)".into()));
                }
                a.dovol_virtualny = true;
            }
            _ => a.volne.push(x),
        }
    }
    Ok(a)
}

fn cislo_disku(s: &str) -> Vysledok<u32> {
    s.parse().map_err(|_| Chyba(1, format!("„{s}“ nie je číslo disku (pozri `sadic zoznam`)")))
}

fn nastavenie(zvysok: &[String]) -> Vysledok<Nastavenie> {
    Nastavenie::z_argumentov(zvysok.iter().map(String::as_str)).map_err(|e| Chyba(1, e))
}

fn main() -> ExitCode {
    match spusti() {
        Ok(()) => ExitCode::SUCCESS,
        Err(Chyba(kod, sprava)) => {
            eprintln!("SADIČ: {sprava}");
            ExitCode::from(kod)
        }
    }
}

fn spusti() -> Vysledok<()> {
    let a = argumenty()?;
    let v: Vec<&str> = a.volne.iter().map(String::as_str).collect();
    match v.as_slice() {
        ["skusobny-obraz", cesta, zvysok @ ..] => {
            let mb = zvysok
                .first()
                .map_or(Ok(64), |s| s.parse::<u64>())
                .map_err(|_| Chyba(1, "veľkosť v MB".into()))?;
            File::create(cesta)?.write_all(&obraz::skusobny_obraz(mb.max(40)))?;
            println!("skúšobný obraz {cesta} ({mb} MB)");
            Ok(())
        }
        ["priprav", cesta, ..] => priprav(cesta, &nastavenie(&a.volne[2..])?),
        ["zoznam"] => platforma::zoznam(a.dovol_virtualny),
        ["zapis", cesta, d, ..] => {
            platforma::zapis(cesta, cislo_disku(d)?, &nastavenie(&a.volne[3..])?, &a, true)
        }
        ["obnov", cesta, d] => platforma::zapis(cesta, cislo_disku(d)?, &Nastavenie::default(), &a, true),
        ["over", cesta, d, ..] => {
            platforma::zapis(cesta, cislo_disku(d)?, &nastavenie(&a.volne[3..])?, &a, false)
        }
        ["zaloha", d, cesta] => platforma::zaloha(cislo_disku(d)?, cesta, &a),
        _ => {
            println!("{POMOC}");
            Ok(())
        }
    }
}

/// Zapíše nastavenie priamo do súboru obrazu (funguje na každom systéme).
fn priprav(cesta: &str, n: &Nastavenie) -> Vysledok<()> {
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(cesta)?;
    if !n.je_prazdne() {
        for z in obraz::zaplaty_nastavenia(&mut f, &n.text()).map_err(|e| Chyba(1, e))? {
            f.seek(SeekFrom::Start(z.posun))?;
            f.write_all(&z.data)?;
        }
    }
    println!("{}", obraz::citaj_nastavenie(&mut f).map_err(|e| Chyba(1, e))?);
    Ok(())
}

/// Načíta kus obrazu so záplatami.
#[cfg_attr(not(windows), allow(dead_code))]
fn kus_obrazu(f: &mut File, zaplaty: &[obraz::Zaplata], posun: u64, kus: &mut [u8]) -> Vysledok<()> {
    f.seek(SeekFrom::Start(posun))?;
    f.read_exact(kus)?;
    obraz::uplatni(zaplaty, posun, kus);
    Ok(())
}

/// Ukazovateľ priebehu – vypíše sa najviac raz za dve sekundy.
#[cfg_attr(not(windows), allow(dead_code))]
struct Priebeh {
    start: std::time::Instant,
    posledny: std::time::Instant,
    celkom: u64,
    co: &'static str,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Priebeh {
    fn novy(co: &'static str, celkom: u64) -> Priebeh {
        let t = std::time::Instant::now();
        Priebeh { start: t, posledny: t, celkom, co }
    }

    fn krok(&mut self, hotovo: u64) {
        if self.posledny.elapsed().as_secs() >= 2 || hotovo == self.celkom {
            let s = self.start.elapsed().as_secs_f64().max(0.001);
            println!(
                "  {} {:5.1} %  ({:.1} MB/s)",
                self.co,
                100.0 * hotovo as f64 / self.celkom.max(1) as f64,
                hotovo as f64 / 1e6 / s
            );
            self.posledny = std::time::Instant::now();
        }
    }
}

#[cfg(windows)]
mod platforma {
    use super::*;
    use crate::disk::{self, InfoDisku};
    use crate::windows::{self, Disk, KUS};
    use orchidea_jadro::Hasovac;

    pub fn zoznam(dovol_virtualny: bool) -> Vysledok<()> {
        for d in windows::disky() {
            let verdikt = match disk::posud(&d, dovol_virtualny) {
                Ok(()) => "✅ smie sa zapisovať".to_string(),
                Err(e) => format!("⛔ {e}"),
            };
            println!(
                "disk {}: {} – {}, {}  {verdikt}",
                d.cislo,
                d.model,
                disk::velkost(d.velkost),
                d.zbernica.popis()
            );
        }
        Ok(())
    }

    fn povolene(cislo: u32, a: &Argumenty) -> Vysledok<InfoDisku> {
        let d = windows::disky()
            .into_iter()
            .find(|d| d.cislo == cislo)
            .ok_or(Chyba(1, format!("disk {cislo} neexistuje (pozri `sadic zoznam`)")))?;
        disk::posud(&d, a.dovol_virtualny)
            .map_err(|e| Chyba(2, format!("na disk {cislo} sa nezapisuje: {e}")))?;
        Ok(d)
    }

    fn potvrd(d: &InfoDisku, a: &Argumenty) -> Vysledok<()> {
        println!(
            "\nCIEĽ: disk {} – {}, {}, {}",
            d.cislo,
            d.model,
            disk::velkost(d.velkost),
            d.zbernica.popis()
        );
        println!("VŠETKY DÁTA NA TOMTO DISKU SA PREPÍŠU.");
        let odpoved = match &a.potvrdenie {
            Some(p) => p.clone(),
            None => {
                print!("Na potvrdenie napíšte číslo disku ({}): ", d.cislo);
                std::io::stdout().flush()?;
                let mut s = String::new();
                std::io::stdin().read_line(&mut s)?;
                s.trim().to_string()
            }
        };
        if odpoved != d.cislo.to_string() {
            return Err(Chyba(2, "nepotvrdené – nič sa nezapísalo".into()));
        }
        Ok(())
    }

    pub fn zapis(cesta: &str, cislo: u32, n: &Nastavenie, a: &Argumenty, zapisovat: bool) -> Vysledok<()> {
        let mut f = File::open(cesta)?;
        let dlzka = f.metadata()?.len();
        if dlzka == 0 || dlzka % obraz::SEKTOR != 0 {
            return Err(Chyba(1, format!("{cesta} nie je obraz disku (dĺžka {dlzka} B)")));
        }
        let zaplaty = if n.je_prazdne() {
            Vec::new()
        } else {
            obraz::zaplaty_nastavenia(&mut f, &n.text()).map_err(|e| Chyba(1, e))?
        };
        let d = povolene(cislo, a)?;
        if dlzka > d.velkost {
            return Err(Chyba(
                2,
                format!("obraz ({}) je väčší ako disk ({})", disk::velkost(dlzka), disk::velkost(d.velkost)),
            ));
        }
        if zapisovat {
            potvrd(&d, a)?;
        }
        let mut zariadenie = Disk::otvor(cislo, zapisovat)
            .map_err(|e| Chyba(1, format!("disk {cislo}: {e} (spustite ako správca)")))?;
        if zariadenie.info != d {
            return Err(Chyba(2, "disk sa medzitým zmenil (vymenená karta?) – nič sa nezapísalo".into()));
        }
        let mut kus = vec![0u8; KUS];
        // odtlačok každého kusu – pri chybe povie, kde na karte je problém
        let mut kusy = Vec::new();
        let mut h = Hasovac::novy();
        let mut p = Priebeh::novy(if zapisovat { "zápis" } else { "čítanie obrazu" }, dlzka);
        let mut posun = 0;
        while posun < dlzka {
            let n = KUS.min((dlzka - posun) as usize);
            kus_obrazu(&mut f, &zaplaty, posun, &mut kus[..n])?;
            h.pridaj(&kus[..n]);
            kusy.push(Hasovac::novy().pridaj(&kus[..n]).dokonci());
            if zapisovat {
                zariadenie.zapis(posun, &kus[..n])?;
            }
            posun += n as u64;
            p.krok(posun);
        }
        if zapisovat {
            zariadenie.dokonci()?;
        }
        let ocakavany = h.dokonci();
        let mut h2 = Hasovac::novy();
        let mut p = Priebeh::novy("overenie", dlzka);
        let mut posun = 0;
        let mut prva_chyba = None;
        for ocakavany_kus in &kusy {
            let n = KUS.min((dlzka - posun) as usize);
            zariadenie.citaj(posun, &mut kus[..n])?;
            h2.pridaj(&kus[..n]);
            if prva_chyba.is_none() && Hasovac::novy().pridaj(&kus[..n]).dokonci() != *ocakavany_kus {
                prva_chyba = Some(posun);
            }
            posun += n as u64;
            p.krok(posun);
        }
        let skutocny = h2.dokonci();
        if zapisovat {
            zariadenie.ohlas_nove_oddiely();
        }
        if skutocny != ocakavany {
            let kde = prva_chyba
                .map_or(String::new(), |o| format!(" – prvý rozdiel v oblasti {} MB", o / 1_000_000));
            return Err(Chyba(
                1,
                format!("OVERENIE ZLYHALO: na disku je iný obsah{kde} (karta môže byť poškodená)"),
            ));
        }
        println!(
            "✅ {} a overené – odtlačok BLAKE3 {ocakavany}",
            if zapisovat { "zapísané" } else { "zhoda" }
        );
        if zapisovat {
            println!("Kartu môžete vybrať a vložiť do Raspberry Pi.");
        }
        Ok(())
    }

    pub fn zaloha(cislo: u32, cesta: &str, a: &Argumenty) -> Vysledok<()> {
        let d = povolene(cislo, a)?;
        let mut zariadenie = Disk::otvor(cislo, false)
            .map_err(|e| Chyba(1, format!("disk {cislo}: {e} (spustite ako správca)")))?;
        let mut f = File::create(cesta)?;
        let mut kus = vec![0u8; KUS];
        let mut h = Hasovac::novy();
        let mut p = Priebeh::novy("záloha", d.velkost);
        let mut posun = 0;
        while posun < d.velkost {
            let n = KUS.min((d.velkost - posun) as usize);
            zariadenie.citaj(posun, &mut kus[..n])?;
            h.pridaj(&kus[..n]);
            f.write_all(&kus[..n])?;
            posun += n as u64;
            p.krok(posun);
        }
        f.sync_all()?;
        println!("✅ záloha {cesta} ({}) – odtlačok BLAKE3 {}", disk::velkost(d.velkost), h.dokonci());
        Ok(())
    }
}

#[cfg(not(windows))]
mod platforma {
    use super::*;

    fn zatial() -> Chyba {
        Chyba(
            1,
            "zápis na disky je zatiaľ len pre Windows 11 (Linux a macOS pribudnú); `priprav` funguje všade"
                .into(),
        )
    }

    pub fn zoznam(_: bool) -> Vysledok<()> {
        Err(zatial())
    }

    pub fn zapis(_: &str, _: u32, _: &Nastavenie, _: &Argumenty, _: bool) -> Vysledok<()> {
        Err(zatial())
    }

    pub fn zaloha(_: u32, _: &str, _: &Argumenty) -> Vysledok<()> {
        Err(zatial())
    }
}
