//! Kodér DEFLATE – LZ77 s reťazou rozptylovej tabuľky, odložený výber a bloky s vlastnými kódmi.

use super::{BITY_DLZKY, BITY_VZDIALENOSTI, PORADIE_DLZOK, ZAKLAD_DLZKY, ZAKLAD_VZDIALENOSTI, pevne_dlzky};
use crate::huffman::{dlzky_kodov, kanonicke_kody, obrat_bity};

const OKNO: usize = 1 << 15;
const NAJMENSIA_ZHODA: usize = 3;
const NAJVACSIA_ZHODA: usize = 258;
const ROZPTYL_BITY: u32 = 15;
/// Počet znakov (literál alebo dvojica dĺžka–vzdialenosť) v jednom bloku.
const ZNAKOV_V_BLOKU: usize = 1 << 15;

/// Nastavenie hľadania podľa úrovne: (najviac krokov reťaze, dosť dobrá dĺžka, odložený výber do dĺžky).
fn nastavenie(uroven: u8) -> (usize, usize, usize) {
    match uroven {
        1 => (4, 8, 0),
        2 => (8, 16, 0),
        3 => (16, 32, 0),
        4 => (16, 32, 8),
        5 => (32, 64, 16),
        6 => (128, 128, 16),
        7 => (256, 128, 32),
        8 => (1024, 258, 128),
        _ => (4096, 258, 258),
    }
}

/// Zapisovač bitov od najnižšieho (poradie DEFLATE).
struct Bity {
    von: Vec<u8>,
    akum: u64,
    pocet: u32,
}

impl Bity {
    fn zapis(&mut self, hodnota: u32, bitov: u32) {
        self.akum |= (hodnota as u64) << self.pocet;
        self.pocet += bitov;
        while self.pocet >= 8 {
            self.von.push(self.akum as u8);
            self.akum >>= 8;
            self.pocet -= 8;
        }
    }

    fn zarovnaj(&mut self) {
        if self.pocet > 0 {
            self.von.push(self.akum as u8);
            self.akum = 0;
            self.pocet = 0;
        }
    }
}

/// Kód dĺžky 3–258 (index do tabuliek 0–28).
fn kod_dlzky(d: usize) -> usize {
    match ZAKLAD_DLZKY.binary_search(&(d as u16)) {
        Ok(i) => i,
        Err(i) => i - 1,
    }
}

/// Kód vzdialenosti 1–32768.
fn kod_vzdialenosti(v: usize) -> usize {
    match ZAKLAD_VZDIALENOSTI.binary_search(&(v as u16)) {
        Ok(i) => i,
        Err(i) => i - 1,
    }
}

/// Znak bloku: literál (`vzd == 0`) alebo zhoda.
#[derive(Clone, Copy)]
struct Znak {
    dlzka: u16,
    vzd: u16,
}

/// Zbalí údaje do surového toku DEFLATE (bez obalu zlib) s úrovňou 0–9.
pub fn zbal(data: &[u8], uroven: u8) -> Vec<u8> {
    let mut b = Bity { von: Vec::with_capacity(data.len() / 2 + 64), akum: 0, pocet: 0 };
    if uroven == 0 || data.is_empty() {
        ulozene_bloky(&mut b, data, true);
        return b.von;
    }
    let (max_krokov, dost_dobra, odlozit_do) = nastavenie(uroven);
    let mut hlava = vec![u32::MAX; 1 << ROZPTYL_BITY];
    let mut predchodca = vec![u32::MAX; OKNO];
    let rozptyl = |i: usize| -> usize {
        let x = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
        (x.wrapping_mul(0x9E37_79B1) >> (32 - ROZPTYL_BITY)) as usize
    };
    let vloz = |i: usize, hlava: &mut Vec<u32>, predchodca: &mut Vec<u32>| {
        if i + NAJMENSIA_ZHODA <= data.len() {
            let h = rozptyl(i);
            predchodca[i % OKNO] = hlava[h];
            hlava[h] = i as u32;
        }
    };
    // najlepšia zhoda na pozícii i
    let najdi = |i: usize, hlava: &Vec<u32>, predchodca: &Vec<u32>, min: usize| -> (usize, usize) {
        if i + NAJMENSIA_ZHODA > data.len() {
            return (0, 0);
        }
        let najviac = NAJVACSIA_ZHODA.min(data.len() - i);
        let (mut najlepsia, mut vzd) = (min.max(NAJMENSIA_ZHODA - 1), 0);
        if najlepsia >= najviac {
            return (0, 0);
        }
        let mut j = hlava[rozptyl(i)];
        let mut krokov = 0;
        while j != u32::MAX && krokov < max_krokov {
            let j0 = j as usize;
            if j0 >= i || i - j0 > OKNO {
                break;
            }
            // rýchla kontrola: posledný bajt možnej lepšej zhody
            if data[j0 + najlepsia] == data[i + najlepsia] && data[j0] == data[i] {
                let mut l = 0;
                while l < najviac && data[j0 + l] == data[i + l] {
                    l += 1;
                }
                if l > najlepsia {
                    najlepsia = l;
                    vzd = i - j0;
                    if l >= dost_dobra || l == najviac {
                        break;
                    }
                }
            }
            let p = predchodca[j0 % OKNO];
            if p == u32::MAX || p as usize >= j0 {
                break;
            }
            j = p;
            krokov += 1;
        }
        if vzd == 0 { (0, 0) } else { (najlepsia, vzd) }
    };

    let mut znaky: Vec<Znak> = Vec::with_capacity(ZNAKOV_V_BLOKU + 2);
    let mut zaciatok_bloku = 0usize;
    let mut i = 0usize;
    // odložená zhoda z predchádzajúcej pozície
    let mut cakajuca: Option<(usize, usize)> = None;
    while i < data.len() {
        let (l, v) = najdi(i, &hlava, &predchodca, cakajuca.map_or(0, |c| c.0));
        match cakajuca.take() {
            Some((pl, pv)) if l <= pl => {
                // predchádzajúca zhoda je lepšia – použije sa od pozície i - 1
                znaky.push(Znak { dlzka: pl as u16, vzd: pv as u16 });
                for k in i..i - 1 + pl {
                    vloz(k, &mut hlava, &mut predchodca);
                }
                i = i - 1 + pl;
            }
            predosla => {
                if predosla.is_some() {
                    znaky.push(Znak { dlzka: data[i - 1] as u16, vzd: 0 });
                }
                vloz(i, &mut hlava, &mut predchodca);
                if l >= NAJMENSIA_ZHODA && l < odlozit_do && i + 1 < data.len() {
                    cakajuca = Some((l, v));
                    i += 1;
                } else if l >= NAJMENSIA_ZHODA {
                    znaky.push(Znak { dlzka: l as u16, vzd: v as u16 });
                    for k in i + 1..i + l {
                        vloz(k, &mut hlava, &mut predchodca);
                    }
                    i += l;
                } else {
                    znaky.push(Znak { dlzka: data[i] as u16, vzd: 0 });
                    i += 1;
                }
            }
        }
        if znaky.len() >= ZNAKOV_V_BLOKU && cakajuca.is_none() {
            zapis_blok(&mut b, &znaky, &data[zaciatok_bloku..i], false);
            znaky.clear();
            zaciatok_bloku = i;
        }
    }
    // odložená zhoda sa vždy vyrieši ešte v cykle (odkladá sa len pred posledným bajtom)
    debug_assert!(cakajuca.is_none());
    zapis_blok(&mut b, &znaky, &data[zaciatok_bloku..], true);
    b.zarovnaj();
    b.von
}

/// Uložené bloky (bez kompresie), po najviac 65 535 bajtov.
fn ulozene_bloky(b: &mut Bity, data: &[u8], posledny: bool) {
    let kusy: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(65_535).collect() };
    let n = kusy.len();
    for (k, kus) in kusy.into_iter().enumerate() {
        b.zapis((posledny && k + 1 == n) as u32, 1);
        b.zapis(0, 2);
        b.zarovnaj();
        let l = kus.len() as u16;
        b.von.extend_from_slice(&l.to_le_bytes());
        b.von.extend_from_slice(&(!l).to_le_bytes());
        b.von.extend_from_slice(kus);
    }
}

/// Zapíše blok spôsobom, ktorý dá najmenej bitov: uložený, s pevnými alebo s vlastnými kódmi.
fn zapis_blok(b: &mut Bity, znaky: &[Znak], surove: &[u8], posledny: bool) {
    let mut pocty_l = [0u32; 286];
    let mut pocty_v = [0u32; 30];
    let mut doplnkove_bity = 0u64;
    for z in znaky {
        if z.vzd == 0 {
            pocty_l[z.dlzka as usize] += 1;
        } else {
            let kl = kod_dlzky(z.dlzka as usize);
            let kv = kod_vzdialenosti(z.vzd as usize);
            pocty_l[257 + kl] += 1;
            pocty_v[kv] += 1;
            doplnkove_bity += BITY_DLZKY[kl] as u64 + BITY_VZDIALENOSTI[kv] as u64;
        }
    }
    pocty_l[256] = 1;
    let cena = |dl: &[u8], dv: &[u8]| -> u64 {
        let a: u64 = pocty_l.iter().zip(dl).map(|(&p, &d)| p as u64 * d as u64).sum();
        let c: u64 = pocty_v.iter().zip(dv).map(|(&p, &d)| p as u64 * d as u64).sum();
        a + c + doplnkove_bity
    };
    // vlastné kódy
    let dl = dlzky_kodov(&pocty_l, 15);
    let mut dv = dlzky_kodov(&pocty_v, 15);
    if dv.iter().all(|&x| x == 0) {
        dv[0] = 1; // aspoň jeden kód vzdialenosti (norma)
    }
    let hlit = 257.max(dl.iter().rposition(|&x| x > 0).map_or(0, |p| p + 1));
    let hdist = 1.max(dv.iter().rposition(|&x| x > 0).map_or(0, |p| p + 1));
    let (rle, pocty_c) = kod_dlzok_rle(&dl[..hlit], &dv[..hdist]);
    let dc = dlzky_kodov(&pocty_c, 7);
    let hclen = 4.max(PORADIE_DLZOK.iter().rposition(|&i| dc[i] > 0).map_or(0, |p| p + 1));
    let hlavicka: u64 =
        14 + 3 * hclen as u64 + rle.iter().map(|&(z, _)| dc[z as usize] as u64 + doplnok_rle(z)).sum::<u64>();
    let dynamicky = 3 + hlavicka + cena(&dl, &dv);
    let (pl, pv) = pevne_dlzky();
    let pevny = 3 + cena(&pl[..286], &pv);
    let ulozeny = surove.len() as u64 * 8 + 40 * surove.len().div_ceil(65_535).max(1) as u64 + 8;
    if ulozeny <= pevny.min(dynamicky) {
        ulozene_bloky(b, surove, posledny);
        return;
    }
    b.zapis(posledny as u32, 1);
    if pevny <= dynamicky {
        b.zapis(1, 2);
        zapis_znaky(b, znaky, &pl, &pv);
    } else {
        b.zapis(2, 2);
        b.zapis((hlit - 257) as u32, 5);
        b.zapis((hdist - 1) as u32, 5);
        b.zapis((hclen - 4) as u32, 4);
        for &i in &PORADIE_DLZOK[..hclen] {
            b.zapis(dc[i] as u32, 3);
        }
        let kc = obratene_kody(&dc);
        for &(z, extra) in &rle {
            b.zapis(kc[z as usize], dc[z as usize] as u32);
            match z {
                16 => b.zapis(extra as u32, 2),
                17 => b.zapis(extra as u32, 3),
                18 => b.zapis(extra as u32, 7),
                _ => {}
            }
        }
        zapis_znaky(b, znaky, &dl, &dv);
    }
}

fn doplnok_rle(z: u8) -> u64 {
    match z {
        16 => 2,
        17 => 3,
        18 => 7,
        _ => 0,
    }
}

/// Kódy s obráteným poradím bitov (DEFLATE posiela Huffmanove kódy od najvyššieho bitu kódu).
fn obratene_kody(dlzky: &[u8]) -> Vec<u32> {
    kanonicke_kody(dlzky).iter().zip(dlzky).map(|(&k, &l)| obrat_bity(k, l as u32)).collect()
}

fn zapis_znaky(b: &mut Bity, znaky: &[Znak], dl: &[u8], dv: &[u8]) {
    let kl = obratene_kody(dl);
    let kv = obratene_kody(dv);
    for z in znaky {
        if z.vzd == 0 {
            b.zapis(kl[z.dlzka as usize], dl[z.dlzka as usize] as u32);
        } else {
            let k = kod_dlzky(z.dlzka as usize);
            b.zapis(kl[257 + k], dl[257 + k] as u32);
            b.zapis(z.dlzka as u32 - ZAKLAD_DLZKY[k] as u32, BITY_DLZKY[k] as u32);
            let k = kod_vzdialenosti(z.vzd as usize);
            b.zapis(kv[k], dv[k] as u32);
            b.zapis(z.vzd as u32 - ZAKLAD_VZDIALENOSTI[k] as u32, BITY_VZDIALENOSTI[k] as u32);
        }
    }
    b.zapis(kl[256], dl[256] as u32);
}

/// Dĺžky kódov oboch tabuliek ako jedna postupnosť so skratkami 16 (opakuj predchádzajúcu), 17 a 18 (nuly).
fn kod_dlzok_rle(dl: &[u8], dv: &[u8]) -> (Vec<(u8, u8)>, [u32; 19]) {
    let vsetky: Vec<u8> = dl.iter().chain(dv).copied().collect();
    let mut von = Vec::new();
    let mut pocty = [0u32; 19];
    let mut i = 0;
    while i < vsetky.len() {
        let x = vsetky[i];
        let mut n = 1;
        while i + n < vsetky.len() && vsetky[i + n] == x {
            n += 1;
        }
        let mut zostava = n;
        if x == 0 {
            while zostava >= 11 {
                let k = zostava.min(138);
                von.push((18, (k - 11) as u8));
                zostava -= k;
            }
            if zostava >= 3 {
                von.push((17, (zostava - 3) as u8));
                zostava = 0;
            }
        } else {
            von.push((x, 0));
            zostava -= 1;
            while zostava >= 3 {
                let k = zostava.min(6);
                von.push((16, (k - 3) as u8));
                zostava -= k;
            }
        }
        for _ in 0..zostava {
            von.push((x, 0));
        }
        i += n;
    }
    for &(z, _) in &von {
        pocty[z as usize] += 1;
    }
    (von, pocty)
}
