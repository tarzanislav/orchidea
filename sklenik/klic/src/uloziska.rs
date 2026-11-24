//! Práca s kartami a USB kľúčmi mimo hlavnej slučky (R215).
//!
//! Hlavná slučka správcu systému (KLÍČ) kŕmi strážny časovač: keď sa na 15 s zastaví, doska sa reštartuje.
//! Zaseknutá karta alebo USB kľúč (Odroid C4, 24. 11. 2026: čítanie poškodenej karty a zápis na kľúč) preto
//! nesmie hlavnú slučku nikdy zastaviť. Každé čítanie a zápis úložiska z hlavnej slučky beží vo vlastnom
//! vlákne; hlavná slučka si výsledok vyzdvihne, keď je hotový, a zaseknutie len ohlási.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Úloha na pozadí – výsledok sa vyzdvihne neskôr.
#[derive(Debug)]
pub struct Cakanie<T> {
    meno: &'static str,
    rx: Receiver<T>,
    od: Instant,
    ohlasene: bool,
}

/// Spustí `f` vo vlastnom vlákne a vráti čakanie na výsledok. Vlákno, ktoré sa zasekne na úložisku, zostane
/// zaseknuté len samo – hlavná slučka beží ďalej.
pub fn na_pozadi<T: Send + 'static>(
    meno: &'static str,
    f: impl FnOnce() -> T + Send + 'static,
) -> Cakanie<T> {
    let (tx, rx) = mpsc::channel();
    let spustene = std::thread::Builder::new().name(format!("uloziska-{meno}")).spawn(move || {
        let _ = tx.send(f());
    });
    if spustene.is_err() {
        // bez vlákna (nedostatok pamäte) – úloha sa vynechá, čakanie ostane prázdne
        let (_tx, rx2) = mpsc::channel();
        return Cakanie { meno, rx: rx2, od: Instant::now(), ohlasene: false };
    }
    Cakanie { meno, rx, od: Instant::now(), ohlasene: false }
}

/// Stav úlohy pri vyzdvihnutí.
#[derive(Debug, PartialEq, Eq)]
pub enum Stav<T> {
    Hotovo(T),
    Bezi,
    /// beží dlhšie ako limit – vráti sa raz, aby sa dalo ohlásiť zaseknuté úložisko
    Zaseknuta(Duration),
    /// vlákno skončilo bez výsledku (panika) alebo sa nespustilo
    Stratena,
}

impl<T> Cakanie<T> {
    pub fn meno(&self) -> &'static str {
        self.meno
    }

    /// Bez čakania: výsledok, ak je hotový; po prekročení `limit` raz [`Stav::Zaseknuta`].
    pub fn vyzdvihni(&mut self, limit: Duration) -> Stav<T> {
        match self.rx.try_recv() {
            Ok(v) => Stav::Hotovo(v),
            Err(TryRecvError::Disconnected) => Stav::Stratena,
            Err(TryRecvError::Empty) => {
                if !self.ohlasene && self.od.elapsed() >= limit {
                    self.ohlasene = true;
                    Stav::Zaseknuta(self.od.elapsed())
                } else {
                    Stav::Bezi
                }
            }
        }
    }

    /// Počká na výsledok najviac `limit` (len pri zriedkavých krokoch, kratších ako strážny časovač).
    pub fn pockaj(self, limit: Duration) -> Option<T> {
        self.rx.recv_timeout(limit).ok()
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    #[cfg_attr(target_os = "wasi", ignore)]
    fn hotova_uloha() {
        let mut c = na_pozadi("skuska", || 41 + 1);
        let mut v = None;
        for _ in 0..200 {
            if let Stav::Hotovo(x) = c.vyzdvihni(Duration::from_secs(5)) {
                v = Some(x);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(v, Some(42));
        assert_eq!(na_pozadi("skuska", || "ok").pockaj(Duration::from_secs(5)), Some("ok"));
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore)]
    fn zaseknuta_uloha_nezastavi_volajuceho() {
        let (tx, rx) = mpsc::channel::<()>();
        let mut c = na_pozadi("zaseknuta", move || {
            // „zaseknuté úložisko“: čaká, kým ho skúška pustí
            let _ = rx.recv();
            7
        });
        let t = Instant::now();
        assert_eq!(c.vyzdvihni(Duration::from_millis(20)), Stav::Bezi);
        std::thread::sleep(Duration::from_millis(30));
        assert!(matches!(c.vyzdvihni(Duration::from_millis(20)), Stav::Zaseknuta(_)));
        // ohlási sa len raz
        assert_eq!(c.vyzdvihni(Duration::from_millis(20)), Stav::Bezi);
        assert!(t.elapsed() < Duration::from_secs(2));
        tx.send(()).unwrap();
        assert_eq!(c.pockaj(Duration::from_secs(5)), Some(7));
        // panika vo vlákne = stratená úloha, nie pád volajúceho
        let mut p = na_pozadi("panika", || -> u8 { panic!("skúška") });
        let mut stav = Stav::Bezi;
        for _ in 0..400 {
            stav = p.vyzdvihni(Duration::from_secs(5));
            if stav != Stav::Bezi {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(stav, Stav::Stratena);
    }
}
