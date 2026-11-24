//! Spätné kvantovanie a spätná transformácia (dequantization, inverse transform) – H.265, kap. 7.4.5, 8.6.2–8.6.4,
//! 8.6.7.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`FaktorySkalovania`], [`ParametreTb`], [`rekonstruuj`].
//!
//! Postup pre jeden transformačný blok nTbS × nTbS (hodnota (x, y) je v rastri na indexe `y * n + x`):
//! 1. spätné kvantovanie (8.6.3) len v ľavom hornom obdĺžniku nenulových koeficientov, priamo v poli `koef` –
//!    výsledok d[x][y] je orezaný na 16 bitov, zmestí sa do i16,
//! 2. 2D spätná transformácia (8.6.4.2) rozložená na 1D transformácie stĺpcov a riadkov. 1D DCT používa párny/
//!    nepárny rozklad („motýlik“): N-bodová transformácia = N/2-bodová transformácia párnych koeficientov + súčet
//!    nepárnych koeficientov s nepárnymi bázami na polovici polôh. Bázy matice normy sú presne symetrické
//!    (párne) a antisymetrické (nepárne), preto je rozklad bitovo zhodný s násobením celou maticou,
//! 3. pripočítanie rezíduí k predikcii s orezaním (8.6.7).
//!
//! Rýchle cesty (výsledok nemenia – skúšky ich porovnávajú s priamym prepisom vzorcov normy): samotný DC
//! koeficient, len prvý riadok koeficientov (všetky riadky rezíduí sú rovnaké) a skrátenie 1D transformácií podľa
//! `max_x`/`max_y`. Pracovné polia sú na zásobníku (žiadne alokácie na blok), žiadne `unsafe`. 1D transformácie sú
//! samostatné funkcie, aby ich krok H5 mohol nahradiť verziou NEON.

use crate::obraz::Vzorka;
use orchidea_hevc::parametre::MaticeKvantovania;

/// Celočíselná matica DCT 32 × 32 (8.6.4.2, trType = 0; v norme transMatrixCol0to15 a transMatrixCol16to31).
/// `MATICA_DCT[k][i]`: k = index bázy (koeficientu), i = poloha vzorky. N-bodová transformácia (N = 4, 8, 16)
/// používa bázy k · 32 / N obmedzené na prvých N polôh.
#[rustfmt::skip]
const MATICA_DCT: [[i8; 32]; 32] = [
    [ 64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,
      64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64], // k = 0
    [ 90,  90,  88,  85,  82,  78,  73,  67,  61,  54,  46,  38,  31,  22,  13,   4,
      -4, -13, -22, -31, -38, -46, -54, -61, -67, -73, -78, -82, -85, -88, -90, -90], // k = 1
    [ 90,  87,  80,  70,  57,  43,  25,   9,  -9, -25, -43, -57, -70, -80, -87, -90,
     -90, -87, -80, -70, -57, -43, -25,  -9,   9,  25,  43,  57,  70,  80,  87,  90], // k = 2
    [ 90,  82,  67,  46,  22,  -4, -31, -54, -73, -85, -90, -88, -78, -61, -38, -13,
      13,  38,  61,  78,  88,  90,  85,  73,  54,  31,   4, -22, -46, -67, -82, -90], // k = 3
    [ 89,  75,  50,  18, -18, -50, -75, -89, -89, -75, -50, -18,  18,  50,  75,  89,
      89,  75,  50,  18, -18, -50, -75, -89, -89, -75, -50, -18,  18,  50,  75,  89], // k = 4
    [ 88,  67,  31, -13, -54, -82, -90, -78, -46,  -4,  38,  73,  90,  85,  61,  22,
     -22, -61, -85, -90, -73, -38,   4,  46,  78,  90,  82,  54,  13, -31, -67, -88], // k = 5
    [ 87,  57,   9, -43, -80, -90, -70, -25,  25,  70,  90,  80,  43,  -9, -57, -87,
     -87, -57,  -9,  43,  80,  90,  70,  25, -25, -70, -90, -80, -43,   9,  57,  87], // k = 6
    [ 85,  46, -13, -67, -90, -73, -22,  38,  82,  88,  54,  -4, -61, -90, -78, -31,
      31,  78,  90,  61,   4, -54, -88, -82, -38,  22,  73,  90,  67,  13, -46, -85], // k = 7
    [ 83,  36, -36, -83, -83, -36,  36,  83,  83,  36, -36, -83, -83, -36,  36,  83,
      83,  36, -36, -83, -83, -36,  36,  83,  83,  36, -36, -83, -83, -36,  36,  83], // k = 8
    [ 82,  22, -54, -90, -61,  13,  78,  85,  31, -46, -90, -67,   4,  73,  88,  38,
     -38, -88, -73,  -4,  67,  90,  46, -31, -85, -78, -13,  61,  90,  54, -22, -82], // k = 9
    [ 80,   9, -70, -87, -25,  57,  90,  43, -43, -90, -57,  25,  87,  70,  -9, -80,
     -80,  -9,  70,  87,  25, -57, -90, -43,  43,  90,  57, -25, -87, -70,   9,  80], // k = 10
    [ 78,  -4, -82, -73,  13,  85,  67, -22, -88, -61,  31,  90,  54, -38, -90, -46,
      46,  90,  38, -54, -90, -31,  61,  88,  22, -67, -85, -13,  73,  82,   4, -78], // k = 11
    [ 75, -18, -89, -50,  50,  89,  18, -75, -75,  18,  89,  50, -50, -89, -18,  75,
      75, -18, -89, -50,  50,  89,  18, -75, -75,  18,  89,  50, -50, -89, -18,  75], // k = 12
    [ 73, -31, -90, -22,  78,  67, -38, -90, -13,  82,  61, -46, -88,  -4,  85,  54,
     -54, -85,   4,  88,  46, -61, -82,  13,  90,  38, -67, -78,  22,  90,  31, -73], // k = 13
    [ 70, -43, -87,   9,  90,  25, -80, -57,  57,  80, -25, -90,  -9,  87,  43, -70,
     -70,  43,  87,  -9, -90, -25,  80,  57, -57, -80,  25,  90,   9, -87, -43,  70], // k = 14
    [ 67, -54, -78,  38,  85, -22, -90,   4,  90,  13, -88, -31,  82,  46, -73, -61,
      61,  73, -46, -82,  31,  88, -13, -90,  -4,  90,  22, -85, -38,  78,  54, -67], // k = 15
    [ 64, -64, -64,  64,  64, -64, -64,  64,  64, -64, -64,  64,  64, -64, -64,  64,
      64, -64, -64,  64,  64, -64, -64,  64,  64, -64, -64,  64,  64, -64, -64,  64], // k = 16
    [ 61, -73, -46,  82,  31, -88, -13,  90,  -4, -90,  22,  85, -38, -78,  54,  67,
     -67, -54,  78,  38, -85, -22,  90,   4, -90,  13,  88, -31, -82,  46,  73, -61], // k = 17
    [ 57, -80, -25,  90,  -9, -87,  43,  70, -70, -43,  87,   9, -90,  25,  80, -57,
     -57,  80,  25, -90,   9,  87, -43, -70,  70,  43, -87,  -9,  90, -25, -80,  57], // k = 18
    [ 54, -85,  -4,  88, -46, -61,  82,  13, -90,  38,  67, -78, -22,  90, -31, -73,
      73,  31, -90,  22,  78, -67, -38,  90, -13, -82,  61,  46, -88,   4,  85, -54], // k = 19
    [ 50, -89,  18,  75, -75, -18,  89, -50, -50,  89, -18, -75,  75,  18, -89,  50,
      50, -89,  18,  75, -75, -18,  89, -50, -50,  89, -18, -75,  75,  18, -89,  50], // k = 20
    [ 46, -90,  38,  54, -90,  31,  61, -88,  22,  67, -85,  13,  73, -82,   4,  78,
     -78,  -4,  82, -73, -13,  85, -67, -22,  88, -61, -31,  90, -54, -38,  90, -46], // k = 21
    [ 43, -90,  57,  25, -87,  70,   9, -80,  80,  -9, -70,  87, -25, -57,  90, -43,
     -43,  90, -57, -25,  87, -70,  -9,  80, -80,   9,  70, -87,  25,  57, -90,  43], // k = 22
    [ 38, -88,  73,  -4, -67,  90, -46, -31,  85, -78,  13,  61, -90,  54,  22, -82,
      82, -22, -54,  90, -61, -13,  78, -85,  31,  46, -90,  67,   4, -73,  88, -38], // k = 23
    [ 36, -83,  83, -36, -36,  83, -83,  36,  36, -83,  83, -36, -36,  83, -83,  36,
      36, -83,  83, -36, -36,  83, -83,  36,  36, -83,  83, -36, -36,  83, -83,  36], // k = 24
    [ 31, -78,  90, -61,   4,  54, -88,  82, -38, -22,  73, -90,  67, -13, -46,  85,
     -85,  46,  13, -67,  90, -73,  22,  38, -82,  88, -54,  -4,  61, -90,  78, -31], // k = 25
    [ 25, -70,  90, -80,  43,   9, -57,  87, -87,  57,  -9, -43,  80, -90,  70, -25,
     -25,  70, -90,  80, -43,  -9,  57, -87,  87, -57,   9,  43, -80,  90, -70,  25], // k = 26
    [ 22, -61,  85, -90,  73, -38,  -4,  46, -78,  90, -82,  54, -13, -31,  67, -88,
      88, -67,  31,  13, -54,  82, -90,  78, -46,   4,  38, -73,  90, -85,  61, -22], // k = 27
    [ 18, -50,  75, -89,  89, -75,  50, -18, -18,  50, -75,  89, -89,  75, -50,  18,
      18, -50,  75, -89,  89, -75,  50, -18, -18,  50, -75,  89, -89,  75, -50,  18], // k = 28
    [ 13, -38,  61, -78,  88, -90,  85, -73,  54, -31,   4,  22, -46,  67, -82,  90,
     -90,  82, -67,  46, -22,  -4,  31, -54,  73, -85,  90, -88,  78, -61,  38, -13], // k = 29
    [  9, -25,  43, -57,  70, -80,  87, -90,  90, -87,  80, -70,  57, -43,  25,  -9,
      -9,  25, -43,  57, -70,  80, -87,  90, -90,  87, -80,  70, -57,  43, -25,   9], // k = 30
    [  4, -13,  22, -31,  38, -46,  54, -61,  67, -73,  78, -82,  85, -88,  90, -90,
      90, -90,  88, -85,  82, -78,  73, -67,  61, -54,  46, -38,  31, -22,  13,  -4], // k = 31
];

/// Matica DST-VII 4 × 4 (8.6.4.2, trType = 1 – jas vnútrosnímkovej jednotky 4 × 4): `MATICA_DST[k][i]`,
/// k = báza, i = poloha.
const MATICA_DST: [[i8; 4]; 4] = [[29, 55, 74, 84], [74, 74, 0, -74], [84, -29, -74, 55], [55, -84, 74, -29]];

/// Báza 0 DCT (konštanta 64 na všetkých polohách) a hodnota bázy 16 na polohe 0 (64; na polohe 1 je −64).
const DCT_DC: i32 = MATICA_DCT[0][0] as i32;
const DCT_16: i32 = MATICA_DCT[16][0] as i32;

/// Nepárne bázy N-bodovej DCT na prvých N/2 polohách: `t[m][i] = MATICA_DCT[krok · (2m + 1)][i]`,
/// krok = 32 / N, P = N / 2.
const fn neparne_bazy<const P: usize>(krok: usize) -> [[i16; P]; P] {
    let mut t = [[0i16; P]; P];
    let mut m = 0;
    while m < P {
        let mut i = 0;
        while i < P {
            t[m][i] = MATICA_DCT[krok * (2 * m + 1)][i] as i16;
            i += 1;
        }
        m += 1;
    }
    t
}

const NEPARNE_4: [[i16; 2]; 2] = neparne_bazy::<2>(8);
const NEPARNE_8: [[i16; 4]; 4] = neparne_bazy::<4>(4);
const NEPARNE_16: [[i16; 8]; 8] = neparne_bazy::<8>(2);
const NEPARNE_32: [[i16; 16]; 16] = neparne_bazy::<16>(1);

/// levelScale[qP % 6] (8.6.3).
const MIERKA_UROVNE: [i32; 6] = [40, 45, 51, 57, 64, 72];

/// Diagonálny prechod nahor doprava (6.5.3, ScanOrder[log2][0]) bloku `velkost` × `velkost`: (x, y) pre každú
/// polohu prechodu. Uhlopriečky sa prechádzajú zdola zľava nahor doprava: (0, 0), (0, 1), (1, 0), (0, 2), …
const fn diagonalny_prechod<const P: usize>(velkost: usize) -> [(u8, u8); P] {
    let mut prechod = [(0u8, 0u8); P];
    let mut i = 0;
    let mut x: isize = 0;
    let mut y: isize = 0;
    while i < P {
        while y >= 0 {
            if (x as usize) < velkost && (y as usize) < velkost {
                prechod[i] = (x as u8, y as u8);
                i += 1;
            }
            y -= 1;
            x += 1;
        }
        y = x;
        x = 0;
    }
    prechod
}

const PRECHOD_4: [(u8, u8); 16] = diagonalny_prechod::<16>(4);
const PRECHOD_8: [(u8, u8); 64] = diagonalny_prechod::<64>(8);

/// Faktory skalovania m[x][y] (ScalingFactor, 7.4.5) pre veľkosti 4, 8, 16, 32 a matice matrixId 0 … 5
/// (matrixId = cIdx + 3 pri medzisnímkovej predikcii). Každý faktor je štvorec n × n v rastri: index `y * n + x`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaktorySkalovania {
    /// `faktory[log2_n − 2][matrixId]` má n · n hodnôt
    pub faktory: [[Vec<u8>; 6]; 4],
}

impl FaktorySkalovania {
    /// Rozbalí matice kvantovania (zoznamy v poradí prechodu + DC) na faktory v rastri podľa 7.4.5:
    /// - 4 × 4: ScalingFactor[0][m][x][y] = ScalingList[0][m][i], (x, y) = ScanOrder[2][0][i],
    /// - 8 × 8: ScalingFactor[1][m][x][y] = ScalingList[1][m][i], (x, y) = ScanOrder[3][0][i],
    /// - 16 × 16 a 32 × 32: každá hodnota 8 × 8 zoznamu pokryje 2 × 2 resp. 4 × 4 vzoriek
    ///   (ScalingFactor[s][m][x · r + k][y · r + j]) a [0][0] sa prepíše hodnotou DC
    ///   (scaling_list_dc_coef_minus8 + 8).
    ///
    /// Norma píše ScalingFactor[…][x][y], výstup je v rastri `y * n + x`.
    pub fn z_matic(matice: &MaticeKvantovania) -> FaktorySkalovania {
        FaktorySkalovania {
            faktory: std::array::from_fn(|velkost| {
                std::array::from_fn(|matica| rozbal_maticu(matice, velkost, matica))
            }),
        }
    }

    /// Faktor pre blok 2^`log2_n` a `matica_id` (raster n × n).
    pub fn faktor(&self, log2_n: u32, matica_id: usize) -> &[u8] {
        &self.faktory[log2_n as usize - 2][matica_id]
    }
}

/// Jedna matica faktorov skalovania (7.4.5) – `velkost` = sizeId (0 … 3), `matica` = matrixId.
fn rozbal_maticu(matice: &MaticeKvantovania, velkost: usize, matica: usize) -> Vec<u8> {
    let zoznam = &matice.zoznamy[velkost][matica];
    let n = 4 << velkost;
    let mut f = vec![0u8; n * n];
    if velkost == 0 {
        debug_assert_eq!(zoznam.len(), 16);
        for (&(x, y), &hodnota) in PRECHOD_4.iter().zip(zoznam) {
            f[y as usize * 4 + x as usize] = hodnota;
        }
        return f;
    }
    debug_assert_eq!(zoznam.len(), 64);
    // pomer rozšírenia r = n / 8 (1, 2 alebo 4)
    let r = n / 8;
    for (&(x, y), &hodnota) in PRECHOD_8.iter().zip(zoznam) {
        let (x, y) = (x as usize * r, y as usize * r);
        for j in 0..r {
            f[(y + j) * n + x..(y + j) * n + x + r].fill(hodnota);
        }
    }
    if velkost >= 2 {
        f[0] = matice.dc[velkost - 2][matica];
    }
    f
}

/// Parametre jedného transformačného bloku.
#[derive(Debug, Clone, Copy)]
pub struct ParametreTb<'a> {
    /// log2 veľkosti bloku (2 … 5)
    pub log2_n: u32,
    /// zložka jasu (cIdx = 0)
    pub jas: bool,
    /// kódovacia jednotka je vnútrosnímková (CuPredMode = MODE_INTRA) – pri jase 4×4 sa použije DST
    pub intra: bool,
    /// kvantizačný parameter Qp′Y, Qp′Cb alebo Qp′Cr (vrátane QpBdOffset)
    pub qp: i32,
    /// transform_skip_flag
    pub preskocenie: bool,
    /// cu_transquant_bypass_flag – rezíduá sú priamo TransCoeffLevel (bez kvantovania a transformácie)
    pub obchadzanie: bool,
    /// faktory skalovania m[x][y] (raster n × n); `None` = plochý faktor 16 (scaling_list_enabled_flag = 0)
    pub faktory: Option<&'a [u8]>,
    pub bitova_hlbka: u32,
}

/// Rekonštrukcia jedného transformačného bloku:
/// 1. spätné kvantovanie TransCoeffLevel → d[x][y] (8.6.3, orezanie na −32768 … 32767),
/// 2. spätná transformácia (8.6.4.2: DST 4×4 pre jas vnútrosnímkovej jednotky, inak DCT 4 … 32; prvý stupeň
///    zvislo s posunom 7 a orezaním na 16 bitov, druhý vodorovne s posunom 20 − BitDepth),
///    alebo preskočenie transformácie (transform_skip) či obchádzanie (cu_transquant_bypass),
/// 3. pripočítanie rezíduí k predikcii, ktorá už je v `ciel`, s orezaním na 0 … (1 << BitDepth) − 1 (8.6.7).
///
/// `koef`: TransCoeffLevel v rastri n × n (index `y * n + x`); smie sa použiť ako pracovná pamäť.
/// `max_x`, `max_y`: najväčšie súradnice nenulového koeficientu – smú sa použiť na skrátenie výpočtu, nesmú
/// zmeniť výsledok. Vzorka (x, y) cieľa je `ciel[y * krok + x]`.
pub fn rekonstruuj(
    koef: &mut [i16],
    max_x: usize,
    max_y: usize,
    p: &ParametreTb,
    ciel: &mut [Vzorka],
    krok: usize,
) {
    // Clip1 (8.6.7): recSamples = Clip1(predSamples + resSamples)
    let max_vzorka = (1i32 << p.bitova_hlbka) - 1;
    rezidua(koef, max_x, max_y, p, |y, rez| {
        let riadok = &mut ciel[y * krok..y * krok + rez.len()];
        for (v, &r) in riadok.iter_mut().zip(rez) {
            *v = (i32::from(*v) + r).clamp(0, max_vzorka) as Vzorka;
        }
    });
}

/// Rezíduá bloku (8.6.2): pre každý riadok y, ktorý môže mať nenulové rezíduá, zavolá `zapis(y, rez)`, kde
/// `rez[x]` je rezíduum vzorky (x, y) pre x = 0 … rez.len() − 1; všetky ostatné rezíduá bloku sú nulové.
fn rezidua(
    koef: &mut [i16],
    max_x: usize,
    max_y: usize,
    p: &ParametreTb,
    mut zapis: impl FnMut(usize, &[i32]),
) {
    let n = 1usize << p.log2_n;
    debug_assert!((2..=5).contains(&p.log2_n), "nTbS mimo 4 … 32");
    debug_assert!(max_x < n && max_y < n && koef.len() >= n * n);
    debug_assert!(p.bitova_hlbka >= 8 && p.bitova_hlbka <= Vzorka::BITS && p.qp >= 0);
    let sirka = max_x + 1;
    let mut rez = [0i32; 32];
    if p.obchadzanie {
        // cu_transquant_bypass_flag = 1: r[x][y] = TransCoeffLevel[x][y] (bez posunu bdShift)
        for y in 0..=max_y {
            for (r, &c) in rez.iter_mut().zip(&koef[y * n..y * n + sirka]) {
                *r = i32::from(c);
            }
            zapis(y, &rez[..sirka]);
        }
        return;
    }
    kvantuj_spat(koef, n, max_x, max_y, p);
    // bdShift = Max(20 − bitDepth, 0) bez extended_precision_processing_flag (8.6.2)
    let posun = 20 - p.bitova_hlbka;
    let zaokruhlenie = 1i32 << (posun - 1);
    if p.preskocenie {
        // transform_skip_flag = 1 (8.6.4.2): r = d << tsShift, tsShift = 5 + Log2(nTbS); potom ako po
        // transformácii (r + (1 << (bdShift − 1))) >> bdShift. Nulové d dá nulové rezíduum.
        let ts_posun = 5 + p.log2_n;
        for y in 0..=max_y {
            for (r, &d) in rez.iter_mut().zip(&koef[y * n..y * n + sirka]) {
                *r = ((i32::from(d) << ts_posun) + zaokruhlenie) >> posun;
            }
            zapis(y, &rez[..sirka]);
        }
        return;
    }
    // trType = 1 (DST) pre CuPredMode = MODE_INTRA, nTbS = 4 a cIdx = 0, inak DCT (8.6.4.2)
    let dst = p.log2_n == 2 && p.jas && p.intra;
    match p.log2_n {
        2 => transformuj::<4>(koef, max_x, max_y, dst, posun, zapis),
        3 => transformuj::<8>(koef, max_x, max_y, false, posun, zapis),
        4 => transformuj::<16>(koef, max_x, max_y, false, posun, zapis),
        _ => transformuj::<32>(koef, max_x, max_y, false, posun, zapis),
    }
}

/// Konštanty spätného kvantovania jedného bloku (8.6.3), s ktorými výpočet stačí v 32 bitoch.
///
/// Norma: d = Clip3(−32768, 32767, ((c · s << q) + (1 << (b − 1))) >> b), s = m · levelScale[qP % 6],
/// q = qP / 6, b = bdShift. Súčin |c · s| ≤ 32768 · 255 · 72 < 2^30 sa zmestí do i32, posun o q nie vždy. Preto:
/// - q < b: ((c·s << q) + 2^(b−1)) >> b = (c·s + 2^(b−q−1)) >> (b − q) (presne, obe strany sú delenie 2^b
///   so zaokrúhlením nadol),
/// - q ≥ b: zaokrúhľovacia konštanta je menšia ako 2^b a c·s << q je násobok 2^b, teda výsledok je c·s << (q − b);
///   Clip3(x << k) = Clip3(Clip3(x) << k) pre k ≥ 0 (znamienko sa nemení a ±32768 << 1 je už mimo rozsahu)
///   a pre k > 16 je výsledok rovnaký ako pre k = 16 (nenulová hodnota je už mimo rozsahu), takže stačí i32.
///
/// Spoločný tvar: d = Clip3(Clip3((c·s + zaokruhlenie) >> vpravo) << vlavo).
#[derive(Debug, Clone, Copy)]
struct Kvantovanie {
    vpravo: u32,
    zaokruhlenie: i32,
    vlavo: u32,
}

impl Kvantovanie {
    /// `qp` = qP (≥ 0), `bd_posun` = bdShift = BitDepth + Log2(nTbS) − 5 (≥ 1).
    fn nove(qp: i32, bd_posun: u32) -> Kvantovanie {
        let q = (qp / 6) as u32;
        if q < bd_posun {
            let vpravo = bd_posun - q;
            Kvantovanie { vpravo, zaokruhlenie: 1 << (vpravo - 1), vlavo: 0 }
        } else {
            Kvantovanie { vpravo: 0, zaokruhlenie: 0, vlavo: (q - bd_posun).min(16) }
        }
    }

    /// d pre koeficient `c` a súčin `s` = m · levelScale[qP % 6].
    #[inline(always)]
    fn pouzi(self, c: i16, s: i32) -> i16 {
        let x = i32::from(c) * s;
        orez16(i32::from(orez16((x + self.zaokruhlenie) >> self.vpravo)) << self.vlavo)
    }
}

/// Clip3(−32768, 32767, x) (coeffMin, coeffMax pri log2TransformRange = 15).
#[inline(always)]
fn orez16(x: i32) -> i16 {
    x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Spätné kvantovanie (8.6.3) obdĺžnika 0 … `max_x` × 0 … `max_y` priamo v poli koeficientov. Nulový koeficient dá
/// nulové d (zaokrúhľovacia konštanta je menšia ako 1 << bdShift), preto sa zvyšok bloku nemusí prechádzať.
fn kvantuj_spat(koef: &mut [i16], n: usize, max_x: usize, max_y: usize, p: &ParametreTb) {
    let mierka = MIERKA_UROVNE[(p.qp % 6) as usize];
    let k = Kvantovanie::nove(p.qp, p.bitova_hlbka + p.log2_n - 5);
    // m[x][y] = 16, ak scaling_list_enabled_flag = 0 alebo transform_skip_flag = 1 a nTbS > 4 (8.6.3)
    let faktory = p.faktory.filter(|_| !(p.preskocenie && p.log2_n > 2));
    for y in 0..=max_y {
        let riadok = &mut koef[y * n..y * n + max_x + 1];
        match faktory {
            None => {
                let s = 16 * mierka;
                for c in riadok {
                    *c = k.pouzi(*c, s);
                }
            }
            Some(f) => {
                for (c, &m) in riadok.iter_mut().zip(&f[y * n..]) {
                    *c = k.pouzi(*c, i32::from(m) * mierka);
                }
            }
        }
    }
}

/// 2D spätná transformácia bloku N × N (8.6.4.2) s výsledným posunom bdShift (8.6.2):
/// 1. každý stĺpec x: e[x][y] = 1D(d[x][·]), g[x][y] = Clip3(−32768, 32767, (e[x][y] + 64) >> 7),
/// 2. každý riadok y: r[x][y] = 1D(g[·][y]), rezíduum = (r[x][y] + (1 << (bdShift − 1))) >> bdShift.
///
/// Stĺpce za `max_x` sú nulové (dajú g = 0) a 1D transformácie čítajú len prvých `max_y + 1` resp. `max_x + 1`
/// koeficientov.
fn transformuj<const N: usize>(
    d: &[i16],
    max_x: usize,
    max_y: usize,
    dst: bool,
    posun: u32,
    mut zapis: impl FnMut(usize, &[i32]),
) {
    let zaokruhlenie = 1i32 << (posun - 1);
    let mut r = [0i32; N];
    if !dst && max_y == 0 {
        // Len prvý riadok koeficientov: stĺpec x má jediný koeficient d[x][0] a báza 0 DCT je konštantná, prvý
        // stupeň dá v celom stĺpci 64 · d[x][0] → všetky riadky g sú rovnaké, rovnaké sú aj riadky rezíduí.
        let mut g = [0i16; N];
        for (g, &d) in g.iter_mut().zip(&d[..=max_x]) {
            *g = orez16((DCT_DC * i32::from(d) + 64) >> 7);
        }
        if max_x == 0 {
            // len DC: aj druhý stupeň dá konštantu 64 · g
            r.fill((DCT_DC * i32::from(g[0]) + zaokruhlenie) >> posun);
        } else {
            spat_1d(&g, 1, max_x + 1, false, &mut r);
            for r in &mut r {
                *r = (*r + zaokruhlenie) >> posun;
            }
        }
        for y in 0..N {
            zapis(y, &r);
        }
        return;
    }
    // 1. stupeň: stĺpce (vstup d[x][k] = d[k * N + x])
    let mut g = [[0i16; N]; N];
    let mut e = [0i32; N];
    for x in 0..=max_x {
        spat_1d(&d[x..], N, max_y + 1, dst, &mut e);
        for (riadok, &e) in g.iter_mut().zip(&e) {
            riadok[x] = orez16((e + 64) >> 7);
        }
    }
    // 2. stupeň: riadky
    for (y, g) in g.iter().enumerate() {
        spat_1d(g, 1, max_x + 1, dst, &mut r);
        for r in &mut r {
            *r = (*r + zaokruhlenie) >> posun;
        }
        zapis(y, &r);
    }
}

/// 1D spätná transformácia (8.6.4.2) podľa dĺžky výstupu: `vystup[i] = Σ_k báza_k[i] · vstup[k · krok]`
/// pre k < `pocet`; ďalšie koeficienty sa nečítajú (berú sa ako nulové).
#[inline(always)]
fn spat_1d(vstup: &[i16], krok: usize, pocet: usize, dst: bool, vystup: &mut [i32]) {
    match vystup.len() {
        4 if dst => spat_dst4(vstup, krok, pocet, vystup),
        4 => spat_dct4(vstup, krok, pocet, vystup),
        8 => spat_dct8(vstup, krok, pocet, vystup),
        16 => spat_dct16(vstup, krok, pocet, vystup),
        32 => spat_dct32(vstup, krok, pocet, vystup),
        _ => unreachable!("1D transformácia len pre 4, 8, 16, 32 bodov"),
    }
}

/// Súčet nepárnych koeficientov (k = 2m + 1 < `pocet`) s nepárnymi bázami na prvých P polohách:
/// O[i] = Σ_m bazy[m][i] · vstup[(2m + 1) · krok]. Vnútorná slučka je vektorová (rovnaký koeficient, P polôh).
#[inline(always)]
fn neparna_cast<const P: usize>(bazy: &[[i16; P]; P], vstup: &[i16], krok: usize, pocet: usize) -> [i32; P] {
    let mut o = [0i32; P];
    for (m, baza) in bazy.iter().enumerate().take(pocet / 2) {
        let c = i32::from(vstup[(2 * m + 1) * krok]);
        if c != 0 {
            for (o, &b) in o.iter_mut().zip(baza) {
                *o += i32::from(b) * c;
            }
        }
    }
    o
}

/// Motýlik: párne bázy sú na polohách i a 2P − 1 − i rovnaké, nepárne opačné, teda
/// vystup[i] = E[i] + O[i] a vystup[2P − 1 − i] = E[i] − O[i] pre i < P.
#[inline(always)]
fn motylik<const P: usize>(parne: &[i32; P], neparne: &[i32; P], vystup: &mut [i32]) {
    let (lavo, pravo) = vystup[..2 * P].split_at_mut(P);
    for (((l, r), &e), &o) in lavo.iter_mut().zip(pravo.iter_mut().rev()).zip(parne).zip(neparne) {
        *l = e + o;
        *r = e - o;
    }
}

/// 1D spätná DST-VII 4 bodov (8.6.4.2, trType = 1): `vystup[i] = Σ_k MATICA_DST[k][i] · vstup[k · krok]`.
fn spat_dst4(vstup: &[i16], krok: usize, pocet: usize, vystup: &mut [i32]) {
    let mut o = [0i32; 4];
    for (k, baza) in MATICA_DST.iter().enumerate().take(pocet) {
        let c = i32::from(vstup[k * krok]);
        for (o, &b) in o.iter_mut().zip(baza) {
            *o += i32::from(b) * c;
        }
    }
    vystup[..4].copy_from_slice(&o);
}

/// 1D spätná DCT 4 bodov (8.6.4.2, trType = 0, nTbS = 4): bázy 0, 8, 16, 24 matice 32 × 32.
fn spat_dct4(vstup: &[i16], krok: usize, pocet: usize, vystup: &mut [i32]) {
    let c = |k: usize| if k < pocet { i32::from(vstup[k * krok]) } else { 0 };
    // párna časť = 2-bodová transformácia koeficientov 0 a 2 (báza 0: 64, 64; báza 16: 64, −64)
    let (a, b) = (DCT_DC * c(0), DCT_16 * c(2));
    let parne = [a + b, a - b];
    // nepárna časť: koeficienty 1 a 3 s bázami 8 a 24 na polohách 0 a 1
    let neparne = neparna_cast(&NEPARNE_4, vstup, krok, pocet);
    motylik(&parne, &neparne, vystup);
}

/// 1D spätná DCT 8 bodov (8.6.4.2, nTbS = 8): párne koeficienty → DCT 4, nepárne → bázy 4 · (2m + 1).
fn spat_dct8(vstup: &[i16], krok: usize, pocet: usize, vystup: &mut [i32]) {
    let mut parne = [0i32; 4];
    spat_dct4(vstup, 2 * krok, pocet.div_ceil(2), &mut parne);
    let neparne = neparna_cast(&NEPARNE_8, vstup, krok, pocet);
    motylik(&parne, &neparne, vystup);
}

/// 1D spätná DCT 16 bodov (8.6.4.2, nTbS = 16): párne koeficienty → DCT 8, nepárne → bázy 2 · (2m + 1).
fn spat_dct16(vstup: &[i16], krok: usize, pocet: usize, vystup: &mut [i32]) {
    let mut parne = [0i32; 8];
    spat_dct8(vstup, 2 * krok, pocet.div_ceil(2), &mut parne);
    let neparne = neparna_cast(&NEPARNE_16, vstup, krok, pocet);
    motylik(&parne, &neparne, vystup);
}

/// 1D spätná DCT 32 bodov (8.6.4.2, nTbS = 32): párne koeficienty → DCT 16, nepárne → bázy 2m + 1.
fn spat_dct32(vstup: &[i16], krok: usize, pocet: usize, vystup: &mut [i32]) {
    let mut parne = [0i32; 16];
    spat_dct16(vstup, 2 * krok, pocet.div_ceil(2), &mut parne);
    let neparne = neparna_cast(&NEPARNE_32, vstup, krok, pocet);
    motylik(&parne, &neparne, vystup);
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    // ---------------------------------------------------------------------------------------------------------
    // Rozhodca: priamy, pomalý prepis vzorcov normy (i64, násobenie celou maticou, žiadne skratky).
    // ---------------------------------------------------------------------------------------------------------

    fn clip3(min: i64, max: i64, x: i64) -> i64 {
        if x < min {
            min
        } else if x > max {
            max
        } else {
            x
        }
    }

    /// transMatrix pre nTbS = `n`: báza (koeficient) k na polohe i (8.6.4.2).
    fn baza(n: usize, dst: bool, k: usize, i: usize) -> i64 {
        if dst { i64::from(MATICA_DST[k][i]) } else { i64::from(MATICA_DCT[k * 32 / n][i]) }
    }

    /// Rezíduá r[x][y] (raster `y * n + x`) podľa 8.6.2, 8.6.3 a 8.6.4.2.
    fn rozhodca_rezidua(koef: &[i16], p: &ParametreTb) -> Vec<i64> {
        let n = 1usize << p.log2_n;
        let bit_depth = i64::from(p.bitova_hlbka);
        if p.obchadzanie {
            return koef[..n * n].iter().map(|&c| i64::from(c)).collect();
        }
        // 8.6.3: spätné kvantovanie
        let bd_shift = bit_depth + i64::from(p.log2_n) - 5;
        let level_scale = [40i64, 45, 51, 57, 64, 72][(p.qp % 6) as usize];
        let mut d = vec![0i64; n * n];
        for y in 0..n {
            for x in 0..n {
                let m = match p.faktory {
                    Some(f) if !(p.preskocenie && n > 4) => i64::from(f[y * n + x]),
                    _ => 16,
                };
                let c = i64::from(koef[y * n + x]);
                d[y * n + x] = clip3(
                    -32768,
                    32767,
                    (((c * m * level_scale) << (p.qp / 6)) + (1 << (bd_shift - 1))) >> bd_shift,
                );
            }
        }
        let mut r = vec![0i64; n * n];
        if p.preskocenie {
            let ts_shift = 5 + p.log2_n;
            for (r, &d) in r.iter_mut().zip(&d) {
                *r = d << ts_shift;
            }
        } else {
            let dst = p.intra && p.jas && n == 4;
            // 1. stĺpce: e[x][y] = Σ_k transMatrix(k, y) · d[x][k]
            let mut g = vec![0i64; n * n];
            for x in 0..n {
                for y in 0..n {
                    let e: i64 = (0..n).map(|k| baza(n, dst, k, y) * d[k * n + x]).sum();
                    g[y * n + x] = clip3(-32768, 32767, (e + 64) >> 7);
                }
            }
            // 2. riadky: r[x][y] = Σ_k transMatrix(k, x) · g[k][y]
            for y in 0..n {
                for x in 0..n {
                    r[y * n + x] = (0..n).map(|k| baza(n, dst, k, x) * g[y * n + k]).sum();
                }
            }
        }
        // 8.6.2: bdShift = 20 − BitDepth
        let bd_shift = 20 - bit_depth;
        for r in &mut r {
            *r = (*r + (1 << (bd_shift - 1))) >> bd_shift;
        }
        r
    }

    /// 8.6.7: recSamples = Clip1(predSamples + resSamples).
    fn rozhodca(koef: &[i16], p: &ParametreTb, ciel: &mut [Vzorka], krok: usize) {
        let n = 1usize << p.log2_n;
        let r = rozhodca_rezidua(koef, p);
        let max = (1i64 << p.bitova_hlbka) - 1;
        for y in 0..n {
            for x in 0..n {
                let v = &mut ciel[y * krok + x];
                *v = clip3(0, max, i64::from(*v) + r[y * n + x]) as Vzorka;
            }
        }
    }

    /// Rezíduá rýchlej cesty (bez orezania Clip1) v rastri n × n.
    fn rychle_rezidua(koef: &[i16], max_x: usize, max_y: usize, p: &ParametreTb) -> Vec<i64> {
        let n = 1usize << p.log2_n;
        let mut k = koef.to_vec();
        let mut r = vec![0i64; n * n];
        rezidua(&mut k, max_x, max_y, p, |y, rez| {
            for (r, &v) in r[y * n..y * n + rez.len()].iter_mut().zip(rez) {
                *r = i64::from(v);
            }
        });
        r
    }

    fn parametre(log2_n: u32, qp: i32, faktory: Option<&[u8]>) -> ParametreTb<'_> {
        ParametreTb {
            log2_n,
            jas: true,
            intra: false,
            qp,
            preskocenie: false,
            obchadzanie: false,
            faktory,
            bitova_hlbka: 8,
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // Matice a prechod
    // ---------------------------------------------------------------------------------------------------------

    #[test]
    fn matica_dct_ma_tvar_normy() {
        // presná symetria (párne bázy) a antisymetria (nepárne bázy) – na nej stojí motýlik
        for (k, baza) in MATICA_DCT.iter().enumerate() {
            for i in 0..32 {
                let zrkadlo = baza[31 - i];
                assert_eq!(baza[i], if k % 2 == 0 { zrkadlo } else { -zrkadlo }, "k = {k}, i = {i}");
            }
        }
        // hodnoty sú do ±1,5 od 64 · √2 · cos(π (2i + 1) k / 64) (báza 0: 64) – zachytí preklep v tabuľke
        for (k, baza) in MATICA_DCT.iter().enumerate().skip(1) {
            for (i, &v) in baza.iter().enumerate() {
                let kosinus =
                    64.0 * 2f64.sqrt() * (std::f64::consts::PI * ((2 * i + 1) * k) as f64 / 64.0).cos();
                assert!((f64::from(v) - kosinus).abs() <= 1.5, "k = {k}, i = {i}: {v} oproti {kosinus}");
            }
        }
        // takmer ortogonálne riadky s normou ≈ 64² · 32
        for (k, bk) in MATICA_DCT.iter().enumerate() {
            for (l, bl) in MATICA_DCT.iter().enumerate() {
                let s: i32 = bk.iter().zip(bl).map(|(&a, &b)| i32::from(a) * i32::from(b)).sum();
                if k == l {
                    assert!((s - 131072).abs() < 400, "norma bázy {k}: {s}");
                } else {
                    assert!(s.abs() < 400, "bázy {k} a {l}: {s}");
                }
            }
        }
        // známe riadky normy
        assert_eq!(MATICA_DCT[1][..16], [90, 90, 88, 85, 82, 78, 73, 67, 61, 54, 46, 38, 31, 22, 13, 4]);
        assert_eq!(MATICA_DCT[2][..8], [90, 87, 80, 70, 57, 43, 25, 9]);
        assert_eq!(MATICA_DCT[4][..4], [89, 75, 50, 18]);
        assert_eq!(MATICA_DCT[8][..2], [83, 36]);
        assert_eq!(MATICA_DCT[24][..4], [36, -83, 83, -36]);
        assert_eq!(NEPARNE_4, [[83, 36], [36, -83]]);
        assert_eq!(NEPARNE_8[1], [75, -18, -89, -50]);
    }

    #[test]
    fn diagonalny_prechod_podla_6_5_3() {
        assert_eq!(PRECHOD_4[..7], [(0, 0), (0, 1), (1, 0), (0, 2), (1, 1), (2, 0), (0, 3)]);
        assert_eq!(PRECHOD_4[15], (3, 3));
        assert_eq!(PRECHOD_8[..4], [(0, 0), (0, 1), (1, 0), (0, 2)]);
        assert_eq!(PRECHOD_8[63], (7, 7));
        // každá poloha práve raz, uhlopriečky (x + y) neklesajú
        for prechod in [&PRECHOD_4[..], &PRECHOD_8[..]] {
            let n = if prechod.len() == 16 { 4 } else { 8 };
            let mut videne = vec![false; n * n];
            for (i, &(x, y)) in prechod.iter().enumerate() {
                assert!(!videne[y as usize * n + x as usize]);
                videne[y as usize * n + x as usize] = true;
                if i > 0 {
                    assert!(prechod[i - 1].0 + prechod[i - 1].1 <= x + y);
                }
            }
        }
    }

    #[test]
    fn faktory_predvolenych_matic() {
        let f = FaktorySkalovania::z_matic(&MaticeKvantovania::predvolene());
        // tab. 7-6 rozbalená do rastra (vnútrosnímková 8 × 8, symetrická)
        let intra_8: [[u8; 8]; 8] = [
            [16, 16, 16, 16, 17, 18, 21, 24],
            [16, 16, 16, 16, 17, 19, 22, 25],
            [16, 16, 17, 18, 20, 22, 25, 29],
            [16, 16, 18, 21, 24, 27, 31, 36],
            [17, 17, 20, 24, 30, 35, 41, 47],
            [18, 19, 22, 27, 35, 44, 54, 65],
            [21, 22, 25, 31, 41, 54, 70, 88],
            [24, 25, 29, 36, 47, 65, 88, 115],
        ];
        let inter_8: [[u8; 8]; 8] = [
            [16, 16, 16, 16, 17, 18, 20, 24],
            [16, 16, 16, 17, 18, 20, 24, 25],
            [16, 16, 17, 18, 20, 24, 25, 28],
            [16, 17, 18, 20, 24, 25, 28, 33],
            [17, 18, 20, 24, 25, 28, 33, 41],
            [18, 20, 24, 25, 28, 33, 41, 54],
            [20, 24, 25, 28, 33, 41, 54, 71],
            [24, 25, 28, 33, 41, 54, 71, 91],
        ];
        for m in 0..6 {
            assert!(f.faktor(2, m).iter().all(|&v| v == 16));
            let ocakavane = if m < 3 { &intra_8 } else { &inter_8 };
            for velkost in 1..4u32 {
                let n = 4usize << velkost;
                let r = n / 8;
                let fak = f.faktor(velkost + 2, m);
                assert_eq!(fak.len(), n * n);
                for y in 0..n {
                    for x in 0..n {
                        assert_eq!(fak[y * n + x], ocakavane[y / r][x / r], "n = {n}, m = {m}, ({x}, {y})");
                    }
                }
            }
        }
    }

    #[test]
    fn faktory_poradie_x_y_a_dc() {
        // nesymetrický zoznam: hodnota = 1 + poloha prechodu, DC = 200 + matica
        let mut matice = MaticeKvantovania::predvolene();
        for velkost in 0..4 {
            for m in 0..6 {
                let pocet = matice.zoznamy[velkost][m].len();
                matice.zoznamy[velkost][m] = (0..pocet).map(|i| (1 + i + m) as u8).collect();
            }
        }
        for m in 0..6 {
            matice.dc[0][m] = 200 + m as u8;
            matice.dc[1][m] = 210 + m as u8;
        }
        let f = FaktorySkalovania::z_matic(&matice);
        for m in 0..6 {
            let m8 = m as u8;
            // 4 × 4: poloha 1 prechodu = (x 0, y 1) → raster 4, poloha 2 = (1, 0) → raster 1
            let f4 = f.faktor(2, m);
            assert_eq!(
                (f4[0], f4[4], f4[1], f4[8], f4[5], f4[2], f4[15]),
                (1 + m8, 2 + m8, 3 + m8, 4 + m8, 5 + m8, 6 + m8, 16 + m8)
            );
            let f8 = f.faktor(3, m);
            assert_eq!((f8[0], f8[8], f8[1], f8[63]), (1 + m8, 2 + m8, 3 + m8, 64 + m8));
            // 16 × 16: hodnota pokryje 2 × 2, [0][0] je DC
            let f16 = f.faktor(4, m);
            assert_eq!((f16[0], f16[1], f16[16], f16[17]), (200 + m8, 1 + m8, 1 + m8, 1 + m8));
            assert_eq!((f16[2 * 16], f16[3 * 16 + 1], f16[2]), (2 + m8, 2 + m8, 3 + m8));
            assert_eq!(f16[255], 64 + m8);
            // 32 × 32: hodnota pokryje 4 × 4
            let f32 = f.faktor(5, m);
            assert_eq!((f32[0], f32[1], f32[3 * 32 + 3]), (210 + m8, 1 + m8, 1 + m8));
            assert_eq!((f32[4 * 32], f32[7 * 32 + 3], f32[4]), (2 + m8, 2 + m8, 3 + m8));
            assert_eq!(f32[1023], 64 + m8);
            for velkost in 0..4 {
                let rozbalene = rozbal_maticu(&matice, velkost, m);
                assert_eq!(f.faktory[velkost][m], rozbalene);
            }
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // 1D transformácie a spätné kvantovanie
    // ---------------------------------------------------------------------------------------------------------

    #[test]
    fn transformacie_1d_ako_nasobenie_maticou() {
        let mut rng = Nahoda::zo_semienka(11);
        for pokus in 0..4000 {
            let n = 4usize << (pokus % 4);
            let dst = n == 4 && pokus % 8 == 0;
            let krok = 1 + (rng.dalsie() % 3) as usize;
            let pocet = 1 + (rng.dalsie() % n as u64) as usize;
            // za `pocet` je „smetie“ – nesmie sa čítať
            let vstup: Vec<i16> = (0..n * krok).map(|_| rng.dalsie() as i16).collect();
            let mut vystup = vec![0i32; n];
            spat_1d(&vstup, krok, pocet, dst, &mut vystup);
            for (i, &v) in vystup.iter().enumerate() {
                let ocakavane: i64 =
                    (0..pocet).map(|k| baza(n, dst, k, i) * i64::from(vstup[k * krok])).sum();
                assert_eq!(i64::from(v), ocakavane, "n = {n}, dst = {dst}, pocet = {pocet}, i = {i}");
            }
        }
    }

    #[test]
    fn spatne_kvantovanie_v_32_bitoch_ako_v_64() {
        let mut rng = Nahoda::zo_semienka(12);
        let koeficienty = [i16::MIN, i16::MIN + 1, -1000, -1, 0, 1, 7, 1000, i16::MAX - 1, i16::MAX];
        for bitova_hlbka in 8..=16u32 {
            for log2_n in 2..=5u32 {
                let b = i64::from(bitova_hlbka + log2_n - 5);
                for qp in 0..=120 {
                    let k = Kvantovanie::nove(qp, bitova_hlbka + log2_n - 5);
                    for pokus in 0..30 {
                        let c =
                            if pokus < koeficienty.len() { koeficienty[pokus] } else { rng.dalsie() as i16 };
                        let m = match pokus % 3 {
                            0 => 16,
                            1 => 255,
                            _ => 1 + (rng.dalsie() % 255) as i32,
                        };
                        let s = m * MIERKA_UROVNE[(qp % 6) as usize];
                        let ocakavane = clip3(
                            -32768,
                            32767,
                            (((i64::from(c) * i64::from(s)) << (qp / 6)) + (1 << (b - 1))) >> b,
                        );
                        assert_eq!(
                            i64::from(k.pouzi(c, s)),
                            ocakavane,
                            "BD {bitova_hlbka}, log2 {log2_n}, qp {qp}, c {c}, m {m}"
                        );
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // Ručne overené príklady
    // ---------------------------------------------------------------------------------------------------------

    #[test]
    fn len_dc_koeficient_rucne() {
        // 4 × 4, c = 10, qp 22 (levelScale 64, posun 3), bdShift 5: d = (10·16·64 << 3 + 16) >> 5 = 2560,
        // g = (64 · 2560 + 64) >> 7 = 1280, r = 64 · 1280 = 81920, rezíduum = (81920 + 2048) >> 12 = 20
        let mut koef = [0i16; 16];
        koef[0] = 10;
        let mut ciel = [100u8; 16];
        rekonstruuj(&mut koef, 0, 0, &parametre(2, 22, None), &mut ciel, 4);
        assert_eq!(ciel, [120; 16]);
        // 8 × 8, c = −3, qp 30 (levelScale 40, posun 5), bdShift 6: d = (−61440 + 32) >> 6 = −960,
        // g = (−61440 + 64) >> 7 = −480, rezíduum = (−30720 + 2048) >> 12 = −7; Clip1(5 − 7) = 0
        for (predikcia, vysledok) in [(200u8, 193u8), (5, 0)] {
            let mut koef = [0i16; 64];
            koef[0] = -3;
            let mut ciel = [predikcia; 64];
            rekonstruuj(&mut koef, 0, 0, &parametre(3, 30, None), &mut ciel, 8);
            assert_eq!(ciel, [vysledok; 64]);
        }
        // 32 × 32 s krokom 40: vzorky mimo bloku sa nemenia
        let mut koef = [0i16; 1024];
        koef[0] = 10;
        let mut ciel = vec![50u8; 40 * 32];
        rekonstruuj(&mut koef, 0, 0, &parametre(5, 22, None), &mut ciel, 40);
        // bdShift 8: d = (81920 + 128) >> 8 = 320, g = (20480 + 64) >> 7 = 160, rezíduum = (10240 + 2048) >> 12 = 3
        for y in 0..32 {
            assert!(ciel[y * 40..y * 40 + 32].iter().all(|&v| v == 53));
            assert!(ciel[y * 40 + 32..y * 40 + 40].iter().all(|&v| v == 50));
        }
    }

    #[test]
    fn dst_jasu_rucne() {
        // 4 × 4 jas intra, c = 64, qp 4 (levelScale 64): d = (64·16·64 + 16) >> 5 = 2048,
        // stĺpec: g = (2048 · {29, 55, 74, 84} + 64) >> 7 = {464, 880, 1184, 1344},
        // riadok y: rezíduum (x, y) = (DST0[x] · g[y] + 2048) >> 12
        let ocakavane: [i64; 16] = [3, 6, 8, 10, 6, 12, 16, 18, 8, 16, 21, 24, 10, 18, 24, 28];
        let mut koef = [0i16; 16];
        koef[0] = 64;
        let p = ParametreTb { intra: true, ..parametre(2, 4, None) };
        assert_eq!(rychle_rezidua(&koef, 0, 0, &p), ocakavane);
        assert_eq!(rozhodca_rezidua(&koef, &p), ocakavane);
        // farba intra 4 × 4 aj jas inter používajú DCT: rovnaký DC koeficient dá ploché rezíduá
        // (d = 2048, g = 1024, rezíduum = (65536 + 2048) >> 12 = 16)
        for p in [ParametreTb { jas: false, intra: true, ..p }, ParametreTb { intra: false, ..p }] {
            assert_eq!(rychle_rezidua(&koef, 0, 0, &p), [16; 16]);
        }
    }

    #[test]
    fn preskocenie_a_obchadzanie_rucne() {
        // preskočenie 4 × 4: c = 3 na (1, 1), qp 4: d = (3·1024 + 16) >> 5 = 96, r = 96 << 7,
        // rezíduum = (12288 + 2048) >> 12 = 3
        let mut koef = [0i16; 16];
        koef[5] = 3;
        let mut ciel = [10u8; 16];
        let p = ParametreTb { preskocenie: true, intra: true, ..parametre(2, 4, None) };
        rekonstruuj(&mut koef, 1, 1, &p, &mut ciel, 4);
        let mut ocakavane = [10u8; 16];
        ocakavane[5] = 13;
        assert_eq!(ciel, ocakavane);
        // obchádzanie: rezíduum = TransCoeffLevel (aj mimo rozsahu vzoriek – orezanie Clip1)
        let mut koef = [0i16; 16];
        koef[2 * 4 + 3] = -5;
        koef[1] = 300;
        let mut ciel = [100u8; 16];
        let p = ParametreTb { obchadzanie: true, ..parametre(2, 51, None) };
        rekonstruuj(&mut koef, 3, 2, &p, &mut ciel, 4);
        let mut ocakavane = [100u8; 16];
        ocakavane[2 * 4 + 3] = 95;
        ocakavane[1] = 255;
        assert_eq!(ciel, ocakavane);
    }

    #[test]
    fn matica_kvantovania_meni_dc() {
        // faktor DC 32 namiesto 16 zdvojnásobí d: 4 × 4, c = 10, qp 22 → d = 5120, g = 2560, rezíduum 40
        let mut faktory = [16u8; 16];
        faktory[0] = 32;
        let mut koef = [0i16; 16];
        koef[0] = 10;
        let mut ciel = [100u8; 16];
        rekonstruuj(&mut koef, 0, 0, &parametre(2, 22, Some(&faktory)), &mut ciel, 4);
        assert_eq!(ciel, [140; 16]);
    }

    // ---------------------------------------------------------------------------------------------------------
    // Náhodné porovnanie s rozhodcom
    // ---------------------------------------------------------------------------------------------------------

    /// Náhodný blok koeficientov s rôznym tvarom oblasti, hustotou a rozdelením hodnôt; vráti aj skutočné
    /// max_x, max_y nenulových koeficientov (0, 0 pre nulový blok).
    fn nahodne_koeficienty(rng: &mut Nahoda, n: usize) -> (Vec<i16>, usize, usize) {
        let mut nahodne = |m: usize| (rng.dalsie() % m as u64) as usize;
        let (sirka, vyska) = match nahodne(6) {
            0 => (1, 1),
            1 => (1 + nahodne(n), 1),
            2 => (1, 1 + nahodne(n)),
            3 => (n, n),
            _ => (1 + nahodne(n), 1 + nahodne(n)),
        };
        let hustota = [100, 50, 10, 3][nahodne(4)];
        let druh = nahodne(5);
        let mut koef = vec![0i16; n * n];
        for y in 0..vyska {
            for x in 0..sirka {
                if nahodne(100) < hustota {
                    koef[y * n + x] = match druh {
                        0 => nahodne(9) as i16 - 4,
                        1 => nahodne(1001) as i16 - 500,
                        2 => nahodne(65536) as u16 as i16,
                        3 => [i16::MIN, i16::MAX][nahodne(2)],
                        _ => [i16::MIN, i16::MAX, 0, 1, -1][nahodne(5)],
                    };
                }
            }
        }
        // zaručene nenulový roh oblasti pri plnej hustote
        if hustota == 100 && koef[(vyska - 1) * n + sirka - 1] == 0 {
            koef[(vyska - 1) * n + sirka - 1] = 1;
        }
        let (mut max_x, mut max_y) = (0, 0);
        for y in 0..n {
            for x in 0..n {
                if koef[y * n + x] != 0 {
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        (koef, max_x, max_y)
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Rezim {
        Transformacia,
        Preskocenie,
        Obchadzanie,
    }

    fn porovnaj_s_rozhodcom(semienko: u64, log2_n: u32, rezim: Rezim, pokusov: usize) {
        let mut rng = Nahoda::zo_semienka(semienko);
        let n = 1usize << log2_n;
        for pokus in 0..pokusov {
            let (koef, max_x, max_y) = nahodne_koeficienty(&mut rng, n);
            let s_maticou = rng.dalsie().is_multiple_of(2);
            let faktory: Vec<u8> = (0..n * n).map(|_| 1 + (rng.dalsie() % 255) as u8).collect();
            // pri 4 × 4 je polovica pokusov jas vnútrosnímkovej jednotky (DST)
            let (jas, intra) = if log2_n == 2 && pokus % 2 == 0 {
                (true, true)
            } else {
                (rng.dalsie().is_multiple_of(2), rng.dalsie().is_multiple_of(2))
            };
            let p = ParametreTb {
                log2_n,
                jas,
                intra,
                qp: (pokus % 52) as i32,
                preskocenie: rezim == Rezim::Preskocenie,
                obchadzanie: rezim == Rezim::Obchadzanie,
                faktory: s_maticou.then_some(&faktory[..]),
                bitova_hlbka: 8,
            };
            let popis = format!(
                "pokus {pokus}: n {n}, qp {}, jas {jas}, intra {intra}, matica {s_maticou}, max ({max_x}, {max_y})",
                p.qp
            );
            // rezíduá bez orezania – presná zhoda, skrátený výpočet aj plný obdĺžnik
            let ocakavane = rozhodca_rezidua(&koef, &p);
            assert_eq!(rychle_rezidua(&koef, max_x, max_y, &p), ocakavane, "{popis}");
            assert_eq!(rychle_rezidua(&koef, n - 1, n - 1, &p), ocakavane, "{popis} (plný obdĺžnik)");
            // rekonštrukcia s krokom väčším ako n: vzorky mimo bloku sa nesmú zmeniť
            let krok = n + (rng.dalsie() % 8) as usize;
            let predikcia: Vec<Vzorka> = match rng.dalsie() % 4 {
                0 => vec![0; krok * n],
                1 => vec![255; krok * n],
                _ => (0..krok * n).map(|_| rng.dalsie() as Vzorka).collect(),
            };
            let mut ciel_rozhodca = predikcia.clone();
            rozhodca(&koef, &p, &mut ciel_rozhodca, krok);
            let mut ciel = predikcia.clone();
            let mut k = koef.clone();
            rekonstruuj(&mut k, max_x, max_y, &p, &mut ciel, krok);
            assert_eq!(ciel, ciel_rozhodca, "{popis} (rekonštrukcia)");
        }
    }

    #[test]
    fn nahodne_transformacie_4_a_8() {
        porovnaj_s_rozhodcom(1, 2, Rezim::Transformacia, 3000);
        porovnaj_s_rozhodcom(2, 3, Rezim::Transformacia, 2000);
    }

    #[test]
    fn nahodne_transformacie_16() {
        porovnaj_s_rozhodcom(3, 4, Rezim::Transformacia, 1200);
    }

    #[test]
    fn nahodne_transformacie_32() {
        porovnaj_s_rozhodcom(4, 5, Rezim::Transformacia, 600);
    }

    #[test]
    fn nahodne_preskocenie_a_obchadzanie() {
        for log2_n in 2..=5 {
            porovnaj_s_rozhodcom(10 + u64::from(log2_n), log2_n, Rezim::Preskocenie, 600);
            porovnaj_s_rozhodcom(20 + u64::from(log2_n), log2_n, Rezim::Obchadzanie, 300);
        }
    }

    #[test]
    fn extremne_koeficienty_vsetky_qp() {
        // plné bloky ±32768/32767 – orezanie d aj medzivýsledku g po prvom stupni
        let mut rng = Nahoda::zo_semienka(30);
        for log2_n in 2..=5u32 {
            let n = 1usize << log2_n;
            for qp in 0..52 {
                for dst in [false, true] {
                    if dst && log2_n != 2 {
                        continue;
                    }
                    let koef: Vec<i16> =
                        (0..n * n).map(|_| [i16::MIN, i16::MAX][(rng.dalsie() % 2) as usize]).collect();
                    let p = ParametreTb { intra: dst, ..parametre(log2_n, qp, None) };
                    assert_eq!(
                        rychle_rezidua(&koef, n - 1, n - 1, &p),
                        rozhodca_rezidua(&koef, &p),
                        "n {n}, qp {qp}"
                    );
                }
            }
        }
    }
}
