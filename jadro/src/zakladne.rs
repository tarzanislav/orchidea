//! Základné operácie jadra – čistá matematika nad signálmi a číslami, bez akéhokoľvek účelu.
//!
//! Registrujú sa rovnakým verejným rozhraním ako rozšírenia (zásada A5). Výpočty po hodnotách bežia
//! v plávajúcej čiarke: výsledok celočíselných vstupov je f32, so vstupom f64 je f64 – nič sa neoreže potichu.

use crate::chyba::{Chyba, Vysledok};
use crate::hodnota::{Hodnota, TypHodnoty};
use crate::meno::m;
use crate::oblast::Oblast;
use crate::operacia::{
    Argumenty, KontextVypoctu, Operacia, PopisOperacie, PopisParametra, PopisPortu, PopisVystupu,
    RegisterOperacii, TypVystupu,
};
use crate::signal::{Data, Signal, TypPrvku};
use std::sync::Arc;

/// Typ výsledku výpočtu po hodnotách.
fn typ_vysledku(a: TypPrvku, b: Option<TypPrvku>) -> TypPrvku {
    if a == TypPrvku::F64 || b == Some(TypPrvku::F64) { TypPrvku::F64 } else { TypPrvku::F32 }
}

fn uloz(osi: &Signal, hodnoty: Vec<f64>, typ: TypPrvku) -> Vysledok<Signal> {
    let s = Signal::zo_zdielanych(osi.zdielane_osi(), Data::F64(hodnoty.into()), osi.vyznam().clone())?;
    Ok(if typ == TypPrvku::F64 { s } else { s.preved(typ).0 })
}

/// Hodnoty signálu `b` rozšírené na osi signálu `a` (b musí mať podmnožinu osí `a` s rovnakými dĺžkami).
pub fn rozsir(b: &Signal, a: &Signal) -> Vysledok<Vec<f64>> {
    if b.osi() == a.osi() {
        return Ok(b.ako_f64());
    }
    let mut mapovanie = Vec::with_capacity(b.osi().len());
    for ob in b.osi() {
        let (i, oa) =
            a.os(ob.meno.ako_text()).ok_or_else(|| Chyba::NezhodaTvaru(format!("os {} chýba", ob.meno)))?;
        if oa.dlzka != ob.dlzka {
            return Err(Chyba::NezhodaTvaru(format!("os {} má dĺžky {} a {}", ob.meno, oa.dlzka, ob.dlzka)));
        }
        mapovanie.push(i);
    }
    let hb = b.ako_f64();
    let (ka, kb) = (a.kroky(), b.kroky());
    let osi = a.osi();
    Ok((0..a.pocet())
        .map(|idx| {
            let mut j = 0;
            for (d, &i) in mapovanie.iter().enumerate() {
                j += ((idx / ka[i]) % osi[i].dlzka) * kb[d];
            }
            hb[j]
        })
        .collect())
}

/// Spojí dve hodnoty (signál a signál, signál a číslo, číslo a číslo) funkciou po hodnotách.
fn po_hodnotach(a: &Hodnota, b: &Hodnota, f: impl Fn(f64, f64) -> f64) -> Vysledok<Hodnota> {
    match (a, b) {
        (Hodnota::Signal(x), Hodnota::Signal(y)) => {
            let (vacsi, mensi, poradie) = if y
                .ma_osi(&x.osi().iter().map(|o| o.meno.clone()).collect::<Vec<_>>())
                && y.osi().len() > x.osi().len()
            {
                (y, x, true)
            } else {
                (x, y, false)
            };
            let hm = rozsir(mensi, vacsi)?;
            let hv = vacsi.ako_f64();
            let v: Vec<f64> =
                hv.iter().zip(&hm).map(|(&p, &q)| if poradie { f(q, p) } else { f(p, q) }).collect();
            Ok(Hodnota::Signal(uloz(vacsi, v, typ_vysledku(x.typ(), Some(y.typ())))?))
        }
        (Hodnota::Signal(x), c) | (c, Hodnota::Signal(x)) if c.ako_cislo().is_some() => {
            let k = c.ako_cislo().unwrap_or(0.0);
            let signal_vlavo = matches!(a, Hodnota::Signal(_));
            let v: Vec<f64> =
                x.ako_f64().into_iter().map(|p| if signal_vlavo { f(p, k) } else { f(k, p) }).collect();
            Ok(Hodnota::Signal(uloz(x, v, typ_vysledku(x.typ(), None))?))
        }
        (p, q) => match (p.ako_cislo(), q.ako_cislo()) {
            (Some(x), Some(y)) => Ok(Hodnota::Cislo(f(x, y))),
            _ => Err(Chyba::NezhodaTypov {
                popis: format!("nemožno spojiť {} a {}", p.typ().popis(), q.typ().popis()),
                navrh: None,
            }),
        },
    }
}

#[derive(Debug)]
struct Dvojica {
    popis: PopisOperacie,
    f: fn(f64, f64) -> f64,
}

impl Operacia for Dvojica {
    fn popis(&self) -> &PopisOperacie {
        &self.popis
    }
    fn vypocitaj(&self, _: &KontextVypoctu<'_>, a: &Argumenty<'_>) -> Vysledok<Vec<Hodnota>> {
        Ok(vec![po_hodnotach(a.vstup(0)?, a.vstup(1)?, self.f)?])
    }

    fn oblast_vstupu(&self, _: &[Hodnota], vystup: &Oblast, _: usize) -> Option<Oblast> {
        Some(vystup.clone())
    }
}

fn dvojica(identita: &str, popis: &str, f: fn(f64, f64) -> f64) -> Arc<dyn Operacia> {
    Arc::new(Dvojica {
        popis: PopisOperacie {
            identita: m(identita),
            verzia: 1,
            popis: popis.into(),
            vstupy: vec![
                PopisPortu::novy(m("a"), TypHodnoty::Lubovolny),
                PopisPortu::novy(m("b"), TypHodnoty::Lubovolny),
            ],
            vystupy: vec![PopisVystupu {
                meno: m("výsledok"),
                typ: TypVystupu::Pevny(TypHodnoty::Lubovolny),
            }],
            parametre: vec![],
            zavisi_od_polohy: false,
        },
        f,
    })
}

#[derive(Debug)]
struct Skaluj(PopisOperacie);

impl Operacia for Skaluj {
    fn popis(&self) -> &PopisOperacie {
        &self.0
    }
    fn vypocitaj(&self, _: &KontextVypoctu<'_>, a: &Argumenty<'_>) -> Vysledok<Vec<Hodnota>> {
        let (k, p) = (a.cislo("faktor")?, a.cislo("posun")?);
        let s = a.signal(0)?;
        let v: Vec<f64> = s.ako_f64().into_iter().map(|x| x * k + p).collect();
        Ok(vec![Hodnota::Signal(uloz(s, v, typ_vysledku(s.typ(), None))?)])
    }

    fn oblast_vstupu(&self, _: &[Hodnota], vystup: &Oblast, _: usize) -> Option<Oblast> {
        Some(vystup.clone())
    }
}

#[derive(Debug)]
struct Zmiesaj(PopisOperacie);

impl Operacia for Zmiesaj {
    fn popis(&self) -> &PopisOperacie {
        &self.0
    }
    fn vypocitaj(&self, _: &KontextVypoctu<'_>, a: &Argumenty<'_>) -> Vysledok<Vec<Hodnota>> {
        let t = a.cislo("pomer")?;
        let (x, y) = (a.signal(0)?, a.signal(1)?);
        if x.osi() != y.osi() {
            return Err(Chyba::NezhodaTvaru("zmiešať možno len signály rovnakého tvaru".into()));
        }
        let v: Vec<f64> = x.ako_f64().iter().zip(y.ako_f64()).map(|(p, q)| p + (q - p) * t).collect();
        Ok(vec![Hodnota::Signal(uloz(x, v, typ_vysledku(x.typ(), Some(y.typ())))?)])
    }

    fn oblast_vstupu(&self, _: &[Hodnota], vystup: &Oblast, _: usize) -> Option<Oblast> {
        Some(vystup.clone())
    }
}

#[derive(Debug)]
struct PrevodTypu(PopisOperacie);

impl Operacia for PrevodTypu {
    fn popis(&self) -> &PopisOperacie {
        &self.0
    }
    fn vypocitaj(&self, _: &KontextVypoctu<'_>, a: &Argumenty<'_>) -> Vysledok<Vec<Hodnota>> {
        let t = a.text("typ")?;
        let typ = TypPrvku::z_mena(t).ok_or_else(|| Chyba::MimoRozsahu(format!("neznámy typ prvku {t}")))?;
        Ok(vec![Hodnota::Signal(a.signal(0)?.preved(typ).0)])
    }

    fn oblast_vstupu(&self, _: &[Hodnota], vystup: &Oblast, _: usize) -> Option<Oblast> {
        Some(vystup.clone())
    }
}

/// Register so základnými operáciami jadra.
pub fn register() -> RegisterOperacii {
    let mut r = RegisterOperacii::prazdny();
    let signal_vstup = || vec![PopisPortu::novy(m("vstup"), TypHodnoty::signal())];
    let ako_vstup = || vec![PopisVystupu { meno: m("výsledok"), typ: TypVystupu::AkoVstup(0) }];
    let ops: Vec<Arc<dyn Operacia>> = vec![
        dvojica(
            "orchidea.súčet",
            "a + b po hodnotách (signál so signálom, signál s číslom, čísla)",
            |a, b| a + b,
        ),
        dvojica("orchidea.rozdiel", "a − b po hodnotách", |a, b| a - b),
        dvojica("orchidea.súčin", "a · b po hodnotách", |a, b| a * b),
        dvojica("orchidea.podiel", "a / b po hodnotách", |a, b| a / b),
        dvojica("orchidea.minimum", "menšia z hodnôt", f64::min),
        dvojica("orchidea.maximum", "väčšia z hodnôt", f64::max),
        Arc::new(Skaluj(PopisOperacie {
            identita: m("orchidea.škáluj"),
            verzia: 1,
            popis: "vstup · faktor + posun".into(),
            vstupy: signal_vstup(),
            vystupy: ako_vstup(),
            parametre: vec![PopisParametra::cislo(m("faktor"), 1.0), PopisParametra::cislo(m("posun"), 0.0)],
            zavisi_od_polohy: false,
        })),
        Arc::new(Zmiesaj(PopisOperacie {
            identita: m("orchidea.zmiešaj"),
            verzia: 1,
            popis: "a + (b − a) · pomer".into(),
            vstupy: vec![
                PopisPortu::novy(m("a"), TypHodnoty::signal()),
                PopisPortu::novy(m("b"), TypHodnoty::signal()),
            ],
            vystupy: ako_vstup(),
            parametre: vec![PopisParametra::cislo(m("pomer"), 0.5).s_rozsahom(0.0, 1.0)],
            zavisi_od_polohy: false,
        })),
    ];
    for op in ops {
        r.zaregistruj(op).expect("základné operácie majú jedinečné identity");
    }
    let mut typ = PopisParametra::cislo(m("typ"), 0.0);
    typ.typ = TypHodnoty::Text;
    typ.predvolena = Hodnota::text("f32");
    let prevod = Arc::new(PrevodTypu(PopisOperacie {
        identita: m("orchidea.prevod_typu"),
        verzia: 1,
        popis: "prevod typu hodnôt so zaokrúhlením a orezaním na rozsah".into(),
        vstupy: signal_vstup(),
        vystupy: ako_vstup(),
        parametre: vec![typ],
        zavisi_od_polohy: false,
    }));
    r.zaregistruj_prevodnik(prevod).expect("prevodník je jedinečný");
    r
}
