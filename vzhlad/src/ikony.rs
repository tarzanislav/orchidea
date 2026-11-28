//! Vlastné vektorové ikony ORCHIDEE – ťahy na jednej mriežke 20 × 20 bodov (živá plocha 2,5–17,5), jedna hrúbka
//! ťahu (`miery::TAH_IKONY`) s okrúhlymi koncami. Pri inej veľkosti sa ťah mení v pomere – všetky ikony vedľa
//! seba majú rovnakú váhu. Vyplnené tvary (trojuholník Prehrať, hviezda) sú mnohouholníky.

/// Ikona – meno je zároveň popis v bublinách.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ikona {
    Otvorit,
    OtocVlavo,
    OtocVpravo,
    Zrkadlit,
    Orezat,
    Spat,
    Znova,
    Porovnat,
    Prezentacia,
    Katalog,
    Ulozit,
    Napoveda,
    Prehrat,
    Pauza,
    Predosla,
    Dalsia,
    Hlasitost,
    Nahodne,
    Opakovat,
    Hladat,
    Nastavenia,
    Info,
    Zavriet,
    Dozadu,
    Domov,
    Film,
    Hudba,
    Fotka,
    Titulky,
    Plus,
    Upravy,
    Zoznam,
    Tema,
    Hviezda,
    /// neobsadená hviezda hodnotenia – obrys (stav sa rozlíši tvarom, nielen farbou)
    HviezdaPrazdna,
}

/// Všetky ikony (na list dizajnového systému a skúšky).
pub const VSETKY: [Ikona; 35] = [
    Ikona::Otvorit,
    Ikona::OtocVlavo,
    Ikona::OtocVpravo,
    Ikona::Zrkadlit,
    Ikona::Orezat,
    Ikona::Spat,
    Ikona::Znova,
    Ikona::Porovnat,
    Ikona::Prezentacia,
    Ikona::Katalog,
    Ikona::Ulozit,
    Ikona::Napoveda,
    Ikona::Prehrat,
    Ikona::Pauza,
    Ikona::Predosla,
    Ikona::Dalsia,
    Ikona::Hlasitost,
    Ikona::Nahodne,
    Ikona::Opakovat,
    Ikona::Hladat,
    Ikona::Nastavenia,
    Ikona::Info,
    Ikona::Zavriet,
    Ikona::Dozadu,
    Ikona::Domov,
    Ikona::Film,
    Ikona::Hudba,
    Ikona::Fotka,
    Ikona::Titulky,
    Ikona::Plus,
    Ikona::Upravy,
    Ikona::Zoznam,
    Ikona::Tema,
    Ikona::Hviezda,
    Ikona::HviezdaPrazdna,
];

/// Lomená čiara – body na mriežke 20 × 20.
pub type Tah = Vec<(f32, f32)>;

/// Oblúk kružnice (uhly v stupňoch, v smere hodinových ručičiek, 0° = vpravo).
pub fn obluk(cx: f32, cy: f32, r: f32, od: f32, po: f32) -> Tah {
    let n = (((po - od).abs() / 15.0).ceil() as usize).max(4);
    (0..=n)
        .map(|i| {
            let u = (od + (po - od) * i as f32 / n as f32).to_radians();
            (cx + r * u.cos(), cy + r * u.sin())
        })
        .collect()
}

fn obdlznik(x0: f32, y0: f32, x1: f32, y1: f32) -> Tah {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]
}

/// Päťcípa hviezda (vonkajší polomer 8, vnútorný 3,4; ťažisko opticky o 0,6 nižšie), zmenšená `k`-krát.
fn hviezda(k: f32) -> Tah {
    (0..10)
        .map(|j| {
            let u = (-90.0 + j as f32 * 36.0f32).to_radians();
            let r = k * if j % 2 == 0 { 8.0 } else { 3.4 };
            (10.0 + r * u.cos(), 10.6 + r * u.sin())
        })
        .collect()
}

/// Bodka (krátky ťah – okrúhly koniec spraví bod).
fn bodka(x: f32, y: f32) -> Tah {
    vec![(x, y), (x, y + 0.01)]
}

impl Ikona {
    /// Popis ikony (bublina, nápoveda).
    pub fn meno(self) -> &'static str {
        match self {
            Ikona::Otvorit => "Otvoriť",
            Ikona::OtocVlavo => "Otočiť doľava",
            Ikona::OtocVpravo => "Otočiť doprava",
            Ikona::Zrkadlit => "Zrkadliť",
            Ikona::Orezat => "Orezať",
            Ikona::Spat => "Späť",
            Ikona::Znova => "Znova",
            Ikona::Porovnat => "Porovnať",
            Ikona::Prezentacia => "Prezentácia",
            Ikona::Katalog => "Katalóg",
            Ikona::Ulozit => "Uložiť",
            Ikona::Napoveda => "Nápoveda",
            Ikona::Prehrat => "Prehrať",
            Ikona::Pauza => "Pauza",
            Ikona::Predosla => "Predošlá",
            Ikona::Dalsia => "Ďalšia",
            Ikona::Hlasitost => "Hlasitosť",
            Ikona::Nahodne => "Náhodne",
            Ikona::Opakovat => "Opakovať",
            Ikona::Hladat => "Hľadať",
            Ikona::Nastavenia => "Nastavenia",
            Ikona::Info => "Informácie",
            Ikona::Zavriet => "Zavrieť",
            Ikona::Dozadu => "Späť (dozadu)",
            Ikona::Domov => "Domov",
            Ikona::Film => "Filmy",
            Ikona::Hudba => "Hudba",
            Ikona::Fotka => "Fotky",
            Ikona::Titulky => "Titulky",
            Ikona::Plus => "Pridať",
            Ikona::Upravy => "Úpravy",
            Ikona::Zoznam => "Zoznam",
            Ikona::Tema => "Téma vzhľadu",
            Ikona::Hviezda => "Hodnotenie",
            Ikona::HviezdaPrazdna => "Bez hodnotenia",
        }
    }

    /// Ťahy ikony.
    pub fn tahy(self) -> Vec<Tah> {
        match self {
            Ikona::Otvorit => vec![vec![
                (2.5, 5.5),
                (8.0, 5.5),
                (10.0, 7.5),
                (17.5, 7.5),
                (17.5, 15.5),
                (2.5, 15.5),
                (2.5, 5.5),
            ]],
            Ikona::OtocVpravo => {
                vec![obluk(10.0, 11.0, 6.0, 200.0, 470.0), vec![(14.5, 3.5), (16.0, 7.3), (12.0, 7.8)]]
            }
            Ikona::OtocVlavo => {
                vec![obluk(10.0, 11.0, 6.0, -110.0, 160.0), vec![(5.5, 3.5), (4.0, 7.3), (8.0, 7.8)]]
            }
            Ikona::Zrkadlit => vec![
                vec![(10.0, 2.5), (10.0, 4.5)],
                vec![(10.0, 7.5), (10.0, 9.5)],
                vec![(10.0, 12.5), (10.0, 14.5)],
                vec![(10.0, 16.5), (10.0, 17.5)],
                vec![(7.5, 5.0), (2.5, 15.0), (7.5, 15.0), (7.5, 5.0)],
                vec![(12.5, 5.0), (17.5, 15.0), (12.5, 15.0), (12.5, 5.0)],
            ],
            Ikona::Orezat => {
                vec![vec![(5.5, 2.5), (5.5, 14.5), (17.5, 14.5)], vec![(2.5, 5.5), (14.5, 5.5), (14.5, 17.5)]]
            }
            Ikona::Spat => {
                vec![obluk(11.0, 11.5, 5.5, 180.0, 400.0), vec![(3.0, 8.0), (5.5, 11.5), (8.5, 9.0)]]
            }
            Ikona::Znova => {
                vec![obluk(9.0, 11.5, 5.5, -40.0, 180.0), vec![(17.0, 8.0), (14.5, 11.5), (11.5, 9.0)]]
            }
            Ikona::Porovnat => vec![obdlznik(2.5, 4.5, 17.5, 15.5), vec![(10.0, 2.5), (10.0, 17.5)]],
            Ikona::Prezentacia => vec![
                obdlznik(2.5, 4.0, 17.5, 14.0),
                vec![(8.5, 6.5), (12.5, 9.0), (8.5, 11.5), (8.5, 6.5)],
                vec![(7.0, 17.0), (13.0, 17.0)],
            ],
            Ikona::Katalog => vec![
                obdlznik(3.0, 3.0, 8.5, 8.5),
                obdlznik(11.5, 3.0, 17.0, 8.5),
                obdlznik(3.0, 11.5, 8.5, 17.0),
                obdlznik(11.5, 11.5, 17.0, 17.0),
            ],
            Ikona::Ulozit => vec![
                vec![(10.0, 3.0), (10.0, 12.0)],
                vec![(6.0, 8.5), (10.0, 12.5), (14.0, 8.5)],
                vec![(3.0, 13.0), (3.0, 17.0), (17.0, 17.0), (17.0, 13.0)],
            ],
            Ikona::Napoveda => vec![
                obluk(10.0, 10.0, 7.5, 0.0, 360.0),
                obluk(10.0, 8.0, 2.5, 180.0, 400.0),
                vec![(11.9, 9.6), (10.0, 11.0), (10.0, 12.0)],
                bodka(10.0, 14.5),
            ],
            Ikona::Prehrat | Ikona::Hviezda => vec![],
            Ikona::Pauza => vec![vec![(7.0, 4.5), (7.0, 15.5)], vec![(13.0, 4.5), (13.0, 15.5)]],
            Ikona::Predosla => {
                vec![vec![(5.0, 4.5), (5.0, 15.5)], vec![(15.5, 4.5), (8.0, 10.0), (15.5, 15.5), (15.5, 4.5)]]
            }
            Ikona::Dalsia => {
                vec![vec![(15.0, 4.5), (15.0, 15.5)], vec![(4.5, 4.5), (12.0, 10.0), (4.5, 15.5), (4.5, 4.5)]]
            }
            Ikona::Hlasitost => vec![
                vec![(3.0, 7.5), (6.0, 7.5), (10.0, 4.0), (10.0, 16.0), (6.0, 12.5), (3.0, 12.5), (3.0, 7.5)],
                obluk(10.0, 10.0, 4.0, -45.0, 45.0),
                obluk(10.0, 10.0, 7.0, -45.0, 45.0),
            ],
            Ikona::Nahodne => vec![
                vec![(2.5, 6.0), (6.0, 6.0), (13.0, 14.0), (17.0, 14.0)],
                vec![(2.5, 14.0), (6.0, 14.0), (13.0, 6.0), (17.0, 6.0)],
                vec![(15.0, 4.0), (17.0, 6.0), (15.0, 8.0)],
                vec![(15.0, 12.0), (17.0, 14.0), (15.0, 16.0)],
            ],
            Ikona::Opakovat => vec![
                vec![(4.0, 9.0), (4.0, 6.0), (15.5, 6.0)],
                vec![(13.5, 4.0), (15.5, 6.0), (13.5, 8.0)],
                vec![(16.0, 11.0), (16.0, 14.0), (4.5, 14.0)],
                vec![(6.5, 12.0), (4.5, 14.0), (6.5, 16.0)],
            ],
            Ikona::Hladat => vec![obluk(8.5, 8.5, 5.5, 0.0, 360.0), vec![(12.5, 12.5), (17.0, 17.0)]],
            Ikona::Nastavenia => {
                let mut v = vec![obluk(10.0, 10.0, 2.6, 0.0, 360.0), obluk(10.0, 10.0, 5.6, 0.0, 360.0)];
                for i in 0..8 {
                    let u = (i as f32 * 45.0f32).to_radians();
                    v.push(vec![
                        (10.0 + 5.6 * u.cos(), 10.0 + 5.6 * u.sin()),
                        (10.0 + 7.6 * u.cos(), 10.0 + 7.6 * u.sin()),
                    ]);
                }
                v
            }
            Ikona::Info => {
                vec![obluk(10.0, 10.0, 7.5, 0.0, 360.0), vec![(10.0, 9.0), (10.0, 14.0)], bodka(10.0, 6.0)]
            }
            Ikona::Zavriet => vec![vec![(5.0, 5.0), (15.0, 15.0)], vec![(15.0, 5.0), (5.0, 15.0)]],
            Ikona::Dozadu => {
                vec![vec![(16.5, 10.0), (3.5, 10.0)], vec![(8.5, 5.0), (3.5, 10.0), (8.5, 15.0)]]
            }
            Ikona::Domov => vec![
                vec![(2.5, 9.5), (10.0, 3.0), (17.5, 9.5)],
                vec![(4.5, 8.0), (4.5, 17.0), (15.5, 17.0), (15.5, 8.0)],
                vec![(8.5, 17.0), (8.5, 12.0), (11.5, 12.0), (11.5, 17.0)],
            ],
            Ikona::Film => vec![
                obdlznik(2.5, 3.5, 17.5, 16.5),
                vec![(6.5, 3.5), (6.5, 16.5)],
                vec![(13.5, 3.5), (13.5, 16.5)],
                vec![(2.5, 7.8), (6.5, 7.8)],
                vec![(2.5, 12.2), (6.5, 12.2)],
                vec![(13.5, 7.8), (17.5, 7.8)],
                vec![(13.5, 12.2), (17.5, 12.2)],
            ],
            Ikona::Hudba => vec![
                vec![(7.5, 14.0), (7.5, 5.5), (16.0, 3.5), (16.0, 12.0)],
                obluk(5.5, 14.5, 2.0, 0.0, 360.0),
                obluk(14.0, 12.5, 2.0, 0.0, 360.0),
            ],
            Ikona::Fotka => vec![
                obdlznik(2.5, 4.5, 17.5, 15.5),
                vec![(2.5, 13.5), (7.0, 9.0), (10.5, 12.5), (12.5, 10.5), (17.5, 15.0)],
                obluk(13.5, 7.8, 1.4, 0.0, 360.0),
            ],
            Ikona::Titulky => vec![
                obdlznik(2.5, 4.5, 17.5, 15.5),
                vec![(5.5, 10.0), (9.0, 10.0)],
                vec![(11.0, 10.0), (14.5, 10.0)],
                vec![(5.5, 12.8), (14.5, 12.8)],
            ],
            Ikona::Plus => vec![vec![(10.0, 4.0), (10.0, 16.0)], vec![(4.0, 10.0), (16.0, 10.0)]],
            Ikona::Upravy => vec![
                vec![(2.5, 6.0), (10.2, 6.0)],
                vec![(13.8, 6.0), (17.5, 6.0)],
                obluk(12.0, 6.0, 1.8, 0.0, 360.0),
                vec![(2.5, 10.0), (5.2, 10.0)],
                vec![(8.8, 10.0), (17.5, 10.0)],
                obluk(7.0, 10.0, 1.8, 0.0, 360.0),
                vec![(2.5, 14.0), (11.2, 14.0)],
                vec![(14.8, 14.0), (17.5, 14.0)],
                obluk(13.0, 14.0, 1.8, 0.0, 360.0),
            ],
            Ikona::Zoznam => vec![
                vec![(7.0, 5.5), (17.0, 5.5)],
                vec![(7.0, 10.0), (17.0, 10.0)],
                vec![(7.0, 14.5), (17.0, 14.5)],
                bodka(3.5, 5.5),
                bodka(3.5, 10.0),
                bodka(3.5, 14.5),
            ],
            Ikona::Tema => vec![obluk(10.0, 10.0, 7.5, 0.0, 360.0)],
            // obrys o polovicu ťahu dnu – vonkajší okraj sedí s plnou hviezdou
            Ikona::HviezdaPrazdna => {
                let mut b = hviezda(1.0 - 0.85 / 8.0);
                b.push(b[0]);
                vec![b]
            }
        }
    }

    /// Vyplnená časť ikony (mnohouholník), ak ju má.
    pub fn vypln(self) -> Option<Tah> {
        match self {
            // trojuholník je o 0,8 bodu vpravo od geometrického stredu – opticky stojí v strede kruhu
            Ikona::Prehrat => Some(vec![(7.3, 4.5), (16.3, 10.0), (7.3, 15.5)]),
            Ikona::Hviezda => Some(hviezda(1.0)),
            Ikona::Tema => Some(obluk(10.0, 10.0, 7.5, 90.0, 270.0)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn ikony_su_na_mriezke_a_v_zivej_ploche() {
        // ťah s polovičnou hrúbkou nesmie vyjsť z mriežky 20 × 20
        for ik in VSETKY {
            let mut body: Vec<(f32, f32)> = ik.tahy().into_iter().flatten().collect();
            body.extend(ik.vypln().unwrap_or_default());
            assert!(!body.is_empty(), "{ik:?} nemá ťahy");
            for (x, y) in body {
                assert!(
                    (1.5..=18.5).contains(&x) && (1.5..=18.5).contains(&y),
                    "{ik:?}: bod ({x}, {y}) mimo plochy"
                );
            }
        }
    }

    #[test]
    fn mena_ikon_su_jedinecne() {
        for (i, a) in VSETKY.iter().enumerate() {
            assert!(VSETKY[i + 1..].iter().all(|b| b.meno() != a.meno()), "{a:?}");
        }
    }
}
