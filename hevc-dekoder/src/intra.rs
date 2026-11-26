//! Vnútrosnímková predikcia (intra prediction) – H.265, kap. 8.4.4.2.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`Susedia`] a [`predikuj`]. Dekodér vyplní susedné vzorky a ich
//! dostupnosť (6.4.1 a obmedzená vnútrosnímková predikcia); tento modul urobí náhradu nedostupných vzoriek
//! (8.4.4.2.2), filtrovanie susedov (8.4.4.2.3) a samotnú predikciu (8.4.4.2.4–8.4.4.2.6).
//!
//! Postup pre jeden blok nTbS × nTbS:
//! 1. náhrada nedostupných vzoriek (8.4.4.2.2) priamo v lineárnom poradí [`Susedia`],
//! 2. filtrovanie susedov (8.4.4.2.3) – len jas: filter [1 2 1] alebo silné (bilineárne) vyhladenie 32×32,
//! 3. predikcia: planárna (8.4.4.2.4), DC (8.4.4.2.5), uhlová 2 … 34 (8.4.4.2.6).
//!
//! Výkon (chladný systém, Cortex-A53): každá veľkosť bloku má vlastnú inštanciu (`const N`) – slučky majú pevný
//! počet opakovaní, prekladač ich rozvinie a vektorizuje; všetky pracovné polia sú na zásobníku, žiadna alokácia.
//! Vodorovné uhlové režimy 2 … 17 sa počítajú ako zvislé so zámenou ľavého stĺpca a horného riadka a výsledok
//! sa transponuje – norma je v tom symetrická (intraPredAngle[m] = intraPredAngle[36 − m], vzorce 8.4.4.2.6 pre
//! predModeIntra < 18 sú vzorce pre ≥ 18 so zámenou x ↔ y). Vnútorné jadrá (`riadok_interpolacie`,
//! `riadok_planarny`, `filtruj_121`, `transponuj`) sú samostatné funkcie – krok H5 ich nahradí NEON.

use super::obraz::Vzorka;

/// Najväčší transformačný blok 32×32 → 4 · 32 + 1 = 129 susedných vzoriek.
pub const MAX_SUSEDOV: usize = 129;

/// Susedné vzorky bloku nTbS × nTbS v poradí, v akom ich prechádza náhrada (8.4.4.2.2):
/// - index `0` = p[−1][2·nTbS − 1] (dole vľavo), smerom nahor po index `2·nTbS − 1` = p[−1][0],
/// - index `2·nTbS` = p[−1][−1] (roh),
/// - index `2·nTbS + 1 + x` = p[x][−1] pre x = 0 … 2·nTbS − 1 (hore, zľava doprava).
///
/// Platných je prvých `4·nTbS + 1` prvkov. `dostupne[i] == false` = vzorka nie je dostupná (mimo obrazu, ešte
/// nedekódovaná, iný rez či dlaždica, alebo medzisnímkový blok pri constrained_intra_pred_flag).
#[derive(Debug, Clone)]
pub struct Susedia {
    pub vzorky: [u16; MAX_SUSEDOV],
    pub dostupne: [bool; MAX_SUSEDOV],
}

impl Default for Susedia {
    fn default() -> Self {
        Susedia { vzorky: [0; MAX_SUSEDOV], dostupne: [false; MAX_SUSEDOV] }
    }
}

/// intraPredAngle podľa predModeIntra (8.4.4.2.6); režimy 0 (planárna) a 1 (DC) nie sú uhlové.
const UHOL: [i8; 35] = [
    0, 0, 32, 26, 21, 17, 13, 9, 5, 2, 0, -2, -5, -9, -13, -17, -21, -26, -32, -26, -21, -17, -13, -9, -5,
    -2, 0, 2, 5, 9, 13, 17, 21, 26, 32,
];

/// invAngle pre predModeIntra 11 … 25 (8.4.4.2.6) – len tieto režimy majú záporný intraPredAngle.
const INV_UHOL: [i16; 15] =
    [-4096, -1638, -910, -630, -482, -390, -315, -256, -315, -390, -482, -630, -910, -1638, -4096];

/// Veľkosť pracovného poľa rozšírenej referencie ref[k], k = −nTbS … 2·nTbS + 1 (posledná je len výplň).
const DLZKA_REF: usize = 3 * 32 + 2;

/// Vnútrosnímková predikcia jedného transformačného bloku nTbS × nTbS, nTbS = 1 << `log2_n` (4 … 32).
///
/// - `rezim`: predModeIntra 0 (planárna), 1 (DC), 2 … 34 (uhlové),
/// - `jas`: true pre zložku jasu (cIdx = 0). Pri 4:2:0 sa susedia filtrujú (8.4.4.2.3) a hraničné filtre DC,
///   režimu 10 a 26 použijú **len pre jas** a len pri nTbS < 32,
/// - `silne_vyhladenie`: strong_intra_smoothing_enabled_flag (bilineárne vyhladenie len jas 32×32),
/// - výsledok sa zapíše do `ciel`: vzorka (x, y) na `ciel[y * krok + x]`.
///
/// Predpoklady (zaručuje ich dekodér, porušenie je chyba programu a skončí panikou): `log2_n` 2 … 5,
/// `rezim` 0 … 34, `bitova_hlbka` 8 … 10 (medzivýsledky jadier sú 16-bitové), `krok` ≥ nTbS a `ciel` má aspoň
/// (nTbS − 1) · `krok` + nTbS prvkov. Mimo bloku sa do `ciel` nič nezapíše.
#[allow(clippy::too_many_arguments)]
pub fn predikuj(
    susedia: &Susedia,
    log2_n: u32,
    rezim: u32,
    jas: bool,
    bitova_hlbka: u32,
    silne_vyhladenie: bool,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    debug_assert!(rezim <= 34, "neplatný režim vnútrosnímkovej predikcie {rezim}");
    debug_assert!((8..=10).contains(&bitova_hlbka), "nepodporovaná bitová hĺbka {bitova_hlbka}");
    match log2_n {
        2 => predikuj_n::<4>(susedia, rezim, jas, bitova_hlbka, silne_vyhladenie, ciel, krok),
        3 => predikuj_n::<8>(susedia, rezim, jas, bitova_hlbka, silne_vyhladenie, ciel, krok),
        4 => predikuj_n::<16>(susedia, rezim, jas, bitova_hlbka, silne_vyhladenie, ciel, krok),
        5 => predikuj_n::<32>(susedia, rezim, jas, bitova_hlbka, silne_vyhladenie, ciel, krok),
        _ => panic!("vnútrosnímková predikcia: nepodporovaná veľkosť bloku 2^{log2_n}"),
    }
}

/// Predikcia bloku pevnej veľkosti N = nTbS (8.4.4.2.1).
fn predikuj_n<const N: usize>(
    susedia: &Susedia,
    rezim: u32,
    jas: bool,
    bitova_hlbka: u32,
    silne_vyhladenie: bool,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    let pocet = 4 * N + 1;
    let mut p: [Vzorka; MAX_SUSEDOV] = [0; MAX_SUSEDOV];
    nahrad(&susedia.vzorky[..pocet], &susedia.dostupne[..pocet], bitova_hlbka, &mut p[..pocet]);

    // 8.4.4.2.3 – pri 4:2:0 len jas (cIdx = 0)
    if jas && filtrovat(rezim, N) {
        let mut pf: [Vzorka; MAX_SUSEDOV] = [0; MAX_SUSEDOV];
        if N == 32 && silne_vyhladenie && bilinearne(&p, bitova_hlbka) {
            silne_vyhlad(&p, &mut pf);
        } else {
            filtruj_121(&p[..pocet], &mut pf[..pocet]);
        }
        p = pf;
    }

    // hraničné filtre DC a režimov 10, 26: cIdx = 0, nTbS < 32 (disableIntraBoundaryFilter = 0 v profile Main)
    let hranicne = jas && N < 32;
    let max = (1i32 << bitova_hlbka) - 1;
    match rezim {
        0 => planarna::<N>(&p, ciel, krok),
        1 => dc::<N>(&p, hranicne, ciel, krok),
        10 => vodorovna::<N>(&p, hranicne, max, ciel, krok),
        26 => zvisla::<N>(&p, hranicne, max, ciel, krok),
        _ => uhlova::<N>(&p, rezim, ciel, krok),
    }
}

/// Riadky bloku v `ciel`: N rezov dĺžky N, riadok y začína na y · `krok`. Jediná kontrola dĺžky na začiatku
/// a žiadne delenie (rozdelenie `chunks_mut`) – na slabom ARM záleží aj na réžii bloku 4×4.
#[inline(always)]
fn riadky<const N: usize>(ciel: &mut [Vzorka], krok: usize) -> impl Iterator<Item = &mut [Vzorka]> {
    ciel[..(N - 1) * krok + N].chunks_mut(krok).map(|r| &mut r[..N])
}

/// Náhrada nedostupných susedných vzoriek (8.4.4.2.2) v lineárnom poradí [`Susedia`].
///
/// Nie je dostupná žiadna → všetky 1 << (BitDepth − 1). Inak p[−1][2N − 1] (index 0), ak chýba, dostane prvú
/// dostupnú vzorku v poradí prechodu a každá ďalšia nedostupná vzorka hodnotu predošlej. Oboje je „posledná
/// videná dostupná hodnota“, ak sa ňou na začiatku stane prvá dostupná vzorka.
///
/// Zámerne jediná kópia pre všetky veľkosti (bez vkladania) – menší kód pre 32 KB vyrovnávaciu pamäť inštrukcií
/// Cortex-A53; meranie (x86) neukázalo spomalenie.
#[inline(never)]
fn nahrad(vzorky: &[u16], dostupne: &[bool], bitova_hlbka: u32, p: &mut [Vzorka]) {
    let dostupnych = dostupne.iter().filter(|&&d| d).count();
    if dostupnych == p.len() {
        for (o, &v) in p.iter_mut().zip(vzorky) {
            *o = v as Vzorka;
        }
    } else if dostupnych == 0 {
        p.fill((1u32 << (bitova_hlbka - 1)) as Vzorka);
    } else {
        let prva = dostupne.iter().position(|&d| d).unwrap_or(0);
        let mut posledna = vzorky[prva] as Vzorka;
        for ((o, &v), &d) in p.iter_mut().zip(vzorky).zip(dostupne) {
            if d {
                posledna = v as Vzorka;
            }
            *o = posledna;
        }
    }
}

/// filterFlag (8.4.4.2.3): nefiltruje sa DC ani blok 4×4; inak sa filtruje, ak minDistVerHor =
/// Min(|predModeIntra − 26|, |predModeIntra − 10|) > intraHorVerDistThres[nTbS] (8: 7, 16: 1, 32: 0).
/// Planárna (0) má vzdialenosť 10 – filtruje sa pre 8, 16 aj 32.
#[inline(always)]
fn filtrovat(rezim: u32, n: usize) -> bool {
    if rezim == 1 || n == 4 {
        return false;
    }
    let vzdialenost = rezim.abs_diff(26).min(rezim.abs_diff(10));
    let prah = match n {
        8 => 7,
        16 => 1,
        _ => 0,
    };
    vzdialenost > prah
}

/// biIntFlag (8.4.4.2.3) bloku 32×32 bez príznaku a zložky (tie overí volajúci): horný riadok aj ľavý stĺpec sú
/// takmer priamky – |p[−1][−1] + p[63][−1] − 2·p[31][−1]| < 1 << (BitDepthY − 5) a rovnako
/// |p[−1][−1] + p[−1][63] − 2·p[−1][31]|. Lineárne indexy: roh 64, p[63][−1] 128, p[31][−1] 96, p[−1][63] 0,
/// p[−1][31] 32.
fn bilinearne(p: &[Vzorka; MAX_SUSEDOV], bitova_hlbka: u32) -> bool {
    let v = |i: usize| i32::from(p[i]);
    let prah = 1i32 << (bitova_hlbka - 5);
    let roh = v(64);
    (roh + v(128) - 2 * v(96)).abs() < prah && (roh + v(0) - 2 * v(32)).abs() < prah
}

/// Silné (bilineárne) vyhladenie susedov bloku 32×32 (8.4.4.2.3, biIntFlag = 1):
/// pF[−1][y] = ((63 − y)·p[−1][−1] + (y + 1)·p[−1][63] + 32) >> 6 a
/// pF[x][−1] = ((63 − x)·p[−1][−1] + (x + 1)·p[63][−1] + 32) >> 6 pre x, y = 0 … 62.
///
/// Lineárne: ľavý stĺpec je index i = 63 − y, horný riadok 65 + x. Tie isté vzorce dajú pre i = 0 (p[−1][63]),
/// 64 (roh) a 128 (p[63][−1]) presne nezmenenú vzorku, ako žiada norma – netreba výnimky.
fn silne_vyhlad(p: &[Vzorka; MAX_SUSEDOV], pf: &mut [Vzorka; MAX_SUSEDOV]) {
    let roh = u32::from(p[64]);
    let dole = u32::from(p[0]);
    let vpravo = u32::from(p[128]);
    let (lavo, hore) = pf.split_at_mut(64);
    for (i, o) in lavo.iter_mut().enumerate() {
        let i = i as u32;
        *o = ((i * roh + (64 - i) * dole + 32) >> 6) as Vzorka;
    }
    for (j, o) in hore.iter_mut().enumerate() {
        let j = j as u32;
        *o = (((64 - j) * roh + j * vpravo + 32) >> 6) as Vzorka;
    }
}

/// Filter [1 2 1] susedov (8.4.4.2.3, biIntFlag = 0). V lineárnom poradí je to jeden súvislý filter cez celé pole
/// vrátane rohu (pF[−1][−1] = (p[−1][0] + 2·p[−1][−1] + p[0][−1] + 2) >> 2); koncové vzorky p[−1][2N − 1]
/// a p[2N − 1][−1] sa nemenia. Jadro pre NEON (krok H5).
#[inline(always)]
fn filtruj_121(p: &[Vzorka], pf: &mut [Vzorka]) {
    let k = p.len();
    pf[0] = p[0];
    pf[k - 1] = p[k - 1];
    for (((o, &a), &b), &c) in pf[1..k - 1].iter_mut().zip(&p[..k - 2]).zip(&p[1..k - 1]).zip(&p[2..]) {
        *o = ((u16::from(a) + 2 * u16::from(b) + u16::from(c) + 2) >> 2) as Vzorka;
    }
}

/// Planárna predikcia (8.4.4.2.4): predSamples[x][y] = ((nTbS − 1 − x)·p[−1][y] + (x + 1)·p[nTbS][−1] +
/// (nTbS − 1 − y)·p[x][−1] + (y + 1)·p[−1][nTbS] + nTbS) >> (Log2(nTbS) + 1).
///
/// Počíta sa prírastkovo v 16 bitoch: zvislá časť v[x] = (nTbS − 1 − y)·p[x][−1] + (y + 1)·p[−1][nTbS] + nTbS sa
/// s každým riadkom zmení o p[−1][nTbS] − p[x][−1] a vodorovná časť riadka je
/// (nTbS − 1)·p[−1][y] + p[nTbS][−1] + x·(p[nTbS][−1] − p[−1][y]). Prírastky môžu byť záporné, preto sa sčítava
/// modulo 2^16; celý súčet je vždy 0 … 2·nTbS·(2^BitDepth − 1) + nTbS < 2^16 (BitDepth ≤ 10), teda presný.
fn planarna<const N: usize>(p: &[Vzorka; MAX_SUSEDOV], ciel: &mut [Vzorka], krok: usize) {
    let s = 2 * N; // index rohu p[−1][−1]
    let hore = &p[s + 1..s + 1 + N]; // p[x][−1]
    let vpravo_hore = u16::from(p[s + 1 + N]); // p[nTbS][−1]
    let vlavo_dole = u16::from(p[N - 1]); // p[−1][nTbS]
    let n = N as u16;
    let posun = N.trailing_zeros() + 1;
    let mut zvisla = [0u16; N];
    let mut prirastok = [0u16; N];
    for ((v, d), &h) in zvisla.iter_mut().zip(&mut prirastok).zip(hore) {
        let h = u16::from(h);
        *v = (n - 1) * h + vlavo_dole + n;
        *d = vlavo_dole.wrapping_sub(h);
    }
    for (y, riadok) in riadky::<N>(ciel, krok).enumerate() {
        let vlavo = u16::from(p[s - 1 - y]); // p[−1][y]
        let zaklad = (n - 1) * vlavo + vpravo_hore;
        let sklon = vpravo_hore.wrapping_sub(vlavo);
        riadok_planarny(&zvisla, zaklad, sklon, posun, riadok);
        for (v, &d) in zvisla.iter_mut().zip(&prirastok) {
            *v = v.wrapping_add(d);
        }
    }
}

/// x = 0 … 31 – váhy vodorovnej časti planárnej predikcie.
const INDEXY: [u16; 32] = {
    let mut a = [0u16; 32];
    let mut i = 0;
    while i < 32 {
        a[i] = i as u16;
        i += 1;
    }
    a
};

/// Jeden riadok planárnej predikcie: (v[x] + zaklad + x·sklon) >> posun, všetko modulo 2^16 (pozri
/// [`planarna`]). Jadro pre NEON (krok H5).
#[inline(always)]
fn riadok_planarny(zvisla: &[u16], zaklad: u16, sklon: u16, posun: u32, vystup: &mut [Vzorka]) {
    for ((o, &v), &x) in vystup.iter_mut().zip(zvisla).zip(&INDEXY) {
        *o = (v.wrapping_add(zaklad).wrapping_add(x.wrapping_mul(sklon)) >> posun) as Vzorka;
    }
}

/// DC predikcia (8.4.4.2.5): dcVal = (Σ p[x′][−1] + Σ p[−1][y′] + nTbS) >> (k + 1), k = Log2(nTbS); pri jase
/// a nTbS < 32 hraničný filter prvého riadka a stĺpca.
fn dc<const N: usize>(p: &[Vzorka; MAX_SUSEDOV], hranicne: bool, ciel: &mut [Vzorka], krok: usize) {
    let s = 2 * N;
    let hore = &p[s + 1..s + 1 + N]; // p[x][−1], x = 0 … N − 1
    let vlavo = &p[N..s]; // p[−1][y], y = N − 1 … 0
    let sucet: u32 = hore.iter().chain(vlavo).map(|&v| u32::from(v)).sum();
    let dc = ((sucet + N as u32) >> (N.trailing_zeros() + 1)) as u16;
    if !hranicne {
        for riadok in riadky::<N>(ciel, krok) {
            riadok.fill(dc as Vzorka);
        }
        return;
    }
    let dc3 = 3 * dc + 2;
    for (y, riadok) in riadky::<N>(ciel, krok).enumerate() {
        if y == 0 {
            // predSamples[0][0] = (p[−1][0] + 2·dcVal + p[0][−1] + 2) >> 2
            riadok[0] = ((u16::from(p[s - 1]) + 2 * dc + u16::from(p[s + 1]) + 2) >> 2) as Vzorka;
            // predSamples[x][0] = (p[x][−1] + 3·dcVal + 2) >> 2, x = 1 … nTbS − 1
            for (o, &h) in riadok[1..].iter_mut().zip(&hore[1..]) {
                *o = ((u16::from(h) + dc3) >> 2) as Vzorka;
            }
        } else {
            riadok.fill(dc as Vzorka);
            // predSamples[0][y] = (p[−1][y] + 3·dcVal + 2) >> 2, y = 1 … nTbS − 1
            riadok[0] = ((u16::from(p[s - 1 - y]) + dc3) >> 2) as Vzorka;
        }
    }
}

/// Hraničný filter režimov 10 a 26 (8.4.4.2.6): Clip1(a + ((b − c) >> 1)); >> je aritmetický posun (zaokrúhlenie
/// nadol aj pre záporné číslo).
#[inline(always)]
fn hranicna(a: Vzorka, b: Vzorka, c: Vzorka, max: i32) -> Vzorka {
    (i32::from(a) + ((i32::from(b) - i32::from(c)) >> 1)).clamp(0, max) as Vzorka
}

/// Čisto zvislý režim 26 (intraPredAngle = 0, 8.4.4.2.6): predSamples[x][y] = p[x][−1]; pri jase a nTbS < 32
/// predSamples[0][y] = Clip1Y(p[0][−1] + ((p[−1][y] − p[−1][−1]) >> 1)).
fn zvisla<const N: usize>(
    p: &[Vzorka; MAX_SUSEDOV],
    hranicne: bool,
    max: i32,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    let s = 2 * N;
    let hore = &p[s + 1..s + 1 + N];
    for (y, riadok) in riadky::<N>(ciel, krok).enumerate() {
        riadok.copy_from_slice(hore);
        if hranicne {
            riadok[0] = hranicna(p[s + 1], p[s - 1 - y], p[s], max);
        }
    }
}

/// Čisto vodorovný režim 10 (intraPredAngle = 0, 8.4.4.2.6): predSamples[x][y] = p[−1][y]; pri jase a nTbS < 32
/// predSamples[x][0] = Clip1Y(p[−1][0] + ((p[x][−1] − p[−1][−1]) >> 1)).
fn vodorovna<const N: usize>(
    p: &[Vzorka; MAX_SUSEDOV],
    hranicne: bool,
    max: i32,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    let s = 2 * N;
    for (y, riadok) in riadky::<N>(ciel, krok).enumerate() {
        if hranicne && y == 0 {
            for (o, &h) in riadok.iter_mut().zip(&p[s + 1..s + 1 + N]) {
                *o = hranicna(p[s - 1], h, p[s], max);
            }
        } else {
            riadok.fill(p[s - 1 - y]);
        }
    }
}

/// Uhlová predikcia s intraPredAngle ≠ 0 (8.4.4.2.6).
///
/// Hlavná referencia ref[k] je v `r[N + k]`: pre predModeIntra ≥ 18 ref[x] = p[−1 + x][−1], inak
/// ref[x] = p[−1][−1 + x] (x = 0 … 2·nTbS). Pri zápornom uhle a (nTbS·intraPredAngle) >> 5 < −1 sa ref rozšíri
/// doľava projekciou druhej strany: ref[x] = p[−1][−1 + ((x·invAngle + 128) >> 8)] (zvislé), resp.
/// p[−1 + ((x·invAngle + 128) >> 8)][−1] (vodorovné). Vodorovné režimy sa počítajú po stĺpcoch a transponujú.
fn uhlova<const N: usize>(p: &[Vzorka; MAX_SUSEDOV], rezim: u32, ciel: &mut [Vzorka], krok: usize) {
    let s = 2 * N;
    let uhol = i32::from(UHOL[rezim as usize]);
    let zvisly = rezim >= 18;
    let mut r: [Vzorka; DLZKA_REF] = [0; DLZKA_REF];
    if zvisly {
        r[N..=3 * N].copy_from_slice(&p[s..=4 * N]);
    } else {
        for (o, &v) in r[N..=3 * N].iter_mut().zip(p[..=s].iter().rev()) {
            *o = v;
        }
    }
    if uhol < 0 {
        let najmensi = (N as i32 * uhol) >> 5;
        if najmensi < -1 {
            let inv = i32::from(INV_UHOL[rezim as usize - 11]);
            for k in najmensi..=-1 {
                let j = ((k * inv + 128) >> 8) as usize;
                // zvislé: p[−1][j − 1] = index 2N − j; vodorovné: p[j − 1][−1] = index 2N + j
                r[(N as i32 + k) as usize] = if zvisly { p[s - j] } else { p[s + j] };
            }
        }
    }
    if zvisly {
        for (y, riadok) in riadky::<N>(ciel, krok).enumerate() {
            uhlovy_riadok::<N>(&r, (y as i32 + 1) * uhol, riadok);
        }
    } else {
        // t[x] = stĺpec x výsledku: predSamples[x][y] počíta ten istý vzorec s (x + 1)·intraPredAngle
        let mut t: [[Vzorka; N]; N] = [[0; N]; N];
        for (x, stlpec) in t.iter_mut().enumerate() {
            uhlovy_riadok::<N>(&r, (x as i32 + 1) * uhol, stlpec);
        }
        transponuj::<N>(&t, ciel, krok);
    }
}

/// Jeden riadok (pri vodorovných režimoch stĺpec) uhlovej predikcie (8.4.4.2.6): poloha = (y + 1)·intraPredAngle,
/// iIdx = poloha >> 5, iFact = poloha & 31; vzorka x je
/// ((32 − iFact)·ref[x + iIdx + 1] + iFact·ref[x + iIdx + 2] + 16) >> 5, pri iFact = 0 priamo ref[x + iIdx + 1].
#[inline(always)]
fn uhlovy_riadok<const N: usize>(r: &[Vzorka; DLZKA_REF], poloha: i32, vystup: &mut [Vzorka]) {
    let i = (N as i32 + (poloha >> 5) + 1) as usize;
    let f = (poloha & 31) as u16;
    if f == 0 {
        vystup.copy_from_slice(&r[i..i + N]);
    } else {
        riadok_interpolacie(&r[i..i + N], &r[i + 1..i + 1 + N], f, vystup);
    }
}

/// Jadro uhlovej predikcie: ((32 − f)·a[x] + f·b[x] + 16) >> 5. Pri BitDepth ≤ 10 stačí 16 bitov
/// (32 · 1023 + 16 < 65536). Krok H5: NEON (vmull_u8, vmlal_u8, vrshrn_n_u16).
#[inline(always)]
fn riadok_interpolacie(a: &[Vzorka], b: &[Vzorka], f: u16, vystup: &mut [Vzorka]) {
    let g = 32 - f;
    for ((o, &x), &y) in vystup.iter_mut().zip(a).zip(b) {
        *o = ((g * u16::from(x) + f * u16::from(y) + 16) >> 5) as Vzorka;
    }
}

/// Zápis transponovaného bloku: `t[x][y]` → `ciel[y * krok + x]`. Jadro pre NEON (krok H5).
#[inline(always)]
fn transponuj<const N: usize>(t: &[[Vzorka; N]; N], ciel: &mut [Vzorka], krok: usize) {
    for (y, riadok) in riadky::<N>(ciel, krok).enumerate() {
        for (o, stlpec) in riadok.iter_mut().zip(t) {
            *o = stlpec[y];
        }
    }
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)] // rozhodca zámerne opisuje vzorce normy s indexmi p[x][y]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    // -----------------------------------------------------------------------------------------------------------
    // ROZHODCA: priamy, pomalý prepis kap. 8.4.4.2.2 – 8.4.4.2.6 s poľami p[x][y] a predSamples[x][y] ako v norme.
    // Nepoužíva nič z rýchlej implementácie okrem rozhrania Susedia.
    // -----------------------------------------------------------------------------------------------------------

    /// Pole p[x][y] pre x, y = −1 … 63.
    #[derive(Clone)]
    struct PoleP([[i32; 65]; 65]);

    impl PoleP {
        fn g(&self, x: i32, y: i32) -> i32 {
            self.0[(x + 1) as usize][(y + 1) as usize]
        }
        fn s(&mut self, x: i32, y: i32, v: i32) {
            self.0[(x + 1) as usize][(y + 1) as usize] = v;
        }
    }

    /// intraPredAngle prepísaný nezávisle od tabuľky modulu.
    fn intra_pred_angle(m: i32) -> i32 {
        match m {
            2 => 32,
            3 => 26,
            4 => 21,
            5 => 17,
            6 => 13,
            7 => 9,
            8 => 5,
            9 => 2,
            10 => 0,
            11 => -2,
            12 => -5,
            13 => -9,
            14 => -13,
            15 => -17,
            16 => -21,
            17 => -26,
            18 => -32,
            19 => -26,
            20 => -21,
            21 => -17,
            22 => -13,
            23 => -9,
            24 => -5,
            25 => -2,
            26 => 0,
            27 => 2,
            28 => 5,
            29 => 9,
            30 => 13,
            31 => 17,
            32 => 21,
            33 => 26,
            34 => 32,
            _ => panic!("režim {m} nie je uhlový"),
        }
    }

    /// invAngle prepísaný nezávisle od tabuľky modulu.
    fn inv_angle(m: i32) -> i32 {
        match m {
            11 => -4096,
            12 => -1638,
            13 => -910,
            14 => -630,
            15 => -482,
            16 => -390,
            17 => -315,
            18 => -256,
            19 => -315,
            20 => -390,
            21 => -482,
            22 => -630,
            23 => -910,
            24 => -1638,
            25 => -4096,
            _ => panic!("režim {m} nemá invAngle"),
        }
    }

    /// Hlavné referenčné pole ref[x], x = −64 … 128; `None` = norma vzorku nedefinuje (čítať ju je chyba).
    struct Ref([Option<i32>; 193]);

    impl Ref {
        fn g(&self, x: i32) -> i32 {
            self.0[(x + 64) as usize].unwrap_or_else(|| panic!("ref[{x}] nie je definovaná normou"))
        }
        fn s(&mut self, x: i32, v: i32) {
            self.0[(x + 64) as usize] = Some(v);
        }
    }

    /// Výsledok rozhodcu: predSamples[x][y] a či sa použilo silné vyhladenie (biIntFlag = 1).
    struct Vysledok {
        pred: [[i32; 32]; 32],
        bi_int: bool,
    }

    fn rozhodca(
        susedia: &Susedia,
        n_tbs: i32,
        pred_mode: i32,
        c_idx: i32,
        bit_depth: i32,
        strong_intra_smoothing: bool,
    ) -> Vysledok {
        let n2 = 2 * n_tbs;
        // rozhranie Susedia → p[x][y] a značky dostupnosti
        let index = |x: i32, y: i32| if x == -1 { (n2 - 1 - y) as usize } else { (n2 + 1 + x) as usize };
        let mut susedne = Vec::new();
        for y in -1..n2 {
            susedne.push((-1, y));
        }
        for x in 0..n2 {
            susedne.push((x, -1));
        }
        let mut p = PoleP([[0; 65]; 65]);
        let mut dost = [[false; 65]; 65];
        for &(x, y) in &susedne {
            p.s(x, y, i32::from(susedia.vzorky[index(x, y)]));
            dost[(x + 1) as usize][(y + 1) as usize] = susedia.dostupne[index(x, y)];
        }
        let dostupna = |x: i32, y: i32| dost[(x + 1) as usize][(y + 1) as usize];

        // 8.4.4.2.2 – náhrada
        if !susedne.iter().any(|&(x, y)| dostupna(x, y)) {
            for &(x, y) in &susedne {
                p.s(x, y, 1 << (bit_depth - 1));
            }
        } else {
            if !dostupna(-1, n2 - 1) {
                // hľadanie od (−1, 2N − 1) po (−1, −1), potom od (0, −1) po (2N − 1, −1)
                let mut poradie = Vec::new();
                for y in (-1..n2).rev() {
                    poradie.push((-1, y));
                }
                for x in 0..n2 {
                    poradie.push((x, -1));
                }
                let &(xn, yn) = poradie.iter().find(|&&(x, y)| dostupna(x, y)).unwrap();
                let v = p.g(xn, yn);
                p.s(-1, n2 - 1, v);
            }
            for y in (-1..=n2 - 2).rev() {
                if !dostupna(-1, y) {
                    let v = p.g(-1, y + 1);
                    p.s(-1, y, v);
                }
            }
            for x in 0..n2 {
                if !dostupna(x, -1) {
                    let v = p.g(x - 1, -1);
                    p.s(x, -1, v);
                }
            }
        }

        // 8.4.4.2.3 – filtrovanie (4:2:0: len cIdx = 0)
        let mut bi_int = false;
        if c_idx == 0 {
            let filter_flag = if pred_mode == 1 || n_tbs == 4 {
                false
            } else {
                let min_dist_ver_hor = (pred_mode - 26).abs().min((pred_mode - 10).abs());
                let intra_hor_ver_dist_thres = match n_tbs {
                    8 => 7,
                    16 => 1,
                    32 => 0,
                    _ => unreachable!(),
                };
                min_dist_ver_hor > intra_hor_ver_dist_thres
            };
            if filter_flag {
                bi_int = strong_intra_smoothing
                    && c_idx == 0
                    && n_tbs == 32
                    && (p.g(-1, -1) + p.g(n_tbs * 2 - 1, -1) - 2 * p.g(n_tbs - 1, -1)).abs()
                        < (1 << (bit_depth - 5))
                    && (p.g(-1, -1) + p.g(-1, n_tbs * 2 - 1) - 2 * p.g(-1, n_tbs - 1)).abs()
                        < (1 << (bit_depth - 5));
                let mut pf = p.clone();
                if bi_int {
                    pf.s(-1, -1, p.g(-1, -1));
                    for y in 0..=62 {
                        pf.s(-1, y, ((63 - y) * p.g(-1, -1) + (y + 1) * p.g(-1, 63) + 32) >> 6);
                    }
                    pf.s(-1, 63, p.g(-1, 63));
                    for x in 0..=62 {
                        pf.s(x, -1, ((63 - x) * p.g(-1, -1) + (x + 1) * p.g(63, -1) + 32) >> 6);
                    }
                    pf.s(63, -1, p.g(63, -1));
                } else {
                    pf.s(-1, -1, (p.g(-1, 0) + 2 * p.g(-1, -1) + p.g(0, -1) + 2) >> 2);
                    for y in 0..=n_tbs * 2 - 2 {
                        pf.s(-1, y, (p.g(-1, y + 1) + 2 * p.g(-1, y) + p.g(-1, y - 1) + 2) >> 2);
                    }
                    pf.s(-1, n_tbs * 2 - 1, p.g(-1, n_tbs * 2 - 1));
                    for x in 0..=n_tbs * 2 - 2 {
                        pf.s(x, -1, (p.g(x - 1, -1) + 2 * p.g(x, -1) + p.g(x + 1, -1) + 2) >> 2);
                    }
                    pf.s(n_tbs * 2 - 1, -1, p.g(n_tbs * 2 - 1, -1));
                }
                p = pf;
            }
        }

        let clip1 = |v: i32| v.clamp(0, (1 << bit_depth) - 1);
        let log2 = n_tbs.trailing_zeros() as i32;
        let mut pred = [[0i32; 32]; 32];
        let n = n_tbs as usize;
        match pred_mode {
            // 8.4.4.2.4
            0 => {
                for x in 0..n_tbs {
                    for y in 0..n_tbs {
                        pred[x as usize][y as usize] = ((n_tbs - 1 - x) * p.g(-1, y)
                            + (x + 1) * p.g(n_tbs, -1)
                            + (n_tbs - 1 - y) * p.g(x, -1)
                            + (y + 1) * p.g(-1, n_tbs)
                            + n_tbs)
                            >> (log2 + 1);
                    }
                }
            }
            // 8.4.4.2.5
            1 => {
                let mut sucet = n_tbs;
                for x in 0..n_tbs {
                    sucet += p.g(x, -1);
                }
                for y in 0..n_tbs {
                    sucet += p.g(-1, y);
                }
                let dc_val = sucet >> (log2 + 1);
                for x in 0..n {
                    for y in 0..n {
                        pred[x][y] = dc_val;
                    }
                }
                if c_idx == 0 && n_tbs < 32 {
                    pred[0][0] = (p.g(-1, 0) + 2 * dc_val + p.g(0, -1) + 2) >> 2;
                    for x in 1..n_tbs {
                        pred[x as usize][0] = (p.g(x, -1) + 3 * dc_val + 2) >> 2;
                    }
                    for y in 1..n_tbs {
                        pred[0][y as usize] = (p.g(-1, y) + 3 * dc_val + 2) >> 2;
                    }
                }
            }
            // 8.4.4.2.6
            m => {
                let angle = intra_pred_angle(m);
                let mut r = Ref([None; 193]);
                if m >= 18 {
                    for x in 0..=n_tbs {
                        r.s(x, p.g(-1 + x, -1));
                    }
                    if angle < 0 {
                        if (n_tbs * angle) >> 5 < -1 {
                            for x in (n_tbs * angle) >> 5..=-1 {
                                r.s(x, p.g(-1, -1 + ((x * inv_angle(m) + 128) >> 8)));
                            }
                        }
                    } else {
                        for x in n_tbs + 1..=2 * n_tbs {
                            r.s(x, p.g(-1 + x, -1));
                        }
                    }
                    for x in 0..n_tbs {
                        for y in 0..n_tbs {
                            let i_idx = ((y + 1) * angle) >> 5;
                            let i_fact = ((y + 1) * angle) & 31;
                            pred[x as usize][y as usize] = if i_fact != 0 {
                                ((32 - i_fact) * r.g(x + i_idx + 1) + i_fact * r.g(x + i_idx + 2) + 16) >> 5
                            } else {
                                r.g(x + i_idx + 1)
                            };
                        }
                    }
                    if m == 26 && c_idx == 0 && n_tbs < 32 {
                        for y in 0..n_tbs {
                            pred[0][y as usize] = clip1(p.g(0, -1) + ((p.g(-1, y) - p.g(-1, -1)) >> 1));
                        }
                    }
                } else {
                    for x in 0..=n_tbs {
                        r.s(x, p.g(-1, -1 + x));
                    }
                    if angle < 0 {
                        if (n_tbs * angle) >> 5 < -1 {
                            for x in (n_tbs * angle) >> 5..=-1 {
                                r.s(x, p.g(-1 + ((x * inv_angle(m) + 128) >> 8), -1));
                            }
                        }
                    } else {
                        for x in n_tbs + 1..=2 * n_tbs {
                            r.s(x, p.g(-1, -1 + x));
                        }
                    }
                    for x in 0..n_tbs {
                        for y in 0..n_tbs {
                            let i_idx = ((x + 1) * angle) >> 5;
                            let i_fact = ((x + 1) * angle) & 31;
                            pred[x as usize][y as usize] = if i_fact != 0 {
                                ((32 - i_fact) * r.g(y + i_idx + 1) + i_fact * r.g(y + i_idx + 2) + 16) >> 5
                            } else {
                                r.g(y + i_idx + 1)
                            };
                        }
                    }
                    if m == 10 && c_idx == 0 && n_tbs < 32 {
                        for x in 0..n_tbs {
                            pred[x as usize][0] = clip1(p.g(-1, 0) + ((p.g(x, -1) - p.g(-1, -1)) >> 1));
                        }
                    }
                }
            }
        }
        Vysledok { pred, bi_int }
    }

    // -----------------------------------------------------------------------------------------------------------
    // Náhodné vstupy
    // -----------------------------------------------------------------------------------------------------------

    fn nahodne(r: &mut Nahoda, n: u64) -> u64 {
        r.mensie_ako(n)
    }

    /// Hodnota v rozsahu 0 … 255 z ľubovoľného celého čísla.
    fn orez(v: i64) -> u16 {
        v.clamp(0, 255) as u16
    }

    /// Náhodní susedia bloku n × n: rôzne vzory hodnôt (aj hladké blízko prahu silného vyhladenia) a dostupnosti.
    fn nahodni_susedia(r: &mut Nahoda, n: usize) -> Susedia {
        let pocet = 4 * n + 1;
        let s2 = 2 * n;
        let mut s = Susedia::default();
        match nahodne(r, 6) {
            0 => {
                for v in &mut s.vzorky[..pocet] {
                    *v = nahodne(r, 256) as u16;
                }
            }
            1 => {
                // hladký priebeh so šumom
                let zaciatok = nahodne(r, 256) as i64;
                let sklon = nahodne(r, 9) as i64 - 4;
                for (i, v) in s.vzorky[..pocet].iter_mut().enumerate() {
                    let sum = nahodne(r, 5) as i64 - 2;
                    *v = orez(zaciatok + sklon * i as i64 / 4 + sum);
                }
            }
            2 => {
                let k = nahodne(r, 256) as u16;
                s.vzorky[..pocet].fill(k);
            }
            3 => {
                for v in &mut s.vzorky[..pocet] {
                    *v = if nahodne(r, 2) == 0 { 0 } else { 255 };
                }
            }
            4 => {
                // dve priamky (ľavý stĺpec a horný riadok) so šumom – blízko prahu biIntFlag
                let roh = nahodne(r, 256) as i64;
                let dole = nahodne(r, 256) as i64;
                let vpravo = nahodne(r, 256) as i64;
                for i in 0..=s2 {
                    let sum = nahodne(r, 9) as i64 - 4;
                    s.vzorky[i] = orez(dole + (roh - dole) * i as i64 / s2 as i64 + sum);
                }
                for j in 1..=s2 {
                    let sum = nahodne(r, 9) as i64 - 4;
                    s.vzorky[s2 + j] = orez(roh + (vpravo - roh) * j as i64 / s2 as i64 + sum);
                }
                s.vzorky[s2] = orez(roh);
            }
            _ => {
                let zaklad = nahodne(r, 248) as u16;
                for v in &mut s.vzorky[..pocet] {
                    *v = zaklad + nahodne(r, 8) as u16;
                }
            }
        }
        match nahodne(r, 7) {
            0 => s.dostupne[..pocet].fill(true),
            1 => {}
            2 => {
                for d in &mut s.dostupne[..pocet] {
                    *d = nahodne(r, 2) == 1;
                }
            }
            3 => {
                // dostupnosť po jednotkách (ako po blokoch 4×4 jasu)
                let jednotka = 1usize << nahodne(r, 4);
                let mut i = 0;
                while i < pocet {
                    let d = nahodne(r, 3) != 0;
                    for k in i..(i + jednotka).min(pocet) {
                        s.dostupne[k] = d;
                    }
                    i += jednotka;
                }
            }
            4 => {
                let k = nahodne(r, pocet as u64 + 1) as usize;
                s.dostupne[k..pocet].fill(true);
            }
            5 => {
                let k = nahodne(r, pocet as u64 + 1) as usize;
                s.dostupne[..k].fill(true);
            }
            _ => s.dostupne[nahodne(r, pocet as u64) as usize] = true,
        }
        // nedostupné vzorky dostanú náhodný obsah – nesmú sa nikde prejaviť
        for i in 0..pocet {
            if !s.dostupne[i] {
                s.vzorky[i] = nahodne(r, 256) as u16;
            }
        }
        s
    }

    const STRAZ: u8 = 0xA5;

    /// Spustí rýchlu predikciu s krokom `krok`, porovná ju s rozhodcom a overí, že mimo bloku sa nič nezapísalo.
    fn over(susedia: &Susedia, log2_n: u32, rezim: u32, jas: bool, silne: bool, krok: usize) -> bool {
        let n = 1usize << log2_n;
        let mut ciel = vec![STRAZ; (n - 1) * krok + n + 7];
        predikuj(susedia, log2_n, rezim, jas, 8, silne, &mut ciel, krok);
        let roz = rozhodca(susedia, n as i32, rezim as i32, if jas { 0 } else { 1 }, 8, silne);
        for (i, &v) in ciel.iter().enumerate() {
            let (x, y) = (i % krok, i / krok);
            if x < n && y < n {
                assert_eq!(
                    i32::from(v),
                    roz.pred[x][y],
                    "nTbS {n}, režim {rezim}, jas {jas}, silné {silne}, vzorka ({x}, {y}), susedia {:?}",
                    &susedia.vzorky[..4 * n + 1]
                );
            } else {
                assert_eq!(v, STRAZ, "zápis mimo bloku: nTbS {n}, režim {rezim}, index {i}");
            }
        }
        roz.bi_int
    }

    #[test]
    fn zhoda_s_rozhodcom() {
        const OPAKOVANI: usize = 40;
        let mut r = Nahoda::zo_semienka(214);
        let mut pripadov = 0;
        let mut silnych = 0;
        for log2_n in 2..=5u32 {
            let n = 1usize << log2_n;
            for rezim in 0..35u32 {
                for jas in [true, false] {
                    for silne in [false, true] {
                        for _ in 0..OPAKOVANI {
                            let susedia = nahodni_susedia(&mut r, n);
                            let krok = n + nahodne(&mut r, 9) as usize;
                            if over(&susedia, log2_n, rezim, jas, silne, krok) {
                                silnych += 1;
                            }
                            pripadov += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(pripadov, 4 * 35 * 2 * 2 * OPAKOVANI);
        // silné vyhladenie musí byť pokryté (32×32, jas, príznak, 33 filtrovaných režimov × 20 vstupov)
        assert!(silnych > 30, "silné vyhladenie pokryté len {silnych}-krát");
    }

    #[test]
    fn zhoda_s_rozhodcom_prah_silneho_vyhladenia() {
        // hladké priamky so šumom okolo prahu |…| < 8 – obe vetvy biIntFlag
        let mut r = Nahoda::zo_semienka(32);
        let (mut ano, mut nie) = (0, 0);
        for _ in 0..400 {
            let mut s = Susedia::default();
            let roh = nahodne(&mut r, 256) as i64;
            let dole = nahodne(&mut r, 256) as i64;
            let vpravo = nahodne(&mut r, 256) as i64;
            for i in 0..=64 {
                s.vzorky[i] = orez(dole + (roh - dole) * i as i64 / 64);
            }
            for j in 1..=64 {
                s.vzorky[64 + j] = orez(roh + (vpravo - roh) * j as i64 / 64);
            }
            s.vzorky[32] = orez(i64::from(s.vzorky[32]) + nahodne(&mut r, 11) as i64 - 5);
            s.vzorky[96] = orez(i64::from(s.vzorky[96]) + nahodne(&mut r, 11) as i64 - 5);
            s.dostupne[..129].fill(true);
            let rezim = [0, 2, 9, 18, 27, 34][nahodne(&mut r, 6) as usize];
            if over(&s, 5, rezim, true, true, 32) {
                ano += 1;
            } else {
                nie += 1;
            }
        }
        assert!(ano > 50 && nie > 50, "biIntFlag: áno {ano}, nie {nie}");
    }

    #[test]
    fn nic_dostupne_da_strednu_hodnotu() {
        let susedia = Susedia::default();
        for log2_n in 2..=5u32 {
            let n = 1usize << log2_n;
            for rezim in 0..35 {
                for jas in [true, false] {
                    let mut ciel = vec![0u8; n * n];
                    predikuj(&susedia, log2_n, rezim, jas, 8, true, &mut ciel, n);
                    assert!(ciel.iter().all(|&v| v == 128), "nTbS {n}, režim {rezim}");
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // Ručne overené príklady
    // -----------------------------------------------------------------------------------------------------------

    /// Všetko dostupné: `lavo[y]` = p[−1][y], `hore[x]` = p[x][−1] (y, x = 0 … 2N − 1).
    fn susedia_z(lavo: &[u16], roh: u16, hore: &[u16]) -> Susedia {
        let n2 = lavo.len();
        let mut s = Susedia::default();
        for (y, &v) in lavo.iter().enumerate() {
            s.vzorky[n2 - 1 - y] = v;
        }
        s.vzorky[n2] = roh;
        for (x, &v) in hore.iter().enumerate() {
            s.vzorky[n2 + 1 + x] = v;
        }
        s.dostupne[..2 * n2 + 1].fill(true);
        s
    }

    /// p[−1][y] = 60, 70, … 130; p[−1][−1] = 55; p[x][−1] = 10, 20, … 80.
    fn vzorovi_susedia_4x4() -> Susedia {
        susedia_z(&[60, 70, 80, 90, 100, 110, 120, 130], 55, &[10, 20, 30, 40, 50, 60, 70, 80])
    }

    /// Blok 4×4 po riadkoch: `[y][x]`.
    fn blok_4x4(s: &Susedia, rezim: u32, jas: bool) -> [[u8; 4]; 4] {
        let mut ciel = [0u8; 16];
        predikuj(s, 2, rezim, jas, 8, false, &mut ciel, 4);
        let mut b = [[0u8; 4]; 4];
        for y in 0..4 {
            b[y].copy_from_slice(&ciel[4 * y..4 * y + 4]);
        }
        b
    }

    #[test]
    fn rucne_planarna_4x4() {
        // napr. (0, 0): (3·60 + 1·50 + 3·10 + 1·100 + 4) >> 3 = 364 >> 3 = 45
        let s = vzorovi_susedia_4x4();
        let ocak = [[45, 48, 50, 53], [60, 60, 60, 60], [75, 73, 70, 68], [90, 85, 80, 75]];
        assert_eq!(blok_4x4(&s, 0, true), ocak);
        assert_eq!(blok_4x4(&s, 0, false), ocak);
    }

    #[test]
    fn rucne_dc_4x4() {
        // dcVal = (100 + 300 + 4) >> 3 = 50; (0, 0) = (60 + 100 + 10 + 2) >> 2 = 43
        let s = vzorovi_susedia_4x4();
        assert_eq!(
            blok_4x4(&s, 1, true),
            [[43, 43, 45, 48], [55, 50, 50, 50], [58, 50, 50, 50], [60, 50, 50, 50]]
        );
        assert_eq!(blok_4x4(&s, 1, false), [[50; 4]; 4]);
    }

    #[test]
    fn rucne_zvisla_26() {
        let s = vzorovi_susedia_4x4();
        // stĺpec 0: 10 + ((60 − 55) >> 1) = 12, 10 + (15 >> 1) = 17, …
        assert_eq!(
            blok_4x4(&s, 26, true),
            [[12, 20, 30, 40], [17, 20, 30, 40], [22, 20, 30, 40], [27, 20, 30, 40]]
        );
        assert_eq!(blok_4x4(&s, 26, false), [[10, 20, 30, 40]; 4]);
        // aritmetický posun: 10 + ((60 − 61) >> 1) = 10 − 1 = 9 (delenie so zaokrúhlením k nule by dalo 10)
        let s = susedia_z(&[60, 70, 80, 90, 0, 0, 0, 0], 61, &[10, 20, 30, 40, 0, 0, 0, 0]);
        let b = blok_4x4(&s, 26, true);
        assert_eq!([b[0][0], b[1][0], b[2][0], b[3][0]], [9, 14, 19, 24]);
        // orezanie Clip1: 10 + ((0 − 200) >> 1) = −90 → 0; 250 + ((255 − 0) >> 1) = 377 → 255
        let s = susedia_z(&[0; 8], 200, &[10; 8]);
        assert_eq!(blok_4x4(&s, 26, true)[2][0], 0);
        let s = susedia_z(&[255; 8], 0, &[250; 8]);
        assert_eq!(blok_4x4(&s, 26, true)[1][0], 255);
    }

    #[test]
    fn rucne_vodorovna_10() {
        // riadok 0: 60 + ((10 − 55) >> 1) = 60 − 23 = 37, …
        let s = vzorovi_susedia_4x4();
        assert_eq!(blok_4x4(&s, 10, true), [[37, 42, 47, 52], [70; 4], [80; 4], [90; 4]]);
        assert_eq!(blok_4x4(&s, 10, false), [[60; 4], [70; 4], [80; 4], [90; 4]]);
    }

    #[test]
    fn rucne_diagonaly() {
        let s = vzorovi_susedia_4x4();
        // 34: predSamples[x][y] = p[x + y + 1][−1]
        assert_eq!(
            blok_4x4(&s, 34, true),
            [[20, 30, 40, 50], [30, 40, 50, 60], [40, 50, 60, 70], [50, 60, 70, 80]]
        );
        // 2: predSamples[x][y] = p[−1][x + y + 1]
        assert_eq!(
            blok_4x4(&s, 2, true),
            [[70, 80, 90, 100], [80, 90, 100, 110], [90, 100, 110, 120], [100, 110, 120, 130]]
        );
        // 18: x ≥ y → p[x − y − 1][−1], inak p[−1][y − x − 1] (projekcia ľavého stĺpca, invAngle = −256)
        assert_eq!(
            blok_4x4(&s, 18, true),
            [[55, 10, 20, 30], [60, 55, 10, 20], [70, 60, 55, 10], [80, 70, 60, 55]]
        );
    }

    #[test]
    fn rucne_interpolacia() {
        let s = vzorovi_susedia_4x4();
        // 27 (uhol 2): iFact = 2 (y + 1); (0, 1) = (28·10 + 4·20 + 16) >> 5 = 376 >> 5 = 11
        assert_eq!(
            blok_4x4(&s, 27, true),
            [[11, 21, 31, 41], [11, 21, 31, 41], [12, 22, 32, 42], [13, 23, 33, 43]]
        );
        // 9 (uhol 2, vodorovný – transpozícia): (x, y) = ((32 − 2(x + 1))·p[−1][y] + 2(x + 1)·p[−1][y + 1] + 16) >> 5
        assert_eq!(
            blok_4x4(&s, 9, true),
            [[61, 61, 62, 63], [71, 71, 72, 73], [81, 81, 82, 83], [91, 91, 92, 93]]
        );
        // 25 (uhol −2): iIdx = −1, iFact = 30, 28, 26, 24; ref[0] = roh; (0, 0) = (2·55 + 30·10 + 16) >> 5 = 13
        assert_eq!(
            blok_4x4(&s, 25, true),
            [[13, 19, 29, 39], [16, 19, 29, 39], [18, 18, 28, 38], [21, 18, 28, 38]]
        );
    }

    #[test]
    fn rucne_projekcia_32x32_rezim_11() {
        // 11 (uhol −2, invAngle −4096), nTbS 32: (32·−2) >> 5 = −2 → ref[−1] = p[15][−1], ref[−2] = p[31][−1]
        let lavo: Vec<u16> = (0..64).map(|y| 200 - y).collect();
        let hore: Vec<u16> = (0..64).map(|x| 100 + x).collect();
        let s = susedia_z(&lavo, 7, &hore);
        let mut ciel = vec![0u8; 32 * 32];
        predikuj(&s, 5, 11, false, 8, false, &mut ciel, 32);
        // stĺpec 31: iIdx = −2, iFact = 0 → predSamples[31][y] = ref[y − 1]
        assert_eq!(ciel[31], 115); // ref[−1] = p[15][−1]
        assert_eq!(ciel[32 + 31], 7); // ref[0] = p[−1][−1]
        assert_eq!(ciel[2 * 32 + 31], 200); // ref[1] = p[−1][0]
        // stĺpec 16: (17·−2) = −34 → iIdx = −2, iFact = 30: (2·ref[−1] + 30·ref[0] + 16) >> 5 = (230 + 210 + 16) >> 5
        assert_eq!(ciel[16], 14);
    }

    #[test]
    fn rucne_nahrada() {
        let vzorky: Vec<u16> = (0..17).map(|i| i * 10).collect();
        let mut p = [0u8; 17];
        // dostupné len 5 a 12: začiatok dostane prvú dostupnú, potom sa opakuje predošlá
        let mut d = [false; 17];
        d[5] = true;
        d[12] = true;
        nahrad(&vzorky, &d, 8, &mut p);
        assert_eq!(p, [50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 120, 120, 120, 120, 120]);
        // dostupné len 0
        let mut d = [false; 17];
        d[0] = true;
        nahrad(&vzorky, &d, 8, &mut p);
        assert_eq!(p, [0; 17]);
        // nič dostupné
        nahrad(&vzorky, &[false; 17], 8, &mut p);
        assert_eq!(p, [128; 17]);
        // všetko dostupné
        nahrad(&vzorky, &[true; 17], 8, &mut p);
        assert!(p.iter().zip(&vzorky).all(|(&a, &b)| u16::from(a) == b));
    }

    #[test]
    fn rucne_filter_121() {
        let mut f = [0u8; 5];
        filtruj_121(&[0, 4, 8, 0, 255], &mut f);
        // (0 + 8 + 8 + 2) >> 2 = 4, (4 + 16 + 0 + 2) >> 2 = 5, (8 + 0 + 255 + 2) >> 2 = 66; konce bez zmeny
        assert_eq!(f, [0, 4, 5, 66, 255]);
    }

    #[test]
    fn rozhodnutie_o_filtrovani() {
        let filtrovane = |n: usize| (0..35).filter(|&m| filtrovat(m, n)).collect::<Vec<u32>>();
        assert!(filtrovane(4).is_empty());
        assert_eq!(filtrovane(8), [0, 2, 18, 34]);
        let ocak16: Vec<u32> = (0..35).filter(|m| ![1, 9, 10, 11, 25, 26, 27].contains(m)).collect();
        assert_eq!(filtrovane(16), ocak16);
        let ocak32: Vec<u32> = (0..35).filter(|m| ![1, 10, 26].contains(m)).collect();
        assert_eq!(filtrovane(32), ocak32);
    }

    #[test]
    fn rucne_silne_vyhladenie() {
        // p[−1][y] = 32 (p[−1][63] = 64), roh 0, p[x][−1] = 64 (p[63][−1] = 128): |0 + 128 − 2·64| = 0,
        // |0 + 64 − 2·32| = 0 → bilineárne: pF[−1][y] = y + 1, pF[x][−1] = 2x + 2
        let mut lavo = [32u16; 64];
        lavo[63] = 64;
        let mut hore = [64u16; 64];
        hore[63] = 128;
        let s = susedia_z(&lavo, 0, &hore);
        let mut p = [0u8; MAX_SUSEDOV];
        nahrad(&s.vzorky, &s.dostupne, 8, &mut p);
        assert!(bilinearne(&p, 8));
        let mut pf = [0u8; MAX_SUSEDOV];
        silne_vyhlad(&p, &mut pf);
        for y in 0..64 {
            let ocak = if y == 63 { 64 } else { y as u8 + 1 };
            assert_eq!(pf[63 - y], ocak, "pF[−1][{y}]");
        }
        assert_eq!(pf[64], 0);
        for x in 0..64 {
            let ocak = if x == 63 { 128 } else { 2 * x as u8 + 2 };
            assert_eq!(pf[65 + x], ocak, "pF[{x}][−1]");
        }
        // planárna 32×32: p[32][−1] = 66, p[−1][32] = 33;
        // (0, 0) = (31·1 + 1·66 + 31·2 + 1·33 + 32) >> 6 = 3, (31, 0) = (32·66 + 31·64 + 33 + 32) >> 6 = 65,
        // (0, 31) = (31·32 + 66 + 32·33 + 32) >> 6 = 33, (31, 31) = (32·66 + 32·33 + 32) >> 6 = 50
        let mut ciel = vec![0u8; 32 * 32];
        predikuj(&s, 5, 0, true, 8, true, &mut ciel, 32);
        assert_eq!([ciel[0], ciel[31], ciel[31 * 32], ciel[31 * 32 + 31]], [3, 65, 33, 50]);
        // bez príznaku sa použije [1 2 1] – iný výsledok; farba sa nefiltruje vôbec
        let mut bez = vec![0u8; 32 * 32];
        predikuj(&s, 5, 0, true, 8, false, &mut bez, 32);
        assert_ne!(ciel, bez);
        assert!(!over(&s, 5, 0, true, false, 32));
        assert!(over(&s, 5, 0, true, true, 32));
        assert!(!over(&s, 5, 0, false, true, 32));
    }

    #[test]
    fn prah_silneho_vyhladenia() {
        // |roh + p[63][−1] − 2·p[31][−1]| < 8: 7 áno, 8 a 9 nie (ľavý stĺpec presne priamka)
        let mut p = [0u8; MAX_SUSEDOV];
        p[0] = 64;
        p[32] = 32;
        p[64] = 0;
        p[128] = 127;
        p[96] = 60; // |127 − 120| = 7
        assert!(bilinearne(&p, 8));
        p[96] = 68; // |127 − 136| = 9
        assert!(!bilinearne(&p, 8));
        p[128] = 128;
        p[96] = 60; // |128 − 120| = 8
        assert!(!bilinearne(&p, 8));
        // to isté pre ľavý stĺpec
        p[96] = 64;
        assert!(bilinearne(&p, 8));
        p[32] = 28; // |64 − 56| = 8
        assert!(!bilinearne(&p, 8));
        p[32] = 29; // |64 − 58| = 6
        assert!(bilinearne(&p, 8));
    }

    #[test]
    fn tabulky_uhlov() {
        for m in 2..=34usize {
            assert_eq!(i32::from(UHOL[m]), intra_pred_angle(m as i32), "intraPredAngle[{m}]");
            assert_eq!(UHOL[m], UHOL[36 - m], "symetria režimov {m} a {}", 36 - m);
        }
        for m in 11..=25usize {
            assert_eq!(i32::from(INV_UHOL[m - 11]), inv_angle(m as i32), "invAngle[{m}]");
            // invAngle = Round(256 · 32 / intraPredAngle)
            let presne = 8192.0 / f64::from(UHOL[m]);
            assert_eq!(f64::from(INV_UHOL[m - 11]), presne.round(), "invAngle[{m}]");
        }
    }
}
