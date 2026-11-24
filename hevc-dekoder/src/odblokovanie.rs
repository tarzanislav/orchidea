//! Odblokovací filter (deblocking filter) – H.265, kap. 8.7.2.
//!
//! ROZHRANIE (pevné, používa ho dekodér): [`odblokuj`].

use crate::filtre::{InfoObrazu, MapyFiltra};
use crate::obraz::Obraz;

/// Odblokovanie celého obrazu na mieste: najprv všetky zvislé hrany (jas aj farba) z rekonštruovaného obrazu,
/// potom všetky vodorovné hrany z výsledku zvislého filtrovania (8.7.2).
///
/// Sila hrany bS (8.7.2.4) sa odvodí z máp `mapy` (hrany, vnútrosnímkové bloky, nenulové koeficienty, pohyb);
/// rozhodnutia a filtre jasu podľa 8.7.2.5.3–8.7.2.5.7 (β a tC z QpY susedov a posunov rezu, ktorý obsahuje
/// vzorku q0,0), farba 4:2:0 len pri bS = 2 na mriežke 8 × 8 vzoriek farby (8.7.2.5.5). Vzorky blokov
/// s `bez_filtra` sa nemenia.
pub fn odblokuj(obraz: &mut Obraz, mapy: &MapyFiltra, info: &InfoObrazu) {
    // ZÁSTUPNÁ IMPLEMENTÁCIA – nahradí ju modul odblokovania
    let _ = (obraz, mapy, info);
}
