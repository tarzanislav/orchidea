//! Matica a tenzor ako Signál.
//!
//! Navonok je tenzor Neurónu **Signál s pomenovanými osami** – ten istý prvotný pojem ako obraz či zvuk.
//! Vnútri sa počíta s hustou maticou po riadkoch. Prevod medzi nimi ide podľa mien osí, nie podľa poradia:
//! násobenie sa spojí cez os s rovnakým menom (napr. „vlastnosť“), nech je v signáli kdekoľvek.

use crate::jadra::{self, Rezim};
use orchidea_jadro::{Chyba, Meno, Os, Signal, TypPrvku, Vysledok};

/// Hustá matica f32 po riadkoch.
#[derive(Debug, Clone, PartialEq)]
pub struct Matica {
    pub riadky: usize,
    pub stlpce: usize,
    pub data: Vec<f32>,
}

impl Matica {
    pub fn nova(riadky: usize, stlpce: usize, data: Vec<f32>) -> Matica {
        assert_eq!(data.len(), riadky * stlpce, "dáta matice nesedia s rozmermi");
        Matica { riadky, stlpce, data }
    }

    pub fn nuly(riadky: usize, stlpce: usize) -> Matica {
        Matica { riadky, stlpce, data: vec![0.0; riadky * stlpce] }
    }

    pub fn hodnota(&self, r: usize, s: usize) -> f32 {
        self.data[r * self.stlpce + s]
    }

    pub fn riadok(&self, r: usize) -> &[f32] {
        &self.data[r * self.stlpce..(r + 1) * self.stlpce]
    }

    /// Súčin self · b.
    pub fn krat(&self, rezim: Rezim, b: &Matica) -> Matica {
        assert_eq!(self.stlpce, b.riadky, "matice nemožno násobiť");
        let mut c = Matica::nuly(self.riadky, b.stlpce);
        jadra::nasob(rezim, self.riadky, b.stlpce, self.stlpce, &self.data, &b.data, &mut c.data);
        c
    }

    pub fn transponovana(&self) -> Matica {
        Matica {
            riadky: self.stlpce,
            stlpce: self.riadky,
            data: jadra::transponuj(self.riadky, self.stlpce, &self.data),
        }
    }

    /// Signál s osami [os_riadkov, os_stĺpcov].
    pub fn do_signalu(&self, os_riadkov: &str, os_stlpcov: &str) -> Vysledok<Signal> {
        Signal::novy(
            vec![Os::poradova(os_riadkov, self.riadky)?, Os::poradova(os_stlpcov, self.stlpce)?],
            self.data.clone(),
        )
    }

    /// Matica zo signálu s dvoma osami – poradie osí v signáli nehrá rolu.
    pub fn zo_signalu(s: &Signal, os_riadkov: &str, os_stlpcov: &str) -> Vysledok<Matica> {
        if s.osi().len() != 2 {
            return Err(Chyba::NezhodaTvaru(format!("matica potrebuje 2 osi, signál má {}", s.osi().len())));
        }
        let poradie = [Meno::nove(os_riadkov)?, Meno::nove(os_stlpcov)?];
        let s = s.preusporiadaj(&poradie)?;
        let data = if s.typ() == TypPrvku::F32 { s.ako_f32() } else { s.preved(TypPrvku::F32).0.ako_f32() };
        Ok(Matica { riadky: s.osi()[0].dlzka, stlpce: s.osi()[1].dlzka, data })
    }
}

/// Násobenie dvoch signálov cez spoločnú pomenovanú os (zúženie podľa mena).
///
/// `a` má osi {r, spoločná}, `b` má osi {spoločná, s} v ľubovoľnom poradí; výsledok má osi [r, s].
pub fn nasob_signaly(rezim: Rezim, a: &Signal, b: &Signal, spolocna: &str) -> Vysledok<Signal> {
    let ina = |s: &Signal| -> Vysledok<String> {
        if s.osi().len() != 2 || s.os(spolocna).is_none() {
            return Err(Chyba::NezhodaTvaru(format!("signál musí mať 2 osi a jednou z nich {spolocna}")));
        }
        Ok(s.osi()
            .iter()
            .find(|o| o.meno.ako_text() != spolocna)
            .map(|o| o.meno.to_string())
            .unwrap_or_default())
    };
    let (ra, sb) = (ina(a)?, ina(b)?);
    if ra == sb {
        return Err(Chyba::NezhodaTvaru(format!("výsledok by mal dve osi s menom {ra}")));
    }
    let ma = Matica::zo_signalu(a, &ra, spolocna)?;
    let mb = Matica::zo_signalu(b, spolocna, &sb)?;
    if ma.stlpce != mb.riadky {
        return Err(Chyba::NezhodaTvaru(format!("os {spolocna} má dĺžky {} a {}", ma.stlpce, mb.riadky)));
    }
    ma.krat(rezim, &mb).do_signalu(&ra, &sb)
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::m;

    #[test]
    fn zuzenie_podla_mena_osi() {
        // a: [dávka=2, vlastnosť=3]; b uložené ako [výstup=2, vlastnosť=3] – opačné poradie osí
        let a =
            Matica::nova(2, 3, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).do_signalu("dávka", "vlastnosť").unwrap();
        let b =
            Matica::nova(2, 3, vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0]).do_signalu("výstup", "vlastnosť").unwrap();
        let c = nasob_signaly(Rezim::Presny, &a, &b, "vlastnosť").unwrap();
        assert_eq!(c.osi()[0].meno, m("dávka"));
        assert_eq!(c.osi()[1].meno, m("výstup"));
        assert_eq!(c.ako_f32(), vec![4.0, 2.0, 10.0, 5.0]);
        assert!(nasob_signaly(Rezim::Presny, &a, &b, "čas").is_err());
    }
}
