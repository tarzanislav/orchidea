//! Dlaždica – obdĺžnikový kus obrazu, nemenný po zdieľaní, s odtlačkom v kanonickom tvare.

use orchidea_jadro::signal::PrvokSignalu;
use orchidea_jadro::{Hasovac, Odtlacok};
use std::sync::OnceLock;

/// Bitová rovnosť dvoch hodnôt (aj pre čísla s plávajúcou čiarkou: −0 a +0 sú rôzne, NaN sa rovná sebe).
#[inline]
pub(crate) fn rovnake<T: PrvokSignalu>(a: T, b: T) -> bool {
    a.do_f64().to_bits() == b.do_f64().to_bits()
}

#[derive(Clone)]
enum Obsah<T> {
    /// Všetky pixely rovnaké – uložený je jeden pixel.
    Jednoliaty(Box<[T]>),
    /// Pixely po riadkoch, kanály za sebou.
    Data(Box<[T]>),
}

/// Dlaždica obrazu. Okrajové dlaždice môžu byť menšie ako strana mriežky.
#[derive(Clone)]
pub struct Dlazdica<T> {
    sirka: usize,
    vyska: usize,
    kanaly: usize,
    obsah: Obsah<T>,
    odtlacok: OnceLock<Odtlacok>,
}

impl<T: PrvokSignalu> std::fmt::Debug for Dlazdica<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let druh = if self.je_jednoliata() { "jednoliata" } else { "rozbalená" };
        write!(f, "Dlaždica {}×{}×{} {} ({druh})", self.sirka, self.vyska, self.kanaly, T::TYP.meno())
    }
}

impl<T: PrvokSignalu> Dlazdica<T> {
    /// Dlaždica plná jednej hodnoty pixela.
    pub(crate) fn jednoliata(sirka: usize, vyska: usize, pixel: &[T]) -> Dlazdica<T> {
        Dlazdica {
            sirka,
            vyska,
            kanaly: pixel.len(),
            obsah: Obsah::Jednoliaty(pixel.into()),
            odtlacok: OnceLock::new(),
        }
    }

    /// Dlaždica z hodnôt; ak sú všetky pixely rovnaké, uloží sa ako jednoliata.
    pub(crate) fn z_dat(sirka: usize, vyska: usize, kanaly: usize, data: Vec<T>) -> Dlazdica<T> {
        debug_assert_eq!(data.len(), sirka * vyska * kanaly);
        let mut d =
            Dlazdica { sirka, vyska, kanaly, obsah: Obsah::Data(data.into()), odtlacok: OnceLock::new() };
        d.zhustni();
        d
    }

    pub fn sirka(&self) -> usize {
        self.sirka
    }

    pub fn vyska(&self) -> usize {
        self.vyska
    }

    pub fn kanaly(&self) -> usize {
        self.kanaly
    }

    /// Je dlaždica uložená ako jednoliata (jedna hodnota pre všetky pixely)?
    pub fn je_jednoliata(&self) -> bool {
        matches!(self.obsah, Obsah::Jednoliaty(_))
    }

    /// Hodnoty pixela na súradniciach dlaždice.
    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> &[T] {
        debug_assert!(x < self.sirka && y < self.vyska);
        match &self.obsah {
            Obsah::Jednoliaty(p) => p,
            Obsah::Data(d) => {
                let i = (y * self.sirka + x) * self.kanaly;
                &d[i..i + self.kanaly]
            }
        }
    }

    /// Riadok dlaždice (len pri rozbalenej dlaždici).
    #[inline]
    pub(crate) fn riadok(&self, y: usize) -> Option<&[T]> {
        match &self.obsah {
            Obsah::Data(d) => {
                let n = self.sirka * self.kanaly;
                Some(&d[y * n..(y + 1) * n])
            }
            Obsah::Jednoliaty(_) => None,
        }
    }

    /// Pamäť hodnôt dlaždice v bajtoch.
    pub fn bajtov(&self) -> usize {
        let n = match &self.obsah {
            Obsah::Jednoliaty(p) => p.len(),
            Obsah::Data(d) => d.len(),
        };
        n * T::TYP.velkost()
    }

    /// Jednoliata hodnota – uložená, alebo zistená z dát (všetky pixely bitovo rovnaké).
    pub fn jednoliata_hodnota(&self) -> Option<&[T]> {
        match &self.obsah {
            Obsah::Jednoliaty(p) => Some(p),
            Obsah::Data(d) => {
                let prvy = &d[..self.kanaly];
                d.chunks_exact(self.kanaly)
                    .all(|p| p.iter().zip(prvy).all(|(&a, &b)| rovnake(a, b)))
                    .then_some(prvy)
            }
        }
    }

    /// Prevedie rovnaké pixely na jednoliatu dlaždicu (odtlačok sa nemení – je kanonický).
    pub(crate) fn zhustni(&mut self) {
        if let Obsah::Data(_) = self.obsah
            && let Some(p) = self.jednoliata_hodnota()
        {
            self.obsah = Obsah::Jednoliaty(p.into());
        }
    }

    /// Hodnoty na zápis: jednoliata dlaždica sa rozbalí, odtlačok sa zabudne.
    pub(crate) fn na_zapis(&mut self) -> &mut [T] {
        self.odtlacok = OnceLock::new();
        if let Obsah::Jednoliaty(p) = &self.obsah {
            let mut d = Vec::with_capacity(self.sirka * self.vyska * self.kanaly);
            for _ in 0..self.sirka * self.vyska {
                d.extend_from_slice(p);
            }
            self.obsah = Obsah::Data(d.into());
        }
        match &mut self.obsah {
            Obsah::Data(d) => d,
            Obsah::Jednoliaty(_) => unreachable!("dlaždica bola práve rozbalená"),
        }
    }

    /// Najmenší obdĺžnik (v súradniciach dlaždice) s bitovo rozdielnymi pixelmi oproti dlaždici rovnakých
    /// rozmerov; `None`, ak sú rovnaké.
    pub(crate) fn rozdiel(&self, ina: &Dlazdica<T>) -> Option<crate::Obdlznik> {
        debug_assert_eq!((self.sirka, self.vyska, self.kanaly), (ina.sirka, ina.vyska, ina.kanaly));
        let k = self.kanaly;
        let mut obal: Option<(usize, usize, usize, usize)> = None;
        for y in 0..self.vyska {
            let rozne =
                |x: usize| !self.pixel(x, y).iter().zip(ina.pixel(x, y)).all(|(&a, &b)| rovnake(a, b));
            let prvy = match (self.riadok(y), ina.riadok(y)) {
                // rýchla cesta: celé riadky po hodnotách
                (Some(a), Some(b)) => a.iter().zip(b).position(|(&p, &q)| !rovnake(p, q)).map(|i| i / k),
                _ => (0..self.sirka).find(|&x| rozne(x)),
            };
            let Some(x0) = prvy else { continue };
            let x1 = (x0..self.sirka).rev().find(|&x| rozne(x)).unwrap_or(x0);
            obal = Some(match obal {
                None => (x0, y, x1, y),
                Some((a, b, c, _)) => (a.min(x0), b, c.max(x1), y),
            });
        }
        obal.map(|(x0, y0, x1, y1)| crate::Obdlznik::novy(x0, y0, x1 - x0 + 1, y1 - y0 + 1))
    }

    /// Je odtlačok už spočítaný? (bez počítania)
    pub(crate) fn znamy_odtlacok(&self) -> Option<Odtlacok> {
        self.odtlacok.get().copied()
    }

    /// Odtlačok obsahu v kanonickom tvare: rozmery, typ, kanály a buď jednoliata hodnota, alebo všetky hodnoty.
    /// Jednoliata a rozbalená dlaždica s rovnakým obsahom majú rovnaký odtlačok. Počíta sa raz.
    pub fn odtlacok(&self) -> Odtlacok {
        *self.odtlacok.get_or_init(|| {
            let mut h = Hasovac::odvodenie_kluca("ORCHIDEA dlaždica v1");
            let mut b = Vec::with_capacity(1 << 14);
            for v in [self.sirka as u64, self.vyska as u64, self.kanaly as u64] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(T::TYP.meno().as_bytes());
            match self.jednoliata_hodnota() {
                Some(p) => {
                    b.push(1);
                    p.iter().for_each(|v| v.zapis_le(&mut b));
                    h.pridaj(&b);
                }
                None => {
                    b.push(0);
                    h.pridaj(&b);
                    if let Obsah::Data(d) = &self.obsah {
                        for kus in d.chunks(4096) {
                            b.clear();
                            kus.iter().for_each(|v| v.zapis_le(&mut b));
                            h.pridaj(&b);
                        }
                    }
                }
            }
            h.dokonci()
        })
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn kanonicky_odtlacok() {
        let j = Dlazdica::jednoliata(4, 3, &[1u8, 2, 3]);
        let mut r = j.clone();
        r.na_zapis();
        assert!(!r.je_jednoliata() && j.je_jednoliata());
        assert_eq!(j.odtlacok(), r.odtlacok());
        assert_eq!(r.bajtov(), 4 * 3 * 3);
        r.na_zapis()[5] = 9;
        assert_ne!(j.odtlacok(), r.odtlacok());
        assert_eq!(r.pixel(1, 0), &[1, 2, 9]);
        // iné rozmery s rovnakou hodnotou = iný odtlačok
        assert_ne!(j.odtlacok(), Dlazdica::jednoliata(3, 4, &[1u8, 2, 3]).odtlacok());
    }

    #[test]
    fn rozdiel_dlazdic() {
        let a = Dlazdica::jednoliata(8, 6, &[1u8, 1]);
        let mut b = a.clone();
        assert_eq!(a.rozdiel(&b), None);
        let d = b.na_zapis();
        d[(2 * 8 + 3) * 2 + 1] = 5;
        d[(4 * 8 + 6) * 2] = 7;
        assert_eq!(a.rozdiel(&b), Some(crate::Obdlznik::novy(3, 2, 4, 3)));
        assert_eq!(b.rozdiel(&a), Some(crate::Obdlznik::novy(3, 2, 4, 3)));
        let mut c = b.clone();
        c.na_zapis()[0] = 9;
        assert_eq!(b.rozdiel(&c), Some(crate::Obdlznik::novy(0, 0, 1, 1)));
    }

    #[test]
    fn zhustenie_a_bitova_rovnost() {
        let d = Dlazdica::z_dat(2, 2, 1, vec![0.5f32; 4]);
        assert!(d.je_jednoliata());
        // −0 a +0 nie sú tá istá hodnota
        let z = Dlazdica::z_dat(2, 1, 1, vec![0.0f32, -0.0]);
        assert!(!z.je_jednoliata());
        assert_ne!(z.odtlacok(), Dlazdica::jednoliata(2, 1, &[0.0f32]).odtlacok());
    }
}
