//! ext2, ext3 a ext4 vlastným kódom: superblok (so zálohami v skupinách podľa sparse_super), popisy
//! skupín, bitmapy blokov a inódov, tabuľky inódov, koreňový priečinok, lost+found a pri ext3/ext4
//! žurnál (JBD2). ext4 používa rozsahy (extents) a rozšírené inódy. Blok 4 KB, 16 KB na inód –
//! rovnaké predvoľby ako mke2fs.

use crate::ciel::{self, Ciel};
use crate::oddiely::{jedinecne, teraz};
use std::io;
use std::sync::atomic::AtomicBool;

const BLOK: u64 = 4096;
const BLOKOV_V_SKUPINE: u64 = 32_768;
const INODOV_V_SKUPINE_MAX: u64 = 8192;
const VELKOST_INODU: u64 = 256;
const PRVY_INOD: u32 = 11;

/// Druh systému.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Druh {
    Ext2,
    Ext3,
    Ext4,
}

impl Druh {
    pub fn nazov(self) -> &'static str {
        match self {
            Druh::Ext2 => "ext2",
            Druh::Ext3 => "ext3",
            Druh::Ext4 => "ext4",
        }
    }
    fn zurnal(self) -> bool {
        self != Druh::Ext2
    }
    fn rozsahy(self) -> bool {
        self == Druh::Ext4
    }
}

/// Rozvrh zväzku ext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ext {
    pub druh: Druh,
    pub blokov: u64,
    pub skupin: u64,
    pub inodov_v_skupine: u64,
    pub blokov_zurnalu: u64,
    pub uuid: [u8; 16],
    pub menovka: [u8; 16],
}

fn u16le(v: &mut [u8], i: usize, x: u16) {
    v[i..i + 2].copy_from_slice(&x.to_le_bytes());
}
fn u32le(v: &mut [u8], i: usize, x: u32) {
    v[i..i + 4].copy_from_slice(&x.to_le_bytes());
}
fn u32be(v: &mut [u8], i: usize, x: u32) {
    v[i..i + 4].copy_from_slice(&x.to_be_bytes());
}

/// Má skupina zálohu superbloku (sparse_super: 0, 1 a mocniny 3, 5, 7)?
fn ma_superblok(g: u64) -> bool {
    if g <= 1 {
        return true;
    }
    for z in [3u64, 5, 7] {
        let mut m = z;
        while m < g {
            m *= z;
        }
        if m == g {
            return true;
        }
    }
    false
}

/// Veľkosť žurnálu v blokoch (ako mke2fs, najviac 64 MB – zmestí sa súvisle do prvej skupiny).
fn velkost_zurnalu(blokov: u64) -> u64 {
    match blokov {
        b if b < 2048 => 0,
        b if b < 32_768 => 1024,
        b if b < 256 * 1024 => 4096,
        b if b < 512 * 1024 => 8192,
        _ => 16_384,
    }
}

/// Menovka ext: najviac 16 bajtov UTF-8.
pub fn menovka(t: &str) -> [u8; 16] {
    let mut m = [0u8; 16];
    let mut n = 0;
    for c in t.trim().chars() {
        let mut b = [0u8; 4];
        let s = c.encode_utf8(&mut b);
        if n + s.len() > 16 {
            break;
        }
        m[n..n + s.len()].copy_from_slice(s.as_bytes());
        n += s.len();
    }
    m
}

impl Ext {
    pub fn navrhni(sektory: u64, sektor: u32, druh: Druh, nazov: &str) -> Result<Ext, String> {
        let velkost = sektory * sektor as u64;
        let mut blokov = velkost / BLOK;
        if blokov < 1024 {
            return Err(format!("zväzok je primalý pre {} (aspoň 4 MB)", druh.nazov()));
        }
        if blokov > u32::MAX as u64 {
            return Err("zväzok je priveľký (nad 16 TB)".into());
        }
        let inodov = |b: u64| (b * BLOK / 16_384).clamp(16, INODOV_V_SKUPINE_MAX);
        let mut skupin = blokov.div_ceil(BLOKOV_V_SKUPINE);
        let mut inodov_v_skupine = inodov(BLOKOV_V_SKUPINE.min(blokov));
        // zaokrúhlenie na celé bloky tabuľky inódov (16 inódov po 256 B na blok)
        inodov_v_skupine = inodov_v_skupine.div_ceil(16) * 16;
        let rezia = |g: u64, skupin: u64| -> u64 {
            let gdt = (skupin * 32).div_ceil(BLOK);
            (if ma_superblok(g) { 1 + gdt } else { 0 }) + 2 + inodov_v_skupine * VELKOST_INODU / BLOK
        };
        // posledná skupina musí uniesť svoju réžiu a aspoň 50 blokov dát
        let posledna = blokov - (skupin - 1) * BLOKOV_V_SKUPINE;
        if skupin > 1 && posledna < rezia(skupin - 1, skupin) + 50 {
            skupin -= 1;
            blokov = skupin * BLOKOV_V_SKUPINE;
        }
        if blokov < rezia(0, skupin) + 64 {
            return Err(format!("zväzok je primalý pre {}", druh.nazov()));
        }
        let blokov_zurnalu = if druh.zurnal() { velkost_zurnalu(blokov) } else { 0 };
        if druh.zurnal() && blokov_zurnalu == 0 {
            return Err(format!("zväzok je primalý pre žurnál {}", druh.nazov()));
        }
        let n = jedinecne();
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&n[16..32]);
        uuid[6] = (uuid[6] & 0x0F) | 0x40;
        uuid[8] = (uuid[8] & 0x3F) | 0x80;
        Ok(Ext { druh, blokov, skupin, inodov_v_skupine, blokov_zurnalu, uuid, menovka: menovka(nazov) })
    }

    fn gdt_blokov(&self) -> u64 {
        (self.skupin * 32).div_ceil(BLOK)
    }

    fn tabulka_blokov(&self) -> u64 {
        self.inodov_v_skupine * VELKOST_INODU / BLOK
    }

    /// Prvý blok skupiny a jej dĺžka.
    fn skupina(&self, g: u64) -> (u64, u64) {
        let od = g * BLOKOV_V_SKUPINE;
        (od, BLOKOV_V_SKUPINE.min(self.blokov - od))
    }

    /// Bloky réžie skupiny: (bitmapa blokov, bitmapa inódov, tabuľka inódov, prvý voľný).
    fn rozlozenie(&self, g: u64) -> (u64, u64, u64, u64) {
        let (od, _) = self.skupina(g);
        let sb = if ma_superblok(g) { 1 + self.gdt_blokov() } else { 0 };
        let bb = od + sb;
        (bb, bb + 1, bb + 2, bb + 2 + self.tabulka_blokov())
    }

    /// Inód. `bloky` = úseky dát; `navyse` = bloky mapy (nepriame bloky ext2/3) do i_blocks;
    /// `iblock` = hotové pole i_block (mapa blokov ext2/3), inak sa vyplní samo.
    #[allow(clippy::too_many_arguments)]
    fn inod(
        &self,
        mod_: u16,
        velkost: u64,
        odkazy: u16,
        bloky: &[(u64, u64)],
        cas: u32,
        navyse: u64,
        iblock: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut i = vec![0u8; VELKOST_INODU as usize];
        u16le(&mut i, 0, mod_);
        u32le(&mut i, 4, velkost as u32);
        for o in [8, 12, 16] {
            u32le(&mut i, o, cas);
        }
        u16le(&mut i, 26, odkazy);
        let data: u64 = bloky.iter().map(|b| b.1).sum::<u64>() + navyse;
        u32le(&mut i, 28, (data * (BLOK / 512)) as u32);
        u32le(&mut i, 108, (velkost >> 32) as u32);
        if self.druh.rozsahy() {
            u32le(&mut i, 32, 0x8_0000); // EXT4_EXTENTS_FL
            u16le(&mut i, 40, 0xF30A);
            u16le(&mut i, 42, bloky.len() as u16);
            u16le(&mut i, 44, 4);
            let mut logicky = 0u64;
            for (k, &(zac, n)) in bloky.iter().enumerate() {
                let e = 52 + 12 * k;
                u32le(&mut i, e, logicky as u32);
                u16le(&mut i, e + 4, n as u16);
                u16le(&mut i, e + 6, (zac >> 32) as u16);
                u32le(&mut i, e + 8, zac as u32);
                logicky += n;
            }
            u16le(&mut i, 128, 32); // i_extra_isize
            u32le(&mut i, 144, cas); // i_crtime
        } else if let Some(ib) = iblock {
            i[40..100].copy_from_slice(&ib[..60]);
        } else {
            let mut k = 0;
            for &(zac, n) in bloky {
                for b in zac..zac + n {
                    if k < 12 {
                        u32le(&mut i, 40 + 4 * k, b as u32);
                        k += 1;
                    }
                }
            }
        }
        i
    }

    /// Mapa blokov ext2/ext3 pre súvislý úsek dát: i_block (60 B) a nepriame bloky, ktoré ležia na
    /// blokoch `nepriame_od…`. Vráti (i_block, [(číslo bloku, obsah)]).
    fn mapa_blokov(zac: u64, n: u64, nepriame_od: u64) -> (Vec<u8>, Vec<(u64, Vec<u8>)>) {
        let na_blok = (BLOK / 4) as usize;
        let mut ib = vec![0u8; 60];
        let mut bloky = Vec::new();
        let mut dalsi = nepriame_od;
        let data: Vec<u32> = (zac..zac + n).map(|b| b as u32).collect();
        for (k, &b) in data.iter().take(12).enumerate() {
            u32le(&mut ib, 4 * k, b);
        }
        let mut zvysok = &data[data.len().min(12)..];
        if !zvysok.is_empty() {
            let mut b = vec![0u8; BLOK as usize];
            let teraz = &zvysok[..zvysok.len().min(na_blok)];
            for (k, &x) in teraz.iter().enumerate() {
                u32le(&mut b, 4 * k, x);
            }
            u32le(&mut ib, 48, dalsi as u32);
            bloky.push((dalsi, b));
            dalsi += 1;
            zvysok = &zvysok[teraz.len()..];
        }
        if !zvysok.is_empty() {
            let dvojity = dalsi;
            dalsi += 1;
            let mut d = vec![0u8; BLOK as usize];
            let mut k = 0;
            while !zvysok.is_empty() && k < na_blok {
                let mut b = vec![0u8; BLOK as usize];
                let teraz = &zvysok[..zvysok.len().min(na_blok)];
                for (j, &x) in teraz.iter().enumerate() {
                    u32le(&mut b, 4 * j, x);
                }
                u32le(&mut d, 4 * k, dalsi as u32);
                bloky.push((dalsi, b));
                dalsi += 1;
                zvysok = &zvysok[teraz.len()..];
                k += 1;
            }
            u32le(&mut ib, 52, dvojity as u32);
            bloky.push((dvojity, d));
        }
        (ib, bloky)
    }

    fn priecinok(polozky: &[(u32, &str, u8)]) -> Vec<u8> {
        let mut b = vec![0u8; BLOK as usize];
        let mut p = 0usize;
        for (k, &(inod, meno, typ)) in polozky.iter().enumerate() {
            let dlzka = (8 + meno.len()).div_ceil(4) * 4;
            let rec = if k + 1 == polozky.len() { BLOK as usize - p } else { dlzka };
            u32le(&mut b, p, inod);
            u16le(&mut b, p + 4, rec as u16);
            b[p + 6] = meno.len() as u8;
            b[p + 7] = typ;
            b[p + 8..p + 8 + meno.len()].copy_from_slice(meno.as_bytes());
            p += rec;
        }
        b
    }

    /// Naformátuje prázdny zväzok.
    pub fn naformatuj(
        &self,
        c: &mut dyn Ciel,
        posun: u64,
        priebeh: &mut dyn FnMut(u64, u64),
        zrus: &AtomicBool,
    ) -> io::Result<()> {
        let cas = teraz() as u32;
        let (_, _, _, volny0) = self.rozlozenie(0);
        // pridelenie v skupine 0: koreň, lost+found (4 bloky), žurnál (+ nepriame bloky pri ext3)
        let koren = volny0;
        let lpf = koren + 1;
        let zurnal = lpf + 4;
        let mut koniec = zurnal + self.blokov_zurnalu;
        let mut nepriame = Vec::new();
        let mut ib_zurnalu = Vec::new();
        if self.druh.zurnal() && !self.druh.rozsahy() {
            let (ib, b) = Ext::mapa_blokov(zurnal, self.blokov_zurnalu, koniec);
            koniec += b.len() as u64;
            ib_zurnalu = ib;
            nepriame = b;
        }
        if koniec > self.skupina(0).1 {
            return Err(io::Error::other("žurnál sa nezmestí do prvej skupiny"));
        }
        // inódy
        let rozsahy = |od: u64, n: u64| -> Vec<(u64, u64)> {
            let mut v = Vec::new();
            let mut p = od;
            while p < od + n {
                let k = (od + n - p).min(32_768);
                v.push((p, k));
                p += k;
            }
            v
        };
        let mut inody = vec![vec![0u8; VELKOST_INODU as usize]; PRVY_INOD as usize];
        inody[1] = self.inod(0x41ED, BLOK, 3, &[(koren, 1)], cas, 0, None);
        inody[10] = self.inod(0x41C0, 4 * BLOK, 2, &[(lpf, 4)], cas, 0, None);
        if self.druh.zurnal() {
            let b = rozsahy(zurnal, self.blokov_zurnalu);
            let ib = (!self.druh.rozsahy()).then_some(ib_zurnalu.as_slice());
            inody[7] = self.inod(0x8180, self.blokov_zurnalu * BLOK, 1, &b, cas, nepriame.len() as u64, ib);
        }
        // superblok
        let pouzite0 = koniec - self.skupina(0).0;
        let mut volne_bloky = 0u64;
        let mut volne_g = Vec::new();
        for g in 0..self.skupin {
            let (od, n) = self.skupina(g);
            let (_, _, _, volny) = self.rozlozenie(g);
            let rezia = volny - od;
            let pouzite = if g == 0 { pouzite0 } else { rezia };
            volne_g.push(n - pouzite);
            volne_bloky += n - pouzite;
        }
        let inodov = self.inodov_v_skupine * self.skupin;
        let mut sb = vec![0u8; 1024];
        u32le(&mut sb, 0, inodov as u32);
        u32le(&mut sb, 4, self.blokov as u32);
        u32le(&mut sb, 8, (self.blokov / 20) as u32);
        u32le(&mut sb, 12, volne_bloky as u32);
        u32le(&mut sb, 16, (inodov - PRVY_INOD as u64) as u32);
        u32le(&mut sb, 24, 2);
        u32le(&mut sb, 28, 2);
        u32le(&mut sb, 32, BLOKOV_V_SKUPINE as u32);
        u32le(&mut sb, 36, BLOKOV_V_SKUPINE as u32);
        u32le(&mut sb, 40, self.inodov_v_skupine as u32);
        u32le(&mut sb, 48, cas);
        u16le(&mut sb, 54, 0xFFFF);
        u16le(&mut sb, 56, 0xEF53);
        u16le(&mut sb, 58, 1);
        u16le(&mut sb, 60, 1);
        u32le(&mut sb, 64, cas);
        u32le(&mut sb, 76, 1);
        u32le(&mut sb, 84, PRVY_INOD);
        u16le(&mut sb, 88, VELKOST_INODU as u16);
        let (compat, incompat, ro) = match self.druh {
            Druh::Ext2 => (0x08, 0x02, 0x03),
            Druh::Ext3 => (0x0C, 0x02, 0x03),
            Druh::Ext4 => (0x0C, 0x02 | 0x40, 0x03 | 0x08 | 0x20 | 0x40),
        };
        u32le(&mut sb, 92, compat);
        u32le(&mut sb, 96, incompat);
        u32le(&mut sb, 100, ro);
        sb[104..120].copy_from_slice(&self.uuid);
        sb[120..136].copy_from_slice(&self.menovka);
        let n = jedinecne();
        sb[236..252].copy_from_slice(&n[..16]);
        sb[252] = 1; // half_md4
        if self.druh.zurnal() {
            u32le(&mut sb, 224, 8);
            sb[253] = 1; // záloha i_block žurnálu v s_jnl_blocks
            let j = &inody[7];
            sb[268..328].copy_from_slice(&j[40..100]);
            u32le(&mut sb, 328, u32::from_le_bytes([j[108], j[109], j[110], j[111]]));
            u32le(&mut sb, 332, u32::from_le_bytes([j[4], j[5], j[6], j[7]]));
        }
        u32le(&mut sb, 264, cas);
        if self.druh.rozsahy() {
            u16le(&mut sb, 348, 32);
            u16le(&mut sb, 350, 32);
        }
        u32le(&mut sb, 352, 1); // s_flags: podpísaný hash priečinkov
        // popisy skupín
        let mut gdt = vec![0u8; (self.gdt_blokov() * BLOK) as usize];
        for g in 0..self.skupin {
            let (bb, ib, it, _) = self.rozlozenie(g);
            let d = &mut gdt[(g * 32) as usize..(g * 32 + 32) as usize];
            u32le(d, 0, bb as u32);
            u32le(d, 4, ib as u32);
            u32le(d, 8, it as u32);
            u16le(d, 12, volne_g[g as usize] as u16);
            let volne_i =
                if g == 0 { self.inodov_v_skupine - PRVY_INOD as u64 } else { self.inodov_v_skupine };
            u16le(d, 14, volne_i as u16);
            u16le(d, 16, if g == 0 { 2 } else { 0 });
        }
        // zápis po skupinách: superblok + GDT, bitmapy, vynulovaná tabuľka inódov
        let celkom = self.skupin * self.tabulka_blokov() * BLOK + self.blokov_zurnalu * BLOK;
        let mut hotovo = 0u64;
        for g in 0..self.skupin {
            let (od, n) = self.skupina(g);
            let (bb, ib, it, volny) = self.rozlozenie(g);
            if ma_superblok(g) {
                let mut s = sb.clone();
                u16le(&mut s, 90, g as u16);
                let mut b = vec![0u8; BLOK as usize];
                let sb_od = if g == 0 { 1024 } else { 0 };
                b[sb_od..sb_od + 1024].copy_from_slice(&s);
                ciel::zapis_vsetko(c, posun + od * BLOK, &b)?;
                ciel::zapis_vsetko(c, posun + (od + 1) * BLOK, &gdt)?;
            }
            let mut bm = vec![0u8; BLOK as usize];
            let pouzite = if g == 0 { koniec - od } else { volny - od };
            for i in 0..pouzite {
                bm[(i / 8) as usize] |= 1 << (i % 8);
            }
            for i in n..BLOKOV_V_SKUPINE {
                bm[(i / 8) as usize] |= 1 << (i % 8);
            }
            ciel::zapis_vsetko(c, posun + bb * BLOK, &bm)?;
            let mut im = vec![0u8; BLOK as usize];
            if g == 0 {
                for i in 0..PRVY_INOD as u64 {
                    im[(i / 8) as usize] |= 1 << (i % 8);
                }
            }
            for i in self.inodov_v_skupine..BLOK * 8 {
                im[(i / 8) as usize] |= 1 << (i % 8);
            }
            ciel::zapis_vsetko(c, posun + ib * BLOK, &im)?;
            let tab = self.tabulka_blokov() * BLOK;
            ciel::vynuluj(c, posun + it * BLOK, tab, &mut |h, _| priebeh(hotovo + h, celkom), zrus)?;
            if g == 0 {
                ciel::zapis_vsetko(c, posun + it * BLOK, &inody.concat())?;
            }
            hotovo += tab;
        }
        // koreň a lost+found
        let koren_b = Ext::priecinok(&[(2, ".", 2), (2, "..", 2), (11, "lost+found", 2)]);
        ciel::zapis_vsetko(c, posun + koren * BLOK, &koren_b)?;
        let mut l = Ext::priecinok(&[(11, ".", 2), (2, "..", 2)]);
        for _ in 1..4 {
            let mut prazdny = vec![0u8; BLOK as usize];
            u16le(&mut prazdny, 4, BLOK as u16);
            l.extend(prazdny);
        }
        ciel::zapis_vsetko(c, posun + lpf * BLOK, &l)?;
        // žurnál: superblok JBD2 v prvom bloku, ostatné nuly
        if self.druh.zurnal() {
            ciel::vynuluj(
                c,
                posun + zurnal * BLOK,
                self.blokov_zurnalu * BLOK,
                &mut |h, _| priebeh(hotovo + h, celkom),
                zrus,
            )?;
            let mut j = vec![0u8; BLOK as usize];
            u32be(&mut j, 0, 0xC03B_3998);
            u32be(&mut j, 4, 4); // superblok v2
            u32be(&mut j, 12, BLOK as u32);
            u32be(&mut j, 16, self.blokov_zurnalu as u32);
            u32be(&mut j, 20, 1);
            u32be(&mut j, 24, 1);
            j[48..64].copy_from_slice(&self.uuid);
            u32be(&mut j, 64, 1);
            ciel::zapis_vsetko(c, posun + zurnal * BLOK, &j)?;
            for (b, obsah) in &nepriame {
                ciel::zapis_vsetko(c, posun + b * BLOK, obsah)?;
            }
        }
        priebeh(celkom, celkom);
        Ok(())
    }
}

impl crate::subory::Zvazok for Ext {
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
            return Err(io::Error::other("zápis súborov na ext zatiaľ nie je – len formátovanie"));
        }
        self.naformatuj(c, posun, priebeh, zrus)?;
        Ok(Vec::new())
    }
    fn over(&self, zaciatok: &[u8]) -> bool {
        zaciatok.len() >= 1084 && zaciatok[1080] == 0x53 && zaciatok[1081] == 0xEF
    }
    fn zmesti_sa(&self, strom: &crate::subory::Strom) -> bool {
        strom.uzly.len() <= 1
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::ciel::Pamat;

    #[test]
    fn zalohy_superbloku() {
        let s: Vec<u64> = (0..100).filter(|&g| ma_superblok(g)).collect();
        assert_eq!(s, vec![0, 1, 3, 5, 7, 9, 25, 27, 49, 81]);
    }

    #[test]
    fn mapa_blokov_s_nepriamymi() {
        let (ib, b) = Ext::mapa_blokov(1000, 2000, 5000);
        assert_eq!(u32::from_le_bytes(ib[0..4].try_into().unwrap()), 1000);
        assert_eq!(u32::from_le_bytes(ib[48..52].try_into().unwrap()), 5000, "nepriamy blok");
        assert_eq!(b.len(), 1 + 1 + 1, "nepriamy, jeden pod dvojitým, dvojitý");
    }

    #[test]
    fn rozvrh_a_podpis() {
        for druh in [Druh::Ext2, Druh::Ext3, Druh::Ext4] {
            let mut p = Pamat::nova(64 << 20, 512);
            let e = Ext::navrhni((64 << 20) / 512, 512, druh, "Skúška").unwrap();
            e.naformatuj(&mut p, 0, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
            assert_eq!(&p.data[1080..1082], &[0x53, 0xEF]);
            assert_eq!(e.skupin, 1);
        }
    }
}
