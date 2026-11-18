//! Paralelné spracovanie po kúskoch – bez cudzích knižníc, len so `std::thread::scope`.
//!
//! Výsledok nezávisí od počtu vlákien: každý kúsok spracuje práve jedno vlákno a výsledky sa skladajú
//! v pevnom poradí. Na webe (WebAssembly bez vlákien) sa všetko vykoná postupne v jednom vlákne.

use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

static OBMEDZENIE: AtomicUsize = AtomicUsize::new(0);

/// Obmedzí počet vlákien (0 = podľa procesora). Hodí sa pre merania a pre slabšie zariadenia.
pub fn obmedz_vlakna(pocet: usize) {
    OBMEDZENIE.store(pocet, Ordering::Relaxed);
}

/// Počet vlákien, ktoré sa použijú.
pub fn pocet_vlakien() -> usize {
    static ZISTENE: OnceLock<usize> = OnceLock::new();
    let zistene = *ZISTENE.get_or_init(|| {
        if cfg!(target_family = "wasm") {
            1
        } else {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
        }
    });
    match OBMEDZENIE.load(Ordering::Relaxed) {
        0 => zistene,
        n => n.min(zistene).max(1),
    }
}

/// Spracuje meniteľné dáta po kúskoch pevnej veľkosti. Funkcia dostane poradové číslo kúska a kúsok.
pub fn pre_kusy<T: Send>(data: &mut [T], velkost_kusa: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let velkost_kusa = velkost_kusa.max(1);
    let kusov = data.len().div_ceil(velkost_kusa);
    let vlakna = pocet_vlakien().min(kusov);
    if vlakna <= 1 {
        for (i, kus) in data.chunks_mut(velkost_kusa).enumerate() {
            f(i, kus);
        }
        return;
    }
    let mut skupiny: Vec<Vec<(usize, &mut [T])>> = (0..vlakna).map(|_| Vec::new()).collect();
    for (i, kus) in data.chunks_mut(velkost_kusa).enumerate() {
        skupiny[i % vlakna].push((i, kus));
    }
    let f = &f;
    std::thread::scope(|s| {
        let mut skupiny = skupiny.into_iter();
        let prva = skupiny.next();
        for skupina in skupiny {
            s.spawn(move || {
                for (i, kus) in skupina {
                    f(i, kus);
                }
            });
        }
        if let Some(skupina) = prva {
            for (i, kus) in skupina {
                f(i, kus);
            }
        }
    });
}

/// Rozdelí rozsah 0..n na kúsky, každý spracuje funkciou a výsledky vráti v poradí kúskov.
pub fn mapuj_rozsah<R: Send>(n: usize, velkost_kusa: usize, f: impl Fn(Range<usize>) -> R + Sync) -> Vec<R> {
    let velkost_kusa = velkost_kusa.max(1);
    let kusov = n.div_ceil(velkost_kusa);
    let mut vysledky: Vec<Option<R>> = (0..kusov).map(|_| None).collect();
    pre_kusy(&mut vysledky, 1, |i, miesto| {
        let od = i * velkost_kusa;
        miesto[0] = Some(f(od..(od + velkost_kusa).min(n)));
    });
    vysledky.into_iter().map(|v| v.expect("každý kúsok bol spracovaný")).collect()
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn vysledok_nezavisi_od_vlakien() {
        let mut a: Vec<u64> = (0..100_003).collect();
        let mut b = a.clone();
        obmedz_vlakna(1);
        pre_kusy(&mut a, 1000, |i, k| k.iter_mut().for_each(|x| *x = *x * 3 + i as u64));
        obmedz_vlakna(0);
        pre_kusy(&mut b, 1000, |i, k| k.iter_mut().for_each(|x| *x = *x * 3 + i as u64));
        assert_eq!(a, b);
        let sucty = mapuj_rozsah(10_001, 100, |r| r.sum::<usize>());
        assert_eq!(sucty.iter().sum::<usize>(), 10_000 * 10_001 / 2);
    }
}
