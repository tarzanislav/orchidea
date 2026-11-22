//! Karta SKLENÍKA: tabuľka oddielov MBR, oddiel FAT32 a súbor nastavenia `SKLENIK.TXT`.
//!
//! Spoločné pre SADIČ (pripraví nastavenie pri zápise) a KLÍČ (prečíta ho pri štarte priamo z karty, len
//! na čítanie, bez pripájania súborového systému – karta sa nikdy nepoškodí).
//!
//! SADIČ obraz nemení na disku počítača – nastavenie zariadenia (meno, Wi-Fi, jazyk) sa pripraví ako
//! **záplaty** (posun v obraze + nové bajty), ktoré sa uplatnia počas zápisu na kartu. Súbor
//! `SKLENIK.TXT` má v obraze pevnú veľkosť, takže sa prepíše na mieste bez zásahu do štruktúry FAT.

use std::io::{self, Read, Seek, SeekFrom};

/// Veľkosť sektora v obraze.
pub const SEKTOR: u64 = 512;
/// Meno súboru nastavenia v adresári FAT (8.3, veľkými písmenami).
const MENO_NASTAVENIA: &[u8; 11] = b"SKLENIK TXT";

/// Zmena obrazu: od posunu `posun` sa zapíšu bajty `data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zaplata {
    pub posun: u64,
    pub data: Vec<u8>,
}

/// Uplatní záplaty na kus obrazu, ktorý začína na posune `zaciatok`.
pub fn uplatni(zaplaty: &[Zaplata], zaciatok: u64, kus: &mut [u8]) {
    let koniec = zaciatok + kus.len() as u64;
    for z in zaplaty {
        let (od, po) = (z.posun.max(zaciatok), (z.posun + z.data.len() as u64).min(koniec));
        if od < po {
            let (i, j) = ((od - zaciatok) as usize, (od - z.posun) as usize);
            let n = (po - od) as usize;
            kus[i..i + n].copy_from_slice(&z.data[j..j + n]);
        }
    }
}

fn citaj_na(z: &mut (impl Read + Seek), posun: u64, n: usize) -> io::Result<Vec<u8>> {
    let mut b = vec![0u8; n];
    z.seek(SeekFrom::Start(posun))?;
    z.read_exact(&mut b)?;
    Ok(b)
}

fn u16_na(b: &[u8], i: usize) -> u64 {
    u16::from_le_bytes([b[i], b[i + 1]]) as u64
}

fn u32_na(b: &[u8], i: usize) -> u64 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as u64
}

/// Oddiel FAT32 v obraze.
#[derive(Debug, Clone, Copy)]
struct Fat32 {
    /// Posun začiatku oddielu (bajty).
    oddiel: u64,
    bps: u64,
    spc: u64,
    rezervovane: u64,
    fat_sektorov: u64,
    pocet_fat: u64,
    korenovy: u64,
}

impl Fat32 {
    fn najdi(z: &mut (impl Read + Seek)) -> Result<Fat32, String> {
        let mbr = citaj_na(z, 0, 512).map_err(|e| format!("obraz sa nedá prečítať: {e}"))?;
        if mbr[510..512] != [0x55, 0xAA] {
            return Err("obraz nemá tabuľku oddielov (MBR)".into());
        }
        let zaznam = (0..4)
            .map(|i| &mbr[446 + 16 * i..462 + 16 * i])
            .find(|p| matches!(p[4], 0x0B | 0x0C))
            .ok_or("obraz nemá oddiel FAT32")?;
        let oddiel = u32_na(zaznam, 8) * SEKTOR;
        let vbr = citaj_na(z, oddiel, 512).map_err(|e| format!("oddiel FAT32 sa nedá prečítať: {e}"))?;
        if &vbr[82..90] != b"FAT32   " || vbr[510..512] != [0x55, 0xAA] {
            return Err("prvý oddiel nie je platný FAT32".into());
        }
        let f = Fat32 {
            oddiel,
            bps: u16_na(&vbr, 11),
            spc: vbr[13] as u64,
            rezervovane: u16_na(&vbr, 14),
            pocet_fat: vbr[16] as u64,
            fat_sektorov: u32_na(&vbr, 36),
            korenovy: u32_na(&vbr, 44),
        };
        if !matches!(f.bps, 512 | 1024 | 2048 | 4096) || f.spc == 0 || f.pocet_fat == 0 {
            return Err("poškodená hlavička FAT32".into());
        }
        Ok(f)
    }

    fn velkost_klastra(&self) -> u64 {
        self.bps * self.spc
    }

    fn klaster(&self, c: u64) -> u64 {
        let data = self.rezervovane + self.pocet_fat * self.fat_sektorov;
        self.oddiel + (data + (c - 2) * self.spc) * self.bps
    }

    fn dalsi(&self, z: &mut (impl Read + Seek), c: u64) -> Result<Option<u64>, String> {
        let posun = self.oddiel + self.rezervovane * self.bps + c * 4;
        let b = citaj_na(z, posun, 4).map_err(|e| format!("FAT sa nedá prečítať: {e}"))?;
        let n = u32_na(&b, 0) & 0x0FFF_FFFF;
        Ok(if (2..0x0FFF_FFF8).contains(&n) { Some(n) } else { None })
    }

    /// Reťaz klastrov od `c` (s ochranou pred zacyklením).
    fn retaz(&self, z: &mut (impl Read + Seek), mut c: u64) -> Result<Vec<u64>, String> {
        let mut out = vec![c];
        while let Some(n) = self.dalsi(z, c)? {
            if out.len() > 1 << 20 {
                return Err("reťaz klastrov FAT je zacyklená".into());
            }
            out.push(n);
            c = n;
        }
        Ok(out)
    }
}

/// Nájde súbor nastavenia v koreňovom adresári; vráti jeho klastre a veľkosť.
fn najdi_nastavenie(z: &mut (impl Read + Seek), f: &Fat32) -> Result<(Vec<u64>, u64), String> {
    for c in f.retaz(z, f.korenovy)? {
        let blok = citaj_na(z, f.klaster(c), f.velkost_klastra() as usize).map_err(|e| e.to_string())?;
        for zaznam in blok.chunks_exact(32) {
            match zaznam[0] {
                0x00 => return Err("obraz neobsahuje súbor SKLENIK.TXT (nie je to obraz SKLENÍKA?)".into()),
                0xE5 => continue,
                _ => {}
            }
            if zaznam[11] & 0x18 == 0 && &zaznam[0..11] == MENO_NASTAVENIA {
                let prvy = (u16_na(zaznam, 20) << 16) | u16_na(zaznam, 26);
                let velkost = u32_na(zaznam, 28);
                return Ok((f.retaz(z, prvy)?, velkost));
            }
        }
    }
    Err("obraz neobsahuje súbor SKLENIK.TXT".into())
}

/// Záplaty, ktoré do súboru `SKLENIK.TXT` zapíšu `text` (doplnený medzerami na pôvodnú veľkosť).
pub fn zaplaty_nastavenia(z: &mut (impl Read + Seek), text: &str) -> Result<Vec<Zaplata>, String> {
    let f = Fat32::najdi(z)?;
    let (klastre, velkost) = najdi_nastavenie(z, &f)?;
    let mut obsah = text.as_bytes().to_vec();
    if obsah.len() + 1 > velkost as usize {
        return Err(format!("nastavenie má {} B, miesto v obraze je {velkost} B", obsah.len()));
    }
    obsah.resize(velkost as usize - 1, b' ');
    obsah.push(b'\n');
    let k = f.velkost_klastra() as usize;
    Ok(obsah
        .chunks(k)
        .zip(klastre)
        .map(|(kus, c)| Zaplata { posun: f.klaster(c), data: kus.to_vec() })
        .collect())
}

/// Prečíta obsah súboru nastavenia (na kontrolu po zápise alebo pri `priprav`).
pub fn citaj_nastavenie(z: &mut (impl Read + Seek)) -> Result<String, String> {
    let f = Fat32::najdi(z)?;
    let (klastre, velkost) = najdi_nastavenie(z, &f)?;
    let mut obsah = Vec::new();
    for c in klastre {
        obsah.extend(citaj_na(z, f.klaster(c), f.velkost_klastra() as usize).map_err(|e| e.to_string())?);
    }
    obsah.truncate(velkost as usize);
    Ok(String::from_utf8_lossy(&obsah).trim_end().to_string())
}

/// Nastavenie zariadenia.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Nastavenie {
    pub meno: Option<String>,
    pub wifi: Option<String>,
    pub wifi_heslo: Option<String>,
    pub jazyk: Option<String>,
    /// Samomeranie pri štarte (R122): `Some(false)` = vypnuté; inak zapnuté.
    pub meranie: Option<bool>,
}

/// Hodnota áno/nie (`ano`, `áno`, `1`, `zapnute` / `nie`, `0`, `vypnute`).
pub fn ano_nie(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "ano" | "áno" | "1" | "zapnute" | "zapnuté" | "yes" | "on" => Some(true),
        "nie" | "0" | "vypnute" | "vypnuté" | "no" | "off" => Some(false),
        _ => None,
    }
}

impl Nastavenie {
    /// Z argumentov tvaru `kľúč=hodnota`.
    pub fn z_argumentov<'a>(argumenty: impl Iterator<Item = &'a str>) -> Result<Nastavenie, String> {
        let mut n = Nastavenie::default();
        for a in argumenty {
            let (k, v) =
                a.split_once('=').ok_or_else(|| format!("nastavenie „{a}“ má mať tvar kľúč=hodnota"))?;
            let v = Some(v.to_string());
            match k {
                "meno" => n.meno = v,
                "wifi" => n.wifi = v,
                "wifi_heslo" => n.wifi_heslo = v,
                "jazyk" => n.jazyk = v,
                "meranie" => {
                    n.meranie = Some(
                        ano_nie(v.as_deref().unwrap_or(""))
                            .ok_or_else(|| "meranie: ano alebo nie".to_string())?,
                    )
                }
                _ => {
                    return Err(format!(
                        "neznáme nastavenie „{k}“ (povolené: meno, wifi, wifi_heslo, jazyk, meranie)"
                    ));
                }
            }
        }
        n.over()?;
        Ok(n)
    }

    pub fn je_prazdne(&self) -> bool {
        *self == Nastavenie::default()
    }

    fn over(&self) -> Result<(), String> {
        if let Some(m) = &self.meno
            && !platne_meno(m)
        {
            return Err(
                "meno zariadenia: 1 až 32 znakov a–z, 0–9 a pomlčka (nie na začiatku ani na konci)".into()
            );
        }
        if let Some(w) = &self.wifi
            && (w.is_empty() || w.len() > 32 || w.contains(['\n', '\r']))
        {
            return Err("názov Wi-Fi: 1 až 32 bajtov, bez nových riadkov".into());
        }
        match (&self.wifi, &self.wifi_heslo) {
            (None, Some(_)) => return Err("heslo Wi-Fi bez názvu siete".into()),
            (_, Some(h)) if !(8..=63).contains(&h.len()) || !h.bytes().all(|c| (0x20..0x7F).contains(&c)) => {
                return Err("heslo Wi-Fi (WPA2): 8 až 63 znakov ASCII".into());
            }
            _ => {}
        }
        if let Some(j) = &self.jazyk
            && !(2..=5).contains(&j.len())
        {
            return Err("jazyk: kód ako sk, en, cs".into());
        }
        Ok(())
    }

    /// Text súboru `SKLENIK.TXT`. Heslo Wi-Fi sa neukladá – len odvodený kľúč WPA (PSK).
    pub fn text(&self) -> String {
        let mut t = String::from("# SKLENÍK – nastavenie zariadenia (zapísal SADIČ)\n");
        if let Some(m) = &self.meno {
            t += &format!("meno={m}\n");
        }
        if let Some(w) = &self.wifi {
            t += &format!("wifi={w}\n");
            if let Some(h) = &self.wifi_heslo {
                let psk = wpa_psk(h, w);
                t += "wifi_psk=";
                t.extend(psk.iter().map(|b| format!("{b:02x}")));
                t += "\n";
            }
        }
        if let Some(j) = &self.jazyk {
            t += &format!("jazyk={j}\n");
        }
        if let Some(m) = self.meranie {
            t += &format!("meranie={}\n", if m { "ano" } else { "nie" });
        }
        t
    }
}

/// Nastavenie, ako ho prečíta zariadenie zo súboru `SKLENIK.TXT` (heslo Wi-Fi tam nikdy nie je – len kľúč).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NastavenieZariadenia {
    pub meno: Option<String>,
    pub wifi: Option<String>,
    pub wifi_psk: Option<[u8; 32]>,
    pub jazyk: Option<String>,
    /// Samomeranie pri štarte (R122): `Some(false)` = vypnuté, inak zapnuté.
    pub meranie: Option<bool>,
}

impl NastavenieZariadenia {
    /// Prečíta text súboru. Súbor sa dá upraviť aj ručne, preto sa neplatné riadky len vynechajú
    /// a vrátia sa ako upozornenia.
    pub fn z_textu(text: &str) -> (NastavenieZariadenia, Vec<String>) {
        let mut n = NastavenieZariadenia::default();
        let mut upozornenia = Vec::new();
        for (i, riadok) in text.lines().enumerate() {
            let riadok = riadok.trim();
            if riadok.is_empty() || riadok.starts_with('#') {
                continue;
            }
            let Some((k, v)) = riadok.split_once('=') else {
                upozornenia.push(format!("riadok {}: chýba „=“", i + 1));
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "meno" if platne_meno(v) => n.meno = Some(v.to_string()),
                "wifi" if !v.is_empty() && v.len() <= 32 => n.wifi = Some(v.to_string()),
                "wifi_psk" => match hex32(v) {
                    Some(k) => n.wifi_psk = Some(k),
                    None => upozornenia
                        .push(format!("riadok {}: kľúč Wi-Fi má mať 64 šestnástkových znakov", i + 1)),
                },
                "jazyk"
                    if (2..=5).contains(&v.len())
                        && v.bytes().all(|c| c.is_ascii_alphabetic() || c == b'-') =>
                {
                    n.jazyk = Some(v.to_ascii_lowercase())
                }
                "meranie" => match ano_nie(v) {
                    Some(m) => n.meranie = Some(m),
                    None => upozornenia.push(format!("riadok {}: meranie má byť ano alebo nie", i + 1)),
                },
                "meno" | "wifi" | "jazyk" => {
                    upozornenia.push(format!("riadok {}: neplatná hodnota „{k}“", i + 1))
                }
                _ => upozornenia.push(format!("riadok {}: neznáme nastavenie „{k}“", i + 1)),
            }
        }
        (n, upozornenia)
    }
}

/// Meno zariadenia: 1 až 32 znakov a–z, 0–9 a pomlčka (platí aj ako meno v sieti).
pub fn platne_meno(m: &str) -> bool {
    (1..=32).contains(&m.len())
        && m.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !m.starts_with('-')
        && !m.ends_with('-')
}

/// Z ľubovoľného textu urobí platné meno zariadenia: malé písmená bez diakritiky, medzery na pomlčky
/// („Dielňa č. 1“ → „dielna-c-1“). Prázdny výsledok = žiadne meno.
pub fn navrhni_meno(text: &str) -> String {
    let mut m = String::new();
    for c in text.trim().to_lowercase().chars() {
        let z = match c {
            'á' | 'ä' => 'a',
            'č' => 'c',
            'ď' => 'd',
            'é' | 'ě' => 'e',
            'í' => 'i',
            'ĺ' | 'ľ' => 'l',
            'ň' => 'n',
            'ó' | 'ô' | 'ö' => 'o',
            'ŕ' | 'ř' => 'r',
            'š' => 's',
            'ť' => 't',
            'ú' | 'ů' | 'ü' => 'u',
            'ý' => 'y',
            'ž' => 'z',
            'a'..='z' | '0'..='9' => c,
            _ => '-',
        };
        if z == '-' && (m.is_empty() || m.ends_with('-')) {
            continue;
        }
        m.push(z);
    }
    let mut m = m.trim_end_matches('-').to_string();
    m.truncate(32);
    m.trim_end_matches('-').to_string()
}

fn hex32(v: &str) -> Option<[u8; 32]> {
    if v.len() != 64 || !v.is_ascii() {
        return None;
    }
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&v[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(k)
}

// ---- WPA2: kľúč z hesla a názvu siete (PBKDF2-HMAC-SHA1, 4096 opakovaní, IEEE 802.11i) ----

/// SHA-1 (len pre WPA2, kde ho predpisuje norma).
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut m = data.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for blok in m.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([blok[4 * i], blok[4 * i + 1], blok[4 * i + 2], blok[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..20 => ((b & c) | (!b & d), 0x5A82_7999),
                20..40 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

fn hmac_sha1(kluc: &[u8], sprava: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if kluc.len() > 64 {
        k[..20].copy_from_slice(&sha1(kluc));
    } else {
        k[..kluc.len()].copy_from_slice(kluc);
    }
    let mut vnutorny: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    vnutorny.extend_from_slice(sprava);
    let mut vonkajsi: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    vonkajsi.extend_from_slice(&sha1(&vnutorny));
    sha1(&vonkajsi)
}

/// Kľúč WPA2-PSK (32 bajtov) z hesla a názvu siete.
pub fn wpa_psk(heslo: &str, ssid: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (blok, kus) in out.chunks_mut(20).enumerate() {
        let mut s = ssid.as_bytes().to_vec();
        s.extend_from_slice(&(blok as u32 + 1).to_be_bytes());
        let mut u = hmac_sha1(heslo.as_bytes(), &s);
        let mut t = u;
        for _ in 1..4096 {
            u = hmac_sha1(heslo.as_bytes(), &u);
            for (x, y) in t.iter_mut().zip(u) {
                *x ^= y;
            }
        }
        kus.copy_from_slice(&t[..kus.len()]);
    }
    out
}

// ---- skúšobný obraz (rovnaká štruktúra ako obraz SKLENÍKA): MBR + FAT32 so SKLENIK.TXT ----

/// Vytvorí skúšobný obraz s oddielom FAT32 (klaster 512 B) a súborom `SKLENIK.TXT` (4 KiB).
pub fn skusobny_obraz(velkost_mib: u64) -> Vec<u8> {
    let celkom = velkost_mib * 1024 * 1024 / SEKTOR;
    let start = 2048u64;
    let oddiel = celkom - start;
    let rezervovane = 32u64;
    let mut fat = 1u64;
    loop {
        let klastrov = oddiel - rezervovane - 2 * fat;
        let potrebne = ((klastrov + 2) * 4).div_ceil(SEKTOR);
        if potrebne <= fat {
            break;
        }
        fat = potrebne;
    }
    let mut o = vec![0u8; (celkom * SEKTOR) as usize];
    // MBR
    let p = &mut o[446..462];
    p[0] = 0x80;
    p[4] = 0x0C;
    p[8..12].copy_from_slice(&(start as u32).to_le_bytes());
    p[12..16].copy_from_slice(&(oddiel as u32).to_le_bytes());
    o[510] = 0x55;
    o[511] = 0xAA;
    // hlavička FAT32
    let v = (start * SEKTOR) as usize;
    let b = &mut o[v..v + 512];
    b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    b[3..11].copy_from_slice(b"SADIC   ");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[13] = 1;
    b[14..16].copy_from_slice(&(rezervovane as u16).to_le_bytes());
    b[16] = 2;
    b[21] = 0xF8;
    b[24..26].copy_from_slice(&32u16.to_le_bytes());
    b[26..28].copy_from_slice(&64u16.to_le_bytes());
    b[28..32].copy_from_slice(&(start as u32).to_le_bytes());
    b[32..36].copy_from_slice(&(oddiel as u32).to_le_bytes());
    b[36..40].copy_from_slice(&(fat as u32).to_le_bytes());
    b[44..48].copy_from_slice(&2u32.to_le_bytes());
    b[48..50].copy_from_slice(&1u16.to_le_bytes());
    b[50..52].copy_from_slice(&6u16.to_le_bytes());
    b[64] = 0x80;
    b[66] = 0x29;
    b[67..71].copy_from_slice(&0x5AD1_C001u32.to_le_bytes());
    b[71..82].copy_from_slice(b"SKLENIK    ");
    b[82..90].copy_from_slice(b"FAT32   ");
    b[510] = 0x55;
    b[511] = 0xAA;
    let kopia: Vec<u8> = o[v..v + 512].to_vec();
    o[v + 6 * 512..v + 7 * 512].copy_from_slice(&kopia);
    // FSInfo
    let fs = v + 512;
    o[fs..fs + 4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    o[fs + 484..fs + 488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    o[fs + 488..fs + 496].copy_from_slice(&[0xFF; 8]);
    o[fs + 508..fs + 512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    // FAT: 0, 1 rezervované; 2 = koreňový adresár; 3..=10 = SKLENIK.TXT (8 klastrov = 4 KiB)
    let zapis_fat = |o: &mut Vec<u8>, c: u64, h: u32| {
        for k in 0..2 {
            let i = (v as u64 + (rezervovane + k * fat) * SEKTOR + c * 4) as usize;
            o[i..i + 4].copy_from_slice(&h.to_le_bytes());
        }
    };
    zapis_fat(&mut o, 0, 0x0FFF_FFF8);
    zapis_fat(&mut o, 1, 0x0FFF_FFFF);
    zapis_fat(&mut o, 2, 0x0FFF_FFFF);
    for c in 3..=10u64 {
        zapis_fat(&mut o, c, if c == 10 { 0x0FFF_FFFF } else { c as u32 + 1 });
    }
    let data = v as u64 + (rezervovane + 2 * fat) * SEKTOR;
    let koren = data as usize;
    o[koren..koren + 11].copy_from_slice(b"SKLENIK    ");
    o[koren + 11] = 0x08;
    let e = koren + 32;
    o[e..e + 11].copy_from_slice(MENO_NASTAVENIA);
    o[e + 11] = 0x20;
    o[e + 26..e + 28].copy_from_slice(&3u16.to_le_bytes());
    o[e + 28..e + 32].copy_from_slice(&4096u32.to_le_bytes());
    let mut obsah = b"# SKLENIK - nastavenie zariadenia (prazdne)\n".to_vec();
    obsah.resize(4095, b' ');
    obsah.push(b'\n');
    let s = (data + SEKTOR) as usize;
    o[s..s + 4096].copy_from_slice(&obsah);
    o
}

#[cfg(test)]
mod skusky {
    use super::*;
    use std::io::Cursor;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_a_wpa_podla_noriem() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        // IEEE 802.11i, príloha H.4
        assert_eq!(
            hex(&wpa_psk("password", "IEEE")),
            "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"
        );
        assert_eq!(
            hex(&wpa_psk("ThisIsAPassword", "ThisIsASSID")),
            "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af"
        );
    }

    #[test]
    fn nastavenie_sa_zapise_do_obrazu_na_mieste() {
        let obraz = skusobny_obraz(64);
        let mut z = Cursor::new(obraz.clone());
        assert!(citaj_nastavenie(&mut z).unwrap().contains("prazdne"));
        let n = Nastavenie::z_argumentov(
            ["meno=dielna-1", "wifi=Domov", "wifi_heslo=tajne-heslo", "jazyk=sk"].into_iter(),
        )
        .unwrap();
        let zaplaty = zaplaty_nastavenia(&mut z, &n.text()).unwrap();
        // záplaty uplatnené po kúskoch (ako pri zápise na kartu) dajú rovnaký výsledok ako naraz
        let mut po_kuskoch = obraz.clone();
        for (i, kus) in po_kuskoch.chunks_mut(3000).enumerate() {
            uplatni(&zaplaty, i as u64 * 3000, kus);
        }
        let mut naraz = obraz.clone();
        uplatni(&zaplaty, 0, &mut naraz);
        assert_eq!(po_kuskoch, naraz);
        let text = citaj_nastavenie(&mut Cursor::new(naraz.clone())).unwrap();
        assert!(text.contains("meno=dielna-1") && text.contains("jazyk=sk"), "{text}");
        assert!(text.contains(&format!("wifi_psk={}", hex(&wpa_psk("tajne-heslo", "Domov")))));
        assert!(!text.contains("tajne-heslo"), "heslo sa nesmie uložiť");
        // mení sa len súbor nastavenia
        let rozdiel = obraz.iter().zip(&naraz).filter(|(a, b)| a != b).count();
        assert!(rozdiel > 0 && rozdiel <= 4096);
    }

    #[test]
    fn chybne_vstupy_sa_odmietnu() {
        assert!(Nastavenie::z_argumentov(["meno=Veľké"].into_iter()).is_err());
        assert!(Nastavenie::z_argumentov(["wifi_heslo=12345678"].into_iter()).is_err(), "heslo bez siete");
        assert!(Nastavenie::z_argumentov(["wifi=a", "wifi_heslo=kratke"].into_iter()).is_err());
        assert!(Nastavenie::z_argumentov(["farba=modra"].into_iter()).is_err());
        let mut zly = vec![0u8; 1 << 20];
        assert!(zaplaty_nastavenia(&mut Cursor::new(&mut zly), "x").is_err(), "obraz bez MBR");
        let dlhe = "x".repeat(5000);
        assert!(zaplaty_nastavenia(&mut Cursor::new(skusobny_obraz(64)), &dlhe).is_err());
    }

    #[test]
    fn zariadenie_precita_nastavenie_od_sadica_aj_rucne_upravene() {
        let n = Nastavenie {
            meno: Some("dielna-1".into()),
            wifi: Some("Domov".into()),
            wifi_heslo: Some("tajneheslo".into()),
            jazyk: Some("sk".into()),
            meranie: Some(false),
        };
        let mut text = n.text();
        assert!(text.contains("meranie=nie\n"));
        text += "   \n# poznámka\nfarba=zelena\nmeno-zle\n";
        let (z, upozornenia) = NastavenieZariadenia::z_textu(&text);
        assert_eq!(z.meno.as_deref(), Some("dielna-1"));
        assert_eq!(z.wifi.as_deref(), Some("Domov"));
        assert_eq!(z.wifi_psk, Some(wpa_psk("tajneheslo", "Domov")));
        assert_eq!(z.jazyk.as_deref(), Some("sk"));
        assert_eq!(z.meranie, Some(false));
        assert_eq!(upozornenia.len(), 2, "{upozornenia:?}");
        // samomeranie (R122): áno/nie aj ručne, neplatná hodnota je len upozornenie
        assert_eq!(NastavenieZariadenia::z_textu("meranie=Áno").0.meranie, Some(true));
        let (z, u) = NastavenieZariadenia::z_textu("meranie=možno");
        assert_eq!((z.meranie, u.len()), (None, 1));
        assert_eq!(Nastavenie::z_argumentov(["meranie=nie"].into_iter()).map(|n| n.meranie), Ok(Some(false)));
        assert!(Nastavenie::z_argumentov(["meranie=snad"].into_iter()).is_err());
        assert!(!text.contains("tajneheslo"));
        // neplatné meno sa nepoužije
        let (z, u) = NastavenieZariadenia::z_textu("meno=Zlé Meno\njazyk=CS\nwifi_psk=12\n");
        assert_eq!(z.meno, None);
        assert_eq!(z.jazyk.as_deref(), Some("cs"));
        assert_eq!(z.wifi_psk, None);
        assert_eq!(u.len(), 2, "{u:?}");
        assert!(!platne_meno("-pi") && !platne_meno("pi-") && platne_meno("pi-1"));
    }

    #[test]
    fn navrh_mena_zariadenia() {
        assert_eq!(navrhni_meno("Dielňa č. 1"), "dielna-c-1");
        assert_eq!(navrhni_meno("  Ťažký kôň_ŽLTÝ  "), "tazky-kon-zlty");
        assert_eq!(navrhni_meno("---"), "");
        assert_eq!(navrhni_meno("pi5"), "pi5");
        let dlhe = navrhni_meno(&"a".repeat(40));
        assert_eq!(dlhe.len(), 32);
        for t in ["Dielňa č. 1", "x y", "Ů", &"ab-".repeat(20)] {
            let m = navrhni_meno(t);
            assert!(m.is_empty() || platne_meno(&m), "{t} → {m}");
        }
    }
}
