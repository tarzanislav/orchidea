//! Obraz dekodéra: tri roviny vzoriek (Y, Cb, Cr) vo formáte 4:2:0 bez okrajov.
//!
//! Typ vzorky [`Vzorka`] určuje nadradený modul: `u8` pre 8 bitov (profil Main, [`crate::b8`]) a `u16` pre
//! 9–10 bitov (Main 10, [`crate::b16`]) – ten istý zdrojový text sa preloží dvakrát, 8-bitová cesta tak nič
//! nestráca (pamäť ani rýchlosť).
//!
//! Dekódovaný obraz ([`ObrazPasov`]) sa **zverejňuje po pásoch** (riadkoch CTB), hneď ako sú hotové aj filtre
//! v slučke. Ďalší obraz, ktorý ho má ako referenciu, môže bežať súčasne v inom vlákne: čítanie riadka pásu,
//! ktorý ešte nie je hotový, počká ([`OnceLock::wait`]) – nehotové vzorky sa teda nedajú prečítať, bez `unsafe`.

use std::sync::OnceLock;

/// Jedna vzorka obrazu (u8 pre 8 bitov, u16 pre Main 10) – určuje nadradený modul.
pub use super::Vzorka;

/// Zarovnanie kroku riadka v prvkoch – riadky začínajú na hranici 32 bajtov (rýchle vektorové čítanie, NEON).
const ZAROVNANIE: usize = 32;

/// Rovina vzoriek: riadok `y` začína na `data[y * krok]`, platných je `sirka` vzoriek.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rovina {
    pub data: Vec<Vzorka>,
    pub sirka: usize,
    pub vyska: usize,
    /// vzdialenosť začiatkov riadkov v prvkoch (≥ `sirka`)
    pub krok: usize,
}

impl Rovina {
    /// Nová rovina vyplnená hodnotou `hodnota`.
    pub fn nova(sirka: usize, vyska: usize, hodnota: Vzorka) -> Rovina {
        let krok = sirka.div_ceil(ZAROVNANIE) * ZAROVNANIE;
        Rovina { data: vec![hodnota; krok * vyska], sirka, vyska, krok }
    }

    /// Vzorka na súradniciach (x, y) – bez kontroly hraníc nad rámec indexovania.
    #[inline]
    pub fn vzorka(&self, x: usize, y: usize) -> Vzorka {
        self.data[y * self.krok + x]
    }

    /// Platné vzorky riadka `y`.
    #[inline]
    pub fn riadok(&self, y: usize) -> &[Vzorka] {
        &self.data[y * self.krok..y * self.krok + self.sirka]
    }

    /// Rovina od bodu (x, y) po koniec – na zápis bloku (riadok i bloku je `[i * krok..]`).
    #[inline]
    pub fn od(&mut self, x: usize, y: usize) -> &mut [Vzorka] {
        let i = y * self.krok + x;
        &mut self.data[i..]
    }
}

/// Dekódovaný obraz 4:2:0: roviny Y, Cb, Cr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obraz {
    pub roviny: [Rovina; 3],
    /// BitDepthY a BitDepthC (Main 10 dovoľuje rôzne)
    pub bitova_hlbka: u32,
    pub bitova_hlbka_farby: u32,
}

impl Obraz {
    /// Nový obraz s rozmermi jasu `sirka` × `vyska` vyplnený strednou hodnotou 1 << (bitova_hlbka − 1)
    /// (to je aj obsah chýbajúceho referenčného obrazu podľa 8.3.3.2).
    pub fn novy(sirka: usize, vyska: usize) -> Obraz {
        Obraz::s_hlbkou(sirka, vyska, 8, 8)
    }

    /// Ako [`Obraz::novy`] s bitovou hĺbkou jasu `hlbka` a farby `hlbka_farby`.
    pub fn s_hlbkou(sirka: usize, vyska: usize, hlbka: u32, hlbka_farby: u32) -> Obraz {
        let (sy, sc) = ((1u32 << (hlbka - 1)) as Vzorka, (1u32 << (hlbka_farby - 1)) as Vzorka);
        Obraz {
            roviny: [
                Rovina::nova(sirka, vyska, sy),
                Rovina::nova(sirka / 2, vyska / 2, sc),
                Rovina::nova(sirka / 2, vyska / 2, sc),
            ],
            bitova_hlbka: hlbka,
            bitova_hlbka_farby: hlbka_farby,
        }
    }

    /// Prázdny obraz bez pamäte (zástupca pri presune rovín).
    pub fn prazdny() -> Obraz {
        let r = || Rovina { data: Vec::new(), sirka: 0, vyska: 0, krok: 0 };
        Obraz { roviny: [r(), r(), r()], bitova_hlbka: 8, bitova_hlbka_farby: 8 }
    }

    pub fn sirka(&self) -> usize {
        self.roviny[0].sirka
    }

    pub fn vyska(&self) -> usize {
        self.roviny[0].vyska
    }
}

/// Zdroj vzoriek referenčného obrazu pre interpoláciu: súvislá [`Rovina`] alebo rovina zverejňovaná po pásoch.
pub trait Zdroj {
    fn sirka(&self) -> usize;
    fn vyska(&self) -> usize;
    /// Platné vzorky riadka `y` (`y` < výška).
    fn riadok(&self, y: usize) -> &[Vzorka];
    /// Riadky `y … y + h − 1` v jednom kuse pamäte: (vzorky od začiatku riadka `y`, krok riadka), inak `None`.
    fn riadky(&self, y: usize, h: usize) -> Option<(&[Vzorka], usize)>;
}

impl Zdroj for Rovina {
    #[inline]
    fn sirka(&self) -> usize {
        self.sirka
    }

    #[inline]
    fn vyska(&self) -> usize {
        self.vyska
    }

    #[inline]
    fn riadok(&self, y: usize) -> &[Vzorka] {
        Rovina::riadok(self, y)
    }

    #[inline]
    fn riadky(&self, y: usize, _h: usize) -> Option<(&[Vzorka], usize)> {
        Some((&self.data[y * self.krok..], self.krok))
    }
}

/// Rovina zverejňovaná po pásoch: pás `i` sú riadky `i * vyska_pasu …` (posledný môže byť nižší), každý pás sa
/// zapíše práve raz celý a potom sa už len číta. Krok riadka = šírka.
pub struct RovinaPasov {
    pub sirka: usize,
    pub vyska: usize,
    pub vyska_pasu: usize,
    pasy: Vec<OnceLock<Box<[Vzorka]>>>,
}

impl std::fmt::Debug for RovinaPasov {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hotove = self.pasy.iter().filter(|p| p.get().is_some()).count();
        write!(f, "RovinaPasov {}×{}, pásy {}/{}", self.sirka, self.vyska, hotove, self.pasy.len())
    }
}

impl RovinaPasov {
    pub fn nova(sirka: usize, vyska: usize, vyska_pasu: usize) -> RovinaPasov {
        let vyska_pasu = vyska_pasu.max(1);
        let n = vyska.div_ceil(vyska_pasu);
        RovinaPasov { sirka, vyska, vyska_pasu, pasy: (0..n).map(|_| OnceLock::new()).collect() }
    }

    pub fn pocet_pasov(&self) -> usize {
        self.pasy.len()
    }

    /// Riadky pásu `i`.
    pub fn riadky_pasu(&self, i: usize) -> std::ops::Range<usize> {
        let od = i * self.vyska_pasu;
        od..(od + self.vyska_pasu).min(self.vyska)
    }

    /// Zverejní pás `i` (riadky za sebou, krok = šírka). Druhé zverejnenie toho istého pásu sa ignoruje.
    pub fn zverejni(&self, i: usize, data: Box<[Vzorka]>) {
        debug_assert_eq!(data.len(), self.riadky_pasu(i).len() * self.sirka, "HEVC: zlý rozmer pásu");
        let _ = self.pasy[i].set(data);
    }

    pub fn je_zverejneny(&self, i: usize) -> bool {
        self.pasy[i].get().is_some()
    }

    /// Vzorky pásu `i` – počká, kým nie je zverejnený.
    #[inline]
    pub fn pas(&self, i: usize) -> &[Vzorka] {
        match self.pasy[i].get() {
            Some(p) => p,
            None => {
                let _m = crate::etapy::meraj(crate::etapy::Etapa::CakanieRef);
                self.pasy[i].wait()
            }
        }
    }

    /// Doplní nezverejnené pásy hodnotou `hodnota` (obraz s chybou – nikto nesmie čakať naveky).
    pub fn dopln(&self, hodnota: Vzorka) {
        for i in 0..self.pasy.len() {
            if !self.je_zverejneny(i) {
                self.zverejni(i, vec![hodnota; self.riadky_pasu(i).len() * self.sirka].into_boxed_slice());
            }
        }
    }
}

impl Zdroj for RovinaPasov {
    #[inline]
    fn sirka(&self) -> usize {
        self.sirka
    }

    #[inline]
    fn vyska(&self) -> usize {
        self.vyska
    }

    #[inline]
    fn riadok(&self, y: usize) -> &[Vzorka] {
        let i = y / self.vyska_pasu;
        let od = (y - i * self.vyska_pasu) * self.sirka;
        &self.pas(i)[od..od + self.sirka]
    }

    #[inline]
    fn riadky(&self, y: usize, h: usize) -> Option<(&[Vzorka], usize)> {
        let i = y / self.vyska_pasu;
        if (y + h.max(1) - 1) / self.vyska_pasu != i {
            return None;
        }
        Some((&self.pas(i)[(y - i * self.vyska_pasu) * self.sirka..], self.sirka))
    }
}

/// Dekódovaný obraz 4:2:0 zverejňovaný po pásoch (riadkoch CTB) – referencia pre ďalšie obrazy aj výstup.
#[derive(Debug)]
pub struct ObrazPasov {
    pub roviny: [RovinaPasov; 3],
    pub bitova_hlbka: u32,
    pub bitova_hlbka_farby: u32,
}

impl ObrazPasov {
    /// Prázdny obraz `sirka` × `vyska` s pásmi výšky `vyska_pasu` (jas; farba polovičná).
    pub fn novy(sirka: usize, vyska: usize, vyska_pasu: usize) -> ObrazPasov {
        let f = || RovinaPasov::nova(sirka / 2, vyska / 2, vyska_pasu / 2);
        ObrazPasov {
            roviny: [RovinaPasov::nova(sirka, vyska, vyska_pasu), f(), f()],
            bitova_hlbka: 8,
            bitova_hlbka_farby: 8,
        }
    }

    /// Celý hotový obraz zo súvislého obrazu (napr. chýbajúci referenčný obraz).
    pub fn z_obrazu(o: &Obraz) -> ObrazPasov {
        let v = o.vyska().max(1);
        let mut p = ObrazPasov::novy(o.sirka(), o.vyska(), v);
        (p.bitova_hlbka, p.bitova_hlbka_farby) = (o.bitova_hlbka, o.bitova_hlbka_farby);
        for c in 0..3 {
            p.zverejni_z(c, 0, &o.roviny[c]);
        }
        p
    }

    /// Zverejní pás `i` roviny `c` skopírovaním z rovnako veľkej súvislej roviny `zdroj`.
    pub fn zverejni_z(&self, c: usize, i: usize, zdroj: &Rovina) {
        let r = &self.roviny[c];
        if i >= r.pocet_pasov() {
            return;
        }
        let mut d = Vec::with_capacity(r.riadky_pasu(i).len() * r.sirka);
        for y in r.riadky_pasu(i) {
            d.extend_from_slice(&zdroj.riadok(y)[..r.sirka]);
        }
        r.zverejni(i, d.into_boxed_slice());
    }

    pub fn sirka(&self) -> usize {
        self.roviny[0].sirka
    }

    pub fn vyska(&self) -> usize {
        self.roviny[0].vyska
    }

    /// Doplní nezverejnené pásy strednou hodnotou.
    pub fn dopln(&self) {
        for (c, r) in self.roviny.iter().enumerate() {
            let h = if c == 0 { self.bitova_hlbka } else { self.bitova_hlbka_farby };
            r.dopln((1u32 << (h - 1)) as Vzorka);
        }
    }

    /// Je zverejnený celý obraz?
    pub fn je_hotovy(&self) -> bool {
        self.roviny.iter().all(|r| (0..r.pocet_pasov()).all(|i| r.je_zverejneny(i)))
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn rozmery_a_zarovnanie() {
        let o = Obraz::novy(1920, 1080);
        assert_eq!((o.sirka(), o.vyska()), (1920, 1080));
        assert_eq!((o.roviny[1].sirka, o.roviny[1].vyska), (960, 540));
        assert_eq!(o.roviny[0].krok % ZAROVNANIE, 0);
        let r = Rovina::nova(100, 3, 7);
        assert_eq!(r.krok, 128);
        assert_eq!(r.riadok(2).len(), 100);
        assert_eq!(r.vzorka(99, 2), 7);
    }

    #[test]
    // wasm32-wasip1 nemá vlákna (skúška čaká na pás z iného vlákna)
    #[cfg_attr(target_os = "wasi", ignore)]
    fn obraz_po_pasoch() {
        let mut o = Obraz::novy(48, 40);
        for (i, v) in o.roviny[0].data.iter_mut().enumerate() {
            *v = (i % 251) as u8;
        }
        let p = ObrazPasov::novy(48, 40, 16);
        assert_eq!(p.roviny[0].pocet_pasov(), 3);
        assert_eq!(p.roviny[0].riadky_pasu(2), 32..40);
        assert_eq!(p.roviny[1].riadky_pasu(2), 16..20);
        let (jas, ostatne) = (&p.roviny[0], &o.roviny[0]);
        for i in 0..3 {
            p.zverejni_z(0, i, ostatne);
        }
        assert!(!p.je_hotovy());
        for y in 0..40 {
            assert_eq!(Zdroj::riadok(jas, y), ostatne.riadok(y));
        }
        assert!(jas.riadky(10, 6).is_some() && jas.riadky(10, 7).is_none());
        let (d, k) = jas.riadky(17, 4).unwrap();
        assert_eq!(&d[3 * k..3 * k + 48], ostatne.riadok(20));
        p.dopln();
        assert!(p.je_hotovy());
        assert_eq!(Zdroj::riadok(&p.roviny[2], 19)[0], 128);
        // čakanie v inom vlákne na pás, ktorý sa zverejní neskôr
        let p = std::sync::Arc::new(ObrazPasov::novy(16, 16, 8));
        let q = p.clone();
        let citac = std::thread::spawn(move || Zdroj::riadok(&q.roviny[0], 12)[5]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        p.roviny[0].zverejni(1, vec![9; 16 * 8].into_boxed_slice());
        assert_eq!(citac.join().unwrap(), 9);
    }
}
