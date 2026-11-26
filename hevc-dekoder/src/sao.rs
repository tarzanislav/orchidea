//! Adaptívny posun vzoriek (sample adaptive offset, SAO) – H.265, kap. 8.7.3.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`SaoCtb`] a [`sao`].
//!
//! Spracovanie ide po CTB a zložkách (`sao_ctb_zlozky` – jeden CTB jednej zložky, základ budúcej
//! súbežnosti po riadkoch CTB). Pre každý CTB sa raz zistí dostupnosť ôsmich susedných CTB (okraj
//! obrazu, rez, dlaždica); vnútro CTB potom ide po celých úsekoch riadkov bez kontrol hraníc
//! (`hrana_riadok`, `pasmo_riadok` – slučky bez vetvenia vhodné na automatickú vektorizáciu, neskôr
//! NEON) a len okrajové stĺpce CTB, ktorých susedia ležia v nedostupnom CTB, idú po jednej vzorke. CTB
//! s SaoTypeIdx = 0 sa len skopíruje po riadkoch. Vzorky blokov `bez_filtra` sa na konci vrátia na vstupnú
//! hodnotu (len ak ich CTB nejaké má).

use super::obraz::{Obraz, Rovina, Vzorka};
use crate::filtre::{InfoObrazu, MapyFiltra};

/// Parametre SAO jedného CTB pre zložky Y, Cb, Cr (už po zlúčení s ľavým či horným CTB).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaoCtb {
    /// SaoTypeIdx: 0 = vypnuté, 1 = pásmový posun (band offset), 2 = hranový posun (edge offset)
    pub typ: [u8; 3],
    /// SaoOffsetVal[1 … 4] so znamienkom (už posunuté o log2OffsetScale); SaoOffsetVal[0] = 0
    pub posuny: [[i16; 4]; 3],
    /// sao_band_position
    pub pasmo: [u8; 3],
    /// SaoEoClass: 0 vodorovne, 1 zvislo, 2 uhlopriečka 135°, 3 uhlopriečka 45°
    pub trieda: [u8; 3],
}

/// SAO celého obrazu: `vstup` je obraz po odblokovaní, do `vystup` (rovnaké rozmery) sa zapíše **každá** vzorka –
/// upravená podľa 8.7.3, alebo skopírovaná, ak sa nemení (SaoTypeIdx = 0, `bez_filtra`, sused mimo obrazu,
/// v inom reze či dlaždici, cez ktorú sa nefiltruje).
///
/// `ctb[ry * sirka_ctb + rx]` sú parametre CTB. Hranica rezu: sused v inom reze sa nepoužije, ak rez vzorky,
/// ktorá je v poradí dekódovania neskôr, má slice_loop_filter_across_slices_enabled_flag = 0 (poradie CTB je
/// `info.ctb_ts`); hranica dlaždice podľa `info.cez_dlazdice`.
pub fn sao(vstup: &Obraz, vystup: &mut Obraz, ctb: &[SaoCtb], mapy: &MapyFiltra, info: &InfoObrazu) {
    // Zmluva hovorí o rovnakých rozmeroch; inak sa výstup najprv prispôsobí (bez pádu, výsledok je rovnaký).
    let rovnake = (0..3).all(|c| {
        let (a, b) = (&vstup.roviny[c], &vystup.roviny[c]);
        a.sirka == b.sirka && a.vyska == b.vyska && b.krok >= b.sirka && b.data.len() >= b.krok * b.vyska
    });
    if !rovnake {
        vystup.clone_from(vstup);
    }
    vystup.bitova_hlbka = vstup.bitova_hlbka;
    vystup.bitova_hlbka_farby = vstup.bitova_hlbka_farby;

    let [a, b, c] = &mut vystup.roviny;
    let mut ciele = [
        CielSao { data: &mut a.data, krok: a.krok, od_riadka: 0 },
        CielSao { data: &mut b.data, krok: b.krok, od_riadka: 0 },
        CielSao { data: &mut c.data, krok: c.krok, od_riadka: 0 },
    ];
    for ry in 0..info.vyska_ctb {
        sao_riadok(vstup, &mut ciele, ctb, mapy, info, ry);
    }
}

/// Cieľ zápisu SAO jednej zložky: `data` začína riadkom `od_riadka` roviny, riadky sú po `krok` prvkoch
/// (celá rovina, alebo len pás riadkov CTB zverejňovaného obrazu).
#[derive(Debug)]
pub struct CielSao<'a> {
    pub data: &'a mut [Vzorka],
    pub krok: usize,
    pub od_riadka: usize,
}

/// SAO jedného riadka CTB `ry` (ako [`sao`]) – zapíše každú vzorku tohto riadka CTB všetkých troch zložiek do
/// `ciele`. Potrebuje odblokované vzorky riadka CTB a susedných riadkov vzoriek nad ním a pod ním.
pub fn sao_riadok(
    vstup: &Obraz,
    ciele: &mut [CielSao<'_>; 3],
    ctb: &[SaoCtb],
    mapy: &MapyFiltra,
    info: &InfoObrazu,
    ry: usize,
) {
    let velkost = 1usize << info.log2_ctb;
    let (sirka_y, vyska_y) = (vstup.sirka(), vstup.vyska());
    {
        for rx in 0..info.sirka_ctb {
            let p = &ctb[ry * info.sirka_ctb + rx];
            // dostupnosť susedných CTB podľa rezov a dlaždíc – len ak ju niektorá zložka potrebuje
            let dost_ctb = if p.typ.contains(&2) { dostupnost_ctb(info, rx, ry) } else { [[false; 3]; 3] };
            // bloky bez_filtra stačí hľadať, ak sa niektorá zložka CTB mení
            let (lx0, ly0) = (rx * velkost, ry * velkost);
            let (lx1, ly1) = ((lx0 + velkost).min(sirka_y), (ly0 + velkost).min(vyska_y));
            let bez = p.typ.iter().any(|&t| t == 1 || t == 2)
                && lx0 < lx1
                && ly0 < ly1
                && ctb_ma_bez_filtra(mapy, lx0, ly0, lx1, ly1);
            for (c, ciel) in ciele.iter_mut().enumerate() {
                // 4:2:0: CTB farby má polovičné rozmery (nCtbSw = CtbSizeY / SubWidthC)
                let podvz = u32::from(c > 0);
                let rovina = &vstup.roviny[c];
                let vel = velkost >> podvz;
                let (x0, y0) = (rx * vel, ry * vel);
                let o = Oblast { x0, y0, x1: (x0 + vel).min(rovina.sirka), y1: (y0 + vel).min(rovina.vyska) };
                if o.x0 >= o.x1 || o.y0 >= o.y1 {
                    continue;
                }
                let mut dost = dost_ctb;
                if p.typ[c] == 2 {
                    // okraj obrazu v súradniciach roviny
                    for riadok in &mut dost {
                        riadok[0] &= o.x0 > 0;
                        riadok[2] &= o.x1 < rovina.sirka;
                    }
                    dost[0] = dost[0].map(|d| d && o.y0 > 0);
                    dost[2] = dost[2].map(|d| d && o.y1 < rovina.vyska);
                }
                let uloha = UlohaCtb {
                    oblast: o,
                    typ: p.typ[c],
                    posuny: p.posuny[c],
                    pasmo: p.pasmo[c],
                    trieda: p.trieda[c],
                    dost,
                    bitova_hlbka: if c == 0 { vstup.bitova_hlbka } else { vstup.bitova_hlbka_farby },
                    bez_filtra: if bez { Some((mapy, 2 - podvz)) } else { None },
                };
                let krok = ciel.krok;
                let od = (y0 - ciel.od_riadka) * krok + x0;
                sao_ctb_zlozky(rovina, &mut ciel.data[od..], krok, &uloha);
            }
        }
    }
}

/// Obdĺžnik vzoriek jedného CTB v súradniciach roviny: stĺpce `x0 … x1 − 1`, riadky `y0 … y1 − 1` (už orezaný
/// okrajom obrazu, neprázdny).
#[derive(Debug, Clone, Copy)]
struct Oblast {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

/// SAO jedného CTB jednej zložky.
#[derive(Debug, Clone, Copy)]
struct UlohaCtb<'a> {
    oblast: Oblast,
    typ: u8,
    posuny: [i16; 4],
    pasmo: u8,
    trieda: u8,
    /// dostupnosť susedného CTB `[dy + 1][dx + 1]` pre hranový posun (vrátane okraja obrazu), stred je `true`
    dost: [[bool; 3]; 3],
    bitova_hlbka: u32,
    /// mapy a log2 veľkosti bloku 4×4 jasu v rovine (jas 2, farba 1), ak má CTB nejaký blok `bez_filtra`
    bez_filtra: Option<(&'a MapyFiltra, u32)>,
}

/// Posuny susedov (`hPos[k]`, `vPos[k]`) pre SaoEoClass 0 … 3 (8.7.3.2): `SMERY[trieda][k] = (dx, dy)`.
const SMERY: [[(isize, isize); 2]; 4] =
    [[(-1, 0), (1, 0)], [(0, -1), (0, 1)], [(-1, -1), (1, 1)], [(1, -1), (-1, 1)]];

/// Dostupnosť ôsmich susedných CTB pre hranový posun (8.7.3.2), `[dy + 1][dx + 1]`; okraj obrazu v rovine
/// dopĺňa volajúci.
///
/// Sused nie je dostupný, ak CTB neexistuje, ak je v inom reze (iné SliceAddrRs) a rez toho z dvojice CTB,
/// ktorý je v poradí dekódovania (CtbAddrRsToTs – medzi rôznymi CTB rovnaké poradie ako MinTbAddrZs) neskôr,
/// má slice_loop_filter_across_slices_enabled_flag = 0, alebo ak je v inej dlaždici pri
/// loop_filter_across_tiles_enabled_flag = 0.
fn dostupnost_ctb(info: &InfoObrazu, rx: usize, ry: usize) -> [[bool; 3]; 3] {
    let i = ry * info.sirka_ctb + rx;
    let rez = &info.rezy[info.ctb_rez[i] as usize];
    let mut d = [[false; 3]; 3];
    for (dy, riadok) in d.iter_mut().enumerate() {
        for (dx, dostupny) in riadok.iter_mut().enumerate() {
            let (nx, ny) = ((rx + dx).wrapping_sub(1), (ry + dy).wrapping_sub(1));
            if nx >= info.sirka_ctb || ny >= info.vyska_ctb {
                continue;
            }
            let j = ny * info.sirka_ctb + nx;
            let rez_j = &info.rezy[info.ctb_rez[j] as usize];
            let mut ok = true;
            if rez_j.adresa != rez.adresa {
                let neskorsi = if info.ctb_ts[j] > info.ctb_ts[i] { rez_j } else { rez };
                ok &= neskorsi.cez_rezy;
            }
            if !info.cez_dlazdice && info.ctb_dlazdica[j] != info.ctb_dlazdica[i] {
                ok = false;
            }
            *dostupny = ok;
        }
    }
    d
}

/// Má CTB s jasom `lx0 … lx1 − 1` × `ly0 … ly1 − 1` aspoň jeden blok 4×4 `bez_filtra`?
fn ctb_ma_bez_filtra(mapy: &MapyFiltra, lx0: usize, ly0: usize, lx1: usize, ly1: usize) -> bool {
    let (bx0, bx1) = (lx0 >> 2, (lx1 - 1) >> 2);
    ((ly0 >> 2)..=((ly1 - 1) >> 2)).any(|by| {
        let z = by * mapy.sirka4;
        mapy.bez_filtra[z + bx0..=z + bx1].iter().any(|&b| b)
    })
}

/// SAO jedného CTB jednej zložky (8.7.3.2). `ciel` začína vzorkou (x0, y0) oblasti, riadok `j` oblasti je na
/// `ciel[j * krok ..]`; zapíše sa každá vzorka oblasti.
fn sao_ctb_zlozky(vstup: &Rovina, ciel: &mut [Vzorka], krok: usize, u: &UlohaCtb) {
    let o = u.oblast;
    let (w, h) = (o.x1 - o.x0, o.y1 - o.y0);
    match u.typ {
        1 => {
            let t = TabulkaPasma::nova(u.posuny, u.pasmo, u.bitova_hlbka);
            for j in 0..h {
                let z = (o.y0 + j) * vstup.krok + o.x0;
                pasmo_riadok(&mut ciel[j * krok..j * krok + w], &vstup.data[z..z + w], &t);
            }
        }
        2 => hrana_ctb(vstup, ciel, krok, u),
        _ => {
            // SaoTypeIdx = 0: vzorky sa nemenia
            for j in 0..h {
                let z = (o.y0 + j) * vstup.krok + o.x0;
                ciel[j * krok..j * krok + w].copy_from_slice(&vstup.data[z..z + w]);
            }
            return;
        }
    }
    if let Some((mapy, log2_bloku)) = u.bez_filtra {
        obnov_bez_filtra(vstup, ciel, krok, o, mapy, log2_bloku);
    }
}

/// Bloky s pcm_loop_filter_disabled_flag a pcm_flag alebo s cu_transquant_bypass_flag sa nemenia (8.7.3.2):
/// ich vzorky sa vrátia na vstupnú hodnotu. Vzorka farby (x, y) patrí bloku jasu so vzorkou (2x, 2y).
fn obnov_bez_filtra(
    vstup: &Rovina,
    ciel: &mut [Vzorka],
    krok: usize,
    o: Oblast,
    mapy: &MapyFiltra,
    log2_bloku: u32,
) {
    let b = log2_bloku;
    for by in (o.y0 >> b)..=((o.y1 - 1) >> b) {
        let (ya, yb) = ((by << b).max(o.y0), ((by + 1) << b).min(o.y1));
        let (bx0, bx1) = (o.x0 >> b, (o.x1 - 1) >> b);
        let zm = by * mapy.sirka4;
        for (bx, _) in (bx0..=bx1).zip(&mapy.bez_filtra[zm + bx0..=zm + bx1]).filter(|&(_, &bez)| bez) {
            let (xa, xb) = ((bx << b).max(o.x0), ((bx + 1) << b).min(o.x1));
            for y in ya..yb {
                let d = (y - o.y0) * krok;
                let z = y * vstup.krok;
                ciel[d + xa - o.x0..d + xb - o.x0].copy_from_slice(&vstup.data[z + xa..z + xb]);
            }
        }
    }
}

/// Pásmový posun jednej zložky CTB (8.7.3.2, SaoTypeIdx = 1).
#[derive(Debug, Clone, Copy)]
struct TabulkaPasma {
    /// bandShift = bitDepth − 5
    posun: u32,
    /// saoLeftClass = sao_band_position
    pasmo: u16,
    /// SaoOffsetVal[1 … 4]
    posuny: [i16; 4],
    /// (1 << bitDepth) − 1
    max: i16,
}

impl TabulkaPasma {
    fn nova(posuny: [i16; 4], pasmo: u8, bitova_hlbka: u32) -> TabulkaPasma {
        TabulkaPasma {
            posun: bitova_hlbka.saturating_sub(5),
            pasmo: u16::from(pasmo),
            posuny,
            max: ((1i32 << bitova_hlbka) - 1) as i16,
        }
    }
}

/// Pásmový posun úseku riadka. bandTable[(k + saoLeftClass) & 31] = k + 1 pre k = 0 … 3, teda pásmo
/// `p = vzorka >> bandShift` má bandIdx = k + 1 práve vtedy, keď k = (p − saoLeftClass) & 31 < 4 (inak 0).
///
/// Posun sa vyberá nezávislými maskami spojenými `|` (nie čítaním z tabuľky ani reťazou `if … else if`, ktorú
/// LLVM pri opt-level 2 zmení na vetvenie) – slučka sa vektorizuje (overené: x86-64 SSE2, AArch64 Cortex-A53
/// NEON so šírkou 16). Súčet so saturáciou v i16 dá po orezaní rovnaký výsledok ako Clip3 v širšom type.
#[inline]
fn pasmo_riadok(ciel: &mut [Vzorka], stred: &[Vzorka], t: &TabulkaPasma) {
    let ([p1, p2, p3, p4], posun_pasma, pasmo, max) = (t.posuny, t.posun, t.pasmo, t.max);
    for (o, &c) in ciel.iter_mut().zip(stred) {
        let k = (u16::from(c) >> posun_pasma).wrapping_sub(pasmo) & 31;
        let posun = (if k == 0 { p1 } else { 0 })
            | (if k == 1 { p2 } else { 0 })
            | (if k == 2 { p3 } else { 0 })
            | (if k == 3 { p4 } else { 0 });
        *o = (c as i16).saturating_add(posun).max(0).min(max) as Vzorka;
    }
}

/// Posuny hranového posunu podľa edgeIdx pred preradením: `t[2 + Sign(c − a) + Sign(c − b)]`.
/// Preradenie 8.7.3.2 (edgeIdx 0 → 1, 1 → 2, 2 → 0, 3 a 4 ostávajú) je už v tabuľke:
/// `t = [SaoOffsetVal[1], SaoOffsetVal[2], 0, SaoOffsetVal[3], SaoOffsetVal[4]]`.
fn tabulka_hrany(posuny: [i16; 4]) -> [i16; 5] {
    [posuny[0], posuny[1], 0, posuny[2], posuny[3]]
}

/// Hranový posun jednej vzorky `c` so susedmi `a`, `b` (oba dostupní); `t` z [`tabulka_hrany`].
/// Výber posunu maskami ako v [`pasmo_riadok`] – bez vetvenia, vektorizuje sa.
#[inline(always)]
fn hrana(c: Vzorka, a: Vzorka, b: Vzorka, t: [i16; 5], max: i16) -> Vzorka {
    // edgeIdx − 2 = Sign(c − a) + Sign(c − b)
    let z = i8::from(c > a) - i8::from(c < a) + i8::from(c > b) - i8::from(c < b);
    let posun = (if z == -2 { t[0] } else { 0 })
        | (if z == -1 { t[1] } else { 0 })
        | (if z == 1 { t[3] } else { 0 })
        | (if z == 2 { t[4] } else { 0 });
    (c as i16).saturating_add(posun).max(0).min(max) as Vzorka
}

/// Hranový posun úseku riadka: vzorky `stred[i]` so susedmi `a[i]` a `b[i]` (už posunutými podľa triedy),
/// všetci susedia dostupní. Bez kontrol hraníc, bez vetvenia.
#[inline]
fn hrana_riadok(ciel: &mut [Vzorka], stred: &[Vzorka], a: &[Vzorka], b: &[Vzorka], t: [i16; 5], max: i16) {
    let n = ciel.len();
    let (stred, a, b) = (&stred[..n], &a[..n], &b[..n]);
    for (((o, &c), &a), &b) in ciel.iter_mut().zip(stred).zip(a).zip(b) {
        *o = hrana(c, a, b, t, max);
    }
}

/// Poloha súradnice voči intervalu `od … po − 1`: 0 pred ním, 1 v ňom, 2 za ním (index do `dost`).
#[inline]
fn poloha(v: isize, od: usize, po: usize) -> usize {
    if v < od as isize {
        0
    } else if v >= po as isize {
        2
    } else {
        1
    }
}

/// Hranový posun jednej vzorky okraja CTB s kontrolou dostupnosti oboch susedov (8.7.3.2): ak sused leží
/// v nedostupnom CTB alebo mimo obrazu, vzorka sa nemení.
#[inline]
fn hrana_okraj(vstup: &Rovina, x: usize, y: usize, u: &UlohaCtb, t: [i16; 5], max: i16) -> Vzorka {
    let o = u.oblast;
    let c = vstup.vzorka(x, y);
    let mut s = [c; 2];
    for (k, &(dx, dy)) in SMERY[usize::from(u.trieda & 3)].iter().enumerate() {
        let (nx, ny) = (x as isize + dx, y as isize + dy);
        if !u.dost[poloha(ny, o.y0, o.y1)][poloha(nx, o.x0, o.x1)] {
            return c;
        }
        s[k] = vstup.vzorka(nx as usize, ny as usize);
    }
    hrana(c, s[0], s[1], t, max)
}

/// Hranový posun jedného CTB jednej zložky (8.7.3.2, SaoTypeIdx = 2).
///
/// Pre každý riadok: ak sú všetky CTB, do ktorých siahajú susedia riadka, dostupné, celý riadok ide naraz
/// ([`hrana_riadok`]). Inak vnútorné stĺpce (susedia len v CTB nad, v ňom alebo pod ním) naraz alebo kópiou
/// a dva okrajové stĺpce po jednej vzorke ([`hrana_okraj`]).
fn hrana_ctb(vstup: &Rovina, ciel: &mut [Vzorka], krok: usize, u: &UlohaCtb) {
    let o = u.oblast;
    let (w, h) = (o.x1 - o.x0, o.y1 - o.y0);
    let t = tabulka_hrany(u.posuny);
    let max = ((1i32 << u.bitova_hlbka) - 1) as i16;
    let [(ha, va), (hb, vb)] = SMERY[usize::from(u.trieda & 3)];
    let kv = vstup.krok;
    // index stĺpca do `dost` pre dx = −1, 0, 1
    let sx = |dx: isize| (dx + 1) as usize;
    for j in 0..h {
        let y = o.y0 + j;
        let (ya, yb) = (y as isize + va, y as isize + vb);
        let (pa, pb) = (poloha(ya, o.y0, o.y1), poloha(yb, o.y0, o.y1));
        let riadok = &mut ciel[j * krok..j * krok + w];
        let zs = y * kv;
        let cely = u.dost[pa][1] && u.dost[pb][1] && u.dost[pa][sx(ha)] && u.dost[pb][sx(hb)];
        if cely {
            // susedia na x0 − 1 a x1 existujú (dostupnosť zahŕňa okraj obrazu)
            let za = (ya as usize * kv + o.x0).wrapping_add_signed(ha);
            let zb = (yb as usize * kv + o.x0).wrapping_add_signed(hb);
            hrana_riadok(riadok, &vstup.data[zs + o.x0..], &vstup.data[za..], &vstup.data[zb..], t, max);
            continue;
        }
        if w > 2 {
            let (xs, n) = (o.x0 + 1, w - 2);
            let vnutro = &mut riadok[1..1 + n];
            if u.dost[pa][1] && u.dost[pb][1] {
                let za = (ya as usize * kv + xs).wrapping_add_signed(ha);
                let zb = (yb as usize * kv + xs).wrapping_add_signed(hb);
                hrana_riadok(vnutro, &vstup.data[zs + xs..], &vstup.data[za..], &vstup.data[zb..], t, max);
            } else {
                vnutro.copy_from_slice(&vstup.data[zs + xs..zs + xs + n]);
            }
        }
        riadok[0] = hrana_okraj(vstup, o.x0, y, u, t, max);
        if w > 1 {
            riadok[w - 1] = hrana_okraj(vstup, o.x1 - 1, y, u, t, max);
        }
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::filtre::RezFiltra;
    use orchidea_jadro::nahoda::Nahoda;

    // ----------------------------------------------------------------------------------------------------
    // Rozhodca: priamy, pomalý prepis 8.7.3 po jednotlivých vzorkách, bez akýchkoľvek skratiek.
    // ----------------------------------------------------------------------------------------------------

    fn clip3(x: i32, y: i32, z: i32) -> i32 {
        if z < x {
            x
        } else if z > y {
            y
        } else {
            z
        }
    }

    fn sign(x: i32) -> i32 {
        if x > 0 {
            1
        } else if x == 0 {
            0
        } else {
            -1
        }
    }

    /// MinTbAddrZs (6.5.2) s jemnosťou vzorky jasu: CtbAddrRsToTs CTB a z-poradie vzorky v CTB. Medzi
    /// vzorkami rôznych CTB (iba tie sa porovnávajú – rezy obsahujú celé CTB) dáva rovnaké poradie ako
    /// MinTbAddrZs.
    fn min_tb_addr_zs(info: &InfoObrazu, x: usize, y: usize) -> u64 {
        let ctb_addr_rs = (y >> info.log2_ctb) * info.sirka_ctb + (x >> info.log2_ctb);
        let maska = (1usize << info.log2_ctb) - 1;
        let mut z = 0u64;
        for b in 0..info.log2_ctb {
            z |= (((x & maska) >> b) as u64 & 1) << (2 * b);
            z |= (((y & maska) >> b) as u64 & 1) << (2 * b + 1);
        }
        (u64::from(info.ctb_ts[ctb_addr_rs]) << (2 * info.log2_ctb)) | z
    }

    /// TileId dlaždice, ktorá obsahuje vzorku jasu (x, y).
    fn tile_id(info: &InfoObrazu, x: usize, y: usize) -> u16 {
        info.ctb_dlazdica[(y >> info.log2_ctb) * info.sirka_ctb + (x >> info.log2_ctb)]
    }

    /// 8.7.3.1 a 8.7.3.2 doslova: saoPicture = recPicture, potom pre každý CTB a zložku každá vzorka zvlášť.
    fn rozhodca(rec_picture: &Obraz, ctb: &[SaoCtb], mapy: &MapyFiltra, info: &InfoObrazu) -> Obraz {
        let mut sao_picture = rec_picture.clone();
        let bit_depth = rec_picture.bitova_hlbka as i32;
        for c_idx in 0..3 {
            let (sub_width_c, sub_height_c) = if c_idx == 0 { (1, 1) } else { (2, 2) };
            let n_ctb_sw = (1usize << info.log2_ctb) / sub_width_c;
            let n_ctb_sh = (1usize << info.log2_ctb) / sub_height_c;
            let rec = &rec_picture.roviny[c_idx];
            for ry in 0..info.vyska_ctb {
                for rx in 0..info.sirka_ctb {
                    let p = &ctb[ry * info.sirka_ctb + rx];
                    let sao_type_idx = p.typ[c_idx];
                    let mut sao_offset_val = [0i32; 5];
                    for i in 0..4 {
                        sao_offset_val[i + 1] = i32::from(p.posuny[c_idx][i]);
                    }
                    let (x_ctb, y_ctb) = (rx * n_ctb_sw, ry * n_ctb_sh);
                    for i in 0..n_ctb_sw {
                        for j in 0..n_ctb_sh {
                            let (x_si, y_sj) = (x_ctb + i, y_ctb + j);
                            if x_si >= rec.sirka || y_sj >= rec.vyska {
                                continue; // CTB presahuje okraj obrazu
                            }
                            // vzorka jasu tej istej kódovacej jednotky (pcm_flag, cu_transquant_bypass_flag)
                            let (x_yi, y_yj) = (x_si * sub_width_c, y_sj * sub_height_c);
                            if mapy.bez_filtra[mapy.index(x_yi, y_yj)] || sao_type_idx == 0 {
                                continue;
                            }
                            let r = i32::from(rec.vzorka(x_si, y_sj));
                            let nova = if sao_type_idx == 2 {
                                let (h_pos, v_pos): ([isize; 2], [isize; 2]) = match p.trieda[c_idx] {
                                    0 => ([-1, 1], [0, 0]),
                                    1 => ([0, 0], [-1, 1]),
                                    2 => ([-1, 1], [-1, 1]),
                                    _ => ([1, -1], [-1, 1]),
                                };
                                let mut nulovy = false;
                                let mut sused = [0i32; 2];
                                for k in 0..2 {
                                    let x_k = x_si as isize + h_pos[k];
                                    let y_k = y_sj as isize + v_pos[k];
                                    if x_k < 0
                                        || y_k < 0
                                        || x_k >= rec.sirka as isize
                                        || y_k >= rec.vyska as isize
                                    {
                                        nulovy = true; // mimo obrazu
                                        break;
                                    }
                                    let (x_k, y_k) = (x_k as usize, y_k as usize);
                                    let (x_ky, y_ky) = (x_k * sub_width_c, y_k * sub_height_c);
                                    let rez_vzorky = info.rez_vzorky(x_yi, y_yj);
                                    let rez_suseda = info.rez_vzorky(x_ky, y_ky);
                                    if rez_vzorky.adresa != rez_suseda.adresa {
                                        let z_vzorky = min_tb_addr_zs(info, x_yi, y_yj);
                                        let z_suseda = min_tb_addr_zs(info, x_ky, y_ky);
                                        if (z_suseda < z_vzorky && !rez_vzorky.cez_rezy)
                                            || (z_vzorky < z_suseda && !rez_suseda.cez_rezy)
                                        {
                                            nulovy = true;
                                        }
                                    }
                                    if !info.cez_dlazdice
                                        && tile_id(info, x_ky, y_ky) != tile_id(info, x_yi, y_yj)
                                    {
                                        nulovy = true;
                                    }
                                    sused[k] = i32::from(rec.vzorka(x_k, y_k));
                                }
                                let mut edge_idx = 0;
                                if !nulovy {
                                    edge_idx = 2 + sign(r - sused[0]) + sign(r - sused[1]);
                                    if edge_idx == 0 || edge_idx == 1 || edge_idx == 2 {
                                        edge_idx = if edge_idx == 2 { 0 } else { edge_idx + 1 };
                                    }
                                }
                                clip3(0, (1 << bit_depth) - 1, r + sao_offset_val[edge_idx as usize])
                            } else {
                                let band_shift = bit_depth - 5;
                                let sao_left_class = usize::from(p.pasmo[c_idx]);
                                let mut band_table = [0usize; 32];
                                for k in 0..4 {
                                    band_table[(k + sao_left_class) & 31] = k + 1;
                                }
                                let band_idx = band_table[(r >> band_shift) as usize];
                                clip3(0, (1 << bit_depth) - 1, r + sao_offset_val[band_idx])
                            };
                            let krok = sao_picture.roviny[c_idx].krok;
                            sao_picture.roviny[c_idx].data[y_sj * krok + x_si] = nova as Vzorka;
                        }
                    }
                }
            }
        }
        sao_picture
    }

    // ----------------------------------------------------------------------------------------------------
    // Pomôcky
    // ----------------------------------------------------------------------------------------------------

    fn nastav(r: &mut Rovina, x: usize, y: usize, v: Vzorka) {
        r.data[y * r.krok + x] = v;
    }

    /// Jeden rez, jedna dlaždica, poradie dekódovania = raster.
    fn jednoduche_info(sirka: usize, vyska: usize, log2_ctb: u32) -> InfoObrazu {
        let vel = 1usize << log2_ctb;
        let (sc, vc) = (sirka.div_ceil(vel), vyska.div_ceil(vel));
        InfoObrazu {
            sirka,
            vyska,
            log2_ctb,
            sirka_ctb: sc,
            vyska_ctb: vc,
            ctb_rez: vec![0; sc * vc],
            ctb_dlazdica: vec![0; sc * vc],
            ctb_ts: (0..(sc * vc) as u32).collect(),
            cez_dlazdice: true,
            rezy: vec![RezFiltra { adresa: 0, cez_rezy: true, ..Default::default() }],
            bitova_hlbka: 8,
            ..Default::default()
        }
    }

    /// Výstup vyplnený odpadom – overí, že `sao` zapíše každú vzorku.
    fn odpad(sirka: usize, vyska: usize, semienko: u64) -> Obraz {
        let mut n = Nahoda::zo_semienka(semienko);
        let mut o = Obraz::novy(sirka, vyska);
        for r in &mut o.roviny {
            for v in &mut r.data {
                *v = n.dalsie() as Vzorka;
            }
        }
        o
    }

    /// Rýchla implementácia na výstupe vyplnenom odpadom.
    fn spusti(vstup: &Obraz, ctb: &[SaoCtb], mapy: &MapyFiltra, info: &InfoObrazu) -> Obraz {
        let mut vystup = odpad(vstup.sirka(), vstup.vyska(), 0xDEAD);
        sao(vstup, &mut vystup, ctb, mapy, info);
        vystup
    }

    /// Porovná platné vzorky všetkých zložiek; pri rozdiele vypíše prvú odlišnú vzorku.
    fn porovnaj(vysledok: &Obraz, ocakavany: &Obraz, vstup: &Obraz, popis: &str) {
        for c in 0..3 {
            let (r, o) = (&vysledok.roviny[c], &ocakavany.roviny[c]);
            for y in 0..o.vyska {
                if let Some(x) = (0..o.sirka).find(|&x| r.vzorka(x, y) != o.vzorka(x, y)) {
                    panic!(
                        "{popis}: zložka {c}, vzorka ({x}, {y}): rýchla {} ≠ rozhodca {} (vstup {})",
                        r.vzorka(x, y),
                        o.vzorka(x, y),
                        vstup.roviny[c].vzorka(x, y)
                    );
                }
            }
        }
    }

    fn over(vstup: &Obraz, ctb: &[SaoCtb], mapy: &MapyFiltra, info: &InfoObrazu, popis: &str) -> Obraz {
        let vysledok = spusti(vstup, ctb, mapy, info);
        porovnaj(&vysledok, &rozhodca(vstup, ctb, mapy, info), vstup, popis);
        vysledok
    }

    // ----------------------------------------------------------------------------------------------------
    // Náhodné porovnanie s rozhodcom
    // ----------------------------------------------------------------------------------------------------

    /// Hranice dlaždíc v CTB: 0, náhodné vnútorné (s pravdepodobnosťou `percent`), `pocet`.
    fn nahodne_hranice(n: &mut Nahoda, pocet: usize, percent: u64) -> Vec<usize> {
        let mut h = vec![0];
        h.extend((1..pocet).filter(|_| n.mensie_ako(100) < percent));
        h.push(pocet);
        h
    }

    /// Náhodné dlaždice (TileId a CtbAddrRsToTs podľa 6.5.1) a rezy: úseky po sebe idúcich CTB v poradí
    /// dekódovania; závislý úsek preberá SliceAddrRs aj slice_loop_filter_across_slices_enabled_flag.
    fn nahodne_info(n: &mut Nahoda, sirka: usize, vyska: usize, log2_ctb: u32) -> InfoObrazu {
        let mut info = jednoduche_info(sirka, vyska, log2_ctb);
        let (sc, vc) = (info.sirka_ctb, info.vyska_ctb);
        let pocet = sc * vc;
        let percent = [0, 0, 30, 70][n.mensie_ako(4) as usize];
        let (stlpce, riadky) = (nahodne_hranice(n, sc, percent), nahodne_hranice(n, vc, percent));
        let mut ts_na_rs = vec![0usize; pocet];
        let (mut ts, mut tile_id) = (0usize, 0u16);
        for r in riadky.windows(2) {
            for s in stlpce.windows(2) {
                for y in r[0]..r[1] {
                    for x in s[0]..s[1] {
                        info.ctb_dlazdica[y * sc + x] = tile_id;
                        info.ctb_ts[y * sc + x] = ts as u32;
                        ts_na_rs[ts] = y * sc + x;
                        ts += 1;
                    }
                }
                tile_id += 1;
            }
        }
        info.cez_dlazdice = n.mensie_ako(2) == 0;
        let max_dlzka = [pocet, 1, 3, 8][n.mensie_ako(4) as usize] as u64;
        info.rezy.clear();
        let (mut adresa, mut cez, mut ts) = (0u32, true, 0usize);
        while ts < pocet {
            let dlzka = 1 + n.mensie_ako(max_dlzka) as usize;
            if ts == 0 || n.mensie_ako(3) != 0 {
                adresa = ts_na_rs[ts] as u32; // nezávislý segment: nový rez
                cez = n.mensie_ako(2) == 0;
            }
            for &rs in &ts_na_rs[ts..(ts + dlzka).min(pocet)] {
                info.ctb_rez[rs] = info.rezy.len() as u16;
            }
            info.rezy.push(RezFiltra { adresa, cez_rezy: cez, ..Default::default() });
            ts += dlzka;
        }
        info
    }

    fn nahodna_rovina(n: &mut Nahoda, r: &mut Rovina) {
        let rezim = n.mensie_ako(5);
        let zaklad = n.mensie_ako(256) as i32;
        for y in 0..r.vyska {
            for x in 0..r.sirka {
                let v = match rezim {
                    0 => n.mensie_ako(256) as i32,
                    1 => zaklad + n.mensie_ako(3) as i32 - 1, // veľa rovnakých susedov (Sign = 0)
                    2 => [0, 1, 2, 3, 252, 253, 254, 255][n.mensie_ako(8) as usize], // orezanie Clip3
                    3 => ((x * 3 + y * 2) as i32 + n.mensie_ako(4) as i32) & 255, // plynulý priebeh
                    _ => zaklad + n.mensie_ako(2) as i32 * 5, // dve hodnoty, veľké plochy rovnakých
                };
                nastav(r, x, y, v.clamp(0, 255) as Vzorka);
            }
        }
    }

    fn nahodne_sao(n: &mut Nahoda) -> SaoCtb {
        let mut p = SaoCtb::default();
        for c in 0..3 {
            p.typ[c] = [0, 1, 2, 2][n.mensie_ako(4) as usize];
            for posun in &mut p.posuny[c] {
                *posun = match n.mensie_ako(25) {
                    0 => [i16::MIN, i16::MAX, -300, 300][n.mensie_ako(4) as usize],
                    _ => n.mensie_ako(15) as i16 - 7,
                };
            }
            p.pasmo[c] = n.mensie_ako(32) as u8;
            p.trieda[c] = n.mensie_ako(4) as u8;
        }
        p
    }

    #[test]
    fn nahodne_obrazy_zhoda_s_rozhodcom() {
        for semienko in 0..1500u64 {
            let mut n = Nahoda::zo_semienka(semienko);
            let log2_ctb = 4 + (semienko % 3) as u32;
            // rozmery: väčšinou násobky 8 (MinCbSizeY), občas len párne; zriedka celé CTB
            let krok = if n.mensie_ako(5) == 0 { 2 } else { 8 };
            let sirka = krok * (1 + n.mensie_ako(200 / krok as u64) as usize);
            let vyska = krok * (1 + n.mensie_ako(130 / krok as u64) as usize);
            let info = nahodne_info(&mut n, sirka, vyska, log2_ctb);
            let mut vstup = Obraz::novy(sirka, vyska);
            for r in &mut vstup.roviny {
                nahodna_rovina(&mut n, r);
            }
            let ctb: Vec<SaoCtb> =
                (0..info.sirka_ctb * info.vyska_ctb).map(|_| nahodne_sao(&mut n)).collect();
            let mut mapy = MapyFiltra::nove(sirka, vyska);
            let percent = [0, 0, 3, 30][n.mensie_ako(4) as usize];
            for b in &mut mapy.bez_filtra {
                *b = n.mensie_ako(100) < percent;
            }
            let popis = format!("semienko {semienko}, {sirka}×{vyska}, CTB {}", 1 << log2_ctb);
            over(&vstup, &ctb, &mapy, &info, &popis);
        }
    }

    /// Všetky CTB s rovnakým typom a triedou (aj vnútorné CTB idú rýchlou cestou celých riadkov).
    #[test]
    fn rovnake_parametre_vsetkych_ctb() {
        let mut n = Nahoda::zo_semienka(77);
        for log2_ctb in 4..=6 {
            for typ in 0..=2u8 {
                for trieda in 0..4u8 {
                    let (sirka, vyska) = (200, 136);
                    let info = nahodne_info(&mut n, sirka, vyska, log2_ctb);
                    let mut vstup = Obraz::novy(sirka, vyska);
                    for r in &mut vstup.roviny {
                        nahodna_rovina(&mut n, r);
                    }
                    let mut p = nahodne_sao(&mut n);
                    p.typ = [typ; 3];
                    p.trieda = [trieda; 3];
                    let ctb = vec![p; info.sirka_ctb * info.vyska_ctb];
                    let mapy = MapyFiltra::nove(sirka, vyska);
                    let popis = format!("CTB {}, typ {typ}, trieda {trieda}", 1 << log2_ctb);
                    over(&vstup, &ctb, &mapy, &info, &popis);
                }
            }
        }
    }

    // ----------------------------------------------------------------------------------------------------
    // Ručne overené príklady
    // ----------------------------------------------------------------------------------------------------

    #[test]
    fn pasmovy_posun_rucne() {
        // obraz 16 × 16, jeden CTB 16; jas: sao_band_position = 30 → pásma 30, 31, 0, 1 dostanú bandIdx 1 … 4
        let info = jednoduche_info(16, 16, 4);
        let mut vstup = Obraz::novy(16, 16);
        let jas = [0, 7, 8, 15, 16, 239, 240, 247, 248, 255, 100];
        for (x, &v) in jas.iter().enumerate() {
            nastav(&mut vstup.roviny[0], x, 0, v);
        }
        // Cb: sao_band_position = 31 → pásma 31, 0, 1, 2; orezanie na 0 a 255
        let cb = [255, 0, 3, 250, 248, 247, 8, 16];
        for (x, &v) in cb.iter().enumerate() {
            nastav(&mut vstup.roviny[1], x, 0, v);
        }
        let p = SaoCtb {
            typ: [1, 1, 0],
            posuny: [[3, -4, 6, 5], [7, -7, 2, -1], [7, 7, 7, 7]],
            pasmo: [30, 31, 0],
            trieda: [0; 3],
        };
        let mapy = MapyFiltra::nove(16, 16);
        let v = over(&vstup, &[p], &mapy, &info, "pásmový posun");
        // pásmo = vzorka >> 3: 0,0 → pásmo 0 (bandIdx 3, +6); 8,15 → 1 (bandIdx 4, +5); 16 → 2 (bez zmeny);
        // 239 → 29 (bez zmeny); 240,247 → 30 (bandIdx 1, +3); 248,255 → 31 (bandIdx 2, −4)
        let ocakavany_jas = [6, 13, 13, 20, 16, 239, 243, 250, 244, 251, 100];
        for (x, &o) in ocakavany_jas.iter().enumerate() {
            assert_eq!(v.roviny[0].vzorka(x, 0), o, "jas x = {x}");
        }
        // 255 → pásmo 31 (bandIdx 1, +7 → 262 → 255); 0, 3 → pásmo 0 (bandIdx 2, −7 → 0); 250, 248 → 31
        // (+7 → 255); 247 → pásmo 30 (bez zmeny); 8 → pásmo 1 (bandIdx 3, +2); 16 → pásmo 2 (bandIdx 4, −1)
        let ocakavane_cb = [255, 0, 0, 255, 255, 247, 10, 15];
        for (x, &o) in ocakavane_cb.iter().enumerate() {
            assert_eq!(v.roviny[1].vzorka(x, 0), o, "Cb x = {x}");
        }
        // stredná hodnota 128 je v pásme 16 – mimo zvolených pásiem; Cr má SaoTypeIdx = 0
        assert_eq!(v.roviny[0].vzorka(5, 5), 128);
        assert_eq!(v.roviny[1].vzorka(5, 5), 128);
        assert!(v.roviny[2].data.chunks(v.roviny[2].krok).all(|r| r[..8].iter().all(|&x| x == 128)));
    }

    #[test]
    fn hranovy_posun_rucne_vsetky_triedy() {
        // vzorka c so susedmi a (k = 0), b (k = 1) → očakávaná hodnota pri SaoOffsetVal[1 … 4] = 5, 2, −3, −6
        let pripady: [(Vzorka, Vzorka, Vzorka, Vzorka); 7] = [
            (100, 110, 120, 105), // edgeIdx 2 − 1 − 1 = 0 → 1: miestne minimum, +5
            (100, 110, 100, 102), // 2 − 1 + 0 = 1 → 2: +2
            (100, 90, 110, 100),  // 2 + 1 − 1 = 2 → 0: monotónny priebeh, bez zmeny
            (100, 90, 100, 97),   // 3: −3
            (100, 90, 80, 94),    // 4: miestne maximum, −6
            (254, 255, 255, 255), // miestne minimum, 254 + 5 → Clip3 → 255
            (2, 0, 1, 0),         // miestne maximum, 2 − 6 → Clip3 → 0
        ];
        for trieda in 0..4u8 {
            let [(ha, va), (hb, vb)] = SMERY[trieda as usize];
            let info = jednoduche_info(64, 64, 6);
            let mut vstup = Obraz::novy(64, 64);
            for c in 0..2 {
                let r = &mut vstup.roviny[c];
                for v in &mut r.data {
                    *v = 50;
                }
                for (m, &(s, a, b, _)) in pripady.iter().enumerate() {
                    let (x, y) = (4 + 4 * m, 4);
                    nastav(r, x, y, s);
                    nastav(r, x.wrapping_add_signed(ha), y.wrapping_add_signed(va), a);
                    nastav(r, x.wrapping_add_signed(hb), y.wrapping_add_signed(vb), b);
                }
            }
            let p =
                SaoCtb { typ: [2, 2, 0], posuny: [[5, 2, -3, -6]; 3], pasmo: [0; 3], trieda: [trieda; 3] };
            let mapy = MapyFiltra::nove(64, 64);
            let v = over(&vstup, &[p], &mapy, &info, &format!("trieda {trieda}"));
            for c in 0..2 {
                for (m, &(_, _, _, ocakavana)) in pripady.iter().enumerate() {
                    assert_eq!(
                        v.roviny[c].vzorka(4 + 4 * m, 4),
                        ocakavana,
                        "trieda {trieda}, zložka {c}, m {m}"
                    );
                }
                // rovná plocha (všetci susedia rovnakí) sa nemení
                assert_eq!(v.roviny[c].vzorka(20, 20), 50);
            }
        }
    }

    /// Dva CTB 16 vedľa seba, stĺpce x = 15 a 16 majú hodnotu 50, inde 100; vodorovná trieda, posuny
    /// SaoOffsetVal[1 … 4] = 5, 2, −3, −6. Vzorky na x = 15 a 16 (edgeIdx 1 → 2) dostanú +2 len vtedy, ak je
    /// sused z druhého CTB dostupný; vzorky na x = 14 a 17 (edgeIdx 3) vždy −3.
    fn dva_ctb(info: &InfoObrazu, mapy: &MapyFiltra) -> Obraz {
        let mut vstup = Obraz::novy(32, 16);
        for r in &mut vstup.roviny {
            for v in &mut r.data {
                *v = 100;
            }
        }
        for y in 0..16 {
            nastav(&mut vstup.roviny[0], 15, y, 50);
            nastav(&mut vstup.roviny[0], 16, y, 50);
            nastav(&mut vstup.roviny[0], 0, y, 50);
        }
        let p = SaoCtb { typ: [2, 0, 0], posuny: [[5, 2, -3, -6]; 3], pasmo: [0; 3], trieda: [0; 3] };
        over(&vstup, &[p, p], mapy, info, "dva CTB")
    }

    fn stlpce(o: &Obraz, y: usize) -> [Vzorka; 4] {
        [14, 15, 16, 17].map(|x| o.roviny[0].vzorka(x, y))
    }

    #[test]
    fn hranice_rezov_dlazdic_a_obrazu_rucne() {
        let mapy = MapyFiltra::nove(32, 16);
        // jeden rez, jedna dlaždica: filtruje sa cez hranicu CTB
        let mut info = jednoduche_info(32, 16, 4);
        let v = dva_ctb(&info, &mapy);
        assert_eq!(stlpce(&v, 3), [97, 52, 52, 97]);
        // okraj obrazu: vzorka (0, y) nemá ľavého suseda → nemení sa (inak by bola miestnym minimom)
        assert_eq!(v.roviny[0].vzorka(0, 3), 50);
        assert_eq!(v.roviny[0].vzorka(1, 3), 97);

        // dva rezy, neskorší (druhý) má slice_loop_filter_across_slices_enabled_flag = 0 → nefiltruje sa
        info.rezy = vec![
            RezFiltra { adresa: 0, cez_rezy: true, ..Default::default() },
            RezFiltra { adresa: 1, cez_rezy: false, ..Default::default() },
        ];
        info.ctb_rez = vec![0, 1];
        assert_eq!(stlpce(&dva_ctb(&info, &mapy), 3), [97, 50, 50, 97]);
        // príznak skoršieho rezu nerozhoduje
        info.rezy[0].cez_rezy = false;
        info.rezy[1].cez_rezy = true;
        assert_eq!(stlpce(&dva_ctb(&info, &mapy), 3), [97, 52, 52, 97]);
        // závislý segment (rovnaké SliceAddrRs) je ten istý rez – hranica segmentu nič nemení
        info.rezy = vec![
            RezFiltra { adresa: 0, cez_rezy: false, ..Default::default() },
            RezFiltra { adresa: 0, cez_rezy: false, ..Default::default() },
        ];
        assert_eq!(stlpce(&dva_ctb(&info, &mapy), 3), [97, 52, 52, 97]);

        // dve dlaždice v jednom reze
        let mut info = jednoduche_info(32, 16, 4);
        info.ctb_dlazdica = vec![0, 1];
        info.cez_dlazdice = false;
        assert_eq!(stlpce(&dva_ctb(&info, &mapy), 3), [97, 50, 50, 97]);
        info.cez_dlazdice = true;
        assert_eq!(stlpce(&dva_ctb(&info, &mapy), 3), [97, 52, 52, 97]);

        // bez_filtra: blok 4×4 so vzorkami x = 16 … 19, y = 0 … 3 sa nemení; vzorka x = 15 vedľa neho sa
        // filtruje a číta jeho (odblokovanú) hodnotu
        let info = jednoduche_info(32, 16, 4);
        let mut mapy = MapyFiltra::nove(32, 16);
        let i = mapy.index(16, 0);
        mapy.bez_filtra[i] = true;
        let v = dva_ctb(&info, &mapy);
        assert_eq!(stlpce(&v, 0), [97, 52, 50, 100]);
        assert_eq!(stlpce(&v, 4), [97, 52, 52, 97]);
    }

    /// Rozhoduje poradie dekódovania (CtbAddrRsToTs), nie raster: dve dlaždice-stĺpce po jednom CTB,
    /// CTB (1, 0) sa dekóduje až po CTB (0, 1). Trieda 3 (45°): vzorka (16, 15) v CTB (1, 0) má suseda
    /// (15, 16) v CTB (0, 1).
    #[test]
    fn poradie_dekodovania_v_dlazdiciach_rucne() {
        let mut info = jednoduche_info(32, 32, 4);
        info.ctb_dlazdica = vec![0, 1, 0, 1];
        info.ctb_ts = vec![0, 2, 1, 3]; // raster (0,0) (1,0) (0,1) (1,1) → TS 0 2 1 3
        // rez A = dlaždica 0 (SliceAddrRs 0), rez B = dlaždica 1 (SliceAddrRs 1)
        info.ctb_rez = vec![0, 1, 0, 1];
        info.cez_dlazdice = true;
        let mut vstup = Obraz::novy(32, 32);
        for r in &mut vstup.roviny {
            for v in &mut r.data {
                *v = 100;
            }
        }
        nastav(&mut vstup.roviny[0], 16, 15, 50); // miestne minimum v smere 45° → +5, ak je sused dostupný
        let p = SaoCtb { typ: [2, 0, 0], posuny: [[5, 2, -3, -6]; 3], pasmo: [0; 3], trieda: [3; 3] };
        let mapy = MapyFiltra::nove(32, 32);
        let mut over_rezy = |a: bool, b: bool| {
            info.rezy = vec![
                RezFiltra { adresa: 0, cez_rezy: a, ..Default::default() },
                RezFiltra { adresa: 1, cez_rezy: b, ..Default::default() },
            ];
            over(&vstup, &[p; 4], &mapy, &info, "poradie").roviny[0].vzorka(16, 15)
        };
        // neskorší je rez B (vzorka v CTB s TS 2) → rozhoduje jeho príznak
        assert_eq!(over_rezy(false, true), 55);
        assert_eq!(over_rezy(true, false), 50);
    }

    #[test]
    fn typ_nula_kopiruje_a_zapise_vsetko() {
        let mut n = Nahoda::zo_semienka(5);
        for (sirka, vyska, log2_ctb) in [(8, 8, 6), (40, 24, 4), (104, 72, 5), (136, 80, 6), (18, 10, 4)] {
            let info = nahodne_info(&mut n, sirka, vyska, log2_ctb);
            let mut vstup = Obraz::novy(sirka, vyska);
            for r in &mut vstup.roviny {
                nahodna_rovina(&mut n, r);
            }
            let ctb = vec![SaoCtb::default(); info.sirka_ctb * info.vyska_ctb];
            let mut mapy = MapyFiltra::nove(sirka, vyska);
            mapy.bez_filtra[0] = true;
            let v = spusti(&vstup, &ctb, &mapy, &info);
            for c in 0..3 {
                for y in 0..vstup.roviny[c].vyska {
                    assert_eq!(
                        v.roviny[c].riadok(y),
                        vstup.roviny[c].riadok(y),
                        "{sirka}×{vyska}, zložka {c}"
                    );
                }
            }
        }
    }

    #[test]
    fn vystup_inych_rozmerov_sa_prisposobi() {
        let info = jednoduche_info(48, 32, 4);
        let mut vstup = Obraz::novy(48, 32);
        let mut n = Nahoda::zo_semienka(9);
        for r in &mut vstup.roviny {
            nahodna_rovina(&mut n, r);
        }
        let ctb: Vec<SaoCtb> = (0..6).map(|_| nahodne_sao(&mut n)).collect();
        let mapy = MapyFiltra::nove(48, 32);
        let mut vystup = Obraz::novy(16, 16);
        sao(&vstup, &mut vystup, &ctb, &mapy, &info);
        porovnaj(&vystup, &rozhodca(&vstup, &ctb, &mapy, &info), &vstup, "iné rozmery");
    }
}
