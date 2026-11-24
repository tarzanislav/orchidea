//! Snímka JPEG XL: tabuľka obsahu (TOC), sekcie LfGlobal, LfGroup, HfGlobal a PassGroup, režim Modular
//! po skupinách; VarDCT v module `vardct`.

use super::bity::{Citac, D, ceil_log2, chyba};
use super::entropia::Histogramy;
use super::hlavicky::{HlavickaSnimky, Metadata, PRIZNAK_KRIVKY, PRIZNAK_SUM, PRIZNAK_ZAPLATY};
use super::modular::{self, Kanal, Obrazok, StromAKody, Transformacia};
use super::vardct;
use orchidea_jadro::{Chyba, Vysledok};

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
    /// Plávajúce farebné roviny (lineárne RGB alebo pôvodné hodnoty) a celé dodatočné kanály.
    Plavajuce([Vec<f32>; 3], Vec<Kanal>),
}

pub(super) struct Snimka {
    pub hlavicka: HlavickaSnimky,
    pub rozmery: Rozmery,
    pub vysledok: Vysledok3,
}

/// Permutácia z Lehmerovho kódu (TOC alebo poradie koeficientov).
pub(super) fn permutacia(c: &mut Citac, h: &Histogramy, n: usize, preskoc: usize) -> Vysledok<Vec<usize>> {
    let mut t = h.citac(c);
    let ctx = |x: u32| ceil_log2(x + 1).min(7) as usize;
    let koniec = t.citaj(h, c, ctx(n as u32))? as usize;
    if koniec > n - preskoc {
        return Err(chyba("neplatná dĺžka permutácie"));
    }
    let mut lehmer = vec![0usize; n];
    let mut predosly = 0u32;
    for (i, l) in lehmer.iter_mut().enumerate().skip(preskoc).take(koniec) {
        let v = t.citaj(h, c, ctx(predosly))?;
        if v as usize >= n - i {
            return Err(chyba("neplatný Lehmerov kód"));
        }
        *l = v as usize;
        predosly = v;
    }
    t.koniec(h)?;
    let mut zostava: Vec<usize> = (0..n).collect();
    let mut p = Vec::with_capacity(n);
    for &l in &lehmer {
        p.push(zostava.remove(l));
    }
    Ok(p)
}

pub(super) fn dekoduj(
    c: &mut Citac,
    data: &[u8],
    m: &Metadata,
    sirka: u32,
    vyska: u32,
) -> Vysledok<(Snimka, usize)> {
    let h = super::hlavicky::hlavicka_snimky(c, m, sirka, vyska)?;
    if h.priznaky & (PRIZNAK_ZAPLATY | PRIZNAK_KRIVKY | PRIZNAK_SUM) != 0 && h.modular {
        return Err(chyba("záplaty, krivky a šum v režime Modular zatiaľ nie sú podporené"));
    }
    if h.zvacsenie != 1 {
        return Err(chyba("zväčšenie snímky zatiaľ nie je podporené"));
    }
    let mut w = h.sirka as usize;
    let mut hh = h.vyska as usize;
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
        Some(permutacia(c, &hp, poctov, 0)?)
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
    // LfGlobal
    let idx = |i: usize| if jedna { 0 } else { i };
    stav.lf_global(&mut citace[idx(0)])?;
    for g in 0..roz.lf_skupin() {
        stav.lf_skupina(&mut citace[idx(1 + g)], g)?;
    }
    stav.hf_global(&mut citace[idx(1 + roz.lf_skupin())])?;
    for prechod in 0..prechodov {
        for g in 0..roz.skupin() {
            let i = 2 + roz.lf_skupin() + prechod * roz.skupin() + g;
            stav.skupina(&mut citace[idx(i)], prechod, g)?;
        }
    }
    for cit in &citace {
        cit.over()?;
    }
    let vysledok = stav.dokonci()?;
    Ok((Snimka { hlavicka: h, rozmery: roz, vysledok }, koniec))
}

/// Stav dekódovania jednej snímky.
struct Stav<'m> {
    h: HlavickaSnimky,
    m: &'m Metadata,
    roz: Rozmery,
    obraz: Obrazok,
    globalnych: usize,
    strom: Option<StromAKody>,
    transformacie: Vec<Transformacia>,
    farebnych: usize,
    vardct: Option<vardct::Stav>,
}

impl<'m> Stav<'m> {
    fn novy(h: &HlavickaSnimky, m: &'m Metadata, roz: Rozmery) -> Vysledok<Stav<'m>> {
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
        if !self.h.modular {
            self.vardct = Some(vardct::Stav::lf_global(c, &self.h, self.m, self.roz, lf_dekv)?);
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
        if let Some(v) = self.vardct.as_mut() {
            v.skupina(c, prechod, g)?;
        }
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
        if let Some(v) = self.vardct.take() {
            let farba = v.dokonci(&self.h, self.m)?;
            return Ok(Vysledok3::Plavajuce(farba, self.obraz.kanaly));
        }
        if self.obraz.kanaly.len() < self.farebnych {
            return Err(chyba("chýbajú farebné kanály"));
        }
        if self.m.xyb {
            // stratový Modular v XYB: kanály sú Y, X, B−Y vynásobené dekvantizáciou LF
            return Err(Chyba::NezhodaTypov {
                popis: "JPEG XL: Modular v XYB zatiaľ nie je podporený".into(),
                navrh: None,
            });
        }
        Ok(Vysledok3::Cele(self.obraz.kanaly))
    }
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
