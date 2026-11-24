//! exFAT vlastným kódom podľa špecifikácie, ktorú Microsoft zverejnil v roku 2019 („exFAT file system
//! specification“): hlavná a záložná zavádzacia oblasť s kontrolným súčtom, FAT, bitmapa obsadenia,
//! tabuľka veľkých písmen a koreňový priečinok. Súbory väčšie ako 4 GB (napr. install.wim) sa naň
//! zmestia; zväzok čítajú Windows, Linux, macOS, Android aj fotoaparáty.

use crate::ciel::{self, Ciel};
use crate::oddiely::{dos_cas, jedinecne, kod_bez_systemu, teraz};
use crate::subory::{Citaj, Strom, Zapisany};
use orchidea_jadro::Hasovac;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

/// Rozvrh zväzku exFAT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExFat {
    pub sektor: u32,
    /// Dĺžka zväzku v sektoroch.
    pub sektory: u64,
    /// Začiatok oddielu na disku v sektoroch.
    pub posun_oddielu: u64,
    pub na_klaster: u32,
    pub fat_od: u32,
    pub fat: u32,
    pub halda_od: u32,
    pub klastre: u32,
    pub seriove: u32,
    pub menovka: Vec<u16>,
}

/// Menovka exFAT: najviac 11 znakov UTF-16, bez zakázaných znakov.
pub fn menovka(t: &str) -> Vec<u16> {
    let cista: String = t.chars().filter(|c| !(*c < ' ' || "\"*/:<>?\\|".contains(*c))).collect();
    cista.trim().encode_utf16().take(11).collect()
}

/// Kontrolný súčet podľa špecifikácie (otočenie doprava a pripočítanie bajtu).
fn sucet(sucet: u32, b: u8) -> u32 {
    sucet.rotate_right(1).wrapping_add(b as u32)
}

/// Veľké písmeno podľa tabuľky zväzku (jeden znak BMP na jeden znak BMP, inak bez zmeny).
pub fn velke_pismeno(k: u32) -> u16 {
    let Some(c) = char::from_u32(k) else { return k as u16 };
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(v), None) if (v as u32) < 0x1_0000 => v as u32 as u16,
        _ => k as u16,
    }
}

/// Tabuľka veľkých písmen v komprimovanom tvare (0xFFFF, počet = úsek znakov, ktoré sa nemenia).
pub fn tabulka_velkych() -> Vec<u8> {
    let velke = velke_pismeno;
    let mut slova: Vec<u16> = Vec::new();
    let mut k = 0u32;
    while k < 0x1_0000 {
        let v = velke(k);
        if v == k as u16 {
            let zac = k;
            while k < 0x1_0000 && velke(k) == k as u16 {
                k += 1;
            }
            let dlzka = k - zac;
            if dlzka > 2 {
                slova.extend([0xFFFF, dlzka as u16]);
            } else {
                slova.extend((zac..k).map(|x| x as u16));
            }
        } else {
            slova.push(v);
            k += 1;
        }
    }
    slova.iter().flat_map(|w| w.to_le_bytes()).collect()
}

impl ExFat {
    /// Navrhne rozvrh. `klaster` v bajtoch (None = podľa veľkosti), `zarovnanie` v bajtoch.
    pub fn navrhni(
        sektory: u64,
        sektor: u32,
        posun_oddielu: u64,
        zarovnanie: u64,
        klaster: Option<u32>,
        nazov: &str,
    ) -> Result<ExFat, String> {
        if !matches!(sektor, 512 | 1024 | 2048 | 4096) {
            return Err(format!("nepodporovaná veľkosť sektora {sektor} B"));
        }
        let s = sektor as u64;
        let velkost = sektory * s;
        if velkost < 2 << 20 {
            return Err("zväzok je primalý pre exFAT".into());
        }
        let k = match klaster {
            Some(k) if k.is_power_of_two() && k >= sektor && k <= 32 << 20 => k as u64,
            Some(k) => return Err(format!("klaster {k} B nie je platný")),
            None if velkost < 256 << 20 => 4096,
            None if velkost <= 32 << 30 => 32 << 10,
            None => 128 << 10,
        }
        .max(s);
        let spc = k / s;
        let z = (zarovnanie / s).max(1);
        // FAT začína na zarovnanej hranici (aspoň za 24 sektormi zavádzacích oblastí)
        let zarovnaj = |x: u64| x.div_ceil(z) * z;
        let fat_od = zarovnaj(posun_oddielu + 24) - posun_oddielu;
        let klastre_max = (sektory - fat_od) / spc;
        let fat = ((klastre_max + 2) * 4).div_ceil(s);
        let halda_od = zarovnaj(posun_oddielu + fat_od + fat) - posun_oddielu;
        if halda_od >= sektory {
            return Err("zväzok je primalý pre exFAT s týmto zarovnaním".into());
        }
        let klastre = ((sektory - halda_od) / spc).min(0xFFFF_FFF5);
        if klastre < 3 || fat_od > u32::MAX as u64 || halda_od > u32::MAX as u64 {
            return Err("zväzok je primalý alebo priveľký pre exFAT".into());
        }
        let n = jedinecne();
        Ok(ExFat {
            sektor,
            sektory,
            posun_oddielu,
            na_klaster: spc as u32,
            fat_od: fat_od as u32,
            fat: fat as u32,
            halda_od: halda_od as u32,
            klastre: klastre as u32,
            seriove: u32::from_le_bytes([n[4], n[5], n[6], n[7]]),
            menovka: menovka(nazov),
        })
    }

    pub fn klaster_b(&self) -> u64 {
        self.na_klaster as u64 * self.sektor as u64
    }

    /// Posun klastra (od 2) vzhľadom na začiatok zväzku v bajtoch.
    pub fn klaster_na(&self, c: u32) -> u64 {
        (self.halda_od as u64 + (c as u64 - 2) * self.na_klaster as u64) * self.sektor as u64
    }

    fn klastrov_na(&self, bajty: u64) -> u32 {
        bajty.div_ceil(self.klaster_b()).max(1) as u32
    }

    /// Zavádzacia oblasť (12 sektorov): zavádzací sektor, 8 rozšírených, OEM, rezerva, kontrolný súčet.
    pub fn zavadzacia_oblast(&self, percent: u8) -> Vec<u8> {
        let s = self.sektor as usize;
        let mut o = vec![0u8; 12 * s];
        let b = &mut o[..s];
        b[0..3].copy_from_slice(&[0xEB, 0x76, 0x90]);
        b[3..11].copy_from_slice(b"EXFAT   ");
        b[64..72].copy_from_slice(&self.posun_oddielu.to_le_bytes());
        b[72..80].copy_from_slice(&self.sektory.to_le_bytes());
        b[80..84].copy_from_slice(&self.fat_od.to_le_bytes());
        b[84..88].copy_from_slice(&self.fat.to_le_bytes());
        b[88..92].copy_from_slice(&self.halda_od.to_le_bytes());
        b[92..96].copy_from_slice(&self.klastre.to_le_bytes());
        b[96..100].copy_from_slice(&self.koren().to_le_bytes());
        b[100..104].copy_from_slice(&self.seriove.to_le_bytes());
        b[104..106].copy_from_slice(&0x0100u16.to_le_bytes());
        b[108] = self.sektor.trailing_zeros() as u8;
        b[109] = self.na_klaster.trailing_zeros() as u8;
        b[110] = 1;
        b[111] = 0x80;
        b[112] = percent;
        let kod = kod_bez_systemu(120);
        b[120..120 + kod.len()].copy_from_slice(&kod);
        b[510] = 0x55;
        b[511] = 0xAA;
        for i in 1..=8 {
            o[(i + 1) * s - 4..(i + 1) * s].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
        }
        let mut k = 0u32;
        for (i, &x) in o[..11 * s].iter().enumerate() {
            if i != 106 && i != 107 && i != 112 {
                k = sucet(k, x);
            }
        }
        for ch in o[11 * s..].chunks_mut(4) {
            ch.copy_from_slice(&k.to_le_bytes());
        }
        o
    }

    /// Klastre: bitmapa od 2, za ňou tabuľka veľkých písmen, potom koreňový priečinok.
    fn bitmapa_b(&self) -> u64 {
        (self.klastre as u64).div_ceil(8)
    }

    fn tabulka_od(&self) -> u32 {
        2 + self.klastrov_na(self.bitmapa_b())
    }

    pub fn koren(&self) -> u32 {
        self.tabulka_od() + self.klastrov_na(tabulka_velkych().len() as u64)
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

    /// Zväzok so súbormi zo stromu: priečinky a súbory ležia súvisle (bez reťazí FAT, príznak
    /// NoFatChain), reťaz vo FAT majú len bitmapa, tabuľka veľkých písmen a koreň.
    pub fn naplnene(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        strom: &Strom,
        citaj: &mut Citaj,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<Vec<Zapisany>> {
        let s = self.sektor as u64;
        let kb = self.klaster_b();
        let tabulka = tabulka_velkych();
        let bitmapa = self.klastrov_na(self.bitmapa_b());
        let koren = self.koren();
        // 1. pridelenie klastrov
        let mut prvy = vec![0u32; strom.uzly.len()];
        let mut pocet = vec![0u64; strom.uzly.len()];
        let priecinky = strom.priecinky();
        let subory = strom.subory();
        let bajty_priecinka = |p: usize| -> u64 {
            let vlastne = if p == 0 { 3 } else { 0 };
            (vlastne + strom.uzly[p].deti.iter().map(|&d| zaznamov_sady(&strom.uzly[d].meno)).sum::<u64>())
                * 32
        };
        let mut dalsi = koren as u64;
        for &p in &priecinky {
            let n = bajty_priecinka(p).div_ceil(kb).max(1);
            prvy[p] = dalsi as u32;
            pocet[p] = n;
            dalsi += n;
        }
        for &f in &subory {
            let n = strom.uzly[f].velkost.div_ceil(kb);
            if n > 0 {
                prvy[f] = dalsi as u32;
                pocet[f] = n;
                dalsi += n;
            }
        }
        if dalsi - 2 > self.klastre as u64 {
            return Err(io::Error::other(format!(
                "súbory sa nezmestia: treba {} MB, zväzok má {} MB",
                (dalsi - 2) * kb / 1_000_000,
                self.klastre as u64 * kb / 1_000_000
            )));
        }
        let obsadene = dalsi - 2;
        let percent = (obsadene * 100 / self.klastre as u64) as u8;
        // 2. zavádzacie oblasti
        let mut zac = self.zavadzacia_oblast(percent);
        zac.extend(self.zavadzacia_oblast(percent));
        zac.resize(self.fat_od as usize * s as usize, 0);
        ciel::zapis_vsetko(c, posun, &zac)?;
        // 3. FAT: médium, koniec a reťaze bitmapy, tabuľky a koreňa
        let fat_b = self.fat as u64 * s;
        let data_celkom = strom.velkost_suborov();
        let celkom = fat_b + data_celkom;
        ciel::vynuluj(
            c,
            posun + self.fat_od as u64 * s,
            (self.halda_od - self.fat_od) as u64 * s,
            &mut |h, _| priebeh(h.min(fat_b), celkom),
            zrus,
        )?;
        let koren_n = pocet[0] as u32;
        let mut fat = vec![0u8; ((koren + koren_n) as usize * 4).div_ceil(s as usize) * s as usize];
        let mut poloz =
            |i: u32, v: u32| fat[i as usize * 4..i as usize * 4 + 4].copy_from_slice(&v.to_le_bytes());
        poloz(0, 0xFFFF_FFF8);
        poloz(1, 0xFFFF_FFFF);
        for (od, n) in [(2, bitmapa), (self.tabulka_od(), koren - self.tabulka_od()), (koren, koren_n)] {
            for i in od..od + n {
                poloz(i, if i + 1 == od + n { 0xFFFF_FFFF } else { i + 1 });
            }
        }
        ciel::zapis_vsetko(c, posun + self.fat_od as u64 * s, &fat)?;
        // 4. bitmapa obsadenia
        let mut bm = vec![0u8; bitmapa as usize * kb as usize];
        for i in 0..obsadene as usize {
            bm[i / 8] |= 1 << (i % 8);
        }
        ciel::zapis_vsetko(c, posun + self.klaster_na(2), &bm)?;
        // 5. tabuľka veľkých písmen
        let mut t = tabulka.clone();
        t.resize((koren - self.tabulka_od()) as usize * kb as usize, 0);
        ciel::zapis_vsetko(c, posun + self.klaster_na(self.tabulka_od()), &t)?;
        // 6. priečinky
        let (datum, cas) = dos_cas(teraz());
        let pecat = (datum as u32) << 16 | cas as u32;
        for &p in &priecinky {
            let mut d = Vec::new();
            if p == 0 {
                let mut z = [0u8; 32];
                z[0] = 0x83;
                z[1] = self.menovka.len() as u8;
                for (i, w) in self.menovka.iter().enumerate() {
                    z[2 + 2 * i..4 + 2 * i].copy_from_slice(&w.to_le_bytes());
                }
                d.extend_from_slice(&z);
                let mut b = [0u8; 32];
                b[0] = 0x81;
                b[20..24].copy_from_slice(&2u32.to_le_bytes());
                b[24..32].copy_from_slice(&self.bitmapa_b().to_le_bytes());
                d.extend_from_slice(&b);
                let mut u = [0u8; 32];
                u[0] = 0x82;
                let suc = tabulka.iter().fold(0u32, |a, &x| sucet(a, x));
                u[4..8].copy_from_slice(&suc.to_le_bytes());
                u[20..24].copy_from_slice(&self.tabulka_od().to_le_bytes());
                u[24..32].copy_from_slice(&(tabulka.len() as u64).to_le_bytes());
                d.extend_from_slice(&u);
            }
            for &dieta in &strom.uzly[p].deti {
                let u = &strom.uzly[dieta];
                let dlzka = if u.priecinok { pocet[dieta] * kb } else { u.velkost };
                d.extend(sada(&u.meno, u.priecinok, prvy[dieta], dlzka, pecat));
            }
            d.resize(pocet[p] as usize * kb as usize, 0);
            ciel::zapis_vsetko(c, posun + self.klaster_na(prvy[p]), &d)?;
        }
        // 7. dáta súborov
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
                priebeh(fat_b + hotovo, celkom);
            }
            zapisane.push(Zapisany { uzol: f, posun: od, velkost, odtlacok: h.dokonci().0 });
        }
        priebeh(celkom, celkom);
        Ok(zapisane)
    }
}

/// Počet 32-bajtových záznamov sady súboru (súbor + rozšírenie prúdu + mená po 15 znakoch).
fn zaznamov_sady(meno: &str) -> u64 {
    2 + meno.encode_utf16().count().div_ceil(15).max(1) as u64
}

/// Odtlačok mena (NameHash) z veľkých písmen podľa tabuľky zväzku.
pub fn odtlacok_mena(meno: &str) -> u16 {
    let mut h = 0u16;
    for w in meno.encode_utf16() {
        for b in velke_pismeno(w as u32).to_le_bytes() {
            h = h.rotate_right(1).wrapping_add(b as u16);
        }
    }
    h
}

/// Sada záznamov priečinka pre jeden súbor alebo priečinok.
fn sada(meno: &str, priecinok: bool, prvy: u32, dlzka: u64, pecat: u32) -> Vec<u8> {
    let w: Vec<u16> = meno.encode_utf16().take(255).collect();
    let mena = w.len().div_ceil(15).max(1);
    let mut v = vec![0u8; 32 * (2 + mena)];
    v[0] = 0x85;
    v[1] = (1 + mena) as u8;
    v[4..6].copy_from_slice(&(if priecinok { 0x10u16 } else { 0x20 }).to_le_bytes());
    for o in [8, 12, 16] {
        v[o..o + 4].copy_from_slice(&pecat.to_le_bytes());
    }
    let p = &mut v[32..64];
    p[0] = 0xC0;
    p[1] = if prvy == 0 { 0x01 } else { 0x03 }; // AllocationPossible, NoFatChain
    p[3] = w.len() as u8;
    p[4..6].copy_from_slice(&odtlacok_mena(meno).to_le_bytes());
    p[8..16].copy_from_slice(&dlzka.to_le_bytes());
    p[20..24].copy_from_slice(&prvy.to_le_bytes());
    p[24..32].copy_from_slice(&dlzka.to_le_bytes());
    for (i, kus) in w.chunks(15).enumerate() {
        let n = &mut v[64 + 32 * i..96 + 32 * i];
        n[0] = 0xC1;
        for (j, ch) in kus.iter().enumerate() {
            n[2 + 2 * j..4 + 2 * j].copy_from_slice(&ch.to_le_bytes());
        }
    }
    let mut k = 0u16;
    for (i, &b) in v.iter().enumerate() {
        if i != 2 && i != 3 {
            k = k.rotate_right(1).wrapping_add(b as u16);
        }
    }
    v[2..4].copy_from_slice(&k.to_le_bytes());
    v
}

impl crate::subory::Zvazok for ExFat {
    fn klaster_b(&self) -> u64 {
        ExFat::klaster_b(self)
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
        ExFat::naplnene(self, c, posun, strom, citaj, priebeh, zrus)
    }
    fn over(&self, zaciatok: &[u8]) -> bool {
        zaciatok.len() >= 11 && &zaciatok[3..11] == b"EXFAT   "
    }
    fn zmesti_sa(&self, strom: &Strom) -> bool {
        let k = self.klaster_b();
        let subory: u64 = strom.subory().iter().map(|&u| strom.uzly[u].velkost.div_ceil(k)).sum();
        let priecinky = strom.priecinky().len() as u64 * 2;
        (self.koren() as u64 - 2) + subory + priecinky + 16 <= self.klastre as u64
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::ciel::Pamat;

    const MIB: u64 = 1 << 20;

    #[test]
    fn tabulka_velkych_pismen() {
        let t = tabulka_velkych();
        // rozbalená tabuľka musí mať 65 536 položiek
        let slova: Vec<u16> = t.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let mut plna = Vec::new();
        let mut i = 0;
        while i < slova.len() {
            if slova[i] == 0xFFFF {
                let n = slova[i + 1];
                let zac = plna.len() as u16;
                plna.extend((0..n).map(|j| zac + j));
                i += 2;
            } else {
                plna.push(slova[i]);
                i += 1;
            }
        }
        assert_eq!(plna.len(), 0x1_0000);
        assert_eq!(plna['a' as usize], 'A' as u16);
        assert_eq!(plna['ž' as usize], 'Ž' as u16);
        assert_eq!(plna['Z' as usize], 'Z' as u16);
        assert!(t.len() < 8000, "komprimovaná tabuľka je malá ({} B)", t.len());
    }

    #[test]
    fn rozvrh_a_zavadzacia_oblast() {
        let f = ExFat::navrhni(128_000_000_000 / 512 - 32_768, 512, 32_768, 16 * MIB, None, "Karta").unwrap();
        assert_eq!(f.klaster_b(), 128 * 1024);
        assert_eq!((f.posun_oddielu + f.halda_od as u64) % 32_768, 0, "halda klastrov zarovnaná");
        assert_eq!((f.posun_oddielu + f.fat_od as u64) % 32_768, 0, "FAT zarovnaná");
        assert!(f.fat as u64 * 512 / 4 >= f.klastre as u64 + 2);
        let o = f.zavadzacia_oblast(0);
        assert_eq!(&o[3..11], b"EXFAT   ");
        assert_eq!(&o[1020..1024], &[0, 0, 0x55, 0xAA], "rozšírený zavádzací sektor");
        let k = u32::from_le_bytes(o[11 * 512..11 * 512 + 4].try_into().unwrap());
        assert!(o[11 * 512..].chunks(4).all(|c| c == k.to_le_bytes()));
        // príznaky zväzku a percento sa do súčtu nerátajú
        let o2 = f.zavadzacia_oblast(57);
        assert_eq!(o[11 * 512..], o2[11 * 512..]);
        assert_eq!(menovka("Škola: 1/2"), "Škola 12".encode_utf16().collect::<Vec<_>>());
    }

    #[test]
    fn naformatovany_zvazok() {
        let mut p = Pamat::nova(64 * MIB, 512);
        let f = ExFat::navrhni(64 * MIB / 512, 512, 0, MIB, None, "ORCHIDEA").unwrap();
        f.naformatuj(&mut p, 0, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
        let d = &p.data;
        assert_eq!(&d[..512 * 12], &d[512 * 12..512 * 24], "záložná oblasť");
        let fat = f.fat_od as usize * 512;
        assert_eq!(&d[fat..fat + 8], &[0xF8, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        let k = f.klaster_na(f.koren()) as usize;
        assert_eq!(d[k], 0x83);
        assert_eq!(d[k + 32], 0x81);
        assert_eq!(d[k + 64], 0x82);
        let bm = f.klaster_na(2) as usize;
        let obsadene = f.koren() - 1;
        assert_eq!(d[bm].count_ones(), obsadene.min(8));
    }
}
