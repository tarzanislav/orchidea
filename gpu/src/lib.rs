//! # GPU vrstva ORCHIDEE
//!
//! - [`ir`] – vlastný jazyk výpočtových kernelov,
//! - [`interpret`] – referenčný interpret na procesore (CPU cesta vždy, rozhodca správnosti),
//! - [`spirv`] – vlastný prekladač do SPIR-V (Vulkan), [`texty`] – do HLSL, MSL a WGSL,
//! - [`zariadenie`] – spoločné rozhranie; [`vulkan`], [`d3d12`] a [`metal`] – zariadenia cez vlastné väzby.
//!
//! Žiadne cudzie knižnice. Nebezpečný kód je len vo väzbách na rozhrania operačného systému a ovládačov.

#![deny(unsafe_code)]

pub mod d3d12;
pub mod interpret;
pub mod ir;
pub mod kernely;
mod kniznica;
pub mod metal;
pub mod spirv;
pub mod texty;
pub mod vulkan;
pub mod zariadenie;

pub use ir::Kernel;
pub use zariadenie::{DataVazby, Hodnota32, Procesor, Zariadenie, vsetky_zariadenia};
