//! # KLÍČ – prvý program systému SKLENÍK
//!
//! Beží ako PID 1 na Raspberry Pi: pripojí systémové súborové systémy, zistí model, spustí tepelného
//! strážcu, ukáže stav na obrazovke a sériovej linke a stará sa o procesy. Celý systém beží z pamäte,
//! karta sa po štarte takmer nepoužíva. Na iných systémoch KLÍČ nič nerobí.

#![deny(unsafe_code)]

// na iných systémoch sa strážca len skúša (simuláciou), riadi ho až Linux
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod strazca;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod sys;

#[cfg(target_os = "linux")]
fn main() {
    linux::spusti();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("KLÍČ je prvý program systému SKLENÍK a beží len na Linuxe.");
}
