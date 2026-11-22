//! F2FS (Flash-Friendly File System) vlastným kódom – rozvrh ako mkfs.f2fs 1.16: superblok (dva
//! razy), dve balíky kontrolného bodu so súhrnmi a bitmi NAT, oblasti SIT, NAT a SSA, hlavná oblasť so
//! segmentmi po 2 MB a koreňový priečinok (inód v horúcom segmente uzlov, záznamy „.“ a „..“ v horúcom
//! segmente dát). Prerozdelenie (overprovision) a rezerva pre upratovanie sa počítajú rovnako ako
//! v mkfs.f2fs, aby sa zväzok správal ako naformátovaný v Linuxe.

use crate::ciel::{self, Ciel};
use crate::oddiely::{jedinecne, teraz};
use std::io;
use std::sync::atomic::AtomicBool;

const BLOK: u64 = 4096;
const BLOKOV_V_SEGMENTE: u64 = 512;
const SEGMENT: u64 = BLOK * BLOKOV_V_SEGMENTE;
const MAGIA: u32 = 0xF2F5_2010;
const SIT_NA_BLOK: u64 = 55;
const NAT_NA_BLOK: u64 = 455;
/// Kontrolný súčet kontrolného bodu leží na konci bloku.
const CRC_POSUN: usize = 4092;
/// Veľkosť štruktúry kontrolného bodu bez bitmáp (193 B).
const VELKOST_CP: u64 = 193;
const MAX_SIT_BITMAPA: u64 = 19_072;
const KOREN: u32 = 3;
/// Druhy aktívnych segmentov v poradí kontrolného bodu: horúce, teplé, studené uzly; horúce, teplé,
/// studené dáta (typy v SIT: dáta 0–2, uzly 3–5).
const HORUCE_UZLY: usize = 0;
const TEPLE_UZLY: usize = 1;
const STUDENE_UZLY: usize = 2;
const HORUCE_DATA: usize = 3;
const TEPLE_DATA: usize = 4;
const STUDENE_DATA: usize = 5;

/// Navrhnutý zväzok F2FS.
#[derive(Debug, Clone)]
pub struct F2fs {
    log_sektora: u32,
    blokov: u64,
    seg0: u64,
    segmentov: u64,
    sit: u64,
    nat: u64,
    ssa: u64,
    hlavnych: u64,
    cp_navyse: u64,
    rezerva: u32,
    prerozdelenie: u32,
    aktivne: [u64; 6],
    uuid: [u8; 16],
    menovka: Vec<u16>,
}

/// CRC-32 (polynóm 0xEDB88320) bez záverečnej negácie, ako f2fs_cal_crc32.
fn crc32(mut c: u32, d: &[u8]) -> u32 {
    for &b in d {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    c
}

fn le16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn le32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn le64(d: &mut [u8], o: usize, v: u64) {
    d[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// Najlepšie prerozdelenie v percentách – rovnaký výpočet (aj s desatinnými číslami) ako mkfs.f2fs.
fn najlepsie_prerozdelenie(hlavnych: u64) -> f64 {
    let (mut kandidat, koniec, krok) = if hlavnych < 256 { (10.0, 95.0, 5.0) } else { (0.01, 10.0, 0.01) };
    let h = hlavnych as f64;
    let (mut najlepsie, mut najviac) = (0.0, 0.0);
    while kandidat <= koniec {
        let rezerva = 100.0 / kandidat + 1.0 + 6.0;
        let ovp = (h - rezerva) * kandidat / 100.0;
        if ovp >= 0.0 {
            let miesto = h - f64::max(rezerva, ovp) - 2.0;
            if najviac < miesto {
                najviac = miesto;
                najlepsie = kandidat;
            }
        }
        kandidat += krok;
    }
    najlepsie
}

impl F2fs {
    /// Navrhne F2FS pre oddiel začínajúci sektorom `zaciatok` (segmenty sa zarovnajú na 2 MB disku).
    pub fn navrhni(sektory: u64, sektor: u32, zaciatok: u64, nazov: &str) -> Result<F2fs, String> {
        if !sektor.is_power_of_two() || !(512..=4096).contains(&sektor) {
            return Err(format!("F2FS: nepodporovaná veľkosť sektora {sektor} B"));
        }
        let log_sektora = sektor.trailing_zeros();
        let bajtov = sektory * sektor as u64;
        let zac_b = zaciatok * sektor as u64;
        let zarovnanie = (zac_b + 2 * BLOK).div_ceil(SEGMENT) * SEGMENT - zac_b;
        if bajtov < zarovnanie + 40 * SEGMENT {
            return Err("zväzok je primalý pre F2FS (aspoň asi 100 MB)".into());
        }
        let segmentov = (bajtov - zarovnanie) / SEGMENT;
        if segmentov * BLOKOV_V_SEGMENTE > u32::MAX as u64 {
            return Err("zväzok je priveľký pre F2FS (nad 16 TB)".into());
        }
        let seg0 = zarovnanie / BLOK;
        let sit = 2 * segmentov.div_ceil(SIT_NA_BLOK).div_ceil(BLOKOV_V_SEGMENTE);
        let nat_blokov = ((segmentov - 2 - sit) * BLOKOV_V_SEGMENTE).div_ceil(NAT_NA_BLOK);
        let mut nat = nat_blokov.div_ceil(BLOKOV_V_SEGMENTE);
        let sit_bitmapa = ((sit / 2) * BLOKOV_V_SEGMENTE / 8).min(MAX_SIT_BITMAPA);
        let volne_v_cp = CRC_POSUN as u64 - VELKOST_CP + 1;
        let (nat_bitmapa, cp_navyse) = if sit_bitmapa > volne_v_cp - 64 {
            (volne_v_cp, sit_bitmapa.div_ceil(BLOK))
        } else {
            (volne_v_cp - sit_bitmapa, 0)
        };
        nat = nat.min(nat_bitmapa * 8 / BLOKOV_V_SEGMENTE);
        let nat = 2 * nat;
        let ssa = (segmentov - 2 - sit - nat + 1).div_ceil(BLOKOV_V_SEGMENTE);
        let meta = 2 + sit + nat + ssa;
        if segmentov <= meta + 6 {
            return Err("zväzok je primalý pre F2FS".into());
        }
        let hlavnych = segmentov - meta;
        let ovp = najlepsie_prerozdelenie(hlavnych);
        if ovp == 0.0 {
            return Err("zväzok je primalý pre F2FS".into());
        }
        let rezerva = (100.0 / ovp + 1.0 + 6.0) as u32;
        if hlavnych - 2 < rezerva as u64 {
            return Err("zväzok je primalý pre F2FS".into());
        }
        let prerozdelenie = (((hlavnych - rezerva as u64) as f64 * ovp / 100.0) as u32).max(rezerva) + 2;
        // aktívne segmenty (bez haldy, ako mkfs.f2fs): uzly 0–2, horúce dáta 3, studené po štvrtine,
        // teplé v polovici hlavnej oblasti
        let studene = (hlavnych / 4).saturating_sub(1).max(3);
        let teple = (hlavnych / 2).saturating_sub(1).max(studene + 1);
        let aktivne = [0, 1, 2, 3, teple, studene];
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&jedinecne()[..16]);
        uuid[6] = (uuid[6] & 0x0F) | 0x40;
        uuid[8] = (uuid[8] & 0x3F) | 0x80;
        let menovka: Vec<u16> = nazov.trim().encode_utf16().take(512).collect();
        Ok(F2fs {
            log_sektora,
            blokov: bajtov / BLOK,
            seg0,
            segmentov,
            sit,
            nat,
            ssa,
            hlavnych,
            cp_navyse,
            rezerva,
            prerozdelenie,
            aktivne,
            uuid,
            menovka,
        })
    }

    fn sit_blok(&self) -> u64 {
        self.seg0 + 2 * BLOKOV_V_SEGMENTE
    }
    fn nat_blok(&self) -> u64 {
        self.sit_blok() + self.sit * BLOKOV_V_SEGMENTE
    }
    fn ssa_blok(&self) -> u64 {
        self.nat_blok() + self.nat * BLOKOV_V_SEGMENTE
    }
    fn hlavny_blok(&self) -> u64 {
        self.ssa_blok() + self.ssa * BLOKOV_V_SEGMENTE
    }
    /// Prvý blok aktívneho segmentu.
    fn aktivny(&self, i: usize) -> u64 {
        self.hlavny_blok() + self.aktivne[i] * BLOKOV_V_SEGMENTE
    }

    fn superblok(&self) -> Vec<u8> {
        let mut b = vec![0u8; BLOK as usize];
        let s = &mut b[1024..];
        le32(s, 0, MAGIA);
        le16(s, 4, 1);
        le16(s, 6, 16);
        le32(s, 8, self.log_sektora);
        le32(s, 12, 12 - self.log_sektora);
        le32(s, 16, 12);
        le32(s, 20, 9);
        le32(s, 24, 1);
        le32(s, 28, 1);
        le64(s, 36, self.blokov);
        le32(s, 44, self.hlavnych as u32);
        le32(s, 48, self.segmentov as u32);
        le32(s, 52, 2);
        le32(s, 56, self.sit as u32);
        le32(s, 60, self.nat as u32);
        le32(s, 64, self.ssa as u32);
        le32(s, 68, self.hlavnych as u32);
        le32(s, 72, self.seg0 as u32);
        le32(s, 76, self.seg0 as u32);
        le32(s, 80, self.sit_blok() as u32);
        le32(s, 84, self.nat_blok() as u32);
        le32(s, 88, self.ssa_blok() as u32);
        le32(s, 92, self.hlavny_blok() as u32);
        le32(s, 96, KOREN);
        le32(s, 100, 1);
        le32(s, 104, 2);
        s[108..124].copy_from_slice(&self.uuid);
        for (i, c) in self.menovka.iter().enumerate() {
            le16(s, 124 + 2 * i, *c);
        }
        le32(s, 1664, self.cp_navyse as u32);
        let verzia = b"SADIC (vlastny kod)";
        s[1668..1668 + verzia.len()].copy_from_slice(verzia);
        s[1924..1924 + verzia.len()].copy_from_slice(verzia);
        b
    }

    /// Kontrolný bod (blok) s danou verziou a CRC.
    fn kontrolny_bod(&self, verzia: u64, priznaky: u32) -> Vec<u8> {
        let mut cp = vec![0u8; BLOK as usize];
        let volnych = self.hlavnych - 6;
        le64(&mut cp, 0, verzia);
        le64(&mut cp, 8, (volnych + 6 - self.prerozdelenie as u64) * BLOKOV_V_SEGMENTE);
        le64(&mut cp, 16, 2);
        le32(&mut cp, 24, self.rezerva);
        le32(&mut cp, 28, self.prerozdelenie);
        le32(&mut cp, 32, volnych as u32);
        for i in 0..8 {
            let uzol = if i < 3 { self.aktivne[i] as u32 } else { u32::MAX };
            let data = if i < 3 { self.aktivne[3 + i] as u32 } else { u32::MAX };
            le32(&mut cp, 36 + 4 * i, uzol);
            le32(&mut cp, 84 + 4 * i, data);
        }
        le16(&mut cp, 68, 1); // horúce uzly: obsadený 1 blok (koreňový inód)
        le16(&mut cp, 116, 1); // horúce dáta: 1 blok (záznamy koreňa)
        le32(&mut cp, 132, priznaky);
        le32(&mut cp, 136, 6 + self.cp_navyse as u32);
        le32(&mut cp, 140, 1 + self.cp_navyse as u32);
        le32(&mut cp, 144, 1);
        le32(&mut cp, 148, 1);
        le32(&mut cp, 152, KOREN + 1);
        le32(&mut cp, 156, ((self.sit / 2) * BLOKOV_V_SEGMENTE / 8) as u32);
        le32(&mut cp, 160, ((self.nat / 2) * BLOKOV_V_SEGMENTE / 8) as u32);
        le32(&mut cp, 164, CRC_POSUN as u32);
        let c = crc32(MAGIA, &cp[..CRC_POSUN]);
        le32(&mut cp, CRC_POSUN, c);
        cp
    }

    /// Bloky bitov NAT (na konci prvého segmentu kontrolného bodu).
    fn blokov_bitov_nat(&self) -> u64 {
        (self.nat * 32 * 2 + 8).div_ceil(BLOK)
    }

    /// Zhustený súhrn dát: denník NAT (koreň), denník SIT (6 aktívnych segmentov), súhrn horúcich dát.
    fn zhusteny_suhrn(&self) -> Vec<u8> {
        let mut s = vec![0u8; BLOK as usize];
        le16(&mut s, 0, 1);
        le32(&mut s, 2, KOREN);
        le32(&mut s, 7, KOREN);
        le32(&mut s, 11, self.aktivny(HORUCE_UZLY) as u32);
        le16(&mut s, 507, 6);
        // poradie v denníku: uzly horúce, teplé, studené, dáta horúce, teplé, studené
        let typy = [(HORUCE_UZLY, 3u16, true), (TEPLE_UZLY, 4, false), (STUDENE_UZLY, 5, false)];
        let data = [(HORUCE_DATA, 0u16, true), (TEPLE_DATA, 1, false), (STUDENE_DATA, 2, false)];
        for (k, (i, typ, obsadeny)) in typy.iter().chain(data.iter()).enumerate() {
            let o = 509 + 78 * k;
            le32(&mut s, o, self.aktivne[*i] as u32);
            le16(&mut s, o + 4, (typ << 10) | u16::from(*obsadeny));
            if *obsadeny {
                s[o + 6] = 0x80; // blok 0 platný (bity od najvyššieho)
            }
        }
        le32(&mut s, 1014, KOREN); // súhrn horúcich dát: blok patrí koreňu, posun 0
        s
    }

    fn suhrn_uzlov(nid: Option<u32>) -> Vec<u8> {
        let mut s = vec![0u8; BLOK as usize];
        if let Some(n) = nid {
            le32(&mut s, 0, n);
        }
        s[4091] = 1; // typ súhrnu: uzly
        s
    }

    fn korenovy_inod(&self, cas: u64) -> Vec<u8> {
        let mut n = vec![0u8; BLOK as usize];
        le16(&mut n, 0, 0x41ED);
        le32(&mut n, 12, 2);
        le64(&mut n, 16, BLOK);
        le64(&mut n, 24, 2);
        for o in [32, 40, 48] {
            le64(&mut n, o, cas);
        }
        le32(&mut n, 72, 1); // hĺbka priečinka
        le32(&mut n, 360, self.aktivny(HORUCE_DATA) as u32);
        // pätka uzla
        le32(&mut n, 4072, KOREN);
        le32(&mut n, 4076, KOREN);
        le64(&mut n, 4084, 1);
        le32(&mut n, 4092, self.aktivny(HORUCE_UZLY) as u32 + 1);
        n
    }

    fn zaznamy_korena() -> Vec<u8> {
        let mut d = vec![0u8; BLOK as usize];
        d[0] = 0x03;
        for (i, dlzka) in [(0usize, 1u16), (1, 2)] {
            let o = 27 + 3 + 11 * i;
            le32(&mut d, o + 4, KOREN);
            le16(&mut d, o + 8, dlzka);
            d[o + 10] = 2; // priečinok
        }
        d[2384] = b'.';
        d[2392..2394].copy_from_slice(b"..");
        d
    }

    /// Zapíše prázdny zväzok od bajtu `posun`.
    pub fn naformatuj(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<()> {
        let at = |blok: u64| posun + blok * BLOK;
        let celkom = (2 + self.sit / 2 + self.nat / 2) * SEGMENT;
        let mut hotovo = 0u64;
        // vynulovanie: kontrolné body, prvá sada SIT, prvá sada NAT (každý druhý segment)
        let mut nuluj = |c: &mut dyn Ciel, od: u64, segmentov: u64, hotovo: &mut u64| -> io::Result<()> {
            let h0 = *hotovo;
            ciel::vynuluj(c, at(od), segmentov * SEGMENT, &mut |h, _| priebeh(h0 + h, celkom), zrus)?;
            *hotovo += segmentov * SEGMENT;
            Ok(())
        };
        nuluj(c, self.seg0, 2, &mut hotovo)?;
        nuluj(c, self.sit_blok(), self.sit / 2, &mut hotovo)?;
        for i in 0..self.nat / 2 {
            nuluj(c, self.nat_blok() + 2 * i * BLOKOV_V_SEGMENTE, 1, &mut hotovo)?;
        }
        // NAT: uzly 1 (uzly), 2 (meta) a koreň
        let mut nat = vec![0u8; BLOK as usize];
        for (ino, blok) in [(1u32, 1u32), (2, 1), (KOREN, self.aktivny(HORUCE_UZLY) as u32)] {
            let o = 9 * ino as usize;
            le32(&mut nat, o + 1, ino);
            le32(&mut nat, o + 5, blok);
        }
        c.zapis(at(self.nat_blok()), &nat)?;
        // koreň: inód a záznamy; začiatky ostatných aktívnych segmentov uzlov vynulované (aby sa po
        // pripojení neobnovovali staré uzly)
        let cas = teraz();
        c.zapis(at(self.aktivny(HORUCE_UZLY)), &self.korenovy_inod(cas))?;
        c.zapis(at(self.aktivny(HORUCE_DATA)), &F2fs::zaznamy_korena())?;
        let nuly = vec![0u8; 2 * BLOK as usize];
        c.zapis(at(self.aktivny(HORUCE_UZLY) + 1), &nuly[..BLOK as usize])?;
        c.zapis(at(self.aktivny(TEPLE_UZLY)), &nuly)?;
        c.zapis(at(self.aktivny(STUDENE_UZLY)), &nuly)?;
        // balík kontrolného bodu 1
        let mut priznaky = 0x1 | 0x4; // odpojený, zhustené súhrny
        let bitov_nat = self.blokov_bitov_nat();
        if 6 + self.cp_navyse <= BLOKOV_V_SEGMENTE - bitov_nat {
            priznaky |= 0x80;
        }
        let verzia = u64::from_le_bytes(jedinecne()[16..24].try_into().unwrap_or([1; 8])) & 0x7FFF_FFFF | 1;
        let cp = self.kontrolny_bod(verzia, priznaky);
        let mut balik = Vec::new();
        balik.extend_from_slice(&cp);
        balik.resize(balik.len() + (self.cp_navyse * BLOK) as usize, 0);
        balik.extend(self.zhusteny_suhrn());
        balik.extend(F2fs::suhrn_uzlov(Some(KOREN)));
        balik.extend(F2fs::suhrn_uzlov(None));
        balik.extend(F2fs::suhrn_uzlov(None));
        balik.extend_from_slice(&cp);
        ciel::zapis_vsetko(c, at(self.seg0), &balik)?;
        if priznaky & 0x80 != 0 {
            let mut bity = vec![0u8; (bitov_nat * BLOK) as usize];
            let crc = u32::from_le_bytes(cp[CRC_POSUN..CRC_POSUN + 4].try_into().unwrap_or([0; 4]));
            le64(&mut bity, 0, verzia | (crc as u64) << 32);
            let n = (self.nat * 32) as usize;
            bity[8 + n..8 + 2 * n].fill(0xFF);
            bity[8 + n] &= !1; // blok NAT 0 nie je prázdny
            ciel::zapis_vsetko(c, at(self.seg0 + BLOKOV_V_SEGMENTE - bitov_nat), &bity)?;
        }
        // balík 2: verzia 0 (prvý a posledný blok)
        let cp2 = self.kontrolny_bod(0, priznaky);
        let zac2 = self.seg0 + BLOKOV_V_SEGMENTE;
        c.zapis(at(zac2), &cp2)?;
        c.zapis(at(zac2 + 6 + self.cp_navyse - 1), &cp2)?;
        // superblok dva razy
        let sb = self.superblok();
        c.zapis(at(0), &sb)?;
        c.zapis(at(1), &sb)?;
        priebeh(celkom, celkom);
        Ok(())
    }
}

impl crate::subory::Zvazok for F2fs {
    fn klaster_b(&self) -> u64 {
        BLOK
    }
    fn naplnene(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        strom: &crate::subory::Strom,
        _citaj: &mut crate::subory::Citaj,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<Vec<crate::subory::Zapisany>> {
        if !strom.subory().is_empty() || strom.uzly.len() > 1 {
            return Err(io::Error::other("zápis súborov na F2FS zatiaľ nie je – len formátovanie"));
        }
        self.naformatuj(c, posun, priebeh, zrus)?;
        Ok(Vec::new())
    }
    fn over(&self, zaciatok: &[u8]) -> bool {
        zaciatok.len() >= 1028
            && u32::from_le_bytes([zaciatok[1024], zaciatok[1025], zaciatok[1026], zaciatok[1027]]) == MAGIA
    }
    fn zmesti_sa(&self, strom: &crate::subory::Strom) -> bool {
        strom.uzly.len() <= 1
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn prerozdelenie_ako_mkfs() {
        // hodnoty z mkfs.f2fs 1.16 pre 64 MB, 300 MB, 1 GB, 4 GB, 32 GB a 256 GB
        for (hlavnych, rezerva, ovp) in [
            (24u64, 8u32, 10u32),
            (142, 17, 19),
            (502, 25, 27),
            (2029, 48, 50),
            (16275, 130, 132),
            (130693, 364, 366),
        ] {
            let o = najlepsie_prerozdelenie(hlavnych);
            let r = (100.0 / o + 1.0 + 6.0) as u32;
            let p = (((hlavnych - r as u64) as f64 * o / 100.0) as u32).max(r) + 2;
            assert_eq!((r, p), (rezerva, ovp), "hlavných segmentov {hlavnych}");
        }
    }

    #[test]
    fn rozvrh_ako_mkfs() {
        // 300 MB od sektora 0: segment0 512, SIT 1536, NAT 2560, SSA 3584, hlavná oblasť 4096
        let f = F2fs::navrhni(614_400, 512, 0, "Skuska").unwrap();
        assert_eq!(
            (f.seg0, f.sit_blok(), f.nat_blok(), f.ssa_blok(), f.hlavny_blok()),
            (512, 1536, 2560, 3584, 4096)
        );
        assert_eq!(
            (f.segmentov, f.hlavnych, f.aktivne[TEPLE_DATA], f.aktivne[STUDENE_DATA]),
            (149, 142, 70, 34)
        );
        // 32 GB: NAT 72 segmentov, SSA 32, hlavná oblasť od 55808
        let f = F2fs::navrhni(32 << 21, 512, 0, "").unwrap();
        assert_eq!((f.nat, f.ssa, f.hlavny_blok()), (72, 32, 55_808));
        assert_eq!(crc32(MAGIA, b""), MAGIA);
    }
}
