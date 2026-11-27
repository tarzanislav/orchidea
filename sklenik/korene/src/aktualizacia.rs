//! Aktualizácia po sieti (F4a, R264): balík súborov s odtlačkami BLAKE3, ktorý zberač meraní rozošle
//! krabičkám ORCHIDEA OS. Spoločné pre zberač (ponúka a posiela), krabičku (sťahuje, overuje, štartuje) aj
//! nástroj CI, ktorý balík zostaví (`orchidea-zberac --balik`).
//!
//! Vlastník 27. 11. 2026: „chcem túto fázu urýchliť, aby som nemusel stále napaľovať SD karty“.
//!
//! **Popis balíka** (`popis.txt`, UTF-8, riadky `kľúč=hodnota`):
//!
//! ```text
//! druh=klic
//! arch=aarch64
//! verzia=f11cab9
//! cas=1795812345
//! subor=init 4012345 <64 šestnástkových znakov BLAKE3>
//! subor=usr/bin/hevc_meranie 1234567 <BLAKE3>
//! ```
//!
//! `cas` je čas zostavenia (sekundy od 1970, čas commitu) – krabička ani zberač nikdy neprejdú na starší balík.
//! Cesty sú relatívne, len písmená, číslice a `._-/`, bez `..` – zberač z priečinka balíka nič iné nepošle
//! a krabička nič iné nezapíše.

use std::path::Path;

/// Meno popisu v priečinku balíka.
pub const POPIS: &str = "popis.txt";
/// Najväčší kus súboru v jednej odpovedi zberača.
pub const KUS_B: usize = 1 << 20;
/// Najväčší celý balík (súčet súborov) – ochrana pamäte krabičky (RPi 3 B+ má 1 GB).
pub const NAJVIAC_BALIK_B: u64 = 96 << 20;

/// Jeden súbor balíka.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subor {
    /// Relatívna cesta v systéme (`init` = program KLÍČ, inak napr. `usr/bin/hevc_meranie`).
    pub cesta: String,
    pub velkost: u64,
    pub odtlacok: [u8; 32],
}

/// Balík aktualizácie.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Balik {
    /// `klic` (systém v pamäti krabičky) alebo `zberac` (program zberača).
    pub druh: String,
    /// Procesor a systém (`aarch64`, `windows-x86_64`).
    pub arch: String,
    /// Krátky odtlačok commitu (7 znakov).
    pub verzia: String,
    /// Čas zostavenia v sekundách od 1970 (0 = neznámy).
    pub cas: u64,
    pub subory: Vec<Subor>,
}

/// Odtlačok BLAKE3 (vlastná implementácia ORCHIDEE, overená vektormi normy).
pub fn odtlacok(data: &[u8]) -> [u8; 32] {
    orchidea_jadro::odtlacok::Odtlacok::z(data).0
}

/// 32 bajtov ako 64 šestnástkových znakov.
pub fn hex(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 64 šestnástkových znakov na 32 bajtov.
pub fn z_hex(t: &str) -> Option<[u8; 32]> {
    crate::zber::kluc_z_textu(t)
}

/// Je cesta bezpečná (relatívna, bez `..`, len povolené znaky)?
pub fn bezpecna_cesta(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= 200
        && !c.starts_with('/')
        && !c.ends_with('/')
        && c.split('/').all(|k| !k.is_empty() && k != "." && k != "..")
        && c.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
}

/// Je text verzie platný (písmená, číslice, najviac 40 znakov)?
pub fn platna_verzia(v: &str) -> bool {
    !v.is_empty() && v.len() <= 40 && v.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl Balik {
    /// Popis ako text (`popis.txt`).
    pub fn do_textu(&self) -> String {
        let mut t = format!(
            "# ORCHIDEA – balík aktualizácie (R264)\ndruh={}\narch={}\nverzia={}\ncas={}\n",
            self.druh, self.arch, self.verzia, self.cas
        );
        for s in &self.subory {
            t += &format!("subor={} {} {}\n", s.cesta, s.velkost, hex(&s.odtlacok));
        }
        t
    }

    /// Načíta popis; odmietne neplatné cesty, verzie, duplicitné súbory a príliš veľký balík.
    pub fn z_textu(t: &str) -> Result<Balik, String> {
        let mut b = Balik::default();
        for r in t.lines().map(str::trim).filter(|r| !r.is_empty() && !r.starts_with('#')) {
            let (k, h) = r.split_once('=').ok_or_else(|| format!("riadok bez „=“: {r}"))?;
            match k {
                "druh" => b.druh = h.to_string(),
                "arch" => b.arch = h.to_string(),
                "verzia" => b.verzia = h.to_string(),
                "cas" => b.cas = h.parse().map_err(|_| format!("neplatný čas: {h}"))?,
                "subor" => {
                    let p: Vec<&str> = h.split(' ').collect();
                    let [cesta, velkost, odtlacok] = p[..] else {
                        return Err(format!("neplatný súbor: {h}"));
                    };
                    if !bezpecna_cesta(cesta) {
                        return Err(format!("nebezpečná cesta: {cesta}"));
                    }
                    if b.subory.iter().any(|s| s.cesta == cesta) {
                        return Err(format!("súbor dvakrát: {cesta}"));
                    }
                    b.subory.push(Subor {
                        cesta: cesta.to_string(),
                        velkost: velkost.parse().map_err(|_| format!("neplatná veľkosť: {velkost}"))?,
                        odtlacok: z_hex(odtlacok).ok_or_else(|| format!("neplatný odtlačok: {odtlacok}"))?,
                    });
                }
                // neznáme polia (novšie verzie) sa preskočia
                _ => {}
            }
        }
        if b.druh.is_empty() || b.arch.is_empty() || !platna_verzia(&b.verzia) {
            return Err("popis bez druhu, procesora alebo platnej verzie".into());
        }
        if b.subory.is_empty() {
            return Err("balík bez súborov".into());
        }
        if b.spolu() > NAJVIAC_BALIK_B {
            return Err(format!("balík je príliš veľký ({} MB)", b.spolu() >> 20));
        }
        Ok(b)
    }

    /// Súčet veľkostí súborov.
    pub fn spolu(&self) -> u64 {
        self.subory.iter().map(|s| s.velkost).fold(0, u64::saturating_add)
    }

    /// Súbor balíka podľa cesty.
    pub fn subor(&self, cesta: &str) -> Option<&Subor> {
        self.subory.iter().find(|s| s.cesta == cesta)
    }

    /// Je tento balík novší ako zostavenie s časom `cas` a verziou `verzia`? Iná verzia a (ak sú oba časy
    /// známe) neskorší čas. Rovnaká verzia nikdy nie je novšia.
    pub fn novsi_ako(&self, verzia: &str, cas: u64) -> bool {
        self.verzia != verzia && (cas == 0 || self.cas == 0 || self.cas > cas)
    }
}

/// Zostaví balík zo všetkých súborov v priečinku `koren` (okrem popisu) a zapíše `popis.txt` (nástroj CI).
pub fn zbal_priecinok(
    koren: &Path,
    druh: &str,
    arch: &str,
    verzia: &str,
    cas: u64,
) -> std::io::Result<Balik> {
    fn prejdi(koren: &Path, d: &Path, v: &mut Vec<String>) -> std::io::Result<()> {
        for z in std::fs::read_dir(d)? {
            let z = z?;
            let p = z.path();
            if z.file_type()?.is_dir() {
                prejdi(koren, &p, v)?;
            } else {
                let rel = p.strip_prefix(koren).map_err(std::io::Error::other)?;
                let t: Vec<String> =
                    rel.components().map(|k| k.as_os_str().to_string_lossy().to_string()).collect();
                v.push(t.join("/"));
            }
        }
        Ok(())
    }
    let mut cesty = Vec::new();
    prejdi(koren, koren, &mut cesty)?;
    cesty.retain(|c| c != POPIS);
    cesty.sort();
    let mut b =
        Balik { druh: druh.into(), arch: arch.into(), verzia: verzia.into(), cas, subory: Vec::new() };
    for c in cesty {
        if !bezpecna_cesta(&c) {
            return Err(std::io::Error::other(format!("nebezpečná cesta v balíku: {c}")));
        }
        let data = std::fs::read(koren.join(&c))?;
        b.subory.push(Subor { cesta: c, velkost: data.len() as u64, odtlacok: odtlacok(&data) });
    }
    // overí, že popis prejde vlastnou kontrolou (rovnakou, akú urobí zberač aj krabička)
    let text = b.do_textu();
    Balik::z_textu(&text).map_err(std::io::Error::other)?;
    std::fs::write(koren.join(POPIS), text)?;
    Ok(b)
}

/// Stiahnutý balík: popis, jeho presný text a obsah súborov v poradí popisu (všetko overené).
#[derive(Debug, Clone)]
pub struct Stiahnuty {
    pub balik: Balik,
    pub popis: Vec<u8>,
    pub subory: Vec<Vec<u8>>,
}

/// Stiahne balík `druh`-`arch` vo verzii `nova` od zberača. `ziadaj` pošle správu (polia `zaklad` – identita
/// krabičky – plus žiadosť) a vráti bajty odpovede. Každý súbor sa skladá po kusoch [`KUS_B`] a overí sa
/// veľkosť aj odtlačok BLAKE3; čokoľvek nesedí → chyba (nič sa nepoužije). `priebeh` dostane (stiahnuté, spolu).
pub fn stiahni(
    ziadaj: &mut dyn FnMut(crate::zber::Sprava) -> Result<Vec<u8>, String>,
    zaklad: &crate::zber::Sprava,
    druh: &str,
    arch: &str,
    nova: &str,
    priebeh: &mut dyn FnMut(u64, u64),
) -> Result<Stiahnuty, String> {
    let ziadost = |co: &str| zaklad.clone().s("ziadost", co).s("druh", druh).s("nova", nova);
    let popis = ziadaj(ziadost("popis"))?;
    let text = std::str::from_utf8(&popis).map_err(|_| "popis nie je text")?;
    let b = Balik::z_textu(text)?;
    if b.druh != druh || b.arch != arch || b.verzia != nova {
        return Err(format!("zberač poslal iný balík ({}-{} {})", b.druh, b.arch, b.verzia));
    }
    let spolu = b.spolu();
    let mut hotovo = 0u64;
    let mut subory = Vec::with_capacity(b.subory.len());
    for s in &b.subory {
        let mut d: Vec<u8> = Vec::with_capacity(s.velkost as usize);
        while (d.len() as u64) < s.velkost {
            let kus = ziadaj(
                ziadost("kus")
                    .s("subor", &s.cesta)
                    .s("od", &d.len().to_string())
                    .s("kolko", &KUS_B.to_string()),
            )?;
            if kus.is_empty() || kus.len() > KUS_B || d.len() as u64 + kus.len() as u64 > s.velkost {
                return Err(format!("{}: kus nesprávnej dĺžky ({} B)", s.cesta, kus.len()));
            }
            hotovo += kus.len() as u64;
            d.extend_from_slice(&kus);
            priebeh(hotovo, spolu);
        }
        if odtlacok(&d) != s.odtlacok {
            return Err(format!("{}: odtlačok BLAKE3 nesedí s popisom", s.cesta));
        }
        subory.push(d);
    }
    Ok(Stiahnuty { balik: b, popis, subory })
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn popis_tam_a_spat() {
        let b = Balik {
            druh: "klic".into(),
            arch: "aarch64".into(),
            verzia: "f11cab9".into(),
            cas: 1_795_812_345,
            subory: vec![
                Subor { cesta: "init".into(), velkost: 3, odtlacok: odtlacok(b"abc") },
                Subor { cesta: "usr/bin/hevc_meranie".into(), velkost: 0, odtlacok: odtlacok(b"") },
            ],
        };
        assert_eq!(Balik::z_textu(&b.do_textu()).unwrap(), b);
        assert_eq!(b.spolu(), 3);
        assert!(b.novsi_ako("abc1234", 1_795_812_000));
        assert!(!b.novsi_ako("f11cab9", 0), "rovnaká verzia nie je novšia");
        assert!(!b.novsi_ako("abc1234", 1_795_812_999), "starší balík sa nikdy nenainštaluje");
        assert!(b.novsi_ako("vyvojova", 0), "vývojové zostavenie bez času");
    }

    /// BLAKE3 prázdneho vstupu podľa normy (oficiálny vektor).
    #[test]
    fn odtlacok_blake3_norma() {
        assert_eq!(hex(&odtlacok(b"")), "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
    }

    #[test]
    fn nebezpecne_cesty_a_popisy() {
        for c in ["../init", "/init", "usr/../../etc", "usr//bin", "usr/bin/", "a b", "init\0", "", "."] {
            assert!(!bezpecna_cesta(c), "{c:?}");
        }
        for c in ["init", "usr/bin/hevc_meranie", "usr/share/orchidea/hevc/film1080_tazky.hevc"] {
            assert!(bezpecna_cesta(c), "{c:?}");
        }
        let o = hex(&[0; 32]);
        assert!(Balik::z_textu(&format!("druh=klic\narch=aarch64\nverzia=abc\nsubor=../x 1 {o}\n")).is_err());
        assert!(Balik::z_textu(&format!("druh=klic\narch=aarch64\nverzia=a b\nsubor=x 1 {o}\n")).is_err());
        assert!(Balik::z_textu("druh=klic\narch=aarch64\nverzia=abc\n").is_err(), "bez súborov");
        let dvakrat = format!("druh=klic\narch=aarch64\nverzia=abc\nsubor=x 1 {o}\nsubor=x 2 {o}\n");
        assert!(Balik::z_textu(&dvakrat).is_err());
        let velky = format!("druh=klic\narch=aarch64\nverzia=abc\nsubor=x {} {o}\n", NAJVIAC_BALIK_B + 1);
        assert!(Balik::z_textu(&velky).is_err());
    }

    #[test]
    fn zbalenie_priecinka() {
        let zaklad =
            if cfg!(target_family = "wasm") { std::path::PathBuf::from(".") } else { std::env::temp_dir() };
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d = zaklad.join(format!("orchidea-balik-{n}"));
        std::fs::create_dir_all(d.join("usr").join("bin")).unwrap();
        std::fs::write(d.join("init"), b"KLIC").unwrap();
        std::fs::write(d.join("usr").join("bin").join("hevc_meranie"), b"HEVC").unwrap();
        let b = zbal_priecinok(&d, "klic", "aarch64", "abc1234", 7).unwrap();
        let precitany = Balik::z_textu(&std::fs::read_to_string(d.join(POPIS)).unwrap()).unwrap();
        assert_eq!(precitany, b);
        assert_eq!(
            b.subory.iter().map(|s| s.cesta.as_str()).collect::<Vec<_>>(),
            ["init", "usr/bin/hevc_meranie"]
        );
        assert_eq!(b.subor("init").unwrap().odtlacok, odtlacok(b"KLIC"));
        // opakované zbalenie popis nezahrnie
        assert_eq!(zbal_priecinok(&d, "klic", "aarch64", "abc1234", 7).unwrap(), b);
        let _ = std::fs::remove_dir_all(&d);
    }
}
