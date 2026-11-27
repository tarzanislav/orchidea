//! Okno prehliadača a editora fotografií (Pohľad) pre Linux – vlastný klient protokolu Wayland
//! (bez knižníc libwayland, libxkbcommon ani GTK/Qt).
//!
//! Chladné ako okno pre Windows: hlavné vlákno spí na kanáli, kým nepríde udalosť od kompozitora
//! (číta ju samostatné vlákno), hotová práca jadra alebo termín časovača prezentácie či opakovania
//! klávesu. Kreslí sa len po zmene a najviac raz za snímok kompozitora (wl_surface.frame).
//! Obraz ide cez zdieľanú pamäť (wl_shm, XRGB8888 = presne plátno 0xRRGGBB).
//!
//! Okno: xdg-shell (xdg_toplevel), vlastná lišta s titulkom a tlačidlami (minimalizovať, maximalizovať,
//! zavrieť), presun a zmena veľkosti ťahaním; ak kompozitor kreslí rámy sám (xdg-decoration),
//! vlastná lišta sa nekreslí. Prezentácia na celú obrazovku (set_fullscreen), pretiahnutie súboru
//! (wl_data_device, text/uri-list), kurzor cez cursor-shape alebo vlastná šípka.
//! Klávesnica: rozloženie XKB od kompozitora (`klavesnica`), mŕtve klávesy, opakovanie podľa repeat_info.
//!
//! Ovládanie je rovnaké ako vo Windows (pozri `okno_win`).

mod portal;
#[cfg(test)]
mod skuska_kompozitor;
mod sys;
mod wayland;

use crate::klavesnica::{self as kl, Klavesnica};
use crate::okno::{self, Dalej, Samotest, povel_znaku};
use crate::prehliadac::{Povel, Prehliadac, Vstup, ZiadostOkna};
use orchidea_pismo::{Pismo, Platno};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};
use wayland::{Citac, Popisovace, Spojenie, Sprava as Ziadost, Udalost};

/// Výška vlastnej lišty okna (body).
const LISTA: usize = 32;
/// Šírka tlačidla lišty.
const TLACIDLO: usize = 46;
/// Okraj na zmenu veľkosti ťahaním.
const OKRAJ: f64 = 6.0;
const LISTA_POZADIE: u32 = 0x1E1F22;
const LISTA_NEAKTIVNA: u32 = 0x26272A;
const LISTA_POD_MYSOU: u32 = 0x3A3B3F;
const ZAVRIET_POD_MYSOU: u32 = 0xC42B1C;
const LISTA_TEXT: u32 = 0xD8D8D8;
const LISTA_TEXT_NEAKTIVNY: u32 = 0x8A8A8A;

const BTN_LAVE: u32 = 0x110;
const BTN_PRAVE: u32 = 0x111;
const BTN_STREDNE: u32 = 0x112;

/// Formáty wl_shm.
const ARGB8888: u32 = 0;
const XRGB8888: u32 = 1;

enum Sprava {
    Wayland(Udalost),
    Fd(OwnedFd),
    /// Pracovné vlákno jadra dokončilo prácu.
    Hotove,
    /// Pretiahnuté súbory (text/uri-list prečítaný v pomocnom vlákne).
    Pretiahnute(Vec<PathBuf>),
    /// Služba dialógov prostredia (xdg-desktop-portal) je k dispozícii.
    Dialogy,
    /// Dialóg sa zavrel: vybraný súbor alebo priečinok (None = zrušené alebo chyba).
    Vybrane(Option<PathBuf>),
    Koniec(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Druh {
    Display,
    Registry,
    Synchronizacia,
    Snimok,
    Compositor,
    Shm,
    Pool,
    Buffer,
    Surface,
    SurfaceKurzora,
    WmBase,
    XdgSurface,
    Toplevel,
    Seat,
    Pointer,
    Keyboard,
    DataManager,
    DataDevice,
    DataOffer,
    DekorManager,
    Dekoracia,
    TvarManager,
    TvarZariadenie,
}

/// Obraz v zdieľanej pamäti.
struct Vyrovnavac {
    id: u32,
    subor: File,
    sirka: usize,
    vyska: usize,
    obsadeny: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TlacidloListy {
    Minimalizovat,
    Maximalizovat,
    Zavriet,
}

struct Okno {
    sp: Spojenie,
    objekty: HashMap<u32, Druh>,
    globaly: Vec<(u32, String, u32)>,
    synchronizovane: bool,
    compositor: u32,
    verzia_compositor: u32,
    shm: u32,
    wm_base: u32,
    seat: u32,
    data_manager: u32,
    dekor_manager: u32,
    tvar_manager: u32,
    surface: u32,
    xdg_surface: u32,
    toplevel: u32,
    pointer: u32,
    keyboard: u32,
    data_device: u32,
    tvar: u32,
    kurzor: u32,
    kurzor_obraz: Option<Vyrovnavac>,
    vyrovnavace: Vec<Vyrovnavac>,
    fds: Popisovace,
    tx: Sender<Sprava>,

    // stav okna od kompozitora
    cakajuca: Option<(i32, i32, Vec<u32>)>,
    sirka: usize,
    vyska: usize,
    maximalizovane: bool,
    cela: bool,
    dlazdica: bool,
    aktivne: bool,
    server_dekoruje: bool,
    nakonfigurovane: bool,
    snimok_caka: bool,
    snimkov: u64,
    prekreslit: bool,
    chce_celu: bool,

    // myš
    mys: (f64, f64),
    mys_serial: u32,
    tahanie: Option<(f64, f64)>,
    delenie: bool,
    /// Posun obrazu pravým alebo stredným tlačidlom (true = pravé).
    posun: Option<bool>,
    posledny_klik: (u32, f64, f64),
    koliesko: (f64, i32),
    pod_mysou: Option<TlacidloListy>,
    tvar_kurzora: u32,

    // klávesnica
    klavesnica: Klavesnica,
    opakovanie: (i32, i32),
    opakuj: Option<(u32, Instant)>,
    serial: u32,

    // pretiahnutie
    ponuky: HashMap<u32, Vec<String>>,
    dnd: Option<(u32, u32)>,

    // jadro
    jadro: Prehliadac,
    platno: Platno,
    lista: Platno,
    pismo: Pismo,
    titulok: String,
    casovac: Option<(u32, Instant)>,
    samotest: Option<Samotest>,
    samotest_dalsi: bool,
    koniec: bool,
    kod: i32,
    obraz_okna: Vec<u32>,
    dialog_otvoreny: bool,
}

fn chyba(e: impl std::fmt::Display) -> String {
    format!("prehliadač a editor fotografií (Pohľad): {e}")
}

/// Dočasný súbor pre zdieľanú pamäť: v XDG_RUNTIME_DIR (tmpfs), hneď zmazaný (ostane len popisovač).
fn docasny_subor(velkost: usize) -> std::io::Result<File> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static POCITADLO: AtomicU32 = AtomicU32::new(0);
    let priecinok = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    loop {
        let n = POCITADLO.fetch_add(1, Ordering::Relaxed);
        let c = priecinok.join(format!("orchidea-pohlad-{}-{n}", std::process::id()));
        match std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&c) {
            Ok(f) => {
                let _ = std::fs::remove_file(&c);
                f.set_len(velkost as u64)?;
                return Ok(f);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1_000_000 => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Zoznam URI (text/uri-list) → cesty k súborom.
fn cesty_z_uri(t: &str) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    t.lines()
        .map(|r| r.trim_end_matches('\r'))
        .filter(|r| !r.is_empty() && !r.starts_with('#'))
        .filter_map(|r| r.strip_prefix("file://"))
        .map(|r| if r.starts_with('/') { r } else { r.find('/').map_or("", |i| &r[i..]) })
        .filter(|r| !r.is_empty())
        .map(|r| {
            let b = r.as_bytes();
            let mut v = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                let hex = |c: u8| (c as char).to_digit(16);
                if b[i] == b'%'
                    && i + 2 < b.len()
                    && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
                {
                    v.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    v.push(b[i]);
                    i += 1;
                }
            }
            PathBuf::from(std::ffi::OsString::from_vec(v))
        })
        .collect()
}

/// Šípka kurzora (ARGB, 16 × 24), ak kompozitor nemá cursor-shape.
fn sipka() -> Vec<u32> {
    let body: [(f64, f64); 7] =
        [(1.0, 1.0), (1.0, 18.0), (5.0, 14.0), (8.0, 21.0), (11.0, 20.0), (8.0, 13.0), (13.0, 13.0)];
    let vnutri = |x: f64, y: f64| {
        let mut v = false;
        let mut j = body.len() - 1;
        for i in 0..body.len() {
            let (xi, yi) = body[i];
            let (xj, yj) = body[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                v = !v;
            }
            j = i;
        }
        v
    };
    let mut o = vec![0u32; 16 * 24];
    for y in 0..24 {
        for x in 0..16 {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            if vnutri(fx, fy) {
                let okraj = [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)]
                    .iter()
                    .any(|(dx, dy)| !vnutri(fx + dx, fy + dy));
                o[y * 16 + x] = if okraj { 0xFF00_0000 } else { 0xFFFF_FFFF };
            }
        }
    }
    o
}

impl Okno {
    fn posli(&mut self, z: Ziadost) {
        if let Err(e) = self.sp.posli(z) {
            self.zlyhanie(chyba(format!("spojenie s kompozitorom: {e}")));
        }
    }

    fn zlyhanie(&mut self, sprava: String) {
        if !self.koniec {
            eprintln!("{sprava}");
        }
        self.koniec = true;
        self.kod = 1;
    }

    fn nove(&mut self, druh: Druh) -> u32 {
        let id = self.sp.nove_id();
        self.objekty.insert(id, druh);
        id
    }

    fn bind(&mut self, rozhranie: &str, najviac: u32, druh: Druh) -> (u32, u32) {
        let Some(&(meno, _, verzia)) = self.globaly.iter().find(|g| g.1 == rozhranie) else { return (0, 0) };
        let v = verzia.min(najviac);
        let id = self.nove(druh);
        self.posli(Ziadost::nova(2, 0).u(meno).s(rozhranie).u(v).u(id));
        (id, v)
    }

    fn vyska_listy(&self) -> usize {
        if self.server_dekoruje || self.cela { 0 } else { LISTA }
    }

    // ————— jadro —————

    fn vstup(&mut self, v: Vstup) {
        let z = self.jadro.vstup(v);
        self.po_zmene(z);
    }

    fn povel(&mut self, p: Povel) {
        self.vstup(Vstup::Povel(p));
    }

    /// Celá obrazovka, časovač a titulok podľa jadra.
    fn po_zmene(&mut self, prekreslit: bool) {
        let chce = self.jadro.prezentuje();
        if chce != self.chce_celu {
            self.chce_celu = chce;
            if chce {
                self.posli(Ziadost::nova(self.toplevel, 11).u(0));
            } else {
                self.posli(Ziadost::nova(self.toplevel, 12));
            }
            self.nastav_kurzor(1);
        }
        let c = self.jadro.casovac();
        if c != self.casovac.map(|x| x.0) {
            self.casovac = c.map(|ms| (ms, Instant::now() + Duration::from_millis(ms as u64)));
        }
        let t = self.jadro.titulok();
        if t != self.titulok {
            self.titulok = t;
            let t = self.titulok.clone();
            self.posli(Ziadost::nova(self.toplevel, 2).s(&t));
            self.prekreslit = true;
        }
        if prekreslit || self.jadro.potrebuje_prekreslit() {
            self.prekreslit = true;
        }
    }

    fn zavri(&mut self) {
        self.koniec = true;
    }

    // ————— kreslenie —————

    fn volny_vyrovnavac(&mut self, w: usize, h: usize) -> Option<usize> {
        // nepoužité vyrovnávače inej veľkosti preč
        let mut i = 0;
        while i < self.vyrovnavace.len() {
            let v = &self.vyrovnavace[i];
            if !v.obsadeny && (v.sirka != w || v.vyska != h) {
                let id = v.id;
                self.vyrovnavace.remove(i);
                self.posli(Ziadost::nova(id, 0));
            } else {
                i += 1;
            }
        }
        if let Some(i) = self.vyrovnavace.iter().position(|v| !v.obsadeny) {
            return Some(i);
        }
        if self.vyrovnavace.len() >= 3 {
            return None;
        }
        let v = self.novy_vyrovnavac(w, h, XRGB8888)?;
        self.vyrovnavace.push(v);
        Some(self.vyrovnavace.len() - 1)
    }

    fn novy_vyrovnavac(&mut self, w: usize, h: usize, format: u32) -> Option<Vyrovnavac> {
        let velkost = w * h * 4;
        let subor = match docasny_subor(velkost) {
            Ok(s) => s,
            Err(e) => {
                self.zlyhanie(chyba(format!("zdieľaná pamäť obrazu: {e}")));
                return None;
            }
        };
        let pool = self.nove(Druh::Pool);
        let z = Ziadost::nova(self.shm, 0).u(pool);
        if let Err(e) = self.sp.posli_s_fd(z.i(velkost as i32), &[subor.as_fd()]) {
            self.zlyhanie(chyba(format!("spojenie s kompozitorom: {e}")));
            return None;
        }
        let id = self.nove(Druh::Buffer);
        self.posli(Ziadost::nova(pool, 0).u(id).i(0).i(w as i32).i(h as i32).i(w as i32 * 4).u(format));
        // zásobník môže zaniknúť hneď, obraz ostane
        self.posli(Ziadost::nova(pool, 1));
        Some(Vyrovnavac { id, subor, sirka: w, vyska: h, obsadeny: false })
    }

    fn kresli_listu(&mut self, w: usize) {
        if self.lista.sirka != w || self.lista.vyska != LISTA {
            self.lista = Platno::nove(w, LISTA, LISTA_POZADIE);
        }
        let p = &mut self.lista;
        p.body.fill(if self.aktivne { LISTA_POZADIE } else { LISTA_NEAKTIVNA });
        let tlacidla = [TlacidloListy::Minimalizovat, TlacidloListy::Maximalizovat, TlacidloListy::Zavriet];
        let x0 = w as i32 - (TLACIDLO * 3) as i32;
        for (i, t) in tlacidla.iter().enumerate() {
            let x = x0 + (i * TLACIDLO) as i32;
            if self.pod_mysou == Some(*t) {
                let f = if *t == TlacidloListy::Zavriet { ZAVRIET_POD_MYSOU } else { LISTA_POD_MYSOU };
                p.vypln(x, 0, TLACIDLO as i32, LISTA as i32, f);
            }
            let (cx, cy) = (x + TLACIDLO as i32 / 2, LISTA as i32 / 2);
            let f =
                if self.aktivne || self.pod_mysou == Some(*t) { LISTA_TEXT } else { LISTA_TEXT_NEAKTIVNY };
            match t {
                TlacidloListy::Minimalizovat => p.vypln(cx - 5, cy, 10, 1, f),
                TlacidloListy::Maximalizovat => {
                    let ram = |p: &mut Platno, x: i32, y: i32, s: i32| {
                        p.vypln(x, y, s, 1, f);
                        p.vypln(x, y + s - 1, s, 1, f);
                        p.vypln(x, y, 1, s, f);
                        p.vypln(x + s - 1, y, 1, s, f);
                    };
                    if self.maximalizovane {
                        ram(p, cx - 3, cy - 5, 8);
                        p.vypln(
                            cx - 5,
                            cy - 3,
                            8,
                            8,
                            if self.pod_mysou == Some(*t) { LISTA_POD_MYSOU } else { p.body[0] },
                        );
                        ram(p, cx - 5, cy - 3, 8);
                    } else {
                        ram(p, cx - 5, cy - 5, 10);
                    }
                }
                TlacidloListy::Zavriet => {
                    for d in -5..=5 {
                        p.vypln(cx + d, cy + d, 1, 1, f);
                        p.vypln(cx + d, cy - d, 1, 1, f);
                    }
                }
            }
        }
        let max = (x0 - 24).max(0) as f32;
        let text = self.pismo.skrat(&self.titulok, max, 13.0, 1.0);
        let f = if self.aktivne { LISTA_TEXT } else { LISTA_TEXT_NEAKTIVNY };
        let zakladna = LISTA as f32 / 2.0 + 13.0 * 0.36;
        p.text(&self.pismo, &text, 12.0, zakladna, 13.0, 1.0, f);
        p.vypln(0, LISTA as i32 - 1, w as i32, 1, 0x101113);
    }

    fn kresli(&mut self) {
        if !self.nakonfigurovane || self.snimok_caka || !self.prekreslit || self.koniec {
            return;
        }
        let (w, h) = (self.sirka.max(1), self.vyska.max(1));
        let lista = self.vyska_listy().min(h - 1);
        if self.jadro.potrebuje_prekreslit() || self.platno.sirka != w || self.platno.vyska != h - lista {
            self.jadro.vykresli(&mut self.platno);
        }
        if self.platno.sirka != w || self.platno.vyska != h - lista {
            // jadro ešte nevie o novej veľkosti
            return;
        }
        if lista > 0 {
            self.kresli_listu(w);
        }
        let Some(i) = self.volny_vyrovnavac(w, h) else { return };
        self.obraz_okna.clear();
        if lista > 0 {
            self.obraz_okna.extend_from_slice(&self.lista.body[..w * lista]);
        }
        self.obraz_okna.extend_from_slice(&self.platno.body);
        let bajty: Vec<u8> = self.obraz_okna.iter().flat_map(|b| b.to_le_bytes()).collect();
        if let Err(e) = self.vyrovnavace[i].subor.write_all_at(&bajty, 0) {
            self.zlyhanie(chyba(format!("zápis obrazu: {e}")));
            return;
        }
        let id = self.vyrovnavace[i].id;
        self.vyrovnavace[i].obsadeny = true;
        let s = self.surface;
        self.posli(Ziadost::nova(s, 1).u(id).i(0).i(0));
        if self.verzia_compositor >= 4 {
            self.posli(Ziadost::nova(s, 9).i(0).i(0).i(w as i32).i(h as i32));
        } else {
            self.posli(Ziadost::nova(s, 2).i(0).i(0).i(w as i32).i(h as i32));
        }
        let cb = self.nove(Druh::Snimok);
        self.posli(Ziadost::nova(s, 3).u(cb));
        self.posli(Ziadost::nova(s, 6));
        self.snimok_caka = true;
        self.prekreslit = false;
    }

    // ————— kurzor —————

    fn nastav_kurzor(&mut self, tvar: u32) {
        self.tvar_kurzora = tvar;
        if self.pointer == 0 || self.mys_serial == 0 {
            return;
        }
        if self.chce_celu {
            // počas prezentácie bez kurzora
            self.posli(Ziadost::nova(self.pointer, 0).u(self.mys_serial).u(0).i(0).i(0));
            return;
        }
        if self.tvar != 0 {
            self.posli(Ziadost::nova(self.tvar, 1).u(self.mys_serial).u(tvar));
            return;
        }
        if self.kurzor == 0 {
            self.kurzor = self.nove(Druh::SurfaceKurzora);
            self.posli(Ziadost::nova(self.compositor, 0).u(self.kurzor));
            let Some(v) = self.novy_vyrovnavac(16, 24, ARGB8888) else { return };
            let b: Vec<u8> = sipka().iter().flat_map(|b| b.to_le_bytes()).collect();
            let _ = v.subor.write_all_at(&b, 0);
            self.posli(Ziadost::nova(self.kurzor, 1).u(v.id).i(0).i(0));
            self.posli(Ziadost::nova(self.kurzor, 2).i(0).i(0).i(16).i(24));
            self.posli(Ziadost::nova(self.kurzor, 6));
            self.kurzor_obraz = Some(v);
        }
        self.posli(Ziadost::nova(self.pointer, 0).u(self.mys_serial).u(self.kurzor).i(1).i(1));
    }

    /// Okraj na zmenu veľkosti pod myšou (xdg_toplevel.resize_edge) a tvar kurzora.
    fn okraj(&self, x: f64, y: f64) -> Option<(u32, u32)> {
        if self.server_dekoruje || self.cela || self.maximalizovane || self.dlazdica {
            return None;
        }
        let (w, h) = (self.sirka as f64, self.vyska as f64);
        let (l, r, t, b) = (x < OKRAJ, x >= w - OKRAJ, y < OKRAJ, y >= h - OKRAJ);
        Some(match (t, b, l, r) {
            (true, _, true, _) => (5, 21),
            (true, _, _, true) => (9, 20),
            (_, true, true, _) => (6, 24),
            (_, true, _, true) => (10, 23),
            (true, ..) => (1, 19),
            (_, true, ..) => (2, 22),
            (_, _, true, _) => (4, 25),
            (_, _, _, true) => (8, 18),
            _ => return None,
        })
    }

    fn tlacidlo_listy(&self, x: f64, y: f64) -> Option<TlacidloListy> {
        if self.vyska_listy() == 0 || y < 0.0 || y >= LISTA as f64 {
            return None;
        }
        let x0 = self.sirka as f64 - (TLACIDLO * 3) as f64;
        if x < x0 {
            return None;
        }
        Some(match ((x - x0) / TLACIDLO as f64) as usize {
            0 => TlacidloListy::Minimalizovat,
            1 => TlacidloListy::Maximalizovat,
            _ => TlacidloListy::Zavriet,
        })
    }

    // ————— udalosti —————

    fn udalost(&mut self, u: Udalost) {
        let Some(&druh) = self.objekty.get(&u.objekt) else { return };
        let mut c = Citac::novy(&u.data);
        match (druh, u.cislo) {
            (Druh::Display, 0) => {
                let (objekt, kod) = (c.u(), c.u());
                let s = c.s();
                self.zlyhanie(chyba(format!("chyba protokolu Wayland (objekt {objekt}, kód {kod}): {s}")));
            }
            (Druh::Display, 1) => {
                let id = c.u();
                self.objekty.remove(&id);
            }
            (Druh::Registry, 0) => {
                let (meno, rozhranie, verzia) = (c.u(), c.s(), c.u());
                self.globaly.push((meno, rozhranie, verzia));
            }
            (Druh::Synchronizacia, 0) => self.synchronizovane = true,
            (Druh::Snimok, 0) => {
                self.snimok_caka = false;
                self.snimkov += 1;
            }
            (Druh::Buffer, 0) => {
                if let Some(v) = self.vyrovnavace.iter_mut().find(|v| v.id == u.objekt) {
                    v.obsadeny = false;
                }
            }
            (Druh::WmBase, 0) => {
                let s = c.u();
                self.posli(Ziadost::nova(self.wm_base, 3).u(s));
            }
            (Druh::Toplevel, 0) => {
                let (w, h) = (c.i(), c.i());
                let stavy =
                    c.pole().chunks_exact(4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]])).collect();
                self.cakajuca = Some((w, h, stavy));
            }
            (Druh::Toplevel, 1) => self.zavri(),
            (Druh::Dekoracia, 0) => self.server_dekoruje = c.u() == 2,
            (Druh::XdgSurface, 0) => {
                let serial = c.u();
                self.nakonfiguruj();
                self.posli(Ziadost::nova(self.xdg_surface, 4).u(serial));
            }
            (Druh::Seat, 0) => {
                let schopnosti = c.u();
                if schopnosti & 1 != 0 && self.pointer == 0 {
                    self.pointer = self.nove(Druh::Pointer);
                    self.posli(Ziadost::nova(self.seat, 0).u(self.pointer));
                    if self.tvar_manager != 0 {
                        self.tvar = self.nove(Druh::TvarZariadenie);
                        self.posli(Ziadost::nova(self.tvar_manager, 1).u(self.tvar).u(self.pointer));
                    }
                }
                if schopnosti & 2 != 0 && self.keyboard == 0 {
                    self.keyboard = self.nove(Druh::Keyboard);
                    self.posli(Ziadost::nova(self.seat, 1).u(self.keyboard));
                }
            }
            (Druh::Pointer, n) => self.mys_udalost(n, &mut c),
            (Druh::Keyboard, n) => self.klavesnica_udalost(n, &mut c),
            (Druh::DataDevice, n) => self.pretiahnutie(n, &mut c),
            (Druh::DataOffer, 0) => {
                let m = c.s();
                self.ponuky.entry(u.objekt).or_default().push(m);
            }
            _ => {}
        }
    }

    fn nakonfiguruj(&mut self) {
        let Some((w, h, stavy)) = self.cakajuca.take() else {
            self.nakonfigurovane = true;
            self.prekreslit = true;
            return;
        };
        self.maximalizovane = stavy.contains(&1);
        self.cela = stavy.contains(&2);
        self.dlazdica = stavy.iter().any(|s| (5..=8).contains(s));
        self.aktivne = stavy.contains(&4);
        if w > 0 && h > 0 {
            self.sirka = w as usize;
            self.vyska = h as usize;
        }
        let lista = self.vyska_listy();
        self.vyska = self.vyska.max(lista + 1);
        self.nakonfigurovane = true;
        self.prekreslit = true;
        let (sirka, vyska) = (self.sirka, self.vyska - lista);
        self.vstup(Vstup::Velkost { sirka, vyska });
    }

    fn mys_udalost(&mut self, n: u16, c: &mut Citac) {
        let lista = self.vyska_listy() as f64;
        match n {
            0 => {
                // enter
                self.mys_serial = c.u();
                let _surface = c.u();
                self.mys = (c.f(), c.f());
                let t = self.tvar_kurzora.max(1);
                self.nastav_kurzor(t);
            }
            1 => {
                self.tahanie = None;
                self.delenie = false;
                self.posun = None;
                if self.pod_mysou.take().is_some() {
                    self.prekreslit = true;
                }
                self.vstup(Vstup::Mys { x: -1.0, y: -1.0 });
            }
            2 => {
                let _cas = c.u();
                let (x, y) = (c.f(), c.f());
                let (px, py) = self.mys;
                self.mys = (x, y);
                if self.tahanie.is_some() {
                    self.tahanie = Some((x, y));
                    self.vstup(Vstup::Tahaj { dx: (x - px) as f32, dy: (y - py) as f32 });
                    return;
                }
                if let Some(pravym) = self.posun {
                    // pravé tlačidlo: deliaca čiara pri porovnaní aj posun obrazu (jadro vyberie)
                    if self.delenie {
                        self.vstup(Vstup::Delic { x: x as f32 });
                    }
                    self.vstup(Vstup::Posun { dx: (x - px) as f32, dy: (y - py) as f32, pravym });
                    return;
                }
                let t = self.tlacidlo_listy(x, y);
                if t != self.pod_mysou {
                    self.pod_mysou = t;
                    self.prekreslit = true;
                }
                let tvar = self.okraj(x, y).map_or(1, |o| o.1);
                if tvar != self.tvar_kurzora {
                    self.nastav_kurzor(tvar);
                }
                self.vstup(Vstup::Mys { x: x as f32, y: (y - lista) as f32 });
            }
            3 => {
                let (serial, cas, tlacidlo, stav) = (c.u(), c.u(), c.u(), c.u());
                self.serial = serial;
                let (x, y) = self.mys;
                if stav == 0 {
                    self.tahanie = None;
                    self.delenie = false;
                    self.posun = None;
                    return;
                }
                if tlacidlo == BTN_LAVE {
                    if let Some(t) = self.tlacidlo_listy(x, y) {
                        match t {
                            TlacidloListy::Minimalizovat => self.posli(Ziadost::nova(self.toplevel, 13)),
                            TlacidloListy::Maximalizovat => self.prepni_maximalizovanie(),
                            TlacidloListy::Zavriet => self.zavri(),
                        }
                    } else if let Some((hrana, _)) = self.okraj(x, y) {
                        self.posli(Ziadost::nova(self.toplevel, 6).u(self.seat).u(serial).u(hrana));
                    } else if y < lista {
                        let (pc, px, py) = self.posledny_klik;
                        self.posledny_klik = (cas, x, y);
                        if cas.wrapping_sub(pc) < 400 && (x - px).abs() < 5.0 && (y - py).abs() < 5.0 {
                            self.posledny_klik = (0, -100.0, -100.0);
                            self.prepni_maximalizovanie();
                        } else {
                            self.posli(Ziadost::nova(self.toplevel, 5).u(self.seat).u(serial));
                        }
                    } else {
                        self.tahanie = Some((x, y));
                        self.vstup(Vstup::Klik { x: x as f32, y: (y - lista) as f32 });
                    }
                } else if tlacidlo == BTN_PRAVE {
                    if y < lista {
                        self.posli(
                            Ziadost::nova(self.toplevel, 4).u(self.seat).u(serial).i(x as i32).i(y as i32),
                        );
                    } else {
                        self.delenie = true;
                        self.posun = Some(true);
                        self.vstup(Vstup::Delic { x: x as f32 });
                    }
                } else if tlacidlo == BTN_STREDNE && y >= lista {
                    // stredné tlačidlo – posun priblíženého obrazu
                    self.posun = Some(false);
                }
            }
            4 => {
                let (_cas, os, hodnota) = (c.u(), c.u(), c.f());
                if os == 0 {
                    self.koliesko.0 += hodnota;
                }
            }
            8 => {
                let (os, krokov) = (c.u(), c.i());
                if os == 0 {
                    self.koliesko.1 += krokov;
                }
            }
            5 => {
                // frame: koliesko naraz (otočenie dole = kladná hodnota = oddialiť)
                let (spolu, krokov) = std::mem::take(&mut self.koliesko);
                let k = if krokov != 0 { -krokov as f32 } else { (-spolu / 10.0) as f32 };
                if k != 0.0 {
                    let (x, y) = self.mys;
                    self.vstup(Vstup::Koliesko { krokov: k, x: x as f32, y: (y - lista) as f32 });
                }
            }
            _ => {}
        }
    }

    fn prepni_maximalizovanie(&mut self) {
        let op = if self.maximalizovane { 10 } else { 9 };
        self.posli(Ziadost::nova(self.toplevel, op));
    }

    fn klavesnica_udalost(&mut self, n: u16, c: &mut Citac) {
        match n {
            0 => {
                // keymap(format, fd, size) – popisovač ide mimo bajtov
                let (format, velkost) = (c.u(), c.u() as usize);
                let Some(fd) = self.fds.pop_front() else { return };
                if format != 1 || velkost == 0 || velkost > 64 << 20 {
                    return;
                }
                let f = File::from(fd);
                let mut b = vec![0u8; velkost];
                // pread: posun súboru môžu zdieľať všetci klienti kompozitora
                if f.read_exact_at(&mut b, 0).is_ok() {
                    let t = String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(&b)).into_owned();
                    match Klavesnica::z_textu(&t) {
                        Some(k) => self.klavesnica = k,
                        None => eprintln!(
                            "{}",
                            chyba("rozloženie klávesnice sa nedá prečítať – použije sa americké")
                        ),
                    }
                }
            }
            1 => self.serial = c.u(),
            2 => {
                self.opakuj = None;
                self.klavesnica.zrus_mrtvy();
            }
            3 => {
                let (serial, _cas, kluc, stav) = (c.u(), c.u(), c.u(), c.u());
                self.serial = serial;
                if stav == 1 {
                    let opakovat = self.klaves(kluc);
                    let (rychlost, oneskorenie) = self.opakovanie;
                    self.opakuj = (opakovat && rychlost > 0)
                        .then(|| (kluc, Instant::now() + Duration::from_millis(oneskorenie.max(1) as u64)));
                } else if self.opakuj.is_some_and(|o| o.0 == kluc) {
                    self.opakuj = None;
                }
            }
            4 => {
                let (_serial, stlacene, zachytene, zamknute, skupina) = (c.u(), c.u(), c.u(), c.u(), c.u());
                self.klavesnica.nastav(stlacene, zachytene, zamknute, skupina);
            }
            5 => self.opakovanie = (c.i(), c.i()),
            _ => {}
        }
    }

    /// Stlačený kláves (evdev kód). Vráti, či sa má pri držaní opakovať.
    fn klaves(&mut self, kod: u32) -> bool {
        let st = self.klavesnica.stlac(kod);
        let s = st.symbol;
        let ctrl = self.klavesnica.modifikatory() & kl::CTRL != 0;
        let pise = self.jadro.pise();
        if !pise {
            // mŕtve klávesy len pri písaní – inak by spojili povel s písmenom
            self.klavesnica.zrus_mrtvy();
        }
        if s == 0 || kl::je_modifikator(s) {
            return false;
        }
        let f = |n: u32| kl::F1 + n - 1;
        // Ctrl+0 … Ctrl+5 – hodnotenie (číselný rad a numerická klávesnica podľa polohy klávesu)
        let cislica = match kod {
            2..=6 => Some(kod as i8 - 1),
            11 => Some(0),
            82 => Some(0),
            79..=81 => Some(kod as i8 - 78),
            75 | 76 => Some(kod as i8 - 71),
            _ => None,
        };
        if s == kl::ESCAPE {
            let z = self.jadro.esc();
            self.po_zmene(z);
            if !z {
                self.zavri();
            }
            return false;
        }
        if s == f(1) {
            self.povel(Povel::Napoveda);
            return false;
        }
        if ctrl && let Some(n) = cislica {
            self.povel(Povel::Hodnotenie(n));
            return false;
        }
        if s == f(5) || s == f(11) {
            self.povel(Povel::Prezentacia);
            return false;
        }
        let navigacia = match s {
            kl::PAGE_UP | 0xff9a => Some(Povel::Predosly),
            kl::PAGE_DOWN | 0xff9b => Some(Povel::Dalsi),
            kl::HOME | 0xff95 => Some(Povel::Prvy),
            kl::END | 0xff9c => Some(Povel::Posledny),
            kl::VLAVO | 0xff96 => Some(Povel::Predosly),
            kl::VPRAVO | 0xff98 => Some(Povel::Dalsi),
            kl::HORE | 0xff97 => Some(Povel::Hore),
            kl::DOLE | 0xff99 => Some(Povel::Dole),
            _ if s == f(3) => Some(Povel::Katalog),
            _ => None,
        };
        if let Some(p) = navigacia {
            self.povel(p);
            return s != f(3);
        }
        if !pise {
            let p = match (s, kod) {
                (0x20, _) => Some(Povel::Dalsi),
                (kl::BACKSPACE, _) => Some(Povel::Predosly),
                (kl::KP_PLUS, _) | (_, 13) => Some(Povel::Priblizit),
                (kl::KP_MINUS, _) | (_, 12) => Some(Povel::Oddialit),
                _ => None,
            };
            if let Some(p) = p {
                self.povel(p);
                return true;
            }
        }
        if let Some(z) = st.znak {
            if pise {
                self.vstup(Vstup::Znak(z));
            } else if let Some(p) = povel_znaku(z) {
                self.povel(p);
            }
        }
        !(0xfe50..=0xfe5c).contains(&s)
    }

    fn pretiahnutie(&mut self, n: u16, c: &mut Citac) {
        match n {
            0 => {
                // data_offer: nový objekt od kompozitora
                let id = c.u();
                self.objekty.insert(id, Druh::DataOffer);
                self.ponuky.insert(id, Vec::new());
            }
            1 => {
                let (serial, _surface, _x, _y, ponuka) = (c.u(), c.u(), c.f(), c.f(), c.u());
                self.zahod_dnd();
                if ponuka == 0 {
                    return;
                }
                let uri = self.ponuky.get(&ponuka).is_some_and(|m| m.iter().any(|m| m == "text/uri-list"));
                if uri {
                    self.posli(Ziadost::nova(ponuka, 0).u(serial).s("text/uri-list"));
                    // kopírovať (verzia 3)
                    self.posli(Ziadost::nova(ponuka, 4).u(1).u(1));
                    self.dnd = Some((ponuka, serial));
                } else {
                    self.posli(Ziadost::nova(ponuka, 0).u(serial).u(0));
                    self.dnd = Some((ponuka, 0));
                }
            }
            2 => self.zahod_dnd(),
            4 => {
                // drop
                let Some((ponuka, serial)) = self.dnd.take() else { return };
                if serial == 0 {
                    self.zahod_ponuku(ponuka);
                    return;
                }
                let Ok((mut citac, pisac)) = std::io::pipe() else { return };
                if let Err(e) =
                    self.sp.posli_s_fd(Ziadost::nova(ponuka, 1).s("text/uri-list"), &[pisac.as_fd()])
                {
                    self.zlyhanie(chyba(format!("spojenie s kompozitorom: {e}")));
                    return;
                }
                drop(pisac);
                self.posli(Ziadost::nova(ponuka, 3));
                self.zahod_ponuku(ponuka);
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let mut t = String::new();
                    let mut obmedzene = (&mut citac).take(1 << 20);
                    if obmedzene.read_to_string(&mut t).is_ok() {
                        let _ = tx.send(Sprava::Pretiahnute(cesty_z_uri(&t)));
                    }
                });
            }
            5 => {
                // schránka: nepoužíva sa
                let ponuka = c.u();
                if ponuka != 0 && self.dnd.is_none_or(|d| d.0 != ponuka) {
                    self.zahod_ponuku(ponuka);
                }
            }
            _ => {}
        }
    }

    fn zahod_ponuku(&mut self, ponuka: u32) {
        if self.ponuky.remove(&ponuka).is_some() {
            self.posli(Ziadost::nova(ponuka, 2));
            self.objekty.remove(&ponuka);
        }
    }

    fn zahod_dnd(&mut self) {
        if let Some((p, _)) = self.dnd.take() {
            self.zahod_ponuku(p);
        }
    }

    fn sprava(&mut self, s: Sprava) {
        match s {
            Sprava::Wayland(u) => self.udalost(u),
            Sprava::Fd(fd) => self.fds.push_back(fd),
            Sprava::Hotove => {
                let z = self.jadro.spracuj_pozadie();
                self.po_zmene(z);
                if self.samotest.is_some() {
                    self.samotest_dalsi = true;
                }
            }
            Sprava::Pretiahnute(c) => {
                if let Some(c) = c.into_iter().next() {
                    self.vstup(Vstup::Otvor(c));
                }
            }
            Sprava::Dialogy => {
                self.jadro.povol_dialogy();
                self.prekreslit = true;
            }
            Sprava::Vybrane(c) => {
                self.dialog_otvoreny = false;
                if let Some(c) = c {
                    self.vstup(Vstup::Otvor(c));
                }
            }
            Sprava::Koniec(e) => self.zlyhanie(e),
        }
    }

    /// Žiadosť jadra o dialóg: ukáže ho prostredie (portál) v pomocnom vlákne; v samoteste sa len započíta.
    fn dialogy(&mut self) {
        let Some(z) = self.jadro.vezmi_ziadost_okna() else { return };
        if let Some(t) = self.samotest.as_mut() {
            t.dialogy += 1;
            return;
        }
        if self.dialog_otvoreny {
            return;
        }
        let (titulok, priecinok, adresar) = match z {
            ZiadostOkna::OtvorSubor { priecinok } => {
                ("Otvoriť obrázok – prehliadač a editor fotografií (Pohľad)", priecinok, false)
            }
            ZiadostOkna::VyberPriecinok { priecinok } => {
                ("Pridať priečinok do katalógu fotografií (Herbár)", priecinok, true)
            }
        };
        self.dialog_otvoreny = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let v = portal::Zbernica::relacie()
                .and_then(|b| portal::vyber(b, titulok, priecinok.as_deref(), adresar))
                .unwrap_or_else(|e| {
                    eprintln!("{}", chyba(format!("dialóg prostredia: {e}")));
                    None
                });
            let _ = tx.send(Sprava::Vybrane(v));
        });
    }

    /// Termíny: časovač prezentácie a opakovanie klávesu.
    fn terminy(&mut self) {
        let teraz = Instant::now();
        if let Some((ms, t)) = self.casovac
            && teraz >= t
        {
            self.casovac = Some((ms, teraz + Duration::from_millis(ms as u64)));
            self.vstup(Vstup::Tik);
        }
        if let Some((kluc, t)) = self.opakuj
            && teraz >= t
        {
            let rychlost = self.opakovanie.0.max(1) as u64;
            self.opakuj = Some((kluc, teraz + Duration::from_micros(1_000_000 / rychlost)));
            if !self.klaves(kluc) {
                self.opakuj = None;
            }
        }
    }

    fn najblizsi_termin(&self) -> Option<Instant> {
        [self.casovac.map(|c| c.1), self.opakuj.map(|o| o.1)].into_iter().flatten().min()
    }

    fn samotest_krok(&mut self) {
        // až keď kompozitor ukázal aspoň jeden snímok okna
        if !self.samotest_dalsi || self.snimkov == 0 || self.koniec {
            return;
        }
        let Some(mut t) = self.samotest.take() else { return };
        self.samotest_dalsi = false;
        match t.krok(&mut self.jadro) {
            Dalej::Cakaj => {}
            Dalej::Vstupy(v) => {
                self.samotest = Some(t);
                for v in v {
                    self.vstup(v);
                    self.dialogy();
                }
                self.samotest_dalsi = true;
                return;
            }
            Dalej::Koniec => {
                let kod = t.dokonci(&mut self.jadro, &mut self.platno);
                // aj celé okno tak, ako išlo do kompozitora (s lištou)
                let lista = self.vyska_listy();
                if self.lista.sirka == self.platno.sirka && lista > 0 {
                    self.kresli_listu(self.platno.sirka);
                }
                let mut cele = Platno::nove(self.platno.sirka, self.platno.vyska + lista, 0);
                if lista > 0 && self.lista.body.len() >= self.platno.sirka * lista {
                    cele.body[..self.platno.sirka * lista]
                        .copy_from_slice(&self.lista.body[..self.platno.sirka * lista]);
                }
                cele.body[self.platno.sirka * lista..].copy_from_slice(&self.platno.body);
                let mut c = t.vystup.clone().into_os_string();
                c.push(".okno.png");
                okno::uloz_png(&cele, &PathBuf::from(c));
                println!(
                    "SAMOTEST Wayland: snímkov {}, okno {}×{}, rám {}",
                    self.snimkov,
                    self.sirka,
                    self.vyska,
                    if self.server_dekoruje { "kompozitora" } else { "vlastný" }
                );
                self.kod = kod;
                self.koniec = true;
            }
        }
        self.samotest = Some(t);
    }
}

/// Katalóg fotografií (Herbár): `$XDG_DATA_HOME/orchidea/herbar.txt` (inak `~/.local/share/orchidea/`).
fn subor_katalogu() -> Option<PathBuf> {
    let zaklad = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(zaklad.join("orchidea").join("herbar.txt"))
}

/// Spustí okno. `argumenty`: [cesta] alebo `--samotest <cesta> <výstup.png> [povely …]`.
pub fn spusti(argumenty: &[String]) -> i32 {
    if !wayland::je_k_dispozicii() {
        eprintln!(
            "{}",
            chyba(
                "okno potrebuje kompozitor Wayland (premenná WAYLAND_DISPLAY nie je nastavená). Okno pre X11 zatiaľ nie je."
            )
        );
        return 1;
    }
    let z = match wayland::pripoj() {
        Ok(z) => z,
        Err(e) => {
            eprintln!("{}", chyba(format!("pripojenie na kompozitor Wayland: {e}")));
            return 1;
        }
    };
    spusti_na(z, argumenty, subor_katalogu())
}

/// Okno na danom spojení s kompozitorom (v skúškach aj so zdanlivým kompozitorom).
fn spusti_na(z: std::os::unix::net::UnixStream, argumenty: &[String], katalog: Option<PathBuf>) -> i32 {
    let (cesta, samotest) = okno::argumenty(argumenty);
    let (tx, rx): (Sender<Sprava>, Receiver<Sprava>) = channel();
    let citanie = match z.try_clone() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", chyba(e));
            return 1;
        }
    };
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            let mut sk = wayland::Skladac::default();
            let mut fds = Vec::new();
            loop {
                match sys::prijmi(&citanie, &mut buf, &mut fds) {
                    Ok(0) => {
                        let _ = tx.send(Sprava::Koniec(chyba("kompozitor ukončil spojenie")));
                        return;
                    }
                    Ok(n) => {
                        for fd in fds.drain(..) {
                            if tx.send(Sprava::Fd(fd)).is_err() {
                                return;
                            }
                        }
                        sk.pridaj(&buf[..n]);
                        while let Some(u) = sk.dalsia() {
                            let s = match u {
                                Ok(u) => Sprava::Wayland(u),
                                Err(e) => Sprava::Koniec(chyba(e)),
                            };
                            let koniec = matches!(s, Sprava::Koniec(_));
                            if tx.send(s).is_err() || koniec {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Sprava::Koniec(chyba(format!("čítanie od kompozitora: {e}"))));
                        return;
                    }
                }
            }
        });
    }
    let zobud = {
        let tx = tx.clone();
        Arc::new(move || {
            let _ = tx.send(Sprava::Hotove);
        })
    };
    let mut jadro = Prehliadac::novy(zobud);
    if let Some(k) = katalog {
        jadro.nastav_katalog(k);
    }
    let mut o = Okno {
        sp: Spojenie::nove(z),
        objekty: HashMap::from([(1, Druh::Display)]),
        globaly: Vec::new(),
        synchronizovane: false,
        compositor: 0,
        verzia_compositor: 0,
        shm: 0,
        wm_base: 0,
        seat: 0,
        data_manager: 0,
        dekor_manager: 0,
        tvar_manager: 0,
        surface: 0,
        xdg_surface: 0,
        toplevel: 0,
        pointer: 0,
        keyboard: 0,
        data_device: 0,
        tvar: 0,
        kurzor: 0,
        kurzor_obraz: None,
        vyrovnavace: Vec::new(),
        fds: Popisovace::new(),
        tx,
        cakajuca: None,
        sirka: 1280,
        vyska: 860,
        maximalizovane: false,
        cela: false,
        dlazdica: false,
        aktivne: true,
        server_dekoruje: false,
        nakonfigurovane: false,
        snimok_caka: false,
        snimkov: 0,
        prekreslit: true,
        chce_celu: false,
        mys: (0.0, 0.0),
        mys_serial: 0,
        tahanie: None,
        delenie: false,
        posun: None,
        posledny_klik: (0, -100.0, -100.0),
        koliesko: (0.0, 0),
        pod_mysou: None,
        tvar_kurzora: 1,
        klavesnica: Klavesnica::zakladna(),
        opakovanie: (25, 600),
        opakuj: None,
        serial: 0,
        ponuky: HashMap::new(),
        dnd: None,
        jadro,
        platno: Platno::nove(1, 1, 0),
        lista: Platno::nove(1, 1, 0),
        pismo: Pismo::stonka(),
        titulok: String::new(),
        casovac: None,
        samotest_dalsi: samotest.is_some(),
        samotest,
        koniec: false,
        kod: 0,
        obraz_okna: Vec::new(),
        dialog_otvoreny: false,
    };
    if o.samotest.is_some() {
        // samotest nečaká na človeka – dialógy sa len započítajú
        o.jadro.povol_dialogy();
    } else {
        let tx = o.tx.clone();
        std::thread::spawn(move || {
            if portal::je_k_dispozicii() {
                let _ = tx.send(Sprava::Dialogy);
            }
        });
    }

    // zoznam rozhraní kompozitora (get_registry + sync)
    o.objekty.insert(2, Druh::Registry);
    let _ = o.sp.nove_id();
    o.posli(Ziadost::nova(1, 1).u(2));
    let sync = o.nove(Druh::Synchronizacia);
    o.posli(Ziadost::nova(1, 0).u(sync));
    while !o.synchronizovane && !o.koniec {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(s) => o.sprava(s),
            Err(_) => o.zlyhanie(chyba("kompozitor neodpovedá")),
        }
    }
    (o.compositor, o.verzia_compositor) = o.bind("wl_compositor", 4, Druh::Compositor);
    o.shm = o.bind("wl_shm", 1, Druh::Shm).0;
    o.wm_base = o.bind("xdg_wm_base", 5, Druh::WmBase).0;
    if o.compositor == 0 || o.shm == 0 || o.wm_base == 0 {
        o.zlyhanie(chyba("kompozitor nemá wl_compositor, wl_shm alebo xdg_wm_base"));
    }
    o.seat = o.bind("wl_seat", 7, Druh::Seat).0;
    o.data_manager = o.bind("wl_data_device_manager", 3, Druh::DataManager).0;
    o.dekor_manager = o.bind("zxdg_decoration_manager_v1", 1, Druh::DekorManager).0;
    o.tvar_manager = o.bind("wp_cursor_shape_manager_v1", 1, Druh::TvarManager).0;
    if !o.koniec {
        o.surface = o.nove(Druh::Surface);
        o.posli(Ziadost::nova(o.compositor, 0).u(o.surface));
        o.xdg_surface = o.nove(Druh::XdgSurface);
        o.posli(Ziadost::nova(o.wm_base, 2).u(o.xdg_surface).u(o.surface));
        o.toplevel = o.nove(Druh::Toplevel);
        o.posli(Ziadost::nova(o.xdg_surface, 1).u(o.toplevel));
        o.posli(Ziadost::nova(o.toplevel, 2).s("prehliadač a editor fotografií (Pohľad)"));
        o.posli(Ziadost::nova(o.toplevel, 3).s("orchidea-pohlad"));
        o.posli(Ziadost::nova(o.toplevel, 8).i(480).i(320));
        if o.dekor_manager != 0 {
            let d = o.nove(Druh::Dekoracia);
            o.posli(Ziadost::nova(o.dekor_manager, 1).u(d).u(o.toplevel));
            o.posli(Ziadost::nova(d, 1).u(2));
        }
        if o.seat != 0 && o.data_manager != 0 {
            o.data_device = o.nove(Druh::DataDevice);
            o.posli(Ziadost::nova(o.data_manager, 1).u(o.data_device).u(o.seat));
        }
        o.posli(Ziadost::nova(o.surface, 6));
        let (sirka, vyska) = (o.sirka, o.vyska - LISTA);
        o.jadro.vstup(Vstup::Velkost { sirka, vyska });
        if let Some(c) = &cesta {
            o.jadro.vstup(Vstup::Otvor(c.clone()));
        }
        o.po_zmene(true);
    }

    while !o.koniec {
        o.kresli();
        o.samotest_krok();
        if o.koniec {
            break;
        }
        if o.samotest_dalsi && o.snimkov > 0 {
            // ďalší krok samotestu po spracovaní čakajúcich udalostí
            while let Ok(s) = rx.try_recv() {
                o.sprava(s);
            }
            o.terminy();
            continue;
        }
        let s = match o.najblizsi_termin() {
            Some(t) => match rx.recv_timeout(t.saturating_duration_since(Instant::now())) {
                Ok(s) => Some(s),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(s) => Some(s),
                Err(_) => break,
            },
        };
        if let Some(s) = s {
            o.sprava(s);
        }
        while let Ok(s) = rx.try_recv() {
            o.sprava(s);
        }
        o.terminy();
        o.dialogy();
    }
    // zmeny katalógu fotografií na disk (súbory XMP sú zapísané hneď)
    o.jadro.pri_zatvoreni();
    o.kod
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn uri_pretiahnutych_suborov() {
        let c = cesty_z_uri(
            "# komentár\r\nfile:///home/a/Fotky/k%C3%BA%20pa.jpg\r\nfile://pc/tmp/b.png\nhttp://x/y\n",
        );
        assert_eq!(c, vec![PathBuf::from("/home/a/Fotky/kú pa.jpg"), PathBuf::from("/tmp/b.png")]);
        assert!(cesty_z_uri("").is_empty());
        assert_eq!(cesty_z_uri("file:///a%2"), vec![PathBuf::from("/a%2")]);
    }

    #[test]
    fn sipka_kurzora_ma_obrys_a_vypln() {
        let s = sipka();
        assert_eq!(s.len(), 16 * 24);
        assert!(s.iter().filter(|&&b| b == 0xFFFF_FFFF).count() > 40);
        assert!(s.iter().filter(|&&b| b == 0xFF00_0000).count() > 30);
        assert_eq!(s[0], 0);
    }

    #[test]
    fn docasny_subor_je_hned_zmazany() {
        let f = docasny_subor(4096).unwrap();
        assert_eq!(f.metadata().unwrap().len(), 4096);
        f.write_all_at(&[1, 2, 3], 100).unwrap();
        let mut b = [0u8; 3];
        f.read_exact_at(&mut b, 100).unwrap();
        assert_eq!(b, [1, 2, 3]);
    }
}
