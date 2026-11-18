//! Jednotný typ chyby jadra. Jadro nikdy nepadá na zlých vstupoch – vráti chybu so zrozumiteľným popisom.

use std::fmt;

/// Chyba jadra ORCHIDEE.
#[derive(Debug, Clone, PartialEq)]
pub enum Chyba {
    /// Neplatné meno (prázdne, príliš dlhé, s riadiacimi znakmi).
    NeplatneMeno(String),
    /// Typy alebo osi nesedia (pravidlo S1). Obsahuje popis a prípadný návrh prevodníka.
    NezhodaTypov { popis: String, navrh: Option<String> },
    /// Tvar signálu nesedí s dátami alebo s požiadavkou.
    NezhodaTvaru(String),
    /// Hodnota je mimo dovoleného rozsahu.
    MimoRozsahu(String),
    /// Niečo potrebné chýba (uzol, operácia, parameter, os, vstup…).
    Chybajuce(String),
    /// Dáta sú poškodené alebo neúplné (kódovanie, denník, súbor).
    PoskodeneData(String),
    /// Prekročený bezpečnostný limit (veľkosť, hĺbka vnorenia, počet prvkov).
    Limit(String),
    /// Graf obsahuje cyklus.
    Cyklus(String),
    /// Chyba výpočtu operácie.
    Vypocet(String),
    /// Chyba vstupu a výstupu (súbory).
    VstupVystup(String),
}

impl fmt::Display for Chyba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Chyba::NeplatneMeno(s) => write!(f, "neplatné meno: {s}"),
            Chyba::NezhodaTypov { popis, navrh: Some(n) } => write!(f, "typy nesedia: {popis} (návrh: {n})"),
            Chyba::NezhodaTypov { popis, navrh: None } => write!(f, "typy nesedia: {popis}"),
            Chyba::NezhodaTvaru(s) => write!(f, "tvar nesedí: {s}"),
            Chyba::MimoRozsahu(s) => write!(f, "mimo rozsahu: {s}"),
            Chyba::Chybajuce(s) => write!(f, "chýba: {s}"),
            Chyba::PoskodeneData(s) => write!(f, "poškodené dáta: {s}"),
            Chyba::Limit(s) => write!(f, "prekročený limit: {s}"),
            Chyba::Cyklus(s) => write!(f, "cyklus v grafe: {s}"),
            Chyba::Vypocet(s) => write!(f, "chyba výpočtu: {s}"),
            Chyba::VstupVystup(s) => write!(f, "vstup a výstup: {s}"),
        }
    }
}

impl std::error::Error for Chyba {}

impl From<std::io::Error> for Chyba {
    fn from(e: std::io::Error) -> Self {
        Chyba::VstupVystup(e.to_string())
    }
}

/// Skrátený výsledok jadra.
pub type Vysledok<T> = Result<T, Chyba>;
