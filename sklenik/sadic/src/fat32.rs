//! FAT32 vlastným kódom (Microsoft FAT32 File System Specification 1.03). Dátová oblasť sa zarovná na
//! hranicu pamäte flash (ako to robí SD Formatter) a veľkosť klastra sa volí podľa karty: väčší klaster
//! = menej zápisov do tabuľky FAT, čo karta zvládne rýchlejšie a s menším opotrebovaním.

use crate::ciel::{self, Ciel};
use crate::oddiely::{bez_diakritiky, dos_cas, jedinecne, kod_bez_systemu, teraz};
use crate::subory::{Citaj, Strom, Zapisany};
use orchidea_jadro::Hasovac;
use std::collections::BTreeSet;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

/// Najmenší počet klastrov zväzku FAT32 (menej by bol podľa špecifikácie FAT16).
pub const NAJMENEJ_KLASTROV: u64 = 65_525;
/// Najväčší počet klastrov (28-bitové položky bez vyhradených hodnôt).
pub const NAJVIAC_KLASTROV: u64 = 0x0FFF_FFF5 - 2;

/// Rozvrh zväzku FAT32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fat32 {
    /// Bajty na sektor.
    pub sektor: u32,
    /// Dĺžka zväzku v sektoroch.
    pub sektory: u64,
    /// Začiatok oddielu na disku v sektoroch (pole „skryté sektory“).
    pub skryte: u64,
    pub na_klaster: u32,
    pub rezervovane: u32,
    /// Sektory jednej tabuľky FAT (tabuľky sú dve).
    pub fat: u32,
    pub klastre: u32,
    pub seriove: u32,
    pub menovka: [u8; 11],
}

/// Menovka zväzku FAT: najviac 11 znakov ASCII veľkými písmenami, bez zakázaných znakov.
pub fn menovka(t: &str) -> [u8; 11] {
    let mut m = [b' '; 11];
    let cista: Vec<u8> = bez_diakritiky(t)
        .to_ascii_uppercase()
        .bytes()
        .filter(|b| (0x20..0x7F).contains(b) && !b"\"*+,./:;<=>?[\\]|".contains(b))
        .take(11)
        .collect();
    m[..cista.len()].copy_from_slice(&cista);
    if cista.iter().all(|&b| b == b' ') {
        m.copy_from_slice(b"NO NAME    ");
    }
    m
}

impl Fat32 {
    /// Navrhne rozvrh. `klaster` = bajty na klaster (None = automaticky), `zarovnanie` v bajtoch.
    pub fn navrhni(
        sektory: u64,
        sektor: u32,
        skryte: u64,
        zarovnanie: u64,
        klaster: Option<u32>,
        nazov: &str,
    ) -> Result<Fat32, String> {
        if !matches!(sektor, 512 | 1024 | 2048 | 4096) {
            return Err(format!("nepodporovaná veľkosť sektora {sektor} B"));
        }
        let moznosti: Vec<u32> = match klaster {
            Some(k) => {
                if !k.is_power_of_two() || k < sektor || k > 64 * 1024 {
                    return Err(format!("klaster {k} B nie je platný (mocnina 2, {sektor} B až 64 KB)"));
                }
                vec![k]
            }
            None => [32 * 1024, 16 * 1024, 8 * 1024, 4096, 2048, 1024, 512]
                .into_iter()
                .filter(|&k| k >= sektor)
                .collect(),
        };
        let mut posledna_chyba = String::from("zväzok je primalý pre FAT32");
        for k in moznosti {
            match Self::s_klastrom(sektory, sektor, skryte, zarovnanie, k / sektor, nazov) {
                Ok(f) => return Ok(f),
                Err(e) => posledna_chyba = e,
            }
        }
        Err(posledna_chyba)
    }

    fn s_klastrom(
        sektory: u64,
        sektor: u32,
        skryte: u64,
        zarovnanie: u64,
        na_klaster: u32,
        nazov: &str,
    ) -> Result<Fat32, String> {
        let s = sektor as u64;
        let spc = na_klaster as u64;
        let mut rezervovane = 32u64;
        // horný odhad klastrov → dosť veľká FAT
        let klastre_max = sektory.saturating_sub(rezervovane) / spc;
        let mut fat = ((klastre_max + 2) * 4).div_ceil(s);
        // zarovnanie dátovej oblasti (vzhľadom na začiatok disku)
        let z = (zarovnanie / s).max(1);
        let dopln = (z - (skryte + rezervovane + 2 * fat) % z) % z;
        let do_fat = dopln.saturating_sub(65_000).div_ceil(2);
        fat += do_fat;
        rezervovane += dopln - 2 * do_fat;
        let data_od = rezervovane + 2 * fat;
        if data_od >= sektory {
            return Err("zväzok je primalý pre FAT32".into());
        }
        let klastre = (sektory - data_od) / spc;
        if klastre < NAJMENEJ_KLASTROV {
            return Err(format!(
                "zväzok je primalý pre FAT32 s klastrom {} B ({klastre} klastrov, treba aspoň {NAJMENEJ_KLASTROV})",
                spc * s
            ));
        }
        if klastre > NAJVIAC_KLASTROV {
            return Err("zväzok je priveľký pre FAT32 – zvoľte väčší klaster alebo exFAT".into());
        }
        debug_assert!(fat * s / 4 >= klastre + 2);
        let n = jedinecne();
        Ok(Fat32 {
            sektor,
            sektory,
            skryte,
            na_klaster,
            rezervovane: rezervovane as u32,
            fat: fat as u32,
            klastre: klastre as u32,
            seriove: u32::from_le_bytes([n[0], n[1], n[2], n[3]]),
            menovka: menovka(nazov),
        })
    }

    /// Prvý sektor dátovej oblasti (klaster 2) vzhľadom na začiatok zväzku.
    pub fn data_od(&self) -> u64 {
        self.rezervovane as u64 + 2 * self.fat as u64
    }

    pub fn klaster_b(&self) -> u64 {
        self.na_klaster as u64 * self.sektor as u64
    }

    /// Posun klastra `c` (od 2) vzhľadom na začiatok zväzku v bajtoch.
    pub fn klaster_na(&self, c: u32) -> u64 {
        (self.data_od() + (c as u64 - 2) * self.na_klaster as u64) * self.sektor as u64
    }

    /// Zavádzací sektor s BPB.
    pub fn boot(&self) -> Vec<u8> {
        let mut b = vec![0u8; self.sektor as usize];
        b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        b[3..11].copy_from_slice(b"MSWIN4.1");
        b[11..13].copy_from_slice(&(self.sektor as u16).to_le_bytes());
        b[13] = self.na_klaster as u8;
        b[14..16].copy_from_slice(&(self.rezervovane as u16).to_le_bytes());
        b[16] = 2;
        b[21] = 0xF8;
        b[24..26].copy_from_slice(&63u16.to_le_bytes());
        b[26..28].copy_from_slice(&255u16.to_le_bytes());
        b[28..32].copy_from_slice(&(self.skryte.min(u32::MAX as u64) as u32).to_le_bytes());
        b[32..36].copy_from_slice(&(self.sektory.min(u32::MAX as u64) as u32).to_le_bytes());
        b[36..40].copy_from_slice(&self.fat.to_le_bytes());
        b[44..48].copy_from_slice(&2u32.to_le_bytes());
        b[48..50].copy_from_slice(&1u16.to_le_bytes());
        b[50..52].copy_from_slice(&6u16.to_le_bytes());
        b[64] = 0x80;
        b[66] = 0x29;
        b[67..71].copy_from_slice(&self.seriove.to_le_bytes());
        b[71..82].copy_from_slice(&self.menovka);
        b[82..90].copy_from_slice(b"FAT32   ");
        let kod = kod_bez_systemu(90);
        b[90..90 + kod.len()].copy_from_slice(&kod);
        b[510] = 0x55;
        b[511] = 0xAA;
        b
    }

    /// Sektor FSInfo (počet voľných klastrov a nápoveda pre ďalší voľný).
    pub fn fsinfo(&self, volne: u32, dalsi: u32) -> Vec<u8> {
        let mut b = vec![0u8; self.sektor as usize];
        b[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
        b[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
        b[488..492].copy_from_slice(&volne.to_le_bytes());
        b[492..496].copy_from_slice(&dalsi.to_le_bytes());
        b[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
        b
    }

    /// Prázdny zväzok s menovkou: vyhradená oblasť, dve FAT a koreňový priečinok (klaster 2).
    pub fn naformatuj(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<()> {
        self.naplnene(c, posun, &Strom::novy(), &mut |_, _, _| Ok(()), priebeh, zrus).map(|_| ())
    }

    /// Koľko klastrov zaberie strom (priečinky aj súbory).
    pub fn potrebne_klastre(&self, strom: &Strom) -> u64 {
        let k = self.klaster_b();
        let mut n = 0;
        for p in strom.priecinky() {
            n += velkost_priecinka(strom, p).div_ceil(k).max(1);
        }
        for f in strom.subory() {
            n += strom.uzly[f].velkost.div_ceil(k);
        }
        n
    }

    /// Zväzok so súbormi zo stromu. Klastre sa prideľujú súvisle: priečinky, potom súbory, takže každý
    /// súbor leží v jednom kuse (rýchly zápis aj čítanie). `citaj(uzol, posun, buf)` dodá dáta súboru.
    /// Vráti zapísané súbory s odtlačkami (na overenie spätným čítaním).
    pub fn naplnene(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        strom: &Strom,
        citaj: &mut Citaj,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<Vec<Zapisany>> {
        let s = self.sektor as usize;
        let k = self.klaster_b();
        // 1. pridelenie klastrov
        let mut prvy = vec![0u32; strom.uzly.len()];
        let mut pocet = vec![0u32; strom.uzly.len()];
        let mut dalsi = 2u64;
        let priecinky = strom.priecinky();
        let subory = strom.subory();
        for &p in &priecinky {
            let n = velkost_priecinka(strom, p).div_ceil(k).max(1);
            prvy[p] = dalsi as u32;
            pocet[p] = n as u32;
            dalsi += n;
        }
        for &f in &subory {
            let n = strom.uzly[f].velkost.div_ceil(k);
            if n > 0 {
                prvy[f] = dalsi as u32;
                pocet[f] = n as u32;
                dalsi += n;
            }
        }
        if dalsi - 2 > self.klastre as u64 {
            return Err(io::Error::other(format!(
                "súbory sa nezmestia: treba {} MB, zväzok má {} MB",
                (dalsi - 2) * k / 1_000_000,
                self.klastre as u64 * k / 1_000_000
            )));
        }
        let obsadene = (dalsi - 2) as u32;
        // 2. vyhradená oblasť
        let mut vyhradene = vec![0u8; self.rezervovane as usize * s];
        let boot = self.boot();
        let info = self.fsinfo(self.klastre - obsadene, dalsi as u32);
        vyhradene[0..s].copy_from_slice(&boot);
        vyhradene[s..2 * s].copy_from_slice(&info);
        vyhradene[6 * s..7 * s].copy_from_slice(&boot);
        vyhradene[7 * s..8 * s].copy_from_slice(&info);
        ciel::zapis_vsetko(c, posun, &vyhradene)?;
        // 3. tabuľky FAT (vynulované, potom reťaze na začiatku)
        let fat_b = self.fat as u64 * s as u64;
        let data_celkom = strom.velkost_suborov();
        let celkom = 2 * fat_b + data_celkom;
        let mut fat = vec![0u8; ((dalsi as usize) * 4).div_ceil(s) * s];
        let mut poloz =
            |i: u32, v: u32| fat[i as usize * 4..i as usize * 4 + 4].copy_from_slice(&v.to_le_bytes());
        poloz(0, 0x0FFF_FFF8);
        poloz(1, 0x0FFF_FFFF);
        for u in 0..strom.uzly.len() {
            for j in 0..pocet[u] {
                let i = prvy[u] + j;
                poloz(i, if j + 1 == pocet[u] { 0x0FFF_FFFF } else { i + 1 });
            }
        }
        for i in 0..2u64 {
            let od = posun + (self.rezervovane as u64 * s as u64) + i * fat_b;
            ciel::vynuluj(c, od, fat_b, &mut |h, _| priebeh(i * fat_b + h, celkom), zrus)?;
            ciel::zapis_vsetko(c, od, &fat)?;
        }
        // 4. priečinky
        let (datum, cas) = dos_cas(teraz());
        for &p in &priecinky {
            let mut d = Vec::new();
            if p == 0 {
                if &self.menovka != b"NO NAME    " {
                    d.extend(zaznam(&self.menovka, 0x08, 0, 0, datum, cas));
                }
            } else {
                d.extend(zaznam(b".          ", 0x10, prvy[p], 0, datum, cas));
                let rodic = strom.uzly.iter().position(|u| u.deti.contains(&p)).unwrap_or(0);
                d.extend(zaznam(
                    b"..         ",
                    0x10,
                    if rodic == 0 { 0 } else { prvy[rodic] },
                    0,
                    datum,
                    cas,
                ));
            }
            let mut pouzite = BTreeSet::new();
            for &dieta in &strom.uzly[p].deti {
                let u = &strom.uzly[dieta];
                let (kratke, dlhe) = kratke_meno(&u.meno, &mut pouzite);
                if dlhe {
                    for z in dlhe_meno(&u.meno, sucet_mena(&kratke)) {
                        d.extend_from_slice(&z);
                    }
                }
                let attr = if u.priecinok { 0x10 } else { 0x20 };
                let velkost = if u.priecinok { 0 } else { u.velkost.min(u32::MAX as u64) as u32 };
                d.extend(zaznam(&kratke, attr, prvy[dieta], velkost, datum, cas));
            }
            d.resize(pocet[p] as usize * k as usize, 0);
            ciel::zapis_vsetko(c, posun + self.klaster_na(prvy[p]), &d)?;
        }
        // 5. dáta súborov
        let mut zapisane = Vec::new();
        let mut buf = vec![0u8; ciel::KUS];
        let mut hotovo = 0u64;
        for &f in &subory {
            let velkost = strom.uzly[f].velkost;
            let mut h = Hasovac::novy();
            let mut v = 0u64;
            let od = if velkost > 0 { posun + self.klaster_na(prvy[f]) } else { 0 };
            while v < velkost {
                if zrus.load(Ordering::Relaxed) {
                    return Err(io::Error::other("zrušené"));
                }
                let n = (ciel::KUS as u64).min(velkost - v) as usize;
                citaj(f, v, &mut buf[..n])?;
                h.pridaj(&buf[..n]);
                c.zapis(od + v, &buf[..n])?;
                v += n as u64;
                hotovo += n as u64;
                priebeh(2 * fat_b + hotovo, celkom);
            }
            zapisane.push(Zapisany { uzol: f, posun: od, velkost, odtlacok: h.dokonci().0 });
        }
        priebeh(celkom, celkom);
        Ok(zapisane)
    }
}

/// Bajty priečinka (vrátane „.“, „..“, dlhých mien a menovky v koreni).
fn velkost_priecinka(strom: &Strom, p: usize) -> u64 {
    let mut n = if p == 0 { 1 } else { 2 };
    let mut pouzite = BTreeSet::new();
    for &d in &strom.uzly[p].deti {
        let m = &strom.uzly[d].meno;
        let (_, dlhe) = kratke_meno(m, &mut pouzite);
        n += 1 + if dlhe { m.encode_utf16().count().div_ceil(13) as u64 } else { 0 };
    }
    n * 32
}

/// 32-bajtový záznam priečinka.
fn zaznam(meno: &[u8; 11], attr: u8, klaster: u32, velkost: u32, datum: u16, cas: u16) -> [u8; 32] {
    let mut z = [0u8; 32];
    z[0..11].copy_from_slice(meno);
    z[11] = attr;
    z[14..16].copy_from_slice(&cas.to_le_bytes());
    z[16..18].copy_from_slice(&datum.to_le_bytes());
    z[18..20].copy_from_slice(&datum.to_le_bytes());
    z[20..22].copy_from_slice(&((klaster >> 16) as u16).to_le_bytes());
    z[22..24].copy_from_slice(&cas.to_le_bytes());
    z[24..26].copy_from_slice(&datum.to_le_bytes());
    z[26..28].copy_from_slice(&(klaster as u16).to_le_bytes());
    z[28..32].copy_from_slice(&velkost.to_le_bytes());
    z
}

fn povoleny(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit() || b"!#$%&'()-@^_`{}~".contains(&b)
}

/// Krátke meno 8.3 (jedinečné v priečinku) a či treba aj dlhé meno.
pub fn kratke_meno(meno: &str, pouzite: &mut BTreeSet<[u8; 11]>) -> ([u8; 11], bool) {
    let (zaklad, pripona) = match meno.rfind('.') {
        Some(i) if i > 0 => (&meno[..i], &meno[i + 1..]),
        _ => (meno, ""),
    };
    let ocisti = |t: &str| -> Vec<u8> {
        bez_diakritiky(t)
            .to_ascii_uppercase()
            .bytes()
            .filter(|&b| b != b' ' && b != b'.')
            .map(|b| if povoleny(b) { b } else { b'_' })
            .collect()
    };
    let (z, p) = (ocisti(zaklad), ocisti(pripona));
    let presne = !z.is_empty()
        && z.len() <= 8
        && p.len() <= 3
        && meno.bytes().all(|b| povoleny(b) || b == b'.')
        && meno.matches('.').count() <= 1
        && !meno.starts_with('.');
    let mut m = [b' '; 11];
    m[8..8 + p.len().min(3)].copy_from_slice(&p[..p.len().min(3)]);
    if presne {
        m[..z.len()].copy_from_slice(&z);
        if pouzite.insert(m) {
            return (m, false);
        }
    }
    let z = if z.is_empty() { b"_".to_vec() } else { z };
    for n in 1..1_000_000u32 {
        let chvost = format!("~{n}");
        let dlzka = (8 - chvost.len()).min(z.len());
        m[..8].fill(b' ');
        m[..dlzka].copy_from_slice(&z[..dlzka]);
        m[dlzka..dlzka + chvost.len()].copy_from_slice(chvost.as_bytes());
        if pouzite.insert(m) {
            break;
        }
    }
    (m, true)
}

/// Kontrolný súčet krátkeho mena pre záznamy dlhého mena.
pub fn sucet_mena(m: &[u8; 11]) -> u8 {
    m.iter().fold(0u8, |s, &b| s.rotate_right(1).wrapping_add(b))
}

/// Záznamy dlhého mena (VFAT) v poradí na disku: posledná časť prvá.
pub fn dlhe_meno(meno: &str, sucet: u8) -> Vec<[u8; 32]> {
    let mut w: Vec<u16> = meno.encode_utf16().collect();
    let n = w.len().div_ceil(13);
    if !w.len().is_multiple_of(13) {
        w.push(0);
    }
    w.resize(n * 13, 0xFFFF);
    let mut v = Vec::new();
    for i in (0..n).rev() {
        let mut z = [0u8; 32];
        z[0] = (i as u8 + 1) | if i + 1 == n { 0x40 } else { 0 };
        z[11] = 0x0F;
        z[13] = sucet;
        let znaky = &w[i * 13..i * 13 + 13];
        for (j, &ch) in znaky.iter().enumerate() {
            let p = match j {
                0..=4 => 1 + 2 * j,
                5..=10 => 14 + 2 * (j - 5),
                _ => 28 + 2 * (j - 11),
            };
            z[p..p + 2].copy_from_slice(&ch.to_le_bytes());
        }
        v.push(z);
    }
    v
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::ciel::Pamat;

    const MIB: u64 = 1 << 20;

    #[test]
    fn menovka_je_platna() {
        assert_eq!(&menovka("Škola č.1"), b"SKOLA C1   ");
        assert_eq!(&menovka(""), b"NO NAME    ");
        assert_eq!(&menovka("abcdefghijklmno"), b"ABCDEFGHIJK");
    }

    #[test]
    fn rozvrh_ako_sd_karta() {
        // 32 GB karta, oddiel od 4 MiB: klaster 32 KB, dáta zarovnané na 4 MiB od začiatku disku
        let sektory = 62_333_952u64 - 8192;
        let f = Fat32::navrhni(sektory, 512, 8192, 4 * MIB, None, "SKLENIK").unwrap();
        assert_eq!(f.klaster_b(), 32 * 1024);
        assert_eq!((f.skryte + f.data_od()) % 8192, 0, "dátová oblasť na hranici 4 MiB");
        assert!(f.fat as u64 * 512 / 4 >= f.klastre as u64 + 2, "FAT pokryje všetky klastre");
        // malý disk: klaster sa zmenší, aby bolo dosť klastrov
        let f = Fat32::navrhni(200 * MIB / 512, 512, 2048, MIB, None, "").unwrap();
        assert!(f.klastre as u64 >= NAJMENEJ_KLASTROV);
        assert!(f.klaster_b() < 32 * 1024);
        assert!(Fat32::navrhni(20 * MIB / 512, 512, 2048, MIB, None, "").is_err(), "príliš malý");
        // veľké zarovnanie (64 MiB) sa zmestí aj do 16-bitového poľa vyhradených sektorov
        let f = Fat32::navrhni(1_000_000_000_000 / 512, 512, 131_072, 64 * MIB, None, "").unwrap();
        assert!(f.rezervovane <= 65_535);
        assert_eq!((f.skryte + f.data_od()) % 131_072, 0);
        // 4K sektory
        let f = Fat32::navrhni(8_000_000_000 / 4096, 4096, 1024, 4 * MIB, None, "").unwrap();
        assert_eq!(f.sektor, 4096);
        assert_eq!((f.skryte + f.data_od()) % 1024, 0);
    }

    #[test]
    fn kratke_a_dlhe_mena() {
        let mut p = BTreeSet::new();
        assert_eq!(kratke_meno("BOOTMGR", &mut p), (*b"BOOTMGR    ", false));
        assert_eq!(kratke_meno("README.TXT", &mut p), (*b"README  TXT", false));
        assert_eq!(kratke_meno("bootmgr.efi", &mut p), (*b"BOOTMG~1EFI", true));
        assert_eq!(kratke_meno("install.wim", &mut p), (*b"INSTAL~1WIM", true));
        assert_eq!(kratke_meno("install.swm", &mut p), (*b"INSTAL~1SWM", true));
        assert_eq!(kratke_meno("install2.wim", &mut p), (*b"INSTAL~2WIM", true));
        assert_eq!(kratke_meno(".disk", &mut p), (*b"DISK~1     ", true));
        assert_eq!(kratke_meno("čítaj ma.txt", &mut p), (*b"CITAJM~1TXT", true));
        let z = dlhe_meno("bootmgr.efi", 0x5A);
        assert_eq!(z.len(), 1);
        assert_eq!(z[0][0], 0x41);
        assert_eq!(&z[0][1..3], &[b'b', 0]);
        assert_eq!(&z[0][28..32], &[0, 0, 0xFF, 0xFF], "ukončenie nulou, potom 0xFFFF");
        assert_eq!(dlhe_meno("abcdefghijklmnopq", 1).len(), 2);
    }

    #[test]
    fn naformatovany_zvazok_ma_spravne_struktury() {
        let mut p = Pamat::nova(300 * MIB, 512);
        let f = Fat32::navrhni(300 * MIB / 512, 512, 0, MIB, None, "Skúška").unwrap();
        f.naformatuj(&mut p, 0, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
        let d = &p.data;
        assert_eq!(&d[82..90], b"FAT32   ");
        assert_eq!(&d[510..512], &[0x55, 0xAA]);
        assert_eq!(&d[6 * 512..6 * 512 + 512], &d[..512], "záložný zavádzací sektor");
        assert_eq!(&d[512..516], b"RRaA");
        let fat = f.rezervovane as usize * 512;
        assert_eq!(
            &d[fat..fat + 12],
            &[0xF8, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F]
        );
        let fat2 = fat + f.fat as usize * 512;
        assert_eq!(&d[fat..fat + 512], &d[fat2..fat2 + 512], "obe FAT rovnaké");
        let koren = f.klaster_na(2) as usize;
        assert_eq!(&d[koren..koren + 11], b"SKUSKA     ");
        assert_eq!(d[koren + 11], 0x08);
    }
}
