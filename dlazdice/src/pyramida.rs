//! Pyramída náhľadu (R71, FOTOSYNTÉZA Z7) – úrovne s polovičnými rozmermi až po jeden pixel.
//!
//! Pixel úrovne k+1 je priemer štvorca 2×2 pixelov úrovne k; na nepárnom okraji sa krajný pixel zopakuje.
//! Súčet sa robí v pevnom poradí v type f64 (pre celé čísla presne), preto je výsledok bitovo rovnaký na každom
//! zariadení, pri každej strane dlaždice a pri každom počte vlákien. Po úprave obrazu sa prepočítajú len
//! dlaždice nad zmenenými dlaždicami ([`Pyramida::obnov`]).
//!
//! Priemer sa počíta z uložených hodnôt; s pribudnutím správy farieb (E1, krok 4) sa náhľad z hodnôt
//! v nelineárnom kódovaní bude počítať v lineárnom svetle. Náhľad je vždy označený úrovňou – export ide z plnej.

use crate::dlazdica::Dlazdica;
use crate::{Obdlznik, Rastr};
use orchidea_jadro::signal::{PrvokSignalu, Signal};
use orchidea_jadro::{Chyba, Vysledok, paralel};
use std::sync::Arc;

/// Pyramída náhľadu: úroveň 0 je celý obraz, každá ďalšia má polovičné rozmery (zaokrúhlené nahor).
#[derive(Clone)]
pub struct Pyramida<T> {
    urovne: Vec<Rastr<T>>,
    prepocitane: usize,
}

impl<T: PrvokSignalu> std::fmt::Debug for Pyramida<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pyramída {} úrovní nad {:?}", self.urovne.len(), self.urovne[0])
    }
}

/// Zdroj riadku dieťaťa: rozbalený riadok alebo jednoliaty pixel.
enum Riadok<'a, T> {
    Data(&'a [T]),
    Jeden(&'a [T]),
}

impl<T: Copy> Riadok<'_, T> {
    #[inline]
    fn hodnota(&self, x: usize, kanal: usize, kanaly: usize) -> T {
        match self {
            Riadok::Data(d) => d[x * kanaly + kanal],
            Riadok::Jeden(p) => p[kanal],
        }
    }
}

fn riadok<T: PrvokSignalu>(d: &Dlazdica<T>, y: usize) -> Riadok<'_, T> {
    match d.riadok(y) {
        Some(r) => Riadok::Data(r),
        None => Riadok::Jeden(d.pixel(0, 0)),
    }
}

/// Indexy dlaždíc nižšej úrovne pod dlaždicou `(c, r)` vyššej úrovne (najviac štyri).
fn deti<T: PrvokSignalu>(nizsia: &Rastr<T>, c: usize, r: usize) -> impl Iterator<Item = usize> + use<T> {
    let (stlpcov, riadkov) = nizsia.mriezka();
    (0..2).flat_map(move |dr| {
        (0..2).filter_map(move |dc| {
            let (cc, rr) = (2 * c + dc, 2 * r + dr);
            (cc < stlpcov && rr < riadkov).then_some(rr * stlpcov + cc)
        })
    })
}

/// Spočíta dlaždicu `i` vyššej úrovne zo štyroch dlaždíc nižšej úrovne. S podkladom `(oblasť, stará dlaždica)`
/// sa prepočíta len oblasť (v súradniciach dlaždice) a zvyšok sa prevezme zo starej dlaždice – pixely mimo
/// oblasti závisia len od nezmenených pixelov, preto je výsledok bitovo rovnaký ako úplný výpočet.
fn zmensi<T: PrvokSignalu>(
    nizsia: &Rastr<T>,
    vyssia: &Rastr<T>,
    i: usize,
    podklad: Option<(Obdlznik, &Dlazdica<T>)>,
) -> Dlazdica<T> {
    let (x0, y0, w, h) = vyssia.miesto(i);
    let k = nizsia.kanaly();
    let s = nizsia.strana();
    let (sw, sh) = (nizsia.sirka(), nizsia.vyska());
    let (stlpcov, _) = nizsia.mriezka();
    let (c, r) = (x0 / s, y0 / s);
    let dlazdice = nizsia.dlazdice();

    // všetky deti jednoliate s rovnakou hodnotou → aj výsledok (priemer rovnakých hodnôt je presne tá hodnota)
    let mut d = deti(nizsia, c, r).map(|j| &dlazdice[j]);
    let prve = d.next().expect("každá dlaždica má aspoň jedno dieťa");
    if prve.je_jednoliata() {
        let p = prve.pixel(0, 0);
        let rovnake = |q: &Arc<Dlazdica<T>>| {
            q.je_jednoliata() && q.pixel(0, 0).iter().zip(p).all(|(&a, &b)| crate::dlazdica::rovnake(a, b))
        };
        if d.all(rovnake) {
            return Dlazdica::jednoliata(w, h, p);
        }
    }

    let (oblast, mut vysledok) = match podklad {
        Some((o, stara)) => (o, stara.clone()),
        None => (Obdlznik::novy(0, 0, w, h), Dlazdica::jednoliata(w, h, prve.pixel(0, 0))),
    };
    let out = vysledok.na_zapis();
    for py in oblast.y..oblast.koniec_y() {
        let ya = 2 * (y0 + py);
        let yb = (ya + 1).min(sh - 1);
        // ya je párne a strana párna – oba riadky ležia v tom istom riadku dlaždíc
        let tr = ya / s;
        let (lya, lyb) = (ya % s, yb % s);
        let riadky: Vec<(Riadok<'_, T>, Riadok<'_, T>)> = (0..2)
            .filter(|dc| 2 * c + dc < stlpcov)
            .map(|dc| {
                let t = &dlazdice[tr * stlpcov + 2 * c + dc];
                (riadok(t, lya), riadok(t, lyb))
            })
            .collect();
        let ciel = &mut out[(py * w + oblast.x) * k..(py * w + oblast.koniec_x()) * k];
        for (px, pixel) in (oblast.x..oblast.koniec_x()).zip(ciel.chunks_exact_mut(k)) {
            let xa = 2 * (x0 + px);
            let (dc, lxa) = (xa / s - 2 * c, xa % s);
            let lxb = if xa + 1 < sw { lxa + 1 } else { lxa };
            let (ra, rb) = &riadky[dc];
            for (ch, v) in pixel.iter_mut().enumerate() {
                let sucet = ((ra.hodnota(lxa, ch, k).do_f64() + ra.hodnota(lxb, ch, k).do_f64())
                    + rb.hodnota(lxa, ch, k).do_f64())
                    + rb.hodnota(lxb, ch, k).do_f64();
                *v = T::z_f64(sucet * 0.25).0;
            }
        }
    }
    vysledok.zhustni();
    vysledok
}

/// Zmenené oblasti dlaždíc úrovne 0 (v súradniciach obrazu) oproti starej verzii; `None` = dlaždica sa nezmenila.
/// Zdieľané dlaždice sa neporovnávajú. Ak sa zmenila viac ako polovica dlaždíc, porovnávanie by stálo viac,
/// ako ušetrí – zmenené sa berú celé.
fn zmeny_urovne_0<T: PrvokSignalu>(novy: &Rastr<T>, stary: &Rastr<T>) -> Option<Vec<Option<Obdlznik>>> {
    let zmenene = novy.zmenene_voci(stary)?;
    let n = zmenene.len();
    let cele = zmenene.iter().filter(|&&z| z).count() * 2 > n;
    let kus = (n / (4 * paralel::pocet_vlakien())).max(1);
    let zmeny = paralel::mapuj_rozsah(n, kus, |rozsah| {
        rozsah
            .map(|i| {
                if !zmenene[i] {
                    return None;
                }
                let (x, y, w, h) = novy.miesto(i);
                if cele {
                    return Some(Obdlznik::novy(x, y, w, h));
                }
                novy.dlazdice()[i]
                    .rozdiel(&stary.dlazdice()[i])
                    .map(|o| Obdlznik::novy(x + o.x, y + o.y, o.sirka, o.vyska))
            })
            .collect::<Vec<_>>()
    });
    Some(zmeny.into_iter().flatten().collect())
}

impl<T: PrvokSignalu> Pyramida<T> {
    /// Postaví celú pyramídu obrazu.
    pub fn postav(obraz: &Rastr<T>) -> Pyramida<T> {
        Pyramida::obnov_z(None, obraz)
    }

    /// Pyramída novej verzie obrazu: dlaždice, pod ktorými sa nič nezmenilo, sa prevezmú z tejto pyramídy.
    /// Výsledok je bitovo rovnaký ako [`Pyramida::postav`].
    pub fn obnov(&self, obraz: &Rastr<T>) -> Pyramida<T> {
        Pyramida::obnov_z(Some(self), obraz)
    }

    fn obnov_z(stara: Option<&Pyramida<T>>, obraz: &Rastr<T>) -> Pyramida<T> {
        let stara = stara.filter(|s| s.urovne[0].rovnaka_mriezka(obraz));
        let mut zmeny = stara.and_then(|s| zmeny_urovne_0(obraz, &s.urovne[0]));
        let mut urovne = vec![obraz.clone()];
        let mut prepocitane = 0;
        loop {
            let nizsia = urovne.last().expect("úroveň 0 je vždy");
            if nizsia.sirka() <= 1 && nizsia.vyska() <= 1 {
                break;
            }
            let mut vyssia = nizsia
                .s_rozmermi_ako(nizsia.sirka().div_ceil(2), nizsia.vyska().div_ceil(2))
                .expect("menšie rozmery sú vždy platné");
            let stara_uroven = stara.map(|s| &s.urovne[urovne.len()]);
            let (stlpcov, riadkov) = vyssia.mriezka();
            let n = stlpcov * riadkov;
            let kus = (n / (4 * paralel::pocet_vlakien())).max(1);
            let (vyssia_ref, zmeny_ref) = (&vyssia, &zmeny);
            type Vysledok<T> = (Arc<Dlazdica<T>>, Option<Obdlznik>, bool);
            let vysledky: Vec<Vysledok<T>> = paralel::mapuj_rozsah(n, kus, |rozsah| {
                rozsah
                    .map(|i| {
                        let (c, r) = (i % stlpcov, i / stlpcov);
                        let (x, y, w, h) = vyssia_ref.miesto(i);
                        let miesto = Obdlznik::novy(x, y, w, h);
                        match (stara_uroven, zmeny_ref) {
                            (Some(su), Some(zm)) => {
                                let zmena = deti(nizsia, c, r)
                                    .filter_map(|j| zm[j])
                                    .map(|o| o.polovica().prienik(&miesto))
                                    .fold(Obdlznik::novy(0, 0, 0, 0), |a, b| a.obal(&b));
                                if zmena.je_prazdny() {
                                    return (su.dlazdice()[i].clone(), None, false);
                                }
                                let lok = Obdlznik::novy(zmena.x - x, zmena.y - y, zmena.sirka, zmena.vyska);
                                let d = zmensi(nizsia, vyssia_ref, i, Some((lok, &su.dlazdice()[i])));
                                (Arc::new(d), Some(zmena), true)
                            }
                            _ => (Arc::new(zmensi(nizsia, vyssia_ref, i, None)), None, true),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .collect();
            prepocitane += vysledky.iter().filter(|v| v.2).count();
            zmeny = stara.map(|_| vysledky.iter().map(|v| v.1).collect());
            vyssia.nastav_dlazdice(vysledky.into_iter().map(|v| v.0).collect());
            vyssia.zdielaj_jednoliate();
            urovne.push(vyssia);
        }
        Pyramida { urovne, prepocitane }
    }

    /// Úrovne od plného obrazu po jeden pixel.
    pub fn urovne(&self) -> &[Rastr<T>] {
        &self.urovne
    }

    /// Koľko dlaždíc vyšších úrovní sa pri stavbe naozaj spočítalo (ostatné sa prevzali).
    pub fn prepocitane(&self) -> usize {
        self.prepocitane
    }

    /// Úroveň pre mierku zobrazenia (pixely obrazovky na pixel obrazu): najmenšia úroveň, ktorá má ešte aspoň
    /// rozlíšenie obrazovky – náhľad sa z nej zmenší najviac dvakrát a nikdy sa nezväčšuje z hrubšej úrovne.
    pub fn uroven_pre_mierku(&self, mierka: f64) -> usize {
        let mut k = 0;
        while k + 1 < self.urovne.len() && 0.5f64.powi(k as i32 + 1) >= mierka {
            k += 1;
        }
        k
    }

    /// Náhľad oblasti obrazu (v súradniciach plného obrazu) pre mierku zobrazenia: vráti úroveň a jej výrez.
    /// Čítajú sa len dlaždice, ktoré do výrezu zasahujú.
    pub fn nahlad(&self, oblast: Obdlznik, mierka: f64) -> Vysledok<(usize, Signal)> {
        let oblast = oblast.prienik(&self.urovne[0].cely());
        if oblast.je_prazdny() {
            return Err(Chyba::MimoRozsahu("náhľad mimo obrazu".into()));
        }
        let k = self.uroven_pre_mierku(mierka);
        let u = &self.urovne[k];
        let (x0, y0) = (oblast.x >> k, oblast.y >> k);
        let x1 = oblast.koniec_x().div_ceil(1 << k).min(u.sirka());
        let y1 = oblast.koniec_y().div_ceil(1 << k).min(u.vyska());
        Ok((k, u.do_signalu(Obdlznik::novy(x0, y0, x1 - x0, y1 - y0))?))
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;
    use orchidea_jadro::os::Os;

    fn obraz<T: PrvokSignalu>(
        sirka: usize,
        vyska: usize,
        kanaly: usize,
        semienko: u64,
        f: impl Fn(u64) -> T,
    ) -> Signal {
        let mut n = Nahoda::zo_semienka(semienko);
        let osi = vec![
            Os::poradova("y", vyska).unwrap(),
            Os::poradova("x", sirka).unwrap(),
            Os::poradova("kanál", kanaly).unwrap(),
        ];
        Signal::novy(osi, (0..sirka * vyska * kanaly).map(|_| f(n.dalsie())).collect()).unwrap()
    }

    /// Priama definícia jednej úrovne na celom poli (rozhodca).
    fn zmensi_priamo(d: &[f64], w: usize, h: usize, k: usize) -> (Vec<f64>, usize, usize) {
        let (w2, h2) = (w.div_ceil(2), h.div_ceil(2));
        let mut out = Vec::new();
        for y in 0..h2 {
            for x in 0..w2 {
                let (xa, xb, ya, yb) = (2 * x, (2 * x + 1).min(w - 1), 2 * y, (2 * y + 1).min(h - 1));
                for c in 0..k {
                    let v = |xx: usize, yy: usize| d[(yy * w + xx) * k + c];
                    out.push(((v(xa, ya) + v(xb, ya)) + v(xa, yb)) + v(xb, yb));
                }
            }
        }
        (out, w2, h2)
    }

    #[test]
    fn zhoda_s_priamym_vypoctom_pri_kazdej_strane() {
        let s = obraz(137, 91, 3, 7, |v| v as u8);
        let mut prva: Option<Vec<Vec<u8>>> = None;
        for strana in [16, 32, 64, 256] {
            let p = Pyramida::postav(&Rastr::<u8>::zo_signalu(&s, strana).unwrap());
            assert_eq!(p.urovne().len(), 9, "137 → 69 → 35 → 18 → 9 → 5 → 3 → 2 → 1");
            let urovne: Vec<Vec<u8>> = p
                .urovne()
                .iter()
                .map(|u| u.do_signalu(u.cely()).unwrap().hodnoty::<u8>().unwrap().to_vec())
                .collect();
            // každá úroveň = priama definícia z predošlej
            let (mut w, mut h) = (137, 91);
            for k in 1..urovne.len() {
                let pred: Vec<f64> = urovne[k - 1].iter().map(|&v| v as f64).collect();
                let (sucty, w2, h2) = zmensi_priamo(&pred, w, h, 3);
                let ocak: Vec<u8> = sucty.iter().map(|&v| u8::z_f64(v * 0.25).0).collect();
                assert_eq!(urovne[k], ocak, "strana {strana}, úroveň {k}");
                (w, h) = (w2, h2);
            }
            match &prva {
                None => prva = Some(urovne),
                Some(a) => assert_eq!(a, &urovne, "strana {strana}"),
            }
        }
    }

    #[test]
    fn plavajuca_ciarka_bitovo_rovnako() {
        let s = obraz(70, 45, 2, 8, |v| (v as f32 / u64::MAX as f32) * 2.0 - 1.0);
        let a = Pyramida::postav(&Rastr::<f32>::zo_signalu(&s, 16).unwrap());
        orchidea_jadro::paralel::obmedz_vlakna(1);
        let b = Pyramida::postav(&Rastr::<f32>::zo_signalu(&s, 64).unwrap());
        orchidea_jadro::paralel::obmedz_vlakna(0);
        for (x, y) in a.urovne().iter().zip(b.urovne()) {
            let hx = x.do_signalu(x.cely()).unwrap();
            let hy = y.do_signalu(y.cely()).unwrap();
            assert_eq!(hx.odtlacok(), hy.odtlacok());
        }
    }

    #[test]
    fn obnova_prepocita_len_zmenu() {
        let s = obraz(1000, 700, 4, 9, |v| v as u8);
        let mut r = Rastr::<u8>::zo_signalu(&s, 64).unwrap();
        let p = Pyramida::postav(&r);
        let vsetky = p.prepocitane();
        r.uprav(Obdlznik::novy(500, 300, 8, 8), |_, _, px| px.copy_from_slice(&[9, 9, 9, 9]));
        let q = p.obnov(&r);
        let nova = Pyramida::postav(&r);
        for (a, b) in q.urovne().iter().zip(nova.urovne()) {
            assert_eq!(a.odtlacok(), b.odtlacok());
        }
        // nad jednou dlaždicou sa prepočíta jedna dlaždica na každej vyššej úrovni
        assert_eq!(q.prepocitane(), q.urovne().len() - 1, "z {vsetky}");
        assert_eq!(vsetky, 48 + 12 + 4 + 7);
        // bez zmeny sa neprepočíta nič; pri inej mriežke všetko
        assert_eq!(q.obnov(&r).prepocitane(), 0);
        let iny = Rastr::<u8>::zo_signalu(&s, 128).unwrap();
        assert_eq!(q.obnov(&iny).prepocitane(), Pyramida::postav(&iny).prepocitane());
    }

    #[test]
    fn obnova_po_nahodnych_upravach_bitovo_rovnaka() {
        let s = obraz(333, 222, 2, 11, |v| (v % 1000) as f32 / 7.0);
        let mut r = Rastr::<f32>::zo_signalu(&s, 32).unwrap();
        let mut p = Pyramida::postav(&r);
        let mut n = Nahoda::zo_semienka(12);
        for krok in 0..25 {
            let (x, y) = ((n.dalsie() % 340) as usize, (n.dalsie() % 230) as usize);
            let (w, h) = (1 + (n.dalsie() % 70) as usize, 1 + (n.dalsie() % 50) as usize);
            let hodnota = krok as f32 * 0.37;
            r.uprav(Obdlznik::novy(x, y, w, h), |px, py, v| {
                v[0] = hodnota + px as f32;
                v[1] = -(py as f32);
            });
            p = p.obnov(&r);
            let znova = Pyramida::postav(&r);
            for (k, (a, b)) in p.urovne().iter().zip(znova.urovne()).enumerate() {
                assert_eq!(a.odtlacok(), b.odtlacok(), "krok {krok}, úroveň {k}");
            }
        }
        // úprava, ktorá nič nezmení: dlaždica je nová, obsah rovnaký – neprepočíta sa nič
        r.uprav(Obdlznik::novy(5, 5, 3, 3), |_, _, _| {});
        assert_eq!(p.obnov(&r).prepocitane(), 0);
    }

    #[test]
    fn prazdne_platno_ostane_jednoliate() {
        let r = Rastr::<u16>::novy(7680, 4320, 256, &[1000, 2000, 3000]).unwrap();
        let p = Pyramida::postav(&r);
        assert_eq!(p.urovne().len(), 14);
        for u in p.urovne() {
            assert!(u.dlazdice().iter().all(|d| d.je_jednoliata()));
        }
        let vsetky: Vec<&Rastr<u16>> = p.urovne().iter().collect();
        assert!(Rastr::bajtov_spolu(&vsetky) < 1000);
    }

    #[test]
    fn nahlad_podla_mierky() {
        let s = obraz(800, 600, 1, 10, |v| v as u8);
        let p = Pyramida::postav(&Rastr::<u8>::zo_signalu(&s, 64).unwrap());
        assert_eq!(p.uroven_pre_mierku(1.0), 0);
        assert_eq!(p.uroven_pre_mierku(2.0), 0);
        assert_eq!(p.uroven_pre_mierku(0.5), 1);
        assert_eq!(p.uroven_pre_mierku(0.3), 1);
        assert_eq!(p.uroven_pre_mierku(0.25), 2);
        assert_eq!(p.uroven_pre_mierku(1e-9), p.urovne().len() - 1);
        let (k, n) = p.nahlad(Obdlznik::novy(100, 50, 400, 300), 0.25).unwrap();
        assert_eq!(k, 2);
        assert_eq!(n.tvar(), vec![76, 100, 1]);
        let (k, n) = p.nahlad(Obdlznik::novy(0, 0, 800, 600), 1.0 / 800.0).unwrap();
        assert_eq!((k, n.tvar()), (9, vec![2, 2, 1]));
        assert!(p.nahlad(Obdlznik::novy(900, 0, 10, 10), 1.0).is_err());
    }
}
