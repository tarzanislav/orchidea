//! **Paket** – jeden komprimovaný rámec stopy (prístupový celok videa, rámec zvuku, titulok) s časovými značkami.

/// Paket z kontajnera.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Paket {
    /// Index stopy v `Kontajner::stopy()` (nie číslo stopy v súbore).
    pub stopa: usize,
    /// Obsah rámca presne ako v kontajneri (po zrušení lacingu, zložení PES a odstránení hlavičky).
    pub data: Vec<u8>,
    /// Čas zobrazenia (PTS) v tikoch stopy.
    pub pts: Option<i64>,
    /// Čas dekódovania (DTS) v tikoch stopy – Matroska ho nemá.
    pub dts: Option<i64>,
    /// Trvanie v tikoch stopy, ak je známe.
    pub trvanie: Option<i64>,
    /// Kľúčový rámec – od neho sa dá začať dekódovať (IDR, IRAP, rámec bez závislostí).
    pub klucovy: bool,
    /// Po tomto pakete sa tok prerušil alebo začína po posune – dekodér treba vyprázdniť.
    pub prerusenie: bool,
    /// Koľko vzoriek zvuku zahodiť na začiatku a na konci dekódovaného rámca (bezmedzerové prehrávanie:
    /// Matroska `DiscardPadding`, zoznam úprav MP4, oneskorenie kodéra).
    pub orezat_zaciatok: u32,
    pub orezat_koniec: u32,
    /// Poloha začiatku paketu v súbore (bajty) – diagnostika a rýchly posun.
    pub pozicia: u64,
}
