//! Exif (JEITA CP-3451, štruktúra TIFF 6.0) a IPTC-IIM (IPTC 4.2): čítanie položiek na zobrazenie a prenos
//! medzi formátmi.
//!
//! Exif je štruktúra TIFF: poradie bajtov „II“ alebo „MM“, číslo 42, posun prvého adresára (IFD0) a položky po 12
//! bajtoch; podadresáre Exif (značka 34665), GPS (34853) a Interop (40965). Pri prenose do TIFF sa adresáre
//! rozoberú a zapíšu nanovo (poradie bajtov little-endian, nové posuny); v JPEG, PNG a JPEG XL sa blok prenesie
//! bez zmeny. Čítanie je ohraničené dĺžkou údajov a počtom položiek – poškodený Exif vráti `None`.

fn u16_v(d: &[u8], i: usize, le: bool) -> Option<u16> {
    let b = d.get(i..i.checked_add(2)?)?;
    Some(if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
}

fn u32_v(d: &[u8], i: usize, le: bool) -> Option<u32> {
    let b = d.get(i..i.checked_add(4)?)?;
    let p = [b[0], b[1], b[2], b[3]];
    Some(if le { u32::from_le_bytes(p) } else { u32::from_be_bytes(p) })
}

/// Poradie bajtov a posun IFD0 z hlavičky TIFF.
fn hlavicka(d: &[u8]) -> Option<(bool, usize)> {
    let le = match d.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if u16_v(d, 2, le)? != 42 {
        return None;
    }
    Some((le, u32_v(d, 4, le)? as usize))
}

/// Natočenie 1–8 podľa Exif (1 = bez zmeny), ak je v údajoch.
pub fn natocenie(exif: &[u8]) -> Option<u8> {
    // niektoré programy nechajú na začiatku hlavičku „Exif\0\0“
    let d = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let (le, ifd) = hlavicka(d)?;
    let pocet = u16_v(d, ifd, le)? as usize;
    for k in 0..pocet.min(1000) {
        let e = ifd + 2 + 12 * k;
        if u16_v(d, e, le)? == 0x0112 {
            let v = u16_v(d, e + 8, le)?;
            return (1..=8).contains(&v).then_some(v as u8);
        }
    }
    None
}

/// Nastaví natočenie v IFD0 (ak tam položka je) priamo v bloku; vráti, či sa našla.
pub fn nastav_natocenie(exif: &mut [u8], n: u8) -> bool {
    let posun = if exif.starts_with(b"Exif\0\0") { 6 } else { 0 };
    let d = &mut exif[posun..];
    let Some((le, ifd)) = hlavicka(d) else { return false };
    let Some(pocet) = u16_v(d, ifd, le) else { return false };
    for k in 0..(pocet as usize).min(1000) {
        let e = ifd + 2 + 12 * k;
        if u16_v(d, e, le) == Some(0x0112) && e + 10 <= d.len() {
            let b = if le { (n as u16).to_le_bytes() } else { (n as u16).to_be_bytes() };
            d[e + 8..e + 10].copy_from_slice(&b);
            return true;
        }
    }
    false
}

/// Exif s jedinou položkou natočenia (pre zápis a skúšky).
pub fn s_natocenim(n: u8) -> Vec<u8> {
    let mut d = b"II\x2A\x00\x08\x00\x00\x00\x01\x00".to_vec();
    d.extend_from_slice(&0x0112u16.to_le_bytes());
    d.extend_from_slice(&3u16.to_le_bytes());
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&(n as u16).to_le_bytes());
    d.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    d
}

// ---------------------------------------------------------------- adresáre

/// Položka adresára; hodnota je vždy little-endian (pri čítaní „MM“ sa prehodí podľa typu).
#[derive(Debug, Clone, PartialEq)]
pub struct Polozka {
    pub znacka: u16,
    pub typ: u16,
    pub pocet: u32,
    pub hodnota: Vec<u8>,
}

/// Rozobraný Exif: IFD0 (bez ukazovateľov na podadresáre) a podadresáre.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Exif {
    pub ifd0: Vec<Polozka>,
    pub exif: Vec<Polozka>,
    pub gps: Vec<Polozka>,
    pub interop: Vec<Polozka>,
}

const UKAZOVATELE: [u16; 3] = [34665, 34853, 40965];

/// Veľkosť jednej hodnoty typu TIFF a šírka prvku pri zmene poradia bajtov.
fn typ_velkost(t: u16) -> Option<(usize, usize)> {
    Some(match t {
        1 | 2 | 6 | 7 => (1, 1),
        3 | 8 => (2, 2),
        4 | 9 | 11 | 13 => (4, 4),
        5 | 10 => (8, 4),
        12 => (8, 8),
        _ => return None,
    })
}

fn citaj_ifd(d: &[u8], ifd: usize, le: bool) -> Option<Vec<Polozka>> {
    let pocet = u16_v(d, ifd, le)? as usize;
    if pocet > 4000 {
        return None;
    }
    let mut v = Vec::with_capacity(pocet);
    for k in 0..pocet {
        let e = ifd + 2 + 12 * k;
        let (znacka, typ, n) = (u16_v(d, e, le)?, u16_v(d, e + 2, le)?, u32_v(d, e + 4, le)?);
        let Some((velkost, prvok)) = typ_velkost(typ) else { continue };
        let dlzka = (n as usize).checked_mul(velkost)?;
        if dlzka > d.len() {
            return None;
        }
        let zaciatok = if dlzka <= 4 { e + 8 } else { u32_v(d, e + 8, le)? as usize };
        let mut h = d.get(zaciatok..zaciatok.checked_add(dlzka)?)?.to_vec();
        if !le && prvok > 1 {
            h.chunks_exact_mut(prvok).for_each(|c| c.reverse());
        }
        v.push(Polozka { znacka, typ, pocet: n, hodnota: h });
    }
    Some(v)
}

fn ukazovatel(p: &[Polozka], z: u16) -> Option<usize> {
    let q = p.iter().find(|q| q.znacka == z && (q.typ == 4 || q.typ == 13) && q.hodnota.len() >= 4)?;
    Some(u32::from_le_bytes([q.hodnota[0], q.hodnota[1], q.hodnota[2], q.hodnota[3]]) as usize)
}

fn rozober_od(d: &[u8], le: bool, ifd: usize) -> Option<Exif> {
    let mut ifd0 = citaj_ifd(d, ifd, le)?;
    let pod = |p: &[Polozka], z| ukazovatel(p, z).and_then(|o| citaj_ifd(d, o, le));
    let mut exif = pod(&ifd0, 34665).unwrap_or_default();
    let gps = pod(&ifd0, 34853).unwrap_or_default();
    let interop = pod(&exif, 40965).unwrap_or_default();
    ifd0.retain(|p| !UKAZOVATELE.contains(&p.znacka));
    exif.retain(|p| !UKAZOVATELE.contains(&p.znacka));
    Some(Exif { ifd0, exif, gps, interop })
}

/// Rozoberie blok Exif (aj s hlavičkou „Exif\0\0“).
pub fn rozober(blok: &[u8]) -> Option<Exif> {
    let d = blok.strip_prefix(b"Exif\0\0").unwrap_or(blok);
    let (le, ifd) = hlavicka(d)?;
    rozober_od(d, le, ifd)
}

/// Značky IFD0 strany TIFF, ktoré patria k popisu (nie k uloženiu obrazu).
const POPISNE_IFD0: [u16; 9] = [270, 271, 272, 305, 306, 315, 316, 33432, 274];

/// Exif strany klasického súboru TIFF (adresár na posune `ifd`): popisné značky IFD0 a podadresáre Exif a GPS.
pub fn z_tiff(subor: &[u8], ifd: usize) -> Option<Vec<u8>> {
    let (le, _) = hlavicka(subor)?;
    let mut e = rozober_od(subor, le, ifd)?;
    e.ifd0.retain(|p| POPISNE_IFD0.contains(&p.znacka));
    (!e.exif.is_empty() || !e.gps.is_empty()).then(|| e.zapis())
}

fn velkost_ifd(p: &[Polozka]) -> usize {
    6 + 12 * p.len()
        + p.iter()
            .map(|q| if q.hodnota.len() > 4 { q.hodnota.len().div_ceil(2) * 2 } else { 0 })
            .sum::<usize>()
}

/// Zapíše adresár na absolútnu polohu `zaklad` (hodnoty nad 4 bajty hneď za ním), little-endian.
pub fn zapis_ifd(polozky: &[Polozka], zaklad: usize, dalsi: u32) -> Vec<u8> {
    let mut p: Vec<&Polozka> = polozky.iter().collect();
    p.sort_by_key(|q| q.znacka);
    let mut v = Vec::with_capacity(velkost_ifd(polozky));
    v.extend_from_slice(&(p.len() as u16).to_le_bytes());
    let mut hodnoty = Vec::new();
    let za = zaklad + 6 + 12 * p.len();
    for q in &p {
        v.extend_from_slice(&q.znacka.to_le_bytes());
        v.extend_from_slice(&q.typ.to_le_bytes());
        v.extend_from_slice(&q.pocet.to_le_bytes());
        if q.hodnota.len() <= 4 {
            let mut b = q.hodnota.clone();
            b.resize(4, 0);
            v.extend_from_slice(&b);
        } else {
            v.extend_from_slice(&((za + hodnoty.len()) as u32).to_le_bytes());
            hodnoty.extend_from_slice(&q.hodnota);
            if hodnoty.len() % 2 == 1 {
                hodnoty.push(0);
            }
        }
    }
    v.extend_from_slice(&dalsi.to_le_bytes());
    v.extend_from_slice(&hodnoty);
    v
}

fn dlhy(z: u16, hodnota: u32) -> Polozka {
    Polozka { znacka: z, typ: 4, pocet: 1, hodnota: hodnota.to_le_bytes().to_vec() }
}

impl Exif {
    /// Podadresáre Exif (s Interop) a GPS zapísané od absolútnej polohy `zaklad`; vráti bajty a posuny
    /// adresárov Exif a GPS (pre ukazovatele v IFD0).
    pub fn zapis_podadresare(&self, zaklad: usize) -> (Vec<u8>, Option<u32>, Option<u32>) {
        let mut v = Vec::new();
        let (mut po_exif, mut po_gps) = (None, None);
        if !self.exif.is_empty() || !self.interop.is_empty() {
            let mut e = self.exif.clone();
            let poloha_exif = zaklad;
            if !self.interop.is_empty() {
                e.push(dlhy(40965, 0));
                let poloha_interop = poloha_exif + velkost_ifd(&e);
                e.last_mut().unwrap().hodnota = (poloha_interop as u32).to_le_bytes().to_vec();
                v.extend(zapis_ifd(&e, poloha_exif, 0));
                v.extend(zapis_ifd(&self.interop, poloha_interop, 0));
            } else {
                v.extend(zapis_ifd(&e, poloha_exif, 0));
            }
            po_exif = Some(poloha_exif as u32);
        }
        if !self.gps.is_empty() {
            let poloha = zaklad + v.len();
            v.extend(zapis_ifd(&self.gps, poloha, 0));
            po_gps = Some(poloha as u32);
        }
        (v, po_exif, po_gps)
    }

    /// Samostatný blok Exif (little-endian, bez hlavičky „Exif\0\0“).
    pub fn zapis(&self) -> Vec<u8> {
        let mut ifd0 = self.ifd0.clone();
        let ma_exif = !self.exif.is_empty() || !self.interop.is_empty();
        if ma_exif {
            ifd0.push(dlhy(34665, 0));
        }
        if !self.gps.is_empty() {
            ifd0.push(dlhy(34853, 0));
        }
        let za_ifd0 = 8 + velkost_ifd(&ifd0);
        let (pod, pe, pg) = self.zapis_podadresare(za_ifd0);
        for q in ifd0.iter_mut() {
            match q.znacka {
                34665 => q.hodnota = pe.unwrap_or(0).to_le_bytes().to_vec(),
                34853 => q.hodnota = pg.unwrap_or(0).to_le_bytes().to_vec(),
                _ => {}
            }
        }
        let mut v = b"II\x2A\x00\x08\x00\x00\x00".to_vec();
        v.extend(zapis_ifd(&ifd0, 8, 0));
        v.extend(pod);
        v
    }
}

// ---------------------------------------------------------------- čitateľné položky

/// Hodnota položky na zobrazenie.
#[derive(Debug, Clone, PartialEq)]
pub enum Hodnota {
    Text(String),
    Cisla(Vec<f64>),
    Bajty(usize),
}

impl Hodnota {
    /// Hodnota ako text (čísla oddelené medzerou, bajty ako počet).
    pub fn text(&self) -> String {
        match self {
            Hodnota::Text(t) => t.clone(),
            Hodnota::Cisla(c) => c.iter().map(|x| format!("{x}")).collect::<Vec<_>>().join(" "),
            Hodnota::Bajty(n) => format!("({n} B)"),
        }
    }
}

fn hodnota(p: &Polozka) -> Hodnota {
    let h = &p.hodnota;
    let u16s = || h.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
    let u32s = || h.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    match p.typ {
        2 => Hodnota::Text(String::from_utf8_lossy(h).trim_end_matches('\0').trim_end().to_string()),
        1 => Hodnota::Cisla(h.iter().map(|&x| x as f64).collect()),
        6 => Hodnota::Cisla(h.iter().map(|&x| x as i8 as f64).collect()),
        3 => Hodnota::Cisla(u16s().map(|x| x as f64).collect()),
        8 => Hodnota::Cisla(u16s().map(|x| x as i16 as f64).collect()),
        4 | 13 => Hodnota::Cisla(u32s().map(|x| x as f64).collect()),
        9 => Hodnota::Cisla(u32s().map(|x| x as i32 as f64).collect()),
        5 => Hodnota::Cisla(
            u32s()
                .collect::<Vec<_>>()
                .chunks_exact(2)
                .map(|c| if c[1] == 0 { 0.0 } else { c[0] as f64 / c[1] as f64 })
                .collect(),
        ),
        10 => Hodnota::Cisla(
            u32s()
                .collect::<Vec<_>>()
                .chunks_exact(2)
                .map(|c| if c[1] == 0 { 0.0 } else { c[0] as i32 as f64 / c[1] as i32 as f64 })
                .collect(),
        ),
        11 => Hodnota::Cisla(u32s().map(|x| f32::from_bits(x) as f64).collect()),
        12 => Hodnota::Cisla(
            h.chunks_exact(8)
                .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect(),
        ),
        // UNDEFINED: verzie a krátke texty ako text, inak len veľkosť
        _ if h.len() <= 64 && h.iter().all(|&b| b == 0 || (0x20..0x7F).contains(&b)) => {
            Hodnota::Text(String::from_utf8_lossy(h).trim_end_matches('\0').to_string())
        }
        _ => Hodnota::Bajty(h.len()),
    }
}

/// Mená značiek podľa normy Exif 2.32 / TIFF 6.0 (najčastejšie; ostatné ako „0x…“).
fn meno(skupina: &str, z: u16) -> String {
    let m = match (skupina, z) {
        ("GPS", 0) => "GPSVersionID",
        ("GPS", 1) => "GPSLatitudeRef",
        ("GPS", 2) => "GPSLatitude",
        ("GPS", 3) => "GPSLongitudeRef",
        ("GPS", 4) => "GPSLongitude",
        ("GPS", 5) => "GPSAltitudeRef",
        ("GPS", 6) => "GPSAltitude",
        ("GPS", 7) => "GPSTimeStamp",
        ("GPS", 18) => "GPSMapDatum",
        ("GPS", 29) => "GPSDateStamp",
        ("GPS", _) => "",
        ("Interop", 1) => "InteropIndex",
        ("Interop", 2) => "InteropVersion",
        ("Interop", _) => "",
        (_, 270) => "ImageDescription",
        (_, 271) => "Make",
        (_, 272) => "Model",
        (_, 274) => "Orientation",
        (_, 282) => "XResolution",
        (_, 283) => "YResolution",
        (_, 296) => "ResolutionUnit",
        (_, 305) => "Software",
        (_, 306) => "ModifyDate",
        (_, 315) => "Artist",
        (_, 316) => "HostComputer",
        (_, 531) => "YCbCrPositioning",
        (_, 33432) => "Copyright",
        (_, 33434) => "ExposureTime",
        (_, 33437) => "FNumber",
        (_, 34850) => "ExposureProgram",
        (_, 34855) => "ISO",
        (_, 36864) => "ExifVersion",
        (_, 37121) => "ComponentsConfiguration",
        (_, 36867) => "DateTimeOriginal",
        (_, 36868) => "CreateDate",
        (_, 36880) => "OffsetTime",
        (_, 36881) => "OffsetTimeOriginal",
        (_, 37377) => "ShutterSpeedValue",
        (_, 37378) => "ApertureValue",
        (_, 37380) => "ExposureCompensation",
        (_, 37381) => "MaxApertureValue",
        (_, 37383) => "MeteringMode",
        (_, 37385) => "Flash",
        (_, 37386) => "FocalLength",
        (_, 37500) => "MakerNote",
        (_, 37510) => "UserComment",
        (_, 37520) => "SubSecTime",
        (_, 37521) => "SubSecTimeOriginal",
        (_, 40960) => "FlashpixVersion",
        (_, 40961) => "ColorSpace",
        (_, 40962) => "ExifImageWidth",
        (_, 40963) => "ExifImageHeight",
        (_, 41495) => "SensingMethod",
        (_, 41986) => "ExposureMode",
        (_, 41987) => "WhiteBalance",
        (_, 41989) => "FocalLengthIn35mmFormat",
        (_, 41990) => "SceneCaptureType",
        (_, 42016) => "ImageUniqueID",
        (_, 42033) => "SerialNumber",
        (_, 42034) => "LensInfo",
        (_, 42035) => "LensMake",
        (_, 42036) => "LensModel",
        _ => "",
    };
    if m.is_empty() { format!("0x{z:04X}") } else { m.to_string() }
}

/// Čitateľné položky Exif: (skupina IFD0/Exif/GPS/Interop, meno značky, značka, hodnota).
pub fn polozky(blok: &[u8]) -> Vec<(&'static str, String, u16, Hodnota)> {
    let Some(e) = rozober(blok) else { return Vec::new() };
    let mut v = Vec::new();
    for (skupina, p) in [("IFD0", &e.ifd0), ("Exif", &e.exif), ("GPS", &e.gps), ("Interop", &e.interop)] {
        for q in p {
            v.push((skupina, meno(skupina, q.znacka), q.znacka, hodnota(q)));
        }
    }
    v
}

// ---------------------------------------------------------------- IPTC-IIM

/// Mená množín IPTC-IIM (záznam 2 – aplikačný; IPTC IIM 4.2).
fn meno_iptc(zaznam: u8, mnozina: u8) -> String {
    let m = match (zaznam, mnozina) {
        (1, 0) => "EnvelopeRecordVersion",
        (1, 90) => "CodedCharacterSet",
        (2, 0) => "ApplicationRecordVersion",
        (2, 5) => "ObjectName",
        (2, 10) => "Urgency",
        (2, 15) => "Category",
        (2, 20) => "SupplementalCategories",
        (2, 25) => "Keywords",
        (2, 40) => "SpecialInstructions",
        (2, 55) => "DateCreated",
        (2, 60) => "TimeCreated",
        (2, 80) => "By-line",
        (2, 85) => "By-lineTitle",
        (2, 90) => "City",
        (2, 92) => "Sub-location",
        (2, 95) => "Province-State",
        (2, 100) => "Country-PrimaryLocationCode",
        (2, 101) => "Country-PrimaryLocationName",
        (2, 103) => "OriginalTransmissionReference",
        (2, 105) => "Headline",
        (2, 110) => "Credit",
        (2, 115) => "Source",
        (2, 116) => "CopyrightNotice",
        (2, 120) => "Caption-Abstract",
        (2, 122) => "Writer-Editor",
        _ => "",
    };
    if m.is_empty() { format!("{zaznam}:{mnozina}") } else { m.to_string() }
}

/// Položky IPTC-IIM: (meno, text). Text je UTF-8, ak to určuje množina 1:90 (ESC % G), inak Latin-1.
pub fn polozky_iptc(d: &[u8]) -> Vec<(String, String)> {
    let mut surove = Vec::new();
    let mut i = 0;
    while i + 5 <= d.len() && d[i] == 0x1C {
        let (zaznam, mnozina) = (d[i + 1], d[i + 2]);
        let mut n = u16::from_be_bytes([d[i + 3], d[i + 4]]) as usize;
        let mut zac = i + 5;
        if n & 0x8000 != 0 {
            // rozšírená dĺžka: n & 0x7FFF bajtov s dĺžkou
            let k = n & 0x7FFF;
            if k > 4 || zac + k > d.len() {
                break;
            }
            n = d[zac..zac + k].iter().fold(0usize, |a, &b| (a << 8) | b as usize);
            zac += k;
        }
        let Some(h) = zac.checked_add(n).and_then(|k| d.get(zac..k)) else { break };
        surove.push((zaznam, mnozina, h));
        i = zac + n;
    }
    let utf8 = surove.iter().any(|&(z, m, h)| z == 1 && m == 90 && h == b"\x1B%G");
    surove
        .into_iter()
        .filter(|&(z, m, _)| !(z == 1 && m == 90))
        .map(|(z, m, h)| {
            // verzie záznamov sú dvojbajtové čísla
            let t = if m == 0 && h.len() == 2 {
                u16::from_be_bytes([h[0], h[1]]).to_string()
            } else if utf8 {
                String::from_utf8_lossy(h).into_owned()
            } else {
                h.iter().map(|&b| b as char).collect()
            };
            (meno_iptc(z, m), t)
        })
        .collect()
}

/// IPTC zo zdrojov Photoshopu (JPEG APP13 „Photoshop 3.0“): obsah zdroja 8BIM 0x0404.
pub fn iptc_z_photoshopu(seg: &[u8]) -> Option<Vec<u8>> {
    iptc_z_8bim(seg.strip_prefix(b"Photoshop 3.0\0")?)
}

/// IPTC zo zoznamu zdrojov 8BIM (aj bez hlavičky APP13 – tak ho ukladá ExifTool do PNG).
pub fn iptc_z_8bim(mut d: &[u8]) -> Option<Vec<u8>> {
    while d.len() >= 12 && &d[..4] == b"8BIM" {
        let id = u16::from_be_bytes([d[4], d[5]]);
        let meno = d[6] as usize;
        let za_menom = 6 + (1 + meno).div_ceil(2) * 2;
        let n = u32_v(d, za_menom, false)? as usize;
        let zac = za_menom + 4;
        let h = d.get(zac..zac.checked_add(n)?)?;
        if id == 0x0404 {
            return Some(h.to_vec());
        }
        d = d.get(zac + n.div_ceil(2) * 2..)?;
    }
    None
}

/// Segment APP13 (bez značky a dĺžky) so zdrojom IPTC.
pub fn photoshop_s_iptc(iptc: &[u8]) -> Vec<u8> {
    let mut v = b"Photoshop 3.0\08BIM\x04\x04\0\0".to_vec();
    v.extend_from_slice(&(iptc.len() as u32).to_be_bytes());
    v.extend_from_slice(iptc);
    if iptc.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// Text „Raw profile type …“ (konvencia ImageMagick v PNG): „\n<typ>\n<dĺžka>\n<šestnástkovo>“.
pub fn surovy_profil(text: &str) -> Option<Vec<u8>> {
    let mut r = text.trim_start().splitn(3, '\n');
    let _typ = r.next()?;
    let n: usize = r.next()?.trim().parse().ok()?;
    let hex: Vec<u8> = r.next()?.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    if hex.len() < 2 * n || n > 64 << 20 {
        return None;
    }
    let cif = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
    Some(hex.chunks_exact(2).take(n).map(|c| cif(c[0]) << 4 | cif(c[1])).collect())
}

/// Opak `surovy_profil`.
pub fn zapis_surovy_profil(typ: &str, d: &[u8]) -> String {
    let mut s = format!("\n{typ}\n{:8}\n", d.len());
    for (i, b) in d.iter().enumerate() {
        s.push_str(&format!("{b:02x}"));
        if i % 36 == 35 {
            s.push('\n');
        }
    }
    s.push('\n');
    s
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn natocenie_tam_a_spat() {
        for n in 1..=8 {
            assert_eq!(natocenie(&s_natocenim(n)), Some(n));
        }
        let mut s = b"Exif\0\0".to_vec();
        s.extend_from_slice(&s_natocenim(6));
        assert_eq!(natocenie(&s), Some(6));
        assert_eq!(natocenie(b"MM\x00\x2A\x00\x00\x00\x08\xFF\xFF"), None);
        assert_eq!(natocenie(b"xx"), None);
    }

    #[test]
    fn exif_rozobrat_a_zapisat() {
        let text = |z: u16, t: &str| Polozka {
            znacka: z,
            typ: 2,
            pocet: t.len() as u32 + 1,
            hodnota: [t.as_bytes(), b"\0"].concat(),
        };
        let rac = |z: u16, a: u32, b: u32| Polozka {
            znacka: z,
            typ: 5,
            pocet: 1,
            hodnota: [a.to_le_bytes(), b.to_le_bytes()].concat(),
        };
        let e = Exif {
            ifd0: vec![text(271, "Výrobca"), text(272, "Model X")],
            exif: vec![rac(33434, 1, 250), text(36867, "2026:11:25 12:00:00")],
            gps: vec![Polozka { znacka: 0, typ: 1, pocet: 4, hodnota: vec![2, 3, 0, 0] }],
            interop: vec![text(1, "R98")],
        };
        let b = e.zapis();
        assert_eq!(rozober(&b).unwrap(), e);
        let p = polozky(&b);
        assert!(p.iter().any(|q| q.1 == "ExposureTime" && q.3 == Hodnota::Cisla(vec![0.004])));
        for i in 0..b.len() {
            let _ = polozky(&b[..i]);
        }
    }

    #[test]
    fn iptc_tam_a_spat() {
        let mut d = b"\x1C\x01\x5A\x00\x03\x1B%G".to_vec();
        for (m, t) in [(25u8, "kvet"), (25, "orchidea"), (120, "Popis – ž")] {
            d.extend_from_slice(&[0x1C, 2, m]);
            d.extend_from_slice(&(t.len() as u16).to_be_bytes());
            d.extend_from_slice(t.as_bytes());
        }
        let p = polozky_iptc(&d);
        assert_eq!(p[2], ("Caption-Abstract".into(), "Popis – ž".into()));
        assert_eq!(iptc_z_photoshopu(&photoshop_s_iptc(&d)).unwrap(), d);
        assert_eq!(surovy_profil(&zapis_surovy_profil("iptc", &d)).unwrap(), d);
    }
}
