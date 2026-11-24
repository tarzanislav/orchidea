//! Režim Modular JPEG XL: rozhodovací strom (MA), prediktory vrátane váženého, transformácie RCT, paleta
//! a Squeeze. Kanály sú celé čísla (i32); výpočty predikcií v i64 ako v norme.

use super::bity::{Citac, D, chyba, so_znamienkom};
use super::entropia::{Histogramy, TokZnakov};
use orchidea_jadro::{Chyba, Vysledok};

/// Kanál obrazu Modular.
#[derive(Debug, Clone)]
pub(super) struct Kanal {
    pub w: usize,
    pub h: usize,
    pub hshift: i32,
    pub vshift: i32,
    pub data: Vec<i32>,
}

impl Kanal {
    pub fn novy(w: usize, h: usize, hshift: i32, vshift: i32) -> Kanal {
        Kanal { w, h, hshift, vshift, data: Vec::new() }
    }

    fn alokuj(&mut self) -> Vysledok<()> {
        let n = self.w.checked_mul(self.h).ok_or_else(|| chyba("kanál je priveľký"))?;
        if n > 1 << 30 {
            return Err(Chyba::Limit("JPEG XL: kanál nad 2^30 vzoriek".into()));
        }
        if self.data.len() != n {
            self.data = vec![0; n];
        }
        Ok(())
    }
}

/// Parametre váženého prediktora.
#[derive(Debug, Clone, Copy)]
pub(super) struct ParametreWp {
    p1: i64,
    p2: i64,
    p3: [i64; 5],
    w: [u32; 4],
}

impl ParametreWp {
    fn citaj(c: &mut Citac) -> ParametreWp {
        if c.bool() {
            return ParametreWp { p1: 16, p2: 10, p3: [7, 7, 7, 0, 0], w: [13, 12, 12, 12] };
        }
        let p1 = c.u(5) as i64;
        let p2 = c.u(5) as i64;
        let mut p3 = [0i64; 5];
        for x in p3.iter_mut() {
            *x = c.u(5) as i64;
        }
        let mut w = [0u32; 4];
        for x in w.iter_mut() {
            *x = c.u(4);
        }
        ParametreWp { p1, p2, p3, w }
    }
}

#[derive(Debug, Clone)]
pub(super) enum Transformacia {
    Rct { zac: usize, typ: u32 },
    Paleta { zac: usize, pocet: usize, farieb: usize, delt: usize, prediktor: u32, wp: ParametreWp },
    Squeeze { kroky: Vec<KrokSqueeze> },
}

#[derive(Debug, Clone, Copy)]
pub(super) struct KrokSqueeze {
    vodorovne: bool,
    na_mieste: bool,
    zac: usize,
    pocet: usize,
}

#[derive(Debug, Clone, Copy)]
enum Uzol {
    Rozhodnutie { vlastnost: usize, hodnota: i32, lavy: usize, pravy: usize },
    List { ctx: usize, prediktor: u8, posun: i64, nasobok: i64 },
}

/// Rozhodovací strom (MA) s histogramami.
#[derive(Debug)]
pub(super) struct Strom {
    uzly: Vec<Uzol>,
    pouziva_wp: bool,
    max_vlastnost: usize,
}

/// Strom spolu s entropickým nastavením.
pub(super) struct StromAKody {
    pub strom: Strom,
    pub kody: Histogramy,
}

pub(super) fn citaj_strom(c: &mut Citac, limit_uzlov: usize) -> Vysledok<StromAKody> {
    let h = Histogramy::citaj(c, 6)?;
    let mut t = h.citac(c);
    let mut uzly = Vec::new();
    let mut na_cesta = 1usize;
    let mut listov = 0usize;
    let (mut pouziva_wp, mut max_vlastnost) = (false, 0usize);
    while na_cesta > 0 {
        if uzly.len() > limit_uzlov {
            return Err(Chyba::Limit("JPEG XL: strom má priveľa uzlov".into()));
        }
        na_cesta -= 1;
        let vlastnost = t.citaj(&h, c, 1)? as i64 - 1;
        if vlastnost < 0 {
            let prediktor = t.citaj(&h, c, 2)?;
            if prediktor > 13 {
                return Err(chyba("neplatný prediktor"));
            }
            let posun = so_znamienkom(t.citaj(&h, c, 3)?) as i64;
            let log = t.citaj(&h, c, 4)?;
            let bity = t.citaj(&h, c, 5)?;
            if log > 30 || bity >= (1 << (31 - log)) {
                return Err(chyba("neplatný násobok listu"));
            }
            let nasobok = ((bity as i64) + 1) << log;
            pouziva_wp |= prediktor == 6;
            uzly.push(Uzol::List { ctx: listov, prediktor: prediktor as u8, posun, nasobok });
            listov += 1;
            continue;
        }
        let v = vlastnost as usize;
        if v > 255 {
            return Err(chyba("neplatná vlastnosť v strome"));
        }
        pouziva_wp |= v == 15;
        max_vlastnost = max_vlastnost.max(v);
        let hodnota = so_znamienkom(t.citaj(&h, c, 0)?);
        let i = uzly.len();
        uzly.push(Uzol::Rozhodnutie {
            vlastnost: v,
            hodnota,
            lavy: i + na_cesta + 1,
            pravy: i + na_cesta + 2,
        });
        na_cesta += 2;
    }
    t.koniec(&h)?;
    let kody = Histogramy::citaj(c, listov)?;
    Ok(StromAKody { strom: Strom { uzly, pouziva_wp, max_vlastnost }, kody })
}

/// Hlavička obrazu Modular (GroupHeader).
pub(super) struct Hlavicka {
    pub globalny_strom: bool,
    pub wp: ParametreWp,
    pub transformacie: Vec<Transformacia>,
}

fn citaj_hlavicku(c: &mut Citac) -> Vysledok<Hlavicka> {
    let globalny_strom = c.bool();
    let wp = ParametreWp::citaj(c);
    let n = c.u32([D::V(0), D::V(1), D::B(4, 2), D::B(8, 18)]);
    let mut transformacie = Vec::new();
    let zac_d = [D::B(3, 0), D::B(6, 8), D::B(10, 72), D::B(13, 1096)];
    for _ in 0..n {
        match c.u32([D::V(0), D::V(1), D::V(2), D::V(3)]) {
            0 => {
                let zac = c.u32(zac_d) as usize;
                let typ = c.u32([D::V(6), D::B(2, 0), D::B(4, 2), D::B(6, 10)]);
                if typ >= 42 {
                    return Err(chyba("neplatný typ RCT"));
                }
                transformacie.push(Transformacia::Rct { zac, typ });
            }
            1 => {
                let zac = c.u32(zac_d) as usize;
                let pocet = c.u32([D::V(1), D::V(3), D::V(4), D::B(13, 1)]) as usize;
                let farieb = c.u32([D::B(8, 0), D::B(10, 256), D::B(12, 1280), D::B(16, 5376)]) as usize;
                let delt = c.u32([D::V(0), D::B(8, 1), D::B(10, 257), D::B(16, 1281)]) as usize;
                let prediktor = c.u(4);
                if prediktor > 13 {
                    return Err(chyba("neplatný prediktor palety"));
                }
                transformacie.push(Transformacia::Paleta { zac, pocet, farieb, delt, prediktor, wp });
            }
            2 => {
                let k = c.u32([D::V(0), D::B(4, 1), D::B(6, 9), D::B(8, 41)]);
                let mut kroky = Vec::with_capacity(k as usize);
                for _ in 0..k {
                    let vodorovne = c.bool();
                    let na_mieste = c.bool();
                    let zac = c.u32(zac_d) as usize;
                    let pocet = c.u32([D::V(1), D::V(2), D::V(3), D::B(4, 4)]) as usize;
                    kroky.push(KrokSqueeze { vodorovne, na_mieste, zac, pocet });
                }
                transformacie.push(Transformacia::Squeeze { kroky });
            }
            _ => return Err(chyba("neznáma transformácia")),
        }
        c.over()?;
    }
    Ok(Hlavicka { globalny_strom, wp, transformacie })
}

/// Obraz Modular: kanály a počet meta kanálov (palety) na začiatku.
pub(super) struct Obrazok {
    pub kanaly: Vec<Kanal>,
    pub meta: usize,
    pub bitov: u32,
}

/// Predvolené kroky Squeeze (keď ich súbor neuvádza).
fn predvolene_squeeze(o: &Obrazok) -> Vec<KrokSqueeze> {
    let mut v = Vec::new();
    let prvy = o.meta;
    let pocet = o.kanaly.len() - o.meta;
    if pocet == 0 {
        return v;
    }
    let (mut w, mut h) = (o.kanaly[prvy].w, o.kanaly[prvy].h);
    if pocet > 2 && o.kanaly[prvy + 1].w == w && o.kanaly[prvy + 1].h == h {
        v.push(KrokSqueeze { vodorovne: true, na_mieste: false, zac: prvy + 1, pocet: 2 });
        v.push(KrokSqueeze { vodorovne: false, na_mieste: false, zac: prvy + 1, pocet: 2 });
    }
    let siroky = w > h;
    if !siroky && h > 8 {
        v.push(KrokSqueeze { vodorovne: false, na_mieste: true, zac: prvy, pocet });
        h = h.div_ceil(2);
    }
    while w > 8 || h > 8 {
        if w > 8 {
            v.push(KrokSqueeze { vodorovne: true, na_mieste: true, zac: prvy, pocet });
            w = w.div_ceil(2);
        }
        if h > 8 {
            v.push(KrokSqueeze { vodorovne: false, na_mieste: true, zac: prvy, pocet });
            h = h.div_ceil(2);
        }
    }
    v
}

/// Úprava zoznamu kanálov podľa transformácie (pred dekódovaním).
fn meta_uprava(o: &mut Obrazok, t: &mut Transformacia) -> Vysledok<()> {
    match t {
        Transformacia::Rct { zac, .. } => {
            let z = *zac;
            if z + 3 > o.kanaly.len() || z < o.meta {
                return Err(chyba("RCT mimo kanálov"));
            }
            let k = &o.kanaly;
            if (1..3).any(|i| k[z + i].w != k[z].w || k[z + i].h != k[z].h) {
                return Err(chyba("RCT na rôznych rozmeroch"));
            }
        }
        Transformacia::Paleta { zac, pocet, farieb, delt, .. } => {
            let (z, n) = (*zac, *pocet);
            if n == 0 || z + n > o.kanaly.len() {
                return Err(chyba("paleta mimo kanálov"));
            }
            let k = &o.kanaly;
            if (1..n).any(|i| k[z + i].w != k[z].w || k[z + i].h != k[z].h) {
                return Err(chyba("paleta na rôznych rozmeroch"));
            }
            if z >= o.meta {
                o.meta += 1;
            } else {
                if o.meta + 2 < n {
                    return Err(chyba("paleta na meta kanáloch"));
                }
                o.meta = o.meta + 2 - n;
            }
            o.kanaly.drain(z + 1..z + n);
            if *farieb + *delt > 1 << 20 {
                return Err(Chyba::Limit("JPEG XL: priveľká paleta".into()));
            }
            // kanál palety má šírku počtu farieb (rozdiely sú jej prvé položky)
            o.kanaly.insert(0, Kanal::novy(*farieb, n, -1, -1));
        }
        Transformacia::Squeeze { kroky } => {
            if kroky.is_empty() {
                *kroky = predvolene_squeeze(o);
            }
            for k in kroky.iter() {
                let koniec = k.zac + k.pocet;
                if k.pocet == 0 || koniec > o.kanaly.len() || k.zac < o.meta {
                    return Err(chyba("Squeeze mimo kanálov"));
                }
                let miesto = if k.na_mieste { koniec } else { o.kanaly.len() };
                for c in k.zac..koniec {
                    let ch = &mut o.kanaly[c];
                    if ch.hshift > 30 || ch.vshift > 30 {
                        return Err(chyba("Squeeze príliš hlboko"));
                    }
                    let (w, h) = (ch.w, ch.h);
                    let (rw, rh) = if k.vodorovne {
                        ch.w = w.div_ceil(2);
                        ch.hshift += 1;
                        (w - w.div_ceil(2), h)
                    } else {
                        ch.h = h.div_ceil(2);
                        ch.vshift += 1;
                        (w, h - h.div_ceil(2))
                    };
                    let novy = Kanal::novy(rw, rh, ch.hshift, ch.vshift);
                    o.kanaly.insert(miesto + (c - k.zac), novy);
                }
            }
        }
    }
    Ok(())
}

/// Dekóduje obraz Modular (hlavička, voliteľne strom, kanály). Kanály väčšie ako `max_velkost` (a všetky za
/// nimi) sa preskočia – dekódujú sa po skupinách. Transformácie sa vrátia (spätne sa použijú neskôr).
pub(super) fn dekoduj(
    c: &mut Citac,
    o: &mut Obrazok,
    stream: u32,
    globalny: Option<&StromAKody>,
    max_velkost: usize,
) -> Vysledok<Vec<Transformacia>> {
    if o.kanaly.is_empty() {
        return Ok(Vec::new());
    }
    let mut h = citaj_hlavicku(c)?;
    for t in h.transformacie.iter_mut() {
        meta_uprava(o, t)?;
    }
    if o.kanaly.len() > 1 << 16 {
        return Err(Chyba::Limit("JPEG XL: priveľa kanálov".into()));
    }
    let vlastny;
    let sk = if h.globalny_strom {
        globalny.ok_or_else(|| chyba("chýba globálny strom"))?
    } else {
        let pixelov: usize = o.kanaly.iter().map(|k| k.w.saturating_mul(k.h)).sum();
        vlastny = citaj_strom(c, (1024 + pixelov).min(1 << 22))?;
        &vlastny
    };
    // násobok vzdialeností LZ77: najväčšia šírka dekódovaných kanálov
    let mut nasobok = 0usize;
    for (i, k) in o.kanaly.iter().enumerate() {
        if k.w == 0 || k.h == 0 {
            continue;
        }
        if i >= o.meta && (k.w > max_velkost || k.h > max_velkost) {
            break;
        }
        nasobok = nasobok.max(k.w);
    }
    let mut tok = sk.kody.citac(c);
    tok.nasobok_vzdialenosti = nasobok as u32;
    for i in 0..o.kanaly.len() {
        let (w, h2) = (o.kanaly[i].w, o.kanaly[i].h);
        if w == 0 || h2 == 0 {
            continue;
        }
        if i >= o.meta && (w > max_velkost || h2 > max_velkost) {
            break;
        }
        o.kanaly[i].alokuj()?;
        dekoduj_kanal(c, &mut tok, sk, &h.wp, o, i, stream)?;
        c.over()?;
    }
    tok.koniec(&sk.kody)?;
    Ok(h.transformacie)
}

#[inline]
fn gradient(w: i64, n: i64, nw: i64) -> i64 {
    let (lo, hi) = (w.min(n), w.max(n));
    (w + n - nw).clamp(lo, hi)
}

/// Stav váženého prediktora (norma H.5).
struct Wp {
    p: ParametreWp,
    w: usize,
    chyby: [Vec<u32>; 4],
    prave: Vec<i32>,
    predikcie: [i64; 4],
    pred: i64,
}

const EXTRA_BITY: u32 = 3;
const ZAOKRUHLENIE: i64 = ((1 << EXTRA_BITY) >> 1) - 1;

impl Wp {
    fn novy(p: ParametreWp, w: usize) -> Wp {
        let n = 2 * (w + 2);
        Wp {
            p,
            w,
            chyby: [vec![0; n], vec![0; n], vec![0; n], vec![0; n]],
            prave: vec![0; n],
            predikcie: [0; 4],
            pred: 0,
        }
    }

    fn vaha(x: u64, max: u32) -> u32 {
        let posun = (63 - (x + 1).leading_zeros() as i32 - 5).max(0) as u32;
        4 + ((max as u64 * ((1u64 << 24) / ((x >> posun) + 1))) >> posun) as u32
    }

    /// Predikcia (so 3 bitmi navyše) a najväčšia chyba suseda (vlastnosť 15).
    #[allow(clippy::too_many_arguments)]
    fn predikuj(&mut self, x: usize, y: usize, n: i64, w: i64, ne: i64, nw: i64, nn: i64) -> (i64, i64) {
        let s = self.w + 2;
        let (cur, prev) = if y & 1 == 1 { (0, s) } else { (s, 0) };
        let pos_n = prev + x;
        let pos_ne = if x + 1 < self.w { pos_n + 1 } else { pos_n };
        let pos_nw = if x > 0 { pos_n - 1 } else { pos_n };
        let mut vahy = [0u32; 4];
        for (i, v) in vahy.iter_mut().enumerate() {
            // súčet v 32 bitoch s pretečením (ako v norme)
            let ch = &self.chyby[i];
            *v = Wp::vaha(ch[pos_n].wrapping_add(ch[pos_ne]).wrapping_add(ch[pos_nw]) as u64, self.p.w[i]);
        }
        let (n, w, ne, nw, nn) =
            (n << EXTRA_BITY, w << EXTRA_BITY, ne << EXTRA_BITY, nw << EXTRA_BITY, nn << EXTRA_BITY);
        let te_w = if x == 0 { 0 } else { self.prave[cur + x - 1] as i64 };
        let te_n = self.prave[pos_n] as i64;
        let te_nw = self.prave[pos_nw] as i64;
        let te_ne = self.prave[pos_ne] as i64;
        let mut najv = te_w;
        for t in [te_n, te_nw, te_ne] {
            if t.abs() > najv.abs() {
                najv = t;
            }
        }
        let sum_wn = te_n + te_w;
        self.predikcie[0] = w + ne - n;
        self.predikcie[1] = n - (((sum_wn + te_ne) * self.p.p1) >> 5);
        self.predikcie[2] = w - (((sum_wn + te_nw) * self.p.p2) >> 5);
        let p3 = &self.p.p3;
        self.predikcie[3] =
            n - ((te_nw * p3[0] + te_n * p3[1] + te_ne * p3[2] + (nn - n) * p3[3] + (nw - w) * p3[4]) >> 5);
        // vážený priemer
        let sucet: u32 = vahy.iter().sum();
        let log = 31 - sucet.leading_zeros();
        let mut sucet_v = 0u32;
        for v in vahy.iter_mut() {
            *v >>= log - 4;
            sucet_v += *v;
        }
        let mut s = (sucet_v >> 1) as i64 - 1;
        for (p, &v) in self.predikcie.iter().zip(&vahy) {
            s += p * v as i64;
        }
        let mut pred = (s * ((1i64 << 24) / sucet_v as i64)) >> 24;
        if ((te_n ^ te_w) | (te_n ^ te_nw)) <= 0 {
            let mx = w.max(ne).max(n);
            let mn = w.min(ne).min(n);
            pred = pred.clamp(mn, mx);
        }
        self.pred = pred;
        ((pred + ZAOKRUHLENIE) >> EXTRA_BITY, najv)
    }

    fn aktualizuj(&mut self, hodnota: i64, x: usize, y: usize) {
        let s = self.w + 2;
        let (cur, prev) = if y & 1 == 1 { (0, s) } else { (s, 0) };
        let v = hodnota << EXTRA_BITY;
        self.prave[cur + x] = (self.pred - v) as i32;
        for i in 0..4 {
            let e = (((self.predikcie[i] - v).abs() + ZAOKRUHLENIE) >> EXTRA_BITY) as u32;
            self.chyby[i][cur + x] = e;
            self.chyby[i][prev + x + 1] = self.chyby[i][prev + x + 1].wrapping_add(e);
        }
    }
}

fn dekoduj_kanal(
    c: &mut Citac,
    tok: &mut TokZnakov,
    sk: &StromAKody,
    wp_p: &ParametreWp,
    o: &mut Obrazok,
    i: usize,
    stream: u32,
) -> Vysledok<()> {
    let strom = &sk.strom;
    let (w, h) = (o.kanaly[i].w, o.kanaly[i].h);
    // referenčné kanály (vlastnosti 16 a vyššie): predchádzajúce kanály s rovnakými rozmermi
    let pocet_ref = strom.max_vlastnost.saturating_sub(15).div_ceil(4);
    let referencie: Vec<usize> = (0..i)
        .rev()
        .filter(|&j| {
            let (a, b) = (&o.kanaly[j], &o.kanaly[i]);
            a.w == b.w && a.h == b.h && a.hshift == b.hshift && a.vshift == b.vshift
        })
        .take(pocet_ref)
        .collect();
    let mut wp = if strom.pouziva_wp { Some(Wp::novy(*wp_p, w)) } else { None };
    // vlastnosti sú 32-bitové s pretečením (norma ich ukladá ako int32)
    let mut vl = vec![0i32; 16 + 4 * pocet_ref];
    vl[0] = i as i32;
    vl[1] = stream as i32;
    // jednoduchý strom bez vlastností: len jeden list
    let (kanaly_pred, zvysok) = o.kanaly.split_at_mut(i);
    let ch = &mut zvysok[0];
    let d = &mut ch.data;
    for y in 0..h {
        vl[2] = y as i32;
        // vlastnosť 8 = W − (vlastnosť 9 predchádzajúcej vzorky v riadku); na začiatku riadku 0
        let mut predosly_grad = 0i32;
        for x in 0..w {
            let p = y * w + x;
            let left = if x > 0 {
                d[p - 1] as i64
            } else if y > 0 {
                d[p - w] as i64
            } else {
                0
            };
            let top = if y > 0 { d[p - w] as i64 } else { left };
            let topleft = if x > 0 && y > 0 { d[p - w - 1] as i64 } else { left };
            let topright = if x + 1 < w && y > 0 { d[p - w + 1] as i64 } else { top };
            let leftleft = if x > 1 { d[p - 2] as i64 } else { left };
            let toptop = if y > 1 { d[p - 2 * w] as i64 } else { top };
            vl[3] = x as i32;
            vl[4] = top.abs() as i32;
            vl[5] = left.abs() as i32;
            vl[6] = top as i32;
            vl[7] = left as i32;
            vl[8] = (left as i32).wrapping_sub(predosly_grad);
            vl[9] = (left + top - topleft) as i32;
            predosly_grad = vl[9];
            vl[10] = (left - topleft) as i32;
            vl[11] = (topleft - top) as i32;
            vl[12] = (top - topright) as i32;
            vl[13] = (top - toptop) as i32;
            vl[14] = (left - leftleft) as i32;
            let wp_pred = if let Some(wp) = wp.as_mut() {
                let (pr, najv) = wp.predikuj(x, y, top, left, topright, topleft, toptop);
                vl[15] = najv as i32;
                pr
            } else {
                0
            };
            for (k, &j) in referencie.iter().enumerate() {
                let r = &kanaly_pred[j].data;
                let v = r[p] as i64;
                let vleft = if x > 0 { r[p - 1] as i64 } else { 0 };
                let vtop = if y > 0 { r[p - w] as i64 } else { vleft };
                let vtopleft = if x > 0 && y > 0 { r[p - w - 1] as i64 } else { vleft };
                let g = gradient(vleft, vtop, vtopleft);
                vl[16 + 4 * k] = v.abs() as i32;
                vl[17 + 4 * k] = v as i32;
                vl[18 + 4 * k] = (v - g).abs() as i32;
                vl[19 + 4 * k] = (v - g) as i32;
            }
            // prechod stromom
            let mut u = 0usize;
            let (ctx, prediktor, posun, nasobok) = loop {
                match strom.uzly[u] {
                    Uzol::Rozhodnutie { vlastnost, hodnota, lavy, pravy } => {
                        let pv = vl.get(vlastnost).copied().unwrap_or(0);
                        u = if pv > hodnota { lavy } else { pravy };
                        if u >= strom.uzly.len() {
                            return Err(chyba("strom ukazuje mimo uzlov"));
                        }
                    }
                    Uzol::List { ctx, prediktor, posun, nasobok } => break (ctx, prediktor, posun, nasobok),
                }
            };
            let pred = match prediktor {
                0 => 0,
                1 => left,
                2 => top,
                3 => (left + top) / 2,
                4 => {
                    // výber: bližší z W a N k gradientu, pri zhode N
                    let pg = top + left - topleft;
                    if (pg - left).abs() < (pg - top).abs() { left } else { top }
                }
                5 => gradient(left, top, topleft),
                6 => wp_pred,
                7 => topright,
                8 => topleft,
                9 => leftleft,
                10 => (left + topleft) / 2,
                11 => (top + topleft) / 2,
                12 => (top + topright) / 2,
                _ => {
                    let toprightright = if x + 2 < w && y > 0 { d[p - w + 2] as i64 } else { topright };
                    (6 * top - 2 * toptop + 7 * left + leftleft + toprightright + 3 * topright + 8) / 16
                }
            };
            let r = so_znamienkom(tok.citaj(&sk.kody, c, ctx)?) as i64;
            let hodnota = r.wrapping_mul(nasobok).wrapping_add(pred + posun) as i32;
            d[p] = hodnota;
            if let Some(wp) = wp.as_mut() {
                wp.aktualizuj(hodnota as i64, x, y);
            }
        }
        if c.za_koncom() {
            return Err(chyba("údaje kanála sú neúplné"));
        }
    }
    Ok(())
}

/// Spätné transformácie v obrátenom poradí.
pub(super) fn spat(o: &mut Obrazok, transformacie: &[Transformacia]) -> Vysledok<()> {
    for t in transformacie.iter().rev() {
        match t {
            Transformacia::Rct { zac, typ } => spat_rct(o, *zac, *typ)?,
            Transformacia::Paleta { zac, pocet, farieb, delt, prediktor, wp } => {
                spat_paleta(o, *zac, *pocet, *farieb, *delt, *prediktor, *wp)?
            }
            Transformacia::Squeeze { kroky } => {
                for k in kroky.iter().rev() {
                    let koniec = k.zac + k.pocet;
                    let miesto = if k.na_mieste { koniec } else { o.kanaly.len() - k.pocet };
                    for c in k.zac..koniec {
                        let r = miesto + (c - k.zac);
                        if r >= o.kanaly.len() {
                            return Err(chyba("Squeeze: chýba zvyškový kanál"));
                        }
                        let zvysok = std::mem::replace(&mut o.kanaly[r], Kanal::novy(0, 0, 0, 0));
                        let priemer = &mut o.kanaly[c];
                        let novy = if k.vodorovne {
                            rozbal_h(priemer, &zvysok)?
                        } else {
                            rozbal_v(priemer, &zvysok)?
                        };
                        o.kanaly[c] = novy;
                    }
                    o.kanaly.drain(miesto..miesto + k.pocet);
                }
            }
        }
    }
    Ok(())
}

fn spat_rct(o: &mut Obrazok, zac: usize, typ: u32) -> Vysledok<()> {
    let permutacia = typ / 7;
    let druh = typ % 7;
    let n = o.kanaly[zac].data.len();
    if o.kanaly[zac + 1].data.len() != n || o.kanaly[zac + 2].data.len() != n {
        return Err(chyba("RCT: kanály nesedia"));
    }
    let a = std::mem::take(&mut o.kanaly[zac].data);
    let b = std::mem::take(&mut o.kanaly[zac + 1].data);
    let cc = std::mem::take(&mut o.kanaly[zac + 2].data);
    let (mut v0, mut v1, mut v2) = (vec![0i32; n], vec![0i32; n], vec![0i32; n]);
    for i in 0..n {
        let (x, y, z) = (a[i], b[i], cc[i]);
        let (r0, r1, r2) = if druh == 6 {
            let tmp = x.wrapping_sub(z >> 1);
            let g = z.wrapping_add(tmp);
            let bb = tmp.wrapping_sub(y >> 1);
            (bb.wrapping_add(y), g, bb)
        } else {
            let mut tretia = z;
            let mut druha = y;
            if druh & 1 == 1 {
                tretia = tretia.wrapping_add(x);
            }
            match druh >> 1 {
                1 => druha = druha.wrapping_add(x),
                2 => druha = druha.wrapping_add(((x as i64 + tretia as i64) >> 1) as i32),
                _ => {}
            }
            (x, druha, tretia)
        };
        v0[i] = r0;
        v1[i] = r1;
        v2[i] = r2;
    }
    let p1 = (permutacia % 3) as usize;
    let p2 = ((permutacia + 1 + permutacia / 3) % 3) as usize;
    let p3 = 3 - p1 - p2;
    o.kanaly[zac + p1].data = v0;
    o.kanaly[zac + p2].data = v1;
    o.kanaly[zac + p3].data = v2;
    Ok(())
}

/// Rozdiely implicitnej palety rozdielov (norma, 72 trojíc).
#[rustfmt::skip]
const PALETA_ROZDIELOV: [[i16; 3]; 72] = [
    [0, 0, 0], [4, 4, 4], [11, 0, 0], [0, 0, -13], [0, -12, 0], [-10, -10, -10],
    [-18, -18, -18], [-27, -27, -27], [-18, -18, 0], [0, 0, -32], [-32, 0, 0], [-37, -37, -37],
    [0, -32, -32], [24, 24, 45], [50, 50, 50], [-45, -24, -24], [-24, -45, -45], [0, -24, -24],
    [-34, -34, 0], [-24, 0, -24], [-45, -45, -24], [64, 64, 64], [-32, 0, -32], [0, -32, 0],
    [-32, 0, 32], [-24, -45, -24], [45, 24, 45], [24, -24, -45], [-45, -24, 24], [80, 80, 80],
    [64, 0, 0], [0, 0, -64], [0, -64, -64], [-24, -24, 45], [96, 96, 96], [64, 64, 0],
    [45, -24, -24], [34, -34, 0], [112, 112, 112], [24, -45, -45], [45, 45, -24], [0, -32, 32],
    [24, -24, 45], [0, 96, 96], [45, -24, 24], [24, -45, -24], [-24, -45, 24], [0, -64, 0],
    [96, 0, 0], [128, 128, 128], [64, 0, 64], [144, 144, 144], [96, 96, 0], [-36, -36, 36],
    [45, -24, -45], [45, -45, -24], [0, 0, -96], [0, 128, 128], [0, 96, 0], [45, 24, -45],
    [-128, 0, 0], [24, -45, 24], [-45, 24, -45], [64, 0, -64], [64, -64, -64], [96, 0, 96],
    [45, -45, 24], [24, 45, -45], [64, 64, -64], [128, 128, 0], [0, 0, -128], [-24, 45, -45],
];

/// Predikcia hodnoty v kanáli `d` (šírka `w`) prediktorom `p` z už známych susedov.
fn predikcia(p: u32, d: &[i32], w: usize, x: usize, y: usize, wp: Option<&mut Wp>) -> i64 {
    let i = y * w + x;
    let left = if x > 0 {
        d[i - 1] as i64
    } else if y > 0 {
        d[i - w] as i64
    } else {
        0
    };
    let top = if y > 0 { d[i - w] as i64 } else { left };
    let topleft = if x > 0 && y > 0 { d[i - w - 1] as i64 } else { left };
    let topright = if x + 1 < w && y > 0 { d[i - w + 1] as i64 } else { top };
    let leftleft = if x > 1 { d[i - 2] as i64 } else { left };
    let toptop = if y > 1 { d[i - 2 * w] as i64 } else { top };
    match p {
        0 => 0,
        1 => left,
        2 => top,
        3 => (left + top) / 2,
        4 => {
            let pg = top + left - topleft;
            if (pg - left).abs() < (pg - top).abs() { left } else { top }
        }
        5 => gradient(left, top, topleft),
        6 => wp.map_or(0, |wp| wp.predikuj(x, y, top, left, topright, topleft, toptop).0),
        7 => topright,
        8 => topleft,
        9 => leftleft,
        10 => (left + topleft) / 2,
        11 => (top + topleft) / 2,
        12 => (top + topright) / 2,
        _ => {
            let toprightright = if x + 2 < w && y > 0 { d[i - w + 2] as i64 } else { topright };
            (6 * top - 2 * toptop + 7 * left + leftleft + toprightright + 3 * topright + 8) / 16
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spat_paleta(
    o: &mut Obrazok,
    zac: usize,
    pocet: usize,
    farieb: usize,
    delt: usize,
    prediktor: u32,
    wp_p: ParametreWp,
) -> Vysledok<()> {
    if o.meta < 1 || zac + 1 >= o.kanaly.len() {
        return Err(chyba("paleta: chýba kanál"));
    }
    let c0 = zac + 1;
    let index = std::mem::take(&mut o.kanaly[c0].data);
    let (w, h, hs, vs) = (o.kanaly[c0].w, o.kanaly[c0].h, o.kanaly[c0].hshift, o.kanaly[c0].vshift);
    let paleta = &o.kanaly[0];
    if paleta.data.len() < farieb * pocet {
        return Err(chyba("paleta: neúplné farby"));
    }
    let bitov = o.bitov.min(24);
    let max = (1i64 << bitov) - 1;
    let hodnota = |idx: i32, c: usize| -> i32 {
        if idx >= 0 && (idx as usize) < farieb {
            return paleta.data[c * paleta.w + idx as usize];
        }
        if idx >= 0 {
            // implicitné farby: 4 × 4 × 4 a 5 × 5 × 5 rovnomerne v rozsahu
            let i = idx as i64 - farieb as i64;
            if i < 64 {
                return (((i >> (2 * c).min(62)) % 4) * max / 4 + (1i64 << bitov.saturating_sub(3))) as i32;
            }
            let mut i = i - 64;
            for _ in 0..c {
                i /= 5;
            }
            return ((i % 5) * max / 4) as i32;
        }
        if c >= 3 {
            return 0;
        }
        // záporný index: pevná paleta rozdielov
        let i = ((-(idx as i64 + 1)) % 143) as usize;
        let mut v = PALETA_ROZDIELOV[(i + 1) >> 1][c] as i32;
        if i & 1 == 0 {
            v = -v;
        }
        if bitov > 8 {
            v <<= bitov - 8;
        }
        v
    };
    let mut nove = Vec::with_capacity(pocet);
    for c in 0..pocet {
        let mut d: Vec<i32> = index.iter().map(|&i| hodnota(i, c)).collect();
        // položky menšie ako počet rozdielov (aj záporné) sú rozdiely k predikcii
        if index.iter().any(|&i| (i as i64) < delt as i64) {
            let mut wp = (prediktor == 6).then(|| Wp::novy(wp_p, w));
            for y in 0..h {
                for x in 0..w {
                    let p = y * w + x;
                    if (index[p] as i64) < delt as i64 {
                        let pr = predikcia(prediktor, &d, w, x, y, wp.as_mut());
                        d[p] = (d[p] as i64).wrapping_add(pr) as i32;
                    } else if let Some(wp) = wp.as_mut() {
                        let left = if x > 0 {
                            d[p - 1] as i64
                        } else if y > 0 {
                            d[p - w] as i64
                        } else {
                            0
                        };
                        let top = if y > 0 { d[p - w] as i64 } else { left };
                        let topleft = if x > 0 && y > 0 { d[p - w - 1] as i64 } else { left };
                        let topright = if x + 1 < w && y > 0 { d[p - w + 1] as i64 } else { top };
                        let toptop = if y > 1 { d[p - 2 * w] as i64 } else { top };
                        wp.predikuj(x, y, top, left, topright, topleft, toptop);
                    }
                    if let Some(wp) = wp.as_mut() {
                        wp.aktualizuj(d[p] as i64, x, y);
                    }
                }
            }
        }
        nove.push(Kanal { w, h, hshift: hs, vshift: vs, data: d });
    }
    o.kanaly.splice(c0..c0 + 1, nove);
    o.kanaly.remove(0);
    o.meta -= 1;
    Ok(())
}

fn trend(b: i64, a: i64, n: i64) -> i64 {
    let mut diff = 0;
    if b >= a && a >= n {
        diff = (4 * b - 3 * n - a + 6) / 12;
        if diff - (diff & 1) > 2 * (b - a) {
            diff = 2 * (b - a) + 1;
        }
        if diff + (diff & 1) > 2 * (a - n) {
            diff = 2 * (a - n);
        }
    } else if b <= a && a <= n {
        diff = (4 * b - 3 * n - a - 6) / 12;
        if diff + (diff & 1) < 2 * (b - a) {
            diff = 2 * (b - a) - 1;
        }
        if diff - (diff & 1) < 2 * (a - n) {
            diff = 2 * (a - n);
        }
    }
    diff
}

fn rozbal_h(priemer: &Kanal, zvysok: &Kanal) -> Vysledok<Kanal> {
    let (w, h) = (priemer.w + zvysok.w, priemer.h);
    if zvysok.h != h || zvysok.w > priemer.w || priemer.w > zvysok.w + 1 {
        return Err(chyba("Squeeze: rozmery nesedia"));
    }
    let mut von = Kanal::novy(w, h, priemer.hshift - 1, priemer.vshift);
    von.alokuj()?;
    for y in 0..h {
        let a = &priemer.data[y * priemer.w..(y + 1) * priemer.w];
        let r = &zvysok.data[y * zvysok.w..(y + 1) * zvysok.w];
        let o = &mut von.data[y * w..(y + 1) * w];
        for x in 0..zvysok.w {
            let avg = a[x] as i64;
            let dalsi = if x + 1 < priemer.w { a[x + 1] as i64 } else { avg };
            let lavy = if x > 0 { o[2 * x - 1] as i64 } else { avg };
            let diff = r[x] as i64 + trend(lavy, avg, dalsi);
            let aa = avg + diff / 2;
            o[2 * x] = aa as i32;
            o[2 * x + 1] = (aa - diff) as i32;
        }
        if w & 1 == 1 {
            o[w - 1] = a[zvysok.w];
        }
    }
    Ok(von)
}

fn rozbal_v(priemer: &Kanal, zvysok: &Kanal) -> Vysledok<Kanal> {
    let (w, h) = (priemer.w, priemer.h + zvysok.h);
    if zvysok.w != w || zvysok.h > priemer.h || priemer.h > zvysok.h + 1 {
        return Err(chyba("Squeeze: rozmery nesedia"));
    }
    let mut von = Kanal::novy(w, h, priemer.hshift, priemer.vshift - 1);
    von.alokuj()?;
    for y in 0..zvysok.h {
        for x in 0..w {
            let avg = priemer.data[y * w + x] as i64;
            let dalsi = if y + 1 < priemer.h { priemer.data[(y + 1) * w + x] as i64 } else { avg };
            let horny = if y > 0 { von.data[(2 * y - 1) * w + x] as i64 } else { avg };
            let diff = zvysok.data[y * w + x] as i64 + trend(horny, avg, dalsi);
            let aa = avg + diff / 2;
            von.data[2 * y * w + x] = aa as i32;
            von.data[(2 * y + 1) * w + x] = (aa - diff) as i32;
        }
    }
    if h & 1 == 1 {
        let z = zvysok.h * w;
        von.data[(h - 1) * w..h * w].copy_from_slice(&priemer.data[z..z + w]);
    }
    Ok(von)
}
