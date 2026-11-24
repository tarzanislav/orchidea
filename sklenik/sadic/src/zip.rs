//! Obraz priamo zo stiahnutého súboru ZIP (napr. výstup CI z GitHubu) – vlastné rozbalenie.
//!
//! Prečíta centrálny adresár ZIP (aj ZIP64), nájde v ňom obraz `.img` a rozbalí ho do dočasného súboru
//! (metóda „uložené“ alebo „deflate“ podľa RFC 1951) s kontrolou CRC-32. Používateľ tak nemusí nič
//! rozbaľovať ručne.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

fn chyba(text: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text.into())
}

// ---- CRC-32 (IEEE 802.3, obrátený polynóm 0xEDB88320) ----

fn tabulka_crc() -> [u32; 256] {
    std::array::from_fn(|i| {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        c
    })
}

/// Priebežný výpočet CRC-32.
pub struct Crc32 {
    t: [u32; 256],
    c: u32,
}

impl Crc32 {
    pub fn novy() -> Crc32 {
        Crc32 { t: tabulka_crc(), c: 0xFFFF_FFFF }
    }
    pub fn pridaj(&mut self, data: &[u8]) {
        for &b in data {
            self.c = self.t[((self.c ^ b as u32) & 0xFF) as usize] ^ (self.c >> 8);
        }
    }
    pub fn hodnota(&self) -> u32 {
        !self.c
    }
}

// ---- rozbalenie DEFLATE (RFC 1951) ----

/// Kanonický Huffmanov kód: počty kódov danej dĺžky a symboly zoradené podľa kódu.
struct Kod {
    pocty: [u16; 16],
    symboly: Vec<u16>,
}

impl Kod {
    fn z_dlzok(dlzky: &[u8]) -> io::Result<Kod> {
        let mut pocty = [0u16; 16];
        for &d in dlzky {
            pocty[d as usize] += 1;
        }
        // kontrola, že kód nie je preplnený
        let mut zostava: i32 = 1;
        for &p in &pocty[1..] {
            zostava = (zostava << 1) - p as i32;
            if zostava < 0 {
                return Err(chyba("ZIP: neplatný Huffmanov kód"));
            }
        }
        let mut posuny = [0u16; 16];
        for d in 1..15 {
            posuny[d + 1] = posuny[d] + pocty[d];
        }
        let mut symboly = vec![0u16; dlzky.len()];
        for (s, &d) in dlzky.iter().enumerate() {
            if d != 0 {
                symboly[posuny[d as usize] as usize] = s as u16;
                posuny[d as usize] += 1;
            }
        }
        pocty[0] = 0;
        Ok(Kod { pocty, symboly })
    }
}

struct Citac<'a> {
    data: &'a [u8],
    poz: usize,
    zasoba: u64,
    bitov: u32,
}

impl Citac<'_> {
    fn bity(&mut self, n: u32) -> io::Result<u32> {
        while self.bitov < n {
            let b = *self.data.get(self.poz).ok_or_else(|| chyba("ZIP: údaje sú skrátené"))?;
            self.poz += 1;
            self.zasoba |= (b as u64) << self.bitov;
            self.bitov += 8;
        }
        let v = (self.zasoba & ((1u64 << n) - 1)) as u32;
        self.zasoba >>= n;
        self.bitov -= n;
        Ok(v)
    }

    fn symbol(&mut self, k: &Kod) -> io::Result<u16> {
        let (mut kod, mut prvy, mut index) = (0i32, 0i32, 0i32);
        for d in 1..16 {
            kod |= self.bity(1)? as i32;
            let pocet = k.pocty[d] as i32;
            if kod - pocet < prvy {
                return Ok(k.symboly[(index + kod - prvy) as usize]);
            }
            index += pocet;
            prvy = (prvy + pocet) << 1;
            kod <<= 1;
        }
        Err(chyba("ZIP: neplatný kód v údajoch"))
    }
}

const DLZKA_ZAKLAD: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
const DLZKA_NAVYSE: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const VZDIAL_ZAKLAD: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
const VZDIAL_NAVYSE: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const PORADIE_DLZOK: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Výstup s oknom 32 KB pre spätné odkazy; hotové údaje odchádzajú do `ciel`.
struct Vystup<'a, W: Write> {
    buf: Vec<u8>,
    ciel: &'a mut W,
    crc: Crc32,
    celkom: u64,
}

impl<W: Write> Vystup<'_, W> {
    const OKNO: usize = 32 * 1024;

    fn odosli(&mut self, vsetko: bool) -> io::Result<()> {
        let ponechaj = if vsetko { 0 } else { Self::OKNO };
        if self.buf.len() > ponechaj {
            let n = self.buf.len() - ponechaj;
            self.ciel.write_all(&self.buf[..n])?;
            self.crc.pridaj(&self.buf[..n]);
            self.celkom += n as u64;
            self.buf.drain(..n);
        }
        Ok(())
    }

    fn bajt(&mut self, b: u8) -> io::Result<()> {
        self.buf.push(b);
        if self.buf.len() >= 1 << 20 {
            self.odosli(false)?;
        }
        Ok(())
    }

    fn kopia(&mut self, vzdialenost: usize, dlzka: usize) -> io::Result<()> {
        if vzdialenost > self.buf.len() {
            return Err(chyba("ZIP: odkaz pred začiatok údajov"));
        }
        let start = self.buf.len() - vzdialenost;
        for i in 0..dlzka {
            let b = self.buf[start + i];
            self.buf.push(b);
        }
        if self.buf.len() >= 1 << 20 {
            self.odosli(false)?;
        }
        Ok(())
    }
}

fn blok_kodmi<W: Write>(c: &mut Citac, v: &mut Vystup<W>, lit: &Kod, vzd: &Kod) -> io::Result<()> {
    loop {
        let s = c.symbol(lit)? as usize;
        match s {
            0..=255 => v.bajt(s as u8)?,
            256 => return Ok(()),
            257..=285 => {
                let i = s - 257;
                let dlzka = DLZKA_ZAKLAD[i] as usize + c.bity(DLZKA_NAVYSE[i] as u32)? as usize;
                let j = c.symbol(vzd)? as usize;
                if j >= 30 {
                    return Err(chyba("ZIP: neplatná vzdialenosť"));
                }
                let vzdialenost = VZDIAL_ZAKLAD[j] as usize + c.bity(VZDIAL_NAVYSE[j] as u32)? as usize;
                v.kopia(vzdialenost, dlzka)?;
            }
            _ => return Err(chyba("ZIP: neplatný symbol")),
        }
    }
}

/// Rozbalí prúd DEFLATE do `ciel`. Vráti (počet bajtov, CRC-32 výstupu).
pub fn rozbal_deflate<W: Write>(data: &[u8], ciel: &mut W) -> io::Result<(u64, u32)> {
    let mut c = Citac { data, poz: 0, zasoba: 0, bitov: 0 };
    let mut v = Vystup { buf: Vec::with_capacity(2 << 20), ciel, crc: Crc32::novy(), celkom: 0 };
    let mut pevne_dlzky = [0u8; 288];
    for (i, d) in pevne_dlzky.iter_mut().enumerate() {
        *d = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let pevny_lit = Kod::z_dlzok(&pevne_dlzky)?;
    let pevna_vzd = Kod::z_dlzok(&[5u8; 30])?;
    loop {
        let posledny = c.bity(1)? == 1;
        match c.bity(2)? {
            0 => {
                // uložený blok: zarovnanie na bajt, dĺžka a jej doplnok
                c.zasoba = 0;
                c.bitov = 0;
                let hlava = data.get(c.poz..c.poz + 4).ok_or_else(|| chyba("ZIP: skrátený blok"))?;
                let dlzka = u16::from_le_bytes([hlava[0], hlava[1]]);
                if dlzka != !u16::from_le_bytes([hlava[2], hlava[3]]) {
                    return Err(chyba("ZIP: poškodený uložený blok"));
                }
                c.poz += 4;
                let kus =
                    data.get(c.poz..c.poz + dlzka as usize).ok_or_else(|| chyba("ZIP: skrátený blok"))?;
                for &b in kus {
                    v.bajt(b)?;
                }
                c.poz += dlzka as usize;
            }
            1 => blok_kodmi(&mut c, &mut v, &pevny_lit, &pevna_vzd)?,
            2 => {
                let hlit = c.bity(5)? as usize + 257;
                let hdist = c.bity(5)? as usize + 1;
                let hclen = c.bity(4)? as usize + 4;
                let mut dl = [0u8; 19];
                for &i in &PORADIE_DLZOK[..hclen] {
                    dl[i] = c.bity(3)? as u8;
                }
                let kod_dlzok = Kod::z_dlzok(&dl)?;
                let mut dlzky = vec![0u8; hlit + hdist];
                let mut i = 0;
                while i < hlit + hdist {
                    let s = c.symbol(&kod_dlzok)?;
                    let (hodnota, opakuj) = match s {
                        0..=15 => (s as u8, 1),
                        16 => {
                            let pred = *dlzky
                                .get(i.wrapping_sub(1))
                                .ok_or_else(|| chyba("ZIP: opakovanie bez dĺžky"))?;
                            (pred, 3 + c.bity(2)? as usize)
                        }
                        17 => (0, 3 + c.bity(3)? as usize),
                        _ => (0, 11 + c.bity(7)? as usize),
                    };
                    if i + opakuj > dlzky.len() {
                        return Err(chyba("ZIP: priveľa dĺžok kódu"));
                    }
                    dlzky[i..i + opakuj].fill(hodnota);
                    i += opakuj;
                }
                if dlzky[256] == 0 {
                    return Err(chyba("ZIP: chýba koniec bloku"));
                }
                let lit = Kod::z_dlzok(&dlzky[..hlit])?;
                let vzd = Kod::z_dlzok(&dlzky[hlit..])?;
                blok_kodmi(&mut c, &mut v, &lit, &vzd)?;
            }
            _ => return Err(chyba("ZIP: neplatný druh bloku")),
        }
        if posledny {
            break;
        }
    }
    v.odosli(true)?;
    Ok((v.celkom, v.crc.hodnota()))
}

// ---- súbor ZIP ----

/// Položka centrálneho adresára.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Polozka {
    pub meno: String,
    pub metoda: u16,
    pub crc: u32,
    pub zbalene: u64,
    pub velkost: u64,
    pub posun_hlavicky: u64,
}

fn u16_(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64_(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bajtov"))
}

/// Prečíta zoznam položiek ZIP (aj ZIP64).
pub fn polozky<R: Read + Seek>(f: &mut R) -> io::Result<Vec<Polozka>> {
    let dlzka = f.seek(SeekFrom::End(0))?;
    let chvost = dlzka.min(65_557 + 20);
    f.seek(SeekFrom::Start(dlzka - chvost))?;
    let mut t = vec![0u8; chvost as usize];
    f.read_exact(&mut t)?;
    let koniec = (0..t.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_(&t, i) == 0x0605_4b50)
        .ok_or_else(|| chyba("nie je to súbor ZIP"))?;
    let mut pocet = u16_(&t, koniec + 10) as u64;
    let mut velkost_adr = u32_(&t, koniec + 12) as u64;
    let mut posun_adr = u32_(&t, koniec + 16) as u64;
    // ZIP64: lokátor 20 bajtov pred koncom adresára
    if (posun_adr == 0xFFFF_FFFF || pocet == 0xFFFF) && koniec >= 20 && u32_(&t, koniec - 20) == 0x0706_4b50 {
        let poz = u64_(&t, koniec - 20 + 8);
        f.seek(SeekFrom::Start(poz))?;
        let mut z = [0u8; 56];
        f.read_exact(&mut z)?;
        if u32_(&z, 0) != 0x0606_4b50 {
            return Err(chyba("ZIP64: poškodený koniec adresára"));
        }
        pocet = u64_(&z, 32);
        velkost_adr = u64_(&z, 40);
        posun_adr = u64_(&z, 48);
    }
    if velkost_adr > 64 << 20 {
        return Err(chyba("ZIP: príliš veľký adresár"));
    }
    f.seek(SeekFrom::Start(posun_adr))?;
    let mut a = vec![0u8; velkost_adr as usize];
    f.read_exact(&mut a)?;
    let mut out = Vec::new();
    let mut i = 0;
    for _ in 0..pocet {
        if i + 46 > a.len() || u32_(&a, i) != 0x0201_4b50 {
            return Err(chyba("ZIP: poškodený adresár"));
        }
        let (n, e, k) = (u16_(&a, i + 28) as usize, u16_(&a, i + 30) as usize, u16_(&a, i + 32) as usize);
        let meno =
            String::from_utf8_lossy(a.get(i + 46..i + 46 + n).ok_or_else(|| chyba("ZIP: skrátený adresár"))?)
                .into_owned();
        let mut p = Polozka {
            meno,
            metoda: u16_(&a, i + 10),
            crc: u32_(&a, i + 16),
            zbalene: u32_(&a, i + 20) as u64,
            velkost: u32_(&a, i + 24) as u64,
            posun_hlavicky: u32_(&a, i + 42) as u64,
        };
        // rozšírenie ZIP64 (0x0001): len polia, ktoré majú v hlavičke 0xFFFFFFFF, v tomto poradí
        let extra = a.get(i + 46 + n..i + 46 + n + e).unwrap_or(&[]);
        let mut j = 0;
        while j + 4 <= extra.len() {
            let (id, dl) = (u16_(extra, j), u16_(extra, j + 2) as usize);
            if id == 1 {
                let mut q = j + 4;
                for pole in [&mut p.velkost, &mut p.zbalene, &mut p.posun_hlavicky] {
                    if *pole == 0xFFFF_FFFF && q + 8 <= extra.len() {
                        *pole = u64_(extra, q);
                        q += 8;
                    }
                }
            }
            j += 4 + dl;
        }
        out.push(p);
        i += 46 + n + e + k;
    }
    Ok(out)
}

/// Vyberie obraz: prvý súbor `.img` (prednosť má názov so „sklenik“).
pub fn vyber_obraz(p: &[Polozka]) -> Option<&Polozka> {
    let img = |x: &&Polozka| x.meno.to_ascii_lowercase().ends_with(".img");
    p.iter()
        .filter(img)
        .find(|x| x.meno.to_ascii_lowercase().contains("sklenik"))
        .or_else(|| p.iter().find(img))
}

/// Rozbalí položku do `ciel` a overí CRC-32.
pub fn rozbal<R: Read + Seek, W: Write>(f: &mut R, p: &Polozka, ciel: &mut W) -> io::Result<u64> {
    f.seek(SeekFrom::Start(p.posun_hlavicky))?;
    let mut h = [0u8; 30];
    f.read_exact(&mut h)?;
    if u32_(&h, 0) != 0x0403_4b50 {
        return Err(chyba("ZIP: poškodená hlavička položky"));
    }
    let preskoc = u16_(&h, 26) as i64 + u16_(&h, 28) as i64;
    f.seek(SeekFrom::Current(preskoc))?;
    if p.zbalene > 8 << 30 {
        return Err(chyba("ZIP: položka je príliš veľká"));
    }
    let mut zbalene = vec![0u8; p.zbalene as usize];
    f.read_exact(&mut zbalene)?;
    let (n, crc) = match p.metoda {
        0 => {
            ciel.write_all(&zbalene)?;
            let mut c = Crc32::novy();
            c.pridaj(&zbalene);
            (zbalene.len() as u64, c.hodnota())
        }
        8 => rozbal_deflate(&zbalene, ciel)?,
        m => return Err(chyba(format!("ZIP: nepodporovaná metóda {m}"))),
    };
    if n != p.velkost || crc != p.crc {
        return Err(chyba("ZIP: rozbalený obraz nesedí (veľkosť alebo CRC) – súbor je poškodený"));
    }
    Ok(n)
}

/// Ak je `cesta` súbor ZIP, rozbalí z neho obraz do dočasného priečinka a vráti cestu k nemu
/// (a `true` = dočasný súbor, po zápise ho treba zmazať). Inak vráti pôvodnú cestu.
pub fn priprav_obraz(cesta: &Path) -> io::Result<(PathBuf, bool)> {
    let mut f = File::open(cesta)?;
    let mut znacka = [0u8; 4];
    let je_zip = f.read(&mut znacka)? == 4 && znacka == *b"PK\x03\x04";
    if !je_zip {
        return Ok((cesta.to_path_buf(), false));
    }
    let zoznam = polozky(&mut f)?;
    let p = vyber_obraz(&zoznam).ok_or_else(|| chyba("v súbore ZIP nie je obraz .img"))?;
    let meno =
        Path::new(&p.meno).file_name().map_or("obraz.img".into(), |m| m.to_string_lossy().into_owned());
    let docasny = std::env::temp_dir().join(format!("sadic-{}-{meno}", std::process::id()));
    let mut w = BufWriter::new(File::create(&docasny)?);
    let vysledok = rozbal(&mut f, p, &mut w).and_then(|_| w.flush());
    drop(w);
    if let Err(e) = vysledok {
        let _ = std::fs::remove_file(&docasny);
        return Err(e);
    }
    Ok((docasny, true))
}

#[cfg(test)]
mod skusky {
    use super::*;
    use std::io::Cursor;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Skúšobné dáta – presné bajty, z ktorých sú vypočítané skúšobné toky DEFLATE nižšie (text je historický).
    fn zmes() -> Vec<u8> {
        let mut v: Vec<u8> = (0..3).flat_map(|_| 0..=255u8).collect();
        v.extend(std::iter::repeat_n(0u8, 5000));
        v.extend("SKLENÍK – Chladné RPi. ".repeat(40).as_bytes());
        v
    }

    const DYNAMICKE: &str = "6360646266616563e7e0e4e2e6e1e5e3171014121611151397909492969195935750545256515553d7d0d4d2d6d1d5d33730343236313533b7b0b4b2b6b1b5b37770747276717573f7f0f4f2f6f1f5f30f080c0a0e090d0b8f888c8a8e898d8b4f484c4a4e494d4bcfc8cccacec9cdcb2f282c2a2e292d2bafa8acaaaea9adab6f686c6a6e696d6befe8eceaeee9edeb9f3071d2e42953a74d9f3173d6ec3973e7cd5fb070d1e2254b972d5fb172d5ea356bd7addfb071d3e62d5bb76ddfb173d7ee3d7bf7ed3f70f0d0e123478f1d3f71f2d4e93367cf9dbf70f1d2e52b57af5dbf71f3d6ed3b77efdd7ff0f0d1e3274f9f3d7ff1f2d5eb376fdfbdfff0f1d3e72f5fbf7dfff1f3d7ef3f7ffffd6718f5ffc8f6ff281805a360148c8251300a46c1281805a360148c8251300a46c1281805a380e620d8dbc7d5ef70afb7c2a386c90ace193989297987572a040564ea298c4a8d4a8d4a8d4a61950200";

    #[test]
    fn crc32_kontrolna_hodnota() {
        let mut c = Crc32::novy();
        c.pridaj(b"123456789");
        assert_eq!(c.hodnota(), 0xCBF4_3926);
    }

    #[test]
    fn deflate_ulozeny_pevny_a_dynamicky_blok() {
        let mut o = Vec::new();
        rozbal_deflate(&hex("010e00f1ff41686f6a204f5243484944454121"), &mut o).unwrap();
        assert_eq!(o, b"Ahoj ORCHIDEA!");
        let mut o = Vec::new();
        rozbal_deflate(&hex("0bf6f671f53bdcebadf0a861b28273464e624adee1950a4101997a0ac143490a00"), &mut o)
            .unwrap();
        assert_eq!(o, &"SKLENÍK – Chladné RPi. ".repeat(40).as_bytes()[..200]);
        let mut o = Vec::new();
        let (n, crc) = rozbal_deflate(&hex(DYNAMICKE), &mut o).unwrap();
        assert_eq!(o, zmes());
        assert_eq!((n, crc), (6848, 0x8f62_5d2a));
        // poškodené údaje sa odhalia, nespadne to
        let mut zle = hex(DYNAMICKE);
        zle.truncate(100);
        assert!(rozbal_deflate(&zle, &mut Vec::new()).is_err());
        assert!(rozbal_deflate(&[0xFF; 8], &mut Vec::new()).is_err());
    }

    const ZIP: &str = "504b03041400000000000000735dc5683f8f03000000030000000d00000071656d752d727069332e6c6f676c6f67504b03041400000008000000735d2a5d628f5d010000c01a0000150000006f6272617a2f736b6c656e696b2d7270692e696d67";
    const ZIP_KONIEC: &str = "504b010214031400000000000000735dc5683f8f03000000030000000d000000000000000000000080010000000071656d752d727069332e6c6f67504b010214031400000008000000735d2a5d628f5d010000c01a000015000000000000000000000080012e0000006f6272617a2f736b6c656e696b2d7270692e696d67504b050600000000020002007e000000be0100000000";

    #[test]
    fn zip_najde_a_rozbali_obraz() {
        let data = hex(&format!("{ZIP}{DYNAMICKE}{ZIP_KONIEC}"));
        let mut f = Cursor::new(data.clone());
        let p = polozky(&mut f).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].meno, "qemu-rpi3.log");
        let obraz = vyber_obraz(&p).unwrap();
        assert_eq!(obraz.meno, "obraz/sklenik-rpi.img");
        assert_eq!((obraz.metoda, obraz.velkost), (8, 6848));
        let mut o = Vec::new();
        assert_eq!(rozbal(&mut f, obraz, &mut o).unwrap(), 6848);
        assert_eq!(o, zmes());
        // uložená položka
        let mut o = Vec::new();
        rozbal(&mut f, &p[0], &mut o).unwrap();
        assert_eq!(o, b"log");
        // zmenený bajt v údajoch → nesedí CRC alebo kód
        let mut zle = data;
        zle[200] ^= 0x10;
        let mut f = Cursor::new(zle);
        let p = polozky(&mut f).unwrap();
        assert!(rozbal(&mut f, vyber_obraz(&p).unwrap(), &mut Vec::new()).is_err());
        assert!(polozky(&mut Cursor::new(b"nie je zip".to_vec())).is_err());
    }
}
