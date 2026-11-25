//! R223: meranie vlastného dekodéra HEVC priamo na zariadení – bez ručného spúšťania skriptov.
//!
//! Program `hevc_meranie` (relácia B, rozhranie R4 v ORCHIDEA/rozhrania.md) a skúšobné toky sú v systéme v
//! pamäti. Pri prvom štarte karty na zariadení (spolu s tepelným pokusom a skúškou videa) KLÍČ pre každý tok:
//! 1. počká, kým doska vychladne pod 60 °C (najviac 5 min) – chladný systém, nie číslo za každú cenu,
//! 2. raz dekóduje s overením (MD5 každého obrazu, zhoda s MD5 v SEI toku) – bitová zhoda,
//! 3. potom čistú rýchlosť pri 1, 2 a 4 vláknach (koľko jadier má zariadenie).
//!
//! Počas behu sleduje teplotu a takt; pri strope regulátora + 5 °C beh preruší. Výsledok ide do záznamu
//! (SPOLU.TXT) v časti „HEVC – VLASTNÝ DEKODÉR (R223)“.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Program merania v systéme v pamäti.
pub const PROGRAM: &str = "/usr/bin/hevc_meranie";
/// Priečinok so skúšobnými tokmi.
pub const TOKY: &str = "/usr/share/orchidea/hevc";
/// Pred každým behom doska vychladne pod túto teplotu (najviac [`CAKANIE_NA_CHLAD`]).
const CHLADNA: f64 = 60.0;
const CAKANIE_NA_CHLAD: Duration = Duration::from_secs(300);
/// Jeden beh najviac (pomalé zariadenie, 1080p jedným vláknom).
const NAJDLHSI_BEH: Duration = Duration::from_secs(240);

/// Výsledok jedného behu programu.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Beh {
    /// polia riadku `VYSLEDOK` (kľúč, hodnota)
    pub vysledok: Vec<(String, String)>,
    /// rozmer prvého obrazu (len v režime s overením)
    pub rozmer: Option<(u32, u32)>,
    pub chyba: Option<String>,
    pub teplota_max: Option<f64>,
    pub takt_mhz: Option<(u32, u32)>,
    pub prerusene: Option<String>,
}

impl Beh {
    pub fn pole(&self, k: &str) -> Option<&str> {
        self.vysledok.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }
}

/// Rozbor výstupu programu (rozhranie R4).
pub fn rozober(vystup: &str) -> Beh {
    let mut b = Beh::default();
    for r in vystup.lines() {
        let mut casti = r.split_whitespace();
        match casti.next() {
            Some("VYSLEDOK") => {
                b.vysledok = casti
                    .filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                    .collect();
            }
            Some("OBRAZ") if b.rozmer.is_none() => {
                let pole = |k: &str| {
                    r.split_whitespace()
                        .find_map(|p| p.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
                        .and_then(|v| v.parse::<u32>().ok())
                };
                if let (Some(s), Some(v)) = (pole("sirka"), pole("vyska")) {
                    b.rozmer = Some((s, v));
                }
            }
            Some("CHYBA") => b.chyba = Some(r.trim_start_matches("CHYBA").trim().to_string()),
            _ => {}
        }
    }
    b
}

fn teplota() -> Option<f64> {
    let t = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok()?;
    t.trim().parse::<f64>().ok().map(|m| m / 1000.0)
}

fn takt_mhz() -> Option<u32> {
    let f = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq").ok()?;
    f.trim().parse::<u32>().ok().map(|k| k / 1000)
}

/// Počká, kým doska vychladne (najviac 5 minút); vráti teplotu na začiatku behu.
fn ochlad() -> Option<f64> {
    let od = Instant::now();
    loop {
        let t = teplota();
        if t.is_none_or(|t| t < CHLADNA) || od.elapsed() >= CAKANIE_NA_CHLAD {
            return t;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// Spustí program s argumentmi, sleduje teplotu a takt; pri `strop_behu` alebo po čase ho ukončí.
fn spusti_beh(program: &Path, argumenty: &[String], strop_behu: f64) -> Beh {
    let mut potomok = match Command::new(program)
        .args(argumenty)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(p) => p,
        Err(e) => return Beh { chyba: Some(format!("program sa nespustil ({e})")), ..Beh::default() },
    };
    // výstup sa číta priebežne v osobitnom vlákne – plný kanál by program zastavil
    let mut out = potomok.stdout.take();
    let citac = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let mut err = potomok.stderr.take();
    let citac_chyb = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    let od = Instant::now();
    let mut tmax: Option<f64> = None;
    let mut takty: Option<(u32, u32)> = None;
    let mut prerusene = None;
    loop {
        if let Some(t) = teplota() {
            tmax = Some(tmax.map_or(t, |m: f64| m.max(t)));
            if t >= strop_behu {
                prerusene = Some(format!("prerušené pri {t:.1} °C (chladný systém)"));
            }
        }
        if let Some(f) = takt_mhz() {
            takty = Some(takty.map_or((f, f), |(a, b)| (a.min(f), b.max(f))));
        }
        if prerusene.is_none() && od.elapsed() >= NAJDLHSI_BEH {
            prerusene = Some(format!("prerušené po {} s", NAJDLHSI_BEH.as_secs()));
        }
        if prerusene.is_some() {
            let _ = potomok.kill();
            let _ = potomok.wait();
            break;
        }
        match potomok.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(_) => break,
        }
    }
    let mut b = rozober(&citac.join().unwrap_or_default());
    let chyby = citac_chyb.join().unwrap_or_default();
    if b.vysledok.is_empty() && b.chyba.is_none() && prerusene.is_none() {
        let prvy = chyby.lines().next().unwrap_or("bez výsledku").to_string();
        b.chyba = Some(prvy);
    }
    b.teplota_max = tmax;
    b.takt_mhz = takty;
    b.prerusene = prerusene;
    b
}

fn cislo(v: &str) -> String {
    v.replace('.', ",")
}

/// Popis behu s overením: bitová zhoda podľa SEI.
pub fn popis_zhody(b: &Beh) -> String {
    if let Some(p) = &b.prerusene {
        return format!("overenie {p}");
    }
    if let Some(e) = &b.chyba {
        return format!("chyba – {e}");
    }
    let n = |k: &str| b.pole(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let (zhoda, nezhoda, bez, obrazov) = (n("sei_zhoda"), n("sei_nezhoda"), n("sei_bez"), n("obrazov"));
    match b.pole("stav") {
        Some("ok") if nezhoda == 0 && zhoda > 0 => {
            format!(
                "bitová zhoda ✓ (MD5 v SEI {zhoda}/{obrazov}{})",
                if bez > 0 { format!(", bez SEI {bez}") } else { String::new() }
            )
        }
        Some("ok") => format!("dekódovaných {obrazov} obrazov, tok bez MD5 v SEI – zhoda sa nedá overiť"),
        Some(s) => format!("NEZHODA ({s}) – MD5 v SEI {zhoda} zhodných, {nezhoda} nezhodných z {obrazov}"),
        None => "bez výsledku".into(),
    }
}

/// Popis behu rýchlosti.
pub fn popis_rychlosti(vlakna: usize, b: &Beh) -> String {
    let slovo = match vlakna {
        1 => "1 vlákno".to_string(),
        2..=4 => format!("{vlakna} vlákna"),
        _ => format!("{vlakna} vlákien"),
    };
    if let Some(p) = &b.prerusene {
        return format!("{slovo} {p}");
    }
    if let Some(e) = &b.chyba {
        return format!("{slovo} chyba – {e}");
    }
    match b.pole("obr_s") {
        Some(f) => format!("{slovo} {} obr./s", cislo(f)),
        None => format!("{slovo} bez výsledku"),
    }
}

/// Skúšobné toky v systéme (zoradené podľa mena).
pub fn toky(priecinok: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(priecinok)
        .into_iter()
        .flatten()
        .flatten()
        .map(|z| z.path())
        .filter(|p| p.extension().is_some_and(|e| e == "hevc"))
        .collect();
    v.sort();
    v
}

/// Spustí celé meranie vo vlastnom vlákne; riadky výsledku pribúdajú do `riadky`.
pub fn spusti(strop_regulatora: f64, riadky: Arc<Mutex<Vec<String>>>, hlas: fn(&str)) {
    std::thread::spawn(move || {
        let pridaj = |r: String| {
            hlas(&format!("HEVC (R223): {r}"));
            if let Ok(mut v) = riadky.lock() {
                v.push(r);
            }
        };
        let program = Path::new(PROGRAM);
        if !program.exists() {
            pridaj("program merania nie je v systéme (karta bez dekodéra HEVC)".into());
            return;
        }
        // bez snímača teploty sa nemeria (chladný systém – nevieme strážiť teplotu; aj emulátor v CI)
        if teplota().is_none() {
            pridaj("bez snímača teploty – meranie sa nerobí (chladný systém)".into());
            return;
        }
        let toky = toky(Path::new(TOKY));
        if toky.is_empty() {
            pridaj("skúšobné toky chýbajú".into());
            return;
        }
        let jadra = std::thread::available_parallelism().map_or(1, |n| n.get());
        let vlakna: Vec<usize> = [1, 2, 4].into_iter().filter(|&n| n <= jadra.max(1)).collect();
        let strop_behu = strop_regulatora + 5.0;
        pridaj(format!(
            "vlastný softvérový dekodér – {} tokov, vlákna {:?}; pred každým behom chlad pod {CHLADNA:.0} °C, \
             strop behu {strop_behu:.0} °C",
            toky.len(),
            vlakna
        ));
        for tok in toky {
            let meno = tok.file_stem().map(|m| m.to_string_lossy().to_string()).unwrap_or_default();
            let start = ochlad();
            let overenie = spusti_beh(program, &[tok.to_string_lossy().to_string()], strop_behu);
            let rozmer = overenie.rozmer.map(|(s, v)| format!(" {s}×{v}")).unwrap_or_default();
            let mut casti = vec![popis_zhody(&overenie)];
            let mut riadky_vysledku =
                vec![format!("    VYSLEDOK overenie {}", vysledok_s_teplotou(&overenie))];
            for &n in &vlakna {
                ochlad();
                let b = spusti_beh(
                    program,
                    &[
                        "--rychlost".into(),
                        "--vlakna".into(),
                        n.to_string(),
                        tok.to_string_lossy().to_string(),
                    ],
                    strop_behu,
                );
                casti.push(popis_rychlosti(n, &b));
                riadky_vysledku.push(format!("    VYSLEDOK vlakna={n} {}", vysledok_s_teplotou(&b)));
                if b.prerusene.is_some() {
                    break;
                }
            }
            pridaj(format!(
                "{meno}{rozmer}: {}{}",
                casti.join(" · "),
                start.map(|t| format!(" · štart {} °C", cislo(&format!("{t:.1}")))).unwrap_or_default()
            ));
            for r in riadky_vysledku {
                pridaj(r);
            }
        }
        pridaj("meranie HEVC hotové".into());
    });
}

fn vysledok_s_teplotou(b: &Beh) -> String {
    let mut s: Vec<String> = b.vysledok.iter().map(|(k, v)| format!("{k}={v}")).collect();
    if let Some(t) = b.teplota_max {
        s.push(format!("teplota_max={t:.1}"));
    }
    if let Some((a, z)) = b.takt_mhz {
        s.push(format!("takt_mhz={a}-{z}"));
    }
    if let Some(p) = &b.prerusene {
        s.push(format!("prerusene=\"{p}\""));
    }
    if let Some(e) = &b.chyba {
        s.push(format!("chyba=\"{e}\""));
    }
    s.join(" ")
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn rozbor_vystupu_r4() {
        let v = "HEVC_MERANIE verzia=1 subor=film720.hevc vlakna=4 rezim=md5\n\
                 OBRAZ poradie=0 poc=0 sirka=1280 vyska=720 md5=0123456789abcdef0123456789abcdef\n\
                 OBRAZ poradie=1 poc=1 sirka=1280 vyska=720 md5=0123456789abcdef0123456789abcdef\n\
                 VYSLEDOK obrazov=96 cas_ms=8123 obr_s=11.8 sei_zhoda=96 sei_nezhoda=0 sei_bez=0 chyby=0 stav=ok\n";
        let b = rozober(v);
        assert_eq!(b.rozmer, Some((1280, 720)));
        assert_eq!(b.pole("obr_s"), Some("11.8"));
        assert_eq!(popis_zhody(&b), "bitová zhoda ✓ (MD5 v SEI 96/96)");
        assert_eq!(popis_rychlosti(4, &b), "4 vlákna 11,8 obr./s");
        assert_eq!(popis_rychlosti(1, &b), "1 vlákno 11,8 obr./s");
        let zla = rozober(
            "VYSLEDOK obrazov=96 cas_ms=1 obr_s=1.0 sei_zhoda=90 sei_nezhoda=6 sei_bez=0 chyby=0 stav=nezhoda",
        );
        assert!(popis_zhody(&zla).starts_with("NEZHODA"));
        let ch = rozober("CHYBA tok sa nedá prečítať");
        assert_eq!(ch.chyba.as_deref(), Some("tok sa nedá prečítať"));
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore)]
    fn chybajuci_program_nespadne() {
        let b = spusti_beh(Path::new("/neexistuje/hevc_meranie"), &[], 200.0);
        assert!(b.chyba.is_some_and(|e| e.contains("nespustil")));
    }
}
