//! R264 (F4a): zberač rozosiela aktualizácie z priečinka `MERANIA\_aktualizacia` a sám sa aktualizuje (F3).
//!
//! Relácia A dá nový balík do priečinka cez spojenie aplikácie Claude s počítačom vlastníka – netreba internet
//! v zberači ani v krabičkách. Každý balík má vlastný priečinok `<druh>-<procesor>` s popisom `popis.txt`
//! (súbory s odtlačkami BLAKE3):
//!
//! ```text
//! MERANIA\_aktualizacia\
//!   klic-aarch64\              ← systém v pamäti krabičiek (KLÍČ, program merania HEVC, skúšobné toky)
//!     popis.txt  init  usr\bin\hevc_meranie  …
//!   zberac-windows-x86_64\     ← tento program
//!     popis.txt  orchidea-zberac.exe
//! ```
//!
//! Zberač pošle len súbory uvedené v popise a len vtedy, keď sedí veľkosť aj odtlačok (overí sa raz pre každú
//! zmenu súboru). Pokazený alebo neúplne skopírovaný balík sa neponúkne.

use sklenik_korene::aktualizacia::{self, Balik, KUS_B, POPIS};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Podpriečinok MERANIA s balíkmi.
pub const PRIECINOK: &str = "_aktualizacia";

/// Balíky v priečinku `_aktualizacia`.
#[derive(Debug)]
pub struct Ponuky {
    koren: PathBuf,
    /// Overené súbory: cesta → (čas zmeny, veľkosť, odtlačok sedí).
    overene: Mutex<HashMap<PathBuf, (SystemTime, u64, bool)>>,
    /// Komu už bola ktorá verzia ponúknutá (aby okno neopakovalo ten istý riadok).
    ponuknute: Mutex<HashSet<(String, String)>>,
}

fn platny_kus(t: &str) -> bool {
    !t.is_empty() && t.len() <= 40 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Ponuky {
    pub fn novy(merania: &Path) -> Ponuky {
        Ponuky {
            koren: merania.join(PRIECINOK),
            overene: Mutex::new(HashMap::new()),
            ponuknute: Mutex::new(HashSet::new()),
        }
    }

    /// Priečinok balíka `druh`-`arch` (len bezpečné mená).
    pub fn priecinok(&self, druh: &str, arch: &str) -> Option<PathBuf> {
        (platny_kus(druh) && platny_kus(arch)).then(|| self.koren.join(format!("{druh}-{arch}")))
    }

    /// Súbor balíka na disku (cesta z popisu s oddeľovačmi systému).
    fn cesta_suboru(priecinok: &Path, cesta: &str) -> PathBuf {
        cesta.split('/').fold(priecinok.to_path_buf(), |p, k| p.join(k))
    }

    /// Sedí súbor s popisom? Výsledok sa pamätá, kým sa súbor nezmení.
    fn over(&self, subor: &Path, s: &aktualizacia::Subor) -> bool {
        let Ok(m) = std::fs::metadata(subor) else { return false };
        let zmena = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if m.len() != s.velkost {
            return false;
        }
        if let Ok(o) = self.overene.lock()
            && let Some(&(z, v, ok)) = o.get(subor)
            && z == zmena
            && v == m.len()
        {
            return ok;
        }
        let ok = std::fs::read(subor)
            .is_ok_and(|d| d.len() as u64 == s.velkost && aktualizacia::odtlacok(&d) == s.odtlacok);
        if let Ok(mut o) = self.overene.lock() {
            o.insert(subor.to_path_buf(), (zmena, m.len(), ok));
        }
        ok
    }

    /// Balík `druh`-`arch`: `Ok(None)` – žiadny nie je; chyba – popis alebo súbory nesedia (neponúka sa).
    pub fn balik(&self, druh: &str, arch: &str) -> Result<Option<(Balik, PathBuf)>, String> {
        let Some(d) = self.priecinok(druh, arch) else { return Ok(None) };
        let Ok(text) = std::fs::read_to_string(d.join(POPIS)) else { return Ok(None) };
        let b = Balik::z_textu(&text).map_err(|e| format!("{druh}-{arch}\\{POPIS}: {e}"))?;
        if b.druh != druh || b.arch != arch {
            return Err(format!("{druh}-{arch}\\{POPIS}: balík je pre {}-{}", b.druh, b.arch));
        }
        for s in &b.subory {
            if !self.over(&Self::cesta_suboru(&d, &s.cesta), s) {
                return Err(format!("{druh}-{arch}: súbor {} chýba alebo nesedí s popisom", s.cesta));
            }
        }
        Ok(Some((b, d)))
    }

    /// Ponuka pre krabičku: verzia balíka, ak je novší ako to, čo krabička práve spúšťa.
    pub fn ponuka(&self, druh: &str, arch: &str, verzia: &str, cas: u64) -> Option<Balik> {
        match self.balik(druh, arch) {
            Ok(Some((b, _))) if b.novsi_ako(verzia, cas) => Some(b),
            _ => None,
        }
    }

    /// Prvá ponuka danej verzie tomuto zariadeniu? (riadok do okna len raz)
    pub fn prvy_raz(&self, kto: &str, verzia: &str) -> bool {
        self.ponuknute.lock().map(|mut p| p.insert((kto.to_string(), verzia.to_string()))).unwrap_or(false)
    }

    /// Popis balíka (presne text súboru) – len ak je to stále ponúkaná verzia.
    pub fn popis(&self, druh: &str, arch: &str, verzia: &str) -> Result<Vec<u8>, String> {
        let (b, d) = self.balik(druh, arch)?.ok_or("balík už nie je")?;
        if b.verzia != verzia {
            return Err(format!("ponúkaná je už verzia {}", b.verzia));
        }
        std::fs::read(d.join(POPIS)).map_err(|e| e.to_string())
    }

    /// Kus súboru balíka (`od` bajtov od začiatku, najviac [`KUS_B`]).
    pub fn kus(
        &self,
        druh: &str,
        arch: &str,
        verzia: &str,
        cesta: &str,
        od: u64,
        kolko: usize,
    ) -> Result<Vec<u8>, String> {
        let (b, d) = self.balik(druh, arch)?.ok_or("balík už nie je")?;
        if b.verzia != verzia {
            return Err(format!("ponúkaná je už verzia {}", b.verzia));
        }
        let s = b.subor(cesta).ok_or_else(|| format!("súbor {cesta} nie je v balíku"))?;
        if od > s.velkost {
            return Err("začiatok za koncom súboru".into());
        }
        let kolko = kolko.min(KUS_B).min((s.velkost - od) as usize);
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(Self::cesta_suboru(&d, cesta)).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(od)).map_err(|e| e.to_string())?;
        let mut v = vec![0u8; kolko];
        f.read_exact(&mut v).map_err(|e| e.to_string())?;
        Ok(v)
    }
}

/// Krátka verzia (7 znakov) odtlačku zostavenia tohto programu.
pub fn vlastna_verzia() -> String {
    option_env!("GITHUB_SHA").map(|c| c[..c.len().min(7)].to_string()).unwrap_or_else(|| "vyvojova".into())
}

/// Čas zostavenia tohto programu (čas commitu z CI, 0 = neznámy).
pub fn vlastny_cas() -> u64 {
    option_env!("ORCHIDEA_CAS").and_then(|c| c.trim().parse().ok()).unwrap_or(0)
}

/// Procesor a systém tohto programu v mene balíka.
pub fn vlastna_arch() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// F3: ak je v `_aktualizacia\zberac-<systém>-<procesor>` novší zberač, vymení tento program a spustí nový
/// (s rovnakými argumentmi). Vráti popis, čo sa stalo (`None` – nič nové). Pri úspechu sa proces ukončí.
pub fn vymen_sa(ponuky: &Ponuky, hlas: &dyn Fn(&str)) -> Option<String> {
    let arch = vlastna_arch();
    let b = match ponuky.balik("zberac", &arch) {
        Ok(Some((b, d))) => (b, d),
        Ok(None) => return None,
        Err(e) => return Some(format!("aktualizácia zberača: {e}")),
    };
    let (b, d) = b;
    if !b.novsi_ako(&vlastna_verzia(), vlastny_cas()) {
        return None;
    }
    // ochrana pred slučkou: nový program po spustení hlási inú verziu, než je v popise
    let znacka = d.join(".nainstalovane");
    if std::fs::read_to_string(&znacka).is_ok_and(|t| t.trim() == b.verzia) {
        return Some(format!(
            "aktualizácia zberača: verzia {} už bola nainštalovaná, no beží {} – popis balíka nesedí, vymeňte zberač ručne",
            b.verzia,
            vlastna_verzia()
        ));
    }
    let exe_meno = if cfg!(windows) { "orchidea-zberac.exe" } else { "orchidea-zberac" };
    let s = b.subor(exe_meno)?;
    let novy = std::fs::read(Ponuky::cesta_suboru(&d, &s.cesta)).ok()?;
    if aktualizacia::odtlacok(&novy) != s.odtlacok {
        return Some("aktualizácia zberača: súbor nesedí s popisom".into());
    }
    let exe = std::env::current_exe().ok()?;
    let stary = exe.with_extension("stary");
    let _ = std::fs::remove_file(&stary);
    if let Err(e) = std::fs::rename(&exe, &stary) {
        return Some(format!("aktualizácia zberača: program sa nedá premenovať ({e}) – vymeňte ho ručne"));
    }
    if let Err(e) = std::fs::write(&exe, &novy) {
        let _ = std::fs::rename(&stary, &exe);
        return Some(format!("aktualizácia zberača: zápis zlyhal ({e})"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::write(&znacka, format!("{}\n", b.verzia));
    hlas(&format!("zberač sa aktualizuje: {} → {} – spúšťam novú verziu", vlastna_verzia(), b.verzia));
    match std::process::Command::new(&exe).args(std::env::args_os().skip(1)).env(NASLEDNIK, "1").spawn() {
        Ok(_) => {
            // nový zberač čaká, kým tento uvoľní porty
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::process::exit(0);
        }
        Err(e) => {
            let _ = std::fs::remove_file(&exe);
            let _ = std::fs::rename(&stary, &exe);
            let _ = std::fs::remove_file(&znacka);
            Some(format!("aktualizácia zberača: nová verzia sa nespustila ({e}) – beží stará"))
        }
    }
}

/// Premenná prostredia nového zberača po výmene – počká, kým starý uvoľní porty.
pub const NASLEDNIK: &str = "ORCHIDEA_ZBERAC_NASLEDNIK";

/// R265: ostatné programy z balíka zberača (ORCHIDEA Zapisovač `sadic.exe`) vedľa zberača – vždy aktuálne.
/// Vlastník 27. 11.: „Zapisovač by mohol byť tiež tam ako zberač a vždy by bol aktuálny“. Vráti popisy zmien.
pub fn obnov_pomocne(ponuky: &Ponuky) -> Vec<String> {
    let Some(priecinok) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else {
        return Vec::new();
    };
    let vlastne =
        std::env::current_exe().ok().and_then(|e| e.file_name().map(|n| n.to_string_lossy().to_string()));
    obnov_pomocne_v(ponuky, &vlastna_arch(), &priecinok, vlastne.as_deref().unwrap_or(""))
}

/// Jadro [`obnov_pomocne`]: súbory balíka `zberac-<arch>` v koreni balíka (okrem zberača `vlastne`) skopíruje do
/// `ciel`, ak tam chýbajú alebo sa líšia od popisu. Bežiaci program sa len premenuje (Windows to dovolí).
pub fn obnov_pomocne_v(ponuky: &Ponuky, arch: &str, ciel: &Path, vlastne: &str) -> Vec<String> {
    let Ok(Some((b, d))) = ponuky.balik("zberac", arch) else { return Vec::new() };
    let mut zmeny = Vec::new();
    for s in &b.subory {
        let zberac = s.cesta.starts_with("orchidea-zberac");
        if s.cesta.contains('/') || zberac || s.cesta.eq_ignore_ascii_case(vlastne) {
            continue;
        }
        let cesta = ciel.join(&s.cesta);
        let aktualny = std::fs::read(&cesta)
            .is_ok_and(|x| x.len() as u64 == s.velkost && aktualizacia::odtlacok(&x) == s.odtlacok);
        if aktualny {
            continue;
        }
        let r = (|| -> std::io::Result<()> {
            let novy = cesta.with_extension("novy");
            std::fs::copy(Ponuky::cesta_suboru(&d, &s.cesta), &novy)?;
            if cesta.exists() {
                let stary = cesta.with_extension("stary");
                let _ = std::fs::remove_file(&stary);
                std::fs::rename(&cesta, &stary)?;
            }
            std::fs::rename(&novy, &cesta)
        })();
        zmeny.push(match r {
            Ok(()) => format!("{} aktualizovaný na {} (R265)", s.cesta, b.verzia),
            Err(e) => format!("{} sa nepodarilo aktualizovať: {e}", s.cesta),
        });
    }
    zmeny
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn docasny(meno: &str) -> PathBuf {
        let zaklad = if cfg!(target_family = "wasm") { PathBuf::from(".") } else { std::env::temp_dir() };
        let n = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        zaklad.join(format!("orchidea-{meno}-{n}"))
    }

    /// R265: Zapisovač z balíka zberača sa skopíruje vedľa zberača; rovnaký sa znova nekopíruje, zmenený áno.
    #[test]
    fn r265_zapisovac_vedla_zberaca() {
        let m = docasny("pomocne");
        let ciel = docasny("pomocne-ciel");
        std::fs::create_dir_all(&ciel).unwrap();
        let p = Ponuky::novy(&m);
        let d = p.priecinok("zberac", "windows-x86_64").unwrap();
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("orchidea-zberac.exe"), b"ZBERAC").unwrap();
        std::fs::write(d.join("sadic.exe"), b"ZAPISOVAC-1").unwrap();
        aktualizacia::zbal_priecinok(&d, "zberac", "windows-x86_64", "abc1234", 1).unwrap();
        let z = obnov_pomocne_v(&p, "windows-x86_64", &ciel, "orchidea-zberac.exe");
        assert_eq!(z.len(), 1, "{z:?}");
        assert_eq!(std::fs::read(ciel.join("sadic.exe")).unwrap(), b"ZAPISOVAC-1");
        assert!(!ciel.join("orchidea-zberac.exe").exists(), "zberač sa vymieňa inak (vymen_sa)");
        assert!(
            obnov_pomocne_v(&p, "windows-x86_64", &ciel, "orchidea-zberac.exe").is_empty(),
            "už aktuálny"
        );
        std::fs::write(d.join("sadic.exe"), b"ZAPISOVAC-2").unwrap();
        aktualizacia::zbal_priecinok(&d, "zberac", "windows-x86_64", "def5678", 2).unwrap();
        assert_eq!(obnov_pomocne_v(&p, "windows-x86_64", &ciel, "orchidea-zberac.exe").len(), 1);
        assert_eq!(std::fs::read(ciel.join("sadic.exe")).unwrap(), b"ZAPISOVAC-2");
        assert_eq!(std::fs::read(ciel.join("sadic.stary")).unwrap(), b"ZAPISOVAC-1");
        let _ = std::fs::remove_dir_all(&m);
        let _ = std::fs::remove_dir_all(&ciel);
    }

    #[test]
    fn r264_ponuka_len_overeneho_noveho_balika() {
        let m = docasny("ponuky");
        let p = Ponuky::novy(&m);
        assert!(p.balik("klic", "aarch64").unwrap().is_none(), "žiadny balík");
        let d = p.priecinok("klic", "aarch64").unwrap();
        std::fs::create_dir_all(d.join("usr").join("bin")).unwrap();
        std::fs::write(d.join("init"), vec![7u8; KUS_B + 10]).unwrap();
        std::fs::write(d.join("usr").join("bin").join("hevc_meranie"), b"HEVC").unwrap();
        aktualizacia::zbal_priecinok(&d, "klic", "aarch64", "abc1234", 100).unwrap();
        let (b, _) = p.balik("klic", "aarch64").unwrap().unwrap();
        assert_eq!(b.verzia, "abc1234");
        assert!(p.ponuka("klic", "aarch64", "0000000", 50).is_some());
        assert!(p.ponuka("klic", "aarch64", "abc1234", 50).is_none(), "tá istá verzia");
        assert!(p.ponuka("klic", "aarch64", "0000000", 200).is_none(), "krabička má novšiu");
        assert!(p.ponuka("klic", "armv7", "0000000", 0).is_none(), "iný procesor");
        // kusy: prvý celý, druhý zvyšok; za koncom chyba; iná verzia chyba
        assert_eq!(p.kus("klic", "aarch64", "abc1234", "init", 0, KUS_B * 4).unwrap().len(), KUS_B);
        assert_eq!(p.kus("klic", "aarch64", "abc1234", "init", KUS_B as u64, KUS_B).unwrap().len(), 10);
        assert!(p.kus("klic", "aarch64", "abc1234", "init", (KUS_B + 11) as u64, 1).is_err());
        assert!(p.kus("klic", "aarch64", "ffff000", "init", 0, 1).is_err());
        assert!(p.kus("klic", "aarch64", "abc1234", "../popis.txt", 0, 1).is_err());
        assert!(p.popis("klic", "aarch64", "abc1234").unwrap().starts_with(b"# ORCHIDEA"));
        // zmenený súbor po zbalení: balík sa neponúkne
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(d.join("usr").join("bin").join("hevc_meranie"), b"HEVX").unwrap();
        assert!(p.balik("klic", "aarch64").is_err());
        assert!(p.ponuka("klic", "aarch64", "0000000", 50).is_none());
        assert!(p.priecinok("..", "x").is_none() && p.priecinok("klic", "a/b").is_none());
        let _ = std::fs::remove_dir_all(&m);
    }
}
