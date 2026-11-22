//! # SADIČ – zapisovač obrazov SKLENÍKA
//!
//! Zapíše obraz na SD kartu alebo USB disk, overí ho spätným čítaním (odtlačok BLAKE3), pripraví
//! nastavenie zariadenia (meno, Wi-Fi, jazyk) a vie kartu zálohovať a obnoviť. Ponúkne len vymeniteľné
//! disky a nikdy disk s operačným systémom počítača. Spustený dvojklikom otvorí jednoduché okno
//! (obraz → karta → nastavenie → Zapísať); obraz vezme aj priamo zo stiahnutého súboru ZIP.
//!
//! Rozšírený režim (tlačidlo „Rozšírené…“ alebo príkazy nižšie): napálenie ISO a obrazov iných
//! systémov (Linux, Windows …) s rozborom a kontrolným súčtom, čisté formátovanie (FAT32, exFAT; MBR
//! alebo GPT, zarovnanie podľa pamäte flash) a kontrola falošnej kapacity a rýchlosti – všetko vlastným
//! kódom, bez cudzích programov.

#![deny(unsafe_code)]

#[cfg_attr(not(windows), allow(dead_code))]
mod ciel;
#[cfg_attr(not(windows), allow(dead_code))]
mod disk;
#[cfg_attr(not(windows), allow(dead_code))]
mod exfat;
#[cfg_attr(not(windows), allow(dead_code))]
mod fat32;
#[cfg_attr(not(windows), allow(dead_code))]
mod formatuj;
#[cfg_attr(not(windows), allow(dead_code))]
mod iso;
#[cfg_attr(not(windows), allow(dead_code))]
mod kontrola;
#[cfg_attr(not(windows), allow(dead_code))]
mod ntfs;
#[cfg_attr(not(windows), allow(dead_code))]
mod oddiely;
#[cfg(windows)]
mod okno;
#[cfg_attr(not(windows), allow(dead_code))]
mod rozbal;
#[cfg_attr(not(windows), allow(dead_code))]
mod sha256;
#[cfg_attr(not(windows), allow(dead_code))]
mod subory;
#[cfg(windows)]
mod windows;
#[cfg_attr(not(windows), allow(dead_code))]
mod zip;

use ciel::Ciel as _;
use sklenik_karta as obraz;
use sklenik_karta::Nastavenie;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::ExitCode;

const POMOC: &str = "SADIČ – zapisovač obrazov SKLENÍKA

  sadic zoznam                                   disky, na ktoré sa smie zapisovať
  sadic                                          okno (rovnaké ako dvojklik)
  sadic zapis <obraz.img|.zip> <disk> [nast.]    zapíše obraz a overí ho (aj priamo zo ZIP)
  sadic over <obraz.img> <disk> [nastavenie]     len overí, či je na disku obraz
  sadic zaloha <disk> <súbor.img>                uloží celý disk do súboru
  sadic obnov <súbor.img> <disk>                 vráti zálohu na disk
  sadic priprav <obraz.img> [nastavenie]         zapíše nastavenie priamo do súboru obrazu
  sadic skusobny-obraz <súbor.img> [MB]          malý obraz na skúšky

  ROZŠÍRENÉ
  sadic skumaj <obraz.iso|.img> [subory] [sha256|sucet=…]  rozbor: druh, systém, štart BIOS/UEFI
  sadic napal <obraz.iso|.img|.zip> <disk> [mbr|gpt] [sucet=…]  napáli ISO (Linux, Windows …) a overí ho
  sadic formatuj <disk> [voľby]                  čisté formátovanie karty alebo USB disku
  sadic formatuj-obraz <súbor.img> <MB> [voľby]  naformátovaný obraz disku (aj na skúšky)
  sadic napal-obraz <obraz.iso> <súbor.img> <MB> [mbr|gpt] [exfat]  ISO „napálené“ do súboru obrazu
  sadic skontroluj <disk> [uplne]                falošná kapacita, chybné miesta, rýchlosť; potom naformátuje

  nastavenie: meno=… wifi=… wifi_heslo=… jazyk=… meranie=ano/nie   (heslo sa neukladá, len kľúč WPA)
  voľby formátovania: auto|fat32|exfat  mbr|gpt  menovka=…  klaster=32K  uplne (vynuluje celý disk)
  sucet=…   kontrolný súčet SHA-256 zo stránky distribúcie – pred napálením sa overí
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
        // bez príkazu (dvojklik): okno
        #[cfg(windows)]
        [] => std::process::exit(okno::spusti(a.dovol_virtualny)),
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
        ["skumaj", cesta, zvysok @ ..] => skumaj(cesta, zvysok),
        ["formatuj-obraz", cesta, mb, zvysok @ ..] => formatuj_obraz(cesta, mb, zvysok),
        ["napal-obraz", iso, cesta, mb, zvysok @ ..] => napal_obraz(iso, cesta, mb, zvysok),
        ["formatuj", d, zvysok @ ..] => {
            let f = formatuj::Formatovanie::z_argumentov(zvysok.iter().copied()).map_err(|e| Chyba(1, e))?;
            platforma::formatuj(cislo_disku(d)?, &f, &a)
        }
        ["napal", cesta, d, zvysok @ ..] => {
            let schema = if zvysok.contains(&"gpt") { oddiely::Schema::Gpt } else { oddiely::Schema::Mbr };
            platforma::napal(cesta, cislo_disku(d)?, &sucet(zvysok)?, schema, &a)
        }
        ["skontroluj", d, zvysok @ ..] => {
            platforma::skontroluj(cislo_disku(d)?, zvysok.contains(&"uplne"), &a)
        }
        _ => {
            println!("{POMOC}");
            Ok(())
        }
    }
}

/// Voľba `sucet=…` (SHA-256 zo stránky distribúcie).
fn sucet(zvysok: &[&str]) -> Vysledok<Option<String>> {
    match zvysok.iter().find_map(|a| a.strip_prefix("sucet=")) {
        Some(t) => Ok(Some(sha256::najdi_sucet(t).ok_or(Chyba(1, format!("„{t}“ nie je súčet SHA-256")))?)),
        None => Ok(None),
    }
}

/// SHA-256 súboru s priebehom.
fn sha256_suboru(cesta: &str, priebeh: &mut dyn FnMut(u64, u64)) -> Vysledok<String> {
    let mut f = File::open(cesta)?;
    let dlzka = f.metadata()?.len();
    let mut h = sha256::Sha256::novy();
    let mut buf = vec![0u8; 1 << 20];
    let mut hotovo = 0;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.pridaj(&buf[..n]);
        hotovo += n as u64;
        priebeh(hotovo, dlzka);
    }
    Ok(sha256::hex(&h.dokonci()))
}

/// Overí súbor proti súčtu zo stránky distribúcie.
fn over_sucet(cesta: &str, ocakavany: &str) -> Vysledok<()> {
    let mut p = Priebeh::novy("kontrolný súčet", 1);
    let s = sha256_suboru(cesta, &mut |h, n| {
        p.celkom = n;
        p.krok(h)
    })?;
    if s != ocakavany {
        return Err(Chyba(
            1,
            format!(
                "SÚČET NESEDÍ: súbor je poškodený alebo iný (SHA-256 {s}, očakávaný {ocakavany}) – stiahnite ho znova"
            ),
        ));
    }
    println!("✅ kontrolný súčet SHA-256 sedí");
    Ok(())
}

/// Rozbor obrazu (funguje na každom systéme).
fn skumaj(cesta: &str, zvysok: &[&str]) -> Vysledok<()> {
    let (cesta_obrazu, docasny) = zip::priprav_obraz(std::path::Path::new(cesta))?;
    let r = iso::rozober(&mut File::open(&cesta_obrazu)?)?;
    println!("{}", r.suhrn());
    if zvysok.contains(&"subory") {
        print!("{}", r.zoznam());
    }
    if let Some(o) = sucet(zvysok)? {
        over_sucet(cesta, &o)?;
    } else if zvysok.contains(&"sha256") {
        println!("SHA-256 {}", sha256_suboru(cesta, &mut |_, _| {})?);
    }
    if docasny {
        let _ = std::fs::remove_file(cesta_obrazu);
    }
    Ok(())
}

/// Vytvorí naformátovaný obraz disku (funguje na každom systéme).
fn formatuj_obraz(cesta: &str, mb: &str, zvysok: &[&str]) -> Vysledok<()> {
    let mb: u64 = mb.parse().map_err(|_| Chyba(1, "veľkosť v MB".into()))?;
    let mut volby = zvysok.to_vec();
    let sektor = if let Some(i) = volby.iter().position(|a| *a == "sektor=4096") {
        volby.remove(i);
        4096
    } else {
        512
    };
    let f = formatuj::Formatovanie::z_argumentov(volby.into_iter()).map_err(|e| Chyba(1, e))?;
    let mut c = ciel::Subor::novy(cesta, mb << 20, sektor)?;
    let z = formatuj::formatuj(&mut c, &f, &mut |_, _, _| {}, &std::sync::atomic::AtomicBool::new(false))
        .map_err(|e| Chyba(1, e))?;
    println!("✅ {cesta}: {}", z.text());
    Ok(())
}

/// Napáli ISO do súboru obrazu disku (funguje na každom systéme; aj na skúšky režimu súborov).
fn napal_obraz(iso_cesta: &str, cesta: &str, mb: &str, zvysok: &[&str]) -> Vysledok<()> {
    let mb: u64 = mb.parse().map_err(|_| Chyba(1, "veľkosť v MB".into()))?;
    let schema = if zvysok.contains(&"gpt") { oddiely::Schema::Gpt } else { oddiely::Schema::Mbr };
    let mut f = File::open(iso_cesta)?;
    let r = iso::rozober(&mut f)?;
    println!("{}", r.suhrn());
    let mut c = ciel::Subor::novy(cesta, mb << 20, 512)?;
    let zrus = std::sync::atomic::AtomicBool::new(false);
    match r.druh() {
        iso::Druh::HybridneIso | iso::Druh::ObrazDisku => {
            if r.velkost > mb << 20 {
                return Err(Chyba(2, "obraz je väčší ako cieľ".into()));
            }
            let mut buf = vec![0u8; ciel::KUS];
            f.seek(SeekFrom::Start(0))?;
            let mut p = 0;
            while p < r.velkost {
                let n = (ciel::KUS as u64).min(r.velkost - p) as usize;
                f.read_exact(&mut buf[..n])?;
                c.zapis(p, &buf[..n])?;
                p += n as u64;
            }
            println!("✅ {cesta}: zapísané bit po bite");
        }
        iso::Druh::Nezname => return Err(Chyba(1, "nie je to obraz disku ani ISO".into())),
        _ => {
            let druhy =
                if zvysok.contains(&"exfat") { formatuj::System::ExFat } else { formatuj::System::Ntfs };
            let n = rozbal::napal_subory(&mut c, &mut f, &r, schema, druhy, &mut |_, _, _| {}, &zrus)
                .map_err(|e| Chyba(1, e))?;
            println!("✅ {cesta}: {}", n.text());
        }
    }
    Ok(())
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

/// Fáza práce SADIČA (pre ukazovateľ priebehu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Faza {
    Rozbalenie,
    Zapis,
    Overenie,
    Citanie,
    Sucet,
    Mazanie,
    Formatovanie,
    SkuskaZapis,
    SkuskaCitanie,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Faza {
    pub fn popis(self) -> &'static str {
        match self {
            Faza::Rozbalenie => "príprava obrazu",
            Faza::Zapis => "zápis",
            Faza::Overenie => "overenie",
            Faza::Citanie => "čítanie obrazu",
            Faza::Sucet => "kontrolný súčet",
            Faza::Mazanie => "vymazanie",
            Faza::Formatovanie => "formátovanie",
            Faza::SkuskaZapis => "skúšobný zápis",
            Faza::SkuskaCitanie => "kontrola čítaním",
        }
    }
}

/// Čo sa má urobiť: obraz (aj ZIP), disk, nastavenie zariadenia.
#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Uloha<'a> {
    pub obraz: &'a str,
    pub disk: u32,
    pub nastavenie: &'a Nastavenie,
    pub dovol_virtualny: bool,
    pub zapisovat: bool,
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
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Dočasný obraz rozbalený zo ZIP sa po práci zmaže.
    struct Upratanie(Option<std::path::PathBuf>);

    impl Drop for Upratanie {
        fn drop(&mut self) {
            if let Some(p) = &self.0 {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    /// Disky, na ktoré sa smie zapisovať (pre okno).
    pub fn povolene_disky(dovol_virtualny: bool) -> Vec<InfoDisku> {
        windows::disky().into_iter().filter(|d| disk::posud(d, dovol_virtualny).is_ok()).collect()
    }

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

    fn povolene(cislo: u32, dovol_virtualny: bool) -> Vysledok<InfoDisku> {
        let d = windows::disky()
            .into_iter()
            .find(|d| d.cislo == cislo)
            .ok_or(Chyba(1, format!("disk {cislo} neexistuje (pozri `sadic zoznam`)")))?;
        disk::posud(&d, dovol_virtualny)
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

    /// Zápis (alebo len overenie) obrazu na disk – spoločné jadro pre príkazy aj okno.
    /// `potvrd` dostane cieľový disk a rozhodne, či sa smie zapisovať; `priebeh` dostáva (fáza, hotovo,
    /// celkom); `zrus` preruší prácu medzi kusmi. Vráti odtlačok BLAKE3 zapísaného obrazu.
    pub fn vykonaj(
        u: &Uloha,
        potvrd: &mut dyn FnMut(&InfoDisku) -> bool,
        priebeh: &mut dyn FnMut(Faza, u64, u64),
        zrus: &AtomicBool,
    ) -> Vysledok<String> {
        priebeh(Faza::Rozbalenie, 0, 1);
        let (cesta, docasny) = zip::priprav_obraz(std::path::Path::new(u.obraz))
            .map_err(|e| Chyba(1, format!("{}: {e}", u.obraz)))?;
        let _upratanie = Upratanie(docasny.then(|| cesta.clone()));
        let mut f = File::open(&cesta)?;
        let dlzka = f.metadata()?.len();
        if dlzka == 0 || dlzka % obraz::SEKTOR != 0 {
            return Err(Chyba(1, format!("{} nie je obraz disku (dĺžka {dlzka} B)", u.obraz)));
        }
        let zaplaty = if u.nastavenie.je_prazdne() {
            Vec::new()
        } else {
            obraz::zaplaty_nastavenia(&mut f, &u.nastavenie.text()).map_err(|e| Chyba(1, e))?
        };
        let d = povolene(u.disk, u.dovol_virtualny)?;
        if dlzka > d.velkost {
            return Err(Chyba(
                2,
                format!("obraz ({}) je väčší ako disk ({})", disk::velkost(dlzka), disk::velkost(d.velkost)),
            ));
        }
        if u.zapisovat && !potvrd(&d) {
            return Err(Chyba(2, "nepotvrdené – nič sa nezapísalo".into()));
        }
        let mut zariadenie = Disk::otvor(u.disk, u.zapisovat)
            .map_err(|e| Chyba(1, format!("disk {}: {e} (spustite ako správca)", u.disk)))?;
        if zariadenie.info != d {
            return Err(Chyba(2, "disk sa medzitým zmenil (vymenená karta?) – nič sa nezapísalo".into()));
        }
        let zrusene = || Chyba(1, "zrušené – karta nie je úplná, zapíšte ju znova".into());
        let mut kus = vec![0u8; KUS];
        // odtlačok každého kusu – pri chybe povie, kde na karte je problém
        let mut kusy = Vec::new();
        let mut h = Hasovac::novy();
        let faza = if u.zapisovat { Faza::Zapis } else { Faza::Citanie };
        let mut posun = 0;
        while posun < dlzka {
            if zrus.load(Ordering::Relaxed) {
                return Err(zrusene());
            }
            let n = KUS.min((dlzka - posun) as usize);
            kus_obrazu(&mut f, &zaplaty, posun, &mut kus[..n])?;
            h.pridaj(&kus[..n]);
            kusy.push(Hasovac::novy().pridaj(&kus[..n]).dokonci());
            if u.zapisovat {
                zariadenie.zapis(posun, &kus[..n])?;
            }
            posun += n as u64;
            priebeh(faza, posun, dlzka);
        }
        if u.zapisovat {
            zariadenie.dokonci()?;
        }
        let ocakavany = h.dokonci();
        let mut h2 = Hasovac::novy();
        let mut posun = 0;
        let mut prva_chyba = None;
        for ocakavany_kus in &kusy {
            if zrus.load(Ordering::Relaxed) {
                return Err(zrusene());
            }
            let n = KUS.min((dlzka - posun) as usize);
            zariadenie.citaj(posun, &mut kus[..n])?;
            h2.pridaj(&kus[..n]);
            if prva_chyba.is_none() && Hasovac::novy().pridaj(&kus[..n]).dokonci() != *ocakavany_kus {
                prva_chyba = Some(posun);
            }
            posun += n as u64;
            priebeh(Faza::Overenie, posun, dlzka);
        }
        let skutocny = h2.dokonci();
        if u.zapisovat {
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
        Ok(ocakavany.to_string())
    }

    /// Príkaz `zapis` / `over` / `obnov` v príkazovom riadku.
    pub fn zapis(cesta: &str, cislo: u32, n: &Nastavenie, a: &Argumenty, zapisovat: bool) -> Vysledok<()> {
        let u =
            Uloha { obraz: cesta, disk: cislo, nastavenie: n, dovol_virtualny: a.dovol_virtualny, zapisovat };
        let mut chyba_potvrdenia = None;
        let mut potvrdenie = |d: &InfoDisku| match potvrd(d, a) {
            Ok(()) => true,
            Err(e) => {
                chyba_potvrdenia = Some(e);
                false
            }
        };
        let mut p: Option<(Faza, Priebeh)> = None;
        let mut priebeh = |f: Faza, hotovo: u64, celkom: u64| {
            if f == Faza::Rozbalenie {
                return;
            }
            if p.as_ref().is_none_or(|(g, _)| *g != f) {
                p = Some((f, Priebeh::novy(f.popis(), celkom)));
            }
            if let Some((_, q)) = p.as_mut() {
                q.krok(hotovo);
            }
        };
        let vysledok = vykonaj(&u, &mut potvrdenie, &mut priebeh, &AtomicBool::new(false));
        if let Some(e) = chyba_potvrdenia {
            return Err(e);
        }
        let odtlacok = vysledok?;
        println!(
            "✅ {} a overené – odtlačok BLAKE3 {odtlacok}",
            if zapisovat { "zapísané" } else { "zhoda" }
        );
        if zapisovat {
            println!("Kartu môžete vybrať a vložiť do Raspberry Pi.");
            println!("{}", sklenik_karta::RADA_UMIESTNENIE);
        }
        Ok(())
    }

    pub fn zaloha(cislo: u32, cesta: &str, a: &Argumenty) -> Vysledok<()> {
        let d = povolene(cislo, a.dovol_virtualny)?;
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

    /// Priebeh v príkazovom riadku pre ľubovoľné fázy.
    fn priebeh_cli() -> impl FnMut(Faza, u64, u64) {
        let mut p: Option<(Faza, Priebeh)> = None;
        move |f: Faza, hotovo: u64, celkom: u64| {
            if f == Faza::Rozbalenie {
                return;
            }
            if p.as_ref().is_none_or(|(g, _)| *g != f) {
                p = Some((f, Priebeh::novy(f.popis(), celkom)));
            }
            if let Some((_, q)) = p.as_mut() {
                q.celkom = celkom;
                q.krok(hotovo);
            }
        }
    }

    /// Otvorí povolený disk na zápis, ak `potvrd` súhlasí (pre okno aj príkazy).
    fn otvor_s_potvrdenim(
        cislo: u32,
        dovol_virtualny: bool,
        potvrd: &mut dyn FnMut(&InfoDisku) -> bool,
    ) -> Vysledok<Disk> {
        let d = povolene(cislo, dovol_virtualny)?;
        if !potvrd(&d) {
            return Err(Chyba(2, "nepotvrdené – nič sa nezapísalo".into()));
        }
        let z = Disk::otvor(cislo, true)
            .map_err(|e| Chyba(1, format!("disk {cislo}: {e} (spustite ako správca)")))?;
        if z.info != d {
            return Err(Chyba(2, "disk sa medzitým zmenil (vymenená karta?) – nič sa nezapísalo".into()));
        }
        Ok(z)
    }

    /// Jadro formátovania pre okno aj príkazy.
    pub fn formatuj_jadro(
        cislo: u32,
        f: &crate::formatuj::Formatovanie,
        dovol_virtualny: bool,
        potvrd: &mut dyn FnMut(&InfoDisku) -> bool,
        priebeh: &mut dyn FnMut(Faza, u64, u64),
        zrus: &AtomicBool,
    ) -> Vysledok<String> {
        let mut z = otvor_s_potvrdenim(cislo, dovol_virtualny, potvrd)?;
        let vysledok = crate::formatuj::formatuj(&mut z, f, priebeh, zrus);
        z.ohlas_nove_oddiely();
        Ok(vysledok.map_err(|e| Chyba(1, e))?.text())
    }

    /// Jadro kontroly: kontrola, potom naformátovanie (falošné médium na skutočnú kapacitu).
    /// Ak médium nie je v poriadku, vráti chybu s kódom 3 a celým výsledkom.
    pub fn skontroluj_jadro(
        cislo: u32,
        uplne: bool,
        dovol_virtualny: bool,
        potvrd: &mut dyn FnMut(&InfoDisku) -> bool,
        priebeh: &mut dyn FnMut(Faza, u64, u64),
        zrus: &AtomicBool,
    ) -> Vysledok<String> {
        let mut z = otvor_s_potvrdenim(cislo, dovol_virtualny, potvrd)?;
        let v = crate::kontrola::skontroluj(&mut z, uplne, priebeh, zrus)?;
        let f = crate::formatuj::Formatovanie::default();
        let s = if v.falosna() {
            let mut o = crate::kontrola::Obmedzeny { c: &mut z, velkost: v.skutocna };
            crate::formatuj::formatuj(&mut o, &f, priebeh, zrus)
        } else {
            crate::formatuj::formatuj(&mut z, &f, priebeh, zrus)
        };
        z.ohlas_nove_oddiely();
        let text = format!(
            "{}\n{}",
            v.text(),
            match s {
                Ok(s) => format!("Naformátované: {}", s.text()),
                Err(e) => format!("Formátovanie po kontrole zlyhalo: {e}"),
            }
        );
        if v.falosna() || !v.v_poriadku() {
            return Err(Chyba(3, text));
        }
        Ok(text)
    }

    fn potvrdenie_cli<'a>(
        a: &'a Argumenty,
        chyba: &'a mut Option<Chyba>,
    ) -> impl FnMut(&InfoDisku) -> bool + 'a {
        move |d: &InfoDisku| match potvrd(d, a) {
            Ok(()) => true,
            Err(e) => {
                *chyba = Some(e);
                false
            }
        }
    }

    /// Príkaz `formatuj`.
    pub fn formatuj(cislo: u32, f: &crate::formatuj::Formatovanie, a: &Argumenty) -> Vysledok<()> {
        let mut chyba = None;
        let vysledok = formatuj_jadro(
            cislo,
            f,
            a.dovol_virtualny,
            &mut potvrdenie_cli(a, &mut chyba),
            &mut priebeh_cli(),
            &AtomicBool::new(false),
        );
        if let Some(e) = chyba {
            return Err(e);
        }
        println!("✅ naformátované: {}", vysledok?);
        Ok(())
    }

    /// Spoločné jadro napálenia pre príkazy aj okno: kontrolný súčet, rozbor, potom zápis bit po bite
    /// (hybridné ISO, obrazy diskov) alebo rozbalenie súborov (ISO Windows a iné) – vždy s overením.
    #[allow(clippy::too_many_arguments)]
    pub fn napal_jadro(
        cesta: &str,
        cislo: u32,
        schema: crate::oddiely::Schema,
        sucet: Option<&str>,
        dovol_virtualny: bool,
        potvrd: &mut dyn FnMut(&InfoDisku) -> bool,
        priebeh: &mut dyn FnMut(Faza, u64, u64),
        zrus: &AtomicBool,
    ) -> Vysledok<String> {
        if let Some(o) = sucet {
            let s = sha256_suboru(cesta, &mut |h, n| priebeh(Faza::Sucet, h, n))?;
            if s != o {
                return Err(Chyba(
                    1,
                    format!(
                        "SÚČET NESEDÍ: súbor je poškodený alebo iný (SHA-256 {s}) – stiahnite ho znova; nič sa nezapísalo"
                    ),
                ));
            }
        }
        priebeh(Faza::Rozbalenie, 0, 1);
        let (obraz, docasny) =
            zip::priprav_obraz(std::path::Path::new(cesta)).map_err(|e| Chyba(1, format!("{cesta}: {e}")))?;
        let _upratanie = Upratanie(docasny.then(|| obraz.clone()));
        let r = crate::iso::rozober(&mut File::open(&obraz)?)?;
        match r.druh() {
            crate::iso::Druh::ObrazDisku | crate::iso::Druh::HybridneIso => {
                let cesta_obrazu = obraz.display().to_string();
                let u = Uloha {
                    obraz: &cesta_obrazu,
                    disk: cislo,
                    nastavenie: &Nastavenie::default(),
                    dovol_virtualny,
                    zapisovat: true,
                };
                let h = vykonaj(&u, potvrd, priebeh, zrus)?;
                Ok(format!("napálené bit po bite a overené – odtlačok BLAKE3 {h}"))
            }
            crate::iso::Druh::Nezname => Err(Chyba(1, format!("{cesta}: nie je to obraz disku ani ISO"))),
            _ => {
                let mut z = otvor_s_potvrdenim(cislo, dovol_virtualny, potvrd)?;
                let n = crate::rozbal::napal_subory(
                    &mut z,
                    &mut File::open(&obraz)?,
                    &r,
                    schema,
                    crate::formatuj::System::Ntfs,
                    priebeh,
                    zrus,
                );
                z.ohlas_nove_oddiely();
                Ok(n.map_err(|e| Chyba(1, e))?.text())
            }
        }
    }

    /// Príkaz `napal`.
    pub fn napal(
        cesta: &str,
        cislo: u32,
        sucet: &Option<String>,
        schema: crate::oddiely::Schema,
        a: &Argumenty,
    ) -> Vysledok<()> {
        let (obraz, docasny) =
            zip::priprav_obraz(std::path::Path::new(cesta)).map_err(|e| Chyba(1, format!("{cesta}: {e}")))?;
        let r = crate::iso::rozober(&mut File::open(&obraz)?)?;
        if docasny {
            let _ = std::fs::remove_file(&obraz);
        }
        println!("{}", r.suhrn());
        let mut chyba_potvrdenia = None;
        let vysledok = napal_jadro(
            cesta,
            cislo,
            schema,
            sucet.as_deref(),
            a.dovol_virtualny,
            &mut potvrdenie_cli(a, &mut chyba_potvrdenia),
            &mut priebeh_cli(),
            &AtomicBool::new(false),
        );
        if let Some(e) = chyba_potvrdenia {
            return Err(e);
        }
        println!("✅ {}", vysledok?);
        Ok(())
    }

    /// Príkaz `skontroluj`.
    pub fn skontroluj(cislo: u32, uplne: bool, a: &Argumenty) -> Vysledok<()> {
        let mut chyba = None;
        let vysledok = skontroluj_jadro(
            cislo,
            uplne,
            a.dovol_virtualny,
            &mut potvrdenie_cli(a, &mut chyba),
            &mut priebeh_cli(),
            &AtomicBool::new(false),
        );
        if let Some(e) = chyba {
            return Err(e);
        }
        match vysledok {
            Ok(t) => {
                println!("{t}");
                Ok(())
            }
            Err(Chyba(3, t)) => {
                println!("{t}");
                Err(Chyba(3, "médium nie je v poriadku – pozri výsledok vyššie".into()))
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(not(windows))]
mod platforma {
    use super::*;

    fn zatial() -> Chyba {
        Chyba(
            1,
            "zápis na disky je zatiaľ len pre Windows 11 (Linux a macOS pribudnú); `priprav`, `skumaj` a \
             `formatuj-obraz` fungujú všade"
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

    pub fn formatuj(_: u32, _: &crate::formatuj::Formatovanie, _: &Argumenty) -> Vysledok<()> {
        Err(zatial())
    }

    pub fn napal(
        _: &str,
        _: u32,
        _: &Option<String>,
        _: crate::oddiely::Schema,
        _: &Argumenty,
    ) -> Vysledok<()> {
        Err(zatial())
    }

    pub fn skontroluj(_: u32, _: bool, _: &Argumenty) -> Vysledok<()> {
        Err(zatial())
    }
}
