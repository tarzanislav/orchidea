//! Vektorový vykonávač kernelov na procesore – „SPMD na SIMD“.
//!
//! Interpret spracúva vlákna jedno po druhom a pri každom uzle výrazu platí réžiu rozhodovania. Vektorový
//! vykonávač preloží kernel raz do lineárneho programu nad registrami a vykoná ho naraz pre blok
//! [`PASY`] vlákien (pruhov): každá inštrukcia je tesný cyklus cez pruhy, ktorý prekladač premení na SIMD
//! inštrukcie (SSE/AVX, NEON), a réžia rozhodovania sa rozpočíta na celý blok – princíp vektorového
//! vykonávania. Rozvetvenie riadia masky aktívnych pruhov (princíp SPMD na SIMD), bloky sa rozdelia medzi
//! všetky jadrá procesora. Registre jedného bloku sa zmestia do vyrovnávacej pamäte L1/L2 – aj na
//! Raspberry Pi.
//!
//! Je to rýchla „CPU cesta vždy“ pre zariadenia bez použiteľného GPU (Raspberry Pi 3, kancelárske PC,
//! virtuálne stroje) a záloha všade inde.
//!
//! **Zhoda:** výsledky sú bitovo rovnaké ako z referenčného interpretu (okrem obsahu NaN, ktorý neurčuje ani
//! žiadne GPU), ak je kernel bez súbehu – žiadne vlákno nečíta prvok, ktorý zapisuje iné vlákno, a žiadne
//! dve vlákna nezapisujú ten istý prvok. Rovnakú podmienku kladie každé GPU. Čítanie a zápis mimo bufferu
//! aj celočíselné delenie nulou sú chyby rovnako ako v interprete.

use crate::ir::{Kernel, Op1, Op2, Prikaz, Typ, Vyraz};
use crate::zariadenie::{DataVazby, Hodnota32, over_argumenty};
use orchidea_jadro::matematika::f32v;
use orchidea_jadro::{Chyba, Vysledok, paralel};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Počet vlákien (pruhov) spracovaných naraz. 256 pruhov × 4 bajty = 1 KiB na register.
pub const PASY: usize = 256;

/// Najviac krokov jedného bloku (ochrana pred nekonečným cyklom).
const NAJVIAC_KROKOV: u64 = 1 << 32;

/// Najmenej blokov na jedno vlákno procesora – menšie úlohy sa nedelia (réžia vlákien by prevýšila zisk).
const NAJMENEJ_BLOKOV_NA_VLAKNO: usize = 8;

type Reg = u32;
type Pas = [u32; PASY];

#[derive(Debug, Clone)]
enum Ins {
    Citaj { r: Reg, vazba: usize, index: Reg },
    Bin { r: Reg, op: Op2, typ: Typ, a: Reg, b: Reg },
    Un { r: Reg, op: Op1, typ: Typ, a: Reg },
    Prevod { r: Reg, z: Typ, na: Typ, a: Reg },
    Vyber { r: Reg, p: Reg, a: Reg, b: Reg },
    Kopia { r: Reg, z: Reg },
    Nastav { premenna: Reg, z: Reg },
    Zapis { vazba: usize, index: Reg, hodnota: Reg },
    Ak { podmienka: Reg, potom: Vec<Ins>, inak: Vec<Ins> },
    Pre { premenna: Reg, od: Reg, po: Reg, telo: Vec<Ins> },
}

/// Obsah jednotného registra (rovnaký pre všetky pruhy, naplní sa raz pri spustení).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Jednotny {
    Bity(u32),
    Konstanta(usize),
    Dlzka(usize),
}

/// Kernel preložený pre vektorový vykonávač.
#[derive(Debug, Clone)]
pub struct Program {
    kernel: Kernel,
    kod: Vec<Ins>,
    registrov: usize,
    premenne: Vec<Reg>,
    jednotne: Vec<(Reg, Jednotny)>,
    id: [Option<Reg>; 3],
    hlbka: usize,
}

#[derive(Debug, Clone, Copy)]
enum Hodnota {
    Docasna(Reg),
    Trvala(Reg),
}

impl Hodnota {
    fn reg(self) -> Reg {
        match self {
            Hodnota::Docasna(r) | Hodnota::Trvala(r) => r,
        }
    }
}

struct Prekladac<'k> {
    kernel: &'k Kernel,
    registrov: Reg,
    volne: Vec<Reg>,
    jednotne: HashMap<Jednotny, Reg>,
    premenne: Vec<Reg>,
    id: [Option<Reg>; 3],
    hlbka: usize,
}

impl Prekladac<'_> {
    fn novy(&mut self) -> Reg {
        self.volne.pop().unwrap_or_else(|| {
            self.registrov += 1;
            self.registrov - 1
        })
    }

    fn uvolni(&mut self, h: Hodnota) {
        if let Hodnota::Docasna(r) = h {
            self.volne.push(r);
        }
    }

    fn jednotny(&mut self, j: Jednotny) -> Hodnota {
        if let Some(r) = self.jednotne.get(&j) {
            return Hodnota::Trvala(*r);
        }
        self.registrov += 1;
        let r = self.registrov - 1;
        self.jednotne.insert(j, r);
        Hodnota::Trvala(r)
    }

    /// Hodnotu, ktorú môže telo cyklu alebo vetva zmeniť (premenná), skopíruje do dočasného registra.
    fn zachovaj(&mut self, h: Hodnota, kod: &mut Vec<Ins>) -> Hodnota {
        match h {
            Hodnota::Docasna(_) => h,
            Hodnota::Trvala(z) => {
                let r = self.novy();
                kod.push(Ins::Kopia { r, z });
                Hodnota::Docasna(r)
            }
        }
    }

    fn vyraz(&mut self, v: &Vyraz, kod: &mut Vec<Ins>) -> Vysledok<(Hodnota, Typ)> {
        Ok(match v {
            Vyraz::F32(x) => (self.jednotny(Jednotny::Bity(x.to_bits())), Typ::F32),
            Vyraz::U32(x) => (self.jednotny(Jednotny::Bity(*x)), Typ::U32),
            Vyraz::I32(x) => (self.jednotny(Jednotny::Bity(*x as u32)), Typ::I32),
            Vyraz::Bool(x) => (self.jednotny(Jednotny::Bity(*x as u32)), Typ::Bool),
            Vyraz::Premenna(p) => (Hodnota::Trvala(self.premenne[p.0]), self.kernel.premenne[p.0]),
            Vyraz::Konstanta(k) => (self.jednotny(Jednotny::Konstanta(k.0)), self.kernel.konstanty[k.0].typ),
            Vyraz::Dlzka(b) => (self.jednotny(Jednotny::Dlzka(b.0)), Typ::U32),
            Vyraz::IdVlakna(os) => {
                let os = *os as usize;
                let r = match self.id[os] {
                    Some(r) => r,
                    None => {
                        self.registrov += 1;
                        self.id[os] = Some(self.registrov - 1);
                        self.registrov - 1
                    }
                };
                (Hodnota::Trvala(r), Typ::U32)
            }
            Vyraz::Citaj { vazba, index } => {
                let (i, _) = self.vyraz(index, kod)?;
                let r = self.novy();
                kod.push(Ins::Citaj { r, vazba: vazba.0, index: i.reg() });
                self.uvolni(i);
                (Hodnota::Docasna(r), self.kernel.vazby[vazba.0].typ)
            }
            Vyraz::Bin(op, a, b) => {
                let (ha, typ) = self.vyraz(a, kod)?;
                let (hb, _) = self.vyraz(b, kod)?;
                let r = self.novy();
                kod.push(Ins::Bin { r, op: *op, typ, a: ha.reg(), b: hb.reg() });
                self.uvolni(ha);
                self.uvolni(hb);
                let t = if op.je_porovnanie() || op.je_logicka() { Typ::Bool } else { typ };
                (Hodnota::Docasna(r), t)
            }
            Vyraz::Un(op, a) => {
                let (ha, typ) = self.vyraz(a, kod)?;
                let r = self.novy();
                kod.push(Ins::Un { r, op: *op, typ, a: ha.reg() });
                self.uvolni(ha);
                (Hodnota::Docasna(r), typ)
            }
            Vyraz::Prevod(na, a) => {
                let (ha, z) = self.vyraz(a, kod)?;
                if z == *na {
                    return Ok((ha, z));
                }
                let r = self.novy();
                kod.push(Ins::Prevod { r, z, na: *na, a: ha.reg() });
                self.uvolni(ha);
                (Hodnota::Docasna(r), *na)
            }
            Vyraz::Vyber(p, a, b) => {
                let (hp, _) = self.vyraz(p, kod)?;
                let (ha, typ) = self.vyraz(a, kod)?;
                let (hb, _) = self.vyraz(b, kod)?;
                let r = self.novy();
                kod.push(Ins::Vyber { r, p: hp.reg(), a: ha.reg(), b: hb.reg() });
                self.uvolni(hp);
                self.uvolni(ha);
                self.uvolni(hb);
                (Hodnota::Docasna(r), typ)
            }
        })
    }

    fn prikazy(&mut self, prikazy: &[Prikaz], hlbka: usize) -> Vysledok<Vec<Ins>> {
        self.hlbka = self.hlbka.max(hlbka);
        let mut kod = Vec::new();
        for p in prikazy {
            match p {
                Prikaz::Nastav(pr, v) => {
                    let (h, _) = self.vyraz(v, &mut kod)?;
                    let premenna = self.premenne[pr.0];
                    if h.reg() != premenna {
                        kod.push(Ins::Nastav { premenna, z: h.reg() });
                    }
                    self.uvolni(h);
                }
                Prikaz::Zapis { vazba, index, hodnota } => {
                    let (i, _) = self.vyraz(index, &mut kod)?;
                    let (h, _) = self.vyraz(hodnota, &mut kod)?;
                    kod.push(Ins::Zapis { vazba: vazba.0, index: i.reg(), hodnota: h.reg() });
                    self.uvolni(i);
                    self.uvolni(h);
                }
                Prikaz::Ak { podmienka, potom, inak } => {
                    let (c, _) = self.vyraz(podmienka, &mut kod)?;
                    // vetva „potom“ môže zmeniť premennú podmienky – maska vetvy „inak“ sa počíta z kópie
                    let c = if inak.is_empty() { c } else { self.zachovaj(c, &mut kod) };
                    let potom = self.prikazy(potom, hlbka + 1)?;
                    let inak = self.prikazy(inak, hlbka + 1)?;
                    kod.push(Ins::Ak { podmienka: c.reg(), potom, inak });
                    self.uvolni(c);
                }
                Prikaz::Pre { premenna, od, po, telo } => {
                    let (o, _) = self.vyraz(od, &mut kod)?;
                    let (k, _) = self.vyraz(po, &mut kod)?;
                    // horná hranica sa vyhodnotí raz – telo ju nesmie zmeniť
                    let k = self.zachovaj(k, &mut kod);
                    let premenna = self.premenne[premenna.0];
                    // počiatočná hodnota sa prečíta pred telom, takže jej register môže telo znovu použiť
                    self.uvolni(o);
                    let telo = self.prikazy(telo, hlbka + 1)?;
                    kod.push(Ins::Pre { premenna, od: o.reg(), po: k.reg(), telo });
                    self.uvolni(k);
                }
            }
        }
        Ok(kod)
    }
}

/// Preloží kernel pre vektorový vykonávač (kernel sa najprv overí).
pub fn preloz(kernel: &Kernel) -> Vysledok<Program> {
    kernel.over()?;
    let mut p = Prekladac {
        kernel,
        registrov: 0,
        volne: Vec::new(),
        jednotne: HashMap::new(),
        premenne: Vec::new(),
        id: [None; 3],
        hlbka: 0,
    };
    p.premenne = (0..kernel.premenne.len() as Reg).collect();
    p.registrov = kernel.premenne.len() as Reg;
    let kod = p.prikazy(&kernel.telo, 0)?;
    let mut jednotne: Vec<(Reg, Jednotny)> = p.jednotne.into_iter().map(|(j, r)| (r, j)).collect();
    jednotne.sort_by_key(|(r, _)| *r);
    Ok(Program {
        kernel: kernel.clone(),
        kod,
        registrov: p.registrov as usize,
        premenne: p.premenne,
        jednotne,
        id: p.id,
        hlbka: p.hlbka + 1,
    })
}

/// Pohľad na buffer zdieľaný vláknami procesora. Kernel bez súbehu nikdy nečíta ani nezapisuje prvok,
/// ktorý zapisuje iné vlákno, takže stačia atomické operácie bez usporiadania – na x86 aj ARM sú to obyčajné
/// čítania a zápisy.
#[derive(Debug, Clone, Copy)]
struct Buffer<'a> {
    data: &'a [AtomicU32],
}

/// Typy prvkov bufferov: 4 bajty bez neplatných bitových vzorov.
trait Prvok32 {}
impl Prvok32 for f32 {}
impl Prvok32 for u32 {}
impl Prvok32 for i32 {}

/// Vytvorí atomický pohľad na výlučne požičaný buffer. Ak pamäť nie je zarovnaná na 4 bajty (na bežných
/// platformách sa to nestane), vráti `None` a vykonávač použije interpret.
#[allow(unsafe_code)]
fn atomicky_pohlad<T: Prvok32>(d: &mut [T]) -> Option<&[AtomicU32]> {
    const { assert!(size_of::<T>() == size_of::<AtomicU32>()) };
    let p = d.as_mut_ptr();
    if !(p as usize).is_multiple_of(align_of::<AtomicU32>()) {
        return None;
    }
    // SAFETY: T je f32, u32 alebo i32 – 4 bajty, každý bitový vzor je platný; AtomicU32 má rovnakú veľkosť
    // a zarovnanie sme overili. Pohľad žije najviac tak dlho ako výlučná výpožička `d`, takže počas jeho
    // života sa k pamäti pristupuje len cez neho (atomickými operáciami).
    Some(unsafe { std::slice::from_raw_parts(p.cast::<AtomicU32>().cast_const(), d.len()) })
}

/// Pracovná pamäť jedného vlákna procesora: registre a masky jedného bloku.
struct Pracovisko {
    reg: Vec<Vec<u32>>,
    masky: Vec<Vec<u32>>,
    kroky: u64,
}

fn chyba(s: impl Into<String>) -> Chyba {
    Chyba::Vypocet(s.into())
}

#[inline(always)]
fn pas(v: &[u32]) -> &Pas {
    v.try_into().expect("register má PASY prvkov")
}

#[inline(always)]
fn pas_mut(v: &mut [u32]) -> &mut Pas {
    v.try_into().expect("register má PASY prvkov")
}

#[inline(always)]
fn mapuj1(o: &mut Pas, a: &Pas, f: impl Fn(u32) -> u32) {
    for l in 0..PASY {
        o[l] = f(a[l]);
    }
}

#[inline(always)]
fn mapuj2(o: &mut Pas, a: &Pas, b: &Pas, f: impl Fn(u32, u32) -> u32) {
    for l in 0..PASY {
        o[l] = f(a[l], b[l]);
    }
}

#[inline(always)]
fn ff(f: impl Fn(f32, f32) -> f32) -> impl Fn(u32, u32) -> u32 {
    move |a, b| f(f32::from_bits(a), f32::from_bits(b)).to_bits()
}

#[inline(always)]
fn fb(f: impl Fn(f32, f32) -> bool) -> impl Fn(u32, u32) -> u32 {
    move |a, b| f(f32::from_bits(a), f32::from_bits(b)) as u32
}

#[inline(always)]
fn ii(f: impl Fn(i32, i32) -> i32) -> impl Fn(u32, u32) -> u32 {
    move |a, b| f(a as i32, b as i32) as u32
}

#[inline(always)]
fn ib(f: impl Fn(i32, i32) -> bool) -> impl Fn(u32, u32) -> u32 {
    move |a, b| f(a as i32, b as i32) as u32
}

#[inline(always)]
fn ub(f: impl Fn(u32, u32) -> bool) -> impl Fn(u32, u32) -> u32 {
    move |a, b| f(a, b) as u32
}

/// Celočíselné delenie a zvyšok – len v aktívnych pruhoch, delenie nulou je chyba (na GPU nedefinované).
fn delenie(o: &mut Pas, a: &Pas, b: &Pas, m: &Pas, f: impl Fn(u32, u32) -> u32) -> Vysledok<()> {
    for l in 0..PASY {
        if m[l] != 0 {
            if b[l] == 0 {
                return Err(chyba("celočíselné delenie nulou (na GPU je výsledok nedefinovaný)"));
            }
            o[l] = f(a[l], b[l]);
        } else {
            o[l] = 0;
        }
    }
    Ok(())
}

fn binarna(op: Op2, typ: Typ, o: &mut Pas, a: &Pas, b: &Pas, m: &Pas) -> Vysledok<()> {
    use Op2::*;
    use Typ::*;
    match (op, typ) {
        (Plus, F32) => mapuj2(o, a, b, ff(|x, y| x + y)),
        (Minus, F32) => mapuj2(o, a, b, ff(|x, y| x - y)),
        (Krat, F32) => mapuj2(o, a, b, ff(|x, y| x * y)),
        (Deleno, F32) => mapuj2(o, a, b, ff(|x, y| x / y)),
        (Zvysok, F32) => mapuj2(o, a, b, ff(|x, y| x % y)),
        (Min, F32) => mapuj2(o, a, b, ff(f32::min)),
        (Max, F32) => mapuj2(o, a, b, ff(f32::max)),
        (Mensie, F32) => mapuj2(o, a, b, fb(|x, y| x < y)),
        (MensieRovne, F32) => mapuj2(o, a, b, fb(|x, y| x <= y)),
        (Vacsie, F32) => mapuj2(o, a, b, fb(|x, y| x > y)),
        (VacsieRovne, F32) => mapuj2(o, a, b, fb(|x, y| x >= y)),
        (Rovne, F32) => mapuj2(o, a, b, fb(|x, y| x == y)),
        (Rozne, F32) => mapuj2(o, a, b, fb(|x, y| x != y)),

        (Plus, U32) => mapuj2(o, a, b, u32::wrapping_add),
        (Minus, U32) => mapuj2(o, a, b, u32::wrapping_sub),
        (Krat, U32) => mapuj2(o, a, b, u32::wrapping_mul),
        (Deleno, U32) => delenie(o, a, b, m, |x, y| x / y)?,
        (Zvysok, U32) => delenie(o, a, b, m, |x, y| x % y)?,
        (Min, U32) => mapuj2(o, a, b, u32::min),
        (Max, U32) => mapuj2(o, a, b, u32::max),
        (Mensie, U32) => mapuj2(o, a, b, ub(|x, y| x < y)),
        (MensieRovne, U32) => mapuj2(o, a, b, ub(|x, y| x <= y)),
        (Vacsie, U32) => mapuj2(o, a, b, ub(|x, y| x > y)),
        (VacsieRovne, U32) => mapuj2(o, a, b, ub(|x, y| x >= y)),
        (Rovne, U32 | Bool) => mapuj2(o, a, b, ub(|x, y| x == y)),
        (Rozne, U32 | Bool) => mapuj2(o, a, b, ub(|x, y| x != y)),

        (Plus, I32) => mapuj2(o, a, b, ii(i32::wrapping_add)),
        (Minus, I32) => mapuj2(o, a, b, ii(i32::wrapping_sub)),
        (Krat, I32) => mapuj2(o, a, b, ii(i32::wrapping_mul)),
        (Deleno, I32) => delenie(o, a, b, m, |x, y| (x as i32).wrapping_div(y as i32) as u32)?,
        (Zvysok, I32) => delenie(o, a, b, m, |x, y| (x as i32).wrapping_rem(y as i32) as u32)?,
        (Min, I32) => mapuj2(o, a, b, ii(i32::min)),
        (Max, I32) => mapuj2(o, a, b, ii(i32::max)),
        (Mensie, I32) => mapuj2(o, a, b, ib(|x, y| x < y)),
        (MensieRovne, I32) => mapuj2(o, a, b, ib(|x, y| x <= y)),
        (Vacsie, I32) => mapuj2(o, a, b, ib(|x, y| x > y)),
        (VacsieRovne, I32) => mapuj2(o, a, b, ib(|x, y| x >= y)),
        (Rovne, I32) => mapuj2(o, a, b, ub(|x, y| x == y)),
        (Rozne, I32) => mapuj2(o, a, b, ub(|x, y| x != y)),

        (A, Bool) => mapuj2(o, a, b, |x, y| x & y),
        (Alebo, Bool) => mapuj2(o, a, b, |x, y| x | y),
        _ => return Err(chyba(format!("neplatná operácia {op:?} na type {typ:?}"))),
    }
    Ok(())
}

fn unarna(op: Op1, typ: Typ, o: &mut Pas, a: &Pas) -> Vysledok<()> {
    let f = |g: fn(f32) -> f32| move |x: u32| g(f32::from_bits(x)).to_bits();
    match (op, typ) {
        (Op1::Zaporne, Typ::F32) => mapuj1(o, a, |x| x ^ 0x8000_0000),
        (Op1::Zaporne, Typ::I32) => mapuj1(o, a, |x| (x as i32).wrapping_neg() as u32),
        (Op1::Nie, Typ::Bool) => mapuj1(o, a, |x| x ^ 1),
        (Op1::Abs, Typ::F32) => mapuj1(o, a, |x| x & 0x7fff_ffff),
        (Op1::Abs, Typ::I32) => mapuj1(o, a, |x| (x as i32).wrapping_abs() as u32),
        (Op1::Odmocnina, Typ::F32) => mapuj1(o, a, f(f32::sqrt)),
        (Op1::Exp, Typ::F32) => mapuj1(o, a, f(f32v::exp)),
        (Op1::Ln, Typ::F32) => mapuj1(o, a, f(f32v::ln)),
        (Op1::Dole, Typ::F32) => mapuj1(o, a, f(f32::floor)),
        _ => return Err(chyba(format!("neplatná operácia {op:?} na type {typ:?}"))),
    }
    Ok(())
}

fn prevod(z: Typ, na: Typ, o: &mut Pas, a: &Pas) -> Vysledok<()> {
    use Typ::*;
    match (z, na) {
        (U32, F32) => mapuj1(o, a, |x| (x as f32).to_bits()),
        (I32, F32) => mapuj1(o, a, |x| (x as i32 as f32).to_bits()),
        (F32, U32) => mapuj1(o, a, |x| f32::from_bits(x) as u32),
        (F32, I32) => mapuj1(o, a, |x| f32::from_bits(x) as i32 as u32),
        (U32 | I32, U32 | I32) | (F32, F32) => o.copy_from_slice(a),
        _ => return Err(chyba("neplatný prevod")),
    }
    Ok(())
}

impl Program {
    /// Pôvodný kernel.
    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    fn nove_pracovisko(&self, buffery: &[Buffer<'_>], konstanty: &[Hodnota32]) -> Pracovisko {
        let mut reg = vec![vec![0u32; PASY]; self.registrov];
        for (r, j) in &self.jednotne {
            let bity = match *j {
                Jednotny::Bity(b) => b,
                Jednotny::Konstanta(k) => konstanty[k].bity(),
                Jednotny::Dlzka(b) => buffery[b].data.len() as u32,
            };
            reg[*r as usize].fill(bity);
        }
        Pracovisko { reg, masky: vec![vec![0u32; PASY]; self.hlbka + 1], kroky: 0 }
    }

    /// Vykoná blok vlákien od lineárneho indexu `od` (najviac PASY vlákien, `pocet` platných).
    fn blok(
        &self,
        ws: &mut Pracovisko,
        buffery: &[Buffer<'_>],
        mriezka: &Mriezka,
        od: u64,
        pocet: usize,
    ) -> Vysledok<()> {
        for (l, m) in ws.masky[0].iter_mut().enumerate() {
            *m = (l < pocet) as u32;
        }
        for r in &self.premenne {
            ws.reg[*r as usize].fill(0);
        }
        if self.id.iter().any(Option::is_some) {
            let mut c = mriezka.rozloz(od);
            for l in 0..pocet {
                let id = mriezka.id(&c);
                for (os, r) in self.id.iter().enumerate() {
                    if let Some(r) = r {
                        ws.reg[*r as usize][l] = id[os];
                    }
                }
                mriezka.dalsi(&mut c);
            }
        }
        ws.kroky = 0;
        self.kod_bloku(&self.kod, 0, ws, buffery)
    }

    fn kod_bloku(&self, kod: &[Ins], h: usize, ws: &mut Pracovisko, buffery: &[Buffer<'_>]) -> Vysledok<()> {
        for ins in kod {
            ws.kroky += 1;
            if ws.kroky > NAJVIAC_KROKOV {
                return Err(Chyba::Limit(format!(
                    "kernel {}: blok prekročil počet krokov",
                    self.kernel.meno
                )));
            }
            match ins {
                Ins::Citaj { r, vazba, index } => {
                    let mut o = std::mem::take(&mut ws.reg[*r as usize]);
                    let (oo, idx, m) = (pas_mut(&mut o), pas(&ws.reg[*index as usize]), pas(&ws.masky[h]));
                    let data = buffery[*vazba].data;
                    for l in 0..PASY {
                        oo[l] = if m[l] != 0 {
                            let i = idx[l] as usize;
                            match data.get(i) {
                                Some(x) => x.load(Ordering::Relaxed),
                                None => {
                                    return Err(chyba(format!(
                                        "čítanie bufferu {vazba} mimo rozsahu (index {i}, dĺžka {})",
                                        data.len()
                                    )));
                                }
                            }
                        } else {
                            0
                        };
                    }
                    ws.reg[*r as usize] = o;
                }
                Ins::Bin { r, op, typ, a, b } => {
                    let mut o = std::mem::take(&mut ws.reg[*r as usize]);
                    let res = binarna(
                        *op,
                        *typ,
                        pas_mut(&mut o),
                        pas(&ws.reg[*a as usize]),
                        pas(&ws.reg[*b as usize]),
                        pas(&ws.masky[h]),
                    );
                    ws.reg[*r as usize] = o;
                    res?;
                }
                Ins::Un { r, op, typ, a } => {
                    let mut o = std::mem::take(&mut ws.reg[*r as usize]);
                    let res = unarna(*op, *typ, pas_mut(&mut o), pas(&ws.reg[*a as usize]));
                    ws.reg[*r as usize] = o;
                    res?;
                }
                Ins::Prevod { r, z, na, a } => {
                    let mut o = std::mem::take(&mut ws.reg[*r as usize]);
                    let res = prevod(*z, *na, pas_mut(&mut o), pas(&ws.reg[*a as usize]));
                    ws.reg[*r as usize] = o;
                    res?;
                }
                Ins::Vyber { r, p, a, b } => {
                    let mut o = std::mem::take(&mut ws.reg[*r as usize]);
                    let oo = pas_mut(&mut o);
                    let (pp, aa, bb) =
                        (pas(&ws.reg[*p as usize]), pas(&ws.reg[*a as usize]), pas(&ws.reg[*b as usize]));
                    for l in 0..PASY {
                        oo[l] = if pp[l] != 0 { aa[l] } else { bb[l] };
                    }
                    ws.reg[*r as usize] = o;
                }
                Ins::Kopia { r, z } => {
                    let mut o = std::mem::take(&mut ws.reg[*r as usize]);
                    o.copy_from_slice(&ws.reg[*z as usize]);
                    ws.reg[*r as usize] = o;
                }
                Ins::Nastav { premenna, z } => {
                    let mut o = std::mem::take(&mut ws.reg[*premenna as usize]);
                    let (oo, zz, m) = (pas_mut(&mut o), pas(&ws.reg[*z as usize]), pas(&ws.masky[h]));
                    for l in 0..PASY {
                        oo[l] = if m[l] != 0 { zz[l] } else { oo[l] };
                    }
                    ws.reg[*premenna as usize] = o;
                }
                Ins::Zapis { vazba, index, hodnota } => {
                    let (idx, hh, m) =
                        (pas(&ws.reg[*index as usize]), pas(&ws.reg[*hodnota as usize]), pas(&ws.masky[h]));
                    let data = buffery[*vazba].data;
                    for l in 0..PASY {
                        if m[l] != 0 {
                            let i = idx[l] as usize;
                            match data.get(i) {
                                Some(x) => x.store(hh[l], Ordering::Relaxed),
                                None => {
                                    return Err(chyba(format!(
                                        "zápis do bufferu {vazba} mimo rozsahu (index {i}, dĺžka {})",
                                        data.len()
                                    )));
                                }
                            }
                        }
                    }
                }
                Ins::Ak { podmienka, potom, inak } => {
                    if !potom.is_empty() && self.nastav_masku(ws, h, *podmienka, 1) {
                        self.kod_bloku(potom, h + 1, ws, buffery)?;
                    }
                    if !inak.is_empty() && self.nastav_masku(ws, h, *podmienka, 0) {
                        self.kod_bloku(inak, h + 1, ws, buffery)?;
                    }
                }
                Ins::Pre { premenna, od, po, telo } => {
                    {
                        let (nizsie, vyssie) = ws.masky.split_at_mut(h + 1);
                        vyssie[0].copy_from_slice(&nizsie[h]);
                    }
                    let mut v = std::mem::take(&mut ws.reg[*premenna as usize]);
                    {
                        let (vv, oo, m) = (pas_mut(&mut v), pas(&ws.reg[*od as usize]), pas(&ws.masky[h]));
                        for l in 0..PASY {
                            vv[l] = if m[l] != 0 { oo[l] } else { vv[l] };
                        }
                    }
                    ws.reg[*premenna as usize] = v;
                    loop {
                        ws.kroky += 1;
                        if ws.kroky > NAJVIAC_KROKOV {
                            return Err(Chyba::Limit(format!(
                                "kernel {}: blok prekročil počet krokov",
                                self.kernel.meno
                            )));
                        }
                        let mut aktivne = 0u32;
                        {
                            let (vv, kk) = (pas(&ws.reg[*premenna as usize]), pas(&ws.reg[*po as usize]));
                            let m = pas_mut(&mut ws.masky[h + 1]);
                            for l in 0..PASY {
                                m[l] &= (vv[l] < kk[l]) as u32;
                                aktivne |= m[l];
                            }
                        }
                        if aktivne == 0 {
                            break;
                        }
                        self.kod_bloku(telo, h + 1, ws, buffery)?;
                        let mut v = std::mem::take(&mut ws.reg[*premenna as usize]);
                        {
                            let (vv, m) = (pas_mut(&mut v), pas(&ws.masky[h + 1]));
                            for l in 0..PASY {
                                vv[l] = vv[l].wrapping_add(m[l]);
                            }
                        }
                        ws.reg[*premenna as usize] = v;
                    }
                }
            }
        }
        Ok(())
    }

    /// Maska vetvy: aktívne pruhy úrovne `h`, v ktorých má podmienka hodnotu `hodnota`. Vráti, či je
    /// niektorý pruh aktívny.
    fn nastav_masku(&self, ws: &mut Pracovisko, h: usize, podmienka: Reg, hodnota: u32) -> bool {
        let (nizsie, vyssie) = ws.masky.split_at_mut(h + 1);
        let (m, nova, c) = (pas(&nizsie[h]), pas_mut(&mut vyssie[0]), pas(&ws.reg[podmienka as usize]));
        let mut aktivne = 0u32;
        for l in 0..PASY {
            nova[l] = m[l] & (c[l] ^ hodnota ^ 1);
            aktivne |= nova[l];
        }
        aktivne != 0
    }

    /// Vykoná program pre všetky vlákna všetkých skupín.
    pub fn vykonaj(
        &self,
        vazby: &mut [DataVazby<'_>],
        konstanty: &[Hodnota32],
        skupiny: [u32; 3],
    ) -> Vysledok<()> {
        over_argumenty(&self.kernel, vazby, konstanty)?;
        let mriezka = Mriezka::nova(self.kernel.skupina, skupiny);
        let vlakien = mriezka.vlakien();
        if vlakien == 0 {
            return Ok(());
        }
        let mut buffery = Vec::with_capacity(vazby.len());
        let mut zarovnane = true;
        for v in vazby.iter_mut() {
            let pohlad = match v {
                DataVazby::F32(d) => atomicky_pohlad(d),
                DataVazby::U32(d) => atomicky_pohlad(d),
                DataVazby::I32(d) => atomicky_pohlad(d),
            };
            match pohlad {
                Some(data) => buffery.push(Buffer { data }),
                None => {
                    zarovnane = false;
                    break;
                }
            }
        }
        if !zarovnane {
            drop(buffery);
            return crate::interpret::vykonaj(&self.kernel, vazby, konstanty, skupiny);
        }
        let blokov = usize::try_from(vlakien.div_ceil(PASY as u64))
            .map_err(|_| Chyba::Limit(format!("kernel {}: príliš veľa vlákien", self.kernel.meno)))?;
        let vlakna = paralel::pocet_vlakien().max(1);
        let kus = blokov.div_ceil(vlakna * 4).max(NAJMENEJ_BLOKOV_NA_VLAKNO);
        let buffery = &buffery;
        let mriezka = &mriezka;
        let vysledky = paralel::mapuj_rozsah(blokov, kus, |rozsah| {
            let mut ws = self.nove_pracovisko(buffery, konstanty);
            for b in rozsah {
                let od = b as u64 * PASY as u64;
                let pocet = (vlakien - od).min(PASY as u64) as usize;
                self.blok(&mut ws, buffery, mriezka, od, pocet)?;
            }
            Ok(())
        });
        // chyba z bloku s najnižším poradím – výsledok nezávisí od počtu vlákien
        vysledky.into_iter().collect::<Vysledok<Vec<()>>>()?;
        Ok(())
    }
}

/// Rozloženie vlákien do skupín v rovnakom poradí ako v interprete (z, y, x skupín; potom z, y, x v skupine).
#[derive(Debug, Clone, Copy)]
struct Mriezka {
    skupina: [u64; 3],
    skupiny: [u64; 3],
}

/// Poloha vlákna: [lx, ly, lz, gx, gy, gz].
type Poloha = [u64; 6];

impl Mriezka {
    fn nova(skupina: [u32; 3], skupiny: [u32; 3]) -> Mriezka {
        Mriezka { skupina: skupina.map(u64::from), skupiny: skupiny.map(u64::from) }
    }

    fn vlakien(&self) -> u64 {
        self.skupina.iter().chain(&self.skupiny).fold(1u64, |a, b| a.saturating_mul(*b))
    }

    fn rozmery(&self) -> [u64; 6] {
        let [sx, sy, sz] = self.skupina;
        let [gx, gy, gz] = self.skupiny;
        [sx, sy, sz, gx, gy, gz]
    }

    fn rozloz(&self, mut t: u64) -> Poloha {
        let mut c = [0u64; 6];
        for (ci, r) in c.iter_mut().zip(self.rozmery()) {
            *ci = t % r;
            t /= r;
        }
        c
    }

    fn dalsi(&self, c: &mut Poloha) {
        for (ci, r) in c.iter_mut().zip(self.rozmery()) {
            *ci += 1;
            if *ci < r {
                return;
            }
            *ci = 0;
        }
    }

    fn id(&self, c: &Poloha) -> [u32; 3] {
        [
            (c[3] * self.skupina[0] + c[0]) as u32,
            (c[4] * self.skupina[1] + c[1]) as u32,
            (c[5] * self.skupina[2] + c[2]) as u32,
        ]
    }
}

/// Preloží a vykoná kernel (pohodlná skratka; pri opakovanom spúšťaní je lepšie preložiť raz).
pub fn vykonaj(
    kernel: &Kernel,
    vazby: &mut [DataVazby<'_>],
    konstanty: &[Hodnota32],
    skupiny: [u32; 3],
) -> Vysledok<()> {
    preloz(kernel)?.vykonaj(vazby, konstanty, skupiny)
}
