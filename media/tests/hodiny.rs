//! **Simulácia hodín prehrávania a synchronizácie** (R305): zvukové zariadenie s kryštálom +120 ppm, displej
//! −60 ppm, chvenie časových značiek meraní ±10 ms, 4 hodiny prehrávania. Podmienky:
//! - chyba hodín voči skutočnému času zvuku najviac 2 ms po ustálení (30 s), pri nábehu najviac 12 ms (prvé
//!   meranie samo nesie chvenie ±10 ms),
//! - odchýlka nástupu snímky voči zvuku najviac P/2 + P/8 + 2 ms, v priemere ≈ 0,
//! - počet skrátených (vrátane vynechaných) a opakovaných snímok po nábehu ±2 od teórie (rozdiel kryštálov),
//!   opačné udalosti žiadne (bez blikania),
//! - pri kadencii 3:2 (24 snímok/s na 60 Hz) snímka 2 alebo 3 obnovenia okrem udalostí.

use orchidea_media::hodiny::{HodinyPrehravania, Synchronizacia};
use orchidea_media::r5::{MeranieZvuku, Obnova};

struct Nahoda(u64);
impl Nahoda {
    fn dalsie(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Rovnomerne v [−a, a].
    fn okolo(&mut self, a: i64) -> i64 {
        (self.dalsie() % (2 * a as u64 + 1)) as i64 - a
    }
}

struct Vysledok {
    chyba_hodin_ns: i64,
    chyba_nabehu_ns: i64,
    vynechanych: u64,
    skratenych: u64,
    opakovanych: u64,
    najvacsia_odchylka_ns: i64,
    priemerna_odchylka_ns: i64,
    teoria_skratenych: f64,
    teoria_opakovanych: f64,
    zle_kadencie: u64,
}

/// `snimok_s` = (čitateľ, menovateľ) snímok za sekundu, `obnova_hz` = (čitateľ, menovateľ) obnovení za sekundu.
fn simuluj(snimok_s: (i64, i64), obnova_hz: (i64, i64), sekund: i64) -> Vysledok {
    const PPM_ZVUK: f64 = 120.0;
    const PPM_DISPLEJ: f64 = -60.0;
    let f = 48_000u64;
    let mut n = Nahoda(0x9E37_79B9_7F4A_7C15);
    // skutočný čas média v monotónnom čase t: zvuk hrá rýchlejšie o 120 ppm
    let skutocny = |t: i64| (t as f64 * (1.0 + PPM_ZVUK * 1e-6)) as i64;
    let mut hodiny = HodinyPrehravania::zo_zvuku(f as u32, 0);
    let mut sync = Synchronizacia::nova(0);
    let p_nominal = 1e9 * obnova_hz.1 as f64 / obnova_hz.0 as f64;
    let p_skutocna = p_nominal / (1.0 + PPM_DISPLEJ * 1e-6); // pomalší displej → dlhšia perióda
    let f_snimky = 1e9 * snimok_s.1 as f64 / snimok_s.0 as f64;
    let pts = |j: i64| ((j as i128 * 1_000_000_000 * snimok_s.1 as i128) / snimok_s.0 as i128) as i64;
    let koniec = sekund * 1_000_000_000;
    let mut dalsie_meranie = 0i64;
    let mut dalsia_snimka = 0i64; // index prvej nezobrazenej
    let (mut chyba_hodin, mut chyba_nabehu) = (0i64, 0i64);
    let mut k = 0i64;
    let (mut zobrazeni, mut zle_kadencie) = (0u32, 0u64);
    let normalne = [(f_snimky / p_nominal).floor() as u32, (f_snimky / p_nominal).ceil() as u32];
    let mut udalosti_pred = (0u64, 0u64);
    // stav po nábehu (30 s): udalosti a snímky sa počítajú až od neho
    let mut po_nabehu: Option<(orchidea_media::hodiny::StatistikaSynchronizacie, i64)> = None;
    loop {
        let t_v = 20_000_000 + (k as f64 * p_skutocna) as i64;
        if t_v > koniec {
            break;
        }
        // merania zvuku 4× za sekundu do času obnovenia
        while dalsie_meranie <= t_v {
            let t = dalsie_meranie;
            let hrane = (skutocny(t) as i128 * f as i128 / 1_000_000_000) as u64;
            let d = 2000 + (n.dalsie() % 2000);
            let m = MeranieZvuku {
                odovzdanych_ramcov: hrane + d,
                oneskorenie_ramcov: d,
                monotonny_ns: t + n.okolo(10_000_000),
            };
            assert_eq!(hodiny.meranie(&m), None, "skok v čase {t}");
            dalsie_meranie += 250_000_000 + n.okolo(20_000_000);
        }
        if let Some(c) = hodiny.cas_ns(t_v) {
            let e = (c - skutocny(t_v)).abs();
            if t_v > 30_000_000_000 {
                chyba_hodin = chyba_hodin.max(e);
            } else {
                chyba_nabehu = chyba_nabehu.max(e);
            }
        }
        let fronta: Vec<i64> = (dalsia_snimka..dalsia_snimka + 6).map(pts).collect();
        let r = sync.obnova(&hodiny, Obnova { monotonny_ns: t_v, perioda_ns: p_nominal as i64 }, &fronta);
        if t_v > 30_000_000_000 && po_nabehu.is_none() {
            po_nabehu = Some((*sync.statistika(), dalsia_snimka));
        }
        if let Some(i) = r.zobraz {
            // kadencia: predošlá snímka mala 2 alebo 3 obnovenia (pri 3:2), inak len pri udalosti
            let st = sync.statistika();
            let udalost = (st.skratenych, st.opakovanych) != udalosti_pred;
            if zobrazeni > 0 && !udalost && !normalne.contains(&zobrazeni) && t_v > 10_000_000_000 {
                zle_kadencie += 1;
            }
            udalosti_pred = (st.skratenych, st.opakovanych);
            dalsia_snimka += 1 + i as i64;
            zobrazeni = 1;
        } else {
            zobrazeni += 1;
        }
        k += 1;
    }
    let st = *sync.statistika();
    let (st0, snimka0) = po_nabehu.expect("dlhšie ako 30 s");
    // teória: snímka trvá ρ skutočných obnovení (⌊ρ⌋ alebo ⌈ρ⌉, podiel ⌈ρ⌉ = ρ − ⌊ρ⌋); udalosť je počet obnovení
    // mimo a ± 3/4, kde a = F/P je nominálny počet
    let rho = f_snimky / (p_skutocna * (1.0 + PPM_ZVUK * 1e-6));
    let a = f_snimky / p_nominal;
    let (lo, w_hi) = (rho.floor(), rho - rho.floor());
    let snimok = (dalsia_snimka - snimka0) as f64;
    let podiel = |pred: &dyn Fn(f64) -> bool| {
        (pred(lo + 1.0) as u8 as f64) * w_hi + (pred(lo) as u8 as f64) * (1.0 - w_hi)
    };
    Vysledok {
        chyba_hodin_ns: chyba_hodin,
        chyba_nabehu_ns: chyba_nabehu,
        vynechanych: st.vynechanych - st0.vynechanych,
        skratenych: st.skratenych - st0.skratenych,
        opakovanych: st.opakovanych - st0.opakovanych,
        najvacsia_odchylka_ns: st.najvacsia_odchylka_ns,
        priemerna_odchylka_ns: st.priemerna_odchylka_ns(),
        teoria_skratenych: snimok * podiel(&|x| x < a - 0.75),
        teoria_opakovanych: snimok * podiel(&|x| x > a + 0.75),
        zle_kadencie,
    }
}

fn over(meno: &str, v: Vysledok, perioda_ns: i64) {
    println!(
        "{meno}: chyba hodín {:.2} ms (nábeh 30 s {:.2} ms), odchýlka max {:.2} ms priemer {:.3} ms, skrátené {} (vynechané {}) teória {:.1}, \
         opakované {} teória {:.1}, zlé kadencie {}",
        v.chyba_hodin_ns as f64 / 1e6,
        v.chyba_nabehu_ns as f64 / 1e6,
        v.najvacsia_odchylka_ns as f64 / 1e6,
        v.priemerna_odchylka_ns as f64 / 1e6,
        v.skratenych,
        v.vynechanych,
        v.teoria_skratenych,
        v.opakovanych,
        v.teoria_opakovanych,
        v.zle_kadencie
    );
    assert!(v.chyba_hodin_ns <= 2_000_000, "{meno}: chyba hodín");
    // pri nábehu je prvé meranie samo (chvenie ±10 ms), kým sa nezíska viac meraní
    assert!(v.chyba_nabehu_ns <= 12_000_000, "{meno}: chyba hodín pri nábehu");
    assert!(v.najvacsia_odchylka_ns <= perioda_ns / 2 + perioda_ns / 8 + 2_000_000, "{meno}: odchýlka");
    assert!(v.priemerna_odchylka_ns.abs() <= 2_000_000, "{meno}: priemerná odchýlka");
    assert!((v.skratenych as f64 - v.teoria_skratenych).abs() <= 2.0, "{meno}: skrátené");
    assert!((v.opakovanych as f64 - v.teoria_opakovanych).abs() <= 2.0, "{meno}: opakované");
    // blikanie kadencie = striedanie opačných udalostí
    assert!(v.skratenych.min(v.opakovanych) == 0, "{meno}: opačné udalosti");
    assert_eq!(v.zle_kadencie, 0, "{meno}: zlá kadencia");
}

const SEKUND: i64 = 4 * 3600;

#[test]
fn film_24000_1001_na_23_976_hz() {
    let v = simuluj((24_000, 1001), (24_000, 1001), SEKUND);
    over("23,976 na 23,976 Hz", v, 41_708_333);
}

#[test]
fn film_24000_1001_na_24_hz() {
    // kadencia: 24 Hz zobrazí film 23,976 o 1/1000 pomalšie – jedno opakovanie za ≈ 1000 obnovení (v teórii)
    let v = simuluj((24_000, 1001), (24, 1), SEKUND);
    over("23,976 na 24 Hz", v, 41_666_667);
}

#[test]
fn video_25_na_50_hz() {
    let v = simuluj((25, 1), (50, 1), SEKUND);
    over("25 na 50 Hz", v, 20_000_000);
}

#[test]
fn film_24_na_60_hz_kadencia_3_2() {
    let v = simuluj((24, 1), (60, 1), SEKUND);
    over("24 na 60 Hz", v, 16_666_667);
}

#[test]
fn pauza_posun_a_monotonnost() {
    let f = 48_000u64;
    let mut n = Nahoda(7);
    let mut h = HodinyPrehravania::zo_zvuku(f as u32, 0);
    // zvuk +120 ppm; pauza v 60.–65. s (zariadenie stojí), posun v 120. s na čas média 1 000 s
    let skutocny = |t: i64| -> i64 {
        let hra = |t: i64| (t as f64 * (1.0 + 120e-6)) as i64;
        if t < 60_000_000_000 {
            hra(t)
        } else if t < 65_000_000_000 {
            hra(60_000_000_000)
        } else if t < 120_000_000_000 {
            hra(t - 5_000_000_000)
        } else {
            1_000_000_000_000 + hra(t - 120_000_000_000)
        }
    };
    let mut posledny = i64::MIN;
    let mut odovzdane_od_posunu = 0u64;
    let mut t = 0i64;
    while t < 200_000_000_000 {
        if t == 60_000_000_000 {
            h.pauza(t);
        }
        if t == 65_000_000_000 {
            h.pokracuj(t);
        }
        if t == 120_000_000_000 {
            h.posun(1_000_000_000_000, t);
            odovzdane_od_posunu = 0;
            posledny = i64::MIN;
        }
        let pauza = (60_000_000_000..65_000_000_000).contains(&t);
        if !pauza && t % 250_000_000 == 0 {
            let zaklad = if t >= 120_000_000_000 { 1_000_000_000_000 } else { 0 };
            let hrane = ((skutocny(t) - zaklad) as i128 * f as i128 / 1_000_000_000) as u64;
            odovzdane_od_posunu = odovzdane_od_posunu.max(hrane);
            let d = 3000;
            let m = MeranieZvuku {
                odovzdanych_ramcov: hrane + d,
                oneskorenie_ramcov: d,
                monotonny_ns: t + n.okolo(10_000_000),
            };
            assert_eq!(h.meranie(&m), None, "bez skoku v {t}");
        }
        if let Some(c) = h.cas_ns(t) {
            assert!(c >= posledny, "monotónnosť v {t}");
            posledny = c;
            let e = (c - skutocny(t)).abs();
            let po = |s: i64| t > s * 1_000_000_000;
            let ustalene = (po(40) && !po(60)) || (po(95) && t < 120_000_000_000) || po(150);
            let medza = if ustalene { 2_000_000 } else { 12_000_000 };
            assert!(e <= medza, "chyba {e} v {} s", t / 1_000_000_000);
            if pauza {
                assert_eq!(c, h.cas_ns(60_000_000_000).unwrap());
            }
        }
        t += 10_000_000;
    }
    assert_eq!(h.skokov(), 0);
    assert!((h.pomer() - 1.000_120).abs() < 20e-6, "pomer {}", h.pomer());
}
