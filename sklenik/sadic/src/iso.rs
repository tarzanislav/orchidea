//! Rozbor obrazu ISO vlastným kódom: ISO 9660 (ECMA-119) s Joliet, zavádzanie El Torito (BIOS, UEFI),
//! UDF (ECMA-167 / OSTA UDF – na ňom stoja inštalačné ISO Windows) a hybridné ISO s tabuľkou MBR alebo
//! GPT na začiatku (takmer všetky súčasné distribúcie Linuxu). Z rozboru vyplýva, ako ISO napáliť:
//! hybridné bit po bite, ostatné rozbalením súborov na FAT32.

use crate::oddiely::{self, ZaznamMbr};
use std::collections::BTreeSet;
use std::io::{self, Read, Seek, SeekFrom};

/// Logický sektor ISO.
pub const SEKTOR_ISO: u64 = 2048;
/// Najviac položiek, ktoré rozbor prečíta (ochrana pred poškodeným obrazom).
const NAJVIAC_POLOZIEK: usize = 200_000;
/// Hranica súboru na FAT32.
pub const HRANICA_FAT32: u64 = (4 << 30) - 1;

/// Súbor alebo priečinok v obraze. `kusy` = (posun v bajtoch, dĺžka) dát v obraze.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Polozka {
    pub cesta: String,
    pub velkost: u64,
    pub priecinok: bool,
    pub kusy: Vec<(u64, u64)>,
}

/// Platforma položky zavádzania El Torito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platforma {
    Bios,
    Efi,
    Ina(u8),
}

impl Platforma {
    fn z(b: u8) -> Platforma {
        match b {
            0 => Platforma::Bios,
            0xEF => Platforma::Efi,
            x => Platforma::Ina(x),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zavadzanie {
    pub platforma: Platforma,
    /// Typ emulácie (0 = bez emulácie).
    pub emulacia: u8,
    pub lba: u32,
    pub sektory: u16,
}

/// Čo obraz je a ako sa napáli.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Druh {
    /// Obraz disku (.img) s tabuľkou oddielov – zapíše sa bit po bite.
    ObrazDisku,
    /// Hybridné ISO (MBR/GPT na začiatku) – zapíše sa bit po bite a štartuje z USB.
    HybridneIso,
    /// Inštalačné ISO Windows (UDF, bez tabuľky oddielov) – treba rozbaliť súbory.
    WindowsIso,
    /// ISO bez tabuľky oddielov – treba rozbaliť súbory (štart z USB len cez UEFI, ak má EFI).
    IsoNaRozbalenie,
    /// Neznámy obsah.
    Nezname,
}

impl Druh {
    pub fn popis(self) -> &'static str {
        match self {
            Druh::ObrazDisku => "obraz disku",
            Druh::HybridneIso => "hybridné ISO",
            Druh::WindowsIso => "inštalačné ISO Windows",
            Druh::IsoNaRozbalenie => "ISO bez tabuľky oddielov",
            Druh::Nezname => "neznámy obsah",
        }
    }
}

/// Výsledok rozboru.
#[derive(Debug, Clone, Default)]
pub struct Rozbor {
    pub velkost: u64,
    pub iso9660: bool,
    pub joliet: bool,
    pub udf: bool,
    pub menovka: String,
    pub aplikacia: String,
    pub zavadzanie: Vec<Zavadzanie>,
    pub mbr: Vec<ZaznamMbr>,
    pub gpt: bool,
    /// Prvých 440 bajtov obsahuje kód zavádzača (štart cez BIOS z USB).
    pub mbr_kod: bool,
    pub polozky: Vec<Polozka>,
    /// Odkiaľ je zoznam súborov (UDF, Joliet, ISO 9660).
    pub zdroj: &'static str,
    /// Názov systému, ak ho obraz prezradí (.disk/info, menovka …).
    pub system: Option<String>,
}

fn u16_le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_le(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]))
}

fn citaj(f: &mut (impl Read + Seek), posun: u64, n: usize) -> io::Result<Vec<u8>> {
    let mut b = vec![0u8; n];
    f.seek(SeekFrom::Start(posun))?;
    let mut hotovo = 0;
    while hotovo < n {
        let k = f.read(&mut b[hotovo..])?;
        if k == 0 {
            break;
        }
        hotovo += k;
    }
    b.truncate(hotovo);
    Ok(b)
}

fn sektor(f: &mut (impl Read + Seek), lba: u64) -> io::Result<Vec<u8>> {
    let b = citaj(f, lba * SEKTOR_ISO, SEKTOR_ISO as usize)?;
    if b.len() < SEKTOR_ISO as usize {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "sektor za koncom obrazu"));
    }
    Ok(b)
}

fn text_a(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim_end_matches([' ', '\0']).to_string()
}

fn text_ucs2(b: &[u8]) -> String {
    let w: Vec<u16> =
        b.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&w).trim_end_matches([' ', '\0']).to_string()
}

/// Meno z ISO 9660 bez „;1“ a koncovej bodky.
fn meno_iso(meno: &str) -> String {
    let m = meno.split(';').next().unwrap_or(meno);
    m.strip_suffix('.').unwrap_or(m).to_string()
}

/// Strom ISO 9660 (alebo Joliet) od záznamu koreňového priečinka.
fn strom_iso(f: &mut (impl Read + Seek), koren: &[u8], joliet: bool) -> io::Result<Vec<Polozka>> {
    let mut vysledok: Vec<Polozka> = Vec::new();
    let mut fronta = vec![(String::new(), u32_le(koren, 2) as u64, u32_le(koren, 10) as u64, 0usize)];
    let mut navstivene = BTreeSet::new();
    while let Some((rodic, lba, dlzka, hlbka)) = fronta.pop() {
        if !navstivene.insert(lba) || hlbka > 32 || dlzka > 64 << 20 {
            continue;
        }
        let data = citaj(f, lba * SEKTOR_ISO, dlzka as usize)?;
        let mut i = 0usize;
        while i < data.len() {
            let n = data[i] as usize;
            if n == 0 {
                // záznamy neprechádzajú cez hranicu sektora
                i = (i / SEKTOR_ISO as usize + 1) * SEKTOR_ISO as usize;
                continue;
            }
            if i + n > data.len() || n < 34 {
                break;
            }
            let z = &data[i..i + n];
            let dlzka_mena = z[32] as usize;
            let meno_b = &z[33..(33 + dlzka_mena).min(n)];
            i += n;
            if dlzka_mena == 1 && (meno_b[0] == 0 || meno_b[0] == 1) {
                continue;
            }
            let meno = meno_iso(&if joliet { text_ucs2(meno_b) } else { text_a(meno_b) });
            let cesta = if rodic.is_empty() { meno } else { format!("{rodic}/{meno}") };
            let priecinok = z[25] & 2 != 0;
            let (zac, velkost) = (u32_le(z, 2) as u64, u32_le(z, 10) as u64);
            if priecinok {
                fronta.push((cesta.clone(), zac, velkost, hlbka + 1));
                vysledok.push(Polozka { cesta, velkost: 0, priecinok, kusy: Vec::new() });
            } else if let Some(p) = vysledok.last_mut().filter(|p| p.cesta == cesta && !p.priecinok) {
                // súbor z viacerých častí (ISO 9660 úrovne 3, súbory nad 4 GB)
                p.velkost += velkost;
                p.kusy.push((zac * SEKTOR_ISO, velkost));
            } else {
                vysledok.push(Polozka { cesta, velkost, priecinok, kusy: vec![(zac * SEKTOR_ISO, velkost)] });
            }
            if vysledok.len() > NAJVIAC_POLOZIEK {
                return Err(io::Error::other("priveľa položiek v obraze"));
            }
        }
    }
    Ok(vysledok)
}

// ---- UDF ----

/// Text dstring/d-characters UDF (OSTA CS0: 8 = 1 bajt na znak, 16 = UTF-16BE).
fn text_udf(b: &[u8]) -> String {
    match b.first() {
        // niektoré programy (genisoimage) sem dajú UTF-8 – ak bajty sú platné UTF-8 s diakritikou, berú sa tak
        Some(8) => match std::str::from_utf8(&b[1..]) {
            Ok(t) if !t.is_ascii() => t.trim_end_matches('\0').to_string(),
            _ => b[1..].iter().map(|&c| c as char).collect::<String>().trim_end_matches('\0').to_string(),
        },
        Some(16) => text_ucs2(&b[1..]),
        _ => String::new(),
    }
}

fn dstring(b: &[u8]) -> String {
    let n = *b.last().unwrap_or(&0) as usize;
    text_udf(&b[..n.min(b.len().saturating_sub(1))])
}

/// Prečítaný záznam súboru UDF: (priečinok?, veľkosť, kusy dát, vložené dáta).
type ZaznamUdf = (bool, u64, Vec<(u64, u64)>, Vec<u8>);

struct Udf {
    /// Začiatok oddielu v logických blokoch.
    oddiel: u64,
    blok: u64,
}

impl Udf {
    fn blok_na(&self, lbn: u32) -> u64 {
        (self.oddiel + lbn as u64) * self.blok
    }

    /// Prečíta záznam súboru (FE/EFE): (priečinok?, veľkosť, kusy dát, vložené dáta).
    fn zaznam(&self, f: &mut (impl Read + Seek), lbn: u32) -> io::Result<ZaznamUdf> {
        let b = citaj(f, self.blok_na(lbn), self.blok as usize)?;
        if b.len() < 176 {
            return Err(io::Error::other("UDF: krátky záznam súboru"));
        }
        let tag = u16_le(&b, 0);
        let (l_ea, l_ad, zac) = match tag {
            261 => (u32_le(&b, 168) as usize, u32_le(&b, 172) as usize, 176),
            266 => (u32_le(&b, 208) as usize, u32_le(&b, 212) as usize, 216),
            t => return Err(io::Error::other(format!("UDF: neočakávaná značka {t}"))),
        };
        let priecinok = b[16 + 11] == 4;
        let velkost = u64_le(&b, 56);
        let typ_ad = u16_le(&b, 16 + 18) & 7;
        let ad = b
            .get(zac + l_ea..zac + l_ea + l_ad)
            .ok_or_else(|| io::Error::other("UDF: deskriptory mimo bloku"))?;
        if typ_ad == 3 {
            return Ok((priecinok, velkost, Vec::new(), ad[..(velkost as usize).min(ad.len())].to_vec()));
        }
        let mut kusy = Vec::new();
        let mut ad = ad.to_vec();
        let mut kola = 0;
        'von: loop {
            let krok = if typ_ad == 0 { 8 } else { 16 };
            for d in ad.chunks(krok).filter(|d| d.len() == krok) {
                let dl = u32_le(d, 0);
                let (dlzka, druh) = ((dl & 0x3FFF_FFFF) as u64, dl >> 30);
                let pos = u32_le(d, 4);
                if dlzka == 0 {
                    break 'von;
                }
                match druh {
                    0 => kusy.push((self.blok_na(pos), dlzka)),
                    3 => {
                        // pokračovanie deskriptorov v ďalšom bloku (Allocation Extent Descriptor)
                        let e = citaj(f, self.blok_na(pos), self.blok as usize)?;
                        let n = u32_le(&e, 20) as usize;
                        ad = e.get(24..24 + n).unwrap_or(&[]).to_vec();
                        kola += 1;
                        if kola > 1000 {
                            break 'von;
                        }
                        continue 'von;
                    }
                    _ => kusy.push((u64::MAX, dlzka)), // nezapísaný úsek (nuly)
                }
            }
            break;
        }
        Ok((priecinok, velkost, kusy, Vec::new()))
    }
}

/// Prečíta časť súboru z obrazu od `posun` (v súbore) do `ciel`.
pub fn citaj_z(
    f: &mut (impl Read + Seek),
    p: &Polozka,
    mut posun: u64,
    mut ciel: &mut [u8],
) -> io::Result<()> {
    let mut od = 0u64;
    for &(zac, dlzka) in &p.kusy {
        if ciel.is_empty() {
            break;
        }
        if posun < od + dlzka {
            let v_kuse = posun - od;
            let n = ((dlzka - v_kuse) as usize).min(ciel.len());
            if zac == u64::MAX {
                ciel[..n].fill(0);
            } else {
                f.seek(SeekFrom::Start(zac + v_kuse))?;
                f.read_exact(&mut ciel[..n])?;
            }
            posun += n as u64;
            ciel = &mut ciel[n..];
        }
        od += dlzka;
    }
    if !ciel.is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{}: za koncom súboru", p.cesta)));
    }
    Ok(())
}

fn citaj_kusy(f: &mut (impl Read + Seek), kusy: &[(u64, u64)], velkost: u64) -> io::Result<Vec<u8>> {
    let mut v = Vec::new();
    for &(od, dlzka) in kusy {
        if v.len() as u64 >= velkost || dlzka > 64 << 20 {
            break;
        }
        if od == u64::MAX {
            v.resize(v.len() + dlzka as usize, 0);
        } else {
            v.extend(citaj(f, od, dlzka as usize)?);
        }
    }
    v.truncate(velkost as usize);
    Ok(v)
}

/// Strom UDF, ak obraz UDF má: (menovka, položky).
fn strom_udf(f: &mut (impl Read + Seek)) -> io::Result<Option<(String, Vec<Polozka>)>> {
    let avdp = match sektor(f, 256) {
        Ok(b) if u16_le(&b, 0) == 2 => b,
        _ => return Ok(None),
    };
    let (vds_dlzka, vds_lba) = (u32_le(&avdp, 16) as u64, u32_le(&avdp, 20) as u64);
    let (mut oddiel, mut blok, mut fsd, mut menovka) = (None, 2048u64, None, String::new());
    for i in 0..(vds_dlzka / SEKTOR_ISO).min(64) {
        let b = sektor(f, vds_lba + i)?;
        match u16_le(&b, 0) {
            5 => oddiel = Some(u32_le(&b, 188) as u64),
            6 => {
                blok = u32_le(&b, 212) as u64;
                menovka = dstring(&b[84..212]);
                fsd = Some(u32_le(&b, 248 + 4));
            }
            8 => break,
            _ => {}
        }
    }
    let (Some(oddiel), Some(fsd)) = (oddiel, fsd) else { return Ok(None) };
    if !(512..=4096).contains(&blok) {
        return Ok(None);
    }
    let u = Udf { oddiel, blok };
    let b = citaj(f, u.blok_na(fsd), blok as usize)?;
    if b.len() < 416 || u16_le(&b, 0) != 256 {
        return Ok(None);
    }
    let koren = u32_le(&b, 400 + 4);
    let mut vysledok = Vec::new();
    let mut fronta = vec![(String::new(), koren, 0usize)];
    let mut navstivene = BTreeSet::new();
    while let Some((rodic, icb, hlbka)) = fronta.pop() {
        if !navstivene.insert(icb) || hlbka > 32 {
            continue;
        }
        let (_, velkost, kusy, vlozene) = u.zaznam(f, icb)?;
        let data = if kusy.is_empty() { vlozene } else { citaj_kusy(f, &kusy, velkost.min(64 << 20))? };
        let mut i = 0usize;
        while i + 38 <= data.len() {
            if u16_le(&data, i) != 257 {
                break;
            }
            let vlastnosti = data[i + 18];
            let l_fi = data[i + 19] as usize;
            let ciel = u32_le(&data, i + 20 + 4);
            let l_iu = u16_le(&data, i + 36) as usize;
            let meno_od = i + 38 + l_iu;
            let meno = text_udf(data.get(meno_od..meno_od + l_fi).unwrap_or(&[]));
            i += (38 + l_iu + l_fi).div_ceil(4) * 4;
            if vlastnosti & 0x0C != 0 || meno.is_empty() {
                continue; // rodič alebo zmazaná položka
            }
            let cesta = if rodic.is_empty() { meno } else { format!("{rodic}/{meno}") };
            let (priecinok, velkost, kusy, _) = u.zaznam(f, ciel)?;
            if priecinok {
                fronta.push((cesta.clone(), ciel, hlbka + 1));
                vysledok.push(Polozka { cesta, velkost: 0, priecinok, kusy: Vec::new() });
            } else {
                vysledok.push(Polozka { cesta, velkost, priecinok, kusy });
            }
            if vysledok.len() > NAJVIAC_POLOZIEK {
                return Err(io::Error::other("priveľa položiek v obraze"));
            }
        }
    }
    Ok(Some((menovka, vysledok)))
}

/// Rozoberie obraz.
pub fn rozober(f: &mut (impl Read + Seek)) -> io::Result<Rozbor> {
    let mut r = Rozbor { velkost: f.seek(SeekFrom::End(0))?, ..Rozbor::default() };
    let zac = citaj(f, 0, 32 * 1024)?;
    if zac.len() >= 512 {
        r.mbr = oddiely::rozloz_mbr(&zac[..512]).unwrap_or_default();
        r.mbr_kod = zac[..440].iter().any(|&b| b != 0);
        r.gpt = zac.len() >= 520 && &zac[512..520] == b"EFI PART"
            || zac.len() >= 4104 && &zac[4096..4104] == b"EFI PART";
    }
    // deskriptory zväzku ISO 9660 od sektora 16
    let mut pvd = None;
    let mut svd = None;
    for lba in 16..64 {
        let Ok(b) = sektor(f, lba) else { break };
        if &b[1..6] == b"CD001" {
            r.iso9660 = true;
            match b[0] {
                0 if b[7..30].starts_with(b"EL TORITO SPECIFICATION") => {
                    r.zavadzanie = el_torito(f, u32_le(&b, 71) as u64).unwrap_or_default();
                }
                1 => pvd = Some(b),
                2 if b[88] == b'%' && b[89] == b'/' && matches!(b[90], b'@' | b'C' | b'E') => svd = Some(b),
                255 => {}
                _ => {}
            }
        } else if matches!(&b[1..6], b"NSR02" | b"NSR03") {
            r.udf = true;
        } else if !matches!(&b[1..6], b"BEA01" | b"TEA01" | b"BOOT2" | b"CDW02") {
            break;
        }
    }
    if let Some(b) = &svd {
        r.joliet = true;
        r.menovka = text_ucs2(&b[40..72]);
    }
    if let Some(b) = &pvd {
        // Joliet má na menovku len 16 znakov, ISO 9660 až 32 – vyhrá dlhšia
        let m = text_a(&b[40..72]);
        if m.chars().count() > r.menovka.chars().count() {
            r.menovka = m;
        }
        r.aplikacia = text_a(&b[574..702]);
    }
    // súbory: UDF má plné mená aj súbory nad 4 GB (Windows), inak Joliet, inak ISO 9660
    if r.udf
        && let Some((menovka, p)) = strom_udf(f)?
    {
        if r.menovka.is_empty() {
            r.menovka = menovka;
        }
        r.polozky = p;
        r.zdroj = "UDF";
    }
    if r.polozky.len() <= 1
        && let Some(b) = svd.as_ref().or(pvd.as_ref())
    {
        let iso = strom_iso(f, &b[156..190], svd.is_some())?;
        if iso.len() > r.polozky.len() {
            r.polozky = iso;
            r.zdroj = if svd.is_some() { "Joliet" } else { "ISO 9660" };
        }
    }
    r.polozky.sort_by_key(|p| p.cesta.to_lowercase());
    r.system = r.rozpoznaj_system(f);
    Ok(r)
}

fn el_torito(f: &mut (impl Read + Seek), katalog: u64) -> io::Result<Vec<Zavadzanie>> {
    let b = sektor(f, katalog)?;
    if b[0] != 1 || b[30] != 0x55 || b[31] != 0xAA {
        return Ok(Vec::new());
    }
    let mut platforma = Platforma::z(b[1]);
    let mut v = Vec::new();
    let mut i = 32;
    let polozka = |z: &[u8], p: Platforma| Zavadzanie {
        platforma: p,
        emulacia: z[1] & 0x0F,
        sektory: u16_le(z, 6),
        lba: u32_le(z, 8),
    };
    if b[i] == 0x88 {
        v.push(polozka(&b[i..i + 32], platforma));
    }
    i += 32;
    while i + 32 <= b.len() {
        match b[i] {
            0x90 | 0x91 => {
                platforma = Platforma::z(b[i + 1]);
                let n = u16_le(&b, i + 2) as usize;
                let posledna = b[i] == 0x91;
                i += 32;
                for _ in 0..n {
                    if i + 32 > b.len() {
                        break;
                    }
                    if b[i] == 0x88 {
                        v.push(polozka(&b[i..i + 32], platforma));
                    }
                    i += 32;
                    while i + 32 <= b.len() && b[i] == 0x44 {
                        i += 32; // rozširujúce položky
                    }
                }
                if posledna {
                    break;
                }
            }
            _ => break,
        }
    }
    Ok(v)
}

impl Rozbor {
    /// Nájde položku (bez ohľadu na veľké a malé písmená).
    pub fn najdi(&self, cesta: &str) -> Option<&Polozka> {
        self.polozky.iter().find(|p| p.cesta.eq_ignore_ascii_case(cesta))
    }

    pub fn ma(&self, cesta: &str) -> bool {
        self.najdi(cesta).is_some()
    }

    /// Súbory nad 4 GB (nezmestia sa na FAT32).
    pub fn nad_4gb(&self) -> Vec<&Polozka> {
        self.polozky.iter().filter(|p| !p.priecinok && p.velkost > HRANICA_FAT32).collect()
    }

    pub fn windows(&self) -> bool {
        ["sources/install.wim", "sources/install.esd", "sources/install.swm"].iter().any(|c| self.ma(c))
            || self.ma("sources/boot.wim") && self.ma("bootmgr")
    }

    pub fn efi(&self) -> bool {
        self.zavadzanie.iter().any(|z| z.platforma == Platforma::Efi)
            || [
                "efi/boot/bootx64.efi",
                "efi/boot/bootaa64.efi",
                "efi/boot/bootia32.efi",
                "efi/boot/bootriscv64.efi",
            ]
            .iter()
            .any(|c| self.ma(c))
    }

    pub fn bios(&self) -> bool {
        self.zavadzanie.iter().any(|z| z.platforma == Platforma::Bios)
    }

    pub fn hybridne(&self) -> bool {
        self.iso9660 && (self.gpt || !self.mbr.is_empty())
    }

    pub fn druh(&self) -> Druh {
        if !self.iso9660 {
            return if self.gpt || !self.mbr.is_empty() { Druh::ObrazDisku } else { Druh::Nezname };
        }
        if self.windows() {
            return Druh::WindowsIso;
        }
        if self.hybridne() { Druh::HybridneIso } else { Druh::IsoNaRozbalenie }
    }

    fn rozpoznaj_system(&self, f: &mut (impl Read + Seek)) -> Option<String> {
        if let Some(p) = self.najdi(".disk/info").filter(|p| p.velkost < 4096) {
            let t = citaj_kusy(f, &p.kusy, p.velkost).ok()?;
            let t = String::from_utf8_lossy(&t).lines().next().unwrap_or("").trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
        let m = self.menovka.to_uppercase();
        if self.windows() {
            let arch = if m.contains("A64FRE") || m.contains("ARM64") {
                " (ARM64)"
            } else if m.contains("X86FRE") {
                " (32-bit)"
            } else {
                " (x64)"
            };
            return Some(format!("Windows – inštalácia{arch}"));
        }
        let podla_priecinka = [
            ("casper", "Ubuntu alebo odvodený systém"),
            ("live", "Debian Live alebo odvodený systém"),
            ("arch", "Arch Linux alebo odvodený systém"),
            ("LiveOS", "Fedora alebo odvodený systém"),
            ("images/pxeboot", "Fedora / Red Hat alebo odvodený systém"),
            ("boot/x86_64/loader", "openSUSE"),
            ("apks", "Alpine Linux"),
        ];
        podla_priecinka.iter().find(|(c, _)| self.ma(c)).map(|(_, n)| n.to_string())
    }

    /// Zoznam súborov (cesta, veľkosť, počet úsekov v obraze).
    pub fn zoznam(&self) -> String {
        let mut t = format!("súbory podľa {}:\n", self.zdroj);
        for p in &self.polozky {
            if p.priecinok {
                t += &format!("  {}/\n", p.cesta);
            } else {
                t += &format!("  {}  {} B  ({} úsek.)\n", p.cesta, p.velkost, p.kusy.len());
            }
        }
        t
    }

    /// Zrozumiteľný súhrn pre človeka.
    pub fn suhrn(&self) -> String {
        let mut r = vec![format!(
            "{} – {}{}",
            self.druh().popis(),
            crate::disk::velkost(self.velkost),
            if self.menovka.is_empty() { String::new() } else { format!(", menovka „{}“", self.menovka) }
        )];
        if let Some(s) = &self.system {
            r.push(format!("systém: {s}"));
        }
        let mut st = Vec::new();
        if self.bios() || self.mbr_kod && self.hybridne() {
            st.push("BIOS");
        }
        if self.efi() {
            st.push("UEFI");
        }
        if !st.is_empty() {
            r.push(format!("štart: {}", st.join(" + ")));
        }
        let mut fs = Vec::new();
        if self.iso9660 {
            fs.push(if self.joliet { "ISO 9660 + Joliet" } else { "ISO 9660" });
        }
        if self.udf {
            fs.push("UDF");
        }
        if self.gpt {
            fs.push("tabuľka GPT");
        } else if !self.mbr.is_empty() {
            fs.push("tabuľka MBR");
        }
        if !fs.is_empty() {
            r.push(format!("štruktúra: {}", fs.join(", ")));
        }
        let subory = self.polozky.iter().filter(|p| !p.priecinok).count();
        if subory > 0 {
            r.push(crate::disk::pocet(subory, "súbor", "súbory", "súborov"));
        }
        for p in self.nad_4gb() {
            r.push(format!("súbor nad 4 GB: {} ({})", p.cesta, crate::disk::velkost(p.velkost)));
        }
        r.push(format!("napálenie: {}", self.odporucanie()));
        r.join("\n")
    }

    pub fn odporucanie(&self) -> &'static str {
        match self.druh() {
            Druh::ObrazDisku => "zapíše sa bit po bite a overí",
            Druh::HybridneIso => "zapíše sa bit po bite (ako dd) a overí – štartuje z USB aj karty",
            Druh::WindowsIso => "súbory sa rozbalia na FAT32 (štart cez UEFI)",
            Druh::IsoNaRozbalenie => "súbory sa rozbalia na FAT32 (štart cez UEFI, ak ho ISO má)",
            Druh::Nezname => "nie je to obraz disku ani ISO",
        }
    }
}

#[cfg(test)]
pub mod skusky {
    use super::*;
    use std::io::Cursor;

    fn zaznam(meno: &[u8], lba: u32, dlzka: u32, priecinok: bool) -> Vec<u8> {
        let n = 33 + meno.len() + (meno.len() + 1) % 2;
        let mut z = vec![0u8; n];
        z[0] = n as u8;
        z[2..6].copy_from_slice(&lba.to_le_bytes());
        z[6..10].copy_from_slice(&lba.to_be_bytes());
        z[10..14].copy_from_slice(&dlzka.to_le_bytes());
        z[14..18].copy_from_slice(&dlzka.to_be_bytes());
        z[25] = if priecinok { 2 } else { 0 };
        z[32] = meno.len() as u8;
        z[33..33 + meno.len()].copy_from_slice(meno);
        z
    }

    /// Malé ISO na skúšky: PVD, El Torito (BIOS + EFI), priečinky EFI/BOOT a súbory; voliteľne hybridné.
    pub fn skusobne_iso(hybridne: bool, windows: bool) -> Vec<u8> {
        let s = SEKTOR_ISO as usize;
        let mut iso = vec![0u8; 40 * s];
        // PVD v sektore 16
        let p = &mut iso[16 * s..17 * s];
        p[0] = 1;
        p[1..6].copy_from_slice(b"CD001");
        p[6] = 1;
        p[40..72].fill(b' ');
        p[40..51].copy_from_slice(if windows { b"CCCOMA_X64F" } else { b"SKUSKA_LIVE" });
        p[80..84].copy_from_slice(&40u32.to_le_bytes());
        p[128..130].copy_from_slice(&2048u16.to_le_bytes());
        let koren = zaznam(&[0], 20, 2048, true);
        p[156..156 + 34].copy_from_slice(&koren[..34]);
        // zavádzací záznam v sektore 17 → katalóg v sektore 19
        let b = &mut iso[17 * s..18 * s];
        b[1..6].copy_from_slice(b"CD001");
        b[6] = 1;
        b[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
        b[71..75].copy_from_slice(&19u32.to_le_bytes());
        // ukončenie v sektore 18
        let t = &mut iso[18 * s..19 * s];
        t[0] = 255;
        t[1..6].copy_from_slice(b"CD001");
        // katalóg: overovacia položka (BIOS), predvolená, sekcia EFI
        let k = &mut iso[19 * s..20 * s];
        k[0] = 1;
        k[30] = 0x55;
        k[31] = 0xAA;
        k[32] = 0x88;
        k[32 + 6] = 4;
        k[32 + 8..32 + 12].copy_from_slice(&30u32.to_le_bytes());
        k[64] = 0x91;
        k[65] = 0xEF;
        k[66] = 1;
        k[96] = 0x88;
        k[96 + 8..96 + 12].copy_from_slice(&31u32.to_le_bytes());
        // koreň (sektor 20): ., .., EFI/, SOURCES/ alebo CASPER/, README.TXT
        let mut kor = Vec::new();
        kor.extend(zaznam(&[0], 20, 2048, true));
        kor.extend(zaznam(&[1], 20, 2048, true));
        kor.extend(zaznam(b"EFI", 21, 2048, true));
        if windows {
            kor.extend(zaznam(b"SOURCES", 23, 2048, true));
            kor.extend(zaznam(b"BOOTMGR;1", 33, 5, false));
        } else {
            kor.extend(zaznam(b"CASPER", 23, 2048, true));
        }
        kor.extend(zaznam(b"README.TXT;1", 32, 12, false));
        iso[20 * s..20 * s + kor.len()].copy_from_slice(&kor);
        let mut efi = zaznam(&[0], 21, 2048, true);
        efi.extend(zaznam(&[1], 20, 2048, true));
        efi.extend(zaznam(b"BOOT", 22, 2048, true));
        iso[21 * s..21 * s + efi.len()].copy_from_slice(&efi);
        let mut boot = zaznam(&[0], 22, 2048, true);
        boot.extend(zaznam(&[1], 21, 2048, true));
        boot.extend(zaznam(b"BOOTX64.EFI;1", 31, 2048, false));
        iso[22 * s..22 * s + boot.len()].copy_from_slice(&boot);
        let mut src = zaznam(&[0], 23, 2048, true);
        src.extend(zaznam(&[1], 20, 2048, true));
        src.extend(zaznam(if windows { b"INSTALL.WIM;1" } else { b"VMLINUZ;1" }, 34, 4096, false));
        iso[23 * s..23 * s + src.len()].copy_from_slice(&src);
        iso[32 * s..32 * s + 12].copy_from_slice(b"Ahoj, svet!\n");
        iso[33 * s..33 * s + 5].copy_from_slice(b"BOOTM");
        if hybridne {
            let o = oddiely::Oddiel {
                zaciatok: 0,
                sektory: 40 * 4,
                typ: oddiely::Typ::Linux,
                meno: String::new(),
                aktivny: true,
            };
            let m = oddiely::mbr(512, &[o], 0xABCD);
            iso[..512].copy_from_slice(&m);
        }
        iso
    }

    #[test]
    fn rozbor_hybridneho_iso() {
        let iso = skusobne_iso(true, false);
        let r = rozober(&mut Cursor::new(&iso)).unwrap();
        assert!(r.iso9660 && !r.udf);
        assert_eq!(r.menovka, "SKUSKA_LIVE");
        assert_eq!(r.zavadzanie.len(), 2);
        assert_eq!(r.zavadzanie[0].platforma, Platforma::Bios);
        assert_eq!(r.zavadzanie[1].platforma, Platforma::Efi);
        assert!(r.bios() && r.efi());
        assert_eq!(r.druh(), Druh::HybridneIso);
        assert!(r.ma("efi/boot/bootx64.efi"));
        assert_eq!(r.system.as_deref(), Some("Ubuntu alebo odvodený systém"));
        let readme = r.najdi("README.TXT").unwrap();
        assert_eq!(
            citaj_kusy(&mut Cursor::new(&iso), &readme.kusy, readme.velkost).unwrap(),
            b"Ahoj, svet!\n"
        );
        assert!(r.suhrn().contains("bit po bite"));
    }

    #[test]
    fn rozbor_windows_a_obrazu_disku() {
        let r = rozober(&mut Cursor::new(skusobne_iso(false, true))).unwrap();
        assert_eq!(r.druh(), Druh::WindowsIso);
        assert!(r.windows());
        assert_eq!(r.system.as_deref(), Some("Windows – inštalácia (x64)"));
        let r = rozober(&mut Cursor::new(skusobne_iso(false, false))).unwrap();
        assert_eq!(r.druh(), Druh::IsoNaRozbalenie);
        let obraz = sklenik_karta::skusobny_obraz(40);
        let r = rozober(&mut Cursor::new(obraz)).unwrap();
        assert_eq!(r.druh(), Druh::ObrazDisku);
        assert!(!r.iso9660);
        let r = rozober(&mut Cursor::new(vec![0u8; 100_000])).unwrap();
        assert_eq!(r.druh(), Druh::Nezname);
    }
}
