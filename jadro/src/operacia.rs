//! Operácia – čistá, deterministická funkcia z údajov na údaje s parametrami (prvotný pojem 4).
//!
//! Každá operácia sa opisuje rovnako – vstavané aj rozšírenia od kohokoľvek (zásada A5). Popis hovorí,
//! aké vstupy, výstupy a parametre má; vyhodnocovač podľa neho overuje typy (S1), parametre aj výsledky.

use crate::chyba::{Chyba, Vysledok};
use crate::hodnota::{Hodnota, TypHodnoty};
use crate::jednotka::Jednotka;
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::Meno;
use crate::oblast::Oblast;
use crate::prostredie::Prostredie;
use crate::signal::Signal;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

/// Vstup operácie alebo grafu.
#[derive(Debug, Clone, PartialEq)]
pub struct PopisPortu {
    pub meno: Meno,
    pub typ: TypHodnoty,
}

impl PopisPortu {
    pub fn novy(meno: Meno, typ: TypHodnoty) -> Self {
        PopisPortu { meno, typ }
    }
}

/// Typ výstupu: pevný, alebo rovnaký ako typ niektorého vstupu.
#[derive(Debug, Clone, PartialEq)]
pub enum TypVystupu {
    Pevny(TypHodnoty),
    AkoVstup(u32),
}

/// Výstup operácie.
#[derive(Debug, Clone, PartialEq)]
pub struct PopisVystupu {
    pub meno: Meno,
    pub typ: TypVystupu,
}

/// Parameter operácie alebo grafu.
#[derive(Debug, Clone, PartialEq)]
pub struct PopisParametra {
    pub meno: Meno,
    pub typ: TypHodnoty,
    pub predvolena: Hodnota,
    /// Najmenšia dovolená číselná hodnota.
    pub od: Option<f64>,
    /// Najväčšia dovolená číselná hodnota.
    pub po: Option<f64>,
    pub jednotka: Option<Jednotka>,
    pub popis: String,
}

impl PopisParametra {
    /// Číselný parameter s predvolenou hodnotou.
    pub fn cislo(meno: Meno, predvolena: f64) -> Self {
        PopisParametra {
            meno,
            typ: TypHodnoty::Cislo,
            predvolena: Hodnota::Cislo(predvolena),
            od: None,
            po: None,
            jednotka: None,
            popis: String::new(),
        }
    }

    pub fn s_rozsahom(mut self, od: f64, po: f64) -> Self {
        self.od = Some(od);
        self.po = Some(po);
        self
    }

    /// Overí hodnotu (typ a rozsah).
    pub fn over(&self, h: &Hodnota) -> Vysledok<()> {
        if !self.typ.splna(h) {
            return Err(Chyba::NezhodaTypov {
                popis: format!(
                    "parameter {} čaká {}, dostal {}",
                    self.meno,
                    self.typ.popis(),
                    h.typ().popis()
                ),
                navrh: None,
            });
        }
        if let Some(c) = h.ako_cislo() {
            if c.is_nan() {
                return Err(Chyba::MimoRozsahu(format!("parameter {} nie je číslo (NaN)", self.meno)));
            }
            if self.od.is_some_and(|od| c < od) || self.po.is_some_and(|po| c > po) {
                return Err(Chyba::MimoRozsahu(format!(
                    "parameter {} = {c} je mimo {}…{}",
                    self.meno,
                    self.od.map_or("−∞".into(), |x| x.to_string()),
                    self.po.map_or("∞".into(), |x| x.to_string())
                )));
            }
        }
        Ok(())
    }
}

/// Úplný popis operácie.
#[derive(Debug, Clone, PartialEq)]
pub struct PopisOperacie {
    /// Celosvetovo jedinečná identita (odporúča sa tvar „autor.oblasť.meno“).
    pub identita: Meno,
    /// Verzia správania – zmena výsledku znamená novú verziu (staré diela sa prepočítajú rovnako).
    pub verzia: u32,
    pub popis: String,
    pub vstupy: Vec<PopisPortu>,
    pub vystupy: Vec<PopisVystupu>,
    pub parametre: Vec<PopisParametra>,
    /// Závisí výsledok od polohy vyhodnotenia (napr. od času)? Ak nie, výsledok sa smie zdieľať.
    pub zavisi_od_polohy: bool,
}

/// Argumenty výpočtu – vstupy a parametre v poradí popisu.
#[derive(Debug)]
pub struct Argumenty<'a> {
    pub vstupy: &'a [Hodnota],
    pub parametre: &'a [Hodnota],
    pub popis: &'a PopisOperacie,
}

impl<'a> Argumenty<'a> {
    pub fn vstup(&self, i: usize) -> Vysledok<&'a Hodnota> {
        self.vstupy
            .get(i)
            .ok_or_else(|| Chyba::Chybajuce(format!("vstup {i} operácie {}", self.popis.identita)))
    }

    pub fn signal(&self, i: usize) -> Vysledok<&'a Signal> {
        let h = self.vstup(i)?;
        h.ako_signal().ok_or_else(|| Chyba::NezhodaTypov {
            popis: format!(
                "vstup {i} operácie {} má byť signál, je {}",
                self.popis.identita,
                h.typ().popis()
            ),
            navrh: None,
        })
    }

    pub fn parameter(&self, meno: &str) -> Vysledok<&'a Hodnota> {
        let i =
            self.popis.parametre.iter().position(|p| p.meno.ako_text() == meno).ok_or_else(|| {
                Chyba::Chybajuce(format!("parameter {meno} operácie {}", self.popis.identita))
            })?;
        self.parametre.get(i).ok_or_else(|| Chyba::Chybajuce(format!("hodnota parametra {meno}")))
    }

    pub fn cislo(&self, meno: &str) -> Vysledok<f64> {
        self.parameter(meno)?.ako_cislo().ok_or_else(|| Chyba::NezhodaTypov {
            popis: format!("parameter {meno} má byť číslo"),
            navrh: None,
        })
    }

    pub fn text(&self, meno: &str) -> Vysledok<&'a str> {
        self.parameter(meno)?.ako_text().ok_or_else(|| Chyba::NezhodaTypov {
            popis: format!("parameter {meno} má byť text"),
            navrh: None,
        })
    }
}

/// Kontext výpočtu: prostredie, polohy vyhodnotenia (napr. t = 2,5 s) a hĺbka vnorenia.
#[derive(Debug, Clone, Copy)]
pub struct KontextVypoctu<'a> {
    pub prostredie: &'a Prostredie,
    pub polohy: &'a BTreeMap<Meno, f64>,
    pub hlbka: u32,
}

/// Operácia. Implementácia musí byť deterministická: rovnaké vstupy a parametre → rovnaký výsledok.
pub trait Operacia: Send + Sync + Debug {
    fn popis(&self) -> &PopisOperacie;
    fn vypocitaj(&self, kontext: &KontextVypoctu<'_>, argumenty: &Argumenty<'_>) -> Vysledok<Vec<Hodnota>>;

    /// Oblasť vstupu `vstup`, ktorú operácia potrebuje na výpočet oblasti `vystup` svojho výsledku
    /// (`parametre` v poradí popisu). `None` = celý vstup – bezpečná predvoľba pre každú operáciu.
    ///
    /// Operácia, ktorá oblasť vráti, sľubuje: (1) zachováva osi – výsledok pokrýva na každej osi rozsah,
    /// ktorý dostal prvý vstup s touto osou; (2) hodnoty v žiadanej oblasti výsledku sú bitovo rovnaké ako
    /// pri výpočte z celých vstupov. Bodové operácie vracajú tú istú oblasť, filtre ju rozšíria o okraj.
    fn oblast_vstupu(&self, parametre: &[Hodnota], vystup: &Oblast, vstup: usize) -> Option<Oblast> {
        let _ = (parametre, vystup, vstup);
        None
    }
}

/// Register operácií. Vstavané operácie sa registrujú rovnako ako rozšírenia.
#[derive(Debug, Default, Clone)]
pub struct RegisterOperacii {
    operacie: BTreeMap<(Meno, u32), Arc<dyn Operacia>>,
    prevodniky: Vec<(Meno, u32)>,
}

impl RegisterOperacii {
    pub fn prazdny() -> Self {
        Self::default()
    }

    /// Zaregistruje operáciu (identita s verziou musí byť nová).
    pub fn zaregistruj(&mut self, op: Arc<dyn Operacia>) -> Vysledok<()> {
        let p = op.popis();
        let kluc = (p.identita.clone(), p.verzia);
        if self.operacie.contains_key(&kluc) {
            return Err(Chyba::MimoRozsahu(format!("operácia {} verzie {} už existuje", kluc.0, kluc.1)));
        }
        if p.vystupy.iter().any(|v| matches!(v.typ, TypVystupu::AkoVstup(i) if i as usize >= p.vstupy.len()))
        {
            return Err(Chyba::MimoRozsahu(format!(
                "operácia {}: výstup odkazuje na neexistujúci vstup",
                kluc.0
            )));
        }
        self.operacie.insert(kluc, op);
        Ok(())
    }

    /// Zaregistruje prevodník – operáciu s jedným vstupom a jedným výstupom, ktorú možno navrhnúť pri
    /// nezhode typov (pravidlo S1).
    pub fn zaregistruj_prevodnik(&mut self, op: Arc<dyn Operacia>) -> Vysledok<()> {
        let p = op.popis();
        if p.vstupy.len() != 1 || p.vystupy.len() != 1 {
            return Err(Chyba::MimoRozsahu("prevodník musí mať jeden vstup a jeden výstup".into()));
        }
        let kluc = (p.identita.clone(), p.verzia);
        self.zaregistruj(op)?;
        self.prevodniky.push(kluc);
        Ok(())
    }

    pub fn najdi(&self, identita: &Meno, verzia: u32) -> Option<&Arc<dyn Operacia>> {
        self.operacie.get(&(identita.clone(), verzia))
    }

    /// Najnovšia verzia operácie.
    pub fn najnovsia(&self, identita: &str) -> Option<&Arc<dyn Operacia>> {
        self.operacie.iter().rev().find(|((m, _), _)| m.ako_text() == identita).map(|(_, o)| o)
    }

    /// Nájde prevodník z typu `z` na typ `na`.
    pub fn najdi_prevodnik(&self, z: &TypHodnoty, na: &TypHodnoty) -> Option<&PopisOperacie> {
        self.prevodniky.iter().filter_map(|k| self.operacie.get(k)).map(|o| o.popis()).find(|p| {
            p.vstupy[0].typ.prijme(z)
                && match &p.vystupy[0].typ {
                    TypVystupu::Pevny(t) => na.prijme(t),
                    TypVystupu::AkoVstup(_) => na.prijme(&crate::graf::bez_typu_prvku(z.clone())),
                }
        })
    }

    pub fn vsetky(&self) -> impl Iterator<Item = &Arc<dyn Operacia>> {
        self.operacie.values()
    }
}

impl Kodovatelne for PopisPortu {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.meno(&self.meno);
        self.typ.zakoduj(z);
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        Ok(PopisPortu { meno: c.meno()?, typ: TypHodnoty::dekoduj(c)? })
    }
}

fn zakoduj_volitelne_cislo(z: &mut Zapisovac, v: Option<f64>) {
    match v {
        Some(x) => z.u8(1).f64(x),
        None => z.u8(0),
    };
}

fn dekoduj_volitelne_cislo(c: &mut Citac<'_>) -> Vysledok<Option<f64>> {
    match c.u8()? {
        0 => Ok(None),
        1 => Ok(Some(c.f64()?)),
        _ => Err(Chyba::PoskodeneData("voliteľné číslo".into())),
    }
}

impl Kodovatelne for PopisParametra {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.meno(&self.meno);
        self.typ.zakoduj(z);
        self.predvolena.zakoduj(z);
        zakoduj_volitelne_cislo(z, self.od);
        zakoduj_volitelne_cislo(z, self.po);
        match &self.jednotka {
            Some(j) => {
                z.u8(1);
                j.zakoduj(z);
            }
            None => {
                z.u8(0);
            }
        }
        z.text(&self.popis);
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        Ok(PopisParametra {
            meno: c.meno()?,
            typ: TypHodnoty::dekoduj(c)?,
            predvolena: Hodnota::dekoduj(c)?,
            od: dekoduj_volitelne_cislo(c)?,
            po: dekoduj_volitelne_cislo(c)?,
            jednotka: match c.u8()? {
                0 => None,
                1 => Some(Jednotka::dekoduj(c)?),
                _ => return Err(Chyba::PoskodeneData("jednotka parametra".into())),
            },
            popis: c.text()?.to_string(),
        })
    }
}
