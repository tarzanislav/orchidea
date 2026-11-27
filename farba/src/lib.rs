//! # orchidea-farba – farba vlastným kódom (vrstva 2)
//!
//! - **prenosové funkcie** (`prenos`): sRGB, BT.709, gama, PQ (ST 2084), HLG (BT.2100), ProPhoto, ACEScc, ACEScct
//!   a krivky ICC (gama, tabuľka, parametrické typy 0–4) – kódovanie ↔ lineárne svetlo;
//! - **základné farby a adaptácia** (`primarne`): sRGB/BT.709, BT.2020, DCI-P3, Display P3, Adobe RGB, ProPhoto,
//!   ACES AP0/AP1; matice RGB ↔ XYZ zo chromatickostí; Bradford, CAT02, von Kries;
//! - **profily ICC v2 a v4** (`icc`): čítanie (matica + TRC, sivá TRC, lut8, lut16, lutAtoB, lutBtoA so štvorstenovou
//!   interpoláciou), PCS XYZ aj Lab, zápis profilu v4 s maticou a krivkami;
//! - **prevod** (`prevod`): ľubovoľný priestor → ľubovoľný cez XYZ D50, rýchle cesty pre 8/16 bitov, paralelne;
//!   voliteľne s kompenzáciou čierneho bodu (ISO 18619, postup Adobe);
//! - **ACES 1.3** (`aces`): AP0 ↔ AP1, RRT a ODT pre sRGB a BT.709 (100 cd/m², šero).
//!
//! Žiadne cudzie knižnice; matematika z `orchidea-jadro` (deterministická na všetkých systémoch).
//! Nezávislí rozhodcovia (len v CI): colour-science, Little CMS (cez Pillow), OpenColorIO.

#![forbid(unsafe_code)]

pub mod aces;
pub mod icc;
pub mod matica;
pub mod prenos;
pub mod prevod;
pub mod primarne;

pub use icc::{Profil, Zamer};
pub use prenos::{Krivka, Prenos};
pub use prevod::{Prevod, Priestor, Volby};
pub use primarne::{Adaptacia, Primarne};
