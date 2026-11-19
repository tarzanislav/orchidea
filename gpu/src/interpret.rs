//! Referenčný interpret kernelov na procesore – „CPU cesta vždy“ (R4) a rozhodca správnosti pre GPU.
//!
//! Interpret je zámerne prísny: čítanie alebo zápis mimo bufferu a celočíselné delenie nulou sú chyby
//! (na GPU sú nedefinované), takže chybu v kerneli odhalí skôr, než sa prejaví na niektorom zariadení.
//! Logické operácie a výber vyhodnocujú obe strany – rovnako ako GPU.

use crate::ir::{Kernel, Op1, Op2, Prikaz, Typ, Vyraz};
use crate::zariadenie::{DataVazby, Hodnota32, over_argumenty};
use orchidea_jadro::matematika::f32v;
use orchidea_jadro::{Chyba, Vysledok};

/// Najviac krokov jedného vlákna (ochrana pred nekonečným cyklom).
const NAJVIAC_KROKOV: u64 = 1 << 32;

#[derive(Debug, Clone, Copy, PartialEq)]
enum H {
    F(f32),
    U(u32),
    I(i32),
    B(bool),
}

impl H {
    fn nula(t: Typ) -> H {
        match t {
            Typ::F32 => H::F(0.0),
            Typ::U32 => H::U(0),
            Typ::I32 => H::I(0),
            Typ::Bool => H::B(false),
        }
    }
}

struct Stav<'a, 'b> {
    kernel: &'a Kernel,
    vazby: &'a mut [DataVazby<'b>],
    konstanty: &'a [Hodnota32],
    premenne: Vec<H>,
    id: [u32; 3],
    kroky: u64,
}

fn chyba(s: impl Into<String>) -> Chyba {
    Chyba::Vypocet(s.into())
}

impl Stav<'_, '_> {
    fn krok(&mut self) -> Vysledok<()> {
        self.kroky += 1;
        if self.kroky > NAJVIAC_KROKOV {
            return Err(Chyba::Limit(format!("kernel {}: vlákno prekročilo počet krokov", self.kernel.meno)));
        }
        Ok(())
    }

    fn u(&mut self, v: &Vyraz) -> Vysledok<u32> {
        match self.vyhodnot(v)? {
            H::U(x) => Ok(x),
            _ => Err(chyba("čakal sa u32")),
        }
    }

    fn vyhodnot(&mut self, v: &Vyraz) -> Vysledok<H> {
        self.krok()?;
        Ok(match v {
            Vyraz::F32(x) => H::F(*x),
            Vyraz::U32(x) => H::U(*x),
            Vyraz::I32(x) => H::I(*x),
            Vyraz::Bool(x) => H::B(*x),
            Vyraz::Premenna(p) => self.premenne[p.0],
            Vyraz::Konstanta(k) => match self.konstanty[k.0] {
                Hodnota32::F32(x) => H::F(x),
                Hodnota32::U32(x) => H::U(x),
                Hodnota32::I32(x) => H::I(x),
            },
            Vyraz::IdVlakna(os) => H::U(self.id[*os as usize]),
            Vyraz::Citaj { vazba, index } => {
                let i = self.u(index)? as usize;
                let b = &self.vazby[vazba.0];
                let mimo = || {
                    chyba(format!(
                        "čítanie bufferu {} mimo rozsahu (index {i}, dĺžka {})",
                        vazba.0,
                        b.dlzka()
                    ))
                };
                match b {
                    DataVazby::F32(d) => H::F(*d.get(i).ok_or_else(mimo)?),
                    DataVazby::U32(d) => H::U(*d.get(i).ok_or_else(mimo)?),
                    DataVazby::I32(d) => H::I(*d.get(i).ok_or_else(mimo)?),
                }
            }
            Vyraz::Dlzka(vazba) => H::U(self.vazby[vazba.0].dlzka() as u32),
            Vyraz::Bin(op, a, b) => {
                let (x, y) = (self.vyhodnot(a)?, self.vyhodnot(b)?);
                bin(*op, x, y)?
            }
            Vyraz::Un(op, a) => {
                let x = self.vyhodnot(a)?;
                match (op, x) {
                    (Op1::Zaporne, H::F(a)) => H::F(-a),
                    (Op1::Zaporne, H::I(a)) => H::I(a.wrapping_neg()),
                    (Op1::Nie, H::B(a)) => H::B(!a),
                    (Op1::Abs, H::F(a)) => H::F(a.abs()),
                    (Op1::Abs, H::I(a)) => H::I(a.wrapping_abs()),
                    (Op1::Odmocnina, H::F(a)) => H::F(a.sqrt()),
                    (Op1::Exp, H::F(a)) => H::F(f32v::exp(a)),
                    (Op1::Ln, H::F(a)) => H::F(f32v::ln(a)),
                    (Op1::Dole, H::F(a)) => H::F(a.floor()),
                    _ => return Err(chyba("neplatná unárna operácia")),
                }
            }
            Vyraz::Prevod(t, a) => {
                let x = self.vyhodnot(a)?;
                match (t, x) {
                    (Typ::F32, H::F(a)) => H::F(a),
                    (Typ::F32, H::U(a)) => H::F(a as f32),
                    (Typ::F32, H::I(a)) => H::F(a as f32),
                    (Typ::U32, H::F(a)) => H::U(a as u32),
                    (Typ::U32, H::U(a)) => H::U(a),
                    (Typ::U32, H::I(a)) => H::U(a as u32),
                    (Typ::I32, H::F(a)) => H::I(a as i32),
                    (Typ::I32, H::U(a)) => H::I(a as i32),
                    (Typ::I32, H::I(a)) => H::I(a),
                    _ => return Err(chyba("neplatný prevod")),
                }
            }
            Vyraz::Vyber(p, a, b) => {
                let (pv, av, bv) = (self.vyhodnot(p)?, self.vyhodnot(a)?, self.vyhodnot(b)?);
                match pv {
                    H::B(true) => av,
                    H::B(false) => bv,
                    _ => return Err(chyba("podmienka výberu nie je áno/nie")),
                }
            }
        })
    }

    fn vykonaj(&mut self, prikazy: &[Prikaz]) -> Vysledok<()> {
        for p in prikazy {
            self.krok()?;
            match p {
                Prikaz::Nastav(pr, v) => {
                    let h = self.vyhodnot(v)?;
                    self.premenne[pr.0] = h;
                }
                Prikaz::Zapis { vazba, index, hodnota } => {
                    let i = self.u(index)? as usize;
                    let h = self.vyhodnot(hodnota)?;
                    let b = &mut self.vazby[vazba.0];
                    let n = b.dlzka();
                    let mimo =
                        || chyba(format!("zápis do bufferu {} mimo rozsahu (index {i}, dĺžka {n})", vazba.0));
                    match (b, h) {
                        (DataVazby::F32(d), H::F(x)) => *d.get_mut(i).ok_or_else(mimo)? = x,
                        (DataVazby::U32(d), H::U(x)) => *d.get_mut(i).ok_or_else(mimo)? = x,
                        (DataVazby::I32(d), H::I(x)) => *d.get_mut(i).ok_or_else(mimo)? = x,
                        _ => return Err(chyba("zápis nesprávneho typu")),
                    }
                }
                Prikaz::Ak { podmienka, potom, inak } => match self.vyhodnot(podmienka)? {
                    H::B(true) => self.vykonaj(potom)?,
                    H::B(false) => self.vykonaj(inak)?,
                    _ => return Err(chyba("podmienka nie je áno/nie")),
                },
                Prikaz::Pre { premenna, od, po, telo } => {
                    let start = self.u(od)?;
                    let koniec = self.u(po)?;
                    self.premenne[premenna.0] = H::U(start);
                    loop {
                        let H::U(c) = self.premenne[premenna.0] else {
                            return Err(chyba("počítadlo nie je u32"));
                        };
                        if c >= koniec {
                            break;
                        }
                        self.vykonaj(telo)?;
                        let H::U(c) = self.premenne[premenna.0] else {
                            return Err(chyba("počítadlo nie je u32"));
                        };
                        self.premenne[premenna.0] = H::U(c.wrapping_add(1));
                    }
                }
            }
        }
        Ok(())
    }
}

fn bin(op: Op2, x: H, y: H) -> Vysledok<H> {
    use H::*;
    Ok(match (op, x, y) {
        (Op2::Plus, F(a), F(b)) => F(a + b),
        (Op2::Plus, U(a), U(b)) => U(a.wrapping_add(b)),
        (Op2::Plus, I(a), I(b)) => I(a.wrapping_add(b)),
        (Op2::Minus, F(a), F(b)) => F(a - b),
        (Op2::Minus, U(a), U(b)) => U(a.wrapping_sub(b)),
        (Op2::Minus, I(a), I(b)) => I(a.wrapping_sub(b)),
        (Op2::Krat, F(a), F(b)) => F(a * b),
        (Op2::Krat, U(a), U(b)) => U(a.wrapping_mul(b)),
        (Op2::Krat, I(a), I(b)) => I(a.wrapping_mul(b)),
        (Op2::Deleno, F(a), F(b)) => F(a / b),
        (Op2::Deleno | Op2::Zvysok, U(_), U(0)) | (Op2::Deleno | Op2::Zvysok, I(_), I(0)) => {
            return Err(chyba("celočíselné delenie nulou (na GPU je výsledok nedefinovaný)"));
        }
        (Op2::Deleno, U(a), U(b)) => U(a / b),
        (Op2::Deleno, I(a), I(b)) => I(a.wrapping_div(b)),
        (Op2::Zvysok, F(a), F(b)) => F(a % b),
        (Op2::Zvysok, U(a), U(b)) => U(a % b),
        (Op2::Zvysok, I(a), I(b)) => I(a.wrapping_rem(b)),
        (Op2::Min, F(a), F(b)) => F(a.min(b)),
        (Op2::Min, U(a), U(b)) => U(a.min(b)),
        (Op2::Min, I(a), I(b)) => I(a.min(b)),
        (Op2::Max, F(a), F(b)) => F(a.max(b)),
        (Op2::Max, U(a), U(b)) => U(a.max(b)),
        (Op2::Max, I(a), I(b)) => I(a.max(b)),
        (Op2::A, B(a), B(b)) => B(a && b),
        (Op2::Alebo, B(a), B(b)) => B(a || b),
        (op, a, b) if op.je_porovnanie() => {
            let o = match (a, b) {
                (F(a), F(b)) => a.partial_cmp(&b),
                (U(a), U(b)) => Some(a.cmp(&b)),
                (I(a), I(b)) => Some(a.cmp(&b)),
                (B(a), B(b)) => Some(a.cmp(&b)),
                _ => return Err(chyba("porovnanie rôznych typov")),
            };
            use std::cmp::Ordering::*;
            B(match op {
                Op2::Mensie => o == Some(Less),
                Op2::MensieRovne => matches!(o, Some(Less | Equal)),
                Op2::Vacsie => o == Some(Greater),
                Op2::VacsieRovne => matches!(o, Some(Greater | Equal)),
                Op2::Rovne => o == Some(Equal),
                _ => o != Some(Equal),
            })
        }
        _ => return Err(chyba(format!("neplatná operácia {op:?}"))),
    })
}

/// Vykoná kernel pre všetky vlákna všetkých skupín.
pub fn vykonaj(
    kernel: &Kernel,
    vazby: &mut [DataVazby<'_>],
    konstanty: &[Hodnota32],
    skupiny: [u32; 3],
) -> Vysledok<()> {
    over_argumenty(kernel, vazby, konstanty)?;
    let [sx, sy, sz] = kernel.skupina;
    let mut stav = Stav {
        kernel,
        vazby,
        konstanty,
        premenne: kernel.premenne.iter().map(|t| H::nula(*t)).collect(),
        id: [0; 3],
        kroky: 0,
    };
    for gz in 0..skupiny[2] {
        for gy in 0..skupiny[1] {
            for gx in 0..skupiny[0] {
                for lz in 0..sz {
                    for ly in 0..sy {
                        for lx in 0..sx {
                            stav.id = [gx * sx + lx, gy * sy + ly, gz * sz + lz];
                            for (h, t) in stav.premenne.iter_mut().zip(&kernel.premenne) {
                                *h = H::nula(*t);
                            }
                            stav.kroky = 0;
                            stav.vykonaj(&kernel.telo)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
