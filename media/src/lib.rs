//! **Kodeky a kontajnery (Médiá)** – základ prehrávača médií LÝRA (prúd D, etapa L0; návrh v dokumente
//! projektu `ORCHIDEA/lyra-media.md`).
//!
//! Tok dát:
//!
//! ```text
//! ZdrojBajtov ─► Kontajner (Matroska | MP4 | MPEG-TS) ─► Paket ─┬─► DekoderZvuku ─► BlokZvuku ─► R5: zvuk pre HDMI
//!                                                             └─► R5: PaketVidea pre hardvérový dekodér (V4L2)
//! HodinyPrehravania (zvuk ako zdroj času) ◄── merania zvuku a obnovy displeja (R5)
//! ```
//!
//! Zásady:
//! - **neutrálny balík** vrstvy 2 – nič zo systému (žiadne ioctl, ALSA ani V4L2; to robí ORCHIDEA OS cez R5), beží
//!   a skúša sa na každej platforme,
//! - **presný čas** – celočíselné tiky stopy a časová základňa ako zlomok ([`cas`]); nanosekundy len na plánovanie,
//! - **nikdy pád** – každá chyba v údajoch je [`Chyba`](orchidea_jadro::Chyba); alokácia až po kontrole voči
//!   [`Limity`] a zvyšku súboru; fuzzer v skúškach (R211),
//! - **dôkaz podľa povahy kodeku** (R301) – bezstratové bitovo, stratové presnosťou podľa skúšky normy
//!   a rovnakými bitmi na každej platforme (R307),
//! - **chladne** – čítanie súvisle dopredu po dávkach, kompaktné indexy, priechod zvuku do televízora (R303, R306).
//!
//! Moduly: [`zdroj`] (čítanie na pozícii), [`cas`] (časová základňa), [`limity`], [`stopa`] (stopy a kodeky),
//! [`paket`], [`kontajner`] (rozpoznanie formátu, rozhranie čítačky), [`zvuk`] (bloky vzoriek, rozhranie
//! dekodéra) a [`r5`] (rozhranie pre ORCHIDEA OS).
#![forbid(unsafe_code)]

pub mod cas;
pub mod kontajner;
pub mod limity;
pub mod paket;
pub mod r5;
pub mod stopa;
pub mod zdroj;
pub mod zvuk;

pub use kontajner::{FormatKontajnera, Kontajner, Posun, rozpoznaj};
pub use limity::Limity;
pub use paket::Paket;
pub use stopa::{DruhStopy, Kodek, ParametreVidea, ParametreZvuku, Stopa};
pub use zdroj::{PametovyZdroj, SuborovyZdroj, ZdrojBajtov};
pub use zvuk::{BlokZvuku, DekoderZvuku, RozlozenieKanalov, VzorkyZvuku};
