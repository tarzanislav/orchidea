//! Zmena – jediný spôsob, ako sa dielo mení (prvotný pojem 9, pravidlo S6).
//!
//! Človek, recept, pomocník, rozšírenie aj iný program menia dielo **len** zmenami. Každá zmena má
//! hybridný logický čas (fyzický čas + logické počítadlo + replika), takže zmeny z viacerých zariadení
//! sa dajú zlúčiť bez konfliktov: všetky repliky prehrajú zjednotenie zmien v rovnakom úplnom poradí
//! a dostanú rovnaké dielo, bez ohľadu na to, v akom poradí zmeny prišli.

use crate::chyba::{Chyba, Vysledok};
use crate::graf::{
    DruhUzla, Graf, Parameter, UzolId, Zdroj, dekoduj_prepisania, dekoduj_volitelne_meno,
    dekoduj_volitelny_zdroj, zakoduj_prepisania, zakoduj_volitelne_meno, zakoduj_volitelny_zdroj,
};
use crate::hodnota::Hodnota;
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::Meno;
use crate::odtlacok::Odtlacok;
use crate::operacia::{PopisParametra, PopisPortu};
use crate::vlastnost::Vlastnosti;
use std::collections::{BTreeMap, BTreeSet};

/// Hybridný logický čas. Poradie: fyzický čas, logické počítadlo, replika.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Cas {
    /// Milisekundy od začiatku epochy Unix.
    pub ms: u64,
    pub logicke: u32,
    pub replika: u64,
}

/// Hodiny repliky – zaručia, že každá ďalšia zmena má väčší čas ako všetko, čo replika videla.
#[derive(Debug, Clone)]
pub struct Hodiny {
    replika: u64,
    posledny: Cas,
}

impl Hodiny {
    pub fn nove(replika: u64) -> Self {
        Hodiny { replika, posledny: Cas { ms: 0, logicke: 0, replika } }
    }

    pub fn replika(&self) -> u64 {
        self.replika
    }

    /// Čas pre novú zmenu (fyzický čas dodá volajúci – v testoch pevne, v programe zo systému).
    pub fn tikni(&mut self, fyzicky_ms: u64) -> Cas {
        let c = if fyzicky_ms > self.posledny.ms {
            Cas { ms: fyzicky_ms, logicke: 0, replika: self.replika }
        } else {
            Cas {
                ms: self.posledny.ms,
                logicke: self.posledny.logicke.saturating_add(1),
                replika: self.replika,
            }
        };
        self.posledny = c;
        c
    }

    /// Prijatie cudzieho času (po zlúčení) – ďalšie lokálne zmeny budú neskôr.
    pub fn prijmi(&mut self, cudzi: Cas) {
        if (cudzi.ms, cudzi.logicke) > (self.posledny.ms, self.posledny.logicke) {
            self.posledny = Cas { ms: cudzi.ms, logicke: cudzi.logicke, replika: self.replika };
        }
    }

    /// Fyzický čas zo systému.
    pub fn teraz_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Čomu patrí vlastnosť.
#[derive(Debug, Clone, PartialEq)]
pub enum CielVlastnosti {
    Dielo,
    Graf(Meno),
    Uzol(Meno, UzolId),
}

/// Jedna elementárna úprava diela.
#[derive(Debug, Clone, PartialEq)]
pub enum Uprava {
    VytvorGraf { graf: Meno },
    OdstranGraf { graf: Meno },
    PridajUzol { graf: Meno, uzol: UzolId, druh: DruhUzla },
    OdstranUzol { graf: Meno, uzol: UzolId },
    Spoj { graf: Meno, uzol: UzolId, vstup: u32, zdroj: Option<Zdroj> },
    NastavParameter { graf: Meno, uzol: UzolId, meno: Meno, hodnota: Option<Parameter> },
    NastavObmedzenie { graf: Meno, uzol: UzolId, zdroj: Option<Zdroj> },
    NastavVypnutie { graf: Meno, uzol: UzolId, vypnuty: bool },
    NastavVariant { graf: Meno, uzol: UzolId, variant: Option<Meno> },
    NastavVstupGrafu { graf: Meno, index: u32, popis: Option<PopisPortu> },
    NastavVystupGrafu { graf: Meno, meno: Meno, zdroj: Option<Zdroj> },
    NastavParameterGrafu { graf: Meno, popis: PopisParametra },
    OdstranParameterGrafu { graf: Meno, meno: Meno },
    NastavVariantGrafu { graf: Meno, variant: Meno, prepisania: Option<BTreeMap<Meno, Parameter>> },
    NastavVlastnost { ciel: CielVlastnosti, meno: Meno, hodnota: Option<Hodnota> },
}

/// Zmena – časovo označená skupina úprav od jedného pôvodcu.
#[derive(Debug, Clone, PartialEq)]
pub struct Zmena {
    pub cas: Cas,
    /// Kto zmenu urobil (človek, recept, pomocník, rozšírenie, iný program) – jadro mu nedáva výsady.
    pub povodca: Meno,
    pub popis: String,
    pub upravy: Vec<Uprava>,
}

impl Zmena {
    /// Identita zmeny = odtlačok jej kanonického zápisu.
    pub fn id(&self) -> Odtlacok {
        Odtlacok::z(&self.do_bajtov())
    }
}

/// Dielo – stav vzniknutý prehraním zmien. Voliteľný kontajner: graf grafov s vlastnosťami.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dielo {
    pub grafy: BTreeMap<Meno, Graf>,
    pub vlastnosti: Vlastnosti,
}

/// Výsledok prehrania zmien.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpravaPrehrania {
    pub zmien: usize,
    pub uprav: usize,
    /// Úpravy, ktoré nebolo možné použiť (napr. súbežne odstránený uzol) – nie sú chybou, len sa preskočia.
    pub preskocenych: usize,
}

impl Dielo {
    pub fn nove() -> Self {
        Dielo::default()
    }

    fn graf(&mut self, m: &Meno) -> Vysledok<&mut Graf> {
        self.grafy.get_mut(m).ok_or_else(|| Chyba::Chybajuce(format!("graf {m}")))
    }

    /// Použije jednu úpravu.
    pub fn pouzi(&mut self, u: &Uprava) -> Vysledok<()> {
        match u {
            Uprava::VytvorGraf { graf } => {
                self.grafy.entry(graf.clone()).or_default();
            }
            Uprava::OdstranGraf { graf } => {
                self.grafy.remove(graf).ok_or_else(|| Chyba::Chybajuce(format!("graf {graf}")))?;
            }
            Uprava::PridajUzol { graf, uzol, druh } => {
                self.graf(graf)?.pridaj(*uzol, crate::graf::Uzol::novy(druh.clone()))?;
            }
            Uprava::OdstranUzol { graf, uzol } => {
                self.graf(graf)?.odstran(*uzol).ok_or_else(|| Chyba::Chybajuce(format!("uzol {uzol:?}")))?;
            }
            Uprava::Spoj { graf, uzol, vstup, zdroj } => {
                self.graf(graf)?.spoj(*uzol, *vstup as usize, *zdroj)?;
            }
            Uprava::NastavParameter { graf, uzol, meno, hodnota } => {
                let g = self.graf(graf)?;
                if let Some(Parameter::Vstup(Zdroj::Uzol { uzol: z, .. })) = hodnota
                    && !g.uzly.contains_key(z)
                {
                    return Err(Chyba::Chybajuce(format!("zdrojový uzol {z:?}")));
                }
                let u = g.uzol_mut(*uzol)?;
                match hodnota {
                    Some(h) => {
                        u.parametre.insert(meno.clone(), h.clone());
                    }
                    None => {
                        u.parametre.remove(meno);
                    }
                }
            }
            Uprava::NastavObmedzenie { graf, uzol, zdroj } => {
                let g = self.graf(graf)?;
                if let Some(Zdroj::Uzol { uzol: z, .. }) = zdroj
                    && !g.uzly.contains_key(z)
                {
                    return Err(Chyba::Chybajuce(format!("zdrojový uzol {z:?}")));
                }
                g.uzol_mut(*uzol)?.obmedzenie = *zdroj;
            }
            Uprava::NastavVypnutie { graf, uzol, vypnuty } => {
                self.graf(graf)?.uzol_mut(*uzol)?.vypnuty = *vypnuty;
            }
            Uprava::NastavVariant { graf, uzol, variant } => {
                self.graf(graf)?.uzol_mut(*uzol)?.variant = variant.clone();
            }
            Uprava::NastavVstupGrafu { graf, index, popis } => {
                let g = self.graf(graf)?;
                let i = *index as usize;
                match popis {
                    Some(p) => {
                        if i > g.vstupy.len() {
                            return Err(Chyba::MimoRozsahu(format!("vstup grafu {i}")));
                        }
                        if i == g.vstupy.len() {
                            g.vstupy.push(p.clone());
                        } else {
                            g.vstupy[i] = p.clone();
                        }
                    }
                    None => {
                        if i + 1 != g.vstupy.len() {
                            return Err(Chyba::MimoRozsahu(
                                "odstrániť možno len posledný vstup grafu".into(),
                            ));
                        }
                        g.vstupy.pop();
                    }
                }
            }
            Uprava::NastavVystupGrafu { graf, meno, zdroj } => {
                self.graf(graf)?.nastav_vystup(meno.clone(), *zdroj);
            }
            Uprava::NastavParameterGrafu { graf, popis } => {
                let g = self.graf(graf)?;
                match g.parametre.iter_mut().find(|p| p.meno == popis.meno) {
                    Some(p) => *p = popis.clone(),
                    None => g.parametre.push(popis.clone()),
                }
            }
            Uprava::OdstranParameterGrafu { graf, meno } => {
                let g = self.graf(graf)?;
                let pred = g.parametre.len();
                g.parametre.retain(|p| &p.meno != meno);
                if g.parametre.len() == pred {
                    return Err(Chyba::Chybajuce(format!("parameter grafu {meno}")));
                }
            }
            Uprava::NastavVariantGrafu { graf, variant, prepisania } => {
                let g = self.graf(graf)?;
                match prepisania {
                    Some(p) => {
                        g.varianty.insert(variant.clone(), p.clone());
                    }
                    None => {
                        g.varianty.remove(variant);
                    }
                }
            }
            Uprava::NastavVlastnost { ciel, meno, hodnota } => {
                let v = match ciel {
                    CielVlastnosti::Dielo => &mut self.vlastnosti,
                    CielVlastnosti::Graf(g) => &mut self.graf(g)?.vlastnosti,
                    CielVlastnosti::Uzol(g, u) => &mut self.graf(g)?.uzol_mut(*u)?.vlastnosti,
                };
                match hodnota {
                    Some(h) => v.nastav(meno.clone(), h.clone()),
                    None => {
                        v.odstran(meno);
                    }
                }
            }
        }
        Ok(())
    }

    /// Prehrá zmeny v úplnom poradí (čas, potom identita). Každá zmena sa použije celá alebo vôbec
    /// (ak niektorá úprava zlyhá, preskočí sa celá zmena – dielo nikdy neostane v polovičnom stave).
    pub fn prehraj<'a>(zmeny: impl IntoIterator<Item = &'a Zmena>) -> (Dielo, SpravaPrehrania) {
        let mut zoradene: Vec<(Cas, Odtlacok, &Zmena)> =
            zmeny.into_iter().map(|z| (z.cas, z.id(), z)).collect();
        zoradene.sort_by_key(|a| (a.0, a.1));
        zoradene.dedup_by(|a, b| a.1 == b.1);
        let mut d = Dielo::nove();
        let mut s = SpravaPrehrania::default();
        for (_, _, z) in zoradene {
            s.zmien += 1;
            if d.pouzi_celu(z) {
                s.uprav += z.upravy.len();
            } else {
                s.preskocenych += z.upravy.len();
            }
        }
        (d, s)
    }

    /// Použije celú zmenu, alebo nič: pri zlyhaní vráti dotknuté grafy a vlastnosti do pôvodného stavu.
    /// Záloha obsahuje len to, čoho sa zmena dotýka – prehranie dlhého denníka ostáva rýchle.
    pub fn pouzi_celu(&mut self, z: &Zmena) -> bool {
        let mut grafy: BTreeMap<Meno, Option<Graf>> = BTreeMap::new();
        let mut vlastnosti = None;
        for u in &z.upravy {
            match dotknuty_graf(u) {
                Some(g) => {
                    if !grafy.contains_key(g) {
                        grafy.insert(g.clone(), self.grafy.get(g).cloned());
                    }
                }
                None => {
                    if vlastnosti.is_none() {
                        vlastnosti = Some(self.vlastnosti.clone());
                    }
                }
            }
        }
        if z.upravy.iter().all(|u| self.pouzi(u).is_ok()) {
            return true;
        }
        for (m, g) in grafy {
            match g {
                Some(g) => {
                    self.grafy.insert(m, g);
                }
                None => {
                    self.grafy.remove(&m);
                }
            }
        }
        if let Some(v) = vlastnosti {
            self.vlastnosti = v;
        }
        false
    }
}

fn dotknuty_graf(u: &Uprava) -> Option<&Meno> {
    match u {
        Uprava::VytvorGraf { graf }
        | Uprava::OdstranGraf { graf }
        | Uprava::PridajUzol { graf, .. }
        | Uprava::OdstranUzol { graf, .. }
        | Uprava::Spoj { graf, .. }
        | Uprava::NastavParameter { graf, .. }
        | Uprava::NastavObmedzenie { graf, .. }
        | Uprava::NastavVypnutie { graf, .. }
        | Uprava::NastavVariant { graf, .. }
        | Uprava::NastavVstupGrafu { graf, .. }
        | Uprava::NastavVystupGrafu { graf, .. }
        | Uprava::NastavParameterGrafu { graf, .. }
        | Uprava::OdstranParameterGrafu { graf, .. }
        | Uprava::NastavVariantGrafu { graf, .. } => Some(graf),
        Uprava::NastavVlastnost { ciel: CielVlastnosti::Graf(g) | CielVlastnosti::Uzol(g, _), .. } => Some(g),
        Uprava::NastavVlastnost { ciel: CielVlastnosti::Dielo, .. } => None,
    }
}

impl Dielo {
    /// Prostredie pre výpočet, v ktorom sú grafy diela dostupné aj pre odkazy.
    pub fn do_prostredia(&self, mut p: crate::prostredie::Prostredie) -> crate::prostredie::Prostredie {
        for (m, g) in &self.grafy {
            p.grafy.insert(m.clone(), std::sync::Arc::new(g.clone()));
        }
        p
    }
}

/// Zjednotenie dvoch množín zmien (podľa identity) – základ Pokračovania na inom zariadení.
pub fn zluc(a: &[Zmena], b: &[Zmena]) -> Vec<Zmena> {
    let mut videne: BTreeSet<Odtlacok> = BTreeSet::new();
    let mut v = Vec::with_capacity(a.len() + b.len());
    for z in a.iter().chain(b) {
        if videne.insert(z.id()) {
            v.push(z.clone());
        }
    }
    v
}

// ---------- kódovanie ----------

impl Kodovatelne for Cas {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.cislo(self.ms).cislo(self.logicke as u64).cislo(self.replika);
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        Ok(Cas {
            ms: c.cislo()?,
            logicke: u32::try_from(c.cislo()?).map_err(|_| Chyba::PoskodeneData("logický čas".into()))?,
            replika: c.cislo()?,
        })
    }
}

impl Kodovatelne for CielVlastnosti {
    fn zakoduj(&self, z: &mut Zapisovac) {
        match self {
            CielVlastnosti::Dielo => {
                z.u8(0);
            }
            CielVlastnosti::Graf(g) => {
                z.u8(1).meno(g);
            }
            CielVlastnosti::Uzol(g, u) => {
                z.u8(2).meno(g);
                u.zakoduj(z);
            }
        }
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        Ok(match c.u8()? {
            0 => CielVlastnosti::Dielo,
            1 => CielVlastnosti::Graf(c.meno()?),
            2 => CielVlastnosti::Uzol(c.meno()?, UzolId::dekoduj(c)?),
            d => return Err(Chyba::PoskodeneData(format!("cieľ vlastnosti {d}"))),
        })
    }
}

impl Kodovatelne for Uprava {
    fn zakoduj(&self, z: &mut Zapisovac) {
        match self {
            Uprava::VytvorGraf { graf } => {
                z.u8(0).meno(graf);
            }
            Uprava::OdstranGraf { graf } => {
                z.u8(1).meno(graf);
            }
            Uprava::PridajUzol { graf, uzol, druh } => {
                z.u8(2).meno(graf);
                uzol.zakoduj(z);
                druh.zakoduj(z);
            }
            Uprava::OdstranUzol { graf, uzol } => {
                z.u8(3).meno(graf);
                uzol.zakoduj(z);
            }
            Uprava::Spoj { graf, uzol, vstup, zdroj } => {
                z.u8(4).meno(graf);
                uzol.zakoduj(z);
                z.cislo(*vstup as u64);
                zakoduj_volitelny_zdroj(z, zdroj);
            }
            Uprava::NastavParameter { graf, uzol, meno, hodnota } => {
                z.u8(5).meno(graf);
                uzol.zakoduj(z);
                z.meno(meno);
                match hodnota {
                    Some(p) => {
                        z.u8(1);
                        p.zakoduj(z);
                    }
                    None => {
                        z.u8(0);
                    }
                }
            }
            Uprava::NastavObmedzenie { graf, uzol, zdroj } => {
                z.u8(6).meno(graf);
                uzol.zakoduj(z);
                zakoduj_volitelny_zdroj(z, zdroj);
            }
            Uprava::NastavVypnutie { graf, uzol, vypnuty } => {
                z.u8(7).meno(graf);
                uzol.zakoduj(z);
                z.logicka(*vypnuty);
            }
            Uprava::NastavVariant { graf, uzol, variant } => {
                z.u8(8).meno(graf);
                uzol.zakoduj(z);
                zakoduj_volitelne_meno(z, variant);
            }
            Uprava::NastavVstupGrafu { graf, index, popis } => {
                z.u8(9).meno(graf).cislo(*index as u64);
                match popis {
                    Some(p) => {
                        z.u8(1);
                        p.zakoduj(z);
                    }
                    None => {
                        z.u8(0);
                    }
                }
            }
            Uprava::NastavVystupGrafu { graf, meno, zdroj } => {
                z.u8(10).meno(graf).meno(meno);
                zakoduj_volitelny_zdroj(z, zdroj);
            }
            Uprava::NastavParameterGrafu { graf, popis } => {
                z.u8(11).meno(graf);
                popis.zakoduj(z);
            }
            Uprava::OdstranParameterGrafu { graf, meno } => {
                z.u8(12).meno(graf).meno(meno);
            }
            Uprava::NastavVariantGrafu { graf, variant, prepisania } => {
                z.u8(13).meno(graf).meno(variant);
                match prepisania {
                    Some(p) => {
                        z.u8(1);
                        zakoduj_prepisania(z, p);
                    }
                    None => {
                        z.u8(0);
                    }
                }
            }
            Uprava::NastavVlastnost { ciel, meno, hodnota } => {
                z.u8(14);
                ciel.zakoduj(z);
                z.meno(meno);
                match hodnota {
                    Some(h) => {
                        z.u8(1);
                        h.zakoduj(z);
                    }
                    None => {
                        z.u8(0);
                    }
                }
            }
        }
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        let cislo32 =
            |c: &mut Citac<'_>| u32::try_from(c.cislo()?).map_err(|_| Chyba::PoskodeneData("index".into()));
        fn volitelne<T>(
            c: &mut Citac<'_>,
            f: impl FnOnce(&mut Citac<'_>) -> Vysledok<T>,
        ) -> Vysledok<Option<T>> {
            match c.u8()? {
                0 => Ok(None),
                1 => Ok(Some(f(c)?)),
                _ => Err(Chyba::PoskodeneData("voliteľná hodnota".into())),
            }
        }
        Ok(match c.u8()? {
            0 => Uprava::VytvorGraf { graf: c.meno()? },
            1 => Uprava::OdstranGraf { graf: c.meno()? },
            2 => {
                Uprava::PridajUzol { graf: c.meno()?, uzol: UzolId::dekoduj(c)?, druh: DruhUzla::dekoduj(c)? }
            }
            3 => Uprava::OdstranUzol { graf: c.meno()?, uzol: UzolId::dekoduj(c)? },
            4 => Uprava::Spoj {
                graf: c.meno()?,
                uzol: UzolId::dekoduj(c)?,
                vstup: cislo32(c)?,
                zdroj: dekoduj_volitelny_zdroj(c)?,
            },
            5 => Uprava::NastavParameter {
                graf: c.meno()?,
                uzol: UzolId::dekoduj(c)?,
                meno: c.meno()?,
                hodnota: volitelne(c, Parameter::dekoduj)?,
            },
            6 => Uprava::NastavObmedzenie {
                graf: c.meno()?,
                uzol: UzolId::dekoduj(c)?,
                zdroj: dekoduj_volitelny_zdroj(c)?,
            },
            7 => Uprava::NastavVypnutie { graf: c.meno()?, uzol: UzolId::dekoduj(c)?, vypnuty: c.logicka()? },
            8 => Uprava::NastavVariant {
                graf: c.meno()?,
                uzol: UzolId::dekoduj(c)?,
                variant: dekoduj_volitelne_meno(c)?,
            },
            9 => Uprava::NastavVstupGrafu {
                graf: c.meno()?,
                index: cislo32(c)?,
                popis: volitelne(c, PopisPortu::dekoduj)?,
            },
            10 => Uprava::NastavVystupGrafu {
                graf: c.meno()?,
                meno: c.meno()?,
                zdroj: dekoduj_volitelny_zdroj(c)?,
            },
            11 => Uprava::NastavParameterGrafu { graf: c.meno()?, popis: PopisParametra::dekoduj(c)? },
            12 => Uprava::OdstranParameterGrafu { graf: c.meno()?, meno: c.meno()? },
            13 => Uprava::NastavVariantGrafu {
                graf: c.meno()?,
                variant: c.meno()?,
                prepisania: volitelne(c, dekoduj_prepisania)?,
            },
            14 => Uprava::NastavVlastnost {
                ciel: CielVlastnosti::dekoduj(c)?,
                meno: c.meno()?,
                hodnota: volitelne(c, Hodnota::dekoduj)?,
            },
            d => return Err(Chyba::PoskodeneData(format!("druh úpravy {d}"))),
        })
    }
}

impl Kodovatelne for Zmena {
    fn zakoduj(&self, z: &mut Zapisovac) {
        self.cas.zakoduj(z);
        z.meno(&self.povodca).text(&self.popis).cislo(self.upravy.len() as u64);
        for u in &self.upravy {
            u.zakoduj(z);
        }
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        let cas = Cas::dekoduj(c)?;
        let povodca = c.meno()?;
        let popis = c.text()?.to_string();
        let n = c.pocet(2)?;
        let mut upravy = Vec::with_capacity(n);
        for _ in 0..n {
            upravy.push(Uprava::dekoduj(c)?);
        }
        Ok(Zmena { cas, povodca, popis, upravy })
    }
}
