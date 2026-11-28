//! Téma vzhľadu (Okvetie) a dizajnový systém ORCHIDEE – spoločný pre prehliadač a editor fotografií (Pohľad),
//! prehrávač médií LÝRA a obrazovku TV krabičky.
//!
//! - [`tema`] – témy vzhľadu ako výmenné sady farieb podľa úlohy (tmavá predvolená, svetlé „nie biele“,
//!   polotmavá, vysoký kontrast, čierna pre OLED),
//! - [`kontrast`] – kontrastný pomer podľa WCAG 2.2, meraný kódom pre každý pár text/pozadie každej témy,
//! - [`miery`] – mriežka 4 bodov, rozostupy, zaoblenia, výšky, typografická stupnica písma STONKA, čitateľnosť
//!   obrazovky na diaľku,
//! - [`ikony`] – vlastné vektorové ikony na jednej mriežke s jednou hrúbkou ťahu,
//! - [`kreslic`] – kreslenie s vyhladením v mierke 100 % aj 200 % (HiDPI),
//! - [`prvky`] – ovládacie prvky a ich stavy (bežný, pod myšou, stlačený, zameraný, vypnutý).
//!
//! Všetko vlastným kódom bez cudzích knižníc. Nová farba alebo rozmer patrí sem, nie do programu – jedna zmena
//! potom platí rovnako v celej suite.

#![deny(unsafe_code)]

pub mod ikony;
pub mod kontrast;
pub mod kreslic;
pub mod miery;
pub mod prvky;
pub mod tema;

pub use ikony::Ikona;
pub use kreslic::Kreslic;
pub use orchidea_pismo::Farba;
pub use prvky::Stav;
pub use tema::{TEMY, Tema};
