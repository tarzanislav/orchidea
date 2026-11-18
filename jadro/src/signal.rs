//! Signál – pole hodnôt nad pomenovanými osami s jednotkami a významom (prvotný pojem 1).
//!
//! Fotka, video, zvuk, spektrum, hĺbka, vedecké meranie aj tenzor umelej inteligencie sú signál. Osi sa
//! nevolajú podľa poradia, ale podľa mena („x“, „t“, „kanál“…), preto operácie nepotrebujú vedieť, či ide
//! o obraz alebo zvuk. Dáta sú nemenné a zdieľané – kópia signálu je lacná, zmena vytvorí nový signál.

use crate::chyba::{Chyba, Vysledok};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::meno::Meno;
use crate::odtlacok::{Hasovac, Odtlacok};
use crate::os::Os;
use crate::vlastnost::Vlastnosti;
use std::fmt;
use std::sync::{Arc, OnceLock};

/// Najväčší počet hodnôt v jednom signáli (2⁴⁰) – ochrana pred poškodenými dátami.
pub const NAJVIAC_HODNOT: u64 = 1 << 40;
/// Najväčší počet osí.
pub const NAJVIAC_OSI: usize = 32;

/// Typ jednej hodnoty signálu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TypPrvku {
    U8,
    U16,
    I8,
    I16,
    I32,
    F32,
    F64,
}

impl TypPrvku {
    pub const VSETKY: [TypPrvku; 7] = [
        TypPrvku::U8,
        TypPrvku::U16,
        TypPrvku::I8,
        TypPrvku::I16,
        TypPrvku::I32,
        TypPrvku::F32,
        TypPrvku::F64,
    ];

    pub fn velkost(self) -> usize {
        match self {
            TypPrvku::U8 | TypPrvku::I8 => 1,
            TypPrvku::U16 | TypPrvku::I16 => 2,
            TypPrvku::I32 | TypPrvku::F32 => 4,
            TypPrvku::F64 => 8,
        }
    }

    pub fn meno(self) -> &'static str {
        match self {
            TypPrvku::U8 => "u8",
            TypPrvku::U16 => "u16",
            TypPrvku::I8 => "i8",
            TypPrvku::I16 => "i16",
            TypPrvku::I32 => "i32",
            TypPrvku::F32 => "f32",
            TypPrvku::F64 => "f64",
        }
    }

    pub fn z_mena(t: &str) -> Option<TypPrvku> {
        TypPrvku::VSETKY.into_iter().find(|x| x.meno() == t)
    }

    pub fn je_plavajuci(self) -> bool {
        matches!(self, TypPrvku::F32 | TypPrvku::F64)
    }

    /// Najmenšia a najväčšia hodnota celočíselného typu.
    pub fn rozsah(self) -> (f64, f64) {
        match self {
            TypPrvku::U8 => (0.0, 255.0),
            TypPrvku::U16 => (0.0, 65_535.0),
            TypPrvku::I8 => (-128.0, 127.0),
            TypPrvku::I16 => (-32_768.0, 32_767.0),
            TypPrvku::I32 => (i32::MIN as f64, i32::MAX as f64),
            TypPrvku::F32 => (-f32::MAX as f64, f32::MAX as f64),
            TypPrvku::F64 => (-f64::MAX, f64::MAX),
        }
    }

    /// Dá sa každá hodnota typu `self` presne zapísať v type `ciel`?
    pub fn bezstratovo_do(self, ciel: TypPrvku) -> bool {
        use TypPrvku::*;
        if self == ciel {
            return true;
        }
        matches!(
            (self, ciel),
            (_, F64)
                | (U8 | U16 | I8 | I16, F32)
                | (U8, U16 | I16 | I32)
                | (U16, I32)
                | (I8, I16 | I32)
                | (I16, I32)
        )
    }

    fn kod(self) -> u8 {
        self as u8
    }

    fn z_kodu(k: u8) -> Vysledok<TypPrvku> {
        TypPrvku::VSETKY
            .get(k as usize)
            .copied()
            .ok_or_else(|| Chyba::PoskodeneData(format!("typ prvku {k}")))
    }
}

mod zapecatene {
    pub trait Zapecatene {}
}

/// Typ, ktorý môže byť hodnotou signálu.
pub trait PrvokSignalu: Copy + Send + Sync + 'static + zapecatene::Zapecatene {
    const TYP: TypPrvku;
    fn do_f64(self) -> f64;
    /// Prevod z f64 so zaokrúhlením a orezaním na rozsah typu. Vracia (hodnota, či bola orezaná).
    fn z_f64(v: f64) -> (Self, bool);
    fn zapis_le(self, ciel: &mut Vec<u8>);
    fn citaj_le(b: &[u8]) -> Self;
    fn data(d: &Data) -> Option<&Arc<[Self]>>;
    fn zabal(v: Arc<[Self]>) -> Data;
}

macro_rules! cele_prvky {
    ($($t:ty => $v:ident),*) => {$(
        impl zapecatene::Zapecatene for $t {}
        impl PrvokSignalu for $t {
            const TYP: TypPrvku = TypPrvku::$v;
            fn do_f64(self) -> f64 { self as f64 }
            fn z_f64(v: f64) -> (Self, bool) {
                if v.is_nan() { return (0 as $t, true); }
                let r = v.round();
                let (od, po) = (<$t>::MIN as f64, <$t>::MAX as f64);
                if r < od { (<$t>::MIN, true) } else if r > po { (<$t>::MAX, true) } else { (r as $t, false) }
            }
            fn zapis_le(self, ciel: &mut Vec<u8>) { ciel.extend_from_slice(&self.to_le_bytes()); }
            fn citaj_le(b: &[u8]) -> Self {
                let mut p = [0u8; std::mem::size_of::<$t>()];
                p.copy_from_slice(b);
                <$t>::from_le_bytes(p)
            }
            fn data(d: &Data) -> Option<&Arc<[Self]>> { if let Data::$v(x) = d { Some(x) } else { None } }
            fn zabal(v: Arc<[Self]>) -> Data { Data::$v(v) }
        }
    )*};
}
cele_prvky!(u8 => U8, u16 => U16, i8 => I8, i16 => I16, i32 => I32);

impl zapecatene::Zapecatene for f32 {}
impl PrvokSignalu for f32 {
    const TYP: TypPrvku = TypPrvku::F32;
    fn do_f64(self) -> f64 {
        self as f64
    }
    fn z_f64(v: f64) -> (Self, bool) {
        let x = v as f32;
        (x, x.is_infinite() && v.is_finite())
    }
    fn zapis_le(self, ciel: &mut Vec<u8>) {
        ciel.extend_from_slice(&self.to_le_bytes());
    }
    fn citaj_le(b: &[u8]) -> Self {
        f32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }
    fn data(d: &Data) -> Option<&Arc<[Self]>> {
        if let Data::F32(x) = d { Some(x) } else { None }
    }
    fn zabal(v: Arc<[Self]>) -> Data {
        Data::F32(v)
    }
}

impl zapecatene::Zapecatene for f64 {}
impl PrvokSignalu for f64 {
    const TYP: TypPrvku = TypPrvku::F64;
    fn do_f64(self) -> f64 {
        self
    }
    fn z_f64(v: f64) -> (Self, bool) {
        (v, false)
    }
    fn zapis_le(self, ciel: &mut Vec<u8>) {
        ciel.extend_from_slice(&self.to_le_bytes());
    }
    fn citaj_le(b: &[u8]) -> Self {
        let mut p = [0u8; 8];
        p.copy_from_slice(b);
        f64::from_le_bytes(p)
    }
    fn data(d: &Data) -> Option<&Arc<[Self]>> {
        if let Data::F64(x) = d { Some(x) } else { None }
    }
    fn zabal(v: Arc<[Self]>) -> Data {
        Data::F64(v)
    }
}

/// Hodnoty signálu v jednom zo siedmich typov.
#[derive(Clone, PartialEq)]
pub enum Data {
    U8(Arc<[u8]>),
    U16(Arc<[u16]>),
    I8(Arc<[i8]>),
    I16(Arc<[i16]>),
    I32(Arc<[i32]>),
    F32(Arc<[f32]>),
    F64(Arc<[f64]>),
}

macro_rules! pre_data {
    ($d:expr, $x:ident => $e:expr) => {
        match $d {
            Data::U8($x) => $e,
            Data::U16($x) => $e,
            Data::I8($x) => $e,
            Data::I16($x) => $e,
            Data::I32($x) => $e,
            Data::F32($x) => $e,
            Data::F64($x) => $e,
        }
    };
}

impl fmt::Debug for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Data({}, {} hodnôt)", self.typ().meno(), self.dlzka())
    }
}

impl Data {
    pub fn typ(&self) -> TypPrvku {
        match self {
            Data::U8(_) => TypPrvku::U8,
            Data::U16(_) => TypPrvku::U16,
            Data::I8(_) => TypPrvku::I8,
            Data::I16(_) => TypPrvku::I16,
            Data::I32(_) => TypPrvku::I32,
            Data::F32(_) => TypPrvku::F32,
            Data::F64(_) => TypPrvku::F64,
        }
    }

    pub fn dlzka(&self) -> usize {
        pre_data!(self, x => x.len())
    }

    pub fn hodnota(&self, i: usize) -> f64 {
        pre_data!(self, x => x[i].do_f64())
    }

    fn zapis_le(&self, ciel: &mut Vec<u8>) {
        ciel.reserve(self.dlzka() * self.typ().velkost());
        pre_data!(self, x => for v in x.iter() { v.zapis_le(ciel) })
    }
}

/// Správa o prevode typu – čo sa pri ňom stratilo (zmluva kvality: nič sa nestratí potichu).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpravaPrevodu {
    /// Počet hodnôt orezaných na rozsah cieľového typu.
    pub orezanych: u64,
    /// Najväčšia absolútna odchýlka spôsobená zaokrúhlením (bez orezaných hodnôt).
    pub najvacsia_odchylka: f64,
    /// Počet hodnôt, ktoré sa zmenili.
    pub zmenenych: u64,
}

impl SpravaPrevodu {
    pub fn je_bezstratovy(&self) -> bool {
        self.zmenenych == 0
    }
}

/// Signál.
#[derive(Clone)]
pub struct Signal {
    osi: Arc<[Os]>,
    data: Data,
    vyznam: Vlastnosti,
    odtlacok: OnceLock<Odtlacok>,
}

impl PartialEq for Signal {
    fn eq(&self, b: &Self) -> bool {
        self.osi == b.osi && self.vyznam == b.vyznam && self.data == b.data
    }
}

impl fmt::Debug for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let osi: Vec<String> = self.osi.iter().map(|o| format!("{}={}", o.meno, o.dlzka)).collect();
        write!(f, "Signal[{}; {}]", osi.join(", "), self.typ().meno())
    }
}

fn pocet_hodnot(osi: &[Os]) -> Vysledok<usize> {
    if osi.len() > NAJVIAC_OSI {
        return Err(Chyba::Limit(format!("viac ako {NAJVIAC_OSI} osí")));
    }
    for (i, o) in osi.iter().enumerate() {
        if osi[..i].iter().any(|p| p.meno == o.meno) {
            return Err(Chyba::NezhodaTvaru(format!("os {} sa opakuje", o.meno)));
        }
    }
    let mut n: u64 = 1;
    for o in osi {
        n = n.checked_mul(o.dlzka as u64).ok_or_else(|| Chyba::Limit("počet hodnôt pretiekol".into()))?;
    }
    if n > NAJVIAC_HODNOT {
        return Err(Chyba::Limit(format!("{n} hodnôt je viac ako {NAJVIAC_HODNOT}")));
    }
    usize::try_from(n).map_err(|_| Chyba::Limit("počet hodnôt".into()))
}

impl Signal {
    /// Nový signál z osí a hodnôt (posledná os sa mení najrýchlejšie).
    pub fn novy<T: PrvokSignalu>(osi: Vec<Os>, data: Vec<T>) -> Vysledok<Signal> {
        let n = pocet_hodnot(&osi)?;
        if n != data.len() {
            return Err(Chyba::NezhodaTvaru(format!("osi dávajú {n} hodnôt, dát je {}", data.len())));
        }
        Ok(Signal {
            osi: osi.into(),
            data: T::zabal(data.into()),
            vyznam: Vlastnosti::nove(),
            odtlacok: OnceLock::new(),
        })
    }

    /// Nový signál zo zdieľaných dát.
    pub fn zo_zdielanych(osi: Arc<[Os]>, data: Data, vyznam: Vlastnosti) -> Vysledok<Signal> {
        let n = pocet_hodnot(&osi)?;
        if n != data.dlzka() {
            return Err(Chyba::NezhodaTvaru(format!("osi dávajú {n} hodnôt, dát je {}", data.dlzka())));
        }
        Ok(Signal { osi, data, vyznam, odtlacok: OnceLock::new() })
    }

    /// Signál plný jednej hodnoty.
    pub fn vyplneny(osi: Vec<Os>, typ: TypPrvku, hodnota: f64) -> Vysledok<Signal> {
        let n = pocet_hodnot(&osi)?;
        fn plny<T: PrvokSignalu>(n: usize, h: f64) -> Data {
            T::zabal(vec![T::z_f64(h).0; n].into())
        }
        let data = match typ {
            TypPrvku::U8 => plny::<u8>(n, hodnota),
            TypPrvku::U16 => plny::<u16>(n, hodnota),
            TypPrvku::I8 => plny::<i8>(n, hodnota),
            TypPrvku::I16 => plny::<i16>(n, hodnota),
            TypPrvku::I32 => plny::<i32>(n, hodnota),
            TypPrvku::F32 => plny::<f32>(n, hodnota),
            TypPrvku::F64 => plny::<f64>(n, hodnota),
        };
        Ok(Signal { osi: osi.into(), data, vyznam: Vlastnosti::nove(), odtlacok: OnceLock::new() })
    }

    /// Ten istý signál s iným významom hodnôt (farebný priestor, jednotka, rozsah…).
    pub fn s_vyznamom(&self, vyznam: Vlastnosti) -> Signal {
        Signal { osi: self.osi.clone(), data: self.data.clone(), vyznam, odtlacok: OnceLock::new() }
    }

    /// Ten istý signál s inými osami rovnakého tvaru (napr. iné vzorkovanie alebo mená).
    pub fn s_osami(&self, osi: Vec<Os>) -> Vysledok<Signal> {
        if osi.len() != self.osi.len() || osi.iter().zip(self.osi.iter()).any(|(a, b)| a.dlzka != b.dlzka) {
            return Err(Chyba::NezhodaTvaru("nové osi musia mať rovnaký tvar".into()));
        }
        Signal::zo_zdielanych(osi.into(), self.data.clone(), self.vyznam.clone())
    }

    pub fn osi(&self) -> &[Os] {
        &self.osi
    }

    pub fn zdielane_osi(&self) -> Arc<[Os]> {
        self.osi.clone()
    }

    pub fn data(&self) -> &Data {
        &self.data
    }

    pub fn vyznam(&self) -> &Vlastnosti {
        &self.vyznam
    }

    pub fn typ(&self) -> TypPrvku {
        self.data.typ()
    }

    pub fn pocet(&self) -> usize {
        self.data.dlzka()
    }

    pub fn tvar(&self) -> Vec<usize> {
        self.osi.iter().map(|o| o.dlzka).collect()
    }

    /// Poradie a os podľa mena.
    pub fn os(&self, meno: &str) -> Option<(usize, &Os)> {
        self.osi.iter().enumerate().find(|(_, o)| o.meno.ako_text() == meno)
    }

    /// Má signál všetky tieto osi?
    pub fn ma_osi(&self, mena: &[Meno]) -> bool {
        mena.iter().all(|n| self.osi.iter().any(|o| &o.meno == n))
    }

    /// Kroky (v počte hodnôt) pre každú os.
    pub fn kroky(&self) -> Vec<usize> {
        let mut k = vec![1usize; self.osi.len()];
        for i in (0..self.osi.len().saturating_sub(1)).rev() {
            k[i] = k[i + 1] * self.osi[i + 1].dlzka;
        }
        k
    }

    /// Hodnoty určitého typu bez kópie.
    pub fn hodnoty<T: PrvokSignalu>(&self) -> Option<&[T]> {
        T::data(&self.data).map(|x| &x[..])
    }

    /// Hodnota na indexe (v poradí osí).
    pub fn hodnota(&self, index: &[usize]) -> Vysledok<f64> {
        if index.len() != self.osi.len() {
            return Err(Chyba::NezhodaTvaru(format!(
                "index má {} zložiek, osí je {}",
                index.len(),
                self.osi.len()
            )));
        }
        let mut poloha = 0;
        for ((&i, o), k) in index.iter().zip(self.osi.iter()).zip(self.kroky()) {
            if i >= o.dlzka {
                return Err(Chyba::MimoRozsahu(format!("index {i} na osi {} s dĺžkou {}", o.meno, o.dlzka)));
            }
            poloha += i * k;
        }
        Ok(self.data.hodnota(poloha))
    }

    /// Všetky hodnoty ako f64 (kópia).
    pub fn ako_f64(&self) -> Vec<f64> {
        pre_data!(&self.data, x => x.iter().map(|v| v.do_f64()).collect())
    }

    /// Všetky hodnoty ako f32 (kópia; pre f64 so zaokrúhlením).
    pub fn ako_f32(&self) -> Vec<f32> {
        match &self.data {
            Data::F32(x) => x.to_vec(),
            d => pre_data!(d, x => x.iter().map(|v| v.do_f64() as f32).collect()),
        }
    }

    /// Prevod na iný typ s presnou správou o strate.
    pub fn preved(&self, ciel: TypPrvku) -> (Signal, SpravaPrevodu) {
        fn na<T: PrvokSignalu>(zdroj: &Data, sprava: &mut SpravaPrevodu) -> Data {
            let mut v = Vec::with_capacity(zdroj.dlzka());
            pre_data!(zdroj, x => for h in x.iter() {
                let p = h.do_f64();
                let (nova, orezana) = T::z_f64(p);
                let spat = nova.do_f64();
                if orezana {
                    sprava.orezanych += 1;
                    sprava.zmenenych += 1;
                } else if spat != p && !(spat.is_nan() && p.is_nan()) {
                    sprava.zmenenych += 1;
                    let o = (spat - p).abs();
                    if o > sprava.najvacsia_odchylka { sprava.najvacsia_odchylka = o; }
                }
                v.push(nova);
            });
            T::zabal(v.into())
        }
        let mut sprava = SpravaPrevodu::default();
        if ciel == self.typ() {
            return (self.clone(), sprava);
        }
        let data = match ciel {
            TypPrvku::U8 => na::<u8>(&self.data, &mut sprava),
            TypPrvku::U16 => na::<u16>(&self.data, &mut sprava),
            TypPrvku::I8 => na::<i8>(&self.data, &mut sprava),
            TypPrvku::I16 => na::<i16>(&self.data, &mut sprava),
            TypPrvku::I32 => na::<i32>(&self.data, &mut sprava),
            TypPrvku::F32 => na::<f32>(&self.data, &mut sprava),
            TypPrvku::F64 => na::<f64>(&self.data, &mut sprava),
        };
        let s =
            Signal { osi: self.osi.clone(), data, vyznam: self.vyznam.clone(), odtlacok: OnceLock::new() };
        (s, sprava)
    }

    /// Nový signál rovnakého tvaru vypočítaný po hodnotách v f64 a uložený v type `ciel`.
    pub fn mapuj(&self, ciel: TypPrvku, f: impl Fn(f64) -> f64) -> Signal {
        let hodnoty: Vec<f64> = self.ako_f64().into_iter().map(f).collect();
        let s = Signal {
            osi: self.osi.clone(),
            data: Data::F64(hodnoty.into()),
            vyznam: self.vyznam.clone(),
            odtlacok: OnceLock::new(),
        };
        s.preved(ciel).0
    }

    /// Preusporiada osi podľa mien (napr. [„kanál“, „y“, „x“]). Mená musia byť práve osi signálu.
    pub fn preusporiadaj(&self, poradie: &[Meno]) -> Vysledok<Signal> {
        if poradie.len() != self.osi.len() {
            return Err(Chyba::NezhodaTvaru("poradie musí obsahovať všetky osi".into()));
        }
        let mut perm = Vec::with_capacity(poradie.len());
        for n in poradie {
            let (i, _) = self.os(n.ako_text()).ok_or_else(|| Chyba::Chybajuce(format!("os {n}")))?;
            if perm.contains(&i) {
                return Err(Chyba::NezhodaTvaru(format!("os {n} je v poradí dvakrát")));
            }
            perm.push(i);
        }
        if perm.iter().enumerate().all(|(a, &b)| a == b) {
            return Ok(self.clone());
        }
        let nove_osi: Vec<Os> = perm.iter().map(|&i| self.osi[i].clone()).collect();
        let stare_kroky = self.kroky();
        let kroky: Vec<usize> = perm.iter().map(|&i| stare_kroky[i]).collect();
        let tvar: Vec<usize> = nove_osi.iter().map(|o| o.dlzka).collect();
        let n = self.pocet();
        fn presun<T: PrvokSignalu>(x: &[T], n: usize, tvar: &[usize], kroky: &[usize]) -> Vec<T> {
            let mut out = Vec::with_capacity(n);
            let mut idx = vec![0usize; tvar.len()];
            let mut zdroj = 0usize;
            for _ in 0..n {
                out.push(x[zdroj]);
                let mut d = tvar.len();
                while d > 0 {
                    d -= 1;
                    idx[d] += 1;
                    zdroj += kroky[d];
                    if idx[d] < tvar[d] {
                        break;
                    }
                    zdroj -= kroky[d] * tvar[d];
                    idx[d] = 0;
                }
            }
            out
        }
        let data = pre_data!(&self.data, x => PrvokSignalu::zabal(presun(x, n, &tvar, &kroky).into()));
        Signal::zo_zdielanych(nove_osi.into(), data, self.vyznam.clone())
    }

    /// Výrez pozdĺž jednej osi (indexy od..po).
    pub fn vyrez(&self, os: &str, od: usize, po: usize) -> Vysledok<Signal> {
        let (ia, o) = self.os(os).ok_or_else(|| Chyba::Chybajuce(format!("os {os}")))?;
        if od > po || po > o.dlzka {
            return Err(Chyba::MimoRozsahu(format!("výrez {od}..{po} osi {os} s dĺžkou {}", o.dlzka)));
        }
        let vonkajsie: usize = self.osi[..ia].iter().map(|o| o.dlzka).product();
        let vnutorne: usize = self.osi[ia + 1..].iter().map(|o| o.dlzka).product();
        let mut nove_osi: Vec<Os> = self.osi.to_vec();
        let mut nova = o.s_dlzkou(po - od);
        if let crate::os::Vzorkovanie::Pravidelne { zaciatok, krok, .. } = &mut nova.vzorkovanie
            && let Some(z) = krok.krat_cele(od as i64).and_then(|p| p.plus(zaciatok))
        {
            *zaciatok = z;
        }
        if let crate::os::Vzorkovanie::Oznacene(mena) = &mut nova.vzorkovanie {
            *mena = mena[od..po].to_vec().into();
        }
        nove_osi[ia] = nova;
        fn rez<T: PrvokSignalu>(
            x: &[T],
            von: usize,
            dlzka: usize,
            vnu: usize,
            od: usize,
            po: usize,
        ) -> Vec<T> {
            let mut out = Vec::with_capacity(von * (po - od) * vnu);
            for v in 0..von {
                let z = (v * dlzka + od) * vnu;
                out.extend_from_slice(&x[z..z + (po - od) * vnu]);
            }
            out
        }
        let data = pre_data!(&self.data, x => PrvokSignalu::zabal(rez(x, vonkajsie, o.dlzka, vnutorne, od, po).into()));
        Signal::zo_zdielanych(nove_osi.into(), data, self.vyznam.clone())
    }

    /// Odtlačok obsahu (osi, typ, hodnoty, význam) – počíta sa raz a pamätá sa.
    pub fn odtlacok(&self) -> Odtlacok {
        *self.odtlacok.get_or_init(|| {
            let mut z = Zapisovac::novy();
            self.zakoduj_hlavicku(&mut z);
            let mut h = Hasovac::novy();
            h.pridaj(z.bajty());
            let mut kus = Vec::with_capacity(1 << 16);
            pre_data!(&self.data, x => for cast in x.chunks(8192) {
                kus.clear();
                for v in cast { v.zapis_le(&mut kus); }
                h.pridaj(&kus);
            });
            h.dokonci()
        })
    }

    fn zakoduj_hlavicku(&self, z: &mut Zapisovac) {
        z.cislo(self.osi.len() as u64);
        for o in self.osi.iter() {
            o.zakoduj(z);
        }
        self.vyznam.zakoduj(z);
        z.u8(self.typ().kod()).cislo(self.pocet() as u64);
    }
}

impl Kodovatelne for Signal {
    fn zakoduj(&self, z: &mut Zapisovac) {
        self.zakoduj_hlavicku(z);
        let mut b = Vec::new();
        self.data.zapis_le(&mut b);
        z.surove(&b);
    }

    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        c.vnor()?;
        let pocet_osi = c.pocet(3)?;
        if pocet_osi > NAJVIAC_OSI {
            return Err(Chyba::Limit("počet osí".into()));
        }
        let mut osi = Vec::with_capacity(pocet_osi);
        for _ in 0..pocet_osi {
            osi.push(Os::dekoduj(c)?);
        }
        let vyznam = Vlastnosti::dekoduj(c)?;
        let typ = TypPrvku::z_kodu(c.u8()?)?;
        let n = usize::try_from(c.cislo()?).map_err(|_| Chyba::Limit("počet hodnôt".into()))?;
        if n != pocet_hodnot(&osi)? {
            return Err(Chyba::PoskodeneData("počet hodnôt nesedí s osami".into()));
        }
        let bajtov = n.checked_mul(typ.velkost()).ok_or_else(|| Chyba::Limit("veľkosť dát".into()))?;
        let b = c.surove(bajtov)?;
        fn citaj<T: PrvokSignalu>(b: &[u8]) -> Data {
            let k = std::mem::size_of::<T>();
            T::zabal(b.chunks_exact(k).map(T::citaj_le).collect::<Vec<T>>().into())
        }
        let data = match typ {
            TypPrvku::U8 => citaj::<u8>(b),
            TypPrvku::U16 => citaj::<u16>(b),
            TypPrvku::I8 => citaj::<i8>(b),
            TypPrvku::I16 => citaj::<i16>(b),
            TypPrvku::I32 => citaj::<i32>(b),
            TypPrvku::F32 => citaj::<f32>(b),
            TypPrvku::F64 => citaj::<f64>(b),
        };
        c.vynor();
        Signal::zo_zdielanych(osi.into(), data, vyznam)
    }
}

#[cfg(test)]
mod testy {
    use super::*;
    use crate::meno::m;

    fn obraz() -> Signal {
        let osi = vec![
            Os::poradova("y", 2).unwrap(),
            Os::poradova("x", 3).unwrap(),
            Os::oznacena("kanál", &["R", "G"]).unwrap(),
        ];
        Signal::novy(osi, (0..12).map(|i| i as f32).collect()).unwrap()
    }

    #[test]
    fn tvar_a_hodnoty() {
        let s = obraz();
        assert_eq!(s.tvar(), vec![2, 3, 2]);
        assert_eq!(s.kroky(), vec![6, 2, 1]);
        assert_eq!(s.hodnota(&[1, 2, 1]).unwrap(), 11.0);
        assert!(s.hodnota(&[2, 0, 0]).is_err());
        assert!(Signal::novy(vec![Os::poradova("x", 3).unwrap()], vec![1u8, 2]).is_err());
        let dvakrat = vec![Os::poradova("x", 1).unwrap(), Os::poradova("x", 1).unwrap()];
        assert!(Signal::novy(dvakrat, vec![1u8]).is_err());
    }

    #[test]
    fn preusporiadanie_podla_mien() {
        let s = obraz();
        let p = s.preusporiadaj(&[m("kanál"), m("y"), m("x")]).unwrap();
        assert_eq!(p.tvar(), vec![2, 2, 3]);
        for y in 0..2 {
            for x in 0..3 {
                for k in 0..2 {
                    assert_eq!(s.hodnota(&[y, x, k]).unwrap(), p.hodnota(&[k, y, x]).unwrap());
                }
            }
        }
        let spat = p.preusporiadaj(&[m("y"), m("x"), m("kanál")]).unwrap();
        assert_eq!(spat, s);
    }

    #[test]
    fn vyrez_a_presna_poloha() {
        let t = Os::casova(10, 1000).unwrap();
        let s = Signal::novy(vec![t], (0..10).map(|i| i as f64).collect()).unwrap();
        let v = s.vyrez("t", 4, 7).unwrap();
        assert_eq!(v.ako_f64(), vec![4.0, 5.0, 6.0]);
        assert_eq!(v.osi()[0].presna_poloha(0), Some(crate::jednotka::Zlomok::novy(4, 1000).unwrap()));
        let k = obraz().vyrez("kanál", 1, 2).unwrap();
        assert_eq!(k.osi()[2].mena_vzoriek().unwrap(), &[m("G")]);
        assert_eq!(k.ako_f32(), vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0]);
    }

    #[test]
    fn prevod_hlasi_straty() {
        let s = Signal::novy(vec![Os::poradova("x", 4).unwrap()], vec![0.4f32, 1.6, 300.0, -5.0]).unwrap();
        let (u, sprava) = s.preved(TypPrvku::U8);
        assert_eq!(u.hodnoty::<u8>().unwrap(), &[0, 2, 255, 0]);
        assert_eq!(sprava.orezanych, 2);
        assert_eq!(sprava.zmenenych, 4);
        assert!((sprava.najvacsia_odchylka - 0.4).abs() < 1e-6);
        let (_, bez) = u.preved(TypPrvku::F32);
        assert!(bez.je_bezstratovy());
        assert!(TypPrvku::U16.bezstratovo_do(TypPrvku::F32));
        assert!(!TypPrvku::I32.bezstratovo_do(TypPrvku::F32));
    }

    #[test]
    fn kodovanie_a_odtlacok() {
        let s = obraz();
        let b = s.do_bajtov();
        let spat = Signal::z_bajtov(&b).unwrap();
        assert_eq!(spat, s);
        assert_eq!(spat.odtlacok(), s.odtlacok());
        let ine = s.mapuj(TypPrvku::F32, |x| x + 1.0);
        assert_ne!(ine.odtlacok(), s.odtlacok());
        for d in 0..b.len() {
            assert!(Signal::z_bajtov(&b[..d]).is_err(), "neúplné dáta dĺžky {d} musia zlyhať");
        }
    }
}
