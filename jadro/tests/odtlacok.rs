//! Overenie vlastného BLAKE3 voči hodnotám z nezávislej referenčnej implementácie.

#[path = "data/vektory_blake3.rs"]
mod vektory_blake3;

use orchidea_jadro::{Hasovac, Odtlacok};
use vektory_blake3::VEKTORY;

const KLUC: &[u8; 32] = b"whats the Elvish word for friend";
const KONTEXT: &str = "BLAKE3 2019-12-27 16:29:52 test vectors context";

fn vstup(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn bezny_odtlacok_aj_rozsireny_vystup() {
    for &(n, ocak, _, _) in VEKTORY {
        let d = vstup(n);
        let mut dlhy = vec![0u8; ocak.len() / 2];
        Hasovac::novy().pridaj(&d).dokonci_do(&mut dlhy);
        assert_eq!(hex(&dlhy), ocak, "dĺžka {n}");
        assert_eq!(Odtlacok::z(&d).sestnastkovo(), &ocak[..64], "dĺžka {n}");
    }
}

#[test]
fn odtlacok_s_klucom() {
    for &(n, _, ocak, _) in VEKTORY {
        assert_eq!(Hasovac::s_klucom(KLUC).pridaj(&vstup(n)).dokonci().sestnastkovo(), ocak, "dĺžka {n}");
    }
}

#[test]
fn odvodenie_kluca() {
    for &(n, _, _, ocak) in VEKTORY {
        assert_eq!(
            Hasovac::odvodenie_kluca(KONTEXT).pridaj(&vstup(n)).dokonci().sestnastkovo(),
            ocak,
            "dĺžka {n}"
        );
    }
}

#[test]
fn po_castiach_je_rovnake_ako_naraz() {
    let d = vstup(20_000);
    let naraz = Odtlacok::z(&d);
    for krok in [1usize, 7, 63, 64, 65, 1023, 1024, 1025, 4096] {
        let mut h = Hasovac::novy();
        for kus in d.chunks(krok) {
            h.pridaj(kus);
        }
        assert_eq!(h.dokonci(), naraz, "krok {krok}");
    }
    let t = naraz.sestnastkovo();
    assert_eq!(Odtlacok::zo_sestnastkovej(&t), Some(naraz));
    assert_eq!(Odtlacok::zo_sestnastkovej("xyz"), None);
}
