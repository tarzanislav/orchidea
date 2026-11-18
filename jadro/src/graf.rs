//! Graf – zloženie operácií (prvotný pojem 6), parameter (5) a odkaz (7).
//!
//! Pravidlá: S2 – graf je sám operácia (vnorenie bez obmedzenia hĺbky); S3 – čokoľvek sa dá použiť odkazom,
//! lokálne prepísať, pomenovať varianty a odkaz kedykoľvek odpojiť; S4 – každý parameter môže byť
//! výstupom inej operácie; S5 – každý uzol možno obmedziť výberom (maskou). Každý uzol sa dá vypnúť (A8).

use crate::chyba::{Chyba, Vysledok};
use crate::hodnota::{Hodnota, TypHodnoty};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::krivka::Krivka;
use crate::meno::Meno;
use crate::odtlacok::Odtlacok;
use crate::operacia::{PopisParametra, PopisPortu, TypVystupu};
use crate::prostredie::Prostredie;
use crate::vlastnost::Vlastnosti;
use crate::vyraz::Vyraz;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Najviac uzlov v jednom grafe.
pub const NAJVIAC_UZLOV: usize = 1 << 20;
/// Najviac vstupov jedného uzla.
pub const NAJVIAC_VSTUPOV: usize = 4096;

/// Identita uzla – jedinečná naprieč zariadeniami (replika = zariadenie alebo inštancia, číslo = poradie).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UzolId {
    pub replika: u64,
    pub cislo: u64,
}

impl UzolId {
    pub fn novy(replika: u64, cislo: u64) -> Self {
        UzolId { replika, cislo }
    }
}

/// Odkiaľ prichádza hodnota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Zdroj {
    /// Výstup uzla.
    Uzol { uzol: UzolId, vystup: u32 },
    /// Vstup samotného grafu.
    VstupGrafu(u32),
}

impl Zdroj {
    pub fn uzol(uzol: UzolId) -> Zdroj {
        Zdroj::Uzol { uzol, vystup: 0 }
    }
}

/// Hodnota parametra (pravidlo S4: každý parameter je rovnako slobodný).
#[derive(Debug, Clone, PartialEq)]
pub enum Parameter {
    Konstanta(Hodnota),
    /// Funkcia osi (napr. času).
    Krivka(Krivka),
    /// Výraz nad polohami vyhodnotenia a parametrami grafu.
    Vyraz(Vyraz),
    /// Výstup inej operácie – riadenie jedného média druhým.
    Vstup(Zdroj),
    /// Parameter grafu, v ktorom uzol je.
    Grafu(Meno),
}

/// Druh uzla.
#[derive(Debug, Clone, PartialEq)]
pub enum DruhUzla {
    /// Operácia z registra.
    Operacia { identita: Meno, verzia: u32 },
    /// Odkaz na graf v knižnici prostredia (zmena v knižnici sa prejaví všade, kde je odkaz).
    Odkaz { graf: Meno },
    /// Vlastná kópia grafu (napr. odpojený odkaz).
    Vnoreny(Arc<Graf>),
}

/// Uzol grafu.
#[derive(Debug, Clone, PartialEq)]
pub struct Uzol {
    pub druh: DruhUzla,
    pub vstupy: Vec<Option<Zdroj>>,
    /// Prepísané parametre (ostatné majú predvolenú hodnotu).
    pub parametre: BTreeMap<Meno, Parameter>,
    /// Pomenovaný variant odkazovaného grafu (sada prepísaní).
    pub variant: Option<Meno>,
    /// Obmedzenie výberom: maska 0–1, ktorá zmieša výsledok s pôvodným vstupom (S5).
    pub obmedzenie: Option<Zdroj>,
    /// Vypnutý uzol prepustí svoj prvý vstup bez zmeny.
    pub vypnuty: bool,
    pub vlastnosti: Vlastnosti,
}

impl Uzol {
    pub fn novy(druh: DruhUzla) -> Uzol {
        Uzol {
            druh,
            vstupy: Vec::new(),
            parametre: BTreeMap::new(),
            variant: None,
            obmedzenie: None,
            vypnuty: false,
            vlastnosti: Vlastnosti::nove(),
        }
    }

    pub fn operacia(identita: Meno, verzia: u32) -> Uzol {
        Uzol::novy(DruhUzla::Operacia { identita, verzia })
    }

    /// Všetky zdroje, od ktorých uzol závisí.
    pub fn zavislosti(&self) -> Vec<Zdroj> {
        let mut z: Vec<Zdroj> = self.vstupy.iter().flatten().copied().collect();
        for p in self.parametre.values() {
            if let Parameter::Vstup(s) = p {
                z.push(*s);
            }
        }
        if let Some(o) = self.obmedzenie {
            z.push(o);
        }
        z
    }
}

/// Graf.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Graf {
    pub uzly: BTreeMap<UzolId, Uzol>,
    pub vstupy: Vec<PopisPortu>,
    pub vystupy: Vec<(Meno, Option<Zdroj>)>,
    pub parametre: Vec<PopisParametra>,
    pub varianty: BTreeMap<Meno, BTreeMap<Meno, Parameter>>,
    pub vlastnosti: Vlastnosti,
}

impl Graf {
    pub fn novy() -> Graf {
        Graf::default()
    }

    /// Pridá uzol so zadanou identitou.
    pub fn pridaj(&mut self, id: UzolId, uzol: Uzol) -> Vysledok<UzolId> {
        if self.uzly.contains_key(&id) {
            return Err(Chyba::MimoRozsahu(format!("uzol {id:?} už existuje")));
        }
        if self.uzly.len() >= NAJVIAC_UZLOV {
            return Err(Chyba::Limit("príliš veľa uzlov v grafe".into()));
        }
        self.uzly.insert(id, uzol);
        Ok(id)
    }

    /// Odstráni uzol a všetky spojenia, ktoré naň viedli.
    pub fn odstran(&mut self, id: UzolId) -> Option<Uzol> {
        let u = self.uzly.remove(&id)?;
        let mieri = |z: &Zdroj| matches!(z, Zdroj::Uzol { uzol, .. } if *uzol == id);
        for ine in self.uzly.values_mut() {
            for v in ine.vstupy.iter_mut() {
                if v.as_ref().is_some_and(mieri) {
                    *v = None;
                }
            }
            if ine.obmedzenie.as_ref().is_some_and(mieri) {
                ine.obmedzenie = None;
            }
            ine.parametre.retain(|_, p| !matches!(p, Parameter::Vstup(z) if mieri(z)));
        }
        for (_, z) in self.vystupy.iter_mut() {
            if z.as_ref().is_some_and(mieri) {
                *z = None;
            }
        }
        Some(u)
    }

    pub fn uzol(&self, id: UzolId) -> Vysledok<&Uzol> {
        self.uzly.get(&id).ok_or_else(|| Chyba::Chybajuce(format!("uzol {id:?}")))
    }

    pub fn uzol_mut(&mut self, id: UzolId) -> Vysledok<&mut Uzol> {
        self.uzly.get_mut(&id).ok_or_else(|| Chyba::Chybajuce(format!("uzol {id:?}")))
    }

    /// Pripojí (alebo odpojí, ak `zdroj` je None) vstup uzla. Bez kontroly typov – tú robí
    /// [`Graf::spoj_overene`] alebo [`Graf::over`].
    pub fn spoj(&mut self, ciel: UzolId, vstup: usize, zdroj: Option<Zdroj>) -> Vysledok<()> {
        if vstup >= NAJVIAC_VSTUPOV {
            return Err(Chyba::Limit("príliš vysoké číslo vstupu".into()));
        }
        if let Some(Zdroj::Uzol { uzol, .. }) = zdroj
            && !self.uzly.contains_key(&uzol)
        {
            return Err(Chyba::Chybajuce(format!("zdrojový uzol {uzol:?}")));
        }
        let u = self.uzol_mut(ciel)?;
        if u.vstupy.len() <= vstup {
            u.vstupy.resize(vstup + 1, None);
        }
        u.vstupy[vstup] = zdroj;
        Ok(())
    }

    /// Pripojí vstup a overí typy (pravidlo S1). Pri nezhode navrhne prevodník z registra.
    pub fn spoj_overene(&mut self, p: &Prostredie, ciel: UzolId, vstup: usize, zdroj: Zdroj) -> Vysledok<()> {
        let ocakavany = p.typ_vstupu(self, ciel, vstup)?;
        let dodany = p.typ_zdroja(self, zdroj, 0)?;
        if !ocakavany.prijme(&dodany) {
            let navrh = p
                .operacie
                .najdi_prevodnik(&dodany, &ocakavany)
                .map(|o| format!("vlož prevodník {} (verzia {})", o.identita, o.verzia));
            return Err(Chyba::NezhodaTypov {
                popis: format!("vstup {vstup} čaká {}, zdroj dáva {}", ocakavany.popis(), dodany.popis()),
                navrh,
            });
        }
        self.spoj(ciel, vstup, Some(zdroj))
    }

    /// Nastaví výstup grafu (pomenovaný).
    pub fn nastav_vystup(&mut self, meno: Meno, zdroj: Option<Zdroj>) {
        match self.vystupy.iter_mut().find(|(m, _)| *m == meno) {
            Some((_, z)) => *z = zdroj,
            None => self.vystupy.push((meno, zdroj)),
        }
    }

    /// Odpojí odkaz: uzol dostane vlastnú kópiu grafu z knižnice so zachovanými prepísaniami (S3).
    pub fn odpoj_odkaz(&mut self, id: UzolId, p: &Prostredie) -> Vysledok<()> {
        let u = self.uzol(id)?;
        let DruhUzla::Odkaz { graf } = &u.druh else {
            return Err(Chyba::MimoRozsahu("uzol nie je odkaz".into()));
        };
        let kopia =
            p.grafy.get(graf).ok_or_else(|| Chyba::Chybajuce(format!("graf {graf} v knižnici")))?.clone();
        self.uzol_mut(id)?.druh = DruhUzla::Vnoreny(Arc::new((*kopia).clone()));
        Ok(())
    }

    /// Overí všetky spojenia grafu; vráti zoznam problémov (prázdny = v poriadku).
    pub fn over(&self, p: &Prostredie) -> Vec<Chyba> {
        let mut chyby = Vec::new();
        for (&id, u) in &self.uzly {
            for (i, z) in u.vstupy.iter().enumerate() {
                let Some(z) = z else { continue };
                let vysledok = p.typ_vstupu(self, id, i).and_then(|oc| {
                    let d = p.typ_zdroja(self, *z, 0)?;
                    if oc.prijme(&d) {
                        Ok(())
                    } else {
                        Err(Chyba::NezhodaTypov {
                            popis: format!(
                                "uzol {id:?}, vstup {i}: čaká {}, dostáva {}",
                                oc.popis(),
                                d.popis()
                            ),
                            navrh: p.operacie.najdi_prevodnik(&d, &oc).map(|o| o.identita.to_string()),
                        })
                    }
                });
                if let Err(e) = vysledok {
                    chyby.push(e);
                }
            }
        }
        if let Err(e) = self.poradie(&self.vystupy.iter().filter_map(|(_, z)| *z).collect::<Vec<_>>()) {
            chyby.push(e);
        }
        chyby
    }

    /// Topologické poradie uzlov potrebných pre dané zdroje (bez rekurzie, s odhalením cyklu).
    pub fn poradie(&self, ciele: &[Zdroj]) -> Vysledok<Vec<UzolId>> {
        #[derive(Clone, Copy, PartialEq)]
        enum Stav {
            Navstevuje,
            Hotovo,
        }
        let mut stav: BTreeMap<UzolId, Stav> = BTreeMap::new();
        let mut poradie = Vec::new();
        for ciel in ciele {
            let Zdroj::Uzol { uzol: zaciatok, .. } = *ciel else { continue };
            if stav.contains_key(&zaciatok) {
                continue;
            }
            if !self.uzly.contains_key(&zaciatok) {
                return Err(Chyba::Chybajuce(format!("uzol {zaciatok:?}, na ktorý vedie výstup")));
            }
            let mut zasobnik: Vec<(UzolId, Vec<Zdroj>, usize)> =
                vec![(zaciatok, self.uzol(zaciatok)?.zavislosti(), 0)];
            stav.insert(zaciatok, Stav::Navstevuje);
            while let Some((id, zavislosti, dalsi)) = zasobnik.last_mut() {
                let id = *id;
                if *dalsi < zavislosti.len() {
                    let z = zavislosti[*dalsi];
                    *dalsi += 1;
                    if let Zdroj::Uzol { uzol: d, .. } = z {
                        match stav.get(&d) {
                            Some(Stav::Navstevuje) => {
                                return Err(Chyba::Cyklus(format!("uzol {d:?} závisí sám od seba")));
                            }
                            Some(Stav::Hotovo) => {}
                            None => {
                                if !self.uzly.contains_key(&d) {
                                    return Err(Chyba::Chybajuce(format!(
                                        "uzol {d:?}, na ktorý vedie spojenie"
                                    )));
                                }
                                stav.insert(d, Stav::Navstevuje);
                                zasobnik.push((d, self.uzol(d)?.zavislosti(), 0));
                            }
                        }
                    }
                } else {
                    stav.insert(id, Stav::Hotovo);
                    poradie.push(id);
                    zasobnik.pop();
                }
            }
        }
        Ok(poradie)
    }

    /// Odtlačok celého grafu (identita obsahu).
    pub fn odtlacok(&self) -> Odtlacok {
        Odtlacok::z(&self.do_bajtov())
    }
}

/// Typ výstupu operácie po dosadení typov vstupov.
pub(crate) fn rozlis_typ_vystupu(
    t: &TypVystupu,
    typy_vstupov: impl Fn(usize) -> Vysledok<TypHodnoty>,
) -> Vysledok<TypHodnoty> {
    match t {
        TypVystupu::Pevny(t) => Ok(t.clone()),
        // „ako vstup“ = rovnaký druh hodnoty a rovnaké osi; typ prvku sa smie zmeniť
        TypVystupu::AkoVstup(i) => Ok(bez_typu_prvku(typy_vstupov(*i as usize)?)),
    }
}

/// Typ hodnoty bez požiadavky na typ prvku signálu.
pub(crate) fn bez_typu_prvku(t: TypHodnoty) -> TypHodnoty {
    match t {
        TypHodnoty::Signal(mut p) => {
            p.typ = None;
            TypHodnoty::Signal(p)
        }
        ine => ine,
    }
}

// ---------- kódovanie ----------

impl Kodovatelne for UzolId {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.cislo(self.replika).cislo(self.cislo);
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        Ok(UzolId { replika: c.cislo()?, cislo: c.cislo()? })
    }
}

impl Kodovatelne for Zdroj {
    fn zakoduj(&self, z: &mut Zapisovac) {
        match self {
            Zdroj::Uzol { uzol, vystup } => {
                z.u8(0);
                uzol.zakoduj(z);
                z.cislo(*vystup as u64);
            }
            Zdroj::VstupGrafu(i) => {
                z.u8(1).cislo(*i as u64);
            }
        }
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        let cislo32 =
            |c: &mut Citac<'_>| u32::try_from(c.cislo()?).map_err(|_| Chyba::PoskodeneData("index".into()));
        match c.u8()? {
            0 => {
                let uzol = UzolId::dekoduj(c)?;
                Ok(Zdroj::Uzol { uzol, vystup: cislo32(c)? })
            }
            1 => Ok(Zdroj::VstupGrafu(cislo32(c)?)),
            d => Err(Chyba::PoskodeneData(format!("druh zdroja {d}"))),
        }
    }
}

pub(crate) fn zakoduj_volitelny_zdroj(z: &mut Zapisovac, zdroj: &Option<Zdroj>) {
    match zdroj {
        Some(s) => {
            z.u8(1);
            s.zakoduj(z);
        }
        None => {
            z.u8(0);
        }
    }
}

pub(crate) fn dekoduj_volitelny_zdroj(c: &mut Citac<'_>) -> Vysledok<Option<Zdroj>> {
    match c.u8()? {
        0 => Ok(None),
        1 => Ok(Some(Zdroj::dekoduj(c)?)),
        _ => Err(Chyba::PoskodeneData("voliteľný zdroj".into())),
    }
}

pub(crate) fn zakoduj_volitelne_meno(z: &mut Zapisovac, m: &Option<Meno>) {
    match m {
        Some(m) => z.u8(1).meno(m),
        None => z.u8(0),
    };
}

pub(crate) fn dekoduj_volitelne_meno(c: &mut Citac<'_>) -> Vysledok<Option<Meno>> {
    match c.u8()? {
        0 => Ok(None),
        1 => Ok(Some(c.meno()?)),
        _ => Err(Chyba::PoskodeneData("voliteľné meno".into())),
    }
}

impl Kodovatelne for Parameter {
    fn zakoduj(&self, z: &mut Zapisovac) {
        match self {
            Parameter::Konstanta(h) => {
                z.u8(0);
                h.zakoduj(z);
            }
            Parameter::Krivka(k) => {
                z.u8(1);
                k.zakoduj(z);
            }
            Parameter::Vyraz(v) => {
                z.u8(2);
                v.zakoduj(z);
            }
            Parameter::Vstup(s) => {
                z.u8(3);
                s.zakoduj(z);
            }
            Parameter::Grafu(m) => {
                z.u8(4).meno(m);
            }
        }
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        Ok(match c.u8()? {
            0 => Parameter::Konstanta(Hodnota::dekoduj(c)?),
            1 => Parameter::Krivka(Krivka::dekoduj(c)?),
            2 => Parameter::Vyraz(Vyraz::dekoduj(c)?),
            3 => Parameter::Vstup(Zdroj::dekoduj(c)?),
            4 => Parameter::Grafu(c.meno()?),
            d => return Err(Chyba::PoskodeneData(format!("druh parametra {d}"))),
        })
    }
}

pub(crate) fn zakoduj_prepisania(z: &mut Zapisovac, p: &BTreeMap<Meno, Parameter>) {
    z.cislo(p.len() as u64);
    for (k, v) in p {
        z.meno(k);
        v.zakoduj(z);
    }
}

pub(crate) fn dekoduj_prepisania(c: &mut Citac<'_>) -> Vysledok<BTreeMap<Meno, Parameter>> {
    let n = c.pocet(3)?;
    let mut m = BTreeMap::new();
    for _ in 0..n {
        let k = c.meno()?;
        if m.insert(k.clone(), Parameter::dekoduj(c)?).is_some() {
            return Err(Chyba::PoskodeneData(format!("parameter {k} sa opakuje")));
        }
    }
    Ok(m)
}

impl Kodovatelne for DruhUzla {
    fn zakoduj(&self, z: &mut Zapisovac) {
        match self {
            DruhUzla::Operacia { identita, verzia } => {
                z.u8(0).meno(identita).cislo(*verzia as u64);
            }
            DruhUzla::Odkaz { graf } => {
                z.u8(1).meno(graf);
            }
            DruhUzla::Vnoreny(g) => {
                z.u8(2);
                g.zakoduj(z);
            }
        }
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        Ok(match c.u8()? {
            0 => {
                let identita = c.meno()?;
                let verzia = u32::try_from(c.cislo()?).map_err(|_| Chyba::PoskodeneData("verzia".into()))?;
                DruhUzla::Operacia { identita, verzia }
            }
            1 => DruhUzla::Odkaz { graf: c.meno()? },
            2 => DruhUzla::Vnoreny(Arc::new(Graf::dekoduj(c)?)),
            d => return Err(Chyba::PoskodeneData(format!("druh uzla {d}"))),
        })
    }
}

impl Kodovatelne for Uzol {
    fn zakoduj(&self, z: &mut Zapisovac) {
        self.druh.zakoduj(z);
        z.cislo(self.vstupy.len() as u64);
        for v in &self.vstupy {
            zakoduj_volitelny_zdroj(z, v);
        }
        zakoduj_prepisania(z, &self.parametre);
        zakoduj_volitelne_meno(z, &self.variant);
        zakoduj_volitelny_zdroj(z, &self.obmedzenie);
        z.logicka(self.vypnuty);
        self.vlastnosti.zakoduj(z);
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        let druh = DruhUzla::dekoduj(c)?;
        let n = c.pocet(1)?;
        if n > NAJVIAC_VSTUPOV {
            return Err(Chyba::Limit("príliš veľa vstupov uzla".into()));
        }
        let mut vstupy = Vec::with_capacity(n);
        for _ in 0..n {
            vstupy.push(dekoduj_volitelny_zdroj(c)?);
        }
        Ok(Uzol {
            druh,
            vstupy,
            parametre: dekoduj_prepisania(c)?,
            variant: dekoduj_volitelne_meno(c)?,
            obmedzenie: dekoduj_volitelny_zdroj(c)?,
            vypnuty: c.logicka()?,
            vlastnosti: Vlastnosti::dekoduj(c)?,
        })
    }
}

impl Kodovatelne for Graf {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.cislo(self.uzly.len() as u64);
        for (id, u) in &self.uzly {
            id.zakoduj(z);
            u.zakoduj(z);
        }
        z.cislo(self.vstupy.len() as u64);
        for v in &self.vstupy {
            v.zakoduj(z);
        }
        z.cislo(self.vystupy.len() as u64);
        for (m, s) in &self.vystupy {
            z.meno(m);
            zakoduj_volitelny_zdroj(z, s);
        }
        z.cislo(self.parametre.len() as u64);
        for p in &self.parametre {
            p.zakoduj(z);
        }
        z.cislo(self.varianty.len() as u64);
        for (m, p) in &self.varianty {
            z.meno(m);
            zakoduj_prepisania(z, p);
        }
        self.vlastnosti.zakoduj(z);
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        c.vnor()?;
        let mut g = Graf::novy();
        let n = c.pocet(4)?;
        if n > NAJVIAC_UZLOV {
            return Err(Chyba::Limit("príliš veľa uzlov".into()));
        }
        for _ in 0..n {
            let id = UzolId::dekoduj(c)?;
            let u = Uzol::dekoduj(c)?;
            if g.uzly.insert(id, u).is_some() {
                return Err(Chyba::PoskodeneData(format!("uzol {id:?} sa opakuje")));
            }
        }
        for _ in 0..c.pocet(2)? {
            g.vstupy.push(PopisPortu::dekoduj(c)?);
        }
        for _ in 0..c.pocet(2)? {
            let m = c.meno()?;
            g.vystupy.push((m, dekoduj_volitelny_zdroj(c)?));
        }
        for _ in 0..c.pocet(4)? {
            g.parametre.push(PopisParametra::dekoduj(c)?);
        }
        for _ in 0..c.pocet(2)? {
            let m = c.meno()?;
            g.varianty.insert(m, dekoduj_prepisania(c)?);
        }
        g.vlastnosti = Vlastnosti::dekoduj(c)?;
        c.vynor();
        Ok(g)
    }
}
