//! Pohľad (prvotný pojem 11) a Nástroj (prvotný pojem 12) – rozhrania pre vyššie vrstvy.
//!
//! Jadro nekreslí ani nevie, ako vyzerá okno. Určuje len pravidlá: pohľad zobrazuje graf (ten istý graf
//! môže mať ľubovoľný počet pohľadov) a nástroj premieňa vstup človeka na **zmeny a výbery** – nič iné
//! (pravidlo S6), takže história, obnova, synchronizácia a pôvod platia pre každý nástroj rovnako.

use crate::hodnota::Hodnota;
use crate::meno::Meno;
use crate::vyber::Vyber;
use crate::zmena::{Dielo, Uprava};
use std::collections::BTreeMap;
use std::fmt::Debug;

/// Udalosť zo vstupného zariadenia (myš, pero, dotyk, klávesy, MIDI, OSC, hlas…) opísaná neutrálne.
#[derive(Debug, Clone, PartialEq)]
pub struct Udalost {
    /// Druh udalosti, napr. „dotyk.začiatok“, „kláves“, „midi.nota“.
    pub druh: Meno,
    /// Pomenované hodnoty (poloha, tlak, sklon, rýchlosť, výška tónu…).
    pub hodnoty: BTreeMap<Meno, f64>,
    /// Čas udalosti v mikrosekundách (monotónny).
    pub cas_us: u64,
}

/// Kontext, v ktorom nástroj pracuje: dielo, graf a uzol, nad ktorým je pohľad, a aktuálny výber.
#[derive(Debug, Clone, Copy)]
pub struct KontextNastroja<'a> {
    pub dielo: &'a Dielo,
    pub graf: &'a Meno,
    pub vyber: &'a Vyber,
    /// Prevod polohy v pohľade na polohy osí (napr. obrazovka → x, y alebo t, f).
    pub polohy: &'a BTreeMap<Meno, f64>,
}

/// Čo nástroj vráti: úpravy (stanú sa jednou zmenou v denníku) a prípadne nový výber.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VysledokNastroja {
    pub upravy: Vec<Uprava>,
    pub vyber: Option<Vyber>,
    /// Dočasný náhľad (nie je súčasťou diela, kým nástroj nevráti úpravy).
    pub nahlad: Option<Hodnota>,
}

/// Nástroj – štetec je štetec všade, kde dáva zmysel.
pub trait Nastroj: Send + Debug {
    fn meno(&self) -> &Meno;
    /// Osi, nad ktorými nástroj vie pracovať (prázdne = ľubovoľné).
    fn osi(&self) -> Vec<Meno>;
    fn spracuj(&mut self, udalost: &Udalost, kontext: &KontextNastroja<'_>) -> VysledokNastroja;
}

/// Pohľad – zobrazenie a ovládanie grafu pre človeka.
pub trait Pohlad: Send + Debug {
    fn meno(&self) -> &Meno;
    /// Osi, ktoré pohľad zobrazuje (napr. x a y, alebo t a f).
    fn osi(&self) -> Vec<Meno>;
    /// Dokáže pohľad zobraziť hodnotu?
    fn zobrazi(&self, hodnota: &Hodnota) -> bool;
}
