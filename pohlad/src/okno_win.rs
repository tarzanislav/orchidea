//! Okno Pohľadu pre Windows – vlastné väzby na user32, gdi32 a shell32 (prvky okna Windows – platforma).
//!
//! Chladné: slučka `GetMessageW` spí, kým nepríde správa; žiadny časovač. Pracovné vlákno jadra pošle
//! `WM_HOTOVE` (PostMessageW), keď dokončí prácu. Kreslí sa len pri `WM_PAINT` a len vtedy, keď jadro povie,
//! že sa niečo zmenilo; inak sa do okna skopíruje posledný obraz (SetDIBitsToDevice, bez prepočtu).
//!
//! Ovládanie: šípky / medzera / Page Up/Down – ďalší a predošlý, Home/End, koliesko a +/− – priblíženie okolo
//! kurzora, ťahanie ľavým tlačidlom – posun, 0 alebo * – zmestiť, 1 – skutočná veľkosť, M – označiť A,
//! C – režim porovnania (delený / rozdiel), pravé tlačidlo – posun deliacej čiary, T – pás náhľadov,
//! I – riadok s údajmi, Esc – koniec. Súbor alebo priečinok sa dá do okna pretiahnuť.

#![allow(unsafe_code)]

use crate::prehliadac::{Povel, Prehliadac, Vstup};
use orchidea_pismo::Platno;
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;

type Hwnd = isize;

#[repr(C)]
struct WndClassExW {
    cb_size: u32,
    styl: u32,
    proc_: Option<unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize>,
    cls_extra: i32,
    wnd_extra: i32,
    instancia: isize,
    ikona: isize,
    kurzor: isize,
    pozadie: isize,
    menu: *const u16,
    meno_triedy: *const u16,
    ikona_mala: isize,
}

#[repr(C)]
#[derive(Default)]
struct Msg {
    hwnd: Hwnd,
    sprava: u32,
    wparam: usize,
    lparam: isize,
    cas: u32,
    x: i32,
    y: i32,
    sukromne: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
}

#[repr(C)]
struct PaintStruct {
    hdc: isize,
    zmaz: i32,
    rc: Rect,
    obnov: i32,
    prirastok: i32,
    rezerva: [u8; 32],
}

#[repr(C)]
struct BitmapInfo {
    velkost: u32,
    sirka: i32,
    vyska: i32,
    roviny: u16,
    bitov: u16,
    kompresia: u32,
    velkost_obrazu: u32,
    xppm: i32,
    yppm: i32,
    farieb: u32,
    dolezitych: u32,
    farby: [u32; 1],
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(wc: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex: u32,
        trieda: *const u16,
        nazov: *const u16,
        styl: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        rodic: Hwnd,
        id: isize,
        inst: isize,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, h: Hwnd, od: u32, po: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(kod: i32);
    fn PostMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> i32;
    fn ShowWindow(h: Hwnd, prikaz: i32) -> i32;
    fn UpdateWindow(h: Hwnd) -> i32;
    fn DestroyWindow(h: Hwnd) -> i32;
    fn SetWindowTextW(h: Hwnd, text: *const u16) -> i32;
    fn LoadCursorW(inst: isize, meno: *const u16) -> isize;
    fn BeginPaint(h: Hwnd, ps: *mut PaintStruct) -> isize;
    fn EndPaint(h: Hwnd, ps: *const PaintStruct) -> i32;
    fn GetClientRect(h: Hwnd, r: *mut Rect) -> i32;
    fn InvalidateRect(h: Hwnd, r: *const Rect, zmaz: i32) -> i32;
    fn SetCapture(h: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
    fn ScreenToClient(h: Hwnd, p: *mut [i32; 2]) -> i32;
    fn SetProcessDpiAwarenessContext(k: isize) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn SetDIBitsToDevice(
        hdc: isize,
        x: i32,
        y: i32,
        w: u32,
        v: u32,
        sx: i32,
        sy: i32,
        start: u32,
        riadky: u32,
        bity: *const c_void,
        bmi: *const BitmapInfo,
        pouzitie: u32,
    ) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn DragAcceptFiles(h: Hwnd, prijmi: i32);
    fn DragQueryFileW(hdrop: isize, i: u32, buf: *mut u16, n: u32) -> u32;
    fn DragFinish(hdrop: isize);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(meno: *const u16) -> isize;
}

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_DROPFILES: u32 = 0x0233;
const WM_HOTOVE: u32 = 0x8001;
const WM_SAMOTEST: u32 = 0x8002;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
const IDC_ARROW: usize = 32512;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Samotest v CI: povely, po ktorých sa uloží snímka okna.
struct Samotest {
    povely: Vec<Vstup>,
    vystup: PathBuf,
    kod: i32,
}

struct Stav {
    hwnd: Hwnd,
    jadro: Prehliadac,
    platno: Platno,
    /// Platno vo formáte BGRA pre SetDIBitsToDevice.
    bgra: Vec<u32>,
    tahanie: Option<(i32, i32)>,
    delenie: bool,
    titulok: String,
    samotest: Option<Samotest>,
}

thread_local! {
    static STAV: RefCell<Option<Stav>> = const { RefCell::new(None) };
}

fn s_stavom<R>(f: impl FnOnce(&mut Stav) -> R) -> Option<R> {
    STAV.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.as_mut().map(f)))
}

fn xy(l: isize) -> (i32, i32) {
    ((l & 0xFFFF) as i16 as i32, ((l >> 16) & 0xFFFF) as i16 as i32)
}

/// Po zmene stavu: titulok a prípadné prekreslenie.
fn po_zmene(s: &mut Stav, prekreslit: bool) {
    let t = s.jadro.titulok();
    if t != s.titulok {
        s.titulok = t;
        let t = w(&s.titulok);
        unsafe { SetWindowTextW(s.hwnd, t.as_ptr()) };
    }
    if prekreslit || s.jadro.potrebuje_prekreslit() {
        unsafe { InvalidateRect(s.hwnd, std::ptr::null(), 0) };
    }
}

fn vstup(s: &mut Stav, v: Vstup) {
    let z = s.jadro.vstup(v);
    po_zmene(s, z);
}

fn povel_klavesu(vk: usize) -> Option<Povel> {
    Some(match vk {
        0x27 | 0x20 | 0x22 => Povel::Dalsi,
        0x25 | 0x08 | 0x21 => Povel::Predosly,
        0x24 => Povel::Prvy,
        0x23 => Povel::Posledny,
        0x6B | 0xBB => Povel::Priblizit,
        0x6D | 0xBD => Povel::Oddialit,
        _ => return None,
    })
}

fn povel_znaku(c: char) -> Option<Povel> {
    Some(match c.to_ascii_lowercase() {
        '0' | '*' => Povel::Prisposobit,
        '1' => Povel::Skutocna,
        'm' => Povel::Oznac,
        'c' => Povel::Rezim,
        't' => Povel::Pas,
        'i' => Povel::Udaje,
        _ => return None,
    })
}

/// Pokračuje samotest: ak jadro nepracuje, použije ďalší povel; po poslednom uloží snímku a zavrie okno.
fn samotest_krok(s: &mut Stav) {
    if s.jadro.pracuje() {
        return;
    }
    let Some(t) = s.samotest.as_mut() else { return };
    if !t.povely.is_empty() {
        let v = t.povely.remove(0);
        vstup(s, v);
        unsafe { PostMessageW(s.hwnd, WM_SAMOTEST, 0, 0) };
        return;
    }
    s.jadro.vykresli(&mut s.platno);
    let body: Vec<u8> =
        s.platno.body.iter().flat_map(|&f| [(f >> 16) as u8, (f >> 8) as u8, f as u8]).collect();
    let ok = orchidea_formaty::Obraz::novy(
        s.platno.sirka,
        s.platno.vyska,
        orchidea_formaty::obraz::Model::Rgb,
        false,
        orchidea_formaty::obraz::Vzorky::U8(body),
    )
    .and_then(|o| orchidea_formaty::zapis(&o, orchidea_formaty::Format::Png))
    .map(|d| std::fs::write(&t.vystup, d).is_ok())
    .unwrap_or(false);
    t.kod = if ok && s.jadro.pocet() > 0 { 0 } else { 2 };
    println!("SAMOTEST vykreslení {} obrázkov {} kód {}", s.jadro.vykresleni, s.jadro.pocet(), t.kod);
    unsafe { PostMessageW(s.hwnd, WM_CLOSE, 0, 0) };
}

unsafe extern "system" fn wnd_proc(hwnd: Hwnd, m: u32, wp: usize, lp: isize) -> isize {
    match m {
        WM_HOTOVE => {
            s_stavom(|s| {
                let z = s.jadro.spracuj_pozadie();
                po_zmene(s, z);
                if s.samotest.is_some() {
                    samotest_krok(s);
                }
            });
            0
        }
        WM_SAMOTEST => {
            s_stavom(samotest_krok);
            0
        }
        WM_SIZE => {
            let (sirka, vyska) = ((lp & 0xFFFF) as usize, ((lp >> 16) & 0xFFFF) as usize);
            s_stavom(|s| vstup(s, Vstup::Velkost { sirka, vyska }));
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps = PaintStruct {
                hdc: 0,
                zmaz: 0,
                rc: Rect::default(),
                obnov: 0,
                prirastok: 0,
                rezerva: [0; 32],
            };
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                s_stavom(|s| {
                    if s.jadro.potrebuje_prekreslit()
                        || s.bgra.len() != s.platno.body.len()
                        || s.bgra.is_empty()
                    {
                        s.jadro.vykresli(&mut s.platno);
                        // 0xRRGGBB je v pamäti little-endian ako B, G, R, 0 – presne BGRA pre DIB
                        s.bgra.clear();
                        s.bgra.extend_from_slice(&s.platno.body);
                    }
                    let (pw, ph) = (s.platno.sirka, s.platno.vyska);
                    let bmi = BitmapInfo {
                        velkost: 40,
                        sirka: pw as i32,
                        vyska: -(ph as i32),
                        roviny: 1,
                        bitov: 32,
                        kompresia: 0,
                        velkost_obrazu: 0,
                        xppm: 0,
                        yppm: 0,
                        farieb: 0,
                        dolezitych: 0,
                        farby: [0],
                    };
                    SetDIBitsToDevice(
                        hdc,
                        0,
                        0,
                        pw as u32,
                        ph as u32,
                        0,
                        0,
                        0,
                        ph as u32,
                        s.bgra.as_ptr() as *const c_void,
                        &bmi,
                        0,
                    );
                });
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_KEYDOWN => {
            if wp == 0x1B {
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            } else if let Some(p) = povel_klavesu(wp) {
                s_stavom(|s| vstup(s, Vstup::Povel(p)));
            }
            0
        }
        WM_CHAR => {
            if let Some(p) = char::from_u32(wp as u32).and_then(povel_znaku) {
                s_stavom(|s| vstup(s, Vstup::Povel(p)));
            }
            0
        }
        WM_MOUSEWHEEL => {
            let krokov = ((wp >> 16) as u16 as i16) as f32 / 120.0;
            let mut bod = [xy(lp).0, xy(lp).1];
            unsafe { ScreenToClient(hwnd, &mut bod) };
            s_stavom(|s| vstup(s, Vstup::Koliesko { krokov, x: bod[0] as f32, y: bod[1] as f32 }));
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = xy(lp);
            unsafe { SetCapture(hwnd) };
            s_stavom(|s| {
                s.tahanie = Some((x, y));
                vstup(s, Vstup::Klik { x: x as f32, y: y as f32 });
            });
            0
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            unsafe { ReleaseCapture() };
            s_stavom(|s| {
                s.tahanie = None;
                s.delenie = false;
            });
            0
        }
        WM_RBUTTONDOWN => {
            let (x, _) = xy(lp);
            unsafe { SetCapture(hwnd) };
            s_stavom(|s| {
                s.delenie = true;
                vstup(s, Vstup::Delic { x: x as f32 });
            });
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            s_stavom(|s| {
                if let Some((px, py)) = s.tahanie {
                    s.tahanie = Some((x, y));
                    vstup(s, Vstup::Tahaj { dx: (x - px) as f32, dy: (y - py) as f32 });
                } else if s.delenie {
                    vstup(s, Vstup::Delic { x: x as f32 });
                }
            });
            0
        }
        WM_DROPFILES => {
            let mut buf = vec![0u16; 32768];
            let n = unsafe { DragQueryFileW(wp as isize, 0, buf.as_mut_ptr(), buf.len() as u32) } as usize;
            unsafe { DragFinish(wp as isize) };
            if n > 0 {
                let cesta = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
                s_stavom(|s| vstup(s, Vstup::Otvor(cesta)));
            }
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, m, wp, lp) },
    }
}

/// Spustí okno. `argumenty`: [cesta] alebo `--samotest <cesta> <výstup.png> [povely …]`.
pub fn spusti(argumenty: &[String]) -> i32 {
    let (cesta, samotest) = match argumenty.first().map(String::as_str) {
        Some("--samotest") if argumenty.len() >= 3 => {
            let povely = argumenty[3..]
                .iter()
                .filter_map(|p| match p.as_str() {
                    "dalsi" => Some(Vstup::Povel(Povel::Dalsi)),
                    "oznac" => Some(Vstup::Povel(Povel::Oznac)),
                    "rezim" => Some(Vstup::Povel(Povel::Rezim)),
                    "priblizit" => Some(Vstup::Povel(Povel::Priblizit)),
                    _ => None,
                })
                .collect();
            (
                Some(PathBuf::from(&argumenty[1])),
                Some(Samotest { povely, vystup: PathBuf::from(&argumenty[2]), kod: 1 }),
            )
        }
        Some(c) => (Some(PathBuf::from(c)), None),
        None => (None, None),
    };
    unsafe {
        SetProcessDpiAwarenessContext(-4);
        let inst = GetModuleHandleW(std::ptr::null());
        let trieda = w("OrchideaPohlad");
        let wc = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            styl: 0,
            proc_: Some(wnd_proc),
            cls_extra: 0,
            wnd_extra: 0,
            instancia: inst,
            ikona: 0,
            kurzor: LoadCursorW(0, IDC_ARROW as *const u16),
            pozadie: 0,
            menu: std::ptr::null(),
            meno_triedy: trieda.as_ptr(),
            ikona_mala: 0,
        };
        RegisterClassExW(&wc);
        let nazov = w("Pohľad");
        let hwnd = CreateWindowExW(
            0,
            trieda.as_ptr(),
            nazov.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            860,
            0,
            0,
            inst,
            std::ptr::null_mut(),
        );
        if hwnd == 0 {
            eprintln!("Pohľad: okno sa nepodarilo vytvoriť");
            return 1;
        }
        let ciel = hwnd;
        // pracovné vlákno zobudí okno správou (žiadny časovač)
        let zobud = Arc::new(move || {
            PostMessageW(ciel, WM_HOTOVE, 0, 0);
        });
        let mut r = Rect::default();
        GetClientRect(hwnd, &mut r);
        let mut jadro = Prehliadac::novy(zobud);
        jadro
            .vstup(Vstup::Velkost { sirka: (r.r - r.l).max(1) as usize, vyska: (r.b - r.t).max(1) as usize });
        if let Some(c) = &cesta {
            jadro.vstup(Vstup::Otvor(c.clone()));
        }
        let je_samotest = samotest.is_some();
        STAV.with(|s| {
            *s.borrow_mut() = Some(Stav {
                hwnd,
                jadro,
                platno: Platno::nove(1, 1, 0),
                bgra: Vec::new(),
                tahanie: None,
                delenie: false,
                titulok: String::new(),
                samotest,
            })
        });
        s_stavom(|s| po_zmene(s, true));
        DragAcceptFiles(hwnd, 1);
        ShowWindow(hwnd, 5);
        UpdateWindow(hwnd);
        if je_samotest {
            PostMessageW(hwnd, WM_SAMOTEST, 0, 0);
        }
        let mut msg = Msg::default();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let kod = s_stavom(|s| s.samotest.as_ref().map_or(0, |t| t.kod)).unwrap_or(0);
        STAV.with(|s| s.borrow_mut().take());
        kod
    }
}
