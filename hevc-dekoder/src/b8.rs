//! Dekodér pre vzorky s 8 bitmi (profil Main) – moduly so vzorkami preložené so vzorkou `u8`.

/// Vzorka tohto dekodéra.
pub type Vzorka = u8;

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
#[cfg(target_arch = "aarch64")]
#[path = "neon.rs"]
mod simd;
#[cfg(not(target_arch = "aarch64"))]
#[path = "simd_skalarne.rs"]
mod simd;
#[path = "syntax.rs"]
pub mod syntax;
#[path = "transformacia.rs"]
pub mod transformacia;
