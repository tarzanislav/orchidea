//! Hodnota – čokoľvek, čo tečie grafom: číslo, text, signál, štruktúra, zoznam. A jej typ (pravidlo S1).

use crate::chyba::{Chyba, Vysledok};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::Meno;
use crate::odtlacok::{Hasovac, Odtlacok};
use crate::signal::{Signal, TypPrvku};
use crate::struktura::Struktura;
use std::fmt;
use std::sync::Arc;

/// Hodnota v grafe.
#[derive(Clone, PartialEq)]
pub enum Hodnota {
    /// Žiadna hodnota.
    Nic,
    Logicka(bool),
    Cislo(f64),
    Text(Arc<str>),
    Signal(Signal),
    Struktura(Arc<Struktura>),
    Zoznam(Arc<[Hodnota]>),
}

impl fmt::Debug for Hodnota {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Hodnota::Nic => f.write_str("Nic"),
            Hodnota::Logicka(b) => write!(f, "{b}"),
            Hodnota::Cislo(c) => write!(f, "{c}"),
            Hodnota::Text(t) => write!(f, "{t:?}"),
            Hodnota::Signal(s) => write!(f, "{s:?}"),
            Hodnota::Struktura(s) => write!(f, "Struktura({})", s.druh),
            Hodnota::Zoznam(z) => f.debug_list().entries(z.iter()).finish(),
        }
    }
}

impl Hodnota {
    pub fn text(t: &str) -> Hodnota {
        Hodnota::Text(Arc::from(t))
    }

    pub fn ako_cislo(&self) -> Option<f64> {
        match self {
            Hodnota::Cislo(c) => Some(*c),
            Hodnota::Logicka(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn ako_signal(&self) -> Option<&Signal> {
        if let Hodnota::Signal(s) = self { Some(s) } else { None }
    }

    pub fn ako_text(&self) -> Option<&str> {
        if let Hodnota::Text(t) = self { Some(t) } else { None }
    }

    pub fn ako_logicka(&self) -> Option<bool> {
        if let Hodnota::Logicka(b) = self { Some(*b) } else { None }
    }

    /// Konkrétny typ hodnoty.
    pub fn typ(&self) -> TypHodnoty {
        match self {
            Hodnota::Nic => TypHodnoty::Nic,
            Hodnota::Logicka(_) => TypHodnoty::Logicka,
            Hodnota::Cislo(_) => TypHodnoty::Cislo,
            Hodnota::Text(_) => TypHodnoty::Text,
            Hodnota::Signal(s) => TypHodnoty::Signal(PoziadavkaSignalu {
                osi: s.osi().iter().map(|o| o.meno.clone()).collect(),
                typ: Some(s.typ()),
            }),
            Hodnota::Struktura(_) => TypHodnoty::Struktura,
            Hodnota::Zoznam(z) => {
                TypHodnoty::Zoznam(Box::new(z.first().map(|h| h.typ()).unwrap_or(TypHodnoty::Lubovolny)))
            }
        }
    }

    /// Odtlačok obsahu (pre signál sa počíta len raz).
    pub fn odtlacok(&self) -> Odtlacok {
        match self {
            Hodnota::Signal(s) => {
                let mut h = Hasovac::novy();
                h.pridaj(&[4]).pridaj(&s.odtlacok().0);
                h.dokonci()
            }
            Hodnota::Zoznam(z) => {
                let mut h = Hasovac::novy();
                h.pridaj(&[6]).pridaj(&(z.len() as u64).to_le_bytes());
                for p in z.iter() {
                    h.pridaj(&p.odtlacok().0);
                }
                h.dokonci()
            }
            ine => Odtlacok::z(&ine.do_bajtov()),
        }
    }
}

/// Čo musí spĺňať signál na vstupe: osi, ktoré musí mať, a prípadne typ hodnôt.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PoziadavkaSignalu {
    /// Osi, ktoré signál musí mať (môže mať aj ďalšie).
    pub osi: Vec<Meno>,
    /// Typ hodnôt (None = ľubovoľný).
    pub typ: Option<TypPrvku>,
}

/// Typ hodnoty (pravidlo S1: spojiť sa dá to, čo typovo sedí).
#[derive(Debug, Clone, PartialEq)]
pub enum TypHodnoty {
    Lubovolny,
    Nic,
    Logicka,
    Cislo,
    Text,
    Signal(PoziadavkaSignalu),
    Struktura,
    Zoznam(Box<TypHodnoty>),
}

impl TypHodnoty {
    /// Ľubovoľný signál.
    pub fn signal() -> TypHodnoty {
        TypHodnoty::Signal(PoziadavkaSignalu::default())
    }

    /// Signál s danými osami.
    pub fn signal_s_osami(osi: &[&str]) -> Vysledok<TypHodnoty> {
        let osi = osi.iter().map(|t| Meno::nove(t)).collect::<Vysledok<Vec<_>>>()?;
        Ok(TypHodnoty::Signal(PoziadavkaSignalu { osi, typ: None }))
    }

    /// Prijme vstup tohto typu hodnotu typu `zdroj`?
    pub fn prijme(&self, zdroj: &TypHodnoty) -> bool {
        match (self, zdroj) {
            (TypHodnoty::Lubovolny, _) | (_, TypHodnoty::Lubovolny) => true,
            (TypHodnoty::Cislo, TypHodnoty::Logicka) => true,
            (TypHodnoty::Signal(p), TypHodnoty::Signal(z)) => {
                p.osi.iter().all(|o| z.osi.contains(o))
                    && (p.typ.is_none() || z.typ.is_none() || p.typ == z.typ)
            }
            (TypHodnoty::Zoznam(a), TypHodnoty::Zoznam(b)) => a.prijme(b),
            (a, b) => a == b,
        }
    }

    /// Spĺňa konkrétna hodnota tento typ?
    pub fn splna(&self, h: &Hodnota) -> bool {
        match (self, h) {
            (TypHodnoty::Zoznam(t), Hodnota::Zoznam(z)) => z.iter().all(|p| t.splna(p)),
            _ => self.prijme(&h.typ()),
        }
    }

    /// Čitateľný popis typu.
    pub fn popis(&self) -> String {
        match self {
            TypHodnoty::Lubovolny => "čokoľvek".into(),
            TypHodnoty::Nic => "nič".into(),
            TypHodnoty::Logicka => "áno/nie".into(),
            TypHodnoty::Cislo => "číslo".into(),
            TypHodnoty::Text => "text".into(),
            TypHodnoty::Signal(p) => {
                let osi: Vec<&str> = p.osi.iter().map(|o| o.ako_text()).collect();
                let typ = p.typ.map(|t| format!(", {}", t.meno())).unwrap_or_default();
                format!("signál [{}{}]", osi.join(", "), typ)
            }
            TypHodnoty::Struktura => "štruktúra".into(),
            TypHodnoty::Zoznam(t) => format!("zoznam ({})", t.popis()),
        }
    }
}

impl Kodovatelne for TypHodnoty {
    fn zakoduj(&self, z: &mut Zapisovac) {
        match self {
            TypHodnoty::Lubovolny => {
                z.u8(0);
            }
            TypHodnoty::Nic => {
                z.u8(1);
            }
            TypHodnoty::Logicka => {
                z.u8(2);
            }
            TypHodnoty::Cislo => {
                z.u8(3);
            }
            TypHodnoty::Text => {
                z.u8(4);
            }
            TypHodnoty::Signal(p) => {
                z.u8(5).cislo(p.osi.len() as u64);
                for o in &p.osi {
                    z.meno(o);
                }
                match p.typ {
                    Some(t) => z.u8(1).text(t.meno()),
                    None => z.u8(0),
                };
            }
            TypHodnoty::Struktura => {
                z.u8(6);
            }
            TypHodnoty::Zoznam(t) => {
                z.u8(7);
                t.zakoduj(z);
            }
        }
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        c.vnor()?;
        let t = match c.u8()? {
            0 => TypHodnoty::Lubovolny,
            1 => TypHodnoty::Nic,
            2 => TypHodnoty::Logicka,
            3 => TypHodnoty::Cislo,
            4 => TypHodnoty::Text,
            5 => {
                let n = c.pocet(2)?;
                let mut osi = Vec::with_capacity(n);
                for _ in 0..n {
                    osi.push(c.meno()?);
                }
                let typ = match c.u8()? {
                    0 => None,
                    1 => Some(
                        TypPrvku::z_mena(c.text()?)
                            .ok_or_else(|| Chyba::PoskodeneData("typ prvku".into()))?,
                    ),
                    _ => return Err(Chyba::PoskodeneData("typ prvku".into())),
                };
                TypHodnoty::Signal(PoziadavkaSignalu { osi, typ })
            }
            6 => TypHodnoty::Struktura,
            7 => TypHodnoty::Zoznam(Box::new(TypHodnoty::dekoduj(c)?)),
            d => return Err(Chyba::PoskodeneData(format!("typ hodnoty {d}"))),
        };
        c.vynor();
        Ok(t)
    }
}

impl Kodovatelne for Hodnota {
    fn zakoduj(&self, z: &mut Zapisovac) {
        match self {
            Hodnota::Nic => {
                z.u8(0);
            }
            Hodnota::Logicka(b) => {
                z.u8(1).logicka(*b);
            }
            Hodnota::Cislo(c) => {
                z.u8(2).f64(*c);
            }
            Hodnota::Text(t) => {
                z.u8(3).text(t);
            }
            Hodnota::Signal(s) => {
                z.u8(4);
                s.zakoduj(z);
            }
            Hodnota::Struktura(s) => {
                z.u8(5);
                s.zakoduj(z);
            }
            Hodnota::Zoznam(l) => {
                z.u8(6).cislo(l.len() as u64);
                for h in l.iter() {
                    h.zakoduj(z);
                }
            }
        }
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        c.vnor()?;
        let h = match c.u8()? {
            0 => Hodnota::Nic,
            1 => Hodnota::Logicka(c.logicka()?),
            2 => Hodnota::Cislo(c.f64()?),
            3 => Hodnota::Text(Arc::from(c.text()?)),
            4 => Hodnota::Signal(Signal::dekoduj(c)?),
            5 => Hodnota::Struktura(Arc::new(Struktura::dekoduj(c)?)),
            6 => {
                let n = c.pocet(1)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(Hodnota::dekoduj(c)?);
                }
                Hodnota::Zoznam(v.into())
            }
            d => return Err(Chyba::PoskodeneData(format!("druh hodnoty {d}"))),
        };
        c.vynor();
        Ok(h)
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::meno::m;
    use crate::os::Os;

    #[test]
    fn typy_sa_skladaju() {
        let obraz = Signal::novy(
            vec![Os::poradova("y", 1).unwrap(), Os::poradova("x", 2).unwrap()],
            vec![0.5f32, 1.0],
        )
        .unwrap();
        let h = Hodnota::Signal(obraz);
        assert!(TypHodnoty::signal_s_osami(&["x"]).unwrap().splna(&h));
        assert!(TypHodnoty::signal_s_osami(&["x", "y"]).unwrap().splna(&h));
        assert!(!TypHodnoty::signal_s_osami(&["t"]).unwrap().splna(&h));
        assert!(TypHodnoty::Cislo.prijme(&TypHodnoty::Logicka));
        assert!(!TypHodnoty::Text.prijme(&TypHodnoty::Cislo));
        let zoznam = Hodnota::Zoznam(vec![h.clone(), Hodnota::Cislo(1.0)].into());
        let b = zoznam.do_bajtov();
        assert_eq!(Hodnota::z_bajtov(&b).unwrap(), zoznam);
        assert_eq!(Hodnota::z_bajtov(&b).unwrap().odtlacok(), zoznam.odtlacok());
        assert_ne!(Hodnota::Cislo(1.0).odtlacok(), Hodnota::Cislo(2.0).odtlacok());
        let t = TypHodnoty::Zoznam(Box::new(TypHodnoty::Signal(PoziadavkaSignalu {
            osi: vec![m("x")],
            typ: Some(TypPrvku::F32),
        })));
        assert_eq!(TypHodnoty::z_bajtov(&t.do_bajtov()).unwrap(), t);
    }

    #[test]
    fn hlboke_vnorenie_neprejde() {
        let mut b = [6u8, 1].repeat(200);
        b.push(0);
        assert!(matches!(Hodnota::z_bajtov(&b), Err(Chyba::Limit(_))));
    }
}
