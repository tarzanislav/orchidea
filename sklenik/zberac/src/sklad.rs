//! Sklad meraní – priečinok MERANIA na počítači vlastníka (R247).
//!
//! ```text
//! MERANIA\
//!   _prehlad.txt                          ← riadok na každý začiatok a koniec merania
//!   hardkernel-odroid-c4 (odroid-tv)\     ← zariadenie (model a meno zo Zapisovača, inak sériové číslo/MAC)
//!     2026-11-27 17-06 f5cd597\           ← jedno meranie = jeden štart krabičky
//!       01-Hardkernel-ODROID-C4-SD.txt    ← záznamy presne ako na karte (pripájajú sa)
//!       HOTOVE.txt                        ← meranie dobehlo (HOTOVÉ na krabičke)
//!       .beh, .prijate                    ← identita štartu a prijaté záznamy (opakovaný prenos sa nezdvojí)
//! ```

use sklenik_korene::zber::Sprava;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Súbor prehľadu v koreni priečinka MERANIA.
pub const PREHLAD: &str = "_prehlad.txt";

/// Čas pre mená priečinkov a prehľad (rok, mesiac, deň, hodina, minúta, sekunda).
pub type Cas = (i32, u32, u32, u32, u32, u32);

#[derive(Debug, Clone)]
pub struct Sklad {
    pub koren: PathBuf,
}

/// Malé písmená, číslice a pomlčky (model „Hardkernel ODROID-C4“ → `hardkernel-odroid-c4`).
pub fn slug(t: &str) -> String {
    let mut v = String::new();
    for c in t.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            v.push(c);
        } else if !v.ends_with('-') && !v.is_empty() {
            v.push('-');
        }
    }
    while v.ends_with('-') {
        v.pop();
    }
    if v.is_empty() { "zariadenie".into() } else { v.chars().take(48).collect() }
}

/// Meno súboru alebo priečinka bezpečné vo Windows (bez ciest, rezervovaných znakov a koncových bodiek).
pub fn bezpecne_meno(t: &str) -> String {
    let posledne = t.rsplit(['/', '\\']).next().unwrap_or("");
    let mut v: String = posledne
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if "<>:\"|?*".contains(c) { '_' } else { c })
        .take(100)
        .collect();
    while v.ends_with(['.', ' ']) {
        v.pop();
    }
    while v.starts_with('.') {
        v.remove(0);
    }
    if v.is_empty() { "subor.txt".into() } else { v }
}

fn cas_text(c: Cas) -> String {
    format!("{:04}-{:02}-{:02} {:02}:{:02}", c.0, c.1, c.2, c.3, c.4)
}

impl Sklad {
    pub fn novy(koren: impl Into<PathBuf>) -> io::Result<Sklad> {
        let koren = koren.into();
        fs::create_dir_all(&koren)?;
        Ok(Sklad { koren })
    }

    /// Priečinok zariadenia podľa správy.
    pub fn priecinok_zariadenia(&self, s: &Sprava) -> PathBuf {
        let model = slug(s.pole("zariadenie").unwrap_or("zariadenie"));
        let kto =
            s.pole("meno").filter(|m| !m.is_empty() && *m != "–").or(s.pole("id")).unwrap_or("bez mena");
        self.koren.join(bezpecne_meno(&format!("{model} ({kto})")))
    }

    /// Priečinok merania (jeden štart krabičky) – existujúci podľa `.beh`, inak nový. Vráti (cesta, nový?).
    fn priecinok_merania(&self, s: &Sprava, teraz: Cas) -> io::Result<(PathBuf, bool)> {
        let zar = self.priecinok_zariadenia(s);
        fs::create_dir_all(&zar)?;
        let beh = s.pole("beh").unwrap_or("").trim().to_string();
        if !beh.is_empty() {
            for z in fs::read_dir(&zar)?.flatten() {
                if fs::read_to_string(z.path().join(".beh")).is_ok_and(|t| t.trim() == beh) {
                    return Ok((z.path(), false));
                }
            }
        }
        let verzia = bezpecne_meno(s.pole("verzia").unwrap_or("?"));
        let zaklad =
            format!("{:04}-{:02}-{:02} {:02}-{:02} {verzia}", teraz.0, teraz.1, teraz.2, teraz.3, teraz.4);
        let mut cesta = zar.join(&zaklad);
        let mut i = 2;
        while cesta.exists() {
            cesta = zar.join(format!("{zaklad} ({i})"));
            i += 1;
        }
        fs::create_dir_all(&cesta)?;
        fs::write(cesta.join(".beh"), format!("{beh}\n"))?;
        Ok((cesta, true))
    }

    fn do_prehladu(&self, riadok: &str) -> io::Result<()> {
        let mut f = fs::OpenOptions::new().create(true).append(true).open(self.koren.join(PREHLAD))?;
        f.write_all(riadok.as_bytes())?;
        f.write_all(b"\r\n")
    }

    /// Prijme správu krabičky: pripojí záznam do súboru merania (raz – opakovaný prenos sa nezdvojí) a pri
    /// konci merania zapíše HOTOVE.txt a riadok do prehľadu. Vráti cestu priečinka merania a popis pre okno.
    pub fn prijmi(&self, s: &Sprava, teraz: Cas) -> io::Result<(PathBuf, String)> {
        let (meranie, nove) = self.priecinok_merania(s, teraz)?;
        let zar = meranie
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let relativna = meranie.strip_prefix(&self.koren).unwrap_or(&meranie).display().to_string();
        let verzia = s.pole("verzia").unwrap_or("?");
        if nove {
            self.do_prehladu(&format!("{} · {zar} · {verzia} · začiatok · {relativna}", cas_text(teraz)))?;
        }
        let kluc = s.pole("zaznam").unwrap_or("").to_string();
        let prijate_cesta = meranie.join(".prijate");
        let prijate = fs::read_to_string(&prijate_cesta).unwrap_or_default();
        let uz_bolo = !kluc.is_empty() && prijate.lines().any(|r| r == kluc);
        let mut popis = String::new();
        if !uz_bolo {
            if !s.obsah.is_empty() {
                let subor = bezpecne_meno(s.pole("subor").unwrap_or("zaznamy.txt"));
                let mut f = fs::OpenOptions::new().create(true).append(true).open(meranie.join(&subor))?;
                f.write_all(&s.obsah)?;
                f.sync_all()?;
                popis = format!("{} kB do {subor}", s.obsah.len().div_ceil(1024));
            }
            if !kluc.is_empty() {
                let mut f = fs::OpenOptions::new().create(true).append(true).open(&prijate_cesta)?;
                writeln!(f, "{kluc}")?;
            }
        } else {
            popis = "už prijaté".into();
        }
        if s.pole("stav") == Some("hotove") && !meranie.join("HOTOVE.txt").exists() {
            fs::write(
                meranie.join("HOTOVE.txt"),
                format!("meranie dobehlo (HOTOVÉ) {}\r\n", cas_text(teraz)),
            )?;
            self.do_prehladu(&format!("{} · {zar} · {verzia} · HOTOVÉ · {relativna}", cas_text(teraz)))?;
            popis = if popis.is_empty() { "HOTOVÉ".into() } else { format!("{popis} · HOTOVÉ") };
        }
        Ok((meranie, format!("{zar} · {} · {popis}", s.pole("zaznam").unwrap_or("správa"))))
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

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

    fn sprava(zaznam: &str, obsah: &str) -> Sprava {
        Sprava::nova()
            .s("zariadenie", "Hardkernel ODROID-C4")
            .s("meno", "odroid-tv")
            .s("id", "00:1e:06:48:4d:60")
            .s("verzia", "f5cd597")
            .s("beh", "b0a1-77")
            .s("subor", "01-Hardkernel-ODROID-C4-SD.txt")
            .s("zaznam", zaznam)
            .s_obsahom(obsah.as_bytes().to_vec())
    }

    #[test]
    fn mena_su_bezpecne() {
        assert_eq!(slug("Hardkernel ODROID-C4"), "hardkernel-odroid-c4");
        assert_eq!(slug("Raspberry Pi 3 Model B Plus Rev 1.3"), "raspberry-pi-3-model-b-plus-rev-1-3");
        assert_eq!(bezpecne_meno("../../Windows/system32"), "system32");
        assert_eq!(bezpecne_meno("a:b*c?.txt. "), "a_b_c_.txt");
        assert_eq!(bezpecne_meno(".."), "subor.txt");
    }

    #[test]
    fn zaznamy_jedneho_merania_v_jednom_priecinku_bez_zdvojenia() {
        let d = docasny("zaznamy");
        let sk = Sklad::novy(&d).unwrap();
        let t = (2026, 11, 27, 17, 6, 0);
        let (m1, _) = sk.prijmi(&sprava("0", "záznam 0\n"), t).unwrap();
        let (m2, _) = sk.prijmi(&sprava("3", "záznam 3\n"), (2026, 11, 27, 17, 9, 0)).unwrap();
        let (m3, popis) = sk.prijmi(&sprava("3", "záznam 3\n"), t).unwrap();
        assert_eq!(m1, m2);
        assert_eq!(m1, m3);
        assert!(popis.contains("už prijaté"));
        assert!(
            m1.ends_with("hardkernel-odroid-c4 (odroid-tv)/2026-11-27 17-06 f5cd597"),
            "{}",
            m1.display()
        );
        let text = fs::read_to_string(m1.join("01-Hardkernel-ODROID-C4-SD.txt")).unwrap();
        assert_eq!(text, "záznam 0\nzáznam 3\n");
        // koniec merania
        let (_, popis) = sk.prijmi(&sprava("hotove", "").s("stav", "hotove"), t).unwrap();
        assert!(popis.contains("HOTOVÉ"));
        assert!(m1.join("HOTOVE.txt").exists());
        let prehlad = fs::read_to_string(d.join(PREHLAD)).unwrap();
        assert_eq!(prehlad.lines().count(), 2, "{prehlad}");
        assert!(prehlad.contains("· začiatok ·") && prehlad.contains("· HOTOVÉ ·"));
        // nový štart = nové meranie
        let mut s = sprava("0", "druhý štart\n");
        s.polia.retain(|(k, _)| k != "beh");
        let s = s.s("beh", "c2-88");
        let (m4, _) = sk.prijmi(&s, t).unwrap();
        assert_ne!(m4, m1);
        assert!(m4.to_string_lossy().ends_with("f5cd597 (2)"));
        let _ = fs::remove_dir_all(&d);
    }

    /// R253: záverečný záznam nesie aj koniec merania – jeden prenos zapíše text aj HOTOVE.txt; opakovaný
    /// prenos tej istej správy (krabička nedostala potvrdenie) nič nezdvojí.
    #[test]
    fn r253_posledny_zaznam_s_koncom() {
        let d = docasny("koniec");
        let sk = Sklad::novy(&d).unwrap();
        let t = (2026, 11, 27, 18, 33, 0);
        let (m, _) = sk.prijmi(&sprava("0", "z0\n"), t).unwrap();
        let (_, popis) = sk.prijmi(&sprava("3", "z3\n").s("stav", "hotove"), t).unwrap();
        assert!(popis.contains("kB do") && popis.contains("HOTOVÉ"), "{popis}");
        assert!(m.join("HOTOVE.txt").exists());
        let (_, popis) = sk.prijmi(&sprava("3", "z3\n").s("stav", "hotove"), t).unwrap();
        assert!(popis.contains("už prijaté"), "{popis}");
        assert_eq!(fs::read_to_string(m.join("01-Hardkernel-ODROID-C4-SD.txt")).unwrap(), "z0\nz3\n");
        let prehlad = fs::read_to_string(d.join(PREHLAD)).unwrap();
        assert_eq!(prehlad.matches("· HOTOVÉ ·").count(), 1, "{prehlad}");
        let _ = fs::remove_dir_all(&d);
    }
}
