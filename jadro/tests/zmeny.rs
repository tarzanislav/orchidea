//! Skúšky pravidla S6 (všetko je zmena), zlúčenia bez konfliktov (Pokračovanie) a denníka (A7).

use orchidea_jadro::dennik::Dennik;
use orchidea_jadro::graf::{DruhUzla, Parameter, UzolId, Zdroj};
use orchidea_jadro::hodnota::Hodnota;
use orchidea_jadro::nahoda::Nahoda;
use orchidea_jadro::operacia::PopisPortu;
use orchidea_jadro::zmena::{CielVlastnosti, zluc};
use orchidea_jadro::{Dielo, Hodiny, Kodovatelne, Meno, TypHodnoty, Uprava, Zmena, m};
use std::path::PathBuf;

fn zmena(h: &mut Hodiny, cas: u64, upravy: Vec<Uprava>) -> Zmena {
    Zmena { cas: h.tikni(cas), povodca: m("skúška"), popis: String::new(), upravy }
}

fn g() -> Meno {
    m("hlavný")
}

fn zaklad(h: &mut Hodiny) -> Vec<Zmena> {
    let u = UzolId::novy(h.replika(), 1);
    vec![zmena(
        h,
        1000,
        vec![
            Uprava::VytvorGraf { graf: g() },
            Uprava::NastavVstupGrafu {
                graf: g(),
                index: 0,
                popis: Some(PopisPortu::novy(m("vstup"), TypHodnoty::signal())),
            },
            Uprava::PridajUzol {
                graf: g(),
                uzol: u,
                druh: DruhUzla::Operacia { identita: m("orchidea.škáluj"), verzia: 1 },
            },
            Uprava::Spoj { graf: g(), uzol: u, vstup: 0, zdroj: Some(Zdroj::VstupGrafu(0)) },
            Uprava::NastavVystupGrafu { graf: g(), meno: m("výsledok"), zdroj: Some(Zdroj::uzol(u)) },
        ],
    )]
}

fn faktor(h: &mut Hodiny, cas: u64, uzol: UzolId, k: f64) -> Zmena {
    zmena(
        h,
        cas,
        vec![Uprava::NastavParameter {
            graf: g(),
            uzol,
            meno: m("faktor"),
            hodnota: Some(Parameter::Konstanta(Hodnota::Cislo(k))),
        }],
    )
}

#[test]
fn subezne_upravy_na_dvoch_zariadeniach_sa_zluca_rovnako() {
    let mut a = Hodiny::nove(1);
    let mut b = Hodiny::nove(2);
    let spolocne = zaklad(&mut a);
    let uzol = UzolId::novy(1, 1);
    b.prijmi(spolocne[0].cas);
    // súbežne: A nastaví faktor 2, B faktor 3 a pridá vlastnosť; A neskôr (podľa hodín) faktor 5
    let za = vec![faktor(&mut a, 2000, uzol, 2.0), faktor(&mut a, 2500, uzol, 5.0)];
    let zb = vec![
        faktor(&mut b, 2200, uzol, 3.0),
        zmena(
            &mut b,
            2300,
            vec![Uprava::NastavVlastnost {
                ciel: CielVlastnosti::Uzol(g(), uzol),
                meno: m("poznámka"),
                hodnota: Some(Hodnota::text("z druhého zariadenia")),
            }],
        ),
    ];
    let mut na_a: Vec<Zmena> = spolocne.iter().chain(&za).cloned().collect();
    let mut na_b: Vec<Zmena> = spolocne.iter().chain(&zb).cloned().collect();
    na_a = zluc(&na_a, &na_b);
    na_b = zluc(&na_b, &na_a);
    let (da, sa) = Dielo::prehraj(&na_a);
    let (db, _) = Dielo::prehraj(na_b.iter().rev());
    assert_eq!(da, db, "obe zariadenia majú rovnaké dielo");
    assert_eq!(sa.preskocenych, 0);
    let u = da.grafy[&g()].uzol(uzol).unwrap();
    assert_eq!(
        u.parametre[&m("faktor")],
        Parameter::Konstanta(Hodnota::Cislo(5.0)),
        "neskoršia zmena vyhráva"
    );
    assert!(u.vlastnosti.ziskaj("poznámka").is_some());

    // výpočet zo zlúčeného diela
    let p = da.do_prostredia(orchidea_jadro::Prostredie::zakladne());
    let s =
        orchidea_jadro::Signal::novy(vec![orchidea_jadro::Os::poradova("x", 2).unwrap()], vec![1.0f32, 2.0])
            .unwrap();
    let out =
        p.vyhodnot(&da.grafy[&g()], &[Hodnota::Signal(s)], &Default::default(), &Default::default()).unwrap();
    assert_eq!(out[0].ako_signal().unwrap().ako_f32(), vec![5.0, 10.0]);
}

#[test]
fn zmena_sa_pouzije_cela_alebo_vobec() {
    let mut a = Hodiny::nove(1);
    let mut zmeny = zaklad(&mut a);
    let uzol = UzolId::novy(1, 1);
    // B odstráni uzol; súbežná zmena A ho chce upraviť a zároveň premenovať výstup – celá sa preskočí
    zmeny.push(zmena(&mut a, 3000, vec![Uprava::OdstranUzol { graf: g(), uzol }]));
    zmeny.push(zmena(
        &mut a,
        3100,
        vec![
            Uprava::NastavVystupGrafu { graf: g(), meno: m("druhý"), zdroj: None },
            Uprava::NastavVypnutie { graf: g(), uzol, vypnuty: true },
        ],
    ));
    let (d, s) = Dielo::prehraj(&zmeny);
    assert_eq!(s.preskocenych, 2);
    assert!(
        d.grafy[&g()].vystupy.iter().all(|(n, _)| n.ako_text() != "druhý"),
        "polovičná zmena nesmie ostať"
    );
}

/// Dočasný súbor pre skúšku. Na webe (WASI) sa používa sprístupnený pracovný priečinok.
fn docasny(meno: &str) -> PathBuf {
    let mut p = if cfg!(target_family = "wasm") { PathBuf::from(".") } else { std::env::temp_dir() };
    let cas =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    p.push(format!("orchidea-skuska-{cas}-{meno}"));
    let _ = std::fs::remove_file(&p);
    let mut z = p.clone().into_os_string();
    z.push(".zvysok");
    let _ = std::fs::remove_file(PathBuf::from(z));
    p
}

#[test]
fn dennik_prezije_vypadok_pocas_zapisu() {
    let cesta = docasny("dennik");
    let mut h = Hodiny::nove(7);
    let mut zmeny = zaklad(&mut h);
    let uzol = UzolId::novy(7, 1);
    for i in 0..20 {
        zmeny.push(faktor(&mut h, 5000 + i, uzol, i as f64));
    }
    {
        let (mut d, s) = Dennik::otvor(&cesta).unwrap();
        assert_eq!(s.zmien, 0);
        assert_eq!(d.zluc(&zmeny).unwrap(), zmeny.len());
        assert_eq!(d.zluc(&zmeny).unwrap(), 0, "rovnaké zmeny sa nezapíšu dvakrát");
    }
    let plna = std::fs::metadata(&cesta).unwrap().len();
    // výpadok: posledný záznam zapísaný len do polovice
    let f = std::fs::OpenOptions::new().write(true).open(&cesta).unwrap();
    f.set_len(plna - 20).unwrap();
    drop(f);
    let (mut d, s) = Dennik::otvor(&cesta).unwrap();
    assert_eq!(s.zmien, zmeny.len() - 1);
    assert!(s.poskodeny_koniec > 0);
    // ďalší zápis odreže poškodený koniec, ale najprv ho uloží vedľa
    let nova = faktor(&mut h, 9000, uzol, 42.0);
    assert!(d.zapis(&nova).unwrap());
    drop(d);
    let mut zvysok = cesta.clone().into_os_string();
    zvysok.push(".zvysok");
    assert_eq!(std::fs::metadata(PathBuf::from(&zvysok)).unwrap().len(), s.poskodeny_koniec);
    let (d, s2) = Dennik::otvor(&cesta).unwrap();
    assert_eq!(s2.poskodeny_koniec, 0);
    assert_eq!(s2.zmien, zmeny.len());
    let (dielo, _) = Dielo::prehraj(d.zmeny());
    let p = &dielo.grafy[&g()].uzol(uzol).unwrap().parametre[&m("faktor")];
    assert_eq!(*p, Parameter::Konstanta(Hodnota::Cislo(42.0)));
    // poškodený bajt uprostred: platné ostanú zmeny pred ním
    let mut data = std::fs::read(&cesta).unwrap();
    let stred = data.len() / 2;
    data[stred] ^= 0xff;
    std::fs::write(&cesta, &data).unwrap();
    let (_, s3) = Dennik::otvor(&cesta).unwrap();
    assert!(s3.zmien < zmeny.len() && s3.zmien > 0);
    assert!(s3.poskodeny_koniec > 0);
    let _ = std::fs::remove_file(&cesta);
    let _ = std::fs::remove_file(PathBuf::from(zvysok));
}

#[test]
fn cudzi_subor_nie_je_dennik() {
    let cesta = docasny("cudzi");
    std::fs::write(&cesta, b"toto nie je denn\xedk").unwrap();
    assert!(Dennik::otvor(&cesta).is_err());
    let _ = std::fs::remove_file(&cesta);
}

/// Vlastný fuzzer: náhodné a pozmenené bajty nesmú spôsobiť pád, len chybu.
#[test]
fn fuzzer_kodovania() {
    let mut h = Hodiny::nove(3);
    let platne: Vec<Vec<u8>> = zaklad(&mut h).iter().map(|z| z.do_bajtov()).collect();
    let mut n = Nahoda::zo_semienka(2026);
    let pokusov = if cfg!(debug_assertions) { 20_000 } else { 100_000 };
    for i in 0..pokusov {
        let data: Vec<u8> = if i % 2 == 0 {
            let dlzka = n.mensie_ako(200) as usize;
            (0..dlzka).map(|_| n.dalsie() as u8).collect()
        } else {
            let mut d = platne[0].clone();
            for _ in 0..1 + n.mensie_ako(4) {
                let j = n.mensie_ako(d.len() as u64) as usize;
                d[j] = n.dalsie() as u8;
            }
            if n.mensie_ako(3) == 0 {
                d.truncate(n.mensie_ako(d.len() as u64) as usize);
            }
            d
        };
        let _ = Zmena::z_bajtov(&data);
        let _ = orchidea_jadro::Graf::z_bajtov(&data);
        let _ = orchidea_jadro::Signal::z_bajtov(&data);
        let _ = Hodnota::z_bajtov(&data);
        let _ = orchidea_jadro::Vyber::z_bajtov(&data);
        if let Ok(t) = std::str::from_utf8(&data) {
            let _ = orchidea_jadro::vyraz::Vyraz::novy(t);
        }
    }
}
