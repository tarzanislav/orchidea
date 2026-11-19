//! Oblasť – časť signálu na pomenovaných osiach (zákon FOTOSYNTÉZY „nepočítaj, čo nikto nevidí“).
//!
//! Oblasť určuje pre niektoré osi rozsah indexov `od..po` v súradniciach celého signálu. Osi, ktoré
//! v oblasti nie sú, sa berú celé. Vyhodnotenie s oblasťou ([`crate::prostredie::Prostredie::vyhodnot_oblast`])
//! spočíta len to, čo je potrebné pre žiadanú oblasť výstupu – napríklad výrez obrazu, ktorý je práve
//! vidieť, alebo úsek zvuku, ktorý sa práve prehráva. Výsledok je bitovo rovnaký ako výrez z úplného
//! výpočtu.

use crate::meno::Meno;
use crate::signal::Signal;
use crate::{Chyba, Vysledok};
use std::collections::BTreeMap;

/// Oblasť: os → rozsah indexov `od..po` v súradniciach celého signálu.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Oblast {
    osi: BTreeMap<Meno, (usize, usize)>,
}

impl Oblast {
    /// Prázdna oblasť – všetky osi celé.
    pub fn cela() -> Oblast {
        Oblast::default()
    }

    /// Pridá (alebo nahradí) rozsah osi.
    pub fn s_osou(mut self, os: Meno, od: usize, po: usize) -> Vysledok<Oblast> {
        if od > po {
            return Err(Chyba::MimoRozsahu(format!("oblasť osi {os}: {od}..{po}")));
        }
        self.osi.insert(os, (od, po));
        Ok(self)
    }

    /// Rozsah osi, ak je obmedzená.
    pub fn os(&self, os: &str) -> Option<(usize, usize)> {
        self.osi.get(os).copied()
    }

    /// Obmedzené osi a ich rozsahy.
    pub fn osi(&self) -> impl Iterator<Item = (&Meno, (usize, usize))> {
        self.osi.iter().map(|(m, r)| (m, *r))
    }

    /// Počet obmedzených osí.
    pub fn je_cela(&self) -> bool {
        self.osi.is_empty()
    }

    /// Rozšíri rozsah osi o okraj na oboch stranách (napr. polomer filtra). Začiatok sa zastaví na nule,
    /// koniec sa obmedzí až pri výreze podľa skutočnej dĺžky.
    pub fn rozsir(mut self, os: &str, okraj: usize) -> Oblast {
        if let Some(r) = self.osi.get_mut(os) {
            *r = (r.0.saturating_sub(okraj), r.1.saturating_add(okraj));
        }
        self
    }

    /// Najmenšia oblasť, ktorá obsahuje obe: na spoločných osiach obal rozsahov, os obmedzená len v jednej
    /// z oblastí sa berie celá.
    pub fn zjednot(&self, ina: &Oblast) -> Oblast {
        let osi = self
            .osi
            .iter()
            .filter_map(|(m, a)| ina.osi.get(m).map(|b| (m.clone(), (a.0.min(b.0), a.1.max(b.1)))))
            .collect();
        Oblast { osi }
    }

    /// Vyreže z hodnôt signálu, ktorý pokrýva oblasť `pokrytie` (`None` = celý signál), túto oblasť.
    /// Rozsah sa obmedzí na dostupné dáta (napr. pri okraji obrazu). Vráti výrez a oblasť, ktorú skutočne
    /// pokrýva. Osi, ktoré signál nemá, sa preskočia.
    pub fn vyrez(&self, signal: &Signal, pokrytie: Option<&Oblast>) -> Vysledok<(Signal, Oblast)> {
        let mut s = signal.clone();
        let mut skutocna = Oblast::cela();
        for (os, &(od, po)) in &self.osi {
            let Some((_, o)) = s.os(os.ako_text()) else { continue };
            let posun = pokrytie.and_then(|p| p.os(os.ako_text())).map_or(0, |r| r.0);
            let koniec = posun + o.dlzka;
            let po = po.clamp(posun, koniec);
            let od = od.clamp(posun, po);
            if od != posun || po != koniec {
                s = s.vyrez(os.ako_text(), od - posun, po - posun)?;
            }
            skutocna.osi.insert(os.clone(), (od, po));
        }
        // osi pokrytia, ktoré táto oblasť neobmedzuje, zostávajú tak, ako ich signál pokrýva
        if let Some(p) = pokrytie {
            for (os, r) in &p.osi {
                if !skutocna.osi.contains_key(os) && s.os(os.ako_text()).is_some() {
                    skutocna.osi.insert(os.clone(), *r);
                }
            }
        }
        Ok((s, skutocna))
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::meno::m;
    use crate::os::Os;

    #[test]
    fn vyrez_a_zjednotenie() {
        let s =
            Signal::novy(vec![Os::poradova("x", 10).unwrap()], (0..10).map(|i| i as f32).collect()).unwrap();
        let a = Oblast::cela().s_osou(m("x"), 2, 5).unwrap();
        let (v, p) = a.vyrez(&s, None).unwrap();
        assert_eq!(v.ako_f64(), vec![2.0, 3.0, 4.0]);
        assert_eq!(p.os("x"), Some((2, 5)));
        // výrez z výrezu v súradniciach celku
        let b = Oblast::cela().s_osou(m("x"), 3, 4).unwrap();
        let (w, _) = b.vyrez(&v, Some(&p)).unwrap();
        assert_eq!(w.ako_f64(), vec![3.0]);
        // obmedzenie na dostupné dáta
        let (z, q) = a.clone().rozsir("x", 4).vyrez(&s, None).unwrap();
        assert_eq!(q.os("x"), Some((0, 9)));
        assert_eq!(z.pocet(), 9);
        let (_, q) = Oblast::cela().s_osou(m("x"), 8, 20).unwrap().vyrez(&s, None).unwrap();
        assert_eq!(q.os("x"), Some((8, 10)));
        // zjednotenie: obal na spoločných osiach, os len v jednej oblasti je celá
        let c = Oblast::cela().s_osou(m("x"), 7, 9).unwrap().s_osou(m("y"), 0, 1).unwrap();
        let u = a.zjednot(&c);
        assert_eq!(u.os("x"), Some((2, 9)));
        assert_eq!(u.os("y"), None);
    }
}
