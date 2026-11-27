//! Značky (metadáta) zvukových súborov – názov, interpret, album, … a **ReplayGain** pre LÝRU.
//!
//! **Zdroje (text normy):** ID3v2.4.0 (`id3v2.40-structure.txt`, `id3v2.4.0-frames.txt`) a ID3v2.3.0
//! (`id3v2.3.0.txt` vrátane prílohy A – žánre) z oficiálneho repozitára id3; Vorbis comment (Vorbis I,
//! `v-comment.html`, Xiph.Org); blok VORBIS_COMMENT podľa RFC 9639 (bez rámcovacieho bitu); značky Matroska podľa
//! RFC 9559 a `matroska_tags.xml` (IETF CELLAR) – v `mkv.rs`; ReplayGain 1.0/2.0 (Hydrogenaudio): hodnoty
//! `REPLAYGAIN_TRACK_GAIN` „−6.20 dB“, `REPLAYGAIN_TRACK_PEAK` „0.988“, album rovnako.
//! **Bez textu normy, overené rozhodcom** (ffprobe): ID3v2.2, ID3v1, APEv2, `ilst` v MP4.
//!
//! **Spoločný slovník kľúčov** je slovník Vorbis comment veľkými písmenami: `TITLE`, `ARTIST`, `ALBUM`,
//! `ALBUMARTIST`, `DATE`, `TRACKNUMBER`, `DISCNUMBER`, `GENRE`, `COMPOSER`, `COMMENT` …; bežné ekvivalenty iných
//! formátov sa naň prevedú ([`spolocny_kluc`]), ostatné kľúče ostanú tak, ako sú v súbore (popis TXXX, kľúč
//! APEv2, TagName Matroska, meno voľnej položky MP4), tiež veľkými písmenami.
//! Kľúč sa môže opakovať (viac interpretov) – [`Znacky::hodnota`] vráti prvú hodnotu.
//!
//! Nikdy pád: poškodená značka sa preskočí, prečíta sa, čo sa dá.

use std::borrow::Cow;

/// Najviac položiek v jednom súbore (poškodený súbor s miliónmi prázdnych položiek nezaplní pamäť).
const NAJVIAC_POLOZIEK: usize = 4096;

/// Najväčší obrázok obalu (bajty) – väčší sa vynechá (R211; návrh rozhrania s reláciou C).
pub const NAJVIAC_OBAL: usize = 16 << 20;
/// Najviac obrázkov v jednom súbore.
const NAJVIAC_OBALOV: usize = 16;

/// Značky súboru: dvojice (kľúč veľkými písmenami, hodnota) v poradí zo súboru a priložené obrázky.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Znacky {
    polozky: Vec<(String, String)>,
    obaly: Vec<Obal>,
}

/// Obrázok priložený k zvuku (obal albumu) – len bajty a typ; dekóduje ho LÝRA (vrstva 3, `orchidea_formaty`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obal {
    /// MIME typ (`image/jpeg`, `image/png` …); kde ho formát nemá, podľa podpisu dát
    pub mime: String,
    /// typ obrázka podľa ID3v2.4 §4.14 (aj FLAC PICTURE): 3 = predná strana obalu, 4 = zadná, 0 = iný …
    pub druh: u8,
    pub popis: String,
    pub data: Vec<u8>,
}

/// MIME podľa podpisu dát (JPEG, PNG, GIF, BMP, WebP), inak „image/“ (ID3v2.4 §4.14: chýbajúci typ = image/).
fn mime_z_dat(d: &[u8]) -> &'static str {
    match d {
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => "image/",
    }
}

/// ReplayGain: odporúčané zosilnenie v dB a vrchol (lineárny, 1,0 = plný rozsah) pre skladbu a album.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReplayGain {
    pub stopa_db: Option<f32>,
    pub stopa_vrchol: Option<f32>,
    pub album_db: Option<f32>,
    pub album_vrchol: Option<f32>,
}

impl Znacky {
    pub fn nove() -> Znacky {
        Znacky::default()
    }

    /// Pridá položku; kľúč sa prevedie na veľké písmená (ASCII), prázdna hodnota a koncové nuly sa vynechajú.
    pub fn pridaj(&mut self, kluc: &str, hodnota: &str) {
        let kluc = spolocny_kluc(&kluc.trim().to_ascii_uppercase()).to_string();
        let hodnota = hodnota.trim_end_matches('\0');
        if kluc.is_empty() || hodnota.is_empty() || self.polozky.len() >= NAJVIAC_POLOZIEK {
            return;
        }
        self.polozky.push((kluc, hodnota.to_string()));
    }

    /// Prvá hodnota kľúča (bez ohľadu na veľkosť písmen).
    pub fn hodnota(&self, kluc: &str) -> Option<&str> {
        self.polozky.iter().find(|(k, _)| k.eq_ignore_ascii_case(kluc)).map(|(_, v)| v.as_str())
    }

    /// Všetky hodnoty kľúča v poradí zo súboru.
    pub fn hodnoty<'a>(&'a self, kluc: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.polozky.iter().filter(move |(k, _)| k.eq_ignore_ascii_case(kluc)).map(|(_, v)| v.as_str())
    }

    pub fn polozky(&self) -> &[(String, String)] {
        &self.polozky
    }

    pub fn je_prazdne(&self) -> bool {
        self.polozky.is_empty() && self.obaly.is_empty()
    }

    /// Pridá obrázok; prázdny, väčší ako [`NAJVIAC_OBAL`] alebo len odkaz (MIME „-->“) sa vynechá.
    pub fn pridaj_obal(&mut self, mime: &str, druh: u8, popis: &str, data: &[u8]) {
        if data.is_empty() || data.len() > NAJVIAC_OBAL || mime == "-->" || self.obaly.len() >= NAJVIAC_OBALOV
        {
            return;
        }
        let mime = if mime.is_empty() || mime == "image/" { mime_z_dat(data) } else { mime };
        let mime = if mime.contains('/') {
            mime.to_ascii_lowercase()
        } else {
            format!("image/{mime}").to_ascii_lowercase()
        };
        self.obaly.push(Obal { mime, druh, popis: popis.to_string(), data: data.to_vec() });
    }

    /// Všetky obrázky v poradí zo súboru.
    pub fn obaly(&self) -> &[Obal] {
        &self.obaly
    }

    /// Obal albumu: prvý obrázok typu 3 (predná strana), inak prvý obrázok.
    pub fn obal(&self) -> Option<&Obal> {
        self.obaly.iter().find(|o| o.druh == 3).or_else(|| self.obaly.first())
    }

    /// Pripojí všetky položky a obrázky (ďalšia časť tých istých značiek).
    pub fn pripoj(&mut self, ine: Znacky) {
        for (k, v) in ine.polozky {
            self.pridaj(&k, &v);
        }
        for o in ine.obaly {
            if self.obaly.len() < NAJVIAC_OBALOV {
                self.obaly.push(o);
            }
        }
    }

    /// Doplní kľúče, ktoré ešte nemá (pri viacerých značkách v súbore má prednosť prvá – ID3v2 pred APEv2
    /// a ID3v1).
    pub fn dopln(&mut self, ine: Znacky) {
        let Znacky { polozky, obaly } = ine;
        let nove: Vec<_> =
            polozky.into_iter().filter(|(k, _)| !self.polozky.iter().any(|(m, _)| m == k)).collect();
        for (k, v) in nove {
            self.pridaj(&k, &v);
        }
        if self.obaly.is_empty() {
            self.obaly = obaly;
        }
    }

    /// ReplayGain z kľúčov `REPLAYGAIN_TRACK_GAIN`, `REPLAYGAIN_TRACK_PEAK`, `REPLAYGAIN_ALBUM_GAIN`,
    /// `REPLAYGAIN_ALBUM_PEAK`; pri Opus z `R128_TRACK_GAIN` a `R128_ALBUM_GAIN` ([`r128_db`]). `None`, ak nie je
    /// ani jedno zosilnenie.
    pub fn replaygain(&self) -> Option<ReplayGain> {
        let rg = ReplayGain {
            stopa_db: self
                .hodnota("REPLAYGAIN_TRACK_GAIN")
                .and_then(decibely)
                .or_else(|| self.hodnota("R128_TRACK_GAIN").and_then(r128_db)),
            stopa_vrchol: self.hodnota("REPLAYGAIN_TRACK_PEAK").and_then(vrchol),
            album_db: self
                .hodnota("REPLAYGAIN_ALBUM_GAIN")
                .and_then(decibely)
                .or_else(|| self.hodnota("R128_ALBUM_GAIN").and_then(r128_db)),
            album_vrchol: self.hodnota("REPLAYGAIN_ALBUM_PEAK").and_then(vrchol),
        };
        (rg.stopa_db.is_some() || rg.album_db.is_some()).then_some(rg)
    }
}

/// Kľúč spoločného slovníka pre bežné ekvivalenty z iných formátov: Vorbis `DESCRIPTION` (Vorbis I – opis
/// obsahu, jeho „komentár“), APEv2 `Year`, `Track`, `Disc`, `Album Artist`, Matroska `PART_NUMBER`,
/// `DATE_RELEASED` (`matroska_tags.xml`) a zaužívané `ALBUM_ARTIST`, `DISC`.
fn spolocny_kluc(k: &str) -> &str {
    match k {
        "DESCRIPTION" => "COMMENT",
        "ALBUM ARTIST" | "ALBUM_ARTIST" => "ALBUMARTIST",
        "YEAR" | "DATE_RELEASED" => "DATE",
        "TRACK" | "PART_NUMBER" => "TRACKNUMBER",
        "DISC" => "DISCNUMBER",
        _ => k,
    }
}

/// Zosilnenie ReplayGain: „−6.20 dB“, „+3.5 dB“, „-6.2dB“ (jednotka bez ohľadu na veľkosť písmen; desatinná
/// čiarka sa pripustí, niektoré zapisovače ju dávajú).
pub fn decibely(s: &str) -> Option<f32> {
    let s = s.trim();
    let s = match s.len().checked_sub(2) {
        Some(i) if s.is_char_boundary(i) && s[i..].eq_ignore_ascii_case("db") => s[..i].trim_end(),
        _ => s,
    };
    let x: f64 = s.replace(',', ".").parse().ok()?;
    (x.is_finite() && x.abs() < 1000.0).then_some(x as f32)
}

/// Zosilnenie z `R128_TRACK_GAIN`/`R128_ALBUM_GAIN` (RFC 7845 §5.2: celé číslo −32768 až 32767, najviac 6 znakov,
/// voliteľné znamienko, Q7.8 dB voči úrovni EBU R128 −23 LUFS, navyše k výstupnému zisku OpusHead, ktorý už
/// uplatní dekodér) prevedené na úroveň ReplayGain 2.0 (−18 LUFS): dB = hodnota / 256 + 5.
pub fn r128_db(s: &str) -> Option<f32> {
    let cisla = s.strip_prefix(['+', '-']).unwrap_or(s);
    if s.len() > 6 || cisla.is_empty() || !cisla.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let q: i32 = s.parse().ok()?;
    (-32768..=32767).contains(&q).then(|| q as f32 / 256.0 + 5.0)
}

/// Vrchol ReplayGain: lineárna amplitúda ≥ 0 (1,0 = plný rozsah, môže byť aj viac).
pub fn vrchol(s: &str) -> Option<f32> {
    let x: f64 = s.trim().replace(',', ".").parse().ok()?;
    (x.is_finite() && (0.0..1000.0).contains(&x)).then_some(x as f32)
}

fn le32(d: &[u8], i: usize) -> Option<usize> {
    d.get(i..i.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn be32(d: &[u8], i: usize) -> Option<usize> {
    d.get(i..i.checked_add(4)?).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// Celé číslo „synchsafe“ (ID3v2.4 §6.2): 4 bajty po 7 bitoch; `None`, ak má niektorý bajt najvyšší bit.
fn synchsafe(d: &[u8]) -> Option<usize> {
    let b = d.get(..4)?;
    b.iter().all(|&x| x < 0x80).then(|| b.iter().fold(0usize, |x, &y| (x << 7) | y as usize))
}

/// Vorbis comment (Vorbis I, „Structure“; FLAC RFC 9639 – blok VORBIS_COMMENT bez rámcovacieho bitu): dĺžka
/// a reťazec dodávateľa, počet položiek, položky `KĽÚČ=hodnota` v UTF-8, všetky dĺžky 32 bitov little-endian.
pub fn vorbis_comment(d: &[u8]) -> Option<Znacky> {
    let dodavatel = le32(d, 0)?;
    let mut i = 4usize.checked_add(dodavatel)?;
    let pocet = le32(d, i)?;
    i += 4;
    let mut z = Znacky::nove();
    for _ in 0..pocet {
        let Some(n) = le32(d, i) else { break };
        let Some(polozka) = d.get(i + 4..(i + 4).saturating_add(n)) else { break };
        i += 4 + n;
        // obal: METADATA_BLOCK_PICTURE = base64 bloku PICTURE z FLAC (Vorbis/Opus v Ogg)
        if polozka.len() > 23 && polozka[..23].eq_ignore_ascii_case(b"METADATA_BLOCK_PICTURE=") {
            if let Some(blok) = base64(&polozka[23..]) {
                flac_picture(&mut z, &blok);
            }
            continue;
        }
        let text = String::from_utf8_lossy(polozka);
        if let Some((k, v)) = text.split_once('=') {
            z.pridaj(k, v);
        }
    }
    Some(z)
}

/// Blok PICTURE vo FLAC (RFC 9639, typ bloku 6): typ obrázka (ako ID3v2.4 APIC), dĺžka a MIME, dĺžka a popis
/// UTF-8, šírka, výška, hĺbka, počet farieb, dĺžka a dáta – čísla 32 bitov big-endian.
pub fn flac_picture(z: &mut Znacky, d: &[u8]) {
    let precitaj = || -> Option<(u8, String, String, &[u8])> {
        let druh = be32(d, 0)?;
        let n = be32(d, 4)?;
        let mime = latin1(d.get(8..8usize.checked_add(n)?)?);
        let mut i = 8 + n;
        let m = be32(d, i)?;
        let popis = String::from_utf8_lossy(d.get(i + 4..(i + 4).checked_add(m)?)?).into_owned();
        i += 4 + m + 16;
        let dlzka = be32(d, i)?;
        let data = d.get(i + 4..(i + 4).checked_add(dlzka)?)?;
        Some((u8::try_from(druh).unwrap_or(0), mime, popis, data))
    };
    if let Some((druh, mime, popis, data)) = precitaj() {
        z.pridaj_obal(&mime, druh, &popis, data);
    }
}

/// Base64 (RFC 4648 §4: abeceda A–Z, a–z, 0–9, „+“, „/“, doplnenie „=“; biele znaky sa preskočia). `None` pri
/// neplatnom znaku alebo výsledku nad [`NAJVIAC_OBAL`] + 1 MB.
fn base64(d: &[u8]) -> Option<Vec<u8>> {
    if d.len() / 4 * 3 > NAJVIAC_OBAL + (1 << 20) {
        return None;
    }
    let mut vysl = Vec::with_capacity(d.len() / 4 * 3);
    let (mut zasobnik, mut bitov) = (0u32, 0u32);
    for &b in d {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        zasobnik = (zasobnik << 6) | v as u32;
        bitov += 6;
        if bitov >= 8 {
            bitov -= 8;
            vysl.push((zasobnik >> bitov) as u8);
            zasobnik &= (1 << bitov) - 1;
        }
    }
    Some(vysl)
}

/// Odstránenie nesynchronizácie (ID3v2.4 §6.1): každá dvojica `FF 00` sa zmení na `FF`.
fn odsynchronizuj(d: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(d.len());
    let mut predosly_ff = false;
    for &b in d {
        if !(predosly_ff && b == 0) {
            v.push(b);
        }
        predosly_ff = b == 0xFF;
    }
    v
}

/// Reťazce textového rámca podľa kódovania (ID3v2.4 §4: 0 ISO-8859-1, 1 UTF-16 s BOM, 2 UTF-16BE, 3 UTF-8),
/// oddelené ukončovacím znakom kódovania.
fn texty(kodovanie: u8, d: &[u8]) -> Vec<String> {
    match kodovanie {
        1 | 2 => {
            let mut vysl = Vec::new();
            let mut zaciatok = 0;
            let mut i = 0;
            while i + 1 < d.len() {
                if d[i] == 0 && d[i + 1] == 0 {
                    vysl.push(utf16(&d[zaciatok..i]));
                    zaciatok = i + 2;
                }
                i += 2;
            }
            if zaciatok < d.len() {
                vysl.push(utf16(&d[zaciatok..]));
            }
            vysl
        }
        _ => d
            .split(|&b| b == 0)
            .map(|s| if kodovanie == 3 { String::from_utf8_lossy(s).into_owned() } else { latin1(s) })
            .collect(),
    }
}

fn latin1(d: &[u8]) -> String {
    d.iter().map(|&b| b as char).collect()
}

/// UTF-16: poradie bajtov podľa BOM (FF FE little-endian, FE FF big-endian), bez BOM big-endian (UTF-16BE).
fn utf16(d: &[u8]) -> String {
    let (le, d) = match d {
        [0xFF, 0xFE, zvysok @ ..] => (true, zvysok),
        [0xFE, 0xFF, zvysok @ ..] => (false, zvysok),
        _ => (false, d),
    };
    let jednotky = d
        .chunks_exact(2)
        .map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    char::decode_utf16(jednotky).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
}

/// Žánre ID3v1 – ID3v2.3.0, príloha A (0–79 ID3v1, 80–125 rozšírenia uvedené tamže).
pub(crate) const ZANRE_ID3V1: [&str; 126] = [
    "Blues",
    "Classic Rock",
    "Country",
    "Dance",
    "Disco",
    "Funk",
    "Grunge",
    "Hip-Hop",
    "Jazz",
    "Metal",
    "New Age",
    "Oldies",
    "Other",
    "Pop",
    "R&B",
    "Rap",
    "Reggae",
    "Rock",
    "Techno",
    "Industrial",
    "Alternative",
    "Ska",
    "Death Metal",
    "Pranks",
    "Soundtrack",
    "Euro-Techno",
    "Ambient",
    "Trip-Hop",
    "Vocal",
    "Jazz+Funk",
    "Fusion",
    "Trance",
    "Classical",
    "Instrumental",
    "Acid",
    "House",
    "Game",
    "Sound Clip",
    "Gospel",
    "Noise",
    "AlternRock",
    "Bass",
    "Soul",
    "Punk",
    "Space",
    "Meditative",
    "Instrumental Pop",
    "Instrumental Rock",
    "Ethnic",
    "Gothic",
    "Darkwave",
    "Techno-Industrial",
    "Electronic",
    "Pop-Folk",
    "Eurodance",
    "Dream",
    "Southern Rock",
    "Comedy",
    "Cult",
    "Gangsta",
    "Top 40",
    "Christian Rap",
    "Pop/Funk",
    "Jungle",
    "Native American",
    "Cabaret",
    "New Wave",
    "Psychadelic",
    "Rave",
    "Showtunes",
    "Trailer",
    "Lo-Fi",
    "Tribal",
    "Acid Punk",
    "Acid Jazz",
    "Polka",
    "Retro",
    "Musical",
    "Rock & Roll",
    "Hard Rock",
    "Folk",
    "Folk-Rock",
    "National Folk",
    "Swing",
    "Fast Fusion",
    "Bebob",
    "Latin",
    "Revival",
    "Celtic",
    "Bluegrass",
    "Avantgarde",
    "Gothic Rock",
    "Progressive Rock",
    "Psychedelic Rock",
    "Symphonic Rock",
    "Slow Rock",
    "Big Band",
    "Chorus",
    "Easy Listening",
    "Acoustic",
    "Humour",
    "Speech",
    "Chanson",
    "Opera",
    "Chamber Music",
    "Sonata",
    "Symphony",
    "Booty Bass",
    "Primus",
    "Porn Groove",
    "Satire",
    "Slow Jam",
    "Club",
    "Tango",
    "Samba",
    "Folklore",
    "Ballad",
    "Power Ballad",
    "Rhythmic Soul",
    "Freestyle",
    "Duet",
    "Punk Rock",
    "Drum Solo",
    "A capella",
    "Euro-House",
    "Dance Hall",
];

/// Žáner ID3v2: „(13)“, „(13)Pop“, „13“ (ID3v2.4), „(RX)“ remix, „(CR)“ cover – ID3v2.3 §4.2.1 TCON.
fn zaner(s: &str) -> String {
    let meno = |n: &str| n.parse::<usize>().ok().and_then(|i| ZANRE_ID3V1.get(i)).map(|m| m.to_string());
    if let Some(zvysok) = s.strip_prefix('(')
        && let Some((odkaz, za)) = zvysok.split_once(')')
    {
        if !za.is_empty() {
            return za.to_string();
        }
        return match odkaz {
            "RX" => "Remix".into(),
            "CR" => "Cover".into(),
            n => meno(n).unwrap_or_else(|| s.to_string()),
        };
    }
    meno(s).unwrap_or_else(|| s.to_string())
}

/// Kľúč spoločného slovníka pre textový rámec ID3v2 (2.3/2.4 štvorznakové, 2.2 trojznakové).
fn kluc_ramca(id: &[u8]) -> Option<&'static str> {
    Some(match id {
        b"TIT2" | b"TT2" => "TITLE",
        b"TPE1" | b"TP1" => "ARTIST",
        b"TPE2" | b"TP2" => "ALBUMARTIST",
        b"TPE3" | b"TP3" => "PERFORMER",
        b"TALB" | b"TAL" => "ALBUM",
        b"TCOM" | b"TCM" => "COMPOSER",
        b"TCON" | b"TCO" => "GENRE",
        b"TCOP" | b"TCR" => "COPYRIGHT",
        b"TENC" | b"TEN" => "ENCODEDBY",
        b"TSSE" | b"TSS" => "ENCODER",
        b"TLAN" | b"TLA" => "LANGUAGE",
        b"TPUB" | b"TPB" => "PUBLISHER",
        b"TRCK" | b"TRK" => "TRACKNUMBER",
        b"TPOS" | b"TPA" => "DISCNUMBER",
        b"TDRC" | b"TYER" | b"TYE" => "DATE",
        _ => return None,
    })
}

/// Značka ID3v2 na začiatku `d` (verzie 2.2, 2.3, 2.4): (značky, celková dĺžka značky). Neznáma verzia alebo
/// komprimovaná značka 2.2 sa preskočí (prázdne značky, správna dĺžka).
pub fn id3v2(d: &[u8]) -> Option<(Znacky, usize)> {
    if d.len() < 10 || &d[..3] != b"ID3" || d[3] == 0xFF || d[4] == 0xFF {
        return None;
    }
    let (verzia, priznaky) = (d[3], d[5]);
    let velkost = synchsafe(&d[6..10])?;
    let celkom = 10 + velkost + if verzia == 4 && priznaky & 0x10 != 0 { 10 } else { 0 };
    let mut z = Znacky::nove();
    if !(2..=4).contains(&verzia) || (verzia == 2 && priznaky & 0x40 != 0) {
        return Some((z, celkom));
    }
    let telo = &d[10..(10 + velkost).min(d.len())];
    // 2.2 a 2.3: nesynchronizácia celej značky (2.3 §5); 2.4 po rámcoch (§4.1.2 n)
    let telo: Cow<[u8]> = if verzia < 4 && priznaky & 0x80 != 0 {
        Cow::Owned(odsynchronizuj(telo))
    } else {
        Cow::Borrowed(telo)
    };
    let mut i = 0usize;
    if verzia >= 3 && priznaky & 0x40 != 0 {
        // rozšírená hlavička: 2.3 – veľkosť bez seba samej (§3.2), 2.4 – celá, synchsafe (§3.2)
        i = match verzia {
            3 => be32(&telo, 0).map_or(telo.len(), |n| n.saturating_add(4)),
            _ => synchsafe(&telo).unwrap_or(telo.len()),
        };
    }
    let (dlzka_id, dlzka_hlavy) = if verzia == 2 { (3, 6) } else { (4, 10) };
    while i + dlzka_hlavy <= telo.len() {
        let id = &telo[i..i + dlzka_id];
        if id[0] == 0 {
            break; // výplň
        }
        let n = match verzia {
            2 => telo[i + 3..i + 6].iter().fold(0usize, |x, &b| (x << 8) | b as usize),
            3 => be32(&telo, i + 4).unwrap_or(usize::MAX),
            _ => synchsafe(&telo[i + 4..]).unwrap_or(usize::MAX),
        };
        let format = if verzia == 2 { 0 } else { telo[i + 9] };
        let zac = i + dlzka_hlavy;
        let Some(ramec) = zac.checked_add(n).and_then(|k| telo.get(zac..k)) else { break };
        i = zac + n;
        let ramec: Cow<[u8]> = match verzia {
            // 2.3 §3.3.1: i – kompresia (+4 B), j – šifrovanie (+1 B), k – skupina (+1 B)
            3 => {
                if format & 0xC0 != 0 {
                    continue;
                }
                Cow::Borrowed(ramec.get(if format & 0x20 != 0 { 1 } else { 0 }..).unwrap_or_default())
            }
            // 2.4 §4.1.2: h – skupina (+1 B), k – kompresia, m – šifrovanie, n – nesynchronizácia,
            // p – indikátor dĺžky (+4 B)
            4 => {
                if format & 0x0C != 0 {
                    continue;
                }
                let posun = (format & 0x40 != 0) as usize + if format & 0x01 != 0 { 4 } else { 0 };
                let r = ramec.get(posun..).unwrap_or_default();
                if format & 0x02 != 0 || priznaky & 0x80 != 0 {
                    Cow::Owned(odsynchronizuj(r))
                } else {
                    Cow::Borrowed(r)
                }
            }
            _ => Cow::Borrowed(ramec),
        };
        ramec_id3(&mut z, id, &ramec);
    }
    // ID3v2.3 §4.2.1: TYER (rok) a TDAT (DDMM) → DATE „RRRR-MM-DD“; TDAT inde nepatrí do slovníka
    if let Some(den) = z.polozky.iter().position(|(k, _)| k == "TDAT") {
        let (_, ddmm) = z.polozky.remove(den);
        if verzia == 3
            && let Some(rok) = z.polozky.iter_mut().find(|(k, _)| k == "DATE")
            && rok.1.len() == 4
            && ddmm.len() == 4
            && ddmm.bytes().all(|b| b.is_ascii_digit())
        {
            rok.1 = format!("{}-{}-{}", rok.1, &ddmm[2..], &ddmm[..2]);
        }
    }
    Some((z, celkom))
}

/// Jeden rámec ID3v2 do značiek: textové rámce (ID3v2.4 §4.2), TXXX (§4.2.6), COMM bez popisu (§4.10).
fn ramec_id3(z: &mut Znacky, id: &[u8], d: &[u8]) {
    let Some((&kodovanie, d)) = d.split_first() else { return };
    match id {
        b"TXXX" | b"TXX" => {
            let t = texty(kodovanie, d);
            if let [popis, hodnoty @ ..] = t.as_slice() {
                for h in hodnoty {
                    z.pridaj(popis, h);
                }
            }
        }
        b"COMM" | b"COM" => {
            let Some(d) = d.get(3..) else { return };
            let t = texty(kodovanie, d);
            if let [popis, text, ..] = t.as_slice()
                && popis.is_empty()
            {
                z.pridaj("COMMENT", text);
            }
        }
        b"TDAT" => {
            if let Some(t) = texty(kodovanie, d).first() {
                z.pridaj("TDAT", t);
            }
        }
        // ID3v2.4 §4.14 APIC: MIME (ISO-8859-1, ukončený), typ, popis (podľa kódovania), dáta;
        // ID3v2.2 PIC: formát 3 znaky („JPG“, „PNG“) namiesto MIME
        b"APIC" | b"PIC" => {
            let (mime, d) = if id == b"PIC" {
                let Some(f) = d.get(..3) else { return };
                let mime = match f {
                    b"JPG" => "image/jpeg".to_string(),
                    b"PNG" => "image/png".to_string(),
                    _ => format!("image/{}", latin1(f).to_ascii_lowercase()),
                };
                (mime, &d[3..])
            } else {
                let Some(k) = d.iter().position(|&b| b == 0) else { return };
                (latin1(&d[..k]), &d[k + 1..])
            };
            let Some((&druh, d)) = d.split_first() else { return };
            let (popis, data) = match kodovanie {
                1 | 2 => {
                    let k = (0..d.len().saturating_sub(1)).step_by(2).find(|&i| d[i] == 0 && d[i + 1] == 0);
                    let Some(k) = k else { return };
                    (utf16(&d[..k]), &d[k + 2..])
                }
                _ => {
                    let Some(k) = d.iter().position(|&b| b == 0) else { return };
                    let p = &d[..k];
                    (
                        if kodovanie == 3 { String::from_utf8_lossy(p).into_owned() } else { latin1(p) },
                        &d[k + 1..],
                    )
                }
            };
            z.pridaj_obal(&mime, druh, &popis, data);
        }
        _ => {
            let Some(kluc) = kluc_ramca(id) else { return };
            for t in texty(kodovanie, d) {
                let t = if kluc == "GENRE" { zaner(&t) } else { t };
                z.pridaj(kluc, &t);
            }
        }
    }
}

/// ID3v1 (posledných 128 B súboru): „TAG“, názov 30, interpret 30, album 30, rok 4, komentár 30, žáner 1;
/// ID3v1.1 – číslo skladby v poslednom bajte komentára, ak je pred ním nula.
pub fn id3v1(d: &[u8]) -> Option<Znacky> {
    if d.len() != 128 || &d[..3] != b"TAG" {
        return None;
    }
    // text ISO-8859-1; niektoré zapisovače (FFmpeg) dávajú UTF-8 – platné UTF-8 sa číta ako UTF-8
    // (bajty ISO-8859-1 nad 0x7F len zriedka tvoria platnú postupnosť UTF-8)
    let pole = |a: usize, b: usize| {
        let s = &d[a..b];
        let k = s.iter().position(|&x| x == 0).unwrap_or(s.len());
        let text = std::str::from_utf8(&s[..k]).map_or_else(|_| latin1(&s[..k]), str::to_string);
        text.trim_end().to_string()
    };
    let mut z = Znacky::nove();
    z.pridaj("TITLE", &pole(3, 33));
    z.pridaj("ARTIST", &pole(33, 63));
    z.pridaj("ALBUM", &pole(63, 93));
    z.pridaj("DATE", &pole(93, 97));
    if d[125] == 0 && d[126] != 0 {
        z.pridaj("COMMENT", &pole(97, 125));
        z.pridaj("TRACKNUMBER", &d[126].to_string());
    } else {
        z.pridaj("COMMENT", &pole(97, 127));
    }
    if let Some(m) = ZANRE_ID3V1.get(d[127] as usize) {
        z.pridaj("GENRE", m);
    }
    Some(z)
}

/// APEv2 (aj APEv1) z bajtov `d`, ktoré končia pätou „APETAGEX“ (32 B: verzia, veľkosť položiek s pätou, počet
/// položiek, príznaky – little-endian). Položka: dĺžka hodnoty, príznaky (bity 1–2: 0 = text UTF-8), kľúč
/// ukončený nulou, hodnota; viac hodnôt v texte oddeľuje nula. Vráti značky a dĺžku celej značky (s hlavičkou).
pub fn apev2(d: &[u8]) -> Option<(Znacky, usize)> {
    let pata = d.get(d.len().checked_sub(32)?..)?;
    if &pata[..8] != b"APETAGEX" {
        return None;
    }
    let velkost = le32(pata, 12)?;
    let pocet = le32(pata, 16)?;
    let priznaky = le32(pata, 20)?;
    let s_hlavickou = priznaky & (1 << 31) != 0;
    let celkom = velkost.checked_add(if s_hlavickou { 32 } else { 0 })?;
    let zaciatok = d.len().checked_sub(velkost)?;
    let polozky = &d[zaciatok..d.len() - 32];
    let mut z = Znacky::nove();
    let mut i = 0;
    for _ in 0..pocet {
        let (Some(n), Some(prz)) = (le32(polozky, i), le32(polozky, i + 4)) else { break };
        let Some(k) = polozky.get(i + 8..).and_then(|r| r.iter().position(|&b| b == 0)) else { break };
        let kluc = latin1(&polozky[i + 8..i + 8 + k]);
        let zac = i + 9 + k;
        let Some(hodnota) = zac.checked_add(n).and_then(|e| polozky.get(zac..e)) else { break };
        i = zac + n;
        match (prz >> 1) & 3 {
            0 => {
                for h in hodnota.split(|&b| b == 0) {
                    z.pridaj(&kluc, &String::from_utf8_lossy(h));
                }
            }
            // binárna položka „Cover Art (Front|Back|…)“: meno súboru ukončené nulou a obrázok
            1 if kluc.to_ascii_lowercase().starts_with("cover art") => {
                if let Some(k) = hodnota.iter().position(|&b| b == 0) {
                    let druh = if kluc.to_ascii_lowercase().contains("back") { 4 } else { 3 };
                    z.pridaj_obal("", druh, &String::from_utf8_lossy(&hodnota[..k]), &hodnota[k + 1..]);
                }
            }
            _ => {}
        }
    }
    Some((z, celkom))
}

/// Box MP4 v pamäti (8 B hlavička, veľkosť 1 = 64-bitová): (typ, obsah); koniec pri chybe.
fn boxy_mp4(d: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut p = 0usize;
    std::iter::from_fn(move || {
        let z = d.get(p..)?;
        let v = be32(z, 0)? as u64;
        let typ: [u8; 4] = z.get(4..8)?.try_into().ok()?;
        let (hlava, velkost) = match v {
            0 => (8, z.len() as u64),
            1 => (16, u64::from_be_bytes(z.get(8..16)?.try_into().ok()?)),
            _ => (8, v),
        };
        if velkost < hlava as u64 || velkost > z.len() as u64 {
            return None;
        }
        p += velkost as usize;
        Some((typ, &z[hlava..velkost as usize]))
    })
}

/// Kľúč spoločného slovníka pre položku `ilst` (MP4/M4A).
fn kluc_ilst(t: &[u8; 4]) -> Option<&'static str> {
    Some(match t {
        b"\xA9nam" => "TITLE",
        b"\xA9ART" => "ARTIST",
        b"aART" => "ALBUMARTIST",
        b"\xA9alb" => "ALBUM",
        b"\xA9wrt" => "COMPOSER",
        b"\xA9gen" => "GENRE",
        b"\xA9day" => "DATE",
        b"\xA9cmt" => "COMMENT",
        b"\xA9too" => "ENCODER",
        b"cprt" => "COPYRIGHT",
        _ => return None,
    })
}

/// Značky z boxu `udta` (MP4, M4A, MOV): `meta` (v ISO plný box, v QuickTime bez verzie) → `ilst`. Položky
/// s boxom `data` (4 B typ – 1 = UTF-8, 0 = binárne; 4 B jazyk; hodnota), `trkn`/`disk` (číslo/počet), `gnre`
/// (žáner ID3v1 + 1) a voľné položky `----` (`mean`, `name`, `data`).
pub fn mp4_udta(udta: &[u8]) -> Option<Znacky> {
    let (_, meta) = boxy_mp4(udta).find(|(t, _)| t == b"meta")?;
    // plný box: 4 B verzia a príznaky (nuly); v QuickTime meta začína priamo podboxom
    let meta = if meta.get(4..8).is_some_and(|t| t == b"hdlr") { meta } else { meta.get(4..)? };
    let (_, ilst) = boxy_mp4(meta).find(|(t, _)| t == b"ilst")?;
    let mut z = Znacky::nove();
    for (typ, polozka) in boxy_mp4(ilst) {
        let mut meno = None;
        for (t, o) in boxy_mp4(polozka) {
            match &t {
                b"name" => meno = o.get(4..).map(|n| String::from_utf8_lossy(n).into_owned()),
                b"data" => {
                    let (Some(druh), Some(h)) = (be32(o, 0), o.get(8..)) else { continue };
                    let text = || String::from_utf8_lossy(h).into_owned();
                    match &typ {
                        b"----" => {
                            if let Some(m) = &meno {
                                z.pridaj(m, &text());
                            }
                        }
                        b"trkn" | b"disk" if h.len() >= 6 => {
                            let (c, n) = (u16::from_be_bytes([h[2], h[3]]), u16::from_be_bytes([h[4], h[5]]));
                            let kluc = if &typ == b"trkn" { "TRACKNUMBER" } else { "DISCNUMBER" };
                            if c > 0 {
                                z.pridaj(kluc, &if n > 0 { format!("{c}/{n}") } else { c.to_string() });
                            }
                        }
                        // obal: typ dát 13 JPEG, 14 PNG, 27 BMP (inak podľa podpisu)
                        b"covr" => {
                            let mime = match druh & 0xFF_FFFF {
                                13 => "image/jpeg",
                                14 => "image/png",
                                27 => "image/bmp",
                                _ => "",
                            };
                            z.pridaj_obal(mime, 3, "", h);
                        }
                        b"gnre" if h.len() >= 2 => {
                            let i = u16::from_be_bytes([h[0], h[1]]) as usize;
                            if let Some(m) = i.checked_sub(1).and_then(|i| ZANRE_ID3V1.get(i)) {
                                z.pridaj("GENRE", m);
                            }
                        }
                        t => {
                            if let (Some(kluc), 1) = (kluc_ilst(t), druh & 0xFF_FFFF) {
                                z.pridaj(kluc, &text());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Some(z)
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn vorbis(polozky: &[&str]) -> Vec<u8> {
        let mut d = 3u32.to_le_bytes().to_vec();
        d.extend(b"abc");
        d.extend((polozky.len() as u32).to_le_bytes());
        for p in polozky {
            d.extend((p.len() as u32).to_le_bytes());
            d.extend(p.as_bytes());
        }
        d
    }

    #[test]
    fn vorbis_comment_a_replaygain() {
        let z = vorbis_comment(&vorbis(&[
            "Title=Svetlo nad jazerom",
            "ARTIST=A",
            "artist=B",
            "REPLAYGAIN_TRACK_GAIN=-6.20 dB",
            "REPLAYGAIN_TRACK_PEAK=0.988",
            "replaygain_album_gain=+1,5dB",
            "bez_rovnasa",
        ]))
        .unwrap();
        assert_eq!(z.hodnota("title"), Some("Svetlo nad jazerom"));
        assert_eq!(z.hodnoty("ARTIST").collect::<Vec<_>>(), ["A", "B"]);
        let rg = z.replaygain().unwrap();
        assert_eq!(
            (rg.stopa_db, rg.stopa_vrchol, rg.album_db, rg.album_vrchol),
            (Some(-6.2), Some(0.988), Some(1.5), None)
        );
        // skrátený blok: prečíta sa, čo sa dá
        let d = vorbis(&["TITLE=x", "ARTIST=y"]);
        assert_eq!(vorbis_comment(&d[..d.len() - 3]).unwrap().polozky().len(), 1);
    }

    #[test]
    fn decibely_a_vrchol() {
        assert_eq!(decibely("-6.20 dB"), Some(-6.2));
        assert_eq!(decibely("+3.5DB"), Some(3.5));
        assert_eq!(decibely(" 2 "), Some(2.0));
        assert_eq!(decibely("nan dB"), None);
        assert_eq!(decibely("dB"), None);
        assert_eq!(decibely("−ž"), None);
        assert_eq!(vrchol("1.0132"), Some(1.0132));
        assert_eq!(vrchol("-1"), None);
    }

    fn ramec24(id: &[u8; 4], obsah: &[u8]) -> Vec<u8> {
        let n = obsah.len();
        let mut d = id.to_vec();
        d.extend([
            (n >> 21) as u8 & 0x7F,
            (n >> 14) as u8 & 0x7F,
            (n >> 7) as u8 & 0x7F,
            n as u8 & 0x7F,
            0,
            0,
        ]);
        d.extend(obsah);
        d
    }

    fn znacka(verzia: u8, priznaky: u8, telo: &[u8]) -> Vec<u8> {
        let n = telo.len();
        let mut d = vec![b'I', b'D', b'3', verzia, 0, priznaky];
        d.extend([(n >> 21) as u8 & 0x7F, (n >> 14) as u8 & 0x7F, (n >> 7) as u8 & 0x7F, n as u8 & 0x7F]);
        d.extend(telo);
        d
    }

    #[test]
    fn id3v24_kodovania_txxx_a_zaner() {
        let mut telo = ramec24(b"TIT2", b"\x03\xC5\xBDlt\xC3\xA1 \x00");
        telo.extend(ramec24(b"TPE1", b"\x01\xFF\xFEA\x00\x00\x00\xFF\xFEB\x00"));
        telo.extend(ramec24(b"TCON", b"\x00(13)"));
        telo.extend(ramec24(b"TXXX", b"\x00replaygain_track_gain\x00-3.10 dB"));
        telo.extend(ramec24(b"COMM", b"\x00slk\x00koment\xE1r"));
        telo.extend(ramec24(b"COMM", b"\x00engiTunNORM\x00 000"));
        telo.extend([0; 20]); // výplň
        let d = znacka(4, 0, &telo);
        let (z, n) = id3v2(&d).unwrap();
        assert_eq!(n, d.len());
        assert_eq!(z.hodnota("TITLE"), Some("Žltá "));
        assert_eq!(z.hodnoty("ARTIST").collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(z.hodnota("GENRE"), Some("Pop"));
        assert_eq!(z.hodnota("COMMENT"), Some("komentár"));
        assert_eq!(z.replaygain().unwrap().stopa_db, Some(-3.1));
        assert_eq!(z.polozky().len(), 6); // ARTIST dvakrát, COMM s popisom vynechaný
    }

    #[test]
    fn id3v23_nesynchronizacia_a_datum() {
        let mut telo = Vec::new();
        for (id, obsah) in [(b"TYER", &b"\x002026"[..]), (b"TDAT", b"\x002711"), (b"TALB", b"\x00\xFF\x00x")]
        {
            telo.extend(id);
            telo.extend((obsah.len() as u32).to_be_bytes());
            telo.extend([0, 0]);
            telo.extend(obsah);
        }
        // nesynchronizácia celej značky: FF 00 → FF 00 00
        let mut nesync = Vec::new();
        for (i, &b) in telo.iter().enumerate() {
            nesync.push(b);
            if b == 0xFF && telo.get(i + 1).is_some_and(|&c| c == 0 || c >= 0xE0) {
                nesync.push(0);
            }
        }
        let (z, _) = id3v2(&znacka(3, 0x80, &nesync)).unwrap();
        assert_eq!(z.hodnota("DATE"), Some("2026-11-27"));
        assert_eq!(z.hodnota("ALBUM"), Some("\u{FF}"));
        assert!(z.hodnota("TDAT").is_none());
    }

    #[test]
    fn id3v1_a_apev2() {
        let mut d = b"TAG".to_vec();
        for (s, n) in [(&b"Nazov"[..], 30), (b"Interpret", 30), (b"Album", 30), (b"1999", 4), (b"pozn", 28)] {
            d.extend(s);
            d.extend(std::iter::repeat_n(0, n - s.len()));
        }
        d.extend([0, 7, 17]);
        let z = id3v1(&d).unwrap();
        assert_eq!(
            (z.hodnota("TITLE"), z.hodnota("TRACKNUMBER"), z.hodnota("GENRE")),
            (Some("Nazov"), Some("7"), Some("Rock"))
        );

        let mut polozky = Vec::new();
        for (k, v, prz) in [
            ("REPLAYGAIN_TRACK_GAIN", &b"-7.00 dB"[..], 0u32),
            ("Cover Art", b"\x01\x02", 2),
            ("Artist", b"X\x00Y", 0),
        ] {
            polozky.extend((v.len() as u32).to_le_bytes());
            polozky.extend(prz.to_le_bytes());
            polozky.extend(k.as_bytes());
            polozky.push(0);
            polozky.extend(v);
        }
        let mut pata = b"APETAGEX".to_vec();
        for x in [2000u32, polozky.len() as u32 + 32, 3, 0] {
            pata.extend(x.to_le_bytes());
        }
        pata.extend([0; 8]);
        let mut d = b"zvuk".to_vec();
        d.extend(&polozky);
        d.extend(&pata);
        let (z, n) = apev2(&d).unwrap();
        assert_eq!(n, polozky.len() + 32);
        assert_eq!(z.replaygain().unwrap().stopa_db, Some(-7.0));
        assert_eq!(z.hodnoty("ARTIST").collect::<Vec<_>>(), ["X", "Y"]);
        assert!(z.hodnota("COVER ART").is_none());
    }

    fn bx(typ: &[u8; 4], obsah: &[u8]) -> Vec<u8> {
        let mut d = ((obsah.len() + 8) as u32).to_be_bytes().to_vec();
        d.extend(typ);
        d.extend(obsah);
        d
    }

    #[test]
    fn mp4_ilst() {
        let data = |druh: u32, h: &[u8]| {
            let mut o = druh.to_be_bytes().to_vec();
            o.extend([0; 4]);
            o.extend(h);
            bx(b"data", &o)
        };
        let mut ilst = bx(b"\xA9nam", &data(1, "Názov".as_bytes()));
        ilst.extend(bx(b"trkn", &data(0, &[0, 0, 0, 3, 0, 12, 0, 0])));
        ilst.extend(bx(b"gnre", &data(0, &[0, 14])));
        let mut volna = bx(b"mean", b"\0\0\0\0com.apple.iTunes");
        volna.extend(bx(b"name", b"\0\0\0\0replaygain_track_gain"));
        volna.extend(data(1, b"-4.50 dB"));
        ilst.extend(bx(b"----", &volna));
        let mut meta = vec![0; 4];
        meta.extend(bx(b"hdlr", &[0; 25]));
        meta.extend(bx(b"ilst", &ilst));
        let udta = bx(b"meta", &meta);
        let z = mp4_udta(&udta).unwrap();
        assert_eq!(z.hodnota("TITLE"), Some("Názov"));
        assert_eq!(z.hodnota("TRACKNUMBER"), Some("3/12"));
        assert_eq!(z.hodnota("GENRE"), Some("Pop"));
        assert_eq!(z.replaygain().unwrap().stopa_db, Some(-4.5));
    }

    #[test]
    fn obaly_apic_picture_a_poradie() {
        // APIC s popisom v UTF-16 (ukončenie 00 00 na párnej pozícii), zadná strana pred prednou
        let mut apic = b"\x01image/png\x00\x04\xFF\xFEZ\x00\x00\x00".to_vec();
        apic.extend(b"\x89PNG zadna");
        let mut telo = ramec24(b"APIC", &apic);
        telo.extend(ramec24(b"APIC", b"\x00\x00\x03\x00\xFF\xD8\xFF predna"));
        let (z, _) = id3v2(&znacka(4, 0, &telo)).unwrap();
        assert_eq!(z.obaly().len(), 2);
        assert_eq!((z.obaly()[0].druh, z.obaly()[0].popis.as_str()), (4, "Z"));
        let o = z.obal().unwrap();
        assert_eq!((o.druh, o.mime.as_str(), &o.data[..3]), (3, "image/jpeg", &b"\xFF\xD8\xFF"[..]));
        // FLAC PICTURE
        let mut d = Vec::new();
        for x in [3u32, 9] {
            d.extend(x.to_be_bytes());
        }
        d.extend(b"image/png");
        d.extend(4u32.to_be_bytes());
        d.extend(b"Obal");
        d.extend([0; 16]);
        d.extend(5u32.to_be_bytes());
        d.extend(b"\x89PNGx");
        let mut z = Znacky::nove();
        flac_picture(&mut z, &d);
        flac_picture(&mut z, &d[..d.len() - 1]); // skrátený blok sa vynechá
        assert_eq!(z.obaly().len(), 1);
        assert_eq!((z.obal().unwrap().popis.as_str(), z.obal().unwrap().data.len()), ("Obal", 5));
        // odkaz a prázdne dáta sa vynechajú
        z.pridaj_obal("-->", 3, "", b"http://x");
        z.pridaj_obal("image/png", 3, "", b"");
        assert_eq!(z.obaly().len(), 1);
    }

    #[test]
    fn base64_podla_rfc4648() {
        // RFC 4648 §10 – skúšobné vektory
        for (a, b) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ] {
            assert_eq!(base64(a.as_bytes()).unwrap(), b.as_bytes());
        }
        assert!(base64(b"Zm9v!").is_none());
    }

    #[test]
    fn poskodene_znacky_nespadnu() {
        let mut n = orchidea_jadro::nahoda::Nahoda::zo_semienka(5);
        for _ in 0..20_000 {
            let dlzka = (n.dalsie() % 300) as usize;
            let mut d: Vec<u8> = (0..dlzka).map(|_| n.dalsie() as u8).collect();
            if d.len() > 10 && n.dalsie().is_multiple_of(2) {
                d[..3].copy_from_slice(b"ID3");
                d[3] = 2 + (n.dalsie() % 3) as u8;
                for b in &mut d[6..10] {
                    *b &= 0x7F;
                }
            }
            let _ = vorbis_comment(&d);
            let _ = id3v2(&d);
            let _ = apev2(&d);
            let _ = mp4_udta(&d);
            if d.len() >= 128 {
                let _ = id3v1(&d[..128]);
            }
        }
    }
}
