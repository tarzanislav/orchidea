//! Dôveryhodné koreňové autority pre HTTPS – zoznam programu Mozilla CA ako dáta (A12 krok 5).
//!
//! Súbor `siet/data/korene-mozilla.txt` vytvára `nastroje/korene_mozilla.py` z `certdata.txt` (NSS, MPL 2.0). Je
//! vložený do programu – nový zoznam príde s aktualizáciou KLÍČA po sieti (R264). Vlastník 28. 11. 2026, 10:43:
//! „súhlasím s odporúčaním“ (verejný zoznam Mozilla).

const ZOZNAM: &str = include_str!("../data/korene-mozilla.txt");

/// Koreňová autorita: meno, certifikát DER a „nedôverovať po“ (certifikáty servera vydané neskôr sa odmietnu).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Koren {
    pub meno: String,
    pub der: Vec<u8>,
    pub nedoverovat_po: Option<i64>,
}

/// Base64 (RFC 4648, 4) bez medzier; `None` pri neplatnom znaku alebo výplni.
pub fn z_base64(t: &str) -> Option<Vec<u8>> {
    let hodnota = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let b = t.trim_end_matches('=').as_bytes();
    if !t.len().is_multiple_of(4) || t.len() - b.len() > 2 {
        return None;
    }
    let mut v = Vec::with_capacity(b.len() * 3 / 4);
    for kus in b.chunks(4) {
        let mut x = 0u32;
        for (i, &c) in kus.iter().enumerate() {
            x |= (hodnota(c)? as u32) << (18 - 6 * i);
        }
        let n = kus.len() * 6 / 8;
        v.extend_from_slice(&x.to_be_bytes()[1..1 + n]);
    }
    Some(v)
}

/// Rozbor zoznamu (formát pozri `nastroje/korene_mozilla.py`).
pub fn rozober(text: &str) -> Result<Vec<Koren>, String> {
    let mut v = Vec::new();
    let mut meno = None;
    let mut po = None;
    for (i, r) in text.lines().enumerate() {
        if r.is_empty() || r.starts_with('#') {
            continue;
        }
        if let Some(m) = r.strip_prefix("= ") {
            meno = Some(m.to_string());
            po = None;
        } else if let Some(s) = r.strip_prefix("po ") {
            po = Some(s.parse().map_err(|_| format!("riadok {}: zlý dátum", i + 1))?);
        } else {
            let der = z_base64(r).ok_or(format!("riadok {}: zlý base64", i + 1))?;
            let meno = meno.take().ok_or(format!("riadok {}: certifikát bez mena", i + 1))?;
            v.push(Koren { meno, der, nedoverovat_po: po.take() });
        }
    }
    Ok(v)
}

/// Zoznam programu Mozilla CA vložený do programu.
pub fn mozilla() -> Vec<Koren> {
    rozober(ZOZNAM).unwrap_or_default()
}

/// SHA-256 zdroja (certdata.txt), z ktorého je vložený zoznam – pre záznam a kontrolu v CI.
pub fn zdroj() -> &'static str {
    ZOZNAM.lines().find_map(|r| r.strip_prefix("# SHA-256 zdroja: ")).unwrap_or("?")
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn base64() {
        assert_eq!(z_base64("").unwrap(), b"");
        assert_eq!(z_base64("Zg==").unwrap(), b"f");
        assert_eq!(z_base64("Zm8=").unwrap(), b"fo");
        assert_eq!(z_base64("Zm9vYmFy").unwrap(), b"foobar");
        assert!(z_base64("Zm9").is_none() && z_base64("Zm9v!A==").is_none() && z_base64("Z===").is_none());
    }

    /// Všetky autority Mozilla prečíta náš rozbor X.509 (skúška rozboru na skutočných dátach).
    #[test]
    fn zoznam_mozilla() {
        let k = mozilla();
        assert!(k.len() >= 100, "{}", k.len());
        assert_eq!(zdroj().len(), 64);
        let mut zle = Vec::new();
        for koren in &k {
            if let Err(e) = crate::x509::Certifikat::precitaj(&koren.der) {
                zle.push(format!("{}: {e}", koren.meno));
            }
        }
        assert!(zle.is_empty(), "{zle:#?}");
        let (o, nepreckane) = crate::x509::Overenie::mozilla(1_795_867_200);
        assert_eq!((o.pocet_korenov(), nepreckane), (k.len(), 0));
    }
}
