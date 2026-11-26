//! CELT – dekódovanie tvaru pásiem (RFC 6716 §4.3.4): PVQ (§4.3.4.2), rozprestretie (§4.3.4.3), delenie
//! a stereo (§4.3.4.4), zmena časovo-frekvenčného rozlíšenia (§4.3.4.5), skladanie spektra a anti-kolaps (§4.3.5).
//!
//! Text RFC opisuje princípy; presné pravidlá, ktoré určujú čítanie bitov (rozdelenie bitov pri delení, kvantovanie
//! uhla, skladanie, masky kolapsu), **odvodené meraním proti libopus** (rozhodnutie A, #15): konečný stav
//! rozsahového dekodéra po každom pakete a SNR výstupu (`media/nastroje/over_opus.py`). Číselné tabuľky sú
//! z prílohy A (`celt_tabulky`).

#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use super::celt_tabulky as t;
use super::rozsah::RozsahovyDekoder;

pub(crate) const BITRES: i32 = 3;
pub(crate) const NB_PASIEM: usize = 21;

/// Rozprestretie (RFC 6716 §4.3.4.3): 0 žiadne, 1 ľahké, 2 normálne, 3 silné.
pub(crate) const ROZPRESTRETIE_SILNE: usize = 3;

pub(crate) fn lcg(s: u32) -> u32 {
    s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

/// Vyrovnávacia pamäť pulzov pre pásmo a LM (−1 … 3): `[max q, bity(1), …, bity(max q)]` v 1/8 bitu − 1.
pub(crate) fn cache(lm: i32, pasmo: usize) -> Option<&'static [u8]> {
    let ix = *t::CACHE_INDEX.get(((lm + 1) as usize) * NB_PASIEM + pasmo)?;
    if ix < 0 {
        return None;
    }
    let c = t::CACHE_BITY.get(ix as usize..)?;
    let n = *c.first()? as usize;
    c.get(..=n)
}

/// Najbližší počet „pseudopulzov“ q k počtu bitov (RFC 6716 §4.3.4.1, zaokrúhlenie nadol pri polovici).
pub(crate) fn bity_na_pulzy(pasmo: usize, lm: i32, bity: i32) -> i32 {
    let Some(c) = cache(lm, pasmo) else { return 0 };
    let (mut lo, mut hi) = (0usize, c[0] as usize);
    let bity = bity - 1;
    for _ in 0..6 {
        let stred = (lo + hi + 1) >> 1;
        if c[stred] as i32 >= bity {
            hi = stred;
        } else {
            lo = stred;
        }
    }
    let dolu = if lo == 0 { -1 } else { c[lo] as i32 };
    if bity - dolu <= c[hi] as i32 - bity { lo as i32 } else { hi as i32 }
}

pub(crate) fn pulzy_na_bity(pasmo: usize, lm: i32, q: i32) -> i32 {
    if q == 0 {
        return 0;
    }
    cache(lm, pasmo).and_then(|c| c.get(q as usize)).map_or(0, |&b| b as i32 + 1)
}

/// Počet pulzov K z pseudopulzov q (prvých 8 presne, potom 8 krokov na oktávu).
pub(crate) fn pulzy(q: i32) -> i32 {
    if q < 8 { q } else { (8 + (q & 7)) << ((q >> 3) - 1) }
}

/// PVQ (RFC 6716 §4.3.4.2): `V(N, K)` rekurziou riadok po riadku; dekódovanie indexu podľa textu.
fn dekoduj_pulzy(rd: &mut RozsahovyDekoder, n: usize, k: usize, iy: &mut [i32]) -> f32 {
    // riadok V(n, ·) pre n = N; V(0, 0) = 1, V(0, k > 0) = 0, V(n, k) = V(n−1, k) + V(n, k−1) + V(n−1, k−1)
    let mut riadok = vec![0u64; k + 1];
    riadok[0] = 1;
    for _ in 0..n {
        let mut predosly_kmin1 = riadok[0]; // V(n−1, k−1)
        for kk in 1..=k {
            let stare = riadok[kk]; // V(n−1, kk)
            riadok[kk] = stare.saturating_add(riadok[kk - 1]).saturating_add(predosly_kmin1).min(1 << 40);
            predosly_kmin1 = stare;
        }
    }
    let ft = riadok[k];
    if ft == 0 || ft > u32::MAX as u64 {
        rd.chyba = true;
        iy[..n].fill(0);
        return 1.0;
    }
    let mut i = rd.uint(ft as u32) as u64;
    // krok nadol: z V(m, ·) sa vypočíta V(m−1, ·) = V(m, k) − V(m, k−1) − V(m−1, k−1) (presne v celých číslach)
    let mut nizsi = vec![0u64; k + 1];
    let mut kk = k;
    let mut ryy = 0.0f32;
    for j in 0..n {
        nizsi[0] = 1;
        for x in 1..=k {
            nizsi[x] = riadok[x].wrapping_sub(riadok[x - 1]).wrapping_sub(nizsi[x - 1]);
        }
        // p = (V(N−j−1, k) + V(N−j, k)) / 2
        let mut p = nizsi[kk].wrapping_add(riadok[kk]) >> 1;
        let znamienko = if i < p {
            1
        } else {
            i = i.wrapping_sub(p);
            -1
        };
        let k0 = kk;
        p = p.wrapping_sub(nizsi[kk]);
        while p > i && kk > 0 {
            kk -= 1;
            p = p.wrapping_sub(nizsi[kk]);
        }
        iy[j] = znamienko * (k0 - kk) as i32;
        i = i.wrapping_sub(p);
        ryy += (iy[j] * iy[j]) as f32;
        core::mem::swap(&mut riadok, &mut nizsi);
    }
    ryy
}

/// Rotácia pre rozprestretie (RFC 6716 §4.3.4.3) – dekodér robí inverznú rotáciu (záporný uhol).
fn rotacia1(x: &mut [f32], dlzka: usize, krok: usize, c: f32, s: f32) {
    if dlzka <= krok {
        return;
    }
    for i in 0..dlzka - krok {
        let (x1, x2) = (x[i], x[i + krok]);
        x[i + krok] = c * x2 + s * x1;
        x[i] = c * x1 - s * x2;
    }
    if dlzka < 2 * krok + 1 {
        return;
    }
    for i in (0..dlzka - 2 * krok).rev() {
        let (x1, x2) = (x[i], x[i + krok]);
        x[i + krok] = c * x2 + s * x1;
        x[i] = c * x1 - s * x2;
    }
}

fn rozprestri(x: &mut [f32], dlzka: usize, blokov: usize, k: i32, rozprestretie: usize) {
    if 2 * k as usize >= dlzka || rozprestretie == 0 {
        return;
    }
    let f = t::FAKTOR_ROZPRESTRETIA[rozprestretie - 1] as f32;
    let g = dlzka as f32 / (dlzka as f32 + f * k as f32);
    let theta = 0.5 * g * g;
    let pi2 = core::f64::consts::FRAC_PI_2;
    let c = orchidea_jadro::matematika::cos(pi2 * theta as f64) as f32;
    let s = orchidea_jadro::matematika::cos(pi2 * (1.0 - theta as f64)) as f32;
    let mut krok2 = 0;
    if dlzka >= 8 * blokov {
        krok2 = 1;
        // ≈ zaokrúhlená √(dĺžka / blokov) (RFC: „round(sqrt(N/nb_blocks))“)
        while (krok2 * krok2 + krok2) * blokov + (blokov >> 2) < dlzka {
            krok2 += 1;
        }
    }
    let d = dlzka / blokov;
    for b in 0..blokov {
        let blok = &mut x[b * d..(b + 1) * d];
        if krok2 > 0 {
            rotacia1(blok, d, krok2, s, c);
        }
        rotacia1(blok, d, 1, c, s);
    }
}

fn maska_kolapsu(iy: &[i32], n: usize, b: usize) -> u32 {
    if b <= 1 {
        return 1;
    }
    let n0 = n / b;
    (0..b).fold(0, |m, i| m | (((iy[i * n0..(i + 1) * n0].iter().any(|&v| v != 0)) as u32) << i))
}

fn normuj(x: &mut [f32], zisk: f32) {
    let e: f32 = 1e-15 + x.iter().map(|v| v * v).sum::<f32>();
    let g = zisk / e.sqrt();
    for v in x {
        *v *= g;
    }
}

/// Haarova transformácia (jedna úroveň) s krokom.
fn haar1(x: &mut [f32], n0: usize, krok: usize) {
    let n0 = n0 >> 1;
    let h = core::f32::consts::FRAC_1_SQRT_2;
    for i in 0..krok {
        for j in 0..n0 {
            let a = h * x[krok * 2 * j + i];
            let b = h * x[krok * (2 * j + 1) + i];
            x[krok * 2 * j + i] = a + b;
            x[krok * (2 * j + 1) + i] = a - b;
        }
    }
}

fn poradie_y(krok: usize) -> &'static [u8] {
    &t::PORADIE_Y[krok - 2..2 * krok - 2]
}

fn rozloz_hadamard(x: &mut [f32], n0: usize, krok: usize, hadamard: bool) {
    let n = n0 * krok;
    let mut tmp = vec![0.0f32; n];
    for i in 0..krok {
        let r = if hadamard { poradie_y(krok)[i] as usize } else { i };
        for j in 0..n0 {
            tmp[r * n0 + j] = x[j * krok + i];
        }
    }
    x[..n].copy_from_slice(&tmp);
}

fn zloz_hadamard(x: &mut [f32], n0: usize, krok: usize, hadamard: bool) {
    let n = n0 * krok;
    let mut tmp = vec![0.0f32; n];
    for i in 0..krok {
        let r = if hadamard { poradie_y(krok)[i] as usize } else { i };
        for j in 0..n0 {
            tmp[j * krok + i] = x[r * n0 + j];
        }
    }
    x[..n].copy_from_slice(&tmp);
}

fn frac_mul16(a: i32, b: i32) -> i32 {
    (16384 + (a as i16 as i32) * (b as i16 as i32)) >> 15
}

/// Celočíselný kosínus uhla v jednotkách π/2 · x/16384 (určuje rozdelenie bitov – musí byť bitovo presný).
fn kosinus_presny(x: i32) -> i32 {
    let x2 = (4096 + x * x) >> 13;
    let x2 = (32767 - x2) + frac_mul16(x2, -7651 + frac_mul16(x2, 8277 + frac_mul16(-626, x2)));
    1 + x2
}

fn ilog(x: i32) -> i32 {
    32 - (x as u32).leading_zeros() as i32
}

/// log2(sin/cos) v Q11 (bitovo presne).
fn log2_tan(sinus: i32, kosinus: i32) -> i32 {
    let (lc, ls) = (ilog(kosinus), ilog(sinus));
    let kosinus = kosinus << (15 - lc);
    let sinus = sinus << (15 - ls);
    (ls - lc) * (1 << 11) + frac_mul16(sinus, frac_mul16(sinus, -2597) + 7932)
        - frac_mul16(kosinus, frac_mul16(kosinus, -2597) + 7932)
}

fn isqrt32(x: u32) -> u32 {
    // celá časť √x (presne)
    let mut r = (x as f64).sqrt() as u32;
    while (r as u64) * (r as u64) > x as u64 {
        r -= 1;
    }
    while ((r + 1) as u64) * ((r + 1) as u64) <= x as u64 {
        r += 1;
    }
    r
}

/// Spoločný stav dekódovania pásiem jedného rámca.
pub(crate) struct Pasma<'a, 'b> {
    pub rd: &'a mut RozsahovyDekoder<'b>,
    pub pasmo: usize,
    pub intenzita: usize,
    pub rozprestretie: usize,
    pub tf_zmena: i32,
    pub zostava: i32,
    pub semienko: u32,
}

struct Theta {
    inv: bool,
    imid: i32,
    iside: i32,
    delta: i32,
    itheta: i32,
    qalloc: i32,
}

impl Pasma<'_, '_> {
    fn kvantuj_n1(&mut self, x: &mut [f32], y: Option<&mut [f32]>, von: Option<&mut [f32]>) -> u32 {
        let jeden = |v: &mut f32, s: &mut Self| {
            let mut z = 0;
            if s.zostava >= 1 << BITRES {
                z = s.rd.bity(1);
                s.zostava -= 1 << BITRES;
            }
            *v = if z != 0 { -1.0 } else { 1.0 };
        };
        jeden(&mut x[0], self);
        if let Some(y) = y {
            jeden(&mut y[0], self);
        }
        if let Some(von) = von {
            von[0] = x[0];
        }
        1
    }

    /// Uhol theta pri delení (mono) alebo spájaní kanálov (stereo).
    fn theta(
        &mut self,
        n: usize,
        b: &mut i32,
        bl: usize,
        b0: usize,
        lm: i32,
        stereo: bool,
        plnenie: &mut u32,
    ) -> Theta {
        let strop = t::LOG_N[self.pasmo] as i32 + lm * (1 << BITRES);
        let posun = (strop >> 1) - if stereo && n == 2 { 16 } else { 4 };
        // počet krokov uhla qn
        let mut n2 = 2 * n as i32 - 1;
        if stereo && n == 2 {
            n2 -= 1;
        }
        let mut qb = (*b + n2 * posun) / n2;
        qb = qb.min(*b - strop - (4 << BITRES)).min(8 << BITRES);
        let mut qn = if qb < (1 << BITRES >> 1) {
            1
        } else {
            let q = (t::EXP2_8[(qb & 7) as usize] as i32) >> (14 - (qb >> BITRES));
            (q + 1) >> 1 << 1
        };
        if stereo && self.pasmo >= self.intenzita {
            qn = 1;
        }
        let tell = self.rd.tell_frac();
        let mut itheta = 0;
        let mut inv = false;
        if qn != 1 {
            if stereo && n > 2 {
                // stupňovité rozdelenie: hodnoty do qn/2 trikrát pravdepodobnejšie
                let (p0, x0) = (3u32, (qn / 2) as u32);
                let ft = p0 * (x0 + 1) + x0;
                let fs = self.rd.dekoduj(ft);
                let x = if fs < (x0 + 1) * p0 { fs / p0 } else { x0 + 1 + (fs - (x0 + 1) * p0) };
                let (fl, fh) = if x <= x0 {
                    (p0 * x, p0 * (x + 1))
                } else {
                    ((x - 1 - x0) + (x0 + 1) * p0, (x - x0) + (x0 + 1) * p0)
                };
                self.rd.aktualizuj(fl, fh, ft);
                itheta = x as i32;
            } else if b0 > 1 || stereo {
                itheta = self.rd.uint(qn as u32 + 1) as i32;
            } else {
                // trojuholníkové rozdelenie
                let h = (qn >> 1) as u32;
                let ft = (h + 1) * (h + 1);
                let fm = self.rd.dekoduj(ft);
                let (fl, fs);
                if fm < (h * (h + 1)) >> 1 {
                    itheta = ((isqrt32(8 * fm + 1) - 1) >> 1) as i32;
                    fs = itheta as u32 + 1;
                    fl = (itheta as u32 * (itheta as u32 + 1)) >> 1;
                } else {
                    itheta = ((2 * (qn as u32 + 1) - isqrt32(8 * (ft - fm - 1) + 1)) >> 1) as i32;
                    fs = (qn + 1 - itheta) as u32;
                    fl = ft - (((qn + 1 - itheta) * (qn + 2 - itheta)) as u32 >> 1);
                }
                self.rd.aktualizuj(fl, fl + fs, ft);
            }
            itheta = itheta * 16384 / qn;
        } else if stereo {
            if *b > 2 << BITRES && self.zostava > 2 << BITRES {
                inv = self.rd.bit_logp(2);
            }
            itheta = 0;
        }
        let qalloc = self.rd.tell_frac() - tell;
        *b -= qalloc;
        let maska = (1u32 << bl) - 1;
        let (imid, iside, delta) = if itheta == 0 {
            *plnenie &= maska;
            (32767, 0, -16384)
        } else if itheta == 16384 {
            *plnenie &= maska << bl;
            (0, 32767, 16384)
        } else {
            let imid = kosinus_presny(itheta);
            let iside = kosinus_presny(16384 - itheta);
            (imid, iside, frac_mul16(((n as i32) - 1) << 7, log2_tan(iside, imid)))
        };
        Theta { inv, imid, iside, delta, itheta, qalloc }
    }

    /// Rekurzívne delenie pásma (mono) a PVQ.
    fn oddiel(
        &mut self,
        x: &mut [f32],
        n: usize,
        mut b: i32,
        mut bl: usize,
        nizke: Option<&[f32]>,
        mut lm: i32,
        zisk: f32,
        mut plnenie: u32,
    ) -> u32 {
        let b0 = bl;
        let delit =
            lm != -1 && n > 2 && cache(lm, self.pasmo).is_some_and(|c| b > c[c[0] as usize] as i32 + 12);
        if delit {
            let n = n >> 1;
            lm -= 1;
            if bl == 1 {
                plnenie = (plnenie & 1) | (plnenie << 1);
            }
            bl = (bl + 1) >> 1;
            let th = self.theta(n, &mut b, bl, b0, lm, false, &mut plnenie);
            let mut delta = th.delta;
            let (mid, side) = (th.imid as f32 / 32768.0, th.iside as f32 / 32768.0);
            // viac bitov krátkym blokom s nízkou energiou (maskovanie pred ozvenou)
            if b0 > 1 && (th.itheta & 0x3fff) != 0 {
                if th.itheta > 8192 {
                    delta -= delta >> (4 - lm);
                } else {
                    delta = (delta + ((n as i32) << BITRES >> (5 - lm))).min(0);
                }
            }
            let mut mbits = ((b - delta) / 2).min(b).max(0);
            let mut sbits = b - mbits;
            self.zostava -= th.qalloc;
            let (xl, xh) = x.split_at_mut(n);
            let nizke2 = nizke.map(|l| &l[n..]);
            let pred = self.zostava;
            let mut cm;
            if mbits >= sbits {
                cm = self.oddiel(xl, n, mbits, bl, nizke, lm, zisk * mid, plnenie);
                let r = mbits - (pred - self.zostava);
                if r > 3 << BITRES && th.itheta != 0 {
                    sbits += r - (3 << BITRES);
                }
                cm |= self.oddiel(xh, n, sbits, bl, nizke2, lm, zisk * side, plnenie >> bl) << (b0 >> 1);
            } else {
                cm = self.oddiel(xh, n, sbits, bl, nizke2, lm, zisk * side, plnenie >> bl) << (b0 >> 1);
                let r = sbits - (pred - self.zostava);
                if r > 3 << BITRES && th.itheta != 16384 {
                    mbits += r - (3 << BITRES);
                }
                cm |= self.oddiel(xl, n, mbits, bl, nizke, lm, zisk * mid, plnenie);
            }
            return cm;
        }
        // bez delenia: PVQ
        let mut q = bity_na_pulzy(self.pasmo, lm, b);
        let mut cena = pulzy_na_bity(self.pasmo, lm, q);
        self.zostava -= cena;
        while self.zostava < 0 && q > 0 {
            self.zostava += cena;
            q -= 1;
            cena = pulzy_na_bity(self.pasmo, lm, q);
            self.zostava -= cena;
        }
        if q != 0 {
            let k = pulzy(q);
            let mut iy = vec![0i32; n];
            let ryy = dekoduj_pulzy(self.rd, n, k as usize, &mut iy);
            let g = zisk / ryy.sqrt();
            for j in 0..n {
                x[j] = g * iy[j] as f32;
            }
            rozprestri(x, n, bl, k, self.rozprestretie);
            return maska_kolapsu(&iy, n, bl);
        }
        // bez pulzov: šum alebo poskladané spektrum
        let maska = (1u32 << bl) - 1;
        plnenie &= maska;
        if plnenie == 0 {
            x[..n].fill(0.0);
            return 0;
        }
        let cm;
        match nizke {
            None => {
                for v in x[..n].iter_mut() {
                    self.semienko = lcg(self.semienko);
                    *v = ((self.semienko as i32) >> 20) as f32;
                }
                cm = maska;
            }
            Some(l) => {
                for j in 0..n {
                    self.semienko = lcg(self.semienko);
                    let d = if self.semienko & 0x8000 != 0 { 1.0 / 256.0 } else { -1.0 / 256.0 };
                    x[j] = l[j] + d;
                }
                cm = plnenie;
            }
        }
        normuj(&mut x[..n], zisk);
        cm
    }

    /// Pásmo jedného kanála: zmena TF, preusporiadanie, delenie; `von` – kópia na neskoršie skladanie.
    pub fn pasmo_mono(
        &mut self,
        x: &mut [f32],
        n: usize,
        b: i32,
        bl: usize,
        nizke: Option<&[f32]>,
        lm: i32,
        von: Option<&mut [f32]>,
        zisk: f32,
        mut plnenie: u32,
    ) -> u32 {
        let n0 = n;
        let mut nb = n;
        let mut bl0 = bl;
        let dlhe = bl0 == 1;
        let mut bl = bl;
        nb /= bl;
        if n == 1 {
            return self.kvantuj_n1(x, None, von);
        }
        let mut tf = self.tf_zmena;
        let spoj = tf.max(0) as usize;
        let mut nizka: Option<Vec<f32>> = nizke.map(|l| l[..n].to_vec());
        for k in 0..spoj {
            if let Some(l) = nizka.as_mut() {
                haar1(l, n >> k, 1 << k);
            }
            plnenie = t::PREKLADANIE_BITOV[(plnenie & 0xF) as usize] as u32
                | (t::PREKLADANIE_BITOV[(plnenie >> 4) as usize & 0xF] as u32) << 2;
        }
        bl >>= spoj;
        nb <<= spoj;
        let mut delenie_casu = 0;
        while nb & 1 == 0 && tf < 0 {
            if let Some(l) = nizka.as_mut() {
                haar1(l, nb, bl);
            }
            plnenie |= plnenie << bl;
            bl <<= 1;
            nb >>= 1;
            delenie_casu += 1;
            tf += 1;
        }
        bl0 = bl;
        let nb0 = nb;
        if bl0 > 1
            && let Some(l) = nizka.as_mut()
        {
            rozloz_hadamard(l, nb >> spoj, bl0 << spoj, dlhe);
        }
        let mut cm = self.oddiel(x, n, b, bl, nizka.as_deref(), lm, zisk, plnenie);
        if bl0 > 1 {
            zloz_hadamard(x, nb >> spoj, bl0 << spoj, dlhe);
        }
        let (mut nb, mut bl) = (nb0, bl0);
        for _ in 0..delenie_casu {
            bl >>= 1;
            nb <<= 1;
            cm |= cm >> bl;
            haar1(x, nb, bl);
        }
        for k in 0..spoj {
            cm = t::ROZKLADANIE_BITOV[(cm & 0xF) as usize] as u32;
            haar1(x, n0 >> k, 1 << k);
        }
        bl <<= spoj;
        if let Some(von) = von {
            let s = (n0 as f32).sqrt();
            for j in 0..n0 {
                von[j] = s * x[j];
            }
        }
        cm & ((1u32 << bl) - 1)
    }

    /// Spojené stereo (stred/bok s uhlom; pri intenzitnom stereu len stred).
    pub fn pasmo_stereo(
        &mut self,
        x: &mut [f32],
        y: &mut [f32],
        n: usize,
        mut b: i32,
        bl: usize,
        nizke: Option<&[f32]>,
        lm: i32,
        von: Option<&mut [f32]>,
        plnenie: u32,
    ) -> u32 {
        let povodne = plnenie;
        let mut plnenie = plnenie;
        if n == 1 {
            return self.kvantuj_n1(x, Some(y), von);
        }
        let th = self.theta(n, &mut b, bl, bl, lm, true, &mut plnenie);
        let (mid, side) = (th.imid as f32 / 32768.0, th.iside as f32 / 32768.0);
        let cm;
        if n == 2 {
            let mut mbits = b;
            let sbits = if th.itheta != 0 && th.itheta != 16384 { 1 << BITRES } else { 0 };
            mbits -= sbits;
            let c = th.itheta > 8192;
            self.zostava -= th.qalloc + sbits;
            let mut znak = 0;
            if sbits != 0 {
                znak = self.rd.bity(1) as i32;
            }
            let znak = (1 - 2 * znak) as f32;
            {
                let (x2, y2): (&mut [f32], &mut [f32]) =
                    if c { (&mut *y, &mut *x) } else { (&mut *x, &mut *y) };
                cm = self.pasmo_mono(x2, n, mbits, bl, nizke, lm, von, 1.0, povodne);
                y2[0] = -znak * x2[1];
                y2[1] = znak * x2[0];
            }
            x[0] *= mid;
            x[1] *= mid;
            y[0] *= side;
            y[1] *= side;
            for j in 0..2 {
                let a = x[j];
                x[j] = a - y[j];
                y[j] += a;
            }
        } else {
            let mut mbits = ((b - th.delta) / 2).min(b).max(0);
            let mut sbits = b - mbits;
            self.zostava -= th.qalloc;
            let pred = self.zostava;
            let mut c;
            if mbits >= sbits {
                c = self.pasmo_mono(x, n, mbits, bl, nizke, lm, von, 1.0, plnenie);
                let r = mbits - (pred - self.zostava);
                if r > 3 << BITRES && th.itheta != 0 {
                    sbits += r - (3 << BITRES);
                }
                c |= self.pasmo_mono(y, n, sbits, bl, None, lm, None, side, plnenie >> bl);
            } else {
                c = self.pasmo_mono(y, n, sbits, bl, None, lm, None, side, plnenie >> bl);
                let r = sbits - (pred - self.zostava);
                if r > 3 << BITRES && th.itheta != 16384 {
                    mbits += r - (3 << BITRES);
                }
                c |= self.pasmo_mono(x, n, mbits, bl, nizke, lm, von, 1.0, plnenie);
            }
            cm = c;
            spoj_stereo(x, y, mid, n);
        }
        if th.inv {
            for v in y[..n].iter_mut() {
                *v = -*v;
            }
        }
        cm
    }
}

/// Stred a bok → ľavý a pravý kanál s normovaním.
fn spoj_stereo(x: &mut [f32], y: &mut [f32], mid: f32, n: usize) {
    let xp: f32 = (0..n).map(|j| y[j] * x[j]).sum::<f32>() * mid;
    let side: f32 = (0..n).map(|j| y[j] * y[j]).sum();
    let el = mid * mid + side - 2.0 * xp;
    let er = mid * mid + side + 2.0 * xp;
    if er < 6e-4 || el < 6e-4 {
        y[..n].copy_from_slice(&x[..n]);
        return;
    }
    let (lz, pz) = (1.0 / el.sqrt(), 1.0 / er.sqrt());
    for j in 0..n {
        let l = mid * x[j];
        let r = y[j];
        x[j] = lz * (l - r);
        y[j] = pz * (l + r);
    }
}

/// Parametre dekódovania všetkých pásiem rámca.
pub(crate) struct ParametrePasiem<'p> {
    pub zaciatok: usize,
    pub koniec: usize,
    pub lm: i32,
    pub kanalov: usize,
    pub kratke: bool,
    pub rozprestretie: usize,
    pub dual: bool,
    pub intenzita: usize,
    pub tf: &'p [i32; NB_PASIEM],
    pub pulzy: &'p [i32; NB_PASIEM],
    pub celkom_bitov: i32,
    pub zostatok: i32,
    pub kodovanych: usize,
}

/// Dekóduje tvar všetkých pásiem (normované spektrum `x` – kanál c od c·N) a masky kolapsu.
pub(crate) fn dekoduj_pasma(
    rd: &mut RozsahovyDekoder,
    p: &ParametrePasiem,
    semienko: &mut u32,
    x: &mut [f32],
    n_ramca: usize,
    masky: &mut [u8; 2 * NB_PASIEM],
) {
    let e = &t::HRANICE_PASIEM;
    let m = 1usize << p.lm;
    let bl = if p.kratke { m } else { 1 };
    let c = p.kanalov;
    let posun_norm = m * e[p.zaciatok] as usize;
    let dlzka_norm = m * e[NB_PASIEM] as usize - posun_norm;
    let mut norm = vec![0.0f32; dlzka_norm];
    let mut norm2 = vec![0.0f32; dlzka_norm];
    let mut ctx = Pasma {
        rd,
        pasmo: 0,
        intenzita: p.intenzita,
        rozprestretie: p.rozprestretie,
        tf_zmena: 0,
        zostava: 0,
        semienko: *semienko,
    };
    let mut zostatok = p.zostatok;
    let mut dual = p.dual;
    let mut nizke_pasmo = 0usize;
    let mut aktualizuj_nizke = true;
    let (xs, ys) = x.split_at_mut(n_ramca);
    for i in p.zaciatok..p.koniec {
        ctx.pasmo = i;
        let tell = ctx.rd.tell_frac();
        let posledne = i == p.koniec - 1;
        let zac = m * e[i] as usize;
        let n = m * e[i + 1] as usize - zac;
        if i != p.zaciatok {
            zostatok -= tell;
        }
        let zostava = p.celkom_bitov - tell - 1;
        ctx.zostava = zostava;
        let b = if i < p.kodovanych {
            let curr = zostatok / (p.kodovanych - i).min(3) as i32;
            (zostava + 1).min(p.pulzy[i] + curr).clamp(0, 16383)
        } else {
            0
        };
        // RFC 8251 „Hybrid Folding“: druhé pásmo môže skladať z prvého
        if (zac as i64 - n as i64 >= posun_norm as i64 || i == p.zaciatok + 1)
            && (aktualizuj_nizke || nizke_pasmo == 0)
        {
            nizke_pasmo = i;
        }
        if i == p.zaciatok + 1 {
            let n1 = m * (e[p.zaciatok + 1] - e[p.zaciatok]) as usize;
            let n2 = m * (e[p.zaciatok + 2] - e[p.zaciatok + 1]) as usize;
            if n2 > n1 {
                norm.copy_within(2 * n1 - n2..n1, n1);
                if dual {
                    norm2.copy_within(2 * n1 - n2..n1, n1);
                }
            }
        }
        ctx.tf_zmena = p.tf[i];
        // odhad masiek kolapsu pásiem, z ktorých sa bude skladať
        let mut efektivne: Option<usize> = None;
        let (mut x_cm, mut y_cm);
        if nizke_pasmo != 0 && (p.rozprestretie != ROZPRESTRETIE_SILNE || bl > 1 || ctx.tf_zmena < 0) {
            let ef = (m * e[nizke_pasmo] as usize).saturating_sub(posun_norm + n);
            efektivne = Some(ef);
            let mut od = nizke_pasmo;
            loop {
                od -= 1;
                if m * e[od] as usize <= ef + posun_norm {
                    break;
                }
            }
            let mut po = nizke_pasmo - 1;
            loop {
                po += 1;
                if !(po < i && m * (e[po] as usize) < ef + posun_norm + n) {
                    break;
                }
            }
            x_cm = 0u32;
            y_cm = 0u32;
            let mut f = od;
            loop {
                x_cm |= masky[f * c] as u32;
                y_cm |= masky[f * c + c - 1] as u32;
                f += 1;
                if f >= po {
                    break;
                }
            }
        } else {
            x_cm = (1u32 << bl) - 1;
            y_cm = x_cm;
        }
        if dual && i == p.intenzita {
            dual = false;
            for j in 0..zac - posun_norm {
                norm[j] = 0.5 * (norm[j] + norm2[j]);
            }
        }
        let o = zac - posun_norm;
        let nizke1 = efektivne.map(|ef| norm[ef..ef + n].to_vec());
        let xb = &mut xs[zac..zac + n];
        let mut von = vec![0.0f32; n];
        if dual {
            let nizke2 = efektivne.map(|ef| norm2[ef..ef + n].to_vec());
            x_cm = ctx.pasmo_mono(
                xb,
                n,
                b / 2,
                bl,
                nizke1.as_deref(),
                p.lm,
                (!posledne).then_some(&mut von[..]),
                1.0,
                x_cm,
            );
            if !posledne {
                norm[o..o + n].copy_from_slice(&von);
            }
            let yb = &mut ys[zac..zac + n];
            y_cm = ctx.pasmo_mono(
                yb,
                n,
                b / 2,
                bl,
                nizke2.as_deref(),
                p.lm,
                (!posledne).then_some(&mut von[..]),
                1.0,
                y_cm,
            );
            if !posledne {
                norm2[o..o + n].copy_from_slice(&von);
            }
        } else {
            if c == 2 {
                let yb = &mut ys[zac..zac + n];
                x_cm = ctx.pasmo_stereo(
                    xb,
                    yb,
                    n,
                    b,
                    bl,
                    nizke1.as_deref(),
                    p.lm,
                    (!posledne).then_some(&mut von[..]),
                    x_cm | y_cm,
                );
            } else {
                x_cm = ctx.pasmo_mono(
                    xb,
                    n,
                    b,
                    bl,
                    nizke1.as_deref(),
                    p.lm,
                    (!posledne).then_some(&mut von[..]),
                    1.0,
                    x_cm | y_cm,
                );
            }
            if !posledne {
                norm[o..o + n].copy_from_slice(&von);
            }
            y_cm = x_cm;
        }
        masky[i * c] = x_cm as u8;
        masky[i * c + c - 1] = y_cm as u8;
        zostatok += p.pulzy[i] + tell;
        aktualizuj_nizke = b > (n as i32) << BITRES;
    }
    *semienko = ctx.semienko;
}

/// Anti-kolaps (RFC 6716 §4.3.5): do krátkych blokov s nulovou energiou šum s energiou podľa dvoch predošlých
/// rámcov.
pub(crate) fn anti_kolaps(
    x: &mut [f32],
    masky: &[u8; 2 * NB_PASIEM],
    lm: i32,
    c: usize,
    n_ramca: usize,
    zaciatok: usize,
    koniec: usize,
    energia: &[f32; 42],
    pred1: &[f32; 42],
    pred2: &[f32; 42],
    pulzy: &[i32; NB_PASIEM],
    semienko: &mut u32,
) {
    let e = &t::HRANICE_PASIEM;
    for i in zaciatok..koniec {
        let n0 = (e[i + 1] - e[i]) as usize;
        let hlbka = (((1 + pulzy[i]) as u32 / n0 as u32) >> lm) as f64;
        let prah = (0.5 * orchidea_jadro::matematika::exp(-0.125 * hlbka * core::f64::consts::LN_2)) as f32;
        let odm = 1.0 / ((n0 << lm) as f32).sqrt();
        for ch in 0..c {
            let mut p1 = pred1[ch * NB_PASIEM + i];
            let mut p2 = pred2[ch * NB_PASIEM + i];
            if c == 1 {
                p1 = p1.max(pred1[NB_PASIEM + i]);
                p2 = p2.max(pred2[NB_PASIEM + i]);
            }
            let ediff = (energia[ch * NB_PASIEM + i] - p1.min(p2)).max(0.0);
            let mut r =
                (2.0 * orchidea_jadro::matematika::exp(-(ediff as f64) * core::f64::consts::LN_2)) as f32;
            if lm == 3 {
                r *= core::f32::consts::SQRT_2;
            }
            r = r.min(prah) * odm;
            let zac = ch * n_ramca + ((e[i] as usize) << lm);
            let blok = &mut x[zac..zac + (n0 << lm)];
            let mut renorm = false;
            for k in 0..1usize << lm {
                if masky[i * c + ch] as u32 & (1 << k) == 0 {
                    for j in 0..n0 {
                        *semienko = lcg(*semienko);
                        blok[(j << lm) + k] = if *semienko & 0x8000 != 0 { r } else { -r };
                    }
                    renorm = true;
                }
            }
            if renorm {
                normuj(blok, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn pocet_kombinacii_pvq() {
        // V(N, K) podľa definície: V(2, K) = 4K, V(3, 1) = 6, V(N, 1) = 2N
        fn v(n: usize, k: usize) -> u64 {
            let mut r = vec![0u64; k + 1];
            r[0] = 1;
            for _ in 0..n {
                let mut p = r[0];
                for kk in 1..=k {
                    let s = r[kk];
                    r[kk] = s + r[kk - 1] + p;
                    p = s;
                }
            }
            r[k]
        }
        assert_eq!(v(2, 5), 20);
        assert_eq!(v(3, 1), 6);
        assert_eq!(v(7, 1), 14);
        assert_eq!(v(3, 2), 18);
    }

    #[test]
    fn pulzy_a_bity() {
        assert_eq!(pulzy(7), 7);
        assert_eq!(pulzy(8), 8);
        assert_eq!(pulzy(15), 15);
        assert_eq!(pulzy(16), 16);
        assert_eq!(pulzy(17), 18);
        assert_eq!(pulzy(40), 128);
        // cache má pre každé pásmo a LM rastúci počet bitov
        for lm in -1..=3 {
            for p in 0..NB_PASIEM {
                if let Some(c) = cache(lm, p) {
                    assert!(c[1..].windows(2).all(|w| w[0] <= w[1]), "lm {lm} pásmo {p}");
                }
            }
        }
    }

    #[test]
    fn presny_kosinus_hranice() {
        assert_eq!(kosinus_presny(1), 32768);
        assert!((kosinus_presny(8192) - 23170).abs() < 40);
        assert!(
            (1..16384)
                .step_by(97)
                .collect::<Vec<i32>>()
                .windows(2)
                .all(|w| kosinus_presny(w[0]) >= kosinus_presny(w[1]))
        );
        // log2(tan) je antisymetrický
        assert_eq!(log2_tan(20000, 20000), 0);
        assert_eq!(log2_tan(10000, 20000), -log2_tan(20000, 10000));
    }
}
