//! Výstup – zhmotnenie grafu podľa cieľovej zmluvy (prvotný pojem 10, pravidlo S7) a zmluva kvality.
//!
//! Cieľ určuje človek (typ hodnôt, kanály, rozlíšenie, význam, ktoré vlastnosti cieľ unesie). Jadro
//! **vopred** vypočíta, čo by sa pri výstupe stratilo, a po zhmotnení overí, že výsledok zmluvu spĺňa.
//! Nič sa nestratí potichu: ak zmluva niečo obetuje, zoznam strát to presne pomenuje.

use crate::chyba::{Chyba, Vysledok};
use crate::hodnota::Hodnota;
use crate::meno::Meno;
use crate::signal::{Signal, TypPrvku};
use crate::vlastnost::Vlastnosti;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Popis kvality signálu – čo nesie.
#[derive(Debug, Clone, PartialEq)]
pub struct PopisKvality {
    pub typ: TypPrvku,
    /// Osi a ich dĺžky v poradí.
    pub osi: Vec<(Meno, usize)>,
    /// Mená vzoriek označených osí (napr. kanály).
    pub mena_vzoriek: BTreeMap<Meno, Vec<Meno>>,
    pub vyznam: Vlastnosti,
}

impl PopisKvality {
    pub fn zo_signalu(s: &Signal) -> Self {
        PopisKvality {
            typ: s.typ(),
            osi: s.osi().iter().map(|o| (o.meno.clone(), o.dlzka)).collect(),
            mena_vzoriek: s
                .osi()
                .iter()
                .filter_map(|o| o.mena_vzoriek().map(|m| (o.meno.clone(), m.to_vec())))
                .collect(),
            vyznam: s.vyznam().clone(),
        }
    }
}

/// Cieľová zmluva výstupu. Prázdne pole znamená „bez požiadavky“.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CielovaZmluva {
    /// Typ hodnôt v cieli.
    pub typ: Option<TypPrvku>,
    /// Ktoré vzorky označenej osi cieľ unesie (napr. kanál → [R, G, B]).
    pub vzorky: BTreeMap<Meno, Vec<Meno>>,
    /// Najväčšia dĺžka osí v cieli (napr. rozlíšenie).
    pub najviac: BTreeMap<Meno, usize>,
    /// Osi, ktoré cieľ vôbec unesie (None = všetky).
    pub osi: Option<BTreeSet<Meno>>,
    /// Význam, ktorý musí mať cieľ (napr. farebný priestor); hodnota sa porovná so zdrojom.
    pub vyznam: Vlastnosti,
    /// Ktoré vlastnosti významu cieľ unesie (None = všetky).
    pub nesie_vlastnosti: Option<BTreeSet<Meno>>,
}

/// Jedna strata pri výstupe.
#[derive(Debug, Clone, PartialEq)]
pub enum Strata {
    /// Cieľový typ nezapíše všetky hodnoty zdroja presne.
    Presnost { z: TypPrvku, na: TypPrvku },
    /// Vzorky (napr. kanály), ktoré cieľ neunesie.
    Vzorky { os: Meno, stratene: Vec<Meno> },
    /// Priehľadnosť sa stratí (osobitne, lebo ju ľudia často prehliadnu).
    Priehladnost,
    /// Os sa zmenší.
    Rozlisenie { os: Meno, z: usize, na: usize },
    /// Celá os sa stratí.
    Os { os: Meno },
    /// Význam sa zmení (napr. iný farebný priestor) – vyžaduje prevod.
    Vyznam { kluc: Meno, z: Option<Hodnota>, na: Hodnota },
    /// Vlastnosti, ktoré cieľ neunesie (metadáta).
    Vlastnosti { kluce: Vec<Meno> },
}

impl fmt::Display for Strata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Strata::Presnost { z, na } => {
                write!(f, "presnosť: {} → {} (nie všetky hodnoty sa zapíšu presne)", z.meno(), na.meno())
            }
            Strata::Vzorky { os, stratene } => {
                let s: Vec<&str> = stratene.iter().map(|m| m.ako_text()).collect();
                write!(f, "os {os}: stratia sa {}", s.join(", "))
            }
            Strata::Priehladnost => f.write_str("stratí sa priehľadnosť"),
            Strata::Rozlisenie { os, z, na } => write!(f, "os {os}: {z} → {na} vzoriek"),
            Strata::Os { os } => write!(f, "stratí sa celá os {os}"),
            Strata::Vyznam { kluc, z, na } => write!(f, "význam {kluc}: {z:?} → {na:?}"),
            Strata::Vlastnosti { kluce } => {
                let s: Vec<&str> = kluce.iter().map(|m| m.ako_text()).collect();
                write!(f, "stratia sa vlastnosti: {}", s.join(", "))
            }
        }
    }
}

fn je_priehladnost(m: &Meno) -> bool {
    matches!(m.ako_text(), "A" | "a" | "alfa" | "alpha" | "priehľadnosť")
}

/// Vypočíta vopred všetky straty, ktoré by výstup spôsobil.
pub fn straty(zdroj: &PopisKvality, ciel: &CielovaZmluva) -> Vec<Strata> {
    let mut s = Vec::new();
    if let Some(t) = ciel.typ
        && !zdroj.typ.bezstratovo_do(t)
    {
        s.push(Strata::Presnost { z: zdroj.typ, na: t });
    }
    for (os, dlzka) in &zdroj.osi {
        if let Some(povolene) = &ciel.osi
            && !povolene.contains(os)
        {
            s.push(Strata::Os { os: os.clone() });
            continue;
        }
        if let Some(&max) = ciel.najviac.get(os)
            && *dlzka > max
        {
            s.push(Strata::Rozlisenie { os: os.clone(), z: *dlzka, na: max });
        }
        if let (Some(mena), Some(unesie)) = (zdroj.mena_vzoriek.get(os), ciel.vzorky.get(os)) {
            let stratene: Vec<Meno> = mena.iter().filter(|m| !unesie.contains(m)).cloned().collect();
            if stratene.iter().any(je_priehladnost) {
                s.push(Strata::Priehladnost);
            }
            if !stratene.is_empty() {
                s.push(Strata::Vzorky { os: os.clone(), stratene });
            }
        }
    }
    for (k, v) in ciel.vyznam.iter() {
        let z = zdroj.vyznam.ziskaj(k.ako_text()).cloned();
        if z.as_ref() != Some(v) {
            s.push(Strata::Vyznam { kluc: k.clone(), z, na: v.clone() });
        }
    }
    if let Some(nesie) = &ciel.nesie_vlastnosti {
        let kluce: Vec<Meno> = zdroj.vyznam.mena().filter(|k| !nesie.contains(*k)).cloned().collect();
        if !kluce.is_empty() {
            s.push(Strata::Vlastnosti { kluce });
        }
    }
    s
}

/// Po zhmotnení overí, že výsledok spĺňa zmluvu (typ, osi, vzorky, dĺžky, význam).
pub fn over(vysledok: &Signal, ciel: &CielovaZmluva) -> Vysledok<()> {
    let p = PopisKvality::zo_signalu(vysledok);
    let mut problemy = Vec::new();
    if let Some(t) = ciel.typ
        && p.typ != t
    {
        problemy.push(format!("typ je {}, zmluva čaká {}", p.typ.meno(), t.meno()));
    }
    for (os, d) in &p.osi {
        if ciel.osi.as_ref().is_some_and(|o| !o.contains(os)) {
            problemy.push(format!("os {os} nemá v cieli čo robiť"));
        }
        if ciel.najviac.get(os).is_some_and(|&m| *d > m) {
            problemy.push(format!("os {os} má {d} vzoriek, cieľ najviac {}", ciel.najviac[os]));
        }
    }
    for (os, unesie) in &ciel.vzorky {
        if let Some(mena) = p.mena_vzoriek.get(os)
            && mena.iter().any(|m| !unesie.contains(m))
        {
            problemy.push(format!("os {os} má vzorky, ktoré cieľ neunesie"));
        }
    }
    for (k, v) in ciel.vyznam.iter() {
        if p.vyznam.ziskaj(k.ako_text()) != Some(v) {
            problemy.push(format!("význam {k} nesedí so zmluvou"));
        }
    }
    if problemy.is_empty() {
        Ok(())
    } else {
        Err(Chyba::NezhodaTypov {
            popis: format!("výstup porušuje zmluvu: {}", problemy.join("; ")),
            navrh: None,
        })
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::meno::m;
    use crate::os::Os;

    fn rgba_f32() -> Signal {
        let osi = vec![
            Os::poradova("y", 4).unwrap(),
            Os::poradova("x", 8).unwrap(),
            Os::oznacena("kanál", &["R", "G", "B", "A"]).unwrap(),
        ];
        Signal::vyplneny(osi, TypPrvku::F32, 0.5).unwrap().s_vyznamom(
            Vlastnosti::nove()
                .s(m("farebný priestor"), Hodnota::text("ACEScg"))
                .s(m("autor"), Hodnota::text("x")),
        )
    }

    #[test]
    fn straty_vopred() {
        let zdroj = PopisKvality::zo_signalu(&rgba_f32());
        let mut ciel = CielovaZmluva { typ: Some(TypPrvku::U8), ..Default::default() };
        ciel.vzorky.insert(m("kanál"), vec![m("R"), m("G"), m("B")]);
        ciel.najviac.insert(m("x"), 4);
        ciel.vyznam.nastav(m("farebný priestor"), Hodnota::text("sRGB"));
        ciel.nesie_vlastnosti = Some([m("farebný priestor")].into_iter().collect());
        let s = straty(&zdroj, &ciel);
        assert!(s.contains(&Strata::Presnost { z: TypPrvku::F32, na: TypPrvku::U8 }));
        assert!(s.contains(&Strata::Priehladnost));
        assert!(s.contains(&Strata::Rozlisenie { os: m("x"), z: 8, na: 4 }));
        assert!(s.iter().any(|x| matches!(x, Strata::Vyznam { .. })));
        assert!(s.contains(&Strata::Vlastnosti { kluce: vec![m("autor")] }));
        assert_eq!(s.len(), 6, "{s:?}");
        let bez = CielovaZmluva { typ: Some(TypPrvku::F64), ..Default::default() };
        assert!(straty(&zdroj, &bez).is_empty());
    }

    #[test]
    fn overenie_po_zhmotneni() {
        let s = rgba_f32();
        let mut ciel = CielovaZmluva { typ: Some(TypPrvku::F32), ..Default::default() };
        assert!(over(&s, &ciel).is_ok());
        ciel.vzorky.insert(m("kanál"), vec![m("R"), m("G"), m("B")]);
        assert!(over(&s, &ciel).is_err());
        let rgb = s.vyrez("kanál", 0, 3).unwrap();
        assert!(over(&rgb, &ciel).is_ok());
    }
}
