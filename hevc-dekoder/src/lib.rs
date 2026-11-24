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
//! - [`filtre`], [`odblokovanie`], [`sao`] – filtre v slučke (8.7).

pub mod cabac;
pub mod dekoder;
pub mod filtre;
pub mod geometria;
pub mod interpolacia;
pub mod intra;
pub mod obraz;
pub mod odblokovanie;
pub mod pohyb;
pub mod sao;
pub mod syntax;
pub mod transformacia;

pub use dekoder::{Dekoder, Statistika};
pub use obraz::{Obraz, ObrazPasov, Rovina, RovinaPasov, Vzorka, Zdroj};
pub use syntax::DekodovanyObraz;
