//! Dekodér JPEG – rozbor značiek, Huffmanovo dekódovanie, progresívne skeny, IDCT, prevzorkovanie, farba.

use super::dct;
use super::{CIK_CAK, STD_AC_FAREBNOST, STD_AC_JAS, STD_DC_FAREBNOST, STD_DC_JAS};
use crate::obraz::{Limity, Metadata, Model, Obraz, Vzorky};
use orchidea_jadro::{Chyba, Vysledok, paralel};

fn chyba(text: &str) -> Chyba {
    Chyba::PoskodeneData(format!("JPEG: {text}"))
}

/// Údaje z hlavičky snímky (SOF) – na prehliadanie bez dekódovania.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hlavicka {
    pub sirka: usize,
    pub vyska: usize,
    pub zloziek: usize,
    pub presnost: u8,
    pub progresivny: bool,
}

/// Huffmanova tabuľka dekodéra: rýchla tabuľka pre kódy do 9 bitov, dlhšie kódy kanonicky po dĺžkach.
#[derive(Clone)]
struct Huffman {
    rychla: Vec<u16>,
    max_kod: [i32; 18],
    posun: [i32; 17],
    hodnoty: Vec<u8>,
}

const RYCHLE_BITY: u32 = 9;

impl Huffman {
    fn nova(pocty: &[u8; 16], hodnoty: &[u8]) -> Vysledok<Huffman> {
        let spolu: usize = pocty.iter().map(|&x| x as usize).sum();
        if spolu != hodnoty.len() || spolu > 256 {
            return Err(chyba("Huffmanova tabuľka nesedí"));
        }
        let mut h = Huffman {
            rychla: vec![0u16; 1 << RYCHLE_BITY],
            max_kod: [-1; 18],
            posun: [0; 17],
            hodnoty: hodnoty.to_vec(),
        };
        let (mut kod, mut k) = (0i32, 0usize);
        for l in 1..=16usize {
            let n = pocty[l - 1] as usize;
            h.posun[l] = k as i32 - kod;
            if n > 0 {
                for _ in 0..n {
                    if kod >= 1 << l {
                        return Err(chyba("prečerpaný Huffmanov kód"));
                    }
                    if l as u32 <= RYCHLE_BITY {
                        let zaciatok = (kod as usize) << (RYCHLE_BITY as usize - l);
                        let koniec = ((kod as usize) + 1) << (RYCHLE_BITY as usize - l);
                        for e in &mut h.rychla[zaciatok..koniec] {
                            *e = (l as u16) << 8 | hodnoty[k] as u16;
                        }
                    }
                    kod += 1;
                    k += 1;
                }
                h.max_kod[l] = kod - 1;
            }
            kod <<= 1;
        }
        h.max_kod[17] = i32::MAX;
        Ok(h)
    }
}

/// Čítač entropicky kódovaných údajov: bity od najvyššieho, vynechá vložené nuly za 0xFF, zastaví na značke.
/// Za značkou alebo koncom dopĺňa nuly; `pretecenie` počíta spotrebované doplnené bity (neúplný súbor).
struct Citac<'a> {
    data: &'a [u8],
    poz: usize,
    akum: u64,
    bitov: u32,
    /// Koľko z `bitov` sú skutočné údaje (sú navrchu zásobníka).
    skutocnych: u32,
    znacka: Option<u8>,
    pretecenie: u32,
}

impl<'a> Citac<'a> {
    fn novy(data: &'a [u8], poz: usize) -> Citac<'a> {
        Citac { data, poz, akum: 0, bitov: 0, skutocnych: 0, znacka: None, pretecenie: 0 }
    }

    #[inline]
    fn dopln(&mut self) {
        while self.bitov <= 56 {
            let mut b = 0u8;
            let mut skutocny = false;
            if self.znacka.is_none() && self.poz < self.data.len() {
                b = self.data[self.poz];
                if b == 0xFF {
                    let mut n = self.poz + 1;
                    while n < self.data.len() && self.data[n] == 0xFF {
                        n += 1; // výplň 0xFF pred značkou
                    }
                    match self.data.get(n) {
                        Some(0) => {
                            self.poz = n + 1;
                            skutocny = true;
                        }
                        Some(&z) => {
                            self.znacka = Some(z);
                            self.poz = n - 1;
                            b = 0;
                        }
                        None => {
                            self.poz = self.data.len();
                            b = 0;
                        }
                    }
                } else {
                    self.poz += 1;
                    skutocny = true;
                }
            }
            self.akum |= (b as u64) << (56 - self.bitov);
            self.bitov += 8;
            if skutocny {
                self.skutocnych += 8;
            }
        }
    }

    #[inline]
    fn zahod(&mut self, n: u32) {
        self.akum <<= n;
        self.bitov -= n;
        if n <= self.skutocnych {
            self.skutocnych -= n;
        } else {
            self.pretecenie += n - self.skutocnych;
            self.skutocnych = 0;
        }
    }

    #[inline]
    fn bity(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.bitov < n {
            self.dopln();
        }
        let v = (self.akum >> (64 - n)) as u32;
        self.zahod(n);
        v
    }

    #[inline]
    fn bit(&mut self) -> u32 {
        self.bity(1)
    }

    /// Prečíta `s` bitov a rozšíri na číslo so znamienkom (F.2.2.1 normy).
    #[inline]
    fn rozsir(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bity(s) as i32;
        if v < 1 << (s - 1) { v - (1 << s) + 1 } else { v }
    }

    #[inline]
    fn znak(&mut self, h: &Huffman) -> Vysledok<u8> {
        if self.bitov < 16 {
            self.dopln();
        }
        let e = h.rychla[(self.akum >> (64 - RYCHLE_BITY)) as usize];
        if e != 0 {
            self.zahod((e >> 8) as u32);
            return Ok(e as u8);
        }
        let mut kod = 0i32;
        for l in 1..=16usize {
            kod = (kod << 1) | self.bit() as i32;
            if kod <= h.max_kod[l] {
                let i = (kod + h.posun[l]) as usize;
                return h.hodnoty.get(i).copied().ok_or_else(|| chyba("neplatný Huffmanov kód"));
            }
        }
        Err(chyba("neplatný Huffmanov kód"))
    }

    /// Údaje sa skončili skôr, než sken (alebo interval reštartu) – súbor je neúplný alebo poškodený.
    fn prebehol(&self) -> bool {
        self.pretecenie > 0
    }

    /// Reštart: zahodí zvyšok bajtu a prejde značku RSTn.
    fn restart(&mut self) -> Vysledok<()> {
        self.akum = 0;
        self.bitov = 0;
        self.skutocnych = 0;
        self.pretecenie = 0;
        match self.znacka {
            Some(z) if (0xD0..=0xD7).contains(&z) => {
                self.znacka = None;
                self.poz += 2;
                Ok(())
            }
            None => {
                // značka chýba alebo je posunutá – nájdi ďalšiu RSTn (odolnosť voči poškodeniu)
                while self.poz + 1 < self.data.len() {
                    if self.data[self.poz] == 0xFF && (0xD0..=0xD7).contains(&self.data[self.poz + 1]) {
                        self.poz += 2;
                        return Ok(());
                    }
                    self.poz += 1;
                }
                Err(chyba("chýba značka reštartu"))
            }
            Some(_) => Err(chyba("neočakávaná značka namiesto reštartu")),
        }
    }
}

#[derive(Clone)]
struct Zlozka {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Bloky vrátane doplnenia na celé MCU.
    bw: usize,
    bh: usize,
    /// Bloky so skutočnými údajmi (pre neprekladané skeny).
    bw_ucinne: usize,
    bh_ucinne: usize,
    koef: Vec<i16>,
    kvant: Option<[u16; 64]>,
    dc: i32,
}

struct Snimka {
    sirka: usize,
    vyska: usize,
    presnost: u8,
    progresivny: bool,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
    zlozky: Vec<Zlozka>,
}

/// Stav rozboru súboru.
struct Stav {
    kvant: [Option<[u16; 64]>; 4],
    dc: [Option<Huffman>; 4],
    ac: [Option<Huffman>; 4],
    restart: usize,
    adobe: Option<u8>,
    jfif: bool,
    snimka: Option<Snimka>,
    icc: Vec<(u8, Vec<u8>)>,
    meta: Metadata,
    metadat: u64,
    eob: u32,
    /// Segmenty APPn a COM v pôvodnom poradí.
    segmenty: Vec<(u8, Vec<u8>)>,
}

fn be16(d: &[u8], i: usize) -> Vysledok<usize> {
    d.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize).ok_or_else(|| chyba("neúplný súbor"))
}

/// Prečíta len hlavičku snímky.
pub fn citaj_hlavicku(data: &[u8]) -> Vysledok<Hlavicka> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(chyba("chýba značka začiatku (SOI)"));
    }
    let mut poz = 2;
    loop {
        while poz < data.len() && data[poz] != 0xFF {
            poz += 1;
        }
        while poz < data.len() && data[poz] == 0xFF {
            poz += 1;
        }
        let z = *data.get(poz).ok_or_else(|| chyba("chýba hlavička snímky"))?;
        poz += 1;
        if z == 0xD8 || (0xD0..=0xD7).contains(&z) || z == 0x01 {
            continue;
        }
        let dlzka = be16(data, poz)?;
        if matches!(z, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let s = data.get(poz + 2..poz + dlzka).ok_or_else(|| chyba("neúplná hlavička snímky"))?;
            if s.len() < 6 {
                return Err(chyba("krátka hlavička snímky"));
            }
            return Ok(Hlavicka {
                sirka: u16::from_be_bytes([s[3], s[4]]) as usize,
                vyska: u16::from_be_bytes([s[1], s[2]]) as usize,
                zloziek: s[5] as usize,
                presnost: s[0],
                progresivny: matches!(z, 0xC2 | 0xC6 | 0xCA | 0xCE),
            });
        }
        poz += dlzka;
    }
}

/// Prečíta obrázok JPEG v plnej veľkosti.
pub fn citaj(data: &[u8], limity: &Limity) -> Vysledok<Obraz> {
    dekoduj(data, limity, 8)
}

/// Prečíta obrázok zmenšený v pomere `1 : mierka` (1, 2, 4 alebo 8) – rýchly náhľad bez plnej IDCT.
pub fn citaj_zmensene(data: &[u8], limity: &Limity, mierka: usize) -> Vysledok<Obraz> {
    let n = match mierka {
        1 => 8,
        2 => 4,
        4 => 2,
        8 => 1,
        _ => return Err(Chyba::MimoRozsahu(format!("mierka JPEG {mierka} (1, 2, 4, 8)"))),
    };
    dekoduj(data, limity, n)
}

fn dekoduj(data: &[u8], limity: &Limity, n: usize) -> Vysledok<Obraz> {
    let (snimka, mut s) = rozober(data, limity)?;
    let mut o = zostav(&snimka, &s, n)?;
    if !s.icc.is_empty() {
        s.icc.sort_by_key(|c| c.0);
        o.farba.icc = Some(s.icc.iter().flat_map(|c| c.1.iter().copied()).collect());
    }
    if let Some(e) = &s.meta.exif {
        s.meta.natocenie = crate::exif::natocenie(e).unwrap_or(1);
    }
    o.metadata = s.meta;
    Ok(o)
}

/// Kvantizované koeficienty jednej zložky (bloky po 64 v prirodzenom poradí, riadok bloku po riadku).
#[derive(Debug, Clone)]
pub struct ZlozkaKoef {
    pub id: u8,
    pub h: usize,
    pub v: usize,
    pub kvant: [u16; 64],
    /// Bloky vrátane doplnenia na celé MCU.
    pub bw: usize,
    pub bh: usize,
    pub koef: Vec<i16>,
}

/// Koeficienty celého súboru a jeho segmenty APPn a COM (na prenesenie bez zmeny).
#[derive(Debug, Clone)]
pub struct Koeficienty {
    pub sirka: usize,
    pub vyska: usize,
    pub presnost: u8,
    pub zlozky: Vec<ZlozkaKoef>,
    pub segmenty: Vec<(u8, Vec<u8>)>,
}

/// Prečíta kvantizované koeficienty (bez inverznej DCT) – základ bezstratových úprav.
pub fn citaj_koeficienty(data: &[u8], limity: &Limity) -> Vysledok<Koeficienty> {
    let (sn, s) = rozober(data, limity)?;
    let zlozky = sn
        .zlozky
        .into_iter()
        .map(|z| ZlozkaKoef {
            id: z.id,
            h: z.h,
            v: z.v,
            kvant: z.kvant.unwrap_or([1; 64]),
            bw: z.bw,
            bh: z.bh,
            koef: z.koef,
        })
        .collect();
    Ok(Koeficienty { sirka: sn.sirka, vyska: sn.vyska, presnost: sn.presnost, zlozky, segmenty: s.segmenty })
}

fn rozober(data: &[u8], limity: &Limity) -> Vysledok<(Snimka, Stav)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(chyba("chýba značka začiatku (SOI)"));
    }
    let mut s = Stav {
        kvant: [None; 4],
        dc: [None, None, None, None],
        ac: [None, None, None, None],
        restart: 0,
        adobe: None,
        jfif: false,
        snimka: None,
        icc: Vec::new(),
        meta: Metadata { natocenie: 1, ..Default::default() },
        metadat: 0,
        eob: 0,
        segmenty: Vec::new(),
    };
    let mut poz = 2;
    let mut skenov = 0usize;
    loop {
        // ďalšia značka (výplň 0xFF sa preskočí)
        while poz < data.len() && data[poz] != 0xFF {
            poz += 1;
        }
        while poz < data.len() && data[poz] == 0xFF {
            poz += 1;
        }
        let Some(&z) = data.get(poz) else {
            // chýbajúci koniec (EOI) – ak už sú údaje, zobrazí sa, čo je (bežné pri neúplných stiahnutiach)
            if skenov > 0 {
                break;
            }
            return Err(chyba("súbor skončil pred obrazovými údajmi"));
        };
        poz += 1;
        match z {
            0xD8 | 0xD0..=0xD7 | 0x01 => continue,
            0xD9 => break,
            _ => {}
        }
        let dlzka = be16(data, poz)?;
        if dlzka < 2 || poz + dlzka > data.len() {
            return Err(chyba("segment presahuje koniec súboru"));
        }
        let seg = &data[poz + 2..poz + dlzka];
        poz += dlzka;
        match z {
            0xDB => dqt(&mut s, seg)?,
            0xC4 => dht(&mut s, seg)?,
            0xDD => {
                s.restart = be16(seg, 0)?;
            }
            0xC0..=0xC2 => sof(&mut s, seg, z == 0xC2, limity)?,
            0xC3 | 0xC5..=0xC7 => {
                return Err(chyba("bezstratový a hierarchický režim zatiaľ nie sú podporené"));
            }
            0xC9..=0xCB | 0xCD..=0xCF => return Err(chyba("aritmetické kódovanie zatiaľ nie je podporené")),
            0xDA => {
                poz = sken(&mut s, seg, data, poz)?;
                skenov += 1;
            }
            0xDC => return Err(chyba("značka DNL nie je podporená")),
            0xE0..=0xEF | 0xFE => {
                app(&mut s, z, seg, limity)?;
                s.segmenty.push((z, seg.to_vec()));
            }
            _ => {}
        }
    }
    let snimka = s.snimka.take().ok_or_else(|| chyba("chýba hlavička snímky (SOF)"))?;
    if skenov == 0 {
        return Err(chyba("súbor nemá obrazové údaje"));
    }
    Ok((snimka, s))
}

fn dqt(s: &mut Stav, mut seg: &[u8]) -> Vysledok<()> {
    while !seg.is_empty() {
        let (pq, tq) = (seg[0] >> 4, (seg[0] & 15) as usize);
        let dlzka = if pq == 0 { 64 } else { 128 };
        if tq > 3 || pq > 1 || seg.len() < 1 + dlzka {
            return Err(chyba("neplatná kvantizačná tabuľka"));
        }
        let mut t = [0u16; 64];
        for k in 0..64 {
            let q = if pq == 0 {
                seg[1 + k] as u16
            } else {
                u16::from_be_bytes([seg[1 + 2 * k], seg[2 + 2 * k]])
            };
            t[CIK_CAK[k]] = q.max(1);
        }
        s.kvant[tq] = Some(t);
        seg = &seg[1 + dlzka..];
    }
    Ok(())
}

fn dht(s: &mut Stav, mut seg: &[u8]) -> Vysledok<()> {
    while !seg.is_empty() {
        if seg.len() < 17 {
            return Err(chyba("neúplná Huffmanova tabuľka"));
        }
        let (tc, th) = (seg[0] >> 4, (seg[0] & 15) as usize);
        let mut pocty = [0u8; 16];
        pocty.copy_from_slice(&seg[1..17]);
        let n: usize = pocty.iter().map(|&x| x as usize).sum();
        if tc > 1 || th > 3 || seg.len() < 17 + n {
            return Err(chyba("neplatná Huffmanova tabuľka"));
        }
        let h = Huffman::nova(&pocty, &seg[17..17 + n])?;
        if tc == 0 {
            s.dc[th] = Some(h);
        } else {
            s.ac[th] = Some(h);
        }
        seg = &seg[17 + n..];
    }
    Ok(())
}

fn sof(s: &mut Stav, seg: &[u8], progresivny: bool, limity: &Limity) -> Vysledok<()> {
    if s.snimka.is_some() {
        return Err(chyba("viac hlavičiek snímky"));
    }
    if seg.len() < 6 {
        return Err(chyba("krátka hlavička snímky"));
    }
    let presnost = seg[0];
    let vyska = u16::from_be_bytes([seg[1], seg[2]]) as usize;
    let sirka = u16::from_be_bytes([seg[3], seg[4]]) as usize;
    let nz = seg[5] as usize;
    if presnost != 8 && presnost != 12 {
        return Err(chyba(&format!("presnosť {presnost} bitov")));
    }
    if !(1..=4).contains(&nz) || seg.len() < 6 + 3 * nz {
        return Err(chyba("neplatný počet zložiek"));
    }
    limity.over(sirka as u64, vyska as u64, (nz as u64) * if presnost > 8 { 2 } else { 1 })?;
    let mut zlozky = Vec::with_capacity(nz);
    for k in 0..nz {
        let c = &seg[6 + 3 * k..9 + 3 * k];
        let (h, v) = ((c[1] >> 4) as usize, (c[1] & 15) as usize);
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || c[2] > 3 {
            return Err(chyba("neplatné podvzorkovanie alebo tabuľka zložky"));
        }
        zlozky.push(Zlozka {
            id: c[0],
            h,
            v,
            tq: c[2] as usize,
            bw: 0,
            bh: 0,
            bw_ucinne: 0,
            bh_ucinne: 0,
            koef: Vec::new(),
            kvant: None,
            dc: 0,
        });
    }
    let hmax = zlozky.iter().map(|z| z.h).max().unwrap_or(1);
    let vmax = zlozky.iter().map(|z| z.v).max().unwrap_or(1);
    let mcux = sirka.div_ceil(8 * hmax);
    let mcuy = vyska.div_ceil(8 * vmax);
    // koeficienty: 2 bajty na vzorku (spolu s podvzorkovaním) – kontrola pamäte pred alokáciou
    let blokov: u64 = zlozky.iter().map(|z| (mcux * z.h * mcuy * z.v) as u64).sum();
    if blokov * 128 > limity.bajtov {
        return Err(Chyba::Limit(format!("JPEG: koeficienty by zabrali {} B", blokov * 128)));
    }
    for z in &mut zlozky {
        z.bw = mcux * z.h;
        z.bh = mcuy * z.v;
        z.bw_ucinne = (sirka * z.h).div_ceil(hmax).div_ceil(8);
        z.bh_ucinne = (vyska * z.v).div_ceil(vmax).div_ceil(8);
        z.koef = vec![0i16; z.bw * z.bh * 64];
    }
    if sirka == 0 || vyska == 0 {
        return Err(chyba("nulový rozmer (DNL nie je podporené)"));
    }
    s.snimka = Some(Snimka { sirka, vyska, presnost, progresivny, hmax, vmax, mcux, mcuy, zlozky });
    Ok(())
}

fn app(s: &mut Stav, z: u8, seg: &[u8], limity: &Limity) -> Vysledok<()> {
    s.metadat += seg.len() as u64;
    if s.metadat > limity.metadat {
        return Err(Chyba::Limit(format!("JPEG: metadáta nad {} B", limity.metadat)));
    }
    match z {
        0xE0 if seg.starts_with(b"JFIF\0") => {
            s.jfif = true;
            if seg.len() >= 12 {
                let (jednotky, x, y) = (seg[7], be16(seg, 8)? as f64, be16(seg, 10)? as f64);
                match jednotky {
                    1 => s.meta.dpi = Some((x, y)),
                    2 => s.meta.dpi = Some((x * 2.54, y * 2.54)),
                    _ => {}
                }
            }
        }
        0xE1 if seg.starts_with(b"Exif\0\0") => s.meta.exif = Some(seg[6..].to_vec()),
        0xE1 if seg.starts_with(b"http://ns.adobe.com/xap/1.0/\0") => s.meta.xmp = Some(seg[29..].to_vec()),
        0xED if seg.starts_with(b"Photoshop 3.0\0") => {
            if let Some(i) = crate::exif::iptc_z_photoshopu(seg) {
                s.meta.iptc = Some(i);
            }
        }
        0xE2 if seg.starts_with(b"ICC_PROFILE\0") && seg.len() > 14 => {
            s.icc.push((seg[12], seg[14..].to_vec()))
        }
        0xEE if seg.starts_with(b"Adobe") && seg.len() >= 12 => s.adobe = Some(seg[11]),
        0xFE => s.meta.texty.push(("Komentár".into(), String::from_utf8_lossy(seg).into_owned())),
        _ => {}
    }
    Ok(())
}

/// Dekóduje jeden sken; vráti polohu za jeho údajmi.
fn sken(s: &mut Stav, seg: &[u8], data: &[u8], poz: usize) -> Vysledok<usize> {
    let snimka = s.snimka.as_mut().ok_or_else(|| chyba("sken pred hlavičkou snímky"))?;
    let ns = *seg.first().ok_or_else(|| chyba("prázdna hlavička skenu"))? as usize;
    if !(1..=4).contains(&ns) || seg.len() < 4 + 2 * ns {
        return Err(chyba("neplatná hlavička skenu"));
    }
    let mut v_skene = Vec::with_capacity(ns);
    for k in 0..ns {
        let (id, t) = (seg[1 + 2 * k], seg[2 + 2 * k]);
        let i =
            snimka.zlozky.iter().position(|z| z.id == id).ok_or_else(|| chyba("sken s neznámou zložkou"))?;
        v_skene.push((i, (t >> 4) as usize, (t & 15) as usize));
    }
    let p = &seg[1 + 2 * ns..];
    let (ss, se, ah, al) = (p[0] as usize, p[1] as usize, (p[2] >> 4) as u32, (p[2] & 15) as u32);
    if snimka.progresivny {
        if se > 63 || ss > se || (ss == 0 && se != 0) || (ss > 0 && ns != 1) || al > 13 {
            return Err(chyba("neplatný progresívny sken"));
        }
    } else if ss != 0 || se != 63 || ah != 0 || al != 0 {
        // niektoré kodéry píšu nezmysly – sekvenčný sken je vždy 0–63
    }
    let (ss, se) = if snimka.progresivny { (ss, se) } else { (0, 63) };
    let mut dc_tab = Vec::new();
    let mut ac_tab = Vec::new();
    for &(i, td, ta) in &v_skene {
        let z = &mut snimka.zlozky[i];
        if z.kvant.is_none() {
            z.kvant = s.kvant[z.tq].or(None);
        }
        if z.kvant.is_none() {
            return Err(chyba("chýba kvantizačná tabuľka"));
        }
        z.dc = 0;
        let pouzi_std = |tab: &Option<Huffman>, std: ([u8; 16], &[u8])| -> Vysledok<Huffman> {
            match tab {
                Some(h) => Ok(h.clone()),
                None => Huffman::nova(&std.0, std.1),
            }
        };
        let jas = td == 0;
        dc_tab.push(if ss == 0 && ah == 0 {
            Some(pouzi_std(&s.dc[td.min(3)], if jas { STD_DC_JAS } else { STD_DC_FAREBNOST })?)
        } else {
            None
        });
        ac_tab.push(if se > 0 {
            Some(pouzi_std(&s.ac[ta.min(3)], if ta == 0 { STD_AC_JAS } else { STD_AC_FAREBNOST })?)
        } else {
            None
        });
    }
    s.eob = 0;
    let mut c = Citac::novy(data, poz);
    let restart = s.restart;
    let neprekladany = ns == 1;
    let (mcux, mcuy) = if neprekladany {
        let z = &snimka.zlozky[v_skene[0].0];
        (z.bw_ucinne, z.bh_ucinne)
    } else {
        (snimka.mcux, snimka.mcuy)
    };
    let mut eob = 0u32;
    let mut mcu = 0usize;
    for my in 0..mcuy {
        for mx in 0..mcux {
            if restart > 0 && mcu > 0 && mcu.is_multiple_of(restart) {
                c.restart()?;
                eob = 0;
                for &(i, ..) in &v_skene {
                    snimka.zlozky[i].dc = 0;
                }
            }
            mcu += 1;
            for (k, &(i, ..)) in v_skene.iter().enumerate() {
                let z = &mut snimka.zlozky[i];
                let (bh, bv) = if neprekladany { (1, 1) } else { (z.h, z.v) };
                for by in 0..bv {
                    for bx in 0..bh {
                        let (x, y) = if neprekladany { (mx, my) } else { (mx * z.h + bx, my * z.v + by) };
                        let b = (y * z.bw + x) * 64;
                        let blok = &mut z.koef[b..b + 64];
                        if !snimka.progresivny {
                            blok_sekvencny(&mut c, blok, &mut z.dc, dc_tab[k].as_ref(), ac_tab[k].as_ref())?;
                        } else if ss == 0 {
                            if ah == 0 {
                                let h = dc_tab[k].as_ref().ok_or_else(|| chyba("chýba tabuľka DC"))?;
                                let t = c.znak(h)? as u32;
                                if t > 16 {
                                    return Err(chyba("neplatný rozdiel DC"));
                                }
                                z.dc += c.rozsir(t);
                                blok[0] = (z.dc << al) as i16;
                            } else if c.bit() == 1 {
                                blok[0] |= 1 << al;
                            }
                        } else if ah == 0 {
                            let h = ac_tab[k].as_ref().ok_or_else(|| chyba("chýba tabuľka AC"))?;
                            ac_prvy(&mut c, blok, h, ss, se, al, &mut eob)?;
                        } else {
                            let h = ac_tab[k].as_ref().ok_or_else(|| chyba("chýba tabuľka AC"))?;
                            ac_spresnenie(&mut c, blok, h, ss, se, al, &mut eob)?;
                        }
                    }
                }
            }
            if c.prebehol() {
                // údaje sa skončili predčasne – zvyšok ostane sivý (neúplný súbor sa zobrazí, kým je)
                return Ok(data.len());
            }
        }
    }
    Ok(c.poz)
}

fn blok_sekvencny(
    c: &mut Citac,
    blok: &mut [i16],
    dc: &mut i32,
    dc_tab: Option<&Huffman>,
    ac_tab: Option<&Huffman>,
) -> Vysledok<()> {
    let (Some(hd), Some(ha)) = (dc_tab, ac_tab) else {
        return Err(chyba("chýba Huffmanova tabuľka"));
    };
    let t = c.znak(hd)? as u32;
    if t > 16 {
        return Err(chyba("neplatný rozdiel DC"));
    }
    *dc += c.rozsir(t);
    blok[0] = *dc as i16;
    let mut k = 1;
    while k < 64 {
        let rs = c.znak(ha)?;
        let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
        if s == 0 {
            if r != 15 {
                break;
            }
            k += 16;
            continue;
        }
        k += r;
        if k > 63 {
            return Err(chyba("koeficient za koncom bloku"));
        }
        blok[CIK_CAK[k]] = c.rozsir(s) as i16;
        k += 1;
    }
    Ok(())
}

/// Prvý prechod koeficientov AC (spektrálny výber, G.1.2.2).
fn ac_prvy(
    c: &mut Citac,
    blok: &mut [i16],
    h: &Huffman,
    ss: usize,
    se: usize,
    al: u32,
    eob: &mut u32,
) -> Vysledok<()> {
    if *eob > 0 {
        *eob -= 1;
        return Ok(());
    }
    let mut k = ss;
    while k <= se {
        let rs = c.znak(h)?;
        let (r, s) = ((rs >> 4) as u32, (rs & 15) as u32);
        if s == 0 {
            if r < 15 {
                *eob = (1 << r) - 1 + c.bity(r);
                break;
            }
            k += 16;
            continue;
        }
        k += r as usize;
        if k > 63 {
            return Err(chyba("koeficient za koncom bloku"));
        }
        blok[CIK_CAK[k]] = (c.rozsir(s) << al) as i16;
        k += 1;
    }
    Ok(())
}

/// Spresňujúci prechod koeficientov AC (postupná aproximácia, G.1.2.3).
fn ac_spresnenie(
    c: &mut Citac,
    blok: &mut [i16],
    h: &Huffman,
    ss: usize,
    se: usize,
    al: u32,
    eob: &mut u32,
) -> Vysledok<()> {
    let plus = 1i16 << al;
    let minus = -1i16 << al;
    let spresni = |c: &mut Citac, x: &mut i16| {
        if c.bit() == 1 && (*x & plus) == 0 {
            *x += if *x >= 0 { plus } else { minus };
        }
    };
    let mut k = ss;
    if *eob == 0 {
        while k <= se {
            let rs = c.znak(h)?;
            let (mut r, s) = ((rs >> 4) as i32, (rs & 15) as u32);
            let mut hodnota = 0i16;
            if s != 0 {
                if s != 1 {
                    return Err(chyba("neplatná hodnota pri spresnení"));
                }
                hodnota = if c.bit() == 1 { plus } else { minus };
            } else if r != 15 {
                *eob = (1 << r) + c.bity(r as u32);
                break;
            }
            while k <= se {
                let z = CIK_CAK[k];
                if blok[z] != 0 {
                    spresni(c, &mut blok[z]);
                } else {
                    if r == 0 {
                        if hodnota != 0 {
                            blok[z] = hodnota;
                        }
                        k += 1;
                        break;
                    }
                    r -= 1;
                }
                k += 1;
            }
        }
    }
    if *eob > 0 {
        while k <= se {
            let z = CIK_CAK[k];
            if blok[z] != 0 {
                spresni(c, &mut blok[z]);
            }
            k += 1;
        }
        *eob -= 1;
    }
    Ok(())
}

/// IDCT všetkých blokov zložky do roviny vzoriek (n × n bodov z bloku), súbežne po riadkoch blokov.
fn rovina(z: &Zlozka, n: usize, presnost: u8) -> Vec<u16> {
    let kvant = z.kvant.unwrap_or([1; 64]);
    let sirka = z.bw * n;
    let stred = if presnost > 8 { 2048.0 } else { 128.0 };
    let max = if presnost > 8 { 4095.0 } else { 255.0 };
    let riadky = paralel::mapuj_rozsah(z.bh, 1.max(z.bh / (4 * paralel::pocet_vlakien())), |rozsah| {
        let mut von = vec![0u16; rozsah.len() * n * sirka];
        let (mut k, mut s) = ([0f32; 64], [0f32; 64]);
        for (j, by) in rozsah.enumerate() {
            for bx in 0..z.bw {
                let b = (by * z.bw + bx) * 64;
                for i in 0..64 {
                    k[i] = z.koef[b + i] as f32 * kvant[i] as f32;
                }
                if n == 8 {
                    dct::inverzia(&k, &mut s);
                } else {
                    dct::inverzia_zmensena(&k, n, &mut s);
                }
                for y in 0..n {
                    let ciel = (j * n + y) * sirka + bx * n;
                    for x in 0..n {
                        let v = (s[y * n + x] + stred).round().clamp(0.0, max);
                        von[ciel + x] = v as u16;
                    }
                }
            }
        }
        von
    });
    riadky.concat()
}

/// Zväčší rovinu `w × h` faktormi `fx`, `fy` lineárnou interpoláciou so stredmi vzoriek (chroma podľa JFIF)
/// do `sirka × vyska`. Celočíselne a presne; neceločíselný pomer sa rieši najbližšou vzorkou.
fn prevzorkuj(r: &[u16], w: usize, h: usize, fx: usize, fy: usize, sirka: usize, vyska: usize) -> Vec<u16> {
    if fx == 1 && fy == 1 {
        let mut von = Vec::with_capacity(sirka * vyska);
        for y in 0..vyska {
            von.extend_from_slice(&r[y * w..y * w + sirka]);
        }
        return von;
    }
    // poloha zdroja pre cieľ x: (2x + 1 − f) / (2f) → index a váha v jednotkách 1/(2f)
    let os = |f: usize, d: usize, max: usize| -> Vec<(usize, usize, u32)> {
        (0..d)
            .map(|x| {
                let c = 2 * x as i64 + 1 - f as i64;
                let i0 = c.div_euclid(2 * f as i64);
                let t = (c - i0 * 2 * f as i64) as u32;
                let a = i0.clamp(0, max as i64 - 1) as usize;
                let b = (i0 + 1).clamp(0, max as i64 - 1) as usize;
                (a, b, t)
            })
            .collect()
    };
    let ox = os(fx, sirka, w);
    let oy = os(fy, vyska, h);
    let (dx, dy) = (2 * fx as u32, 2 * fy as u32);
    let delitel = dx * dy;
    let mut von = vec![0u16; sirka * vyska];
    let mut stlpec = vec![0u32; w];
    for (y, &(a, b, t)) in oy.iter().enumerate() {
        for x in 0..w {
            stlpec[x] = (dy - t) * r[a * w + x] as u32 + t * r[b * w + x] as u32;
        }
        for (x, &(a, b, t)) in ox.iter().enumerate() {
            von[y * sirka + x] = (((dx - t) * stlpec[a] + t * stlpec[b] + delitel / 2) / delitel) as u16;
        }
    }
    von
}

fn zostav(sn: &Snimka, s: &Stav, n: usize) -> Vysledok<Obraz> {
    let sirka = (sn.sirka * n).div_ceil(8);
    let vyska = (sn.vyska * n).div_ceil(8);
    let roviny: Vec<Vec<u16>> = sn
        .zlozky
        .iter()
        .map(|z| {
            let r = rovina(z, n, sn.presnost);
            let (w, h) = (z.bw * n, z.bh * n);
            if sn.hmax.is_multiple_of(z.h) && sn.vmax.is_multiple_of(z.v) {
                prevzorkuj(&r, w, h, sn.hmax / z.h, sn.vmax / z.v, sirka, vyska)
            } else {
                // neceločíselný pomer (zriedkavé) – najbližšia vzorka
                let mut von = vec![0u16; sirka * vyska];
                for y in 0..vyska {
                    for x in 0..sirka {
                        von[y * sirka + x] =
                            r[(y * z.v / sn.vmax).min(h - 1) * w + (x * z.h / sn.hmax).min(w - 1)];
                    }
                }
                von
            }
        })
        .collect();
    let nz = roviny.len();
    let max: i32 = if sn.presnost > 8 { 4095 } else { 255 };
    let stred = (max + 1) / 2;
    let ids: Vec<u8> = sn.zlozky.iter().map(|z| z.id).collect();
    let rgb_priamo = s.adobe == Some(0) || (s.adobe.is_none() && !s.jfif && ids == *b"RGB");
    let px = sirka * vyska;
    let (model, kanaly) = match nz {
        1 => (Model::Siva, 1),
        3 => (Model::Rgb, 3),
        4 => (Model::Cmyk, 4),
        _ => return Err(chyba("nepodporovaný počet zložiek (2)")),
    };
    let mut von = vec![0u16; px * kanaly];
    let ycc = |y: i32, cb: i32, cr: i32| -> [i32; 3] {
        let (cb, cr) = (cb - stred, cr - stred);
        [
            (y + ((91_881 * cr + 32_768) >> 16)).clamp(0, max),
            (y + ((-22_554 * cb - 46_802 * cr + 32_768) >> 16)).clamp(0, max),
            (y + ((116_130 * cb + 32_768) >> 16)).clamp(0, max),
        ]
    };
    match nz {
        1 => von.copy_from_slice(&roviny[0]),
        3 => {
            for i in 0..px {
                let (a, b, c) = (roviny[0][i] as i32, roviny[1][i] as i32, roviny[2][i] as i32);
                let p = if rgb_priamo { [a, b, c] } else { ycc(a, b, c) };
                for k in 0..3 {
                    von[3 * i + k] = p[k] as u16;
                }
            }
        }
        _ => {
            // CMYK: Adobe ukladá obrátene (255 = bez farby); YCCK sa najprv prevedie z YCbCr
            let obratene = s.adobe.is_some();
            for i in 0..px {
                let (a, b, c, k) =
                    (roviny[0][i] as i32, roviny[1][i] as i32, roviny[2][i] as i32, roviny[3][i] as i32);
                let cmyk = if s.adobe == Some(2) {
                    let p = ycc(a, b, c);
                    [p[0], p[1], p[2], max - k]
                } else if obratene {
                    [max - a, max - b, max - c, max - k]
                } else {
                    [a, b, c, k]
                };
                for j in 0..4 {
                    von[4 * i + j] = cmyk[j] as u16;
                }
            }
        }
    }
    let vzorky = if sn.presnost > 8 {
        Vzorky::U16(von.into_iter().map(|v| (v << 4) | (v >> 8)).collect())
    } else {
        Vzorky::U8(von.into_iter().map(|v| v as u8).collect())
    };
    let mut o = Obraz::novy(sirka, vyska, model, false, vzorky)?;
    o.bitov = sn.presnost;
    Ok(o)
}
