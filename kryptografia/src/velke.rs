//! Veľké celé čísla v aritmetike modulo n (Montgomeryho násobenie) – pre overenie podpisov RSA a ECDSA.
//!
//! Montgomery, P. L.: *Modular multiplication without trial division*, Math. Comp. 44 (1985) 519–521; postup CIOS
//! podľa Koç, Ç. K., Acar, T., Kaliski, B. S.: *Analyzing and comparing Montgomery multiplication algorithms*, IEEE
//! Micro 16 (1996). Čísla sú slová po 64 bitoch od najnižšieho (little-endian).
//!
//! **Len verejné údaje** (podpis, verejný kľúč, odtlačok) – výpočet preto nie je v konštantnom čase. Na tajné kľúče
//! sa tento modul nepoužíva.

use std::cmp::Ordering;

/// Číslo zo slov (najnižšie prvé).
pub type Slova = Vec<u64>;

/// Bajty vo veľkom endiane → slová (aspoň jedno slovo).
pub fn z_bajtov(b: &[u8]) -> Slova {
    let mut v = vec![0u64; b.len().div_ceil(8).max(1)];
    for (i, &x) in b.iter().rev().enumerate() {
        v[i / 8] |= (x as u64) << (8 * (i % 8));
    }
    v
}

/// Slová → presne `dlzka` bajtov vo veľkom endiane; `None`, ak sa číslo nezmestí.
pub fn na_bajty(a: &[u64], dlzka: usize) -> Option<Vec<u8>> {
    let mut v = vec![0u8; dlzka];
    for (i, &slovo) in a.iter().enumerate() {
        for j in 0..8 {
            let bajt = (slovo >> (8 * j)) as u8;
            let poz = i * 8 + j;
            if poz < dlzka {
                v[dlzka - 1 - poz] = bajt;
            } else if bajt != 0 {
                return None;
            }
        }
    }
    Some(v)
}

/// Počet významných bitov.
pub fn bity(a: &[u64]) -> usize {
    a.iter().rposition(|&x| x != 0).map_or(0, |i| 64 * i + 64 - a[i].leading_zeros() as usize)
}

/// Bit `i` (0 = najnižší).
pub fn bit(a: &[u64], i: usize) -> bool {
    a.get(i / 64).is_some_and(|x| (x >> (i % 64)) & 1 == 1)
}

pub fn je_nula(a: &[u64]) -> bool {
    a.iter().all(|&x| x == 0)
}

/// Porovnanie čísel rôznej dĺžky.
pub fn porovnaj(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    for i in (0..n).rev() {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    Ordering::Equal
}

/// a −= b (rovnaká dĺžka); vráti výpožičku.
fn odcitaj_do(a: &mut [u64], b: &[u64]) -> bool {
    let mut pozicka = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (r1, p1) = x.overflowing_sub(y);
        let (r2, p2) = r1.overflowing_sub(pozicka as u64);
        *x = r2;
        pozicka = p1 | p2;
    }
    pozicka
}

/// a += b (rovnaká dĺžka); vráti prenos.
fn pricitaj_do(a: &mut [u64], b: &[u64]) -> bool {
    let mut prenos = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (r1, p1) = x.overflowing_add(y);
        let (r2, p2) = r1.overflowing_add(prenos as u64);
        *x = r2;
        prenos = p1 | p2;
    }
    prenos
}

/// Modul n (nepárny, > 1) s predpočítanými hodnotami pre Montgomeryho tvar (R = 2^(64·k)).
#[derive(Debug, Clone)]
pub struct Modul {
    n: Slova,
    /// −n⁻¹ mod 2⁶⁴
    n0: u64,
    /// R² mod n
    r2: Slova,
    /// R mod n = 1 v Montgomeryho tvare
    jedna: Slova,
}

impl Modul {
    /// `None`, ak n nie je nepárne alebo je ≤ 1.
    pub fn novy(n: &[u64]) -> Option<Modul> {
        let k = n.iter().rposition(|&x| x != 0)? + 1;
        let n = n[..k].to_vec();
        if n[0] & 1 == 0 || (k == 1 && n[0] == 1) {
            return None;
        }
        // n⁻¹ mod 2⁶⁴ Newtonovou iteráciou (každý krok zdvojnásobí počet správnych bitov: 1 → 64 za 6 krokov)
        let mut inv: u64 = 1;
        for _ in 0..6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(n[0].wrapping_mul(inv)));
        }
        // R mod n a R² mod n opakovaným zdvojnásobením 1 (2·64·k krokov)
        let mut x = vec![0u64; k];
        x[0] = 1;
        let mut jedna = Vec::new();
        for i in 0..128 * k {
            let prenos = x[k - 1] >> 63;
            for j in (1..k).rev() {
                x[j] = (x[j] << 1) | (x[j - 1] >> 63);
            }
            x[0] <<= 1;
            if prenos == 1 || porovnaj(&x, &n) != Ordering::Less {
                odcitaj_do(&mut x, &n);
            }
            if i + 1 == 64 * k {
                jedna = x.clone();
            }
        }
        Some(Modul { n0: inv.wrapping_neg(), r2: x, jedna, n })
    }

    /// Počet slov modulu.
    pub fn slov(&self) -> usize {
        self.n.len()
    }

    pub fn n(&self) -> &[u64] {
        &self.n
    }

    /// Počet bitov modulu.
    pub fn bity(&self) -> usize {
        bity(&self.n)
    }

    /// Montgomeryho súčin a·b·R⁻¹ mod n (a, b < n, dĺžka k).
    pub fn nasob(&self, a: &[u64], b: &[u64]) -> Slova {
        let k = self.n.len();
        let mut t = vec![0u64; k + 2];
        for &ai in &a[..k] {
            let mut prenos: u128 = 0;
            for j in 0..k {
                let s = t[j] as u128 + ai as u128 * b[j] as u128 + prenos;
                t[j] = s as u64;
                prenos = s >> 64;
            }
            let s = t[k] as u128 + prenos;
            t[k] = s as u64;
            t[k + 1] = (s >> 64) as u64;
            let m = t[0].wrapping_mul(self.n0);
            let s = t[0] as u128 + m as u128 * self.n[0] as u128;
            let mut prenos = s >> 64;
            for j in 1..k {
                let s = t[j] as u128 + m as u128 * self.n[j] as u128 + prenos;
                t[j - 1] = s as u64;
                prenos = s >> 64;
            }
            let s = t[k] as u128 + prenos;
            t[k - 1] = s as u64;
            t[k] = t[k + 1] + (s >> 64) as u64;
        }
        let mut r = t[..k].to_vec();
        if t[k] != 0 || porovnaj(&r, &self.n) != Ordering::Less {
            odcitaj_do(&mut r, &self.n);
        }
        r
    }

    /// Bežné číslo (už < n, ľubovoľná dĺžka ≤ k) → Montgomeryho tvar.
    pub fn do_tvaru(&self, a: &[u64]) -> Slova {
        let mut x = a.to_vec();
        x.resize(self.n.len(), 0);
        self.nasob(&x, &self.r2)
    }

    /// Montgomeryho tvar → bežné číslo.
    pub fn z_tvaru(&self, a: &[u64]) -> Slova {
        let mut jedna = vec![0u64; self.n.len()];
        jedna[0] = 1;
        self.nasob(a, &jedna)
    }

    /// Jednotka v Montgomeryho tvare.
    pub fn jedna(&self) -> Slova {
        self.jedna.clone()
    }

    /// Redukcia čísla s najviac 2·k slovami mod n (bežný tvar) – postupným odčítaním posunutého n by bolo pomalé,
    /// preto cez Montgomeryho tvar: a = a_h·2^(64k) + a_l → (a_l + a_h·R) mod n.
    pub fn redukuj(&self, a: &[u64]) -> Option<Slova> {
        let k = self.n.len();
        if a.len() > 2 * k && !je_nula(&a[2 * k..]) {
            return None;
        }
        let mut nizke = a[..a.len().min(k)].to_vec();
        nizke.resize(k, 0);
        while porovnaj(&nizke, &self.n) != Ordering::Less {
            odcitaj_do(&mut nizke, &self.n);
        }
        if a.len() <= k {
            return Some(nizke);
        }
        let mut vysoke = a[k..a.len().min(2 * k)].to_vec();
        vysoke.resize(k, 0);
        while porovnaj(&vysoke, &self.n) != Ordering::Less {
            odcitaj_do(&mut vysoke, &self.n);
        }
        // vysoke·R mod n = nasob(vysoke, R²)
        let v = self.nasob(&vysoke, &self.r2);
        Some(self.pricitaj(&nizke, &v))
    }

    /// (a + b) mod n (a, b < n; platí v oboch tvaroch).
    pub fn pricitaj(&self, a: &[u64], b: &[u64]) -> Slova {
        let mut r = a.to_vec();
        let prenos = pricitaj_do(&mut r, b);
        if prenos || porovnaj(&r, &self.n) != Ordering::Less {
            odcitaj_do(&mut r, &self.n);
        }
        r
    }

    /// (a − b) mod n.
    pub fn odcitaj(&self, a: &[u64], b: &[u64]) -> Slova {
        let mut r = a.to_vec();
        if odcitaj_do(&mut r, b) {
            pricitaj_do(&mut r, &self.n);
        }
        r
    }

    /// a^e mod n; `a` v Montgomeryho tvare, výsledok tiež; exponent ako bežné číslo.
    pub fn mocnina(&self, a: &[u64], e: &[u64]) -> Slova {
        let mut r = self.jedna();
        for i in (0..bity(e)).rev() {
            r = self.nasob(&r, &r);
            if bit(e, i) {
                r = self.nasob(&r, a);
            }
        }
        r
    }

    /// a⁻¹ mod n pre prvočíselné n (malá Fermatova veta: a^(n−2)); Montgomeryho tvar.
    pub fn inverzia_prvocislo(&self, a: &[u64]) -> Slova {
        let mut e = self.n.clone();
        let mut dva = vec![0u64; e.len()];
        dva[0] = 2;
        odcitaj_do(&mut e, &dva);
        self.mocnina(a, &e)
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn hex(s: &str) -> Slova {
        let b: Vec<u8> =
            (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
        z_bajtov(&b)
    }

    #[test]
    fn bajty_tam_a_spat() {
        let b = [1u8, 2, 3, 4, 5, 6, 7, 8, 9];
        let a = z_bajtov(&b);
        assert_eq!(a, vec![0x0203040506070809, 0x01]);
        assert_eq!(na_bajty(&a, 9).unwrap(), b);
        assert_eq!(na_bajty(&a, 11).unwrap(), [0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(na_bajty(&a, 8).is_none());
        assert_eq!(bity(&a), 65);
        assert!(bit(&a, 64) && !bit(&a, 63) && bit(&a, 0));
    }

    /// Malé čísla proti u128: Montgomeryho súčin, mocnina a inverzia.
    #[test]
    fn male_cisla_proti_u128() {
        for n in [0xffff_ffff_ffff_ffc5u64, 1_000_000_007, 0xd2b7_3f4a_9e1c_0e3b] {
            let m = Modul::novy(&[n]).unwrap();
            for (a, b) in [(3u64, 5u64), (n - 1, n - 2), (123_456_789, 987_654_321 % n)] {
                let am = m.do_tvaru(&[a]);
                let bm = m.do_tvaru(&[b]);
                let c = m.z_tvaru(&m.nasob(&am, &bm));
                assert_eq!(c[0] as u128, (a as u128 * b as u128) % n as u128, "n={n:x}");
                let s = m.z_tvaru(&m.pricitaj(&am, &bm));
                assert_eq!(s[0] as u128, (a as u128 + b as u128) % n as u128);
                let d = m.z_tvaru(&m.odcitaj(&am, &bm));
                assert_eq!(d[0] as u128, (a as u128 + n as u128 - b as u128) % n as u128);
            }
        }
        // 2^64−59 je prvočíslo: a · a⁻¹ = 1
        let p = 0xffff_ffff_ffff_ffc5u64;
        let m = Modul::novy(&[p]).unwrap();
        let a = m.do_tvaru(&[0x1234_5678_9abc_def0]);
        assert_eq!(m.z_tvaru(&m.nasob(&a, &m.inverzia_prvocislo(&a))), vec![1]);
        assert!(Modul::novy(&[10]).is_none() && Modul::novy(&[1]).is_none() && Modul::novy(&[0]).is_none());
    }

    /// Viacslovné čísla: 3^(p−1) ≡ 1 mod p pre prvočíslo P-256 (malá Fermatova veta) a redukcia dlhého čísla.
    #[test]
    fn velke_prvocislo() {
        let p = hex("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff");
        let m = Modul::novy(&p).unwrap();
        let mut e = p.clone();
        e[0] -= 1;
        let tri = m.do_tvaru(&[3]);
        assert_eq!(m.z_tvaru(&m.mocnina(&tri, &e)), vec![1, 0, 0, 0]);
        // (p + 5) aj 2^256 + 5 = R + 5 sa zredukujú správne
        let p5 = vec![4, 0x1_0000_0000, 0, 0xffff_ffff_0000_0001];
        assert_eq!(m.redukuj(&p5).unwrap(), vec![5, 0, 0, 0]);
        let dlhe = vec![5, 0, 0, 0, 1, 0, 0, 0];
        // R mod p = 2^256 − p = 2^224 − 2^192 − 2^96 + 1
        assert_eq!(m.jedna(), vec![0x1, 0xffffffff00000000, 0xffffffffffffffff, 0xfffffffe]);
        assert_eq!(m.redukuj(&dlhe).unwrap(), m.pricitaj(&m.jedna(), &[5, 0, 0, 0]));
        assert!(m.redukuj(&[0, 0, 0, 0, 0, 0, 0, 0, 1]).is_none(), "viac ako 2k slov");
    }
}
