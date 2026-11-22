//! # KLÍČ – prvý program systému SKLENÍK
//!
//! Beží ako PID 1 na Raspberry Pi: pripojí systémové súborové systémy, zistí model, spíše periférie a zapne
//! sieť, spustí tepelného strážcu, ukáže stav na obrazovke a sériovej linke a stará sa o procesy. Celý systém beží z pamäte,
//! karta sa po štarte takmer nepoužíva. Na iných systémoch KLÍČ nič nerobí.

#![deny(unsafe_code)]

// na iných systémoch sa strážca len skúša (simuláciou), riadi ho až Linux
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod strazca;

// stavová obrazovka (rozloženie a formát bodov) – skúša sa na každom systéme
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod obrazovka;

// súpis periférií číta len súbory jadra – skúša sa na každom systéme na vymyslenom strome /sys
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod periferie;

// strážca pravdy – test jadier so známym výsledkom (R100)
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod test_jadier;

// skúška pamäte RAM pri štarte (R120)
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod test_pamate;

// PÔDA – profil hardvéru zo súborov jadra; skúša sa na každom systéme na vymyslenom strome
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod poda;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod sys;

/// Verzia SKLENÍKA (dátum a odtlačok zostavenia z CI) – na obrazovke, aby bolo vidieť, ktorá karta beží.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const VERZIA: &str = match option_env!("SKLENIK_VERZIA") {
    Some(v) => v,
    None => "vývojová",
};

#[cfg(target_os = "linux")]
fn main() {
    linux::spusti();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("KLÍČ je prvý program systému SKLENÍK a beží len na Linuxe.");
}
