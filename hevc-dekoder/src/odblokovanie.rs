//! Odblokovací filter (deblocking filter) – H.265, kap. 8.7.2.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`odblokuj`].
//!
//! Stavba s výhľadom na NEON a súbežnosť po pásoch CTB (krok H5):
//! - `zvisle_hrany` a `vodorovne_hrany` odblokujú pás riadkov blokov 4×4 – jas aj farbu naraz, sila hrany bS
//!   sa pre úsek počíta raz,
//! - `filtruj_usek_jasu` sú rozhodnutia a filter jedného úseku hrany jasu (4 línie), `filtruj_usek_farby` filter
//!   jedného úseku hrany farby (2 línie). Oba pracujú na malom poli „vzorky naprieč hranou × línie pozdĺž hrany“,
//!   preto tie isté funkcie slúžia zvislým aj vodorovným hranám a slučky cez línie sa dajú vektorizovať.
//!
//! Hrany na mriežke 8 × 8 sú od seba aspoň 8 vzoriek a filter mení najviac 3 a číta najviac 4 vzorky na každej
//! strane hrany – úseky jednej fázy (zvislej alebo vodorovnej) sú nezávislé a na ich poradí nezáleží.
//!
//! Úsek hrany, ktorý celý neleží v rovine, sa nefiltruje a nič sa nečíta mimo roviny. V tokoch podľa normy to
//! nenastane (šírka aj výška obrazu sú násobkom MinCbSizeY ≥ 8); ide len o bezpečnosť pri iných rozmeroch.

use std::ops::Range;

use super::obraz::{Obraz, Rovina, Vzorka};
use crate::filtre::{InfoObrazu, MapyFiltra, PohybBloku, RezFiltra};

/// β′ pre Q = 0 … 51 (tabuľka prahov β′ a tC′, 8.7.2.5.3).
const BETA: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 20, 22,
    24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64,
];

/// tC′ pre Q = 0 … 53 (tabuľka prahov β′ a tC′, 8.7.2.5.3).
const TC: [u8; 54] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3,
    4, 4, 4, 5, 5, 6, 6, 7, 8, 9, 10, 11, 13, 14, 16, 18, 20, 22, 24,
];

/// QpC pre qPi = 30 … 43 pri ChromaArrayType = 1 (tabuľka QpC v 8.6.1); pod 30 je QpC = qPi, nad 43 QpC = qPi − 6.
const QPC_30_43: [u8; 14] = [29, 30, 31, 32, 33, 33, 34, 34, 35, 35, 36, 36, 37, 37];

/// Úsek hrany jasu: `u[j]` pre j = 0 … 7 sú vzorky p3, p2, p1, p0, q0, q1, q2, q3 a každá má 4 línie pozdĺž hrany
/// (k = 0 … 3: riadky zvislej hrany, stĺpce vodorovnej) – tvar pre spracovanie všetkých línií naraz.
type UsekJasu = [[i32; 4]; 8];

/// Úsek hrany farby: p1, p0, q0, q1, každá pre 2 línie pozdĺž hrany.
type UsekFarby = [[i32; 2]; 4];

/// Hodnoty spoločné pre celý obraz.
#[derive(Debug, Clone, Copy)]
struct Spolocne {
    /// BitDepth − 8: β = β′ · (1 << posun), tC = tC′ · (1 << posun)
    posun: u32,
    /// najväčšia hodnota vzorky jasu – horná hranica Clip1Y
    max: i32,
    /// to isté pre farbu (BitDepthC môže byť iná ako BitDepthY): posun tC a Clip1C
    posun_c: u32,
    max_c: i32,
    /// cQpPicOffset pre Cb a Cr (pps_cb_qp_offset, pps_cr_qp_offset)
    c_qp: [i32; 2],
}

/// Prahy a obmedzenia jedného úseku hrany.
#[derive(Debug, Clone, Copy)]
struct Prahy {
    beta: i32,
    tc: i32,
    /// najväčšia hodnota vzorky (Clip1)
    max: i32,
    /// vzorky strany P (Q) sa nemenia, nDp = 0 (nDq = 0): pcm_flag pri pcm_loop_filter_disabled_flag alebo
    /// cu_transquant_bypass_flag
    bez_p: bool,
    bez_q: bool,
}

/// Odblokovanie celého obrazu na mieste: najprv všetky zvislé hrany (jas aj farba) z rekonštruovaného obrazu,
/// potom všetky vodorovné hrany z výsledku zvislého filtrovania (8.7.2).
///
/// Sila hrany bS (8.7.2.4) sa odvodí z máp `mapy` (hrany, vnútrosnímkové bloky, nenulové koeficienty, pohyb);
/// rozhodnutia a filtre jasu podľa 8.7.2.5.3–8.7.2.5.7 (β a tC z QpY susedov a posunov rezu, ktorý obsahuje
/// vzorku q0,0), farba 4:2:0 len pri bS = 2 na mriežke 8 × 8 vzoriek farby (8.7.2.5.5). Vzorky blokov
/// s `bez_filtra` sa nemenia.
pub fn odblokuj(obraz: &mut Obraz, mapy: &MapyFiltra, info: &InfoObrazu) {
    let hlbka = obraz.bitova_hlbka;
    debug_assert!(
        (8..=16).contains(&hlbka) && (1u32 << hlbka) - 1 <= u32::from(Vzorka::MAX),
        "nepodporovaná bitová hĺbka {hlbka}"
    );
    let hlbka_c = obraz.bitova_hlbka_farby;
    let s = Spolocne {
        posun: hlbka.saturating_sub(8),
        max: (1 << hlbka) - 1,
        posun_c: hlbka_c.saturating_sub(8),
        max_c: (1 << hlbka_c) - 1,
        c_qp: [info.cb_qp_offset, info.cr_qp_offset],
    };
    zvisle_hrany(&mut obraz.roviny, mapy, info, &s, 0..mapy.vyska4);
    vodorovne_hrany(&mut obraz.roviny, mapy, info, &s, 0..mapy.vyska4);
}

/// Odblokovanie pásu riadkov jasu `y0 … y1 − 1` (riadok CTB; `y0` násobok 8, `y1` začiatok ďalšieho pásu alebo
/// výška obrazu): zvislé hrany pásu a potom vodorovné hrany s y v páse. Postupné volanie pre pásy zhora nadol
/// dá ten istý výsledok ako [`odblokuj`] – vodorovná hrana na hornom okraji pásu číta 4 riadky predošlého pásu
/// (už zvislo filtrované) a mení najviac 3 z nich; hrany v rámci pásu sa neprekrývajú (mriežka 8).
///
/// Pás nesmie zasiahnuť riadky, ktoré ešte číta vnútrosnímková predikcia (spodný riadok rekonštruovanej časti).
pub fn odblokuj_pas(obraz: &mut Obraz, mapy: &MapyFiltra, info: &InfoObrazu, y0: usize, y1: usize) {
    let hlbka = obraz.bitova_hlbka;
    let hlbka_c = obraz.bitova_hlbka_farby;
    let s = Spolocne {
        posun: hlbka.saturating_sub(8),
        max: (1 << hlbka) - 1,
        posun_c: hlbka_c.saturating_sub(8),
        max_c: (1 << hlbka_c) - 1,
        c_qp: [info.cb_qp_offset, info.cr_qp_offset],
    };
    debug_assert!(y0.is_multiple_of(8), "HEVC: pás odblokovania nezačína na mriežke 8");
    let koniec4 = if y1 >= obraz.vyska() { mapy.vyska4 } else { y1 / 4 };
    zvisle_hrany(&mut obraz.roviny, mapy, info, &s, y0 / 4..koniec4);
    vodorovne_hrany(&mut obraz.roviny, mapy, info, &s, y0 / 4..koniec4);
}

/// Zvislé hrany (EDGE_VER) jasu aj farby v riadkoch blokov 4×4 `riadky4` (8.7.2.5.1). Úseky rôznych riadkov
/// blokov sú nezávislé, pás riadkov (napr. riadok CTB) sa dá spracovať samostatne.
fn zvisle_hrany(
    roviny: &mut [Rovina; 3],
    mapy: &MapyFiltra,
    info: &InfoObrazu,
    s: &Spolocne,
    riadky4: Range<usize>,
) {
    let [jas, cb, cr] = roviny;
    let s4 = mapy.sirka4;
    // celý úsek jasu v rovine: q3 na x + 3 < šírka, štvrtá línia na y + 3 < výška
    let x4_koniec = s4.min(jas.sirka / 4);
    let y4_koniec = riadky4.end.min(mapy.vyska4).min(jas.vyska / 4);
    for y4 in riadky4.start..y4_koniec {
        let y = 4 * y4;
        let riadok = y4 * s4;
        let hrany = &mapy.hrana_v[riadok..riadok + x4_koniec];
        // len mriežka 8 (x4 párne); hrana x = 0 je okraj obrazu (filterEdgeFlag = 0)
        for (x4, &hrana) in hrany.iter().enumerate().skip(2).step_by(2) {
            if hrana == 0 {
                continue;
            }
            let q = riadok + x4;
            let p = q - 1;
            let bs = sila_hrany(mapy, p, q, hrana);
            if bs == 0 || (mapy.bez_filtra[p] && mapy.bez_filtra[q]) {
                continue;
            }
            let x = 4 * x4;
            let rez = info.rez_vzorky(x, y);
            let (qp_p, qp_q) = (i32::from(mapy.qp[p]), i32::from(mapy.qp[q]));
            let (beta, tc) = prahy_jasu(qp_p, qp_q, bs, rez, s);
            let pr = Prahy { beta, tc, max: s.max, bez_p: mapy.bez_filtra[p], bez_q: mapy.bez_filtra[q] };
            if beta > 0 && tc > 0 {
                filtruj_zvislu_jasu(jas, x, y, &pr);
            }
            // farba: len bS = 2 a hrana na mriežke 8 vzoriek farby (16 vzoriek jasu)
            if bs == 2 && x % 16 == 0 {
                let (cx, cy) = (x / 2, y / 2);
                for (rovina, &c_qp) in [&mut *cb, &mut *cr].into_iter().zip(&s.c_qp) {
                    let tc = prah_farby(qp_p, qp_q, c_qp, rez, s);
                    if tc > 0 && cx + 2 <= rovina.sirka && cy + 2 <= rovina.vyska {
                        filtruj_zvislu_farby(rovina, cx, cy, &Prahy { tc, max: s.max_c, ..pr });
                    }
                }
            }
        }
    }
}

/// Vodorovné hrany (EDGE_HOR) jasu aj farby na horných stranách riadkov blokov 4×4 `riadky4` (8.7.2.5.2).
/// Volá sa až po zvislých hranách oblasti, ktorej vzorky číta (4 riadky nad a pod hranou).
fn vodorovne_hrany(
    roviny: &mut [Rovina; 3],
    mapy: &MapyFiltra,
    info: &InfoObrazu,
    s: &Spolocne,
    riadky4: Range<usize>,
) {
    let [jas, cb, cr] = roviny;
    let s4 = mapy.sirka4;
    let x4_koniec = s4.min(jas.sirka / 4);
    let y4_koniec = riadky4.end.min(mapy.vyska4).min(jas.vyska / 4);
    // len mriežka 8 (y4 párne); hrana y = 0 je okraj obrazu (filterEdgeFlag = 0)
    let y4_zaciatok = riadky4.start.max(2).next_multiple_of(2);
    for y4 in (y4_zaciatok..y4_koniec).step_by(2) {
        let y = 4 * y4;
        let riadok = y4 * s4;
        let hrany = &mapy.hrana_h[riadok..riadok + x4_koniec];
        for (x4, &hrana) in hrany.iter().enumerate() {
            if hrana == 0 {
                continue;
            }
            let q = riadok + x4;
            let p = q - s4;
            let bs = sila_hrany(mapy, p, q, hrana);
            if bs == 0 || (mapy.bez_filtra[p] && mapy.bez_filtra[q]) {
                continue;
            }
            let x = 4 * x4;
            let rez = info.rez_vzorky(x, y);
            let (qp_p, qp_q) = (i32::from(mapy.qp[p]), i32::from(mapy.qp[q]));
            let (beta, tc) = prahy_jasu(qp_p, qp_q, bs, rez, s);
            let pr = Prahy { beta, tc, max: s.max, bez_p: mapy.bez_filtra[p], bez_q: mapy.bez_filtra[q] };
            if beta > 0 && tc > 0 {
                filtruj_vodorovnu_jasu(jas, x, y, &pr);
            }
            if bs == 2 && y % 16 == 0 {
                let (cx, cy) = (x / 2, y / 2);
                for (rovina, &c_qp) in [&mut *cb, &mut *cr].into_iter().zip(&s.c_qp) {
                    let tc = prah_farby(qp_p, qp_q, c_qp, rez, s);
                    if tc > 0 && cx + 2 <= rovina.sirka && cy + 2 <= rovina.vyska {
                        filtruj_vodorovnu_farby(rovina, cx, cy, &Prahy { tc, max: s.max_c, ..pr });
                    }
                }
            }
        }
    }
}

/// Sila hrany bS (8.7.2.4) medzi blokmi 4×4 `p` (obsahuje p0) a `q` (obsahuje q0); `hrana` je hodnota z mapy
/// hrán (2 = hrana transformačného bloku).
#[inline]
fn sila_hrany(mapy: &MapyFiltra, p: usize, q: usize, hrana: u8) -> u8 {
    if mapy.intra[p] || mapy.intra[q] {
        2
    } else if hrana == 2 && (mapy.nenulove[p] || mapy.nenulove[q]) {
        1
    } else {
        u8::from(iny_pohyb(&mapy.pohyb[p], &mapy.pohyb[q]))
    }
}

/// Pravidlá pohybu pre bS = 1 (8.7.2.4): iné referenčné obrazy alebo iný počet vektorov, alebo vektory, ktoré sa
/// líšia o 4 a viac štvrtín vzorky jasu. Obrazy sa porovnávajú podľa identity (POC), nie podľa zoznamu či indexu.
fn iny_pohyb(a: &PohybBloku, b: &PohybBloku) -> bool {
    if a == b {
        return false; // tá istá predikcia (najčastejšie vnútri jednej PU)
    }
    match (a.obraz, b.obraz) {
        ([Some(a0), Some(a1)], [Some(b0), Some(b1)]) => {
            if a0 != a1 {
                // dva rôzne obrazy: porovnajú sa vektory k tomu istému obrazu
                if (a0, a1) == (b0, b1) {
                    daleko(a.mv[0], b.mv[0]) || daleko(a.mv[1], b.mv[1])
                } else if (a0, a1) == (b1, b0) {
                    daleko(a.mv[0], b.mv[1]) || daleko(a.mv[1], b.mv[0])
                } else {
                    true
                }
            } else if b0 != b1 || b0 != a0 {
                true
            } else {
                // oba vektory oboch blokov k tomu istému obrazu: musia sa líšiť obe párovania
                (daleko(a.mv[0], b.mv[0]) || daleko(a.mv[1], b.mv[1]))
                    && (daleko(a.mv[0], b.mv[1]) || daleko(a.mv[1], b.mv[0]))
            }
        }
        // dva vektory proti najviac jednému
        ([Some(_), Some(_)], _) | (_, [Some(_), Some(_)]) => true,
        _ => match (jediny_vektor(a), jediny_vektor(b)) {
            (Some((ra, va)), Some((rb, vb))) => ra != rb || daleko(va, vb),
            (None, None) => false,
            _ => true,
        },
    }
}

/// Jediný použitý vektor bloku s jeho obrazom (L0 alebo L1).
#[inline]
fn jediny_vektor(m: &PohybBloku) -> Option<(i32, [i16; 2])> {
    match m.obraz {
        [Some(o), _] => Some((o, m.mv[0])),
        [None, Some(o)] => Some((o, m.mv[1])),
        [None, None] => None,
    }
}

/// Vodorovná alebo zvislá zložka vektorov sa líši aspoň o 4 štvrtiny vzorky jasu.
#[inline]
fn daleko(u: [i16; 2], v: [i16; 2]) -> bool {
    (i32::from(u[0]) - i32::from(v[0])).abs() >= 4 || (i32::from(u[1]) - i32::from(v[1])).abs() >= 4
}

/// β a tC úseku jasu (8.7.2.5.3): qPL = (QpQ + QpP + 1) >> 1, posuny z rezu `rez`, ktorý obsahuje q0,0.
#[inline]
fn prahy_jasu(qp_p: i32, qp_q: i32, bs: u8, rez: &RezFiltra, s: &Spolocne) -> (i32, i32) {
    let qpl = (qp_q + qp_p + 1) >> 1;
    let q_beta = (qpl + (i32::from(rez.beta_div2) << 1)).clamp(0, 51);
    let q_tc = (qpl + 2 * (i32::from(bs) - 1) + (i32::from(rez.tc_div2) << 1)).clamp(0, 53);
    (i32::from(BETA[q_beta as usize]) << s.posun, i32::from(TC[q_tc as usize]) << s.posun)
}

/// tC úseku farby pri bS = 2 (8.7.2.5.5): QpC z qPi = ((QpQ + QpP + 1) >> 1) + cQpPicOffset.
#[inline]
fn prah_farby(qp_p: i32, qp_q: i32, c_qp: i32, rez: &RezFiltra, s: &Spolocne) -> i32 {
    let qpc = qpc_z_qpi(((qp_q + qp_p + 1) >> 1) + c_qp);
    // 2 · (bS − 1) = 2
    let q_tc = (qpc + 2 + (i32::from(rez.tc_div2) << 1)).clamp(0, 53);
    i32::from(TC[q_tc as usize]) << s.posun_c
}

/// QpC ako funkcia qPi pre ChromaArrayType = 1 (tabuľka QpC v 8.6.1).
#[inline]
fn qpc_z_qpi(qpi: i32) -> i32 {
    match qpi {
        ..30 => qpi,
        30..=43 => i32::from(QPC_30_43[(qpi - 30) as usize]),
        _ => qpi - 6,
    }
}

/// Zvislá hrana jasu: úsek 4 riadkov so vzorkou q0,0 na (x, y).
fn filtruj_zvislu_jasu(r: &mut Rovina, x: usize, y: usize, pr: &Prahy) {
    let krok = r.krok;
    // p3 prvej línie; línia k je riadok (krok vzorky 1)
    odblokuj_usek_jasu(&mut r.data, y * krok + x - 4, krok, true, pr);
}

/// Vodorovná hrana jasu: úsek 4 stĺpcov so vzorkou q0,0 na (x, y).
fn filtruj_vodorovnu_jasu(r: &mut Rovina, x: usize, y: usize, pr: &Prahy) {
    let krok = r.krok;
    // p3 prvej línie; línia k je stĺpec (krok vzorky = krok riadka)
    odblokuj_usek_jasu(&mut r.data, (y - 4) * krok + x, krok, false, pr);
}

/// Úsek hrany jasu (4 línie) v `data`: p3 prvej línie je `data[i]`; pri zvislej hrane je línia k riadok
/// `i + k · krok` a vzorky p3 … q3 idú za sebou, pri vodorovnej je línia k stĺpec `i + k` a vzorky idú po `krok`.
/// Pre 8 bitov na aarch64 ručné NEON (modul `simd`, bitovo rovnaké), inak [`odblokuj_usek_jasu_skalarne`].
#[inline]
fn odblokuj_usek_jasu(data: &mut [Vzorka], i: usize, krok: usize, zvisla: bool, pr: &Prahy) {
    if pr.max == 255 && super::simd::odblokuj_jas(data, i, krok, zvisla, pr.beta, pr.tc, [pr.bez_p, pr.bez_q])
    {
        return;
    }
    odblokuj_usek_jasu_skalarne(data, i, krok, zvisla, pr.beta, pr.tc, [pr.bez_p, pr.bez_q], pr.max);
}

/// Skalárny úsek hrany jasu (ako [`odblokuj_usek_jasu`]) – aj rozhodca pre skúšku NEON. `bez` = [nDp = 0,
/// nDq = 0] (strana P či Q sa nemení), `max` = Clip1Y.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub(super) fn odblokuj_usek_jasu_skalarne(
    data: &mut [Vzorka],
    i: usize,
    krok: usize,
    zvisla: bool,
    beta: i32,
    tc: i32,
    bez: [bool; 2],
    max: i32,
) {
    let mut u: UsekJasu = [[0; 4]; 8];
    if zvisla {
        // línia k = riadok: 8 vzoriek p3 … q3 za sebou
        for k in 0..4 {
            for (uj, &v) in u.iter_mut().zip(&data[i + k * krok..][..8]) {
                uj[k] = i32::from(v);
            }
        }
    } else {
        // vzorka j = riadok: 4 línie za sebou
        for (j, uj) in u.iter_mut().enumerate() {
            for (d, &v) in uj.iter_mut().zip(&data[i + j * krok..][..4]) {
                *d = i32::from(v);
            }
        }
    }
    if !filtruj_usek_jasu(&mut u, beta, tc, max) {
        return;
    }
    let (od, po) = rozsah_zapisu(&Prahy { beta, tc, max, bez_p: bez[0], bez_q: bez[1] }, 3);
    if zvisla {
        for k in 0..4 {
            let linia = &mut data[i + k * krok..][..8];
            for (v, uj) in linia[od..po].iter_mut().zip(&u[od..po]) {
                *v = uj[k] as Vzorka;
            }
        }
    } else {
        for (j, uj) in u.iter().enumerate().take(po).skip(od) {
            for (v, &d) in data[i + j * krok..][..4].iter_mut().zip(uj) {
                *v = d as Vzorka;
            }
        }
    }
}

/// Rozsah indexov úseku (naprieč hranou), ktoré sa zapíšu späť: `n` vzoriek na každej strane okrem strán
/// s `bez_filtra` (nDp = 0, nDq = 0).
#[inline]
fn rozsah_zapisu(pr: &Prahy, n: usize) -> (usize, usize) {
    let stred = n + 1; // index q0 = počet vzoriek strany P v úseku
    let od = if pr.bez_p { stred } else { 1 };
    let po = if pr.bez_q { stred } else { stred + n };
    (od, po)
}

/// Rozhodnutia a filter jedného úseku hrany jasu (8.7.2.5.3, 8.7.2.5.6, 8.7.2.5.7) na mieste. Vráti `false`,
/// ak sa úsek nefiltruje (d ≥ β); inak sú v `u` nové hodnoty p2 … q2 (nezmenené vzorky si nechajú hodnotu).
fn filtruj_usek_jasu(u: &mut UsekJasu, beta: i32, tc: i32, max: i32) -> bool {
    let [p3, p2, p1, p0, q0, q1, q2, q3] = *u;
    // 8.7.2.5.3: druhé diferencie línií 0 a 3
    let dp0 = (p2[0] - 2 * p1[0] + p0[0]).abs();
    let dp3 = (p2[3] - 2 * p1[3] + p0[3]).abs();
    let dq0 = (q2[0] - 2 * q1[0] + q0[0]).abs();
    let dq3 = (q2[3] - 2 * q1[3] + q0[3]).abs();
    let (dpq0, dpq3) = (dp0 + dq0, dp3 + dq3);
    if dpq0 + dpq3 >= beta {
        return false; // dE = 0
    }
    // 8.7.2.5.6: rozhodnutie pre vzorku línie k (dSam)
    let silna_linia = |k: usize, dpq: i32| {
        dpq < (beta >> 2)
            && (p3[k] - p0[k]).abs() + (q0[k] - q3[k]).abs() < (beta >> 3)
            && (p0[k] - q0[k]).abs() < ((5 * tc + 1) >> 1)
    };
    if silna_linia(0, 2 * dpq0) && silna_linia(3, 2 * dpq3) {
        // dE = 2: silný filter, nDp = nDq = 3
        let t2 = 2 * tc;
        for k in 0..4 {
            let (a3, a2, a1, a0) = (p3[k], p2[k], p1[k], p0[k]);
            let (b0, b1, b2, b3) = (q0[k], q1[k], q2[k], q3[k]);
            u[3][k] = ((a2 + 2 * a1 + 2 * a0 + 2 * b0 + b1 + 4) >> 3).clamp(a0 - t2, a0 + t2);
            u[2][k] = ((a2 + a1 + a0 + b0 + 2) >> 2).clamp(a1 - t2, a1 + t2);
            u[1][k] = ((2 * a3 + 3 * a2 + a1 + a0 + b0 + 4) >> 3).clamp(a2 - t2, a2 + t2);
            u[4][k] = ((a1 + 2 * a0 + 2 * b0 + 2 * b1 + b2 + 4) >> 3).clamp(b0 - t2, b0 + t2);
            u[5][k] = ((a0 + b0 + b1 + b2 + 2) >> 2).clamp(b1 - t2, b1 + t2);
            u[6][k] = ((a0 + b0 + b1 + 3 * b2 + 2 * b3 + 4) >> 3).clamp(b2 - t2, b2 + t2);
        }
    } else {
        // dE = 1: slabý filter; dEp, dEq z línií 0 a 3
        let prah = (beta + (beta >> 1)) >> 3;
        let (dep, deq) = (dp0 + dp3 < prah, dq0 + dq3 < prah);
        let (t10, t2) = (10 * tc, tc >> 1);
        for k in 0..4 {
            let (a2, a1, a0) = (p2[k], p1[k], p0[k]);
            let (b0, b1, b2) = (q0[k], q1[k], q2[k]);
            let delta = (9 * (b0 - a0) - 3 * (b1 - a1) + 8) >> 4;
            if delta.abs() >= t10 {
                continue; // línia sa nemení (nDp = nDq = 0)
            }
            let delta = delta.clamp(-tc, tc);
            u[3][k] = (a0 + delta).clamp(0, max);
            u[4][k] = (b0 - delta).clamp(0, max);
            if dep {
                let dp = ((((a2 + a0 + 1) >> 1) - a1 + delta) >> 1).clamp(-t2, t2);
                u[2][k] = (a1 + dp).clamp(0, max);
            }
            if deq {
                let dq = ((((b2 + b0 + 1) >> 1) - b1 - delta) >> 1).clamp(-t2, t2);
                u[5][k] = (b1 + dq).clamp(0, max);
            }
        }
    }
    true
}

/// Zvislá hrana farby: úsek 2 riadkov so vzorkou q0,0 na (x, y) roviny farby.
fn filtruj_zvislu_farby(r: &mut Rovina, x: usize, y: usize, pr: &Prahy) {
    let krok = r.krok;
    let zaciatok = y * krok + x - 2; // p1 prvej línie
    let mut u: UsekFarby = [[0; 2]; 4];
    for k in 0..2 {
        let linia = &r.data[zaciatok + k * krok..][..4];
        for (uj, &v) in u.iter_mut().zip(linia) {
            uj[k] = i32::from(v);
        }
    }
    filtruj_usek_farby(&mut u, pr.tc, pr.max);
    let (od, po) = rozsah_zapisu(pr, 1);
    for k in 0..2 {
        let linia = &mut r.data[zaciatok + k * krok..][..4];
        for (v, uj) in linia[od..po].iter_mut().zip(&u[od..po]) {
            *v = uj[k] as Vzorka;
        }
    }
}

/// Vodorovná hrana farby: úsek 2 stĺpcov so vzorkou q0,0 na (x, y) roviny farby.
fn filtruj_vodorovnu_farby(r: &mut Rovina, x: usize, y: usize, pr: &Prahy) {
    let krok = r.krok;
    let zaciatok = (y - 2) * krok + x; // p1 prvej línie
    let mut u: UsekFarby = [[0; 2]; 4];
    for (j, uj) in u.iter_mut().enumerate() {
        let riadok = &r.data[zaciatok + j * krok..][..2];
        for (d, &v) in uj.iter_mut().zip(riadok) {
            *d = i32::from(v);
        }
    }
    filtruj_usek_farby(&mut u, pr.tc, pr.max);
    let (od, po) = rozsah_zapisu(pr, 1);
    for (j, uj) in u.iter().enumerate().take(po).skip(od) {
        let riadok = &mut r.data[zaciatok + j * krok..][..2];
        for (v, &d) in riadok.iter_mut().zip(uj) {
            *v = d as Vzorka;
        }
    }
}

/// Filter jedného úseku hrany farby (8.7.2.5.8) na mieste: nové p0 a q0 oboch línií.
fn filtruj_usek_farby(u: &mut UsekFarby, tc: i32, max: i32) {
    let [p1, p0, q0, q1] = *u;
    for k in 0..2 {
        let delta = ((((q0[k] - p0[k]) * 4) + p1[k] - q1[k] + 4) >> 3).clamp(-tc, tc);
        u[1][k] = (p0[k] + delta).clamp(0, max);
        u[2][k] = (q0[k] - delta).clamp(0, max);
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    // ---------------------------------------------------------------------------------------------------------
    // Rozhodca: priamy, pomalý prepis normy (8.7.2) po hranách v poradí normy, so samostatným výpočtom bS, β, tC,
    // dE, dEp, dEq a s vlastným prepisom tabuliek. Z rýchlej implementácie nepoužíva nič.
    // ---------------------------------------------------------------------------------------------------------

    /// Rovina rozhodcu: vzorky ako i32 bez zarovnania riadkov.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RovinaR {
        v: Vec<i32>,
        sirka: usize,
        vyska: usize,
    }

    impl RovinaR {
        fn z_roviny(r: &Rovina) -> RovinaR {
            let mut v = Vec::with_capacity(r.sirka * r.vyska);
            for y in 0..r.vyska {
                v.extend(r.riadok(y).iter().map(|&s| i32::from(s)));
            }
            RovinaR { v, sirka: r.sirka, vyska: r.vyska }
        }

        fn daj(&self, (x, y): (usize, usize)) -> i32 {
            assert!(x < self.sirka && y < self.vyska, "rozhodca číta mimo roviny ({x}, {y})");
            self.v[y * self.sirka + x]
        }

        fn nastav(&mut self, (x, y): (usize, usize), h: i32) {
            assert!(x < self.sirka && y < self.vyska, "rozhodca píše mimo roviny ({x}, {y})");
            self.v[y * self.sirka + x] = h;
        }
    }

    /// Javy, ktoré rozhodca videl – skúška overí, že náhodné prípady pokryli všetky vetvy normy.
    #[derive(Debug, Default)]
    struct Pokrytie {
        /// pravidlá bS: 0 vnútrosnímkový blok, 1 koeficienty, 2 iné obrazy alebo počet vektorov, 3 jeden vektor,
        /// 4 dva vektory k dvom rôznym obrazom, 5 dva vektory k tomu istému obrazu, 6 bS = 0,
        /// 7 bS = 0, hoci sa jedno párovanie vektorov k tomu istému obrazu líši
        bs: [u32; 8],
        silne: u32,
        slabe: u32,
        slabe_p1: u32,
        slabe_q1: u32,
        velka_delta: u32,
        farba: u32,
        bez_filtra: u32,
        orezanie: u32,
    }

    fn clip3(x: i32, y: i32, z: i32) -> i32 {
        if z < x {
            x
        } else if z > y {
            y
        } else {
            z
        }
    }

    /// β′(Q) podľa tabuľky normy, prepísané po úsekoch.
    fn beta_r(q: i32) -> i32 {
        match q {
            0..=15 => 0,
            16..=28 => q - 10,
            29..=51 => 2 * q - 38,
            _ => panic!("Q mimo tabuľky β′: {q}"),
        }
    }

    /// tC′(Q) podľa tabuľky normy, prepísané po úsekoch.
    fn tc_r(q: i32) -> i32 {
        match q {
            0..=17 => 0,
            18..=26 => 1,
            27..=30 => 2,
            31..=34 => 3,
            35..=37 => 4,
            38..=39 => 5,
            40..=41 => 6,
            42 => 7,
            43 => 8,
            44 => 9,
            45 => 10,
            46 => 11,
            47 => 13,
            48 => 14,
            49 => 16,
            50 => 18,
            51 => 20,
            52 => 22,
            53 => 24,
            _ => panic!("Q mimo tabuľky tC′: {q}"),
        }
    }

    /// QpC(qPi) pre ChromaArrayType = 1.
    fn qpc_r(qpi: i32) -> i32 {
        match qpi {
            ..=29 => qpi,
            30 => 29,
            31 => 30,
            32 => 31,
            33 => 32,
            34 | 35 => 33,
            36 | 37 => 34,
            38 | 39 => 35,
            40 | 41 => 36,
            42 | 43 => 37,
            _ => qpi - 6,
        }
    }

    /// bS podľa 8.7.2.4 doslovne pre vzorky p0 na (xp, yp) a q0 na (xq, yq) v jase; vráti aj pravidlo.
    fn bs_r(
        mapy: &MapyFiltra,
        (xp, yp): (usize, usize),
        (xq, yq): (usize, usize),
        hrana_tu: bool,
    ) -> (i32, usize) {
        let p = mapy.index(xp, yp);
        let q = mapy.index(xq, yq);
        if mapy.intra[p] || mapy.intra[q] {
            return (2, 0);
        }
        if hrana_tu && (mapy.nenulove[p] || mapy.nenulove[q]) {
            return (1, 1);
        }
        // vektory bloku: (obraz, vektor) v poradí L0, L1
        let vektory = |m: &PohybBloku| -> Vec<(i32, [i32; 2])> {
            (0..2)
                .filter_map(|l| m.obraz[l].map(|o| (o, [i32::from(m.mv[l][0]), i32::from(m.mv[l][1])])))
                .collect()
        };
        let (vp, vq) = (vektory(&mapy.pohyb[p]), vektory(&mapy.pohyb[q]));
        let mut obrazy_p: Vec<i32> = vp.iter().map(|v| v.0).collect();
        let mut obrazy_q: Vec<i32> = vq.iter().map(|v| v.0).collect();
        obrazy_p.sort();
        obrazy_q.sort();
        // iné referenčné obrazy (bez ohľadu na zoznam a index) alebo iný počet vektorov
        if vp.len() != vq.len() || obrazy_p != obrazy_q {
            return (1, 2);
        }
        let rozdiel = |a: [i32; 2], b: [i32; 2]| (a[0] - b[0]).abs() >= 4 || (a[1] - b[1]).abs() >= 4;
        match vp.len() {
            1 => {
                if rozdiel(vp[0].1, vq[0].1) {
                    (1, 3)
                } else {
                    (0, 6)
                }
            }
            2 if vp[0].0 != vp[1].0 => {
                // dva rôzne obrazy: vektory oboch blokov k tomu istému obrazu
                let mut r = false;
                for &(obraz, v) in &vp {
                    let w = vq.iter().find(|w| w.0 == obraz).expect("ten istý obraz").1;
                    r |= rozdiel(v, w);
                }
                if r { (1, 4) } else { (0, 6) }
            }
            2 => {
                // oba vektory k tomu istému obrazu
                let (p0, p1, q0, q1) = (vp[0].1, vp[1].1, vq[0].1, vq[1].1);
                let c1 = rozdiel(p0, q0) || rozdiel(p1, q1);
                let c2 = rozdiel(p0, q1) || rozdiel(p1, q0);
                if c1 && c2 {
                    (1, 5)
                } else if c1 || c2 {
                    (0, 7)
                } else {
                    (0, 6)
                }
            }
            _ => (0, 6),
        }
    }

    /// Filter vzorky jasu jednej línie (8.7.2.5.7): vráti nové p0 … p2, q0 … q2 a nDp, nDq.
    fn vzorka_jasu_r(
        p: [i32; 4],
        q: [i32; 4],
        (de, dep, deq): (i32, i32, i32),
        tc: i32,
        max: i32,
        pok: &mut Pokrytie,
    ) -> ([i32; 3], [i32; 3], usize, usize) {
        let mut pn = [p[0], p[1], p[2]];
        let mut qn = [q[0], q[1], q[2]];
        if de == 2 {
            pn[0] =
                clip3(p[0] - 2 * tc, p[0] + 2 * tc, (p[2] + 2 * p[1] + 2 * p[0] + 2 * q[0] + q[1] + 4) >> 3);
            pn[1] = clip3(p[1] - 2 * tc, p[1] + 2 * tc, (p[2] + p[1] + p[0] + q[0] + 2) >> 2);
            pn[2] = clip3(p[2] - 2 * tc, p[2] + 2 * tc, (2 * p[3] + 3 * p[2] + p[1] + p[0] + q[0] + 4) >> 3);
            qn[0] =
                clip3(q[0] - 2 * tc, q[0] + 2 * tc, (p[1] + 2 * p[0] + 2 * q[0] + 2 * q[1] + q[2] + 4) >> 3);
            qn[1] = clip3(q[1] - 2 * tc, q[1] + 2 * tc, (p[0] + q[0] + q[1] + q[2] + 2) >> 2);
            qn[2] = clip3(q[2] - 2 * tc, q[2] + 2 * tc, (p[0] + q[0] + q[1] + 3 * q[2] + 2 * q[3] + 4) >> 3);
            return (pn, qn, 3, 3);
        }
        let mut delta = (9 * (q[0] - p[0]) - 3 * (q[1] - p[1]) + 8) >> 4;
        if delta.abs() >= tc * 10 {
            pok.velka_delta += 1;
            return (pn, qn, 0, 0);
        }
        delta = clip3(-tc, tc, delta);
        let clip1 = |h: i32, pok: &mut Pokrytie| {
            if h < 0 || h > max {
                pok.orezanie += 1;
            }
            clip3(0, max, h)
        };
        pn[0] = clip1(p[0] + delta, pok);
        qn[0] = clip1(q[0] - delta, pok);
        if dep == 1 {
            let delta_p = clip3(-(tc >> 1), tc >> 1, (((p[2] + p[0] + 1) >> 1) - p[1] + delta) >> 1);
            pn[1] = clip1(p[1] + delta_p, pok);
            pok.slabe_p1 += 1;
        }
        if deq == 1 {
            let delta_q = clip3(-(tc >> 1), tc >> 1, (((q[2] + q[0] + 1) >> 1) - q[1] - delta) >> 1);
            qn[1] = clip1(q[1] + delta_q, pok);
            pok.slabe_q1 += 1;
        }
        (pn, qn, dep as usize + 1, deq as usize + 1)
    }

    /// Úsek hrany jasu so vzorkou q0,0 na (x, y): 8.7.2.4 (bS), 8.7.2.5.3 (rozhodnutia), 8.7.2.5.4 a 8.7.2.5.7.
    fn hrana_jasu_r(
        rov: &mut RovinaR,
        mapy: &MapyFiltra,
        info: &InfoObrazu,
        zvisla: bool,
        (x, y): (usize, usize),
        hlbka: u32,
        pok: &mut Pokrytie,
    ) {
        // filterEdgeFlag = 0 na ľavom a hornom okraji obrazu
        if (zvisla && x == 0) || (!zvisla && y == 0) {
            return;
        }
        let i = mapy.index(x, y);
        let hrana = if zvisla { mapy.hrana_v[i] } else { mapy.hrana_h[i] };
        if hrana == 0 {
            return;
        }
        let (xp, yp) = if zvisla { (x - 1, y) } else { (x, y - 1) };
        let (bs, pravidlo) = bs_r(mapy, (xp, yp), (x, y), hrana == 2);
        pok.bs[pravidlo] += 1;
        if bs == 0 {
            return;
        }
        // úsek celý v rovine (mimo normy – rozmery, ktoré nie sú násobkom 8)
        if x + 4 > rov.sirka || y + 4 > rov.vyska {
            return;
        }
        // p_i,k a q_i,k: i naprieč hranou, k pozdĺž hrany
        let miesto = |strana_q: bool, i: usize, k: usize| match (zvisla, strana_q) {
            (true, true) => (x + i, y + k),
            (true, false) => (x - i - 1, y + k),
            (false, true) => (x + k, y + i),
            (false, false) => (x + k, y - i - 1),
        };
        let mut p = [[0i32; 4]; 4]; // p[k][i]
        let mut q = [[0i32; 4]; 4];
        for k in 0..4 {
            for i in 0..4 {
                p[k][i] = rov.daj(miesto(false, i, k));
                q[k][i] = rov.daj(miesto(true, i, k));
            }
        }
        let qp_q = i32::from(mapy.qp[mapy.index(x, y)]);
        let qp_p = i32::from(mapy.qp[mapy.index(xp, yp)]);
        let qpl = (qp_q + qp_p + 1) >> 1;
        let rez = info.rez_vzorky(x, y); // rez so vzorkou q0,0
        let q_beta = clip3(0, 51, qpl + (i32::from(rez.beta_div2) << 1));
        let beta = beta_r(q_beta) * (1 << (hlbka - 8));
        let q_tc = clip3(0, 53, qpl + 2 * (bs - 1) + (i32::from(rez.tc_div2) << 1));
        let tc = tc_r(q_tc) * (1 << (hlbka - 8));
        let dp0 = (p[0][2] - 2 * p[0][1] + p[0][0]).abs();
        let dp3 = (p[3][2] - 2 * p[3][1] + p[3][0]).abs();
        let dq0 = (q[0][2] - 2 * q[0][1] + q[0][0]).abs();
        let dq3 = (q[3][2] - 2 * q[3][1] + q[3][0]).abs();
        let dpq0 = dp0 + dq0;
        let dpq3 = dp3 + dq3;
        let dp = dp0 + dp3;
        let dq = dq0 + dq3;
        let d = dpq0 + dpq3;
        let (mut de, mut dep, mut deq) = (0, 0, 0);
        if d < beta {
            // 8.7.2.5.6
            let dsam = |dpq: i32, k: usize| {
                dpq < (beta >> 2)
                    && (p[k][3] - p[k][0]).abs() + (q[k][0] - q[k][3]).abs() < (beta >> 3)
                    && (p[k][0] - q[k][0]).abs() < ((5 * tc + 1) >> 1)
            };
            let dsam0 = dsam(2 * dpq0, 0);
            let dsam3 = dsam(2 * dpq3, 3);
            de = 1;
            if dsam0 && dsam3 {
                de = 2;
            }
            if dp < ((beta + (beta >> 1)) >> 3) {
                dep = 1;
            }
            if dq < ((beta + (beta >> 1)) >> 3) {
                deq = 1;
            }
        }
        if de == 0 {
            return;
        }
        if de == 2 {
            pok.silne += 1;
        } else {
            pok.slabe += 1;
        }
        let bez_p = mapy.bez_filtra[mapy.index(xp, yp)];
        let bez_q = mapy.bez_filtra[mapy.index(x, y)];
        if bez_p || bez_q {
            pok.bez_filtra += 1;
        }
        let max = (1 << hlbka) - 1;
        for k in 0..4 {
            let (pn, qn, mut n_dp, mut n_dq) = vzorka_jasu_r(p[k], q[k], (de, dep, deq), tc, max, pok);
            if bez_p {
                n_dp = 0;
            }
            if bez_q {
                n_dq = 0;
            }
            for (i, &h) in pn.iter().enumerate().take(n_dp) {
                rov.nastav(miesto(false, i, k), h);
            }
            for (i, &h) in qn.iter().enumerate().take(n_dq) {
                rov.nastav(miesto(true, i, k), h);
            }
        }
    }

    /// Úsek hrany farby so vzorkou q0,0 na (x, y) roviny farby `c_idx` (8.7.2.5.5, 8.7.2.5.8).
    #[allow(clippy::too_many_arguments)]
    fn hrana_farby_r(
        rov: &mut RovinaR,
        c_idx: usize,
        mapy: &MapyFiltra,
        info: &InfoObrazu,
        zvisla: bool,
        (x, y): (usize, usize),
        hlbka: u32,
        pok: &mut Pokrytie,
    ) {
        if (zvisla && x == 0) || (!zvisla && y == 0) {
            return;
        }
        // len hrany na mriežke 8 vzoriek farby
        if (zvisla && x % 8 != 0) || (!zvisla && y % 8 != 0) {
            return;
        }
        // zodpovedajúce vzorky jasu (SubWidthC = SubHeightC = 2)
        let (xl, yl) = (2 * x, 2 * y);
        let i = mapy.index(xl, yl);
        let hrana = if zvisla { mapy.hrana_v[i] } else { mapy.hrana_h[i] };
        if hrana == 0 {
            return;
        }
        let (xpl, ypl) = if zvisla { (2 * (x - 1), yl) } else { (xl, 2 * (y - 1)) };
        let (bs, _) = bs_r(mapy, (xpl, ypl), (xl, yl), hrana == 2);
        if bs != 2 {
            return;
        }
        if x + 2 > rov.sirka || y + 2 > rov.vyska {
            return;
        }
        let qp_q = i32::from(mapy.qp[mapy.index(xl, yl)]);
        let qp_p = i32::from(mapy.qp[mapy.index(xpl, ypl)]);
        let c_qp_pic_offset = if c_idx == 1 { info.cb_qp_offset } else { info.cr_qp_offset };
        let qpi = ((qp_q + qp_p + 1) >> 1) + c_qp_pic_offset;
        let qpc = qpc_r(qpi);
        let rez = info.rez_vzorky(xl, yl);
        let q_tc = clip3(0, 53, qpc + 2 * (bs - 1) + (i32::from(rez.tc_div2) << 1));
        let tc = tc_r(q_tc) * (1 << (hlbka - 8));
        let bez_p = mapy.bez_filtra[mapy.index(xpl, ypl)];
        let bez_q = mapy.bez_filtra[mapy.index(xl, yl)];
        let max = (1 << hlbka) - 1;
        let miesto = |strana_q: bool, i: usize, k: usize| match (zvisla, strana_q) {
            (true, true) => (x + i, y + k),
            (true, false) => (x - i - 1, y + k),
            (false, true) => (x + k, y + i),
            (false, false) => (x + k, y - i - 1),
        };
        pok.farba += 1;
        for k in 0..2 {
            let (p0, p1) = (rov.daj(miesto(false, 0, k)), rov.daj(miesto(false, 1, k)));
            let (q0, q1) = (rov.daj(miesto(true, 0, k)), rov.daj(miesto(true, 1, k)));
            let delta = clip3(-tc, tc, (((q0 - p0) << 2) + p1 - q1 + 4) >> 3);
            if !bez_p {
                rov.nastav(miesto(false, 0, k), clip3(0, max, p0 + delta));
            }
            if !bez_q {
                rov.nastav(miesto(true, 0, k), clip3(0, max, q0 - delta));
            }
        }
    }

    /// Rozhodca celého obrazu: všetky zvislé hrany zľava doprava, potom všetky vodorovné zhora nadol.
    fn rozhodca(obraz: &Obraz, mapy: &MapyFiltra, info: &InfoObrazu, pok: &mut Pokrytie) -> [RovinaR; 3] {
        let (hlbka, hlbka_c) = (obraz.bitova_hlbka, obraz.bitova_hlbka_farby);
        let mut r = obraz.roviny.each_ref().map(RovinaR::z_roviny);
        let [jas, cb, cr] = &mut r;
        let (w, h) = (jas.sirka, jas.vyska);
        for x in (0..w).step_by(8) {
            for y in (0..h).step_by(4) {
                hrana_jasu_r(jas, mapy, info, true, (x, y), hlbka, pok);
            }
        }
        for (c_idx, rov) in [(1, &mut *cb), (2, &mut *cr)] {
            for x in (0..rov.sirka).step_by(8) {
                for y in (0..rov.vyska).step_by(2) {
                    hrana_farby_r(rov, c_idx, mapy, info, true, (x, y), hlbka_c, pok);
                }
            }
        }
        for y in (0..h).step_by(8) {
            for x in (0..w).step_by(4) {
                hrana_jasu_r(jas, mapy, info, false, (x, y), hlbka, pok);
            }
        }
        for (c_idx, rov) in [(1, &mut *cb), (2, &mut *cr)] {
            for y in (0..rov.vyska).step_by(8) {
                for x in (0..rov.sirka).step_by(2) {
                    hrana_farby_r(rov, c_idx, mapy, info, false, (x, y), hlbka_c, pok);
                }
            }
        }
        r
    }

    // ---------------------------------------------------------------------------------------------------------
    // Náhodné prípady
    // ---------------------------------------------------------------------------------------------------------

    /// Celé číslo v [od, po].
    fn cislo(n: &mut Nahoda, od: i32, po: i32) -> i32 {
        od + n.mensie_ako((po - od + 1) as u64) as i32
    }

    fn sanca(n: &mut Nahoda, percent: u64) -> bool {
        n.mensie_ako(100) < percent
    }

    fn vyber<T: Copy>(n: &mut Nahoda, moznosti: &[T]) -> T {
        moznosti[n.mensie_ako(moznosti.len() as u64) as usize]
    }

    /// Rovina s blokmi rôznej úrovne (skoky na hranách), sklonom a šumom – aby sa filtre naozaj uplatnili.
    fn vypln_rovinu(n: &mut Nahoda, r: &mut Rovina) {
        let blok = vyber(n, &[4usize, 8, 8, 16]);
        let skok = vyber(n, &[1, 3, 8, 20, 50, 255]);
        let sum = vyber(n, &[0, 0, 1, 2, 5, 40]);
        let (bw, bh) = (r.sirka.div_ceil(blok), r.vyska.div_ceil(blok));
        let mut urovne = vec![0i32; bw * bh];
        let mut sklony = vec![(0i32, 0i32); bw * bh];
        for by in 0..bh {
            for bx in 0..bw {
                let zaklad = if bx > 0 {
                    urovne[by * bw + bx - 1]
                } else if by > 0 {
                    urovne[(by - 1) * bw]
                } else {
                    cislo(n, 0, 255)
                };
                urovne[by * bw + bx] = (zaklad + cislo(n, -skok, skok)).clamp(0, 255);
                sklony[by * bw + bx] = if sanca(n, 30) { (cislo(n, -3, 3), cislo(n, -3, 3)) } else { (0, 0) };
            }
        }
        for y in 0..r.vyska {
            for x in 0..r.sirka {
                let b = (y / blok) * bw + x / blok;
                let (gx, gy) = sklony[b];
                let h = urovne[b] + gx * (x % blok) as i32 + gy * (y % blok) as i32 + cislo(n, -sum, sum);
                r.data[y * r.krok + x] = h.clamp(0, 255) as Vzorka;
            }
        }
    }

    /// Pohyb medzisnímkového bloku: 1 vektor (L0 alebo L1) alebo 2 vektory; nepoužitý vektor má náhodný obsah,
    /// ktorý sa nesmie uplatniť.
    fn nahodny_pohyb(n: &mut Nahoda) -> PohybBloku {
        let mut m = PohybBloku::default();
        for mv in &mut m.mv {
            *mv = [cislo(n, -12, 12) as i16, cislo(n, -12, 12) as i16];
        }
        // malá zásoba obrazov (POC −1 … 1) – časté zhody aj rozdiely
        let (a, b) = (Some(cislo(n, -1, 1)), Some(cislo(n, -1, 1)));
        m.obraz = match cislo(n, 0, 3) {
            0 => [a, None],
            1 => [None, a],
            _ => [a, b],
        };
        m
    }

    /// Mapy s blokmi, ktoré často zdieľajú vlastnosti so susedom (bloky jednej PU či CU), hrany na mriežke 8 × 8
    /// a niekedy aj smeti mimo mriežky a na okraji obrazu (norma ich nepoužije).
    fn nahodne_mapy(n: &mut Nahoda, sirka: usize, vyska: usize) -> MapyFiltra {
        let mut m = MapyFiltra::nove(sirka, vyska);
        let (s4, v4) = (m.sirka4, m.vyska4);
        let qp_stred = cislo(n, 0, 51);
        let qp_rozptyl = vyber(n, &[0, 2, 6, 51]);
        let p_intra = vyber(n, &[0, 10, 25, 100]);
        let p_nenulove = vyber(n, &[0, 30, 70]);
        let p_bez = vyber(n, &[0, 0, 5, 30]);
        let p_hrana = vyber(n, &[30, 70, 100]);
        let smeti = sanca(n, 15);
        for y4 in 0..v4 {
            for x4 in 0..s4 {
                let i = y4 * s4 + x4;
                let zdroj = if x4 > 0 && sanca(n, 40) {
                    Some(i - 1)
                } else if y4 > 0 && sanca(n, 30) {
                    Some(i - s4)
                } else {
                    None
                };
                if let Some(z) = zdroj {
                    m.intra[i] = m.intra[z];
                    m.nenulove[i] = m.nenulove[z];
                    m.pohyb[i] = m.pohyb[z];
                    m.qp[i] = m.qp[z];
                    m.bez_filtra[i] = m.bez_filtra[z];
                    if sanca(n, 30) {
                        // malá zmena vektora okolo prahu 4 štvrtín vzorky
                        let l = cislo(n, 0, 1) as usize;
                        let c = cislo(n, 0, 1) as usize;
                        m.pohyb[i].mv[l][c] += cislo(n, -5, 5) as i16;
                    }
                    if sanca(n, 10) {
                        m.qp[i] = (qp_stred + cislo(n, -qp_rozptyl, qp_rozptyl)).clamp(0, 51) as i8;
                    }
                } else {
                    m.intra[i] = sanca(n, p_intra);
                    m.nenulove[i] = sanca(n, p_nenulove);
                    m.pohyb[i] = if m.intra[i] { PohybBloku::default() } else { nahodny_pohyb(n) };
                    m.qp[i] = (qp_stred + cislo(n, -qp_rozptyl, qp_rozptyl)).clamp(0, 51) as i8;
                    m.bez_filtra[i] = sanca(n, p_bez);
                }
                let hodnota = |n: &mut Nahoda| if sanca(n, p_hrana) { vyber(n, &[1u8, 2, 2]) } else { 0 };
                if x4 % 2 == 0 && x4 > 0 || smeti && sanca(n, 20) {
                    m.hrana_v[i] = hodnota(n);
                }
                if y4 % 2 == 0 && y4 > 0 || smeti && sanca(n, 20) {
                    m.hrana_h[i] = hodnota(n);
                }
            }
        }
        m
    }

    /// Info obrazu s 1 až 4 rezmi (rôzne posuny β a tC −6 … 6) a CTB 16, 32 alebo 64.
    fn nahodne_info(n: &mut Nahoda, sirka: usize, vyska: usize) -> InfoObrazu {
        let log2_ctb = cislo(n, 4, 6) as u32;
        let ctb = 1usize << log2_ctb;
        let (sirka_ctb, vyska_ctb) = (sirka.div_ceil(ctb), vyska.div_ceil(ctb));
        let pocet = sirka_ctb * vyska_ctb;
        let rezov = cislo(n, 1, 4) as usize;
        let rezy = (0..rezov)
            .map(|_| RezFiltra {
                adresa: 0,
                cez_rezy: true,
                beta_div2: cislo(n, -6, 6) as i8,
                tc_div2: cislo(n, -6, 6) as i8,
            })
            .collect();
        let mut ctb_rez = Vec::with_capacity(pocet);
        let mut rez = 0;
        for _ in 0..pocet {
            if rez + 1 < rezov && sanca(n, 25) {
                rez += 1;
            }
            ctb_rez.push(rez as u16);
        }
        InfoObrazu {
            sirka,
            vyska,
            log2_ctb,
            sirka_ctb,
            vyska_ctb,
            ctb_rez,
            ctb_dlazdica: vec![0; pocet],
            ctb_ts: (0..pocet as u32).collect(),
            cez_dlazdice: true,
            rezy,
            cb_qp_offset: cislo(n, -12, 12),
            cr_qp_offset: cislo(n, -12, 12),
            bitova_hlbka: 8,
        }
    }

    fn nahodny_pripad(semienko: u64) -> (Obraz, MapyFiltra, InfoObrazu) {
        let mut n = Nahoda::zo_semienka(semienko);
        let rozmer = |n: &mut Nahoda, najviac: i32| match cislo(n, 0, 4) {
            0 => 16 * cislo(n, 1, najviac / 16) as usize,
            1 => 8 * cislo(n, 1, najviac / 8) as usize,
            2 => cislo(n, 1, 12) as usize,
            _ => cislo(n, 9, najviac) as usize,
        };
        let (sirka, vyska) = (rozmer(&mut n, 96), rozmer(&mut n, 72));
        let mut obraz = Obraz::novy(sirka, vyska);
        for r in &mut obraz.roviny {
            vypln_rovinu(&mut n, r);
        }
        let mapy = nahodne_mapy(&mut n, sirka, vyska);
        let info = nahodne_info(&mut n, sirka, vyska);
        (obraz, mapy, info)
    }

    // ---------------------------------------------------------------------------------------------------------
    // Skúšky
    // ---------------------------------------------------------------------------------------------------------

    #[test]
    fn zhoda_s_rozhodcom_na_nahodnych_obrazoch() {
        let mut pok = Pokrytie::default();
        let mut zmenene = 0usize;
        for semienko in 0..600 {
            let (povodny, mapy, info) = nahodny_pripad(semienko);
            let ocakavane = rozhodca(&povodny, &mapy, &info, &mut pok);
            let mut obraz = povodny.clone();
            odblokuj(&mut obraz, &mapy, &info);
            for (c, (r, o)) in obraz.roviny.iter().zip(&ocakavane).enumerate() {
                for y in 0..r.vyska {
                    for x in 0..r.sirka {
                        assert_eq!(
                            i32::from(r.vzorka(x, y)),
                            o.daj((x, y)),
                            "semienko {semienko}, zložka {c}, vzorka ({x}, {y}), obraz {}×{}",
                            info.sirka,
                            info.vyska
                        );
                    }
                    // výplň za koncom riadka sa nemení
                    let zaciatok = y * r.krok;
                    assert_eq!(
                        r.data[zaciatok + r.sirka..zaciatok + r.krok],
                        povodny.roviny[c].data[zaciatok + r.sirka..zaciatok + r.krok]
                    );
                }
                zmenene += r.data.iter().zip(&povodny.roviny[c].data).filter(|(a, b)| a != b).count();
            }
        }
        // náhodné prípady pokryli všetky vetvy normy
        assert!(pok.bs.iter().all(|&n| n > 0), "pravidlá bS: {:?}", pok.bs);
        assert!(pok.silne > 100 && pok.slabe > 100, "{pok:?}");
        assert!(pok.slabe_p1 > 100 && pok.slabe_q1 > 100 && pok.velka_delta > 0, "{pok:?}");
        assert!(pok.farba > 100 && pok.bez_filtra > 10 && pok.orezanie > 0, "{pok:?}");
        assert!(zmenene > 10_000, "zmenených vzoriek {zmenene}");
    }

    /// Rozhodca a rýchla implementácia na všetkých malých rozmeroch (aj nie násobkoch 4, 8, 16) so všetkými
    /// hranami označenými (aj mimo mriežky a na okraji) a silným filtrovaním – nič sa nečíta ani nepíše mimo.
    #[test]
    fn male_rozmery_so_vsetkymi_hranami() {
        let mut pok = Pokrytie::default();
        for sirka in 1..=36 {
            for vyska in 1..=36 {
                let mut n = Nahoda::zo_semienka((sirka * 100 + vyska) as u64);
                let mut obraz = Obraz::novy(sirka, vyska);
                for r in &mut obraz.roviny {
                    vypln_rovinu(&mut n, r);
                }
                let mut mapy = MapyFiltra::nove(sirka, vyska);
                mapy.hrana_v.fill(2);
                mapy.hrana_h.fill(2);
                mapy.intra.fill(true);
                mapy.qp.fill(51);
                let info = jednoduche_info(
                    sirka,
                    vyska,
                    4,
                    None,
                    &[RezFiltra { beta_div2: 6, tc_div2: 6, ..RezFiltra::default() }],
                );
                let ocakavane = rozhodca(&obraz, &mapy, &info, &mut pok);
                odblokuj(&mut obraz, &mapy, &info);
                for (r, o) in obraz.roviny.iter().zip(&ocakavane) {
                    assert_eq!(RovinaR::z_roviny(r), *o, "obraz {sirka}×{vyska}");
                }
            }
        }
        assert!(pok.silne > 0 && pok.farba > 0);
    }

    /// Info s jedným rezom na CTB podľa `ctb_rez` (alebo všetko rez 0).
    fn jednoduche_info(
        sirka: usize,
        vyska: usize,
        log2_ctb: u32,
        ctb_rez: Option<Vec<u16>>,
        rezy: &[RezFiltra],
    ) -> InfoObrazu {
        let ctb = 1usize << log2_ctb;
        let (sirka_ctb, vyska_ctb) = (sirka.div_ceil(ctb), vyska.div_ceil(ctb));
        let pocet = sirka_ctb * vyska_ctb;
        InfoObrazu {
            sirka,
            vyska,
            log2_ctb,
            sirka_ctb,
            vyska_ctb,
            ctb_rez: ctb_rez.unwrap_or_else(|| vec![0; pocet]),
            ctb_dlazdica: vec![0; pocet],
            ctb_ts: (0..pocet as u32).collect(),
            cez_dlazdice: true,
            rezy: rezy.to_vec(),
            cb_qp_offset: 0,
            cr_qp_offset: 0,
            bitova_hlbka: 8,
        }
    }

    fn vypln(r: &mut Rovina, f: impl Fn(usize, usize) -> Vzorka) {
        for y in 0..r.vyska {
            for x in 0..r.sirka {
                r.data[y * r.krok + x] = f(x, y);
            }
        }
    }

    /// Silný filter ručne: p = 100, q = 110, QP 37 → β = 36, tC = tC′(39) = 5; dSam: 0 < 9, 0 < 4, 10 < 13.
    /// p2′ = 814 >> 3 = 101, p1′ = 412 >> 2 = 103, p0′ = 834 >> 3 = 104,
    /// q0′ = 854 >> 3 = 106, q1′ = 432 >> 2 = 108, q2′ = 874 >> 3 = 109.
    #[test]
    fn silny_filter_rucne() {
        let mut obraz = Obraz::novy(16, 16);
        vypln(&mut obraz.roviny[0], |x, _| if x < 8 { 100 } else { 110 });
        let mut mapy = MapyFiltra::nove(16, 16);
        for y4 in 0..4 {
            mapy.hrana_v[y4 * 4 + 2] = 2;
        }
        mapy.intra.fill(true);
        mapy.qp.fill(37);
        let info = jednoduche_info(16, 16, 4, None, &[RezFiltra::default()]);
        let farba = obraz.roviny[1].clone();
        odblokuj(&mut obraz, &mapy, &info);
        for y in 0..16 {
            assert_eq!(
                obraz.roviny[0].riadok(y),
                [100, 100, 100, 100, 100, 101, 103, 104, 106, 108, 109, 110, 110, 110, 110, 110]
            );
        }
        // x = 8 jasu nie je na mriežke 8 vzoriek farby
        assert_eq!(obraz.roviny[1], farba);
    }

    /// Slabý filter ručne (vodorovná hrana, bS = 1 z koeficientov): p = 60, q = 70, QP 30 → β = 22, tC = 2;
    /// |p0 − q0| = 10 ≥ 5 → slabý; dEp = dEq = 1 (0 < 4). Δ = 68 >> 4 = 4 → 2: p0′ = 62, q0′ = 68,
    /// Δp = (60 − 60 + 2) >> 1 = 1: p1′ = 61, Δq = (70 − 70 − 2) >> 1 = −1: q1′ = 69.
    #[test]
    fn slaby_filter_rucne() {
        for bez_p in [false, true] {
            let mut obraz = Obraz::novy(16, 16);
            vypln(&mut obraz.roviny[0], |_, y| if y < 8 { 60 } else { 70 });
            let mut mapy = MapyFiltra::nove(16, 16);
            for x4 in 0..4 {
                mapy.hrana_h[2 * 4 + x4] = 2;
            }
            for i in 8..16 {
                mapy.nenulove[i] = true;
            }
            for i in 0..8 {
                mapy.bez_filtra[i] = bez_p;
            }
            mapy.pohyb.fill(PohybBloku { mv: [[3, -2], [0, 0]], obraz: [Some(4), None] });
            mapy.qp.fill(30);
            let info = jednoduche_info(16, 16, 4, None, &[RezFiltra::default()]);
            odblokuj(&mut obraz, &mapy, &info);
            let stlpec: Vec<Vzorka> = (0..16).map(|y| obraz.roviny[0].vzorka(5, y)).collect();
            let ocakavane: [Vzorka; 16] = if bez_p {
                [60, 60, 60, 60, 60, 60, 60, 60, 68, 69, 70, 70, 70, 70, 70, 70]
            } else {
                [60, 60, 60, 60, 60, 60, 61, 62, 68, 69, 70, 70, 70, 70, 70, 70]
            };
            assert_eq!(stlpec, ocakavane, "bez_filtra strany P: {bez_p}");
            for x in 0..16 {
                assert_eq!(obraz.roviny[0].vzorka(x, 7), obraz.roviny[0].vzorka(5, 7));
            }
        }
        // bez koeficientov a s rovnakým pohybom je bS = 0
        let mut obraz = Obraz::novy(16, 16);
        vypln(&mut obraz.roviny[0], |_, y| if y < 8 { 60 } else { 70 });
        let povodny = obraz.clone();
        let mut mapy = MapyFiltra::nove(16, 16);
        mapy.hrana_h[8..12].fill(2);
        mapy.pohyb.fill(PohybBloku { mv: [[3, -2], [0, 0]], obraz: [Some(4), None] });
        mapy.qp.fill(30);
        odblokuj(&mut obraz, &mapy, &jednoduche_info(16, 16, 4, None, &[RezFiltra::default()]));
        assert_eq!(obraz, povodny);
    }

    /// Farba ručne (zvislá hrana x = 16 jasu = 8 farby, bS = 2, QP 40): Cb qPi = 40 → QpC 36, tC′(38) = 5;
    /// Cr qPi = 40 + 5 = 45 → QpC 39, tC′(41) = 6. Δ = (120 + 50 − 80 + 4) >> 3 = 11 → 5 (Cb), 6 (Cr).
    #[test]
    fn farba_rucne() {
        let mut obraz = Obraz::novy(32, 32);
        for c in 1..3 {
            vypln(&mut obraz.roviny[c], |x, _| if x < 8 { 50 } else { 80 });
        }
        let jas = obraz.roviny[0].clone();
        let mut mapy = MapyFiltra::nove(32, 32);
        for y4 in 0..8 {
            mapy.hrana_v[y4 * 8 + 4] = 1;
        }
        mapy.intra.fill(true);
        mapy.qp.fill(40);
        let mut info = jednoduche_info(32, 32, 4, None, &[RezFiltra::default()]);
        info.cr_qp_offset = 5;
        odblokuj(&mut obraz, &mapy, &info);
        assert_eq!(obraz.roviny[0], jas, "rovný jas sa nemení");
        for y in 0..16 {
            assert_eq!(obraz.roviny[1].riadok(y)[5..11], [50, 50, 55, 75, 80, 80]);
            assert_eq!(obraz.roviny[2].riadok(y)[5..11], [50, 50, 56, 74, 80, 80]);
        }
    }

    /// Farba na vodorovnej hrane medzi rezmi: posun tC sa berie z rezu so vzorkou q0,0. Rez Q (tc_div2 = 1):
    /// QpC 36 + 2 + 2 = 40 → tC 6, Δ = 6; s rezom P (tc_div2 = −6) by bolo tC′(26) = 1.
    #[test]
    fn farba_posun_z_rezu_q() {
        let mut obraz = Obraz::novy(32, 32);
        for c in 1..3 {
            vypln(&mut obraz.roviny[c], |_, y| if y < 8 { 50 } else { 80 });
        }
        let mut mapy = MapyFiltra::nove(32, 32);
        mapy.hrana_h[4 * 8..5 * 8].fill(2);
        mapy.intra.fill(true);
        mapy.qp.fill(40);
        let rezy = [
            RezFiltra { tc_div2: -6, beta_div2: -6, ..RezFiltra::default() },
            RezFiltra { adresa: 2, tc_div2: 1, ..RezFiltra::default() },
        ];
        let info = jednoduche_info(32, 32, 4, Some(vec![0, 0, 1, 1]), &rezy);
        odblokuj(&mut obraz, &mapy, &info);
        for c in 1..3 {
            let stlpec: Vec<Vzorka> = (5..11).map(|y| obraz.roviny[c].vzorka(3, y)).collect();
            assert_eq!(stlpec, [50, 50, 56, 74, 80, 80], "zložka {c}");
        }
    }

    /// Pravidlá sily hrany (8.7.2.4) na ručne zvolených prípadoch.
    #[test]
    fn sila_hrany_pravidla() {
        let jeden =
            |obraz: [Option<i32>; 2], mv0: [i16; 2], mv1: [i16; 2]| PohybBloku { mv: [mv0, mv1], obraz };
        let mut mapy = MapyFiltra::nove(8, 4);
        let mut bs = |a: PohybBloku, b: PohybBloku, hrana: u8, nenulove: bool, intra: bool| {
            mapy.pohyb[0] = a;
            mapy.pohyb[1] = b;
            mapy.nenulove[0] = nenulove;
            mapy.intra[1] = intra;
            let rychly = sila_hrany(&mapy, 0, 1, hrana);
            let (pomaly, _) = bs_r(&mapy, (3, 0), (4, 0), hrana == 2);
            assert_eq!(i32::from(rychly), pomaly);
            rychly
        };
        let a = jeden([Some(0), None], [0, 0], [0, 0]);
        assert_eq!(bs(a, a, 1, false, true), 2, "vnútrosnímkový blok");
        assert_eq!(bs(a, a, 2, true, false), 1, "hrana TU s koeficientmi");
        assert_eq!(bs(a, a, 1, true, false), 0, "koeficienty na hrane len PU sa nerátajú");
        assert_eq!(bs(a, jeden([Some(0), None], [3, -3], [9, 9]), 1, false, false), 0, "rozdiel 3");
        assert_eq!(bs(a, jeden([Some(0), None], [0, -4], [0, 0]), 1, false, false), 1, "rozdiel 4 v y");
        assert_eq!(bs(a, jeden([Some(0), None], [4, 0], [0, 0]), 1, false, false), 1, "rozdiel 4 v x");
        assert_eq!(
            bs(a, jeden([None, Some(0)], [7, 7], [1, -1]), 1, false, false),
            0,
            "ten istý obraz cez L1"
        );
        assert_eq!(bs(a, jeden([None, Some(1)], [0, 0], [0, 0]), 1, false, false), 1, "iný obraz");
        assert_eq!(
            bs(a, jeden([Some(0), Some(0)], [0, 0], [0, 0]), 1, false, false),
            1,
            "iný počet vektorov"
        );
        // dva rôzne obrazy, v druhom bloku vymenené zoznamy
        let d = jeden([Some(0), Some(1)], [10, 0], [-10, 0]);
        assert_eq!(bs(d, jeden([Some(1), Some(0)], [-7, 0], [13, 0]), 1, false, false), 0);
        assert_eq!(bs(d, jeden([Some(1), Some(0)], [-6, 0], [13, 0]), 1, false, false), 1);
        assert_eq!(bs(d, jeden([Some(0), Some(2)], [10, 0], [-10, 0]), 1, false, false), 1, "iné obrazy");
        assert_eq!(
            bs(d, jeden([Some(0), Some(0)], [10, 0], [-10, 0]), 1, false, false),
            1,
            "(0, 1) proti (0, 0)"
        );
        // dva vektory k tomu istému obrazu: bS = 1 len ak sa líšia obe párovania
        let r = jeden([Some(5), Some(5)], [0, 0], [20, 0]);
        assert_eq!(bs(r, jeden([Some(5), Some(5)], [20, 0], [0, 0]), 1, false, false), 0, "vymenené vektory");
        assert_eq!(bs(r, jeden([Some(5), Some(5)], [1, 1], [21, -1]), 1, false, false), 0);
        assert_eq!(bs(r, jeden([Some(5), Some(5)], [4, 0], [20, 0]), 1, false, false), 1);
        assert_eq!(bs(r, jeden([Some(5), Some(5)], [0, 0], [20, 4]), 1, false, false), 1);
        assert_eq!(
            bs(r, jeden([Some(5), Some(6)], [0, 0], [20, 0]), 1, false, false),
            1,
            "(5, 5) proti (5, 6)"
        );
    }

    /// Tabuľky β′, tC′ a QpC: pole rýchlej implementácie = prepis rozhodcu po úsekoch + kotvy z normy.
    #[test]
    fn tabulky_podla_normy() {
        for (q, &b) in BETA.iter().enumerate() {
            assert_eq!(i32::from(b), beta_r(q as i32), "β′({q})");
        }
        for (q, &t) in TC.iter().enumerate() {
            assert_eq!(i32::from(t), tc_r(q as i32), "tC′({q})");
        }
        for qpi in -20..70 {
            assert_eq!(qpc_z_qpi(qpi), qpc_r(qpi), "QpC({qpi})");
        }
        assert_eq!((BETA[15], BETA[16], BETA[28], BETA[29], BETA[51]), (0, 6, 18, 20, 64));
        assert_eq!((TC[17], TC[18], TC[27], TC[42], TC[47], TC[53]), (0, 1, 2, 7, 13, 24));
        assert_eq!([29, 30, 34, 35, 43, 44, 57].map(qpc_z_qpi), [29, 29, 33, 33, 37, 38, 51]);
    }
}
