//! Návrh dizajnu ORCHIDEA – obrázky obrazoviek na schválenie vlastníkom (nie je to ešte rozhranie programu).
//!
//! Kreslí sa vlastným kódom: písmo STONKA (`pismo`), vlastné vektorové ikony (ťahy na mriežke 20 × 20 bodov),
//! zaoblené plochy s vyhladením okrajov. Návrhové hodnoty (farby, rozostupy, zaoblenia, písmo) sú v `Tema`.
//!
//! `cargo run --release -p orchidea-pohlad --example navrh_vzhladu -- <priečinok>` → pohlad_tmava.png,
//! pohlad_svetla.png, pohlad_orezanie.png, lyra_tmava.png.

// kreslenie návrhu: veľa súradníc v jednom volaní je tu čitateľnejšie ako štruktúry
#![allow(clippy::too_many_arguments)]

use orchidea_pismo::{Farba, Pismo, Platno, zmiesaj};

/// Návrhové hodnoty jednej témy.
#[derive(Clone, Copy)]
struct Tema {
    /// okolie obrazu (neutrálne – nefarbí vnímanie fotky)
    okolie: Farba,
    /// panely a lišty
    plocha: Farba,
    /// ovládacie prvky
    prvok: Farba,
    prvok_pod_mysou: Farba,
    okraj: Farba,
    text: Farba,
    text_druhy: Farba,
    /// jediný výrazný akcent – orchideová fialová
    akcent: Farba,
    akcent_text: Farba,
    uspech: Farba,
    hviezda: Farba,
}

const TMAVA: Tema = Tema {
    okolie: 0x131417,
    plocha: 0x1B1D21,
    prvok: 0x262930,
    prvok_pod_mysou: 0x31353D,
    okraj: 0x2C3037,
    text: 0xE9EAEE,
    text_druhy: 0x9AA0AA,
    akcent: 0xA77BFF,
    akcent_text: 0x15101F,
    uspech: 0x4CC38A,
    hviezda: 0xF2C14E,
};

const SVETLA: Tema = Tema {
    okolie: 0xE6E7EB,
    plocha: 0xF7F7F9,
    prvok: 0xECEDF1,
    prvok_pod_mysou: 0xE1E3E8,
    okraj: 0xD7D9DF,
    text: 0x1C1E22,
    text_druhy: 0x5E646E,
    akcent: 0x7B4BDB,
    akcent_text: 0xFFFFFF,
    uspech: 0x1F9D63,
    hviezda: 0xD99A13,
};

// rozostupy (mriežka 4 bodov), zaoblenia, výšky
const R: f32 = 8.0;
const R_MALE: f32 = 6.0;
const LISTA: f32 = 52.0;
const TLACIDLO: f32 = 36.0;

struct Kreslic {
    p: Platno,
    pismo: Pismo,
}

impl Kreslic {
    fn novy(w: usize, h: usize, f: Farba) -> Kreslic {
        Kreslic { p: Platno::nove(w, h, f), pismo: Pismo::stonka() }
    }

    /// Vyplní tvar daný vzdialenosťou (záporná = vnútri) s vyhladením okraja; `a` = krytie 0–255.
    fn tvar(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, f: Farba, a: f32, d: impl Fn(f32, f32) -> f32) {
        let (w, h) = (self.p.sirka as i32, self.p.vyska as i32);
        for y in (y0.floor() as i32 - 1).max(0)..((y1.ceil() as i32) + 1).min(h) {
            for x in (x0.floor() as i32 - 1).max(0)..((x1.ceil() as i32) + 1).min(w) {
                let k = (0.5 - d(x as f32 + 0.5, y as f32 + 0.5)).clamp(0.0, 1.0);
                if k > 0.0 {
                    let i = y as usize * self.p.sirka + x as usize;
                    self.p.body[i] = zmiesaj(self.p.body[i], f, (k * a) as u8);
                }
            }
        }
    }

    fn obdlznik(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, f: Farba) {
        self.obdlznik_a(x, y, w, h, r, f, 255.0);
    }

    #[allow(clippy::too_many_arguments)]
    fn obdlznik_a(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, f: Farba, a: f32) {
        let (cx, cy, hx, hy) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        let r = r.min(hx).min(hy);
        self.tvar(x, y, x + w, y + h, f, a, |px, py| {
            let qx = (px - cx).abs() - (hx - r);
            let qy = (py - cy).abs() - (hy - r);
            (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r
        });
    }

    fn ram(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, hrubka: f32, f: Farba) {
        let (cx, cy, hx, hy) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        let r = r.min(hx).min(hy);
        self.tvar(x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0, f, 255.0, |px, py| {
            let qx = (px - cx).abs() - (hx - r);
            let qy = (py - cy).abs() - (hy - r);
            let d = (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r;
            d.abs() - hrubka / 2.0
        });
    }

    fn kruh(&mut self, cx: f32, cy: f32, r: f32, f: Farba) {
        self.tvar(cx - r, cy - r, cx + r, cy + r, f, 255.0, |x, y| (x - cx).hypot(y - cy) - r);
    }

    /// Úsečka s okrúhlymi koncami.
    fn ciara(&mut self, a: (f32, f32), b: (f32, f32), hrubka: f32, f: Farba) {
        let r = hrubka / 2.0;
        let (x0, x1) = (a.0.min(b.0) - r, a.0.max(b.0) + r);
        let (y0, y1) = (a.1.min(b.1) - r, a.1.max(b.1) + r);
        self.tvar(x0, y0, x1, y1, f, 255.0, |x, y| {
            let (vx, vy) = (b.0 - a.0, b.1 - a.1);
            let t = (((x - a.0) * vx + (y - a.1) * vy) / (vx * vx + vy * vy).max(1e-6)).clamp(0.0, 1.0);
            (x - a.0 - vx * t).hypot(y - a.1 - vy * t) - r
        });
    }

    fn text(&mut self, t: &str, x: f32, y: f32, velkost: f32, hrubka: f32, f: Farba) -> f32 {
        let pismo = &self.pismo;
        self.p.text(pismo, t, x, y, velkost, hrubka, f)
    }

    fn sirka(&self, t: &str, velkost: f32, hrubka: f32) -> f32 {
        self.pismo.sirka(t, velkost, hrubka)
    }

    /// Ikona z ťahov na mriežke 20 × 20 (ľavý horný roh x, y; veľkosť s).
    fn ikona(&mut self, ik: Ikona, x: f32, y: f32, s: f32, f: Farba) {
        let k = s / 20.0;
        let h = 1.7 * k.max(1.0);
        for ciara in ikona(ik) {
            for w in ciara.windows(2) {
                self.ciara((x + w[0].0 * k, y + w[0].1 * k), (x + w[1].0 * k, y + w[1].1 * k), h, f);
            }
        }
        if let Ikona::Prehrat = ik {
            // vyplnený trojuholník
            let (ax, ay, bx, by, cx2) = (x + 6.5 * k, y + 4.5 * k, x + 6.5 * k, y + 15.5 * k, x + 15.5 * k);
            let cy2 = y + 10.0 * k;
            self.tvar(ax, ay, cx2, by, f, 255.0, |px, py| {
                let e = |x0: f32, y0: f32, x1: f32, y1: f32| {
                    let (nx, ny) = (y1 - y0, x0 - x1);
                    let l = nx.hypot(ny);
                    ((px - x0) * nx + (py - y0) * ny) / l
                };
                e(ax, ay, bx, by).max(e(bx, by, cx2, cy2)).max(e(cx2, cy2, ax, ay))
            });
        }
    }

    /// Obraz (body 0xRRGGBB) zmenšený do obdĺžnika bilineárne.
    fn obraz(&mut self, o: &[Farba], ow: usize, oh: usize, x: f32, y: f32, w: f32, h: f32) {
        for py in 0..h as usize {
            for px in 0..w as usize {
                let (sx, sy) = (px as f32 * ow as f32 / w, py as f32 * oh as f32 / h);
                let (tx, ty) = (x as usize + px, y as usize + py);
                if tx < self.p.sirka && ty < self.p.vyska {
                    self.p.body[ty * self.p.sirka + tx] =
                        o[(sy as usize).min(oh - 1) * ow + (sx as usize).min(ow - 1)];
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Ikona {
    Otvorit,
    OtocVlavo,
    OtocVpravo,
    Zrkadlit,
    Orezat,
    Spat,
    Znova,
    Porovnat,
    Prezentacia,
    Katalog,
    Ulozit,
    Napoveda,
    Prehrat,
    Pauza,
    Predosla,
    Dalsia,
    Hlasitost,
    Nahodne,
    Opakovat,
    Hladat,
}

fn obluk(cx: f32, cy: f32, r: f32, od: f32, po: f32) -> Vec<(f32, f32)> {
    (0..=24)
        .map(|i| {
            let u = (od + (po - od) * i as f32 / 24.0).to_radians();
            (cx + r * u.cos(), cy + r * u.sin())
        })
        .collect()
}

fn ikona(ik: Ikona) -> Vec<Vec<(f32, f32)>> {
    match ik {
        Ikona::Otvorit => vec![vec![
            (2.5, 5.5),
            (8.0, 5.5),
            (10.0, 7.5),
            (17.5, 7.5),
            (17.5, 15.5),
            (2.5, 15.5),
            (2.5, 5.5),
        ]],
        Ikona::OtocVpravo => {
            vec![obluk(10.0, 11.0, 6.0, 200.0, 470.0), vec![(14.5, 3.5), (16.0, 7.3), (12.0, 7.8)]]
        }
        Ikona::OtocVlavo => {
            vec![obluk(10.0, 11.0, 6.0, -110.0, 160.0), vec![(5.5, 3.5), (4.0, 7.3), (8.0, 7.8)]]
        }
        Ikona::Zrkadlit => vec![
            vec![(10.0, 2.5), (10.0, 4.5)],
            vec![(10.0, 7.5), (10.0, 9.5)],
            vec![(10.0, 12.5), (10.0, 14.5)],
            vec![(10.0, 16.5), (10.0, 17.5)],
            vec![(7.5, 5.0), (2.5, 15.0), (7.5, 15.0), (7.5, 5.0)],
            vec![(12.5, 5.0), (17.5, 15.0), (12.5, 15.0), (12.5, 5.0)],
        ],
        Ikona::Orezat => {
            vec![vec![(5.5, 2.0), (5.5, 14.5), (18.0, 14.5)], vec![(2.0, 5.5), (14.5, 5.5), (14.5, 18.0)]]
        }
        Ikona::Spat => vec![obluk(11.0, 11.5, 5.5, 180.0, 400.0), vec![(3.0, 8.0), (5.5, 11.5), (8.5, 9.0)]],
        Ikona::Znova => {
            vec![obluk(9.0, 11.5, 5.5, -40.0, 180.0), vec![(17.0, 8.0), (14.5, 11.5), (11.5, 9.0)]]
        }
        Ikona::Porovnat => vec![
            vec![(2.5, 4.5), (17.5, 4.5), (17.5, 15.5), (2.5, 15.5), (2.5, 4.5)],
            vec![(10.0, 2.5), (10.0, 17.5)],
        ],
        Ikona::Prezentacia => vec![
            vec![(2.5, 4.0), (17.5, 4.0), (17.5, 14.0), (2.5, 14.0), (2.5, 4.0)],
            vec![(8.5, 6.5), (12.5, 9.0), (8.5, 11.5), (8.5, 6.5)],
            vec![(7.0, 17.0), (13.0, 17.0)],
        ],
        Ikona::Katalog => vec![
            vec![(3.0, 3.0), (8.5, 3.0), (8.5, 8.5), (3.0, 8.5), (3.0, 3.0)],
            vec![(11.5, 3.0), (17.0, 3.0), (17.0, 8.5), (11.5, 8.5), (11.5, 3.0)],
            vec![(3.0, 11.5), (8.5, 11.5), (8.5, 17.0), (3.0, 17.0), (3.0, 11.5)],
            vec![(11.5, 11.5), (17.0, 11.5), (17.0, 17.0), (11.5, 17.0), (11.5, 11.5)],
        ],
        Ikona::Ulozit => vec![
            vec![(10.0, 3.0), (10.0, 12.0)],
            vec![(6.0, 8.5), (10.0, 12.5), (14.0, 8.5)],
            vec![(3.0, 13.0), (3.0, 17.0), (17.0, 17.0), (17.0, 13.0)],
        ],
        Ikona::Napoveda => vec![
            obluk(10.0, 10.0, 7.5, 0.0, 360.0),
            obluk(10.0, 8.0, 2.5, 180.0, 400.0),
            vec![(11.9, 9.6), (10.0, 11.0), (10.0, 12.0)],
            vec![(10.0, 14.5), (10.0, 14.6)],
        ],
        Ikona::Prehrat => vec![],
        Ikona::Pauza => vec![vec![(7.0, 4.5), (7.0, 15.5)], vec![(13.0, 4.5), (13.0, 15.5)]],
        Ikona::Predosla => {
            vec![vec![(5.0, 4.5), (5.0, 15.5)], vec![(15.5, 4.5), (8.0, 10.0), (15.5, 15.5), (15.5, 4.5)]]
        }
        Ikona::Dalsia => {
            vec![vec![(15.0, 4.5), (15.0, 15.5)], vec![(4.5, 4.5), (12.0, 10.0), (4.5, 15.5), (4.5, 4.5)]]
        }
        Ikona::Hlasitost => vec![
            vec![(3.0, 7.5), (6.0, 7.5), (10.0, 4.0), (10.0, 16.0), (6.0, 12.5), (3.0, 12.5), (3.0, 7.5)],
            obluk(10.0, 10.0, 4.0, -45.0, 45.0),
            obluk(10.0, 10.0, 7.0, -45.0, 45.0),
        ],
        Ikona::Nahodne => vec![
            vec![(2.5, 6.0), (6.0, 6.0), (13.0, 14.0), (17.0, 14.0)],
            vec![(2.5, 14.0), (6.0, 14.0), (13.0, 6.0), (17.0, 6.0)],
            vec![(15.0, 4.0), (17.0, 6.0), (15.0, 8.0)],
            vec![(15.0, 12.0), (17.0, 14.0), (15.0, 16.0)],
        ],
        Ikona::Opakovat => vec![
            vec![(4.0, 9.0), (4.0, 6.0), (15.5, 6.0)],
            vec![(13.5, 4.0), (15.5, 6.0), (13.5, 8.0)],
            vec![(16.0, 11.0), (16.0, 14.0), (4.5, 14.0)],
            vec![(6.5, 12.0), (4.5, 14.0), (6.5, 16.0)],
        ],
        Ikona::Hladat => vec![obluk(8.5, 8.5, 5.5, 0.0, 360.0), vec![(12.5, 12.5), (17.0, 17.0)]],
    }
}

/// Skúšobná „fotka“: západ slnka nad horami a jazerom (vypočítaná, bez cudzích súborov).
fn fotka(w: usize, h: usize) -> Vec<Farba> {
    let mut o = vec![0; w * h];
    let horizont = h as f32 * 0.62;
    let mix = |a: Farba, b: Farba, t: f32| zmiesaj(a, b, (t.clamp(0.0, 1.0) * 255.0) as u8);
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
                // jazero s odrazom
                let t = (fy - horizont) / (h as f32 - horizont);
                let vlna = ((fy * 0.21).sin() * 0.5 + 0.5) * 0.06 * (1.0 - t);
                let odraz = o[((2.0 * horizont - fy).max(0.0) as usize).min(h - 1) * w + x];
                let blizko = (1.0 - (x as f32 - 0.66 * w as f32).abs() / (60.0 * (1.0 - t) + 1.0)).max(0.0);
                mix(mix(odraz, 0x15203A, 0.35 + t * 0.4), 0xFFD9A0, vlna * blizko * 4.0)
            };
            o[y * w + x] = c;
        }
    }
    o
}

fn tlacidlo_ikona(k: &mut Kreslic, t: &Tema, ik: Ikona, x: f32, y: f32, aktivne: bool) {
    if aktivne {
        k.obdlznik(x, y, TLACIDLO, TLACIDLO, R_MALE, t.prvok_pod_mysou);
    }
    k.ikona(ik, x + 8.0, y + 8.0, 20.0, t.text);
}

fn posuvnik(k: &mut Kreslic, t: &Tema, meno: &str, hodnota: f32, x: f32, y: f32, w: f32) {
    k.text(meno, x, y, 13.0, 1.0, t.text_druhy);
    let v = format!("{:+.0}", hodnota * 100.0);
    let vw = k.sirka(&v, 13.0, 1.0);
    k.text(&v, x + w - vw, y, 13.0, 1.0, t.text);
    let ty = y + 14.0;
    k.obdlznik(x, ty, w, 4.0, 2.0, t.prvok_pod_mysou);
    let stred = x + w / 2.0;
    let poz = stred + hodnota * w / 2.0;
    k.obdlznik(stred.min(poz), ty, (poz - stred).abs(), 4.0, 2.0, t.akcent);
    k.kruh(poz, ty + 2.0, 8.0, t.text);
    k.kruh(poz, ty + 2.0, 5.0, t.akcent);
}

fn histogram(k: &mut Kreslic, t: &Tema, o: &[Farba], x: f32, y: f32, w: f32, h: f32) {
    k.obdlznik(x, y, w, h, R, t.prvok);
    let mut hist = [[0u32; 64]; 3];
    for &c in o {
        for (i, s) in [16, 8, 0].iter().enumerate() {
            hist[i][((c >> s) & 0xFF) as usize / 4] += 1;
        }
    }
    let max = hist.iter().flat_map(|h| h.iter()).copied().max().unwrap_or(1) as f32;
    let farby = [0xF0605D, 0x5FD38A, 0x5B9BFF];
    let sb = (w - 16.0) / 64.0;
    for (i, hi) in hist.iter().enumerate() {
        for (j, &n) in hi.iter().enumerate() {
            let v = (n as f32 / max).sqrt() * (h - 16.0);
            k.obdlznik_a(x + 8.0 + j as f32 * sb, y + h - 8.0 - v, sb, v, 0.0, farby[i], 110.0);
        }
    }
}

/// Prehliadač a editor fotografií (Pohľad) – hlavné okno.
fn pohlad(t: &Tema, foto: &[Farba], fw: usize, fh: usize, orezanie: bool) -> Platno {
    let (w, h) = (1440usize, 900usize);
    let mut k = Kreslic::novy(w, h, t.okolie);
    let (wf, hf) = (w as f32, h as f32);
    // horná lišta: meno programu, nástroje v skupinách, vpravo Uložiť
    k.obdlznik(0.0, 0.0, wf, LISTA, 0.0, t.plocha);
    k.obdlznik(0.0, LISTA - 1.0, wf, 1.0, 0.0, t.okraj);
    k.kruh(30.0, 26.0, 10.0, t.akcent);
    let x = k.text("prehliadač a editor fotografií (Pohľad)", 48.0, 31.0, 14.0, 1.5, t.text);
    let mut x = x + 36.0;
    let skupiny: [&[(Ikona, bool)]; 4] = [
        &[(Ikona::Otvorit, false)],
        &[
            (Ikona::OtocVlavo, false),
            (Ikona::OtocVpravo, false),
            (Ikona::Zrkadlit, false),
            (Ikona::Orezat, orezanie),
        ],
        &[(Ikona::Spat, false), (Ikona::Znova, false)],
        &[(Ikona::Porovnat, false), (Ikona::Prezentacia, false), (Ikona::Katalog, false)],
    ];
    for s in skupiny {
        for &(ik, a) in s {
            tlacidlo_ikona(&mut k, t, ik, x, 8.0, a);
            x += TLACIDLO + 4.0;
        }
        x += 8.0;
        k.obdlznik(x - 6.0, 16.0, 1.0, 20.0, 0.0, t.okraj);
        x += 6.0;
    }
    tlacidlo_ikona(&mut k, t, Ikona::Napoveda, wf - 52.0, 8.0, false);
    let uw = 112.0;
    k.obdlznik(wf - 64.0 - uw, 8.0, uw, TLACIDLO, R_MALE, t.akcent);
    k.ikona(Ikona::Ulozit, wf - 64.0 - uw + 14.0, 16.0, 20.0, t.akcent_text);
    k.text("Uložiť", wf - 64.0 - uw + 42.0, 31.0, 14.0, 1.5, t.akcent_text);
    // pravý panel úprav
    let pw = 320.0;
    let px = wf - pw;
    k.obdlznik(px, LISTA, pw, hf - LISTA, 0.0, t.plocha);
    k.obdlznik(px, LISTA, 1.0, hf - LISTA, 0.0, t.okraj);
    let mut y = LISTA + 24.0;
    let nadpis = |k: &mut Kreslic, s: &str, y: f32| {
        k.text(s, px + 20.0, y, 14.0, 1.6, t.text);
    };
    if orezanie {
        nadpis(&mut k, "Orezanie", y + 10.0);
        y += 34.0;
        k.text("Pomer strán", px + 20.0, y + 10.0, 13.0, 1.0, t.text_druhy);
        y += 22.0;
        let mut cx = px + 20.0;
        for (i, p) in ["Voľný", "1:1", "4:3", "3:2", "16:9"].iter().enumerate() {
            let tw = k.sirka(p, 13.0, 1.2) + 18.0;
            k.obdlznik(cx, y, tw, 30.0, 15.0, if i == 3 { t.akcent } else { t.prvok });
            k.text(p, cx + 9.0, y + 20.0, 13.0, 1.2, if i == 3 { t.akcent_text } else { t.text });
            cx += tw + 4.0;
        }
        y += 48.0;
        for (i, (m, v)) in [("Šírka", "3 600"), ("Výška", "2 400")].iter().enumerate() {
            let bx = px + 20.0 + i as f32 * 144.0;
            k.text(m, bx, y + 10.0, 12.0, 1.0, t.text_druhy);
            k.obdlznik(bx, y + 18.0, 136.0, 34.0, R_MALE, t.prvok);
            k.text(v, bx + 12.0, y + 40.0, 14.0, 1.2, t.text);
            k.text("bodov", bx + 80.0, y + 40.0, 12.0, 1.0, t.text_druhy);
        }
        y += 74.0;
        k.obdlznik(px + 20.0, y, 280.0, 30.0, R_MALE, t.prvok);
        k.kruh(px + 34.0, y + 15.0, 4.0, t.uspech);
        k.text("Bezstratovo (mriežka 16 × 16)", px + 46.0, y + 20.0, 12.0, 1.0, t.text_druhy);
        y += 54.0;
        k.obdlznik(px + 20.0, y, 136.0, 38.0, R_MALE, t.akcent);
        k.text("Orezať", px + 58.0, y + 25.0, 14.0, 1.5, t.akcent_text);
        k.obdlznik(px + 164.0, y, 136.0, 38.0, R_MALE, t.prvok);
        k.text("Zrušiť", px + 206.0, y + 25.0, 14.0, 1.2, t.text);
        y += 62.0;
        for r in
            ["Ťahaním po fotke nakreslíte výber.", "Rohy a okraje sa dajú ťahať.", "Enter oreže, Esc zruší."]
        {
            k.text(r, px + 20.0, y, 12.0, 1.0, t.text_druhy);
            y += 20.0;
        }
    } else {
        nadpis(&mut k, "Histogram", y + 10.0);
        y += 26.0;
        histogram(&mut k, t, foto, px + 20.0, y, 280.0, 110.0);
        y += 136.0;
        nadpis(&mut k, "Svetlo", y + 10.0);
        y += 36.0;
        for (m, v) in [("Jas", 0.12f32), ("Kontrast", 0.2), ("Gama", -0.06)] {
            posuvnik(&mut k, t, m, v, px + 20.0, y, 280.0);
            y += 46.0;
        }
        nadpis(&mut k, "Farba", y + 6.0);
        y += 32.0;
        for (m, v) in [("Sýtosť", 0.08f32), ("Teplota", 0.15)] {
            posuvnik(&mut k, t, m, v, px + 20.0, y, 280.0);
            y += 46.0;
        }
        nadpis(&mut k, "Hodnotenie", y + 6.0);
        y += 22.0;
        for i in 0..5 {
            let f = if i < 4 { t.hviezda } else { t.prvok_pod_mysou };
            let (cx, cy) = (px + 30.0 + i as f32 * 26.0, y + 10.0);
            let body: Vec<(f32, f32)> = (0..10)
                .map(|j| {
                    let u = (-90.0 + j as f32 * 36.0f32).to_radians();
                    let r = if j % 2 == 0 { 10.0 } else { 4.2 };
                    (cx + r * u.cos(), cy + r * u.sin())
                })
                .collect();
            k.tvar(cx - 11.0, cy - 11.0, cx + 11.0, cy + 11.0, f, 255.0, |x, y| {
                let mut vnutri = false;
                let mut j = body.len() - 1;
                for i in 0..body.len() {
                    let (xi, yi, xj, yj) = (body[i].0, body[i].1, body[j].0, body[j].1);
                    if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                        vnutri = !vnutri;
                    }
                    j = i;
                }
                if vnutri { -1.0 } else { 1.0 }
            });
        }
        y += 40.0;
        nadpis(&mut k, "Kľúčové slová", y + 6.0);
        y += 22.0;
        let mut cx = px + 20.0;
        for s in ["západ slnka", "hory", "jazero", "+ pridať"] {
            let tw = k.sirka(s, 12.0, 1.0) + 20.0;
            if cx + tw > px + 300.0 {
                cx = px + 20.0;
                y += 32.0;
            }
            let pridat = s.starts_with('+');
            if pridat {
                k.ram(cx, y, tw, 26.0, 13.0, 1.2, t.okraj);
            } else {
                k.obdlznik(cx, y, tw, 26.0, 13.0, t.prvok);
            }
            k.text(s, cx + 10.0, y + 17.0, 12.0, 1.0, if pridat { t.text_druhy } else { t.text });
            cx += tw + 6.0;
        }
        y += 44.0;
        k.text("Canon EOS R6 · 24–105 mm", px + 20.0, y, 12.0, 1.0, t.text_druhy);
        k.text("ISO 200 · f/8 · 1/250 s · 12. 10. 2026", px + 20.0, y + 18.0, 12.0, 1.0, t.text_druhy);
    }
    // pás náhľadov dole
    let pas = 96.0;
    let py = hf - pas;
    k.obdlznik(0.0, py, px, pas, 0.0, t.plocha);
    k.obdlznik(0.0, py, px, 1.0, 0.0, t.okraj);
    for i in 0..9 {
        let (nx, nw, nh) = (24.0 + i as f32 * 120.0, 108.0, 68.0);
        let vybrany = i == 3;
        if vybrany {
            k.ram(nx - 3.0, py + 11.0, nw + 6.0, nh + 6.0, R + 2.0, 2.5, t.akcent);
        }
        // náhľad: posunutý výrez fotky
        let mut n = Kreslic::novy(nw as usize, nh as usize, 0);
        n.obraz(foto, fw, fh, 0.0, 0.0, nw, nh);
        let posun = (i as f32 * 0.07).sin() * 40.0;
        for (j, b) in n.p.body.iter_mut().enumerate() {
            let jas = 1.0 - (i % 3) as f32 * 0.12 + posun / 400.0;
            let c = |s: u32| (((*b >> s) & 0xFF) as f32 * jas).clamp(0.0, 255.0) as u32;
            *b = (c(16) << 16) | (c(8) << 8) | c(0);
            let _ = j;
        }
        k.obdlznik(nx, py + 14.0, nw, nh, R, t.prvok);
        for yy in 0..nh as usize {
            for xx in 0..nw as usize {
                let (gx, gy) = (nx as usize + xx, py as usize + 14 + yy);
                // zaoblené rohy náhľadu
                let (dx, dy) = (
                    (xx as f32 + 0.5).min(nw - xx as f32 - 0.5),
                    (yy as f32 + 0.5).min(nh - yy as f32 - 0.5),
                );
                if dx < R && dy < R && (R - dx).hypot(R - dy) > R {
                    continue;
                }
                k.p.body[gy * w + gx] = n.p.body[yy * nw as usize + xx];
            }
        }
    }
    // fotka v strede s ohraničením a plávajúcim ukazovateľom priblíženia
    let (ax, ay, aw, ah) = (0.0, LISTA, px, py - LISTA);
    let mierka = ((aw - 96.0) / fw as f32).min((ah - 72.0) / fh as f32);
    let (ow, oh) = (fw as f32 * mierka, fh as f32 * mierka);
    let (ox, oy) = (ax + (aw - ow) / 2.0, ay + (ah - oh) / 2.0);
    for i in 0..12 {
        let s = 12.0 - i as f32;
        k.obdlznik_a(ox - s, oy - s + 6.0, ow + 2.0 * s, oh + 2.0 * s, 6.0 + s, 0x000000, 6.0);
    }
    k.obraz(foto, fw, fh, ox, oy, ow, oh);
    if orezanie {
        let (sx, sy, sw, sh) = (ox + ow * 0.18, oy + oh * 0.14, ow * 0.6, oh * 0.6);
        for yy in oy as usize..(oy + oh) as usize {
            for xx in ox as usize..(ox + ow) as usize {
                let (fx, fy) = (xx as f32, yy as f32);
                if fx < sx || fx >= sx + sw || fy < sy || fy >= sy + sh {
                    let i = yy * w + xx;
                    k.p.body[i] = zmiesaj(k.p.body[i], 0x000000, 150);
                }
            }
        }
        for j in 1..3 {
            k.obdlznik_a(sx + sw * j as f32 / 3.0, sy, 1.0, sh, 0.0, 0xFFFFFF, 80.0);
            k.obdlznik_a(sx, sy + sh * j as f32 / 3.0, sw, 1.0, 0.0, 0xFFFFFF, 80.0);
        }
        k.ram(sx, sy, sw, sh, 0.0, 1.5, 0xFFFFFF);
        for (cx, cy) in [(sx, sy), (sx + sw, sy), (sx, sy + sh), (sx + sw, sy + sh)] {
            let (dx, dy) = (if cx > sx { -1.0 } else { 1.0 }, if cy > sy { -1.0 } else { 1.0 });
            k.ciara((cx, cy), (cx + dx * 22.0, cy), 4.0, 0xFFFFFF);
            k.ciara((cx, cy), (cx, cy + dy * 22.0), 4.0, 0xFFFFFF);
        }
        for (cx, cy) in
            [(sx + sw / 2.0, sy), (sx + sw / 2.0, sy + sh), (sx, sy + sh / 2.0), (sx + sw, sy + sh / 2.0)]
        {
            let horiz = cy == sy || cy == sy + sh;
            if horiz {
                k.ciara((cx - 10.0, cy), (cx + 10.0, cy), 4.0, 0xFFFFFF);
            } else {
                k.ciara((cx, cy - 10.0), (cx, cy + 10.0), 4.0, 0xFFFFFF);
            }
        }
        let s = "3 600 × 2 400";
        let tw = k.sirka(s, 13.0, 1.2) + 20.0;
        k.obdlznik_a(sx + 10.0, sy + 10.0, tw, 26.0, 13.0, 0x000000, 170.0);
        k.text(s, sx + 20.0, sy + 27.0, 13.0, 1.2, 0xFFFFFF);
    }
    // plávajúci ukazovateľ: meno, priblíženie
    let s = "zapad_slnka.jpg  ·  6000 × 4000  ·  24 %";
    let tw = k.sirka(s, 12.0, 1.0) + 28.0;
    k.obdlznik_a(ax + 20.0, py - 48.0, tw, 30.0, 15.0, t.plocha, 235.0);
    k.text(s, ax + 34.0, py - 28.0, 12.0, 1.0, t.text_druhy);
    k.p
}

/// Prehrávač médií LÝRA – hudba.
fn lyra(t: &Tema, foto: &[Farba], fw: usize, fh: usize) -> Platno {
    let (w, h) = (1440usize, 900usize);
    let mut k = Kreslic::novy(w, h, t.okolie);
    let (wf, hf) = (w as f32, h as f32);
    k.obdlznik(0.0, 0.0, wf, LISTA, 0.0, t.plocha);
    k.obdlznik(0.0, LISTA - 1.0, wf, 1.0, 0.0, t.okraj);
    k.kruh(30.0, 26.0, 10.0, t.akcent);
    k.text("prehrávač médií LÝRA", 48.0, 31.0, 14.0, 1.5, t.text);
    k.obdlznik(wf / 2.0 - 200.0, 10.0, 400.0, 32.0, 16.0, t.prvok);
    k.ikona(Ikona::Hladat, wf / 2.0 - 188.0, 16.0, 20.0, t.text_druhy);
    k.text("Hľadať v knižnici", wf / 2.0 - 158.0, 31.0, 13.0, 1.0, t.text_druhy);
    // obal a údaje
    let (ox, oy, os) = (64.0, LISTA + 48.0, 420.0);
    for i in 0..14 {
        let s = 14.0 - i as f32;
        k.obdlznik_a(ox - s, oy - s + 8.0, os + 2.0 * s, os + 2.0 * s, 12.0 + s, 0x000000, 6.0);
    }
    // obal: štvorcový výrez zo stredu fotky
    let x0 = (fw - fh) / 2;
    let stvorec: Vec<Farba> =
        (0..fh).flat_map(|y| foto[y * fw + x0..y * fw + x0 + fh].iter().copied()).collect();
    k.obraz(&stvorec, fh, fh, ox, oy, os, os);
    k.text("Svetlo nad jazerom", ox + os + 56.0, oy + 60.0, 30.0, 2.2, t.text);
    k.text("Orchestrálna suita · 2026", ox + os + 56.0, oy + 98.0, 16.0, 1.2, t.text_druhy);
    let mut cx = ox + os + 56.0;
    for s in ["FLAC", "24 b / 96 kHz", "ReplayGain -6,2 dB"] {
        let tw = k.sirka(s, 12.0, 1.0) + 20.0;
        k.obdlznik(cx, oy + 120.0, tw, 26.0, 13.0, t.prvok);
        k.text(s, cx + 10.0, oy + 137.0, 12.0, 1.0, t.text);
        cx += tw + 8.0;
    }
    // spektrum
    let (sx, sy, sw, sh) = (ox + os + 56.0, oy + 180.0, 520.0, 160.0);
    k.obdlznik(sx, sy, sw, sh, R, t.plocha);
    for i in 0..48 {
        let u = i as f32 / 48.0;
        let v = ((u * 9.0).sin() * 0.3 + 0.55 - u * 0.35 + (u * 31.0).sin() * 0.08).clamp(0.05, 0.95);
        let bh = v * (sh - 24.0);
        let f = zmiesaj(t.akcent, t.text, (u * 120.0) as u8);
        k.obdlznik(sx + 12.0 + i as f32 * 10.4, sy + sh - 12.0 - bh, 7.0, bh, 3.0, f);
    }
    // informácie o médiu (karty)
    let iy = sy + sh + 24.0;
    for (i, (m, v)) in
        [("Kontajner", "FLAC"), ("Kanály", "2.0 stereo"), ("Tok", "2 304 kb/s"), ("Výstup", "priamo")]
            .iter()
            .enumerate()
    {
        let ix = sx + i as f32 * 132.0;
        k.obdlznik(ix, iy, 124.0, 58.0, R, t.plocha);
        k.text(m, ix + 12.0, iy + 22.0, 12.0, 1.0, t.text_druhy);
        k.text(v, ix + 12.0, iy + 44.0, 14.0, 1.4, t.text);
    }
    // zoznam skladieb vpravo
    let lx = wf - 360.0;
    k.obdlznik(lx, LISTA, 360.0, hf - LISTA, 0.0, t.plocha);
    k.obdlznik(lx, LISTA, 1.0, hf - LISTA, 0.0, t.okraj);
    k.text("Zoznam skladieb", lx + 24.0, LISTA + 36.0, 14.0, 1.6, t.text);
    let skladby = [
        ("Ranná hmla", "3:41"),
        ("Svetlo nad jazerom", "5:12"),
        ("Horský vietor", "4:05"),
        ("Odraz", "6:30"),
        ("Nočné ticho", "4:48"),
        ("Návrat", "3:57"),
    ];
    for (i, (n, d)) in skladby.iter().enumerate() {
        let y = LISTA + 60.0 + i as f32 * 52.0;
        let hraje = i == 1;
        if hraje {
            k.obdlznik(lx + 12.0, y, 336.0, 46.0, R, t.prvok_pod_mysou);
            k.obdlznik(lx + 12.0, y + 10.0, 3.0, 26.0, 1.5, t.akcent);
        }
        if hraje {
            k.ikona(Ikona::Prehrat, lx + 22.0, y + 13.0, 20.0, t.akcent);
        } else {
            k.text(&format!("{}", i + 1), lx + 28.0, y + 29.0, 13.0, 1.0, t.text_druhy);
        }
        k.text(
            n,
            lx + 56.0,
            y + 29.0,
            14.0,
            if hraje { 1.6 } else { 1.0 },
            if hraje { t.akcent } else { t.text },
        );
        let dw = k.sirka(d, 13.0, 1.0);
        k.text(d, lx + 336.0 - dw, y + 29.0, 13.0, 1.0, t.text_druhy);
    }
    // ovládanie a časová os dole
    let oy2 = hf - 150.0;
    k.obdlznik(0.0, oy2, lx, 150.0, 0.0, t.plocha);
    k.obdlznik(0.0, oy2, lx, 1.0, 0.0, t.okraj);
    let (tx, tw) = (64.0, lx - 128.0);
    let ty = oy2 + 34.0;
    k.obdlznik(tx, ty, tw, 6.0, 3.0, t.prvok_pod_mysou);
    k.obdlznik(tx, ty, tw * 0.62, 6.0, 3.0, zmiesaj(t.prvok_pod_mysou, t.text_druhy, 90));
    k.obdlznik(tx, ty, tw * 0.38, 6.0, 3.0, t.akcent);
    for z in [0.21f32, 0.55, 0.8] {
        k.obdlznik(tx + tw * z - 1.0, ty - 3.0, 2.0, 12.0, 1.0, t.text_druhy);
    }
    k.kruh(tx + tw * 0.38, ty + 3.0, 9.0, t.text);
    k.text("1:58", tx, ty + 30.0, 12.0, 1.0, t.text_druhy);
    let z = "-3:14";
    let zw = k.sirka(z, 12.0, 1.0);
    k.text(z, tx + tw - zw, ty + 30.0, 12.0, 1.0, t.text_druhy);
    let cy = oy2 + 100.0;
    let cxm = tx + tw / 2.0;
    tlacidlo_ikona(&mut k, t, Ikona::Nahodne, cxm - 160.0, cy - 18.0, false);
    tlacidlo_ikona(&mut k, t, Ikona::Predosla, cxm - 92.0, cy - 18.0, false);
    k.kruh(cxm, cy, 30.0, t.akcent);
    k.ikona(Ikona::Pauza, cxm - 14.0, cy - 14.0, 28.0, t.akcent_text);
    tlacidlo_ikona(&mut k, t, Ikona::Dalsia, cxm + 56.0, cy - 18.0, false);
    tlacidlo_ikona(&mut k, t, Ikona::Opakovat, cxm + 124.0, cy - 18.0, true);
    // hlasitosť a úroveň
    let vx = tx + tw - 240.0;
    k.ikona(Ikona::Hlasitost, vx, cy - 10.0, 20.0, t.text);
    k.obdlznik(vx + 32.0, cy - 2.0, 140.0, 4.0, 2.0, t.prvok_pod_mysou);
    k.obdlznik(vx + 32.0, cy - 2.0, 98.0, 4.0, 2.0, t.akcent);
    k.kruh(vx + 130.0, cy, 7.0, t.text);
    k.text("-6 dB", vx + 184.0, cy + 5.0, 12.0, 1.0, t.text_druhy);
    for (i, v) in [0.72f32, 0.64].iter().enumerate() {
        let yy = cy + 16.0 + i as f32 * 6.0;
        k.obdlznik(vx + 32.0, yy, 140.0, 3.0, 1.5, t.prvok);
        k.obdlznik(vx + 32.0, yy, 140.0 * v, 3.0, 1.5, t.uspech);
    }
    k.p
}

fn uloz(p: &Platno, cesta: &std::path::Path) {
    let body: Vec<u8> = p.body.iter().flat_map(|&f| [(f >> 16) as u8, (f >> 8) as u8, f as u8]).collect();
    let o = orchidea_formaty::Obraz::novy(
        p.sirka,
        p.vyska,
        orchidea_formaty::obraz::Model::Rgb,
        false,
        orchidea_formaty::obraz::Vzorky::U8(body),
    )
    .expect("obraz");
    std::fs::write(cesta, orchidea_formaty::zapis(&o, orchidea_formaty::Format::Png).expect("PNG"))
        .expect("zápis");
    println!("{}", cesta.display());
}

fn main() {
    let ciel = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&ciel).expect("priečinok");
    let (fw, fh) = (900, 600);
    let foto = fotka(fw, fh);
    uloz(&pohlad(&TMAVA, &foto, fw, fh, false), &ciel.join("pohlad_tmava.png"));
    uloz(&pohlad(&SVETLA, &foto, fw, fh, false), &ciel.join("pohlad_svetla.png"));
    uloz(&pohlad(&TMAVA, &foto, fw, fh, true), &ciel.join("pohlad_orezanie.png"));
    uloz(&lyra(&TMAVA, &foto, fw, fh), &ciel.join("lyra_tmava.png"));
}
