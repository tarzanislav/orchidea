//! Jazyk kernelov ORCHIDEE – malý typovaný jazyk pre výpočtové programy na GPU.
//!
//! Kernel sa napíše raz a vlastné prekladače z neho vyrobia SPIR-V (Vulkan), HLSL (Direct3D 12), MSL (Metal)
//! aj WGSL (WebGPU). Ten istý kernel vie vykonať aj referenčný interpret na procesore, takže výsledok GPU sa dá
//! vždy porovnať s presnou referenciou. Jazyk je zámerne malý: skaláre, buffery, konštanty, premenné,
//! podmienky a cykly s počítadlom – nič, čo by sa na niektorej platforme správalo inak.

use orchidea_jadro::{Chyba, Odtlacok, Vysledok};
use std::fmt;

/// Typ hodnoty v kerneli.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Typ {
    F32,
    U32,
    I32,
    Bool,
}

impl Typ {
    pub fn je_cislo(self) -> bool {
        self != Typ::Bool
    }
}

/// Prístup kernelu k bufferu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pristup {
    Citanie,
    Zapis,
    CitanieZapis,
}

impl Pristup {
    pub fn cita(self) -> bool {
        self != Pristup::Zapis
    }
    pub fn zapisuje(self) -> bool {
        self != Pristup::Citanie
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VazbaId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KonstId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PremId(pub usize);

/// Binárna operácia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op2 {
    Plus,
    Minus,
    Krat,
    Deleno,
    Zvysok,
    Min,
    Max,
    Mensie,
    MensieRovne,
    Vacsie,
    VacsieRovne,
    Rovne,
    Rozne,
    A,
    Alebo,
}

impl Op2 {
    pub fn je_porovnanie(self) -> bool {
        matches!(
            self,
            Op2::Mensie | Op2::MensieRovne | Op2::Vacsie | Op2::VacsieRovne | Op2::Rovne | Op2::Rozne
        )
    }
    pub fn je_logicka(self) -> bool {
        matches!(self, Op2::A | Op2::Alebo)
    }
}

/// Unárna operácia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op1 {
    Zaporne,
    Nie,
    Abs,
    Odmocnina,
    Exp,
    Ln,
    Dole,
}

/// Výraz.
#[derive(Debug, Clone, PartialEq)]
pub enum Vyraz {
    F32(f32),
    U32(u32),
    I32(i32),
    Bool(bool),
    Premenna(PremId),
    Konstanta(KonstId),
    /// Globálny index vlákna v osi 0, 1 alebo 2.
    IdVlakna(u8),
    Citaj {
        vazba: VazbaId,
        index: Box<Vyraz>,
    },
    /// Počet prvkov bufferu (u32).
    Dlzka(VazbaId),
    Bin(Op2, Box<Vyraz>, Box<Vyraz>),
    Un(Op1, Box<Vyraz>),
    /// Prevod medzi f32, u32 a i32 (medzi u32 a i32 sa bity zachovajú).
    Prevod(Typ, Box<Vyraz>),
    /// podmienka ? a : b
    Vyber(Box<Vyraz>, Box<Vyraz>, Box<Vyraz>),
}

/// Príkaz.
#[derive(Debug, Clone, PartialEq)]
pub enum Prikaz {
    Nastav(PremId, Vyraz),
    Zapis {
        vazba: VazbaId,
        index: Vyraz,
        hodnota: Vyraz,
    },
    Ak {
        podmienka: Vyraz,
        potom: Vec<Prikaz>,
        inak: Vec<Prikaz>,
    },
    /// for (premenná = od; premenná < po; premenná += 1) – premenná je u32, `po` sa vyhodnotí raz.
    Pre {
        premenna: PremId,
        od: Vyraz,
        po: Vyraz,
        telo: Vec<Prikaz>,
    },
}

/// Buffer kernelu.
#[derive(Debug, Clone, PartialEq)]
pub struct Vazba {
    pub meno: String,
    pub typ: Typ,
    pub pristup: Pristup,
}

/// Konštanta kernelu (rovnaká pre všetky vlákna, nastaví sa pri spustení).
#[derive(Debug, Clone, PartialEq)]
pub struct Konstanta {
    pub meno: String,
    pub typ: Typ,
}

/// Kernel.
#[derive(Debug, Clone, PartialEq)]
pub struct Kernel {
    pub meno: String,
    pub vazby: Vec<Vazba>,
    pub konstanty: Vec<Konstanta>,
    pub premenne: Vec<Typ>,
    /// Počet vlákien v jednej skupine v osiach x, y, z.
    pub skupina: [u32; 3],
    pub telo: Vec<Prikaz>,
}

/// Najviac vlákien v jednej skupine (bezpečne podporované všetkými rozhraniami).
pub const NAJVIAC_VLAKIEN_V_SKUPINE: u32 = 256;
/// Najviac bufferov a konštánt.
pub const NAJVIAC_VAZIEB: usize = 8;
pub const NAJVIAC_KONSTANT: usize = 16;

impl Kernel {
    pub fn novy(meno: &str, skupina: [u32; 3]) -> Kernel {
        Kernel {
            meno: meno.to_string(),
            vazby: vec![],
            konstanty: vec![],
            premenne: vec![],
            skupina,
            telo: vec![],
        }
    }

    pub fn vazba(&mut self, meno: &str, typ: Typ, pristup: Pristup) -> VazbaId {
        self.vazby.push(Vazba { meno: meno.to_string(), typ, pristup });
        VazbaId(self.vazby.len() - 1)
    }

    pub fn konstanta(&mut self, meno: &str, typ: Typ) -> KonstId {
        self.konstanty.push(Konstanta { meno: meno.to_string(), typ });
        KonstId(self.konstanty.len() - 1)
    }

    pub fn premenna(&mut self, typ: Typ) -> PremId {
        self.premenne.push(typ);
        PremId(self.premenne.len() - 1)
    }

    /// Odtlačok kernelu (pre pamäť skompilovaných programov).
    pub fn odtlacok(&self) -> Odtlacok {
        Odtlacok::z(format!("{self:?}").as_bytes())
    }

    /// Typ výrazu (a kontrola typov v ňom).
    pub fn typ_vyrazu(&self, v: &Vyraz) -> Vysledok<Typ> {
        let chyba =
            |s: String| Chyba::NezhodaTypov { popis: format!("kernel {}: {s}", self.meno), navrh: None };
        Ok(match v {
            Vyraz::F32(x) => {
                if !x.is_finite() {
                    return Err(chyba("konštanta f32 musí byť konečné číslo".into()));
                }
                Typ::F32
            }
            Vyraz::U32(_) => Typ::U32,
            Vyraz::I32(_) => Typ::I32,
            Vyraz::Bool(_) => Typ::Bool,
            Vyraz::Premenna(p) => {
                *self.premenne.get(p.0).ok_or_else(|| chyba(format!("premenná {} neexistuje", p.0)))?
            }
            Vyraz::Konstanta(k) => {
                self.konstanty.get(k.0).ok_or_else(|| chyba(format!("konštanta {} neexistuje", k.0)))?.typ
            }
            Vyraz::IdVlakna(os) => {
                if *os > 2 {
                    return Err(chyba("os vlákna musí byť 0, 1 alebo 2".into()));
                }
                Typ::U32
            }
            Vyraz::Citaj { vazba, index } => {
                let b =
                    self.vazby.get(vazba.0).ok_or_else(|| chyba(format!("buffer {} neexistuje", vazba.0)))?;
                if !b.pristup.cita() {
                    return Err(chyba(format!("buffer {} je len na zápis", b.meno)));
                }
                if self.typ_vyrazu(index)? != Typ::U32 {
                    return Err(chyba("index bufferu musí byť u32".into()));
                }
                b.typ
            }
            Vyraz::Dlzka(vazba) => {
                self.vazby.get(vazba.0).ok_or_else(|| chyba(format!("buffer {} neexistuje", vazba.0)))?;
                Typ::U32
            }
            Vyraz::Bin(op, a, b) => {
                let (ta, tb) = (self.typ_vyrazu(a)?, self.typ_vyrazu(b)?);
                if ta != tb {
                    return Err(chyba(format!("{op:?} spája rôzne typy {ta:?} a {tb:?}")));
                }
                if op.je_logicka() {
                    if ta != Typ::Bool {
                        return Err(chyba(format!("{op:?} potrebuje áno/nie")));
                    }
                    Typ::Bool
                } else if op.je_porovnanie() {
                    if ta == Typ::Bool && !matches!(op, Op2::Rovne | Op2::Rozne) {
                        return Err(chyba("áno/nie sa dá len porovnať na rovnosť".into()));
                    }
                    Typ::Bool
                } else {
                    if ta == Typ::Bool {
                        return Err(chyba(format!("{op:?} nefunguje na áno/nie")));
                    }
                    ta
                }
            }
            Vyraz::Un(op, a) => {
                let t = self.typ_vyrazu(a)?;
                match op {
                    Op1::Nie if t == Typ::Bool => Typ::Bool,
                    Op1::Nie => return Err(chyba("negácia potrebuje áno/nie".into())),
                    Op1::Zaporne if t == Typ::F32 || t == Typ::I32 => t,
                    Op1::Abs if t == Typ::F32 || t == Typ::I32 => t,
                    Op1::Odmocnina | Op1::Exp | Op1::Ln | Op1::Dole if t == Typ::F32 => t,
                    _ => return Err(chyba(format!("{op:?} nefunguje na type {t:?}"))),
                }
            }
            Vyraz::Prevod(ciel, a) => {
                let t = self.typ_vyrazu(a)?;
                if !t.je_cislo() || !ciel.je_cislo() {
                    return Err(chyba("prevod je len medzi číselnými typmi".into()));
                }
                *ciel
            }
            Vyraz::Vyber(p, a, b) => {
                if self.typ_vyrazu(p)? != Typ::Bool {
                    return Err(chyba("podmienka výberu musí byť áno/nie".into()));
                }
                let (ta, tb) = (self.typ_vyrazu(a)?, self.typ_vyrazu(b)?);
                if ta != tb {
                    return Err(chyba("obe vetvy výberu musia mať rovnaký typ".into()));
                }
                ta
            }
        })
    }

    fn over_prikazy(&self, p: &[Prikaz], hlbka: usize) -> Vysledok<()> {
        let chyba =
            |s: String| Chyba::NezhodaTypov { popis: format!("kernel {}: {s}", self.meno), navrh: None };
        if hlbka > 32 {
            return Err(Chyba::Limit("kernel je príliš hlboko vnorený".into()));
        }
        for prikaz in p {
            match prikaz {
                Prikaz::Nastav(pr, v) => {
                    let t = *self
                        .premenne
                        .get(pr.0)
                        .ok_or_else(|| chyba(format!("premenná {} neexistuje", pr.0)))?;
                    if self.typ_vyrazu(v)? != t {
                        return Err(chyba(format!("do premennej typu {t:?} sa priraďuje iný typ")));
                    }
                }
                Prikaz::Zapis { vazba, index, hodnota } => {
                    let b = self
                        .vazby
                        .get(vazba.0)
                        .ok_or_else(|| chyba(format!("buffer {} neexistuje", vazba.0)))?;
                    if !b.pristup.zapisuje() {
                        return Err(chyba(format!("buffer {} je len na čítanie", b.meno)));
                    }
                    if self.typ_vyrazu(index)? != Typ::U32 {
                        return Err(chyba("index bufferu musí byť u32".into()));
                    }
                    if self.typ_vyrazu(hodnota)? != b.typ {
                        return Err(chyba(format!("do bufferu {} sa zapisuje iný typ", b.meno)));
                    }
                }
                Prikaz::Ak { podmienka, potom, inak } => {
                    if self.typ_vyrazu(podmienka)? != Typ::Bool {
                        return Err(chyba("podmienka musí byť áno/nie".into()));
                    }
                    self.over_prikazy(potom, hlbka + 1)?;
                    self.over_prikazy(inak, hlbka + 1)?;
                }
                Prikaz::Pre { premenna, od, po, telo } => {
                    if self.premenne.get(premenna.0) != Some(&Typ::U32) {
                        return Err(chyba("počítadlo cyklu musí byť premenná u32".into()));
                    }
                    if self.typ_vyrazu(od)? != Typ::U32 || self.typ_vyrazu(po)? != Typ::U32 {
                        return Err(chyba("hranice cyklu musia byť u32".into()));
                    }
                    self.over_prikazy(telo, hlbka + 1)?;
                }
            }
        }
        Ok(())
    }

    /// Overí celý kernel (typy, limity, mená).
    pub fn over(&self) -> Vysledok<()> {
        let [x, y, z] = self.skupina;
        if x == 0 || y == 0 || z == 0 || x.saturating_mul(y).saturating_mul(z) > NAJVIAC_VLAKIEN_V_SKUPINE {
            return Err(Chyba::MimoRozsahu(format!(
                "kernel {}: skupina {x}×{y}×{z} musí mať 1 až {NAJVIAC_VLAKIEN_V_SKUPINE} vlákien",
                self.meno
            )));
        }
        if self.vazby.is_empty()
            || self.vazby.len() > NAJVIAC_VAZIEB
            || self.konstanty.len() > NAJVIAC_KONSTANT
        {
            return Err(Chyba::Limit(format!(
                "kernel {}: 1 až {NAJVIAC_VAZIEB} bufferov a najviac {NAJVIAC_KONSTANT} konštánt",
                self.meno
            )));
        }
        if self.vazby.iter().any(|v| v.typ == Typ::Bool) || self.konstanty.iter().any(|k| k.typ == Typ::Bool)
        {
            return Err(Chyba::NezhodaTypov {
                popis: "buffery a konštanty musia byť číselné".into(),
                navrh: None,
            });
        }
        self.over_prikazy(&self.telo, 0)
    }
}

// ---------- pohodlné písanie kernelov ----------

pub fn f(x: f32) -> Vyraz {
    Vyraz::F32(x)
}
pub fn u(x: u32) -> Vyraz {
    Vyraz::U32(x)
}
pub fn i(x: i32) -> Vyraz {
    Vyraz::I32(x)
}
pub fn id(os: u8) -> Vyraz {
    Vyraz::IdVlakna(os)
}
pub fn p(x: PremId) -> Vyraz {
    Vyraz::Premenna(x)
}
pub fn k(x: KonstId) -> Vyraz {
    Vyraz::Konstanta(x)
}
pub fn citaj(vazba: VazbaId, index: Vyraz) -> Vyraz {
    Vyraz::Citaj { vazba, index: Box::new(index) }
}
pub fn dlzka(vazba: VazbaId) -> Vyraz {
    Vyraz::Dlzka(vazba)
}
pub fn vyber(podmienka: Vyraz, a: Vyraz, b: Vyraz) -> Vyraz {
    Vyraz::Vyber(Box::new(podmienka), Box::new(a), Box::new(b))
}
pub fn nastav(x: PremId, v: Vyraz) -> Prikaz {
    Prikaz::Nastav(x, v)
}
pub fn zapis(vazba: VazbaId, index: Vyraz, hodnota: Vyraz) -> Prikaz {
    Prikaz::Zapis { vazba, index, hodnota }
}
pub fn ak(podmienka: Vyraz, potom: Vec<Prikaz>) -> Prikaz {
    Prikaz::Ak { podmienka, potom, inak: vec![] }
}
pub fn ak_inak(podmienka: Vyraz, potom: Vec<Prikaz>, inak: Vec<Prikaz>) -> Prikaz {
    Prikaz::Ak { podmienka, potom, inak }
}
pub fn pre(premenna: PremId, od: Vyraz, po: Vyraz, telo: Vec<Prikaz>) -> Prikaz {
    Prikaz::Pre { premenna, od, po, telo }
}

impl Vyraz {
    fn bin(self, op: Op2, b: Vyraz) -> Vyraz {
        Vyraz::Bin(op, Box::new(self), Box::new(b))
    }
    pub fn mensie(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::Mensie, b)
    }
    pub fn mensie_rovne(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::MensieRovne, b)
    }
    pub fn vacsie(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::Vacsie, b)
    }
    pub fn vacsie_rovne(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::VacsieRovne, b)
    }
    pub fn rovne(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::Rovne, b)
    }
    pub fn rozne(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::Rozne, b)
    }
    pub fn a(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::A, b)
    }
    pub fn alebo(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::Alebo, b)
    }
    pub fn min(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::Min, b)
    }
    pub fn max(self, b: Vyraz) -> Vyraz {
        self.bin(Op2::Max, b)
    }
    pub fn un(self, op: Op1) -> Vyraz {
        Vyraz::Un(op, Box::new(self))
    }
    pub fn na(self, t: Typ) -> Vyraz {
        Vyraz::Prevod(t, Box::new(self))
    }
}

macro_rules! operator {
    ($tr:ident, $f:ident, $op:ident) => {
        impl std::ops::$tr for Vyraz {
            type Output = Vyraz;
            fn $f(self, b: Vyraz) -> Vyraz {
                self.bin(Op2::$op, b)
            }
        }
    };
}
operator!(Add, add, Plus);
operator!(Sub, sub, Minus);
operator!(Mul, mul, Krat);
operator!(Div, div, Deleno);
operator!(Rem, rem, Zvysok);

impl std::ops::Neg for Vyraz {
    type Output = Vyraz;
    fn neg(self) -> Vyraz {
        self.un(Op1::Zaporne)
    }
}

impl fmt::Display for Typ {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Typ::F32 => "f32",
            Typ::U32 => "u32",
            Typ::I32 => "i32",
            Typ::Bool => "bool",
        })
    }
}
