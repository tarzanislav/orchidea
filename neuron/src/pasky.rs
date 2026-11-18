//! Páska – automatické derivovanie (spätné šírenie) pre malé modely a dolaďovanie.
//!
//! Dopredný výpočet zapisuje operácie na pásku, spätný prechod ich prejde odzadu a vypočíta gradienty
//! parametrov. Vrstva **BitLinear** počíta dopredu presne tak, ako bude model bežať po nasadení
//! (trojhodnotové váhy, 8-bitové aktivácie, celočíselný súčet), a dozadu prepúšťa gradient cez
//! kvantizáciu priamo (straight-through estimator), takže sa model učí rovno pre trojhodnotový beh.

use crate::jadra::{self, Rezim};
use crate::kvant::{TrojhodnotoveVahy, kvantizuj_aktivacie};
use crate::matica::Matica;
use orchidea_jadro::matematika::f32v;
use orchidea_jadro::nahoda::Nahoda;
use orchidea_jadro::{Hasovac, Odtlacok};

/// Identita parametra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamId(pub usize);

/// Parametre modelu s gradientmi.
#[derive(Debug, Clone, Default)]
pub struct Parametre {
    pub mena: Vec<String>,
    pub hodnoty: Vec<Matica>,
    pub gradienty: Vec<Matica>,
}

impl Parametre {
    pub fn nove() -> Self {
        Self::default()
    }

    pub fn pridaj(&mut self, meno: &str, m: Matica) -> ParamId {
        self.gradienty.push(Matica::nuly(m.riadky, m.stlpce));
        self.hodnoty.push(m);
        self.mena.push(meno.to_string());
        ParamId(self.hodnoty.len() - 1)
    }

    /// Váhy plne prepojenej vrstvy (výstupy × vstupy), rovnomerná inicializácia podľa Glorota.
    pub fn pridaj_vahy(&mut self, meno: &str, vystupy: usize, vstupy: usize, n: &mut Nahoda) -> ParamId {
        let a = orchidea_jadro::matematika::odmocnina(6.0 / (vstupy + vystupy) as f64) as f32;
        let data = (0..vystupy * vstupy).map(|_| (n.f32() * 2.0 - 1.0) * a).collect();
        self.pridaj(meno, Matica::nova(vystupy, vstupy, data))
    }

    pub fn pridaj_nuly(&mut self, meno: &str, riadky: usize, stlpce: usize) -> ParamId {
        self.pridaj(meno, Matica::nuly(riadky, stlpce))
    }

    pub fn pridaj_jednotky(&mut self, meno: &str, riadky: usize, stlpce: usize) -> ParamId {
        self.pridaj(meno, Matica::nova(riadky, stlpce, vec![1.0; riadky * stlpce]))
    }

    pub fn vynuluj_gradienty(&mut self) {
        for g in &mut self.gradienty {
            g.data.fill(0.0);
        }
    }

    pub fn pocet_hodnot(&self) -> usize {
        self.hodnoty.iter().map(|m| m.data.len()).sum()
    }

    /// Odtlačok všetkých hodnôt (bitovo) – overenie opakovateľnosti učenia.
    pub fn odtlacok(&self) -> Odtlacok {
        let mut h = Hasovac::novy();
        for (meno, m) in self.mena.iter().zip(&self.hodnoty) {
            h.pridaj(meno.as_bytes()).pridaj(&[0]);
            h.pridaj(&(m.riadky as u64).to_le_bytes()).pridaj(&(m.stlpce as u64).to_le_bytes());
            for v in &m.data {
                h.pridaj(&v.to_bits().to_le_bytes());
            }
        }
        h.dokonci()
    }
}

/// Premenná na páske.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Premenna(usize);

#[derive(Debug)]
enum Op {
    Vstup,
    Linear { x: Premenna, w: ParamId, b: Option<ParamId> },
    BitLinear { x: Premenna, w: ParamId, b: Option<ParamId>, w_deq: Vec<f32>, x_deq: Vec<f32> },
    Relu(Premenna),
    Gelu(Premenna),
    RmsNorm { x: Premenna, g: ParamId, inv_rms: Vec<f32> },
    Plus(Premenna, Premenna),
    KrizovaEntropia { logity: Premenna, triedy: Vec<usize>, pravdepodobnosti: Vec<f32> },
    StvorcovaChyba { a: Premenna, ciel: Vec<f32> },
}

#[derive(Debug)]
struct Uzol {
    op: Op,
    hodnota: Matica,
    grad: Option<Matica>,
}

/// Páska výpočtu.
#[derive(Debug)]
pub struct Paska {
    pub rezim: Rezim,
    uzly: Vec<Uzol>,
}

const GELU_K: f32 = 0.797_884_6; // √(2/π)

impl Paska {
    pub fn nova(rezim: Rezim) -> Self {
        Paska { rezim, uzly: Vec::new() }
    }

    fn pridaj(&mut self, op: Op, hodnota: Matica) -> Premenna {
        self.uzly.push(Uzol { op, hodnota, grad: None });
        Premenna(self.uzly.len() - 1)
    }

    pub fn hodnota(&self, p: Premenna) -> &Matica {
        &self.uzly[p.0].hodnota
    }

    pub fn vstup(&mut self, m: Matica) -> Premenna {
        self.pridaj(Op::Vstup, m)
    }

    /// y = x · Wᵀ + b; W má tvar (výstupy × vstupy), b (1 × výstupy).
    pub fn linear(&mut self, p: &Parametre, x: Premenna, w: ParamId, b: Option<ParamId>) -> Premenna {
        let xm = &self.uzly[x.0].hodnota;
        let wm = &p.hodnoty[w.0];
        assert_eq!(xm.stlpce, wm.stlpce, "vstup vrstvy nesedí s váhami");
        let mut y = Matica::nuly(xm.riadky, wm.riadky);
        jadra::nasob_t(
            self.rezim,
            false,
            true,
            xm.riadky,
            wm.riadky,
            wm.stlpce,
            &xm.data,
            &wm.data,
            &mut y.data,
        );
        if let Some(b) = b {
            pripocitaj_posun(&mut y, &p.hodnoty[b.0]);
        }
        self.pridaj(Op::Linear { x, w, b }, y)
    }

    /// Trojhodnotová vrstva (BitLinear): dopredu presne ako pri nasadení, dozadu priamy prechod gradientu.
    pub fn bitlinear(&mut self, p: &Parametre, x: Premenna, w: ParamId, b: Option<ParamId>) -> Premenna {
        let xm = &self.uzly[x.0].hodnota;
        let wm = &p.hodnoty[w.0];
        assert_eq!(xm.stlpce, wm.stlpce, "vstup vrstvy nesedí s váhami");
        let q = TrojhodnotoveVahy::z_f32(wm.riadky, wm.stlpce, &wm.data);
        let a = kvantizuj_aktivacie(xm.riadky, xm.stlpce, &xm.data);
        let mut y = Matica::nova(xm.riadky, wm.riadky, q.nasob(self.rezim, &a));
        if let Some(b) = b {
            pripocitaj_posun(&mut y, &p.hodnoty[b.0]);
        }
        let x_deq =
            a.hodnoty.iter().enumerate().map(|(i, &v)| v as f32 / a.mierky[i / a.stlpce.max(1)]).collect();
        self.pridaj(Op::BitLinear { x, w, b, w_deq: q.ako_f32(), x_deq }, y)
    }

    pub fn relu(&mut self, x: Premenna) -> Premenna {
        let mut y = self.uzly[x.0].hodnota.clone();
        y.data.iter_mut().for_each(|v| *v = v.max(0.0));
        self.pridaj(Op::Relu(x), y)
    }

    /// GELU (aproximácia s tanh).
    pub fn gelu(&mut self, x: Premenna) -> Premenna {
        let mut y = self.uzly[x.0].hodnota.clone();
        y.data.iter_mut().for_each(|v| {
            let u = GELU_K * (*v + 0.044_715 * *v * *v * *v);
            *v = 0.5 * *v * (1.0 + f32v::tanh(u));
        });
        self.pridaj(Op::Gelu(x), y)
    }

    /// RMSNorm po riadkoch so ziskom g (1 × stĺpce).
    pub fn rms_norm(&mut self, p: &Parametre, x: Premenna, g: ParamId) -> Premenna {
        let xm = &self.uzly[x.0].hodnota;
        let gm = &p.hodnoty[g.0];
        assert_eq!(gm.data.len(), xm.stlpce);
        let mut y = xm.clone();
        let mut inv_rms = vec![0.0f32; xm.riadky];
        for (r, inv_r) in inv_rms.iter_mut().enumerate() {
            let riadok = &mut y.data[r * xm.stlpce..(r + 1) * xm.stlpce];
            let mut s = 0.0f32;
            for v in riadok.iter() {
                s += v * v;
            }
            let inv = 1.0 / (s / xm.stlpce as f32 + 1e-6).sqrt();
            *inv_r = inv;
            for (v, &gi) in riadok.iter_mut().zip(&gm.data) {
                *v = *v * inv * gi;
            }
        }
        self.pridaj(Op::RmsNorm { x, g, inv_rms }, y)
    }

    pub fn plus(&mut self, a: Premenna, b: Premenna) -> Premenna {
        let mut y = self.uzly[a.0].hodnota.clone();
        let bm = &self.uzly[b.0].hodnota;
        assert_eq!(y.data.len(), bm.data.len());
        y.data.iter_mut().zip(&bm.data).for_each(|(x, &z)| *x += z);
        self.pridaj(Op::Plus(a, b), y)
    }

    /// Priemerná krížová entropia so softmaxom (logity: dávka × triedy).
    pub fn krizova_entropia(&mut self, logity: Premenna, triedy: &[usize]) -> Premenna {
        let lm = &self.uzly[logity.0].hodnota;
        assert_eq!(lm.riadky, triedy.len());
        let mut pravdepodobnosti = vec![0.0f32; lm.data.len()];
        let mut strata = 0.0f64;
        for r in 0..lm.riadky {
            let riadok = lm.riadok(r);
            let max = riadok.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
            let pr = &mut pravdepodobnosti[r * lm.stlpce..(r + 1) * lm.stlpce];
            let mut sucet = 0.0f32;
            for (p, &l) in pr.iter_mut().zip(riadok) {
                *p = f32v::exp(l - max);
                sucet += *p;
            }
            for p in pr.iter_mut() {
                *p /= sucet;
            }
            strata -= orchidea_jadro::matematika::ln(pr[triedy[r]].max(1e-12) as f64);
        }
        let strata = (strata / lm.riadky.max(1) as f64) as f32;
        self.pridaj(
            Op::KrizovaEntropia { logity, triedy: triedy.to_vec(), pravdepodobnosti },
            Matica::nova(1, 1, vec![strata]),
        )
    }

    /// Priemerná štvorcová chyba.
    pub fn stvorcova_chyba(&mut self, a: Premenna, ciel: &[f32]) -> Premenna {
        let am = &self.uzly[a.0].hodnota;
        assert_eq!(am.data.len(), ciel.len());
        let mut s = 0.0f32;
        for (x, y) in am.data.iter().zip(ciel) {
            s += (x - y) * (x - y);
        }
        let s = s / ciel.len().max(1) as f32;
        self.pridaj(Op::StvorcovaChyba { a, ciel: ciel.to_vec() }, Matica::nova(1, 1, vec![s]))
    }

    fn pripocitaj_grad(&mut self, p: Premenna, g: &[f32]) {
        let u = &mut self.uzly[p.0];
        match &mut u.grad {
            Some(m) => m.data.iter_mut().zip(g).for_each(|(a, &b)| *a += b),
            None => u.grad = Some(Matica::nova(u.hodnota.riadky, u.hodnota.stlpce, g.to_vec())),
        }
    }

    /// Spätný prechod od skalárnej straty; gradienty sa pripočítajú k parametrom.
    pub fn spatne(&mut self, strata: Premenna, p: &mut Parametre) {
        assert_eq!(self.uzly[strata.0].hodnota.data.len(), 1, "strata musí byť jedno číslo");
        self.uzly[strata.0].grad = Some(Matica::nova(1, 1, vec![1.0]));
        let rezim = self.rezim;
        for i in (0..=strata.0).rev() {
            let Some(dy) = self.uzly[i].grad.take() else { continue };
            let op = std::mem::replace(&mut self.uzly[i].op, Op::Vstup);
            match &op {
                Op::Vstup => {}
                Op::Linear { x, w, b } | Op::BitLinear { x, w, b, .. } => {
                    let (w_data, x_data): (&[f32], &[f32]) = match &op {
                        Op::BitLinear { w_deq, x_deq, .. } => (w_deq, x_deq),
                        _ => (&p.hodnoty[w.0].data, &self.uzly[x.0].hodnota.data),
                    };
                    let wm = &p.hodnoty[w.0];
                    let (davka, vystupy, vstupy) = (dy.riadky, wm.riadky, wm.stlpce);
                    // dX = dY · W
                    let mut dx = vec![0.0f32; davka * vstupy];
                    jadra::nasob(rezim, davka, vstupy, vystupy, &dy.data, w_data, &mut dx);
                    // dW = dYᵀ · X
                    let mut dw = vec![0.0f32; vystupy * vstupy];
                    jadra::nasob_t(rezim, true, false, vystupy, vstupy, davka, &dy.data, x_data, &mut dw);
                    p.gradienty[w.0].data.iter_mut().zip(&dw).for_each(|(a, &b)| *a += b);
                    if let Some(b) = b {
                        let gb = &mut p.gradienty[b.0].data;
                        for r in 0..davka {
                            gb.iter_mut().zip(dy.riadok(r)).for_each(|(a, &v)| *a += v);
                        }
                    }
                    let x = *x;
                    self.pripocitaj_grad(x, &dx);
                }
                Op::Relu(x) => {
                    let xm = &self.uzly[x.0].hodnota;
                    let g: Vec<f32> =
                        dy.data.iter().zip(&xm.data).map(|(&d, &v)| if v > 0.0 { d } else { 0.0 }).collect();
                    self.pripocitaj_grad(*x, &g);
                }
                Op::Gelu(x) => {
                    let xm = &self.uzly[x.0].hodnota;
                    let g: Vec<f32> = dy
                        .data
                        .iter()
                        .zip(&xm.data)
                        .map(|(&d, &v)| {
                            let u = GELU_K * (v + 0.044_715 * v * v * v);
                            let t = f32v::tanh(u);
                            let du = GELU_K * (1.0 + 3.0 * 0.044_715 * v * v);
                            d * (0.5 * (1.0 + t) + 0.5 * v * (1.0 - t * t) * du)
                        })
                        .collect();
                    self.pripocitaj_grad(*x, &g);
                }
                Op::RmsNorm { x, g, inv_rms } => {
                    let xm = &self.uzly[x.0].hodnota;
                    let gm = &p.hodnoty[g.0];
                    let n = xm.stlpce;
                    let mut dx = vec![0.0f32; xm.data.len()];
                    for r in 0..xm.riadky {
                        let inv = inv_rms[r];
                        let xr = xm.riadok(r);
                        let dyr = dy.riadok(r);
                        let mut sucet = 0.0f32;
                        for c in 0..n {
                            let xhat = xr[c] * inv;
                            sucet += dyr[c] * gm.data[c] * xhat;
                            p.gradienty[g.0].data[c] += dyr[c] * xhat;
                        }
                        let priemer = sucet / n as f32;
                        for c in 0..n {
                            let xhat = xr[c] * inv;
                            dx[r * n + c] = (dyr[c] * gm.data[c] - xhat * priemer) * inv;
                        }
                    }
                    self.pripocitaj_grad(*x, &dx);
                }
                Op::Plus(a, b) => {
                    let (a, b) = (*a, *b);
                    self.pripocitaj_grad(a, &dy.data);
                    self.pripocitaj_grad(b, &dy.data);
                }
                Op::KrizovaEntropia { logity, triedy, pravdepodobnosti } => {
                    let lm = &self.uzly[logity.0].hodnota;
                    let davka = lm.riadky.max(1) as f32;
                    let mut g = pravdepodobnosti.clone();
                    for (r, &t) in triedy.iter().enumerate() {
                        g[r * lm.stlpce + t] -= 1.0;
                    }
                    let k = dy.data[0] / davka;
                    g.iter_mut().for_each(|v| *v *= k);
                    self.pripocitaj_grad(*logity, &g);
                }
                Op::StvorcovaChyba { a, ciel } => {
                    let am = &self.uzly[a.0].hodnota;
                    let k = 2.0 * dy.data[0] / ciel.len().max(1) as f32;
                    let g: Vec<f32> = am.data.iter().zip(ciel).map(|(x, y)| k * (x - y)).collect();
                    self.pripocitaj_grad(*a, &g);
                }
            }
            self.uzly[i].op = op;
        }
    }
}

fn pripocitaj_posun(y: &mut Matica, b: &Matica) {
    assert_eq!(b.data.len(), y.stlpce, "posun nesedí s počtom výstupov");
    for r in 0..y.riadky {
        y.data[r * y.stlpce..(r + 1) * y.stlpce].iter_mut().zip(&b.data).for_each(|(v, &x)| *v += x);
    }
}

/// Optimalizátor AdamW (deterministický).
#[derive(Debug, Clone)]
pub struct AdamW {
    pub rychlost: f32,
    pub beta1: f32,
    pub beta2: f32,
    pub eps: f32,
    pub utlm_vah: f32,
    m: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
    b1t: f32,
    b2t: f32,
}

impl AdamW {
    pub fn novy(rychlost: f32) -> Self {
        AdamW {
            rychlost,
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            utlm_vah: 0.0,
            m: vec![],
            v: vec![],
            b1t: 1.0,
            b2t: 1.0,
        }
    }

    pub fn krok(&mut self, p: &mut Parametre) {
        if self.m.len() != p.hodnoty.len() {
            self.m = p.hodnoty.iter().map(|h| vec![0.0; h.data.len()]).collect();
            self.v = self.m.clone();
        }
        self.b1t *= self.beta1;
        self.b2t *= self.beta2;
        let (k1, k2) = (1.0 - self.b1t, 1.0 - self.b2t);
        for (i, (h, g)) in p.hodnoty.iter_mut().zip(&p.gradienty).enumerate() {
            for (j, (w, &d)) in h.data.iter_mut().zip(&g.data).enumerate() {
                let m = &mut self.m[i][j];
                let v = &mut self.v[i][j];
                *m = self.beta1 * *m + (1.0 - self.beta1) * d;
                *v = self.beta2 * *v + (1.0 - self.beta2) * d * d;
                let mh = *m / k1;
                let vh = *v / k2;
                *w -= self.rychlost * (mh / (vh.sqrt() + self.eps) + self.utlm_vah * *w);
            }
        }
    }
}
