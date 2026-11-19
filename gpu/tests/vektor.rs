//! Vektorový vykonávač musí dávať bitovo rovnaké výsledky ako referenčný interpret – na štandardných
//! kerneloch aj na tisíckach náhodne vygenerovaných kernelov so všetkými konštrukciami jazyka.

use orchidea_gpu::ir::*;
use orchidea_gpu::kernely;
use orchidea_gpu::zariadenie::{DataVazby, Hodnota32, Procesor, Zariadenie};
use orchidea_gpu::{interpret, vektor};
use orchidea_jadro::nahoda::Nahoda;
use orchidea_jadro::paralel;

/// Generátor náhodných kernelov bez súbehu: každé vlákno zapisuje len do svojich prvkov výstupov
/// a číta len zo vstupov.
struct Generator<'a> {
    n: &'a mut Nahoda,
    premenne: Vec<(PremId, Typ)>,
    pocitadla: Vec<PremId>,
    konstanty: Vec<(KonstId, Typ)>,
    vstupy: [(VazbaId, Typ); 3],
    vystupy: [(VazbaId, Typ); 3],
    gid: Vyraz,
    zapisov: u32,
}

impl Generator<'_> {
    fn nahoda(&mut self, n: u64) -> u64 {
        self.n.mensie_ako(n)
    }

    fn list(&mut self, t: Typ) -> Vyraz {
        let kandidati: Vec<Vyraz> = self
            .premenne
            .iter()
            .filter(|(_, tt)| *tt == t)
            .map(|(p, _)| Vyraz::Premenna(*p))
            .chain(self.konstanty.iter().filter(|(_, tt)| *tt == t).map(|(k, _)| Vyraz::Konstanta(*k)))
            .collect();
        if !kandidati.is_empty() && self.nahoda(2) == 0 {
            return kandidati[self.nahoda(kandidati.len() as u64) as usize].clone();
        }
        match t {
            Typ::F32 => {
                let x = [0.0, -0.0, 1.0, -2.5, 0.5, 3.75, 1e-30, 1e30, 100.0][self.nahoda(9) as usize];
                f(x)
            }
            Typ::U32 => match self.nahoda(4) {
                0 => id(self.nahoda(3) as u8),
                1 => dlzka(self.vstupy[self.nahoda(3) as usize].0),
                _ => u([0, 1, 2, 3, 7, 1000, u32::MAX][self.nahoda(7) as usize]),
            },
            Typ::I32 => i([0, 1, -1, 5, -7, i32::MIN, i32::MAX][self.nahoda(7) as usize]),
            Typ::Bool => Vyraz::Bool(self.nahoda(2) == 0),
        }
    }

    fn nenulove(&mut self, v: Vyraz, t: Typ) -> Vyraz {
        let (nula, jedna) = if t == Typ::U32 { (u(0), u(1)) } else { (i(0), i(1)) };
        vyber(v.clone().rovne(nula), jedna, v)
    }

    fn vyraz(&mut self, t: Typ, hlbka: u32) -> Vyraz {
        if hlbka == 0 || self.nahoda(4) == 0 {
            return self.list(t);
        }
        let h = hlbka - 1;
        match t {
            Typ::Bool => match self.nahoda(4) {
                0 => {
                    let tt = [Typ::F32, Typ::U32, Typ::I32][self.nahoda(3) as usize];
                    let (a, b) = (self.vyraz(tt, h), self.vyraz(tt, h));
                    let op = [
                        Op2::Mensie,
                        Op2::MensieRovne,
                        Op2::Vacsie,
                        Op2::VacsieRovne,
                        Op2::Rovne,
                        Op2::Rozne,
                    ][self.nahoda(6) as usize];
                    Vyraz::Bin(op, Box::new(a), Box::new(b))
                }
                1 => {
                    let (a, b) = (self.vyraz(Typ::Bool, h), self.vyraz(Typ::Bool, h));
                    let op = [Op2::A, Op2::Alebo, Op2::Rovne, Op2::Rozne][self.nahoda(4) as usize];
                    Vyraz::Bin(op, Box::new(a), Box::new(b))
                }
                2 => self.vyraz(Typ::Bool, h).un(Op1::Nie),
                _ => {
                    let (p, a, b) =
                        (self.vyraz(Typ::Bool, h), self.vyraz(Typ::Bool, h), self.vyraz(Typ::Bool, h));
                    vyber(p, a, b)
                }
            },
            _ => match self.nahoda(6) {
                0 | 1 => {
                    let ops =
                        [Op2::Plus, Op2::Minus, Op2::Krat, Op2::Deleno, Op2::Zvysok, Op2::Min, Op2::Max];
                    let op = ops[self.nahoda(7) as usize];
                    let a = self.vyraz(t, h);
                    let mut b = self.vyraz(t, h);
                    if t != Typ::F32 && matches!(op, Op2::Deleno | Op2::Zvysok) && self.nahoda(8) != 0 {
                        b = self.nenulove(b, t);
                    }
                    Vyraz::Bin(op, Box::new(a), Box::new(b))
                }
                2 => {
                    let ops: &[Op1] = match t {
                        Typ::F32 => &[Op1::Zaporne, Op1::Abs, Op1::Odmocnina, Op1::Exp, Op1::Ln, Op1::Dole],
                        Typ::I32 => &[Op1::Zaporne, Op1::Abs],
                        _ => &[],
                    };
                    if ops.is_empty() {
                        return self.vyraz(t, h);
                    }
                    let op = ops[self.nahoda(ops.len() as u64) as usize];
                    self.vyraz(t, h).un(op)
                }
                3 => {
                    let z = [Typ::F32, Typ::U32, Typ::I32][self.nahoda(3) as usize];
                    self.vyraz(z, h).na(t)
                }
                4 => {
                    let kandidati: Vec<(VazbaId, Typ)> =
                        self.vstupy.iter().copied().filter(|(_, tt)| *tt == t).collect();
                    let (v, _) = kandidati[0];
                    let index = self.vyraz(Typ::U32, h) % dlzka(v);
                    citaj(v, index)
                }
                _ => {
                    let (p, a, b) = (self.vyraz(Typ::Bool, h), self.vyraz(t, h), self.vyraz(t, h));
                    vyber(p, a, b)
                }
            },
        }
    }

    fn prikazy(&mut self, hlbka: u32, pocet: u32) -> Vec<Prikaz> {
        let mut p = Vec::new();
        for _ in 0..pocet {
            match self.nahoda(if hlbka == 0 { 2 } else { 5 }) {
                0 => {
                    let i = self.nahoda(self.premenne.len() as u64) as usize;
                    let (pr, t) = self.premenne[i];
                    let v = self.vyraz(t, 4);
                    p.push(nastav(pr, v));
                }
                1 => {
                    if self.zapisov < 8 {
                        let (v, t) = self.vystupy[self.nahoda(3) as usize];
                        let j = self.zapisov;
                        self.zapisov += 1;
                        let hodnota = self.vyraz(t, 4);
                        p.push(zapis(v, self.gid.clone() * u(8) + u(j), hodnota));
                    }
                }
                2 => {
                    let c = self.vyraz(Typ::Bool, 3);
                    let potom = self.prikazy(hlbka - 1, 3);
                    let inak = if self.nahoda(2) == 0 { self.prikazy(hlbka - 1, 2) } else { vec![] };
                    p.push(ak_inak(c, potom, inak));
                }
                3 if !self.pocitadla.is_empty() => {
                    let q = self.pocitadla.pop().expect("počítadlo");
                    let od = self.vyraz(Typ::U32, 2) % u(4);
                    let po = od.clone() + self.vyraz(Typ::U32, 2) % u(5);
                    let telo = self.prikazy(hlbka - 1, 3);
                    p.push(pre(q, od, po, telo));
                    self.pocitadla.push(q);
                }
                _ => {
                    let i = self.nahoda(self.premenne.len() as u64) as usize;
                    let (pr, t) = self.premenne[i];
                    let v = self.vyraz(t, 2);
                    p.push(nastav(pr, v));
                }
            }
        }
        p
    }
}

fn nahodny_kernel(n: &mut Nahoda) -> (Kernel, [u32; 3]) {
    let skupina = [[8, 4, 1], [32, 1, 1], [4, 4, 2], [1, 1, 1], [16, 16, 1]][n.mensie_ako(5) as usize];
    let skupiny = [1 + n.mensie_ako(3) as u32, 1 + n.mensie_ako(3) as u32, 1 + n.mensie_ako(2) as u32];
    let mut k = Kernel::novy("náhodný", skupina);
    let vstupy = [
        (k.vazba("vf", Typ::F32, Pristup::Citanie), Typ::F32),
        (k.vazba("vu", Typ::U32, Pristup::Citanie), Typ::U32),
        (k.vazba("vi", Typ::I32, Pristup::Citanie), Typ::I32),
    ];
    let vystupy = [
        (k.vazba("of", Typ::F32, Pristup::Zapis), Typ::F32),
        (k.vazba("ou", Typ::U32, Pristup::Zapis), Typ::U32),
        (k.vazba("oi", Typ::I32, Pristup::Zapis), Typ::I32),
    ];
    let konstanty = vec![
        (k.konstanta("kf", Typ::F32), Typ::F32),
        (k.konstanta("ku", Typ::U32), Typ::U32),
        (k.konstanta("ki", Typ::I32), Typ::I32),
    ];
    let typy = [Typ::F32, Typ::U32, Typ::I32, Typ::Bool, Typ::F32, Typ::U32];
    let premenne: Vec<(PremId, Typ)> = typy.iter().map(|t| (k.premenna(*t), *t)).collect();
    let pocitadla: Vec<PremId> = (0..3).map(|_| k.premenna(Typ::U32)).collect();
    let (wx, wy) = (skupina[0] * skupiny[0], skupina[1] * skupiny[1]);
    let gid = id(0) + id(1) * u(wx) + id(2) * u(wx * wy);
    let mut g = Generator { n, premenne, pocitadla, konstanty, vstupy, vystupy, gid, zapisov: 0 };
    k.telo = g.prikazy(3, 6);
    (k, skupiny)
}

type Vystupy = (Vec<f32>, Vec<u32>, Vec<i32>);

fn spusti(z: &dyn Zariadenie, k: &Kernel, skupiny: [u32; 3], semienko: u64) -> Result<Vystupy, String> {
    let mut n = Nahoda::zo_semienka(semienko);
    let vlakien = (k.skupina.iter().product::<u32>() * skupiny.iter().product::<u32>()) as usize;
    let mut vf: Vec<f32> = (0..37).map(|_| (n.f32() - 0.5) * 20.0).collect();
    vf[0] = f32::INFINITY;
    vf[1] = -0.0;
    let mut vu: Vec<u32> = (0..41).map(|_| n.dalsie() as u32 >> (n.mensie_ako(32) as u32)).collect();
    let mut vi: Vec<i32> = (0..29).map(|_| n.dalsie() as i32 >> (n.mensie_ako(32) as u32)).collect();
    let mut of = vec![0.0f32; vlakien * 8];
    let mut ou = vec![0u32; vlakien * 8];
    let mut oi = vec![0i32; vlakien * 8];
    z.spusti(
        k,
        &mut [
            DataVazby::F32(&mut vf),
            DataVazby::U32(&mut vu),
            DataVazby::I32(&mut vi),
            DataVazby::F32(&mut of),
            DataVazby::U32(&mut ou),
            DataVazby::I32(&mut oi),
        ],
        &[Hodnota32::F32(1.5), Hodnota32::U32(13), Hodnota32::I32(-3)],
        skupiny,
    )
    .map_err(|e| e.to_string())?;
    Ok((of, ou, oi))
}

/// Bitová zhoda; všetky NaN sa považujú za rovnaké (obsah NaN neurčuje ani žiadne GPU).
fn bity(x: f32) -> u32 {
    if x.is_nan() { 0x7fc0_0000 } else { x.to_bits() }
}

#[test]
fn nahodne_kernely_sa_zhoduju_s_interpretom_bit_po_bite() {
    let referencia = Procesor::default();
    let vektorovy = Procesor::vektorovy();
    let mut n = Nahoda::zo_semienka(20_261_119);
    let (mut uspesnych, mut chyb) = (0, 0);
    for pokus in 0..1500u64 {
        let (k, skupiny) = nahodny_kernel(&mut n);
        k.over().unwrap_or_else(|e| panic!("generátor vyrobil neplatný kernel: {e}\n{k:?}"));
        let a = spusti(&referencia, &k, skupiny, pokus);
        let b = spusti(&vektorovy, &k, skupiny, pokus);
        match (&a, &b) {
            (Ok(a), Ok(b)) => {
                uspesnych += 1;
                let rozne_f = a.0.iter().zip(&b.0).filter(|(x, y)| bity(**x) != bity(**y)).count();
                assert_eq!(rozne_f, 0, "pokus {pokus}: f32 sa líši\n{k:?}");
                assert_eq!(a.1, b.1, "pokus {pokus}: u32 sa líši\n{k:?}");
                assert_eq!(a.2, b.2, "pokus {pokus}: i32 sa líši\n{k:?}");
            }
            (Err(_), Err(_)) => chyb += 1,
            _ => panic!("pokus {pokus}: interpret {:?} vs vektorový {:?}\n{k:?}", a.is_ok(), b.is_ok()),
        }
    }
    eprintln!("náhodné kernely: {uspesnych} zhodných výsledkov, {chyb} zhodne odhalených chýb");
    assert!(uspesnych > 1000, "príliš málo úspešných behov ({uspesnych}) – generátor treba upraviť");
    assert!(chyb > 0, "generátor má občas vyrobiť aj delenie nulou");
}

#[test]
fn standardne_kernely_a_pocet_vlakien() {
    let mut n = Nahoda::zo_semienka(5);
    let (m, nn, k) = (70, 45, 33);
    let a: Vec<f32> = (0..m * k).map(|_| n.f32() * 2.0 - 1.0).collect();
    let b: Vec<f32> = (0..k * nn).map(|_| n.f32() * 2.0 - 1.0).collect();
    let kern = kernely::nasobenie_presne();
    let program = vektor::preloz(&kern).expect("preklad");
    let nasob = |vykonaj: &dyn Fn(&mut [DataVazby<'_>]) -> orchidea_jadro::Vysledok<()>| {
        let (mut a, mut b, mut c) = (a.clone(), b.clone(), vec![f32::NAN; m * nn]);
        vykonaj(&mut [DataVazby::F32(&mut a), DataVazby::F32(&mut b), DataVazby::F32(&mut c)])
            .expect("výpočet");
        c
    };
    let konst = [Hodnota32::U32(m as u32), Hodnota32::U32(nn as u32), Hodnota32::U32(k as u32)];
    let skupiny = [(nn as u32).div_ceil(16), (m as u32).div_ceil(16), 1];
    let ref_c = nasob(&|v| interpret::vykonaj(&kern, v, &konst, skupiny));
    for vlakna in [1, 2, 3, 0] {
        paralel::obmedz_vlakna(vlakna);
        let c = nasob(&|v| program.vykonaj(v, &konst, skupiny));
        assert!(c.iter().zip(&ref_c).all(|(x, y)| x.to_bits() == y.to_bits()), "vlákien {vlakna}");
    }
    paralel::obmedz_vlakna(0);
    // čítanie mimo bufferu odhalí aj vektorový vykonávač
    let mut kr = Kernel::novy("mimo", [64, 1, 1]);
    let x = kr.vazba("x", Typ::F32, Pristup::CitanieZapis);
    kr.telo = vec![ak(id(0).rovne(u(63)), vec![zapis(x, u(0), citaj(x, u(5)))])];
    let mut d = vec![1.0f32; 2];
    assert!(vektor::vykonaj(&kr, &mut [DataVazby::F32(&mut d)], &[], [1, 1, 1]).is_err());
    // neaktívne pruhy nečítajú: rovnaký kernel s podmienkou, ktorá nikdy neplatí, prebehne
    kr.telo = vec![ak(id(0).rovne(u(64)), vec![zapis(x, u(0), citaj(x, u(5)))])];
    assert!(vektor::vykonaj(&kr, &mut [DataVazby::F32(&mut d)], &[], [1, 1, 1]).is_ok());
}
