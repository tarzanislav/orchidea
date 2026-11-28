//! Zberač meraní (LOVEC ZBERAČ, R247) – beží na počítači vlastníka a prijíma výsledky meraní krabičiek
//! ORCHIDEA OS v miestnej sieti; ukladá ich do priečinka MERANIA, ktorý číta aj relácia A.
//!
//! R253: okno nikdy nezastaví prenos – krabička dostane potvrdenie skôr, než sa vypíše riadok, riadky píše
//! samostatné vlákno a „Rýchla úprava“ okna (zastavuje výpis pri označení textu) je vypnutá.
//!
//! Prvé spustenie sa opýta na priečinok MERANIA a zapne spúšťanie po prihlásení (Windows). Kľúč domácnosti
//! vytvorí ORCHIDEA Zapisovač pri prvom zápise karty – zberač ho číta pri každom spojení.
//!
//! Príkazy: `orchidea-zberac` · `orchidea-zberac --priecinok <cesta>` · `--bez-spustania` (nezapne spúšťanie po
//! prihlásení) · `--adresa <ipv6> --porty <hladanie> <prenos>` (skúšky) · `--balik <priečinok> <druh> <procesor>
//! <verzia> <čas>` (R264: zapíše `popis.txt` balíka aktualizácie – nástroj CI).
//!
//! R264: aktualizácie krabičiek a samotného zberača z podpriečinka `_aktualizacia` (pozri [`aktualizacia`]).

mod aktualizacia;
mod siet;
mod sklad;
mod zoznam;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

fn cas() -> sklad::Cas {
    #[cfg(windows)]
    {
        windows::miestny_cas()
    }
    #[cfg(not(windows))]
    {
        utc()
    }
}

/// Čas UTC zo systémových hodín (mimo Windows – skúšky).
#[cfg_attr(windows, allow(dead_code))]
fn utc() -> sklad::Cas {
    let s =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (dni, sek) = ((s / 86_400) as i64, s % 86_400);
    // občiansky dátum z počtu dní (algoritmus H. Hinnanta, „days from civil“ naopak)
    let z = dni + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let r = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (r, m, d, (sek / 3600) as u32, (sek / 60 % 60) as u32, (sek % 60) as u32)
}

#[cfg(windows)]
mod windows {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        rok: u16,
        mesiac: u16,
        den_tyzdna: u16,
        den: u16,
        hodina: u16,
        minuta: u16,
        sekunda: u16,
        ms: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(cas: *mut SystemTime);
        fn GetStdHandle(ktory: u32) -> *mut core::ffi::c_void;
        fn GetConsoleMode(konzola: *mut core::ffi::c_void, rezim: *mut u32) -> i32;
        fn SetConsoleMode(konzola: *mut core::ffi::c_void, rezim: u32) -> i32;
    }

    /// R253: vypne v okne „Rýchlu úpravu“ (QuickEdit). Kým je v okne s ňou označený text, Windows zastaví každý
    /// výpis programu – zberač by stál. Kopírovať z okna ide aj potom (ponuka okna → Upraviť → Označiť).
    pub fn vypni_rychlu_upravu() -> bool {
        const STD_INPUT_HANDLE: u32 = -10i32 as u32;
        const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;
        const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;
        // SAFETY: GetStdHandle nemá predpoklady; neplatný alebo nulový úchyt GetConsoleMode odmietne (vráti 0).
        let h = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if h.is_null() || h as isize == -1 {
            return false;
        }
        let mut rezim = 0u32;
        // SAFETY: úchyt je zo systému, premenná pre režim žije počas volania.
        if unsafe { GetConsoleMode(h, &mut rezim) } == 0 {
            return false;
        }
        // SAFETY: ten istý úchyt konzoly; nový režim len bez Rýchlej úpravy.
        unsafe { SetConsoleMode(h, (rezim & !ENABLE_QUICK_EDIT_MODE) | ENABLE_EXTENDED_FLAGS) != 0 }
    }

    pub fn miestny_cas() -> crate::sklad::Cas {
        let mut c = SystemTime::default();
        // SAFETY: GetLocalTime vyplní štruktúru SYSTEMTIME, ktorá žije počas volania.
        unsafe { GetLocalTime(&mut c) };
        (c.rok as i32, c.mesiac as u32, c.den as u32, c.hodina as u32, c.minuta as u32, c.sekunda as u32)
    }
}

/// R266: denník zberača v priečinku MERANIA – každý riadok okna aj s dátumom, priebežne (relácia A ho číta).
const DENNIK: &str = "_dennik-zberaca.txt";
/// Pri tejto veľkosti sa denník presunie do `_dennik-zberaca-stary.txt` (ostane jeden starší).
const DENNIK_NAJVIAC_B: u64 = 4 << 20;

/// Otvorí denník na pripisovanie; príliš veľký najprv presunie.
fn otvor_dennik(cesta: &std::path::Path) -> Option<std::fs::File> {
    if std::fs::metadata(cesta).is_ok_and(|m| m.len() >= DENNIK_NAJVIAC_B) {
        let stary = cesta.with_file_name("_dennik-zberaca-stary.txt");
        let _ = std::fs::remove_file(&stary);
        let _ = std::fs::rename(cesta, stary);
    }
    std::fs::OpenOptions::new().create(true).append(true).open(cesta).ok()
}

/// R253: riadky do okna píše samostatné vlákno – keď okno stojí, prenos ani ukladanie nečakajú.
/// R266: to isté vlákno ich pripisuje aj do denníka `dennik` (každý riadok hneď, s dátumom).
fn hlasatel(dennik: Option<PathBuf>) -> impl Fn(&str) + Send + Sync + 'static {
    let (tx, rx) = std::sync::mpsc::channel::<(sklad::Cas, String)>();
    let _ = std::thread::Builder::new().name("okno".into()).spawn(move || {
        let mut subor = dennik.as_deref().and_then(otvor_dennik);
        let mut zapisane = 0u64;
        // chyba zápisu do okna (println! by pri nej spadol) sa ticho preskočí – prenos beží ďalej
        for (c, t) in rx {
            let _ = writeln!(std::io::stdout(), "{:02}:{:02}:{:02}  {t}", c.3, c.4, c.5);
            if let Some(f) = subor.as_mut() {
                let riadok =
                    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}  {t}\r\n", c.0, c.1, c.2, c.3, c.4, c.5);
                if f.write_all(riadok.as_bytes()).is_ok() {
                    let _ = f.flush();
                    zapisane += riadok.len() as u64;
                }
                // občas skontrolovať veľkosť (denník beží týždne)
                if zapisane >= DENNIK_NAJVIAC_B / 8 {
                    zapisane = 0;
                    subor = dennik.as_deref().and_then(otvor_dennik);
                }
            }
        }
    });
    move |t: &str| {
        let _ = tx.send((cas(), t.to_string()));
    }
}

fn subor_nastavenia() -> Option<PathBuf> {
    sklenik_korene::zber::priecinok_nastavenia().map(|p| p.join("zberac.txt"))
}

fn nacitaj_priecinok() -> Option<PathBuf> {
    let t = std::fs::read_to_string(subor_nastavenia()?).ok()?;
    t.lines()
        .find_map(|r| r.strip_prefix("priecinok="))
        .map(|p| PathBuf::from(p.trim()))
        .filter(|p| !p.as_os_str().is_empty())
}

fn uloz_priecinok(p: &std::path::Path) -> std::io::Result<()> {
    let s = subor_nastavenia().ok_or_else(|| std::io::Error::other("priečinok nastavenia nie je známy"))?;
    if let Some(d) = s.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(s, format!("# ORCHIDEA – zberač meraní (R247)\npriecinok={}\n", p.display()))
}

fn predvoleny_priecinok() -> PathBuf {
    let domov = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_default();
    domov.join("Documents").join("ORCHIDEA").join("MERANIA")
}

/// Príkaz v priečinku Po spustení (Windows).
fn subor_spustania() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    Some(
        PathBuf::from(std::env::var_os("APPDATA")?)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup")
            .join("ORCHIDEA zberac.cmd"),
    )
}

/// Windows: zberač sa spustí sám po prihlásení (príkaz v priečinku Po spustení, minimalizované okno).
fn zapni_spustanie() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = subor_spustania()?;
    let text = format!("@echo off\r\nstart \"ORCHIDEA zberac\" /min \"{}\"\r\n", exe.display());
    if std::fs::read_to_string(&p).is_ok_and(|t| t == text) {
        return Some(p);
    }
    std::fs::write(&p, text).ok()?;
    Some(p)
}

fn main() {
    #[cfg(windows)]
    windows::vypni_rychlu_upravu();
    let arg: Vec<String> = std::env::args().skip(1).collect();
    // R264: zostavenie balíka aktualizácie (CI)
    if arg.first().map(String::as_str) == Some("--balik") {
        let [_, priecinok, druh, arch, verzia, cas] = &arg[..] else {
            eprintln!("použitie: orchidea-zberac --balik <priečinok> <druh> <procesor> <verzia> <čas>");
            std::process::exit(1);
        };
        match sklenik_korene::aktualizacia::zbal_priecinok(
            std::path::Path::new(priecinok),
            druh,
            arch,
            verzia,
            cas.parse().unwrap_or(0),
        ) {
            Ok(b) => {
                print!("{}", b.do_textu());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("balík sa nepodarilo zostaviť: {e}");
                std::process::exit(1);
            }
        }
    }
    let mut n = siet::Nastavenie::default();
    let mut priecinok = None;
    let mut spustanie = true;
    let mut i = 0;
    while i < arg.len() {
        match arg[i].as_str() {
            "--priecinok" if i + 1 < arg.len() => {
                priecinok = Some(PathBuf::from(&arg[i + 1]));
                i += 1;
            }
            "--bez-spustania" => spustanie = false,
            "--adresa" if i + 1 < arg.len() => {
                n.adresa = arg[i + 1].parse().unwrap_or(n.adresa);
                i += 1;
            }
            "--porty" if i + 2 < arg.len() => {
                n.port_hladania = arg[i + 1].parse().unwrap_or(n.port_hladania);
                n.port_prenosu = arg[i + 2].parse().unwrap_or(n.port_prenosu);
                i += 2;
            }
            ine => {
                eprintln!("neznámy príkaz „{ine}“ – pozri orchidea-zberac --priecinok <cesta>");
                std::process::exit(1);
            }
        }
        i += 1;
    }
    // R253: aj odtlačok zostavenia (commit z GitHub Actions), aby bolo vidno, ktorý zberač beží
    let zostavenie =
        option_env!("GITHUB_SHA").map(|c| format!(" · {}", &c[..c.len().min(7)])).unwrap_or_default();
    println!("ORCHIDEA – zberač meraní (LOVEC ZBERAČ) · verzia {}{zostavenie}", env!("CARGO_PKG_VERSION"));
    let prvy = nacitaj_priecinok().is_none();
    let priecinok = match priecinok.or_else(nacitaj_priecinok) {
        Some(p) => p,
        None => {
            let d = predvoleny_priecinok();
            print!("Priečinok pre výsledky meraní (Enter = {}): ", d.display());
            let _ = std::io::stdout().flush();
            let mut r = String::new();
            let _ = std::io::stdin().lock().read_line(&mut r);
            let r = r.trim().trim_matches('"');
            if r.is_empty() { d } else { PathBuf::from(r) }
        }
    };
    if let Err(e) = uloz_priecinok(&priecinok) {
        eprintln!("nastavenie sa nepodarilo uložiť: {e}");
    }
    let ponuky = aktualizacia::Ponuky::novy(&priecinok);
    let sklad = match sklad::Sklad::novy(&priecinok) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("priečinok {} sa nedá použiť: {e}", priecinok.display());
            std::process::exit(1);
        }
    };
    if prvy && spustanie {
        match zapni_spustanie() {
            Some(p) => println!("Zberač sa odteraz spustí sám po prihlásení ({}).", p.display()),
            None if cfg!(windows) => println!("Spúšťanie po prihlásení sa nepodarilo zapnúť."),
            None => {}
        }
    } else if spustanie && subor_spustania().is_some_and(|p| p.exists()) {
        // R264: nový zberač spustený z iného miesta – spúšťanie po prihlásení ukáže naň (nie na starý)
        let _ = zapni_spustanie();
    }
    println!("Priečinok MERANIA: {}", priecinok.display());
    // R254: zoznam zariadení (HERBÁR) – stále čísla podľa MAC adresy
    println!("Zoznam zariadení ({}):", zoznam::SUBOR);
    let z_zoznam = sklad.zoznam.polozky().len();
    for z in sklad.zoznam.polozky() {
        println!("  {:>2}  {}  {}", z.cislo, z.mac, z.nazov);
    }
    match sklenik_korene::zber::nacitaj_kluc() {
        Some(_) => println!("Kľúč domácnosti: ✓"),
        None => {
            println!("Kľúč domácnosti ešte nie je – vznikne pri prvom zápise karty v ORCHIDEA Zapisovači.")
        }
    }
    let hlas = Arc::new(hlasatel(Some(priecinok.join(DENNIK))));
    // R266: začiatok behu do denníka – verzia, priečinok, zoznam zariadení, kľúč
    hlas(&format!(
        "zberač spustený · verzia {}{zostavenie} · priečinok {} · zariadení v zozname {} · kľúč domácnosti {}",
        env!("CARGO_PKG_VERSION"),
        priecinok.display(),
        z_zoznam,
        if sklenik_korene::zber::nacitaj_kluc().is_some() { "✓" } else { "chýba" }
    ));
    let z = Arc::new(siet::Zberac {
        sklad,
        ponuky,
        kluc: Box::new(sklenik_korene::zber::nacitaj_kluc),
        cas,
        hlas: Box::new({
            let h = hlas.clone();
            move |t: &str| h(t)
        }),
    });
    let hlas_sieti = hlas.clone();
    // R264 (F3): zberač sa sám vymení za novší z _aktualizacia (kontrola pri štarte a každú minútu)
    {
        let z = z.clone();
        let _ = std::thread::Builder::new().name("samoaktualizacia".into()).spawn(move || {
            let mut posledne = String::new();
            loop {
                if let Some(t) = aktualizacia::vymen_sa(&z.ponuky, &*hlas)
                    && t != posledne
                {
                    hlas(&t);
                    posledne = t;
                }
                // R265: Zapisovač (a ďalšie programy balíka) vedľa zberača vždy aktuálny
                for t in aktualizacia::obnov_pomocne(&z.ponuky) {
                    hlas(&t);
                }
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        });
    }
    match aktualizacia::vlastny_cas() {
        0 => {}
        c => println!(
            "Aktualizácie (R264): krabičky aj zberač z priečinka {}\\ (zostavenie {c})",
            aktualizacia::PRIECINOK
        ),
    }
    match siet::spusti(z, &n) {
        Ok((h, p)) => {
            println!(
                "Čakám na krabičky v miestnej sieti (hľadanie UDP {h}, prenos TCP {p}). Okno môžete minimalizovať."
            );
            hlas_sieti(&format!("čakám na krabičky (hľadanie UDP {h}, prenos TCP {p}) · denník {DENNIK}"));
        }
        Err(e) => {
            eprintln!("zberač sa nespustil: {e} (beží už iný zberač?)");
            hlas_sieti(&format!("zberač sa nespustil: {e} (beží už iný zberač?)"));
            std::thread::sleep(std::time::Duration::from_secs(5));
            std::process::exit(1);
        }
    }
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use sklenik_korene::zber::{self, Sprava};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::time::Duration;

    /// Zberač na slučke: IPv6, kde nie je, IPv4 (kontajnery bez IPv6). Vráti (adresa, porty) alebo None.
    fn na_slucke(z: Arc<siet::Zberac>) -> Option<(IpAddr, u16, u16)> {
        for a in [IpAddr::V6(Ipv6Addr::LOCALHOST), IpAddr::V4(Ipv4Addr::LOCALHOST)] {
            let n = siet::Nastavenie { port_hladania: 0, port_prenosu: 0, adresa: a };
            if let Ok((h, p)) = siet::spusti(z.clone(), &n) {
                return Some((a, h, p));
            }
        }
        None
    }

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
    fn utc_datum() {
        let c = utc();
        assert!(c.0 >= 2026 && (1..=12).contains(&c.1) && (1..=31).contains(&c.2));
    }

    /// Krabička nájde zberač, pošle dva záznamy a koniec; zberač ich uloží a odpovie overenou odpoveďou.
    /// Cudzí kľúč zberač odmietne.
    #[test]
    fn hladanie_prenos_a_ulozenie_cez_slucku() {
        let d = docasny("zberac-skuska");
        let kluc = [5u8; 32];
        let z = Arc::new(siet::Zberac {
            sklad: sklad::Sklad::novy(&d).unwrap(),
            ponuky: aktualizacia::Ponuky::novy(&d),
            kluc: Box::new(move || Some(kluc)),
            cas: || (2026, 11, 27, 17, 6, 0),
            hlas: Box::new(|_| {}),
        });
        let Some((a, h, p)) = na_slucke(z) else {
            eprintln!("slučka nie je dostupná – skúška vynechaná");
            return;
        };
        let ciel = SocketAddr::from((a, h));
        let adresa = zber::najdi(ciel, "odroid-tv", Duration::from_secs(3)).unwrap();
        assert_eq!(adresa.port(), p);
        let sprava = |zaznam: &str, obsah: &str| {
            Sprava::nova()
                .s("zariadenie", "Hardkernel ODROID-C4")
                .s("meno", "odroid-tv")
                .s("verzia", "abc1234")
                .s("beh", "beh-1")
                .s("subor", "01-Hardkernel-ODROID-C4-SD.txt")
                .s("zaznam", zaznam)
                .s_obsahom(obsah.as_bytes().to_vec())
        };
        let t = Duration::from_secs(5);
        assert_eq!(zber::posli(adresa, &kluc, &[1; 12], &sprava("0", "z0\n"), t).unwrap(), "ok");
        assert_eq!(zber::posli(adresa, &kluc, &[2; 12], &sprava("3", "z3\n"), t).unwrap(), "ok");
        assert_eq!(
            zber::posli(adresa, &kluc, &[3; 12], &sprava("hotove", "").s("stav", "hotove"), t).unwrap(),
            "ok"
        );
        assert!(zber::posli(adresa, &[6; 32], &[4; 12], &sprava("x", "cudzí\n"), t).is_err(), "cudzí kľúč");
        // bez MAC adresy: priečinok podľa modelu a mena; meno zo Zapisovača je aj poznámka k meraniu (R254)
        let m = d.join("hardkernel-odroid-c4 (odroid-tv)").join("2026-11-27 17-06 abc1234 – odroid-tv");
        assert_eq!(std::fs::read_to_string(m.join("01-Hardkernel-ODROID-C4-SD.txt")).unwrap(), "z0\nz3\n");
        assert!(m.join("HOTOVE.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// R264: zberač ponúkne novšiu verziu v odpovedi „ok“, krabička stiahne popis a súbory po kusoch (kód
    /// krabičky z KORENÍ) a dostane presne tie isté bajty; tá istá verzia sa už neponúkne; stará krabička
    /// (bez `aktualizacia=1`) dostane len „ok“.
    #[test]
    fn r264_aktualizacia_cez_slucku() {
        use sklenik_korene::aktualizacia as balik;
        let d = docasny("zberac-aktualizacia");
        let kluc = [9u8; 32];
        let ponuky = aktualizacia::Ponuky::novy(&d);
        let b = ponuky.priecinok("klic", "aarch64").unwrap();
        std::fs::create_dir_all(b.join("usr").join("bin")).unwrap();
        let klic: Vec<u8> = (0..(balik::KUS_B * 2 + 777)).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(b.join("init"), &klic).unwrap();
        std::fs::write(b.join("usr").join("bin").join("hevc_meranie"), b"HEVC").unwrap();
        balik::zbal_priecinok(&b, "klic", "aarch64", "f00d123", 2_000).unwrap();
        let z = Arc::new(siet::Zberac {
            sklad: sklad::Sklad::novy(&d).unwrap(),
            ponuky,
            kluc: Box::new(move || Some(kluc)),
            cas: || (2026, 11, 27, 23, 30, 0),
            hlas: Box::new(|_| {}),
        });
        let Some((a, _, p)) = na_slucke(z) else {
            eprintln!("slučka nie je dostupná – skúška vynechaná");
            return;
        };
        let adresa = SocketAddr::from((a, p));
        let t = Duration::from_secs(5);
        let identita_s_casom = |verzia: &str, cas: &str| {
            Sprava::nova()
                .s("zariadenie", "Raspberry Pi 5 Model B Rev 1.0")
                .s("mac", "2c:cf:67:00:00:06")
                .s("verzia", verzia)
                .s("beh", "beh-r264")
                .s("aktualizacia", "1")
                .s("arch", "aarch64")
                .s("cas", cas)
        };
        let identita = |verzia: &str| identita_s_casom(verzia, "1000");
        let zaznam =
            identita("abc1234").s("subor", "01-RPi5.txt").s("zaznam", "1").s_obsahom(b"z\n".to_vec());
        assert_eq!(zber::posli(adresa, &kluc, &[1; 12], &zaznam, t).unwrap(), "ok nova=f00d123");
        let stara = Sprava::nova().s("zariadenie", "RPi 5").s("verzia", "abc1234").s("zaznam", "2");
        assert_eq!(zber::posli(adresa, &kluc, &[2; 12], &stara, t).unwrap(), "ok", "stará karta len „ok“");
        let otazka = identita("abc1234").s("ziadost", "ponuka");
        assert_eq!(zber::posli(adresa, &kluc, &[3; 12], &otazka, t).unwrap(), "ok nova=f00d123");
        // sťahovanie: každá žiadosť má nové nonce
        let mut n = 10u8;
        let mut ziadaj = |s: Sprava| {
            n += 1;
            zber::poziadaj(adresa, &kluc, &[n; 12], &s, t)
        };
        let mut kroky = 0;
        let st =
            balik::stiahni(&mut ziadaj, &identita("abc1234"), "klic", "aarch64", "f00d123", &mut |_, _| {
                kroky += 1
            })
            .unwrap();
        assert_eq!(st.balik.verzia, "f00d123");
        assert_eq!(st.subory, vec![klic.clone(), b"HEVC".to_vec()]);
        assert_eq!(kroky, 4, "KLÍČ v 3 kusoch, HEVC v 1");
        // iná verzia, než je ponúkaná, sa nepošle
        assert!(
            balik::stiahni(&mut ziadaj, &identita("abc1234"), "klic", "aarch64", "0000000", &mut |_, _| {})
                .is_err()
        );
        // krabička s ponúkanou verziou (alebo novšou) už ponuku nedostane
        let nova = identita("f00d123").s("ziadost", "ponuka");
        assert_eq!(zber::posli(adresa, &kluc, &[40; 12], &nova, t).unwrap(), "ok");
        let novsia = identita_s_casom("1234abc", "3000").s("ziadost", "ponuka");
        assert_eq!(zber::posli(adresa, &kluc, &[41; 12], &novsia, t).unwrap(), "ok");
        // hlásenie krabičky ide do prehľadu
        let h = identita("f00d123").s("ziadost", "hlasenie").s("text", "aktualizácia f00d123 potvrdená");
        assert_eq!(zber::posli(adresa, &kluc, &[42; 12], &h, t).unwrap(), "ok");
        assert!(
            std::fs::read_to_string(d.join("_prehlad.txt"))
                .unwrap()
                .contains("aktualizácia f00d123 potvrdená")
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// R266: denník – riadok s dátumom hneď v súbore; príliš veľký sa presunie do staršieho. Vo WebAssembly
    /// (WASI) nie sú vlákna – vlákno okna sa nespustí, zberač tam nebeží.
    #[test]
    #[cfg_attr(target_family = "wasm", ignore = "WASI nemá vlákna")]
    fn r266_dennik_zberaca() {
        let d = docasny("dennik");
        std::fs::create_dir_all(&d).unwrap();
        let cesta = d.join(DENNIK);
        let hlas = hlasatel(Some(cesta.clone()));
        hlas("prvý riadok");
        hlas("druhý riadok");
        let t = std::time::Instant::now();
        while !std::fs::read_to_string(&cesta).is_ok_and(|x| x.contains("druhý riadok")) {
            assert!(t.elapsed() < Duration::from_secs(5), "denník sa nezapísal");
            std::thread::sleep(Duration::from_millis(20));
        }
        let text = std::fs::read_to_string(&cesta).unwrap();
        let r: Vec<&str> = text.lines().collect();
        assert_eq!(r.len(), 2);
        assert!(r[0].ends_with("  prvý riadok") && r[0].as_bytes()[4] == b'-' && r[0].as_bytes()[13] == b':');
        std::fs::write(&cesta, vec![b'x'; DENNIK_NAJVIAC_B as usize]).unwrap();
        drop(otvor_dennik(&cesta));
        assert_eq!(std::fs::metadata(d.join("_dennik-zberaca-stary.txt")).unwrap().len(), DENNIK_NAJVIAC_B);
        assert_eq!(std::fs::metadata(&cesta).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}
