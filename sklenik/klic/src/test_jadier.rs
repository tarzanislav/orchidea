//! Strážca pravdy (R100): krátky test každého jadra procesora pri štarte. Každé jadro spočíta tú istú úlohu
//! so známym výsledkom – celočíselné násobenie a rotácie, výpočty s plávajúcou čiarkou podľa IEEE 754 (presne
//! zaokrúhlené, rovnaké na každom procesore) a súčin vektorov, ktorý prekladač vykoná inštrukciami NEON.
//! Jadro s iným výsledkom „počíta potichu zle“ (Google: vrtkavé jadrá) – KLÍČ to ohlási.

use std::hint::black_box;

/// Výsledok, ktorý musí dať každé zdravé jadro (overené skúškou na každej platforme CI).
pub const OCAKAVANE: u64 = 0x6390_b373_fa9f_6d95;

/// Testovacia úloha (asi 5–30 ms podľa procesora).
pub fn vypocet() -> u64 {
    let mut x: u64 = black_box(0x9E37_79B9_7F4A_7C15);
    let mut a: u64 = 0;
    let mut f: f64 = black_box(1.0);
    let mut v = [0u32; 1024];
    for i in 0..100_000u64 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        a = a.wrapping_mul(0x0000_0100_0000_01B3).wrapping_add(x ^ i).rotate_left(5);
        f = (f * 1.000_000_1 + (x >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)).sqrt() + 0.5;
        v[(i & 1023) as usize] = (x >> 32) as u32;
    }
    let mut s = 0u32;
    for _ in 0..64 {
        s = v.iter().zip(v.iter().rev()).fold(s, |s, (p, q)| s.wrapping_add(p.wrapping_mul(*q)));
        v.iter_mut().for_each(|p| *p = p.rotate_left(3) ^ s);
    }
    a ^ f.to_bits() ^ ((s as u64) << 17)
}

#[cfg(test)]
mod testy {
    #[test]
    fn znamy_vysledok() {
        assert_eq!(super::vypocet(), super::OCAKAVANE, "{:#x}", super::vypocet());
    }
}
