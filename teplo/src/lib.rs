//! **Tepelný model** – spoločná schopnosť ORCHIDEE a SKLENÍKA (R204, R205).
//!
//! - `okolie` – odhad teploty okolia a vlastného ohrevu zariadenia len zo snímača v čipe, rozpoznanie zdroja
//!   tepla a tepelného úteku, tepelný rozpočet pre programy (chladné okno).
//!
//! Čistá logika bez hardvéru: hodnoty dodá správca systému (SKLENÍK) alebo systém počítača. Na nej postaví
//! plánovač ORCHIDEE zákon Z14 – každá práca má známu cenu v energii a teple.

pub mod okolie;
