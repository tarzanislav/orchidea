//! Samomeranie ceny dekódovania zvuku (R6, zákon Z14) – program pre karty KLÍČA a krabičky: jeden súbor so zvukom
//! (FLAC, Opus, MP3/MP2, AC-3, PCM v ľubovoľnom kontajneri, ktorý otvorí `kontajner::otvor`) → na štandardný výstup
//! riadky v pevnom tvare (ako R4 – `hevc_meranie`), ktoré sa dajú rozobrať bez knižníc.
//!
//! Použitie: `media_meranie [--opakovani N] [--tempo SEKUND] [--davka MS] <súbor>`
//! - `--opakovani N` – počet meraných behov dekódovania naplno (predvolene 3; navyše jeden nemeraný beh s odtlačkom),
//! - `--tempo SEKUND` – simulácia prehrávania v reálnom čase (predvolene 20 s, najviac dĺžka súboru; 0 = vynechať),
//! - `--davka MS` – perióda dopĺňania vyrovnávacej pamäte pri simulácii (predvolene 250 ms = polovica 0,5 s, R306).
//!
//! Výstup (každý riadok začína kľúčovým slovom, polia `meno=hodnota` oddelené medzerou; neznáma hodnota = `-`):
//! ```text
//! MEDIA_MERANIE verzia=1 subor=<meno> kontajner=<formát> kodek=<kodek> kanalov=<n> frekvencia=<Hz> paketov=<n> trvanie_ms=<ms>
//! RYCHLOST opakovani=<n> stena_ms=<x.y> cpu_ms=<x.y> x_rt=<x.y> jadro_pct=<x.yy> cpu_zdroj=<schedstat|stat|stena>
//! ODTLACOK vzoriek=<n> blake3=<64 hex>
//! TEMPO sekund=<x.y> davka_ms=<n> cpu_ms=<x.y> jadro_pct=<x.yy> budenia_s=<x.y> vynutene_s=<x.y> meskanie_max_ms=<x.y> podtecenia=<n> teplota_zac_mc=<n> teplota_kon_mc=<n> takt_mhz=<n>
//! VYSLEDOK stav=ok|chyba chyby=<n>
//! CHYBA <text>                                                               (0 až 5 riadkov)
//! ```
//! - `RYCHLOST` – dekódovanie všetkých paketov naplno (pakety sú vopred v pamäti – úložisko sa nemeria); `stena_ms`
//!   a `cpu_ms` sú z **najrýchlejšieho** behu; `x_rt` = trvanie zvuku / čas procesora (koľkokrát rýchlejšie ako
//!   reálny čas), `jadro_pct` = 100 / `x_rt` (podiel jedného jadra pri prehrávaní, ak takt ostane rovnaký).
//!   `cpu_zdroj`: `schedstat` (ns, `/proc/thread-self/schedstat`), `stat` (10 ms, `/proc/self/stat`), `stena`
//!   (systém bez /proc – nástenné hodiny).
//! - `ODTLACOK` – BLAKE3 dekódovaných vzoriek (planárne bloky za sebou: `f32` alebo `i32` malý koniec) – na každej
//!   platforme rovnaký (R307); porovná sa s hodnotou z CI.
//! - `TEMPO` – prehrávanie ako v prehrávači: každých `davka_ms` jedno prebudenie, dekóduje sa toľko, aby vo
//!   vyrovnávacej pamäti boli 2 dávky, potom spánok do ďalšieho termínu (absolútne termíny, žiadne čakanie
//!   v slučke). `jadro_pct` = čas procesora / nástenný čas; `budenia_s` = dobrovoľné prepnutia kontextu za sekundu
//!   (spánky), `vynutene_s` = nedobrovoľné; `meskanie_max_ms` = najväčšie oneskorenie prebudenia za termínom;
//!   `podtecenia` = koľkokrát by zvuk došiel (dekódované < prehrané). Teplota z `thermal_zone0` (m°C) na začiatku
//!   a na konci, takt = priemer `scaling_cur_freq` jadra 0 (raz za sekundu).
//!
//! Návratový kód: 0 = ok, 1 = chyba dekódovania, 2 = zlé argumenty alebo súbor sa nedá otvoriť.

use orchidea_jadro::odtlacok::Hasovac;
use orchidea_media::kontajner;
use orchidea_media::limity::Limity;
use orchidea_media::paket::Paket;
use orchidea_media::stopa::Stopa;
use orchidea_media::zdroj::SuborovyZdroj;
use orchidea_media::zvuk::{BlokZvuku, DekoderZvuku, VzorkyZvuku, dekoder_zvuku};
use std::time::{Duration, Instant};

/// Najviac dát paketov v pamäti (súbory na meranie majú desiatky sekúnd, nie celé filmy).
const MAX_DAT: usize = 256 << 20;

fn pouzitie() -> ! {
    eprintln!("použitie: media_meranie [--opakovani N] [--tempo SEKUND] [--davka MS] <súbor>");
    std::process::exit(2);
}

fn main() {
    let (mut opakovani, mut tempo_s, mut davka_ms, mut subor) = (3u32, 20.0f64, 250u64, None);
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--opakovani" => {
                opakovani = a
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| (1..=100).contains(&n))
                    .unwrap_or_else(|| pouzitie())
            }
            "--tempo" => {
                tempo_s = a
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&s: &f64| (0.0..=3600.0).contains(&s))
                    .unwrap_or_else(|| pouzitie())
            }
            "--davka" => {
                davka_ms = a
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| (10..=2000).contains(&n))
                    .unwrap_or_else(|| pouzitie())
            }
            _ if subor.is_none() && !x.starts_with("--") => subor = Some(x),
            _ => pouzitie(),
        }
    }
    let Some(subor) = subor else { pouzitie() };
    let meno =
        std::path::Path::new(&subor).file_name().map_or(subor.clone(), |m| m.to_string_lossy().into_owned());

    let (format, stopa, pakety) = match nacitaj(&subor) {
        Ok(v) => v,
        Err(e) => {
            println!(
                "MEDIA_MERANIE verzia=1 subor={meno} kontajner=- kodek=- kanalov=- frekvencia=- paketov=0 trvanie_ms=-"
            );
            println!("VYSLEDOK stav=chyba chyby=1");
            println!("CHYBA {}", bez_medzier_na_konci(&e));
            std::process::exit(2);
        }
    };
    let mut chyby: Vec<String> = Vec::new();

    // nemeraný beh: odtlačok výstupu, počet vzoriek a trvanie (zahreje aj vyrovnávacie pamäte procesora)
    let mut has = Hasovac::novy();
    let (vzoriek, frekvencia, kanalov, chyb_dek) = match dekoduj_vsetko(&stopa, &pakety, Some(&mut has)) {
        Ok(v) => v,
        Err(e) => {
            println!(
                "MEDIA_MERANIE verzia=1 subor={meno} kontajner={format} kodek={} kanalov=- frekvencia=- paketov={} trvanie_ms=-",
                stopa.kodek.meno(),
                pakety.len()
            );
            println!("VYSLEDOK stav=chyba chyby=1");
            println!("CHYBA {}", bez_medzier_na_konci(&e));
            std::process::exit(1);
        }
    };
    if chyb_dek > 0 {
        chyby.push(format!("{chyb_dek} paketov sa nedalo dekódovať"));
    }
    let trvanie_s = if frekvencia > 0 { vzoriek as f64 / frekvencia as f64 } else { 0.0 };
    println!(
        "MEDIA_MERANIE verzia=1 subor={meno} kontajner={format} kodek={} kanalov={kanalov} frekvencia={frekvencia} paketov={} trvanie_ms={}",
        stopa.kodek.meno(),
        pakety.len(),
        (trvanie_s * 1000.0).round() as u64
    );

    // dekódovanie naplno: najrýchlejší z N behov (najmenej rušený inými procesmi)
    let mut najlepsi: Option<(f64, Option<f64>)> = None;
    let mut zdroj = "stena";
    for _ in 0..opakovani {
        let c0 = cpu_vlakna();
        let t0 = Instant::now();
        let _ = dekoduj_vsetko(&stopa, &pakety, None);
        let stena = t0.elapsed().as_secs_f64();
        let cpu = match (c0, cpu_vlakna()) {
            (Some((a, z)), Some((b, _))) => {
                zdroj = z;
                Some(b.saturating_sub(a) as f64 * 1e-9)
            }
            _ => None,
        };
        let kluc = cpu.unwrap_or(stena);
        if najlepsi.is_none_or(|(s, c)| kluc < c.unwrap_or(s)) {
            najlepsi = Some((stena, cpu));
        }
    }
    let (stena, cpu) = najlepsi.unwrap_or((0.0, None));
    let cas = cpu.unwrap_or(stena).max(1e-9);
    let x_rt = trvanie_s / cas;
    println!(
        "RYCHLOST opakovani={opakovani} stena_ms={:.1} cpu_ms={} x_rt={x_rt:.1} jadro_pct={:.2} cpu_zdroj={zdroj}",
        stena * 1000.0,
        cpu.map_or("-".into(), |c| format!("{:.1}", c * 1000.0)),
        if x_rt > 0.0 { 100.0 / x_rt } else { 0.0 }
    );
    println!("ODTLACOK vzoriek={vzoriek} blake3={}", has.dokonci().sestnastkovo());

    if tempo_s > 0.0 && trvanie_s > 0.0 {
        let t = tempo(&stopa, &pakety, tempo_s.min(trvanie_s), davka_ms, frekvencia);
        println!(
            "TEMPO sekund={:.1} davka_ms={davka_ms} cpu_ms={} jadro_pct={} budenia_s={} vynutene_s={} meskanie_max_ms={:.1} \
             podtecenia={} teplota_zac_mc={} teplota_kon_mc={} takt_mhz={}",
            t.sekund,
            t.cpu_s.map_or("-".into(), |c| format!("{:.1}", c * 1000.0)),
            t.cpu_s.map_or("-".into(), |c| format!("{:.2}", 100.0 * c / t.sekund.max(1e-9))),
            t.prepnutia.map_or("-".into(), |(d, _)| format!("{:.1}", d as f64 / t.sekund.max(1e-9))),
            t.prepnutia.map_or("-".into(), |(_, n)| format!("{:.1}", n as f64 / t.sekund.max(1e-9))),
            t.meskanie_max_s * 1000.0,
            t.podtecenia,
            moze(t.teplota_zac),
            moze(t.teplota_kon),
            t.takt_mhz.map_or("-".into(), |m| format!("{m:.0}"))
        );
        if t.podtecenia > 0 {
            chyby.push(format!("pri tempe {} podtečení", t.podtecenia));
        }
    }

    let stav = if chyb_dek == 0 { "ok" } else { "chyba" };
    println!("VYSLEDOK stav={stav} chyby={}", chyb_dek);
    for c in chyby.iter().take(5) {
        println!("CHYBA {c}");
    }
    std::process::exit(if chyb_dek == 0 { 0 } else { 1 });
}

fn moze(v: Option<i64>) -> String {
    v.map_or("-".into(), |x| x.to_string())
}

fn bez_medzier_na_konci(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Otvorí kontajner a načíta všetky pakety prvej zvukovej stopy do pamäte.
fn nacitaj(cesta: &str) -> Result<(&'static str, Stopa, Vec<Paket>), String> {
    let zdroj = SuborovyZdroj::otvor(cesta).map_err(|e| format!("súbor sa nedá otvoriť: {e}"))?;
    let mut k = kontajner::otvor(zdroj, Limity::default()).map_err(|e| format!("kontajner: {e}"))?;
    let (index, stopa) = k
        .stopy()
        .iter()
        .enumerate()
        .find(|(_, s)| s.zvuk.is_some())
        .map(|(i, s)| (i, s.clone()))
        .ok_or_else(|| "súbor nemá zvukovú stopu".to_string())?;
    let mut pakety = Vec::new();
    let mut dat = 0usize;
    while let Some(p) = k.dalsi_paket().map_err(|e| format!("paket: {e}"))? {
        if p.stopa != index {
            continue;
        }
        dat += p.data.len();
        if dat > MAX_DAT {
            return Err(format!("zvuk má viac ako {} MB – na meranie stačí kratší súbor", MAX_DAT >> 20));
        }
        pakety.push(p);
    }
    Ok((k.format().meno(), stopa, pakety))
}

/// Dekóduje všetky pakety novým dekodérom; s hašovačom pridá vzorky do odtlačku.
/// Vráti (vzoriek na kanál, frekvencia, kanálov, chybných paketov).
fn dekoduj_vsetko(
    stopa: &Stopa,
    pakety: &[Paket],
    mut has: Option<&mut Hasovac>,
) -> Result<(u64, u32, usize, u64), String> {
    let mut dek = dekoder_zvuku(stopa).map_err(|e| format!("dekodér: {e}"))?;
    let p = *dek.parametre();
    let mut blok = BlokZvuku::f32(p.frekvencia, p.rozlozenie);
    let (mut vzoriek, mut chyb, mut kanalov) = (0u64, 0u64, p.rozlozenie.kanalov() as usize);
    let mut bajty = Vec::new();
    for paket in pakety {
        if dek.dekoduj(paket, &mut blok).is_err() {
            chyb += 1;
            continue;
        }
        vzoriek += blok.ramcov as u64;
        kanalov = blok.kanalov();
        if let Some(h) = has.as_deref_mut() {
            bajty.clear();
            match &blok.vzorky {
                VzorkyZvuku::F32(d) => {
                    d.iter().for_each(|x| bajty.extend_from_slice(&x.to_bits().to_le_bytes()))
                }
                VzorkyZvuku::Cele { data, .. } => {
                    data.iter().for_each(|x| bajty.extend_from_slice(&x.to_le_bytes()))
                }
            }
            h.pridaj(&bajty);
        }
    }
    Ok((vzoriek, p.frekvencia, kanalov, chyb))
}

#[derive(Debug)]
struct Tempo {
    sekund: f64,
    cpu_s: Option<f64>,
    prepnutia: Option<(u64, u64)>,
    meskanie_max_s: f64,
    podtecenia: u64,
    teplota_zac: Option<i64>,
    teplota_kon: Option<i64>,
    takt_mhz: Option<f64>,
}

/// Simulácia prehrávania: absolútne termíny po `davka_ms`, pred každým spánkom dekódované aspoň 2 dávky dopredu.
fn tempo(stopa: &Stopa, pakety: &[Paket], sekund: f64, davka_ms: u64, frekvencia: u32) -> Tempo {
    let mut dek: Box<dyn DekoderZvuku> = match dekoder_zvuku(stopa) {
        Ok(d) => d,
        Err(_) => {
            return Tempo {
                sekund: 0.0,
                cpu_s: None,
                prepnutia: None,
                meskanie_max_s: 0.0,
                podtecenia: 0,
                teplota_zac: None,
                teplota_kon: None,
                takt_mhz: None,
            };
        }
    };
    let p = *dek.parametre();
    let mut blok = BlokZvuku::f32(p.frekvencia, p.rozlozenie);
    let perioda = Duration::from_millis(davka_ms);
    let ciel_vzoriek = (sekund * frekvencia as f64) as u64;
    let vopred = 2 * davka_ms * frekvencia as u64 / 1000;
    let teplota_zac = teplota();
    let (c0, s0) = (cpu_procesu(), prepnutia());
    let t0 = Instant::now();
    let (mut i, mut dekodovane, mut k) = (0usize, 0u64, 0u32);
    let (mut meskanie_max, mut podtecenia) = (0.0f64, 0u64);
    let (mut takt_sucet, mut takt_n) = (0.0f64, 0u32);
    let za_sekundu = (1000 / davka_ms).max(1) as u32;
    loop {
        let prehrane = (t0.elapsed().as_secs_f64() * frekvencia as f64) as u64;
        if k > 0 && dekodovane < prehrane.min(ciel_vzoriek) {
            podtecenia += 1;
        }
        let ciel = (prehrane + vopred).min(ciel_vzoriek);
        while dekodovane < ciel && i < pakety.len() {
            if dek.dekoduj(&pakety[i], &mut blok).is_ok() {
                dekodovane += blok.ramcov as u64;
            }
            i += 1;
        }
        if prehrane >= ciel_vzoriek || (i >= pakety.len() && prehrane >= dekodovane) {
            break;
        }
        if k % za_sekundu == 0
            && let Some(m) = takt_mhz()
        {
            takt_sucet += m;
            takt_n += 1;
        }
        k += 1;
        let termin = t0 + perioda * k;
        let teraz = Instant::now();
        if termin > teraz {
            std::thread::sleep(termin - teraz);
        }
        meskanie_max = meskanie_max.max(Instant::now().saturating_duration_since(termin).as_secs_f64());
    }
    let stena = t0.elapsed().as_secs_f64();
    let cpu_s = match (c0, cpu_procesu()) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a) as f64 * 1e-9),
        _ => None,
    };
    let prep = match (s0, prepnutia()) {
        (Some((d0, n0)), Some((d1, n1))) => Some((d1.saturating_sub(d0), n1.saturating_sub(n0))),
        _ => None,
    };
    Tempo {
        sekund: stena,
        cpu_s,
        prepnutia: prep,
        meskanie_max_s: meskanie_max,
        podtecenia,
        teplota_zac,
        teplota_kon: teplota(),
        takt_mhz: (takt_n > 0).then(|| takt_sucet / takt_n as f64),
    }
}

/// Čas procesora tohto vlákna v ns: `/proc/thread-self/schedstat` (ns), inak `/proc/self/stat` (tiky 1/100 s).
fn cpu_vlakna() -> Option<(u64, &'static str)> {
    if let Some(ns) = std::fs::read_to_string("/proc/thread-self/schedstat")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()))
    {
        return Some((ns, "schedstat"));
    }
    cpu_stat().map(|ns| (ns, "stat"))
}

/// Čas procesora celého procesu v ns (`/proc/self/schedstat`, inak `/proc/self/stat`).
fn cpu_procesu() -> Option<u64> {
    std::fs::read_to_string("/proc/self/schedstat")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()))
        .or_else(cpu_stat)
}

/// `utime + stime` z `/proc/self/stat` (polia 14 a 15; jednotka USER_HZ = 100 Hz podľa rozhrania /proc).
fn cpu_stat() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    // meno procesu v zátvorkách môže obsahovať medzery – počíta sa od poslednej „)“
    let za = &s[s.rfind(')')? + 1..];
    let p: Vec<&str> = za.split_whitespace().collect();
    // za „)“ nasleduje pole 3 (stav) → utime = pole 14 = index 11, stime = index 12
    let u: u64 = p.get(11)?.parse().ok()?;
    let st: u64 = p.get(12)?.parse().ok()?;
    Some((u + st) * 10_000_000)
}

/// (dobrovoľné, nedobrovoľné) prepnutia kontextu procesu z `/proc/self/status`.
fn prepnutia() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let pole = |kluc: &str| {
        s.lines().find_map(|r| {
            r.strip_prefix(kluc).and_then(|v| v.trim().trim_start_matches(':').trim().parse().ok())
        })
    };
    Some((pole("voluntary_ctxt_switches")?, pole("nonvoluntary_ctxt_switches")?))
}

fn teplota() -> Option<i64> {
    std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok()?.trim().parse().ok()
}

fn takt_mhz() -> Option<f64> {
    let khz: f64 = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(khz / 1000.0)
}
