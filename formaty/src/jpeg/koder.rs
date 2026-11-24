//! Kodér JPEG – presná DCT, kvantizácia podľa kvality, optimalizované Huffmanove tabuľky, progresívny zápis.

use super::dct;
use super::{CIK_CAK, KVANT_FAREBNOST, KVANT_JAS};
use crate::huffman::dlzky_kodov;
use crate::obraz::{Model, Obraz, Vzorky};
use orchidea_jadro::{Chyba, Vysledok, paralel};

/// Podvzorkovanie farebnosti (Cb, Cr).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Podvzorkovanie {
    /// Plné rozlíšenie farby.
    S444,
    /// Polovičné vodorovne.
    S422,
    /// Polovičné vodorovne aj zvislo.
    S420,
    /// Pri kvalite od 90 plné, inak 4:2:0.
    Automaticky,
}

/// Nastavenie zápisu JPEG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NastavenieJpeg {
    /// Kvalita 1–100 (stupnica ako v bežných programoch; 90 je takmer nerozoznateľná od originálu).
    pub kvalita: u8,
    pub podvzorkovanie: Podvzorkovanie,
    /// Progresívny zápis – obrázok sa pri načítaní zobrazuje od hrubého k jemnému; býva aj menší.
    pub progresivny: bool,
}

impl Default for NastavenieJpeg {
    fn default() -> Self {
        NastavenieJpeg { kvalita: 90, podvzorkovanie: Podvzorkovanie::Automaticky, progresivny: false }
    }
}

/// Kvantizačná tabuľka normy prepočítaná podľa kvality (zaužívaná stupnica: 50 = tabuľka normy).
fn tabulka(zaklad: &[u16; 64], kvalita: u8) -> [u16; 64] {
    let q = kvalita.clamp(1, 100) as u32;
    let mierka = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut t = [0u16; 64];
    for i in 0..64 {
        t[i] = ((zaklad[i] as u32 * mierka + 50) / 100).clamp(1, 255) as u16;
    }
    t
}

/// Rovina 8-bitových vzoriek jednej zložky, doplnená opakovaním okraja na celé bloky MCU.
struct Rovina {
    w: usize,
    h: usize,
    vzorky: Vec<u8>,
    hf: usize,
    vf: usize,
    tq: usize,
    /// Bloky so skutočnými údajmi (neprekladaný sken kóduje len tieto).
    bw_ucinne: usize,
    bh_ucinne: usize,
    /// Kvantizované koeficienty blokov (prirodzené poradie).
    koef: Vec<i16>,
}

/// Cieľ zápisu znakov – raz počítadlo početností, raz skutočný zápis.
trait Ciel {
    fn znak(&mut self, tab: usize, z: u8);
    fn bity(&mut self, hodnota: u32, n: u32);
}

struct Pocty([[u32; 257]; 4]);

impl Ciel for Pocty {
    fn znak(&mut self, tab: usize, z: u8) {
        self.0[tab][z as usize] += 1;
    }
    fn bity(&mut self, _: u32, _: u32) {}
}

/// Zápis bitov od najvyššieho s vložením nuly za každý bajt 0xFF.
struct Zapisovac<'a> {
    von: &'a mut Vec<u8>,
    akum: u64,
    pocet: u32,
    kody: [[(u16, u8); 256]; 4],
}

impl Zapisovac<'_> {
    fn zapis(&mut self, hodnota: u32, n: u32) {
        if n == 0 {
            return;
        }
        self.akum = (self.akum << n) | (hodnota as u64 & ((1u64 << n) - 1));
        self.pocet += n;
        while self.pocet >= 8 {
            let b = (self.akum >> (self.pocet - 8)) as u8;
            self.von.push(b);
            if b == 0xFF {
                self.von.push(0);
            }
            self.pocet -= 8;
        }
    }

    /// Doplní posledný bajt jednotkami (norma).
    fn dokonci(&mut self) {
        if self.pocet > 0 {
            let n = 8 - self.pocet;
            self.zapis((1 << n) - 1, n);
        }
    }
}

impl Ciel for Zapisovac<'_> {
    fn znak(&mut self, tab: usize, z: u8) {
        let (k, l) = self.kody[tab][z as usize];
        self.zapis(k as u32, l as u32);
    }
    fn bity(&mut self, hodnota: u32, n: u32) {
        self.zapis(hodnota, n);
    }
}

/// Počet bitov potrebných na |v| (kategória v norme).
#[inline]
fn kategoria(v: i32) -> u32 {
    32 - v.unsigned_abs().leading_zeros()
}

/// Bity hodnoty v kategórii `s` (záporné ako v − 1 v doplnku).
#[inline]
fn bity_hodnoty(v: i32, s: u32) -> u32 {
    if v < 0 { (v - 1) as u32 & ((1 << s) - 1) } else { v as u32 }
}

/// Opis skenu: zložky, spektrálny výber od–po, tabuľky (DC, AC).
struct Sken {
    zlozky: Vec<usize>,
    od: usize,
    po: usize,
}

/// Zapíše obrázok do JPEG. Sivý obrázok má jednu zložku, RGB sa prevedie na YCbCr, CMYK sa uloží
/// v konvencii Adobe. Alfa sa vynechá (JPEG ju nemá); 16-bitové a plávajúce vzorky sa prevedú na 8 bitov.
pub fn zapis(o: &Obraz, n: &NastavenieJpeg) -> Vysledok<Vec<u8>> {
    o.over()?;
    if o.sirka > 65_535 || o.vyska > 65_535 {
        return Err(Chyba::Limit("JPEG: rozmer nad 65 535".into()));
    }
    let farebnych = match o.model {
        Model::Siva => 1,
        Model::Rgb => 3,
        Model::Cmyk => 4,
        Model::Iny => {
            return Err(Chyba::NezhodaTypov {
                popis: "JPEG nevie uložiť iné kanály".into(), navrh: None
            });
        }
    };
    let k = o.kanaly();
    let (w, h) = (o.sirka, o.vyska);
    let vzorka = |i: usize| -> f32 {
        match &o.vzorky {
            Vzorky::U8(v) => v[i] as f32,
            Vzorky::U16(v) => v[i] as f32 / 257.0,
            Vzorky::F32(v) => v[i].clamp(0.0, 1.0) * 255.0,
        }
    };
    let podv = match n.podvzorkovanie {
        _ if farebnych != 3 => Podvzorkovanie::S444,
        Podvzorkovanie::Automaticky if n.kvalita >= 90 => Podvzorkovanie::S444,
        Podvzorkovanie::Automaticky => Podvzorkovanie::S420,
        p => p,
    };
    let (hmax, vmax) = match podv {
        Podvzorkovanie::S420 => (2, 2),
        Podvzorkovanie::S422 => (2, 1),
        _ => (1, 1),
    };
    let (mcux, mcuy) = (w.div_ceil(8 * hmax), h.div_ceil(8 * vmax));
    // plné roviny (doplnené na celé MCU opakovaním okraja)
    let (pw, ph) = (mcux * 8 * hmax, mcuy * 8 * vmax);
    let mut plne: Vec<Vec<u8>> = vec![vec![0u8; pw * ph]; farebnych];
    for y in 0..ph {
        let sy = y.min(h - 1);
        for x in 0..pw {
            let i = (sy * w + x.min(w - 1)) * k;
            let ciel = y * pw + x;
            match farebnych {
                1 => plne[0][ciel] = vzorka(i).round() as u8,
                3 => {
                    let (r, g, b) = (vzorka(i), vzorka(i + 1), vzorka(i + 2));
                    let yy = 0.299 * r + 0.587 * g + 0.114 * b;
                    let cb = -0.168_736 * r - 0.331_264 * g + 0.5 * b + 128.0;
                    let cr = 0.5 * r - 0.418_688 * g - 0.081_312 * b + 128.0;
                    plne[0][ciel] = yy.round().clamp(0.0, 255.0) as u8;
                    plne[1][ciel] = cb.round().clamp(0.0, 255.0) as u8;
                    plne[2][ciel] = cr.round().clamp(0.0, 255.0) as u8;
                }
                _ => {
                    // Adobe CMYK sa ukladá obrátene (255 = bez farby)
                    for (c, p) in plne.iter_mut().enumerate() {
                        p[ciel] = 255 - vzorka(i + c).round() as u8;
                    }
                }
            }
        }
    }
    let mut roviny: Vec<Rovina> = plne
        .into_iter()
        .enumerate()
        .map(|(c, p)| {
            let farebnost = farebnych == 3 && c > 0;
            let (hf, vf) = if farebnost { (1, 1) } else { (hmax, vmax) };
            let (fx, fy) = (hmax / hf, vmax / vf);
            let (rw, rh) = (pw / fx, ph / fy);
            let vzorky = if fx == 1 && fy == 1 {
                p
            } else {
                let mut v = vec![0u8; rw * rh];
                for y in 0..rh {
                    for x in 0..rw {
                        let mut s = 0u32;
                        for dy in 0..fy {
                            for dx in 0..fx {
                                s += p[(y * fy + dy) * pw + x * fx + dx] as u32;
                            }
                        }
                        let n = (fx * fy) as u32;
                        v[y * rw + x] = ((s + n / 2) / n) as u8;
                    }
                }
                v
            };
            let bw_ucinne = (w * hf).div_ceil(hmax).div_ceil(8);
            let bh_ucinne = (h * vf).div_ceil(vmax).div_ceil(8);
            Rovina {
                w: rw,
                h: rh,
                vzorky,
                hf,
                vf,
                tq: farebnost as usize,
                bw_ucinne,
                bh_ucinne,
                koef: Vec::new(),
            }
        })
        .collect();
    let tabulky = [tabulka(&KVANT_JAS, n.kvalita), tabulka(&KVANT_FAREBNOST, n.kvalita)];
    // DCT a kvantizácia všetkých blokov (súbežne po riadkoch blokov)
    for r in roviny.iter_mut() {
        let (bw, bh) = (r.w / 8, r.h / 8);
        let q = &tabulky[r.tq];
        let vz = &r.vzorky;
        let casti = paralel::mapuj_rozsah(bh, 1.max(bh / (4 * paralel::pocet_vlakien())), |rozsah| {
            let mut von = Vec::with_capacity(rozsah.len() * bw * 64);
            let (mut b, mut f) = ([0f32; 64], [0f32; 64]);
            for by in rozsah {
                for bx in 0..bw {
                    for y in 0..8 {
                        for x in 0..8 {
                            b[y * 8 + x] = vz[(by * 8 + y) * r.w + bx * 8 + x] as f32 - 128.0;
                        }
                    }
                    dct::dopredna(&b, &mut f);
                    for i in 0..64 {
                        von.push((f[i] / q[i] as f32).round() as i16);
                    }
                }
            }
            von
        });
        r.koef = casti.concat();
    }
    let skeny: Vec<Sken> = if !n.progresivny {
        vec![Sken { zlozky: (0..farebnych).collect(), od: 0, po: 63 }]
    } else {
        let mut s = vec![Sken { zlozky: (0..farebnych).collect(), od: 0, po: 0 }];
        s.push(Sken { zlozky: vec![0], od: 1, po: 5 });
        for c in 1..farebnych {
            s.push(Sken { zlozky: vec![c], od: 1, po: 63 });
        }
        s.push(Sken { zlozky: vec![0], od: 6, po: 63 });
        s
    };

    let mut von = vec![0xFF, 0xD8];
    segmenty_hlavicky(&mut von, o, farebnych, &tabulky, &roviny, n.progresivny, (hmax, vmax))?;
    for sken in &skeny {
        // tabuľka pre sken: jas 0, farebnosť 1 (DC aj AC)
        let tab_dc: Vec<usize> = sken.zlozky.iter().map(|&c| roviny[c].tq).collect();
        let mut pocty = Pocty([[0; 257]; 4]);
        koduj_sken(&mut pocty, &roviny, sken, (mcux, mcuy), &tab_dc);
        let mut kody = [[(0u16, 0u8); 256]; 4];
        let mut dht = Vec::new();
        for (t, poc) in pocty.0.iter().enumerate() {
            if poc.iter().all(|&x| x == 0) {
                continue;
            }
            let (bity, hodnoty, k) = huffman_tabulka(poc);
            kody[t] = k;
            // DC tabuľky 0–1 → trieda 0, id 0–1; AC tabuľky 2–3 → trieda 1, id 0–1
            dht.push(if t < 2 { t as u8 } else { 0x10 | (t - 2) as u8 });
            dht.extend_from_slice(&bity);
            dht.extend_from_slice(&hodnoty);
        }
        if !dht.is_empty() {
            segment(&mut von, 0xC4, &dht);
        }
        let mut sos = vec![sken.zlozky.len() as u8];
        for &c in &sken.zlozky {
            sos.push(c as u8 + 1);
            sos.push((roviny[c].tq as u8) << 4 | roviny[c].tq as u8);
        }
        sos.extend_from_slice(&[sken.od as u8, sken.po as u8, 0]);
        segment(&mut von, 0xDA, &sos);
        let mut z = Zapisovac { von: &mut von, akum: 0, pocet: 0, kody };
        koduj_sken(&mut z, &roviny, sken, (mcux, mcuy), &tab_dc);
        z.dokonci();
    }
    von.extend_from_slice(&[0xFF, 0xD9]);
    Ok(von)
}

fn segment(von: &mut Vec<u8>, znacka: u8, obsah: &[u8]) {
    von.extend_from_slice(&[0xFF, znacka]);
    von.extend_from_slice(&((obsah.len() + 2) as u16).to_be_bytes());
    von.extend_from_slice(obsah);
}

fn segmenty_hlavicky(
    von: &mut Vec<u8>,
    o: &Obraz,
    farebnych: usize,
    tabulky: &[[u16; 64]; 2],
    roviny: &[Rovina],
    progresivny: bool,
    (hmax, vmax): (usize, usize),
) -> Vysledok<()> {
    // JFIF (rozlíšenie v bodoch na palec, inak pomer strán 1:1)
    let mut jfif = b"JFIF\0\x01\x01".to_vec();
    match o.metadata.dpi {
        Some((x, y)) => {
            jfif.push(1);
            jfif.extend_from_slice(&(x.round().clamp(1.0, 65_535.0) as u16).to_be_bytes());
            jfif.extend_from_slice(&(y.round().clamp(1.0, 65_535.0) as u16).to_be_bytes());
        }
        None => jfif.extend_from_slice(&[0, 0, 1, 0, 1]),
    }
    jfif.extend_from_slice(&[0, 0]);
    if farebnych != 4 {
        segment(von, 0xE0, &jfif);
    }
    if let Some(e) = &o.metadata.exif {
        let mut d = b"Exif\0\0".to_vec();
        d.extend_from_slice(e);
        if d.len() > 65_533 {
            return Err(Chyba::Limit("JPEG: Exif nad 64 kB".into()));
        }
        segment(von, 0xE1, &d);
    }
    if let Some(x) = &o.metadata.xmp
        && x.len() + 29 <= 65_533
    {
        let mut d = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        d.extend_from_slice(x);
        segment(von, 0xE1, &d);
    }
    if let Some(icc) = &o.farba.icc {
        let kusy: Vec<&[u8]> = icc.chunks(65_519).collect();
        if kusy.len() > 255 {
            return Err(Chyba::Limit("JPEG: profil ICC nad 16 MB".into()));
        }
        for (i, kus) in kusy.iter().enumerate() {
            let mut d = b"ICC_PROFILE\0".to_vec();
            d.push(i as u8 + 1);
            d.push(kusy.len() as u8);
            d.extend_from_slice(kus);
            segment(von, 0xE2, &d);
        }
    }
    if farebnych == 4 {
        // Adobe: verzia 100, bez príznakov, transformácia 0 (CMYK bez prevodu)
        segment(von, 0xEE, b"Adobe\x00\x64\x00\x00\x00\x00\x00");
    }
    for (t, tab) in tabulky.iter().enumerate().take(if farebnych == 3 { 2 } else { 1 }) {
        let mut d = vec![t as u8];
        d.extend(CIK_CAK.iter().map(|&i| tab[i] as u8));
        segment(von, 0xDB, &d);
    }
    let mut sof = vec![8];
    sof.extend_from_slice(&(o.vyska as u16).to_be_bytes());
    sof.extend_from_slice(&(o.sirka as u16).to_be_bytes());
    sof.push(farebnych as u8);
    debug_assert!(roviny.iter().all(|r| r.hf <= hmax && r.vf <= vmax));
    for (c, r) in roviny.iter().enumerate() {
        sof.extend_from_slice(&[c as u8 + 1, (r.hf as u8) << 4 | r.vf as u8, r.tq as u8]);
    }
    segment(von, if progresivny { 0xC2 } else { 0xC0 }, &sof);
    Ok(())
}

/// Optimálna Huffmanova tabuľka (najviac 16 bitov, bez kódu zo samých jednotiek): počty dĺžok, hodnoty
/// a kód každého znaku.
fn huffman_tabulka(pocty: &[u32; 257]) -> ([u8; 16], Vec<u8>, [(u16, u8); 256]) {
    let mut p = *pocty;
    p[256] = 1; // vyhradený znak – dostane kód zo samých jednotiek, ktorý sa nepoužije
    let mut d = dlzky_kodov(&p, 16);
    let najdlhsia = *d.iter().max().unwrap_or(&0);
    if d[256] < najdlhsia
        && let Some(i) = (0..256).rev().find(|&i| d[i] == najdlhsia)
    {
        d.swap(i, 256);
    }
    let mut poradie: Vec<usize> = (0..256).filter(|&i| d[i] > 0).collect();
    poradie.sort_by_key(|&i| (d[i], i));
    let mut bity = [0u8; 16];
    for &i in &poradie {
        bity[d[i] as usize - 1] += 1;
    }
    let mut kody = [(0u16, 0u8); 256];
    let mut kod = 0u32;
    let mut dlzka = 1u8;
    for &i in &poradie {
        while d[i] > dlzka {
            kod <<= 1;
            dlzka += 1;
        }
        kody[i] = (kod as u16, dlzka);
        kod += 1;
    }
    (bity, poradie.iter().map(|&i| i as u8).collect(), kody)
}

fn koduj_sken<C: Ciel>(
    c: &mut C,
    roviny: &[Rovina],
    sken: &Sken,
    (mcux, mcuy): (usize, usize),
    tab: &[usize],
) {
    let mut dc_pred = vec![0i32; sken.zlozky.len()];
    let mut eob = 0u32;
    let blok = |c: &mut C, i: usize, r: &Rovina, bx: usize, by: usize, dc_pred: &mut i32, eob: &mut u32| {
        let b = (by * (r.w / 8) + bx) * 64;
        let koef = &r.koef[b..b + 64];
        let (td, ta) = (tab[i], 2 + tab[i]);
        if sken.od == 0 {
            let dc = koef[0] as i32;
            let diff = dc - *dc_pred;
            *dc_pred = dc;
            let s = kategoria(diff);
            c.znak(td, s as u8);
            c.bity(bity_hodnoty(diff, s), s);
            if sken.po == 0 {
                return;
            }
        }
        let progresivny = sken.od > 0;
        let mut beh = 0u32;
        for k in sken.od.max(1)..=sken.po {
            let v = koef[CIK_CAK[k]] as i32;
            if v == 0 {
                beh += 1;
                continue;
            }
            if progresivny {
                vypis_eob(c, ta, eob);
            }
            while beh > 15 {
                c.znak(ta, 0xF0);
                beh -= 16;
            }
            let s = kategoria(v);
            c.znak(ta, (beh << 4) as u8 | s as u8);
            c.bity(bity_hodnoty(v, s), s);
            beh = 0;
        }
        if beh > 0 {
            if progresivny {
                *eob += 1;
                if *eob == 0x7FFF {
                    vypis_eob(c, ta, eob);
                }
            } else {
                c.znak(ta, 0x00);
            }
        }
    };
    if sken.zlozky.len() == 1 {
        let i = 0;
        let r = &roviny[sken.zlozky[0]];
        // neprekladaný sken: len bloky so skutočnými údajmi
        for by in 0..r.bh_ucinne {
            for bx in 0..r.bw_ucinne {
                blok(c, i, r, bx, by, &mut dc_pred[0], &mut eob);
            }
        }
    } else {
        for my in 0..mcuy {
            for mx in 0..mcux {
                for (i, &z) in sken.zlozky.iter().enumerate() {
                    let r = &roviny[z];
                    for v in 0..r.vf {
                        for h in 0..r.hf {
                            blok(c, i, r, mx * r.hf + h, my * r.vf + v, &mut dc_pred[i], &mut eob);
                        }
                    }
                }
            }
        }
    }
    if sken.od > 0 {
        vypis_eob(c, 2 + tab[0], &mut eob);
    }
}

/// Zapíše nahromadený beh prázdnych blokov (EOBRUN) v progresívnom skene.
fn vypis_eob<C: Ciel>(c: &mut C, ta: usize, eob: &mut u32) {
    if *eob == 0 {
        return;
    }
    let n = 31 - eob.leading_zeros();
    c.znak(ta, (n << 4) as u8);
    c.bity(*eob - (1 << n), n);
    *eob = 0;
}
