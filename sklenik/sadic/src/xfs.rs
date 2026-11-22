//! XFS v5 (s kontrolnými súčtami CRC32c) vlastným kódom – rozvrh ako mkfs.xfs 6.x pre jeden disk:
//! 4 skupiny pridelenia (AG), v každej superblok, AGF, AGI a AGFL, korene stromov voľného miesta
//! (podľa začiatku aj dĺžky) a inódov (aj voľných inódov – finobt), vyhradené bloky zoznamu AGFL;
//! v AG 0 prvý zhluk 64 inódov s koreňovým priečinkom a inódmi bitmapy a súhrnu realtime, v strednej
//! AG vnútorný žurnál so záznamom o odpojení. Blok 4 KB, inód 512 B, mená s typom (ftype),
//! časy za rok 2038 (bigtime). Bez reflink a rmapbt – zväzok pripojí každé jadro Linuxu od 5.10.

use crate::ciel::{self, Ciel};
use crate::oddiely::{jedinecne, teraz};
use std::io;
use std::sync::atomic::AtomicBool;

const BLOK: u64 = 4096;
const INOD: u64 = 512;
const INODOV_V_ZHLUKU: u64 = 64;
/// Zarovnanie zhlukov inódov (bloky).
const ZAROVNANIE_INODOV: u64 = 4;
const BLOKOV_AGFL: u64 = 4;
const NULA_AG: u32 = 0xFFFF_FFFF;
/// Časy bigtime: nanosekundy od 13. 12. 1901 (2^31 s pred rokom 1970).
const POSUN_BIGTIME: u64 = 1 << 31;

/// CRC-32C (Castagnoli) – kontrolné súčty XFS (uložené ako little-endian).
fn crc32c(d: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in d {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0x82F6_3B78 } else { c >> 1 };
        }
    }
    !c
}

fn be16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}
fn be32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}
fn be64(d: &mut [u8], o: usize, v: u64) {
    d[o..o + 8].copy_from_slice(&v.to_be_bytes());
}
/// Kontrolný súčet celého bufera s nulovým poľom na `o`.
fn podpis(d: &mut [u8], o: usize) {
    d[o..o + 4].fill(0);
    let c = crc32c(d);
    d[o..o + 4].copy_from_slice(&c.to_le_bytes());
}

/// Navrhnutý zväzok XFS.
#[derive(Debug, Clone)]
pub struct Xfs {
    sektor: u64,
    blokov: u64,
    ag_blokov: u64,
    ag_pocet: u64,
    ag_log: u32,
    zurnal_ag: u64,
    zurnal_blokov: u64,
    uuid: [u8; 16],
    menovka: [u8; 12],
}

impl Xfs {
    /// Navrhne XFS pre oddiel daného počtu sektorov.
    pub fn navrhni(sektory: u64, sektor: u32, nazov: &str) -> Result<Xfs, String> {
        if sektor != 512 && sektor != 4096 {
            return Err(format!("XFS: nepodporovaná veľkosť sektora {sektor} B"));
        }
        let mut blokov = sektory * sektor as u64 / BLOK;
        if blokov * BLOK < 270 << 20 {
            return Err("zväzok je primalý pre XFS (aspoň 300 MB, ako vyžaduje mkfs.xfs)".into());
        }
        // veľkosť AG ako mkfs.xfs pre jeden disk: do 4 TB štyri AG, inak najväčšie (1 TB)
        let max_ag = (1u64 << 40) / BLOK;
        let mut ag_blokov = if blokov >= (4u64 << 40) / BLOK { max_ag } else { blokov.div_ceil(4) };
        ag_blokov = ag_blokov.min(max_ag);
        let mut ag_pocet = blokov.div_ceil(ag_blokov);
        // posledná AG aspoň 16 MB, inak sa vynechá
        let posledna = blokov - (ag_pocet - 1) * ag_blokov;
        if ag_pocet > 1 && posledna < (16 << 20) / BLOK {
            ag_pocet -= 1;
            blokov = ag_pocet * ag_blokov;
        }
        let ag_log = 64 - (ag_blokov - 1).leading_zeros();
        // žurnál ako mkfs.xfs: 1/2048 zväzku, aspoň 64 MB, najviac necelé 2 GB (0x7F600000 B)
        let zurnal_blokov = (blokov / 2048).clamp(16_384, 0x7F60_0000 / BLOK);
        let zurnal_ag = ag_pocet / 2;
        let hlavicky = (4 * sektor as u64).div_ceil(BLOK);
        if hlavicky + 4 + BLOKOV_AGFL + zurnal_blokov + 16 > ag_blokov.min(posledna.max(ag_blokov)) {
            return Err("zväzok je primalý pre žurnál XFS".into());
        }
        let n = jedinecne();
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&n[..16]);
        uuid[6] = (uuid[6] & 0x0F) | 0x40;
        uuid[8] = (uuid[8] & 0x3F) | 0x80;
        let mut menovka = [0u8; 12];
        for (i, b) in crate::oddiely::bez_diakritiky(nazov.trim()).bytes().take(12).enumerate() {
            menovka[i] = b;
        }
        Ok(Xfs {
            sektor: sektor as u64,
            blokov,
            ag_blokov,
            ag_pocet,
            ag_log,
            zurnal_ag,
            zurnal_blokov,
            uuid,
            menovka,
        })
    }

    fn blokov_ag(&self, ag: u64) -> u64 {
        if ag + 1 == self.ag_pocet { self.blokov - ag * self.ag_blokov } else { self.ag_blokov }
    }
    /// Bloky so superblokom, AGF, AGI a AGFL.
    fn hlavicky(&self) -> u64 {
        (4 * self.sektor).div_ceil(BLOK)
    }
    /// Prvý blok za koreňmi stromov a zoznamom AGFL.
    fn prvy_volny(&self) -> u64 {
        self.hlavicky() + 4 + BLOKOV_AGFL
    }
    fn zhluk_inodov(&self) -> u64 {
        self.prvy_volny().div_ceil(ZAROVNANIE_INODOV) * ZAROVNANIE_INODOV
    }
    fn inopblog(&self) -> u32 {
        (BLOK / INOD).trailing_zeros()
    }
    /// Číslo inódu v AG 0.
    fn inod(&self, i: u64) -> u64 {
        (self.zhluk_inodov() << self.inopblog()) + i
    }
    /// Obsadené rozsahy AG (okrem hlavičiek, koreňov a AGFL): zhluk inódov, žurnál.
    fn obsadene(&self, ag: u64) -> Vec<(u64, u64)> {
        let mut v = Vec::new();
        if ag == 0 {
            v.push((self.zhluk_inodov(), INODOV_V_ZHLUKU * INOD / BLOK));
        }
        if ag == self.zurnal_ag {
            v.push((self.prvy_volny(), self.zurnal_blokov));
        }
        v.sort();
        v
    }
    /// Voľné rozsahy AG (začiatok, dĺžka) podľa začiatku.
    fn volne(&self, ag: u64) -> Vec<(u64, u64)> {
        let mut v = Vec::new();
        let mut p = self.prvy_volny();
        for (od, n) in self.obsadene(ag) {
            if od > p {
                v.push((p, od - p));
            }
            p = p.max(od + n);
        }
        let koniec = self.blokov_ag(ag);
        if koniec > p {
            v.push((p, koniec - p));
        }
        v
    }
    fn volnych(&self) -> u64 {
        (0..self.ag_pocet).map(|ag| self.volne(ag).iter().map(|x| x.1).sum::<u64>() + BLOKOV_AGFL).sum()
    }

    fn superblok(&self) -> Vec<u8> {
        let mut s = vec![0u8; self.sektor as usize];
        s[0..4].copy_from_slice(b"XFSB");
        be32(&mut s, 4, BLOK as u32);
        be64(&mut s, 8, self.blokov);
        s[32..48].copy_from_slice(&self.uuid);
        be64(&mut s, 48, (self.zurnal_ag << self.ag_log) + self.prvy_volny());
        be64(&mut s, 56, self.inod(0));
        be64(&mut s, 64, self.inod(1));
        be64(&mut s, 72, self.inod(2));
        be32(&mut s, 80, 1);
        be32(&mut s, 84, self.ag_blokov as u32);
        be32(&mut s, 88, self.ag_pocet as u32);
        be32(&mut s, 96, self.zurnal_blokov as u32);
        // verzia 5: dir v2, žurnál v2, zarovnané inódy, nlink, extflg (+ väčší sektor)
        be16(&mut s, 100, if self.sektor > 512 { 0xBCA5 } else { 0xB4A5 });
        be16(&mut s, 102, self.sektor as u16);
        be16(&mut s, 104, INOD as u16);
        be16(&mut s, 106, (BLOK / INOD) as u16);
        s[108..120].copy_from_slice(&self.menovka);
        s[120] = 12;
        s[121] = self.sektor.trailing_zeros() as u8;
        s[122] = INOD.trailing_zeros() as u8;
        s[123] = self.inopblog() as u8;
        s[124] = self.ag_log as u8;
        s[127] = if self.blokov * BLOK < 1 << 40 {
            25
        } else if self.blokov * BLOK < 50 << 40 {
            5
        } else {
            1
        };
        be64(&mut s, 128, INODOV_V_ZHLUKU);
        be64(&mut s, 136, INODOV_V_ZHLUKU - 3);
        be64(&mut s, 144, self.volnych());
        be32(&mut s, 180, ZAROVNANIE_INODOV as u32);
        if self.sektor > 512 {
            s[193] = self.sektor.trailing_zeros() as u8;
            be16(&mut s, 194, self.sektor as u16);
            be32(&mut s, 196, self.sektor as u32);
        } else {
            be32(&mut s, 196, 1); // logsunit
        }
        be32(&mut s, 200, 0x18A); // lazy-count, attr2, projid32, CRC
        be32(&mut s, 204, 0x18A);
        be32(&mut s, 212, 0x1); // finobt
        be32(&mut s, 216, 0x1 | 0x8); // ftype, bigtime
        podpis(&mut s, 224);
        s
    }

    fn agf(&self, ag: u64) -> Vec<u8> {
        let mut a = vec![0u8; self.sektor as usize];
        let volne = self.volne(ag);
        a[0..4].copy_from_slice(b"XAGF");
        be32(&mut a, 4, 1);
        be32(&mut a, 8, ag as u32);
        be32(&mut a, 12, self.blokov_ag(ag) as u32);
        let h = self.hlavicky() as u32;
        be32(&mut a, 16, h);
        be32(&mut a, 20, h + 1);
        be32(&mut a, 28, 1);
        be32(&mut a, 32, 1);
        be32(&mut a, 40, 0);
        be32(&mut a, 44, BLOKOV_AGFL as u32 - 1);
        be32(&mut a, 48, BLOKOV_AGFL as u32);
        be32(&mut a, 52, volne.iter().map(|x| x.1).sum::<u64>() as u32);
        be32(&mut a, 56, volne.iter().map(|x| x.1).max().unwrap_or(0) as u32);
        a[64..80].copy_from_slice(&self.uuid);
        podpis(&mut a, 216);
        a
    }

    fn agi(&self, ag: u64) -> Vec<u8> {
        let mut a = vec![0u8; self.sektor as usize];
        a[0..4].copy_from_slice(b"XAGI");
        be32(&mut a, 4, 1);
        be32(&mut a, 8, ag as u32);
        be32(&mut a, 12, self.blokov_ag(ag) as u32);
        let h = self.hlavicky() as u32;
        if ag == 0 {
            be32(&mut a, 16, INODOV_V_ZHLUKU as u32);
            be32(&mut a, 28, INODOV_V_ZHLUKU as u32 - 3);
            be32(&mut a, 32, self.inod(0) as u32);
        } else {
            be32(&mut a, 32, NULA_AG);
        }
        be32(&mut a, 20, h + 2);
        be32(&mut a, 24, 1);
        be32(&mut a, 36, NULA_AG);
        for i in 0..64 {
            be32(&mut a, 40 + 4 * i, NULA_AG);
        }
        a[296..312].copy_from_slice(&self.uuid);
        be32(&mut a, 328, h + 3);
        be32(&mut a, 332, 1);
        podpis(&mut a, 312);
        a
    }

    fn agfl(&self, ag: u64) -> Vec<u8> {
        let mut a = vec![0xFFu8; self.sektor as usize];
        a[..36].fill(0);
        a[0..4].copy_from_slice(b"XAFL");
        be32(&mut a, 4, ag as u32);
        a[8..24].copy_from_slice(&self.uuid);
        for i in 0..BLOKOV_AGFL {
            be32(&mut a, 36 + 4 * i as usize, (self.hlavicky() + 4 + i) as u32);
        }
        podpis(&mut a, 32);
        a
    }

    /// List stromu B (krátka hlavička v5) so záznamami.
    fn list(&self, ag: u64, blok: u64, magia: &[u8; 4], zaznamy: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![0u8; BLOK as usize];
        b[0..4].copy_from_slice(magia);
        be16(&mut b, 6, zaznamy.len() as u16);
        be32(&mut b, 8, NULA_AG);
        be32(&mut b, 12, NULA_AG);
        be64(&mut b, 16, (ag * self.ag_blokov + blok) * (BLOK / 512));
        b[32..48].copy_from_slice(&self.uuid);
        be32(&mut b, 48, ag as u32);
        let mut p = 56;
        for z in zaznamy {
            b[p..p + z.len()].copy_from_slice(z);
            p += z.len();
        }
        podpis(&mut b, 52);
        b
    }

    fn cas(unix: u64) -> u64 {
        (unix + POSUN_BIGTIME) * 1_000_000_000
    }

    /// Inód v3 (512 B) s číslom `ino`; `obsah` = dáta priamo v inóde.
    #[allow(clippy::too_many_arguments)]
    fn inod_v3(
        &self,
        ino: u64,
        rezim: u16,
        format: u8,
        odkazy: u32,
        velkost: u64,
        obsah: &[u8],
        generacia: u32,
    ) -> Vec<u8> {
        let mut d = vec![0u8; INOD as usize];
        d[0..2].copy_from_slice(b"IN");
        be16(&mut d, 2, rezim);
        d[4] = 3;
        d[5] = format;
        be32(&mut d, 16, odkazy);
        if rezim != 0 {
            let t = Xfs::cas(teraz());
            for o in [32, 40, 48, 144] {
                be64(&mut d, o, t);
            }
            be64(&mut d, 56, velkost);
            d[83] = 2; // formát atribútov: rozsahy
            be64(&mut d, 104, 1);
            be64(&mut d, 120, 0x8); // bigtime
        }
        be32(&mut d, 92, generacia);
        be32(&mut d, 96, NULA_AG);
        be64(&mut d, 152, ino);
        d[160..176].copy_from_slice(&self.uuid);
        d[176..176 + obsah.len()].copy_from_slice(obsah);
        podpis(&mut d, 100);
        d
    }

    /// Zhluk 64 inódov: koreň (priečinok s rodičom = sám), bitmapa a súhrn realtime, zvyšok voľný.
    fn zhluk(&self) -> Vec<u8> {
        let generacia = u32::from_le_bytes(jedinecne()[..4].try_into().unwrap_or([0; 4]));
        let mut v = Vec::with_capacity((INODOV_V_ZHLUKU * INOD) as usize);
        let koren = self.inod(0);
        for i in 0..INODOV_V_ZHLUKU {
            let ino = self.inod(i);
            let d = match i {
                0 => {
                    let mut sf = vec![0u8; 6];
                    be32(&mut sf, 2, koren as u32);
                    self.inod_v3(ino, 0o40755, 1, 2, 6, &sf, 0)
                }
                1 | 2 => self.inod_v3(ino, 0o100000, 2, 1, 0, &[], 0),
                _ => self.inod_v3(ino, 0, 0, 0, 0, &[], generacia),
            };
            v.extend(d);
        }
        v
    }

    /// Žurnál: záznam o odpojení v cykle 1 – hlavička (512 B) a dáta doplnené na sektor žurnálu;
    /// prvé slovo každého dátového bloku 512 B sa presúva do hlavičky a na jeho miesto ide cyklus.
    fn zaznam_zurnalu(&self) -> Vec<u8> {
        let dat_bb = if self.sektor > 512 { self.sektor as usize / 512 - 1 } else { 1 };
        let mut z = vec![0u8; 512 * (1 + dat_bb)];
        {
            let d = &mut z[512..];
            be32(d, 0, 0xB0C0_D0D0); // tid
            be32(d, 4, 8);
            d[8] = 0xAA; // klient: žurnál
            d[9] = 0x20; // transakcia odpojenia
            d[12..14].copy_from_slice(&0x556Eu16.to_le_bytes());
        }
        let h = &mut z[..512];
        be32(h, 0, 0xFEED_BABE);
        be32(h, 4, 1);
        be32(h, 8, 2);
        be32(h, 12, (512 * dat_bb) as u32);
        be64(h, 16, 1 << 32);
        be64(h, 24, 1 << 32);
        be32(h, 36, 0xFFFF_FFFF);
        be32(h, 40, 1);
        be32(h, 300, 1); // formát: Linux little-endian
        h[304..320].copy_from_slice(&self.uuid);
        be32(h, 320, 32_768);
        for i in 0..dat_bb {
            let o = 512 * (1 + i);
            let povodne = u32::from_be_bytes(z[o..o + 4].try_into().unwrap_or([0; 4]));
            be32(&mut z, 44 + 4 * i, povodne);
            be32(&mut z, o, 1);
        }
        z
    }

    /// Zapíše prázdny zväzok od bajtu `posun`.
    pub fn naformatuj(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<()> {
        let at = |ag: u64, blok: u64| posun + (ag * self.ag_blokov + blok) * BLOK;
        let celkom = self.zurnal_blokov * BLOK + self.ag_pocet * self.prvy_volny() * BLOK;
        let sb = self.superblok();
        let mut hotovo = 0;
        for ag in 0..self.ag_pocet {
            // hlavičky: superblok, AGF, AGI, AGFL (každá v sektore), zvyšok blokov nuly
            let mut hl = vec![0u8; (self.hlavicky() * BLOK) as usize];
            let s = self.sektor as usize;
            hl[..s].copy_from_slice(&sb);
            hl[s..2 * s].copy_from_slice(&self.agf(ag));
            hl[2 * s..3 * s].copy_from_slice(&self.agi(ag));
            hl[3 * s..4 * s].copy_from_slice(&self.agfl(ag));
            let h = self.hlavicky();
            let zaznamy_volne = |podla_dlzky: bool| -> Vec<Vec<u8>> {
                let mut v = self.volne(ag);
                if podla_dlzky {
                    v.sort_by_key(|&(od, n)| (n, od));
                }
                v.iter()
                    .map(|&(od, n)| {
                        let mut r = vec![0u8; 8];
                        be32(&mut r, 0, od as u32);
                        be32(&mut r, 4, n as u32);
                        r
                    })
                    .collect()
            };
            let inody: Vec<Vec<u8>> = if ag == 0 {
                let mut r = vec![0u8; 16];
                be32(&mut r, 0, self.inod(0) as u32);
                be32(&mut r, 4, INODOV_V_ZHLUKU as u32 - 3);
                be64(&mut r, 8, !0u64 << 3); // voľné všetky okrem prvých troch
                vec![r]
            } else {
                Vec::new()
            };
            hl.extend(self.list(ag, h, b"AB3B", &zaznamy_volne(false)));
            hl.extend(self.list(ag, h + 1, b"AB3C", &zaznamy_volne(true)));
            hl.extend(self.list(ag, h + 2, b"IAB3", &inody));
            hl.extend(self.list(ag, h + 3, b"FIB3", &inody));
            ciel::zapis_vsetko(c, at(ag, 0), &hl)?;
            hotovo += self.prvy_volny() * BLOK;
            priebeh(hotovo, celkom);
        }
        ciel::zapis_vsetko(c, at(0, self.zhluk_inodov()), &self.zhluk())?;
        // žurnál: nuly a na začiatku záznam o odpojení
        let zac = at(self.zurnal_ag, self.prvy_volny());
        let h0 = hotovo;
        ciel::vynuluj(c, zac, self.zurnal_blokov * BLOK, &mut |h, _| priebeh(h0 + h, celkom), zrus)?;
        c.zapis(zac, &self.zaznam_zurnalu())?;
        priebeh(celkom, celkom);
        Ok(())
    }
}

impl crate::subory::Zvazok for Xfs {
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
            return Err(io::Error::other("zápis súborov na XFS zatiaľ nie je – len formátovanie"));
        }
        self.naformatuj(c, posun, priebeh, zrus)?;
        Ok(Vec::new())
    }
    fn over(&self, zaciatok: &[u8]) -> bool {
        zaciatok.len() >= 4 && &zaciatok[..4] == b"XFSB"
    }
    fn zmesti_sa(&self, strom: &crate::subory::Strom) -> bool {
        strom.uzly.len() <= 1
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn crc32c_ako_v_norme() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn rozvrh_ako_mkfs() {
        // 300 MB: 4 AG po 19 200 blokoch, koreň 96, voľné v AG 0 od 9 (3 bloky) a od 20
        let x = Xfs::navrhni(614_400, 512, "Skuska").unwrap();
        assert_eq!((x.ag_pocet, x.ag_blokov, x.ag_log, x.inod(0)), (4, 19_200, 15, 96));
        assert_eq!(x.volne(0), vec![(9, 3), (20, 19_180)]);
        assert_eq!(x.zurnal_blokov, 16_384);
        // 256 GB: žurnál 32 768 blokov, 2 TB: 262 144
        assert_eq!(Xfs::navrhni(256 << 21, 512, "").unwrap().zurnal_blokov, 32_768);
        assert_eq!(Xfs::navrhni(2048 << 21, 512, "").unwrap().zurnal_blokov, 262_144);
    }
}
