//! Polovičná presnosť (IEEE 754 binary16, „half“) – OpenEXR, TIFF, JPEG XL.

/// Polovičná presnosť (IEEE 754 binary16) na f32 – presne, vrátane subnormálnych, nekonečna a NaN.
pub fn f16_na_f32(h: u16) -> f32 {
    let znamienko = (h as u32 & 0x8000) << 16;
    let exp = (h >> 10) & 0x1F;
    let mant = (h & 0x3FF) as u32;
    let bity = match exp {
        0 if mant == 0 => znamienko,
        0 => {
            // subnormálne: normalizácia mantisy
            let mut e = 113u32;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            znamienko | e << 23 | (m & 0x3FF) << 13
        }
        31 => znamienko | 0x7F80_0000 | mant << 13,
        e => znamienko | (e as u32 + 112) << 23 | mant << 13,
    };
    f32::from_bits(bity)
}

/// f32 na polovičnú presnosť so zaokrúhlením k najbližšej (pri zhode k párnej).
pub fn f32_na_f16(f: f32) -> u16 {
    let b = f.to_bits();
    let znamienko = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32;
    let mant = b & 0x7F_FFFF;
    if exp == 0xFF {
        return znamienko | 0x7C00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return znamienko | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return znamienko;
        }
        let m = mant | 0x80_0000;
        let posun = (14 - e) as u32;
        let mut v = m >> posun;
        let zvysok = m & ((1 << posun) - 1);
        let polovica = 1 << (posun - 1);
        if zvysok > polovica || (zvysok == polovica && v & 1 == 1) {
            v += 1;
        }
        return znamienko | v as u16;
    }
    let mut v = (e as u32) << 10 | mant >> 13;
    let zvysok = mant & 0x1FFF;
    if zvysok > 0x1000 || (zvysok == 0x1000 && v & 1 == 1) {
        v += 1; // prenos do exponentu dá správne aj nekonečno
    }
    znamienko | v as u16
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn polovicna_presnost() {
        // všetky hodnoty f16 tam a späť presne
        for h in 0..=u16::MAX {
            let f = f16_na_f32(h);
            if f.is_nan() {
                assert!(f16_na_f32(f32_na_f16(f)).is_nan());
            } else {
                assert_eq!(f32_na_f16(f), h, "{h:#06x} → {f}");
            }
        }
        assert_eq!(f16_na_f32(0x3C00), 1.0);
        assert_eq!(f16_na_f32(0xC000), -2.0);
        assert_eq!(f16_na_f32(0x0001), 5.960_464_5e-8);
        assert_eq!(f32_na_f16(1.0 + 1.0 / 4096.0), 0x3C00); // zhoda → párna
        assert_eq!(f32_na_f16(70_000.0), 0x7C00);
    }
}
