//! # KLÍČ – prvý program systému SKLENÍK
//!
//! Beží ako PID 1 na Raspberry Pi: pripojí systémové súborové systémy, zistí model, spíše periférie a zapne
//! sieť, spustí tepelného strážcu, ukáže stav na obrazovke a sériovej linke a stará sa o procesy. Celý systém beží z pamäte,
//! karta sa po štarte takmer nepoužíva – len samomeranie (R122) na ňu štyrikrát zapíše výsledky skúšky.
//! Na iných systémoch KLÍČ nič nerobí.

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

// samomeranie (R122) – mená súborov, poradie, krivka teploty, identifikácia; skúša sa na každom systéme
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod samomeranie;

// správca chladu TV krabičiek (R158–R161) – obraz, USB, sieť, tepelný pokus; skúša sa na každom systéme
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod chlad;

// merač pamäte a budení (R162, R163)
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod pamat;
// R167, R178: hlboký spánok jadier a nepoužité radiče kariet cez /sys
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod radice;

// skúšobné video s presne známym obrazom (R165) – vlastný generátor tokov H.264 a HEVC
#[cfg_attr(not(all(target_os = "linux", target_pointer_width = "64")), allow(dead_code))]
mod video;

// vlastný ovládač obrazovky cez DRM/KMS (R158 – chladný obraz)
// strážca linky HDMI – stav televízora cez SCDC, oprava zaseknutia, reštart linky (R185)
#[cfg(target_os = "linux")]
mod hdmi;
// vlastný ovládač CEC – televízor a krabička sa dohovoria cez HDMI (R181)
#[cfg(target_os = "linux")]
mod cec;
#[cfg(target_os = "linux")]
mod kms;
#[cfg(target_os = "linux")]
mod linux;
// vlastný klient dekodéra videa V4L2 (R165) – rozloženie štruktúr pre 64-bitové jadro
#[cfg(target_os = "linux")]
mod sys;
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
mod v4l2;
#[cfg(target_os = "linux")]
mod zaznam;

// R215: čítanie a zápis kariet a USB kľúčov mimo hlavnej slučky – zaseknuté úložisko nezastaví strážny časovač
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod uloziska;

// R223: meranie vlastného dekodéra HEVC na zariadení (rozhranie R4)
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod hevc_meranie;
// R226: zvuk cez HDMI – vlastný zápis do ALSA, skúšobný tón pri štarte
// R247: záznamy merania pre zberač meraní (LOVEC ZBERAČ) v miestnej sieti
#[cfg(target_os = "linux")]
mod zber;
// R264: aktualizácia systému v pamäti po sieti (štartér, skúška, návrat)
#[cfg(target_os = "linux")]
mod aktualizacia;
// R268 (A9): internet – adresa z routera (DHCP), preklad mien (DNS), čas zo siete (SNTP)
#[cfg(target_os = "linux")]
mod internet;
#[cfg(target_os = "linux")]
mod zvuk;
// R230: cesta zvuku na kartách ASoC s prepínačmi (Amlogic)
#[cfg(target_os = "linux")]
mod zvuk_cesta;
// vlastný klient schránky firmvéru Raspberry Pi – takty, napätia, škrtenie VideoCore (R193)
#[cfg(target_os = "linux")]
mod vcio;
// R198: chladný pokoj – nižšie napätie čipu RPi 2/3 len v pokoji, s poistkou
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod pokoj;

/// Verzia SKLENÍKA (dátum a odtlačok zostavenia z CI) – na obrazovke, aby bolo vidieť, ktorá karta beží.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const VERZIA: &str = match option_env!("SKLENIK_VERZIA") {
    Some(v) => v,
    None => "vývojová",
};

#[cfg(target_os = "linux")]
fn main() {
    // R268: skúška internetu pre CI a ručné overenie – `klic --internet <rozhranie> [--meno <meno>]…`
    let argumenty: Vec<String> = std::env::args().collect();
    if argumenty.iter().any(|a| a == "--internet") {
        std::process::exit(internet::skuska(&argumenty));
    }
    linux::spusti();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Správca systému (KLÍČ) je prvý program ORCHIDEA OS a beží len na Linuxe.");
}
