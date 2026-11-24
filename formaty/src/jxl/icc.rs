//! Zabalený profil ICC v JPEG XL (ISO/IEC 18181-1, príloha E.4): entropicky kódované bajty (41 kontextov)
//! a ich rozbalenie – predpoveď hlavičky, zoznam značiek (tagov) a príkazy pre dáta značiek.

use super::bity::{Citac, chyba};
use super::entropia::Histogramy;
use orchidea_jadro::Vysledok;

/// Najväčší prijatý profil (zakódovaný aj výsledný).
const NAJVIAC: u64 = 1 << 28;

/// Kontext bajtu podľa polohy a dvoch predchádzajúcich bajtov.
fn kontext(i: usize, b1: u8, b2: u8) -> usize {
    if i <= 128 {
        return 0;
    }
    let p1 = match b1 {
        b'a'..=b'z' | b'A'..=b'Z' => 0,
        b'0'..=b'9' | b'.' | b',' => 1,
        0 | 1 => 2 + b1 as usize,
        2..=15 => 4,
        241..=254 => 5,
        255 => 6,
        _ => 7,
    };
    let p2 = match b2 {
        b'a'..=b'z' | b'A'..=b'Z' => 0,
        b'0'..=b'9' | b'.' | b',' => 1,
        0..=15 => 2,
        241..=255 => 3,
        _ => 4,
    };
    1 + p1 + 8 * p2
}

/// Prečíta zakódovaný profil z kódového toku a rozbalí ho.
pub(super) fn citaj(c: &mut Citac, limit: u64) -> Vysledok<Vec<u8>> {
    let velkost = c.u64();
    if velkost > NAJVIAC.min(limit) {
        return Err(chyba("priveľký profil ICC"));
    }
    let h = Histogramy::citaj(c, 41)?;
    let mut t = h.citac(c);
    let mut zak = Vec::with_capacity((velkost as usize).min(1 << 16));
    let (mut b1, mut b2) = (0u8, 0u8);
    for i in 0..velkost as usize {
        let v = t.citaj(&h, c, kontext(i, b1, b2))?;
        if v > 255 {
            return Err(chyba("bajt profilu ICC mimo rozsahu"));
        }
        b2 = b1;
        b1 = v as u8;
        zak.push(b1);
        if i == 17 || i + 1 == velkost as usize {
            // skorá kontrola veľkosti výsledku, kým sa nečíta celý tok
            let mut r = Bajty { d: &zak, p: 0 };
            if let (Ok(vystup), Ok(_)) = (r.varint(), r.varint())
                && (vystup > NAJVIAC || vystup + 65_536 < velkost)
            {
                return Err(chyba("nezmyselná veľkosť profilu ICC"));
            }
        }
        if c.za_koncom() {
            return Err(chyba("profil ICC presahuje údaje"));
        }
    }
    t.koniec(&h)?;
    rozbal(&zak)
}

/// Čítanie bajtov s premenlivou dĺžkou čísel (7 bitov na bajt).
struct Bajty<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Bajty<'a> {
    fn bajt(&mut self) -> Option<u8> {
        let b = *self.d.get(self.p)?;
        self.p += 1;
        Some(b)
    }

    fn varint(&mut self) -> Vysledok<u64> {
        let mut v = 0u64;
        let mut posun = 0;
        while posun < 63 {
            let b = self.bajt().ok_or_else(|| chyba("profil ICC je skrátený"))?;
            v |= ((b & 0x7F) as u64) << posun;
            if b & 0x80 == 0 {
                break;
            }
            posun += 7;
        }
        Ok(v)
    }

    fn kus(&mut self, n: usize) -> Vysledok<&'a [u8]> {
        if n > self.d.len() - self.p {
            return Err(chyba("profil ICC je skrátený"));
        }
        let k = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(k)
    }
}

/// Predpovedaný bajt hlavičky na pozícii `i`.
fn predpoved(i: usize, velkost: u32, h: &[u8]) -> u8 {
    let pod = |a: u8, b: u8| h.get(40) == Some(&a) && (b == 0 || h.get(41) == Some(&b));
    match i {
        0..=3 => velkost.to_be_bytes()[i],
        8 => 4,
        12..=23 => b"mntrRGB XYZ "[i - 12],
        36..=39 => b"acsp"[i - 36],
        41 | 42 if pod(b'A', 0) => b'P',
        43 if pod(b'A', 0) => b'L',
        41 if pod(b'M', 0) => b'S',
        42 if pod(b'M', 0) => b'F',
        43 if pod(b'M', 0) => b'T',
        42 if pod(b'S', b'G') => b'I',
        43 if pod(b'S', b'G') => b' ',
        42 if pod(b'S', b'U') => b'N',
        43 if pod(b'S', b'U') => b'W',
        70 => 246,
        71 => 214,
        73 => 1,
        78 => 211,
        79 => 45,
        80..=83 => h[4 + i - 80],
        _ => 0,
    }
}

/// Transpozícia: vstup sú stĺpce matice so šírkou `w` (prvé stĺpce o prvok dlhšie), výstup po riadkoch.
fn premiesaj(b: &[u8], w: usize) -> Vec<u8> {
    let n = b.len();
    let (zakl, dlhsich) = (n / w, n % w);
    let zaciatok = |j: usize| j * zakl + j.min(dlhsich);
    let mut v = Vec::with_capacity(n);
    for r in 0..zakl + (dlhsich > 0) as usize {
        for j in 0..w {
            let dlzka = zakl + (j < dlhsich) as usize;
            if r < dlzka {
                v.push(b[zaciatok(j) + r]);
            }
        }
    }
    v
}

const ZNACKY: [&[u8; 4]; 19] = [
    b"rTRC", b"rXYZ", b"cprt", b"wtpt", b"bkpt", b"rXYZ", b"gXYZ", b"bXYZ", b"kXYZ", b"rTRC", b"gTRC",
    b"bTRC", b"kTRC", b"chad", b"desc", b"chrm", b"dmnd", b"dmdd", b"lumi",
];
const TYPY: [&[u8; 4]; 8] = [b"XYZ ", b"desc", b"text", b"mluc", b"para", b"curv", b"sf32", b"gbd "];

/// Rozbalí zakódovaný profil na pôvodné bajty ICC.
pub(super) fn rozbal(zak: &[u8]) -> Vysledok<Vec<u8>> {
    let mut r = Bajty { d: zak, p: 0 };
    let velkost = r.varint()?;
    let dlzka_prikazov = r.varint()?;
    if velkost > NAJVIAC || dlzka_prikazov > (zak.len() - r.p) as u64 {
        return Err(chyba("neplatné veľkosti v profile ICC"));
    }
    let prikazy_d = r.kus(dlzka_prikazov as usize)?;
    let mut prikazy = Bajty { d: prikazy_d, p: 0 };
    let mut data = Bajty { d: &zak[r.p..], p: 0 };
    let mut von: Vec<u8> = Vec::with_capacity(velkost as usize);
    // hlavička
    let hd = data.kus((velkost as usize).min(128))?;
    for (i, &e) in hd.iter().enumerate() {
        von.push(predpoved(i, velkost as u32, hd).wrapping_add(e));
    }
    if velkost <= 128 {
        return Ok(von);
    }
    // zoznam značiek
    let n = prikazy.varint()?;
    if n > 0 {
        let pocet = n - 1;
        if (velkost - 128) / 12 < pocet {
            return Err(chyba("priveľa značiek v profile ICC"));
        }
        von.extend_from_slice(&(pocet as u32).to_be_bytes());
        let mut pred_zac = pocet as u32 * 12 + 128;
        let mut pred_vel = 0u32;
        loop {
            let Some(prikaz) = prikazy.bajt() else {
                return Ok(von);
            };
            let kod = prikaz & 63;
            let znacka: [u8; 4] = match kod {
                0 => break,
                1 => data.kus(4)?.try_into().map_err(|_| chyba("značka ICC"))?,
                2..=20 => *ZNACKY[kod as usize - 2],
                _ => return Err(chyba("neplatný kód značky ICC")),
            };
            let zac =
                if prikaz & 64 == 0 { pred_zac.wrapping_add(pred_vel) } else { prikazy.varint()? as u32 };
            let vel = if prikaz & 128 != 0 {
                prikazy.varint()? as u32
            } else if matches!(&znacka, b"rXYZ" | b"gXYZ" | b"bXYZ" | b"kXYZ" | b"wtpt" | b"bkpt" | b"lumi") {
                20
            } else {
                pred_vel
            };
            if zac as u64 + vel as u64 > velkost {
                return Err(chyba("značka ICC mimo profilu"));
            }
            pred_zac = zac;
            pred_vel = vel;
            let mut zapis = |z: &[u8], a: u32| {
                von.extend_from_slice(z);
                von.extend_from_slice(&a.to_be_bytes());
                von.extend_from_slice(&vel.to_be_bytes());
            };
            zapis(&znacka, zac);
            if kod == 2 {
                zapis(b"gTRC", zac);
                zapis(b"bTRC", zac);
            } else if kod == 3 {
                zapis(b"gXYZ", zac.wrapping_add(vel));
                zapis(b"bXYZ", zac.wrapping_add(vel.wrapping_mul(2)));
            }
        }
    }
    // dáta značiek
    while let Some(prikaz) = prikazy.bajt() {
        match prikaz {
            1 => {
                let n = prikazy.varint()? as usize;
                von.extend_from_slice(data.kus(n)?);
            }
            2 | 3 => {
                let n = prikazy.varint()? as usize;
                let w = if prikaz == 2 { 2 } else { 4 };
                von.extend_from_slice(&premiesaj(data.kus(n)?, w));
            }
            4 => {
                let priznaky = prikazy.bajt().ok_or_else(|| chyba("profil ICC je skrátený"))?;
                let sirka = (priznaky & 3) as usize + 1;
                let rad = ((priznaky >> 2) & 3) as usize;
                if sirka == 3 || rad == 3 {
                    return Err(chyba("neplatná predpoveď v profile ICC"));
                }
                let krok = if priznaky & 16 != 0 { prikazy.varint()? as usize } else { sirka };
                if krok < sirka || krok.saturating_mul(4) >= von.len() {
                    return Err(chyba("neplatný krok predpovede v profile ICC"));
                }
                let n = prikazy.varint()? as usize;
                let surove = data.kus(n)?;
                let b = if sirka == 1 { surove.to_vec() } else { premiesaj(surove, sirka) };
                let mut i = 0;
                while i < n {
                    let mut pred = [0u32; 3];
                    for (j, p) in pred.iter_mut().enumerate().take(rad + 1) {
                        let z = von.len() - krok * (j + 1);
                        let mut x = [0u8; 4];
                        x[4 - sirka..].copy_from_slice(&von[z..z + sirka]);
                        *p = u32::from_be_bytes(x);
                    }
                    let p = match rad {
                        0 => pred[0],
                        1 => pred[0].wrapping_mul(2).wrapping_sub(pred[1]),
                        _ => pred[0].wrapping_sub(pred[1]).wrapping_mul(3).wrapping_add(pred[2]),
                    };
                    for j in 0..sirka.min(n - i) {
                        von.push((b[i + j] as u32).wrapping_add(p >> (8 * (sirka - 1 - j))) as u8);
                    }
                    i += sirka;
                }
            }
            10 => {
                von.extend_from_slice(&[b'X', b'Y', b'Z', b' ', 0, 0, 0, 0]);
                von.extend_from_slice(data.kus(12)?);
            }
            16..=23 => {
                von.extend_from_slice(TYPY[prikaz as usize - 16]);
                von.extend_from_slice(&[0; 4]);
            }
            _ => return Err(chyba("neplatný príkaz v profile ICC")),
        }
        if von.len() as u64 > velkost {
            return Err(chyba("profil ICC je dlhší, ako hlási"));
        }
    }
    if von.len() as u64 != velkost {
        return Err(chyba("profil ICC má inú dĺžku, ako hlási"));
    }
    Ok(von)
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn premiesanie_je_transpozicia() {
        assert_eq!(premiesaj(&[1, 2, 3, 4, 5], 2), vec![1, 4, 2, 5, 3]);
        // stĺpce: [0,1] [2,3] [4] [5] (n = 6, w = 4 → dva dlhšie stĺpce)
        assert_eq!(premiesaj(&[0, 1, 2, 3, 4, 5], 4), vec![0, 2, 4, 5, 1, 3]);
        assert_eq!(premiesaj(&[7, 8, 9], 4), vec![7, 8, 9]);
    }

    #[test]
    fn kratka_hlavicka() {
        // výsledná veľkosť 4, žiadne príkazy, 4 bajty hlavičky (rozdiel voči predpovedi = 0)
        let p = rozbal(&[4, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(p, vec![0, 0, 0, 4]);
    }
}
