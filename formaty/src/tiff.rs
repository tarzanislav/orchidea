//! TIFF (Adobe TIFF 6.0, BigTIFF, TIFF/EP) – čítanie aj zápis vlastným kódom.
//!
//! **Čítanie:** klasický TIFF aj BigTIFF (súbory nad 4 GB), oboje poradie bajtov, pruhy aj dlaždice,
//! zložky za sebou aj v rovinách; kompresia žiadna, PackBits, LZW, Deflate a JPEG (nový štýl s JPEGTables);
//! prediktor vodorovný (2) aj plávajúci (3); vzorky 1–16 bitov celé, 32 bitov celé, 16/32/64 bitov plávajúce;
//! sivá (aj „biela je nula“), RGB, paleta, CMYK, YCbCr (cez JPEG); profil ICC, XMP, Exif natočenie,
//! rozlíšenie, popisné texty. Viac strán – číta sa prvá plná strana (miniatúry sa preskočia).
//!
//! **Zápis:** sivá, RGB, CMYK s alfou alebo bez, 8/16 bitov celé a 32 bitov plávajúce; kompresia Deflate
//! s prediktorom (celé čísla vodorovný, plávajúce bajtový) alebo bez; pruhy po ~256 kB, BigTIFF nad 4 GB.

use crate::deflate;
use crate::obraz::{Limity, Metadata, Model, Obraz, Vzorky};
use crate::polovicna::f16_na_f32;
use orchidea_jadro::{Chyba, Vysledok};

fn chyba(text: &str) -> Chyba {
    Chyba::PoskodeneData(format!("TIFF: {text}"))
}

/// Čítač čísel podľa poradia bajtov súboru.
#[derive(Clone, Copy)]
struct Bajty<'a> {
    d: &'a [u8],
    le: bool,
}

impl<'a> Bajty<'a> {
    fn u16(&self, i: u64) -> Vysledok<u16> {
        let b = self.kus(i, 2)?;
        Ok(if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    }
    fn u32(&self, i: u64) -> Vysledok<u32> {
        let b = self.kus(i, 4)?;
        let p = [b[0], b[1], b[2], b[3]];
        Ok(if self.le { u32::from_le_bytes(p) } else { u32::from_be_bytes(p) })
    }
    fn u64(&self, i: u64) -> Vysledok<u64> {
        let b = self.kus(i, 8)?;
        let mut p = [0u8; 8];
        p.copy_from_slice(b);
        Ok(if self.le { u64::from_le_bytes(p) } else { u64::from_be_bytes(p) })
    }
    fn kus(&self, i: u64, n: u64) -> Vysledok<&'a [u8]> {
        let k = i.checked_add(n).ok_or_else(|| chyba("posun mimo súboru"))?;
        if k > self.d.len() as u64 {
            return Err(chyba("údaje mimo súboru"));
        }
        Ok(&self.d[i as usize..k as usize])
    }
}

/// Položka adresára (IFD): značka, typ, počet a poloha hodnôt v súbore.
#[derive(Debug, Clone, Copy)]
struct Polozka {
    znacka: u16,
    typ: u16,
    pocet: u64,
    poloha: u64,
}

fn velkost_typu(t: u16) -> u64 {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => 0,
    }
}

struct Adresar<'a> {
    b: Bajty<'a>,
    polozky: Vec<Polozka>,
}

impl<'a> Adresar<'a> {
    fn najdi(&self, z: u16) -> Option<&Polozka> {
        self.polozky.iter().find(|p| p.znacka == z)
    }

    /// Celé čísla položky (SHORT, LONG, LONG8, BYTE).
    fn cisla(&self, z: u16, limit: u64) -> Vysledok<Option<Vec<u64>>> {
        let Some(p) = self.najdi(z) else { return Ok(None) };
        if p.pocet > limit {
            return Err(Chyba::Limit(format!("TIFF: značka {z} má {} hodnôt", p.pocet)));
        }
        let s = velkost_typu(p.typ);
        // hodnoty musia byť v súbore – inak by veľký počet alokoval pamäť zbytočne
        self.b.kus(p.poloha, p.pocet.saturating_mul(s))?;
        let mut v = Vec::with_capacity(p.pocet as usize);
        for i in 0..p.pocet {
            let a = p.poloha + i * s;
            v.push(match p.typ {
                1 | 7 => self.b.kus(a, 1)?[0] as u64,
                3 => self.b.u16(a)? as u64,
                4 | 13 => self.b.u32(a)? as u64,
                16 | 18 => self.b.u64(a)?,
                _ => return Err(chyba(&format!("značka {z} má neočakávaný typ {}", p.typ))),
            });
        }
        Ok(Some(v))
    }

    fn cislo(&self, z: u16) -> Vysledok<Option<u64>> {
        Ok(self.cisla(z, 1 << 20)?.and_then(|v| v.first().copied()))
    }

    fn racionalne(&self, z: u16) -> Option<f64> {
        let p = self.najdi(z)?;
        if p.typ != 5 || p.pocet < 1 {
            return None;
        }
        let (c, m) = (self.b.u32(p.poloha).ok()?, self.b.u32(p.poloha + 4).ok()?);
        (m != 0).then(|| c as f64 / m as f64)
    }

    fn bajty(&self, z: u16, limit: u64) -> Vysledok<Option<&'a [u8]>> {
        let Some(p) = self.najdi(z) else { return Ok(None) };
        let n = p.pocet * velkost_typu(p.typ);
        if n > limit {
            return Err(Chyba::Limit(format!("TIFF: značka {z} má {n} B")));
        }
        Ok(Some(self.b.kus(p.poloha, n)?))
    }

    fn text(&self, z: u16) -> Option<String> {
        let b = self.bajty(z, 1 << 20).ok()??;
        let b = b.split(|&c| c == 0).next().unwrap_or(&[]);
        Some(String::from_utf8_lossy(b).trim().to_string()).filter(|t| !t.is_empty())
    }
}

/// Hlavička súboru a polohy všetkých adresárov (strán).
struct Subor<'a> {
    b: Bajty<'a>,
    big: bool,
}

impl<'a> Subor<'a> {
    fn otvor(d: &'a [u8]) -> Vysledok<(Subor<'a>, u64)> {
        if d.len() < 8 {
            return Err(chyba("krátky súbor"));
        }
        let le = match &d[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err(chyba("chýba poradie bajtov II/MM")),
        };
        let b = Bajty { d, le };
        match b.u16(2)? {
            42 => Ok((Subor { b, big: false }, b.u32(4)? as u64)),
            43 => {
                if b.u16(4)? != 8 || b.u16(6)? != 0 {
                    return Err(chyba("neplatná hlavička BigTIFF"));
                }
                Ok((Subor { b, big: true }, b.u64(8)?))
            }
            _ => Err(chyba("chýba číslo 42")),
        }
    }

    /// Prečíta adresár na polohe `p`; vráti ho aj polohu ďalšieho.
    fn adresar(&self, p: u64) -> Vysledok<(Adresar<'a>, u64)> {
        let (pocet, prvy, velkost, mala) =
            if self.big { (self.b.u64(p)?, p + 8, 20, 8) } else { (self.b.u16(p)? as u64, p + 2, 12, 4) };
        if pocet > 4096 {
            return Err(Chyba::Limit(format!("TIFF: adresár má {pocet} položiek")));
        }
        let mut polozky = Vec::with_capacity(pocet as usize);
        for i in 0..pocet {
            let e = prvy + i * velkost;
            let znacka = self.b.u16(e)?;
            let typ = self.b.u16(e + 2)?;
            let n = if self.big { self.b.u64(e + 4)? } else { self.b.u32(e + 4)? as u64 };
            let hodnota = e + if self.big { 12 } else { 8 };
            let bajtov = n.saturating_mul(velkost_typu(typ));
            let poloha = if bajtov <= mala {
                hodnota
            } else if self.big {
                self.b.u64(hodnota)?
            } else {
                self.b.u32(hodnota)? as u64
            };
            polozky.push(Polozka { znacka, typ, pocet: n, poloha });
        }
        let dalsi_p = prvy + pocet * velkost;
        let dalsi = if self.big { self.b.u64(dalsi_p)? } else { self.b.u32(dalsi_p)? as u64 };
        Ok((Adresar { b: self.b, polozky }, dalsi))
    }
}

/// Počet strán (adresárov) v súbore.
pub fn pocet_stran(data: &[u8]) -> Vysledok<usize> {
    let (s, mut p) = Subor::otvor(data)?;
    let mut n = 0;
    let mut videne = Vec::new();
    while p != 0 && n < 10_000 {
        if videne.contains(&p) {
            break; // slučka v reťazi adresárov
        }
        videne.push(p);
        p = s.adresar(p)?.1;
        n += 1;
    }
    Ok(n)
}

/// Prečíta prvú plnú stranu (miniatúry a zmenšené kópie sa preskočia).
pub fn citaj(data: &[u8], limity: &Limity) -> Vysledok<Obraz> {
    let (s, mut p) = Subor::otvor(data)?;
    let mut videne = Vec::new();
    let mut prvy = None;
    while p != 0 && videne.len() < 10_000 && !videne.contains(&p) {
        videne.push(p);
        let (a, dalsi) = s.adresar(p)?;
        let typ = a.cislo(254)?.unwrap_or(0);
        if typ & 1 == 0 {
            return s_exif(data, p, strana(&a, limity));
        }
        prvy.get_or_insert(p);
        p = dalsi;
    }
    match prvy {
        Some(p) => s_exif(data, p, strana(&s.adresar(p)?.0, limity)),
        None => Err(chyba("súbor nemá žiadnu stranu")),
    }
}

/// Prečíta stranu `n` (od nuly).
pub fn citaj_stranu(data: &[u8], n: usize, limity: &Limity) -> Vysledok<Obraz> {
    let (s, mut p) = Subor::otvor(data)?;
    for _ in 0..n {
        p = s.adresar(p)?.1;
        if p == 0 {
            return Err(Chyba::MimoRozsahu(format!("TIFF nemá stranu {n}")));
        }
    }
    s_exif(data, p, strana(&s.adresar(p)?.0, limity))
}

/// Doplní Exif strany (popisné značky IFD0 a podadresáre Exif a GPS) – len klasický TIFF.
fn s_exif(data: &[u8], ifd: u64, o: Vysledok<Obraz>) -> Vysledok<Obraz> {
    let mut o = o?;
    if o.metadata.exif.is_none() {
        o.metadata.exif = crate::exif::z_tiff(data, ifd as usize);
    }
    Ok(o)
}

/// Rozmery prvej strany bez dekódovania.
pub fn citaj_hlavicku(data: &[u8]) -> Vysledok<(usize, usize)> {
    let (s, p) = Subor::otvor(data)?;
    let a = s.adresar(p)?.0;
    let w = a.cislo(256)?.ok_or_else(|| chyba("chýba šírka"))?;
    let h = a.cislo(257)?.ok_or_else(|| chyba("chýba výška"))?;
    Ok((w as usize, h as usize))
}

/// Typ vzoriek strany.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Cele,
    SoZnamienkom,
    Plavajuce,
}

fn strana(a: &Adresar, limity: &Limity) -> Vysledok<Obraz> {
    let w = a.cislo(256)?.ok_or_else(|| chyba("chýba šírka"))?;
    let h = a.cislo(257)?.ok_or_else(|| chyba("chýba výška"))?;
    let spp = a.cislo(277)?.unwrap_or(1);
    if !(1..=16).contains(&spp) {
        return Err(chyba(&format!("{spp} zložiek na pixel")));
    }
    let bity = a.cisla(258, 64)?.unwrap_or_else(|| vec![1; spp as usize]);
    let bps = bity.first().copied().unwrap_or(1);
    if bity.iter().any(|&b| b != bps) {
        return Err(chyba("rôzne bity zložiek nie sú podporené"));
    }
    let format = match a.cislo(339)?.unwrap_or(1) {
        2 => Format::SoZnamienkom,
        3 => Format::Plavajuce,
        _ => Format::Cele,
    };
    let dovolene = match format {
        Format::Plavajuce => matches!(bps, 16 | 24 | 32 | 64),
        _ => matches!(bps, 1 | 2 | 4 | 8 | 10 | 12 | 14 | 16 | 32),
    };
    if !dovolene {
        return Err(chyba(&format!("{bps} bitov na vzorku ({format:?})")));
    }
    let kompresia = a.cislo(259)?.unwrap_or(1);
    let fotometria = a.cislo(262)?.unwrap_or(if spp >= 3 { 2 } else { 1 });
    let planarne = a.cislo(284)?.unwrap_or(1) == 2;
    // prediktor platí len pre LZW a Deflate (tak ho chápe aj libtiff; pri inej kompresii sa ignoruje)
    let prediktor = if matches!(kompresia, 5 | 8 | 32946) { a.cislo(317)?.unwrap_or(1) } else { 1 };
    let vystup_bajtov = if bps <= 8 && format == Format::Cele && fotometria != 3 { 1 } else { 4 };
    let kanaly_vystup = if fotometria == 3 { 3 } else { spp };
    limity.over(w, h, kanaly_vystup * vystup_bajtov)?;
    let (w, h, spp_u) = (w as usize, h as usize, spp as usize);

    // pruhy alebo dlaždice
    let dlazdice = a.najdi(322).is_some();
    let (kw, kh) = if dlazdice {
        let tw = a.cislo(322)?.ok_or_else(|| chyba("chýba šírka dlaždice"))? as usize;
        let th = a.cislo(323)?.ok_or_else(|| chyba("chýba výška dlaždice"))? as usize;
        if tw == 0 || th == 0 || tw > 1 << 16 || th > 1 << 16 {
            return Err(chyba("neplatný rozmer dlaždice"));
        }
        (tw, th)
    } else {
        let rps = a.cislo(278)?.unwrap_or(h as u64).min(h as u64).max(1) as usize;
        (w, rps)
    };
    let (zn_pol, zn_dl) = if dlazdice { (324, 325) } else { (273, 279) };
    let pocet_kusov = w.div_ceil(kw) * h.div_ceil(kh) * if planarne { spp_u } else { 1 };
    let polohy = a.cisla(zn_pol, 1 << 24)?.ok_or_else(|| chyba("chýbajú polohy údajov"))?;
    let dlzky = match a.cisla(zn_dl, 1 << 24)? {
        Some(d) => d,
        // chýbajúce dĺžky (staré súbory bez kompresie) – dopočítajú sa
        None if kompresia == 1 => vec![(kw * kh * spp_u * bps as usize).div_ceil(8) as u64; polohy.len()],
        None => return Err(chyba("chýbajú dĺžky údajov")),
    };
    if polohy.len() < pocet_kusov || dlzky.len() < pocet_kusov {
        return Err(chyba("málo pruhov alebo dlaždíc"));
    }
    let jpeg_tabulky = a.bajty(347, 1 << 20)?;
    let spp_kusu = if planarne { 1 } else { spp_u };
    let bajtov_riadku_kusu = (kw * spp_kusu * bps as usize).div_ceil(8);
    let bajtov_kusu = bajtov_riadku_kusu * kh;

    // všetky vzorky ako f64 alebo u32 by bolo veľa pamäte – rozbalené bajty v pôvodnom tvare
    // (riadky obrazu, zložky za sebou), z nich potom priamo výstup
    let bajtov_riadku = (w * spp_u * bps as usize).div_ceil(8);
    let mut surove = vec![0u8; bajtov_riadku * h];
    let stlpcov = w.div_ceil(kw);
    let kusov_v_rovine = stlpcov * h.div_ceil(kh);
    let mut jpeg_obraz: Option<Vec<u8>> = None;
    for k in 0..pocet_kusov {
        let rovina = k / kusov_v_rovine;
        let i = k % kusov_v_rovine;
        let (x0, y0) = ((i % stlpcov) * kw, (i / stlpcov) * kh);
        let udaje = a.b.kus(polohy[k], dlzky[k])?;
        let rozbalene: Vec<u8> = match kompresia {
            1 => udaje.to_vec(),
            5 => lzw_rozbal(udaje, bajtov_kusu)?,
            8 | 32946 => {
                let mut v = Vec::with_capacity(bajtov_kusu);
                // poškodený koniec toku nevadí, ak je údajov dosť (bežné u starých kodérov)
                match deflate::zlib_rozbal_do(udaje, bajtov_kusu, &mut v) {
                    Ok(_) => {}
                    Err(Chyba::Limit(e)) => return Err(Chyba::Limit(e)),
                    Err(e) if v.len() < bajtov_kusu => return Err(e),
                    Err(_) => {}
                }
                v
            }
            32773 => packbits_rozbal(udaje, bajtov_kusu)?,
            7 => {
                // JPEG v TIFF: tabuľky zo značky JPEGTables + údaje kúska
                let o = jpeg_kus(udaje, jpeg_tabulky, fotometria == 2, limity)?;
                let ciel = jpeg_obraz.get_or_insert_with(|| vec![0u8; w * h * o.kanaly()]);
                let Vzorky::U8(v) = &o.vzorky else { return Err(chyba("JPEG v TIFF s 12 bitmi")) };
                let k = o.kanaly();
                for y in 0..o.vyska.min(h.saturating_sub(y0)) {
                    for x in 0..o.sirka.min(w.saturating_sub(x0)) {
                        let z = (y * o.sirka + x) * k;
                        let c = ((y0 + y) * w + x0 + x) * k;
                        if c + k <= ciel.len() {
                            ciel[c..c + k].copy_from_slice(&v[z..z + k]);
                        }
                    }
                }
                continue;
            }
            n => return Err(chyba(&format!("kompresia {n} nie je podporená"))),
        };
        let mut kus = rozbalene;
        kus.resize(bajtov_kusu, 0);
        match prediktor {
            1 => {}
            2 => vodorovny_spat(&mut kus, kw, spp_kusu, bps as usize, a.b.le)?,
            3 => plavajuci_spat(&mut kus, kw, spp_kusu, bps as usize / 8, a.b.le)?,
            p => return Err(chyba(&format!("prediktor {p}"))),
        }
        // presun kúska do surových riadkov
        let vyska_kusu = kh.min(h - y0);
        let sirka_kusu = kw.min(w - x0);
        let bitov_px = spp_u * bps as usize;
        for y in 0..vyska_kusu {
            let zdroj = &kus[y * bajtov_riadku_kusu..(y + 1) * bajtov_riadku_kusu];
            let riadok = &mut surove[(y0 + y) * bajtov_riadku..(y0 + y + 1) * bajtov_riadku];
            if !planarne && bitov_px.is_multiple_of(8) {
                let b = bitov_px / 8;
                riadok[x0 * b..(x0 + sirka_kusu) * b].copy_from_slice(&zdroj[..sirka_kusu * b]);
            } else {
                // bity po jednom (nízke hĺbky, roviny)
                for x in 0..sirka_kusu {
                    for c in 0..spp_kusu {
                        let zc = if planarne { rovina } else { c };
                        let v = citaj_bity(zdroj, (x * spp_kusu + c) * bps as usize, bps as usize);
                        zapis_bity(riadok, ((x0 + x) * spp_u + zc) * bps as usize, bps as usize, v);
                    }
                }
            }
        }
    }
    let mut o = if let Some(v) = jpeg_obraz {
        let model = if v.len() == w * h {
            Model::Siva
        } else if v.len() == w * h * 4 {
            Model::Cmyk
        } else {
            Model::Rgb
        };
        Obraz::novy(w, h, model, false, Vzorky::U8(v))?
    } else {
        prevod_vzoriek(a, &surove, w, h, spp_u, bps as usize, format, fotometria)?
    };
    // farba a metadáta
    if let Some(icc) = a.bajty(34675, limity.metadat)? {
        o.farba.icc = Some(icc.to_vec());
    }
    o.farba.linearne = format == Format::Plavajuce;
    let mut meta = Metadata {
        natocenie: a.cislo(274)?.filter(|n| (1..=8).contains(n)).unwrap_or(1) as u8,
        ..o.metadata
    };
    if let Some(x) = a.bajty(700, limity.metadat)? {
        meta.xmp = Some(x.to_vec());
    }
    if let Some(x) = a.bajty(33723, limity.metadat)? {
        meta.iptc = Some(x.to_vec());
    }
    if let (Some(x), Some(y)) = (a.racionalne(282), a.racionalne(283)) {
        match a.cislo(296)?.unwrap_or(2) {
            2 => meta.dpi = Some((x, y)),
            3 => meta.dpi = Some((x * 2.54, y * 2.54)),
            _ => {}
        }
    }
    for (z, meno) in
        [(270, "Popis"), (305, "Program"), (306, "Dátum"), (315, "Autor"), (33432, "Autorské práva")]
    {
        if let Some(t) = a.text(z) {
            meta.texty.push((meno.into(), t));
        }
    }
    o.metadata = meta;
    Ok(o)
}

fn jpeg_kus(udaje: &[u8], tabulky: Option<&[u8]>, rgb: bool, limity: &Limity) -> Vysledok<Obraz> {
    let mut tok = vec![0xFF, 0xD8];
    if let Some(t) = tabulky {
        // tabuľky sú samostatný tok SOI … EOI
        let t = t.strip_prefix(&[0xFF, 0xD8]).unwrap_or(t);
        let t = t.strip_suffix(&[0xFF, 0xD9]).unwrap_or(t);
        tok.extend_from_slice(t);
    }
    if rgb {
        // značka Adobe s transformáciou 0 – zložky sú RGB, nie YCbCr
        tok.extend_from_slice(&[0xFF, 0xEE, 0, 14]);
        tok.extend_from_slice(b"Adobe\x00\x64\x00\x00\x00\x00\x00");
    }
    tok.extend_from_slice(udaje.strip_prefix(&[0xFF, 0xD8]).unwrap_or(udaje));
    crate::jpeg::citaj(&tok, limity)
}

#[inline]
fn citaj_bity(d: &[u8], bit: usize, n: usize) -> u64 {
    match n {
        8 => d[bit / 8] as u64,
        16 => u16::from_be_bytes([d[bit / 8], d[bit / 8 + 1]]) as u64,
        _ => {
            let mut v = 0u64;
            for k in 0..n {
                let b = bit + k;
                v = (v << 1) | ((d[b / 8] >> (7 - b % 8)) & 1) as u64;
            }
            v
        }
    }
}

#[inline]
fn zapis_bity(d: &mut [u8], bit: usize, n: usize, v: u64) {
    for k in 0..n {
        let b = bit + k;
        let x = ((v >> (n - 1 - k)) & 1) as u8;
        d[b / 8] = (d[b / 8] & !(0x80 >> (b % 8))) | (x << (7 - b % 8));
    }
}

/// Spätný vodorovný prediktor: každá vzorka je rozdiel od predchádzajúcej v riadku (rovnakej zložky).
fn vodorovny_spat(d: &mut [u8], w: usize, spp: usize, bps: usize, le: bool) -> Vysledok<()> {
    let riadok = (w * spp * bps).div_ceil(8);
    for r in d.chunks_exact_mut(riadok) {
        match bps {
            8 => {
                for i in spp..w * spp {
                    r[i] = r[i].wrapping_add(r[i - spp]);
                }
            }
            16 => {
                let g = |r: &[u8], i: usize| -> u16 {
                    let b = [r[2 * i], r[2 * i + 1]];
                    if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
                };
                for i in spp..w * spp {
                    let v = g(r, i).wrapping_add(g(r, i - spp));
                    let b = if le { v.to_le_bytes() } else { v.to_be_bytes() };
                    r[2 * i..2 * i + 2].copy_from_slice(&b);
                }
            }
            32 => {
                let g = |r: &[u8], i: usize| -> u32 {
                    let b = [r[4 * i], r[4 * i + 1], r[4 * i + 2], r[4 * i + 3]];
                    if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
                };
                for i in spp..w * spp {
                    let v = g(r, i).wrapping_add(g(r, i - spp));
                    let b = if le { v.to_le_bytes() } else { v.to_be_bytes() };
                    r[4 * i..4 * i + 4].copy_from_slice(&b);
                }
            }
            _ => {
                for x in 1..w {
                    for c in 0..spp {
                        let i = (x * spp + c) * bps;
                        let v = citaj_bity(r, i, bps).wrapping_add(citaj_bity(r, i - spp * bps, bps));
                        zapis_bity(r, i, bps, v & ((1 << bps) - 1));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Spätný plávajúci prediktor (Adobe TIFF Technical Note 3): bajty riadku sú rozdiely a usporiadané
/// po rovinách významnosti (najvyšší bajt všetkých vzoriek, potom ďalší …) v poradí big-endian.
fn plavajuci_spat(d: &mut [u8], w: usize, spp: usize, bajtov: usize, le: bool) -> Vysledok<()> {
    if !matches!(bajtov, 2..=8) {
        return Err(chyba("plávajúci prediktor pre túto hĺbku"));
    }
    let n = w * spp;
    let riadok = n * bajtov;
    let mut pomoc = vec![0u8; riadok];
    for r in d.chunks_exact_mut(riadok) {
        for i in spp..riadok {
            r[i] = r[i].wrapping_add(r[i - spp]);
        }
        for i in 0..n {
            for b in 0..bajtov {
                // rovina b obsahuje b-ty najvýznamnejší bajt
                let v = r[b * n + i];
                let poz = if le { bajtov - 1 - b } else { b };
                pomoc[i * bajtov + poz] = v;
            }
        }
        r.copy_from_slice(&pomoc);
    }
    Ok(())
}

/// Rozbalenie PackBits (Apple): n ≥ 0 → n + 1 bajtov doslova, n < 0 → nasledujúci bajt 1 − n krát.
fn packbits_rozbal(d: &[u8], ocakavane: usize) -> Vysledok<Vec<u8>> {
    let mut von = Vec::with_capacity(ocakavane);
    let mut i = 0;
    while i < d.len() && von.len() < ocakavane {
        let n = d[i] as i8;
        i += 1;
        if n >= 0 {
            let k = n as usize + 1;
            let kus = d.get(i..i + k).ok_or_else(|| chyba("PackBits za koncom"))?;
            von.extend_from_slice(kus);
            i += k;
        } else if n != -128 {
            let b = *d.get(i).ok_or_else(|| chyba("PackBits za koncom"))?;
            i += 1;
            von.extend(std::iter::repeat_n(b, (1 - n as isize) as usize));
        }
    }
    von.truncate(ocakavane);
    Ok(von)
}

/// Rozbalenie LZW v TIFF (kódy 9–12 bitov od najvyššieho, 256 = vymazanie, 257 = koniec, šírka kódu
/// rastie o jeden kód skôr – „early change“).
fn lzw_rozbal(d: &[u8], ocakavane: usize) -> Vysledok<Vec<u8>> {
    // slovník ako (predchodca, posledný bajt, dĺžka) – reťazec sa skladá odzadu
    let mut predchodca = vec![0u16; 4096];
    let mut bajt = vec![0u8; 4096];
    let mut dlzka = vec![0u16; 4096];
    for i in 0..256 {
        bajt[i] = i as u8;
        dlzka[i] = 1;
    }
    let mut von = Vec::with_capacity(ocakavane);
    let (mut akum, mut bitov, mut poz) = (0u32, 0u32, 0usize);
    let mut sirka = 9;
    let mut dalsi = 258usize;
    let mut stary: Option<u16> = None;
    let mut pomoc = Vec::with_capacity(4096);
    loop {
        while bitov < sirka {
            if poz >= d.len() {
                return Ok(von); // koniec bez kódu 257 – staré kodéry
            }
            akum = (akum << 8) | d[poz] as u32;
            poz += 1;
            bitov += 8;
        }
        let kod = ((akum >> (bitov - sirka)) & ((1 << sirka) - 1)) as usize;
        bitov -= sirka;
        if kod == 257 {
            break;
        }
        if kod == 256 {
            sirka = 9;
            dalsi = 258;
            stary = None;
            continue;
        }
        let zapis_retazec = |k: usize, pomoc: &mut Vec<u8>, von: &mut Vec<u8>| {
            pomoc.clear();
            let mut c = k;
            loop {
                pomoc.push(bajt[c]);
                if dlzka[c] == 1 {
                    break;
                }
                c = predchodca[c] as usize;
            }
            von.extend(pomoc.iter().rev());
            pomoc[pomoc.len() - 1]
        };
        match stary {
            None => {
                if kod > 255 {
                    return Err(chyba("LZW: prvý kód nie je bajt"));
                }
                von.push(kod as u8);
            }
            Some(s) => {
                let prvy = if kod < dalsi {
                    zapis_retazec(kod, &mut pomoc, &mut von)
                } else if kod == dalsi {
                    // kód, ktorý sa práve tvorí: reťazec starého + jeho prvý bajt
                    let p = zapis_retazec(s as usize, &mut pomoc, &mut von);
                    von.push(p);
                    p
                } else {
                    return Err(chyba("LZW: neplatný kód"));
                };
                if dalsi < 4096 {
                    predchodca[dalsi] = s;
                    bajt[dalsi] = prvy;
                    dlzka[dalsi] = dlzka[s as usize].saturating_add(1);
                    dalsi += 1;
                }
            }
        }
        stary = Some(kod as u16);
        if dalsi + 1 >= (1 << sirka) && sirka < 12 {
            sirka += 1;
        }
        if von.len() >= ocakavane {
            break;
        }
    }
    von.truncate(ocakavane);
    Ok(von)
}

/// Prevod surových vzoriek na výstupný obraz podľa fotometrie.
#[allow(clippy::too_many_arguments)]
fn prevod_vzoriek(
    a: &Adresar,
    surove: &[u8],
    w: usize,
    h: usize,
    spp: usize,
    bps: usize,
    format: Format,
    fotometria: u64,
) -> Vysledok<Obraz> {
    let le = a.b.le;
    let riadok = (w * spp * bps).div_ceil(8);
    let vzorka = |y: usize, i: usize| -> u64 {
        let r = &surove[y * riadok..(y + 1) * riadok];
        match bps {
            16 => {
                let b = [r[2 * i], r[2 * i + 1]];
                (if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }) as u64
            }
            32 => {
                let b = [r[4 * i], r[4 * i + 1], r[4 * i + 2], r[4 * i + 3]];
                (if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }) as u64
            }
            64 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&r[8 * i..8 * i + 8]);
                if le { u64::from_le_bytes(b) } else { u64::from_be_bytes(b) }
            }
            24 => {
                let b = &r[3 * i..3 * i + 3];
                if le {
                    b[0] as u64 | (b[1] as u64) << 8 | (b[2] as u64) << 16
                } else {
                    (b[0] as u64) << 16 | (b[1] as u64) << 8 | b[2] as u64
                }
            }
            _ => citaj_bity(r, i * bps, bps),
        }
    };
    // dodatočné zložky: prvá navyše je alfa, ak to ExtraSamples hovorí (1 = násobená, 2 = nenásobená)
    let extra = a.cisla(338, 16)?.unwrap_or_default();
    let zakladnych = match fotometria {
        0 | 1 | 3 => 1,
        5 => 4.min(spp),
        _ => 3.min(spp),
    };
    let alfa = spp > zakladnych && extra.first().is_some_and(|&e| e == 1 || e == 2);
    let nasobena = extra.first() == Some(&1);
    let model = match fotometria {
        0 | 1 => Model::Siva,
        2 | 3 | 6 => Model::Rgb,
        5 if zakladnych == 4 => Model::Cmyk,
        _ => Model::Iny,
    };
    if fotometria == 6 {
        return Err(chyba("nekomprimované YCbCr nie je podporené"));
    }
    if fotometria == 8 || fotometria == 9 || fotometria == 10 {
        return Err(chyba("CIE L*a*b* zatiaľ nie je podporené"));
    }
    let kanaly_von = if fotometria == 3 { 3 } else { zakladnych } + alfa as usize;
    let mut o = if fotometria == 3 {
        // paleta: 3 × 2^bps hodnôt po 16 bitoch (najprv všetky R, potom G, potom B)
        let mapa = a.cisla(320, 3 << 16)?.ok_or_else(|| chyba("paleta bez ColorMap"))?;
        let n = 1usize << bps;
        if mapa.len() < 3 * n || bps > 16 {
            return Err(chyba("krátka paleta"));
        }
        let mut v = Vec::with_capacity(w * h * kanaly_von);
        for y in 0..h {
            for x in 0..w {
                let i = vzorka(y, x * spp) as usize;
                for c in 0..3 {
                    v.push((mapa[c * n + i] >> 8) as u8);
                }
                if alfa {
                    v.push((vzorka(y, x * spp + 1) >> (bps.saturating_sub(8))) as u8);
                }
            }
        }
        Obraz::novy(w, h, Model::Rgb, alfa, Vzorky::U8(v))?
    } else {
        let biela_nula = fotometria == 0;
        let n = w * h * kanaly_von;
        let vybrat = |x: usize| -> usize {
            // výstupný kanál → vstupná zložka (alfa je prvá navyše)
            if x < zakladnych { x } else { zakladnych }
        };
        let vzorky = match (format, bps) {
            (Format::Cele, 1..=8) => {
                let max = (1u64 << bps) - 1;
                let mut v = Vec::with_capacity(n);
                for y in 0..h {
                    for x in 0..w {
                        for c in 0..kanaly_von {
                            let mut s = vzorka(y, x * spp + vybrat(c));
                            if biela_nula && c < zakladnych {
                                s = max - s;
                            }
                            v.push(((s * 255 + max / 2) / max) as u8);
                        }
                    }
                }
                Vzorky::U8(v)
            }
            (Format::Cele, 9..=16) => {
                let max = (1u64 << bps) - 1;
                let mut v = Vec::with_capacity(n);
                for y in 0..h {
                    for x in 0..w {
                        for c in 0..kanaly_von {
                            let mut s = vzorka(y, x * spp + vybrat(c));
                            if biela_nula && c < zakladnych {
                                s = max - s;
                            }
                            v.push(if bps == 16 { s as u16 } else { ((s * 65_535 + max / 2) / max) as u16 });
                        }
                    }
                }
                Vzorky::U16(v)
            }
            _ => {
                let mut v = Vec::with_capacity(n);
                for y in 0..h {
                    for x in 0..w {
                        for c in 0..kanaly_von {
                            let s = vzorka(y, x * spp + vybrat(c));
                            let mut f = match (format, bps) {
                                (Format::Plavajuce, 16) => f16_na_f32(s as u16),
                                (Format::Plavajuce, 24) => f24_na_f32(s as u32),
                                (Format::Plavajuce, 32) => f32::from_bits(s as u32),
                                (Format::Plavajuce, _) => f64::from_bits(s) as f32,
                                (Format::SoZnamienkom, 8) => (s as u8 as i8) as f32 / 127.0,
                                (Format::SoZnamienkom, 16) => (s as u16 as i16) as f32 / 32_767.0,
                                (Format::SoZnamienkom, _) => (s as u32 as i32) as f32 / 2_147_483_647.0,
                                _ => s as f32 / ((1u64 << bps) - 1) as f32,
                            };
                            if biela_nula && c < zakladnych {
                                f = 1.0 - f;
                            }
                            v.push(f);
                        }
                    }
                }
                Vzorky::F32(v)
            }
        };
        let mut o = Obraz::novy(w, h, model, alfa, vzorky)?;
        if model == Model::Iny {
            o.mena_kanalov = (0..kanaly_von).map(|i| format!("K{i}")).collect();
        }
        o
    };
    if nasobena && alfa {
        // výstup je nenásobená alfa
        crate::obraz::rozdel_alfu(&mut o);
    }
    o.bitov = bps.min(32) as u8;
    Ok(o)
}

/// 24-bitové plávajúce (1 + 7 + 16 bitov, používajú niektoré skenery a Photoshop).
fn f24_na_f32(v: u32) -> f32 {
    let znamienko = (v & 0x80_0000) << 8;
    let exp = (v >> 16) & 0x7F;
    let mant = v & 0xFFFF;
    if exp == 0 {
        return f32::from_bits(znamienko);
    }
    if exp == 0x7F {
        return f32::from_bits(znamienko | 0x7F80_0000 | mant << 7);
    }
    f32::from_bits(znamienko | (exp + 64) << 23 | mant << 7)
}

/// Nastavenie zápisu TIFF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NastavenieTiff {
    /// Kompresia Deflate s prediktorom (inak bez kompresie).
    pub deflate: bool,
    pub uroven: u8,
}

impl Default for NastavenieTiff {
    fn default() -> Self {
        NastavenieTiff { deflate: true, uroven: 6 }
    }
}

/// Zapíše obrázok do TIFF (little-endian; BigTIFF, ak by súbor presiahol 4 GB).
pub fn zapis(o: &Obraz, n: &NastavenieTiff) -> Vysledok<Vec<u8>> {
    o.over()?;
    let (fotometria, zakladnych) = match o.model {
        Model::Siva => (1u16, 1usize),
        Model::Rgb => (2, 3),
        Model::Cmyk => (5, 4),
        Model::Iny => (1, o.kanaly()),
    };
    let k = o.kanaly();
    let (bps, format, bajtov) = match o.vzorky {
        Vzorky::U8(_) => (8u16, 1u16, 1usize),
        Vzorky::U16(_) => (16, 1, 2),
        Vzorky::F32(_) => (32, 3, 4),
    };
    let (w, h) = (o.sirka, o.vyska);
    let bajtov_riadku = w * k * bajtov;
    let riadkov_pruhu = (256 * 1024 / bajtov_riadku.max(1)).clamp(1, h);
    let prediktor: u16 = if !n.deflate {
        1
    } else if format == 3 {
        3
    } else {
        2
    };
    // pruhy
    let mut pruhy: Vec<Vec<u8>> = Vec::new();
    let mut y = 0;
    while y < h {
        let r = riadkov_pruhu.min(h - y);
        let mut d = Vec::with_capacity(r * bajtov_riadku);
        for yy in y..y + r {
            let z = yy * w * k;
            match &o.vzorky {
                Vzorky::U8(v) => d.extend_from_slice(&v[z..z + w * k]),
                Vzorky::U16(v) => v[z..z + w * k].iter().for_each(|x| d.extend_from_slice(&x.to_le_bytes())),
                Vzorky::F32(v) => v[z..z + w * k].iter().for_each(|x| d.extend_from_slice(&x.to_le_bytes())),
            }
        }
        if n.deflate {
            for riadok in d.chunks_exact_mut(bajtov_riadku) {
                if prediktor == 2 {
                    vodorovny_tam(riadok, w * k, k, bajtov);
                } else {
                    plavajuci_tam(riadok, w * k, k, bajtov);
                }
            }
            d = deflate::zlib_zbal(&d, n.uroven);
        }
        pruhy.push(d);
        y += r;
    }
    let spolu: u64 = pruhy.iter().map(|p| p.len() as u64).sum::<u64>()
        + o.farba.icc.as_ref().map_or(0, |i| i.len() as u64);
    let big = spolu + 1_000_000 > u32::MAX as u64;
    // položky adresára (značka, typ, hodnoty)
    let mut polozky: Vec<(u16, u16, Vec<u8>)> = Vec::new();
    let skratky = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let dlhe = |v: &[u32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    polozky.push((256, 4, dlhe(&[w as u32])));
    polozky.push((257, 4, dlhe(&[h as u32])));
    polozky.push((258, 3, skratky(&vec![bps; k])));
    polozky.push((259, 3, skratky(&[if n.deflate { 8 } else { 1 }])));
    polozky.push((262, 3, skratky(&[fotometria])));
    if o.metadata.natocenie > 1 {
        polozky.push((274, 3, skratky(&[o.metadata.natocenie as u16])));
    }
    polozky.push((277, 3, skratky(&[k as u16])));
    polozky.push((278, 4, dlhe(&[riadkov_pruhu as u32])));
    if let Some((x, yy)) = o.metadata.dpi {
        let rac = |v: f64| -> Vec<u8> {
            let m = 10_000u32;
            let mut b = ((v * m as f64).round() as u32).to_le_bytes().to_vec();
            b.extend_from_slice(&m.to_le_bytes());
            b
        };
        polozky.push((282, 5, rac(x)));
        polozky.push((283, 5, rac(yy)));
        polozky.push((296, 3, skratky(&[2])));
    }
    polozky.push((284, 3, skratky(&[1])));
    let mut program = b"ORCHIDEA\0".to_vec();
    if program.len() % 2 == 1 {
        program.push(0);
    }
    polozky.push((305, 2, program));
    if prediktor > 1 {
        polozky.push((317, 3, skratky(&[prediktor])));
    }
    if k > zakladnych {
        let mut e = vec![0u16; k - zakladnych];
        if o.alfa {
            e[k - zakladnych - 1] = 2;
        }
        polozky.push((338, 3, skratky(&e)));
    }
    polozky.push((339, 3, skratky(&vec![format; k])));
    if let Some(x) = &o.metadata.xmp {
        polozky.push((700, 1, x.clone()));
    }
    if let Some(icc) = &o.farba.icc {
        polozky.push((34675, 7, icc.clone()));
    }
    if let Some(i) = &o.metadata.iptc {
        polozky.push((33723, 7, i.clone()));
    }
    // Exif: popisné značky IFD0, ktoré ešte nie sú zapísané, a podadresáre Exif a GPS (tie len v klasickom TIFF,
    // zapíšu sa za pruhy, ukazovatele sa doplnia nižšie)
    let exif = o.metadata.exif.as_deref().and_then(crate::exif::rozober);
    if let Some(e) = &exif {
        for q in &e.ifd0 {
            if [270, 271, 272, 306, 315, 316, 33432].contains(&q.znacka)
                && !polozky.iter().any(|p| p.0 == q.znacka)
            {
                polozky.push((q.znacka, q.typ, q.hodnota.clone()));
            }
        }
        if !big && (!e.exif.is_empty() || !e.interop.is_empty()) {
            polozky.push((34665, 4, vec![0; 4]));
        }
        if !big && !e.gps.is_empty() {
            polozky.push((34853, 4, vec![0; 4]));
        }
    }
    // polohy pruhov sa doplnia po rozložení súboru
    polozky.push((273, if big { 16 } else { 4 }, vec![0; pruhy.len() * if big { 8 } else { 4 }]));
    polozky.push((279, if big { 16 } else { 4 }, vec![0; pruhy.len() * if big { 8 } else { 4 }]));
    polozky.sort_by_key(|p| p.0);

    let mut von: Vec<u8> = if big {
        let mut v = b"II\x2B\x00\x08\x00\x00\x00".to_vec();
        v.extend_from_slice(&16u64.to_le_bytes());
        v
    } else {
        b"II\x2A\x00\x08\x00\x00\x00".to_vec()
    };
    let (velkost, mala) = if big { (20usize, 8usize) } else { (12, 4) };
    let zaciatok_adr = von.len();
    let dlzka_adr = if big { 8 } else { 2 } + polozky.len() * velkost + if big { 8 } else { 4 };
    // dlhé hodnoty za adresárom, potom pruhy
    let mut dalsie = zaciatok_adr + dlzka_adr;
    let mut polohy_hodnot = Vec::new();
    for p in &polozky {
        if p.2.len() > mala {
            dalsie += dalsie % 2;
            polohy_hodnot.push(Some(dalsie));
            dalsie += p.2.len();
        } else {
            polohy_hodnot.push(None);
        }
    }
    let mut polohy_pruhov = Vec::new();
    for p in &pruhy {
        dalsie += dalsie % 2;
        polohy_pruhov.push(dalsie as u64);
        dalsie += p.len();
    }
    // podadresáre Exif a GPS na koniec súboru
    dalsie += dalsie % 2;
    let (podadresare, poloha_exif, poloha_gps) = match &exif {
        Some(e) if !big => e.zapis_podadresare(dalsie),
        _ => (Vec::new(), None, None),
    };
    let zaciatok_podadresarov = dalsie;
    for p in polozky.iter_mut() {
        match p.0 {
            34665 => p.2 = poloha_exif.unwrap_or(0).to_le_bytes().to_vec(),
            34853 => p.2 = poloha_gps.unwrap_or(0).to_le_bytes().to_vec(),
            _ => {}
        }
    }
    // doplnenie polôh a dĺžok pruhov
    for p in polozky.iter_mut() {
        let hodnoty: Vec<u64> = match p.0 {
            273 => polohy_pruhov.clone(),
            279 => pruhy.iter().map(|x| x.len() as u64).collect(),
            _ => continue,
        };
        p.2 = if big {
            hodnoty.iter().flat_map(|x| x.to_le_bytes()).collect()
        } else {
            hodnoty.iter().flat_map(|&x| (x as u32).to_le_bytes()).collect()
        };
    }
    if big {
        von.extend_from_slice(&(polozky.len() as u64).to_le_bytes());
    } else {
        von.extend_from_slice(&(polozky.len() as u16).to_le_bytes());
    }
    for (p, poloha) in polozky.iter().zip(&polohy_hodnot) {
        let pocet = p.2.len() as u64 / velkost_typu(p.1);
        von.extend_from_slice(&p.0.to_le_bytes());
        von.extend_from_slice(&p.1.to_le_bytes());
        if big {
            von.extend_from_slice(&pocet.to_le_bytes());
        } else {
            von.extend_from_slice(&(pocet as u32).to_le_bytes());
        }
        match poloha {
            Some(x) if big => von.extend_from_slice(&(*x as u64).to_le_bytes()),
            Some(x) => von.extend_from_slice(&(*x as u32).to_le_bytes()),
            None => {
                let mut b = p.2.clone();
                b.resize(mala, 0);
                von.extend_from_slice(&b);
            }
        }
    }
    von.extend_from_slice(&vec![0u8; if big { 8 } else { 4 }]);
    for (p, poloha) in polozky.iter().zip(&polohy_hodnot) {
        if let Some(x) = poloha {
            von.resize(*x, 0);
            von.extend_from_slice(&p.2);
        }
    }
    for (p, &x) in pruhy.iter().zip(&polohy_pruhov) {
        von.resize(x as usize, 0);
        von.extend_from_slice(p);
    }
    if !podadresare.is_empty() {
        von.resize(zaciatok_podadresarov, 0);
        von.extend_from_slice(&podadresare);
    }
    Ok(von)
}

/// Vodorovný prediktor pri zápise (little-endian vzorky).
fn vodorovny_tam(r: &mut [u8], n: usize, spp: usize, bajtov: usize) {
    match bajtov {
        1 => {
            for i in (spp..n).rev() {
                r[i] = r[i].wrapping_sub(r[i - spp]);
            }
        }
        _ => {
            for i in (spp..n).rev() {
                let a = u16::from_le_bytes([r[2 * i], r[2 * i + 1]]);
                let b = u16::from_le_bytes([r[2 * (i - spp)], r[2 * (i - spp) + 1]]);
                r[2 * i..2 * i + 2].copy_from_slice(&a.wrapping_sub(b).to_le_bytes());
            }
        }
    }
}

/// Plávajúci prediktor pri zápise: bajty po rovinách významnosti, potom rozdiely.
fn plavajuci_tam(r: &mut [u8], n: usize, spp: usize, bajtov: usize) {
    let mut pomoc = vec![0u8; r.len()];
    for i in 0..n {
        for b in 0..bajtov {
            pomoc[b * n + i] = r[i * bajtov + (bajtov - 1 - b)];
        }
    }
    for i in (spp..r.len()).rev() {
        pomoc[i] = pomoc[i].wrapping_sub(pomoc[i - spp]);
    }
    r.copy_from_slice(&pomoc);
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    #[test]
    fn tam_a_spat() {
        let mut n = Nahoda::zo_semienka(6);
        for (model, alfa) in
            [(Model::Siva, false), (Model::Rgb, true), (Model::Cmyk, false), (Model::Rgb, false)]
        {
            for typ in 0..3 {
                let (w, h) = (1 + n.mensie_ako(70) as usize, 1 + n.mensie_ako(40) as usize);
                let k = match model {
                    Model::Siva => 1,
                    Model::Cmyk => 4,
                    _ => 3,
                } + alfa as usize;
                let vzorky = match typ {
                    0 => Vzorky::U8((0..w * h * k).map(|_| n.dalsie() as u8).collect()),
                    1 => Vzorky::U16((0..w * h * k).map(|_| n.dalsie() as u16).collect()),
                    _ => Vzorky::F32((0..w * h * k).map(|_| n.f32() * 4.0 - 1.0).collect()),
                };
                let mut o = Obraz::novy(w, h, model, alfa, vzorky).unwrap();
                o.farba.icc = Some(vec![9; 500]);
                o.metadata.dpi = Some((300.0, 150.0));
                for deflate in [false, true] {
                    let t = zapis(&o, &NastavenieTiff { deflate, uroven: 6 }).unwrap();
                    let s = citaj(&t, &Limity::default()).unwrap();
                    assert_eq!(s.vzorky, o.vzorky, "{model:?} alfa {alfa} typ {typ} deflate {deflate}");
                    assert_eq!((s.model, s.alfa), (model, alfa));
                    assert_eq!(s.farba.icc, o.farba.icc);
                    assert_eq!(s.metadata.dpi, Some((300.0, 150.0)));
                }
            }
        }
    }

    #[test]
    fn lzw_a_packbits() {
        // PackBits z dokumentácie Apple
        let p = [0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7, 0xAA];
        let v = packbits_rozbal(&p, 24).unwrap();
        assert_eq!(v.len(), 24);
        assert_eq!(&v[..6], &[0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A]);
        // LZW: kód 256 (vymazanie), bajty 7, 7, 7 a kód 259 (7 7) – šírka 9 bitov, potom 257
        let kody = [256u32, 7, 7, 258, 257];
        let mut bity = String::new();
        for k in kody {
            bity += &format!("{k:09b}");
        }
        bity += &"0".repeat((8 - bity.len() % 8) % 8);
        let d: Vec<u8> =
            (0..bity.len() / 8).map(|i| u8::from_str_radix(&bity[8 * i..8 * i + 8], 2).unwrap()).collect();
        assert_eq!(lzw_rozbal(&d, 100).unwrap(), vec![7, 7, 7, 7]);
    }
}
