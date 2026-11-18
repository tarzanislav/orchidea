//! Vlastnosť – údaj pripojený k čomukoľvek (prvotný pojem 3): metadáta, značka, význam, hodnotenie,
//! pripomienka, pôvod. Jadro nehovorí, *na čo* údaj je – to určuje ten, kto ho zapisuje alebo číta.

use crate::chyba::Vysledok;
use crate::hodnota::Hodnota;
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::Meno;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Množina vlastností (meno → hodnota), lacno zdieľaná.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Vlastnosti(Arc<BTreeMap<Meno, Hodnota>>);

impl Vlastnosti {
    pub fn nove() -> Self {
        Vlastnosti::default()
    }

    pub fn ziskaj(&self, meno: &str) -> Option<&Hodnota> {
        self.0.get(meno)
    }

    pub fn nastav(&mut self, meno: Meno, hodnota: Hodnota) {
        Arc::make_mut(&mut self.0).insert(meno, hodnota);
    }

    pub fn s(mut self, meno: Meno, hodnota: Hodnota) -> Self {
        self.nastav(meno, hodnota);
        self
    }

    pub fn odstran(&mut self, meno: &Meno) -> Option<Hodnota> {
        Arc::make_mut(&mut self.0).remove(meno)
    }

    pub fn je_prazdne(&self) -> bool {
        self.0.is_empty()
    }

    pub fn dlzka(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Meno, &Hodnota)> {
        self.0.iter()
    }

    pub fn mena(&self) -> impl Iterator<Item = &Meno> {
        self.0.keys()
    }
}

impl Kodovatelne for Vlastnosti {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.cislo(self.0.len() as u64);
        for (k, v) in self.0.iter() {
            z.meno(k);
            v.zakoduj(z);
        }
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        c.vnor()?;
        let n = c.pocet(3)?;
        let mut mapa = BTreeMap::new();
        for _ in 0..n {
            let k = c.meno()?;
            let v = Hodnota::dekoduj(c)?;
            if mapa.insert(k.clone(), v).is_some() {
                return Err(crate::Chyba::PoskodeneData(format!("vlastnosť {k} sa opakuje")));
            }
        }
        c.vynor();
        Ok(Vlastnosti(Arc::new(mapa)))
    }
}
