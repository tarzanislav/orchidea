//! Dekodér pre vzorky s 9–10 bitmi (Main 10, HDR filmy) – tie isté moduly so vzorkou `u16`. Skúšky
//! modulov bežia v [`crate::b8`] (rovnaký zdrojový text); 10 bitov overujú skúšobné toky normy.

// zdrojový text píše u16::from(vzorka) kvôli u8; pri u16 je to zbytočný, ale správny prevod
#![allow(clippy::useless_conversion, clippy::unnecessary_cast)]

/// Vzorka tohto dekodéra.
pub type Vzorka = u16;

#[path = "dekoder.rs"]
pub mod dekoder;
#[path = "interpolacia.rs"]
pub mod interpolacia;
#[path = "intra.rs"]
pub mod intra;
#[path = "obraz.rs"]
pub mod obraz;
#[path = "odblokovanie.rs"]
pub mod odblokovanie;
#[path = "sao.rs"]
pub mod sao;
#[path = "simd_skalarne.rs"]
mod simd;
#[path = "syntax.rs"]
pub mod syntax;
#[path = "transformacia.rs"]
pub mod transformacia;
