//! Počítanie v poli modulo p = 2^255 − 19 a funkcia X25519 (RFC 7748, kap. 5).
//!
//! Číslo je uložené v piatich 51-bitových častiach. Montgomeryho rebrík robí pre každý bit tajného kľúča
//! rovnaké výpočty; výmena hodnôt sa robí maskou, nie vetvením.

const M51: u64 = (1 << 51) - 1;

#[derive(Clone, Copy)]
struct Fe([u64; 5]);

impl Fe {
    const NULA: Fe = Fe([0; 5]);
    const JEDNA: Fe = Fe([1, 0, 0, 0, 0]);

    fn z_bajtov(b: &[u8; 32]) -> Fe {
        let slovo = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bajtov"));
        Fe([
            slovo(0) & M51,
            (slovo(6) >> 3) & M51,
            (slovo(12) >> 6) & M51,
            (slovo(19) >> 1) & M51,
            (slovo(24) >> 12) & M51,
        ])
    }

    fn prenes(mut h: [u64; 5]) -> [u64; 5] {
        for _ in 0..2 {
            for i in 0..4 {
                h[i + 1] += h[i] >> 51;
                h[i] &= M51;
            }
            h[0] += 19 * (h[4] >> 51);
            h[4] &= M51;
        }
        h
    }

    fn na_bajty(self) -> [u8; 32] {
        let mut h = Fe::prenes(self.0);
        // odčítanie p, ak h ≥ p (q je 0 alebo 1, bez vetvenia)
        let mut q = (h[0] + 19) >> 51;
        for &x in &h[1..] {
            q = (x + q) >> 51;
        }
        h[0] += 19 * q;
        for i in 0..4 {
            h[i + 1] += h[i] >> 51;
            h[i] &= M51;
        }
        h[4] &= M51;
        let mut out = [0u8; 32];
        let mut akum: u128 = 0;
        let mut bitov = 0;
        let mut j = 0;
        for &x in &h {
            akum |= (x as u128) << bitov;
            bitov += 51;
            while bitov >= 8 && j < 32 {
                out[j] = akum as u8;
                akum >>= 8;
                bitov -= 8;
                j += 1;
            }
        }
        if j < 32 {
            out[j] = akum as u8;
        }
        out
    }

    fn plus(self, b: Fe) -> Fe {
        Fe(std::array::from_fn(|i| self.0[i] + b.0[i]))
    }

    /// a − b (pripočíta 2p, aby nič nepodtieklo; b musí byť po násobení alebo prenesení)
    fn minus(self, b: Fe) -> Fe {
        const DVE_P: [u64; 5] = [
            0xF_FFFF_FFFF_FFDA,
            0xF_FFFF_FFFF_FFFE,
            0xF_FFFF_FFFF_FFFE,
            0xF_FFFF_FFFF_FFFE,
            0xF_FFFF_FFFF_FFFE,
        ];
        Fe(Fe::prenes(std::array::from_fn(|i| self.0[i] + DVE_P[i] - b.0[i])))
    }

    fn krat(self, b: Fe) -> Fe {
        let [a0, a1, a2, a3, a4] = self.0.map(|x| x as u128);
        let [b0, b1, b2, b3, b4] = b.0.map(|x| x as u128);
        let (c1, c2, c3, c4) = (19 * b1, 19 * b2, 19 * b3, 19 * b4);
        let r = [
            a0 * b0 + a1 * c4 + a2 * c3 + a3 * c2 + a4 * c1,
            a0 * b1 + a1 * b0 + a2 * c4 + a3 * c3 + a4 * c2,
            a0 * b2 + a1 * b1 + a2 * b0 + a3 * c4 + a4 * c3,
            a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0 + a4 * c4,
            a0 * b4 + a1 * b3 + a2 * b2 + a3 * b1 + a4 * b0,
        ];
        Fe::z_dlhych(r)
    }

    fn z_dlhych(mut r: [u128; 5]) -> Fe {
        for i in 0..4 {
            r[i + 1] += r[i] >> 51;
            r[i] &= M51 as u128;
        }
        r[0] += 19 * (r[4] >> 51);
        r[4] &= M51 as u128;
        let mut h = r.map(|x| x as u64);
        h[1] += h[0] >> 51;
        h[0] &= M51;
        Fe(h)
    }

    fn stvorec(self) -> Fe {
        self.krat(self)
    }

    fn krat_male(self, k: u64) -> Fe {
        Fe::z_dlhych(self.0.map(|x| x as u128 * k as u128))
    }

    /// Prevrátená hodnota: a^(p−2) (exponent je verejná konštanta).
    fn prevratena(self) -> Fe {
        // p − 2 = 2^255 − 21: bity 254…0 sú samé jednotky okrem bitov 2 a 4
        let mut r = Fe::JEDNA;
        for bit in (0..255).rev() {
            r = r.stvorec();
            if bit != 2 && bit != 4 {
                r = r.krat(self);
            }
        }
        r
    }
}

/// Výmena dvoch hodnôt, ak `vymen` = 1 (bez vetvenia).
fn vymen(vymen: u64, a: &mut Fe, b: &mut Fe) {
    let maska = 0u64.wrapping_sub(vymen);
    for i in 0..5 {
        let t = maska & (a.0[i] ^ b.0[i]);
        a.0[i] ^= t;
        b.0[i] ^= t;
    }
}

/// X25519: násobok bodu `u` tajným skalárom `k` (RFC 7748, kap. 5).
pub fn x25519(k: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let mut k = *k;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;
    let x1 = Fe::z_bajtov(u);
    let (mut x2, mut z2, mut x3, mut z3) = (Fe::JEDNA, Fe::NULA, x1, Fe::JEDNA);
    let mut zamena = 0u64;
    for t in (0..255).rev() {
        let kt = ((k[t / 8] >> (t % 8)) & 1) as u64;
        zamena ^= kt;
        vymen(zamena, &mut x2, &mut x3);
        vymen(zamena, &mut z2, &mut z3);
        zamena = kt;
        let a = x2.plus(z2);
        let aa = a.stvorec();
        let b = x2.minus(z2);
        let bb = b.stvorec();
        let e = aa.minus(bb);
        let c = x3.plus(z3);
        let d = x3.minus(z3);
        let da = d.krat(a);
        let cb = c.krat(b);
        x3 = da.plus(cb).stvorec();
        z3 = x1.krat(da.minus(cb).stvorec());
        x2 = aa.krat(bb);
        z2 = e.krat(aa.plus(e.krat_male(121_665)));
    }
    vymen(zamena, &mut x2, &mut x3);
    vymen(zamena, &mut z2, &mut z3);
    x2.krat(z2.prevratena()).na_bajty()
}
