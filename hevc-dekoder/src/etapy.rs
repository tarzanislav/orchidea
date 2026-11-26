//! **Rozpis času dekódovania po etapách** (meranie na doske, rozhranie R4 – riadky `CAS`). Zapína sa len na
//! meranie ([`zapni`]); vypnuté stojí jedno načítanie príznaku na úsek.
//!
//! Úseky sa môžu vnárať (napr. čakanie na referenciu vnútri medzisnímkovej predikcie vnútri rezu): každá etapa
//! dostane len **vlastný čas** – čas úseku mínus čas vnorených meraných úsekov. Etapa `cabac` je tak vlastný čas
//! dekódovania rezu: aritmetické dekódovanie a syntax (aj odvodenie pohybu), bez predikcie, transformácií a filtrov.
//! Každé vlákno sčítava zvlášť a do spoločných súčtov prispeje po každom obraze ([`vyprazdni`]).

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// Etapa dekódovania.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Etapa {
    /// hlavné vlákno: jednotky NAL, sady parametrov, hlavičky rezov, správa obrazov
    Rozbor,
    /// vlastný čas dekódovania rezu: CABAC a syntax CTU (aj zlučovanie a AMVP)
    Cabac,
    /// vnútrosnímková predikcia
    Intra,
    /// medzisnímková predikcia (interpolácia a váženie) bez čakania na referenciu
    Inter,
    /// spätné kvantovanie, transformácia a pripočítanie rezídua
    Transformacia,
    Odblokovanie,
    Sao,
    /// kopírovanie hotových riadkov do zverejneného obrazu (bez SAO) a pohybu pre TMVP
    Zverejnenie,
    /// overenie odtlačku obrazu zo SEI
    Odtlacok,
    /// zvyšok úlohy obrazu (príprava stavu a pod.)
    Ostatne,
    /// čakanie na riadky referenčného obrazu, ktorý ešte dekóduje iné vlákno
    CakanieRef,
    /// hlavné vlákno čaká na voľné miesto v rade úloh alebo na koniec dekódovania
    CakanieHlavne,
    /// pracovné vlákno nemá úlohu
    Necinnost,
}

/// Počet etáp.
pub const POCET: usize = 13;

/// Všetky etapy v poradí výpisu.
pub const VSETKY: [Etapa; POCET] = [
    Etapa::Rozbor,
    Etapa::Cabac,
    Etapa::Intra,
    Etapa::Inter,
    Etapa::Transformacia,
    Etapa::Odblokovanie,
    Etapa::Sao,
    Etapa::Zverejnenie,
    Etapa::Odtlacok,
    Etapa::Ostatne,
    Etapa::CakanieRef,
    Etapa::CakanieHlavne,
    Etapa::Necinnost,
];

impl Etapa {
    /// Meno v riadku `CAS` (bez diakritiky, stabilné pre parser).
    pub fn meno(self) -> &'static str {
        match self {
            Etapa::Rozbor => "rozbor",
            Etapa::Cabac => "cabac",
            Etapa::Intra => "intra",
            Etapa::Inter => "inter",
            Etapa::Transformacia => "transformacia",
            Etapa::Odblokovanie => "odblokovanie",
            Etapa::Sao => "sao",
            Etapa::Zverejnenie => "zverejnenie",
            Etapa::Odtlacok => "odtlacok",
            Etapa::Ostatne => "ostatne",
            Etapa::CakanieRef => "cakanie_ref",
            Etapa::CakanieHlavne => "cakanie_hlavne",
            Etapa::Necinnost => "necinnost",
        }
    }

    /// Je to práca procesora (nie čakanie)?
    pub fn je_praca(self) -> bool {
        !matches!(self, Etapa::CakanieRef | Etapa::CakanieHlavne | Etapa::Necinnost)
    }
}

static ZAPNUTE: AtomicBool = AtomicBool::new(false);
static SPOLU: [AtomicU64; POCET] = [const { AtomicU64::new(0) }; POCET];

thread_local! {
    static LOKALNE: [Cell<u64>; POCET] = const { [const { Cell::new(0) }; POCET] };
    /// čas skončených vnorených úsekov aktuálne otvoreného úseku
    static VNORENE: Cell<u64> = const { Cell::new(0) };
}

/// Zapne alebo vypne meranie (pred dekódovaním).
pub fn zapni(zapnute: bool) {
    ZAPNUTE.store(zapnute, Ordering::Relaxed);
}

/// Prebieha meranie?
#[inline]
pub fn zapnute() -> bool {
    ZAPNUTE.load(Ordering::Relaxed)
}

/// Meraný úsek – koniec pri zániku (aj pri skoršom návrate z funkcie).
#[must_use]
#[derive(Debug)]
pub struct Usek {
    etapa: Etapa,
    zaciatok: Option<(Instant, u64)>,
}

/// Začne meraný úsek etapy `etapa`.
#[inline]
pub fn meraj(etapa: Etapa) -> Usek {
    let zaciatok = zapnute().then(|| (Instant::now(), VNORENE.with(|v| v.replace(0))));
    Usek { etapa, zaciatok }
}

impl Drop for Usek {
    #[inline]
    fn drop(&mut self) {
        if let Some((t, predtym)) = self.zaciatok {
            let ns = t.elapsed().as_nanos() as u64;
            let deti = VNORENE.with(|v| v.get());
            LOKALNE.with(|l| {
                let c = &l[self.etapa as usize];
                c.set(c.get() + ns.saturating_sub(deti));
            });
            VNORENE.with(|v| v.set(predtym + ns));
        }
    }
}

/// Prenesie súčty tohto vlákna do spoločných súčtov.
pub fn vyprazdni() {
    LOKALNE.with(|l| {
        for (i, c) in l.iter().enumerate() {
            let v = c.replace(0);
            if v > 0 {
                SPOLU[i].fetch_add(v, Ordering::Relaxed);
            }
        }
    });
}

/// Spoločné súčty v nanosekundách (po etapách v poradí [`VSETKY`]) a ich vynulovanie.
pub fn vezmi() -> [u64; POCET] {
    vyprazdni();
    std::array::from_fn(|i| SPOLU[i].swap(0, Ordering::Relaxed))
}

#[cfg(test)]
mod testy {
    use super::*;

    fn lokalne() -> [u64; POCET] {
        LOKALNE.with(|l| std::array::from_fn(|i| l[i].replace(0)))
    }

    /// Súčty tohto vlákna (iné skúšky bežiace naraz ich neovplyvnia).
    #[test]
    fn vnorene_useky_maju_vlastny_cas() {
        zapni(true);
        let _ = lokalne();
        {
            let _rez = meraj(Etapa::Cabac);
            std::thread::sleep(std::time::Duration::from_millis(5));
            {
                let _inter = meraj(Etapa::Inter);
                std::thread::sleep(std::time::Duration::from_millis(5));
                let _cak = meraj(Etapa::CakanieRef);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        let v = lokalne();
        let ms = |e: Etapa| v[e as usize] as f64 / 1e6;
        assert!((4.0..20.0).contains(&ms(Etapa::Cabac)), "cabac {}", ms(Etapa::Cabac));
        assert!((4.0..20.0).contains(&ms(Etapa::Inter)), "inter {}", ms(Etapa::Inter));
        assert!((4.0..20.0).contains(&ms(Etapa::CakanieRef)), "čakanie {}", ms(Etapa::CakanieRef));
        assert!(ms(Etapa::Cabac) + ms(Etapa::Inter) + ms(Etapa::CakanieRef) < 40.0);
        zapni(false);
    }
}
