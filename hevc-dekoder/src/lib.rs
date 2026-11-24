//! **Softvérový dekodér HEVC (H.265) Main 8-bit** – kroky H2–H6 prúdu „HEVC na Raspberry Pi“ (R214).
//!
//! **Samostatný voliteľný modul – výnimka vlastníka R214.** HEVC je patentovaný formát; tento balík je oddelený
//! od jadra aj ostatných schopností ORCHIDEE, nič od neho nezávisí a pred zverejnením balíka s ním pre iných
//! ľudí ho vlastník ešte raz posúdi (`ORCHIDEA/hevc-na-rpi.md`, časť 6). Hardvérová cesta (RPi 4/5, X96, Odroid)
//! ho nepotrebuje.
//!
//! Cieľ: Raspberry Pi 3 a Zero 2 W (bez hardvérového dekodéra HEVC) prehrá HEVC samo. Každý krok končí
//! **bitovou zhodou** so skúšobnými tokmi normy (odtlačok MD5 každého obrazu) – rýchlosť sa nekupuje stratou
//! presnosti (zákon nulovej straty).
//!
//! Moduly:
//! - [`obraz`] – roviny vzoriek,
//! - [`intra`] – vnútrosnímková predikcia (8.4.4.2),
//! - [`transformacia`] – spätné kvantovanie a transformácie (8.6),
//! - [`interpolacia`] – interpolácia vzoriek a vážená predikcia (8.5.3.3),
//! - [`filtre`], [`odblokovanie`], [`sao`] – filtre v slučke (8.7).

pub mod filtre;
pub mod interpolacia;
pub mod intra;
pub mod obraz;
pub mod odblokovanie;
pub mod sao;
pub mod transformacia;

pub use obraz::{Obraz, Rovina, Vzorka};
