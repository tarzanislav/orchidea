//! Výber – podmnožina čohokoľvek (prvotný pojem 8) a pravidlo S5: každú operáciu možno obmedziť výberom
//! v ktorejkoľvek osi. Pre signál sa výber zhmotní do masky – signálu s hodnotami 0 až 1 – rovnako pre
//! pixely, čas, frekvencie aj vlnové dĺžky.

use crate::chyba::{Chyba, Vysledok};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::Meno;
use crate::signal::{Signal, TypPrvku};
use crate::struktura::{Cesta, Struktura};

/// Interval na jednej osi v jednotkách osi (pri poradových osiach v indexoch), s mäkkým okrajom.
#[derive(Debug, Clone, PartialEq)]
pub struct Interval {
    pub os: Meno,
    pub od: f64,
    pub po: f64,
    /// Šírka mäkkého prechodu na každom okraji (0 = ostrý okraj).
    pub makkost: f64,
}

/// Výber.
#[derive(Debug, Clone, PartialEq)]
pub enum Vyber {
    Vsetko,
    Nic,
    /// Kváder: prienik intervalov na osiach (osi, ktoré v ňom nie sú, sa berú celé).
    Oblast(Vec<Interval>),
    /// Uložený výber – maska (hodnoty 0 až 1) nad niektorými osami cieľa.
    Maska(Signal),
    /// Prvky štruktúry podľa ciest.
    Prvky(Vec<Cesta>),
    Prienik(Box<Vyber>, Box<Vyber>),
    Zjednotenie(Box<Vyber>, Box<Vyber>),
    Rozdiel(Box<Vyber>, Box<Vyber>),
    Doplnok(Box<Vyber>),
}

fn vaha_intervalu(p: f64, iv: &Interval) -> f64 {
    let (od, po) = if iv.od <= iv.po { (iv.od, iv.po) } else { (iv.po, iv.od) };
    if iv.makkost <= 0.0 {
        return if p >= od && p <= po { 1.0 } else { 0.0 };
    }
    let t = ((p - od + iv.makkost) / iv.makkost).min((po + iv.makkost - p) / iv.makkost).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Vyber {
    /// Zhmotní výber do masky s rovnakými osami ako cieľový signál (typ f32, hodnoty 0 až 1).
    pub fn maska_pre(&self, ciel: &Signal) -> Vysledok<Signal> {
        let hodnoty = self.hodnoty_masky(ciel, 0)?;
        Signal::novy(ciel.osi().to_vec(), hodnoty)
    }

    fn hodnoty_masky(&self, ciel: &Signal, hlbka: u32) -> Vysledok<Vec<f32>> {
        if hlbka > 64 {
            return Err(Chyba::Limit("výber je príliš hlboko vnorený".into()));
        }
        let n = ciel.pocet();
        Ok(match self {
            Vyber::Vsetko => vec![1.0; n],
            Vyber::Nic => vec![0.0; n],
            Vyber::Oblast(intervaly) => {
                let osi = ciel.osi();
                // Váhy pre každú os zvlášť, potom súčin – kváder je oddeliteľný.
                let mut vahy: Vec<Vec<f32>> = osi.iter().map(|o| vec![1.0f32; o.dlzka]).collect();
                for iv in intervaly {
                    let (i, o) = ciel.os(iv.os.ako_text()).ok_or_else(|| {
                        Chyba::Chybajuce(format!("výber na osi {}, ktorú signál nemá", iv.os))
                    })?;
                    for (j, v) in vahy[i].iter_mut().enumerate() {
                        *v *= vaha_intervalu(o.poloha(j), iv) as f32;
                    }
                }
                let kroky = ciel.kroky();
                (0..n)
                    .map(|idx| {
                        let mut v = 1.0f32;
                        for (d, k) in kroky.iter().enumerate() {
                            v *= vahy[d][(idx / k) % osi[d].dlzka];
                        }
                        v
                    })
                    .collect()
            }
            Vyber::Maska(m) => rozsir_masku(m, ciel)?,
            Vyber::Prvky(_) => {
                return Err(Chyba::NezhodaTypov {
                    popis: "výber prvkov štruktúry nemožno použiť na signál".into(),
                    navrh: Some("použi výber oblasti alebo masku".into()),
                });
            }
            Vyber::Prienik(a, b) => kombinuj(a, b, ciel, hlbka, |x, y| x.min(y))?,
            Vyber::Zjednotenie(a, b) => kombinuj(a, b, ciel, hlbka, |x, y| x.max(y))?,
            Vyber::Rozdiel(a, b) => kombinuj(a, b, ciel, hlbka, |x, y| x.min(1.0 - y))?,
            Vyber::Doplnok(a) => a.hodnoty_masky(ciel, hlbka + 1)?.into_iter().map(|x| 1.0 - x).collect(),
        })
    }

    /// Vybrané prvky štruktúry.
    pub fn prvky_v(&self, s: &Struktura) -> Vysledok<Vec<Cesta>> {
        let vsetky = || s.najdi(&|_| true);
        Ok(match self {
            Vyber::Vsetko => vsetky(),
            Vyber::Nic => Vec::new(),
            Vyber::Prvky(c) => c.iter().filter(|c| s.prvok(c).is_some()).cloned().collect(),
            Vyber::Prienik(a, b) => {
                let b = b.prvky_v(s)?;
                a.prvky_v(s)?.into_iter().filter(|c| b.contains(c)).collect()
            }
            Vyber::Zjednotenie(a, b) => {
                let mut v = a.prvky_v(s)?;
                for c in b.prvky_v(s)? {
                    if !v.contains(&c) {
                        v.push(c);
                    }
                }
                v
            }
            Vyber::Rozdiel(a, b) => {
                let b = b.prvky_v(s)?;
                a.prvky_v(s)?.into_iter().filter(|c| !b.contains(c)).collect()
            }
            Vyber::Doplnok(a) => {
                let a = a.prvky_v(s)?;
                vsetky().into_iter().filter(|c| !a.contains(c)).collect()
            }
            Vyber::Oblast(_) | Vyber::Maska(_) => {
                return Err(Chyba::NezhodaTypov {
                    popis: "výber oblasti alebo masky nemožno použiť na štruktúru".into(),
                    navrh: None,
                });
            }
        })
    }
}

fn kombinuj(a: &Vyber, b: &Vyber, ciel: &Signal, hlbka: u32, f: fn(f32, f32) -> f32) -> Vysledok<Vec<f32>> {
    let x = a.hodnoty_masky(ciel, hlbka + 1)?;
    let y = b.hodnoty_masky(ciel, hlbka + 1)?;
    Ok(x.into_iter().zip(y).map(|(p, q)| f(p, q)).collect())
}

/// Rozšíri masku nad niektorými osami na všetky osi cieľa (chýbajúce osi sa opakujú).
fn rozsir_masku(maska: &Signal, ciel: &Signal) -> Vysledok<Vec<f32>> {
    let osi = ciel.osi();
    let mut mapovanie = Vec::with_capacity(maska.osi().len());
    for om in maska.osi() {
        let (i, oc) = ciel
            .os(om.meno.ako_text())
            .ok_or_else(|| Chyba::NezhodaTvaru(format!("maska má os {}, ktorú cieľ nemá", om.meno)))?;
        if oc.dlzka != om.dlzka {
            return Err(Chyba::NezhodaTvaru(format!(
                "maska má na osi {} dĺžku {}, cieľ {}",
                om.meno, om.dlzka, oc.dlzka
            )));
        }
        mapovanie.push(i);
    }
    let hodnoty =
        if maska.typ() == TypPrvku::F32 { maska.ako_f32() } else { maska.preved(TypPrvku::F32).0.ako_f32() };
    let kroky_c = ciel.kroky();
    let kroky_m = maska.kroky();
    Ok((0..ciel.pocet())
        .map(|idx| {
            let mut im = 0usize;
            for (j, &i) in mapovanie.iter().enumerate() {
                im += ((idx / kroky_c[i]) % osi[i].dlzka) * kroky_m[j];
            }
            hodnoty[im].clamp(0.0, 1.0)
        })
        .collect())
}

impl Kodovatelne for Vyber {
    fn zakoduj(&self, z: &mut Zapisovac) {
        match self {
            Vyber::Vsetko => {
                z.u8(0);
            }
            Vyber::Nic => {
                z.u8(1);
            }
            Vyber::Oblast(iv) => {
                z.u8(2).cislo(iv.len() as u64);
                for i in iv {
                    z.meno(&i.os).f64(i.od).f64(i.po).f64(i.makkost);
                }
            }
            Vyber::Maska(s) => {
                z.u8(3);
                s.zakoduj(z);
            }
            Vyber::Prvky(c) => {
                z.u8(4).cislo(c.len() as u64);
                for cesta in c {
                    z.cislo(cesta.len() as u64);
                    for &i in cesta {
                        z.cislo(i as u64);
                    }
                }
            }
            Vyber::Prienik(a, b) | Vyber::Zjednotenie(a, b) | Vyber::Rozdiel(a, b) => {
                z.u8(match self {
                    Vyber::Prienik(..) => 5,
                    Vyber::Zjednotenie(..) => 6,
                    _ => 7,
                });
                a.zakoduj(z);
                b.zakoduj(z);
            }
            Vyber::Doplnok(a) => {
                z.u8(8);
                a.zakoduj(z);
            }
        }
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        c.vnor()?;
        let v = match c.u8()? {
            0 => Vyber::Vsetko,
            1 => Vyber::Nic,
            2 => {
                let n = c.pocet(26)?;
                let mut iv = Vec::with_capacity(n);
                for _ in 0..n {
                    iv.push(Interval { os: c.meno()?, od: c.f64()?, po: c.f64()?, makkost: c.f64()? });
                }
                Vyber::Oblast(iv)
            }
            3 => Vyber::Maska(Signal::dekoduj(c)?),
            4 => {
                let n = c.pocet(1)?;
                let mut cesty = Vec::with_capacity(n);
                for _ in 0..n {
                    let d = c.pocet(1)?;
                    let mut cesta = Vec::with_capacity(d);
                    for _ in 0..d {
                        cesta.push(usize::try_from(c.cislo()?).map_err(|_| Chyba::Limit("index".into()))?);
                    }
                    cesty.push(cesta);
                }
                Vyber::Prvky(cesty)
            }
            k @ 5..=7 => {
                let a = Box::new(Vyber::dekoduj(c)?);
                let b = Box::new(Vyber::dekoduj(c)?);
                match k {
                    5 => Vyber::Prienik(a, b),
                    6 => Vyber::Zjednotenie(a, b),
                    _ => Vyber::Rozdiel(a, b),
                }
            }
            8 => Vyber::Doplnok(Box::new(Vyber::dekoduj(c)?)),
            d => return Err(Chyba::PoskodeneData(format!("druh výberu {d}"))),
        };
        c.vynor();
        Ok(v)
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::meno::m;
    use crate::os::Os;

    fn obraz() -> Signal {
        let osi = vec![
            Os::poradova("y", 2).unwrap(),
            Os::poradova("x", 4).unwrap(),
            Os::oznacena("kanál", &["R", "G"]).unwrap(),
        ];
        Signal::vyplneny(osi, TypPrvku::F32, 0.0).unwrap()
    }

    #[test]
    fn oblast_maska_a_kombinacie() {
        let s = obraz();
        let iv = |os: &str, od, po| Interval { os: m(os), od, po, makkost: 0.0 };
        let v = Vyber::Oblast(vec![iv("x", 1.0, 2.0)]);
        let mk = v.maska_pre(&s).unwrap();
        assert_eq!(mk.hodnota(&[0, 1, 0]).unwrap(), 1.0);
        assert_eq!(mk.hodnota(&[1, 2, 1]).unwrap(), 1.0);
        assert_eq!(mk.hodnota(&[0, 3, 0]).unwrap(), 0.0);
        let kanal = Vyber::Oblast(vec![iv("kanál", 1.0, 1.0)]);
        let p = Vyber::Prienik(Box::new(v.clone()), Box::new(kanal)).maska_pre(&s).unwrap();
        assert_eq!(p.ako_f32().iter().sum::<f32>(), 2.0 * 2.0);
        let d = Vyber::Doplnok(Box::new(v.clone())).maska_pre(&s).unwrap();
        assert_eq!(d.ako_f32().iter().sum::<f32>(), 8.0);
        let len_x = Signal::novy(vec![Os::poradova("x", 4).unwrap()], vec![0.0f32, 0.5, 1.0, 2.0]).unwrap();
        let r = Vyber::Maska(len_x).maska_pre(&s).unwrap();
        assert_eq!(r.hodnota(&[1, 1, 1]).unwrap(), 0.5);
        assert_eq!(r.hodnota(&[1, 3, 0]).unwrap(), 1.0, "maska sa oreže na 0 až 1");
        assert!(Vyber::Oblast(vec![iv("t", 0.0, 1.0)]).maska_pre(&s).is_err());
        let makky = Vyber::Oblast(vec![Interval { os: m("x"), od: 1.0, po: 2.0, makkost: 1.0 }]);
        let mm = makky.maska_pre(&s).unwrap();
        assert_eq!(mm.hodnota(&[0, 0, 0]).unwrap(), 0.0);
        assert_eq!(mm.hodnota(&[0, 1, 0]).unwrap(), 1.0);
        let b = Vyber::Rozdiel(Box::new(v), Box::new(makky)).do_bajtov();
        assert!(Vyber::z_bajtov(&b).is_ok());
    }

    #[test]
    fn vyber_prvkov() {
        let s =
            Struktura::nova(m("a")).s_dietatom(Struktura::nova(m("b"))).s_dietatom(Struktura::nova(m("c")));
        let v = Vyber::Prvky(vec![vec![1], vec![7]]);
        assert_eq!(v.prvky_v(&s).unwrap(), vec![vec![1]]);
        assert_eq!(Vyber::Doplnok(Box::new(v)).prvky_v(&s).unwrap(), vec![vec![], vec![0]]);
    }
}
