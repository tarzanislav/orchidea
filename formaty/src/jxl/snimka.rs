//! Snímka JPEG XL: tabuľka obsahu (TOC), sekcie LfGlobal, LfGroup, HfGlobal a PassGroup, režim Modular
//! po skupinách; VarDCT v module `vardct`.

use super::bity::{Citac, D, ceil_log2, chyba};
use super::entropia::{Histogramy, TokZnakov};
use super::hlavicky::{HlavickaSnimky, Metadata, PRIZNAK_KRIVKY, PRIZNAK_SUM, PRIZNAK_ZAPLATY};
use super::modular::{self, Kanal, Obrazok, StromAKody, Transformacia};
use super::vardct;
use crate::obraz::Limity;
use orchidea_jadro::Vysledok;

/// Rozmery snímky a skupín.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rozmery {
    pub w: usize,
    pub h: usize,
    pub skupina: usize,
    pub skup_x: usize,
    pub skup_y: usize,
    pub lf_x: usize,
    pub lf_y: usize,
}

impl Rozmery {
    pub fn skupin(&self) -> usize {
        self.skup_x * self.skup_y
    }
    pub fn lf_skupin(&self) -> usize {
        self.lf_x * self.lf_y
    }
}

/// Výsledok snímky: kanály v plnom rozlíšení snímky (farba ako f32 v lineárnom svetle pri XYB, alebo celé
/// čísla pri Modular bez XYB) a dodatočné kanály.
pub(super) enum Vysledok3 {
    /// Celé vzorky Modular (farebné kanály, potom dodatočné).
    Cele(Vec<Kanal>),
    /// Plávajúce farebné roviny v lineárnom RGB (z XYB) a celé dodatočné kanály.
    Plavajuce([Vec<f32>; 3], Vec<Kanal>),
    /// Roviny X, Y, B pred prevodom farieb (LF snímka pre nasledujúcu snímku).
    Xyb([Vec<f32>; 3]),
    /// Plávajúce farebné roviny (1 alebo 3) v prenosovej funkcii súboru, rozsah 0–1, a dodatočné kanály.
    Kodovane(Vec<Vec<f32>>, Vec<Kanal>),
}

pub(super) struct Snimka {
    pub hlavicka: HlavickaSnimky,
    pub vysledok: Vysledok3,
}

/// Permutácia z Lehmerovho kódu (TOC alebo poradie koeficientov) čítaná zo zdieľaného toku `t`.
pub(super) fn permutacia(
    c: &mut Citac,
    h: &Histogramy,
    t: &mut TokZnakov,
    n: usize,
    preskoc: usize,
) -> Vysledok<Vec<usize>> {
    let ctx = |x: u32| ceil_log2(x.saturating_add(1)).min(7) as usize;
    let koniec = t.citaj(h, c, ctx(n as u32))? as usize;
    if preskoc > n || koniec > n - preskoc {
        return Err(chyba("neplatná dĺžka permutácie"));
    }
    let m = n - preskoc;
    let mut lehmer = Vec::with_capacity(koniec);
    let mut predosly = 0u32;
    for i in 0..koniec {
        let v = t.citaj(h, c, ctx(predosly))?;
        if v as usize >= m - i {
            return Err(chyba("neplatný Lehmerov kód"));
        }
        lehmer.push(v as usize);
        predosly = v;
    }
    // výber k-teho zostávajúceho prvku Fenwickovým stromom (O(n log n) aj pre 65 536 prvkov)
    let mut strom = vec![0u32; m + 1];
    for i in 1..=m {
        strom[i] += 1;
        let j = i + (i & i.wrapping_neg());
        if j <= m {
            strom[j] += strom[i];
        }
    }
    let mut najvyssi = 1usize;
    while najvyssi * 2 <= m {
        najvyssi *= 2;
    }
    let mut pouzite = vec![false; m];
    let mut p: Vec<usize> = (0..preskoc).collect();
    p.reserve(m);
    for &l in &lehmer {
        let (mut poz, mut zostava) = (0usize, l as u32 + 1);
        let mut krok = najvyssi;
        while krok > 0 {
            if poz + krok <= m && strom[poz + krok] < zostava {
                poz += krok;
                zostava -= strom[poz];
            }
            krok >>= 1;
        }
        let mut i = poz + 1;
        while i <= m {
            strom[i] -= 1;
            i += i & i.wrapping_neg();
        }
        pouzite[poz] = true;
        p.push(preskoc + poz);
    }
    p.extend((0..m).filter(|&i| !pouzite[i]).map(|i| preskoc + i));
    Ok(p)
}

pub(super) fn dekoduj(
    c: &mut Citac,
    data: &[u8],
    m: &Metadata,
    sirka: u32,
    vyska: u32,
    lf_snimky: &[Option<[Vec<f32>; 3]>],
    limity: &Limity,
) -> Vysledok<(Snimka, usize)> {
    let h = super::hlavicky::hlavicka_snimky(c, m, sirka, vyska)?;
    // rozmery snímky (aj výrez) pred akoukoľvek alokáciou; vzorky i32/f32 vo viacerých kópiách
    let kanalov = 3 + m.dodatocne.len() as u64;
    limity.over(h.sirka as u64, h.vyska as u64, kanalov * 4 * 3)?;
    if h.priznaky & (PRIZNAK_ZAPLATY | PRIZNAK_KRIVKY | PRIZNAK_SUM) != 0 && h.modular {
        return Err(chyba("záplaty, krivky a šum v režime Modular zatiaľ nie sú podporené"));
    }
    if h.zvacsenie_dodatocnych.iter().any(|&z| z != h.zvacsenie) {
        return Err(chyba("iné zväčšenie dodatočných kanálov ako farby zatiaľ nie je podporené"));
    }
    let mut w = (h.sirka as usize).div_ceil(h.zvacsenie as usize);
    let mut hh = (h.vyska as usize).div_ceil(h.zvacsenie as usize);
    if h.lf_uroven > 0 {
        let d = 1usize << (3 * h.lf_uroven);
        w = w.div_ceil(d);
        hh = hh.div_ceil(d);
    }
    let skupina = 128usize << h.posun_skupiny;
    let roz = Rozmery {
        w,
        h: hh,
        skupina,
        skup_x: w.div_ceil(skupina),
        skup_y: hh.div_ceil(skupina),
        lf_x: w.div_ceil(skupina * 8),
        lf_y: hh.div_ceil(skupina * 8),
    };
    let prechodov = h.prechody.pocet as usize;
    let jedna = roz.skupin() == 1 && prechodov == 1;
    let poctov = if jedna { 1 } else { 1 + roz.lf_skupin() + 1 + roz.skupin() * prechodov };
    let perm = if c.bool() {
        let hp = Histogramy::citaj(c, 8)?;
        let mut t = hp.citac(c);
        let p = permutacia(c, &hp, &mut t, poctov, 0)?;
        t.koniec(&hp)?;
        Some(p)
    } else {
        None
    };
    c.zarovnaj();
    let mut velkosti = Vec::with_capacity(poctov);
    for _ in 0..poctov {
        velkosti.push(c.u32([D::B(10, 0), D::B(14, 1024), D::B(22, 17_408), D::B(30, 4_211_712)]) as usize);
    }
    c.zarovnaj();
    c.over()?;
    let zaciatok = (c.precitane() / 8) as usize;
    let mut polohy = Vec::with_capacity(poctov);
    let mut p = zaciatok;
    for &v in &velkosti {
        polohy.push(p);
        p = p.checked_add(v).ok_or_else(|| chyba("sekcia mimo súboru"))?;
    }
    let koniec = p;
    if koniec > data.len() {
        return Err(chyba("sekcie presahujú údaje"));
    }
    let sekcie: Vec<&[u8]> = (0..poctov)
        .map(|i| {
            let j = perm.as_ref().map_or(i, |pm| pm[i]);
            &data[polohy[j]..polohy[j] + velkosti[j]]
        })
        .collect();
    let mut citace: Vec<Citac> = sekcie.iter().map(|s| Citac::novy(s)).collect();
    let mut stav = Stav::novy(&h, m, roz)?;
    // LF snímka úrovne n slúži snímke úrovne n − 1 (bežná snímka má úroveň 0)
    stav.lf_snimka = lf_snimky.get(h.lf_uroven as usize + 1).and_then(|x| x.as_ref());
    // LfGlobal
    let idx = |i: usize| if jedna { 0 } else { i };
    stav.lf_global(&mut citace[idx(0)])?;
    for g in 0..roz.lf_skupin() {
        stav.lf_skupina(&mut citace[idx(1 + g)], g)?;
    }
    stav.hf_global(&mut citace[idx(1 + roz.lf_skupin())])?;
    for prechod in 0..prechodov {
        let zac = 2 + roz.lf_skupin() + prechod * roz.skupin();
        if let Some(v) = stav.vardct.as_mut() {
            // koeficienty VarDCT všetkých skupín naraz (paralelne), potom údaje Modular skupín
            let c: Vec<&mut Citac> = if jedna {
                vec![&mut citace[0]]
            } else {
                citace[zac..zac + roz.skupin()].iter_mut().collect()
            };
            v.prechod(prechod, c)?;
        }
        for g in 0..roz.skupin() {
            stav.skupina(&mut citace[idx(zac + g)], prechod, g)?;
        }
    }
    for cit in &citace {
        cit.over()?;
    }
    let vysledok = stav.dokonci()?;
    Ok((Snimka { hlavicka: h, vysledok }, koniec))
}

/// Stav dekódovania jednej snímky.
struct Stav<'m, 'l> {
    h: HlavickaSnimky,
    m: &'m Metadata,
    roz: Rozmery,
    obraz: Obrazok,
    globalnych: usize,
    strom: Option<StromAKody>,
    transformacie: Vec<Transformacia>,
    farebnych: usize,
    vardct: Option<vardct::Stav>,
    lf_dekv: [f32; 3],
    lf_snimka: Option<&'l [Vec<f32>; 3]>,
}

impl<'m, 'l> Stav<'m, 'l> {
    fn novy(h: &HlavickaSnimky, m: &'m Metadata, roz: Rozmery) -> Vysledok<Stav<'m, 'l>> {
        let sivy = m.farba.priestor == 1 && !m.xyb;
        let farebnych = if h.modular { if sivy { 1 } else { 3 } } else { 0 };
        let mut kanaly = Vec::new();
        for _ in 0..farebnych {
            kanaly.push(Kanal::novy(roz.w, roz.h, 0, 0));
        }
        for d in &m.dodatocne {
            if d.posun_rozmeru != 0 {
                return Err(chyba("dodatočné kanály so zmenšením zatiaľ nie sú podporené"));
            }
            kanaly.push(Kanal::novy(roz.w, roz.h, 0, 0));
        }
        Ok(Stav {
            h: h.clone(),
            m,
            roz,
            obraz: Obrazok { kanaly, meta: 0, bitov: m.hlbka.bitov },
            globalnych: 0,
            strom: None,
            transformacie: Vec::new(),
            farebnych,
            vardct: None,
            lf_dekv: [1.0 / 4096.0, 1.0 / 512.0, 1.0 / 256.0],
            lf_snimka: None,
        })
    }

    fn lf_global(&mut self, c: &mut Citac) -> Vysledok<()> {
        if self.h.priznaky & PRIZNAK_ZAPLATY != 0 {
            return Err(chyba("záplaty (patches) zatiaľ nie sú podporené"));
        }
        if self.h.priznaky & PRIZNAK_KRIVKY != 0 {
            return Err(chyba("krivky (splines) zatiaľ nie sú podporené"));
        }
        if self.h.priznaky & PRIZNAK_SUM != 0 {
            return Err(chyba("šum (noise) zatiaľ nie je podporený"));
        }
        // dekvantizácia LF
        let mut lf_dekv = [1.0f32 / 4096.0, 1.0 / 512.0, 1.0 / 256.0];
        if !c.bool() {
            for x in lf_dekv.iter_mut() {
                *x = c.f16()? / 128.0;
            }
        }
        self.lf_dekv = lf_dekv;
        if !self.h.modular {
            self.vardct =
                Some(vardct::Stav::lf_global(c, &self.h, self.m, self.roz, lf_dekv, self.lf_snimka)?);
        }
        if c.bool() {
            let pixelov: usize = self.obraz.kanaly.iter().map(|k| k.w * k.h).sum();
            self.strom = Some(modular::citaj_strom(c, (1024 + pixelov).min(1 << 22))?);
        }
        let t = modular::dekoduj(c, &mut self.obraz, 0, self.strom.as_ref(), self.roz.skupina)?;
        self.transformacie = t;
        // kanály dekódované globálne: všetky meta a menšie ako skupina až po prvý väčší
        let o = &self.obraz;
        self.globalnych = (o.meta..o.kanaly.len())
            .find(|&i| o.kanaly[i].w > self.roz.skupina || o.kanaly[i].h > self.roz.skupina)
            .unwrap_or(o.kanaly.len());
        Ok(())
    }

    fn lf_skupina(&mut self, c: &mut Citac, g: usize) -> Vysledok<()> {
        if let Some(v) = self.vardct.as_mut() {
            v.lf_skupina(c, g, self.strom.as_ref())?;
        }
        let d = self.roz.skupina * 8;
        let (x0, y0) = ((g % self.roz.lf_x) * d, (g / self.roz.lf_x) * d);
        let stream = 1 + self.roz.lf_skupin() + g;
        self.modular_skupina(c, x0, y0, d, 3, i32::MAX, stream as u32)?;
        if let Some(v) = self.vardct.as_mut() {
            v.hf_metadata(c, g, self.strom.as_ref())?;
        }
        Ok(())
    }

    fn hf_global(&mut self, c: &mut Citac) -> Vysledok<()> {
        if let Some(v) = self.vardct.as_mut() {
            v.hf_global(c, self.strom.as_ref())?;
        }
        Ok(())
    }

    fn skupina(&mut self, c: &mut Citac, prechod: usize, g: usize) -> Vysledok<()> {
        let (min, max) = rozsah_posunov(&self.h, prechod);
        let d = self.roz.skupina;
        let (x0, y0) = ((g % self.roz.skup_x) * d, (g / self.roz.skup_x) * d);
        let stream = 1 + 3 * self.roz.lf_skupin() + 17 + self.roz.skupin() * prechod + g;
        self.modular_skupina(c, x0, y0, d, min, max, stream as u32)
    }

    /// Kanály s posunom v rozsahu [min, max], ktoré sa nedekódovali globálne, v obdĺžniku skupiny.
    #[allow(clippy::too_many_arguments)]
    fn modular_skupina(
        &mut self,
        c: &mut Citac,
        x0: usize,
        y0: usize,
        d: usize,
        min: i32,
        max: i32,
        stream: u32,
    ) -> Vysledok<()> {
        let mut vybrane = Vec::new();
        let mut kanaly = Vec::new();
        for i in self.globalnych..self.obraz.kanaly.len() {
            let k = &self.obraz.kanaly[i];
            let posun = k.hshift.min(k.vshift);
            if posun < min || posun > max {
                continue;
            }
            let (rx, ry) = (x0 >> k.hshift, y0 >> k.vshift);
            if rx >= k.w || ry >= k.h {
                continue;
            }
            let rw = (d >> k.hshift).min(k.w - rx);
            let rh = (d >> k.vshift).min(k.h - ry);
            if rw == 0 || rh == 0 {
                continue;
            }
            vybrane.push((i, rx, ry));
            kanaly.push(Kanal::novy(rw, rh, k.hshift, k.vshift));
        }
        if kanaly.is_empty() {
            return Ok(());
        }
        let mut o = Obrazok { kanaly, meta: 0, bitov: self.obraz.bitov };
        let t = modular::dekoduj(c, &mut o, stream, self.strom.as_ref(), usize::MAX)?;
        modular::spat(&mut o, &t)?;
        if o.kanaly.len() != vybrane.len() {
            return Err(chyba("skupina zmenila počet kanálov"));
        }
        for (k, &(i, rx, ry)) in o.kanaly.iter().zip(&vybrane) {
            let ciel = &mut self.obraz.kanaly[i];
            if ciel.data.len() != ciel.w * ciel.h {
                ciel.data = vec![0; ciel.w * ciel.h];
            }
            for y in 0..k.h {
                let z = (ry + y) * ciel.w + rx;
                ciel.data[z..z + k.w].copy_from_slice(&k.data[y * k.w..(y + 1) * k.w]);
            }
        }
        Ok(())
    }

    fn dokonci(mut self) -> Vysledok<Vysledok3> {
        for k in self.obraz.kanaly.iter_mut() {
            if k.data.len() != k.w * k.h {
                k.data = vec![0; k.w * k.h];
            }
        }
        modular::spat(&mut self.obraz, &self.transformacie)?;
        if self.obraz.meta != 0 {
            return Err(chyba("po transformáciách ostali meta kanály"));
        }
        let (w, hh) = (self.roz.w, self.roz.h);
        let zvacsenie = self.h.zvacsenie as usize;
        let (cw, ch) = (self.h.sirka as usize, self.h.vyska as usize);
        let lf_snimka = self.h.typ == 1;
        if let Some(v) = self.vardct.take() {
            let farba = v.dokonci(&self.h, self.m, !lf_snimka)?;
            if lf_snimka {
                return Ok(Vysledok3::Xyb(farba));
            }
            let dodatocne = zvacsi_dodatocne(self.obraz.kanaly, zvacsenie, self.m, cw, ch);
            return Ok(Vysledok3::Plavajuce(farba, dodatocne));
        }
        if self.obraz.kanaly.len() < self.farebnych {
            return Err(chyba("chýbajú farebné kanály"));
        }
        if self.m.xyb {
            // stratový Modular v XYB: kanály Y, X, B−Y vynásobené dekvantizáciou LF
            if self.farebnych != 3 {
                return Err(chyba("Modular v XYB potrebuje tri kanály"));
            }
            let mut k = std::mem::take(&mut self.obraz.kanaly);
            let dodatocne = k.split_off(3);
            let [y, x, b] = [&k[0].data, &k[1].data, &k[2].data];
            let d = self.lf_dekv;
            let mut roviny: [Vec<f32>; 3] = [
                x.iter().map(|&v| v as f32 * d[0]).collect(),
                y.iter().map(|&v| v as f32 * d[1]).collect(),
                b.iter().zip(y).map(|(&b, &y)| b.saturating_add(y) as f32 * d[2]).collect(),
            ];
            let sigma = vardct::Sigma::Stala(self.h.filtre.epf_sigma_modular);
            vardct::obnov(&mut roviny, w, hh, &self.h, self.m, sigma)?;
            if lf_snimka {
                return Ok(Vysledok3::Xyb(roviny));
            }
            let dodatocne = zvacsi_dodatocne(dodatocne, zvacsenie, self.m, cw, ch);
            return Ok(Vysledok3::Plavajuce(vardct::xyb_na_rgb(roviny, self.m), dodatocne));
        }
        let f = &self.h.filtre;
        if f.gabor || f.epf_iteracie > 0 || zvacsenie > 1 {
            // filtre a zväčšenie pracujú s plávajúcimi hodnotami (sivá sa na ne zdvojí na tri kanály)
            let mut k = std::mem::take(&mut self.obraz.kanaly);
            let dodatocne = k.split_off(self.farebnych);
            let hb = self.m.hlbka;
            let na_f = |v: &[i32]| -> Vec<f32> {
                if hb.plavajuca {
                    v.iter().map(|&x| super::cele_na_plavajuce(x, &hb)).collect()
                } else {
                    let max = ((1u64 << hb.bitov.min(31)) - 1) as f32;
                    v.iter().map(|&x| x as f32 / max).collect()
                }
            };
            let mut roviny: [Vec<f32>; 3] = if self.farebnych == 1 {
                let r = na_f(&k[0].data);
                [r.clone(), r.clone(), r]
            } else {
                [na_f(&k[0].data), na_f(&k[1].data), na_f(&k[2].data)]
            };
            let sigma = vardct::Sigma::Stala(f.epf_sigma_modular);
            vardct::obnov(&mut roviny, w, hh, &self.h, self.m, sigma)?;
            let farba: Vec<Vec<f32>> = roviny.into_iter().take(self.farebnych).collect();
            let dodatocne = zvacsi_dodatocne(dodatocne, zvacsenie, self.m, cw, ch);
            return Ok(Vysledok3::Kodovane(farba, dodatocne));
        }
        Ok(Vysledok3::Cele(self.obraz.kanaly))
    }
}

/// Zväčší celé dodatočné kanály tým istým jadrom ako farbu (hodnoty sa zaokrúhlia späť na celé čísla).
fn zvacsi_dodatocne(kanaly: Vec<Kanal>, k: usize, m: &Metadata, cw: usize, ch: usize) -> Vec<Kanal> {
    if k <= 1 {
        return kanaly;
    }
    kanaly
        .into_iter()
        .map(|kn| {
            let f: Vec<f32> = kn.data.iter().map(|&x| x as f32).collect();
            let v = vardct::zvacsi_rovinu(&f, kn.w, kn.h, k, m, cw, ch);
            let mut n = Kanal::novy(cw, ch, 0, 0);
            n.data = v.iter().map(|&x| x.round() as i32).collect();
            n
        })
        .collect()
}

/// Rozsah posunov (zmenšení) kanálov dekódovaných v prechode `p`.
fn rozsah_posunov(h: &HlavickaSnimky, prechod: usize) -> (i32, i32) {
    let pr = &h.prechody;
    let mut max = 2;
    let mut min = 3;
    for i in 0..pr.pocet as usize {
        for (j, &posl) in pr.posledny.iter().enumerate() {
            if i == posl as usize {
                min = match pr.zmensenie[j] {
                    8 => 3,
                    4 => 2,
                    2 => 1,
                    _ => 0,
                };
            }
        }
        if i == pr.pocet as usize - 1 {
            min = 0;
        }
        if i == prechod {
            return (min, max);
        }
        max = min - 1;
    }
    (min, max)
}
