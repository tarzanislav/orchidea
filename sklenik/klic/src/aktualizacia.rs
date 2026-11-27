//! R264 (F4a): aktualizácia systému v pamäti po sieti – bez napaľovania karty.
//!
//! Vlastník 27. 11. 2026: „chcem túto fázu urýchliť, aby som nemusel stále napaľovať SD karty, je to náročné
//! na čas a môžem sa pomýliť“.
//!
//! **Ako to ide:**
//! 1. Zberač meraní v odpovedi „ok“ ponúkne novšiu verziu (`ok nova=<verzia>`). Krabička ju stiahne po kusoch
//!    (šifrované kľúčom domácnosti), overí veľkosť a odtlačok BLAKE3 každého súboru, zapíše ju na kartu do
//!    `aktualizacia/<verzia>/` a odtlačky overí ešte raz čítaním z karty.
//! 2. Keď meranie dobehne (HOTOVÉ odišlo zberaču), krabička sa reštartuje.
//! 3. **Štartér** – začiatok KLÍČA z karty (systém v pamäti zapísaný Zapisovačom) – pred všetkým ostatným
//!    prečíta `aktualizacia/stav.txt`, overí odtlačky súborov verzie, skopíruje ich do pamäte a spustí nový
//!    KLÍČ (exec, ostáva procesom 1). Strážny časovač mu odovzdá otvorený.
//! 4. Nová verzia je **na skúške**: potvrdí ju až overená odpoveď zberača (nová verzia vie prijať ďalšie
//!    opravy). Bez potvrdenia do 15 minút sa reštartuje; pád, zamrznutie (strážny časovač) aj vypnutie počas
//!    skúšky sú pokus. Po 2 nepotvrdených pokusoch štartér spustí poslednú potvrdenú verziu, alebo KLÍČ z karty.
//!    Zlyhaná verzia sa už nikdy znova nenainštaluje.
//!
//! KLÍČ z karty sa nikdy neprepíše – je to vždy poistka. Jadro, popis hardvéru a firmvér táto fáza nemení.

use crate::linux::hlas;
use crate::{sys, zaznam};
use sklenik_korene::aktualizacia::{self as balik, Balik, POPIS};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Priečinok na karte (koreň oddielu FAT).
pub const PRIECINOK: &str = "aktualizacia";
const STAV: &str = "stav.txt";
/// Premenná prostredia novej verzie: `skuska <verzia> <pokus>` alebo `dobra <verzia>`.
const REZIM: &str = "ORCHIDEA_KLIC";
/// Číslo deskriptora odovzdaného strážneho časovača.
const STRAZCA: &str = "ORCHIDEA_STRAZCA_FD";
/// Verzia KLÍČA z karty (štartéra) – do záznamu.
const KARTA: &str = "ORCHIDEA_KARTA";
const BOD: &str = "/run/karta-aktualizacia";
/// Kam štartér skopíruje nový KLÍČ (`/run` je tmpfs v pamäti).
const KLIC_V_PAMATI: &str = "/run/orchidea-klic";
/// Najviac nepotvrdených štartov jednej verzie.
pub const NAJVIAC_POKUSOV: u32 = 2;
/// Do kedy musí nová verzia dostať overenú odpoveď zberača.
const LEHOTA_POTVRDENIA: Duration = Duration::from_secs(15 * 60);
/// Do kedy musí hlavná slučka novej verzie prevziať strážny časovač (dovtedy ho kŕmi pomocné vlákno).
const PREVZATIE_STRAZCU: Duration = Duration::from_secs(180);
/// Ako často sa nečinná krabička opýta zberača na novú verziu.
pub const KONTROLA_PONUKY: Duration = Duration::from_secs(5 * 60);
/// Koľko zlyhaných verzií si stav pamätá.
const PAMATAJ_ZLYHANE: usize = 16;

/// Krátka verzia tohto KLÍČA (odtlačok commitu).
pub fn verzia() -> &'static str {
    crate::VERZIA.rsplit(' ').next().unwrap_or(crate::VERZIA)
}

/// Čas zostavenia (čas commitu z CI, 0 = neznámy) – nikdy sa neprejde na starší balík.
pub fn cas() -> u64 {
    option_env!("ORCHIDEA_CAS").and_then(|c| c.trim().parse().ok()).unwrap_or(0)
}

/// Procesor balíka (`aarch64`, `arm`).
pub fn arch() -> &'static str {
    std::env::consts::ARCH
}

/// Odkiaľ beží tento KLÍČ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rezim {
    /// KLÍČ zo systému v pamäti na karte.
    ZKarty,
    /// Potvrdená aktualizácia.
    Potvrdena(String),
    /// Aktualizácia na skúške (pokus 1 alebo 2).
    NaSkuske { verzia: String, pokus: u32 },
}

/// Beží tento KLÍČ ako nová verzia spustená štartérom? (systémové priečinky sú už pripojené)
pub fn spusteny_starterom() -> bool {
    std::env::var_os(REZIM).is_some()
}

pub fn rezim() -> Rezim {
    let t = std::env::var(REZIM).unwrap_or_default();
    let p: Vec<&str> = t.split_whitespace().collect();
    match p[..] {
        ["skuska", v, n] => Rezim::NaSkuske { verzia: v.into(), pokus: n.parse().unwrap_or(1) },
        ["dobra", v] => Rezim::Potvrdena(v.into()),
        _ => Rezim::ZKarty,
    }
}

/// Popis pre záznam a obrazovku.
pub fn popis_rezimu() -> String {
    let karta = std::env::var(KARTA).unwrap_or_else(|_| crate::VERZIA.to_string());
    match rezim() {
        Rezim::ZKarty => format!("KLÍČ z karty ({karta})"),
        Rezim::Potvrdena(v) => format!("aktualizácia {v} (potvrdená) · karta {karta}"),
        Rezim::NaSkuske { verzia, pokus } => {
            let potvrdena = if POTVRDENE.load(Ordering::Relaxed) { ", potvrdená" } else { "" };
            format!(
                "aktualizácia {verzia} na skúške (pokus {pokus} z {NAJVIAC_POKUSOV}{potvrdena}) · karta {karta}"
            )
        }
    }
}

/// `aktualizacia/stav.txt` na karte.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stav {
    /// Posledná potvrdená verzia.
    pub dobra: Option<String>,
    /// Verzia na skúške.
    pub skusana: Option<String>,
    /// Koľkokrát už štartér skúšanú verziu spustil.
    pub pokusov: u32,
    /// Verzie, ktoré zlyhali (nikdy znova).
    pub zlyhane: Vec<String>,
}

impl Stav {
    pub fn z_textu(t: &str) -> Stav {
        let mut s = Stav::default();
        let verzia = |h: &str| Some(h.trim().to_string()).filter(|v| balik::platna_verzia(v));
        for r in t.lines() {
            match r.split_once('=') {
                Some(("dobra", h)) => s.dobra = verzia(h),
                Some(("skusana", h)) => s.skusana = verzia(h),
                Some(("pokusov", h)) => s.pokusov = h.trim().parse().unwrap_or(NAJVIAC_POKUSOV),
                Some(("zlyhane", h)) => {
                    s.zlyhane =
                        h.split_whitespace().filter(|v| balik::platna_verzia(v)).map(String::from).collect()
                }
                _ => {}
            }
        }
        s
    }

    pub fn do_textu(&self) -> String {
        format!(
            "# ORCHIDEA OS – stav aktualizácie (R264). Nemeniť ručne; zmazanie celého priečinka = KLÍČ z karty.\n\
             dobra={}\nskusana={}\npokusov={}\nzlyhane={}\n",
            self.dobra.as_deref().unwrap_or(""),
            self.skusana.as_deref().unwrap_or(""),
            self.pokusov,
            self.zlyhane.join(" ")
        )
    }

    /// Verzia zlyhala: už sa nespustí ani nenainštaluje.
    pub fn zlyhala(&mut self, v: &str) {
        if self.skusana.as_deref() == Some(v) {
            self.skusana = None;
            self.pokusov = 0;
        }
        if self.dobra.as_deref() == Some(v) {
            self.dobra = None;
        }
        if !self.zlyhane.iter().any(|z| z == v) {
            self.zlyhane.push(v.to_string());
        }
        let n = self.zlyhane.len();
        if n > PAMATAJ_ZLYHANE {
            self.zlyhane.drain(..n - PAMATAJ_ZLYHANE);
        }
    }
}

/// Čo spustí štartér.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Volba {
    ZKarty,
    NaSkuske(String, u32),
    Potvrdena(String),
}

/// Rozhodnutie štartéra (zmení stav: pokus sa zapíše pred spustením, vyčerpaná verzia zlyhala).
pub fn rozhodni(s: &mut Stav) -> Volba {
    if let Some(v) = s.skusana.clone() {
        if s.pokusov < NAJVIAC_POKUSOV {
            s.pokusov += 1;
            return Volba::NaSkuske(v, s.pokusov);
        }
        s.zlyhala(&v);
    }
    match &s.dobra {
        Some(v) => Volba::Potvrdena(v.clone()),
        None => Volba::ZKarty,
    }
}

/// Kam sa súbor balíka dostane v systéme v pamäti (`None` – nepovolené).
/// Relatívne k `/` – skúšky dajú iný koreň.
fn ciel_v_pamati(cesta: &str) -> Option<PathBuf> {
    if cesta == "init" {
        Some(PathBuf::from(KLIC_V_PAMATI.trim_start_matches('/')))
    } else if balik::bezpecna_cesta(cesta) && cesta.starts_with("usr/") {
        Some(PathBuf::from(cesta))
    } else {
        None
    }
}

/// Je balík pre túto krabičku a smie ísť do pamäte?
fn over_balik(b: &Balik, v: &str) -> Result<(), String> {
    if b.druh != "klic" || b.arch != arch() || b.verzia != v {
        return Err(format!(
            "balík {}-{} {} nie je pre túto krabičku ({} {v})",
            b.druh,
            b.arch,
            b.verzia,
            arch()
        ));
    }
    if b.subor("init").is_none() {
        return Err("balík bez programu KLÍČ (init)".into());
    }
    if let Some(s) = b.subory.iter().find(|s| ciel_v_pamati(&s.cesta).is_none()) {
        return Err(format!("nepovolený súbor v balíku: {}", s.cesta));
    }
    Ok(())
}

fn na_karte(d: &Path, cesta: &str) -> PathBuf {
    cesta.split('/').fold(d.to_path_buf(), |p, k| p.join(k))
}

// ---------------------------------------------------------------- karta

/// Naraz len jedna práca s kartou v tomto module (štartér, sťahovanie, potvrdenie).
static KARTA_ZAMOK: Mutex<()> = Mutex::new(());
/// Disk, z ktorého krabička štartovala (nastavenie SKLENIK.TXT) – sem sa ukladá aktualizácia.
static SPUSTACI: Mutex<Option<String>> = Mutex::new(None);

pub fn nastav_spustaci(disk: Option<String>) {
    if let Ok(mut s) = SPUSTACI.lock() {
        *s = disk;
    }
}

fn pripoj_s(n: &zaznam::Nosic, priznaky: std::ffi::c_ulong) -> Result<(), String> {
    let _ = fs::create_dir_all(BOD);
    sys::pripoj_s_volbami(
        &format!("/dev/{}", n.oddiel),
        BOD,
        "vfat",
        priznaky | sys::MS_NOSUID | sys::MS_NODEV | sys::MS_NOEXEC | sys::MS_NOATIME,
        "flush",
    )
    .map_err(|e| format!("pripojenie {}: {e}", n.oddiel))
}

fn pripoj(n: &zaznam::Nosic) -> Result<(), String> {
    pripoj_s(n, 0)
}

/// Má nosič priečinok aktualizácie? Pozrie sa len na čítanie – na cudzí oddiel (napr. vnútorná pamäť TV
/// krabičky) sa nič nezapíše.
fn ma_aktualizaciu(n: &zaznam::Nosic) -> bool {
    ma_subor(n, &Path::new(PRIECINOK).join(STAV))
}

/// Je na nosiči (v koreni oddielu FAT) súbor `cesta`? Len na čítanie.
fn ma_subor(n: &zaznam::Nosic, cesta: &Path) -> bool {
    if pripoj_s(n, sys::MS_RDONLY).is_err() {
        return false;
    }
    let je = Path::new(BOD).join(cesta).exists();
    let _ = sys::odpoj(BOD);
    je
}

fn odpoj() {
    let _ = sys::odpoj(BOD);
    sys::zapis_vyrovnavacie();
}

fn citaj_stav(koren: &Path) -> Option<Stav> {
    fs::read_to_string(koren.join(STAV)).ok().map(|t| Stav::z_textu(&t))
}

fn zapis_stav(koren: &Path, s: &Stav) -> std::io::Result<()> {
    fs::create_dir_all(koren)?;
    let novy = koren.join("stav.novy");
    let mut f = fs::File::create(&novy)?;
    f.write_all(s.do_textu().as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&novy, koren.join(STAV))?;
    sys::zapis_vyrovnavacie();
    Ok(())
}

/// Nosič s aktualizáciou (má `aktualizacia/stav.txt`), inak nosič spúšťacieho disku. Pripojený na [`BOD`];
/// vráti cestu priečinka aktualizácie. Volajúci drží [`KARTA_ZAMOK`] a nakoniec zavolá [`odpoj`].
fn pripoj_nosic() -> Result<PathBuf, String> {
    let spustaci = SPUSTACI.lock().ok().and_then(|s| s.clone());
    let nosice = zaznam::najdi_nosice(spustaci.as_deref());
    let n = nosice
        .iter()
        .find(|n| ma_aktualizaciu(n))
        .or(nosice.first())
        .ok_or("karta ani USB kľúč s FAT nie sú v systéme")?;
    pripoj(n)?;
    Ok(Path::new(BOD).join(PRIECINOK))
}

// ---------------------------------------------------------------- štartér

/// Skopíruje overenú verziu `v` z karty (`koren` = priečinok aktualizácie) do pamäte pod koreň `pamat`.
fn do_pamate(koren: &Path, v: &str, pamat: &Path) -> Result<(), String> {
    if !balik::platna_verzia(v) {
        return Err(format!("neplatná verzia {v}"));
    }
    let d = koren.join(v);
    let text = fs::read_to_string(d.join(POPIS)).map_err(|e| format!("{POPIS}: {e}"))?;
    let b = Balik::z_textu(&text)?;
    over_balik(&b, v)?;
    let mut subory = Vec::new();
    for s in &b.subory {
        let data = fs::read(na_karte(&d, &s.cesta)).map_err(|e| format!("{}: {e}", s.cesta))?;
        if data.len() as u64 != s.velkost || balik::odtlacok(&data) != s.odtlacok {
            return Err(format!("{}: na karte nesedí s popisom (poškodená karta?)", s.cesta));
        }
        subory.push((s.cesta.clone(), data));
    }
    for (cesta, data) in subory {
        let ciel = pamat.join(ciel_v_pamati(&cesta).ok_or_else(|| format!("nepovolený súbor {cesta}"))?);
        if let Some(p) = ciel.parent() {
            fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        let docasny = PathBuf::from(format!("{}.novy", ciel.display()));
        fs::write(&docasny, &data).map_err(|e| format!("{}: {e}", docasny.display()))?;
        let vykonatelny = cesta == "init" || cesta.starts_with("usr/bin/");
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(
            &docasny,
            fs::Permissions::from_mode(if vykonatelny { 0o755 } else { 0o644 }),
        );
        fs::rename(&docasny, &ciel).map_err(|e| format!("{}: {e}", ciel.display()))?;
    }
    Ok(())
}

/// Otvorí strážny časovač (15 s), nakŕmi ho a nechá ho prežiť exec. Vráti číslo deskriptora.
fn odovzdaj_strazcu() -> Option<i32> {
    use std::os::fd::IntoRawFd;
    let mut f = sys::otvor_strazny_casovac(15).ok()?;
    let _ = f.write_all(b"\0");
    let fd = f.into_raw_fd();
    if sys::dedi_pri_exec(fd, true).is_err() {
        prevezmi_fd(fd);
        return None;
    }
    Some(fd)
}

/// Spustí nový KLÍČ z pamäte (exec – ostáva procesom 1). Vráti sa len pri chybe.
fn spusti(rezim: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let strazca = odovzdaj_strazcu();
    let mut c = std::process::Command::new(KLIC_V_PAMATI);
    c.args(std::env::args_os().skip(1)).env(REZIM, rezim).env(KARTA, crate::VERZIA);
    if let Some(fd) = strazca {
        c.env(STRAZCA, fd.to_string());
    }
    let e = c.exec();
    // exec zlyhal – strážny časovač prevezme tento KLÍČ (inak by ho nik nekŕmil)
    if let Some(fd) = strazca {
        prevezmi_fd(fd);
    }
    e
}

/// **Štartér** (KLÍČ z karty, proces 1, hneď po pripojení /proc, /sys, /dev a /run): ak je na karte overená
/// aktualizácia, spustí ju. Pri akejkoľvek chybe beží ďalej KLÍČ z karty – štart sa nikdy nezastaví.
pub fn startuj() {
    if std::env::var_os(REZIM).is_some() {
        return;
    }
    // poistka: ani nečakaná chyba v štartéri nesmie zastaviť štart (proces 1 by skončil a jadro by stálo)
    if std::panic::catch_unwind(startuj_vnutri).is_err() {
        hlas("aktualizácia (R264): štartér zlyhal – beží KLÍČ z karty");
    }
}

fn startuj_vnutri() {
    let _z = KARTA_ZAMOK.lock();
    // čaká na nosič, z ktorého krabička štartovala (má nastavenie sklenik.txt; FAT nerozlišuje veľkosť písmen):
    // karta sa ohlási do sekundy, USB kľúč X96 o pár sekúnd – najviac 20 s
    let od = Instant::now();
    let nosice = loop {
        let n = zaznam::najdi_nosice(None);
        if n.iter().any(|x| ma_subor(x, Path::new("sklenik.txt"))) || od.elapsed() >= Duration::from_secs(20)
        {
            break n;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    for n in &nosice {
        if !ma_aktualizaciu(n) || pripoj(n).is_err() {
            continue;
        }
        let koren = Path::new(BOD).join(PRIECINOK);
        let Some((volba, zapis, zlyhanie)) = vyber_verziu(&koren, Path::new("/"), &mut |t| hlas(&t)) else {
            odpoj();
            continue;
        };
        odpoj();
        match (volba, zapis) {
            (Some((rezim, _)), Err(e)) if rezim.starts_with("skuska") => {
                // pokus sa nedá zaznamenať – skúšaná verzia by sa po páde spúšťala dookola
                hlas(&format!("aktualizácia (R264): stav sa nedá zapísať ({e}) – beží KLÍČ z karty"));
            }
            (Some((rezim, popis)), _) => {
                hlas(&format!("aktualizácia (R264): spúšťam {popis} ({})", n.oddiel));
                let e = spusti(&rezim);
                hlas(&format!("aktualizácia (R264): nový KLÍČ sa nespustil ({e}) – beží KLÍČ z karty"));
            }
            (None, Err(e)) => hlas(&format!("aktualizácia (R264): stav sa nedá zapísať ({e})")),
            (None, Ok(())) => {
                if zlyhanie {
                    hlas("aktualizácia (R264): beží KLÍČ z karty (aktualizácia zlyhala)");
                }
            }
        }
        return;
    }
}

/// Jadro štartéra bez pripájania (skúšky ho volajú s dočasnými priečinkami): prečíta stav v `koren`,
/// rozhodne, overí a skopíruje verziu do pamäte pod `pamat` a zapíše stav (pokus pred spustením).
/// Vráti (režim a popis na spustenie, výsledok zápisu stavu, niečo zlyhalo) alebo `None` bez stavu.
#[allow(clippy::type_complexity)]
fn vyber_verziu(
    koren: &Path,
    pamat: &Path,
    hlas: &mut dyn FnMut(String),
) -> Option<(Option<(String, String)>, std::io::Result<()>, bool)> {
    let mut stav = citaj_stav(koren)?;
    let povodny = stav.clone();
    let mut zlyhanie = false;
    let volba = loop {
        match rozhodni(&mut stav) {
            Volba::ZKarty => break None,
            Volba::NaSkuske(v, p) => match do_pamate(koren, &v, pamat) {
                Ok(()) => {
                    break Some((
                        format!("skuska {v} {p}"),
                        format!("aktualizácia {v} na skúške, pokus {p}"),
                    ));
                }
                Err(e) => {
                    hlas(format!("aktualizácia (R264): verzia {v} sa nedá spustiť – {e}"));
                    stav.zlyhala(&v);
                    zlyhanie = true;
                }
            },
            Volba::Potvrdena(v) => match do_pamate(koren, &v, pamat) {
                Ok(()) => break Some((format!("dobra {v}"), format!("aktualizácia {v} (potvrdená)"))),
                Err(e) => {
                    hlas(format!("aktualizácia (R264): potvrdená verzia {v} sa nedá spustiť – {e}"));
                    stav.dobra = None;
                    zlyhanie = true;
                }
            },
        }
    };
    if stav.zlyhane.len() > povodny.zlyhane.len() {
        zlyhanie = true;
    }
    let zapis = if stav != povodny { zapis_stav(koren, &stav) } else { Ok(()) };
    Some((volba, zapis, zlyhanie))
}

// ---------------------------------------------------------------- strážny časovač

static PREVZATY: Mutex<Option<fs::File>> = Mutex::new(None);

/// Nová verzia hneď po štarte: prevezme strážny časovač od štartéra a kŕmi ho pomocným vláknom, kým ho
/// neprevezme hlavná slučka (najviac [`PREVZATIE_STRAZCU`] – zamrznutá nová verzia dosku reštartuje).
pub fn prevezmi_strazcu() {
    if let Some(fd) = std::env::var(STRAZCA).ok().and_then(|t| t.trim().parse().ok()) {
        prevezmi_fd(fd);
    }
}

fn prevezmi_fd(fd: i32) {
    let Some(f) = sys::zdedeny_subor(fd) else { return };
    // programy, ktoré KLÍČ spúšťa (meranie HEVC), ho nezdedia
    let _ = sys::dedi_pri_exec(fd, false);
    if let Ok(mut g) = PREVZATY.lock() {
        *g = Some(f);
    }
    let _ = std::thread::Builder::new().name("strazca-start".into()).spawn(|| {
        let od = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(4));
            let Ok(mut g) = PREVZATY.lock() else { return };
            let Some(f) = g.as_mut() else { return };
            if od.elapsed() >= PREVZATIE_STRAZCU {
                hlas("aktualizácia (R264): hlavná slučka KLÍČA nezačala do 3 minút – strážny časovač dosku reštartuje");
                return;
            }
            let _ = f.write_all(b"\0");
        }
    });
}

/// Strážny časovač pre hlavnú slučku: prevzatý od štartéra, inak nový (R99).
pub fn strazny_casovac(sekund: i32) -> std::io::Result<fs::File> {
    if let Some(f) = PREVZATY.lock().ok().and_then(|mut g| g.take()) {
        let _ = sys::nastav_strazny_casovac(&f, sekund);
        return Ok(f);
    }
    sys::otvor_strazny_casovac(sekund)
}

// ---------------------------------------------------------------- skúška a potvrdenie

static POTVRDENE: AtomicBool = AtomicBool::new(false);

/// Nová verzia na skúške: lehota na potvrdenie (overená odpoveď zberača).
pub fn zacni_skusku() {
    let Rezim::NaSkuske { verzia, pokus } = rezim() else { return };
    hlas(&format!(
        "aktualizácia (R264): verzia {verzia} na skúške (pokus {pokus} z {NAJVIAC_POKUSOV}) – potvrdí ju odpoveď \
         zberača do 15 min"
    ));
    let _ = std::thread::Builder::new().name("skuska-verzie".into()).spawn(move || {
        std::thread::sleep(LEHOTA_POTVRDENIA);
        if !POTVRDENE.load(Ordering::Relaxed) {
            hlas(&format!(
                "aktualizácia (R264): verzia {verzia} sa do 15 min nespojila so zberačom – reštart (štartér ju \
                 skúsi znova alebo vráti predošlú)"
            ));
            std::thread::sleep(Duration::from_secs(2));
            let _ = sys::vypni(true);
        }
    });
}

/// Overená odpoveď zberača: potvrdí skúšanú verziu a spracuje ponuku `nova=<verzia>`.
pub fn po_odpovedi(odpoved: &str) {
    if let Rezim::NaSkuske { verzia, .. } = rezim()
        && !POTVRDENE.swap(true, Ordering::Relaxed)
    {
        let _ = std::thread::Builder::new().name("potvrdenie".into()).spawn(move || match potvrd(&verzia) {
            Ok(()) => {
                hlas(&format!("aktualizácia (R264): verzia {verzia} potvrdená – zberač odpovedá"));
                crate::zber::hlasenie(&format!("aktualizácia {verzia} potvrdená (beží, zberač odpovedá)"));
            }
            Err(e) => hlas(&format!("aktualizácia (R264): potvrdenie verzie {verzia} sa nedá zapísať – {e}")),
        });
    }
    if let Some(v) = odpoved.split_whitespace().find_map(|w| w.strip_prefix("nova=")) {
        ponuka(v);
    }
}

fn potvrd(v: &str) -> Result<(), String> {
    let _z = KARTA_ZAMOK.lock();
    let koren = pripoj_nosic()?;
    let r = (|| {
        let mut s = citaj_stav(&koren).ok_or("stav aktualizácie chýba")?;
        if s.skusana.as_deref() != Some(v) {
            return Ok(());
        }
        if let Some(stara) = s.dobra.replace(v.to_string())
            && stara != v
        {
            let _ = fs::remove_dir_all(koren.join(&stara));
        }
        s.skusana = None;
        s.pokusov = 0;
        zapis_stav(&koren, &s).map_err(|e| e.to_string())
    })();
    odpoj();
    r
}

// ---------------------------------------------------------------- sťahovanie a reštart

static STAHUJE: AtomicBool = AtomicBool::new(false);
static PRIPRAVENA: Mutex<Option<String>> = Mutex::new(None);
static ODMIETNUTE: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Meranie dobehlo (HOTOVÉ) – krabička smie reštartovať do novej verzie a pýta sa na ponuky.
static NECINNA: AtomicBool = AtomicBool::new(false);
static RESTART: AtomicBool = AtomicBool::new(false);

/// Má sa krabička sama pýtať zberača na novú verziu? (po meraní, a nová verzia hneď po štarte)
pub fn pytat_sa() -> bool {
    NECINNA.load(Ordering::Relaxed)
        || (matches!(rezim(), Rezim::NaSkuske { .. }) && !POTVRDENE.load(Ordering::Relaxed))
}

/// Zberač ponúka verziu `v` – stiahne ju vo vlastnom vlákne (raz naraz), až keď meranie dobehlo (sťahovanie
/// a zápis na kartu nesmú ovplyvniť meranie; po HOTOVÉ sa krabička na ponuku opýta sama).
pub fn ponuka(v: &str) {
    let bezi = match rezim() {
        Rezim::NaSkuske { verzia, .. } | Rezim::Potvrdena(verzia) => verzia,
        Rezim::ZKarty => String::new(),
    };
    if !balik::platna_verzia(v) || v == verzia() || v == bezi || !NECINNA.load(Ordering::Relaxed) {
        return;
    }
    if PRIPRAVENA.lock().is_ok_and(|p| p.as_deref() == Some(v))
        || ODMIETNUTE.lock().is_ok_and(|o| o.iter().any(|x| x == v))
        || STAHUJE.swap(true, Ordering::Relaxed)
    {
        return;
    }
    let v = v.to_string();
    let _ = std::thread::Builder::new().name("aktualizacia".into()).spawn(move || {
        let _ = sys::nizka_priorita();
        let t = Instant::now();
        let r = stiahni_a_uloz(&v);
        STAHUJE.store(false, Ordering::Relaxed);
        match r {
            Ok(true) => {
                if let Ok(mut p) = PRIPRAVENA.lock() {
                    *p = Some(v.clone());
                }
                let kedy = if NECINNA.load(Ordering::Relaxed) { "hneď" } else { "po skončení merania" };
                hlas(&format!(
                    "aktualizácia (R264): verzia {v} stiahnutá a overená na karte ({:.0} s) – reštart {kedy}",
                    t.elapsed().as_secs_f64()
                ));
                crate::zber::hlasenie(&format!("aktualizácia {v} pripravená na karte – reštart {kedy}"));
                restartuj_ak_mozno();
            }
            Ok(false) => {
                if let Ok(mut o) = ODMIETNUTE.lock() {
                    o.push(v.clone());
                }
                hlas(&format!("aktualizácia (R264): verzia {v} predtým zlyhala – nenainštaluje sa"));
                crate::zber::hlasenie(&format!(
                    "aktualizácia {v} odmietnutá – na tejto krabičke predtým zlyhala"
                ));
            }
            Err(e) => {
                hlas(&format!("aktualizácia (R264): verzia {v} – {e} (skúsi sa znova pri ďalšej ponuke)"));
                crate::zber::hlasenie(&format!("aktualizácia {v} zlyhala: {e}"));
            }
        }
    });
}

/// Stiahne verziu `v` a uloží ju na kartu ako skúšanú. `Ok(false)` – verzia predtým zlyhala.
fn stiahni_a_uloz(v: &str) -> Result<bool, String> {
    {
        let _z = KARTA_ZAMOK.lock();
        let koren = pripoj_nosic()?;
        let stav = citaj_stav(&koren).unwrap_or_default();
        odpoj();
        if stav.zlyhane.iter().any(|z| z == v) {
            return Ok(false);
        }
        if stav.skusana.as_deref() == Some(v) {
            return Ok(true);
        }
    }
    crate::zber::prenos_aktualizacie(true);
    let mut posledne = 0;
    let r = balik::stiahni(
        &mut |s| crate::zber::poziadaj(s),
        &crate::zber::identita_spravy(),
        "klic",
        arch(),
        v,
        &mut |h, s| {
            let p = (h * 100).checked_div(s).unwrap_or(100);
            if p >= posledne + 25 {
                posledne = p - p % 25;
                hlas(&format!("aktualizácia (R264): {v} – stiahnuté {p} % ({} kB)", h / 1024));
            }
        },
    );
    crate::zber::prenos_aktualizacie(false);
    let st = r?;
    over_balik(&st.balik, v)?;
    let _z = KARTA_ZAMOK.lock();
    let koren = pripoj_nosic()?;
    let r = uloz(&koren, &st);
    odpoj();
    r.map(|()| true)
}

fn uloz(koren: &Path, st: &balik::Stiahnuty) -> Result<(), String> {
    let v = &st.balik.verzia;
    let d = koren.join(v);
    let chyba = |c: &dyn std::fmt::Display, e: std::io::Error| format!("zápis na kartu ({c}): {e}");
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).map_err(|e| chyba(&d.display(), e))?;
    for (s, data) in st.balik.subory.iter().zip(&st.subory) {
        let c = na_karte(&d, &s.cesta);
        if let Some(p) = c.parent() {
            fs::create_dir_all(p).map_err(|e| chyba(&s.cesta, e))?;
        }
        let mut f = fs::File::create(&c).map_err(|e| chyba(&s.cesta, e))?;
        f.write_all(data).map_err(|e| chyba(&s.cesta, e))?;
        f.sync_all().map_err(|e| chyba(&s.cesta, e))?;
        sys::zabudni_v_pamati(&f);
        drop(f);
        // overenie čítaním z karty (stránky zahodené z vyrovnávacej pamäte)
        let precitane = fs::read(&c).map_err(|e| chyba(&s.cesta, e))?;
        if precitane.len() as u64 != s.velkost || balik::odtlacok(&precitane) != s.odtlacok {
            let _ = fs::remove_dir_all(&d);
            return Err(format!("{}: po zápise na kartu nesedí odtlačok (karta?)", s.cesta));
        }
    }
    let mut f = fs::File::create(d.join(POPIS)).map_err(|e| chyba(&POPIS, e))?;
    f.write_all(&st.popis).map_err(|e| chyba(&POPIS, e))?;
    f.sync_all().map_err(|e| chyba(&POPIS, e))?;
    drop(f);
    let mut stav = citaj_stav(koren).unwrap_or_default();
    stav.skusana = Some(v.clone());
    stav.pokusov = 0;
    stav.zlyhane.retain(|z| z != v);
    zapis_stav(koren, &stav).map_err(|e| chyba(&STAV, e))?;
    // staré verzie preč (ostane potvrdená a táto)
    for z in fs::read_dir(koren).into_iter().flatten().flatten() {
        let meno = z.file_name().to_string_lossy().to_string();
        if z.path().is_dir() && Some(meno.as_str()) != stav.dobra.as_deref() && &meno != v {
            let _ = fs::remove_dir_all(z.path());
        }
    }
    Ok(())
}

/// Meranie dobehlo (HOTOVÉ): od teraz sa krabička pýta na nové verzie a smie reštartovať do pripravenej.
pub fn meranie_hotove() {
    NECINNA.store(true, Ordering::Relaxed);
    restartuj_ak_mozno();
}

/// Reštart do pripravenej verzie, keď meranie dobehlo a zberač má všetky záznamy (rad je prázdny).
pub fn restartuj_ak_mozno() {
    let Some(v) = PRIPRAVENA.lock().ok().and_then(|p| p.clone()) else { return };
    if !NECINNA.load(Ordering::Relaxed)
        || !crate::zber::rad_prazdny()
        || RESTART.swap(true, Ordering::Relaxed)
    {
        return;
    }
    let _ = std::thread::Builder::new().name("restart".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        hlas(&format!("aktualizácia (R264): reštart do verzie {v}"));
        std::thread::sleep(Duration::from_secs(1));
        let _ = sys::vypni(true);
    });
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn r264_stav_tam_a_spat() {
        let s = Stav {
            dobra: Some("abc1234".into()),
            skusana: Some("def5678".into()),
            pokusov: 1,
            zlyhane: vec!["0000001".into()],
        };
        assert_eq!(Stav::z_textu(&s.do_textu()), s);
        assert_eq!(Stav::z_textu(""), Stav::default());
        // poškodené hodnoty: neplatná verzia sa zahodí, neplatný počet = vyčerpané pokusy
        let z = Stav::z_textu("skusana=../x\npokusov=?\ndobra=abc1234\n");
        assert_eq!(z.skusana, None);
        assert_eq!(z.pokusov, NAJVIAC_POKUSOV);
        assert_eq!(z.dobra.as_deref(), Some("abc1234"));
    }

    /// Celý život verzie: dva nepotvrdené pokusy → zlyhala → potvrdená predošlá → KLÍČ z karty.
    #[test]
    fn r264_rozhodnutie_startera() {
        let mut s =
            Stav { dobra: Some("dobra01".into()), skusana: Some("nova002".into()), ..Default::default() };
        assert_eq!(rozhodni(&mut s), Volba::NaSkuske("nova002".into(), 1));
        assert_eq!(s.pokusov, 1, "pokus sa zapíše pred spustením");
        assert_eq!(rozhodni(&mut s), Volba::NaSkuske("nova002".into(), 2));
        assert_eq!(rozhodni(&mut s), Volba::Potvrdena("dobra01".into()), "po 2 pokusoch predošlá potvrdená");
        assert_eq!(s.skusana, None);
        assert_eq!(s.zlyhane, ["nova002"]);
        assert_eq!(rozhodni(&mut s), Volba::Potvrdena("dobra01".into()));
        s.zlyhala("dobra01");
        assert_eq!(rozhodni(&mut s), Volba::ZKarty);
        assert_eq!(s.zlyhane, ["nova002", "dobra01"]);
        // pamätá si najviac 16 zlyhaných
        for i in 0..40 {
            s.zlyhala(&format!("v{i:06}"));
        }
        assert_eq!(s.zlyhane.len(), PAMATAJ_ZLYHANE);
        assert_eq!(s.zlyhane.last().map(String::as_str), Some("v000039"));
    }

    #[test]
    fn r264_povolene_ciele_v_pamati() {
        assert_eq!(Path::new("/").join(ciel_v_pamati("init").unwrap()), PathBuf::from(KLIC_V_PAMATI));
        assert_eq!(ciel_v_pamati("usr/bin/hevc_meranie"), Some(PathBuf::from("usr/bin/hevc_meranie")));
        assert_eq!(ciel_v_pamati("etc/passwd"), None);
        assert_eq!(ciel_v_pamati("usr/../etc/x"), None);
        let o = [0u8; 32];
        let b = |subory: &[&str]| Balik {
            druh: "klic".into(),
            arch: arch().into(),
            verzia: "abc1234".into(),
            cas: 1,
            subory: subory
                .iter()
                .map(|c| balik::Subor { cesta: c.to_string(), velkost: 0, odtlacok: o })
                .collect(),
        };
        assert!(over_balik(&b(&["init", "usr/bin/hevc_meranie"]), "abc1234").is_ok());
        assert!(over_balik(&b(&["usr/bin/hevc_meranie"]), "abc1234").is_err(), "bez KLÍČA");
        assert!(over_balik(&b(&["init", "lib/x"]), "abc1234").is_err(), "mimo usr/");
        assert!(over_balik(&b(&["init"]), "inna123").is_err(), "iná verzia");
    }

    fn docasny(meno: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("orchidea-{meno}-{n}"))
    }

    /// Verzia na karte presne tak, ako ju uloží sťahovanie (popis + súbory).
    fn verzia_na_karte(koren: &Path, v: &str, klic: &[u8]) {
        let d = koren.join(v);
        fs::create_dir_all(d.join("usr").join("bin")).unwrap();
        fs::write(d.join("init"), klic).unwrap();
        fs::write(d.join("usr").join("bin").join("hevc_meranie"), b"HEVC").unwrap();
        let b = balik::zbal_priecinok(&d, "klic", arch(), v, 1).unwrap();
        assert_eq!(b.subory.len(), 2);
    }

    /// Štartér bez pripájania: skúšaná verzia sa skopíruje do pamäte a pokus sa zapíše; pokazený súbor na
    /// karte → verzia zlyhala a spustí sa potvrdená; bez potvrdenej KLÍČ z karty.
    #[test]
    fn r264_starter_na_docasnej_karte() {
        let karta = docasny("karta");
        let pamat = docasny("pamat");
        verzia_na_karte(&karta, "dobra01", b"KLIC-DOBRY");
        verzia_na_karte(&karta, "nova002", b"KLIC-NOVY");
        let stav =
            Stav { dobra: Some("dobra01".into()), skusana: Some("nova002".into()), ..Default::default() };
        zapis_stav(&karta, &stav).unwrap();
        let mut hlasenia = Vec::new();
        let (volba, zapis, zlyhanie) = vyber_verziu(&karta, &pamat, &mut |t| hlasenia.push(t)).unwrap();
        assert_eq!(volba.unwrap().0, "skuska nova002 1");
        assert!(zapis.is_ok() && !zlyhanie);
        assert_eq!(fs::read(pamat.join("run").join("orchidea-klic")).unwrap(), b"KLIC-NOVY");
        assert_eq!(fs::read(pamat.join("usr").join("bin").join("hevc_meranie")).unwrap(), b"HEVC");
        assert_eq!(citaj_stav(&karta).unwrap().pokusov, 1, "pokus zapísaný na kartu pred spustením");
        // karta pokazila súbor novej verzie → zlyhala, spustí sa potvrdená
        fs::write(karta.join("nova002").join("init"), b"KLIC-NOVX").unwrap();
        let (volba, _, zlyhanie) = vyber_verziu(&karta, &pamat, &mut |t| hlasenia.push(t)).unwrap();
        assert_eq!(volba.unwrap().0, "dobra dobra01");
        assert!(zlyhanie);
        assert_eq!(fs::read(pamat.join("run").join("orchidea-klic")).unwrap(), b"KLIC-DOBRY");
        let s = citaj_stav(&karta).unwrap();
        assert_eq!((s.skusana, s.zlyhane), (None, vec!["nova002".to_string()]));
        assert!(hlasenia.iter().any(|h| h.contains("nova002") && h.contains("nesedí")), "{hlasenia:?}");
        // aj potvrdená poškodená → KLÍČ z karty, nič sa nespustí
        fs::write(karta.join("dobra01").join(POPIS), b"druh=klic\n").unwrap();
        let (volba, _, zlyhanie) = vyber_verziu(&karta, &pamat, &mut |_| {}).unwrap();
        assert!(volba.is_none() && zlyhanie);
        assert_eq!(citaj_stav(&karta).unwrap().dobra, None);
        // bez stavu štartér nerobí nič
        assert!(vyber_verziu(&pamat, &pamat, &mut |_| {}).is_none());
        let _ = fs::remove_dir_all(&karta);
        let _ = fs::remove_dir_all(&pamat);
    }

    /// Stiahnutý balík sa uloží na kartu, štartér ho spustí na skúšku; staré verzie (okrem potvrdenej) zmiznú.
    #[test]
    fn r264_ulozenie_stiahnuteho_balika() {
        let karta = docasny("karta-uloz");
        let pamat = docasny("pamat-uloz");
        verzia_na_karte(&karta, "dobra01", b"KLIC-DOBRY");
        verzia_na_karte(&karta, "stara00", b"KLIC-STARY");
        zapis_stav(&karta, &Stav { dobra: Some("dobra01".into()), ..Default::default() }).unwrap();
        let data = vec![b"KLIC-STIAHNUTY".to_vec(), b"HEVC2".to_vec()];
        let mut b = Balik {
            druh: "klic".into(),
            arch: arch().into(),
            verzia: "nova003".into(),
            cas: 5,
            subory: vec![],
        };
        for (c, d) in ["init", "usr/bin/hevc_meranie"].iter().zip(&data) {
            b.subory.push(balik::Subor {
                cesta: c.to_string(),
                velkost: d.len() as u64,
                odtlacok: balik::odtlacok(d),
            });
        }
        let st = balik::Stiahnuty { popis: b.do_textu().into_bytes(), balik: b, subory: data };
        uloz(&karta, &st).unwrap();
        let s = citaj_stav(&karta).unwrap();
        assert_eq!(
            (s.dobra.as_deref(), s.skusana.as_deref(), s.pokusov),
            (Some("dobra01"), Some("nova003"), 0)
        );
        assert!(!karta.join("stara00").exists(), "stará verzia zmazaná");
        assert!(karta.join("dobra01").exists(), "potvrdená ostáva ako poistka");
        let (volba, _, _) = vyber_verziu(&karta, &pamat, &mut |_| {}).unwrap();
        assert_eq!(volba.unwrap().0, "skuska nova003 1");
        assert_eq!(fs::read(pamat.join("run").join("orchidea-klic")).unwrap(), b"KLIC-STIAHNUTY");
        let _ = fs::remove_dir_all(&karta);
        let _ = fs::remove_dir_all(&pamat);
    }

    #[test]
    fn r264_rezim_z_prostredia_bez_premennej() {
        // v skúškach premenná nie je → KLÍČ z karty
        assert_eq!(rezim(), Rezim::ZKarty);
        assert!(popis_rezimu().starts_with("KLÍČ z karty"));
    }
}
