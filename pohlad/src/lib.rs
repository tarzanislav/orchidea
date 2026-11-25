//! # orchidea-pohlad – Pohľad (prehliadač obrázkov), vrstva 3
//!
//! Používa len verejné rozhrania balíkov `formaty`, `farba` a `jadro` (R30).
//!
//! - `zmensenie` – Lanczos-3 v lineárnom svetle, s alfou,
//! - `davka` – hromadný prevod, premenovanie a zmenšenie (jedno pracovné vlákno, obmedzený počet vlákien,
//!   jeden obraz v pamäti, zrušenie, udalosti pre rozhranie).

#![forbid(unsafe_code)]

pub mod davka;
pub mod zmensenie;
