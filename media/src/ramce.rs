//! **Rámce zvukových tokov** – hlavičky rámcov tam, kde kontajner (MPEG-TS) prenáša súvislý tok bajtov a pakety
//! treba vyrezať podľa obsahu. Z hlavičky sa určí dĺžka rámca, počet vzoriek, frekvencia a kanály.
//!
//! Zdroje (vlastná implementácia podľa noriem):
//! - **ADTS** (AAC): ISO/IEC 14496-3, príloha 1.A (adts_fixed_header, adts_variable_header),
//! - **MPEG audio** vrstvy I–III: ISO/IEC 11172-3 §2.4.2.3 a ISO/IEC 13818-3 (nižšie frekvencie, MPEG 2.5),
//! - **AC-3** a **E-AC-3**: ATSC A/52 §5.4.1 (syncinfo, bsi) a príloha E (E-AC-3 bsi),
//! - **Opus v MPEG-TS**: „Encapsulation of Opus in MPEG-2 Transport Stream“ (ETSI TS 102 366 je AC-3) –
//!   riadiaca hlavička prístupovej jednotky (prefix 0x3FF, au_size, start_trim, end_trim).

use crate::stopa::Kodek;
use crate::zvuk::RozlozenieKanalov;

/// Údaje z hlavičky rámca.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlavickaRamca {
    /// Dĺžka celého rámca v bajtoch (vrátane hlavičky).
    pub dlzka: usize,
    /// Vzoriek na kanál v rámci.
    pub vzoriek: u32,
    pub frekvencia: u32,
    pub kanalov: u8,
    pub rozlozenie: Option<RozlozenieKanalov>,
    /// Vrstva MPEG audio (1–3); pri iných formátoch 0.
    pub vrstva: u8,
    /// E-AC-3: rámec závislého podtoku (patrí k predošlému nezávislému rámcu).
    pub zavisly: bool,
}

/// Najmenej bajtov potrebných na rozpoznanie hlavičky.
pub const NAJMENEJ_HLAVICKY: usize = 8;

/// Frekvencie ADTS podľa sampling_frequency_index (ISO/IEC 14496-3 tabuľka 1.18).
const ADTS_FREKVENCIE: [u32; 13] =
    [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// Hlavička rámca ADTS.
pub fn adts(b: &[u8]) -> Option<HlavickaRamca> {
    if b.len() < 7 || b[0] != 0xFF || b[1] & 0xF6 != 0xF0 {
        return None;
    }
    let frekvencia = *ADTS_FREKVENCIE.get(((b[2] >> 2) & 0x0F) as usize)?;
    let konfiguracia = ((b[2] & 1) << 2) | (b[3] >> 6);
    let dlzka = (((b[3] & 3) as usize) << 11) | ((b[4] as usize) << 3) | (b[5] >> 5) as usize;
    let bez_crc = b[1] & 1 != 0;
    if dlzka < if bez_crc { 7 } else { 9 } {
        return None;
    }
    let blokov = (b[6] & 3) as u32 + 1;
    // konfigurácia 0 = rozloženie v PCE (vnútri rámca); 7 = 7.1 (8 kanálov)
    let kanalov = match konfiguracia {
        0 => 0,
        7 => 8,
        k => k,
    };
    Some(HlavickaRamca {
        dlzka,
        vzoriek: 1024 * blokov,
        frekvencia,
        kanalov,
        rozlozenie: RozlozenieKanalov::predvolene(kanalov),
        vrstva: 0,
        zavisly: false,
    })
}

/// Bitové toky MPEG audio v kb/s: [MPEG-1 I, II, III, MPEG-2/2.5 I, II a III] (index 1–14).
const MPA_TOKY: [[u16; 14]; 4] = [
    [32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
    [32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
    [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    [32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
];
const MPA_TOKY_LSF: [u16; 14] = [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

/// Hlavička rámca MPEG audio (vrstvy I–III, MPEG-1, MPEG-2 a MPEG 2.5). Voľný bitový tok (index 0) nie je
/// podporovaný – dĺžka rámca sa nedá určiť z hlavičky.
pub fn mpeg_audio(b: &[u8]) -> Option<HlavickaRamca> {
    if b.len() < 4 || b[0] != 0xFF || b[1] & 0xE0 != 0xE0 {
        return None;
    }
    let verzia = (b[1] >> 3) & 3; // 3 = MPEG-1, 2 = MPEG-2, 0 = MPEG 2.5
    let vrstva = match (b[1] >> 1) & 3 {
        3 => 1u8,
        2 => 2,
        1 => 3,
        _ => return None,
    };
    let index_toku = (b[2] >> 4) as usize;
    let index_frekvencie = ((b[2] >> 2) & 3) as usize;
    if verzia == 1 || index_toku == 0 || index_toku == 15 || index_frekvencie == 3 {
        return None;
    }
    let zaklad = [44100u32, 48000, 32000][index_frekvencie];
    let frekvencia = match verzia {
        3 => zaklad,
        2 => zaklad / 2,
        _ => zaklad / 4,
    };
    let mpeg1 = verzia == 3;
    let tok = if mpeg1 {
        MPA_TOKY[vrstva as usize - 1][index_toku - 1]
    } else if vrstva == 1 {
        MPA_TOKY[3][index_toku - 1]
    } else {
        MPA_TOKY_LSF[index_toku - 1]
    } as u32
        * 1000;
    let vypln = ((b[2] >> 1) & 1) as u32;
    let (dlzka, vzoriek) = match vrstva {
        1 => ((12 * tok / frekvencia + vypln) * 4, 384),
        2 => (144 * tok / frekvencia + vypln, 1152),
        _ if mpeg1 => (144 * tok / frekvencia + vypln, 1152),
        _ => (72 * tok / frekvencia + vypln, 576),
    };
    let kanalov = if b[3] >> 6 == 3 { 1 } else { 2 };
    Some(HlavickaRamca {
        dlzka: dlzka as usize,
        vzoriek,
        frekvencia,
        kanalov,
        rozlozenie: RozlozenieKanalov::predvolene(kanalov),
        vrstva,
        zavisly: false,
    })
}

/// Nominálne bitové toky AC-3 v kb/s podľa frmsizecod / 2 (A/52 tabuľka 5.18).
const AC3_TOKY: [u32; 19] =
    [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640];

/// Kanály podľa acmod (A/52 tabuľka 5.8) – bez LFE.
fn ac3_rozlozenie(acmod: u8, lfe: bool) -> (u8, RozlozenieKanalov) {
    type R = RozlozenieKanalov;
    let maska = match acmod {
        0 => R::STEREO.0, // 1+1 (dva nezávislé kanály)
        1 => R::PREDNY_STRED,
        2 => R::STEREO.0,
        3 => R::STEREO.0 | R::PREDNY_STRED,
        4 => R::STEREO.0 | R::ZADNY_STRED,
        5 => R::STEREO.0 | R::PREDNY_STRED | R::ZADNY_STRED,
        6 => R::STEREO.0 | R::BOCNY_LAVY | R::BOCNY_PRAVY,
        _ => R::STEREO.0 | R::PREDNY_STRED | R::BOCNY_LAVY | R::BOCNY_PRAVY,
    } | if lfe { R::BASY } else { 0 };
    let r = RozlozenieKanalov(maska);
    (r.kanalov() as u8, r)
}

/// Hlavička synchronizačného rámca AC-3 alebo E-AC-3 (podľa bsid).
pub fn ac3(b: &[u8]) -> Option<HlavickaRamca> {
    if b.len() < 8 || b[0] != 0x0B || b[1] != 0x77 {
        return None;
    }
    let bsid = b[5] >> 3;
    if bsid <= 10 {
        // AC-3: syncinfo (crc1, fscod, frmsizecod), bsi (bsid, bsmod, acmod, voliteľné úrovne, lfeon)
        let fscod = b[4] >> 6;
        let frmsizecod = (b[4] & 0x3F) as usize;
        if fscod == 3 || frmsizecod >= 38 {
            return None;
        }
        let tok = AC3_TOKY[frmsizecod / 2];
        let slov = match fscod {
            0 => tok * 2,
            1 => tok * 1000 * 1536 / (44100 * 16) + (frmsizecod as u32 & 1),
            _ => tok * 3,
        };
        // bsid 9 a 10: polovičná a štvrtinová frekvencia (A/52 príloha, rovnaký formát)
        let delitel = 1u32 << bsid.saturating_sub(8);
        let frekvencia = [48000u32, 44100, 32000][fscod as usize] / delitel;
        let acmod = b[6] >> 5;
        let mut bit = 3u32; // bity za acmod v bajte 6
        if acmod & 1 != 0 && acmod != 1 {
            bit += 2; // cmixlev
        }
        if acmod & 4 != 0 {
            bit += 2; // surmixlev
        }
        if acmod == 2 {
            bit += 2; // dsurmod
        }
        let lfe = (u16::from_be_bytes([b[6], b[7]]) >> (15 - bit)) & 1 != 0;
        let (kanalov, r) = ac3_rozlozenie(acmod, lfe);
        return Some(HlavickaRamca {
            dlzka: slov as usize * 2,
            vzoriek: 1536,
            frekvencia,
            kanalov,
            rozlozenie: Some(r),
            vrstva: 0,
            zavisly: false,
        });
    }
    if bsid > 16 {
        return None;
    }
    // E-AC-3: strmtyp, substreamid, frmsiz, fscod, (fscod2 | numblkscod), acmod, lfeon, bsid
    let strmtyp = b[2] >> 6;
    if strmtyp == 3 {
        return None;
    }
    let frmsiz = (((b[2] & 7) as usize) << 8) | b[3] as usize;
    let fscod = b[4] >> 6;
    let (frekvencia, blokov) = if fscod == 3 {
        let fscod2 = (b[4] >> 4) & 3;
        if fscod2 == 3 {
            return None;
        }
        ([24000u32, 22050, 16000][fscod2 as usize], 6)
    } else {
        ([48000u32, 44100, 32000][fscod as usize], [1u32, 2, 3, 6][((b[4] >> 4) & 3) as usize])
    };
    let (kanalov, r) = ac3_rozlozenie((b[4] >> 1) & 7, b[4] & 1 != 0);
    Some(HlavickaRamca {
        dlzka: (frmsiz + 1) * 2,
        vzoriek: 256 * blokov,
        frekvencia,
        kanalov,
        rozlozenie: Some(r),
        vrstva: 0,
        zavisly: strmtyp == 1,
    })
}

/// Hlavička rámca pre kodek, ktorého pakety sa v MPEG-TS vyrezávajú z toku.
pub fn hlavicka(kodek: &Kodek, b: &[u8]) -> Option<HlavickaRamca> {
    match kodek {
        Kodek::Aac => adts(b),
        Kodek::MpegAudio { .. } => mpeg_audio(b),
        Kodek::Ac3 | Kodek::Eac3 => ac3(b),
        _ => None,
    }
}

/// Kodeky s rámcovaním z toku (ostatné kodeky: paket = celý PES).
pub fn ma_ramcovanie(kodek: &Kodek) -> bool {
    matches!(kodek, Kodek::Aac | Kodek::MpegAudio { .. } | Kodek::Ac3 | Kodek::Eac3)
}

/// Riadiaca hlavička prístupovej jednotky Opus v MPEG-TS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpusTs {
    /// Dĺžka riadiacej hlavičky (bajtov pred paketom Opus).
    pub hlavicka: usize,
    /// Dĺžka paketu Opus.
    pub dlzka: usize,
    /// Vzoriek (pri 48 kHz) zahodiť na začiatku a na konci.
    pub orezat_zaciatok: u16,
    pub orezat_koniec: u16,
}

/// Prečíta riadiacu hlavičku Opus v MPEG-TS: 11 bitov prefixu 0x3FF, start_trim_flag, end_trim_flag,
/// control_extension_flag, 2 rezervované bity; au_size ako súčet bajtov (0xFF pokračuje); voliteľne
/// start_trim (16 b), end_trim (16 b) a rozšírenie s dĺžkou.
pub fn opus_ts(b: &[u8]) -> Option<OpusTs> {
    if b.len() < 3 || (u16::from_be_bytes([b[0], b[1]]) >> 5) != 0x3FF {
        return None;
    }
    let zaciatok = b[1] & 0x10 != 0;
    let koniec = b[1] & 0x08 != 0;
    let rozsirenie = b[1] & 0x04 != 0;
    let mut i = 2;
    let mut dlzka = 0usize;
    loop {
        let v = *b.get(i)?;
        i += 1;
        dlzka += v as usize;
        if v != 0xFF {
            break;
        }
    }
    let cislo16 = |i: &mut usize| -> Option<u16> {
        let v = u16::from_be_bytes([*b.get(*i)?, *b.get(*i + 1)?]);
        *i += 2;
        Some(v)
    };
    let orezat_zaciatok = if zaciatok { cislo16(&mut i)? } else { 0 };
    let orezat_koniec = if koniec { cislo16(&mut i)? } else { 0 };
    if rozsirenie {
        i += 1 + *b.get(i)? as usize;
    }
    Some(OpusTs { hlavicka: i, dlzka, orezat_zaciatok, orezat_koniec })
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn adts_hlavicka() {
        // AAC LC, 48 kHz, stereo, 371 B, bez CRC, 1 blok
        let d = 371usize;
        let b = [0xFF, 0xF1, 0x4C, 0x80 | (d >> 11) as u8, (d >> 3) as u8, ((d & 7) << 5) as u8 | 0x1F, 0xFC];
        let h = adts(&b).unwrap();
        assert_eq!((h.dlzka, h.vzoriek, h.frekvencia, h.kanalov), (371, 1024, 48000, 2));
        assert!(adts(&[0xFF, 0xF3, 0x4C, 0x80, 0, 0, 0]).is_none(), "vrstva musí byť 0");
        assert!(adts(&[0xFF, 0xF1, 0x4C, 0x80, 0, 0x1F, 0xFC]).is_none(), "príliš krátky rámec");
    }

    #[test]
    fn mpeg_audio_hlavicka() {
        // MPEG-1 vrstva II, 192 kb/s, 48 kHz → 576 B; vrstva III 128 kb/s 44,1 kHz s výplňou → 418 B
        let h = mpeg_audio(&[0xFF, 0xFD, 0xA4, 0x00]).unwrap();
        assert_eq!((h.dlzka, h.vzoriek, h.frekvencia, h.vrstva), (576, 1152, 48000, 2));
        let h = mpeg_audio(&[0xFF, 0xFB, 0x92, 0xC0]).unwrap();
        assert_eq!((h.dlzka, h.vzoriek, h.frekvencia, h.kanalov), (418, 1152, 44100, 1));
        // MPEG-2 vrstva III, 64 kb/s, 24 kHz → 72·64000/24000 = 192 B, 576 vzoriek
        let h = mpeg_audio(&[0xFF, 0xF3, 0x84, 0x00]).unwrap();
        assert_eq!((h.dlzka, h.vzoriek, h.frekvencia), (192, 576, 24000));
        // vrstva I, 384 kb/s, 32 kHz → (12·384000/32000)·4 = 576 B
        let h = mpeg_audio(&[0xFF, 0xFF, 0xC8, 0x00]).unwrap();
        assert_eq!((h.dlzka, h.vzoriek), (576, 384));
        assert!(mpeg_audio(&[0xFF, 0xFD, 0x04, 0x00]).is_none(), "voľný tok");
        assert!(mpeg_audio(&[0xFF, 0xFD, 0xAC, 0x00]).is_none(), "zlá frekvencia");
    }

    #[test]
    fn ac3_hlavicka() {
        // 48 kHz, 192 kb/s (frmsizecod 20) → 768 B; acmod 2 (stereo), dsurmod, lfeon 0
        let h = ac3(&[0x0B, 0x77, 0, 0, 0x14, 0x40, 0x40, 0x00]).unwrap();
        assert_eq!((h.dlzka, h.vzoriek, h.frekvencia, h.kanalov), (768, 1536, 48000, 2));
        // 44,1 kHz, 32 kb/s nepárny kód → 70 slov
        assert_eq!(ac3(&[0x0B, 0x77, 0, 0, 0x41, 0x40, 0x40, 0]).unwrap().dlzka, 140);
        // 5.1: acmod 7, cmixlev, surmixlev, lfeon = najnižší bit bajtu 6
        let h = ac3(&[0x0B, 0x77, 0, 0, 0x1E, 0x40, 0xE1, 0x00]).unwrap();
        assert_eq!(h.kanalov, 6);
        assert_eq!(h.rozlozenie.unwrap().0 & RozlozenieKanalov::BASY, RozlozenieKanalov::BASY);
        // E-AC-3: nezávislý, frmsiz 383 → 768 B, 48 kHz, 6 blokov, acmod 2, bsid 16
        let h = ac3(&[0x0B, 0x77, 0x01, 0x7F, 0x34, 0x80, 0, 0]).unwrap();
        assert_eq!((h.dlzka, h.vzoriek, h.frekvencia, h.kanalov, h.zavisly), (768, 1536, 48000, 2, false));
        assert!(ac3(&[0x0B, 0x77, 0x41, 0x7F, 0x34, 0x80, 0, 0]).unwrap().zavisly);
    }

    #[test]
    fn opus_v_ts() {
        let h = opus_ts(&[0x7F, 0xE0, 0xFF, 0x10]).unwrap();
        assert_eq!((h.hlavicka, h.dlzka, h.orezat_zaciatok), (4, 0x10F, 0));
        let h = opus_ts(&[0x7F, 0xF8, 0x05, 0x01, 0x38, 0x00, 0x10]).unwrap();
        assert_eq!((h.hlavicka, h.dlzka, h.orezat_zaciatok, h.orezat_koniec), (7, 5, 312, 16));
        assert!(opus_ts(&[0x7F, 0xC0, 0x05]).is_none());
        assert!(opus_ts(&[0x7F, 0xE0, 0xFF]).is_none());
    }
}
