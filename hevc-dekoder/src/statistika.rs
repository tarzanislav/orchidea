//! Výsledky overenia bitovej zhody – spoločné pre 8-bitový aj 10-bitový dekodér.

/// Výsledky overenia bitovej zhody.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Statistika {
    /// dekódované obrazy
    pub obrazov: usize,
    /// obrazy s odtlačkom MD5 v SEI, ktorý sa zhoduje
    pub zhoda: usize,
    /// obrazy, ktorých odtlačok sa nezhoduje (poradie v dekódovaní, POC)
    pub nezhoda: Vec<(usize, i32)>,
    /// obrazy bez odtlačku MD5
    pub bez_odtlacku: usize,
    /// chyby dekódovania (poškodený tok, nepodporovaná vlastnosť)
    pub chyby: Vec<String>,
    /// preskočené obrazy RASL po náhodnom prístupe
    pub preskocene: usize,
    /// opis rezov prvých obrazov s nezhodou (ladenie na diaľku – záznam CI)
    pub opis_nezhody: Vec<String>,
}

impl Statistika {
    /// Pripočíta výsledky ďalšieho dekodéra (pri zmene bitovej hĺbky toku).
    pub fn pridaj(&mut self, s: Statistika) {
        self.obrazov += s.obrazov;
        self.zhoda += s.zhoda;
        self.nezhoda.extend(s.nezhoda);
        self.bez_odtlacku += s.bez_odtlacku;
        self.chyby.extend(s.chyby);
        self.preskocene += s.preskocene;
        self.opis_nezhody.extend(s.opis_nezhody);
    }
}
