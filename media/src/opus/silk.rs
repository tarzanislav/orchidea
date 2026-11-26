//! **Dekodér SILK** (RFC 6716, §4.2) – vrstva LP v režimoch SILK a hybrid.
//!
//! Dekódovanie parametrov (hlavička, LBRR, stereo váhy, typ rámca, zisky, NLSF a ich stabilizácia, prevod na LPC
//! s obmedzením rozsahu a zisku, výška tónu, filtre LTP, budenie) je **bitovo presné** podľa celočíselných vzťahov
//! normy. Rekonštrukcia (LTP a LPC syntéza, stereo) podľa §4.2.7.9 nemusí byť bitovo presná – počíta sa
//! v plávajúcej čiarke (`f32`, rozsah −1 až 1) v pevnom poradí operácií (rovnaké bity na každej platforme).
//!
//! Spresnenia nejasností textu (overené rozhodcom, záznam v `ORCHIDEA/lyra-media.md`):
//! - rezíduum pre LTP sa drží v **skutočnej mierke** (r = zisk · e), takže zmena zisku medzi podrámcami netreba
//!   prepočítavať; prebielenie výstupu (`rewhitening`) prebieha na začiatku každého hlasového rámca (so škálou
//!   LTP) a v 3. podrámci 20 ms rámca s interpoláciou NLSF (zmena LPC), pre najväčšie oneskorenie rámca,
//! - rámce LBRR sa len rozoberú (posun rozsahového dekodéra) – bez straty paketu sa nesyntetizujú.

// DSP podľa vzorcov normy s indexmi (a[k], lpc[i − k − 1]) – čitateľnejšie ako iterátory
#![allow(clippy::needless_range_loop)]

use super::rozsah::{RozsahovyDekoder, ilog};
use super::silk_tabulky as t;

/// Šírka pásma SILK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasmoSilk {
    Nb,
    Mb,
    Wb,
}

impl PasmoSilk {
    pub fn frekvencia(self) -> u32 {
        match self {
            PasmoSilk::Nb => 8000,
            PasmoSilk::Mb => 12000,
            PasmoSilk::Wb => 16000,
        }
    }
    fn podramec(self) -> usize {
        match self {
            PasmoSilk::Nb => 40,
            PasmoSilk::Mb => 60,
            PasmoSilk::Wb => 80,
        }
    }
    fn rad(self) -> usize {
        if self == PasmoSilk::Wb { 16 } else { 10 }
    }
}

/// Typ signálu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Neaktivny = 0,
    Neznely = 1,
    Znely = 2,
}

/// Rozdekódované parametre jedného rámca SILK.
#[derive(Debug, Clone)]
struct Parametre {
    signal: Signal,
    zisky_q16: [i32; 4],
    /// LPC Q12 pre prvú polovicu (interpolované) a druhú polovicu rámca.
    lpc_q12: [[i32; 16]; 2],
    interpolacia: bool,
    lagy: [i32; 4],
    ltp_q7: [[i32; 5]; 4],
    ltp_skala_q14: i32,
    budenie_q23: Vec<i32>,
}

/// Stav kanála (stredný alebo bočný).
#[derive(Debug, Clone)]
pub struct KanalSilk {
    posledny_log_zisk: Option<i32>,
    nlsf_q15: [i32; 16],
    ma_nlsf: bool,
    /// Primárne oneskorenie predošlého rámca a či bol hlasový (relatívne kódovanie v rámci paketu).
    lag: i32,
    /// Výstup (orezaný) – história pre prebielenie, najviac 306 vzoriek + rámec.
    vystup: Vec<f32>,
    /// Rezíduum v skutočnej mierke – história pre LTP.
    rezidua: Vec<f32>,
    /// Neorezaná história LPC syntézy.
    lpc_hist: [f32; 16],
}

const HISTORIA: usize = 288 + 16 + 2 + 8;

impl Default for KanalSilk {
    fn default() -> Self {
        KanalSilk {
            posledny_log_zisk: None,
            nlsf_q15: [0; 16],
            ma_nlsf: false,
            lag: 0,
            vystup: vec![0.0; HISTORIA],
            rezidua: vec![0.0; HISTORIA],
            lpc_hist: [0.0; 16],
        }
    }
}

impl KanalSilk {
    /// Stav po vynechanom rámci bočného kanála (alebo po resete): LTP a LPC história nulová, bez interpolácie
    /// NLSF a bez obmedzenia zisku.
    fn vynuluj(&mut self) {
        *self = KanalSilk::default();
    }
}

/// Kontext rozboru v rámci jedného paketu pre jeden typ rámcov (LBRR alebo bežné).
#[derive(Debug, Clone, Copy, Default)]
struct Kontext {
    /// Predošlý rámec tohto typu v tomto kanáli v tom istom pakete bol kódovaný.
    predosly_kodovany: bool,
    predosly_znely: bool,
    predosly_lag: i32,
    log_zisk: Option<i32>,
}

/// `silk_log2lin`: približne 2^(x/128).
fn log2lin(x: i32) -> i32 {
    if x < 0 {
        return 0;
    }
    if x >= 3967 {
        return i32::MAX;
    }
    let i = x >> 7;
    let f = (x & 127) as i64;
    let mocnina = 1i64 << i;
    (mocnina + ((((-174 * f * (128 - f)) >> 16) + f) * (mocnina >> 7))) as i32
}

fn znamienko(x: i32) -> i32 {
    x.signum()
}

/// Dekóduje NLSF (§4.2.7.5.1–4) a vráti NLSF_Q15 po stabilizácii.
fn dekoduj_nlsf(rd: &mut RozsahovyDekoder, pasmo: PasmoSilk, znely: bool) -> [i32; 16] {
    let wb = pasmo == PasmoSilk::Wb;
    let d = pasmo.rad();
    let i1 = rd.pdf(&t::NLSF_1[(wb as usize) * 2 + znely as usize]);
    let mut i2 = [0i32; 16];
    for (k, v) in i2.iter_mut().enumerate().take(d) {
        let kniha = if wb { t::NLSF_2_VYBER_WB[i1][k] } else { t::NLSF_2_VYBER_NBMB[i1][k] } as usize;
        let mut x = rd.pdf(&t::NLSF_2[kniha]) as i32 - 4;
        if x == -4 || x == 4 {
            x += znamienko(x) * rd.pdf(&t::NLSF_2_ROZSIRENIE) as i32;
        }
        *v = x;
    }
    // spätná predikcia rezídua
    let qstep: i64 = if wb { 9830 } else { 11796 };
    let mut res_q10 = [0i32; 16];
    for k in (0..d).rev() {
        let pred = if k + 1 < d {
            let q8 = if wb {
                if t::NLSF_PRED_VYBER_WB[i1][k] == 0 { t::NLSF_PRED_C[k] } else { t::NLSF_PRED_D[k] }
            } else if t::NLSF_PRED_VYBER_NBMB[i1][k] == 0 {
                t::NLSF_PRED_A[k]
            } else {
                t::NLSF_PRED_B[k]
            };
            (res_q10[k + 1] * q8) >> 8
        } else {
            0
        };
        let x = i2[k] as i64;
        res_q10[k] = pred + ((((x << 10) - znamienko(i2[k]) as i64 * 102) * qstep) >> 16) as i32;
    }
    // váhy IHMW z kódovej knihy prvého stupňa a rekonštrukcia
    let cb: &[i32] = if wb { &t::NLSF_CB_WB[i1] } else { &t::NLSF_CB_NBMB[i1] };
    let mut nlsf = [0i32; 16];
    for k in 0..d {
        let pred = if k == 0 { 0 } else { cb[k - 1] };
        let po = if k + 1 == d { 256 } else { cb[k + 1] };
        let w2_q18 = ((1024 / (cb[k] - pred).max(1) + 1024 / (po - cb[k]).max(1)) as u32) << 16;
        let i = ilog(w2_q18) as i32;
        let f = ((w2_q18 >> (i - 8)) & 127) as i32;
        let y = (if i & 1 != 0 { 32768 } else { 46214 }) >> ((32 - i) >> 1);
        let w_q9 = y + ((213 * f * y) >> 16);
        nlsf[k] = ((cb[k] << 7) + (res_q10[k] << 14) / w_q9.max(1)).clamp(0, 32767);
    }
    stabilizuj(&mut nlsf[..d], if wb { &t::NLSF_MIN_WB } else { &t::NLSF_MIN_NBMB });
    nlsf
}

/// Stabilizácia NLSF (§4.2.7.5.4) vrátane opravy RFC 8251 (nasýtené sčítanie v 16 bitoch).
fn stabilizuj(n: &mut [i32], min: &[i32]) {
    let d = n.len();
    for _ in 0..20 {
        // najmenšia rezerva medzery
        let mut najmensia = i32::MAX;
        let mut kde = 0;
        for i in 0..=d {
            let pred = if i == 0 { 0 } else { n[i - 1] };
            let po = if i == d { 32768 } else { n[i] };
            let r = po - pred - min[i];
            if r < najmensia {
                najmensia = r;
                kde = i;
            }
        }
        if najmensia >= 0 {
            return;
        }
        if kde == 0 {
            n[0] = min[0];
        } else if kde == d {
            n[d - 1] = 32768 - min[d];
        } else {
            let pol = min[kde] >> 1;
            let min_stred = pol + min[..kde].iter().sum::<i32>();
            let max_stred = 32768 - pol - min[kde + 1..=d].iter().sum::<i32>();
            let stred = ((n[kde - 1] + n[kde] + 1) >> 1).clamp(min_stred, max_stred.max(min_stred));
            n[kde - 1] = stred - pol;
            n[kde] = n[kde - 1] + min[kde];
        }
    }
    // záložný postup
    n.sort_unstable();
    for k in 0..d {
        let pred = if k == 0 { 0 } else { n[k - 1] };
        n[k] = n[k].max((pred + min[k]).min(32767));
    }
    for k in (0..d).rev() {
        let po = if k + 1 == d { 32768 } else { n[k + 1] };
        n[k] = n[k].min(po - min[k + 1]);
    }
}

/// Prevod NLSF Q15 na LPC Q12 (§4.2.7.5.6–8).
fn nlsf_na_lpc(n: &[i32], pasmo: PasmoSilk) -> [i32; 16] {
    let d = n.len();
    let d2 = d / 2;
    let poradie: &[usize] = if pasmo == PasmoSilk::Wb { &t::PORADIE_WB } else { &t::PORADIE_NBMB };
    let mut c_q17 = [0i64; 16];
    for k in 0..d {
        let i = (n[k] >> 8) as usize;
        let f = (n[k] & 255) as i64;
        let c0 = t::COS_Q12[i.min(128)] as i64;
        let c1 = t::COS_Q12[(i + 1).min(128)] as i64;
        c_q17[poradie[k]] = (c0 * 256 + (c1 - c0) * f + 4) >> 3;
    }
    let polynom = |zaciatok: usize| -> Vec<i64> {
        // p[j] pre j = 0..=d2 (posledný riadok rekurzie)
        let mut p = vec![0i64; d2 + 2];
        p[0] = 1 << 16;
        p[1] = -c_q17[zaciatok];
        for k in 1..d2 {
            let c = c_q17[2 * k + zaciatok];
            let stary = p.clone();
            let daj = |j: isize| -> i64 {
                if j < 0 {
                    0
                } else if j as usize > k {
                    // symetria riadku k−1: p[k−1][k+1] = p[k−1][k−1]
                    stary[(2 * k as isize - j) as usize]
                } else {
                    stary[j as usize]
                }
            };
            for j in 0..=(k + 1) {
                let j = j as isize;
                p[j as usize] = daj(j) + daj(j - 2) - ((c * daj(j - 1) + 32768) >> 16);
            }
        }
        p
    };
    let p = polynom(0);
    let q = polynom(1);
    let mut a32 = [0i64; 16];
    for k in 0..d2 {
        let qd = q[k + 1] - q[k];
        let ps = p[k + 1] + p[k];
        a32[k] = -qd - ps;
        a32[d - k - 1] = qd - ps;
    }
    // obmedzenie rozsahu: najviac 10 kôl rozšírenia pásma
    let mut kol = 0;
    while kol < 10 {
        let (k, maxabs) = a32[..d]
            .iter()
            .enumerate()
            .fold((0, 0i64), |m, (k, &v)| if v.abs() > m.1 { (k, v.abs()) } else { m });
        let maxabs_q12 = ((maxabs + 16) >> 5).min(163_838);
        if maxabs_q12 <= 32767 {
            break;
        }
        let sc = 65470 - ((maxabs_q12 - 32767) << 14) / ((maxabs_q12 * (k as i64 + 1)) >> 2);
        rozsir(&mut a32[..d], sc);
        kol += 1;
    }
    if kol == 10 {
        for v in a32[..d].iter_mut() {
            *v = (((*v + 16) >> 5).clamp(-32768, 32767)) << 5;
        }
    }
    // obmedzenie zisku: najviac 18 kôl
    let mut a_q12 = [0i32; 16];
    for i in 1..=18 {
        for k in 0..d {
            a_q12[k] = ((a32[k] + 16) >> 5) as i32;
        }
        if stabilny(&a_q12[..d]) {
            break;
        }
        rozsir(&mut a32[..d], 65536 - (2 << i));
    }
    a_q12
}

/// Rozšírenie pásma (`silk_bwexpander_32`): a[k] = a[k]·sc_k ≫ 16, sc_{k+1} = (sc_0·sc_k + 32768) ≫ 16.
fn rozsir(a: &mut [i64], sc0: i64) {
    let mut sc = sc0;
    for v in a.iter_mut() {
        *v = (*v * sc) >> 16;
        sc = (sc0 * sc + 32768) >> 16;
    }
}

/// Kontrola stability filtra (§4.2.7.5.8) – Levinsonova rekurzia v pevnej čiarke (RFC 8251: hodnoty mimo 32 bitov
/// znamenajú nestabilný filter). **Odvodené meraním** (text RFC 6716 to neuvádza): filter so ziskom predikcie
/// (výkonu) nad 10⁴ – inverzný zisk Π(1 − rc²) v Q30 pod 107 374 – je tiež nestabilný a dostane ďalšie kolo
/// rozšírenia pásma; prah je ostré optimum zhody s libopus 1.4 (polovičný aj dvojnásobný dávajú horšiu zhodu).
fn stabilny(a_q12: &[i32]) -> bool {
    let dc: i32 = a_q12.iter().sum();
    if dc > 4096 {
        return false;
    }
    let d = a_q12.len();
    let mut a: Vec<i64> = a_q12.iter().map(|&x| (x as i64) << 12).collect();
    let mut inv_zisk_q30: i64 = 1 << 30;
    for k in (0..d).rev() {
        if a[k].abs() > 16_773_022 {
            return false;
        }
        let rc_q31 = -a[k] << 7;
        let div_q30 = (1i64 << 30) - ((rc_q31 * rc_q31) >> 32);
        inv_zisk_q30 = ((inv_zisk_q30 * div_q30) >> 32) << 2;
        if inv_zisk_q30 < 107_374 {
            return false;
        }
        let b1 = ilog(div_q30 as u32) as i64;
        let b2 = b1 - 16;
        let inv_qb2 = ((1i64 << 29) - 1) / (div_q30 >> (b2 + 1));
        let err_q29 = (1i64 << 29) - (((div_q30 << (15 - b2)) * inv_qb2) >> 16);
        let gain_qb1 = (inv_qb2 << 16) + ((err_q29 * inv_qb2) >> 13);
        let stary = a.clone();
        for n in 0..k {
            let num = (stary[n] - ((stary[k - n - 1] * rc_q31 + (1 << 30)) >> 31))
                .clamp(i32::MIN as i64, i32::MAX as i64);
            let nove = (num * gain_qb1 + (1i64 << (b1 - 1))) >> b1;
            if nove > i32::MAX as i64 || nove < i32::MIN as i64 {
                return false;
            }
            a[n] = nove;
        }
    }
    true
}

/// Dekodér SILK pre jeden prúd (mono alebo stereo).
#[derive(Debug, Clone)]
pub struct DekoderSilk {
    kanaly: [KanalSilk; 2],
    /// Predošlé stereo váhy (w0, w1) v Q13.
    vahy_q13: [i32; 2],
    /// História stereo: mid[−2], mid[−1], side[−1].
    stereo_hist: [f32; 3],
    bol_stereo: bool,
    pasmo: Option<PasmoSilk>,
}

impl Default for DekoderSilk {
    fn default() -> Self {
        DekoderSilk {
            kanaly: [KanalSilk::default(), KanalSilk::default()],
            vahy_q13: [0; 2],
            stereo_hist: [0.0; 3],
            bol_stereo: false,
            pasmo: None,
        }
    }
}

/// Výstup rámca Opus z vrstvy SILK: vzorky pri vnútornej frekvencii, kanály samostatne.
#[derive(Debug, Clone, Default)]
pub struct VystupSilk {
    pub frekvencia: u32,
    pub kanaly: Vec<Vec<f32>>,
}

impl DekoderSilk {
    pub fn novy() -> DekoderSilk {
        DekoderSilk::default()
    }

    /// Reset stavu (§4.5.2).
    pub fn reset(&mut self) {
        *self = DekoderSilk::default();
    }

    /// Dekóduje vrstvu SILK jedného rámca Opus (`trvanie_ms` 10, 20, 40 alebo 60).
    pub fn dekoduj(
        &mut self,
        rd: &mut RozsahovyDekoder,
        pasmo: PasmoSilk,
        trvanie_ms: u32,
        stereo: bool,
    ) -> VystupSilk {
        if self.pasmo != Some(pasmo) {
            // zmena vnútornej frekvencie: história nemá zmysel (§4.5.1 – prechody so zmenou pásma SILK resetujú)
            self.reset();
            self.pasmo = Some(pasmo);
        }
        if stereo && !self.bol_stereo {
            self.vahy_q13 = [0; 2];
            self.kanaly[1].vynuluj();
            self.stereo_hist = [0.0; 3];
        }
        self.bol_stereo = stereo;
        let ramcov = if trvanie_ms <= 20 { 1 } else { (trvanie_ms / 20) as usize };
        let ms_ramca = if trvanie_ms == 10 { 10 } else { 20 };
        let kanalov = 1 + stereo as usize;
        // hlavička: VAD a príznak LBRR pre každý kanál
        let mut vad = [[false; 3]; 2];
        let mut lbrr = [[false; 3]; 2];
        for c in 0..kanalov {
            for v in vad[c].iter_mut().take(ramcov) {
                *v = rd.pdf(&[1, 1]) == 1;
            }
            lbrr[c][0] = rd.pdf(&[1, 1]) == 1;
        }
        for c in 0..kanalov {
            if lbrr[c][0] && ramcov > 1 {
                let priznaky = if ramcov == 2 { rd.pdf(&t::LBRR_40) } else { rd.pdf(&t::LBRR_60) };
                for (i, l) in lbrr[c].iter_mut().enumerate().take(ramcov) {
                    *l = priznaky >> i & 1 != 0;
                }
            }
        }
        // rámce LBRR: len rozbor
        let mut ctx_lbrr = [Kontext::default(); 2];
        for i in 0..ramcov {
            for c in 0..kanalov {
                if !lbrr[c][i] {
                    ctx_lbrr[c].predosly_kodovany = false;
                    continue;
                }
                if c == 0 && stereo {
                    let _ = dekoduj_vahy(rd);
                    if !lbrr[1][i] {
                        let _ = rd.pdf(&t::LEN_STRED);
                    }
                }
                let mut docasny = KanalSilk::default();
                let nezavisly = i == 0 || !ctx_lbrr[c].predosly_kodovany;
                let _ =
                    rozober(rd, pasmo, ms_ramca, true, true, nezavisly, &mut ctx_lbrr[c], &mut docasny, i);
                ctx_lbrr[c].predosly_kodovany = true;
            }
        }
        // bežné rámce
        let n = pasmo.podramec() * ms_ramca as usize / 5;
        let mut vystup = vec![Vec::with_capacity(n * ramcov); kanalov];
        let mut ctx = [Kontext::default(); 2];
        for (c, k) in ctx.iter_mut().enumerate() {
            k.log_zisk = self.kanaly[c].posledny_log_zisk;
            k.predosly_lag = self.kanaly[c].lag;
        }
        for i in 0..ramcov {
            let mut len_stred = false;
            let mut vahy = self.vahy_q13;
            let mut stred = Vec::new();
            for c in 0..kanalov {
                if c == 0 && stereo {
                    vahy = dekoduj_vahy(rd);
                    if !vad[1][i] {
                        len_stred = rd.pdf(&t::LEN_STRED) == 1;
                    }
                }
                if c == 1 && len_stred {
                    // bočný kanál nekódovaný: nuly, stav kanála sa vynuluje
                    self.kanaly[1].vynuluj();
                    ctx[1] = Kontext::default();
                    continue;
                }
                let aktivny = vad[c][i];
                let nezavisly = i == 0 || !ctx[c].predosly_kodovany;
                let par = rozober(
                    rd,
                    pasmo,
                    ms_ramca,
                    false,
                    aktivny,
                    nezavisly,
                    &mut ctx[c],
                    &mut self.kanaly[c],
                    i,
                );
                ctx[c].predosly_kodovany = true;
                let vzorky = self.kanaly[c].syntetizuj(&par, pasmo, ms_ramca);
                if c == 0 {
                    stred = vzorky;
                } else {
                    vystup[1].extend_from_slice(&vzorky);
                }
            }
            if stereo {
                let bocny = if len_stred {
                    vec![0.0; stred.len()]
                } else {
                    {
                        let od = vystup[1].len() - stred.len();
                        vystup[1].split_off(od)
                    }
                };
                let (l, r) = self.odmiesaj(&stred, &bocny, vahy, pasmo);
                self.vahy_q13 = vahy;
                vystup[0].extend_from_slice(&l);
                vystup[1].extend_from_slice(&r);
            } else {
                // mono: rovnaké oneskorenie o jednu vzorku ako stereo
                let mut o = Vec::with_capacity(stred.len());
                for &x in &stred {
                    o.push(self.stereo_hist[1]);
                    self.stereo_hist[0] = self.stereo_hist[1];
                    self.stereo_hist[1] = x;
                }
                vystup[0].extend_from_slice(&o);
            }
        }
        for c in 0..kanalov {
            self.kanaly[c].posledny_log_zisk = ctx[c].log_zisk;
            self.kanaly[c].lag = ctx[c].predosly_lag;
        }
        VystupSilk { frekvencia: pasmo.frekvencia(), kanaly: vystup }
    }

    /// Stereo z reprezentácie stred–bok (§4.2.8).
    fn odmiesaj(
        &mut self,
        stred: &[f32],
        bok: &[f32],
        vahy: [i32; 2],
        pasmo: PasmoSilk,
    ) -> (Vec<f32>, Vec<f32>) {
        let n1 = match pasmo {
            PasmoSilk::Nb => 64,
            PasmoSilk::Mb => 96,
            PasmoSilk::Wb => 128,
        };
        let [p0, p1] = self.vahy_q13;
        let (mut m2, mut m1, mut s1) = (self.stereo_hist[0], self.stereo_hist[1], self.stereo_hist[2]);
        let mut l = Vec::with_capacity(stred.len());
        let mut r = Vec::with_capacity(stred.len());
        for i in 0..stred.len() {
            let t = i.min(n1) as f32 / n1 as f32;
            let w0 = (p0 as f32 + t * (vahy[0] - p0) as f32) / 8192.0;
            let w1 = (p1 as f32 + t * (vahy[1] - p1) as f32) / 8192.0;
            let m = stred[i];
            let p = (m2 + 2.0 * m1 + m) / 4.0;
            l.push(((1.0 + w1) * m1 + s1 + w0 * p).clamp(-1.0, 1.0));
            r.push(((1.0 - w1) * m1 - s1 - w0 * p).clamp(-1.0, 1.0));
            m2 = m1;
            m1 = m;
            s1 = bok[i];
        }
        self.stereo_hist = [m2, m1, s1];
        (l, r)
    }
}

/// Stereo váhy predikcie (§4.2.7.1).
fn dekoduj_vahy(rd: &mut RozsahovyDekoder) -> [i32; 2] {
    let n = rd.pdf(&t::STEREO_1) as i32;
    let i0 = rd.pdf(&t::STEREO_2) as i32;
    let i1 = rd.pdf(&t::STEREO_3) as i32;
    let i2 = rd.pdf(&t::STEREO_2) as i32;
    let i3 = rd.pdf(&t::STEREO_3) as i32;
    let wi0 = (i0 + 3 * (n / 5)) as usize;
    let wi1 = (i2 + 3 * (n % 5)) as usize;
    let w = &t::STEREO_VAHY_Q13;
    let w1 = w[wi1] + (((w[wi1 + 1] - w[wi1]) * 6554) >> 16) * (2 * i3 + 1);
    let w0 = w[wi0] + (((w[wi0 + 1] - w[wi0]) * 6554) >> 16) * (2 * i1 + 1) - w1;
    [w0, w1]
}

/// Rozbor parametrov jedného rámca SILK (§4.2.7.3–4.2.7.8).
#[allow(clippy::too_many_arguments)]
fn rozober(
    rd: &mut RozsahovyDekoder,
    pasmo: PasmoSilk,
    ms: u32,
    lbrr: bool,
    aktivny: bool,
    nezavisly: bool,
    k: &mut Kontext,
    kanal: &mut KanalSilk,
    index: usize,
) -> Parametre {
    let podramcov = (ms / 5) as usize;
    // typ rámca
    let typ = if aktivny { rd.pdf(&t::TYP_AKTIVNY) } else { rd.pdf(&t::TYP_NEAKTIVNY) };
    let signal = match typ / 2 {
        0 => Signal::Neaktivny,
        1 => Signal::Neznely,
        _ => Signal::Znely,
    };
    let vysoky_posun = typ & 1 == 1;
    // zisky
    let mut zisky_q16 = [0i32; 4];
    for (s, z) in zisky_q16.iter_mut().enumerate().take(podramcov) {
        let log = if s == 0 && nezavisly {
            let msb = rd.pdf(&t::ZISK_MSB[signal as usize]) as i32;
            let lsb = rd.pdf(&t::ZISK_LSB) as i32;
            let idx = msb * 8 + lsb;
            match k.log_zisk {
                Some(p) => idx.max(p - 16),
                None => idx,
            }
        } else {
            let d = rd.pdf(&t::ZISK_DELTA) as i32;
            let p = k.log_zisk.unwrap_or(0);
            (2 * d - 16).max(p + d - 4).clamp(0, 63)
        };
        k.log_zisk = Some(log);
        *z = log2lin(((0x1D1C71i64 * log as i64) >> 16) as i32 + 2090);
    }
    // NLSF a interpolácia
    let d = pasmo.rad();
    let n2 = dekoduj_nlsf(rd, pasmo, signal == Signal::Znely);
    let mut w_q2 = 4;
    if ms == 20 {
        w_q2 = rd.pdf(&t::NLSF_INTERP) as i32;
        if !kanal.ma_nlsf {
            w_q2 = 4;
        }
    }
    let mut lpc_q12 = [[0i32; 16]; 2];
    let druha = nlsf_na_lpc(&n2[..d], pasmo);
    lpc_q12[1] = druha;
    let interpolacia = w_q2 < 4;
    if interpolacia {
        let mut n1 = [0i32; 16];
        for i in 0..d {
            n1[i] = kanal.nlsf_q15[i] + ((w_q2 * (n2[i] - kanal.nlsf_q15[i])) >> 2);
        }
        lpc_q12[0] = nlsf_na_lpc(&n1[..d], pasmo);
    } else {
        lpc_q12[0] = druha;
    }
    if !lbrr {
        kanal.nlsf_q15 = n2;
        kanal.ma_nlsf = true;
    }
    // LTP
    let mut lagy = [0i32; 4];
    let mut ltp_q7 = [[0i32; 5]; 4];
    let mut ltp_skala_q14 = 15565;
    let (lag_min, lag_max, skala, nizka) = match pasmo {
        PasmoSilk::Nb => (16, 144, 4, 0),
        PasmoSilk::Mb => (24, 216, 6, 1),
        PasmoSilk::Wb => (32, 288, 8, 2),
    };
    if signal == Signal::Znely {
        let absolutne = |rd: &mut RozsahovyDekoder| {
            let h = rd.pdf(&t::LAG_VYSOKA) as i32;
            let l = rd.pdf(t::LAG_NIZKA[nizka]) as i32;
            h * skala + l + lag_min
        };
        let lag = if nezavisly || !k.predosly_znely {
            absolutne(rd)
        } else {
            let dl = rd.pdf(&t::LAG_DELTA) as i32;
            if dl == 0 { absolutne(rd) } else { k.predosly_lag + dl - 9 }
        };
        k.predosly_lag = lag;
        let kniha = match (pasmo, ms) {
            (PasmoSilk::Nb, 10) => 0,
            (PasmoSilk::Nb, _) => 1,
            (_, 10) => 2,
            _ => 3,
        };
        let ci = rd.pdf(t::KONTURA[kniha]);
        for (s, l) in lagy.iter_mut().enumerate().take(podramcov) {
            let o = match kniha {
                0 => t::KONTURA_NB_10[ci][s],
                1 => t::KONTURA_NB_20[ci][s],
                2 => t::KONTURA_10[ci][s],
                _ => t::KONTURA_20[ci][s],
            };
            *l = (lag + o).clamp(lag_min, lag_max);
        }
        let per = rd.pdf(&t::PERIODICITA);
        for f in ltp_q7.iter_mut().take(podramcov) {
            let i = rd.pdf(t::LTP_FILTER[per]);
            *f = match per {
                0 => t::LTP_TAPY_0[i],
                1 => t::LTP_TAPY_1[i],
                _ => t::LTP_TAPY_2[i],
            };
        }
        if index == 0 || (lbrr && nezavisly) {
            ltp_skala_q14 = [15565, 12288, 8192][rd.pdf(&t::LTP_SKALA)];
        }
    }
    k.predosly_znely = signal == Signal::Znely;
    // semienko LCG a budenie
    let mut semienko = rd.pdf(&t::SEMIENKO) as u32;
    let blokov = match (pasmo, ms) {
        (PasmoSilk::Nb, 10) => 5,
        (PasmoSilk::Mb, 10) => 8,
        (PasmoSilk::Wb, 10) => 10,
        (PasmoSilk::Nb, _) => 10,
        (PasmoSilk::Mb, _) => 15,
        (PasmoSilk::Wb, _) => 20,
    };
    let uroven = rd.pdf(&t::UROVEN[(signal == Signal::Znely) as usize]);
    let mut pulzov = vec![0usize; blokov];
    let mut lsb = vec![0u32; blokov];
    for b in 0..blokov {
        let mut c = rd.pdf(&t::PULZY[uroven]);
        while c == 17 {
            lsb[b] += 1;
            c = rd.pdf(&t::PULZY[if lsb[b] >= 10 { 10 } else { 9 }]);
        }
        pulzov[b] = c;
    }
    let mut e = vec![0i32; blokov * 16];
    for b in 0..blokov {
        if pulzov[b] > 0 {
            polohy(rd, &mut e[b * 16..b * 16 + 16], pulzov[b] as i32);
        }
    }
    for b in 0..blokov {
        for x in e[b * 16..b * 16 + 16].iter_mut() {
            for _ in 0..lsb[b] {
                *x = *x * 2 + rd.pdf(&t::LSB) as i32;
            }
        }
    }
    for b in 0..blokov {
        let tab = &t::ZNAMIENKO[(signal as usize * 2 + vysoky_posun as usize) * 7 + pulzov[b].min(6)];
        for x in e[b * 16..b * 16 + 16].iter_mut() {
            if *x != 0 && rd.pdf(tab) == 0 {
                *x = -*x;
            }
        }
    }
    let posun = match (signal, vysoky_posun) {
        (Signal::Znely, false) => 8,
        (Signal::Znely, true) => 25,
        (_, false) => 25,
        (_, true) => 60,
    };
    let vzoriek = pasmo.podramec() * podramcov;
    let mut budenie_q23 = Vec::with_capacity(vzoriek);
    for &raw in e.iter().take(vzoriek) {
        let mut v = (raw << 8) - znamienko(raw) * 20 + posun;
        semienko = semienko.wrapping_mul(196_314_165).wrapping_add(907_633_515);
        if semienko & 0x8000_0000 != 0 {
            v = -v;
        }
        semienko = semienko.wrapping_add(raw as u32);
        budenie_q23.push(v);
    }
    Parametre { signal, zisky_q16, lpc_q12, interpolacia, lagy, ltp_q7, ltp_skala_q14, budenie_q23 }
}

/// Polohy pulzov v bloku (§4.2.7.8.3): rekurzívne delenie na polovice, najprv ľavá.
fn polohy(rd: &mut RozsahovyDekoder, blok: &mut [i32], pulzov: i32) {
    if pulzov == 0 {
        return;
    }
    let n = blok.len();
    if n == 1 {
        blok[0] = pulzov;
        return;
    }
    let tab: &[u16] = match n {
        16 => t::SHELL_16[pulzov as usize - 1],
        8 => t::SHELL_8[pulzov as usize - 1],
        4 => t::SHELL_4[pulzov as usize - 1],
        _ => t::SHELL_2[pulzov as usize - 1],
    };
    let vlavo = rd.pdf(tab) as i32;
    let (l, p) = blok.split_at_mut(n / 2);
    polohy(rd, l, vlavo);
    polohy(rd, p, pulzov - vlavo);
}

impl KanalSilk {
    /// Rekonštrukcia rámca (§4.2.7.9): LTP a LPC syntéza; vráti orezaný výstup.
    fn syntetizuj(&mut self, p: &Parametre, pasmo: PasmoSilk, ms: u32) -> Vec<f32> {
        let n = pasmo.podramec();
        let d = pasmo.rad();
        let podramcov = (ms / 5) as usize;
        let h = self.vystup.len();
        self.vystup.resize(h + n * podramcov, 0.0);
        self.rezidua.resize(h + n * podramcov, 0.0);
        let znely = p.signal == Signal::Znely;
        let max_lag = p.lagy[..podramcov].iter().copied().max().unwrap_or(0) as usize;
        for s in 0..podramcov {
            let j = h + s * n;
            let a = if s < 2 { &p.lpc_q12[0] } else { &p.lpc_q12[1] };
            let zisk = p.zisky_q16[s] as f32 / 65536.0;
            if znely && (s == 0 || (s == 2 && p.interpolacia)) {
                // prebielenie výstupu: rezíduum histórie s LPC aktuálneho podrámca (skutočná mierka)
                let skala = if s == 0 { p.ltp_skala_q14 as f32 / 16384.0 } else { 1.0 };
                let od = j.saturating_sub(max_lag + 2).max(d);
                for i in od..j {
                    let mut x = self.vystup[i];
                    for k in 0..d {
                        x -= self.vystup[i - k - 1] * a[k] as f32 / 4096.0;
                    }
                    self.rezidua[i] = skala * x.clamp(-1.0, 1.0);
                }
            }
            for i in j..j + n {
                let e = p.budenie_q23[i - h] as f32 / 8_388_608.0 * zisk;
                let mut r = e;
                if znely {
                    let lag = p.lagy[s] as usize;
                    for (k, &b) in p.ltp_q7[s].iter().enumerate() {
                        let idx = (i + 2).saturating_sub(lag + k);
                        r += self.rezidua[idx] * b as f32 / 128.0;
                    }
                }
                self.rezidua[i] = r;
                let mut x = r;
                for k in 0..d {
                    x += self.lpc_hist[k] * a[k] as f32 / 4096.0;
                }
                self.lpc_hist.copy_within(0..15, 1);
                self.lpc_hist[0] = x;
                self.vystup[i] = x.clamp(-1.0, 1.0);
            }
        }
        let vystup = self.vystup[h..].to_vec();
        // ponechá sa len história
        let zvysok = self.vystup.len() - HISTORIA;
        self.vystup.drain(..zvysok);
        self.rezidua.drain(..zvysok);
        vystup
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn pdf_sucty() {
        let mut vsetky: Vec<&[u16]> =
            vec![&t::LBRR_40, &t::LBRR_60, &t::STEREO_1, &t::STEREO_2, &t::STEREO_3, &t::LEN_STRED];
        vsetky.extend([
            &t::TYP_AKTIVNY[..],
            &t::TYP_NEAKTIVNY,
            &t::ZISK_LSB,
            &t::ZISK_DELTA,
            &t::NLSF_2_ROZSIRENIE,
        ]);
        vsetky.extend(t::ZISK_MSB.iter().map(|x| &x[..]));
        vsetky.extend(t::NLSF_1.iter().map(|x| &x[..]));
        vsetky.extend(t::NLSF_2.iter().map(|x| &x[..]));
        vsetky.extend([
            &t::NLSF_INTERP[..],
            &t::LAG_VYSOKA,
            &t::LAG_DELTA,
            &t::PERIODICITA,
            &t::LTP_SKALA,
            &t::SEMIENKO,
            &t::LSB,
        ]);
        vsetky.extend(t::LAG_NIZKA.iter().copied());
        vsetky.extend(t::KONTURA.iter().copied());
        vsetky.extend(t::LTP_FILTER.iter().copied());
        vsetky.extend(t::UROVEN.iter().map(|x| &x[..]));
        vsetky.extend(t::PULZY.iter().map(|x| &x[..]));
        for s in [&t::SHELL_16, &t::SHELL_8, &t::SHELL_4, &t::SHELL_2] {
            vsetky.extend(s.iter().copied());
        }
        vsetky.extend(t::ZNAMIENKO.iter().map(|x| &x[..]));
        for p in vsetky {
            assert_eq!(p.iter().map(|&x| x as u32).sum::<u32>(), 256, "{p:?}");
        }
    }

    #[test]
    fn log2lin_hranice() {
        // zisk Q16: najmenší 81 920 (log_gain 0), najväčší 1 686 110 208 (log_gain 63)
        let q = |log: i64| log2lin(((0x1D1C71i64 * log) >> 16) as i32 + 2090);
        assert_eq!(q(0), 81920);
        assert_eq!(q(63), 1_686_110_208);
    }

    #[test]
    fn nlsf_na_lpc_stabilny_vystup() {
        // rovnomerne rozložené NLSF → stabilný filter s malými koeficientmi
        for pasmo in [PasmoSilk::Nb, PasmoSilk::Wb] {
            let d = pasmo.rad();
            let n: Vec<i32> = (0..d).map(|k| ((k as i32 + 1) * 32768) / (d as i32 + 1)).collect();
            let a = nlsf_na_lpc(&n, pasmo);
            assert!(stabilny(&a[..d]));
            assert!(a[..d].iter().all(|x| x.abs() < 4096), "{a:?}");
        }
    }

    #[test]
    fn stabilizacia() {
        let mut n = [100, 50, 50, 20000, 20001, 20002, 32000, 32700, 32760, 32767];
        stabilizuj(&mut n, &t::NLSF_MIN_NBMB);
        let mut pred = 0;
        for (k, &x) in n.iter().enumerate() {
            assert!(x - pred >= t::NLSF_MIN_NBMB[k], "{n:?}");
            pred = x;
        }
        assert!(32768 - pred >= t::NLSF_MIN_NBMB[10]);
    }

    #[test]
    fn nahodne_ramce_nikdy_pad() {
        let mut x = 99u32;
        for _ in 0..300 {
            let data: Vec<u8> = (0..60)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let mut d = DekoderSilk::novy();
            for (pasmo, ms, st) in [
                (PasmoSilk::Nb, 10, false),
                (PasmoSilk::Mb, 20, true),
                (PasmoSilk::Wb, 60, true),
                (PasmoSilk::Wb, 40, false),
            ] {
                let mut rd = RozsahovyDekoder::novy(&data);
                let v = d.dekoduj(&mut rd, pasmo, ms, st);
                let ocak = pasmo.frekvencia() as usize * ms as usize / 1000;
                assert!(
                    v.kanaly
                        .iter()
                        .all(|k| k.len() == ocak && k.iter().all(|s| s.is_finite() && s.abs() <= 1.0))
                );
            }
        }
    }
}
