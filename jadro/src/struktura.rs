//! Štruktúra – nesignálové dáta (prvotný pojem 2): tvary a cesty, text, strany, tabuľky, scény, časové
//! usporiadania, zoznamy udalostí. Jadro pozná len strom prvkov s druhom a vlastnosťami; čo druh znamená,
//! určujú schopnosti vyšších vrstiev.

use crate::chyba::{Chyba, Vysledok};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::Meno;
use crate::vlastnost::Vlastnosti;

/// Najväčší počet prvkov jednej štruktúry.
pub const NAJVIAC_PRVKOV: usize = 1 << 26;

/// Prvok štruktúry (strom).
#[derive(Debug, Clone, PartialEq)]
pub struct Struktura {
    pub druh: Meno,
    pub vlastnosti: Vlastnosti,
    pub deti: Vec<Struktura>,
}

/// Cesta k prvku: indexy detí od koreňa.
pub type Cesta = Vec<usize>;

impl Struktura {
    pub fn nova(druh: Meno) -> Self {
        Struktura { druh, vlastnosti: Vlastnosti::nove(), deti: Vec::new() }
    }

    pub fn s_dietatom(mut self, dieta: Struktura) -> Self {
        self.deti.push(dieta);
        self
    }

    /// Prvok na ceste.
    pub fn prvok(&self, cesta: &[usize]) -> Option<&Struktura> {
        let mut p = self;
        for &i in cesta {
            p = p.deti.get(i)?;
        }
        Some(p)
    }

    /// Počet všetkých prvkov vrátane koreňa.
    pub fn pocet_prvkov(&self) -> usize {
        1 + self.deti.iter().map(Struktura::pocet_prvkov).sum::<usize>()
    }

    /// Prejde všetky prvky do hĺbky a vráti cesty tých, ktoré spĺňajú podmienku.
    pub fn najdi(&self, podmienka: &dyn Fn(&Struktura) -> bool) -> Vec<Cesta> {
        let mut vysledok = Vec::new();
        let mut zasobnik: Vec<(Cesta, &Struktura)> = vec![(Vec::new(), self)];
        while let Some((cesta, p)) = zasobnik.pop() {
            if podmienka(p) {
                vysledok.push(cesta.clone());
            }
            for (i, d) in p.deti.iter().enumerate().rev() {
                let mut c = cesta.clone();
                c.push(i);
                zasobnik.push((c, d));
            }
        }
        vysledok
    }
}

impl Kodovatelne for Struktura {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.meno(&self.druh);
        self.vlastnosti.zakoduj(z);
        z.cislo(self.deti.len() as u64);
        for d in &self.deti {
            d.zakoduj(z);
        }
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        c.vnor()?;
        let druh = c.meno()?;
        let vlastnosti = Vlastnosti::dekoduj(c)?;
        let n = c.pocet(3)?;
        if n > NAJVIAC_PRVKOV {
            return Err(Chyba::Limit("príliš veľa prvkov štruktúry".into()));
        }
        let mut deti = Vec::with_capacity(n);
        for _ in 0..n {
            deti.push(Struktura::dekoduj(c)?);
        }
        c.vynor();
        Ok(Struktura { druh, vlastnosti, deti })
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::hodnota::Hodnota;
    use crate::meno::m;

    #[test]
    fn strom_a_hladanie() {
        let s = Struktura::nova(m("skupina"))
            .s_dietatom(Struktura::nova(m("cesta")))
            .s_dietatom(Struktura::nova(m("skupina")).s_dietatom(Struktura::nova(m("cesta"))));
        assert_eq!(s.pocet_prvkov(), 4);
        assert_eq!(s.najdi(&|p| p.druh.ako_text() == "cesta"), vec![vec![0], vec![1, 0]]);
        assert_eq!(s.prvok(&[1, 0]).unwrap().druh, m("cesta"));
        let mut t = s.clone();
        t.vlastnosti.nastav(m("meno"), Hodnota::text("prvá"));
        assert_eq!(Struktura::z_bajtov(&t.do_bajtov()).unwrap(), t);
    }
}
