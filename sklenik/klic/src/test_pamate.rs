//! Skúška pamäte RAM pri štarte (R120 – skúška súčiastok).
//!
//! Krabičky s požičaným nastavením pamäte (Tanix TX68 má nastavenie DDR z dosky Transpeed 8K618-T) alebo
//! prehriate kusy môžu mať chybnú pamäť, ktorá sa prejaví až „náhodnými“ pádmi. Krátka skúška pri štarte
//! zapíše a prečíta úsek RAM väčší než všetky vyrovnávacie pamäte procesora tromi vzorkami:
//!   1. adresa v adrese (každé slovo nesie svoju polohu – odhalí skraty a zámeny adresových vodičov),
//!   2. jej doplnok (každý bit raz 0 a raz 1 – odhalí zaseknuté bity),
//!   3. striedanie 0101…/1010… po susedných slovách (odhalí presluch dátových vodičov).
//!
//! Zápis aj čítanie idú cez atomické slová (na ARM obyčajné inštrukcie str/ldr) – prekladač ich nesmie
//! vynechať ani nahradiť známou hodnotou, a to bez jediného riadku `unsafe`.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// Chyba pamäte: poloha (bajt od začiatku úseku), očakávaná a prečítaná hodnota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChybaPamate {
    pub bajt: usize,
    pub ocakavane: u64,
    pub precitane: u64,
}

const KLUC: u64 = 0x9E37_79B9_7F4A_7C15;

fn vzorka(prechod: usize, i: usize) -> u64 {
    let a = (i as u64).wrapping_mul(KLUC) ^ (i as u64);
    match prechod {
        0 => a,
        1 => !a,
        _ => {
            if i.is_multiple_of(2) {
                0x5555_5555_5555_5555
            } else {
                0xAAAA_AAAA_AAAA_AAAA
            }
        }
    }
}

/// Otestuje `mb` megabajtov RAM. Vráti trvanie alebo prvú nájdenú chybu.
pub fn otestuj(mb: usize) -> Result<Duration, ChybaPamate> {
    let slov = (mb.max(1) << 20) / 8;
    let mut pamat: Vec<AtomicU64> = Vec::new();
    // ak systém toľko pamäte nedá, skúška sa zmenší – nesmie sama zhodiť systém
    if pamat.try_reserve_exact(slov).is_err() {
        return if mb > 1 { otestuj(mb / 2) } else { Ok(Duration::ZERO) };
    }
    pamat.extend((0..slov).map(|_| AtomicU64::new(0)));
    let t = Instant::now();
    for prechod in 0..3 {
        for (i, slovo) in pamat.iter().enumerate() {
            slovo.store(vzorka(prechod, i), Relaxed);
        }
        for (i, slovo) in pamat.iter().enumerate() {
            let v = slovo.load(Relaxed);
            let ocakavane = vzorka(prechod, i);
            if v != ocakavane {
                return Err(ChybaPamate { bajt: i * 8, ocakavane, precitane: v });
            }
        }
    }
    Ok(t.elapsed())
}

/// Koľko MB skúšať: 64 MB (viac než vyrovnávacie pamäte každého procesora), najviac 1/8 voľnej pamäte.
pub fn velkost_mb(dostupna_kb: u64) -> usize {
    ((dostupna_kb / 1024 / 8) as usize).clamp(4, 64)
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn zdrava_pamat_prejde() {
        let t = otestuj(4).expect("pamäť počítača je v poriadku");
        assert!(t < Duration::from_secs(10));
    }

    #[test]
    fn vzorky_sa_lisia_a_doplnok_meni_kazdy_bit() {
        for i in [0usize, 1, 2, 1000, 1 << 20] {
            assert_eq!(vzorka(0, i) ^ vzorka(1, i), u64::MAX);
            assert_ne!(vzorka(0, i), vzorka(0, i + 1), "susedné slová nesmú mať rovnakú hodnotu");
        }
        assert_eq!(vzorka(2, 0) ^ vzorka(2, 1), u64::MAX);
    }

    #[test]
    fn velkost_skusky() {
        assert_eq!(velkost_mb(4_000_000), 64);
        assert_eq!(velkost_mb(400_000), 48);
        assert_eq!(velkost_mb(0), 4);
    }
}
