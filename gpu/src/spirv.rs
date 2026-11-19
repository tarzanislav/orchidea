//! Vlastný prekladač kernelu do SPIR-V 1.3 (binárny jazyk shaderov pre Vulkan).
//!
//! Žiadny cudzí prekladač: inštrukcie sa skladajú priamo podľa špecifikácie SPIR-V. Každé sčítanie,
//! odčítanie, násobenie a delenie v plávajúcej čiarke nesie dekoráciu NoContraction – ovládač ich nesmie
//! spojiť do FMA, takže poradie zaokrúhľovania je rovnaké ako v presnom režime procesora.
//! Správnosť výstupu overuje v CI nezávislý validátor `spirv-val`.

use crate::ir::{Kernel, Op1, Op2, Prikaz, Typ, Vyraz};
use orchidea_jadro::Vysledok;
use std::collections::HashMap;

// operačné kódy
const OP_EXT_INST_IMPORT: u16 = 11;
const OP_EXT_INST: u16 = 12;
const OP_MEMORY_MODEL: u16 = 14;
const OP_ENTRY_POINT: u16 = 15;
const OP_EXECUTION_MODE: u16 = 16;
const OP_CAPABILITY: u16 = 17;
const OP_TYPE_VOID: u16 = 19;
const OP_TYPE_BOOL: u16 = 20;
const OP_TYPE_INT: u16 = 21;
const OP_TYPE_FLOAT: u16 = 22;
const OP_TYPE_VECTOR: u16 = 23;
const OP_TYPE_RUNTIME_ARRAY: u16 = 29;
const OP_TYPE_STRUCT: u16 = 30;
const OP_TYPE_POINTER: u16 = 32;
const OP_TYPE_FUNCTION: u16 = 33;
const OP_CONSTANT_TRUE: u16 = 41;
const OP_CONSTANT_FALSE: u16 = 42;
const OP_CONSTANT: u16 = 43;
const OP_FUNCTION: u16 = 54;
const OP_FUNCTION_END: u16 = 56;
const OP_VARIABLE: u16 = 59;
const OP_LOAD: u16 = 61;
const OP_STORE: u16 = 62;
const OP_ACCESS_CHAIN: u16 = 65;
const OP_ARRAY_LENGTH: u16 = 68;
const OP_DECORATE: u16 = 71;
const OP_MEMBER_DECORATE: u16 = 72;
const OP_COMPOSITE_EXTRACT: u16 = 81;
const OP_CONVERT_F_TO_U: u16 = 109;
const OP_CONVERT_F_TO_S: u16 = 110;
const OP_CONVERT_S_TO_F: u16 = 111;
const OP_CONVERT_U_TO_F: u16 = 112;
const OP_BITCAST: u16 = 124;
const OP_S_NEGATE: u16 = 126;
const OP_F_NEGATE: u16 = 127;
const OP_I_ADD: u16 = 128;
const OP_F_ADD: u16 = 129;
const OP_I_SUB: u16 = 130;
const OP_F_SUB: u16 = 131;
const OP_I_MUL: u16 = 132;
const OP_F_MUL: u16 = 133;
const OP_U_DIV: u16 = 134;
const OP_S_DIV: u16 = 135;
const OP_F_DIV: u16 = 136;
const OP_U_MOD: u16 = 137;
const OP_S_REM: u16 = 138;
const OP_F_REM: u16 = 140;
const OP_LOGICAL_EQUAL: u16 = 164;
const OP_LOGICAL_NOT_EQUAL: u16 = 165;
const OP_LOGICAL_OR: u16 = 166;
const OP_LOGICAL_AND: u16 = 167;
const OP_LOGICAL_NOT: u16 = 168;
const OP_SELECT: u16 = 169;
const OP_I_EQUAL: u16 = 170;
const OP_I_NOT_EQUAL: u16 = 171;
const OP_U_GREATER_THAN: u16 = 172;
const OP_S_GREATER_THAN: u16 = 173;
const OP_U_GREATER_THAN_EQUAL: u16 = 174;
const OP_S_GREATER_THAN_EQUAL: u16 = 175;
const OP_U_LESS_THAN: u16 = 176;
const OP_S_LESS_THAN: u16 = 177;
const OP_U_LESS_THAN_EQUAL: u16 = 178;
const OP_S_LESS_THAN_EQUAL: u16 = 179;
const OP_F_ORD_EQUAL: u16 = 180;
const OP_F_UNORD_NOT_EQUAL: u16 = 183;
const OP_F_ORD_LESS_THAN: u16 = 184;
const OP_F_ORD_GREATER_THAN: u16 = 186;
const OP_F_ORD_LESS_THAN_EQUAL: u16 = 188;
const OP_F_ORD_GREATER_THAN_EQUAL: u16 = 190;
const OP_LOOP_MERGE: u16 = 246;
const OP_SELECTION_MERGE: u16 = 247;
const OP_LABEL: u16 = 248;
const OP_BRANCH: u16 = 249;
const OP_BRANCH_CONDITIONAL: u16 = 250;
const OP_RETURN: u16 = 253;

// GLSL.std.450
const GLSL_F_ABS: u32 = 4;
const GLSL_S_ABS: u32 = 5;
const GLSL_FLOOR: u32 = 8;
const GLSL_EXP: u32 = 27;
const GLSL_LOG: u32 = 28;
const GLSL_SQRT: u32 = 31;
const GLSL_F_MIN: u32 = 37;
const GLSL_U_MIN: u32 = 38;
const GLSL_S_MIN: u32 = 39;
const GLSL_F_MAX: u32 = 40;
const GLSL_U_MAX: u32 = 41;
const GLSL_S_MAX: u32 = 42;

// triedy úložiska, dekorácie, vstavané premenné
const SC_INPUT: u32 = 1;
const SC_FUNCTION: u32 = 7;
const SC_PUSH_CONSTANT: u32 = 9;
const SC_STORAGE_BUFFER: u32 = 12;
const DEC_BLOCK: u32 = 2;
const DEC_ARRAY_STRIDE: u32 = 6;
const DEC_BUILT_IN: u32 = 11;
const DEC_NON_WRITABLE: u32 = 24;
const DEC_BINDING: u32 = 33;
const DEC_DESCRIPTOR_SET: u32 = 34;
const DEC_OFFSET: u32 = 35;
const DEC_NO_CONTRACTION: u32 = 42;
const BUILT_IN_GLOBAL_INVOCATION_ID: u32 = 28;

fn instrukcia(ciel: &mut Vec<u32>, op: u16, operandy: &[u32]) {
    ciel.push(((operandy.len() as u32 + 1) << 16) | op as u32);
    ciel.extend_from_slice(operandy);
}

fn retazec(t: &str) -> Vec<u32> {
    let mut b = t.as_bytes().to_vec();
    b.push(0);
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

struct Prekladac<'k> {
    k: &'k Kernel,
    id: u32,
    dekoracie: Vec<u32>,
    typy: Vec<u32>,
    telo: Vec<u32>,
    glsl: u32,
    t_void: u32,
    t_bool: u32,
    t_u32: u32,
    t_i32: u32,
    t_f32: u32,
    t_v3u: u32,
    ukazovatele: HashMap<(u32, u32), u32>,
    konstanty: HashMap<(u32, u32), u32>,
    pravda: Option<u32>,
    nepravda: Option<u32>,
    premenne: Vec<u32>,
    vazby: Vec<u32>,
    konst_var: u32,
    id_vlakna: u32,
}

impl<'k> Prekladac<'k> {
    fn nove_id(&mut self) -> u32 {
        let i = self.id;
        self.id += 1;
        i
    }

    fn typ(&self, t: Typ) -> u32 {
        match t {
            Typ::F32 => self.t_f32,
            Typ::U32 => self.t_u32,
            Typ::I32 => self.t_i32,
            Typ::Bool => self.t_bool,
        }
    }

    fn ukazovatel(&mut self, trieda: u32, na: u32) -> u32 {
        if let Some(&u) = self.ukazovatele.get(&(trieda, na)) {
            return u;
        }
        let u = self.nove_id();
        instrukcia(&mut self.typy, OP_TYPE_POINTER, &[u, trieda, na]);
        self.ukazovatele.insert((trieda, na), u);
        u
    }

    fn konstanta(&mut self, typ: u32, bity: u32) -> u32 {
        if let Some(&c) = self.konstanty.get(&(typ, bity)) {
            return c;
        }
        let c = self.nove_id();
        instrukcia(&mut self.typy, OP_CONSTANT, &[typ, c, bity]);
        self.konstanty.insert((typ, bity), c);
        c
    }

    fn logicka(&mut self, b: bool) -> u32 {
        let slot = if b { &mut self.pravda } else { &mut self.nepravda };
        if let Some(c) = *slot {
            return c;
        }
        let c = self.id;
        self.id += 1;
        instrukcia(&mut self.typy, if b { OP_CONSTANT_TRUE } else { OP_CONSTANT_FALSE }, &[self.t_bool, c]);
        if b {
            self.pravda = Some(c);
        } else {
            self.nepravda = Some(c);
        }
        c
    }

    fn dekoruj(&mut self, ciel: u32, dekoracia: &[u32]) {
        let mut o = vec![ciel];
        o.extend_from_slice(dekoracia);
        instrukcia(&mut self.dekoracie, OP_DECORATE, &o);
    }

    /// Operácia s výsledkom: vráti nové id.
    fn op(&mut self, op: u16, typ: u32, operandy: &[u32]) -> u32 {
        let r = self.nove_id();
        let mut o = vec![typ, r];
        o.extend_from_slice(operandy);
        instrukcia(&mut self.telo, op, &o);
        r
    }

    fn presna(&mut self, op: u16, typ: u32, operandy: &[u32]) -> u32 {
        let r = self.op(op, typ, operandy);
        self.dekoruj(r, &[DEC_NO_CONTRACTION]);
        r
    }

    fn ext(&mut self, typ: u32, instr: u32, operandy: &[u32]) -> u32 {
        let mut o = vec![self.glsl, instr];
        o.extend_from_slice(operandy);
        self.op(OP_EXT_INST, typ, &o)
    }

    fn navestie(&mut self, l: u32) {
        instrukcia(&mut self.telo, OP_LABEL, &[l]);
    }

    fn vyraz(&mut self, v: &Vyraz) -> Vysledok<(u32, Typ)> {
        let t = self.k.typ_vyrazu(v)?;
        let tid = self.typ(t);
        let r = match v {
            Vyraz::F32(x) => self.konstanta(self.t_f32, x.to_bits()),
            Vyraz::U32(x) => self.konstanta(self.t_u32, *x),
            Vyraz::I32(x) => self.konstanta(self.t_i32, *x as u32),
            Vyraz::Bool(b) => self.logicka(*b),
            Vyraz::Premenna(p) => {
                let var = self.premenne[p.0];
                self.op(OP_LOAD, tid, &[var])
            }
            Vyraz::Konstanta(kid) => {
                let u = self.ukazovatel(SC_PUSH_CONSTANT, tid);
                let index = self.konstanta(self.t_u32, kid.0 as u32);
                let ptr = self.op(OP_ACCESS_CHAIN, u, &[self.konst_var, index]);
                self.op(OP_LOAD, tid, &[ptr])
            }
            Vyraz::IdVlakna(os) => {
                let v3 = self.op(OP_LOAD, self.t_v3u, &[self.id_vlakna]);
                self.op(OP_COMPOSITE_EXTRACT, self.t_u32, &[v3, *os as u32])
            }
            Vyraz::Citaj { vazba, index } => {
                let (i, _) = self.vyraz(index)?;
                let u = self.ukazovatel(SC_STORAGE_BUFFER, tid);
                let nula = self.konstanta(self.t_u32, 0);
                let ptr = self.op(OP_ACCESS_CHAIN, u, &[self.vazby[vazba.0], nula, i]);
                self.op(OP_LOAD, tid, &[ptr])
            }
            Vyraz::Dlzka(vazba) => self.op(OP_ARRAY_LENGTH, self.t_u32, &[self.vazby[vazba.0], 0]),
            Vyraz::Bin(op, a, b) => {
                let (x, ta) = self.vyraz(a)?;
                let (y, _) = self.vyraz(b)?;
                self.binarna(*op, ta, tid, x, y)
            }
            Vyraz::Un(op, a) => {
                let (x, ta) = self.vyraz(a)?;
                match (op, ta) {
                    (Op1::Zaporne, Typ::F32) => self.presna(OP_F_NEGATE, tid, &[x]),
                    (Op1::Zaporne, _) => self.op(OP_S_NEGATE, tid, &[x]),
                    (Op1::Nie, _) => self.op(OP_LOGICAL_NOT, tid, &[x]),
                    (Op1::Abs, Typ::F32) => self.ext(tid, GLSL_F_ABS, &[x]),
                    (Op1::Abs, _) => self.ext(tid, GLSL_S_ABS, &[x]),
                    (Op1::Odmocnina, _) => self.ext(tid, GLSL_SQRT, &[x]),
                    (Op1::Exp, _) => self.ext(tid, GLSL_EXP, &[x]),
                    (Op1::Ln, _) => self.ext(tid, GLSL_LOG, &[x]),
                    (Op1::Dole, _) => self.ext(tid, GLSL_FLOOR, &[x]),
                }
            }
            Vyraz::Prevod(ciel, a) => {
                let (x, z) = self.vyraz(a)?;
                match (z, *ciel) {
                    (z, c) if z == c => x,
                    (Typ::U32, Typ::F32) => self.op(OP_CONVERT_U_TO_F, tid, &[x]),
                    (Typ::I32, Typ::F32) => self.op(OP_CONVERT_S_TO_F, tid, &[x]),
                    (Typ::F32, Typ::U32) => self.op(OP_CONVERT_F_TO_U, tid, &[x]),
                    (Typ::F32, Typ::I32) => self.op(OP_CONVERT_F_TO_S, tid, &[x]),
                    _ => self.op(OP_BITCAST, tid, &[x]),
                }
            }
            Vyraz::Vyber(p, a, b) => {
                let (c, _) = self.vyraz(p)?;
                let (x, _) = self.vyraz(a)?;
                let (y, _) = self.vyraz(b)?;
                self.op(OP_SELECT, tid, &[c, x, y])
            }
        };
        Ok((r, t))
    }

    fn binarna(&mut self, op: Op2, ta: Typ, tid: u32, x: u32, y: u32) -> u32 {
        use Typ::*;
        let o = |f: u16, uu: u16, s: u16| match ta {
            F32 => f,
            U32 => uu,
            _ => s,
        };
        match op {
            Op2::Plus if ta == F32 => self.presna(OP_F_ADD, tid, &[x, y]),
            Op2::Minus if ta == F32 => self.presna(OP_F_SUB, tid, &[x, y]),
            Op2::Krat if ta == F32 => self.presna(OP_F_MUL, tid, &[x, y]),
            Op2::Deleno if ta == F32 => self.presna(OP_F_DIV, tid, &[x, y]),
            Op2::Plus => self.op(OP_I_ADD, tid, &[x, y]),
            Op2::Minus => self.op(OP_I_SUB, tid, &[x, y]),
            Op2::Krat => self.op(OP_I_MUL, tid, &[x, y]),
            Op2::Deleno => self.op(o(OP_F_DIV, OP_U_DIV, OP_S_DIV), tid, &[x, y]),
            Op2::Zvysok => self.op(o(OP_F_REM, OP_U_MOD, OP_S_REM), tid, &[x, y]),
            Op2::Min => {
                let i = match ta {
                    F32 => GLSL_F_MIN,
                    U32 => GLSL_U_MIN,
                    _ => GLSL_S_MIN,
                };
                self.ext(tid, i, &[x, y])
            }
            Op2::Max => {
                let i = match ta {
                    F32 => GLSL_F_MAX,
                    U32 => GLSL_U_MAX,
                    _ => GLSL_S_MAX,
                };
                self.ext(tid, i, &[x, y])
            }
            Op2::A => self.op(OP_LOGICAL_AND, tid, &[x, y]),
            Op2::Alebo => self.op(OP_LOGICAL_OR, tid, &[x, y]),
            Op2::Rovne if ta == Bool => self.op(OP_LOGICAL_EQUAL, tid, &[x, y]),
            Op2::Rozne if ta == Bool => self.op(OP_LOGICAL_NOT_EQUAL, tid, &[x, y]),
            Op2::Rovne => self.op(if ta == F32 { OP_F_ORD_EQUAL } else { OP_I_EQUAL }, tid, &[x, y]),
            Op2::Rozne => {
                self.op(if ta == F32 { OP_F_UNORD_NOT_EQUAL } else { OP_I_NOT_EQUAL }, tid, &[x, y])
            }
            Op2::Mensie => self.op(o(OP_F_ORD_LESS_THAN, OP_U_LESS_THAN, OP_S_LESS_THAN), tid, &[x, y]),
            Op2::MensieRovne => {
                self.op(o(OP_F_ORD_LESS_THAN_EQUAL, OP_U_LESS_THAN_EQUAL, OP_S_LESS_THAN_EQUAL), tid, &[x, y])
            }
            Op2::Vacsie => {
                self.op(o(OP_F_ORD_GREATER_THAN, OP_U_GREATER_THAN, OP_S_GREATER_THAN), tid, &[x, y])
            }
            Op2::VacsieRovne => self.op(
                o(OP_F_ORD_GREATER_THAN_EQUAL, OP_U_GREATER_THAN_EQUAL, OP_S_GREATER_THAN_EQUAL),
                tid,
                &[x, y],
            ),
        }
    }

    fn prikazy(&mut self, p: &[Prikaz]) -> Vysledok<()> {
        for prikaz in p {
            match prikaz {
                Prikaz::Nastav(pr, v) => {
                    let (x, _) = self.vyraz(v)?;
                    instrukcia(&mut self.telo, OP_STORE, &[self.premenne[pr.0], x]);
                }
                Prikaz::Zapis { vazba, index, hodnota } => {
                    let (i, _) = self.vyraz(index)?;
                    let (x, t) = self.vyraz(hodnota)?;
                    let tid = self.typ(t);
                    let u = self.ukazovatel(SC_STORAGE_BUFFER, tid);
                    let nula = self.konstanta(self.t_u32, 0);
                    let ptr = self.op(OP_ACCESS_CHAIN, u, &[self.vazby[vazba.0], nula, i]);
                    instrukcia(&mut self.telo, OP_STORE, &[ptr, x]);
                }
                Prikaz::Ak { podmienka, potom, inak } => {
                    let (c, _) = self.vyraz(podmienka)?;
                    let (l_potom, l_koniec) = (self.nove_id(), self.nove_id());
                    let l_inak = if inak.is_empty() { l_koniec } else { self.nove_id() };
                    instrukcia(&mut self.telo, OP_SELECTION_MERGE, &[l_koniec, 0]);
                    instrukcia(&mut self.telo, OP_BRANCH_CONDITIONAL, &[c, l_potom, l_inak]);
                    self.navestie(l_potom);
                    self.prikazy(potom)?;
                    instrukcia(&mut self.telo, OP_BRANCH, &[l_koniec]);
                    if !inak.is_empty() {
                        self.navestie(l_inak);
                        self.prikazy(inak)?;
                        instrukcia(&mut self.telo, OP_BRANCH, &[l_koniec]);
                    }
                    self.navestie(l_koniec);
                }
                Prikaz::Pre { premenna, od, po, telo } => {
                    let var = self.premenne[premenna.0];
                    let (zaciatok, _) = self.vyraz(od)?;
                    instrukcia(&mut self.telo, OP_STORE, &[var, zaciatok]);
                    let (hranica, _) = self.vyraz(po)?;
                    let [hlavicka, podmienka, l_telo, pokracuj, koniec] =
                        [self.nove_id(), self.nove_id(), self.nove_id(), self.nove_id(), self.nove_id()];
                    instrukcia(&mut self.telo, OP_BRANCH, &[hlavicka]);
                    self.navestie(hlavicka);
                    instrukcia(&mut self.telo, OP_LOOP_MERGE, &[koniec, pokracuj, 0]);
                    instrukcia(&mut self.telo, OP_BRANCH, &[podmienka]);
                    self.navestie(podmienka);
                    let c = self.op(OP_LOAD, self.t_u32, &[var]);
                    let menej = self.op(OP_U_LESS_THAN, self.t_bool, &[c, hranica]);
                    instrukcia(&mut self.telo, OP_BRANCH_CONDITIONAL, &[menej, l_telo, koniec]);
                    self.navestie(l_telo);
                    self.prikazy(telo)?;
                    instrukcia(&mut self.telo, OP_BRANCH, &[pokracuj]);
                    self.navestie(pokracuj);
                    let c2 = self.op(OP_LOAD, self.t_u32, &[var]);
                    let jedna = self.konstanta(self.t_u32, 1);
                    let dalsi = self.op(OP_I_ADD, self.t_u32, &[c2, jedna]);
                    instrukcia(&mut self.telo, OP_STORE, &[var, dalsi]);
                    instrukcia(&mut self.telo, OP_BRANCH, &[hlavicka]);
                    self.navestie(koniec);
                }
            }
        }
        Ok(())
    }
}

/// Preloží kernel do SPIR-V 1.3. Buffery sú v množine 0 s väzbami 0, 1, …; konštanty sú push constants.
pub fn do_spirv(k: &Kernel) -> Vysledok<Vec<u32>> {
    k.over()?;
    let mut p = Prekladac {
        k,
        id: 1,
        dekoracie: vec![],
        typy: vec![],
        telo: vec![],
        glsl: 0,
        t_void: 0,
        t_bool: 0,
        t_u32: 0,
        t_i32: 0,
        t_f32: 0,
        t_v3u: 0,
        ukazovatele: HashMap::new(),
        konstanty: HashMap::new(),
        pravda: None,
        nepravda: None,
        premenne: vec![],
        vazby: vec![],
        konst_var: 0,
        id_vlakna: 0,
    };
    p.glsl = p.nove_id();
    let funkcia = p.nove_id();
    p.t_void = p.nove_id();
    instrukcia(&mut p.typy, OP_TYPE_VOID, &[p.t_void]);
    p.t_bool = p.nove_id();
    instrukcia(&mut p.typy, OP_TYPE_BOOL, &[p.t_bool]);
    p.t_u32 = p.nove_id();
    instrukcia(&mut p.typy, OP_TYPE_INT, &[p.t_u32, 32, 0]);
    p.t_i32 = p.nove_id();
    instrukcia(&mut p.typy, OP_TYPE_INT, &[p.t_i32, 32, 1]);
    p.t_f32 = p.nove_id();
    instrukcia(&mut p.typy, OP_TYPE_FLOAT, &[p.t_f32, 32]);
    p.t_v3u = p.nove_id();
    instrukcia(&mut p.typy, OP_TYPE_VECTOR, &[p.t_v3u, p.t_u32, 3]);
    let t_fn = p.nove_id();
    instrukcia(&mut p.typy, OP_TYPE_FUNCTION, &[t_fn, p.t_void]);

    // vstavaný index vlákna
    let u_v3 = p.ukazovatel(SC_INPUT, p.t_v3u);
    p.id_vlakna = p.nove_id();
    instrukcia(&mut p.typy, OP_VARIABLE, &[u_v3, p.id_vlakna, SC_INPUT]);
    p.dekoruj(p.id_vlakna, &[DEC_BUILT_IN, BUILT_IN_GLOBAL_INVOCATION_ID]);

    // buffery: jeden typ štruktúry na typ prvku
    let mut struktury: HashMap<u32, u32> = HashMap::new();
    for (i, v) in k.vazby.iter().enumerate() {
        let prvok = p.typ(v.typ);
        let s = match struktury.get(&prvok) {
            Some(&s) => s,
            None => {
                let pole = p.nove_id();
                instrukcia(&mut p.typy, OP_TYPE_RUNTIME_ARRAY, &[pole, prvok]);
                p.dekoruj(pole, &[DEC_ARRAY_STRIDE, 4]);
                let s = p.nove_id();
                instrukcia(&mut p.typy, OP_TYPE_STRUCT, &[s, pole]);
                p.dekoruj(s, &[DEC_BLOCK]);
                instrukcia(&mut p.dekoracie, OP_MEMBER_DECORATE, &[s, 0, DEC_OFFSET, 0]);
                struktury.insert(prvok, s);
                s
            }
        };
        let u = p.ukazovatel(SC_STORAGE_BUFFER, s);
        let var = p.nove_id();
        instrukcia(&mut p.typy, OP_VARIABLE, &[u, var, SC_STORAGE_BUFFER]);
        p.dekoruj(var, &[DEC_DESCRIPTOR_SET, 0]);
        p.dekoruj(var, &[DEC_BINDING, i as u32]);
        if !v.pristup.zapisuje() {
            p.dekoruj(var, &[DEC_NON_WRITABLE]);
        }
        p.vazby.push(var);
    }

    // konštanty ako push constants
    if !k.konstanty.is_empty() {
        let clenovia: Vec<u32> = k.konstanty.iter().map(|c| p.typ(c.typ)).collect();
        let s = p.nove_id();
        let mut o = vec![s];
        o.extend_from_slice(&clenovia);
        instrukcia(&mut p.typy, OP_TYPE_STRUCT, &o);
        p.dekoruj(s, &[DEC_BLOCK]);
        for i in 0..clenovia.len() {
            instrukcia(&mut p.dekoracie, OP_MEMBER_DECORATE, &[s, i as u32, DEC_OFFSET, 4 * i as u32]);
        }
        let u = p.ukazovatel(SC_PUSH_CONSTANT, s);
        p.konst_var = p.nove_id();
        instrukcia(&mut p.typy, OP_VARIABLE, &[u, p.konst_var, SC_PUSH_CONSTANT]);
    }

    // funkcia
    instrukcia(&mut p.telo, OP_FUNCTION, &[p.t_void, funkcia, 0, t_fn]);
    let vstup = p.nove_id();
    p.navestie(vstup);
    for &t in &k.premenne {
        let tid = p.typ(t);
        let u = p.ukazovatel(SC_FUNCTION, tid);
        let var = p.nove_id();
        instrukcia(&mut p.telo, OP_VARIABLE, &[u, var, SC_FUNCTION]);
        p.premenne.push(var);
    }
    p.prikazy(&k.telo)?;
    instrukcia(&mut p.telo, OP_RETURN, &[]);
    instrukcia(&mut p.telo, OP_FUNCTION_END, &[]);

    // zostavenie modulu
    let mut m = vec![0x0723_0203, 0x0001_0300, 0, 0, 0];
    instrukcia(&mut m, OP_CAPABILITY, &[1]); // Shader
    let mut o = vec![p.glsl];
    o.extend(retazec("GLSL.std.450"));
    instrukcia(&mut m, OP_EXT_INST_IMPORT, &o);
    instrukcia(&mut m, OP_MEMORY_MODEL, &[0, 1]); // Logical, GLSL450
    let mut o = vec![5, funkcia]; // GLCompute
    o.extend(retazec("main"));
    o.push(p.id_vlakna);
    instrukcia(&mut m, OP_ENTRY_POINT, &o);
    let [x, y, z] = k.skupina;
    instrukcia(&mut m, OP_EXECUTION_MODE, &[funkcia, 17, x, y, z]); // LocalSize
    m.extend_from_slice(&p.dekoracie);
    m.extend_from_slice(&p.typy);
    m.extend_from_slice(&p.telo);
    m[3] = p.id;
    Ok(m)
}

/// SPIR-V ako bajty (malý endian).
pub fn do_bajtov(slova: &[u32]) -> Vec<u8> {
    slova.iter().flat_map(|w| w.to_le_bytes()).collect()
}
