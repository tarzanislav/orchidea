//! Predikcia pohybu – H.265, kap. 8.5.3.2: zlučovanie (merge), AMVP a časový kandidát (TMVP), plus dostupnosť
//! predikčného bloku (6.4.2).

use std::sync::OnceLock;

/// Pohyb predikčného bloku: vektory v štvrtinách vzorky jasu a referenčné indexy (−1 = zoznam sa nepoužíva).
/// Vnútrosnímkový (alebo nedekódovaný) blok má oba indexy −1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PohybPu {
    pub mv: [[i16; 2]; 2],
    pub ref_idx: [i8; 2],
}

impl PohybPu {
    pub const ZIADNY: PohybPu = PohybPu { mv: [[0; 2]; 2], ref_idx: [-1, -1] };

    #[inline]
    pub fn pred(&self, lx: usize) -> bool {
        self.ref_idx[lx] >= 0
    }

    #[inline]
    pub fn je_inter(&self) -> bool {
        self.ref_idx[0] >= 0 || self.ref_idx[1] >= 0
    }
}

impl Default for PohybPu {
    fn default() -> Self {
        Self::ZIADNY
    }
}

/// Pohyb kolokovaného bloku uložený s obrazom pre TMVP: POC referenčného obrazu a či bol pri dekódovaní dlhodobý.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PohybKol {
    pub mv: [[i16; 2]; 2],
    pub poc: [i32; 2],
    pub dlhodoba: [bool; 2],
    pub pred: [bool; 2],
}

/// Pohyb obrazu na mriežke 16 × 16 (ľavý horný blok 4×4 každej bunky – 8.5.3.2.8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoleKol {
    pub sirka16: usize,
    pub vyska16: usize,
    pub bloky: Vec<PohybKol>,
    pub poc: i32,
}

/// Pohyb dekódovaného obrazu pre TMVP zverejňovaný po pásoch (riadkoch CTB) – ďalší obraz v inom vlákne počká
/// len na pás, ktorý potrebuje (ako [`crate::obraz::ObrazPasov`]).
pub struct PoleKolPasov {
    pub sirka16: usize,
    pub vyska16: usize,
    pub poc: i32,
    /// riadkov mriežky 16 × 16 v jednom páse
    riadkov_pasu: usize,
    pasy: Vec<OnceLock<Box<[PohybKol]>>>,
}

impl std::fmt::Debug for PoleKolPasov {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PoleKolPasov {}×{}, POC {}", self.sirka16, self.vyska16, self.poc)
    }
}

impl PoleKolPasov {
    pub fn nove(sirka16: usize, vyska16: usize, riadkov_pasu: usize, poc: i32) -> PoleKolPasov {
        let riadkov_pasu = riadkov_pasu.max(1);
        let n = vyska16.div_ceil(riadkov_pasu);
        PoleKolPasov { sirka16, vyska16, poc, riadkov_pasu, pasy: (0..n).map(|_| OnceLock::new()).collect() }
    }

    pub fn pocet_pasov(&self) -> usize {
        self.pasy.len()
    }

    /// Zverejní pás `i` z poľa `zdroj` s rovnakými rozmermi (dekódovanie obrazu ho práve dokončilo).
    pub fn zverejni_z(&self, i: usize, zdroj: &PoleKol) {
        if i >= self.pasy.len() {
            return;
        }
        let od = i * self.riadkov_pasu;
        let po = (od + self.riadkov_pasu).min(self.vyska16);
        let d: Box<[PohybKol]> = zdroj.bloky[od * self.sirka16..po * self.sirka16].into();
        let _ = self.pasy[i].set(d);
    }

    /// Pohyb bunky (`bx`, `by`) mriežky 16 × 16 – počká na jej pás.
    #[inline]
    pub fn blok(&self, bx: usize, by: usize) -> Option<&PohybKol> {
        if bx >= self.sirka16 || by >= self.vyska16 {
            return None;
        }
        let i = by / self.riadkov_pasu;
        self.pasy[i].wait().get((by - i * self.riadkov_pasu) * self.sirka16 + bx)
    }

    /// Doplní nezverejnené pásy (obraz s chybou): bloky bez pohybu.
    pub fn dopln(&self) {
        for (i, p) in self.pasy.iter().enumerate() {
            if p.get().is_none() {
                let od = i * self.riadkov_pasu;
                let n = ((od + self.riadkov_pasu).min(self.vyska16) - od) * self.sirka16;
                let _ = p.set(vec![PohybKol::default(); n].into_boxed_slice());
            }
        }
    }
}

/// Druh rozdelenia kódovacej jednotky na predikčné bloky (PartMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rozdelenie {
    P2Nx2N,
    P2NxN,
    PNx2N,
    PNxN,
    P2NxnU,
    P2NxnD,
    PnLx2N,
    PnRx2N,
}

/// Okolie predikčného bloku: mapa pohybu obrazu, dostupnosť a zoznamy referencií rezu.
#[allow(missing_debug_implementations)]
pub struct Okolie<'a> {
    /// pohyb po blokoch 4×4 aktuálneho obrazu (index `(y / 4) * sirka4 + x / 4`)
    pub mapa: &'a [PohybPu],
    pub sirka4: usize,
    /// dostupnosť v poradí z (6.4.1): (xCurr, yCurr, xNbY, yNbY)
    pub dostupny: &'a dyn Fn(i32, i32, i32, i32) -> bool,
    pub sirka: i32,
    pub vyska: i32,
    pub log2_ctb: u32,
    pub poc: i32,
    /// POC a dlhodobosť položiek RefPicList0/1
    pub ref_poc: [&'a [i32]; 2],
    pub ref_lt: [&'a [bool]; 2],
    /// kolokovaný obraz (ColPic), ak je slice_temporal_mvp_enabled_flag = 1
    pub kol: Option<&'a PoleKolPasov>,
    pub b_rez: bool,
    /// collocated_from_l0_flag
    pub kol_z_l0: bool,
    /// NoBackwardPredFlag
    pub bez_spatnej: bool,
    /// Log2ParMrgLevel
    pub log2_par_mrg: u32,
    pub max_zlucenie: u32,
    pub aktivne: [u32; 2],
}

/// Geometria predikčného bloku v kódovacej jednotke.
#[derive(Debug, Clone, Copy)]
pub struct Blok {
    pub x_cb: i32,
    pub y_cb: i32,
    pub n_cb: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub index: u32,
    pub rozdelenie: Rozdelenie,
}

/// Škálovanie vektora podľa vzdialenosti POC (8-180 … 8-183, 8-194 … 8-197).
#[inline]
fn skaluj(mv: i32, td: i32, tb: i32) -> i32 {
    let td = td.clamp(-128, 127);
    let tb = tb.clamp(-128, 127);
    if td == 0 {
        return mv;
    }
    let tx = (16384 + (td.abs() >> 1)) / td;
    let f = ((tb * tx + 32) >> 6).clamp(-4096, 4095);
    let p = f * mv;
    (p.signum() * ((p.abs() + 127) >> 8)).clamp(-32768, 32767)
}

fn skaluj_mv(mv: [i16; 2], td: i32, tb: i32) -> [i16; 2] {
    [skaluj(mv[0] as i32, td, tb) as i16, skaluj(mv[1] as i32, td, tb) as i16]
}

impl Okolie<'_> {
    /// Pohyb susedného predikčného bloku, ak je dostupný a medzisnímkový (6.4.2).
    fn sused(&self, b: &Blok, x_nb: i32, y_nb: i32) -> Option<PohybPu> {
        let v_cb = b.x_cb <= x_nb && x_nb < b.x_cb + b.n_cb && b.y_cb <= y_nb && y_nb < b.y_cb + b.n_cb;
        let dostupny = if !v_cb {
            (self.dostupny)(b.x, b.y, x_nb, y_nb)
        } else {
            !((b.w << 1) == b.n_cb
                && (b.h << 1) == b.n_cb
                && b.index == 1
                && b.y_cb + b.h <= y_nb
                && b.x_cb + b.w > x_nb)
        };
        if !dostupny {
            return None;
        }
        let m = self.mapa[(y_nb as usize >> 2) * self.sirka4 + (x_nb as usize >> 2)];
        m.je_inter().then_some(m)
    }

    /// Kolokovaný vektor (8.5.3.2.9) pre zoznam `lx` a referenčný index `ref_idx`.
    fn kolokovany(&self, x: i32, y: i32, lx: usize, ref_idx: usize) -> Option<[i16; 2]> {
        let kol = self.kol?;
        let (bx, by) = ((x >> 4) as usize, (y >> 4) as usize);
        let c = kol.blok(bx, by)?;
        if !c.pred[0] && !c.pred[1] {
            return None;
        }
        let lc = if !c.pred[0] {
            1
        } else if !c.pred[1] {
            0
        } else if self.bez_spatnej {
            lx
        } else if self.kol_z_l0 {
            1
        } else {
            0
        };
        let akt_lt = self.ref_lt[lx][ref_idx];
        if akt_lt != c.dlhodoba[lc] {
            return None;
        }
        let rozdiel_kol = kol.poc - c.poc[lc];
        let rozdiel_akt = self.poc - self.ref_poc[lx][ref_idx];
        if akt_lt || rozdiel_kol == rozdiel_akt {
            Some(c.mv[lc])
        } else {
            Some(skaluj_mv(c.mv[lc], rozdiel_kol, rozdiel_akt))
        }
    }

    /// Časový kandidát (8.5.3.2.8): najprv blok vpravo dole, potom stred.
    fn casovy(&self, b: &Blok, lx: usize, ref_idx: usize) -> Option<[i16; 2]> {
        self.kol?;
        let (xbr, ybr) = (b.x + b.w, b.y + b.h);
        if (b.y >> self.log2_ctb) == (ybr >> self.log2_ctb)
            && ybr < self.vyska
            && xbr < self.sirka
            && let Some(mv) = self.kolokovany(xbr, ybr, lx, ref_idx)
        {
            return Some(mv);
        }
        self.kolokovany(b.x + (b.w >> 1), b.y + (b.h >> 1), lx, ref_idx)
    }

    /// Zlučovanie (8.5.3.2.2): pohyb kandidáta `merge_idx`.
    pub fn zlucenie(&self, povodny: &Blok, merge_idx: u32) -> PohybPu {
        // spoločný zoznam pre všetky PB jednotky 8×8 pri Log2ParMrgLevel > 2
        let b = if self.log2_par_mrg > 2 && povodny.n_cb == 8 {
            Blok {
                x: povodny.x_cb,
                y: povodny.y_cb,
                w: povodny.n_cb,
                h: povodny.n_cb,
                index: 0,
                rozdelenie: Rozdelenie::P2Nx2N,
                ..*povodny
            }
        } else {
            *povodny
        };
        let l = self.log2_par_mrg;
        let mimo_par = |x_nb: i32, y_nb: i32| (b.x >> l) != (x_nb >> l) || (b.y >> l) != (y_nb >> l);
        let mut zoznam: [PohybPu; 5] = [PohybPu::ZIADNY; 5];
        let mut n = 0usize;
        let max = self.max_zlucenie as usize;

        // priestoroví kandidáti (8.5.3.2.3)
        let (xa1, ya1) = (b.x - 1, b.y + b.h - 1);
        let a1 = if mimo_par(xa1, ya1)
            && !(b.index == 1
                && matches!(b.rozdelenie, Rozdelenie::PNx2N | Rozdelenie::PnLx2N | Rozdelenie::PnRx2N))
        {
            self.sused(&b, xa1, ya1)
        } else {
            None
        };
        let (xb1, yb1) = (b.x + b.w - 1, b.y - 1);
        let b1 = if mimo_par(xb1, yb1)
            && !(b.index == 1
                && matches!(b.rozdelenie, Rozdelenie::P2NxN | Rozdelenie::P2NxnU | Rozdelenie::P2NxnD))
        {
            self.sused(&b, xb1, yb1)
        } else {
            None
        };
        let (xb0, yb0) = (b.x + b.w, b.y - 1);
        let b0 = if mimo_par(xb0, yb0) { self.sused(&b, xb0, yb0) } else { None };
        let (xa0, ya0) = (b.x - 1, b.y + b.h);
        let a0 = if mimo_par(xa0, ya0) { self.sused(&b, xa0, ya0) } else { None };
        let (xb2, yb2) = (b.x - 1, b.y - 1);
        let b2 = if mimo_par(xb2, yb2) { self.sused(&b, xb2, yb2) } else { None };

        let rovnake = |p: Option<PohybPu>, q: Option<PohybPu>| matches!((p, q), (Some(p), Some(q)) if p == q);
        let pridaj = |zoznam: &mut [PohybPu; 5], n: &mut usize, p: PohybPu| {
            if *n < 5 {
                zoznam[*n] = p;
                *n += 1;
            }
        };
        if let Some(p) = a1 {
            pridaj(&mut zoznam, &mut n, p);
        }
        if let Some(p) = b1
            && !rovnake(a1, b1)
        {
            pridaj(&mut zoznam, &mut n, p);
        }
        if let Some(p) = b0
            && !rovnake(b1, b0)
        {
            pridaj(&mut zoznam, &mut n, p);
        }
        if let Some(p) = a0
            && !rovnake(a1, a0)
        {
            pridaj(&mut zoznam, &mut n, p);
        }
        if let Some(p) = b2
            && !rovnake(a1, b2)
            && !rovnake(b1, b2)
            && n < 4
        {
            pridaj(&mut zoznam, &mut n, p);
        }
        // časový kandidát (refIdxLXCol = 0)
        if n < max && self.kol.is_some() {
            let l0 = self.casovy(&b, 0, 0);
            let l1 = if self.b_rez { self.casovy(&b, 1, 0) } else { None };
            if l0.is_some() || l1.is_some() {
                let mut p = PohybPu::ZIADNY;
                if let Some(mv) = l0 {
                    p.mv[0] = mv;
                    p.ref_idx[0] = 0;
                }
                if let Some(mv) = l1 {
                    p.mv[1] = mv;
                    p.ref_idx[1] = 0;
                }
                pridaj(&mut zoznam, &mut n, p);
            }
        }
        let n = n.min(max);
        let mut n_spolu = n;
        // kombinovaní obojsmerní kandidáti (8.5.3.2.4)
        if self.b_rez && n > 1 && n < max {
            const L0: [usize; 12] = [0, 1, 0, 2, 1, 2, 0, 3, 1, 3, 2, 3];
            const L1: [usize; 12] = [1, 0, 2, 0, 2, 1, 3, 0, 3, 1, 3, 2];
            let mut kombinacie = 0;
            while kombinacie < n * (n - 1) && n_spolu < max {
                let p0 = zoznam[L0[kombinacie]];
                let p1 = zoznam[L1[kombinacie]];
                if p0.pred(0)
                    && p1.pred(1)
                    && (self.ref_poc[0][p0.ref_idx[0] as usize] != self.ref_poc[1][p1.ref_idx[1] as usize]
                        || p0.mv[0] != p1.mv[1])
                {
                    zoznam[n_spolu] =
                        PohybPu { mv: [p0.mv[0], p1.mv[1]], ref_idx: [p0.ref_idx[0], p1.ref_idx[1]] };
                    n_spolu += 1;
                }
                kombinacie += 1;
            }
        }
        let mut vysledok = if (merge_idx as usize) < n_spolu {
            zoznam[merge_idx as usize]
        } else {
            // nulové kandidáty (8.5.3.2.5)
            let pocet_ref =
                if self.b_rez { self.aktivne[0].min(self.aktivne[1]) } else { self.aktivne[0] } as i32;
            let nulovy = merge_idx as i32 - n_spolu as i32;
            let r = if nulovy < pocet_ref { nulovy as i8 } else { 0 };
            PohybPu { mv: [[0; 2]; 2], ref_idx: [r, if self.b_rez { r } else { -1 }] }
        };
        if vysledok.pred(0) && vysledok.pred(1) && povodny.w + povodny.h == 12 {
            vysledok.ref_idx[1] = -1;
            vysledok.mv[1] = [0, 0];
        }
        vysledok
    }

    /// Prediktor vektora AMVP (8.5.3.2.6, 8.5.3.2.7) pre zoznam `lx`, index `ref_idx` a príznak mvp_lX_flag.
    pub fn amvp(&self, b: &Blok, lx: usize, ref_idx: usize, mvp_priznak: u32) -> [i16; 2] {
        let ly = 1 - lx;
        let ciel_poc = self.ref_poc[lx][ref_idx];
        let ciel_lt = self.ref_lt[lx][ref_idx];
        let poc_z = |l: usize, i: i8| self.ref_poc[l][i as usize];
        let lt_z = |l: usize, i: i8| self.ref_lt[l][i as usize];
        // rovnaký referenčný obraz – bez škálovania
        let rovnaky = |p: &PohybPu| -> Option<[i16; 2]> {
            if p.pred(lx) && poc_z(lx, p.ref_idx[lx]) == ciel_poc {
                Some(p.mv[lx])
            } else if p.pred(ly) && poc_z(ly, p.ref_idx[ly]) == ciel_poc {
                Some(p.mv[ly])
            } else {
                None
            }
        };
        // iný referenčný obraz s rovnakou dlhodobosťou – so škálovaním, ak sú oba krátkodobé
        let skalovany = |p: &PohybPu| -> Option<[i16; 2]> {
            let (mv, l, i) = if p.pred(lx) && lt_z(lx, p.ref_idx[lx]) == ciel_lt {
                (p.mv[lx], lx, p.ref_idx[lx])
            } else if p.pred(ly) && lt_z(ly, p.ref_idx[ly]) == ciel_lt {
                (p.mv[ly], ly, p.ref_idx[ly])
            } else {
                return None;
            };
            if !lt_z(l, i) && !ciel_lt {
                Some(skaluj_mv(mv, self.poc - poc_z(l, i), self.poc - ciel_poc))
            } else {
                Some(mv)
            }
        };

        let a = [self.sused(b, b.x - 1, b.y + b.h), self.sused(b, b.x - 1, b.y + b.h - 1)];
        let je_skalovany = a[0].is_some() || a[1].is_some();
        let mut mv_a = a.iter().flatten().find_map(rovnaky);
        if mv_a.is_none() {
            mv_a = a.iter().flatten().find_map(skalovany);
        }
        let bb = [
            self.sused(b, b.x + b.w, b.y - 1),
            self.sused(b, b.x + b.w - 1, b.y - 1),
            self.sused(b, b.x - 1, b.y - 1),
        ];
        let mut mv_b = bb.iter().flatten().find_map(rovnaky);
        if !je_skalovany {
            if mv_b.is_some() && mv_a.is_none() {
                mv_a = mv_b;
            }
            mv_b = bb.iter().flatten().find_map(skalovany);
        }
        let mut zoznam = [[0i16; 2]; 2];
        let mut n = 0;
        if let Some(a) = mv_a {
            zoznam[n] = a;
            n += 1;
        }
        if let Some(bv) = mv_b
            && (mv_a != Some(bv))
        {
            zoznam[n] = bv;
            n += 1;
        }
        if n < 2
            && let Some(c) = self.casovy(b, lx, ref_idx)
        {
            zoznam[n] = c;
        }
        zoznam[mvp_priznak as usize & 1]
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn skalovanie_vektora() {
        // rovnaká vzdialenosť → bez zmeny; polovičná → polovica; opačný smer → záporné
        assert_eq!(skaluj(100, 4, 4), 100);
        assert_eq!(skaluj(100, 4, 2), 50);
        assert_eq!(skaluj(-100, 4, -4), 100);
        assert_eq!(skaluj(32767, 1, 127), 32767);
        assert_eq!(skaluj(-32768, 1, 127), -32768);
        // tx = (16384 + 1) / 3 = 5461; f = (2 · 5461 + 32) >> 6 = 171; (171 · 9 + 127) >> 8 = 6
        assert_eq!(skaluj(9, 3, 2), 6);
    }
}
