//! **Časová základňa (Čas)** – časové značky stôp ako celočíselné tiky so základňou v sekundách na tik.
//!
//! Matroska má základňu `TimestampScale` (predvolene 1 ms), MP4 `1 / timescale` z `mdhd`, MPEG-TS 1/90 000 s.
//! Tiky a zlomok sú presné aj pri 1001/30000 s; nanosekundy sa počítajú len na plánovanie (hodiny prehrávania,
//! synchronizácia, rozhranie R5) – presne v `i128` a so zaokrúhlením k najbližšiemu, polovica od nuly, takže ten
//! istý tik dá na každej platforme tie isté nanosekundy.

use orchidea_jadro::Zlomok;

/// Nanosekúnd v sekunde.
pub const NS_ZA_S: i64 = 1_000_000_000;

/// Základňa MPEG-TS a MPEG-PS: 1/90 000 s.
pub fn zakladna_90k() -> Zlomok {
    Zlomok::novy(1, 90_000).unwrap_or(Zlomok::JEDNA)
}

/// Základňa z počtu tikov za sekundu (MP4 `timescale`, frekvencia zvuku). `None` pri nule.
pub fn zakladna_z_frekvencie(tikov_za_s: u64) -> Option<Zlomok> {
    if tikov_za_s == 0 { None } else { Zlomok::novy(1, tikov_za_s).ok() }
}

/// Základňa z nanosekúnd na tik (Matroska `TimestampScale`). `None` pri nule alebo pretečení.
pub fn zakladna_z_ns(ns_na_tik: u64) -> Option<Zlomok> {
    if ns_na_tik == 0 {
        return None;
    }
    Zlomok::novy(i64::try_from(ns_na_tik).ok()?, NS_ZA_S as u64).ok()
}

/// Celočíselné delenie so zaokrúhlením k najbližšiemu, polovica od nuly (`m > 0`).
fn del_zaokruhli(c: i128, m: i128) -> i128 {
    let q = c / m;
    let r = c % m;
    if 2 * r.abs() >= m { q + c.signum() } else { q }
}

/// Tiky stopy → nanosekundy. `None`, ak sa výsledok nezmestí do `i64` (≈ ±292 rokov) alebo základňa nie je kladná.
pub fn na_ns(tik: i64, zakladna: Zlomok) -> Option<i64> {
    if zakladna.citatel() <= 0 {
        return None;
    }
    let c = (tik as i128).checked_mul(zakladna.citatel() as i128)?.checked_mul(NS_ZA_S as i128)?;
    i64::try_from(del_zaokruhli(c, zakladna.menovatel() as i128)).ok()
}

/// Nanosekundy → tiky stopy (zaokrúhlené k najbližšiemu). `None` pri pretečení alebo nekladnej základni.
pub fn z_ns(ns: i64, zakladna: Zlomok) -> Option<i64> {
    if zakladna.citatel() <= 0 {
        return None;
    }
    let c = (ns as i128).checked_mul(zakladna.menovatel() as i128)?;
    let m = (zakladna.citatel() as i128).checked_mul(NS_ZA_S as i128)?;
    i64::try_from(del_zaokruhli(c, m)).ok()
}

/// Rozbalenie 33-bitových časových značiek MPEG (PTS, DTS; ITU-T H.222.0) do súvislej osi `i64`.
///
/// Každá nová hodnota sa umiestni najbližšie k predchádzajúcej: rozdiel je v (−2^32, 2^32]. Pretečenie po
/// ≈ 26,5 hodiny (2^33 tikov pri 90 kHz) tak nespôsobí skok času; mierne neusporiadané PTS pri snímkach B
/// sú v poriadku.
#[derive(Debug, Clone, Default)]
pub struct Rozbalenie33 {
    posledna: Option<i64>,
}

impl Rozbalenie33 {
    /// 2^33.
    pub const MODUL: i64 = 1 << 33;

    pub fn nove() -> Rozbalenie33 {
        Rozbalenie33::default()
    }

    /// Rozbalí hodnotu (vyššie bity nad 33 sa ignorujú).
    pub fn rozbal(&mut self, hodnota: u64) -> i64 {
        let v = (hodnota & (Self::MODUL as u64 - 1)) as i64;
        let r = match self.posledna {
            None => v,
            Some(p) => {
                let mut x = p - p.rem_euclid(Self::MODUL) + v;
                if x - p > Self::MODUL / 2 {
                    x -= Self::MODUL;
                } else if p - x >= Self::MODUL / 2 {
                    x += Self::MODUL;
                }
                x
            }
        };
        self.posledna = Some(r);
        r
    }

    /// Začne od známej rozbalenej hodnoty (po posune v súbore: ďalšie hodnoty sa rozbalia k nej).
    pub fn zacni(&mut self, hodnota: i64) {
        self.posledna = Some(hodnota);
    }

    /// Zabudne históriu (po posune v súbore alebo prerušení toku).
    pub fn reset(&mut self) {
        self.posledna = None;
    }
}

/// Typické snímkové frekvencie (snímky za sekundu ako zlomok): film, PAL, NTSC, vysoké frekvencie.
const TYPICKE: [(i64, u64); 16] = [
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (48000, 1001),
    (48, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
    (100, 1),
    (120000, 1001),
    (120, 1),
    (12, 1),
    (15, 1),
    (144, 1),
];

/// Snímková frekvencia z trvania snímky v ns (Matroska `DefaultDuration` je zaokrúhlená na ns), prichytená
/// k typickej hodnote do odchýlky 0,01 %. Prúd A podľa nej volí obnovu displeja (24000/1001 → 23,976 Hz).
/// `None`, ak trvanie nie je blízko žiadnej typickej frekvencie.
pub fn typicka_frekvencia(trvanie_snimky_ns: i64) -> Option<Zlomok> {
    if trvanie_snimky_ns <= 0 {
        return None;
    }
    let t = trvanie_snimky_ns as i128;
    for (n, d) in TYPICKE {
        // presné trvanie snímky = d · 10^9 / n; porovnanie bez delenia: |t·n − d·10^9| ≤ d·10^9 / 10^4
        let presne = d as i128 * NS_ZA_S as i128;
        if (t * n as i128 - presne).abs() * 10_000 <= presne {
            return Zlomok::novy(n, d).ok();
        }
    }
    None
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn z(c: i64, m: u64) -> Zlomok {
        Zlomok::novy(c, m).unwrap()
    }

    #[test]
    fn prevod_na_ns() {
        let mkv = zakladna_z_ns(1_000_000).unwrap();
        assert_eq!(na_ns(1234, mkv), Some(1_234_000_000));
        assert_eq!(na_ns(-5, mkv), Some(-5_000_000));
        let ts = zakladna_90k();
        assert_eq!(na_ns(90_000, ts), Some(NS_ZA_S));
        // 1 tik pri 90 kHz = 11 111,1 ns → 11 111
        assert_eq!(na_ns(1, ts), Some(11_111));
        // 1001/30000 s: 33 366 666,67 ns → 33 366 667 (k najbližšiemu)
        assert_eq!(na_ns(1, z(1001, 30000)), Some(33_366_667));
        assert_eq!(na_ns(-1, z(1001, 30000)), Some(-33_366_667));
        // polovica od nuly: 1/2 000 000 000 s = 0,5 ns
        assert_eq!(na_ns(1, z(1, 2_000_000_000)), Some(1));
        assert_eq!(na_ns(-1, z(1, 2_000_000_000)), Some(-1));
        // pretečenie
        assert_eq!(na_ns(i64::MAX, mkv), None);
    }

    #[test]
    fn prevod_z_ns() {
        let ts = zakladna_90k();
        assert_eq!(z_ns(NS_ZA_S, ts), Some(90_000));
        let f = z(1001, 30000);
        for tik in [-7i64, 0, 1, 2, 1000, 123_456_789] {
            assert_eq!(z_ns(na_ns(tik, f).unwrap(), f), Some(tik), "tik {tik}");
        }
        let zvuk = zakladna_z_frekvencie(48_000).unwrap();
        assert_eq!(z_ns(NS_ZA_S / 2, zvuk), Some(24_000));
        assert!(zakladna_z_frekvencie(0).is_none());
        assert!(zakladna_z_ns(0).is_none());
    }

    #[test]
    fn rozbalenie_33_bitov() {
        let m = Rozbalenie33::MODUL;
        let mut r = Rozbalenie33::nove();
        assert_eq!(r.rozbal(m as u64 - 3000), m - 3000);
        // pretečenie dopredu
        assert_eq!(r.rozbal(1000), m + 1000);
        assert_eq!(r.rozbal(4000), m + 4000);
        // snímka B mierne dozadu cez hranicu
        assert_eq!(r.rozbal(m as u64 - 500), m - 500);
        // vyššie bity sa ignorujú
        assert_eq!(r.rozbal((1u64 << 40) | 7000), m + 7000);
        // druhé pretečenie
        let mut r = Rozbalenie33::nove();
        let mut x = 0i64;
        for _ in 0..5 {
            x += m / 3;
            assert_eq!(r.rozbal((x % m) as u64), x);
        }
        r.reset();
        assert_eq!(r.rozbal(5), 5);
    }

    #[test]
    fn typicke_frekvencie() {
        assert_eq!(typicka_frekvencia(41_708_333), Some(z(24000, 1001)));
        assert_eq!(typicka_frekvencia(41_666_667), Some(z(24, 1)));
        assert_eq!(typicka_frekvencia(40_000_000), Some(z(25, 1)));
        assert_eq!(typicka_frekvencia(33_366_667), Some(z(30000, 1001)));
        assert_eq!(typicka_frekvencia(16_683_333), Some(z(60000, 1001)));
        assert_eq!(typicka_frekvencia(20_000_000), Some(z(50, 1)));
        assert_eq!(typicka_frekvencia(37_000_000), None);
        assert_eq!(typicka_frekvencia(0), None);
        assert_eq!(typicka_frekvencia(-40_000_000), None);
    }
}
