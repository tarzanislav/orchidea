//! **Profil zariadenia (PÔDA)** – spoločná schopnosť ORCHIDEE a SKLENÍKA (R204).
//!
//! Jeden popis hardvéru pre všetkých: správca systému SKLENÍKA (KLÍČ) podľa neho nastaví strážcu teploty,
//! pamäť a periférie; plánovač ORCHIDEE (FOTOSYNTÉZA, pravidlo Z11) podľa neho zvolí veľkosť dlaždíc, počet
//! vlákien a cestu výpočtu. Číta sa len zo súborov systému – nič sa nemeria, nič sa nezapisuje.
//!
//! Dnes: Linux (/proc, /sys, strom popisu hardvéru) vrátane Raspberry Pi, Amlogic a Allwinner. Na ostatných
//! systémoch vráti prázdny profil s počtom jadier – ďalšie zdroje (Windows, macOS, Android, web) pribudnú
//! v rovnakom type.

mod profil;

pub use profil::{ProfilHardveru, nacitaj, rozloz_reviziu};
