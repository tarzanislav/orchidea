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
//! Použitie: [`Dekoder::nal`] pre každú jednotku NAL toku (bez štartovacieho kódu), [`Dekoder::koniec`] na konci;
//! obidve vracajú obrazy v poradí zobrazenia. [`Dekoder::statistika`] hovorí, koľko obrazov sa zhodovalo
//! s odtlačkom MD5 v toku.
//!
//! Moduly:
//! - [`dekoder`] – jednotky NAL, správa obrazov (spoločná s hardvérovou cestou), filtre, odtlačky MD5,
//! - [`syntax`] – dáta rezu: CTU, SAO, strom kódovania, predikčné a transformačné jednotky, rezíduá, PCM,
//! - [`cabac`] – aritmetické dekódovanie a kontexty (9.3), [`geometria`] – dlaždice a poradie z (6.4, 6.5),
//! - [`pohyb`] – predikcia pohybu: zlučovanie, AMVP, časový kandidát (8.5.3.2),
//! - [`obraz`] – roviny vzoriek,
//! - [`intra`] – vnútrosnímková predikcia (8.4.4.2),
//! - [`transformacia`] – spätné kvantovanie a transformácie (8.6),
//! - [`interpolacia`] – interpolácia vzoriek a vážená predikcia (8.5.3.3),
//! - [`filtre`], [`odblokovanie`], [`sao`] – filtre v slučke (8.7),
//! - `neon` (len aarch64) – ručné NEON jadier interpolácie; jediné miesto s `unsafe`.
//!
//! **8 a 10 bitov:** moduly so vzorkami (obraz, predikcia, transformácie, filtre, syntax, dekóder obrazu) sa
//! prekladajú dvakrát – [`b8`] so vzorkou `u8` (profil Main) a [`b16`] so vzorkou `u16` (Main 10, aj rôzna hĺbka
//! jasu a farby). [`Dekoder`] podľa SPS použije ten, ktorý tok potrebuje; 8-bitová cesta nič nestráca.

#![deny(unsafe_code)]
// zámerne: moduly so vzorkami sa prekladajú dvakrát (b8 so vzorkou u8, b16 so vzorkou u16)
#![allow(clippy::duplicate_mod)]

pub mod cabac;
pub mod etapy;
pub mod filtre;
pub mod geometria;
pub mod pohyb;
mod prepinac;
pub mod statistika;
pub mod vzorka;

#[cfg(not(test))]
pub mod b16;
pub mod b8;

pub use b8::obraz::{Obraz, ObrazPasov, Rovina, RovinaPasov, Vzorka, Zdroj};
pub use prepinac::{Dekoder, Vystup};
pub use statistika::Statistika;
