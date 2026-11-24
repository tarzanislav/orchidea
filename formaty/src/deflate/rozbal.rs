//! Dekodér DEFLATE – rozbalenie blokov uložených, s pevnými a s dynamickými Huffmanovými kódmi.

use super::{BITY_DLZKY, BITY_VZDIALENOSTI, PORADIE_DLZOK, ZAKLAD_DLZKY, ZAKLAD_VZDIALENOSTI, pevne_dlzky};
use crate::huffman::{kanonicke_kody, obrat_bity};
use orchidea_jadro::{Chyba, Vysledok};

/// Počet bitov prvej úrovne tabuľky – kódy do tejto dĺžky sa prečítajú jedným pohľadom.
const PRVA_UROVEN: u32 = 10;
/// Príznak odkazu na podtabuľku v položke.
const ODKAZ: u32 = 0x8000_0000;

/// Tabuľka Huffmanovho dekodéra. Položka: `znak << 8 | dĺžka` (dĺžka 0 = neplatný kód), alebo
/// `ODKAZ | začiatok << 8 | bity podtabuľky`.
pub(crate) struct Dekoder {
    tabulka: Vec<u32>,
}

impl Dekoder {
    /// Zostaví dekodér z dĺžok kódov. Prečerpaný kód je chyba; neúplný je dovolený len s jediným kódom
    /// (DEFLATE ho pripúšťa pre vzdialenosti).
    pub(crate) fn novy(dlzky: &[u8]) -> Vysledok<Dekoder> {
        let mut pocet = [0i32; 16];
        for &l in dlzky {
            if l > 15 {
                return Err(Chyba::PoskodeneData("deflate: dĺžka kódu nad 15".into()));
            }
            pocet[l as usize] += 1;
        }
        pocet[0] = 0;
        let mut volne = 1i32;
        for &p in &pocet[1..] {
            volne = (volne << 1) - p;
            if volne < 0 {
                return Err(Chyba::PoskodeneData("deflate: prečerpaný Huffmanov kód".into()));
            }
        }
        let najdlhsi = (1..16).rev().find(|&l| pocet[l] > 0).unwrap_or(0) as u32;
        if volne > 0 && najdlhsi > 1 {
            return Err(Chyba::PoskodeneData("deflate: neúplný Huffmanov kód".into()));
        }
        let kody = kanonicke_kody(dlzky);
        let mut tabulka = vec![0u32; 1 << PRVA_UROVEN];
        // podtabuľky: pre každú predponu prvej úrovne najväčšia dĺžka kódu, ktorý ňou začína
        let mut hlbka_predpony = vec![0u32; 1 << PRVA_UROVEN];
        let maska = (1u32 << PRVA_UROVEN) - 1;
        for (znak, &l) in dlzky.iter().enumerate() {
            let l = l as u32;
            if l == 0 {
                continue;
            }
            let r = obrat_bity(kody[znak], l);
            if l <= PRVA_UROVEN {
                let mut i = r;
                while i < (1 << PRVA_UROVEN) {
                    tabulka[i as usize] = (znak as u32) << 8 | l;
                    i += 1 << l;
                }
            } else {
                let p = (r & maska) as usize;
                hlbka_predpony[p] = hlbka_predpony[p].max(l - PRVA_UROVEN);
            }
        }
        for (p, &h) in hlbka_predpony.iter().enumerate() {
            if h > 0 {
                let zaciatok = tabulka.len() as u32;
                tabulka.resize(tabulka.len() + (1 << h), 0);
                tabulka[p] = ODKAZ | zaciatok << 8 | h;
            }
        }
        for (znak, &l) in dlzky.iter().enumerate() {
            let l = l as u32;
            if l <= PRVA_UROVEN {
                continue;
            }
            let r = obrat_bity(kody[znak], l);
            let odkaz = tabulka[(r & maska) as usize];
            let (zaciatok, h) = ((odkaz & !ODKAZ) >> 8, odkaz & 0xFF);
            let mut i = r >> PRVA_UROVEN;
            while i < (1 << h) {
                tabulka[(zaciatok + i) as usize] = (znak as u32) << 8 | l;
                i += 1 << (l - PRVA_UROVEN);
            }
        }
        Ok(Dekoder { tabulka })
    }

    /// Prečíta jeden znak.
    #[inline]
    fn znak(&self, c: &mut CitacBitov) -> Vysledok<u32> {
        c.dopln();
        let b = c.akum as u32;
        let mut e = self.tabulka[(b & ((1 << PRVA_UROVEN) - 1)) as usize];
        if e & ODKAZ != 0 {
            let (zaciatok, h) = ((e & !ODKAZ) >> 8, e & 0xFF);
            e = self.tabulka[(zaciatok + ((b >> PRVA_UROVEN) & ((1 << h) - 1))) as usize];
        }
        let l = e & 0xFF;
        if l == 0 {
            return Err(Chyba::PoskodeneData("deflate: neplatný Huffmanov kód".into()));
        }
        c.zahod(l);
        Ok(e >> 8)
    }
}

/// Čítač bitov od najnižšieho. Za koncom údajov dopĺňa nuly a počíta ich – prečítanie za koniec je chyba.
pub(crate) struct CitacBitov<'a> {
    data: &'a [u8],
    poz: usize,
    akum: u64,
    bitov: u32,
    za_koncom: u32,
}

impl<'a> CitacBitov<'a> {
    pub(crate) fn novy(data: &'a [u8]) -> CitacBitov<'a> {
        CitacBitov { data, poz: 0, akum: 0, bitov: 0, za_koncom: 0 }
    }

    #[inline]
    fn dopln(&mut self) {
        if self.bitov > 32 {
            return;
        }
        if self.poz + 8 <= self.data.len() {
            // osem bajtov naraz; čiastočne pridaný bajt sa nabudúce pridá znovu (rovnaké bity, OR ich nezmení)
            let mut b = [0u8; 8];
            b.copy_from_slice(&self.data[self.poz..self.poz + 8]);
            self.akum |= u64::from_le_bytes(b) << self.bitov;
            let pridat = (63 - self.bitov) / 8;
            self.poz += pridat as usize;
            self.bitov += pridat * 8;
        } else {
            while self.bitov <= 56 {
                let b = if self.poz < self.data.len() {
                    self.poz += 1;
                    self.data[self.poz - 1]
                } else {
                    self.za_koncom += 8;
                    0
                };
                self.akum |= (b as u64) << self.bitov;
                self.bitov += 8;
            }
        }
    }

    #[inline]
    fn zahod(&mut self, n: u32) {
        self.akum >>= n;
        self.bitov -= n;
    }

    /// Prečíta `n` bitov (najviac 16).
    #[inline]
    fn bity(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.dopln();
        let v = (self.akum & ((1u64 << n) - 1)) as u32;
        self.zahod(n);
        v
    }

    /// Zahodí bity do konca bajtu a vráti polohu ďalšieho bajtu vstupu; zásobník sa vyprázdni.
    fn zarovnaj_a_vyprazdni(&mut self) -> usize {
        let n = self.bitov % 8;
        self.zahod(n);
        let p = self.precitane();
        self.akum = 0;
        self.bitov = 0;
        self.za_koncom = 0;
        self.poz = p;
        p
    }

    /// Počet skutočne prečítaných bajtov (bez celých bajtov, ktoré ešte čakajú v zásobníku).
    fn precitane(&self) -> usize {
        self.poz - (self.bitov.saturating_sub(self.za_koncom) / 8) as usize
    }

    fn over(&self) -> Vysledok<()> {
        // doplnené nuly za koncom sú na vrchu zásobníka; ak sa už spotrebovali, údaje boli neúplné
        if self.za_koncom > self.bitov {
            Err(Chyba::PoskodeneData("deflate: údaje sú neúplné".into()))
        } else {
            Ok(())
        }
    }
}

/// Rozbalí tok DEFLATE; `limit` je najväčšia dovolená dĺžka výsledku.
pub fn rozbal(data: &[u8], limit: usize) -> Vysledok<Vec<u8>> {
    let mut von = Vec::new();
    rozbal_do(data, limit, &mut von)?;
    Ok(von)
}

/// Rozbalí tok DEFLATE na koniec `von` (najviac `limit` nových bajtov). Vráti počet prečítaných bajtov vstupu.
pub fn rozbal_do(data: &[u8], limit: usize, von: &mut Vec<u8>) -> Vysledok<usize> {
    let zaciatok = von.len();
    let mut c = CitacBitov::novy(data);
    loop {
        let posledny = c.bity(1) == 1;
        match c.bity(2) {
            0 => {
                c.over()?;
                let p = c.zarovnaj_a_vyprazdni();
                if p + 4 > data.len() {
                    return Err(Chyba::PoskodeneData("deflate: uložený blok je neúplný".into()));
                }
                let dlzka = u16::from_le_bytes([data[p], data[p + 1]]) as usize;
                let doplnok = u16::from_le_bytes([data[p + 2], data[p + 3]]) as usize;
                if dlzka != !doplnok & 0xFFFF {
                    return Err(Chyba::PoskodeneData("deflate: dĺžka uloženého bloku nesedí".into()));
                }
                if von.len() - zaciatok + dlzka > limit {
                    return Err(Chyba::Limit(format!("rozbalené údaje presahujú {limit} B")));
                }
                if p + 4 + dlzka > data.len() {
                    return Err(Chyba::PoskodeneData("deflate: uložený blok je neúplný".into()));
                }
                von.extend_from_slice(&data[p + 4..p + 4 + dlzka]);
                c.poz = p + 4 + dlzka;
            }
            1 => {
                let (l, d) = pevne_dlzky();
                blok(&mut c, &Dekoder::novy(&l)?, &Dekoder::novy(&d)?, von, zaciatok, limit)?;
            }
            2 => {
                let (l, d) = dynamicke_kody(&mut c)?;
                blok(&mut c, &l, &d, von, zaciatok, limit)?;
            }
            _ => return Err(Chyba::PoskodeneData("deflate: neplatný typ bloku".into())),
        }
        c.over()?;
        if posledny {
            return Ok(c.precitane());
        }
    }
}

fn dynamicke_kody(c: &mut CitacBitov) -> Vysledok<(Dekoder, Dekoder)> {
    let hlit = c.bity(5) as usize + 257;
    let hdist = c.bity(5) as usize + 1;
    let hclen = c.bity(4) as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(Chyba::PoskodeneData("deflate: priveľa kódov v hlavičke bloku".into()));
    }
    let mut dlzky_dlzok = [0u8; 19];
    for &i in &PORADIE_DLZOK[..hclen] {
        dlzky_dlzok[i] = c.bity(3) as u8;
    }
    let kod_dlzok = Dekoder::novy(&dlzky_dlzok)?;
    let mut dlzky = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < dlzky.len() {
        let z = kod_dlzok.znak(c)?;
        c.over()?;
        let (hodnota, opakuj) = match z {
            0..=15 => (z as u8, 1),
            16 => {
                if i == 0 {
                    return Err(Chyba::PoskodeneData("deflate: opakovanie bez predchodcu".into()));
                }
                (dlzky[i - 1], 3 + c.bity(2) as usize)
            }
            17 => (0, 3 + c.bity(3) as usize),
            _ => (0, 11 + c.bity(7) as usize),
        };
        if i + opakuj > dlzky.len() {
            return Err(Chyba::PoskodeneData("deflate: opakovanie presahuje počet kódov".into()));
        }
        dlzky[i..i + opakuj].fill(hodnota);
        i += opakuj;
    }
    if dlzky[256] == 0 {
        return Err(Chyba::PoskodeneData("deflate: chýba kód konca bloku".into()));
    }
    Ok((Dekoder::novy(&dlzky[..hlit])?, Dekoder::novy(&dlzky[hlit..])?))
}

fn blok(
    c: &mut CitacBitov,
    dlzky: &Dekoder,
    vzdialenosti: &Dekoder,
    von: &mut Vec<u8>,
    zaciatok: usize,
    limit: usize,
) -> Vysledok<()> {
    loop {
        let z = dlzky.znak(c)?;
        if z < 256 {
            if von.len() - zaciatok >= limit {
                return Err(Chyba::Limit(format!("rozbalené údaje presahujú {limit} B")));
            }
            von.push(z as u8);
            continue;
        }
        if z == 256 {
            return Ok(());
        }
        let k = (z - 257) as usize;
        if k >= 29 {
            return Err(Chyba::PoskodeneData("deflate: neplatný kód dĺžky".into()));
        }
        let dlzka = ZAKLAD_DLZKY[k] as usize + c.bity(BITY_DLZKY[k] as u32) as usize;
        let kv = vzdialenosti.znak(c)? as usize;
        if kv >= 30 {
            return Err(Chyba::PoskodeneData("deflate: neplatný kód vzdialenosti".into()));
        }
        let vzd = ZAKLAD_VZDIALENOSTI[kv] as usize + c.bity(BITY_VZDIALENOSTI[kv] as u32) as usize;
        c.over()?;
        if vzd > von.len() - zaciatok {
            return Err(Chyba::PoskodeneData("deflate: vzdialenosť pred začiatkom údajov".into()));
        }
        if von.len() - zaciatok + dlzka > limit {
            return Err(Chyba::Limit(format!("rozbalené údaje presahujú {limit} B")));
        }
        let od = von.len() - vzd;
        if vzd >= dlzka {
            von.extend_from_within(od..od + dlzka);
        } else {
            // prekrývanie: opakovanie krátkeho vzoru
            for j in 0..dlzka {
                let b = von[od + j];
                von.push(b);
            }
        }
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn prazdny_ulozeny_a_pevny() {
        // uložený prázdny posledný blok
        assert_eq!(rozbal(&[0x01, 0x00, 0x00, 0xFF, 0xFF], 10).unwrap(), b"");
        // pevný blok s jediným znakom „a“
        assert_eq!(rozbal(&[0x4B, 0x04, 0x00], 10).unwrap(), b"a");
    }

    #[test]
    fn neplatne_kody() {
        assert!(Dekoder::novy(&[1, 1, 1]).is_err());
        assert!(Dekoder::novy(&[2, 2, 2]).is_err());
        assert!(Dekoder::novy(&[1]).is_ok());
        assert!(rozbal(&[0x07], 10).is_err());
    }
}
