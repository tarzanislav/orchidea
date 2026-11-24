//! Rastr – obraz ako mriežka zdieľaných dlaždíc s kopírovaním pri zápise (R70).

use crate::dlazdica::Dlazdica;
use crate::{NAJMENSIA_STRANA, NAJVACSIA_STRANA, Obdlznik};
use orchidea_jadro::kodovanie::{Kodovatelne, Zapisovac};
use orchidea_jadro::os::Os;
use orchidea_jadro::signal::{NAJVIAC_HODNOT, PrvokSignalu, Signal};
use orchidea_jadro::{Chyba, Hasovac, Odtlacok, Vlastnosti, Vysledok, m, paralel};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

/// Obraz rozdelený na štvorcové dlaždice so stranou `strana`. Kópia je lacná (zdieľa dlaždice); úprava
/// skopíruje len dlaždice, ktorých sa dotkne.
#[derive(Clone)]
pub struct Rastr<T> {
    sirka: usize,
    vyska: usize,
    kanaly: usize,
    strana: usize,
    stlpcov: usize,
    riadkov: usize,
    dlazdice: Vec<Arc<Dlazdica<T>>>,
    os_y: Os,
    os_x: Os,
    os_kanalov: Os,
    vyznam: Vlastnosti,
    odtlacok: OnceLock<Odtlacok>,
}

/// Zdieľané jednoliate dlaždice podľa rozmerov.
type Zdielane<T> = Vec<((usize, usize), Arc<Dlazdica<T>>)>;

impl<T: PrvokSignalu> std::fmt::Debug for Rastr<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Rastr {}×{}×{} {} (dlaždice {}×{} po {} px)",
            self.sirka,
            self.vyska,
            self.kanaly,
            T::TYP.meno(),
            self.stlpcov,
            self.riadkov,
            self.strana
        )
    }
}

fn over_rozmery(sirka: usize, vyska: usize, kanaly: usize, strana: usize) -> Vysledok<()> {
    if sirka == 0 || vyska == 0 || kanaly == 0 {
        return Err(Chyba::MimoRozsahu(format!("obraz {sirka}×{vyska} s {kanaly} kanálmi")));
    }
    if !strana.is_power_of_two() || !(NAJMENSIA_STRANA..=NAJVACSIA_STRANA).contains(&strana) {
        return Err(Chyba::MimoRozsahu(format!(
            "strana dlaždice {strana} (mocnina dvoch {NAJMENSIA_STRANA}–{NAJVACSIA_STRANA})"
        )));
    }
    let n = (sirka as u128) * (vyska as u128) * (kanaly as u128);
    if n > NAJVIAC_HODNOT as u128 {
        return Err(Chyba::Limit(format!("{n} hodnôt je viac ako {NAJVIAC_HODNOT}")));
    }
    Ok(())
}

impl<T: PrvokSignalu> Rastr<T> {
    fn prazdny(sirka: usize, vyska: usize, kanaly: usize, strana: usize) -> Vysledok<Rastr<T>> {
        over_rozmery(sirka, vyska, kanaly, strana)?;
        Ok(Rastr {
            sirka,
            vyska,
            kanaly,
            strana,
            stlpcov: sirka.div_ceil(strana),
            riadkov: vyska.div_ceil(strana),
            dlazdice: Vec::new(),
            os_y: Os::poradova("y", vyska)?,
            os_x: Os::poradova("x", sirka)?,
            os_kanalov: Os::poradova("kanál", kanaly)?,
            vyznam: Vlastnosti::nove(),
            odtlacok: OnceLock::new(),
        })
    }

    /// Obraz plný jednej hodnoty pixela (napríklad prázdne plátno). Všetky dlaždice rovnakých rozmerov
    /// zdieľajú jednu jednoliatu dlaždicu – plátno ľubovoľnej veľkosti zaberie najviac štyri malé dlaždice.
    pub fn novy(sirka: usize, vyska: usize, strana: usize, pixel: &[T]) -> Vysledok<Rastr<T>> {
        let mut r = Rastr::prazdny(sirka, vyska, pixel.len(), strana)?;
        let mut zdielane = Vec::new();
        r.dlazdice = (0..r.stlpcov * r.riadkov).map(|i| r.jednoliata_pre(i, pixel, &mut zdielane)).collect();
        Ok(r)
    }

    /// Jednoliata dlaždica pre miesto `i` – rovnaké rozmery dostanú tú istú zdieľanú dlaždicu.
    fn jednoliata_pre(&self, i: usize, pixel: &[T], zdielane: &mut Zdielane<T>) -> Arc<Dlazdica<T>> {
        let (_, _, w, h) = self.miesto(i);
        if let Some((_, d)) = zdielane.iter().find(|(r, _)| *r == (w, h)) {
            return d.clone();
        }
        let d = Arc::new(Dlazdica::jednoliata(w, h, pixel));
        zdielane.push(((w, h), d.clone()));
        d
    }

    /// Obraz z pixelov po riadkoch (kanály za sebou) s menami kanálov – napríklad rozbalený obrázok zo súboru.
    /// Dlaždice sa vyrežú súbežne; jednoliate dlaždice sa zdieľajú.
    pub fn z_pixelov(
        sirka: usize,
        vyska: usize,
        mena_kanalov: &[&str],
        strana: usize,
        data: &[T],
    ) -> Vysledok<Rastr<T>> {
        let kanaly = mena_kanalov.len();
        let mut r = Rastr::prazdny(sirka, vyska, kanaly, strana)?;
        if data.len() != sirka * vyska * kanaly {
            return Err(Chyba::NezhodaTvaru(format!(
                "obraz {sirka}×{vyska}×{kanaly} má {} hodnôt",
                data.len()
            )));
        }
        r.os_kanalov = Os::oznacena("kanál", mena_kanalov)?;
        let n = r.stlpcov * r.riadkov;
        let kus = (n / (4 * paralel::pocet_vlakien())).max(1);
        let casti = paralel::mapuj_rozsah(n, kus, |rozsah| {
            rozsah
                .map(|i| {
                    let (x0, y0, w, h) = r.miesto(i);
                    let mut d = Vec::with_capacity(w * h * kanaly);
                    for y in y0..y0 + h {
                        let z = (y * sirka + x0) * kanaly;
                        d.extend_from_slice(&data[z..z + w * kanaly]);
                    }
                    Arc::new(Dlazdica::z_dat(w, h, kanaly, d))
                })
                .collect::<Vec<_>>()
        });
        r.dlazdice = casti.into_iter().flatten().collect();
        r.zdielaj_jednoliate();
        Ok(r)
    }

    /// Obraz zo signálu s osami „y“, „x“ a voliteľne „kanál“ (iné osi nie sú dovolené). Typ hodnôt
    /// signálu musí byť `T`. Mená kanálov, vzorkovanie osí a význam sa zachovajú.
    pub fn zo_signalu(signal: &Signal, strana: usize) -> Vysledok<Rastr<T>> {
        let ma_kanal = signal.os("kanál").is_some();
        let poradie = if ma_kanal { vec![m("y"), m("x"), m("kanál")] } else { vec![m("y"), m("x")] };
        if signal.osi().len() != poradie.len() {
            return Err(Chyba::NezhodaTvaru("obraz má mať osi „y“, „x“ a voliteľne „kanál“".into()));
        }
        let s = signal.preusporiadaj(&poradie)?;
        let data = s.hodnoty::<T>().ok_or_else(|| Chyba::NezhodaTypov {
            popis: format!("signál má hodnoty {}, obraz {}", s.typ().meno(), T::TYP.meno()),
            navrh: Some("previesť signál metódou preved".into()),
        })?;
        let (vyska, sirka) = (s.osi()[0].dlzka, s.osi()[1].dlzka);
        let kanaly = if ma_kanal { s.osi()[2].dlzka } else { 1 };
        let mut r = Rastr::prazdny(sirka, vyska, kanaly, strana)?;
        r.os_y = s.osi()[0].clone();
        r.os_x = s.osi()[1].clone();
        if ma_kanal {
            r.os_kanalov = s.osi()[2].clone();
        }
        r.vyznam = s.vyznam().clone();
        let n = r.stlpcov * r.riadkov;
        let kus = (n / (4 * paralel::pocet_vlakien())).max(1);
        let casti = paralel::mapuj_rozsah(n, kus, |rozsah| {
            rozsah
                .map(|i| {
                    let (x0, y0, w, h) = r.miesto(i);
                    let mut d = Vec::with_capacity(w * h * kanaly);
                    for y in y0..y0 + h {
                        let z = (y * sirka + x0) * kanaly;
                        d.extend_from_slice(&data[z..z + w * kanaly]);
                    }
                    Arc::new(Dlazdica::z_dat(w, h, kanaly, d))
                })
                .collect::<Vec<_>>()
        });
        r.dlazdice = casti.into_iter().flatten().collect();
        r.zdielaj_jednoliate();
        Ok(r)
    }

    /// Jednoliate dlaždice s rovnakými rozmermi a hodnotou budú jedna zdieľaná (bez hašovania dát).
    pub(crate) fn zdielaj_jednoliate(&mut self) {
        let mut videne: HashMap<(usize, usize, Vec<u64>), Arc<Dlazdica<T>>> = HashMap::new();
        for a in self.dlazdice.iter_mut().filter(|a| a.je_jednoliata()) {
            let kluc = (a.sirka(), a.vyska(), a.pixel(0, 0).iter().map(|v| v.do_f64().to_bits()).collect());
            match videne.get(&kluc) {
                Some(b) => *a = b.clone(),
                None => {
                    videne.insert(kluc, a.clone());
                }
            }
        }
    }

    /// Prázdny obraz s rovnakou mriežkou a popisom osí (pre ďalšiu úroveň pyramídy a pod.).
    pub(crate) fn s_rozmermi_ako(&self, sirka: usize, vyska: usize) -> Vysledok<Rastr<T>> {
        let mut r = Rastr::prazdny(sirka, vyska, self.kanaly, self.strana)?;
        r.os_kanalov = self.os_kanalov.clone();
        r.vyznam = self.vyznam.clone();
        Ok(r)
    }

    pub(crate) fn nastav_dlazdice(&mut self, dlazdice: Vec<Arc<Dlazdica<T>>>) {
        debug_assert_eq!(dlazdice.len(), self.stlpcov * self.riadkov);
        self.dlazdice = dlazdice;
        self.odtlacok = OnceLock::new();
    }

    pub fn sirka(&self) -> usize {
        self.sirka
    }

    pub fn vyska(&self) -> usize {
        self.vyska
    }

    pub fn kanaly(&self) -> usize {
        self.kanaly
    }

    /// Strana dlaždice v pixeloch.
    pub fn strana(&self) -> usize {
        self.strana
    }

    /// Počet dlaždíc vo vodorovnom a zvislom smere.
    pub fn mriezka(&self) -> (usize, usize) {
        (self.stlpcov, self.riadkov)
    }

    /// Celý obraz ako obdĺžnik.
    pub fn cely(&self) -> Obdlznik {
        Obdlznik::novy(0, 0, self.sirka, self.vyska)
    }

    /// Dlaždice po riadkoch mriežky.
    pub fn dlazdice(&self) -> &[Arc<Dlazdica<T>>] {
        &self.dlazdice
    }

    pub fn os_kanalov(&self) -> &Os {
        &self.os_kanalov
    }

    pub fn vyznam(&self) -> &Vlastnosti {
        &self.vyznam
    }

    /// Poloha a rozmery dlaždice `i`: (x, y, šírka, výška).
    pub fn miesto(&self, i: usize) -> (usize, usize, usize, usize) {
        let (x, y) = ((i % self.stlpcov) * self.strana, (i / self.stlpcov) * self.strana);
        (x, y, self.strana.min(self.sirka - x), self.strana.min(self.vyska - y))
    }

    /// Hodnoty pixela.
    pub fn pixel(&self, x: usize, y: usize) -> &[T] {
        assert!(x < self.sirka && y < self.vyska, "pixel {x},{y} mimo obrazu {}×{}", self.sirka, self.vyska);
        let s = self.strana;
        self.dlazdice[(y / s) * self.stlpcov + x / s].pixel(x % s, y % s)
    }

    /// Rovnaké rozmery, kanály a strana dlaždice (dlaždice si navzájom zodpovedajú).
    pub fn rovnaka_mriezka(&self, iny: &Rastr<T>) -> bool {
        (self.sirka, self.vyska, self.kanaly, self.strana) == (iny.sirka, iny.vyska, iny.kanaly, iny.strana)
    }

    /// Indexy dlaždíc, ktoré zasahujú do obdĺžnika (už orezaného na obraz).
    fn dlazdice_v(&self, r: &Obdlznik) -> impl Iterator<Item = usize> + use<T> {
        let s = self.strana;
        let (c0, c1) = (r.x / s, (r.koniec_x() - 1) / s);
        let (r0, r1) = (r.y / s, (r.koniec_y() - 1) / s);
        let stlpcov = self.stlpcov;
        (r0..=r1).flat_map(move |rr| (c0..=c1).map(move |cc| rr * stlpcov + cc))
    }

    /// Zmení pixely v obdĺžniku: `f(x, y, pixel)` dostane súradnice v obraze a hodnoty na prepis. Skopírujú
    /// sa len dlaždice, ktorých sa obdĺžnik dotkne a ktoré zdieľa iná verzia obrazu; dlaždice sa spracúvajú
    /// súbežne na všetkých jadrách.
    pub fn uprav(&mut self, r: Obdlznik, f: impl Fn(usize, usize, &mut [T]) + Sync) {
        let r = r.prienik(&self.cely());
        if r.je_prazdny() {
            return;
        }
        let ktore: HashSet<usize> = self.dlazdice_v(&r).collect();
        let (s, stlpcov, k) = (self.strana, self.stlpcov, self.kanaly);
        let mut prace: Vec<(usize, &mut Arc<Dlazdica<T>>)> =
            self.dlazdice.iter_mut().enumerate().filter(|(i, _)| ktore.contains(i)).collect();
        paralel::pre_kusy(&mut prace, 1, |_, kus| {
            for (i, a) in kus.iter_mut() {
                let (tx, ty) = ((*i % stlpcov) * s, (*i / stlpcov) * s);
                let d = Arc::make_mut(a);
                let w = d.sirka();
                let cast = r.prienik(&Obdlznik::novy(tx, ty, w, d.vyska()));
                let hodnoty = d.na_zapis();
                for y in cast.y..cast.koniec_y() {
                    for x in cast.x..cast.koniec_x() {
                        let z = ((y - ty) * w + (x - tx)) * k;
                        f(x, y, &mut hodnoty[z..z + k]);
                    }
                }
            }
        });
        self.odtlacok = OnceLock::new();
    }

    /// Vyplní obdĺžnik jednou hodnotou pixela. Dlaždice, ktoré obdĺžnik pokryje celé, sa nahradia zdieľanou
    /// jednoliatou dlaždicou bez zápisu pixelov; ostatné sa upravia.
    pub fn vypln(&mut self, r: Obdlznik, pixel: &[T]) -> Vysledok<()> {
        self.over_pixel(pixel)?;
        let r = r.prienik(&self.cely());
        if r.je_prazdny() {
            return Ok(());
        }
        let mut zdielane = Vec::new();
        let mut ciastocne = false;
        for i in self.dlazdice_v(&r).collect::<Vec<_>>() {
            let (x, y, w, h) = self.miesto(i);
            if r.obsahuje(&Obdlznik::novy(x, y, w, h)) {
                self.dlazdice[i] = self.jednoliata_pre(i, pixel, &mut zdielane);
            } else {
                ciastocne = true;
            }
        }
        if ciastocne {
            let s = self.strana;
            let cele = |x: usize, y: usize| {
                let (tx, ty) = ((x / s) * s, (y / s) * s);
                r.obsahuje(&Obdlznik::novy(tx, ty, s.min(self.sirka - tx), s.min(self.vyska - ty)))
            };
            // celé dlaždice sú už vymenené – upraví sa len zvyšok
            let okraj: Vec<usize> = self
                .dlazdice_v(&r)
                .filter(|&i| {
                    let (x, y, _, _) = self.miesto(i);
                    !cele(x, y)
                })
                .collect();
            for i in okraj {
                let (x, y, w, h) = self.miesto(i);
                self.uprav(r.prienik(&Obdlznik::novy(x, y, w, h)), |_, _, p| p.copy_from_slice(pixel));
            }
        }
        self.odtlacok = OnceLock::new();
        Ok(())
    }

    /// Vloží obdĺžnik hodnôt (po riadkoch, kanály za sebou) na miesto `x`, `y`. Časť mimo obrazu sa vynechá.
    pub fn vloz(&mut self, x: usize, y: usize, sirka: usize, vyska: usize, data: &[T]) -> Vysledok<()> {
        let k = self.kanaly;
        if data.len() != sirka * vyska * k {
            return Err(Chyba::NezhodaTvaru(format!(
                "vkladaný obdĺžnik {sirka}×{vyska}×{k} má {} hodnôt",
                data.len()
            )));
        }
        self.uprav(Obdlznik::novy(x, y, sirka, vyska), |px, py, p| {
            let z = ((py - y) * sirka + (px - x)) * k;
            p.copy_from_slice(&data[z..z + k]);
        });
        Ok(())
    }

    fn over_pixel(&self, pixel: &[T]) -> Vysledok<()> {
        if pixel.len() != self.kanaly {
            return Err(Chyba::NezhodaTvaru(format!(
                "pixel má {} hodnôt, obraz {} kanálov",
                pixel.len(),
                self.kanaly
            )));
        }
        Ok(())
    }

    /// Zhustí obraz: rozbalené dlaždice s rovnakými pixelmi sa stanú jednoliatymi a dlaždice s rovnakým
    /// obsahom sa zlúčia do jednej zdieľanej (podľa odtlačku). Vhodné pred uložením verzie do histórie.
    pub fn zhustni(&mut self) {
        let mut podla_odtlacku: HashMap<Odtlacok, Arc<Dlazdica<T>>> = HashMap::new();
        for a in self.dlazdice.iter_mut() {
            if !a.je_jednoliata()
                && let Some(p) = a.jednoliata_hodnota()
            {
                // nová jednoliata dlaždica – zdieľanú rozbalenú netreba kopírovať
                let nova = Arc::new(Dlazdica::jednoliata(a.sirka(), a.vyska(), p));
                *a = nova;
            }
            let o = a.odtlacok();
            match podla_odtlacku.get(&o) {
                Some(b) if !Arc::ptr_eq(a, b) => *a = b.clone(),
                Some(_) => {}
                None => {
                    podla_odtlacku.insert(o, a.clone());
                }
            }
        }
    }

    /// Výrez obrazu ako signál jadra s osami „y“, „x“, „kanál“. Celý obraz zachová pôvodné osi; výrez má
    /// osi s poradím vzoriek. Čítajú sa len dlaždice, ktoré do výrezu zasahujú.
    pub fn do_signalu(&self, r: Obdlznik) -> Vysledok<Signal> {
        if r.je_prazdny() || !self.cely().obsahuje(&r) {
            return Err(Chyba::MimoRozsahu(format!(
                "výrez {}×{} na {},{} z obrazu {}×{}",
                r.sirka, r.vyska, r.x, r.y, self.sirka, self.vyska
            )));
        }
        let k = self.kanaly;
        let n_riadok = r.sirka * k;
        let mut data: Vec<T> = vec![self.pixel(r.x, r.y)[0]; r.vyska * n_riadok];
        let s = self.strana;
        paralel::pre_kusy(&mut data, n_riadok * 16, |i, kus| {
            for (j, riadok) in kus.chunks_exact_mut(n_riadok).enumerate() {
                let y = r.y + i * 16 + j;
                let (tr, ly) = (y / s, y % s);
                let mut x = r.x;
                while x < r.koniec_x() {
                    let (tc, lx) = (x / s, x % s);
                    let d = &self.dlazdice[tr * self.stlpcov + tc];
                    let kolko = (d.sirka() - lx).min(r.koniec_x() - x);
                    let ciel = &mut riadok[(x - r.x) * k..(x - r.x + kolko) * k];
                    match d.riadok(ly) {
                        Some(zdroj) => ciel.copy_from_slice(&zdroj[lx * k..(lx + kolko) * k]),
                        None => {
                            let p = d.pixel(0, 0);
                            ciel.chunks_exact_mut(k).for_each(|c| c.copy_from_slice(p));
                        }
                    }
                    x += kolko;
                }
            }
        });
        let osi = if r == self.cely() {
            vec![self.os_y.clone(), self.os_x.clone(), self.os_kanalov.clone()]
        } else {
            vec![Os::poradova("y", r.vyska)?, Os::poradova("x", r.sirka)?, self.os_kanalov.clone()]
        };
        Signal::zo_zdielanych(osi.into(), T::zabal(data.into()), self.vyznam.clone())
    }

    /// Odtlačok obrazu: strom z odtlačkov dlaždíc spolu s rozmermi, stranou, typom, osou kanálov a významom.
    /// Po úprave sa prepočítajú len odtlačky zmenených dlaždíc. Závisí od strany dlaždice.
    pub fn odtlacok(&self) -> Odtlacok {
        *self.odtlacok.get_or_init(|| {
            let n = self.dlazdice.len();
            let kus = (n / (4 * paralel::pocet_vlakien())).max(1);
            let odtlacky =
                paralel::mapuj_rozsah(n, kus, |r| r.map(|i| self.dlazdice[i].odtlacok()).collect::<Vec<_>>());
            let mut z = Zapisovac::novy();
            for v in [self.sirka, self.vyska, self.kanaly, self.strana] {
                z.cislo(v as u64);
            }
            z.text(T::TYP.meno());
            self.os_kanalov.zakoduj(&mut z);
            self.vyznam.zakoduj(&mut z);
            let mut h = Hasovac::odvodenie_kluca("ORCHIDEA rastr v1");
            h.pridaj(z.bajty());
            for o in odtlacky.iter().flatten() {
                h.pridaj(&o.0);
            }
            h.dokonci()
        })
    }

    /// Ktoré dlaždice sa zmenili oproti inej verzii s rovnakou mriežkou (`None` pri inej mriežke).
    /// Zdieľaná dlaždica je nezmenená bez počítania; inak rozhodnú už spočítané odtlačky, a ak ich niet,
    /// dlaždica sa berie ako zmenená (nič sa kvôli tomu nehašuje).
    pub fn zmenene_voci(&self, stary: &Rastr<T>) -> Option<Vec<bool>> {
        self.rovnaka_mriezka(stary).then(|| {
            self.dlazdice
                .iter()
                .zip(&stary.dlazdice)
                .map(|(a, b)| {
                    if Arc::ptr_eq(a, b) {
                        return false;
                    }
                    match (a.znamy_odtlacok(), b.znamy_odtlacok()) {
                        (Some(x), Some(y)) => x != y,
                        _ => true,
                    }
                })
                .collect()
        })
    }

    /// Pamäť hodnôt obrazu v bajtoch (zdieľaná dlaždica sa počíta raz).
    pub fn bajtov(&self) -> usize {
        Rastr::bajtov_spolu(&[self])
    }

    /// Pamäť hodnôt viacerých verzií spolu (napríklad celej histórie) – každá zdieľaná dlaždica raz.
    pub fn bajtov_spolu(verzie: &[&Rastr<T>]) -> usize {
        let mut videne = HashSet::new();
        verzie
            .iter()
            .flat_map(|r| r.dlazdice.iter())
            .filter(|a| videne.insert(Arc::as_ptr(a)))
            .map(|a| a.bajtov())
            .sum()
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    fn nahodny_obraz(sirka: usize, vyska: usize, semienko: u64) -> Signal {
        let mut n = Nahoda::zo_semienka(semienko);
        let osi = vec![
            Os::poradova("y", vyska).unwrap(),
            Os::poradova("x", sirka).unwrap(),
            Os::oznacena("kanál", &["R", "G", "B"]).unwrap(),
        ];
        Signal::novy(osi, (0..sirka * vyska * 3).map(|_| n.dalsie() as u8).collect()).unwrap()
    }

    #[test]
    fn signal_tam_a_spat() {
        let s = nahodny_obraz(100, 70, 1);
        for strana in [16, 32, 64, 128] {
            let r = Rastr::<u8>::zo_signalu(&s, strana).unwrap();
            assert_eq!(r.mriezka(), (100usize.div_ceil(strana), 70usize.div_ceil(strana)));
            let spat = r.do_signalu(r.cely()).unwrap();
            assert_eq!(spat.odtlacok(), s.odtlacok(), "strana {strana}");
            let v = r.do_signalu(Obdlznik::novy(13, 7, 40, 33)).unwrap();
            let ocak = s.vyrez("y", 7, 40).unwrap().vyrez("x", 13, 53).unwrap();
            assert_eq!(v.hodnoty::<u8>(), ocak.hodnoty::<u8>());
        }
        assert!(Rastr::<f32>::zo_signalu(&s, 64).is_err());
        assert!(Rastr::<u8>::zo_signalu(&s, 48).is_err());
        let r = Rastr::<u8>::zo_signalu(&s, 64).unwrap();
        assert!(r.do_signalu(Obdlznik::novy(90, 0, 20, 5)).is_err());
    }

    #[test]
    fn prazdne_platno_takmer_nic_nezaberie() {
        // 8K RGBA: štyri zdieľané jednoliate dlaždice
        let r = Rastr::<u8>::novy(7680, 4320, 256, &[255, 255, 255, 255]).unwrap();
        assert_eq!(r.mriezka(), (30, 17));
        assert!(r.bajtov() <= 16);
        assert_eq!(r.pixel(7679, 4319), &[255, 255, 255, 255]);
    }

    #[test]
    fn kopirovanie_pri_zapise_a_historia() {
        let s = nahodny_obraz(640, 480, 2);
        let povodny = Rastr::<u8>::zo_signalu(&s, 64).unwrap();
        let cely = povodny.bajtov();
        assert_eq!(cely, 640 * 480 * 3);
        let mut historia = vec![povodny.clone()];
        let mut r = povodny.clone();
        for t in 0..50 {
            // malý ťah štetcom 10×10 – zasiahne jednu až štyri dlaždice
            r.uprav(Obdlznik::novy(5 + t * 12, 100 + t * 3, 10, 10), |_, _, p| p.copy_from_slice(&[0, 0, 0]));
            historia.push(r.clone());
        }
        let verzie: Vec<&Rastr<u8>> = historia.iter().collect();
        let spolu = Rastr::bajtov_spolu(&verzie);
        // 51 verzií bez zdieľania by bolo 51 × obraz; so zdieľaním menej ako trojnásobok
        assert!(spolu < 3 * cely, "{spolu} B");
        // pôvodná verzia ostala nezmenená
        assert_eq!(historia[0].do_signalu(historia[0].cely()).unwrap().odtlacok(), s.odtlacok());
        assert_eq!(r.pixel(5, 100), &[0, 0, 0]);
        let zmenene = historia[1].zmenene_voci(&historia[0]).unwrap();
        assert_eq!(zmenene.iter().filter(|&&z| z).count(), 1);
    }

    #[test]
    fn vyplnenie_a_zhustenie() {
        let s = nahodny_obraz(300, 200, 3);
        let mut r = Rastr::<u8>::zo_signalu(&s, 32).unwrap();
        r.vypln(Obdlznik::novy(10, 10, 280, 180), &[1, 2, 3]).unwrap();
        assert_eq!(r.pixel(10, 10), &[1, 2, 3]);
        assert_eq!(r.pixel(289, 189), &[1, 2, 3]);
        assert_ne!(r.pixel(9, 10), &[1, 2, 3]);
        assert_ne!(r.pixel(290, 189), &[1, 2, 3]);
        let jednoliatych = r.dlazdice().iter().filter(|d| d.je_jednoliata()).count();
        // celé pokryté: stĺpce 1–8 a riadky 1–4 mriežky 32 px
        assert_eq!(jednoliatych, 8 * 4);
        // úprava, po ktorej je dlaždica znova rovnaká: zhustenie ju zlúči so zdieľanou
        let pred = r.bajtov();
        r.uprav(Obdlznik::novy(64, 64, 32, 32), |_, _, p| p.copy_from_slice(&[1, 2, 3]));
        assert!(r.bajtov() > pred);
        let o = r.odtlacok();
        r.zhustni();
        assert_eq!(r.bajtov(), pred);
        assert_eq!(r.odtlacok(), o);
        assert!(r.vypln(r.cely(), &[1, 2]).is_err());
    }

    #[test]
    fn odtlacok_podla_obsahu() {
        let s = nahodny_obraz(200, 150, 4);
        let a = Rastr::<u8>::zo_signalu(&s, 64).unwrap();
        let mut b = a.clone();
        assert_eq!(a.odtlacok(), b.odtlacok());
        b.uprav(Obdlznik::novy(3, 3, 1, 1), |_, _, p| p[0] ^= 1);
        assert_ne!(a.odtlacok(), b.odtlacok());
        b.uprav(Obdlznik::novy(3, 3, 1, 1), |_, _, p| p[0] ^= 1);
        assert_eq!(a.odtlacok(), b.odtlacok());
        // iný význam kanálov = iný obsah
        let c = Rastr::<u8>::zo_signalu(
            &s.s_osami(vec![
                Os::poradova("y", 150).unwrap(),
                Os::poradova("x", 200).unwrap(),
                Os::oznacena("kanál", &["B", "G", "R"]).unwrap(),
            ])
            .unwrap(),
            64,
        )
        .unwrap();
        assert_ne!(a.odtlacok(), c.odtlacok());
    }

    #[test]
    fn vlozenie_bloku() {
        let mut r = Rastr::<f32>::novy(50, 40, 16, &[0.0]).unwrap();
        let blok: Vec<f32> = (0..20 * 10).map(|i| i as f32).collect();
        r.vloz(40, 35, 20, 10, &blok).unwrap();
        assert_eq!(r.pixel(40, 35), &[0.0]);
        assert_eq!(r.pixel(49, 39), &[(4 * 20 + 9) as f32]);
        assert!(r.vloz(0, 0, 2, 2, &[1.0]).is_err());
    }
}
