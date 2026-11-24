//! Čítač bitov RBSP (najvyšší bit bajtu prvý) s kódmi Exp-Golomb – podľa H.265, kap. 7.2 a 9.2.

use orchidea_jadro::{Chyba, Vysledok};

/// Čítač bitov z RBSP (bez bajtov ochrany proti emulácii).
#[derive(Debug, Clone)]
pub struct Citac<'a> {
    data: &'a [u8],
    /// pozícia v bitoch
    poz: usize,
}

fn koniec() -> Chyba {
    Chyba::PoskodeneData("HEVC: dáta sa skončili uprostred syntaxe".into())
}

impl<'a> Citac<'a> {
    pub fn novy(data: &'a [u8]) -> Citac<'a> {
        Citac { data, poz: 0 }
    }

    /// Prečítané bity.
    pub fn pozicia(&self) -> usize {
        self.poz
    }

    /// Zostávajúce bity.
    pub fn zostava(&self) -> usize {
        self.data.len() * 8 - self.poz
    }

    pub fn je_zarovnany(&self) -> bool {
        self.poz.is_multiple_of(8)
    }

    /// Jeden bit – u(1).
    pub fn bit(&mut self) -> Vysledok<bool> {
        let b = *self.data.get(self.poz / 8).ok_or_else(koniec)?;
        let v = (b >> (7 - self.poz % 8)) & 1;
        self.poz += 1;
        Ok(v != 0)
    }

    /// Celé číslo bez znamienka s `n` bitmi (n ≤ 32) – u(n).
    pub fn u(&mut self, n: u32) -> Vysledok<u32> {
        debug_assert!(n <= 32);
        if self.zostava() < n as usize {
            return Err(koniec());
        }
        let mut v: u64 = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()? as u64;
        }
        Ok(v as u32)
    }

    /// Preskočí `n` bitov.
    pub fn preskoc(&mut self, n: usize) -> Vysledok<()> {
        if self.zostava() < n {
            return Err(koniec());
        }
        self.poz += n;
        Ok(())
    }

    /// Exp-Golomb bez znamienka – ue(v). Najviac 32 úvodných núl (hodnota do 2³² − 2).
    pub fn ue(&mut self) -> Vysledok<u32> {
        let mut nuly = 0u32;
        while !self.bit()? {
            nuly += 1;
            if nuly > 31 {
                return Err(Chyba::PoskodeneData("HEVC: kód Exp-Golomb dlhší ako 32 bitov".into()));
            }
        }
        let zvysok = self.u(nuly)? as u64;
        let v = (1u64 << nuly) - 1 + zvysok;
        u32::try_from(v).map_err(|_| Chyba::PoskodeneData("HEVC: hodnota ue(v) mimo rozsahu".into()))
    }

    /// Exp-Golomb so znamienkom – se(v).
    pub fn se(&mut self) -> Vysledok<i32> {
        let k = self.ue()? as i64;
        let v = if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) };
        Ok(v as i32)
    }

    /// ue(v) s kontrolou najväčšej dovolenej hodnoty (meno prvku do chybovej správy).
    pub fn ue_najviac(&mut self, najviac: u32, meno: &str) -> Vysledok<u32> {
        let v = self.ue()?;
        if v > najviac {
            return Err(Chyba::PoskodeneData(format!("HEVC: {meno} = {v} (najviac {najviac})")));
        }
        Ok(v)
    }

    /// se(v) v dovolenom rozsahu.
    pub fn se_rozsah(&mut self, od: i32, po: i32, meno: &str) -> Vysledok<i32> {
        let v = self.se()?;
        if v < od || v > po {
            return Err(Chyba::PoskodeneData(format!("HEVC: {meno} = {v} (rozsah {od}..={po})")));
        }
        Ok(v)
    }

    /// Sú v RBSP ešte dáta pred záverečnými bitmi (rbsp_trailing_bits)? – more_rbsp_data().
    pub fn dalsie_data(&self) -> bool {
        // posledný nenulový bajt nesie záverečný bit 1
        let Some(posledny) = self.data.iter().rposition(|&b| b != 0) else { return false };
        let koncovy_bit = posledny * 8 + 7 - self.data[posledny].trailing_zeros() as usize;
        self.poz < koncovy_bit
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn exp_golomb() {
        // 1 | 010 | 011 | 00100 | 00101 → ue 0, 1, 2, 3, 4
        let d = [0b1010_0110, 0b0100_0010, 0b1000_0000];
        let mut c = Citac::novy(&d);
        assert_eq!(
            [c.ue().unwrap(), c.ue().unwrap(), c.ue().unwrap(), c.ue().unwrap(), c.ue().unwrap()],
            [0, 1, 2, 3, 4]
        );
        // se: 010 → +1, 011 → −1
        let mut c = Citac::novy(&[0b0100_1100]);
        assert_eq!((c.se().unwrap(), c.se().unwrap()), (1, -1));
        // príliš dlhý kód
        assert!(Citac::novy(&[0, 0, 0, 0, 0, 0]).ue().is_err());
        assert!(Citac::novy(&[0x80]).u(9).is_err());
    }

    #[test]
    fn zaverecne_bity() {
        let d = [0b1011_0000, 0b1000_0000];
        let mut c = Citac::novy(&d);
        assert!(c.dalsie_data());
        c.u(8).unwrap();
        assert!(!c.dalsie_data());
    }
}
