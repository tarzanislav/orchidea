//! Vlastné prekladače kernelu do textových jazykov shaderov: HLSL (Direct3D 12), MSL (Metal), WGSL (WebGPU).
//!
//! Textový shader preloží do strojového kódu až systém (prekladač Direct3D vo Windows, Metal v systémoch
//! Apple, prehliadač pri WebGPU) – to sú rozhrania operačného systému (trieda B kódexu), nie cudzí kód
//! v ORCHIDEI. Výstupy overujú v CI nezávislé nástroje.

use crate::ir::{Kernel, Op1, Op2, Prikaz, Typ, Vyraz};
use orchidea_jadro::Vysledok;
use std::fmt::Write;

/// Jazyk výstupu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jazyk {
    Hlsl,
    Msl,
    Wgsl,
}

fn meno_typu(j: Jazyk, t: Typ) -> &'static str {
    match (j, t) {
        (Jazyk::Wgsl, Typ::F32) => "f32",
        (Jazyk::Wgsl, Typ::U32) => "u32",
        (Jazyk::Wgsl, Typ::I32) => "i32",
        (_, Typ::F32) => "float",
        (_, Typ::U32) => "uint",
        (_, Typ::I32) => "int",
        (_, Typ::Bool) => "bool",
    }
}

fn nula(j: Jazyk, t: Typ) -> &'static str {
    match (j, t) {
        (_, Typ::F32) => "0.0f",
        (_, Typ::U32) => "0u",
        (Jazyk::Wgsl, Typ::I32) => "0i",
        (_, Typ::I32) => "0",
        (_, Typ::Bool) => "false",
    }
}

fn f32_text(x: f32) -> String {
    let mut t = format!("{x:?}");
    if !t.contains('.') && !t.contains('e') {
        t.push_str(".0");
    }
    if x.is_sign_negative() { format!("({t}f)") } else { format!("{t}f") }
}

struct Pisar<'k> {
    j: Jazyk,
    k: &'k Kernel,
    vystup: String,
    odsadenie: usize,
    hranice: usize,
}

impl Pisar<'_> {
    fn riadok(&mut self, t: &str) {
        for _ in 0..self.odsadenie {
            self.vystup.push_str("    ");
        }
        self.vystup.push_str(t);
        self.vystup.push('\n');
    }

    fn vyraz(&self, v: &Vyraz) -> Vysledok<String> {
        let j = self.j;
        Ok(match v {
            Vyraz::F32(x) => f32_text(*x),
            Vyraz::U32(x) => format!("{x}u"),
            Vyraz::I32(x) => match (j, *x) {
                (Jazyk::Wgsl, i32::MIN) => "(-2147483647i - 1i)".into(),
                (_, i32::MIN) => "(-2147483647 - 1)".into(),
                (Jazyk::Wgsl, x) => format!("({x}i)"),
                (_, x) => format!("({x})"),
            },
            Vyraz::Bool(b) => b.to_string(),
            Vyraz::Premenna(p) => format!("p{}", p.0),
            Vyraz::Konstanta(c) => match j {
                Jazyk::Hlsl => format!("k{}", c.0),
                _ => format!("konst.k{}", c.0),
            },
            Vyraz::IdVlakna(os) => format!("id_vlakna.{}", ["x", "y", "z"][*os as usize]),
            Vyraz::Citaj { vazba, index } => format!("v{}[{}]", vazba.0, self.vyraz(index)?),
            Vyraz::Dlzka(vazba) => match j {
                Jazyk::Hlsl => format!("dlzka{}", vazba.0),
                Jazyk::Msl => format!("dlzky[{}]", vazba.0),
                Jazyk::Wgsl => format!("arrayLength(&v{})", vazba.0),
            },
            Vyraz::Bin(op, a, b) => {
                let ta = self.k.typ_vyrazu(a)?;
                let (x, y) = (self.vyraz(a)?, self.vyraz(b)?);
                let znak = match op {
                    Op2::Plus => "+",
                    Op2::Minus => "-",
                    Op2::Krat => "*",
                    Op2::Deleno => "/",
                    Op2::Zvysok if ta == Typ::F32 && j != Jazyk::Wgsl => {
                        return Ok(format!("fmod({x}, {y})"));
                    }
                    Op2::Zvysok => "%",
                    Op2::Min => return Ok(format!("min({x}, {y})")),
                    Op2::Max => return Ok(format!("max({x}, {y})")),
                    Op2::Mensie => "<",
                    Op2::MensieRovne => "<=",
                    Op2::Vacsie => ">",
                    Op2::VacsieRovne => ">=",
                    Op2::Rovne => "==",
                    Op2::Rozne => "!=",
                    Op2::A => "&&",
                    Op2::Alebo => "||",
                };
                format!("({x} {znak} {y})")
            }
            Vyraz::Un(op, a) => {
                let x = self.vyraz(a)?;
                match op {
                    Op1::Zaporne => format!("(-{x})"),
                    Op1::Nie => format!("(!{x})"),
                    Op1::Abs => format!("abs({x})"),
                    Op1::Odmocnina => format!("sqrt({x})"),
                    Op1::Exp => format!("exp({x})"),
                    Op1::Ln => format!("log({x})"),
                    Op1::Dole => format!("floor({x})"),
                }
            }
            Vyraz::Prevod(ciel, a) => {
                let z = self.k.typ_vyrazu(a)?;
                let x = self.vyraz(a)?;
                if z == *ciel {
                    return Ok(x);
                }
                let bitovy = z != Typ::F32 && *ciel != Typ::F32;
                let t = meno_typu(j, *ciel);
                match (j, bitovy) {
                    (Jazyk::Hlsl, true) => format!("as{t}({x})"),
                    (Jazyk::Hlsl, false) => format!("(({t})({x}))"),
                    (Jazyk::Msl, true) => format!("as_type<{t}>({x})"),
                    (Jazyk::Msl, false) => format!("{t}({x})"),
                    (Jazyk::Wgsl, true) => format!("bitcast<{t}>({x})"),
                    (Jazyk::Wgsl, false) => format!("{t}({x})"),
                }
            }
            Vyraz::Vyber(p, a, b) => {
                let (c, x, y) = (self.vyraz(p)?, self.vyraz(a)?, self.vyraz(b)?);
                match j {
                    Jazyk::Wgsl => format!("select({y}, {x}, {c})"),
                    _ => format!("({c} ? {x} : {y})"),
                }
            }
        })
    }

    fn prikazy(&mut self, p: &[Prikaz]) -> Vysledok<()> {
        for prikaz in p {
            match prikaz {
                Prikaz::Nastav(pr, v) => {
                    let t = format!("p{} = {};", pr.0, self.vyraz(v)?);
                    self.riadok(&t);
                }
                Prikaz::Zapis { vazba, index, hodnota } => {
                    let typ = self.k.vazby[vazba.0].typ;
                    let (i, h) = (self.vyraz(index)?, self.vyraz(hodnota)?);
                    if self.j == Jazyk::Hlsl && typ == Typ::F32 {
                        // „precise“ zakáže spájanie násobenia a sčítania v celom výpočte hodnoty
                        let t = format!("{{ precise float hodnota = {h}; v{}[{i}] = hodnota; }}", vazba.0);
                        self.riadok(&t);
                    } else {
                        let t = format!("v{}[{i}] = {h};", vazba.0);
                        self.riadok(&t);
                    }
                }
                Prikaz::Ak { podmienka, potom, inak } => {
                    let t = format!("if ({}) {{", self.vyraz(podmienka)?);
                    self.riadok(&t);
                    self.odsadenie += 1;
                    self.prikazy(potom)?;
                    self.odsadenie -= 1;
                    if inak.is_empty() {
                        self.riadok("}");
                    } else {
                        self.riadok("} else {");
                        self.odsadenie += 1;
                        self.prikazy(inak)?;
                        self.odsadenie -= 1;
                        self.riadok("}");
                    }
                }
                Prikaz::Pre { premenna, od, po, telo } => {
                    let h = self.hranice;
                    self.hranice += 1;
                    let (od, po) = (self.vyraz(od)?, self.vyraz(po)?);
                    let n = premenna.0;
                    self.riadok("{");
                    self.odsadenie += 1;
                    let t = match self.j {
                        Jazyk::Wgsl => format!("let hranica{h}: u32 = {po};"),
                        _ => format!("uint hranica{h} = {po};"),
                    };
                    self.riadok(&t);
                    let t = format!("for (p{n} = {od}; p{n} < hranica{h}; p{n} = p{n} + 1u) {{");
                    self.riadok(&t);
                    self.odsadenie += 1;
                    self.prikazy(telo)?;
                    self.odsadenie -= 1;
                    self.riadok("}");
                    self.odsadenie -= 1;
                    self.riadok("}");
                }
            }
        }
        Ok(())
    }
}

/// Preloží kernel do zvoleného textového jazyka. Vstupný bod sa volá `hlavna`.
///
/// Rozloženie väzieb:
/// - HLSL: buffery `u0`, `u1`, … (RWStructuredBuffer), konštanty v `b0` (32-bitové koreňové konštanty);
/// - MSL: buffery `[[buffer(0…n−1)]]`, dĺžky bufferov `[[buffer(n)]]`, konštanty `[[buffer(n+1)]]`;
/// - WGSL: buffery `@group(0) @binding(0…n−1)`, konštanty `@binding(n)` (uniform).
pub fn do_textu(k: &Kernel, j: Jazyk) -> Vysledok<String> {
    k.over()?;
    let mut w = Pisar { j, k, vystup: String::new(), odsadenie: 0, hranice: 0 };
    let n = k.vazby.len();
    let [x, y, z] = k.skupina;
    let _ = writeln!(w.vystup, "// ORCHIDEA – kernel „{}“ (vygenerované vlastným prekladačom)", k.meno);
    match j {
        Jazyk::Hlsl => {
            for (i, v) in k.vazby.iter().enumerate() {
                let _ =
                    writeln!(w.vystup, "RWStructuredBuffer<{}> v{i} : register(u{i});", meno_typu(j, v.typ));
            }
            if !k.konstanty.is_empty() {
                w.vystup.push_str("cbuffer Konstanty : register(b0) {\n");
                for (i, c) in k.konstanty.iter().enumerate() {
                    let _ = writeln!(w.vystup, "    {} k{i};", meno_typu(j, c.typ));
                }
                w.vystup.push_str("};\n");
            }
            let _ = writeln!(w.vystup, "[numthreads({x}, {y}, {z})]");
            w.vystup.push_str("void hlavna(uint3 id_vlakna : SV_DispatchThreadID) {\n");
        }
        Jazyk::Msl => {
            w.vystup.push_str("#include <metal_stdlib>\nusing namespace metal;\n");
            if !k.konstanty.is_empty() {
                w.vystup.push_str("struct Konstanty {\n");
                for (i, c) in k.konstanty.iter().enumerate() {
                    let _ = writeln!(w.vystup, "    {} k{i};", meno_typu(j, c.typ));
                }
                w.vystup.push_str("};\n");
            }
            w.vystup.push_str("kernel void hlavna(\n");
            for (i, v) in k.vazby.iter().enumerate() {
                let c = if v.pristup.zapisuje() { "" } else { "const " };
                let _ = writeln!(w.vystup, "    {c}device {}* v{i} [[buffer({i})]],", meno_typu(j, v.typ));
            }
            let _ = writeln!(w.vystup, "    constant uint* dlzky [[buffer({n})]],");
            if !k.konstanty.is_empty() {
                let _ = writeln!(w.vystup, "    constant Konstanty& konst [[buffer({})]],", n + 1);
            }
            w.vystup.push_str("    uint3 id_vlakna [[thread_position_in_grid]]) {\n");
            w.vystup.push_str("    (void)dlzky;\n");
        }
        Jazyk::Wgsl => {
            if !k.konstanty.is_empty() {
                w.vystup.push_str("struct Konstanty {\n");
                for (i, c) in k.konstanty.iter().enumerate() {
                    let _ = writeln!(w.vystup, "    k{i}: {},", meno_typu(j, c.typ));
                }
                w.vystup.push_str("}\n");
                let _ = writeln!(w.vystup, "@group(0) @binding({n}) var<uniform> konst: Konstanty;");
            }
            for (i, v) in k.vazby.iter().enumerate() {
                let p = if v.pristup.zapisuje() { "read_write" } else { "read" };
                let _ = writeln!(
                    w.vystup,
                    "@group(0) @binding({i}) var<storage, {p}> v{i}: array<{}>;",
                    meno_typu(j, v.typ)
                );
            }
            let _ = writeln!(w.vystup, "@compute @workgroup_size({x}, {y}, {z})");
            w.vystup.push_str("fn hlavna(@builtin(global_invocation_id) id_vlakna: vec3<u32>) {\n");
        }
    }
    w.odsadenie = 1;
    for (i, t) in k.premenne.iter().enumerate() {
        let r = match j {
            Jazyk::Hlsl if *t == Typ::F32 => format!("precise float p{i} = 0.0f;"),
            Jazyk::Wgsl => format!("var p{i}: {} = {};", meno_typu(j, *t), nula(j, *t)),
            _ => format!("{} p{i} = {};", meno_typu(j, *t), nula(j, *t)),
        };
        w.riadok(&r);
    }
    if j == Jazyk::Hlsl {
        for i in 0..n {
            w.riadok(&format!("uint dlzka{i}; {{ uint krok; v{i}.GetDimensions(dlzka{i}, krok); }}"));
        }
    }
    w.prikazy(&k.telo)?;
    w.vystup.push_str("}\n");
    Ok(w.vystup)
}
