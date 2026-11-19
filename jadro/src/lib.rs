//! # Jadro ORCHIDEE
//!
//! Gramatika dvanástich prvotných pojmov a siedmich pravidiel skladania:
//!
//! | Pojem | Modul |
//! |---|---|
//! | 1 Signál | [`signal`], [`os`], [`jednotka`] |
//! | 2 Štruktúra | [`struktura`] |
//! | 3 Vlastnosť | [`vlastnost`] |
//! | 4 Operácia | [`operacia`], [`zakladne`] |
//! | 5 Parameter | [`graf::Parameter`], [`krivka`], [`vyraz`] |
//! | 6 Graf | [`graf`], [`prostredie`], [`oblast`] |
//! | 7 Odkaz | [`graf::DruhUzla`] |
//! | 8 Výber | [`vyber`] |
//! | 9 Zmena | [`zmena`], [`dennik`] |
//! | 10 Výstup | [`vystup`] |
//! | 11 Pohľad, 12 Nástroj | [`pohlad`] |
//!
//! Jadro je **mechanizmus bez politiky** (zásady A1 a A2): nepozná účel, odbor ani postup. Nemá žiadne
//! cudzie závislosti a neobsahuje nebezpečný kód. Výpočty sú deterministické na každej platforme.

#![forbid(unsafe_code)]

pub mod chyba;
pub mod dennik;
pub mod graf;
pub mod hodnota;
pub mod jednotka;
pub mod kodovanie;
pub mod krivka;
pub mod matematika;
pub mod meno;
pub mod nahoda;
pub mod oblast;
pub mod odtlacok;
pub mod operacia;
pub mod os;
pub mod paralel;
pub mod pohlad;
pub mod prostredie;
pub mod signal;
pub mod struktura;
pub mod vlastnost;
pub mod vyber;
pub mod vyraz;
pub mod vystup;
pub mod zakladne;
pub mod zmena;

pub use chyba::{Chyba, Vysledok};
pub use graf::{DruhUzla, Graf, Parameter, Uzol, UzolId, Zdroj};
pub use hodnota::{Hodnota, PoziadavkaSignalu, TypHodnoty};
pub use jednotka::{Jednotka, Zlomok};
pub use kodovanie::Kodovatelne;
pub use meno::{Meno, m};
pub use oblast::Oblast;
pub use odtlacok::{Hasovac, Odtlacok};
pub use operacia::{Operacia, PopisOperacie, RegisterOperacii};
pub use os::{Os, RegisterOsi, Vzorkovanie};
pub use prostredie::Prostredie;
pub use signal::{Signal, TypPrvku};
pub use struktura::Struktura;
pub use vlastnost::Vlastnosti;
pub use vyber::Vyber;
pub use zmena::{Cas, Dielo, Hodiny, Uprava, Zmena};
