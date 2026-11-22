//! NTFS 3.1 vlastným kódom: zavádzací sektor, MFT so všetkými metasúbormi ($MFT, $MFTMirr, $LogFile,
//! $Volume, $AttrDef, koreň, $Bitmap, $Boot, $BadClus, $Secure, $UpCase, $Extend s $Quota, $ObjId a
//! $Reparse), zabezpečenie cez $Secure, indexy priečinkov ako B-strom (INDX) a súbory so súvislými behmi.
//!
//! Štruktúry podľa verejnej dokumentácie formátu (projekt Linux-NTFS, NTFS-3G); kód je vlastný.
//! Klaster 4 KB, záznam MFT 1 KB (pri sektore 4 KB záznam 4 KB), indexový blok 4 KB – ako Windows.

use crate::ciel::{self, Ciel, KUS};
use crate::oddiely::{jedinecne, kod_bez_systemu, teraz};
use crate::subory::{Citaj, Strom, Zapisany};
use orchidea_jadro::Hasovac;
use std::cmp::Ordering as Poradie;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

const KLASTER: u64 = 4096;
const INDX: usize = 4096;
/// Krok ochrany viacsektorových záznamov (update sequence) – v NTFS vždy 512 B.
const KROK_USA: usize = 512;
/// Prvý záznam MFT pre súbory používateľa (nižšie sú metasúbory a rezerva systému).
const PRVY_POUZIVATELSKY: u64 = 64;

// typy atribútov
const AT_SI: u32 = 0x10;
const AT_FN: u32 = 0x30;
const AT_VOLUME_NAME: u32 = 0x60;
const AT_VOLUME_INFO: u32 = 0x70;
const AT_DATA: u32 = 0x80;
const AT_INDEX_ROOT: u32 = 0x90;
const AT_INDEX_ALLOC: u32 = 0xA0;
const AT_BITMAP: u32 = 0xB0;

// atribúty súborov
const SKRYTY: u32 = 0x02;
const SYSTEMOVY: u32 = 0x04;
const ARCHIV: u32 = 0x20;
const INDEX_I30: u32 = 0x1000_0000;
const INDEX_POHLAD: u32 = 0x2000_0000;

// zoraďovacie pravidlá indexov
const ZORAD_MENO: u32 = 1;
const ZORAD_ULONG: u32 = 16;
const ZORAD_SID: u32 = 17;
const ZORAD_HASH: u32 = 18;
const ZORAD_ULONGS: u32 = 19;

// bezpečnostné ID v $Secure
const SEC_SYSTEM: u32 = 0x100;
const SEC_SYSTEM_PRIECINOK: u32 = 0x101;
const SEC_POUZIVATEL: u32 = 0x102;

fn u16le(v: &mut [u8], i: usize, x: u16) {
    v[i..i + 2].copy_from_slice(&x.to_le_bytes());
}
fn u32le(v: &mut [u8], i: usize, x: u32) {
    v[i..i + 4].copy_from_slice(&x.to_le_bytes());
}
fn u64le(v: &mut [u8], i: usize, x: u64) {
    v[i..i + 8].copy_from_slice(&x.to_le_bytes());
}
fn zarovnaj8(n: usize) -> usize {
    (n + 7) & !7
}
fn utf16(t: &str) -> Vec<u16> {
    t.encode_utf16().collect()
}

/// Čas NTFS (stovky nanosekúnd od 1. 1. 1601).
fn cas_ntfs(unix: u64) -> u64 {
    (unix + 11_644_473_600) * 10_000_000
}

/// Odkaz na záznam MFT (poradové číslo << 48 | číslo záznamu).
fn odkaz(zaznam: u64, poradie: u16) -> u64 {
    (poradie as u64) << 48 | zaznam
}

// ---- tabuľka veľkých písmen ($UpCase, ako Windows 7) ----

/// Tabuľka veľkých písmen NTFS (65 536 položiek) – zhodná s tabuľkou Windows 7 (údaje o rozsahoch
/// podľa NTFS-3G, ktoré opisujú tabuľky Windows XP a zmeny vo Viste a Windows 7).
pub fn tabulka_velkych() -> Vec<u16> {
    let mut u: Vec<u16> = (0..=0xFFFFu32).map(|i| i as u16).collect();
    // úseky s posunom: (od, po – bez, posun)
    const BEHY: &[(u32, u32, i32)] = &[
        (0x0061, 0x007B, -32),
        (0x0451, 0x045D, -80),
        (0x1F70, 0x1F72, 74),
        (0x00E0, 0x00F7, -32),
        (0x045E, 0x0460, -80),
        (0x1F72, 0x1F76, 86),
        (0x00F8, 0x00FF, -32),
        (0x0561, 0x0587, -48),
        (0x1F76, 0x1F78, 100),
        (0x0256, 0x0258, -205),
        (0x1F00, 0x1F08, 8),
        (0x1F78, 0x1F7A, 128),
        (0x028A, 0x028C, -217),
        (0x1F10, 0x1F16, 8),
        (0x1F7A, 0x1F7C, 112),
        (0x03AC, 0x03AD, -38),
        (0x1F20, 0x1F28, 8),
        (0x1F7C, 0x1F7E, 126),
        (0x03AD, 0x03B0, -37),
        (0x1F30, 0x1F38, 8),
        (0x1FB0, 0x1FB2, 8),
        (0x03B1, 0x03C2, -32),
        (0x1F40, 0x1F46, 8),
        (0x1FD0, 0x1FD2, 8),
        (0x03C2, 0x03C3, -31),
        (0x1F51, 0x1F52, 8),
        (0x1FE0, 0x1FE2, 8),
        (0x03C3, 0x03CC, -32),
        (0x1F53, 0x1F54, 8),
        (0x1FE5, 0x1FE6, 7),
        (0x03CC, 0x03CD, -64),
        (0x1F55, 0x1F56, 8),
        (0x2170, 0x2180, -16),
        (0x03CD, 0x03CF, -63),
        (0x1F57, 0x1F58, 8),
        (0x24D0, 0x24EA, -26),
        (0x0430, 0x0450, -32),
        (0x1F60, 0x1F68, 8),
        (0xFF41, 0xFF5B, -32),
    ];
    for &(od, po, d) in BEHY {
        for i in od..po {
            u[i as usize] = (i as i32 + d) as u16;
        }
    }
    // dvojice veľké/malé (malé písmeno nasleduje za veľkým)
    const DVOJICE: &[(u32, u32)] = &[
        (0x0100, 0x012F),
        (0x01A0, 0x01A6),
        (0x03E2, 0x03EF),
        (0x04CB, 0x04CC),
        (0x0132, 0x0137),
        (0x01B3, 0x01B7),
        (0x0460, 0x0481),
        (0x04D0, 0x04EB),
        (0x0139, 0x0149),
        (0x01CD, 0x01DD),
        (0x0490, 0x04BF),
        (0x04EE, 0x04F5),
        (0x014A, 0x0178),
        (0x01DE, 0x01EF),
        (0x04BF, 0x04BF),
        (0x04F8, 0x04F9),
        (0x0179, 0x017E),
        (0x01F4, 0x01F5),
        (0x04C1, 0x04C4),
        (0x1E00, 0x1E95),
        (0x018B, 0x018B),
        (0x01FA, 0x0218),
        (0x04C7, 0x04C8),
        (0x1EA0, 0x1EF9),
    ];
    for &(od, po) in DVOJICE {
        let mut i = od;
        while i < po {
            u[i as usize + 1] = i as u16;
            i += 2;
        }
    }
    const JEDNOTLIVE: &[(u32, u16)] = &[
        (0x00FF, 0x0178),
        (0x01AD, 0x01AC),
        (0x01F3, 0x01F1),
        (0x0269, 0x0196),
        (0x0183, 0x0182),
        (0x01B0, 0x01AF),
        (0x0253, 0x0181),
        (0x026F, 0x019C),
        (0x0185, 0x0184),
        (0x01B9, 0x01B8),
        (0x0254, 0x0186),
        (0x0272, 0x019D),
        (0x0188, 0x0187),
        (0x01BD, 0x01BC),
        (0x0259, 0x018F),
        (0x0275, 0x019F),
        (0x018C, 0x018B),
        (0x01C6, 0x01C4),
        (0x025B, 0x0190),
        (0x0283, 0x01A9),
        (0x0192, 0x0191),
        (0x01C9, 0x01C7),
        (0x0260, 0x0193),
        (0x0288, 0x01AE),
        (0x0199, 0x0198),
        (0x01CC, 0x01CA),
        (0x0263, 0x0194),
        (0x0292, 0x01B7),
        (0x01A8, 0x01A7),
        (0x01DD, 0x018E),
        (0x0268, 0x0197),
    ];
    for &(i, v) in JEDNOTLIVE {
        u[i as usize] = v;
    }
    // zmeny vo Windows Vista a 7: (od, po – vrátane, posun, krok)
    const NOVE: &[(u32, u32, i32, u32)] = &[
        (0x37b, 0x37d, 0x82, 1),
        (0x1f80, 0x1f87, 0x8, 1),
        (0x1f90, 0x1f97, 0x8, 1),
        (0x1fa0, 0x1fa7, 0x8, 1),
        (0x2c30, 0x2c5e, -0x30, 1),
        (0x2d00, 0x2d25, -0x1c60, 1),
        (0x2c68, 0x2c6c, -0x1, 2),
        (0x219, 0x21f, -0x1, 2),
        (0x223, 0x233, -0x1, 2),
        (0x247, 0x24f, -0x1, 2),
        (0x3d9, 0x3e1, -0x1, 2),
        (0x48b, 0x48f, -0x1, 2),
        (0x4fb, 0x513, -0x1, 2),
        (0x2c81, 0x2ce3, -0x1, 2),
        (0x3f8, 0x3fb, -0x1, 3),
        (0x4c6, 0x4ce, -0x1, 4),
        (0x23c, 0x242, -0x1, 6),
        (0x4ed, 0x4f7, -0x1, 10),
        (0x450, 0x45d, -0x50, 13),
        (0x2c61, 0x2c76, -0x1, 21),
        (0x1fcc, 0x1ffc, -0x9, 48),
        (0x180, 0x180, 0xc3, 1),
        (0x195, 0x195, 0x61, 1),
        (0x19a, 0x19a, 0xa3, 1),
        (0x19e, 0x19e, 0x82, 1),
        (0x1bf, 0x1bf, 0x38, 1),
        (0x1f9, 0x1f9, -0x1, 1),
        (0x23a, 0x23a, 0x2a2b, 1),
        (0x23e, 0x23e, 0x2a28, 1),
        (0x26b, 0x26b, 0x29f7, 1),
        (0x27d, 0x27d, 0x29e7, 1),
        (0x280, 0x280, -0xda, 1),
        (0x289, 0x289, -0x45, 1),
        (0x28c, 0x28c, -0x47, 1),
        (0x3f2, 0x3f2, 0x7, 1),
        (0x4cf, 0x4cf, -0xf, 1),
        (0x1d7d, 0x1d7d, 0xee6, 1),
        (0x1fb3, 0x1fb3, 0x9, 1),
        (0x214e, 0x214e, -0x1c, 1),
        (0x2184, 0x2184, -0x1, 1),
        // Windows 7
        (0x23a, 0x23e, 0x0, 4),
        (0x250, 0x250, 0x2a1f, 2),
        (0x251, 0x251, 0x2a1c, 2),
        (0x271, 0x271, 0x29fd, 2),
        (0x371, 0x373, -0x1, 2),
        (0x377, 0x377, -0x1, 2),
        (0x3c2, 0x3c2, 0x0, 2),
        (0x3d7, 0x3d7, -0x8, 2),
        (0x515, 0x523, -0x1, 2),
        (0x1d79, 0x1d79, -0x75fc, 2),
        (0x1efb, 0x1eff, -0x1, 2),
        (0x1fc3, 0x1ff3, 0x9, 48),
        (0x1fcc, 0x1ffc, 0x0, 48),
        (0x2c65, 0x2c65, -0x2a2b, 2),
        (0x2c66, 0x2c66, -0x2a28, 2),
        (0x2c73, 0x2c73, -0x1, 2),
        (0xa641, 0xa65f, -0x1, 2),
        (0xa663, 0xa66d, -0x1, 2),
        (0xa681, 0xa697, -0x1, 2),
        (0xa723, 0xa72f, -0x1, 2),
        (0xa733, 0xa76f, -0x1, 2),
        (0xa77a, 0xa77c, -0x1, 2),
        (0xa77f, 0xa787, -0x1, 2),
        (0xa78c, 0xa78c, -0x1, 2),
    ];
    for &(od, po, d, krok) in NOVE {
        let mut i = od;
        while i <= po {
            u[i as usize] = (i as i32 + d) as u16;
            i += krok;
        }
    }
    u
}

/// CRC-64 pre $UpCase:$Info (polynóm 0x9A6C9329AC4BC9B5, odrazený, s doplnkom) – podľa neho chkdsk
/// pozná, že tabuľka je platná.
pub fn crc64(data: &[u8]) -> u64 {
    let mut t = [0u64; 256];
    for (i, x) in t.iter_mut().enumerate() {
        let mut c = i as u64;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0x9A6C_9329_AC4B_C9B5 ^ (c >> 1) } else { c >> 1 };
        }
        *x = c;
    }
    let mut c = !0u64;
    for &b in data {
        c = t[((c ^ b as u64) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// Zoradenie mien ako v indexe $I30: bez ohľadu na veľkosť písmen podľa $UpCase, potom dĺžka,
/// potom presné znaky.
fn porovnaj(a: &[u16], b: &[u16], up: &[u16]) -> Poradie {
    let ua = a.iter().map(|&c| up[c as usize]);
    let ub = b.iter().map(|&c| up[c as usize]);
    ua.cmp(ub).then(a.len().cmp(&b.len())).then(a.cmp(b))
}

// ---- $AttrDef ----

/// Tabuľka definícií atribútov (2 560 B) – hodnoty formátu NTFS 3.1.
fn attrdef() -> Vec<u8> {
    const D: &[(&str, u32, u32, u32, u64, i64)] = &[
        ("$STANDARD_INFORMATION", 0x10, 0, 0x40, 0x30, 0x48),
        ("$ATTRIBUTE_LIST", 0x20, 0, 0x80, 0, -1),
        ("$FILE_NAME", 0x30, 0, 0x42, 0x44, 0x242),
        ("$OBJECT_ID", 0x40, 0, 0x40, 0, 0x100),
        ("$SECURITY_DESCRIPTOR", 0x50, 0, 0x80, 0, -1),
        ("$VOLUME_NAME", 0x60, 0, 0x40, 2, 0x100),
        ("$VOLUME_INFORMATION", 0x70, 0, 0x40, 0xC, 0xC),
        ("$DATA", 0x80, 0, 0, 0, -1),
        ("$INDEX_ROOT", 0x90, 0, 0x40, 0, -1),
        ("$INDEX_ALLOCATION", 0xA0, 0, 0x80, 0, -1),
        ("$BITMAP", 0xB0, 0, 0x80, 0, -1),
        ("$REPARSE_POINT", 0xC0, 0, 0x80, 0, 0x4000),
        ("$EA_INFORMATION", 0xD0, 0, 0x40, 8, 8),
        ("$EA", 0xE0, 0, 0, 0, 0x1_0000),
        ("$LOGGED_UTILITY_STREAM", 0x100, 0, 0x80, 0, 0x1_0000),
    ];
    let mut v = vec![0u8; 2560];
    for (i, &(meno, typ, zorad, priznaky, min, max)) in D.iter().enumerate() {
        let z = &mut v[i * 160..(i + 1) * 160];
        for (j, c) in meno.encode_utf16().enumerate() {
            u16le(z, 2 * j, c);
        }
        u32le(z, 128, typ);
        u32le(z, 136, zorad);
        u32le(z, 140, priznaky);
        u64le(z, 144, min);
        u64le(z, 152, max as u64);
    }
    v
}

// ---- zabezpečenie ----

/// SID: (autorita, podautority).
fn sid(autorita: u8, pod: &[u32]) -> Vec<u8> {
    let mut v = vec![1, pod.len() as u8, 0, 0, 0, 0, 0, autorita];
    for p in pod {
        v.extend_from_slice(&p.to_le_bytes());
    }
    v
}

fn sid_system() -> Vec<u8> {
    sid(5, &[18])
}
fn sid_spravcovia() -> Vec<u8> {
    sid(5, &[32, 544])
}
fn sid_vsetci() -> Vec<u8> {
    sid(1, &[0])
}

/// Bezpečnostný opis (self-relative): vlastník, skupina a DACL s povoľujúcimi položkami
/// (príznaky dedenia, maska prístupu, SID).
fn opis(vlastnik: &[u8], skupina: &[u8], ace: &[(u8, u32, Vec<u8>)]) -> Vec<u8> {
    let mut acl = vec![2u8, 0, 0, 0, 0, 0, 0, 0];
    for (priznaky, maska, s) in ace {
        let n = 8 + s.len();
        acl.extend([0u8, *priznaky]);
        acl.extend((n as u16).to_le_bytes());
        acl.extend(maska.to_le_bytes());
        acl.extend_from_slice(s);
    }
    let dlzka_acl = acl.len();
    u16le(&mut acl, 2, dlzka_acl as u16);
    u16le(&mut acl, 4, ace.len() as u16);
    let mut v = vec![0u8; 20];
    v[0] = 1;
    u16le(&mut v, 2, 0x8004); // SE_SELF_RELATIVE | SE_DACL_PRESENT
    u32le(&mut v, 16, 20);
    v.extend(acl);
    let o = v.len();
    v.extend_from_slice(vlastnik);
    let g = v.len();
    v.extend_from_slice(skupina);
    u32le(&mut v, 4, o as u32);
    u32le(&mut v, 8, g as u32);
    v
}

/// Odtlačok opisu pre index $SDH.
fn odtlacok_opisu(sd: &[u8]) -> u32 {
    sd.chunks_exact(4)
        .fold(0u32, |h, w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]).wrapping_add(h.rotate_left(3)))
}

/// Opisy v $SDS: (id, opis).
fn opisy() -> Vec<(u32, Vec<u8>)> {
    let s = || sid_system();
    let a = || sid_spravcovia();
    vec![
        // systémové súbory: SYSTEM a správcovia čítanie
        (SEC_SYSTEM, opis(&a(), &a(), &[(0, 0x0012_0089, s()), (0, 0x0012_0089, a())])),
        // systémové priečinky a pohľady
        (SEC_SYSTEM_PRIECINOK, opis(&a(), &a(), &[(0, 0x0012_019F, s()), (0, 0x0012_019F, a())])),
        // koreň a súbory používateľa: všetci plný prístup (vymeniteľný disk), s dedením
        (
            SEC_POUZIVATEL,
            opis(
                &a(),
                &s(),
                &[(0x03, 0x001F_01FF, sid_vsetci()), (0x03, 0x001F_01FF, s()), (0x03, 0x001F_01FF, a())],
            ),
        ),
    ]
}

// ---- atribúty a záznamy MFT ----

/// Rezidentný atribút (číslo inštancie sa doplní pri skladaní záznamu).
fn rezidentny(typ: u32, meno: &[u16], hodnota: &[u8], indexovany: bool) -> Vec<u8> {
    let meno_od = 24;
    let hodnota_od = zarovnaj8(meno_od + 2 * meno.len());
    let dlzka = zarovnaj8(hodnota_od + hodnota.len());
    let mut a = vec![0u8; dlzka];
    u32le(&mut a, 0, typ);
    u32le(&mut a, 4, dlzka as u32);
    a[9] = meno.len() as u8;
    u16le(&mut a, 10, if meno.is_empty() { 0 } else { meno_od as u16 });
    u32le(&mut a, 16, hodnota.len() as u32);
    u16le(&mut a, 20, hodnota_od as u16);
    a[22] = indexovany as u8;
    for (i, c) in meno.iter().enumerate() {
        u16le(&mut a, meno_od + 2 * i, *c);
    }
    a[hodnota_od..hodnota_od + hodnota.len()].copy_from_slice(hodnota);
    a
}

/// Behy (LCN, počet klastrov); LCN None = riedky beh.
type Behy = Vec<(Option<u64>, u64)>;

/// Mapovanie behov (mapping pairs) so znamienkovými posunmi LCN.
fn mapovanie(behy: &Behy) -> Vec<u8> {
    fn bajty(x: i64) -> Vec<u8> {
        let mut v = x.to_le_bytes().to_vec();
        while v.len() > 1 {
            let n = v.len();
            let posledny = v[n - 1];
            let predposledny = v[n - 2];
            if (posledny == 0 && predposledny & 0x80 == 0) || (posledny == 0xFF && predposledny & 0x80 != 0) {
                v.pop();
            } else {
                break;
            }
        }
        v
    }
    let mut v = Vec::new();
    let mut predosly = 0i64;
    for &(lcn, dlzka) in behy {
        let d = bajty(dlzka as i64);
        let o = match lcn {
            Some(l) => {
                let x = bajty(l as i64 - predosly);
                predosly = l as i64;
                x
            }
            None => Vec::new(),
        };
        v.push((o.len() as u8) << 4 | d.len() as u8);
        v.extend(d);
        v.extend(o);
    }
    v.push(0);
    v
}

/// Nerezidentný atribút.
fn nerezidentny(typ: u32, meno: &[u16], behy: &Behy, velkost: u64, inicializovane: u64) -> Vec<u8> {
    let klastrov: u64 = behy.iter().map(|b| b.1).sum();
    let mp = mapovanie(behy);
    let mp_od = zarovnaj8(64 + 2 * meno.len());
    let dlzka = zarovnaj8(mp_od + mp.len());
    let mut a = vec![0u8; dlzka];
    u32le(&mut a, 0, typ);
    u32le(&mut a, 4, dlzka as u32);
    a[8] = 1;
    a[9] = meno.len() as u8;
    u16le(&mut a, 10, if meno.is_empty() { 0 } else { 64 });
    u64le(&mut a, 16, 0);
    u64le(&mut a, 24, klastrov.wrapping_sub(1));
    u16le(&mut a, 32, mp_od as u16);
    u64le(&mut a, 40, klastrov * KLASTER);
    u64le(&mut a, 48, velkost);
    u64le(&mut a, 56, inicializovane);
    for (i, c) in meno.iter().enumerate() {
        u16le(&mut a, 64 + 2 * i, *c);
    }
    a[mp_od..mp_od + mp.len()].copy_from_slice(&mp);
    a
}

fn standardne_info(atributy: u32, bezpecnost: u32, cas: u64) -> Vec<u8> {
    let mut v = vec![0u8; 72];
    for o in [0, 8, 16, 24] {
        u64le(&mut v, o, cas);
    }
    u32le(&mut v, 32, atributy);
    u32le(&mut v, 52, bezpecnost);
    v
}

/// Hodnota atribútu $FILE_NAME (rovnaká v zázname MFT aj ako kľúč v indexe rodiča).
#[allow(clippy::too_many_arguments)]
fn meno_suboru(
    rodic: u64,
    pridelene: u64,
    velkost: u64,
    atributy: u32,
    meno: &[u16],
    typ: u8,
    cas: u64,
) -> Vec<u8> {
    let mut v = vec![0u8; 66 + 2 * meno.len()];
    u64le(&mut v, 0, rodic);
    for o in [8, 16, 24, 32] {
        u64le(&mut v, o, cas);
    }
    u64le(&mut v, 40, pridelene);
    u64le(&mut v, 48, velkost);
    u32le(&mut v, 56, atributy);
    v[64] = meno.len() as u8;
    v[65] = typ;
    for (i, c) in meno.iter().enumerate() {
        u16le(&mut v, 66 + 2 * i, *c);
    }
    v
}

/// Ochrana viacsektorového záznamu: posledné 2 bajty každých 512 B sa presunú do poľa USA.
fn ochran(b: &mut [u8], usa_od: usize) {
    let pocet = b.len() / KROK_USA;
    let usn = 1u16;
    u16le(b, usa_od, usn);
    for i in 0..pocet {
        let k = (i + 1) * KROK_USA - 2;
        let p = usa_od + 2 + 2 * i;
        b[p] = b[k];
        b[p + 1] = b[k + 1];
        u16le(b, k, usn);
    }
}

/// Záznam MFT.
struct Zaznam {
    cislo: u64,
    poradie: u16,
    priznaky: u16,
    odkazy: u16,
    atributy: Vec<Vec<u8>>,
}

impl Zaznam {
    fn novy(cislo: u64, poradie: u16, priznaky: u16) -> Zaznam {
        Zaznam { cislo, poradie, priznaky, odkazy: 0, atributy: Vec::new() }
    }

    fn pridaj(&mut self, a: Vec<u8>) {
        self.atributy.push(a);
    }

    fn velkost_atributov(&self) -> usize {
        self.atributy.iter().map(Vec::len).sum()
    }

    /// Bajty záznamu s ochranou USA. Atribúty zoradené podľa typu a mena.
    fn bajty(mut self, velkost: usize) -> Result<Vec<u8>, String> {
        let meno = |a: &Vec<u8>| -> Vec<u8> {
            let n = a[9] as usize;
            let o = u16::from_le_bytes([a[10], a[11]]) as usize;
            a[o..o + 2 * n].to_vec()
        };
        self.atributy.sort_by(|x, y| {
            let tx = u32::from_le_bytes([x[0], x[1], x[2], x[3]]);
            let ty = u32::from_le_bytes([y[0], y[1], y[2], y[3]]);
            tx.cmp(&ty).then_with(|| meno(x).cmp(&meno(y)))
        });
        let usa_pocet = velkost / KROK_USA + 1;
        let atr_od = zarovnaj8(0x30 + 2 * usa_pocet);
        let mut b = vec![0u8; velkost];
        b[0..4].copy_from_slice(b"FILE");
        u16le(&mut b, 4, 0x30);
        u16le(&mut b, 6, usa_pocet as u16);
        u16le(&mut b, 16, self.poradie);
        u16le(&mut b, 18, self.odkazy);
        u16le(&mut b, 20, atr_od as u16);
        u16le(&mut b, 22, self.priznaky);
        u32le(&mut b, 28, velkost as u32);
        u32le(&mut b, 44, self.cislo as u32);
        let mut p = atr_od;
        let pocet = self.atributy.len();
        for (i, mut a) in self.atributy.into_iter().enumerate() {
            u16le(&mut a, 14, i as u16);
            if p + a.len() + 8 > velkost {
                return Err(format!("záznam MFT {} je preplnený", self.cislo));
            }
            b[p..p + a.len()].copy_from_slice(&a);
            p += a.len();
        }
        u32le(&mut b, p, 0xFFFF_FFFF);
        p += 8;
        u32le(&mut b, 24, p as u32);
        u16le(&mut b, 40, pocet as u16);
        ochran(&mut b, 0x30);
        Ok(b)
    }
}

/// Prázdny (nepoužitý) záznam MFT.
fn prazdny_zaznam(cislo: u64, velkost: usize) -> Vec<u8> {
    Zaznam::novy(cislo, cislo as u16, 0).bajty(velkost).unwrap_or_else(|_| vec![0; velkost])
}

// ---- indexy ----

/// Položka indexu $I30 (bez ukazovateľa na podstrom).
fn polozka_mena(subor: u64, kluc: &[u8]) -> Vec<u8> {
    let dlzka = zarovnaj8(16 + kluc.len());
    let mut e = vec![0u8; dlzka];
    u64le(&mut e, 0, subor);
    u16le(&mut e, 8, dlzka as u16);
    u16le(&mut e, 10, kluc.len() as u16);
    e[16..16 + kluc.len()].copy_from_slice(kluc);
    e
}

/// Položka pohľadu (view index: $SII, $SDH, $Q, $O …): kľúč, dáta a celková dĺžka.
fn polozka_pohladu(kluc: &[u8], data: &[u8], data_od: usize, dlzka: usize) -> Vec<u8> {
    let mut e = vec![0u8; dlzka];
    u16le(&mut e, 0, data_od as u16);
    u16le(&mut e, 2, data.len() as u16);
    u16le(&mut e, 8, dlzka as u16);
    u16le(&mut e, 10, kluc.len() as u16);
    e[16..16 + kluc.len()].copy_from_slice(kluc);
    e[data_od..data_od + data.len()].copy_from_slice(data);
    e
}

/// Pridá ukazovateľ na podstrom (VCN) a príznak uzla.
fn s_podstromom(mut e: Vec<u8>, vcn: u64) -> Vec<u8> {
    let n = e.len() + 8;
    e.resize(n, 0);
    u16le(&mut e, 8, n as u16);
    let f = u16::from_le_bytes([e[12], e[13]]) | 1;
    u16le(&mut e, 12, f);
    u64le(&mut e, n - 8, vcn);
    e
}

fn koncova(podstrom: Option<u64>) -> Vec<u8> {
    let mut e = vec![0u8; 16];
    u16le(&mut e, 8, 16);
    u16le(&mut e, 12, 2);
    match podstrom {
        Some(v) => s_podstromom(e, v),
        None => e,
    }
}

/// Hodnota $INDEX_ROOT s položkami (vrátane koncovej).
fn koren_indexu(typ: u32, zoradenie: u32, polozky: &[u8], velky: bool) -> Vec<u8> {
    let mut v = vec![0u8; 32];
    u32le(&mut v, 0, typ);
    u32le(&mut v, 4, zoradenie);
    u32le(&mut v, 8, INDX as u32);
    v[12] = (INDX as u64 / KLASTER).max(1) as u8;
    u32le(&mut v, 16, 16);
    u32le(&mut v, 20, 16 + polozky.len() as u32);
    u32le(&mut v, 24, 16 + polozky.len() as u32);
    v[28] = velky as u8;
    v.extend_from_slice(polozky);
    v
}

/// Indexový blok INDX.
fn blok_indexu(vcn: u64, polozky: &[u8], uzol: bool) -> Vec<u8> {
    let mut b = vec![0u8; INDX];
    b[0..4].copy_from_slice(b"INDX");
    let usa = INDX / KROK_USA + 1;
    u16le(&mut b, 4, 0x28);
    u16le(&mut b, 6, usa as u16);
    u64le(&mut b, 16, vcn);
    let polozky_od = zarovnaj8(0x28 + 2 * usa) - 0x18;
    u32le(&mut b, 0x18, polozky_od as u32);
    u32le(&mut b, 0x1C, (polozky_od + polozky.len()) as u32);
    u32le(&mut b, 0x20, (INDX - 0x18) as u32);
    b[0x24] = uzol as u8;
    let o = 0x18 + polozky_od;
    b[o..o + polozky.len()].copy_from_slice(polozky);
    ochran(&mut b, 0x28);
    b
}

/// Index priečinka: položky v koreni a bloky INDX (VCN = poradie bloku).
struct Index {
    koren: Vec<u8>,
    bloky: Vec<Vec<u8>>,
}

/// Zostaví B-strom z položiek zoradených podľa mena. `miesto_v_koreni` = bajty pre položky v
/// rezidentnom koreni, ak má index aj bloky.
fn zostav_index(polozky: Vec<Vec<u8>>, miesto_male: usize, miesto_v_koreni: usize) -> Index {
    let spolu: usize = polozky.iter().map(Vec::len).sum::<usize>() + 16;
    if spolu <= miesto_male {
        let mut k: Vec<u8> = polozky.concat();
        k.extend(koncova(None));
        return Index { koren: k, bloky: Vec::new() };
    }
    let miesto_bloku = INDX - zarovnaj8(0x28 + 2 * (INDX / KROK_USA + 1)) - 24;
    // úroveň: zoznam (položka bez podstromu, podstrom z nižšej úrovne)
    let mut bloky: Vec<Vec<u8>> = Vec::new();
    let mut uroven: Vec<(Vec<u8>, Option<u64>)> = polozky.into_iter().map(|p| (p, None)).collect();
    let mut koniec: Option<u64> = None;
    loop {
        let s_uzlom = |p: &(Vec<u8>, Option<u64>)| match p.1 {
            Some(v) => s_podstromom(p.0.clone(), v),
            None => p.0.clone(),
        };
        let velkost: usize = uroven.iter().map(|p| s_uzlom(p).len()).sum::<usize>() + koncova(koniec).len();
        if velkost <= miesto_v_koreni && !bloky.is_empty() {
            let mut k: Vec<u8> = uroven.iter().map(s_uzlom).collect::<Vec<_>>().concat();
            k.extend(koncova(koniec));
            return Index { koren: k, bloky };
        }
        // rozdeľ úroveň do blokov; medzi blokmi jedna položka ide o úroveň vyššie (s podstromom = blok)
        let uzol = uroven.iter().any(|p| p.1.is_some()) || koniec.is_some();
        let rezerva = if uzol { 24 } else { 16 };
        let mut vyssia: Vec<(Vec<u8>, Option<u64>)> = Vec::new();
        let mut aktualny: Vec<(Vec<u8>, Option<u64>)> = Vec::new();
        let mut obsadene = 0usize;
        let pocet = uroven.len();
        for (i, p) in uroven.into_iter().enumerate() {
            let e = s_uzlom(&p).len();
            if !aktualny.is_empty() && obsadene + e + rezerva > miesto_bloku {
                // ide hore p, alebo (ak je posledná) predchádzajúca položka
                let (hore, p_dalej) =
                    if i + 1 < pocet { (p, None) } else { (aktualny.pop().unwrap_or_default(), Some(p)) };
                let vcn = bloky.len() as u64;
                let mut obsah: Vec<u8> = aktualny.iter().map(s_uzlom).collect::<Vec<_>>().concat();
                obsah.extend(if uzol { koncova(hore.1) } else { koncova(None) });
                bloky.push(blok_indexu(vcn, &obsah, uzol));
                vyssia.push((hore.0, Some(vcn)));
                aktualny.clear();
                obsadene = 0;
                if let Some(p) = p_dalej {
                    obsadene += s_uzlom(&p).len();
                    aktualny.push(p);
                }
                continue;
            }
            obsadene += e;
            aktualny.push(p);
        }
        let aktualny: Vec<u8> = aktualny.iter().map(s_uzlom).collect::<Vec<_>>().concat();
        let mut aktualny = aktualny;
        let vcn = bloky.len() as u64;
        aktualny.extend(if uzol { koncova(koniec) } else { koncova(None) });
        bloky.push(blok_indexu(vcn, &aktualny, uzol));
        koniec = Some(vcn);
        uroven = vyssia;
    }
}

// ---- zväzok ----

/// Rozvrh zväzku NTFS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ntfs {
    pub sektor: u32,
    /// Dĺžka oddielu v sektoroch.
    pub sektory: u64,
    pub posun_oddielu: u64,
    pub klastrov: u64,
    pub zaznam: usize,
    pub seriove: u64,
    pub menovka: Vec<u16>,
}

/// Menovka NTFS: najviac 32 znakov, bez zakázaných znakov.
pub fn menovka(t: &str) -> Vec<u16> {
    let c: String = t.chars().filter(|c| !(*c < ' ' || "\"*/:<>?\\|".contains(*c))).collect();
    c.trim().encode_utf16().take(32).collect()
}

/// Veľkosť $LogFile podľa zväzku (ako Windows).
fn velkost_zurnalu(zvazok: u64) -> u64 {
    let v = if zvazok < 2 << 20 {
        256 << 10
    } else if zvazok < 4_000_000 {
        512 << 10
    } else if zvazok <= 200 << 20 {
        2 << 20
    } else if zvazok >= 12 << 30 {
        64 << 20
    } else {
        (zvazok / 200) & !(KLASTER - 1)
    };
    v.div_ceil(KLASTER) * KLASTER
}

/// Pridelenie klastrov (súvislé, za sebou).
struct Pridelenie {
    dalsi: u64,
    obsadene: Vec<(u64, u64)>,
}

impl Pridelenie {
    fn vezmi(&mut self, n: u64) -> Behy {
        if n == 0 {
            return Vec::new();
        }
        let l = self.dalsi;
        self.dalsi += n;
        self.obsadene.push((l, n));
        vec![(Some(l), n)]
    }
}

fn klastre(bajty: u64) -> u64 {
    bajty.div_ceil(KLASTER)
}

impl Ntfs {
    pub fn navrhni(sektory: u64, sektor: u32, posun_oddielu: u64, nazov: &str) -> Result<Ntfs, String> {
        if !matches!(sektor, 512 | 1024 | 2048 | 4096) {
            return Err(format!("nepodporovaná veľkosť sektora {sektor} B"));
        }
        let velkost = sektory * sektor as u64;
        if velkost < 16 << 20 {
            return Err("zväzok je primalý pre NTFS (aspoň 16 MB)".into());
        }
        let klastrov = (sektory - 1) * sektor as u64 / KLASTER;
        if klastrov > u32::MAX as u64 {
            return Err("zväzok je priveľký pre NTFS s klastrom 4 KB".into());
        }
        let n = jedinecne();
        Ok(Ntfs {
            sektor,
            sektory,
            posun_oddielu,
            klastrov,
            zaznam: (sektor as usize).max(1024),
            seriove: u64::from_le_bytes(n[8..16].try_into().unwrap_or([3; 8])),
            menovka: menovka(nazov),
        })
    }

    pub fn klaster_b(&self) -> u64 {
        KLASTER
    }

    /// Hrubý odhad klastrov pre strom (súbory, indexy, MFT) – na rozvrh oddielov.
    pub fn potrebne_klastre(&self, strom: &Strom) -> u64 {
        let subory: u64 = strom.subory().iter().map(|&u| klastre(strom.uzly[u].velkost)).sum();
        let mft = klastre((PRVY_POUZIVATELSKY + strom.uzly.len() as u64) * self.zaznam as u64);
        let indexy: u64 = strom.priecinky().iter().map(|&p| strom.uzly[p].deti.len() as u64 / 20 + 1).sum();
        subory + mft + indexy + klastre(velkost_zurnalu(self.klastrov * KLASTER)) + 200
    }

    /// Prázdny zväzok s menovkou.
    #[cfg(test)]
    pub fn naformatuj(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<()> {
        self.naplnene(c, posun, &Strom::novy(), &mut |_, _, _| Ok(()), priebeh, zrus).map(|_| ())
    }

    /// Zavádzací sektor.
    fn boot(&self, mft: u64, zrkadlo: u64) -> Vec<u8> {
        let mut b = vec![0u8; self.sektor as usize];
        b[0..3].copy_from_slice(&[0xEB, 0x52, 0x90]);
        b[3..11].copy_from_slice(b"NTFS    ");
        u16le(&mut b, 11, self.sektor as u16);
        b[13] = (KLASTER / self.sektor as u64) as u8;
        b[21] = 0xF8;
        u16le(&mut b, 24, 63);
        u16le(&mut b, 26, 255);
        u32le(&mut b, 28, self.posun_oddielu.min(u32::MAX as u64) as u32);
        b[36] = 0x80;
        b[38] = 0x80;
        u64le(&mut b, 40, self.sektory - 1);
        u64le(&mut b, 48, mft);
        u64le(&mut b, 56, zrkadlo);
        b[64] = if self.zaznam as u64 >= KLASTER {
            (self.zaznam as u64 / KLASTER) as u8
        } else {
            (-(self.zaznam.trailing_zeros() as i32)) as i8 as u8
        };
        b[68] = (INDX as u64 / KLASTER).max(1) as u8;
        u64le(&mut b, 72, self.seriove);
        let kod = kod_bez_systemu(0x54);
        b[0x54..0x54 + kod.len()].copy_from_slice(&kod);
        b[510] = 0x55;
        b[511] = 0xAA;
        b
    }

    /// Zväzok so súbormi zo stromu. Vráti zapísané súbory s odtlačkami.
    pub fn naplnene(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        strom: &Strom,
        citaj: &mut Citaj,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<Vec<Zapisany>> {
        let chyba = |e: &str| io::Error::other(e.to_string());
        let up = tabulka_velkych();
        let cas = cas_ntfs(teraz());
        let rz = self.zaznam;
        // 1. čísla záznamov: metasúbory 0–26, používateľ od 64
        let priecinky = strom.priecinky();
        let subory = strom.subory();
        let mut cislo = vec![0u64; strom.uzly.len()];
        cislo[0] = 5;
        let mut n = PRVY_POUZIVATELSKY;
        for &u in priecinky.iter().skip(1).chain(subory.iter()) {
            cislo[u] = n;
            n += 1;
        }
        let na_klaster = (KLASTER / rz as u64).max(1);
        let zaznamov = n.div_ceil(na_klaster) * na_klaster;
        let ref_korena = odkaz(5, 5);
        let ref_uzla = |u: usize| if u == 0 { ref_korena } else { odkaz(cislo[u], 1) };
        // kontrola mien: rovnaké bez ohľadu na veľkosť písmen v jednom priečinku NTFS nedovolí
        for &p in &priecinky {
            let mut mena: Vec<Vec<u16>> =
                strom.uzly[p].deti.iter().map(|&d| utf16(&strom.uzly[d].meno)).collect();
            mena.sort_by(|a, b| porovnaj(a, b, &up));
            for w in mena.windows(2) {
                if w[0].iter().map(|&c| up[c as usize]).eq(w[1].iter().map(|&c| up[c as usize])) {
                    return Err(chyba(&format!(
                        "dve mená líšiace sa len veľkosťou písmen: {}",
                        String::from_utf16_lossy(&w[0])
                    )));
                }
            }
        }
        // 2. rozloženie: $Boot, $MFTMirr, bitmapa MFT, $MFT, rezerva na rast MFT, ostatné
        let mut pr = Pridelenie { dalsi: 0, obsadene: Vec::new() };
        let boot = pr.vezmi(klastre(8192));
        let zrkadlo = pr.vezmi(klastre(4 * rz as u64));
        let bitmapa_mft_b = zaznamov.div_ceil(8).div_ceil(8) * 8;
        let bitmapa_mft = pr.vezmi(klastre(bitmapa_mft_b));
        pr.dalsi = pr.dalsi.max(klastre(16 * 1024));
        let mft_b = zaznamov * rz as u64;
        let mft = pr.vezmi(klastre(mft_b));
        pr.dalsi += (self.klastrov / 64).min(klastre(64 << 20));
        let zurnal_b = velkost_zurnalu(self.klastrov * KLASTER);
        let zurnal = pr.vezmi(klastre(zurnal_b));
        let atrdef = attrdef();
        let attrdef_behy = pr.vezmi(klastre(atrdef.len() as u64));
        let up_b: Vec<u8> = up.iter().flat_map(|c| c.to_le_bytes()).collect();
        let upcase = pr.vezmi(klastre(up_b.len() as u64));
        // $Secure:$SDS – položky zarovnané na 16 B, zrkadlo bloku na posune 0x40000
        let mut sds_blok = Vec::new();
        let mut sd_hlavicky = Vec::new();
        for (id, sd) in opisy() {
            let h = odtlacok_opisu(&sd);
            let od = sds_blok.len() as u64;
            let dlzka = 20 + sd.len();
            let mut hl = vec![0u8; 20];
            u32le(&mut hl, 0, h);
            u32le(&mut hl, 4, id);
            u64le(&mut hl, 8, od);
            u32le(&mut hl, 16, dlzka as u32);
            sds_blok.extend_from_slice(&hl);
            sds_blok.extend_from_slice(&sd);
            sds_blok.resize((sds_blok.len() + 15) & !15, 0);
            sd_hlavicky.push((h, id, hl));
        }
        let posledna = sd_hlavicky
            .last()
            .map_or(0, |x| u32::from_le_bytes(x.2[16..20].try_into().unwrap_or([0; 4])) as usize);
        let sds_velkost = 0x40000u64 + (sds_blok.len() - zarovnaj16_zvysok(posledna)) as u64;
        let mut sds = vec![0u8; sds_velkost as usize];
        sds[..sds_blok.len()].copy_from_slice(&sds_blok);
        let zrk = sds.len() - 0x40000;
        sds[0x40000..0x40000 + zrk].copy_from_slice(&sds_blok[..zrk]);
        let sds_behy = pr.vezmi(klastre(sds_velkost));
        let bitmapa_b = self.klastrov.div_ceil(8).div_ceil(8) * 8;
        let bitmapa = pr.vezmi(klastre(bitmapa_b));
        // 3. indexy priečinkov používateľa (koreň zvlášť – obsahuje metasúbory)
        let mena_uzlov: Vec<Vec<u16>> = strom.uzly.iter().map(|u| utf16(&u.meno)).collect();
        let fn_uzla = |u: usize, rodic: usize| -> Vec<u8> {
            let uz = &strom.uzly[u];
            let (pridelene, velkost, atr) = if uz.priecinok {
                (0, 0, INDEX_I30)
            } else {
                (klastre(uz.velkost) * KLASTER, uz.velkost, ARCHIV)
            };
            meno_suboru(ref_uzla(rodic), pridelene, velkost, atr, &mena_uzlov[u], 1, cas)
        };
        // metasúbory v koreni: (záznam, poradie, meno, pridelené, veľkosť, atribúty)
        let meta: Vec<(u64, u16, &str, u64, u64, u32)> = vec![
            (0, 1, "$MFT", klastre(mft_b) * KLASTER, mft_b, SKRYTY | SYSTEMOVY),
            (1, 1, "$MFTMirr", klastre(4 * rz as u64) * KLASTER, 4 * rz as u64, SKRYTY | SYSTEMOVY),
            (2, 2, "$LogFile", zurnal_b, zurnal_b, SKRYTY | SYSTEMOVY),
            (3, 3, "$Volume", 0, 0, SKRYTY | SYSTEMOVY),
            (4, 4, "$AttrDef", KLASTER, atrdef.len() as u64, SKRYTY | SYSTEMOVY),
            (5, 5, ".", 0, 0, SKRYTY | SYSTEMOVY | INDEX_I30),
            (6, 6, "$Bitmap", klastre(bitmapa_b) * KLASTER, bitmapa_b, SKRYTY | SYSTEMOVY),
            (7, 7, "$Boot", klastre(8192) * KLASTER, 8192, SKRYTY | SYSTEMOVY),
            (8, 8, "$BadClus", 0, 0, SKRYTY | SYSTEMOVY),
            (9, 9, "$Secure", 0, 0, SKRYTY | SYSTEMOVY | INDEX_POHLAD),
            (10, 10, "$UpCase", klastre(up_b.len() as u64) * KLASTER, up_b.len() as u64, SKRYTY | SYSTEMOVY),
            (11, 11, "$Extend", 0, 0, SKRYTY | SYSTEMOVY | INDEX_I30),
        ];
        // meno záznamu 1: zrkadlo má rovnaké poradie ako číslo
        let meta: Vec<_> =
            meta.into_iter().map(|m| if m.0 == 1 { (1, 1, m.2, m.3, m.4, m.5) } else { m }).collect();
        let fn_meta: Vec<(u64, u16, Vec<u16>, Vec<u8>)> = meta
            .iter()
            .map(|&(z, p, m, pr_, v, a)| {
                let w = utf16(m);
                let f = meno_suboru(ref_korena, pr_, v, a, &w, 3, cas);
                (z, p, w, f)
            })
            .collect();
        // miesto v zázname priečinka pre položky indexu
        let miesto = |dlzka_fn: usize, velky: bool| -> usize {
            let pevne = zarovnaj8(0x30 + 2 * (rz / KROK_USA + 1))
                + 8
                + 24
                + 72
                + zarovnaj8(24 + dlzka_fn)
                + 24
                + 8
                + 32;
            let velke = if velky { zarovnaj8(64 + 8 + 16) + zarovnaj8(24 + 8 + 8) } else { 0 };
            rz.saturating_sub(pevne + velke)
        };
        let mut indexy: Vec<Option<Index>> = (0..strom.uzly.len()).map(|_| None).collect();
        for &p in &priecinky {
            let mut polozky: Vec<(Vec<u16>, Vec<u8>)> = strom.uzly[p]
                .deti
                .iter()
                .map(|&d| (mena_uzlov[d].clone(), polozka_mena(ref_uzla(d), &fn_uzla(d, p))))
                .collect();
            let dlzka_fn = if p == 0 { 66 + 2 } else { 66 + 2 * mena_uzlov[p].len() };
            if p == 0 {
                for (z, por, w, f) in &fn_meta {
                    polozky.push((w.clone(), polozka_mena(odkaz(*z, *por), f)));
                }
            }
            polozky.sort_by(|a, b| porovnaj(&a.0, &b.0, &up));
            let e: Vec<Vec<u8>> = polozky.into_iter().map(|x| x.1).collect();
            indexy[p] = Some(zostav_index(e, miesto(dlzka_fn, false), miesto(dlzka_fn, true)));
        }
        let mut behy_indexu: Vec<Behy> = vec![Vec::new(); strom.uzly.len()];
        for &p in &priecinky {
            if let Some(ix) = &indexy[p] {
                behy_indexu[p] = pr.vezmi(ix.bloky.len() as u64 * (INDX as u64 / KLASTER));
            }
        }
        // 4. dáta súborov
        let mut behy_dat: Vec<Behy> = vec![Vec::new(); strom.uzly.len()];
        for &f in &subory {
            behy_dat[f] = pr.vezmi(klastre(strom.uzly[f].velkost));
        }
        if pr.dalsi > self.klastrov {
            return Err(chyba(&format!(
                "súbory sa nezmestia: treba {} MB, zväzok má {} MB",
                pr.dalsi * KLASTER / 1_000_000,
                self.klastrov * KLASTER / 1_000_000
            )));
        }
        // 5. záznamy MFT
        let si = |a: u32, s: u32| rezidentny(AT_SI, &[], &standardne_info(a, s, cas), false);
        let fn_atr = |f: &[u8]| rezidentny(AT_FN, &[], f, true);
        let i30 = utf16("$I30");
        let mut zaznamy: Vec<Option<Vec<u8>>> = (0..zaznamov).map(|_| None).collect();
        let uloz = |zaznamy: &mut Vec<Option<Vec<u8>>>, z: Zaznam| -> io::Result<()> {
            let c = z.cislo as usize;
            zaznamy[c] = Some(z.bajty(rz).map_err(|e| chyba(&e))?);
            Ok(())
        };
        let fn_meta_z = |z: u64| fn_meta.iter().find(|m| m.0 == z).map(|m| m.3.clone()).unwrap_or_default();
        let ziadne: Vec<u16> = Vec::new();
        // $MFT
        let mut z = Zaznam::novy(0, 1, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(0)));
        z.pridaj(nerezidentny(AT_DATA, &ziadne, &mft, mft_b, mft_b));
        z.pridaj(nerezidentny(AT_BITMAP, &ziadne, &bitmapa_mft, bitmapa_mft_b, bitmapa_mft_b));
        uloz(&mut zaznamy, z)?;
        // $MFTMirr
        let mut z = Zaznam::novy(1, 1, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(1)));
        z.pridaj(nerezidentny(AT_DATA, &ziadne, &zrkadlo, 4 * rz as u64, 4 * rz as u64));
        uloz(&mut zaznamy, z)?;
        // $LogFile
        let mut z = Zaznam::novy(2, 2, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(2)));
        z.pridaj(nerezidentny(AT_DATA, &ziadne, &zurnal, zurnal_b, zurnal_b));
        uloz(&mut zaznamy, z)?;
        // $Volume
        let mut z = Zaznam::novy(3, 3, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(3)));
        let meno_b: Vec<u8> = self.menovka.iter().flat_map(|c| c.to_le_bytes()).collect();
        z.pridaj(rezidentny(AT_VOLUME_NAME, &[], &meno_b, false));
        let mut vi = vec![0u8; 12];
        vi[8] = 3;
        vi[9] = 1;
        z.pridaj(rezidentny(AT_VOLUME_INFO, &[], &vi, false));
        z.pridaj(rezidentny(AT_DATA, &[], &[], false));
        uloz(&mut zaznamy, z)?;
        // $AttrDef
        let mut z = Zaznam::novy(4, 4, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(4)));
        z.pridaj(nerezidentny(AT_DATA, &ziadne, &attrdef_behy, atrdef.len() as u64, atrdef.len() as u64));
        uloz(&mut zaznamy, z)?;
        // priečinky (koreň = záznam 5)
        for &p in &priecinky {
            let ix = indexy[p].as_ref().ok_or_else(|| chyba("chýba index"))?;
            let velky = !ix.bloky.is_empty();
            let (mut z, f) = if p == 0 {
                let mut z = Zaznam::novy(5, 5, 3);
                z.pridaj(si(SKRYTY | SYSTEMOVY | ARCHIV, SEC_POUZIVATEL));
                (z, fn_meta_z(5))
            } else {
                let rodic = strom.uzly.iter().position(|u| u.deti.contains(&p)).unwrap_or(0);
                let mut z = Zaznam::novy(cislo[p], 1, 3);
                z.pridaj(si(0, SEC_POUZIVATEL));
                (z, fn_uzla(p, rodic))
            };
            z.odkazy = 1;
            z.pridaj(fn_atr(&f));
            z.pridaj(rezidentny(
                AT_INDEX_ROOT,
                &i30,
                &koren_indexu(AT_FN, ZORAD_MENO, &ix.koren, velky),
                false,
            ));
            if velky {
                let n = ix.bloky.len() as u64 * INDX as u64;
                z.pridaj(nerezidentny(AT_INDEX_ALLOC, &i30, &behy_indexu[p], n, n));
                let mut bm = vec![0u8; ix.bloky.len().div_ceil(64) * 8];
                for i in 0..ix.bloky.len() {
                    bm[i / 8] |= 1 << (i % 8);
                }
                z.pridaj(rezidentny(AT_BITMAP, &i30, &bm, false));
            }
            uloz(&mut zaznamy, z)?;
        }
        // $Bitmap
        let mut z = Zaznam::novy(6, 6, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(6)));
        z.pridaj(nerezidentny(AT_DATA, &ziadne, &bitmapa, bitmapa_b, bitmapa_b));
        uloz(&mut zaznamy, z)?;
        // $Boot
        let mut z = Zaznam::novy(7, 7, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(7)));
        z.pridaj(nerezidentny(AT_DATA, &ziadne, &boot, 8192, 8192));
        uloz(&mut zaznamy, z)?;
        // $BadClus – prázdny $DATA a riedky $Bad cez celý zväzok
        let mut z = Zaznam::novy(8, 8, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(8)));
        z.pridaj(rezidentny(AT_DATA, &[], &[], false));
        z.pridaj(nerezidentny(
            AT_DATA,
            &utf16("$Bad"),
            &vec![(None, self.klastrov)],
            self.klastrov * KLASTER,
            0,
        ));
        uloz(&mut zaznamy, z)?;
        // $Secure
        let mut z = Zaznam::novy(9, 9, 1 | 8);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY | INDEX_POHLAD, SEC_SYSTEM_PRIECINOK));
        z.pridaj(fn_atr(&fn_meta_z(9)));
        z.pridaj(nerezidentny(AT_DATA, &utf16("$SDS"), &sds_behy, sds_velkost, sds_velkost));
        let mut sdh: Vec<(u32, u32, Vec<u8>)> = sd_hlavicky
            .iter()
            .map(|(h, id, hl)| {
                let mut kluc = h.to_le_bytes().to_vec();
                kluc.extend(id.to_le_bytes());
                let mut data = hl.clone();
                data.extend(0x0049_0049u32.to_le_bytes());
                let mut e = polozka_pohladu(&kluc, &data, 0x18, 0x30);
                u16le(&mut e, 2, 0x14);
                (*h, *id, e)
            })
            .collect();
        sdh.sort_by_key(|x| (x.0, x.1));
        let mut sdh_b: Vec<u8> = sdh.into_iter().flat_map(|x| x.2).collect();
        sdh_b.extend(koncova(None));
        z.pridaj(rezidentny(
            AT_INDEX_ROOT,
            &utf16("$SDH"),
            &koren_indexu(0, ZORAD_HASH, &sdh_b, false),
            false,
        ));
        let mut sii: Vec<u8> = sd_hlavicky
            .iter()
            .flat_map(|(_, id, hl)| polozka_pohladu(&id.to_le_bytes(), hl, 0x14, 0x28))
            .collect();
        sii.extend(koncova(None));
        z.pridaj(rezidentny(
            AT_INDEX_ROOT,
            &utf16("$SII"),
            &koren_indexu(0, ZORAD_ULONG, &sii, false),
            false,
        ));
        uloz(&mut zaznamy, z)?;
        // $UpCase s $Info
        let mut z = Zaznam::novy(10, 10, 1);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
        z.pridaj(fn_atr(&fn_meta_z(10)));
        z.pridaj(nerezidentny(AT_DATA, &ziadne, &upcase, up_b.len() as u64, up_b.len() as u64));
        let mut info = vec![0u8; 32];
        u32le(&mut info, 0, 32);
        u64le(&mut info, 8, crc64(&up_b));
        z.pridaj(rezidentny(AT_DATA, &utf16("$Info"), &info, false));
        uloz(&mut zaznamy, z)?;
        // $Extend s $ObjId, $Quota, $Reparse
        let ref_extend = odkaz(11, 11);
        let rozsirene = [(25u64, "$ObjId"), (24, "$Quota"), (26, "$Reparse")];
        let atr_ext = SKRYTY | SYSTEMOVY | ARCHIV | INDEX_POHLAD;
        let fn_ext: Vec<(u64, Vec<u16>, Vec<u8>)> = rozsirene
            .iter()
            .map(|&(c, m)| {
                let w = utf16(m);
                let f = meno_suboru(ref_extend, 0, 0, atr_ext, &w, 3, cas);
                (c, w, f)
            })
            .collect();
        let mut z = Zaznam::novy(11, 11, 3);
        z.odkazy = 1;
        z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM_PRIECINOK));
        z.pridaj(fn_atr(&fn_meta_z(11)));
        let mut e: Vec<(Vec<u16>, Vec<u8>)> =
            fn_ext.iter().map(|(c, w, f)| (w.clone(), polozka_mena(odkaz(*c, 1), f))).collect();
        e.sort_by(|a, b| porovnaj(&a.0, &b.0, &up));
        let mut ext_b: Vec<u8> = e.into_iter().flat_map(|x| x.1).collect();
        ext_b.extend(koncova(None));
        z.pridaj(rezidentny(AT_INDEX_ROOT, &i30, &koren_indexu(AT_FN, ZORAD_MENO, &ext_b, false), false));
        uloz(&mut zaznamy, z)?;
        // rezervované systémové záznamy 12–15
        for c in 12..16u64 {
            let mut z = Zaznam::novy(c, c as u16, 1);
            z.pridaj(si(SKRYTY | SYSTEMOVY, SEC_SYSTEM));
            z.pridaj(rezidentny(AT_DATA, &[], &[], false));
            uloz(&mut zaznamy, z)?;
        }
        // $Quota, $ObjId, $Reparse
        for (c, _, f) in &fn_ext {
            let mut z = Zaznam::novy(*c, 1, 1 | 4 | 8);
            z.odkazy = 1;
            z.pridaj(si(atr_ext, SEC_SYSTEM_PRIECINOK));
            z.pridaj(fn_atr(f));
            match c {
                24 => {
                    let mut q = Vec::new();
                    let kontrola = |sid: Option<Vec<u8>>| {
                        let mut d = vec![0u8; 48];
                        u32le(&mut d, 0, 2);
                        u32le(&mut d, 4, 1);
                        u64le(&mut d, 16, cas);
                        u64le(&mut d, 24, u64::MAX);
                        u64le(&mut d, 32, u64::MAX);
                        if let Some(s) = sid {
                            d.extend(s);
                        }
                        d
                    };
                    q.extend(polozka_pohladu(&1u32.to_le_bytes(), &kontrola(None), 0x14, 0x48));
                    q.extend(polozka_pohladu(
                        &0x100u32.to_le_bytes(),
                        &kontrola(Some(sid_spravcovia())),
                        0x14,
                        0x58,
                    ));
                    q.extend(koncova(None));
                    let mut o = polozka_pohladu(&sid_spravcovia(), &0x100u32.to_le_bytes(), 0x20, 0x28);
                    u32le(&mut o, 0x24, 0x20);
                    o.extend(koncova(None));
                    z.pridaj(rezidentny(
                        AT_INDEX_ROOT,
                        &utf16("$O"),
                        &koren_indexu(0, ZORAD_SID, &o, false),
                        false,
                    ));
                    z.pridaj(rezidentny(
                        AT_INDEX_ROOT,
                        &utf16("$Q"),
                        &koren_indexu(0, ZORAD_ULONG, &q, false),
                        false,
                    ));
                }
                25 => {
                    z.pridaj(rezidentny(
                        AT_INDEX_ROOT,
                        &utf16("$O"),
                        &koren_indexu(0, ZORAD_ULONGS, &koncova(None), false),
                        false,
                    ));
                }
                _ => {
                    z.pridaj(rezidentny(
                        AT_INDEX_ROOT,
                        &utf16("$R"),
                        &koren_indexu(0, ZORAD_ULONGS, &koncova(None), false),
                        false,
                    ));
                }
            }
            uloz(&mut zaznamy, z)?;
        }
        // súbory používateľa
        for &f in &subory {
            let rodic = strom.uzly.iter().position(|u| u.deti.contains(&f)).unwrap_or(0);
            let mut z = Zaznam::novy(cislo[f], 1, 1);
            z.odkazy = 1;
            z.pridaj(si(ARCHIV, SEC_POUZIVATEL));
            z.pridaj(fn_atr(&fn_uzla(f, rodic)));
            let v = strom.uzly[f].velkost;
            if v == 0 {
                z.pridaj(rezidentny(AT_DATA, &[], &[], false));
            } else {
                z.pridaj(nerezidentny(AT_DATA, &ziadne, &behy_dat[f], v, v));
            }
            if z.velkost_atributov() + 64 > rz {
                return Err(chyba(&format!("meno je pridlhé: {}", strom.uzly[f].meno)));
            }
            uloz(&mut zaznamy, z)?;
        }
        // 6. zápis
        let s = KLASTER;
        let lcn = |b: &Behy| b.first().and_then(|x| x.0).unwrap_or(0);
        let mut mft_bajty = Vec::with_capacity(mft_b as usize);
        for (i, z) in zaznamy.into_iter().enumerate() {
            mft_bajty.extend(z.unwrap_or_else(|| prazdny_zaznam(i as u64, rz)));
        }
        let celkom = mft_b + zurnal_b + strom.velkost_suborov();
        // zavádzací sektor ($Boot) a jeho záloha v poslednom sektore oddielu
        let bs = self.boot(lcn(&mft), lcn(&zrkadlo));
        let mut boot_b = vec![0u8; (klastre(8192) * s) as usize];
        boot_b[..bs.len()].copy_from_slice(&bs);
        ciel::zapis_vsetko(c, posun, &boot_b)?;
        c.zapis(posun + (self.sektory - 1) * self.sektor as u64, &bs)?;
        ciel::zapis_vsetko(c, posun + lcn(&zrkadlo) * s, &mft_bajty[..4 * rz])?;
        let mut bm = vec![0u8; (klastre(bitmapa_mft_b) * s) as usize];
        for i in 0..zaznamov as usize {
            let pouzity = mft_bajty[i * rz + 22] & 1 != 0;
            if pouzity {
                bm[i / 8] |= 1 << (i % 8);
            }
        }
        ciel::zapis_vsetko(c, posun + lcn(&bitmapa_mft) * s, &bm)?;
        ciel::zapis_vsetko(c, posun + lcn(&mft) * s, &mft_bajty)?;
        priebeh(mft_b, celkom);
        // $LogFile: samé 0xFF = prázdny žurnál, Windows ho pri pripojení založí
        let ff = vec![0xFFu8; KUS];
        let mut p = 0;
        while p < zurnal_b {
            if zrus.load(Ordering::Relaxed) {
                return Err(chyba("zrušené"));
            }
            let n = (KUS as u64).min(zurnal_b - p) as usize;
            c.zapis(posun + lcn(&zurnal) * s + p, &ff[..n])?;
            p += n as u64;
            priebeh(mft_b + p, celkom);
        }
        let mut a = atrdef.clone();
        a.resize(KLASTER as usize, 0);
        ciel::zapis_vsetko(c, posun + lcn(&attrdef_behy) * s, &a)?;
        ciel::zapis_vsetko(c, posun + lcn(&upcase) * s, &up_b)?;
        let mut sds_z = sds.clone();
        sds_z.resize((klastre(sds_velkost) * s) as usize, 0);
        ciel::zapis_vsetko(c, posun + lcn(&sds_behy) * s, &sds_z)?;
        for &p in &priecinky {
            if let Some(ix) = &indexy[p]
                && !ix.bloky.is_empty()
            {
                ciel::zapis_vsetko(c, posun + lcn(&behy_indexu[p]) * s, &ix.bloky.concat())?;
            }
        }
        // bitmapa klastrov: obsadené behy a bity za koncom zväzku
        let mut b = vec![0u8; (klastre(bitmapa_b) * s) as usize];
        let mut nastav = |od: u64, n: u64| {
            for i in od..od + n {
                b[(i / 8) as usize] |= 1 << (i % 8);
            }
        };
        for &(od, n) in &pr.obsadene {
            nastav(od, n);
        }
        nastav(self.klastrov, bitmapa_b * 8 - self.klastrov);
        ciel::zapis_vsetko(c, posun + lcn(&bitmapa) * s, &b)?;
        // dáta súborov
        let mut zapisane = Vec::new();
        let mut buf = vec![0u8; KUS];
        let mut hotovo = mft_b + zurnal_b;
        for &f in &subory {
            let velkost = strom.uzly[f].velkost;
            let od = if velkost > 0 { posun + lcn(&behy_dat[f]) * s } else { 0 };
            let mut h = Hasovac::novy();
            let mut v = 0;
            while v < velkost {
                if zrus.load(Ordering::Relaxed) {
                    return Err(chyba("zrušené"));
                }
                let n = (KUS as u64).min(velkost - v) as usize;
                citaj(f, v, &mut buf[..n])?;
                h.pridaj(&buf[..n]);
                c.zapis(od + v, &buf[..n])?;
                v += n as u64;
                hotovo += n as u64;
                priebeh(hotovo, celkom);
            }
            zapisane.push(Zapisany { uzol: f, posun: od, velkost, odtlacok: h.dokonci().0 });
        }
        priebeh(celkom, celkom);
        Ok(zapisane)
    }
}

/// Koľko bajtov zarovnania na 16 B má posledná položka $SDS (aby dĺžka prúdu končila na nej).
fn zarovnaj16_zvysok(dlzka_poslednej: usize) -> usize {
    ((dlzka_poslednej + 15) & !15) - dlzka_poslednej
}

impl crate::subory::Zvazok for Ntfs {
    fn klaster_b(&self) -> u64 {
        Ntfs::klaster_b(self)
    }
    fn naplnene(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        strom: &Strom,
        citaj: &mut Citaj,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<Vec<Zapisany>> {
        Ntfs::naplnene(self, c, posun, strom, citaj, priebeh, zrus)
    }
    fn over(&self, zaciatok: &[u8]) -> bool {
        zaciatok.len() >= 11 && &zaciatok[3..11] == b"NTFS    "
    }
    fn zmesti_sa(&self, strom: &Strom) -> bool {
        self.potrebne_klastre(strom) + 16 <= self.klastrov
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::ciel::Pamat;

    #[test]
    fn tabulka_a_crc() {
        let t = tabulka_velkych();
        assert_eq!(t['a' as usize], 'A' as u16);
        assert_eq!(t['ž' as usize], 'Ž' as u16);
        assert_eq!(t['ŕ' as usize], 'Ŕ' as u16);
        assert_eq!(t['я' as usize], 'Я' as u16);
        assert_eq!(t['A' as usize], 'A' as u16);
        // kontrolná hodnota overená nezávislým výpočtom (rovnaký polynóm ako $UpCase:$Info)
        assert_eq!(crc64(b"123456789"), 0xAE8B_1486_0A79_9888);
        assert_eq!(porovnaj(&utf16("abc"), &utf16("ABD"), &t), Poradie::Less);
        assert_eq!(porovnaj(&utf16("$MFT"), &utf16("$MFTMirr"), &t), Poradie::Less);
        assert_eq!(porovnaj(&utf16("$Volume"), &utf16("."), &t), Poradie::Less);
    }

    #[test]
    fn opis_a_odtlacok_ako_mkntfs() {
        // prvý opis zo $SDS zväzkov mkntfs má známy odtlačok 0xF80312F0
        let s = sid_system();
        let a = sid_spravcovia();
        let sd = opis(&a, &a, &[(0, 0x0012_0089, s), (0, 0x0012_0089, a.clone())]);
        assert_eq!(sd.len(), 0x68);
        assert_eq!(odtlacok_opisu(&sd), 0xF803_12F0);
    }

    #[test]
    fn mapovanie_behov() {
        assert_eq!(mapovanie(&vec![(Some(4), 7)]), vec![0x11, 7, 4, 0]);
        assert_eq!(mapovanie(&vec![(Some(0x100), 0x80)]), vec![0x22, 0x80, 0, 0, 1, 0]);
        assert_eq!(mapovanie(&vec![(None, 1000)]), vec![0x02, 0xE8, 0x03, 0]);
        // druhý beh pred prvým: záporný posun
        assert_eq!(mapovanie(&vec![(Some(100), 1), (Some(90), 1)]), vec![0x11, 1, 100, 0x11, 1, 0xF6, 0]);
    }

    #[test]
    fn velky_index_ma_bloky_a_poradie() {
        let up = tabulka_velkych();
        let mut e: Vec<(Vec<u16>, Vec<u8>)> = (0..600)
            .map(|i| {
                let m = utf16(&format!("subor s dlhym menom cislo {i:04}.txt"));
                let f = meno_suboru(odkaz(5, 5), 0, 0, ARCHIV, &m, 1, 0);
                (m, polozka_mena(odkaz(100 + i, 1), &f))
            })
            .collect();
        e.sort_by(|a, b| porovnaj(&a.0, &b.0, &up));
        let ix = zostav_index(e.into_iter().map(|x| x.1).collect(), 500, 400);
        assert!(ix.bloky.len() > 10, "{} blokov", ix.bloky.len());
        assert!(ix.koren.len() <= 400);
        for b in &ix.bloky {
            assert_eq!(&b[0..4], b"INDX");
        }
    }

    #[test]
    fn naformatovany_zvazok() {
        let mut p = Pamat::nova(64 << 20, 512);
        let n = Ntfs::navrhni((64 << 20) / 512, 512, 0, "SADIČ").unwrap();
        n.naformatuj(&mut p, 0, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(&p.data[3..11], b"NTFS    ");
        let posledny = p.data.len() - 512;
        assert_eq!(&p.data[..512], &p.data[posledny..], "záloha zavádzacieho sektora");
        let mft = u64::from_le_bytes(p.data[48..56].try_into().unwrap()) as usize * 4096;
        assert_eq!(&p.data[mft..mft + 4], b"FILE");
    }
}
