//! Spätná kvantizácia, spracovanie sterea a preradenie krátkych blokov vrstvy III MPEG-1.
//!
//! Zdroje: [S] §4.3.9 (konštanta 210, výstup v rozsahu ±1,0, pretab – tabuľka 23, scalefac_multiplier – tabuľka 20),
//! CD 11172-3 2.4.3.4 (tvar vzorca spätnej kvantizácie, intenzitné stereo `is_ratio = tan(is_pos·π/12)`),
//! [S] §4.3.10 (preradenie), [S] §4.3.11 (pravidlá M/S a intenzitného sterea, hranica podľa pravého kanála,
//! pri krátkych blokoch po oknách, neplatná poloha 7).
//!
//! **Rozpory textov rozhodnuté meraním** (ISO/IEC 11172-4 proti rozhodcovi, `over_mpa.py`):
//! - systémová konštanta zisku: CD uvádza 64, [S] 210 – platí 210 (výstup ±1,0);
//! - normovanie M/S: CD uvádza (M ± S)/2, vzorec v [S] chýba – platí (M ± S)/√2.
//!
//! **Odvodené (text nehovorí):** intenzitná poloha posledného pásma (dlhé 21, krátke 12 – bez škálovacieho faktora)
//! sa preberá z predošlého pásma (20, 11).

use super::tabulky;
use super::{Granula, Skalovacie};
use orchidea_jadro::matematika::{cos, exp, mocnina, sin};
use std::f64::consts::{LN_2, PI};
use std::sync::OnceLock;

/// Najväčšia hodnota |is|: 15 + (2¹³ − 1) pri linbits 13.
const NAJVIAC_IS: usize = 15 + 8191;

struct Tabulky {
    /// |is|^(4/3)
    mocnina43: Vec<f32>,
    /// 2^(q/4) pre q = 0..3
    stvrtiny: [f64; 4],
}

fn tab() -> &'static Tabulky {
    static T: OnceLock<Tabulky> = OnceLock::new();
    T.get_or_init(|| Tabulky {
        mocnina43: (0..=NAJVIAC_IS).map(|n| mocnina(n as f64, 4.0 / 3.0) as f32).collect(),
        stvrtiny: [1.0, exp(0.25 * LN_2), exp(0.5 * LN_2), exp(0.75 * LN_2)],
    })
}

/// 2^(q/4) presne: celá mocnina dvoch krát jedna zo štyroch konštánt.
fn dva_na_stvrtinu(q: i32) -> f32 {
    let t = tab();
    let (cela, zvysok) = (q.div_euclid(4), q.rem_euclid(4) as usize);
    (t.stvrtiny[zvysok] * f64::powi(2.0, cela)) as f32
}

/// Spätná kvantizácia granuly (poradie čiar ako v bitovom toku).
///
/// `xr = sign(is)·|is|^(4/3) · 2^(¼(global_gain − 210 − 8·subblock_gain[w])) · 2^(−m·(scalefac + preflag·pretab))`,
/// m = 0,5 (scalefac_scale 0) alebo 1 ([S] tabuľka 20); exponent v štvrtinách je celé číslo.
pub(crate) fn kvantizacia(
    is: &[i32; 576],
    g: &Granula,
    sf: &Skalovacie,
    index_f: usize,
    xr: &mut [f32; 576],
) {
    let t = tab();
    let dlhe = &tabulky::PASMA_DLHE[index_f];
    let kratke = &tabulky::PASMA_KRATKE[index_f];
    let m4 = if g.scalefac_scale { 4 } else { 2 }; // 4·m
    let zaklad = g.global_gain as i32 - 210;
    let hodnota = |v: i32, q: i32| -> f32 {
        let a = t.mocnina43[(v.unsigned_abs() as usize).min(NAJVIAC_IS)] * dva_na_stvrtinu(q);
        if v < 0 { -a } else { a }
    };
    let kratke_bloky = g.prepinanie && g.typ_bloku == 2;
    // dlhá časť: celé spektrum, pri zmiešanom bloku pásma 0–7 (čiary 0–35)
    let dlhych_pasiem = if !kratke_bloky {
        22
    } else if g.zmiesany {
        8
    } else {
        0
    };
    for sfb in 0..dlhych_pasiem {
        let pre = if g.preflag { tabulky::PRETAB[sfb] as i32 } else { 0 };
        let q = zaklad - m4 * (sf.l[sfb] as i32 + pre);
        for l in dlhe[sfb] as usize..dlhe[sfb + 1] as usize {
            xr[l] = if is[l] == 0 { 0.0 } else { hodnota(is[l], q) };
        }
    }
    if kratke_bloky {
        // krátka časť: pásmo po pásme, v ňom okno po okne ([S] §4.3.10)
        let od = if g.zmiesany { 3 } else { 0 };
        for sfb in od..13 {
            let (zac, sirka) = (kratke[sfb] as usize, (kratke[sfb + 1] - kratke[sfb]) as usize);
            for w in 0..3 {
                let q = zaklad - 8 * g.subblock_gain[w] as i32 - m4 * sf.s[sfb][w] as i32;
                let z = 3 * zac + w * sirka;
                for l in z..z + sirka {
                    xr[l] = if is[l] == 0 { 0.0 } else { hodnota(is[l], q) };
                }
            }
        }
    }
}

/// Úseky čiar granuly ako (pásmo, okno, začiatok, koniec) v poradí bitového toku; dlhé pásma majú okno `None`.
fn useky(g: &Granula, index_f: usize) -> Vec<(usize, Option<usize>, usize, usize)> {
    let dlhe = &tabulky::PASMA_DLHE[index_f];
    let kratke = &tabulky::PASMA_KRATKE[index_f];
    let mut v = Vec::with_capacity(40);
    let kratke_bloky = g.prepinanie && g.typ_bloku == 2;
    let dlhych = if !kratke_bloky {
        22
    } else if g.zmiesany {
        8
    } else {
        0
    };
    for sfb in 0..dlhych {
        v.push((sfb, None, dlhe[sfb] as usize, dlhe[sfb + 1] as usize));
    }
    if kratke_bloky {
        for sfb in if g.zmiesany { 3 } else { 0 }..13 {
            let (zac, sirka) = (kratke[sfb] as usize, (kratke[sfb + 1] - kratke[sfb]) as usize);
            for w in 0..3 {
                v.push((sfb, Some(w), 3 * zac + w * sirka, 3 * zac + (w + 1) * sirka));
            }
        }
    }
    v
}

/// Spracovanie sterea MPEG-1 ([S] §4.3.11): M/S a intenzitné stereo podľa `mode_extension`.
/// `is_p` sú kvantované hodnoty pravého kanála (hranica intenzity), `sf_p` jeho škálovacie faktory (polohy).
/// Vráti počet úsekov (pásmo, pri krátkych blokoch pásmo a okno) dekódovaných intenzitne.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stereo(
    l: &mut [f32; 576],
    p: &mut [f32; 576],
    is_p: &[i32; 576],
    g: &Granula,
    sf_p: &Skalovacie,
    index_f: usize,
    ms: bool,
    intenzita: bool,
) -> usize {
    let useky = useky(g, index_f);
    // intenzitné úseky: nad posledným pásmom pravého kanála, ktoré nie je celé nulové; pri krátkych blokoch
    // po oknách; pri zmiešanom bloku dlhá časť len vtedy, ak je celá krátka časť pravého kanála nulová
    let mut intenzitny = vec![false; useky.len()];
    if intenzita {
        let nenulovy =
            |&(_, _, a, b): &(usize, Option<usize>, usize, usize)| is_p[a..b].iter().any(|&v| v != 0);
        let mut posledny_dlhy: Option<usize> = None;
        let mut posledny_kratky: [Option<usize>; 3] = [None; 3];
        let mut kratke_nenulove = false;
        for (i, u) in useky.iter().enumerate() {
            if nenulovy(u) {
                match u.1 {
                    None => posledny_dlhy = Some(i),
                    Some(w) => {
                        posledny_kratky[w] = Some(i);
                        kratke_nenulove = true;
                    }
                }
            }
        }
        for (i, u) in useky.iter().enumerate() {
            intenzitny[i] = match u.1 {
                None => !kratke_nenulove && posledny_dlhy.is_none_or(|p| i > p),
                Some(w) => posledny_kratky[w].is_none_or(|p| i > p),
            };
        }
    }
    let s2 = (1.0 / std::f64::consts::SQRT_2) as f32;
    let mut intenzitnych = 0;
    for (i, &(sfb, w, a, b)) in useky.iter().enumerate() {
        if intenzitny[i] {
            // posledné pásmo nemá škálovací faktor – poloha z predošlého pásma
            let poloha = match w {
                None => sf_p.l[sfb.min(20)],
                Some(w) => sf_p.s[sfb.min(11)][w],
            };
            if poloha != 7 {
                // is_ratio = tan(is_pos·π/12): kl = r/(1 + r), kr = 1/(1 + r) – zapísané cez sin a cos, aby
                // poloha 6 (r = ∞) dala presne kl = 1, kr = 0
                let uhol = poloha as f64 * PI / 12.0;
                let (s, c) = (sin(uhol), cos(uhol));
                let (kl, kr) = ((s / (s + c)) as f32, (c / (s + c)) as f32);
                for j in a..b {
                    let v = l[j];
                    l[j] = v * kl;
                    p[j] = v * kr;
                }
                intenzitnych += 1;
                continue;
            }
        }
        if ms {
            for j in a..b {
                let (m, s) = (l[j], p[j]);
                l[j] = (m + s) * s2;
                p[j] = (m - s) * s2;
            }
        }
    }
    intenzitnych
}

/// Preradenie krátkych blokov ([S] §4.3.10): z poradia [pásmo][okno][čiara] do [podpásmo][okno][čiara]
/// (podpásmo = 6 čiar okna); dlhá časť zmiešaného bloku ostáva.
pub(crate) fn preradenie(xr: &mut [f32; 576], g: &Granula, index_f: usize) {
    if !(g.prepinanie && g.typ_bloku == 2) {
        return;
    }
    let kratke = &tabulky::PASMA_KRATKE[index_f];
    let zdroj = *xr;
    for sfb in if g.zmiesany { 3 } else { 0 }..13 {
        let (zac, sirka) = (kratke[sfb] as usize, (kratke[sfb + 1] - kratke[sfb]) as usize);
        for w in 0..3 {
            for f in 0..sirka {
                let ciara = zac + f; // čiara v okne 0..191
                xr[(ciara / 6) * 18 + w * 6 + ciara % 6] = zdroj[3 * zac + w * sirka + f];
            }
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn stvrtiny_su_presne() {
        assert_eq!(dva_na_stvrtinu(0), 1.0);
        assert_eq!(dva_na_stvrtinu(-8), 0.25);
        assert!((dva_na_stvrtinu(-1) as f64 - 2f64.powf(-0.25)).abs() < 1e-7);
        assert!((dva_na_stvrtinu(-210) as f64 - 2f64.powf(-52.5)).abs() < 1e-22);
        assert!((tab().mocnina43[8] as f64 - 16.0).abs() < 1e-4);
    }

    #[test]
    fn kvantizacia_podla_vzorca() {
        let g = Granula { global_gain: 210, scalefac_scale: true, preflag: true, ..Granula::default() };
        let mut sf = Skalovacie::default();
        sf.l[2] = 1;
        let mut is = [0i32; 576];
        is[0] = 1;
        is[8] = -8; // pásmo 2 pri 44,1 kHz (čiary 8–11)
        let mut xr = [0f32; 576];
        kvantizacia(&is, &g, &sf, 0, &mut xr);
        assert_eq!(xr[0], 1.0);
        // −16 · 2^(−1·(1 + pretab[2] = 0)) = −8
        assert!((xr[8] + 8.0).abs() < 1e-5);
    }

    #[test]
    fn preradenie_kratkych_blokov() {
        let g = Granula { prepinanie: true, typ_bloku: 2, ..Granula::default() };
        let mut xr = [0f32; 576];
        for (i, v) in xr.iter_mut().enumerate() {
            *v = i as f32;
        }
        preradenie(&mut xr, &g, 0);
        // pásmo 0 (4 čiary na okno): okno 1, čiara 2 je v toku na 4 + 2 = 6 → podpásmo 0, okno 1, čiara 2
        assert_eq!(xr[6 + 2], 6.0);
        // čiara 6 okna 2 (pásmo 1, šírka 4, začiatok 4): v toku 12 + 8 + 2 = 22 → podpásmo 1, okno 2, čiara 0
        assert_eq!(xr[18 + 12], 22.0);
        let mut z: Vec<f32> = xr.to_vec();
        z.sort_by(f32::total_cmp);
        assert!(z.iter().enumerate().all(|(i, &v)| v == i as f32), "preradenie nie je permutácia");
    }
}
