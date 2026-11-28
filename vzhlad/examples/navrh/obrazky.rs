//! Vypočítané obrázky do návrhov (bez cudzích súborov): fotka, obaly albumov, plagáty filmov.

use orchidea_pismo::zmiesaj;
use orchidea_vzhlad::Farba;
use orchidea_vzhlad::kreslic::Kreslic;
use orchidea_vzhlad::miery::Rez;

/// Obraz v pamäti (0xRRGGBB po riadkoch).
pub struct Obr {
    pub w: usize,
    pub h: usize,
    pub body: Vec<Farba>,
}

fn mix(a: Farba, b: Farba, t: f32) -> Farba {
    zmiesaj(a, b, (t.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Skúšobná „fotka“: západ slnka nad horami a jazerom.
pub fn fotka(w: usize, h: usize) -> Obr {
    let mut o = vec![0; w * h];
    let horizont = h as f32 * 0.62;
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32);
            let hora = |faza: f32, vys: f32, amp: f32| {
                horizont
                    - vys
                    - amp
                        * ((fx * 7.0 + faza).sin() * 0.6
                            + (fx * 17.0 + faza * 2.0).sin() * 0.25
                            + (fx * 41.0).sin() * 0.1)
            };
            let c = if fy < horizont {
                let t = fy / horizont;
                let mut c = if t < 0.55 {
                    mix(0x1B2A5C, 0x6B3F8F, t / 0.55)
                } else {
                    mix(0x6B3F8F, 0xF29A5B, (t - 0.55) / 0.45)
                };
                let (sx, sy) = (0.66 * w as f32, horizont - h as f32 * 0.12);
                let d = (x as f32 - sx).hypot(fy - sy) / h as f32;
                c = mix(c, 0xFFD9A0, (0.35 - d).max(0.0) * 2.2);
                if d < 0.045 {
                    c = mix(c, 0xFFF1D6, ((0.045 - d) * 300.0).min(1.0));
                }
                if fy > hora(1.3, h as f32 * 0.16, h as f32 * 0.09) {
                    c = mix(0x3A2A55, 0x5A3B66, (fy - horizont * 0.6) / (horizont * 0.4));
                }
                if fy > hora(4.1, h as f32 * 0.06, h as f32 * 0.05) {
                    c = 0x231C36;
                }
                c
            } else {
                let t = (fy - horizont) / (h as f32 - horizont);
                let vlna = ((fy * 0.21).sin() * 0.5 + 0.5) * 0.06 * (1.0 - t);
                let odraz = o[((2.0 * horizont - fy).max(0.0) as usize).min(h - 1) * w + x];
                let blizko = (1.0 - (x as f32 - 0.66 * w as f32).abs() / (60.0 * (1.0 - t) + 1.0)).max(0.0);
                mix(mix(odraz, 0x15203A, 0.35 + t * 0.4), 0xFFD9A0, vlna * blizko * 4.0)
            };
            o[y * w + x] = c;
        }
    }
    Obr { w, h, body: o }
}

/// Farebné dvojice pre obaly a plagáty (odtiene, nie značky).
const PALETY: [(Farba, Farba, Farba); 10] = [
    (0x0F2A3D, 0x2F7F8F, 0xF2C14E),
    (0x2B1440, 0x8E3B8F, 0xFFB38A),
    (0x1E1E1E, 0x5A5A5A, 0xE84B3C),
    (0x0D3B2E, 0x3C8D5A, 0xE8E2C8),
    (0x3D1F0F, 0xB5653A, 0xF6E3C5),
    (0x101C4A, 0x3B5BDB, 0x9FE3FF),
    (0x3A0F1F, 0xC2185B, 0xFFD1DC),
    (0x22301A, 0x7A8F3C, 0xF0E68C),
    (0x1A1033, 0x5B3FA0, 0xA77BFF),
    (0x2E2E38, 0x8C8CA3, 0xF5F5F7),
];

/// Obal albumu `i` (štvorec `s × s`): prechod a geometrické tvary.
pub fn obal(i: usize, s: usize) -> Obr {
    let (a, b, c) = PALETY[i % PALETY.len()];
    let mut o = vec![0; s * s];
    let sf = s as f32;
    for y in 0..s {
        for x in 0..s {
            let (u, v) = (x as f32 / sf, y as f32 / sf);
            let mut f = mix(a, b, (u * 0.4 + v * 0.8) * 0.9);
            match i % 4 {
                0 => {
                    let d = (u - 0.62).hypot(v - 0.4);
                    if d < 0.22 {
                        f = mix(f, c, ((0.22 - d) * 60.0).min(1.0));
                    }
                }
                1 => {
                    for k in 0..5 {
                        let r = 0.12 + k as f32 * 0.08;
                        let d = ((u - 0.5).hypot(v - 0.55) - r).abs();
                        if d < 0.008 {
                            f = mix(f, c, 0.85);
                        }
                    }
                }
                2 => {
                    if (v - 0.35 - (u * 9.0).sin() * 0.05).abs() < 0.03
                        || (v - 0.6 - (u * 7.0 + 1.0).sin() * 0.06).abs() < 0.02
                    {
                        f = mix(f, c, 0.9);
                    }
                }
                _ => {
                    let pas = ((u + v) * 6.0).fract();
                    if pas < 0.12 {
                        f = mix(f, c, 0.55);
                    }
                }
            }
            o[y * s + x] = f;
        }
    }
    Obr { w: s, h: s, body: o }
}

/// Plagát filmu `i` (`w × h`) s názvom vpísaným písmom STONKA.
pub fn plagat(i: usize, w: usize, h: usize, nazov: &str) -> Obr {
    let (a, b, c) = PALETY[(i * 3 + 1) % PALETY.len()];
    let mut k = Kreslic::novy(w as f32, h as f32, 1.0, a);
    let (wf, hf) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / wf, y as f32 / hf);
            let mut f = mix(a, b, 1.0 - v * 1.1 + (u * 3.0 + i as f32).sin() * 0.08);
            let d = (u - 0.3 - (i % 3) as f32 * 0.2).hypot(v - 0.35);
            f = mix(f, c, (0.3 - d).max(0.0) * 2.0);
            if v > 0.62 + (u * 5.0 + i as f32).sin() * 0.04 {
                f = mix(f, 0x000000, 0.55);
            }
            k.p.body[y * w + x] = f;
        }
    }
    let mut rez = Rez { meno: "plagát", velkost: (hf * 0.075).max(10.0), hrubka: (hf * 0.009).max(1.4) };
    let tw = k.sirka(nazov, rez);
    if tw > wf * 0.86 {
        // názov sa musí zmestiť – zmenší sa celý rez (veľkosť aj ťah)
        let q = wf * 0.86 / tw;
        rez = Rez { meno: "plagát", velkost: rez.velkost * q, hrubka: (rez.hrubka * q).max(1.2) };
    }
    let tw = k.sirka(nazov, rez);
    let x = (wf - tw) / 2.0;
    k.text(nazov, x, hf * 0.8, rez, 0xF5F2EC);
    Obr { w, h, body: k.p.body }
}

/// Priemerná farba obrázka (na pozadie „teraz hrá“).
pub fn priemer(o: &Obr) -> Farba {
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for &f in &o.body {
        r += ((f >> 16) & 0xFF) as u64;
        g += ((f >> 8) & 0xFF) as u64;
        b += (f & 0xFF) as u64;
    }
    let n = o.body.len() as u64;
    (((r / n) as u32) << 16) | (((g / n) as u32) << 8) | (b / n) as u32
}

/// Histogram R, G, B (64 košov).
pub fn histogram(o: &Obr) -> [[u32; 64]; 3] {
    let mut hist = [[0u32; 64]; 3];
    for &c in &o.body {
        for (i, s) in [16, 8, 0].iter().enumerate() {
            hist[i][((c >> s) & 0xFF) as usize / 4] += 1;
        }
    }
    hist
}

/// Štvorcový výrez zo stredu obrázka (obal albumu z fotky).
pub fn stvorec(o: &Obr) -> Obr {
    let s = o.w.min(o.h);
    let (x0, y0) = ((o.w - s) / 2, (o.h - s) / 2);
    let body =
        (0..s).flat_map(|y| o.body[(y0 + y) * o.w + x0..(y0 + y) * o.w + x0 + s].iter().copied()).collect();
    Obr { w: s, h: s, body }
}
