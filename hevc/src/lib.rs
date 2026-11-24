//! **Rozbor syntaxe HEVC (H.265)** – krok H0 prúdu „HEVC na Raspberry Pi“ (R214).
//!
//! Spoločný základ pre všetky cesty dekódovania HEVC v ORCHIDEI a SKLENÍKU:
//! - hardvérové dekodéry *stateless* (RPi 4, 400, 5 – `rpi-hevc-dec`): program musí rozobrať parametrické
//!   sady a hlavičky rezov a odovzdať ich jadru systému,
//! - hardvérové dekodéry *stateful* (X96, Odroid C4): rozbor slúži na kontrolu toku a skúšobné toky,
//! - vlastný softvérový dekodér pre RPi 3 (samostatný voliteľný modul).
//!
//! Obsah: čítač bitov s Exp-Golomb ([`bity`]), jednotky NAL a tok Annex B ([`nal`]), parametrické sady
//! VPS, SPS, PPS vrátane matíc kvantovania, sád referencií, VUI a rozšírení rozsahu ([`parametre`], [`sady`]),
//! správy SEI s odtlačkom dekódovaného obrazu ([`sei`]), hlavička rezu ([`rez`], krok H1), správa obrazov
//! dekodéra – POC, sady referencií, zoznamy, poradie výstupu ([`obrazy`], H1) a parametre pre hardvérové
//! dekodéry V4L2 *stateless* ([`v4l2`], H1, rozhranie R1). Každá hodnota sa kontroluje voči hraniciam normy (R211).
//!
//! Je to rozbor syntaxe a správa obrazov, nie dekódovanie vzoriek (to robí voliteľný balík `hevc-dekoder`
//! alebo hardvér).

pub mod bity;
pub mod nal;
pub mod obrazy;
pub mod parametre;
pub mod rez;
pub mod sady;
pub mod sei;
pub mod v4l2;

pub use nal::{HlavickaNal, jednotky_annex_b, rbsp};
pub use obrazy::{SpravaObrazov, Udalost};
pub use parametre::{Pps, Sps, Vps};
pub use rez::{HlavickaRezu, TypRezu};
pub use sady::Sady;
pub use v4l2::{ParametreDekodovania, PripravaV4l2};

#[cfg(test)]
mod testy;
