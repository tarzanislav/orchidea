//! Výraz – malý bezpečný jazyk pre parametre (napr. `2 * sin(t * tau) + jas`).
//!
//! Bez cyklov a bez prístupu k čomukoľvek okrem pomenovaných čísel – vyhodnotenie vždy skončí. Dĺžka textu,
//! hĺbka vnorenia aj počet uzlov sú obmedzené, takže ani poškodený súbor nemôže spôsobiť pád. Matematika je
//! deterministická (rovnaký výsledok na každej platforme).

use crate::chyba::{Chyba, Vysledok};
use crate::kodovanie::{Citac, Kodovatelne, Zapisovac};
use crate::matematika as mat;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

const NAJDLHSI_TEXT: usize = 64 * 1024;
const NAJVACSIA_HLBKA: usize = 128;
const NAJVIAC_UZLOV: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op2 {
    Plus,
    Minus,
    Krat,
    Deleno,
    Zvysok,
    Mocnina,
    Mensie,
    Vacsie,
    MensieRovne,
    VacsieRovne,
    Rovne,
    Rozne,
    A,
    Alebo,
}

#[derive(Debug, Clone, PartialEq)]
enum Uzol {
    Cislo(f64),
    Premenna(Arc<str>),
    Zaporne(Box<Uzol>),
    Nie(Box<Uzol>),
    Dvojica(Op2, Box<Uzol>, Box<Uzol>),
    Funkcia(Funkcia, Vec<Uzol>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Funkcia {
    Sin,
    Cos,
    Tan,
    Exp,
    Ln,
    Log2,
    Log10,
    Odmocnina,
    Abs,
    Min,
    Max,
    Obmedz,
    Zmiesaj,
    Dole,
    Hore,
    Zaokruhli,
    Mocnina,
    Ak,
    Tanh,
    Krok,
    Hladky,
    Znamienko,
}

impl Funkcia {
    fn z_mena(t: &str) -> Option<(Funkcia, usize)> {
        Some(match t {
            "sin" => (Funkcia::Sin, 1),
            "cos" => (Funkcia::Cos, 1),
            "tan" => (Funkcia::Tan, 1),
            "exp" => (Funkcia::Exp, 1),
            "ln" => (Funkcia::Ln, 1),
            "log2" => (Funkcia::Log2, 1),
            "log10" => (Funkcia::Log10, 1),
            "sqrt" | "odmocnina" => (Funkcia::Odmocnina, 1),
            "abs" => (Funkcia::Abs, 1),
            "min" => (Funkcia::Min, 2),
            "max" => (Funkcia::Max, 2),
            "obmedz" | "clamp" => (Funkcia::Obmedz, 3),
            "zmiesaj" | "lerp" => (Funkcia::Zmiesaj, 3),
            "dole" | "floor" => (Funkcia::Dole, 1),
            "hore" | "ceil" => (Funkcia::Hore, 1),
            "zaokruhli" | "round" => (Funkcia::Zaokruhli, 1),
            "mocnina" | "pow" => (Funkcia::Mocnina, 2),
            "ak" | "if" => (Funkcia::Ak, 3),
            "tanh" => (Funkcia::Tanh, 1),
            "krok" | "step" => (Funkcia::Krok, 2),
            "hladky" | "smoothstep" => (Funkcia::Hladky, 3),
            "znamienko" | "sign" => (Funkcia::Znamienko, 1),
            _ => return None,
        })
    }
}

/// Skompilovaný výraz.
#[derive(Clone)]
pub struct Vyraz {
    text: Arc<str>,
    koren: Arc<Uzol>,
}

impl PartialEq for Vyraz {
    fn eq(&self, b: &Self) -> bool {
        self.text == b.text
    }
}

impl fmt::Debug for Vyraz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Vyraz({:?})", &*self.text)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Cislo(f64),
    Meno(String),
    Op(&'static str),
    Lava,
    Prava,
    Ciarka,
}

fn tokeny(text: &str) -> Vysledok<Vec<Token>> {
    let z: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut v = Vec::new();
    while i < z.len() {
        let c = z[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && z.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < z.len() && (z[i].is_ascii_digit() || z[i] == '.') {
                i += 1;
            }
            if i < z.len() && (z[i] == 'e' || z[i] == 'E') {
                let mut j = i + 1;
                if j < z.len() && (z[j] == '+' || z[j] == '-') {
                    j += 1;
                }
                if j < z.len() && z[j].is_ascii_digit() {
                    i = j;
                    while i < z.len() && z[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let s: String = z[start..i].iter().collect();
            let n: f64 = s.parse().map_err(|_| Chyba::MimoRozsahu(format!("neplatné číslo „{s}“")))?;
            v.push(Token::Cislo(n));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < z.len() && (z[i].is_alphanumeric() || z[i] == '_') {
                i += 1;
            }
            v.push(Token::Meno(z[start..i].iter().collect()));
        } else {
            let dva: String = z[i..(i + 2).min(z.len())].iter().collect();
            let op2 = ["<=", ">=", "==", "!=", "&&", "||"].into_iter().find(|o| *o == dva);
            if let Some(o) = op2 {
                v.push(Token::Op(o));
                i += 2;
                continue;
            }
            let t = match c {
                '(' => Token::Lava,
                ')' => Token::Prava,
                ',' => Token::Ciarka,
                '+' => Token::Op("+"),
                '-' | '−' => Token::Op("-"),
                '*' | '×' | '·' => Token::Op("*"),
                '/' | '÷' => Token::Op("/"),
                '%' => Token::Op("%"),
                '^' => Token::Op("^"),
                '<' => Token::Op("<"),
                '>' => Token::Op(">"),
                '!' => Token::Op("!"),
                _ => return Err(Chyba::MimoRozsahu(format!("neočakávaný znak „{c}“ vo výraze"))),
            };
            v.push(t);
            i += 1;
        }
    }
    Ok(v)
}

struct Parser {
    t: Vec<Token>,
    i: usize,
    hlbka: usize,
    uzlov: usize,
}

impl Parser {
    fn pozri(&self) -> Option<&Token> {
        self.t.get(self.i)
    }

    fn uzol(&mut self, u: Uzol) -> Vysledok<Uzol> {
        self.uzlov += 1;
        if self.uzlov > NAJVIAC_UZLOV {
            return Err(Chyba::Limit("výraz má príliš veľa častí".into()));
        }
        Ok(u)
    }

    fn vnor(&mut self) -> Vysledok<()> {
        self.hlbka += 1;
        if self.hlbka > NAJVACSIA_HLBKA {
            return Err(Chyba::Limit("výraz je príliš hlboko vnorený".into()));
        }
        Ok(())
    }

    fn dvojica(&mut self, uroven: usize) -> Vysledok<Uzol> {
        const UROVNE: [&[(&str, Op2)]; 5] = [
            &[("||", Op2::Alebo)],
            &[("&&", Op2::A)],
            &[
                ("<", Op2::Mensie),
                (">", Op2::Vacsie),
                ("<=", Op2::MensieRovne),
                (">=", Op2::VacsieRovne),
                ("==", Op2::Rovne),
                ("!=", Op2::Rozne),
            ],
            &[("+", Op2::Plus), ("-", Op2::Minus)],
            &[("*", Op2::Krat), ("/", Op2::Deleno), ("%", Op2::Zvysok)],
        ];
        if uroven == UROVNE.len() {
            return self.unarny();
        }
        let mut lavy = self.dvojica(uroven + 1)?;
        while let Some(Token::Op(o)) = self.pozri() {
            let Some(&(_, op)) = UROVNE[uroven].iter().find(|(z, _)| z == o) else { break };
            self.i += 1;
            let pravy = self.dvojica(uroven + 1)?;
            lavy = self.uzol(Uzol::Dvojica(op, Box::new(lavy), Box::new(pravy)))?;
        }
        Ok(lavy)
    }

    fn unarny(&mut self) -> Vysledok<Uzol> {
        self.vnor()?;
        let u = match self.pozri() {
            Some(Token::Op("-")) => {
                self.i += 1;
                let u = self.unarny()?;
                self.uzol(Uzol::Zaporne(Box::new(u)))?
            }
            Some(Token::Op("+")) => {
                self.i += 1;
                self.unarny()?
            }
            Some(Token::Op("!")) => {
                self.i += 1;
                let u = self.unarny()?;
                self.uzol(Uzol::Nie(Box::new(u)))?
            }
            _ => self.mocnina()?,
        };
        self.hlbka -= 1;
        Ok(u)
    }

    fn mocnina(&mut self) -> Vysledok<Uzol> {
        let zaklad = self.atom()?;
        if let Some(Token::Op("^")) = self.pozri() {
            self.i += 1;
            let exponent = self.unarny()?;
            return self.uzol(Uzol::Dvojica(Op2::Mocnina, Box::new(zaklad), Box::new(exponent)));
        }
        Ok(zaklad)
    }

    fn atom(&mut self) -> Vysledok<Uzol> {
        match self.t.get(self.i).cloned() {
            Some(Token::Cislo(n)) => {
                self.i += 1;
                self.uzol(Uzol::Cislo(n))
            }
            Some(Token::Lava) => {
                self.i += 1;
                self.vnor()?;
                let u = self.dvojica(0)?;
                self.hlbka -= 1;
                if self.pozri() != Some(&Token::Prava) {
                    return Err(Chyba::MimoRozsahu("chýba „)“".into()));
                }
                self.i += 1;
                Ok(u)
            }
            Some(Token::Meno(n)) => {
                self.i += 1;
                if self.pozri() == Some(&Token::Lava) {
                    let (f, arita) =
                        Funkcia::z_mena(&n).ok_or_else(|| Chyba::Chybajuce(format!("funkcia „{n}“")))?;
                    self.i += 1;
                    let mut argumenty = Vec::new();
                    if self.pozri() != Some(&Token::Prava) {
                        loop {
                            self.vnor()?;
                            argumenty.push(self.dvojica(0)?);
                            self.hlbka -= 1;
                            match self.pozri() {
                                Some(Token::Ciarka) => self.i += 1,
                                Some(Token::Prava) => break,
                                _ => {
                                    return Err(Chyba::MimoRozsahu(format!(
                                        "v „{n}(…)“ chýba „,“ alebo „)“"
                                    )));
                                }
                            }
                        }
                    }
                    self.i += 1;
                    if argumenty.len() != arita {
                        return Err(Chyba::MimoRozsahu(format!(
                            "„{n}“ potrebuje {arita} argumentov, má {}",
                            argumenty.len()
                        )));
                    }
                    return self.uzol(Uzol::Funkcia(f, argumenty));
                }
                let konst = match n.as_str() {
                    "pi" | "π" => Some(std::f64::consts::PI),
                    "tau" | "τ" => Some(std::f64::consts::TAU),
                    "e" => Some(std::f64::consts::E),
                    _ => None,
                };
                match konst {
                    Some(c) => self.uzol(Uzol::Cislo(c)),
                    None => self.uzol(Uzol::Premenna(Arc::from(n.as_str()))),
                }
            }
            Some(t) => Err(Chyba::MimoRozsahu(format!("neočakávaný prvok {t:?}"))),
            None => Err(Chyba::MimoRozsahu("výraz sa nečakane skončil".into())),
        }
    }
}

fn pravda(x: f64) -> bool {
    x != 0.0 && !x.is_nan()
}

fn b(x: bool) -> f64 {
    if x { 1.0 } else { 0.0 }
}

fn vyhodnot_uzol(u: &Uzol, premenne: &dyn Fn(&str) -> Option<f64>) -> Vysledok<f64> {
    Ok(match u {
        Uzol::Cislo(n) => *n,
        Uzol::Premenna(n) => premenne(n).ok_or_else(|| Chyba::Chybajuce(format!("premenná „{n}“")))?,
        Uzol::Zaporne(a) => -vyhodnot_uzol(a, premenne)?,
        Uzol::Nie(a) => b(!pravda(vyhodnot_uzol(a, premenne)?)),
        Uzol::Dvojica(op, a, c) => {
            let x = vyhodnot_uzol(a, premenne)?;
            match op {
                Op2::A if !pravda(x) => return Ok(0.0),
                Op2::Alebo if pravda(x) => return Ok(1.0),
                _ => {}
            }
            let y = vyhodnot_uzol(c, premenne)?;
            match op {
                Op2::Plus => x + y,
                Op2::Minus => x - y,
                Op2::Krat => x * y,
                Op2::Deleno => x / y,
                Op2::Zvysok => x.rem_euclid(y),
                Op2::Mocnina => mat::mocnina(x, y),
                Op2::Mensie => b(x < y),
                Op2::Vacsie => b(x > y),
                Op2::MensieRovne => b(x <= y),
                Op2::VacsieRovne => b(x >= y),
                Op2::Rovne => b(x == y),
                Op2::Rozne => b(x != y),
                Op2::A | Op2::Alebo => b(pravda(y)),
            }
        }
        Uzol::Funkcia(f, a) => {
            if *f == Funkcia::Ak {
                let p = vyhodnot_uzol(&a[0], premenne)?;
                return vyhodnot_uzol(if pravda(p) { &a[1] } else { &a[2] }, premenne);
            }
            let mut v = [0.0f64; 3];
            for (i, x) in a.iter().enumerate() {
                v[i] = vyhodnot_uzol(x, premenne)?;
            }
            match f {
                Funkcia::Sin => mat::sin(v[0]),
                Funkcia::Cos => mat::cos(v[0]),
                Funkcia::Tan => mat::tan(v[0]),
                Funkcia::Exp => mat::exp(v[0]),
                Funkcia::Ln => mat::ln(v[0]),
                Funkcia::Log2 => mat::log2(v[0]),
                Funkcia::Log10 => mat::log10(v[0]),
                Funkcia::Odmocnina => mat::odmocnina(v[0]),
                Funkcia::Abs => v[0].abs(),
                Funkcia::Min => v[0].min(v[1]),
                Funkcia::Max => v[0].max(v[1]),
                Funkcia::Obmedz => v[0].max(v[1]).min(v[2]),
                Funkcia::Zmiesaj => v[0] + (v[1] - v[0]) * v[2],
                Funkcia::Dole => v[0].floor(),
                Funkcia::Hore => v[0].ceil(),
                Funkcia::Zaokruhli => v[0].round(),
                Funkcia::Mocnina => mat::mocnina(v[0], v[1]),
                Funkcia::Tanh => mat::tanh(v[0]),
                Funkcia::Krok => b(v[1] >= v[0]),
                Funkcia::Hladky => {
                    let t = ((v[2] - v[0]) / (v[1] - v[0])).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                }
                Funkcia::Znamienko => {
                    if v[0] > 0.0 {
                        1.0
                    } else if v[0] < 0.0 {
                        -1.0
                    } else {
                        0.0
                    }
                }
                Funkcia::Ak => unreachable!("spracované vyššie"),
            }
        }
    })
}

fn zbieraj(u: &Uzol, mena: &mut BTreeSet<String>) {
    match u {
        Uzol::Cislo(_) => {}
        Uzol::Premenna(n) => {
            mena.insert(n.to_string());
        }
        Uzol::Zaporne(a) | Uzol::Nie(a) => zbieraj(a, mena),
        Uzol::Dvojica(_, a, b) => {
            zbieraj(a, mena);
            zbieraj(b, mena);
        }
        Uzol::Funkcia(_, a) => a.iter().for_each(|x| zbieraj(x, mena)),
    }
}

impl Vyraz {
    /// Skompiluje výraz z textu.
    pub fn novy(text: &str) -> Vysledok<Vyraz> {
        if text.len() > NAJDLHSI_TEXT {
            return Err(Chyba::Limit("výraz je dlhší ako 64 KiB".into()));
        }
        let mut p = Parser { t: tokeny(text)?, i: 0, hlbka: 0, uzlov: 0 };
        let koren = p.dvojica(0)?;
        if p.i != p.t.len() {
            return Err(Chyba::MimoRozsahu(format!("nadbytočný text vo výraze od prvku {:?}", p.t[p.i])));
        }
        Ok(Vyraz { text: Arc::from(text), koren: Arc::new(koren) })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Vyhodnotí výraz; premenné dodá funkcia.
    pub fn vyhodnot(&self, premenne: &dyn Fn(&str) -> Option<f64>) -> Vysledok<f64> {
        vyhodnot_uzol(&self.koren, premenne)
    }

    /// Mená premenných, od ktorých výraz závisí.
    pub fn premenne(&self) -> BTreeSet<String> {
        let mut m = BTreeSet::new();
        zbieraj(&self.koren, &mut m);
        m
    }
}

impl Kodovatelne for Vyraz {
    fn zakoduj(&self, z: &mut Zapisovac) {
        z.text(&self.text);
    }
    fn dekoduj(c: &mut Citac<'_>) -> Vysledok<Self> {
        Vyraz::novy(c.text()?)
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    fn v(t: &str) -> f64 {
        Vyraz::novy(t).unwrap().vyhodnot(&|n| if n == "t" { Some(0.5) } else { None }).unwrap()
    }

    #[test]
    fn aritmetika_a_priorita() {
        assert_eq!(v("1 + 2 * 3"), 7.0);
        assert_eq!(v("(1 + 2) * 3"), 9.0);
        assert_eq!(v("2 ^ 3 ^ 2"), 512.0);
        assert_eq!(v("-2 ^ 2"), -4.0);
        assert_eq!(v("10 % 3"), 1.0);
        assert_eq!(v("1.5e3 + .5"), 1500.5);
        assert_eq!(v("t * 4"), 2.0);
        assert_eq!(v("ak(t > 0.25, 10, 20)"), 10.0);
        assert_eq!(v("1 < 2 && 3 > 4 || 1"), 1.0);
        assert_eq!(v("obmedz(5, 0, 1)"), 1.0);
        assert_eq!(v("zmiesaj(0, 10, t)"), 5.0);
        assert!((v("sin(pi / 2)") - 1.0).abs() < 1e-15);
        assert_eq!(v("hladky(0, 1, t)"), 0.5);
        assert_eq!(v("2 × 3 − 1"), 5.0);
    }

    #[test]
    fn chyby_a_limity() {
        assert!(Vyraz::novy("1 +").is_err());
        assert!(Vyraz::novy("sin(1, 2)").is_err());
        assert!(Vyraz::novy("neznama(1)").is_err());
        assert!(Vyraz::novy("(1").is_err());
        assert!(Vyraz::novy("1 2").is_err());
        assert!(Vyraz::novy("1 $ 2").is_err());
        assert!(Vyraz::novy(&"(".repeat(1000)).is_err());
        assert!(Vyraz::novy(&"-".repeat(1000)).is_err());
        assert!(Vyraz::novy("x").unwrap().vyhodnot(&|_| None).is_err());
        let x = Vyraz::novy("a * sin(b) + a").unwrap();
        assert_eq!(x.premenne().into_iter().collect::<Vec<_>>(), vec!["a".to_string(), "b".to_string()]);
    }
}
