//! Skladanie snímok JPEG XL: plátno s plávajúcimi vzorkami, referenčné snímky, záplaty (patches – opakované
//! kúsky obrazu z referenčnej snímky, napr. písmená na snímke obrazovky) a prekrytie vrstiev (blending).

use super::bity::{Citac, chyba, so_znamienkom};
use super::entropia::Histogramy;
use super::hlavicky::{KANAL_ALFA, Metadata, Prekrytie};
use orchidea_jadro::Vysledok;

/// Plávajúce vzorky snímky: farba (1 alebo 3 roviny; pri XYB pred prevodom farieb roviny X, Y, B, inak
/// hodnoty v prenosovej funkcii súboru 0–1) a dodatočné kanály (0–1).
#[derive(Debug, Clone)]
pub(super) struct Platno {
    pub w: usize,
    pub h: usize,
    pub farba: Vec<Vec<f32>>,
    pub dodatocne: Vec<Vec<f32>>,
}

impl Platno {
    fn prazdne(w: usize, h: usize, farebnych: usize, dodatocnych: usize) -> Platno {
        Platno {
            w,
            h,
            farba: (0..farebnych).map(|_| vec![0.0; w * h]).collect(),
            dodatocne: (0..dodatocnych).map(|_| vec![0.0; w * h]).collect(),
        }
    }

    fn kanal(&self, i: usize) -> &[f32] {
        if i < self.farba.len() { &self.farba[i] } else { &self.dodatocne[i - self.farba.len()] }
    }

    fn kanal_mut(&mut self, i: usize) -> &mut Vec<f32> {
        let f = self.farba.len();
        if i < f { &mut self.farba[i] } else { &mut self.dodatocne[i - f] }
    }

    fn kanalov(&self) -> usize {
        self.farba.len() + self.dodatocne.len()
    }
}

/// Uložená referenčná snímka.
#[derive(Debug, Clone)]
pub(super) struct Referencia {
    pub platno: Platno,
    /// Vzorky sú pred prevodom farieb (pri XYB roviny X, Y, B).
    pub pred_ct: bool,
}

// ---------------------------------------------------------------- miešanie jedného kanála

#[derive(Debug, Clone, Copy)]
enum Rezim {
    Nahrad,
    Pridaj,
    Nasob {
        orez: bool,
    },
    /// Zmiešanie podľa alfy (nad alebo pod – `prehod`).
    Zmiesaj {
        orez: bool,
        prehod: bool,
        nasobena: bool,
    },
    NasobPridaj {
        orez: bool,
        prehod: bool,
    },
    /// Samotný kanál alfy pri zmiešaní.
    ZmiesajAlfu {
        orez: bool,
        prehod: bool,
    },
    Preskoc,
}

/// Obdĺžnik spoločný pre základ a novú vrstvu.
#[derive(Debug, Clone, Copy)]
struct Oblast {
    zx: usize,
    zy: usize,
    nx: usize,
    ny: usize,
    w: usize,
    h: usize,
}

/// Zmieša kanál `nova` (šírka `nw`) do `zaklad` (šírka `zw`) v oblasti `o`; alfy sú rovnako rozložené ako
/// ich kanály.
#[allow(clippy::too_many_arguments)]
fn zmiesaj(
    zaklad: &mut [f32],
    zw: usize,
    nova: &[f32],
    nw: usize,
    o: Oblast,
    rezim: Rezim,
    alfa_zakladu: Option<&[f32]>,
    alfa_novej: Option<&[f32]>,
) {
    for dy in 0..o.h {
        for dx in 0..o.w {
            let zi = (o.zy + dy) * zw + o.zx + dx;
            let ni = (o.ny + dy) * nw + o.nx + dx;
            let (z, n) = (zaklad[zi], nova[ni]);
            zaklad[zi] = match rezim {
                Rezim::Nahrad => n,
                Rezim::Pridaj => z + n,
                Rezim::Nasob { orez } => z * if orez { n.clamp(0.0, 1.0) } else { n },
                Rezim::Preskoc => z,
                Rezim::ZmiesajAlfu { orez, prehod } => {
                    let (b, mut a) = if prehod { (n, z) } else { (z, n) };
                    if orez {
                        a = a.clamp(0.0, 1.0);
                    }
                    b + a * (1.0 - b)
                }
                Rezim::Zmiesaj { orez, prehod, nasobena } => {
                    let Some(an) = alfa_novej else {
                        zaklad[zi] = n;
                        continue;
                    };
                    let az = alfa_zakladu.map_or(0.0, |a| a[zi]);
                    let an = an[ni];
                    let (zs, ns, za, mut na) = if prehod { (n, z, an, az) } else { (z, n, az, an) };
                    if orez {
                        na = na.clamp(0.0, 1.0);
                    }
                    if nasobena {
                        ns + zs * (1.0 - na)
                    } else {
                        let zmes = 1.0 - (1.0 - na) * (1.0 - za);
                        let obr = if zmes > 0.0 { 1.0 / zmes } else { 0.0 };
                        (na * ns + za * zs * (1.0 - na)) * obr
                    }
                }
                Rezim::NasobPridaj { orez, prehod } => {
                    let Some(an) = alfa_novej else {
                        zaklad[zi] = z + n;
                        continue;
                    };
                    let (zs, ns, mut na) =
                        if prehod { (n, z, alfa_zakladu.map_or(0.0, |a| a[zi])) } else { (z, n, an[ni]) };
                    if orez {
                        na = na.clamp(0.0, 1.0);
                    }
                    zs + na * ns
                }
            };
        }
    }
}

/// Spoločná oblasť: základ `zw × zh`, nová vrstva `nw × nh` položená na (x, y) základu.
fn oblast(zw: usize, zh: usize, nw: usize, nh: usize, x: i64, y: i64) -> Option<Oblast> {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + nw as i64).min(zw as i64);
    let y1 = (y + nh as i64).min(zh as i64);
    (x1 > x0 && y1 > y0).then(|| Oblast {
        zx: x0 as usize,
        zy: y0 as usize,
        nx: (x0 - x) as usize,
        ny: (y0 - y) as usize,
        w: (x1 - x0) as usize,
        h: (y1 - y0) as usize,
    })
}

/// Alfa je predom vynásobená (alpha associated)?
fn nasobena(m: &Metadata, alfa: usize) -> bool {
    m.dodatocne.get(alfa).is_some_and(|d| d.alfa_nasobena)
}

/// Prekryje novú snímku `nova` (položenú na x0, y0) na základ – referenčnú snímku alebo prázdne plátno –
/// a vráti nové plátno `w × h`.
pub(super) fn prekry(
    zaklad: Option<&Platno>,
    nova: &Platno,
    (x0, y0): (i64, i64),
    (w, h): (usize, usize),
    prekrytie: &Prekrytie,
    dodatocnych: &[Prekrytie],
    m: &Metadata,
) -> Vysledok<Platno> {
    let farebnych = nova.farba.len();
    let mut p = match zaklad {
        Some(z)
            if z.w == w
                && z.h == h
                && z.farba.len() == farebnych
                && z.dodatocne.len() == nova.dodatocne.len() =>
        {
            z.clone()
        }
        Some(_) => return Err(chyba("referenčná snímka má iný tvar ako plátno")),
        None => Platno::prazdne(w, h, farebnych, nova.dodatocne.len()),
    };
    let Some(o) = oblast(w, h, nova.w, nova.h, x0, y0) else { return Ok(p) };
    let ma_dodatocne = !nova.dodatocne.is_empty();
    for i in 0..p.kanalov() {
        let info = if i < farebnych { prekrytie } else { &dodatocnych[i - farebnych] };
        let alfa = (ma_dodatocne && (info.rezim == 2 || info.rezim == 3)).then_some(info.alfa as usize);
        if alfa.is_some_and(|a| a >= nova.dodatocne.len()) {
            return Err(chyba("prekrytie odkazuje na chýbajúci kanál alfy"));
        }
        let je_alfa = alfa.is_some_and(|a| a + farebnych == i);
        let rezim = match info.rezim {
            0 => Rezim::Nahrad,
            1 => Rezim::Pridaj,
            2 if je_alfa => Rezim::ZmiesajAlfu { orez: info.orez, prehod: false },
            2 => Rezim::Zmiesaj {
                orez: info.orez,
                prehod: false,
                nasobena: alfa.is_some_and(|a| nasobena(m, a)),
            },
            3 if je_alfa => Rezim::Preskoc,
            3 => Rezim::NasobPridaj { orez: info.orez, prehod: false },
            _ => Rezim::Nasob { orez: info.orez },
        };
        let alfa_novej = alfa.filter(|_| !je_alfa).map(|a| nova.dodatocne[a].as_slice());
        // alfa základu je z pôvodnej referencie (nie z už zmiešaného plátna)
        let alfa_zakladu = alfa.filter(|_| !je_alfa).and_then(|a| zaklad.map(|z| z.dodatocne[a].as_slice()));
        let zw = p.w;
        zmiesaj(p.kanal_mut(i), zw, nova.kanal(i), nova.w, o, rezim, alfa_zakladu, alfa_novej);
    }
    Ok(p)
}

// ---------------------------------------------------------------- záplaty

#[derive(Debug, Clone, Copy)]
struct ZmiesanieZaplaty {
    rezim: u32,
    alfa: usize,
    orez: bool,
}

#[derive(Debug, Clone)]
struct Ciel {
    x: i64,
    y: i64,
    zmiesanie: Vec<ZmiesanieZaplaty>,
}

/// Záplata: obdĺžnik referenčnej snímky a miesta, kam sa kladie.
#[derive(Debug, Clone)]
pub(super) struct Zaplata {
    referencia: usize,
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    ciele: Vec<Ciel>,
}

/// Prečíta slovník záplat (v LfGlobal) pre snímku `w × h`.
pub(super) fn citaj_zaplaty(c: &mut Citac, m: &Metadata, w: usize, h: usize) -> Vysledok<Vec<Zaplata>> {
    let alfy: Vec<usize> =
        m.dodatocne.iter().enumerate().filter(|(_, d)| d.typ == KANAL_ALFA).map(|(i, _)| i).collect();
    let hs = Histogramy::citaj(c, 10)?;
    let mut t = hs.citac(c);
    let max_ref = ((w as u64 * h as u64 / 16) as u32).min(1 << 24);
    let max_zaplat = max_ref as u64 * 4;
    let pocet = t.citaj(&hs, c, 0)?;
    if pocet > max_ref {
        return Err(chyba("priveľa záplat"));
    }
    let mut spolu = 0u64;
    let mut v = Vec::with_capacity(pocet.min(1 << 16) as usize);
    for _ in 0..pocet {
        let referencia = t.citaj(&hs, c, 1)? as usize;
        if referencia >= 4 {
            return Err(chyba("záplata odkazuje na neplatnú referenciu"));
        }
        let x0 = t.citaj(&hs, c, 3)? as usize;
        let y0 = t.citaj(&hs, c, 3)? as usize;
        let zw = t.citaj(&hs, c, 2)? as usize + 1;
        let zh = t.citaj(&hs, c, 2)? as usize + 1;
        let kolko = t.citaj(&hs, c, 7)? as u64 + 1;
        spolu += kolko;
        if spolu > max_zaplat {
            return Err(chyba("priveľa umiestnení záplat"));
        }
        let mut ciele = Vec::with_capacity(kolko.min(1 << 16) as usize);
        let mut pred: Option<(i64, i64)> = None;
        for _ in 0..kolko {
            let (x, y) = match pred {
                Some((px, py)) => {
                    let dx = so_znamienkom(t.citaj(&hs, c, 6)?) as i64;
                    let dy = so_znamienkom(t.citaj(&hs, c, 6)?) as i64;
                    (px + dx, py + dy)
                }
                None => (t.citaj(&hs, c, 4)? as i64, t.citaj(&hs, c, 4)? as i64),
            };
            pred = Some((x, y));
            let mut zmiesanie = Vec::with_capacity(m.dodatocne.len() + 1);
            for _ in 0..=m.dodatocne.len() {
                let rezim = t.citaj(&hs, c, 5)?;
                if rezim > 7 {
                    return Err(chyba("neznámy spôsob zmiešania záplaty"));
                }
                let alfa = if rezim >= 4 && alfy.len() >= 2 {
                    t.citaj(&hs, c, 8)? as usize
                } else {
                    alfy.first().copied().unwrap_or(0)
                };
                let orez = rezim >= 3 && t.citaj(&hs, c, 9)? != 0;
                zmiesanie.push(ZmiesanieZaplaty { rezim, alfa, orez });
            }
            ciele.push(Ciel { x, y, zmiesanie });
        }
        v.push(Zaplata { referencia, x0, y0, w: zw, h: zh, ciele });
        if c.za_koncom() {
            return Err(chyba("záplaty presahujú údaje"));
        }
    }
    t.koniec(&hs)?;
    Ok(v)
}

/// Položí záplaty na snímku (pred prevodom farieb).
pub(super) fn poloz_zaplaty(
    p: &mut Platno,
    zaplaty: &[Zaplata],
    referencie: &[Option<Referencia>],
    m: &Metadata,
) -> Vysledok<()> {
    let farebnych = p.farba.len();
    for z in zaplaty {
        let r = referencie
            .get(z.referencia)
            .and_then(|r| r.as_ref())
            .ok_or_else(|| chyba("záplata odkazuje na chýbajúcu referenčnú snímku"))?;
        if m.xyb && !r.pred_ct {
            return Err(chyba("záplata z referenčnej snímky po prevode farieb zatiaľ nie je podporená"));
        }
        let r = &r.platno;
        if r.farba.len() != farebnych || r.dodatocne.len() != p.dodatocne.len() {
            return Err(chyba("referenčná snímka záplaty má iné kanály"));
        }
        if z.x0 + z.w > r.w || z.y0 + z.h > r.h {
            return Err(chyba("záplata presahuje referenčnú snímku"));
        }
        for ciel in &z.ciele {
            let Some(o) = oblast(p.w, p.h, z.w, z.h, ciel.x, ciel.y) else { continue };
            let o = Oblast { nx: o.nx + z.x0, ny: o.ny + z.y0, ..o };
            for i in 0..p.kanalov() {
                let zm = if i < farebnych { ciel.zmiesanie[0] } else { ciel.zmiesanie[1 + i - farebnych] };
                let pouzi_alfu = (4..=7).contains(&zm.rezim);
                if pouzi_alfu && zm.alfa >= p.dodatocne.len() {
                    return Err(chyba("záplata odkazuje na chýbajúci kanál alfy"));
                }
                let je_alfa = pouzi_alfu && zm.alfa + farebnych == i;
                let prehod = zm.rezim == 5 || zm.rezim == 7;
                let rezim = match zm.rezim {
                    0 => continue,
                    1 => Rezim::Nahrad,
                    2 => Rezim::Pridaj,
                    3 => Rezim::Nasob { orez: zm.orez },
                    4 | 5 if je_alfa => Rezim::ZmiesajAlfu { orez: zm.orez, prehod },
                    4 | 5 => Rezim::Zmiesaj { orez: zm.orez, prehod, nasobena: nasobena(m, zm.alfa) },
                    _ if je_alfa => {
                        if prehod {
                            Rezim::Nahrad
                        } else {
                            Rezim::Preskoc
                        }
                    }
                    _ => Rezim::NasobPridaj { orez: zm.orez, prehod },
                };
                let alfa_zakladu: Option<Vec<f32>> =
                    (pouzi_alfu && !je_alfa).then(|| p.dodatocne[zm.alfa].clone());
                let alfa_novej = (pouzi_alfu && !je_alfa).then(|| r.dodatocne[zm.alfa].as_slice());
                let pw = p.w;
                zmiesaj(p.kanal_mut(i), pw, r.kanal(i), r.w, o, rezim, alfa_zakladu.as_deref(), alfa_novej);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn oblast_s_orezanim() {
        let o = oblast(10, 8, 4, 4, -1, 6).unwrap();
        assert_eq!((o.zx, o.zy, o.nx, o.ny, o.w, o.h), (0, 6, 1, 0, 3, 2));
        assert!(oblast(10, 8, 4, 4, 10, 0).is_none());
    }

    #[test]
    fn zmiesanie_podla_alfy() {
        let mut z = vec![0.2f32];
        let o = Oblast { zx: 0, zy: 0, nx: 0, ny: 0, w: 1, h: 1 };
        let r = Rezim::Zmiesaj { orez: false, prehod: false, nasobena: false };
        zmiesaj(&mut z, 1, &[0.8], 1, o, r, Some(&[1.0]), Some(&[0.5]));
        // (0,5 · 0,8 + 1 · 0,2 · 0,5) / 1 = 0,5
        assert!((z[0] - 0.5).abs() < 1e-6);
    }
}
