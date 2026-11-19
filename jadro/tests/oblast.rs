//! Vyhodnotenie s oblasťou: výsledok musí byť bitovo rovnaký ako výrez z úplného výpočtu – na náhodných
//! grafoch s bodovými operáciami, rozširovaním (vysielaním) a filtrom s okrajom. A musí šetriť prácu.

use orchidea_jadro::graf::{Graf, Parameter, Uzol, UzolId, Zdroj};
use orchidea_jadro::hodnota::{Hodnota, TypHodnoty};
use orchidea_jadro::nahoda::Nahoda;
use orchidea_jadro::operacia::{
    Argumenty, KontextVypoctu, Operacia, PopisOperacie, PopisParametra, PopisPortu, PopisVystupu, TypVystupu,
};
use orchidea_jadro::{Oblast, Os, Prostredie, Signal, Vysledok, m};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Súčet okolia s polomerom r pozdĺž osi x (okraj sa opakuje) – priamy výpočet, počíta spracované prvky.
#[derive(Debug)]
struct Okolie(PopisOperacie, Arc<AtomicUsize>);

impl Operacia for Okolie {
    fn popis(&self) -> &PopisOperacie {
        &self.0
    }
    fn vypocitaj(&self, _: &KontextVypoctu<'_>, a: &Argumenty<'_>) -> Vysledok<Vec<Hodnota>> {
        let r = a.cislo("polomer")? as usize;
        let s = a.signal(0)?;
        let (ix, os) = s.os("x").expect("os x");
        let n = os.dlzka;
        let krok = s.kroky()[ix];
        let v = s.ako_f64();
        let out: Vec<f64> = (0..v.len())
            .map(|i| {
                let x = (i / krok) % n;
                let zaklad = i - x * krok;
                (0..=2 * r).map(|d| v[zaklad + (x + d).saturating_sub(r).min(n - 1) * krok]).sum::<f64>()
            })
            .collect();
        self.1.fetch_add(v.len(), Ordering::Relaxed);
        let typy: Vec<f32> = out.iter().map(|x| *x as f32).collect();
        Ok(vec![Hodnota::Signal(Signal::novy(s.osi().to_vec(), typy)?)])
    }
    fn oblast_vstupu(&self, parametre: &[Hodnota], vystup: &Oblast, _: usize) -> Option<Oblast> {
        let r = parametre[0].ako_cislo()? as usize;
        Some(vystup.clone().rozsir("x", r))
    }
}

fn prostredie() -> (Prostredie, Arc<AtomicUsize>) {
    let mut p = Prostredie::zakladne();
    let pocitadlo = Arc::new(AtomicUsize::new(0));
    p.operacie
        .zaregistruj(Arc::new(Okolie(
            PopisOperacie {
                identita: m("skúška.okolie"),
                verzia: 1,
                popis: "súčet okolia".into(),
                vstupy: vec![PopisPortu::novy(m("vstup"), TypHodnoty::signal())],
                vystupy: vec![PopisVystupu { meno: m("výsledok"), typ: TypVystupu::AkoVstup(0) }],
                parametre: vec![PopisParametra::cislo(m("polomer"), 1.0).s_rozsahom(0.0, 16.0)],
                zavisi_od_polohy: false,
            },
            pocitadlo.clone(),
        )))
        .expect("registrácia");
    p.nastav_kapacitu_pamate(0);
    (p, pocitadlo)
}

fn obraz(n: &mut Nahoda, vyska: usize, sirka: usize) -> Signal {
    let osi = vec![Os::poradova("y", vyska).unwrap(), Os::poradova("x", sirka).unwrap()];
    Signal::novy(osi, (0..vyska * sirka).map(|_| n.f32() * 4.0 - 2.0).collect()).unwrap()
}

/// Náhodný graf. Zdroje: 0, 1 = obrazy (y, x), 2 = riadok (x), 3 = číslo.
fn nahodny_graf(n: &mut Nahoda) -> Graf {
    let mut g = Graf::novy();
    for meno in ["a", "b"] {
        g.vstupy.push(PopisPortu::novy(m(meno), TypHodnoty::signal()));
    }
    g.vstupy.push(PopisPortu::novy(m("riadok"), TypHodnoty::signal()));
    g.vstupy.push(PopisPortu::novy(m("číslo"), TypHodnoty::Cislo));
    let mut obrazy: Vec<Zdroj> = vec![Zdroj::VstupGrafu(0), Zdroj::VstupGrafu(1)];
    let uzlov = 2 + n.mensie_ako(8);
    for c in 1..=uzlov {
        let id = UzolId::novy(1, c);
        let a = obrazy[n.mensie_ako(obrazy.len() as u64) as usize];
        let (op, b): (&str, Option<Zdroj>) = match n.mensie_ako(7) {
            0..=2 => {
                let op = [
                    "orchidea.súčet",
                    "orchidea.rozdiel",
                    "orchidea.súčin",
                    "orchidea.minimum",
                    "orchidea.maximum",
                ][n.mensie_ako(5) as usize];
                let b = match n.mensie_ako(3) {
                    0 => obrazy[n.mensie_ako(obrazy.len() as u64) as usize],
                    1 => Zdroj::VstupGrafu(2),
                    _ => Zdroj::VstupGrafu(3),
                };
                (op, Some(b))
            }
            3 => ("orchidea.škáluj", None),
            4 => ("orchidea.zmiešaj", Some(obrazy[n.mensie_ako(obrazy.len() as u64) as usize])),
            5 => ("orchidea.prevod_typu", None),
            _ => ("skúška.okolie", None),
        };
        let mut u = Uzol::operacia(m(op), 1);
        match op {
            "orchidea.škáluj" => {
                u.parametre.insert(m("faktor"), Parameter::Konstanta(Hodnota::Cislo(1.5)));
                u.parametre.insert(m("posun"), Parameter::Konstanta(Hodnota::Cislo(-0.25)));
            }
            "orchidea.zmiešaj" => {
                u.parametre.insert(m("pomer"), Parameter::Konstanta(Hodnota::Cislo(0.3)));
            }
            "orchidea.prevod_typu" => {
                let typ = ["f64", "u16", "f32"][n.mensie_ako(3) as usize];
                u.parametre.insert(m("typ"), Parameter::Konstanta(Hodnota::text(typ)));
            }
            "skúška.okolie" => {
                u.parametre
                    .insert(m("polomer"), Parameter::Konstanta(Hodnota::Cislo(n.mensie_ako(4) as f64)));
            }
            _ => {}
        }
        g.pridaj(id, u).unwrap();
        g.spoj(id, 0, Some(a)).unwrap();
        if let Some(b) = b {
            g.spoj(id, 1, Some(b)).unwrap();
        }
        obrazy.push(Zdroj::uzol(id));
    }
    g.nastav_vystup(m("výsledok"), obrazy.last().copied());
    if n.mensie_ako(2) == 0 {
        // druhý výstup z iného miesta grafu – uzly s viacerými spotrebiteľmi
        g.nastav_vystup(m("ďalší"), Some(obrazy[2 + n.mensie_ako(obrazy.len() as u64 - 2) as usize]));
    }
    g
}

#[test]
fn oblast_sa_rovna_vyrezu_z_celeho_vypoctu() {
    let (p, _) = prostredie();
    let mut n = Nahoda::zo_semienka(19_112_026);
    let (vyska, sirka) = (23, 41);
    let mut porovnanych = 0;
    for pokus in 0..400 {
        let g = nahodny_graf(&mut n);
        let riadok = Signal::novy(
            vec![Os::poradova("x", sirka).unwrap()],
            (0..sirka).map(|i| i as f32 * 0.5 - 3.0).collect(),
        )
        .unwrap();
        let vstupy = vec![
            Hodnota::Signal(obraz(&mut n, vyska, sirka)),
            Hodnota::Signal(obraz(&mut n, vyska, sirka)),
            Hodnota::Signal(riadok),
            Hodnota::Cislo(0.75),
        ];
        let (y0, x0) = (n.mensie_ako(vyska as u64 + 3) as usize, n.mensie_ako(sirka as u64 + 3) as usize);
        let (y1, x1) = (y0 + n.mensie_ako(12) as usize, x0 + n.mensie_ako(20) as usize);
        let mut oblast = Oblast::cela();
        if pokus % 5 != 0 {
            oblast = oblast.s_osou(m("x"), x0, x1).unwrap();
        }
        if pokus % 3 != 0 {
            oblast = oblast.s_osou(m("y"), y0, y1).unwrap();
        }
        let cely = p.vyhodnot(&g, &vstupy, &BTreeMap::new(), &BTreeMap::new());
        let cast = p.vyhodnot_oblast(&g, &vstupy, &BTreeMap::new(), &BTreeMap::new(), &oblast);
        let (cely, cast) = match (cely, cast) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(_), Err(_)) => continue,
            (a, b) => panic!("pokus {pokus}: celý {:?} vs oblasť {:?}", a.err(), b.err()),
        };
        for (c, o) in cely.iter().zip(&cast) {
            let ocakavany = oblast.vyrez(c.ako_signal().expect("signál"), None).unwrap().0;
            let dostal = o.ako_signal().expect("signál");
            assert_eq!(ocakavany.osi(), dostal.osi(), "pokus {pokus}: osi");
            assert_eq!(
                ocakavany.odtlacok(),
                dostal.odtlacok(),
                "pokus {pokus}: hodnoty sa líšia\n{g:?}\n{oblast:?}"
            );
            porovnanych += 1;
        }
    }
    assert!(porovnanych > 300, "porovnaných len {porovnanych}");
}

#[test]
fn oblast_setri_pracu() {
    let (p, pocitadlo) = prostredie();
    let mut n = Nahoda::zo_semienka(7);
    // obraz Full HD, viditeľný výrez 320 × 180, reťaz: okolie → škáluj → súčet s druhým obrazom → okolie
    let (vyska, sirka) = (1080, 1920);
    let mut g = Graf::novy();
    g.vstupy.push(PopisPortu::novy(m("a"), TypHodnoty::signal()));
    g.vstupy.push(PopisPortu::novy(m("b"), TypHodnoty::signal()));
    let mut okolie = Uzol::operacia(m("skúška.okolie"), 1);
    okolie.parametre.insert(m("polomer"), Parameter::Konstanta(Hodnota::Cislo(3.0)));
    let mut skaluj = Uzol::operacia(m("orchidea.škáluj"), 1);
    skaluj.parametre.insert(m("faktor"), Parameter::Konstanta(Hodnota::Cislo(0.5)));
    let id = |c| UzolId::novy(1, c);
    g.pridaj(id(1), okolie.clone()).unwrap();
    g.pridaj(id(2), skaluj).unwrap();
    g.pridaj(id(3), Uzol::operacia(m("orchidea.súčet"), 1)).unwrap();
    g.pridaj(id(4), okolie).unwrap();
    g.spoj(id(1), 0, Some(Zdroj::VstupGrafu(0))).unwrap();
    g.spoj(id(2), 0, Some(Zdroj::uzol(id(1)))).unwrap();
    g.spoj(id(3), 0, Some(Zdroj::uzol(id(2)))).unwrap();
    g.spoj(id(3), 1, Some(Zdroj::VstupGrafu(1))).unwrap();
    g.spoj(id(4), 0, Some(Zdroj::uzol(id(3)))).unwrap();
    g.nastav_vystup(m("výsledok"), Some(Zdroj::uzol(id(4))));
    let vstupy =
        vec![Hodnota::Signal(obraz(&mut n, vyska, sirka)), Hodnota::Signal(obraz(&mut n, vyska, sirka))];
    let oblast = Oblast::cela().s_osou(m("y"), 400, 580).unwrap().s_osou(m("x"), 800, 1120).unwrap();

    let t = std::time::Instant::now();
    let cely = p.vyhodnot(&g, &vstupy, &BTreeMap::new(), &BTreeMap::new()).unwrap();
    let (t_cely, prvky_cely) = (t.elapsed(), pocitadlo.swap(0, Ordering::Relaxed));
    let t = std::time::Instant::now();
    let cast = p.vyhodnot_oblast(&g, &vstupy, &BTreeMap::new(), &BTreeMap::new(), &oblast).unwrap();
    let (t_cast, prvky_cast) = (t.elapsed(), pocitadlo.swap(0, Ordering::Relaxed));

    let ocakavany = oblast.vyrez(cely[0].ako_signal().unwrap(), None).unwrap().0;
    assert_eq!(ocakavany.odtlacok(), cast[0].ako_signal().unwrap().odtlacok());
    eprintln!(
        "celý obraz: {prvky_cely} prvkov filtra za {t_cely:?}; viditeľný výrez: {prvky_cast} prvkov za {t_cast:?} \
         ({:.1} % práce, {:.0}× rýchlejšie)",
        100.0 * prvky_cast as f64 / prvky_cely as f64,
        t_cely.as_secs_f64() / t_cast.as_secs_f64()
    );
    // výrez má 2,8 % plochy; s okrajmi filtrov to musí byť pod 4 % práce
    assert!(prvky_cast * 25 < prvky_cely, "{prvky_cast} z {prvky_cely}");
}
