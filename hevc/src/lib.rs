//! **Rozbor syntaxe HEVC (H.265)** – krok H0 prúdu „HEVC na Raspberry Pi“ (R214).
//!
//! Spoločný základ pre všetky cesty dekódovania HEVC v ORCHIDEI a SKLENÍKU:
//! - hardvérové dekodéry *stateless* (RPi 4, 400, 5 – `rpi-hevc-dec`): program musí rozobrať parametrické
//!   sady a hlavičky rezov a odovzdať ich jadru systému,
//! - hardvérové dekodéry *stateful* (X96, Odroid C4): rozbor slúži na kontrolu toku a skúšobné toky,
//! - vlastný softvérový dekodér pre RPi 3 (samostatný voliteľný modul).
//!
//! Obsah: čítač bitov s Exp-Golomb ([`bity`]), jednotky NAL a tok Annex B ([`nal`]), parametrické sady
//! VPS, SPS, PPS vrátane matíc kvantovania, sád referencií, VUI a rozšírení rozsahu ([`parametre`]), správy SEI
//! s odtlačkom dekódovaného obrazu ([`sei`]). Každá hodnota sa kontroluje voči hraniciam normy (R211).
//!
//! Je to len rozbor syntaxe (čítanie hodnôt z toku), nie dekódovanie obrazu.

pub mod bity;
pub mod nal;
pub mod parametre;
pub mod sei;

pub use nal::{HlavickaNal, jednotky_annex_b, rbsp};
pub use parametre::{Pps, Sps, Vps};

#[cfg(test)]
mod testy;
