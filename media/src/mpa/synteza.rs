//! Hybridná banka filtrov syntézy vrstvy III: redukcia aliasu, IMDCT, okná, prekrytie, inverzia frekvencie
//! a polyfázová syntéza.
//!
//! Zdroje: CD 11172-3 2.4.3.4 (vzorec IMDCT, okná typov 0–3, spájanie krátkych blokov, prekrytie) a 2.4.3.2
//! (matica `N[i][k] = cos((16 + i)(2k + 1)π/64)`); [S] §4.3.12 (postup redukcie aliasu, inverzia frekvencie, kroky
//! syntézy), [S] tabuľka 60 (c[i]) a okno D[i] ([S] príloha I = 11172-3 tabuľka 3-B.3).
//!
//! **Rozpor rozhodnutý meraním:** redukcia aliasu pri zmiešanom bloku (pozri `alias`).
//!
//! **Čo text neobsahuje a je odvodené** (vzorce a obrázky v [S] sa do PDF nepreniesli, CD nemá prílohy):
//! - koeficienty motýlika aliasu `cs = 1/√(1 + c²)`, `ca = c/√(1 + c²)` – jediná dvojica, pri ktorej je motýlik
//!   otočením (cs² + ca² = 1) s pomerom ca/cs = c;
//! - výber vektora U z V a súčet výstupu (obrázok 3-A.2 / [S] obrázok 2): `U[64i + j] = V[128i + j]`,
//!   `U[64i + 32 + j] = V[128i + 96 + j]`, `x[j] = Σᵢ U[j + 32i]·D[j + 32i]` – podľa mojej znalosti normy.
//!
//! Obe hypotézy dokazuje meranie presnosti podľa ISO/IEC 11172-4 proti nezávislému rozhodcovi (`over_mpa.py`).
//! Výpočet v f32 bez FMA v pevnom poradí, tabuľky z `orchidea_jadro::matematika` – rovnaké bity na každej
//! platforme (R307).

use super::tabulky;
use orchidea_jadro::matematika::{cos, odmocnina, sin};
use std::f64::consts::PI;
use std::sync::OnceLock;

/// Tabuľky banky filtrov (počítané raz pri prvom použití tým istým kódom na každej platforme).
pub(crate) struct Tabulky {
    /// IMDCT n = 36: `cos(π/72 · (2i + 1 + 18)(2k + 1))`, [i][k]
    imdct36: [[f32; 18]; 36],
    /// IMDCT n = 12: `cos(π/24 · (2i + 1 + 6)(2k + 1))`, [i][k]
    imdct12: [[f32; 6]; 12],
    /// okná dlhých blokov podľa block_type 0, 1, 3 (index 0, 1, 2)
    okna: [[f32; 36]; 3],
    /// okno krátkeho bloku `sin(π/12 · (i + ½))`
    kratke: [f32; 12],
    cs: [f32; 8],
    ca: [f32; 8],
    /// matica syntézy N[i][k], i = 0..63, k = 0..31
    n: [[f32; 32]; 64],
    d: [f32; 512],
}

pub(crate) fn tabulky() -> &'static Tabulky {
    static T: OnceLock<Tabulky> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tabulky {
            imdct36: [[0.0; 18]; 36],
            imdct12: [[0.0; 6]; 12],
            okna: [[0.0; 36]; 3],
            kratke: [0.0; 12],
            cs: [0.0; 8],
            ca: [0.0; 8],
            n: [[0.0; 32]; 64],
            d: [0.0; 512],
        };
        // CD 2.4.3.4: x_i = Σ X_k cos(π/(2n) · (2i + 1 + n/2)(2k + 1))
        for i in 0..36 {
            for k in 0..18 {
                t.imdct36[i][k] = cos(PI / 72.0 * (2 * i + 1 + 18) as f64 * (2 * k + 1) as f64) as f32;
            }
        }
        for i in 0..12 {
            for k in 0..6 {
                t.imdct12[i][k] = cos(PI / 24.0 * (2 * i + 1 + 6) as f64 * (2 * k + 1) as f64) as f32;
            }
        }
        let dlhe = |i: usize| sin(PI / 36.0 * (i as f64 + 0.5)) as f32;
        let kratke = |i: usize| sin(PI / 12.0 * (i as f64 + 0.5)) as f32;
        for i in 0..36 {
            // a) block_type 0 – normálne okno
            t.okna[0][i] = dlhe(i);
            // b) block_type 1 – štartovacie okno
            t.okna[1][i] = match i {
                0..=17 => dlhe(i),
                18..=23 => 1.0,
                24..=29 => kratke(i - 18),
                _ => 0.0,
            };
            // c) block_type 3 – koncové okno
            t.okna[2][i] = match i {
                0..=5 => 0.0,
                6..=11 => kratke(i - 6),
                12..=17 => 1.0,
                _ => dlhe(i),
            };
        }
        for i in 0..12 {
            t.kratke[i] = kratke(i);
        }
        for i in 0..8 {
            let c = tabulky::ALIAS_C[i];
            let m = odmocnina(1.0 + c * c);
            t.cs[i] = (1.0 / m) as f32;
            t.ca[i] = (c / m) as f32;
        }
        // CD 2.4.3.2: N_ik = cos((16 + i)(2k + 1)π/64)
        for i in 0..64 {
            for k in 0..32 {
                t.n[i][k] = cos((16 + i) as f64 * (2 * k + 1) as f64 * PI / 64.0) as f32;
            }
        }
        for i in 0..512 {
            t.d[i] = tabulky::OKNO_D[i] as f32;
        }
        t
    })
}

/// Stav banky filtrov jedného kanála: polovice blokov na prekrytie a vektor V polyfázovej syntézy.
#[derive(Clone, Debug)]
pub(crate) struct StavSyntezy {
    prekrytie: [[f32; 18]; 32],
    v: Box<[f32; 1024]>,
}

impl Default for StavSyntezy {
    fn default() -> Self {
        StavSyntezy { prekrytie: [[0.0; 18]; 32], v: Box::new([0.0; 1024]) }
    }
}

/// Typ bloku granuly pre banku filtrov.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Blok {
    /// block_type 0–3 (0 aj bez prepínania okien)
    pub typ: u8,
    pub zmiesany: bool,
}

/// Redukcia aliasu ([S] §4.3.12.1): motýliky na hraniciach susedných podpásiem dlhých blokov.
///
/// **Rozpor rozhodnutý meraním (zmiešaný blok):** pseudokód [S] dáva pri zmiešanom bloku MPEG-1 jednu hranicu
/// (podpásma 0 a 1). Referenčný výstup skúšobného toku ISO/IEC 11172-4 `he_mode` (jediný so zmiešanými blokmi)
/// zodpovedá **zmiešanému bloku bez redukcie aliasu**: s motýlikom na hranici 0/1 je rozdiel 11 LSB (RMS 4,3·10⁻⁵),
/// bez neho ≤ 1 LSB (RMS 2,6·10⁻⁷, zaokrúhlenie referencie); pri viac hraniciach 240 LSB.
pub(crate) fn alias(xr: &mut [f32; 576], blok: Blok) {
    let t = tabulky();
    let pocet = if blok.typ == 2 { 0 } else { 32 };
    for sb in 1..pocet {
        for i in 0..8 {
            let (a, b) = (18 * sb - 1 - i, 18 * sb + i);
            let (xa, xb) = (xr[a], xr[b]);
            xr[a] = xa * t.cs[i] - xb * t.ca[i];
            xr[b] = xb * t.cs[i] + xa * t.ca[i];
        }
    }
}

impl StavSyntezy {
    pub(crate) fn reset(&mut self) {
        *self = StavSyntezy::default();
    }

    /// IMDCT, okná, prekrytie a inverzia frekvencie: z 576 čiar (po redukcii aliasu; krátke bloky v poradí
    /// [podpásmo][okno][čiara]) urobí 18 časových vzoriek pre každé z 32 podpásiem `vzorky[t][sb]`.
    pub(crate) fn hybrid(&mut self, xr: &[f32; 576], blok: Blok, vzorky: &mut [[f32; 32]; 18]) {
        let t = tabulky();
        for sb in 0..32 {
            let x = &xr[18 * sb..18 * sb + 18];
            let mut y = [0f32; 36];
            // [S] tabuľka 19: pri zmiešanom bloku majú dolné dve podpásma vždy normálne okno
            let typ = if blok.zmiesany && sb < 2 { 0 } else { blok.typ };
            if typ == 2 {
                // tri krátke bloky – každý zvlášť IMDCT n = 12, okno, spojenie s posunom 6 (CD 2.4.3.4 d)
                for w in 0..3 {
                    for i in 0..12 {
                        let mut s = 0f32;
                        for k in 0..6 {
                            s += x[6 * w + k] * t.imdct12[i][k];
                        }
                        y[6 + 6 * w + i] += s * t.kratke[i];
                    }
                }
            } else {
                let okno = &t.okna[match typ {
                    0 => 0,
                    1 => 1,
                    _ => 2,
                }];
                for i in 0..36 {
                    let mut s = 0f32;
                    for k in 0..18 {
                        s += x[k] * t.imdct36[i][k];
                    }
                    y[i] = s * okno[i];
                }
            }
            // prekrytie s druhou polovicou predošlého bloku; inverzia frekvencie: nepárne vzorky nepárnych podpásiem
            let p = &mut self.prekrytie[sb];
            for i in 0..18 {
                let mut v = y[i] + p[i];
                if sb & 1 == 1 && i & 1 == 1 {
                    v = -v;
                }
                vzorky[i][sb] = v;
                p[i] = y[i + 18];
            }
        }
    }

    /// Polyfázová syntéza ([S] §4.3.12.6): 32 vzoriek podpásiem → 32 vzoriek zvuku.
    pub(crate) fn polyfaza(&mut self, s: &[f32; 32], von: &mut [f32]) {
        let t = tabulky();
        // posun V o 64 a 64 nových hodnôt maticou N (najnovšie na pozíciách 0–63)
        self.v.copy_within(0..960, 64);
        for i in 0..64 {
            let mut a = 0f32;
            for k in 0..32 {
                a += t.n[i][k] * s[k];
            }
            self.v[i] = a;
        }
        // U z V (8 dvojíc blokov po 32), okno D a súčet 16 hodnôt na výstupnú vzorku
        for j in 0..32 {
            let mut a = 0f32;
            for i in 0..8 {
                a += self.v[128 * i + j] * t.d[64 * i + j];
                a += self.v[128 * i + 96 + j] * t.d[64 * i + 32 + j];
            }
            von[j] = a;
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn motylik_aliasu_je_otocenie() {
        let t = tabulky();
        for i in 0..8 {
            let (cs, ca) = (t.cs[i] as f64, t.ca[i] as f64);
            assert!((cs * cs + ca * ca - 1.0).abs() < 1e-6);
            assert!((ca / cs - tabulky::ALIAS_C[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn alias_len_pri_dlhych_blokoch() {
        // zmiešaný aj krátky blok ostane bez motýlikov (rozhodnuté meraním na toku ISO he_mode), dlhý sa zmení
        let mut xr = [0f32; 576];
        for (i, v) in xr.iter_mut().enumerate() {
            *v = (i % 7) as f32 - 3.0;
        }
        for blok in [Blok { typ: 2, zmiesany: true }, Blok { typ: 2, zmiesany: false }] {
            let mut x = xr;
            alias(&mut x, blok);
            assert_eq!(x, xr);
        }
        let mut x = xr;
        alias(&mut x, Blok { typ: 0, zmiesany: false });
        assert_ne!(x[17], xr[17]);
        assert_eq!(x[0..10], xr[0..10]);
    }

    #[test]
    fn okna_su_princenove_bradleyho() {
        // dlhé okná: w²(i) + w²(i + 18) = 1 na prekrývajúcich sa polovicách (prechod normálne → štart → krátke …)
        let t = tabulky();
        for i in 0..18 {
            let (a, b) = (t.okna[0][i + 18], t.okna[0][i]);
            assert!((a * a + b * b - 1.0).abs() < 1e-6);
        }
        for i in 0..6 {
            let (a, b) = (t.kratke[i + 6], t.kratke[i]);
            assert!((a * a + b * b - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn imdct_s_prekrytim_obnovi_signal() {
        // TDAC: dva po sebe idúce bloky normálneho okna s prekrytím obnovia stred presne (pri vhodnej MDCT)
        let t = tabulky();
        let n = 36;
        let signal: Vec<f64> = (0..54).map(|i| sin(0.37 * i as f64) + 0.2 * cos(1.9 * i as f64)).collect();
        let mdct = |zac: usize| -> Vec<f64> {
            (0..18)
                .map(|k| {
                    (0..n)
                        .map(|i| {
                            signal[zac + i]
                                * t.okna[0][i] as f64
                                * cos(PI / 72.0 * (2 * i + 1 + 18) as f64 * (2 * k + 1) as f64)
                        })
                        .sum()
                })
                .collect()
        };
        let (a, b) = (mdct(0), mdct(18));
        let mut stav = StavSyntezy::default();
        let mut xr = [0f32; 576];
        let mut von = [[0f32; 32]; 18];
        let blok = Blok { typ: 0, zmiesany: false };
        for k in 0..18 {
            xr[k] = a[k] as f32;
        }
        stav.hybrid(&xr, blok, &mut von);
        for k in 0..18 {
            xr[k] = b[k] as f32;
        }
        stav.hybrid(&xr, blok, &mut von);
        // stred (vzorky 18–35) = signál · 9 (mierka IMDCT bez normovania: n/4 = 9), podpásmo 0 bez inverzie
        for i in 0..18 {
            assert!((von[i][0] as f64 / 9.0 - signal[18 + i]).abs() < 1e-5, "{i}");
        }
    }
}
