//! # orchidea-pohlad – Pohľad (prehliadač obrázkov), vrstva 3
//!
//! Používa len verejné rozhrania balíkov `formaty`, `farba` a `jadro` (R30).
//!
//! - `zmensenie` – Lanczos-3 v lineárnom svetle, s alfou,
//! - `davka` – hromadný prevod, premenovanie a zmenšenie (jedno pracovné vlákno, obmedzený počet vlákien,
//!   jeden obraz v pamäti, zrušenie, udalosti pre rozhranie),
//! - `porovnanie` – miery rozdielu (najväčší, priemerný, PSNR, SSIM), obraz rozdielu a delený pohľad,
//! - `okno_win` – okno pre Windows (vlastné väzby na user32/gdi32/shell32), program `pohlad`,
//! - `prehliadac` – jadro okna (nezávislé od systému): chladné – počíta len po vstupe alebo hotovej práci,
//! - `upravy` – otočenie, zrkadlenie, orezanie, jas, kontrast, gama, zmenšenie; história; uloženie (JPEG bezstratovo),
//! - `herbar` – základ katalógu fotiek: index priečinkov bez importu, metadáta, rýchle vyhľadávanie,
//! - `nahlady` – zoznam obrázkov priečinka bez importu a náhľady (sRGB, natočené podľa Exif) na pozadí.

// `unsafe` len vo väzbách na systém okien (okno_win); jadro a všetko ostatné je bez neho
#![deny(unsafe_code)]

pub mod davka;
pub mod herbar;
pub mod nahlady;
#[cfg(windows)]
pub mod okno_win;
pub mod panel;
pub mod porovnanie;
pub mod prehliadac;
pub mod upravy;
pub mod zmensenie;
