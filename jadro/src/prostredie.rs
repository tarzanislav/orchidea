//! Prostredie a vyhodnocovanie grafov.
//!
//! Prostredie spája register operácií, knižnicu grafov (pre odkazy) a register osí. Vyhodnocovač počíta
//! len uzly, ktoré výstupy naozaj potrebujú, overuje typy a parametre, dodržiava zmluvu operácie
//! (počet a typy výstupov) a zdieľa výsledky cez pamäť podľa odtlačku obsahu – deterministické operácie
//! sa s rovnakými vstupmi nepočítajú dvakrát.

use crate::chyba::{Chyba, Vysledok};
use crate::graf::{DruhUzla, Graf, Parameter, UzolId, Zdroj, rozlis_typ_vystupu};
use crate::hodnota::{Hodnota, TypHodnoty};
use crate::meno::Meno;
use crate::oblast::Oblast;
use crate::odtlacok::{Hasovac, Odtlacok};
use crate::operacia::{Argumenty, KontextVypoctu, Operacia, PopisOperacie, RegisterOperacii, TypVystupu};
use crate::os::RegisterOsi;
use crate::signal::{Signal, TypPrvku};
use crate::vyber::Vyber;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Najväčšia hĺbka vnorenia grafov a odkazov.
pub const NAJVACSIA_HLBKA: u32 = 64;

#[derive(Debug)]
struct Pamat {
    mapa: HashMap<Odtlacok, Arc<Vec<Hodnota>>>,
    poradie: VecDeque<Odtlacok>,
    kapacita: usize,
    zasahy: u64,
    minutia: u64,
}

/// Štatistika zdieľanej pamäte výsledkov.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatistikaPamate {
    pub zasahy: u64,
    pub minutia: u64,
    pub zaznamov: usize,
}

/// Prostredie výpočtu.
#[derive(Debug)]
pub struct Prostredie {
    pub operacie: RegisterOperacii,
    /// Knižnica grafov, na ktoré môžu ukazovať odkazy.
    pub grafy: BTreeMap<Meno, Arc<Graf>>,
    pub osi: RegisterOsi,
    pamat: Mutex<Pamat>,
}

enum PopisUzla<'a> {
    Op(&'a Arc<dyn Operacia>),
    Graf(Arc<Graf>),
}

struct Rozsah<'a> {
    graf: &'a Graf,
    vstupy: &'a [Hodnota],
    parametre: &'a BTreeMap<Meno, Hodnota>,
    polohy: &'a BTreeMap<Meno, f64>,
    hlbka: u32,
}

impl Prostredie {
    pub fn nove(operacie: RegisterOperacii) -> Self {
        Prostredie {
            operacie,
            grafy: BTreeMap::new(),
            osi: RegisterOsi::zakladny(),
            pamat: Mutex::new(Pamat {
                mapa: HashMap::new(),
                poradie: VecDeque::new(),
                kapacita: 4096,
                zasahy: 0,
                minutia: 0,
            }),
        }
    }

    /// Prostredie so základnými operáciami jadra.
    pub fn zakladne() -> Self {
        Self::nove(crate::zakladne::register())
    }

    pub fn statistika_pamate(&self) -> StatistikaPamate {
        let p = self.pamat.lock().unwrap_or_else(|e| e.into_inner());
        StatistikaPamate { zasahy: p.zasahy, minutia: p.minutia, zaznamov: p.mapa.len() }
    }

    pub fn vycisti_pamat(&self) {
        let mut p = self.pamat.lock().unwrap_or_else(|e| e.into_inner());
        p.mapa.clear();
        p.poradie.clear();
    }

    /// Nastaví počet zapamätaných výsledkov (0 = pamäť vypnutá).
    pub fn nastav_kapacitu_pamate(&self, kapacita: usize) {
        let mut p = self.pamat.lock().unwrap_or_else(|e| e.into_inner());
        p.kapacita = kapacita;
        while p.mapa.len() > kapacita {
            if let Some(k) = p.poradie.pop_front() {
                p.mapa.remove(&k);
            }
        }
    }

    fn z_pamate(&self, k: &Odtlacok) -> Option<Arc<Vec<Hodnota>>> {
        let mut p = self.pamat.lock().unwrap_or_else(|e| e.into_inner());
        let v = p.mapa.get(k).cloned();
        if v.is_some() {
            p.zasahy += 1;
        } else {
            p.minutia += 1;
        }
        v
    }

    fn do_pamate(&self, k: Odtlacok, v: Arc<Vec<Hodnota>>) {
        let mut p = self.pamat.lock().unwrap_or_else(|e| e.into_inner());
        if p.kapacita == 0 {
            return;
        }
        if p.mapa.insert(k, v).is_none() {
            p.poradie.push_back(k);
        }
        while p.mapa.len() > p.kapacita {
            if let Some(stary) = p.poradie.pop_front() {
                p.mapa.remove(&stary);
            } else {
                break;
            }
        }
    }

    /// Vyhodnotí graf: vstupy, parametre grafu a polohy (napr. „t“ → 2,5).
    pub fn vyhodnot(
        &self,
        graf: &Graf,
        vstupy: &[Hodnota],
        parametre: &BTreeMap<Meno, Hodnota>,
        polohy: &BTreeMap<Meno, f64>,
    ) -> Vysledok<Vec<Hodnota>> {
        self.vyhodnot_graf(graf, vstupy, parametre, polohy, 0)
    }

    /// Overí vstupy a parametre grafu; vráti hodnoty parametrov a zdroje výstupov.
    fn priprav(
        &self,
        graf: &Graf,
        vstupy: &[Hodnota],
        dane: &BTreeMap<Meno, Hodnota>,
        hlbka: u32,
    ) -> Vysledok<(BTreeMap<Meno, Hodnota>, Vec<Zdroj>)> {
        if hlbka > NAJVACSIA_HLBKA {
            return Err(Chyba::Limit(format!("grafy sú vnorené hlbšie ako {NAJVACSIA_HLBKA}")));
        }
        if vstupy.len() != graf.vstupy.len() {
            return Err(Chyba::NezhodaTvaru(format!(
                "graf čaká {} vstupov, dostal {}",
                graf.vstupy.len(),
                vstupy.len()
            )));
        }
        for (p, h) in graf.vstupy.iter().zip(vstupy) {
            if !p.typ.splna(h) {
                return Err(Chyba::NezhodaTypov {
                    popis: format!(
                        "vstup grafu {} čaká {}, dostal {}",
                        p.meno,
                        p.typ.popis(),
                        h.typ().popis()
                    ),
                    navrh: None,
                });
            }
        }
        if let Some(nezname) = dane.keys().find(|k| !graf.parametre.iter().any(|p| &p.meno == *k)) {
            return Err(Chyba::Chybajuce(format!("graf nemá parameter {nezname}")));
        }
        let mut parametre = BTreeMap::new();
        for p in &graf.parametre {
            let h = dane.get(&p.meno).cloned().unwrap_or_else(|| p.predvolena.clone());
            p.over(&h)?;
            parametre.insert(p.meno.clone(), h);
        }
        let mut ciele = Vec::with_capacity(graf.vystupy.len());
        for (m, z) in &graf.vystupy {
            ciele.push(z.ok_or_else(|| Chyba::Chybajuce(format!("výstup grafu {m} nie je pripojený")))?);
        }
        Ok((parametre, ciele))
    }

    fn vyhodnot_graf(
        &self,
        graf: &Graf,
        vstupy: &[Hodnota],
        dane: &BTreeMap<Meno, Hodnota>,
        polohy: &BTreeMap<Meno, f64>,
        hlbka: u32,
    ) -> Vysledok<Vec<Hodnota>> {
        let (parametre, ciele) = self.priprav(graf, vstupy, dane, hlbka)?;
        let r = Rozsah { graf, vstupy, parametre: &parametre, polohy, hlbka };
        let mut vysledky: BTreeMap<UzolId, Arc<Vec<Hodnota>>> = BTreeMap::new();
        for id in graf.poradie(&ciele)? {
            let v = self.vyhodnot_uzol(&r, id, &vysledky)?;
            vysledky.insert(id, v);
        }
        ciele.iter().map(|z| hodnota_zdroja(&r, &vysledky, *z)).collect()
    }

    /// Vyhodnotí graf len pre oblasť výstupov (napr. viditeľný výrez obrazu alebo prehrávaný úsek zvuku).
    ///
    /// Žiadosť o oblasť sa šíri od výstupov k vstupom: operácia, ktorá to dovoľuje
    /// ([`Operacia::oblast_vstupu`]), povie, akú časť vstupov potrebuje; ostatné uzly sa spočítajú celé.
    /// Keď uzol potrebuje viac spotrebiteľov, spočíta sa raz pre obal ich oblastí. Výsledok je bitovo
    /// rovnaký ako výrez z úplného vyhodnotenia (signály sa obmedzia na dostupné dáta).
    pub fn vyhodnot_oblast(
        &self,
        graf: &Graf,
        vstupy: &[Hodnota],
        dane: &BTreeMap<Meno, Hodnota>,
        polohy: &BTreeMap<Meno, f64>,
        oblast: &Oblast,
    ) -> Vysledok<Vec<Hodnota>> {
        let (parametre, ciele) = self.priprav(graf, vstupy, dane, 0)?;
        let r = Rozsah { graf, vstupy, parametre: &parametre, polohy, hlbka: 0 };
        let poradie = graf.poradie(&ciele)?;

        // 1. žiadosti od výstupov k vstupom: uzol → oblasť (None = celý)
        let mut ziadosti: BTreeMap<UzolId, Option<Oblast>> = BTreeMap::new();
        fn ziadaj(z: &mut BTreeMap<UzolId, Option<Oblast>>, zdroj: Zdroj, o: Option<Oblast>) {
            if let Zdroj::Uzol { uzol, .. } = zdroj {
                let nova = match (z.get(&uzol), o) {
                    (None, o) => o,
                    (Some(Some(a)), Some(b)) => Some(a.zjednot(&b)),
                    _ => None,
                };
                z.insert(uzol, nova);
            }
        }
        for c in &ciele {
            ziadaj(&mut ziadosti, *c, Some(oblast.clone()));
        }
        let prazdne = BTreeMap::new();
        let mut plan: BTreeMap<UzolId, Vec<Hodnota>> = BTreeMap::new();
        for id in poradie.iter().rev() {
            let u = graf.uzol(*id)?;
            let ziadost = ziadosti.get(id).cloned().flatten();
            let schopna = match (&ziadost, self.popis_uzla(&u.druh)?) {
                (Some(_), PopisUzla::Op(op)) if !u.vypnuty && u.obmedzenie.is_none() => {
                    self.parametre_bez_uzlov(&r, &prazdne, u, op.popis())?.map(|p| (op.clone(), p))
                }
                _ => None,
            };
            match (schopna, ziadost) {
                (Some((op, par)), Some(o)) => {
                    for (i, z) in u.vstupy.iter().enumerate() {
                        if let Some(z) = z {
                            ziadaj(&mut ziadosti, *z, op.oblast_vstupu(&par, &o, i));
                        }
                    }
                    plan.insert(*id, par);
                }
                _ => {
                    for z in u.vstupy.iter().flatten().chain(u.obmedzenie.iter()) {
                        ziadaj(&mut ziadosti, *z, None);
                    }
                    for p in u.parametre.values() {
                        if let Parameter::Vstup(z) = p {
                            ziadaj(&mut ziadosti, *z, None);
                        }
                    }
                    if let Some(sada) = u.variant.as_ref().and_then(|v| match self.popis_uzla(&u.druh) {
                        Ok(PopisUzla::Graf(g)) => g.varianty.get(v).cloned(),
                        _ => None,
                    }) {
                        for p in sada.values() {
                            if let Parameter::Vstup(z) = p {
                                ziadaj(&mut ziadosti, *z, None);
                            }
                        }
                    }
                }
            }
        }

        // 2. výpočet od vstupov k výstupom; pri každom uzle si pamätáme, akú oblasť pokrýva
        let mut vysledky: BTreeMap<UzolId, Arc<Vec<Hodnota>>> = BTreeMap::new();
        let mut pokrytie: BTreeMap<UzolId, Oblast> = BTreeMap::new();
        let pokrytie_zdroja = |p: &BTreeMap<UzolId, Oblast>, z: &Zdroj| match z {
            Zdroj::Uzol { uzol, .. } => p.get(uzol).cloned(),
            Zdroj::VstupGrafu(_) => None,
        };
        for id in &poradie {
            let (Some(parametre), Some(Some(o))) = (plan.get(id), ziadosti.get(id)) else {
                let v = self.vyhodnot_uzol(&r, *id, &vysledky)?;
                vysledky.insert(*id, v);
                continue;
            };
            let u = graf.uzol(*id)?;
            let PopisUzla::Op(op) = self.popis_uzla(&u.druh)? else {
                return Err(Chyba::Vypocet("plán oblasti pre uzol, ktorý nie je operácia".into()));
            };
            let mut vstupy = Vec::with_capacity(u.vstupy.len());
            let mut pokryte = Oblast::cela();
            for (i, z) in u.vstupy.iter().enumerate() {
                let Some(z) = z else {
                    vstupy.push(Hodnota::Nic);
                    continue;
                };
                let h = hodnota_zdroja(&r, &vysledky, *z)?;
                match (h, op.oblast_vstupu(parametre, o, i)) {
                    (Hodnota::Signal(s), Some(potrebna)) => {
                        let (v, skutocna) = potrebna.vyrez(&s, pokrytie_zdroja(&pokrytie, z).as_ref())?;
                        for (os, rozsah) in skutocna.osi() {
                            if pokryte.os(os.ako_text()).is_none() {
                                pokryte = pokryte.s_osou(os.clone(), rozsah.0, rozsah.1)?;
                            }
                        }
                        vstupy.push(Hodnota::Signal(v));
                    }
                    (h, _) => vstupy.push(h),
                }
            }
            let out = self.vypocitaj_operaciu(&r, *id, op, vstupy, parametre.clone())?;
            vysledky.insert(*id, out);
            pokrytie.insert(*id, pokryte);
        }
        ciele
            .iter()
            .map(|z| match hodnota_zdroja(&r, &vysledky, *z)? {
                Hodnota::Signal(s) => {
                    Ok(Hodnota::Signal(oblast.vyrez(&s, pokrytie_zdroja(&pokrytie, z).as_ref())?.0))
                }
                h => Ok(h),
            })
            .collect()
    }

    /// Hodnoty parametrov uzla, ak nezávisia od výsledkov iných uzlov (inak `None`).
    fn parametre_bez_uzlov(
        &self,
        r: &Rozsah<'_>,
        prazdne: &BTreeMap<UzolId, Arc<Vec<Hodnota>>>,
        u: &crate::graf::Uzol,
        popis: &PopisOperacie,
    ) -> Vysledok<Option<Vec<Hodnota>>> {
        if u.parametre.values().any(|p| matches!(p, Parameter::Vstup(_))) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(popis.parametre.len());
        for p in &popis.parametre {
            let h = match u.parametre.get(&p.meno) {
                Some(par) => rozlis_parameter(r, prazdne, par)?,
                None => p.predvolena.clone(),
            };
            p.over(&h)?;
            out.push(h);
        }
        Ok(Some(out))
    }

    fn popis_uzla(&self, druh: &DruhUzla) -> Vysledok<PopisUzla<'_>> {
        match druh {
            DruhUzla::Operacia { identita, verzia } => {
                self.operacie.najdi(identita, *verzia).map(PopisUzla::Op).ok_or_else(|| {
                    Chyba::Chybajuce(format!("operácia {identita} verzie {verzia} v registri"))
                })
            }
            DruhUzla::Odkaz { graf } => self
                .grafy
                .get(graf)
                .cloned()
                .map(PopisUzla::Graf)
                .ok_or_else(|| Chyba::Chybajuce(format!("graf {graf} v knižnici"))),
            DruhUzla::Vnoreny(g) => Ok(PopisUzla::Graf(g.clone())),
        }
    }

    /// Vstupy sú už overené a doplnené; výsledok sa hľadá a ukladá v pamäti podľa obsahu.
    fn vypocitaj_operaciu(
        &self,
        r: &Rozsah<'_>,
        id: UzolId,
        op: &Arc<dyn Operacia>,
        vstupy: Vec<Hodnota>,
        parametre: Vec<Hodnota>,
    ) -> Vysledok<Arc<Vec<Hodnota>>> {
        let popis = op.popis();
        let vstupy = dopln_vstupy(vstupy, popis.vstupy.len(), &popis.identita)?;
        let kluc = kluc_pamate(popis, &vstupy, &parametre, r.polohy);
        if let Some(v) = self.z_pamate(&kluc) {
            return Ok(v);
        }
        let k = KontextVypoctu { prostredie: self, polohy: r.polohy, hlbka: r.hlbka };
        let a = Argumenty { vstupy: &vstupy, parametre: &parametre, popis };
        let out = op.vypocitaj(&k, &a).map_err(|e| match e {
            Chyba::Vypocet(s) => Chyba::Vypocet(format!("uzol {}/{}: {s}", id.replika, id.cislo)),
            ine => ine,
        })?;
        over_vystupy(popis, &vstupy, &out)?;
        let out = Arc::new(out);
        self.do_pamate(kluc, out.clone());
        Ok(out)
    }

    fn vyhodnot_uzol(
        &self,
        r: &Rozsah<'_>,
        id: UzolId,
        vysledky: &BTreeMap<UzolId, Arc<Vec<Hodnota>>>,
    ) -> Vysledok<Arc<Vec<Hodnota>>> {
        let u = r.graf.uzol(id)?;
        let vstupy: Vec<Hodnota> = u
            .vstupy
            .iter()
            .map(|z| match z {
                Some(z) => hodnota_zdroja(r, vysledky, *z),
                None => Ok(Hodnota::Nic),
            })
            .collect::<Vysledok<_>>()?;
        let vystupy = match self.popis_uzla(&u.druh)? {
            PopisUzla::Op(op) => {
                let popis = op.popis();
                if u.vypnuty {
                    return prepust(vstupy, popis.vystupy.len());
                }
                let vstupy = dopln_vstupy(vstupy, popis.vstupy.len(), &popis.identita)?;
                for (i, (p, h)) in popis.vstupy.iter().zip(&vstupy).enumerate() {
                    if !p.typ.splna(h) {
                        return Err(Chyba::NezhodaTypov {
                            popis: format!(
                                "vstup {i} ({}) operácie {} čaká {}, dostal {}",
                                p.meno,
                                popis.identita,
                                p.typ.popis(),
                                h.typ().popis()
                            ),
                            navrh: None,
                        });
                    }
                }
                if let Some(nezname) =
                    u.parametre.keys().find(|k| !popis.parametre.iter().any(|p| &p.meno == *k))
                {
                    return Err(Chyba::Chybajuce(format!(
                        "operácia {} nemá parameter {nezname}",
                        popis.identita
                    )));
                }
                let mut parametre = Vec::with_capacity(popis.parametre.len());
                for p in &popis.parametre {
                    let h = match u.parametre.get(&p.meno) {
                        Some(par) => rozlis_parameter(r, vysledky, par)?,
                        None => p.predvolena.clone(),
                    };
                    p.over(&h)?;
                    parametre.push(h);
                }
                let vysledok = self.vypocitaj_operaciu(r, id, op, vstupy.clone(), parametre)?;
                obmedz(r, vysledky, u.obmedzenie, &vstupy, vysledok)?
            }
            PopisUzla::Graf(g) => {
                if u.vypnuty {
                    return prepust(vstupy, g.vystupy.len());
                }
                let vstupy = dopln_vstupy(vstupy, g.vstupy.len(), &Meno::nove("vnorený graf")?)?;
                let mut prepisania: BTreeMap<Meno, Hodnota> = BTreeMap::new();
                if let Some(v) = &u.variant {
                    let sada = g.varianty.get(v).ok_or_else(|| Chyba::Chybajuce(format!("variant {v}")))?;
                    for (k, p) in sada {
                        prepisania.insert(k.clone(), rozlis_parameter(r, vysledky, p)?);
                    }
                }
                for (k, p) in &u.parametre {
                    prepisania.insert(k.clone(), rozlis_parameter(r, vysledky, p)?);
                }
                let out = self.vyhodnot_graf(&g, &vstupy, &prepisania, r.polohy, r.hlbka + 1)?;
                obmedz(r, vysledky, u.obmedzenie, &vstupy, Arc::new(out))?
            }
        };
        Ok(vystupy)
    }

    /// Očakávaný typ vstupu uzla.
    pub fn typ_vstupu(&self, graf: &Graf, uzol: UzolId, vstup: usize) -> Vysledok<TypHodnoty> {
        let u = graf.uzol(uzol)?;
        let typ = match self.popis_uzla(&u.druh)? {
            PopisUzla::Op(op) => op.popis().vstupy.get(vstup).map(|p| p.typ.clone()),
            PopisUzla::Graf(g) => g.vstupy.get(vstup).map(|p| p.typ.clone()),
        };
        typ.ok_or_else(|| Chyba::Chybajuce(format!("vstup {vstup} uzla {uzol:?}")))
    }

    /// Typ hodnoty, ktorú dáva zdroj (bez výpočtu).
    pub fn typ_zdroja(&self, graf: &Graf, zdroj: Zdroj, hlbka: u32) -> Vysledok<TypHodnoty> {
        if hlbka > 256 {
            return Err(Chyba::Limit("príliš dlhá reťaz typov".into()));
        }
        match zdroj {
            Zdroj::VstupGrafu(i) => graf
                .vstupy
                .get(i as usize)
                .map(|p| p.typ.clone())
                .ok_or_else(|| Chyba::Chybajuce(format!("vstup grafu {i}"))),
            Zdroj::Uzol { uzol, vystup } => {
                let u = graf.uzol(uzol)?;
                match self.popis_uzla(&u.druh)? {
                    PopisUzla::Op(op) => {
                        let p = op.popis();
                        let v = p.vystupy.get(vystup as usize).ok_or_else(|| {
                            Chyba::Chybajuce(format!("výstup {vystup} operácie {}", p.identita))
                        })?;
                        rozlis_typ_vystupu(&v.typ, |j| match u.vstupy.get(j).copied().flatten() {
                            Some(z) => self.typ_zdroja(graf, z, hlbka + 1),
                            None => {
                                Ok(p.vstupy.get(j).map(|x| x.typ.clone()).unwrap_or(TypHodnoty::Lubovolny))
                            }
                        })
                    }
                    PopisUzla::Graf(_) => Ok(TypHodnoty::Lubovolny),
                }
            }
        }
    }
}

fn hodnota_zdroja(
    r: &Rozsah<'_>,
    vysledky: &BTreeMap<UzolId, Arc<Vec<Hodnota>>>,
    z: Zdroj,
) -> Vysledok<Hodnota> {
    match z {
        Zdroj::VstupGrafu(i) => {
            r.vstupy.get(i as usize).cloned().ok_or_else(|| Chyba::Chybajuce(format!("vstup grafu {i}")))
        }
        Zdroj::Uzol { uzol, vystup } => vysledky
            .get(&uzol)
            .and_then(|v| v.get(vystup as usize))
            .cloned()
            .ok_or_else(|| Chyba::Chybajuce(format!("výstup {vystup} uzla {uzol:?}"))),
    }
}

fn rozlis_parameter(
    r: &Rozsah<'_>,
    vysledky: &BTreeMap<UzolId, Arc<Vec<Hodnota>>>,
    p: &Parameter,
) -> Vysledok<Hodnota> {
    Ok(match p {
        Parameter::Konstanta(h) => h.clone(),
        Parameter::Krivka(k) => {
            let poloha = r
                .polohy
                .get(&k.os)
                .ok_or_else(|| Chyba::Chybajuce(format!("poloha osi {} pre krivku", k.os)))?;
            Hodnota::Cislo(k.hodnota(*poloha))
        }
        Parameter::Vyraz(v) => Hodnota::Cislo(v.vyhodnot(&|n| {
            r.polohy.get(n).copied().or_else(|| r.parametre.get(n).and_then(Hodnota::ako_cislo))
        })?),
        Parameter::Vstup(z) => hodnota_zdroja(r, vysledky, *z)?,
        Parameter::Grafu(m) => {
            r.parametre.get(m).cloned().ok_or_else(|| Chyba::Chybajuce(format!("parameter grafu {m}")))?
        }
    })
}

fn dopln_vstupy(mut v: Vec<Hodnota>, n: usize, kto: &Meno) -> Vysledok<Vec<Hodnota>> {
    if v.len() > n {
        if v[n..].iter().any(|h| *h != Hodnota::Nic) {
            return Err(Chyba::NezhodaTvaru(format!("{kto} má len {n} vstupov, pripojených je viac")));
        }
        v.truncate(n);
    }
    v.resize(n, Hodnota::Nic);
    Ok(v)
}

fn prepust(vstupy: Vec<Hodnota>, vystupov: usize) -> Vysledok<Arc<Vec<Hodnota>>> {
    let prvy = vstupy
        .into_iter()
        .next()
        .ok_or_else(|| Chyba::MimoRozsahu("vypnutý uzol nemá vstup, ktorý by prepustil".into()))?;
    let mut out = vec![Hodnota::Nic; vystupov.max(1)];
    out[0] = prvy;
    Ok(Arc::new(out))
}

fn kluc_pamate(
    p: &PopisOperacie,
    vstupy: &[Hodnota],
    parametre: &[Hodnota],
    polohy: &BTreeMap<Meno, f64>,
) -> Odtlacok {
    let mut h = Hasovac::novy();
    h.pridaj(b"orchidea.vysledok.1");
    h.pridaj(p.identita.ako_text().as_bytes()).pridaj(&[0]).pridaj(&p.verzia.to_le_bytes());
    h.pridaj(&(vstupy.len() as u64).to_le_bytes());
    for v in vstupy {
        h.pridaj(&v.odtlacok().0);
    }
    h.pridaj(&(parametre.len() as u64).to_le_bytes());
    for v in parametre {
        h.pridaj(&v.odtlacok().0);
    }
    if p.zavisi_od_polohy {
        for (k, v) in polohy {
            h.pridaj(k.ako_text().as_bytes()).pridaj(&[0]).pridaj(&v.to_bits().to_le_bytes());
        }
    }
    h.dokonci()
}

fn over_vystupy(p: &PopisOperacie, vstupy: &[Hodnota], out: &[Hodnota]) -> Vysledok<()> {
    if out.len() != p.vystupy.len() {
        return Err(Chyba::Vypocet(format!(
            "operácia {} vrátila {} výstupov namiesto {}",
            p.identita,
            out.len(),
            p.vystupy.len()
        )));
    }
    for (v, h) in p.vystupy.iter().zip(out) {
        let typ = match &v.typ {
            TypVystupu::Pevny(t) => t.clone(),
            TypVystupu::AkoVstup(i) => {
                vstupy.get(*i as usize).map(|x| x.typ()).unwrap_or(TypHodnoty::Lubovolny)
            }
        };
        let typ = match (&v.typ, typ) {
            // „rovnaký ako vstup“ znamená rovnaký druh hodnoty a rovnaké osi, typ prvku sa smie meniť
            (TypVystupu::AkoVstup(_), TypHodnoty::Signal(mut p)) => {
                p.typ = None;
                TypHodnoty::Signal(p)
            }
            (_, t) => t,
        };
        if !typ.splna(h) {
            return Err(Chyba::Vypocet(format!(
                "operácia {} porušila svoj popis: výstup {} má byť {}, je {}",
                p.identita,
                v.meno,
                typ.popis(),
                h.typ().popis()
            )));
        }
    }
    Ok(())
}

/// Pravidlo S5: výsledok obmedzený výberom = pôvodný + maska · (nový − pôvodný).
fn obmedz(
    r: &Rozsah<'_>,
    vysledky: &BTreeMap<UzolId, Arc<Vec<Hodnota>>>,
    obmedzenie: Option<Zdroj>,
    vstupy: &[Hodnota],
    out: Arc<Vec<Hodnota>>,
) -> Vysledok<Arc<Vec<Hodnota>>> {
    let Some(z) = obmedzenie else { return Ok(out) };
    let maska = hodnota_zdroja(r, vysledky, z)?;
    let maska = maska.ako_signal().ok_or_else(|| Chyba::NezhodaTypov {
        popis: "obmedzenie výberom potrebuje masku – signál s hodnotami 0 až 1".into(),
        navrh: None,
    })?;
    let mut nove = (*out).clone();
    let mut pouzite = false;
    for (i, h) in nove.iter_mut().enumerate() {
        if let (Hodnota::Signal(novy), Some(Hodnota::Signal(povodny))) = (&*h, vstupy.get(i))
            && novy.osi() == povodny.osi()
        {
            *h = Hodnota::Signal(zmiesaj_maskou(povodny, novy, maska)?);
            pouzite = true;
        }
    }
    if !pouzite {
        return Err(Chyba::NezhodaTvaru(
            "obmedzenie výberom: žiadny výstup nemá zodpovedajúci vstup rovnakého tvaru".into(),
        ));
    }
    Ok(Arc::new(nove))
}

/// Zmieša dva signály rovnakého tvaru maskou (maska sa rozšíri na všetky osi).
pub fn zmiesaj_maskou(povodny: &Signal, novy: &Signal, maska: &Signal) -> Vysledok<Signal> {
    let m = Vyber::Maska(maska.clone()).maska_pre(novy)?;
    let m = m.hodnoty::<f32>().unwrap_or(&[]);
    let a = povodny.ako_f64();
    let b = novy.ako_f64();
    let zmes: Vec<f64> = a.iter().zip(&b).zip(m).map(|((x, y), &w)| x + (y - x) * w as f64).collect();
    let typ = novy.typ();
    let s = Signal::zo_zdielanych(
        novy.zdielane_osi(),
        crate::signal::Data::F64(zmes.into()),
        novy.vyznam().clone(),
    )?;
    Ok(if typ == TypPrvku::F64 { s } else { s.preved(typ).0 })
}
