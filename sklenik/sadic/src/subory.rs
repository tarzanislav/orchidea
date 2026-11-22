//! Strom súborov na zápis do nového zväzku (FAT32, exFAT) – z rozbaleného ISO alebo zo skúšok.

use crate::iso::Polozka;

/// Uzol stromu: priečinok alebo súbor.
#[derive(Debug, Clone)]
pub struct Uzol {
    pub meno: String,
    pub priecinok: bool,
    pub velkost: u64,
    /// Index položky v rozbore ISO (odkiaľ sa čítajú dáta súboru).
    pub polozka: Option<usize>,
    pub deti: Vec<usize>,
}

/// Dodávateľ dát súboru: (uzol, posun v súbore, kam) – napr. čítanie z ISO.
pub type Citaj<'a> = dyn FnMut(usize, u64, &mut [u8]) -> std::io::Result<()> + 'a;

/// Strom; uzol 0 je koreňový priečinok.
#[derive(Debug, Clone)]
pub struct Strom {
    pub uzly: Vec<Uzol>,
}

impl Default for Strom {
    fn default() -> Self {
        Strom::novy()
    }
}

/// Zapísaný súbor: uzol, posun dát na disku (vzhľadom na začiatok zväzku), veľkosť, odtlačok BLAKE3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zapisany {
    pub uzol: usize,
    pub posun: u64,
    pub velkost: u64,
    pub odtlacok: [u8; 32],
}

impl Strom {
    pub fn novy() -> Strom {
        Strom {
            uzly: vec![Uzol {
                meno: String::new(),
                priecinok: true,
                velkost: 0,
                polozka: None,
                deti: Vec::new(),
            }],
        }
    }

    fn dieta(&self, rodic: usize, meno: &str) -> Option<usize> {
        self.uzly[rodic].deti.iter().copied().find(|&d| self.uzly[d].meno.eq_ignore_ascii_case(meno))
    }

    /// Pridá položku (aj s chýbajúcimi priečinkami po ceste). Vráti index uzla.
    pub fn pridaj(&mut self, cesta: &str, priecinok: bool, velkost: u64, polozka: Option<usize>) -> usize {
        let casti: Vec<&str> = cesta.split('/').filter(|c| !c.is_empty()).collect();
        let mut u = 0;
        for (i, c) in casti.iter().enumerate() {
            let posledna = i + 1 == casti.len();
            u = match self.dieta(u, c) {
                Some(d) => {
                    if posledna && !priecinok {
                        self.uzly[d].velkost = velkost;
                        self.uzly[d].polozka = polozka;
                    }
                    d
                }
                None => {
                    let novy = self.uzly.len();
                    let je_priecinok = !posledna || priecinok;
                    self.uzly.push(Uzol {
                        meno: c.to_string(),
                        priecinok: je_priecinok,
                        velkost: if je_priecinok { 0 } else { velkost },
                        polozka: if je_priecinok { None } else { polozka },
                        deti: Vec::new(),
                    });
                    self.uzly[u].deti.push(novy);
                    novy
                }
            };
        }
        u
    }

    /// Strom z položiek rozboru, ktoré vyberie `ber`.
    pub fn z_poloziek(polozky: &[Polozka], ber: impl Fn(&Polozka) -> bool) -> Strom {
        let mut s = Strom::novy();
        for (i, p) in polozky.iter().enumerate().filter(|(_, p)| ber(p)) {
            s.pridaj(&p.cesta, p.priecinok, p.velkost, (!p.priecinok).then_some(i));
        }
        s
    }

    /// Priečinky v poradí do šírky (koreň prvý).
    pub fn priecinky(&self) -> Vec<usize> {
        let mut v = vec![0];
        let mut i = 0;
        while i < v.len() {
            let u = v[i];
            v.extend(self.uzly[u].deti.iter().copied().filter(|&d| self.uzly[d].priecinok));
            i += 1;
        }
        v
    }

    /// Súbory v poradí priečinkov.
    pub fn subory(&self) -> Vec<usize> {
        self.priecinky()
            .into_iter()
            .flat_map(|p| {
                self.uzly[p].deti.iter().copied().filter(|&d| !self.uzly[d].priecinok).collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn velkost_suborov(&self) -> u64 {
        self.uzly.iter().filter(|u| !u.priecinok).map(|u| u.velkost).sum()
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn strom_z_ciest() {
        let mut s = Strom::novy();
        s.pridaj("sources/install.wim", false, 5, Some(0));
        s.pridaj("EFI/BOOT/bootx64.efi", false, 3, Some(1));
        s.pridaj("efi/boot/grubx64.efi", false, 4, Some(2));
        s.pridaj("prazdny", true, 0, None);
        assert_eq!(s.priecinky().len(), 5, "koreň, sources, EFI, BOOT, prazdny");
        assert_eq!(s.subory().len(), 3);
        assert_eq!(s.velkost_suborov(), 12);
    }
}
