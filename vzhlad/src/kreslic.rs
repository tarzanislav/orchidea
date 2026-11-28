//! Kreslenie dizajnového systému do plátna – tvary určené vzdialenosťou (zaoblené obdĺžniky, kruhy, úsečky)
//! s vyhladeným okrajom, písmo STONKA, ikony a obraz. Všetky súradnice sú v **bodoch**; plátno má
//! `mierka`-krát viac obrazových bodov (100 % = 1, 200 % = 2 pre HiDPI). Čo sa nakreslí tu, uvidí človek
//! rovnako v návrhu aj v programe.

use crate::ikony::Ikona;
use crate::miery::{Rez, TAH_IKONY};
use orchidea_pismo::{Farba, Pismo, Platno, zmiesaj};

/// Kreslič s plátnom, písmom a mierkou.
#[derive(Debug)]
pub struct Kreslic {
    pub p: Platno,
    pub pismo: Pismo,
    pub mierka: f32,
}

impl Kreslic {
    /// Plátno `w × h` bodov v mierke `mierka`, vyplnené farbou `f`.
    pub fn novy(w: f32, h: f32, mierka: f32, f: Farba) -> Kreslic {
        let (pw, ph) = ((w * mierka).round() as usize, (h * mierka).round() as usize);
        Kreslic { p: Platno::nove(pw, ph, f), pismo: Pismo::stonka(), mierka }
    }

    /// Šírka a výška v bodoch.
    pub fn rozmer(&self) -> (f32, f32) {
        (self.p.sirka as f32 / self.mierka, self.p.vyska as f32 / self.mierka)
    }

    /// Vyplní tvar daný vzdialenosťou v obrazových bodoch (záporná = vnútri); `a` = krytie 0–255.
    #[allow(clippy::too_many_arguments)]
    fn tvar(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, f: Farba, a: f32, d: impl Fn(f32, f32) -> f32) {
        let (w, h) = (self.p.sirka as i32, self.p.vyska as i32);
        for y in (y0.floor() as i32 - 1).max(0)..((y1.ceil() as i32) + 1).min(h) {
            for x in (x0.floor() as i32 - 1).max(0)..((x1.ceil() as i32) + 1).min(w) {
                let k = (0.5 - d(x as f32 + 0.5, y as f32 + 0.5)).clamp(0.0, 1.0);
                if k > 0.0 {
                    let i = y as usize * self.p.sirka + x as usize;
                    self.p.body[i] = zmiesaj(self.p.body[i], f, (k * a).round() as u8);
                }
            }
        }
    }

    /// Zaoblený obdĺžnik.
    pub fn obdlznik(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, f: Farba) {
        self.obdlznik_a(x, y, w, h, r, f, 255.0);
    }

    /// Zaoblený obdĺžnik s krytím `a` (0–255).
    #[allow(clippy::too_many_arguments)]
    pub fn obdlznik_a(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, f: Farba, a: f32) {
        let m = self.mierka;
        let (x, y, w, h, r) = (x * m, y * m, w * m, h * m, r * m);
        let (cx, cy, hx, hy) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        let r = r.min(hx).min(hy);
        self.tvar(x, y, x + w, y + h, f, a, |px, py| zaobleny(px - cx, py - cy, hx, hy, r));
    }

    /// Obrys zaobleného obdĺžnika hrúbky `hrubka` (stred čiary na hrane).
    #[allow(clippy::too_many_arguments)]
    pub fn ram(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, hrubka: f32, f: Farba) {
        let m = self.mierka;
        let (x, y, w, h, r, t) = (x * m, y * m, w * m, h * m, r * m, hrubka * m);
        let (cx, cy, hx, hy) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        let r = r.min(hx).min(hy);
        self.tvar(x - t, y - t, x + w + t, y + h + t, f, 255.0, |px, py| {
            zaobleny(px - cx, py - cy, hx, hy, r).abs() - t / 2.0
        });
    }

    /// Obrys vnútri obdĺžnika (celá hrúbka dnu) – okraj poľa, rámik vybranej dlaždice.
    #[allow(clippy::too_many_arguments)]
    pub fn ram_dnu(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, hrubka: f32, f: Farba) {
        let t = hrubka / 2.0;
        self.ram(x + t, y + t, w - hrubka, h - hrubka, (r - t).max(0.0), hrubka, f);
    }

    pub fn kruh(&mut self, cx: f32, cy: f32, r: f32, f: Farba) {
        let m = self.mierka;
        let (cx, cy, r) = (cx * m, cy * m, r * m);
        self.tvar(cx - r, cy - r, cx + r, cy + r, f, 255.0, |x, y| (x - cx).hypot(y - cy) - r);
    }

    /// Úsečka s okrúhlymi koncami.
    pub fn ciara(&mut self, a: (f32, f32), b: (f32, f32), hrubka: f32, f: Farba) {
        let m = self.mierka;
        self.ciara_px((a.0 * m, a.1 * m), (b.0 * m, b.1 * m), hrubka * m, f);
    }

    fn ciara_px(&mut self, a: (f32, f32), b: (f32, f32), hrubka: f32, f: Farba) {
        let r = hrubka / 2.0;
        let (x0, x1) = (a.0.min(b.0) - r, a.0.max(b.0) + r);
        let (y0, y1) = (a.1.min(b.1) - r, a.1.max(b.1) + r);
        self.tvar(x0, y0, x1, y1, f, 255.0, |x, y| {
            let (vx, vy) = (b.0 - a.0, b.1 - a.1);
            let t = (((x - a.0) * vx + (y - a.1) * vy) / (vx * vx + vy * vy).max(1e-6)).clamp(0.0, 1.0);
            (x - a.0 - vx * t).hypot(y - a.1 - vy * t) - r
        });
    }

    /// Vyplnený mnohouholník (body v obrazových bodoch) s vyhladeným okrajom.
    fn mnohouholnik_px(&mut self, body: &[(f32, f32)], f: Farba) {
        let x0 = body.iter().map(|b| b.0).fold(f32::MAX, f32::min);
        let x1 = body.iter().map(|b| b.0).fold(f32::MIN, f32::max);
        let y0 = body.iter().map(|b| b.1).fold(f32::MAX, f32::min);
        let y1 = body.iter().map(|b| b.1).fold(f32::MIN, f32::max);
        self.tvar(x0, y0, x1, y1, f, 255.0, |x, y| {
            let mut vnutri = false;
            let mut d = f32::MAX;
            let mut j = body.len() - 1;
            for i in 0..body.len() {
                let (a, b) = (body[j], body[i]);
                if (b.1 > y) != (a.1 > y) && x < (a.0 - b.0) * (y - b.1) / (a.1 - b.1) + b.0 {
                    vnutri = !vnutri;
                }
                let (vx, vy) = (b.0 - a.0, b.1 - a.1);
                let t = (((x - a.0) * vx + (y - a.1) * vy) / (vx * vx + vy * vy).max(1e-6)).clamp(0.0, 1.0);
                d = d.min((x - a.0 - vx * t).hypot(y - a.1 - vy * t));
                j = i;
            }
            if vnutri { -d } else { d }
        });
    }

    /// Mäkký tieň pod plávajúcim prvkom (sila = krytie jednej vrstvy, 0 = bez tieňa).
    #[allow(clippy::too_many_arguments)]
    pub fn tien(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, rozsah: f32, sila: u8) {
        if sila == 0 {
            return;
        }
        let n = (rozsah as usize).max(1);
        for i in 0..n {
            let s = rozsah - i as f32;
            self.obdlznik_a(
                x - s,
                y - s + rozsah / 2.0,
                w + 2.0 * s,
                h + 2.0 * s,
                r + s,
                0x000000,
                sila as f32,
            );
        }
    }

    /// Text na základnej čiare `zakladna`; vráti koniec textu (v bodoch).
    pub fn text(&mut self, t: &str, x: f32, zakladna: f32, rez: Rez, f: Farba) -> f32 {
        let m = self.mierka;
        let pismo = &self.pismo;
        self.p.text(pismo, t, x * m, zakladna * m, rez.velkost * m, rez.hrubka * m, f) / m
    }

    /// Šírka textu (v bodoch).
    pub fn sirka(&self, t: &str, rez: Rez) -> f32 {
        self.pismo.sirka(t, rez.velkost * self.mierka, rez.hrubka * self.mierka) / self.mierka
    }

    /// Text zarovnaný vpravo ku `x`.
    pub fn text_vpravo(&mut self, t: &str, x: f32, zakladna: f32, rez: Rez, f: Farba) {
        let w = self.sirka(t, rez);
        self.text(t, x - w, zakladna, rez, f);
    }

    /// Text v strede šírky `w` od `x`.
    pub fn text_v_strede(&mut self, t: &str, x: f32, w: f32, zakladna: f32, rez: Rez, f: Farba) {
        let s = self.sirka(t, rez);
        self.text(t, x + (w - s) / 2.0, zakladna, rez, f);
    }

    /// Šírka jednej číslice pri číslach rovnakej šírky.
    fn sirka_cislice(&self, rez: Rez) -> f32 {
        ('0'..='9')
            .map(|c| self.pismo.posun(c, rez.velkost * self.mierka, rez.hrubka * self.mierka))
            .fold(0.0, f32::max)
            / self.mierka
    }

    /// Šírka textu s **číslicami rovnakej šírky** (hodnoty, časy – pri zmene sa nehýbu).
    pub fn sirka_cisel(&self, t: &str, rez: Rez) -> f32 {
        let c = self.sirka_cislice(rez);
        let s: f32 = t
            .chars()
            .map(|z| {
                if z.is_ascii_digit() {
                    c
                } else {
                    self.pismo.posun(z, rez.velkost * self.mierka, rez.hrubka * self.mierka) / self.mierka
                }
            })
            .sum();
        if t.is_empty() { 0.0 } else { s - 0.14 * rez.velkost }
    }

    /// Text s číslicami rovnakej šírky (každá číslica v strede rovnako širokej bunky); vráti koniec.
    pub fn cisla(&mut self, t: &str, x: f32, zakladna: f32, rez: Rez, f: Farba) -> f32 {
        let c = self.sirka_cislice(rez);
        let mut pero = x;
        let mut znak = [0u8; 4];
        for z in t.chars() {
            let s = z.encode_utf8(&mut znak);
            let vlastna =
                self.pismo.posun(z, rez.velkost * self.mierka, rez.hrubka * self.mierka) / self.mierka;
            if z.is_ascii_digit() {
                self.text(s, pero + (c - vlastna) / 2.0, zakladna, rez, f);
                pero += c;
            } else {
                self.text(s, pero, zakladna, rez, f);
                pero += vlastna;
            }
        }
        pero
    }

    /// Čísla rovnakej šírky zarovnané vpravo ku `x`.
    pub fn cisla_vpravo(&mut self, t: &str, x: f32, zakladna: f32, rez: Rez, f: Farba) {
        let w = self.sirka_cisel(t, rez);
        self.cisla(t, x - w, zakladna, rez, f);
    }

    /// Ikona veľkosti `s` bodov s ľavým horným rohom v (x, y).
    pub fn ikona(&mut self, ik: Ikona, x: f32, y: f32, s: f32, f: Farba) {
        let m = self.mierka;
        let k = s / 20.0 * m;
        let h = TAH_IKONY * s / 20.0 * m;
        let (x, y) = (x * m, y * m);
        for tah in ik.tahy() {
            if tah.len() == 2 && (tah[0].0 - tah[1].0).abs() + (tah[0].1 - tah[1].1).abs() < 0.05 {
                // bodka: o niečo väčšia ako ťah, aby sa opticky vyrovnala čiaram
                let (cx, cy) = (x + tah[0].0 * k, y + tah[0].1 * k);
                let r = h * 0.62;
                self.tvar(cx - r, cy - r, cx + r, cy + r, f, 255.0, |px, py| (px - cx).hypot(py - cy) - r);
                continue;
            }
            for w in tah.windows(2) {
                self.ciara_px((x + w[0].0 * k, y + w[0].1 * k), (x + w[1].0 * k, y + w[1].1 * k), h, f);
            }
        }
        if let Some(v) = ik.vypln() {
            let body: Vec<(f32, f32)> = v.iter().map(|b| (x + b.0 * k, y + b.1 * k)).collect();
            self.mnohouholnik_px(&body, f);
        }
    }

    /// Obraz (body 0xRRGGBB, `ow × oh`) zmenšený do obdĺžnika so zaoblenými rohmi `r` (priemer 2 × 2 vzoriek).
    #[allow(clippy::too_many_arguments)]
    pub fn obraz(&mut self, o: &[Farba], ow: usize, oh: usize, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let m = self.mierka;
        let (x, y, w, h, r) = (x * m, y * m, w * m, h * m, r * m);
        let (cx, cy, hx, hy) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        let sirka = self.p.sirka;
        for py in (y.floor().max(0.0) as usize)..((y + h).ceil() as usize).min(self.p.vyska) {
            for px in (x.floor().max(0.0) as usize)..((x + w).ceil() as usize).min(sirka) {
                let k =
                    (0.5 - zaobleny(px as f32 + 0.5 - cx, py as f32 + 0.5 - cy, hx, hy, r)).clamp(0.0, 1.0);
                if k <= 0.0 {
                    continue;
                }
                let vzorka = |dx: f32, dy: f32| {
                    let sx = ((px as f32 + dx - x) / w * ow as f32).clamp(0.0, ow as f32 - 1.0) as usize;
                    let sy = ((py as f32 + dy - y) / h * oh as f32).clamp(0.0, oh as f32 - 1.0) as usize;
                    o[sy * ow + sx]
                };
                let a = zmiesaj(vzorka(0.25, 0.25), vzorka(0.75, 0.25), 128);
                let b = zmiesaj(vzorka(0.25, 0.75), vzorka(0.75, 0.75), 128);
                let i = py * sirka + px;
                self.p.body[i] = zmiesaj(self.p.body[i], zmiesaj(a, b, 128), (k * 255.0).round() as u8);
            }
        }
    }

    /// Stmaví (alebo zosvetlí) obdĺžnik – clona okolo výberu, popis cez fotku.
    pub fn clona(&mut self, x: f32, y: f32, w: f32, h: f32, f: Farba, a: u8) {
        self.obdlznik_a(x, y, w, h, 0.0, f, a as f32);
    }

    /// Uloží plátno ako PNG.
    pub fn uloz_png(&self, cesta: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(cesta, self.p.png())
    }
}

/// Základná čiara textu, ktorý má byť opticky v strede výšky `h` od `y` (stred veľkých písmen).
pub fn zakladna_v_strede(y: f32, h: f32, rez: Rez) -> f32 {
    y + (h + rez.velkost) / 2.0
}

/// Vzdialenosť bodu (dx, dy) od stredu zaobleného obdĺžnika s polovičnými rozmermi hx, hy a zaoblením r.
fn zaobleny(dx: f32, dy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = dx.abs() - (hx - r);
    let qy = dy.abs() - (hy - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::miery::TEXT;

    #[test]
    fn ciara_jedneho_bodu_je_ostra_v_kazdej_mierke() {
        for m in [1.0, 2.0] {
            let mut k = Kreslic::novy(10.0, 4.0, m, 0x000000);
            k.obdlznik(3.0, 0.0, 1.0, 4.0, 0.0, 0xFFFFFF);
            let riadok = &k.p.body[..k.p.sirka];
            let biele = riadok.iter().filter(|&&f| f == 0xFFFFFF).count();
            let sive = riadok.iter().filter(|&&f| f != 0xFFFFFF && f != 0).count();
            assert_eq!((biele, sive), (m as usize, 0), "mierka {m}");
        }
    }

    #[test]
    fn cisla_rovnakej_sirky() {
        let k = Kreslic::novy(10.0, 10.0, 1.0, 0);
        assert!((k.sirka_cisel("1:11", TEXT) - k.sirka_cisel("8:88", TEXT)).abs() < 1e-4);
        assert!((k.sirka_cisel("100", TEXT) - k.sirka_cisel("999", TEXT)).abs() < 1e-4);
    }

    #[test]
    fn dvojnasobna_mierka_ma_dvojnasobny_rozmer() {
        let a = Kreslic::novy(320.0, 52.0, 1.0, 0);
        let b = Kreslic::novy(320.0, 52.0, 2.0, 0);
        assert_eq!((b.p.sirka, b.p.vyska), (2 * a.p.sirka, 2 * a.p.vyska));
        assert_eq!(a.rozmer(), b.rozmer());
        assert!((b.sirka("Uložiť", TEXT) - a.sirka("Uložiť", TEXT)).abs() < 0.6);
    }
}
