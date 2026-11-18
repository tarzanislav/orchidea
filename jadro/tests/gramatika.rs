//! Skúšky gramatiky: pravidlá skladania S1–S7 a zásady A7, A8, A9 na celých grafoch a dielach.

use orchidea_jadro::chyba::Chyba;
use orchidea_jadro::graf::{DruhUzla, Graf, Parameter, Uzol, UzolId, Zdroj};
use orchidea_jadro::hodnota::{Hodnota, PoziadavkaSignalu, TypHodnoty};
use orchidea_jadro::krivka::{Interpolacia, Kluc, Krivka, Mimo};
use orchidea_jadro::operacia::{
    Argumenty, KontextVypoctu, Operacia, PopisOperacie, PopisParametra, PopisPortu, PopisVystupu, TypVystupu,
};
use orchidea_jadro::vyber::{Interval, Vyber};
use orchidea_jadro::vyraz::Vyraz;
use orchidea_jadro::{Kodovatelne, Os, Prostredie, Signal, TypPrvku, m};
use std::collections::BTreeMap;
use std::sync::Arc;

fn id(n: u64) -> UzolId {
    UzolId::novy(1, n)
}

fn signal(hodnoty: &[f32]) -> Signal {
    Signal::novy(vec![Os::poradova("x", hodnoty.len()).unwrap()], hodnoty.to_vec()).unwrap()
}

fn obraz(sirka: usize, vyska: usize) -> Signal {
    let osi = vec![Os::poradova("y", vyska).unwrap(), Os::poradova("x", sirka).unwrap()];
    Signal::novy(osi, (0..sirka * vyska).map(|i| i as f32).collect()).unwrap()
}

fn bez() -> BTreeMap<orchidea_jadro::Meno, Hodnota> {
    BTreeMap::new()
}

fn nikde() -> BTreeMap<orchidea_jadro::Meno, f64> {
    BTreeMap::new()
}

/// Graf: vstup → škáluj(faktor) → výstup.
fn graf_skalovania(faktor: Parameter) -> Graf {
    let mut g = Graf::novy();
    g.vstupy.push(PopisPortu::novy(m("vstup"), TypHodnoty::signal()));
    let mut u = Uzol::operacia(m("orchidea.škáluj"), 1);
    u.parametre.insert(m("faktor"), faktor);
    g.pridaj(id(1), u).unwrap();
    g.spoj(id(1), 0, Some(Zdroj::VstupGrafu(0))).unwrap();
    g.nastav_vystup(m("výsledok"), Some(Zdroj::uzol(id(1))));
    g
}

fn vysledok(p: &Prostredie, g: &Graf, vstup: Signal) -> Vec<f32> {
    let out = p.vyhodnot(g, &[Hodnota::Signal(vstup)], &bez(), &nikde()).unwrap();
    out[0].ako_signal().unwrap().ako_f32()
}

#[test]
fn s1_typy_a_navrh_prevodnika() {
    #[derive(Debug)]
    struct ChceU8(PopisOperacie);
    impl Operacia for ChceU8 {
        fn popis(&self) -> &PopisOperacie {
            &self.0
        }
        fn vypocitaj(
            &self,
            _: &KontextVypoctu<'_>,
            a: &Argumenty<'_>,
        ) -> orchidea_jadro::Vysledok<Vec<Hodnota>> {
            Ok(vec![a.vstup(0)?.clone()])
        }
    }
    let mut p = Prostredie::zakladne();
    let u8_signal = TypHodnoty::Signal(PoziadavkaSignalu { osi: vec![m("x")], typ: Some(TypPrvku::U8) });
    p.operacie
        .zaregistruj(Arc::new(ChceU8(PopisOperacie {
            identita: m("skúška.chce_u8"),
            verzia: 1,
            popis: String::new(),
            vstupy: vec![PopisPortu::novy(m("vstup"), u8_signal)],
            vystupy: vec![PopisVystupu { meno: m("výstup"), typ: TypVystupu::AkoVstup(0) }],
            parametre: vec![],
            zavisi_od_polohy: false,
        })))
        .unwrap();
    let mut g = Graf::novy();
    g.vstupy.push(PopisPortu::novy(
        m("obraz"),
        TypHodnoty::Signal(PoziadavkaSignalu { osi: vec![m("x")], typ: Some(TypPrvku::F32) }),
    ));
    g.vstupy.push(PopisPortu::novy(m("číslo"), TypHodnoty::Cislo));
    g.pridaj(id(1), Uzol::operacia(m("skúška.chce_u8"), 1)).unwrap();
    match g.spoj_overene(&p, id(1), 0, Zdroj::VstupGrafu(0)) {
        Err(Chyba::NezhodaTypov { navrh: Some(n), .. }) => assert!(n.contains("orchidea.prevod_typu"), "{n}"),
        ine => panic!("čakal som nezhodu typov s návrhom prevodníka, je {ine:?}"),
    }
    assert!(matches!(
        g.spoj_overene(&p, id(1), 0, Zdroj::VstupGrafu(1)),
        Err(Chyba::NezhodaTypov { navrh: None, .. })
    ));
    // s prevodníkom sedí
    let mut prevod = Uzol::operacia(m("orchidea.prevod_typu"), 1);
    prevod.parametre.insert(m("typ"), Parameter::Konstanta(Hodnota::text("u8")));
    g.pridaj(id(2), prevod).unwrap();
    g.spoj_overene(&p, id(2), 0, Zdroj::VstupGrafu(0)).unwrap();
    g.spoj(id(1), 0, Some(Zdroj::uzol(id(2)))).unwrap();
    g.nastav_vystup(m("výstup"), Some(Zdroj::uzol(id(1))));
    let out = p
        .vyhodnot(&g, &[Hodnota::Signal(signal(&[1.4, 300.0])), Hodnota::Cislo(0.0)], &bez(), &nikde())
        .unwrap();
    assert_eq!(out[0].ako_signal().unwrap().hodnoty::<u8>().unwrap(), &[1, 255]);
}

#[test]
fn s2_vnorenie_a_s3_odkaz_prepisanie_variant_odpojenie() {
    let mut p = Prostredie::zakladne();
    // knižničný graf s parametrom k (predvolene 2) a variantom „silné“ (k = 10)
    let mut kniznicny = graf_skalovania(Parameter::Grafu(m("k")));
    kniznicny.parametre.push(PopisParametra::cislo(m("k"), 2.0));
    kniznicny.varianty.insert(m("silné"), [(m("k"), Parameter::Konstanta(Hodnota::Cislo(10.0)))].into());
    p.grafy.insert(m("zosilnenie"), Arc::new(kniznicny.clone()));

    // S2: graf ako vnorený uzol
    let mut vonkajsi = Graf::novy();
    vonkajsi.vstupy.push(PopisPortu::novy(m("vstup"), TypHodnoty::signal()));
    vonkajsi.pridaj(id(10), Uzol::novy(DruhUzla::Vnoreny(Arc::new(kniznicny.clone())))).unwrap();
    vonkajsi.spoj(id(10), 0, Some(Zdroj::VstupGrafu(0))).unwrap();
    vonkajsi.nastav_vystup(m("výsledok"), Some(Zdroj::uzol(id(10))));
    assert_eq!(vysledok(&p, &vonkajsi, signal(&[1.0, 2.0])), vec![2.0, 4.0]);

    // S3: odkaz, variant, prepísanie (prepísanie uzla má prednosť pred variantom)
    vonkajsi.uzol_mut(id(10)).unwrap().druh = DruhUzla::Odkaz { graf: m("zosilnenie") };
    assert_eq!(vysledok(&p, &vonkajsi, signal(&[1.0])), vec![2.0]);
    vonkajsi.uzol_mut(id(10)).unwrap().variant = Some(m("silné"));
    assert_eq!(vysledok(&p, &vonkajsi, signal(&[1.0])), vec![10.0]);
    vonkajsi.uzol_mut(id(10)).unwrap().parametre.insert(m("k"), Parameter::Konstanta(Hodnota::Cislo(3.0)));
    assert_eq!(vysledok(&p, &vonkajsi, signal(&[1.0])), vec![3.0]);

    // zmena v knižnici sa prejaví cez odkaz…
    let mut zmeneny = kniznicny.clone();
    zmeneny
        .uzol_mut(id(1))
        .unwrap()
        .parametre
        .insert(m("posun"), Parameter::Konstanta(Hodnota::Cislo(100.0)));
    let odpojeny = {
        let mut g = vonkajsi.clone();
        g.odpoj_odkaz(id(10), &p).unwrap();
        g
    };
    p.grafy.insert(m("zosilnenie"), Arc::new(zmeneny));
    assert_eq!(vysledok(&p, &vonkajsi, signal(&[1.0])), vec![103.0]);
    // …ale nie cez odpojenú kópiu, ktorá si drží variant aj prepísania
    assert_eq!(vysledok(&p, &odpojeny, signal(&[1.0])), vec![3.0]);
    assert!(matches!(odpojeny.uzol(id(10)).unwrap().druh, DruhUzla::Vnoreny(_)));
}

#[test]
fn s4_parameter_je_vstup_krivka_a_vyraz() {
    let p = Prostredie::zakladne();
    // faktor = výstup súčtu dvoch čísel (riadenie jedného výsledku druhým)
    let mut g = graf_skalovania(Parameter::Vstup(Zdroj::uzol(id(2))));
    g.vstupy.push(PopisPortu::novy(m("a"), TypHodnoty::Cislo));
    g.pridaj(id(2), Uzol::operacia(m("orchidea.súčet"), 1)).unwrap();
    g.spoj(id(2), 0, Some(Zdroj::VstupGrafu(1))).unwrap();
    g.spoj(id(2), 1, Some(Zdroj::VstupGrafu(1))).unwrap();
    let out = p
        .vyhodnot(&g, &[Hodnota::Signal(signal(&[1.0, 2.0])), Hodnota::Cislo(1.5)], &bez(), &nikde())
        .unwrap();
    assert_eq!(out[0].ako_signal().unwrap().ako_f32(), vec![3.0, 6.0]);

    // krivka v čase
    let k = Krivka::nova(
        m("t"),
        vec![Kluc::novy(0.0, 0.0, Interpolacia::Linearna), Kluc::novy(2.0, 4.0, Interpolacia::Linearna)],
        Mimo::Drz,
    )
    .unwrap();
    let g = graf_skalovania(Parameter::Krivka(k));
    let v = |t: f64| {
        let polohy = [(m("t"), t)].into();
        p.vyhodnot(&g, &[Hodnota::Signal(signal(&[1.0]))], &bez(), &polohy).unwrap()[0]
            .ako_signal()
            .unwrap()
            .ako_f32()
    };
    assert_eq!(v(0.5), vec![1.0]);
    assert_eq!(v(1.5), vec![3.0]);
    assert!(p.vyhodnot(&g, &[Hodnota::Signal(signal(&[1.0]))], &bez(), &nikde()).is_err(), "chýba poloha t");

    // výraz nad polohou a parametrom grafu
    let mut g = graf_skalovania(Parameter::Vyraz(Vyraz::novy("2 * t + k").unwrap()));
    g.parametre.push(PopisParametra::cislo(m("k"), 1.0));
    let polohy = [(m("t"), 3.0)].into();
    let out = p
        .vyhodnot(&g, &[Hodnota::Signal(signal(&[1.0]))], &[(m("k"), Hodnota::Cislo(0.5))].into(), &polohy)
        .unwrap();
    assert_eq!(out[0].ako_signal().unwrap().ako_f32(), vec![6.5]);
    assert!(
        p.vyhodnot(&g, &[Hodnota::Signal(signal(&[1.0]))], &[(m("q"), Hodnota::Cislo(0.5))].into(), &polohy)
            .is_err()
    );
}

#[test]
fn s5_obmedzenie_vyberom_a_a8_vypnutie() {
    let p = Prostredie::zakladne();
    let mut g = graf_skalovania(Parameter::Konstanta(Hodnota::Cislo(0.0)));
    // maska je vstup grafu – výber oblasti x ∈ [1, 2]
    g.vstupy.push(PopisPortu::novy(m("maska"), TypHodnoty::signal()));
    g.uzol_mut(id(1)).unwrap().obmedzenie = Some(Zdroj::VstupGrafu(1));
    let vstup = obraz(4, 2);
    let maska = Vyber::Oblast(vec![Interval { os: m("x"), od: 1.0, po: 2.0, makkost: 0.0 }])
        .maska_pre(&vstup)
        .unwrap();
    let out = p
        .vyhodnot(&g, &[Hodnota::Signal(vstup.clone()), Hodnota::Signal(maska.clone())], &bez(), &nikde())
        .unwrap();
    assert_eq!(out[0].ako_signal().unwrap().ako_f32(), vec![0.0, 0.0, 0.0, 3.0, 4.0, 0.0, 0.0, 7.0]);
    // maska len nad osou x sa rozšíri na celý obraz
    let len_x = Signal::novy(vec![Os::poradova("x", 4).unwrap()], vec![0.0f32, 0.5, 0.0, 0.0]).unwrap();
    let out =
        p.vyhodnot(&g, &[Hodnota::Signal(vstup.clone()), Hodnota::Signal(len_x)], &bez(), &nikde()).unwrap();
    assert_eq!(out[0].ako_signal().unwrap().ako_f32(), vec![0.0, 0.5, 2.0, 3.0, 4.0, 2.5, 6.0, 7.0]);
    // vypnutý uzol prepustí vstup bez zmeny
    g.uzol_mut(id(1)).unwrap().vypnuty = true;
    let out =
        p.vyhodnot(&g, &[Hodnota::Signal(vstup.clone()), Hodnota::Signal(maska)], &bez(), &nikde()).unwrap();
    assert_eq!(out[0].ako_signal().unwrap(), &vstup);
}

#[test]
fn pamat_vysledkov_cyklus_a_zmluva_operacie() {
    let p = Prostredie::zakladne();
    let g = graf_skalovania(Parameter::Konstanta(Hodnota::Cislo(2.0)));
    let s = obraz(64, 64);
    let pred = p.statistika_pamate();
    vysledok(&p, &g, s.clone());
    vysledok(&p, &g, s.clone());
    let po = p.statistika_pamate();
    assert_eq!(po.zasahy - pred.zasahy, 1, "druhý výpočet sa vzal z pamäte");
    let iny = graf_skalovania(Parameter::Konstanta(Hodnota::Cislo(3.0)));
    vysledok(&p, &iny, s);
    assert_eq!(p.statistika_pamate().minutia - po.minutia, 1);

    // cyklus
    let mut c = Graf::novy();
    c.pridaj(id(1), Uzol::operacia(m("orchidea.súčet"), 1)).unwrap();
    c.pridaj(id(2), Uzol::operacia(m("orchidea.súčet"), 1)).unwrap();
    c.spoj(id(1), 0, Some(Zdroj::uzol(id(2)))).unwrap();
    c.spoj(id(2), 0, Some(Zdroj::uzol(id(1)))).unwrap();
    c.nastav_vystup(m("x"), Some(Zdroj::uzol(id(1))));
    assert!(matches!(p.vyhodnot(&c, &[], &bez(), &nikde()), Err(Chyba::Cyklus(_))));
    assert!(c.over(&p).iter().any(|e| matches!(e, Chyba::Cyklus(_))));

    // operácia, ktorá poruší vlastný popis, sa zastaví chybou
    #[derive(Debug)]
    struct Klamar(PopisOperacie);
    impl Operacia for Klamar {
        fn popis(&self) -> &PopisOperacie {
            &self.0
        }
        fn vypocitaj(
            &self,
            _: &KontextVypoctu<'_>,
            _: &Argumenty<'_>,
        ) -> orchidea_jadro::Vysledok<Vec<Hodnota>> {
            Ok(vec![Hodnota::text("nie som signál")])
        }
    }
    let mut p = Prostredie::zakladne();
    p.operacie
        .zaregistruj(Arc::new(Klamar(PopisOperacie {
            identita: m("skúška.klamár"),
            verzia: 1,
            popis: String::new(),
            vstupy: vec![],
            vystupy: vec![PopisVystupu { meno: m("v"), typ: TypVystupu::Pevny(TypHodnoty::signal()) }],
            parametre: vec![],
            zavisi_od_polohy: false,
        })))
        .unwrap();
    let mut k = Graf::novy();
    k.pridaj(id(1), Uzol::operacia(m("skúška.klamár"), 1)).unwrap();
    k.nastav_vystup(m("v"), Some(Zdroj::uzol(id(1))));
    assert!(matches!(p.vyhodnot(&k, &[], &bez(), &nikde()), Err(Chyba::Vypocet(_))));
}

#[test]
fn graf_sa_zakoduje_presne() {
    let mut g = graf_skalovania(Parameter::Vyraz(Vyraz::novy("sin(t) * 2").unwrap()));
    g.parametre.push(PopisParametra::cislo(m("k"), 1.0).s_rozsahom(0.0, 10.0));
    g.varianty.insert(m("v"), [(m("k"), Parameter::Konstanta(Hodnota::Cislo(4.0)))].into());
    g.pridaj(id(5), Uzol::novy(DruhUzla::Vnoreny(Arc::new(graf_skalovania(Parameter::Grafu(m("k")))))))
        .unwrap();
    let b = g.do_bajtov();
    let spat = Graf::z_bajtov(&b).unwrap();
    assert_eq!(spat, g);
    assert_eq!(spat.odtlacok(), g.odtlacok());
}
