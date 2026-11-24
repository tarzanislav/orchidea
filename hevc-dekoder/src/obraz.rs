//! Obraz dekodéra: tri roviny vzoriek (Y, Cb, Cr) vo formáte 4:2:0 bez okrajov.
//!
//! Vzorky majú 8 bitov (profil Main). Celý dekodér používa typ [`Vzorka`] – prechod na 10 bitov (Main 10) znamená
//! zmeniť tento typ a bitovú hĺbku, nie prepísať moduly.

/// Jedna vzorka obrazu (profil Main: 8 bitov).
pub type Vzorka = u8;

/// Zarovnanie kroku riadka v prvkoch – riadky začínajú na hranici 32 bajtov (rýchle vektorové čítanie, NEON).
const ZAROVNANIE: usize = 32;

/// Rovina vzoriek: riadok `y` začína na `data[y * krok]`, platných je `sirka` vzoriek.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rovina {
    pub data: Vec<Vzorka>,
    pub sirka: usize,
    pub vyska: usize,
    /// vzdialenosť začiatkov riadkov v prvkoch (≥ `sirka`)
    pub krok: usize,
}

impl Rovina {
    /// Nová rovina vyplnená hodnotou `hodnota`.
    pub fn nova(sirka: usize, vyska: usize, hodnota: Vzorka) -> Rovina {
        let krok = sirka.div_ceil(ZAROVNANIE) * ZAROVNANIE;
        Rovina { data: vec![hodnota; krok * vyska], sirka, vyska, krok }
    }

    /// Vzorka na súradniciach (x, y) – bez kontroly hraníc nad rámec indexovania.
    #[inline]
    pub fn vzorka(&self, x: usize, y: usize) -> Vzorka {
        self.data[y * self.krok + x]
    }

    /// Platné vzorky riadka `y`.
    #[inline]
    pub fn riadok(&self, y: usize) -> &[Vzorka] {
        &self.data[y * self.krok..y * self.krok + self.sirka]
    }

    /// Rovina od bodu (x, y) po koniec – na zápis bloku (riadok i bloku je `[i * krok..]`).
    #[inline]
    pub fn od(&mut self, x: usize, y: usize) -> &mut [Vzorka] {
        let i = y * self.krok + x;
        &mut self.data[i..]
    }
}

/// Dekódovaný obraz 4:2:0: roviny Y, Cb, Cr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obraz {
    pub roviny: [Rovina; 3],
    pub bitova_hlbka: u32,
}

impl Obraz {
    /// Nový obraz s rozmermi jasu `sirka` × `vyska` vyplnený strednou hodnotou 1 << (bitova_hlbka − 1)
    /// (to je aj obsah chýbajúceho referenčného obrazu podľa 8.3.3.2).
    pub fn novy(sirka: usize, vyska: usize) -> Obraz {
        let stred = 1 << 7;
        Obraz {
            roviny: [
                Rovina::nova(sirka, vyska, stred),
                Rovina::nova(sirka / 2, vyska / 2, stred),
                Rovina::nova(sirka / 2, vyska / 2, stred),
            ],
            bitova_hlbka: 8,
        }
    }

    /// Prázdny obraz bez pamäte (zástupca pri presune rovín).
    pub fn prazdny() -> Obraz {
        let r = || Rovina { data: Vec::new(), sirka: 0, vyska: 0, krok: 0 };
        Obraz { roviny: [r(), r(), r()], bitova_hlbka: 8 }
    }

    pub fn sirka(&self) -> usize {
        self.roviny[0].sirka
    }

    pub fn vyska(&self) -> usize {
        self.roviny[0].vyska
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn rozmery_a_zarovnanie() {
        let o = Obraz::novy(1920, 1080);
        assert_eq!((o.sirka(), o.vyska()), (1920, 1080));
        assert_eq!((o.roviny[1].sirka, o.roviny[1].vyska), (960, 540));
        assert_eq!(o.roviny[0].krok % ZAROVNANIE, 0);
        let r = Rovina::nova(100, 3, 7);
        assert_eq!(r.krok, 128);
        assert_eq!(r.riadok(2).len(), 100);
        assert_eq!(r.vzorka(99, 2), 7);
    }
}
