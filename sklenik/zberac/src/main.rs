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
//! prihlásení) · `--adresa <ipv6> --porty <hladanie> <prenos>` (skúšky).

mod siet;
mod sklad;

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

/// R253: riadky do okna píše samostatné vlákno – keď okno stojí, prenos ani ukladanie nečakajú.
fn hlasatel() -> impl Fn(&str) + Send + Sync + 'static {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let _ = std::thread::Builder::new().name("okno".into()).spawn(move || {
        // chyba zápisu do okna (println! by pri nej spadol) sa ticho preskočí – prenos beží ďalej
        for riadok in rx {
            let _ = writeln!(std::io::stdout(), "{riadok}");
        }
    });
    move |t: &str| {
        let c = cas();
        let _ = tx.send(format!("{:02}:{:02}:{:02}  {t}", c.3, c.4, c.5));
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

/// Windows: zberač sa spustí sám po prihlásení (príkaz v priečinku Po spustení, minimalizované okno).
fn zapni_spustanie() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let p = PathBuf::from(std::env::var_os("APPDATA")?)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("ORCHIDEA zberac.cmd");
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
    }
    println!("Priečinok MERANIA: {}", priecinok.display());
    match sklenik_korene::zber::nacitaj_kluc() {
        Some(_) => println!("Kľúč domácnosti: ✓"),
        None => {
            println!("Kľúč domácnosti ešte nie je – vznikne pri prvom zápise karty v ORCHIDEA Zapisovači.")
        }
    }
    let z = Arc::new(siet::Zberac {
        sklad,
        kluc: Box::new(sklenik_korene::zber::nacitaj_kluc),
        cas,
        hlas: Box::new(hlasatel()),
    });
    match siet::spusti(z, &n) {
        Ok((h, p)) => println!(
            "Čakám na krabičky v miestnej sieti (hľadanie UDP {h}, prenos TCP {p}). Okno môžete minimalizovať."
        ),
        Err(e) => {
            eprintln!("zberač sa nespustil: {e} (beží už iný zberač?)");
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
    use std::net::{Ipv6Addr, SocketAddr};
    use std::time::Duration;

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
            kluc: Box::new(move || Some(kluc)),
            cas: || (2026, 11, 27, 17, 6, 0),
            hlas: Box::new(|_| {}),
        });
        let n = siet::Nastavenie { port_hladania: 0, port_prenosu: 0, adresa: Ipv6Addr::LOCALHOST };
        let Ok((h, p)) = siet::spusti(z, &n) else {
            eprintln!("IPv6 slučka nie je dostupná – skúška vynechaná");
            return;
        };
        let ciel = SocketAddr::from((Ipv6Addr::LOCALHOST, h));
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
        let m = d.join("hardkernel-odroid-c4 (odroid-tv)").join("2026-11-27 17-06 abc1234");
        assert_eq!(std::fs::read_to_string(m.join("01-Hardkernel-ODROID-C4-SD.txt")).unwrap(), "z0\nz3\n");
        assert!(m.join("HOTOVE.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
