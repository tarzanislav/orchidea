//! Pomenované osi signálu a otvorený register osí (zásada A3).
//!
//! Os má meno, počet vzoriek a vzorkovanie: pravidelné (začiatok, krok a jednotka – presne ako zlomky),
//! označené (napr. kanály s menami) alebo len poradové. Register hovorí, akú veličinu os nesie; dopĺňať ho
//! môže ktokoľvek bez zmeny jadra.

use crate::chyba::{Chyba, Vysledok};
use crate::jednotka::{Jednotka, Zlomok};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::{Meno, m};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Ako sú rozmiestnené vzorky pozdĺž osi.
#[derive(Debug, Clone, PartialEq)]
pub enum Vzorkovanie {
    /// Len poradie vzoriek (0, 1, 2, …) bez fyzikálneho významu.
    Poradove,
    /// Pravidelné vzorky: poloha(i) = začiatok + i · krok v danej jednotke.
    Pravidelne { zaciatok: Zlomok, krok: Zlomok, jednotka: Jednotka },
    /// Každá vzorka má meno (napr. kanály „R“, „G“, „B“ alebo „L“, „R“, „C“, „LFE“).
    Oznacene(Arc<[Meno]>),
}

/// Os signálu.
#[derive(Debug, Clone, PartialEq)]
pub struct Os {
    pub meno: Meno,
    pub dlzka: usize,
    pub vzorkovanie: Vzorkovanie,
}

impl Os {
    /// Os len s poradím vzoriek.
    pub fn poradova(meno: &str, dlzka: usize) -> Vysledok<Os> {
        Ok(Os { meno: Meno::nove(meno)?, dlzka, vzorkovanie: Vzorkovanie::Poradove })
    }

    /// Pravidelne vzorkovaná os.
    pub fn pravidelna(
        meno: &str,
        dlzka: usize,
        zaciatok: Zlomok,
        krok: Zlomok,
        jednotka: Jednotka,
    ) -> Vysledok<Os> {
        if krok == Zlomok::NULA {
            return Err(Chyba::MimoRozsahu("krok osi nesmie byť 0".into()));
        }
        Ok(Os {
            meno: Meno::nove(meno)?,
            dlzka,
            vzorkovanie: Vzorkovanie::Pravidelne { zaciatok, krok, jednotka },
        })
    }

    /// Časová os so vzorkovacou frekvenciou v Hz (krok = 1/frekvencia s, presne).
    pub fn casova(dlzka: usize, frekvencia: u64) -> Vysledok<Os> {
        Self::pravidelna("t", dlzka, Zlomok::NULA, Zlomok::novy(1, frekvencia)?, Jednotka::sekunda())
    }

    /// Os s menovitými vzorkami (dĺžka = počet mien, mená musia byť jedinečné).
    pub fn oznacena(meno: &str, mena: &[&str]) -> Vysledok<Os> {
        let mut v = Vec::with_capacity(mena.len());
        for t in mena {
            let n = Meno::nove(t)?;
            if v.contains(&n) {
                return Err(Chyba::MimoRozsahu(format!("meno vzorky {t} sa opakuje")));
            }
            v.push(n);
        }
        Ok(Os { meno: Meno::nove(meno)?, dlzka: v.len(), vzorkovanie: Vzorkovanie::Oznacene(v.into()) })
    }

    /// Presná poloha vzorky i (len pre pravidelné vzorkovanie).
    pub fn presna_poloha(&self, i: usize) -> Option<Zlomok> {
        match &self.vzorkovanie {
            Vzorkovanie::Pravidelne { zaciatok, krok, .. } => {
                krok.krat_cele(i64::try_from(i).ok()?)?.plus(zaciatok)
            }
            _ => None,
        }
    }

    /// Poloha vzorky i v jednotkách osi (poradové a označené osi vracajú index).
    pub fn poloha(&self, i: usize) -> f64 {
        match &self.vzorkovanie {
            Vzorkovanie::Pravidelne { zaciatok, krok, .. } => zaciatok.ako_f64() + i as f64 * krok.ako_f64(),
            _ => i as f64,
        }
    }

    /// Spojitý index pre polohu (opak funkcie `poloha`).
    pub fn index_polohy(&self, poloha: f64) -> f64 {
        match &self.vzorkovanie {
            Vzorkovanie::Pravidelne { zaciatok, krok, .. } => (poloha - zaciatok.ako_f64()) / krok.ako_f64(),
            _ => poloha,
        }
    }

    /// Index vzorky podľa mena (len označené osi).
    pub fn index_mena(&self, meno: &str) -> Option<usize> {
        match &self.vzorkovanie {
            Vzorkovanie::Oznacene(mena) => mena.iter().position(|x| x.ako_text() == meno),
            _ => None,
        }
    }

    pub fn jednotka(&self) -> Option<&Jednotka> {
        match &self.vzorkovanie {
            Vzorkovanie::Pravidelne { jednotka, .. } => Some(jednotka),
            _ => None,
        }
    }

    /// Mená vzoriek (len označené osi).
    pub fn mena_vzoriek(&self) -> Option<&[Meno]> {
        match &self.vzorkovanie {
            Vzorkovanie::Oznacene(m) => Some(m),
            _ => None,
        }
    }

    /// Rovnaká os s inou dĺžkou (výrez, zmena rozlíšenia).
    pub fn s_dlzkou(&self, dlzka: usize) -> Os {
        let mut o = self.clone();
        o.dlzka = dlzka;
        o
    }
}

impl Kodovatelne for Os {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.meno(&self.meno).cislo(self.dlzka as u64);
        match &self.vzorkovanie {
            Vzorkovanie::Poradove => {
                z.u8(0);
            }
            Vzorkovanie::Pravidelne { zaciatok, krok, jednotka } => {
                z.u8(1);
                zaciatok.zakoduj(z);
                krok.zakoduj(z);
                jednotka.zakoduj(z);
            }
            Vzorkovanie::Oznacene(mena) => {
                z.u8(2);
                for n in mena.iter() {
                    z.meno(n);
                }
            }
        }
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        let meno = c.meno()?;
        let dlzka = usize::try_from(c.cislo()?).map_err(|_| Chyba::Limit("dĺžka osi".into()))?;
        let vzorkovanie = match c.u8()? {
            0 => Vzorkovanie::Poradove,
            1 => {
                let zaciatok = Zlomok::dekoduj(c)?;
                let krok = Zlomok::dekoduj(c)?;
                if krok == Zlomok::NULA {
                    return Err(Chyba::PoskodeneData("krok osi je 0".into()));
                }
                Vzorkovanie::Pravidelne { zaciatok, krok, jednotka: Jednotka::dekoduj(c)? }
            }
            2 => {
                if dlzka > c.zostava() {
                    return Err(Chyba::PoskodeneData("príliš veľa mien vzoriek".into()));
                }
                let mut v = Vec::with_capacity(dlzka);
                for _ in 0..dlzka {
                    v.push(c.meno()?);
                }
                Vzorkovanie::Oznacene(v.into())
            }
            d => return Err(Chyba::PoskodeneData(format!("neznáme vzorkovanie {d}"))),
        };
        Ok(Os { meno, dlzka, vzorkovanie })
    }
}

/// Druh veličiny, ktorú os nesie.
#[derive(Debug, Clone, PartialEq)]
pub enum DruhOsi {
    /// Spojitá veličina s daným rozmerom (jednotka určuje rozmer, nie konkrétnu mierku).
    Spojita(Jednotka),
    /// Konečná množina pomenovaných prvkov (napr. kanály).
    Diskretna,
}

/// Záznam v registri osí.
#[derive(Debug, Clone, PartialEq)]
pub struct PopisOsi {
    pub meno: Meno,
    pub druh: DruhOsi,
    pub popis: String,
}

/// Otvorený register osí. Vstavané záznamy nemajú výsady – každý záznam sa dá doplniť rovnako.
#[derive(Debug, Clone)]
pub struct RegisterOsi {
    osi: BTreeMap<Meno, PopisOsi>,
}

impl Default for RegisterOsi {
    fn default() -> Self {
        Self::zakladny()
    }
}

impl RegisterOsi {
    /// Prázdny register.
    pub fn prazdny() -> Self {
        RegisterOsi { osi: BTreeMap::new() }
    }

    /// Register so základnými osami (priestor, čas, frekvencia, vlnová dĺžka, uhly, kanál).
    pub fn zakladny() -> Self {
        let mut r = Self::prazdny();
        let dlzka = Jednotka::meter();
        let uhol = Jednotka::radian();
        for (meno, druh, popis) in [
            ("x", DruhOsi::Spojita(dlzka.clone()), "vodorovná poloha"),
            ("y", DruhOsi::Spojita(dlzka.clone()), "zvislá poloha"),
            ("z", DruhOsi::Spojita(dlzka.clone()), "hĺbková poloha"),
            ("t", DruhOsi::Spojita(Jednotka::sekunda()), "čas"),
            ("f", DruhOsi::Spojita(Jednotka::hertz()), "frekvencia"),
            ("λ", DruhOsi::Spojita(dlzka), "vlnová dĺžka"),
            ("zemepisná dĺžka", DruhOsi::Spojita(uhol.clone()), "uhol okolo zvislej osi"),
            ("zemepisná šírka", DruhOsi::Spojita(uhol), "uhol od rovníka"),
            ("kanál", DruhOsi::Diskretna, "zložky jednej vzorky (farebné, zvukové, ľubovoľné)"),
        ] {
            r.osi.insert(m(meno), PopisOsi { meno: m(meno), druh, popis: popis.into() });
        }
        r
    }

    /// Doplní alebo nahradí záznam.
    pub fn zaregistruj(&mut self, popis: PopisOsi) {
        self.osi.insert(popis.meno.clone(), popis);
    }

    pub fn popis(&self, meno: &Meno) -> Option<&PopisOsi> {
        self.osi.get(meno)
    }

    pub fn vsetky(&self) -> impl Iterator<Item = &PopisOsi> {
        self.osi.values()
    }

    /// Overí os voči registru. Neregistrované osi sú dovolené (otvorený svet), registrované musia sedieť.
    pub fn over(&self, os: &Os) -> Vysledok<()> {
        let Some(p) = self.osi.get(&os.meno) else { return Ok(()) };
        match (&p.druh, &os.vzorkovanie) {
            (_, Vzorkovanie::Poradove) => Ok(()),
            (DruhOsi::Spojita(rozmer), Vzorkovanie::Pravidelne { jednotka, .. }) => {
                if jednotka.je_zlucitelna(rozmer) {
                    Ok(())
                } else {
                    Err(Chyba::NezhodaTypov {
                        popis: format!(
                            "os {} má jednotku {jednotka}, register čaká rozmer {rozmer}",
                            os.meno
                        ),
                        navrh: None,
                    })
                }
            }
            (DruhOsi::Diskretna, Vzorkovanie::Oznacene(_)) => Ok(()),
            (DruhOsi::Diskretna, Vzorkovanie::Pravidelne { .. }) => Err(Chyba::NezhodaTypov {
                popis: format!("os {} je v registri diskrétna, nemôže mať pravidelné vzorkovanie", os.meno),
                navrh: None,
            }),
            (DruhOsi::Spojita(_), Vzorkovanie::Oznacene(_)) => Err(Chyba::NezhodaTypov {
                popis: format!("os {} je v registri spojitá, nemôže mať pomenované vzorky", os.meno),
                navrh: None,
            }),
        }
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn casova_os_je_presna() {
        let t = Os::casova(48_000 * 3600, 48_000).unwrap();
        let hodina = t.presna_poloha(48_000 * 3600).unwrap();
        assert_eq!(hodina, Zlomok::cele(3600));
        let ntsc =
            Os::pravidelna("t", 0, Zlomok::NULA, Zlomok::novy(1001, 30000).unwrap(), Jednotka::sekunda())
                .unwrap();
        assert_eq!(ntsc.presna_poloha(30000).unwrap(), Zlomok::cele(1001));
    }

    #[test]
    fn register_overuje() {
        let r = RegisterOsi::zakladny();
        assert!(r.over(&Os::casova(10, 44_100).unwrap()).is_ok());
        let zla = Os::pravidelna("t", 4, Zlomok::NULA, Zlomok::JEDNA, Jednotka::meter()).unwrap();
        assert!(r.over(&zla).is_err());
        assert!(r.over(&Os::oznacena("kanál", &["R", "G", "B"]).unwrap()).is_ok());
        assert!(r.over(&Os::poradova("ľubovoľná", 3).unwrap()).is_ok());
        assert!(Os::oznacena("kanál", &["L", "L"]).is_err());
        let nm = Os::pravidelna("λ", 31, Zlomok::cele(400), Zlomok::cele(10), Jednotka::nanometer()).unwrap();
        assert!(r.over(&nm).is_ok());
        let b = nm.do_bajtov();
        assert_eq!(Os::z_bajtov(&b).unwrap(), nm);
    }
}
