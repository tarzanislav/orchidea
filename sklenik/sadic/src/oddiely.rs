//! Tabuľky oddielov MBR a GPT – zápis aj čítanie, vlastným kódom podľa špecifikácií (MBR: IBM PC,
//! GPT: UEFI 2.x, kap. 5). Zarovnanie oddielov podľa veľkosti disku, ako ho robí SD Formatter: karta
//! potom zapisuje do celých blokov svojej pamäte flash, je rýchlejšia a dlhšie vydrží.

use crate::zip::Crc32;
use orchidea_jadro::Hasovac;
use std::sync::atomic::{AtomicU64, Ordering};

const MIB: u64 = 1 << 20;
const GB: u64 = 1_000_000_000;

/// Schéma tabuľky oddielov.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    /// Klasická tabuľka (BIOS aj UEFI, disky do 2 TiB) – najširšia podpora v zariadeniach.
    Mbr,
    /// GUID Partition Table (UEFI) so zálohou na konci disku.
    Gpt,
}

impl Schema {
    pub fn nazov(self) -> &'static str {
        match self {
            Schema::Mbr => "MBR",
            Schema::Gpt => "GPT",
        }
    }
}

/// Druh oddielu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum Typ {
    Fat32,
    /// exFAT aj NTFS majú v MBR aj GPT rovnaký kód („základné dáta“).
    ExFat,
    /// Systémový oddiel EFI.
    Esp,
    Linux,
    /// HFS+ (Mac OS Extended).
    Hfs,
}

/// GPT: základné dáta (Microsoft Basic Data).
pub const GUID_ZAKLADNE_DATA: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";
/// GPT: systémový oddiel EFI.
pub const GUID_ESP: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
/// GPT: súborový systém Linuxu.
pub const GUID_LINUX: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
/// GPT: HFS+ (Apple).
pub const GUID_HFS: &str = "48465300-0000-11AA-AA11-00306543ECAC";

impl Typ {
    pub fn mbr(self) -> u8 {
        match self {
            Typ::Fat32 => 0x0C,
            Typ::ExFat => 0x07,
            Typ::Esp => 0xEF,
            Typ::Linux => 0x83,
            Typ::Hfs => 0xAF,
        }
    }

    pub fn gpt(self) -> [u8; 16] {
        guid(match self {
            Typ::Fat32 | Typ::ExFat => GUID_ZAKLADNE_DATA,
            Typ::Esp => GUID_ESP,
            Typ::Linux => GUID_LINUX,
            Typ::Hfs => GUID_HFS,
        })
    }
}

/// Oddiel: začiatok a dĺžka v sektoroch disku.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oddiel {
    pub zaciatok: u64,
    pub sektory: u64,
    pub typ: Typ,
    pub meno: String,
    pub aktivny: bool,
}

/// GUID z textu (prvé tri skupiny sú v pamäti little-endian, posledné dve big-endian).
pub fn guid(text: &str) -> [u8; 16] {
    let hex: Vec<u8> = text
        .bytes()
        .filter(u8::is_ascii_hexdigit)
        .map(|c| (c as char).to_digit(16).unwrap_or(0) as u8)
        .collect();
    let mut b = [0u8; 16];
    for (i, par) in hex.chunks(2).take(16).enumerate() {
        b[i] = par[0] << 4 | par.get(1).copied().unwrap_or(0);
    }
    b[0..4].reverse();
    b[4..6].reverse();
    b[6..8].reverse();
    b
}

/// GUID ako text (na výpisy a skúšky).
#[cfg(test)]
pub fn guid_text(b: &[u8; 16]) -> String {
    let mut c = *b;
    c[0..4].reverse();
    c[4..6].reverse();
    c[6..8].reverse();
    let h: String = c.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

static POCITADLO: AtomicU64 = AtomicU64::new(0);

/// 32 nepredvídateľných bajtov (čas, proces, počítadlo) – pre GUID a sériové čísla zväzkov.
pub fn jedinecne() -> [u8; 32] {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let mut h = Hasovac::novy();
    h.pridaj(b"SADIC jedinecne");
    h.pridaj(&t.as_nanos().to_le_bytes());
    // na WebAssembly (WASI) číslo procesu nie je
    #[cfg(not(target_family = "wasm"))]
    h.pridaj(&std::process::id().to_le_bytes());
    h.pridaj(&POCITADLO.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let adresa = &h as *const Hasovac as usize;
    h.pridaj(&adresa.to_le_bytes());
    h.dokonci().0
}

/// Náhodný GUID verzie 4.
pub fn novy_guid() -> [u8; 16] {
    let mut b = [0u8; 16];
    b.copy_from_slice(&jedinecne()[..16]);
    b[7] = (b[7] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    b
}

/// Zarovnanie začiatku oddielu a dátovej oblasti podľa veľkosti média (blok vymazania pamäte flash
/// je pri väčších kartách väčší).
pub fn zarovnanie(velkost: u64) -> u64 {
    match velkost {
        v if v < GB => MIB,
        v if v <= 32 * GB => 4 * MIB,
        v if v <= 128 * GB => 16 * MIB,
        v if v <= 512 * GB => 32 * MIB,
        _ => 64 * MIB,
    }
}

/// Sektory, ktoré GPT potrebuje na konci disku (záložné položky + hlavička).
pub fn gpt_koniec(sektor: u32) -> u64 {
    (128 * 128u64).div_ceil(sektor as u64) + 1
}

/// Jeden oddiel cez celý disk, zarovnaný na začiatku aj dĺžkou (na celé MiB).
pub fn jeden_oddiel(
    velkost: u64,
    sektor: u32,
    schema: Schema,
    typ: Typ,
    meno: &str,
) -> Result<Oddiel, String> {
    let s = sektor as u64;
    let zac = zarovnanie(velkost) / s;
    let mut koniec = velkost / s;
    if schema == Schema::Gpt {
        koniec -= gpt_koniec(sektor);
    }
    if schema == Schema::Mbr {
        koniec = koniec.min(u32::MAX as u64);
    }
    let na_mib = (MIB / s).max(1);
    let sektory = (koniec.saturating_sub(zac)) / na_mib * na_mib;
    if sektory < 2 * na_mib {
        return Err(format!("disk je primalý ({} B)", velkost));
    }
    Ok(Oddiel { zaciatok: zac, sektory, typ, meno: meno.into(), aktivny: false })
}

/// CHS adresa pre MBR (255 hláv, 63 sektorov; za hranicou 1023 valcov FE FF FF).
fn chs(lba: u64) -> [u8; 3] {
    let (h, s) = (255u64, 63u64);
    let c = lba / (h * s);
    if c > 1023 {
        return [0xFE, 0xFF, 0xFF];
    }
    let hlava = (lba / s) % h;
    let sek = lba % s + 1;
    [hlava as u8, (sek as u8) | (((c >> 2) & 0xC0) as u8), c as u8]
}

fn zaznam_mbr(b: &mut [u8], aktivny: bool, typ: u8, zac: u64, dlzka: u64) {
    b[0] = if aktivny { 0x80 } else { 0 };
    b[1..4].copy_from_slice(&chs(zac));
    b[4] = typ;
    b[5..8].copy_from_slice(&chs(zac + dlzka.max(1) - 1));
    b[8..12].copy_from_slice(&(zac as u32).to_le_bytes());
    b[12..16].copy_from_slice(&(dlzka.min(u32::MAX as u64) as u32).to_le_bytes());
}

/// Strojový kód x86, ktorý pri štarte z disku bez systému vypíše správu, počká na kláves a odovzdá
/// štart ďalšiemu zariadeniu (INT 19h). `zac` = posun kódu v sektore (načítanom na 0x7C00).
pub fn kod_bez_systemu(zac: usize) -> Vec<u8> {
    let sprava = b"\r\nSADIC: na tomto disku nie je system na spustenie.\r\n\
Vyberte iny disk a stlacte klaves.\r\n\0";
    let adresa = (0x7C00 + zac + 37) as u16;
    let mut k = vec![
        0xFA, // cli
        0x31,
        0xC0, // xor ax, ax
        0x8E,
        0xD8, // mov ds, ax
        0x8E,
        0xD0, // mov ss, ax
        0xBC,
        0x00,
        0x7C, // mov sp, 0x7C00
        0xFB, // sti
        0xBE,
        adresa as u8,
        (adresa >> 8) as u8, // mov si, správa
        0xAC,                // slučka: lodsb
        0x84,
        0xC0, // test al, al
        0x74,
        0x09, // jz koniec
        0xB4,
        0x0E, // mov ah, 0x0E
        0xBB,
        0x07,
        0x00, // mov bx, 7
        0xCD,
        0x10, // int 0x10
        0xEB,
        0xF2, // jmp slučka
        0x31,
        0xC0, // koniec: xor ax, ax
        0xCD,
        0x16, // int 0x16 (kláves)
        0xCD,
        0x19, // int 0x19 (ďalšie zariadenie)
        0xF4, // hlt
        0xEB,
        0xFD, // jmp hlt
    ];
    debug_assert_eq!(k.len(), 37);
    k.extend_from_slice(sprava);
    k
}

/// Sektor MBR s až štyrmi oddielmi.
pub fn mbr(sektor: u32, oddiely: &[Oddiel], podpis: u32) -> Vec<u8> {
    let mut b = vec![0u8; sektor as usize];
    let kod = kod_bez_systemu(0);
    b[..kod.len()].copy_from_slice(&kod);
    b[440..444].copy_from_slice(&podpis.to_le_bytes());
    for (i, o) in oddiely.iter().take(4).enumerate() {
        zaznam_mbr(&mut b[446 + 16 * i..462 + 16 * i], o.aktivny, o.typ.mbr(), o.zaciatok, o.sektory);
    }
    b[510] = 0x55;
    b[511] = 0xAA;
    b
}

/// Tabuľka GPT: (začiatok disku od LBA 0 po koniec položiek, posun zálohy v bajtoch, záloha).
pub fn gpt(
    sektory_disku: u64,
    sektor: u32,
    oddiely: &[Oddiel],
    disk_guid: [u8; 16],
) -> (Vec<u8>, u64, Vec<u8>) {
    let s = sektor as usize;
    let polozky_sektory = (128 * 128usize).div_ceil(s);
    let posledny = sektory_disku - 1;
    let prvy_volny = 2 + polozky_sektory as u64;
    let posledny_volny = posledny - polozky_sektory as u64 - 1;
    let mut polozky = vec![0u8; polozky_sektory * s];
    for (i, o) in oddiely.iter().take(128).enumerate() {
        let p = &mut polozky[i * 128..(i + 1) * 128];
        p[0..16].copy_from_slice(&o.typ.gpt());
        p[16..32].copy_from_slice(&novy_guid());
        p[32..40].copy_from_slice(&o.zaciatok.to_le_bytes());
        p[40..48].copy_from_slice(&(o.zaciatok + o.sektory - 1).to_le_bytes());
        for (j, z) in o.meno.encode_utf16().take(36).enumerate() {
            p[56 + 2 * j..58 + 2 * j].copy_from_slice(&z.to_le_bytes());
        }
    }
    let mut crc = Crc32::novy();
    crc.pridaj(&polozky[..128 * 128]);
    let crc_poloziek = crc.hodnota();
    let hlavicka = |moja: u64, druha: u64, polozky_od: u64| {
        let mut h = vec![0u8; s];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&moja.to_le_bytes());
        h[32..40].copy_from_slice(&druha.to_le_bytes());
        h[40..48].copy_from_slice(&prvy_volny.to_le_bytes());
        h[48..56].copy_from_slice(&posledny_volny.to_le_bytes());
        h[56..72].copy_from_slice(&disk_guid);
        h[72..80].copy_from_slice(&polozky_od.to_le_bytes());
        h[80..84].copy_from_slice(&128u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&crc_poloziek.to_le_bytes());
        let mut c = Crc32::novy();
        c.pridaj(&h[..92]);
        let v = c.hodnota();
        h[16..20].copy_from_slice(&v.to_le_bytes());
        h
    };
    // ochranný MBR: jeden oddiel 0xEE cez celý disk
    let mut ochranny = vec![0u8; s];
    let kod = kod_bez_systemu(0);
    ochranny[..kod.len()].copy_from_slice(&kod);
    zaznam_mbr(&mut ochranny[446..462], false, 0xEE, 1, (sektory_disku - 1).min(u32::MAX as u64));
    ochranny[446 + 1..446 + 4].copy_from_slice(&[0x00, 0x02, 0x00]);
    ochranny[510] = 0x55;
    ochranny[511] = 0xAA;
    let mut zaciatok = ochranny;
    zaciatok.extend(hlavicka(1, posledny, 2));
    zaciatok.extend_from_slice(&polozky);
    let zaloha_od = posledny - polozky_sektory as u64;
    let mut zaloha = polozky;
    zaloha.extend(hlavicka(posledny, 1, zaloha_od));
    (zaciatok, zaloha_od * sektor as u64, zaloha)
}

/// Prečítaný záznam MBR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZaznamMbr {
    pub aktivny: bool,
    pub typ: u8,
    pub zaciatok: u64,
    pub sektory: u64,
}

/// Rozloží sektor MBR (None, ak chýba podpis 55 AA).
pub fn rozloz_mbr(b: &[u8]) -> Option<Vec<ZaznamMbr>> {
    if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
        return None;
    }
    let u32_na = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as u64;
    Some(
        (0..4)
            .map(|i| 446 + 16 * i)
            .filter(|&p| b[p + 4] != 0 && u32_na(p + 12) != 0)
            .map(|p| ZaznamMbr {
                aktivny: b[p] & 0x80 != 0,
                typ: b[p + 4],
                zaciatok: u32_na(p + 8),
                sektory: u32_na(p + 12),
            })
            .collect(),
    )
}

/// Prečítaná položka GPT.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolozkaGpt {
    pub typ: [u8; 16],
    pub prvy: u64,
    pub posledny: u64,
    pub meno: String,
}

/// Rozloží hlavičku GPT a jej položky (`data` začína na LBA 0). Overí podpis a CRC hlavičky aj položiek.
#[cfg(test)]
pub fn rozloz_gpt(data: &[u8], sektor: usize) -> Result<Vec<PolozkaGpt>, String> {
    let h = data.get(sektor..sektor + 92).ok_or("krátke dáta")?;
    if &h[0..8] != b"EFI PART" {
        return Err("chýba podpis EFI PART".into());
    }
    let u32_na = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let u64_na = |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]));
    let mut kopia = h.to_vec();
    kopia[16..20].fill(0);
    let mut c = Crc32::novy();
    c.pridaj(&kopia);
    if c.hodnota() != u32_na(h, 16) {
        return Err("zlé CRC hlavičky".into());
    }
    let od = u64_na(h, 72) as usize * sektor;
    let (pocet, velkost) = (u32_na(h, 80) as usize, u32_na(h, 84) as usize);
    let p = data.get(od..od + pocet * velkost).ok_or("položky mimo dát")?;
    let mut c = Crc32::novy();
    c.pridaj(p);
    if c.hodnota() != u32_na(h, 88) {
        return Err("zlé CRC položiek".into());
    }
    Ok(p.chunks(velkost)
        .filter(|z| z[0..16].iter().any(|&b| b != 0))
        .map(|z| {
            let meno: Vec<u16> = z[56..128]
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&c| c != 0)
                .collect();
            PolozkaGpt {
                typ: z[0..16].try_into().unwrap_or([0; 16]),
                prvy: u64_na(z, 32),
                posledny: u64_na(z, 40),
                meno: String::from_utf16_lossy(&meno),
            }
        })
        .collect())
}

/// Text bez diakritiky (Ľ → L, ô → o …); ostatné znaky mimo ASCII vypadnú.
pub fn bez_diakritiky(t: &str) -> String {
    const Z: &str = "áäčďéěíĺľňóôöŕřšťúůüýžÁÄČĎÉĚÍĹĽŇÓÔÖŔŘŠŤÚŮÜÝŽ";
    const NA: &str = "aacdeeillnooorrstuuuyzAACDEEILLNOOORRSTUUUYZ";
    t.chars()
        .filter_map(|c| match Z.chars().position(|z| z == c) {
            Some(i) => NA.chars().nth(i),
            None => c.is_ascii().then_some(c),
        })
        .collect()
}

/// Občiansky dátum a čas (rok, mesiac, deň, hodina, minúta, sekunda) z času Unix (UTC).
pub fn obciansky(unix: u64) -> (i64, u8, u8, u8, u8, u8) {
    let dni = (unix / 86_400) as i64;
    let sek = unix % 86_400;
    // občiansky dátum z počtu dní (H. Hinnant)
    let z = dni + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let den = doy - (153 * mp + 2) / 5 + 1;
    let mesiac = if mp < 10 { mp + 3 } else { mp - 9 };
    let rok = yoe + era * 400 + i64::from(mesiac <= 2);
    (rok, mesiac as u8, den as u8, (sek / 3600) as u8, ((sek / 60) % 60) as u8, (sek % 60) as u8)
}

/// Dátum a čas vo formáte DOS (FAT) z času Unix (UTC).
pub fn dos_cas(unix: u64) -> (u16, u16) {
    let (rok, mesiac, den, h, m, sek) = obciansky(unix);
    let datum = (((rok - 1980).clamp(0, 127) as u16) << 9) | ((mesiac as u16) << 5) | den as u16;
    let cas = ((h as u16) << 11) | ((m as u16) << 5) | (sek / 2) as u16;
    (datum, cas)
}

/// Aktuálny čas Unix v sekundách.
pub fn teraz() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn guid_tam_a_spat() {
        let g = guid(GUID_ZAKLADNE_DATA);
        assert_eq!(&g[..4], &[0xA2, 0xA0, 0xD0, 0xEB], "prvá skupina je little-endian");
        assert_eq!(guid_text(&g), GUID_ZAKLADNE_DATA);
        assert_eq!(guid_text(&Typ::Esp.gpt()), GUID_ESP);
        assert_eq!(guid_text(&Typ::Linux.gpt()), GUID_LINUX);
        assert_eq!((Typ::Esp.mbr(), Typ::Linux.mbr(), Typ::ExFat.mbr()), (0xEF, 0x83, 0x07));
        let a = novy_guid();
        let b = novy_guid();
        assert_ne!(a, b);
        assert_eq!(a[7] >> 4, 4, "verzia 4");
    }

    #[test]
    fn mbr_tam_a_spat() {
        let o = Oddiel {
            zaciatok: 8192,
            sektory: 1_000_000,
            typ: Typ::Fat32,
            meno: String::new(),
            aktivny: true,
        };
        let m = mbr(512, std::slice::from_ref(&o), 0x1234_5678);
        let z = rozloz_mbr(&m).unwrap();
        assert_eq!(z, vec![ZaznamMbr { aktivny: true, typ: 0x0C, zaciatok: 8192, sektory: 1_000_000 }]);
        assert_eq!(&m[440..444], &0x1234_5678u32.to_le_bytes());
        assert_eq!(chs(0), [0, 1, 0]);
        assert_eq!(chs(16_450_560), [0xFE, 0xFF, 0xFF], "za 1023 valcami");
    }

    #[test]
    fn gpt_tam_a_spat_aj_so_4k_sektorom() {
        for sektor in [512u32, 4096] {
            let velkost = 64 * MIB;
            let n = velkost / sektor as u64;
            let o = jeden_oddiel(velkost, sektor, Schema::Gpt, Typ::ExFat, "SADIČ").unwrap();
            assert_eq!(o.zaciatok * sektor as u64, MIB, "malý disk: zarovnanie 1 MiB");
            assert!(o.zaciatok + o.sektory <= n - gpt_koniec(sektor));
            let (zac, zaloha_od, zaloha) = gpt(n, sektor, std::slice::from_ref(&o), novy_guid());
            let p = rozloz_gpt(&zac, sektor as usize).unwrap();
            assert_eq!(p.len(), 1);
            assert_eq!(p[0].prvy, o.zaciatok);
            assert_eq!(p[0].posledny, o.zaciatok + o.sektory - 1);
            assert_eq!(p[0].meno, "SADIČ");
            assert_eq!(guid_text(&p[0].typ), GUID_ZAKLADNE_DATA);
            assert_eq!(zaloha_od + zaloha.len() as u64, velkost, "záloha končí na poslednom sektore");
            // ochranný MBR
            let m = rozloz_mbr(&zac).unwrap();
            assert_eq!(m[0].typ, 0xEE);
            // záloha: hlavička na poslednom sektore ukazuje na seba
            let h = &zaloha[zaloha.len() - sektor as usize..];
            assert_eq!(&h[0..8], b"EFI PART");
            assert_eq!(u64::from_le_bytes(h[24..32].try_into().unwrap()), n - 1);
        }
    }

    #[test]
    fn zarovnanie_rastie_s_kartou() {
        assert_eq!(zarovnanie(512 * MIB), MIB);
        assert_eq!(zarovnanie(32 * GB), 4 * MIB);
        assert_eq!(zarovnanie(64 * GB), 16 * MIB);
        assert_eq!(zarovnanie(256 * GB), 32 * MIB);
        assert_eq!(zarovnanie(1000 * GB), 64 * MIB);
    }

    #[test]
    fn pomocnici() {
        assert_eq!(bez_diakritiky("Ľadová ŠKOLA č.5 ☃"), "Ladova SKOLA c.5 ");
        // 22. 11. 2026 18:58:30 UTC
        let (d, c) = dos_cas(1_795_373_910);
        assert_eq!((d >> 9) + 1980, 2026);
        assert_eq!((d >> 5) & 15, 11);
        assert_eq!(d & 31, 22);
        assert_eq!(c >> 11, 18);
        assert_eq!((c >> 5) & 63, 58);
        let k = kod_bez_systemu(0x5A);
        assert_eq!(k[11..14], [0xBE, 0x7F, 0x7C], "adresa správy 0x7C00 + 0x5A + 37");
        assert!(k.len() < 420);
    }
}
