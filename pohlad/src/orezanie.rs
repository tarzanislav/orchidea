//! Orezanie výberom obdĺžnika (režim orezania prehliadača): ťahanie rohov a okrajov, posun výberu, nový výber
//! ťahaním mimo neho, pomer strán (voľný, 1:1, 4:3, 3:2, 16:9, na šírku aj na výšku), rozmer v bodoch.
//!
//! Pri JPEG, ktorý sa dá uložiť bezstratovo, sa začiatok výberu (ľavý a horný okraj) zarovná na mriežku MCU –
//! výber na obrazovke je presne to, čo sa uloží. Pravý a dolný okraj môžu byť kdekoľvek.
//!
//! Všetko v súradniciach obrazu (body obrazu po doterajších úpravách); bez `unsafe`, bez stavu okna.

use orchidea_formaty::jpeg::bezstratovo::Mriezka;

/// Pomer strán výberu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pomer {
    Volny,
    Stvorec,
    P4x3,
    P3x2,
    P16x9,
}

impl Pomer {
    /// Pomer v celých číslach (šírka, výška) na šírku.
    fn cela(self) -> Option<(usize, usize)> {
        match self {
            Pomer::Volny => None,
            Pomer::Stvorec => Some((1, 1)),
            Pomer::P4x3 => Some((4, 3)),
            Pomer::P3x2 => Some((3, 2)),
            Pomer::P16x9 => Some((16, 9)),
        }
    }

    /// Šírka / výška na šírku (na výšku je to prevrátená hodnota).
    fn hodnota(self) -> Option<f32> {
        match self {
            Pomer::Volny => None,
            Pomer::Stvorec => Some(1.0),
            Pomer::P4x3 => Some(4.0 / 3.0),
            Pomer::P3x2 => Some(1.5),
            Pomer::P16x9 => Some(16.0 / 9.0),
        }
    }
}

/// Časť výberu, ktorá sa ťahá.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uchop {
    Lavy,
    Pravy,
    Horny,
    Dolny,
    LavyHorny,
    PravyHorny,
    LavyDolny,
    PravyDolny,
    /// Vnútri výberu – posun.
    Stred,
    /// Nový výber kreslený od kotvy (dá sa ťahať do každého smeru).
    Novy,
}

impl Uchop {
    /// (ľavý, pravý, horný, dolný) okraj sa hýbe.
    fn okraje(self) -> (bool, bool, bool, bool) {
        match self {
            Uchop::Lavy => (true, false, false, false),
            Uchop::Pravy => (false, true, false, false),
            Uchop::Horny => (false, false, true, false),
            Uchop::Dolny => (false, false, false, true),
            Uchop::LavyHorny => (true, false, true, false),
            Uchop::PravyHorny => (false, true, true, false),
            Uchop::LavyDolny => (true, false, false, true),
            Uchop::PravyDolny => (false, true, false, true),
            Uchop::Stred | Uchop::Novy => (false, false, false, false),
        }
    }
}

/// Výber (x, y, šírka, výška) v bodoch obrazu.
pub type Obdlznik = (f32, f32, f32, f32);

/// Stav režimu orezania.
#[derive(Debug, Clone, PartialEq)]
pub struct Orezanie {
    /// Výber v bodoch obrazu (plynulý počas ťahania).
    pub vyber: Obdlznik,
    /// Rozmer obrazu.
    pub obraz: (usize, usize),
    pub pomer: Pomer,
    pub na_vysku: bool,
    /// Mriežka bezstratového orezania JPEG (ak sa dá uložiť bezstratovo).
    pub mriezka: Option<Mriezka>,
    /// Ťahaná časť, poloha kurzora na začiatku a výber na začiatku.
    tahanie: Option<(Uchop, (f32, f32), Obdlznik)>,
    /// Časť pod myšou (zvýrazní sa).
    pub pod_mysou: Option<Uchop>,
}

/// Najmenší výber v bodoch obrazu.
const NAJMENEJ: f32 = 1.0;

impl Orezanie {
    /// Nový výber: zadaný (napríklad zobrazený výrez) alebo celý obraz.
    pub fn novy(
        sirka: usize,
        vyska: usize,
        vyrez: Option<(usize, usize, usize, usize)>,
        mriezka: Option<Mriezka>,
    ) -> Orezanie {
        let vyber = vyrez.map(|(x, y, w, h)| (x as f32, y as f32, w as f32, h as f32)).unwrap_or((
            0.0,
            0.0,
            sirka as f32,
            vyska as f32,
        ));
        let mut o = Orezanie {
            vyber,
            obraz: (sirka, vyska),
            pomer: Pomer::Volny,
            na_vysku: vyska > sirka,
            mriezka,
            tahanie: None,
            pod_mysou: None,
        };
        o.vyber = o.v_obraze(o.vyber);
        o
    }

    fn pomer_hodnota(&self) -> Option<f32> {
        self.pomer.hodnota().map(|r| if self.na_vysku && r != 1.0 { 1.0 / r } else { r })
    }

    /// Výber orezaný na obraz a s najmenším rozmerom.
    fn v_obraze(&self, (x, y, w, h): Obdlznik) -> Obdlznik {
        let (sw, sh) = (self.obraz.0 as f32, self.obraz.1 as f32);
        let x0 = x.clamp(0.0, sw - NAJMENEJ);
        let y0 = y.clamp(0.0, sh - NAJMENEJ);
        let x1 = (x + w).clamp(x0 + NAJMENEJ, sw);
        let y1 = (y + h).clamp(y0 + NAJMENEJ, sh);
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// Časť výberu v bode obrazu; `tolerancia` = polomer úchopu v bodoch obrazu. `None` = mimo výberu.
    pub fn uchop_na(&self, bx: f32, by: f32, tolerancia: f32) -> Option<Uchop> {
        let (x, y, w, h) = self.vysledok_f();
        let (x1, y1) = (x + w, y + h);
        let t = tolerancia;
        if bx < x - t || bx > x1 + t || by < y - t || by > y1 + t {
            return None;
        }
        let (l, p) = ((bx - x).abs() <= t, (bx - x1).abs() <= t);
        let (hr, d) = ((by - y).abs() <= t, (by - y1).abs() <= t);
        Some(match (l, p, hr, d) {
            (true, _, true, _) => Uchop::LavyHorny,
            (_, true, true, _) => Uchop::PravyHorny,
            (true, _, _, true) => Uchop::LavyDolny,
            (_, true, _, true) => Uchop::PravyDolny,
            (true, ..) => Uchop::Lavy,
            (_, true, ..) => Uchop::Pravy,
            (_, _, true, _) => Uchop::Horny,
            (_, _, _, true) => Uchop::Dolny,
            _ if bx >= x && bx <= x1 && by >= y && by <= y1 => Uchop::Stred,
            _ => return None,
        })
    }

    /// Nový výber, ktorý sa práve kreslí ťahaním od bodu obrazu (ťahanie myšou po obraze bez režimu orezania).
    pub fn nakreslene(sirka: usize, vyska: usize, bx: f32, by: f32, mriezka: Option<Mriezka>) -> Orezanie {
        let mut o = Orezanie::novy(sirka, vyska, None, mriezka);
        o.nakresli_od(bx, by);
        o
    }

    /// Stlačenie tlačidla myši v bode obrazu: ťahanie úchopu alebo posun. `false` = mimo výberu (nový výber
    /// sa začne kresliť až ťahaním – samotné kliknutie mimo výber zruší).
    pub fn zacni(&mut self, bx: f32, by: f32, tolerancia: f32) -> bool {
        match self.uchop_na(bx, by, tolerancia) {
            Some(u) => {
                self.tahanie = Some((u, (bx, by), self.vyber));
                true
            }
            None => false,
        }
    }

    /// Začne kresliť nový výber od bodu obrazu (kotva); ťahá sa do ľubovoľného smeru.
    pub fn nakresli_od(&mut self, bx: f32, by: f32) {
        let (sw, sh) = (self.obraz.0 as f32, self.obraz.1 as f32);
        let (x, y) = (bx.clamp(0.0, sw), by.clamp(0.0, sh));
        self.vyber = self.v_obraze((x, y, NAJMENEJ, NAJMENEJ));
        self.tahanie = Some((Uchop::Novy, (x, y), self.vyber));
    }

    /// Ťahá sa? (Kým áno, posun obrazu je vypnutý.)
    pub fn taha(&self) -> bool {
        self.tahanie.is_some()
    }

    /// Koniec ťahania.
    pub fn pusti(&mut self) {
        self.tahanie = None;
    }

    /// Kurzor sa pri ťahaní posunul do bodu obrazu (bx, by). Vráti, či sa výber zmenil.
    pub fn tahaj(&mut self, bx: f32, by: f32) -> bool {
        let Some((u, (zx, zy), (x, y, w, h))) = self.tahanie else { return false };
        let (dx, dy) = (bx - zx, by - zy);
        let (sw, sh) = (self.obraz.0 as f32, self.obraz.1 as f32);
        let pred = self.vyber;
        if u == Uchop::Novy {
            // obdĺžnik medzi kotvou a kurzorom (orezaným na obraz), s pomerom podľa kratšej strany
            let (cx, cy) = (bx.clamp(0.0, sw), by.clamp(0.0, sh));
            let (mut w, mut h) = ((cx - zx).abs().max(NAJMENEJ), (cy - zy).abs().max(NAJMENEJ));
            if let Some(r) = self.pomer_hodnota() {
                if w / h > r {
                    w = h * r;
                } else {
                    h = w / r;
                }
            }
            let x0 = if cx >= zx { zx } else { zx - w };
            let y0 = if cy >= zy { zy } else { zy - h };
            self.vyber = self.v_obraze((x0, y0, w, h));
            return self.vyber != pred;
        }
        if u == Uchop::Stred {
            // posun celého výberu, rozmer ostáva
            let nx = (x + dx).clamp(0.0, (sw - w).max(0.0));
            let ny = (y + dy).clamp(0.0, (sh - h).max(0.0));
            self.vyber = (nx, ny, w, h);
            return self.vyber != pred;
        }
        let (l, p, hr, d) = u.okraje();
        let (mut x0, mut y0, mut x1, mut y1) = (x, y, x + w, y + h);
        if l {
            x0 = (x0 + dx).clamp(0.0, x1 - NAJMENEJ);
        }
        if p {
            x1 = (x1 + dx).clamp(x0 + NAJMENEJ, sw);
        }
        if hr {
            y0 = (y0 + dy).clamp(0.0, y1 - NAJMENEJ);
        }
        if d {
            y1 = (y1 + dy).clamp(y0 + NAJMENEJ, sh);
        }
        if let Some(r) = self.pomer_hodnota() {
            (x0, y0, x1, y1) = s_pomerom((x0, y0, x1, y1), (x, y, x + w, y + h), u, r, (sw, sh));
        }
        self.vyber = (x0, y0, x1 - x0, y1 - y0);
        self.vyber != pred
    }

    /// Nastaví pomer strán: výber sa zmenší okolo stredu tak, aby mal pomer a ostal v obraze.
    pub fn nastav_pomer(&mut self, p: Pomer) {
        if p == self.pomer && p != Pomer::Volny {
            // znova ten istý pomer = otočiť na výšku / na šírku
            self.na_vysku = !self.na_vysku;
        }
        self.pomer = p;
        self.pouzi_pomer();
    }

    /// Vymení na šírku / na výšku.
    pub fn otoc_pomer(&mut self) {
        self.na_vysku = !self.na_vysku;
        self.pouzi_pomer();
    }

    fn pouzi_pomer(&mut self) {
        let Some(r) = self.pomer_hodnota() else { return };
        let (x, y, w, h) = self.vyber;
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        // najväčší obdĺžnik s pomerom r v doterajšom rozmere (aspoň trochu, aby sa dal zväčšiť)
        let (mut nw, mut nh) = if w / h > r { (h * r, h) } else { (w, w / r) };
        let (sw, sh) = (self.obraz.0 as f32, self.obraz.1 as f32);
        if nw > sw {
            (nw, nh) = (sw, sw / r);
        }
        if nh > sh {
            (nw, nh) = (sh * r, sh);
        }
        let nx = (cx - nw / 2.0).clamp(0.0, (sw - nw).max(0.0));
        let ny = (cy - nh / 2.0).clamp(0.0, (sh - nh).max(0.0));
        self.vyber = (nx, ny, nw, nh);
    }

    /// Výber v celých bodoch; začiatok zarovnaný na mriežku bezstratového JPEG.
    pub fn vysledok(&self) -> (usize, usize, usize, usize) {
        let (sw, sh) = self.obraz;
        let (x, y, w, h) = self.vyber;
        let mut x0 = (x.round().max(0.0) as usize).min(sw.saturating_sub(1));
        let mut y0 = (y.round().max(0.0) as usize).min(sh.saturating_sub(1));
        let mut x1 = (x + w).round().max(0.0) as usize;
        let mut y1 = (y + h).round().max(0.0) as usize;
        if let Some(m) = self.mriezka {
            // nadol na mriežku od posunu; neúplný okraj pred posunom bezstratová cesta odstráni vždy
            let z = |v: usize, mcu: usize, posun: usize| -> usize {
                if mcu == 0 || v <= posun { posun } else { posun + (v - posun) / mcu * mcu }
            };
            x0 = z(x0, m.mcu.0, m.posun.0).min(sw.saturating_sub(1));
            y0 = z(y0, m.mcu.1, m.posun.1).min(sh.saturating_sub(1));
        }
        x1 = x1.clamp(x0 + 1, sw.max(x0 + 1));
        y1 = y1.clamp(y0 + 1, sh.max(y0 + 1));
        // presný pomer strán aj po zarovnaní začiatku a zaokrúhlení: násobok a × b
        if let Some((a, b)) = self.pomer.cela() {
            let (a, b) = if self.na_vysku { (b, a) } else { (a, b) };
            // najväčší násobok, ktorý sa zmestí do doterajšieho výberu (ten je v obraze)
            let k = ((x1 - x0) / a).min((y1 - y0) / b).max(1);
            x1 = (x0 + k * a).min(sw);
            y1 = (y0 + k * b).min(sh);
        }
        (x0, y0, x1 - x0, y1 - y0)
    }

    fn vysledok_f(&self) -> Obdlznik {
        let (x, y, w, h) = self.vysledok();
        (x as f32, y as f32, w as f32, h as f32)
    }

    /// Výber je celý obraz (potvrdenie nič neoreže)? Pri bezstratovom JPEG aj vtedy, keď chýba len neúplný okraj.
    pub fn cely_obraz(&self) -> bool {
        let (x, y, w, h) = self.vyber;
        let (sw, sh) = (self.obraz.0 as f32, self.obraz.1 as f32);
        x.round() <= 0.0 && y.round() <= 0.0 && (x + w).round() >= sw && (y + h).round() >= sh
    }
}

/// Upraví obdĺžnik po ťahaní tak, aby mal pomer `r`: pevný ostane okraj oproti ťahanému (pri okraji stred).
fn s_pomerom(
    (x0, y0, x1, y1): (f32, f32, f32, f32),
    (px0, py0, px1, py1): (f32, f32, f32, f32),
    u: Uchop,
    r: f32,
    (sw, sh): (f32, f32),
) -> (f32, f32, f32, f32) {
    let (l, p, hr, d) = u.okraje();
    let (mut w, mut h) = (x1 - x0, y1 - y0);
    let roh = (l || p) && (hr || d);
    if roh {
        // rozhoduje väčšia relatívna zmena
        if (w / (px1 - px0)).max((px1 - px0) / w) >= (h / (py1 - py0)).max((py1 - py0) / h) {
            h = w / r;
        } else {
            w = h * r;
        }
    } else if l || p {
        h = w / r;
    } else {
        w = h * r;
    }
    // kotva: protiľahlý okraj (pri okraji stred druhej osi)
    let ax = if l {
        px1
    } else if p {
        px0
    } else {
        (px0 + px1) / 2.0
    };
    let ay = if hr {
        py1
    } else if d {
        py0
    } else {
        (py0 + py1) / 2.0
    };
    // najväčší rozmer, ktorý sa od kotvy zmestí do obrazu
    let max_w = if l {
        ax
    } else if p {
        sw - ax
    } else {
        2.0 * ax.min(sw - ax)
    };
    let max_h = if hr {
        ay
    } else if d {
        sh - ay
    } else {
        2.0 * ay.min(sh - ay)
    };
    let k = (max_w / w).min(max_h / h).min(1.0);
    let (w, h) = ((w * k).max(NAJMENEJ), (h * k).max(NAJMENEJ));
    let nx0 = if l {
        ax - w
    } else if p {
        ax
    } else {
        ax - w / 2.0
    };
    let ny0 = if hr {
        ay - h
    } else if d {
        ay
    } else {
        ay - h / 2.0
    };
    (nx0, ny0, nx0 + w, ny0 + h)
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn tahanie_rohov_okrajov_a_posun() {
        let mut o = Orezanie::novy(400, 300, None, None);
        assert_eq!(o.vysledok(), (0, 0, 400, 300));
        assert!(o.cely_obraz());
        // pravý dolný roh dovnútra
        o.zacni(400.0, 300.0, 6.0);
        assert!(o.taha());
        o.tahaj(300.0, 250.0);
        o.pusti();
        assert_eq!(o.vysledok(), (0, 0, 300, 250));
        // ľavý okraj
        o.zacni(1.0, 100.0, 6.0);
        o.tahaj(50.0, 120.0);
        o.pusti();
        assert_eq!(o.vysledok(), (49, 0, 251, 250));
        // posun vnútri – za okraj obrazu nevyjde
        o.zacni(150.0, 100.0, 6.0);
        o.tahaj(1000.0, 1000.0);
        o.pusti();
        assert_eq!(o.vysledok(), (149, 50, 251, 250));
        // kliknutie mimo výberu nič neuchopí; nový výber sa kreslí od kotvy do každého smeru
        let mut o = Orezanie::novy(400, 300, Some((100, 100, 50, 50)), None);
        assert!(!o.zacni(10.0, 10.0, 6.0));
        o.nakresli_od(10.0, 10.0);
        o.tahaj(60.0, 40.0);
        assert_eq!(o.vysledok(), (10, 10, 50, 30));
        // za kotvu doľava hore a za okraj obrazu
        o.tahaj(-100.0, -100.0);
        assert_eq!(o.vysledok(), (0, 0, 10, 10));
        assert_eq!(o.uchop_na(500.0, 500.0, 6.0), None);
        // nakreslený výber (bez režimu orezania): od (300, 250) doľava hore, s pomerom 4:3
        let mut o = Orezanie::nakreslene(400, 300, 300.0, 250.0, None);
        o.tahaj(100.0, 50.0);
        assert_eq!(o.vysledok(), (100, 50, 200, 200));
        o.nastav_pomer(Pomer::P4x3);
        o.nakresli_od(300.0, 250.0);
        o.tahaj(100.0, 50.0);
        let (x, y, w, h) = o.vysledok();
        assert_eq!((w * 3, x + w, y + h), (h * 4, 300, 250));
    }

    #[test]
    fn pomer_stran() {
        let mut o = Orezanie::novy(400, 300, None, None);
        o.nastav_pomer(Pomer::Stvorec);
        let (x, y, w, h) = o.vysledok();
        assert_eq!((w, h), (300, 300));
        assert_eq!((x, y), (50, 0));
        o.nastav_pomer(Pomer::P16x9);
        let (_, _, w, h) = o.vysledok();
        assert!((w as f32 / h as f32 - 16.0 / 9.0).abs() < 0.02, "{w}×{h}");
        // znova 16:9 = na výšku
        o.nastav_pomer(Pomer::P16x9);
        let (_, _, w, h) = o.vysledok();
        assert!((h as f32 / w as f32 - 16.0 / 9.0).abs() < 0.03, "{w}×{h}");
        // pomer platí presne aj po zarovnaní na mriežku JPEG
        let mut o = Orezanie::novy(
            400,
            300,
            Some((37, 23, 200, 150)),
            Some(Mriezka { mcu: (16, 16), posun: (0, 0) }),
        );
        o.nastav_pomer(Pomer::P4x3);
        let (x, y, w, h) = o.vysledok();
        assert_eq!((x, y), (32, 16));
        assert_eq!(w * 3, h * 4, "{w}×{h}");
        // ťahanie rohu drží pomer a ostane v obraze
        let mut o = Orezanie::novy(400, 300, Some((0, 0, 200, 150)), None);
        o.nastav_pomer(Pomer::P4x3);
        o.zacni(200.0, 150.0, 6.0);
        o.tahaj(900.0, 160.0);
        let (x, y, w, h) = o.vysledok();
        assert_eq!((x, y), (0, 0));
        assert!(w <= 400 && h <= 300 && (w as f32 / h as f32 - 4.0 / 3.0).abs() < 0.02, "{w}×{h}");
        assert_eq!((w, h), (400, 300));
        // okraj s pomerom: druhý rozmer okolo stredu
        let mut o = Orezanie::novy(400, 300, Some((100, 100, 100, 100)), None);
        o.nastav_pomer(Pomer::Stvorec);
        o.zacni(200.0, 150.0, 6.0);
        o.tahaj(240.0, 150.0);
        let (x, y, w, h) = o.vysledok();
        assert_eq!((w, h), (140, 140));
        assert_eq!((x, y), (100, 80));
    }

    #[test]
    fn mriezka_bezstratoveho_jpeg() {
        let m = Mriezka { mcu: (16, 16), posun: (4, 0) };
        let mut o = Orezanie::nakreslene(100, 75, 30.0, 20.0, Some(m));
        o.tahaj(80.0, 60.0);
        // začiatok nadol na 4 + 16·k, 16·k; koniec ostane
        assert_eq!(o.vysledok(), (20, 16, 60, 44));
        // pred posunom → začiatok na posune (neúplný okraj sa aj tak odstráni)
        let mut o = Orezanie::novy(100, 75, Some((2, 0, 50, 50)), Some(m));
        assert_eq!(o.vysledok(), (4, 0, 48, 50));
        o.vyber.0 = 10.0;
        assert_eq!(o.vysledok().0, 4);
        // celý obraz bez neúplného okraja = nič sa neoreže
        assert!(Orezanie::novy(100, 75, None, Some(m)).cely_obraz());
        assert_eq!(Orezanie::novy(100, 75, None, Some(m)).vysledok(), (4, 0, 96, 75));
    }
}
