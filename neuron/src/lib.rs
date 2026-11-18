//! # Neurón – výpočtový motor MYKORÍZY (etapa MY0)
//!
//! - **Tenzor je Signál** s pomenovanými osami – ten istý prvotný pojem jadra ako obraz a zvuk
//!   ([`matica`]); násobenie sa spája cez os s rovnakým menom.
//! - **Jadrá** v presnom režime (bitovo rovnaké všade) a rýchlom režime (AVX2 + FMA, NEON) ([`jadra`]).
//! - **Kvantizácia**: 8-bitové aktivácie a trojhodnotové váhy −1, 0, +1 bez násobenia ([`kvant`]).
//! - **Učenie**: páska so spätným šírením, vrstvy Linear, BitLinear, RMSNorm, GELU, ReLU a AdamW
//!   ([`pasky`]).
//!
//! Žiadne cudzie knižnice. Nebezpečný kód je len vo vektorových jadrách pre konkrétny procesor.

#![deny(unsafe_code)]

pub mod jadra;
pub mod kvant;
pub mod matica;
pub mod pasky;

pub use jadra::Rezim;
pub use matica::{Matica, nasob_signaly};
pub use pasky::{AdamW, ParamId, Parametre, Paska, Premenna};
