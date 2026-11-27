//! Okno prehliadača a editora fotografií (Pohľad) pre Windows – vlastné väzby na user32, gdi32 a shell32 (prvky okna Windows – platforma).
//!
//! Chladné: slučka `GetMessageW` spí, kým nepríde správa; žiadny časovač. Pracovné vlákno jadra pošle
//! `WM_HOTOVE` (PostMessageW), keď dokončí prácu. Kreslí sa len pri `WM_PAINT` a len vtedy, keď jadro povie,
//! že sa niečo zmenilo; inak sa do okna skopíruje posledný obraz (SetDIBitsToDevice, bez prepočtu).
//!
//! Ovládanie: šípky / medzera / Page Up/Down – ďalší a predošlý, Home/End, koliesko a +/− – priblíženie okolo
//! kurzora, ťahanie ľavým tlačidlom – posun, 0 alebo * – zmestiť, 1 – skutočná veľkosť, M – označiť A,
//! C – režim porovnania (delený / rozdiel), pravé tlačidlo – posun deliacej čiary, T – pás náhľadov,
//! I – riadok s údajmi, F5 / F11 – prezentácia na celú obrazovku (každých 5 s ďalší obraz, P pozastaví,
//! Esc ukončí; časovač beží len počas prezentácie), Esc – koniec. Úpravy: R / Shift+R otočiť, F zrkadliť, X orezať na zobrazený výrez,
//! J/K/G (Shift = menej) jas, kontrast, gama, Z zmenšiť na polovicu, Ctrl+Z späť, Ctrl+Y znova, Ctrl+S uložiť
//! vedľa pôvodného súboru (JPEG s otočením a orezaním bezstratovo). Súbor alebo priečinok sa dá do okna pretiahnuť.
//! Všetky úpravy sú aj na paneli nástrojov nad obrazom (kreslí ho jadro); F1 alebo ? – nápoveda klávesov,
//! Tab – skryť / ukázať panel. Esc najprv zavrie nápovedu alebo prezentáciu, až potom okno.

#![allow(unsafe_code)]

use crate::okno::{self, Dalej, Samotest, povel_znaku};
use crate::prehliadac::{Povel, Prehliadac, Vstup, ZiadostOkna};
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
    fn SendMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
    fn GetKeyState(vk: i32) -> i16;
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
    fn GetWindowLongPtrW(h: Hwnd, index: i32) -> isize;
    fn SetWindowLongPtrW(h: Hwnd, index: i32, hodnota: isize) -> isize;
    fn SetWindowPos(h: Hwnd, za: Hwnd, x: i32, y: i32, w: i32, v: i32, priznaky: u32) -> i32;
    fn GetWindowRect(h: Hwnd, r: *mut Rect) -> i32;
    fn MonitorFromWindow(h: Hwnd, priznaky: u32) -> isize;
    fn GetMonitorInfoW(m: isize, info: *mut MonitorInfo) -> i32;
    fn SetTimer(h: Hwnd, id: usize, ms: u32, f: *const c_void) -> usize;
    fn KillTimer(h: Hwnd, id: usize) -> i32;
    fn ShowCursor(ukaz: i32) -> i32;
}

#[repr(C)]
#[derive(Default)]
struct MonitorInfo {
    velkost: u32,
    monitor: Rect,
    praca: Rect,
    priznaky: u32,
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

/// Dialóg „Otvoriť“ (comdlg32, OPENFILENAMEW).
#[repr(C)]
struct OtvorSuborW {
    velkost: u32,
    vlastnik: Hwnd,
    instancia: isize,
    filter: *const u16,
    vlastny_filter: *mut u16,
    max_vlastny_filter: u32,
    index_filtra: u32,
    subor: *mut u16,
    max_subor: u32,
    nazov_suboru: *mut u16,
    max_nazov_suboru: u32,
    pociatocny_priecinok: *const u16,
    titulok: *const u16,
    priznaky: u32,
    posun_mena: u16,
    posun_pripony: u16,
    predvolena_pripona: *const u16,
    data: isize,
    hak: *const c_void,
    sablona: *const u16,
    rezerva: *mut c_void,
    rezerva2: u32,
    priznaky_ex: u32,
}

/// Výber priečinka (shell32, BROWSEINFOW).
#[repr(C)]
struct VyberPriecinkaW {
    vlastnik: Hwnd,
    koren: *const c_void,
    meno: *mut u16,
    titulok: *const u16,
    priznaky: u32,
    spatne_volanie: Option<unsafe extern "system" fn(Hwnd, u32, isize, isize) -> i32>,
    data: isize,
    obrazok: i32,
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(o: *mut OtvorSuborW) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHBrowseForFolderW(b: *const VyberPriecinkaW) -> *mut c_void;
    fn SHGetPathFromIDListW(pidl: *const c_void, cesta: *mut u16) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(rezerva: *const c_void, model: u32) -> i32;
    fn CoTaskMemFree(p: *mut c_void);
}

const OFN_NOCHANGEDIR: u32 = 0x8;
const OFN_PATHMUSTEXIST: u32 = 0x800;
const OFN_FILEMUSTEXIST: u32 = 0x1000;
const OFN_EXPLORER: u32 = 0x8_0000;
const BIF_RETURNONLYFSDIRS: u32 = 0x1;
const BIF_NEWDIALOGSTYLE: u32 = 0x40;
const BFFM_INITIALIZED: u32 = 1;
const BFFM_SETSELECTIONW: u32 = 0x0467;

fn z_utf16(b: &[u16]) -> PathBuf {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    PathBuf::from(String::from_utf16_lossy(&b[..n]))
}

/// Dialóg „Otvoriť“ – vráti vybraný obrázok.
fn dialog_otvor(hwnd: Hwnd, priecinok: Option<&std::path::Path>) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let filter: Vec<u16> = "Obrázky (JPEG, PNG, TIFF, OpenEXR, JPEG XL)\0*.jpg;*.jpeg;*.jpe;*.png;*.tif;*.tiff;*.exr;*.jxl\0Všetky súbory\0*.*\0\0"
        .encode_utf16()
        .collect();
    let titulok = w("Otvoriť obrázok – prehliadač a editor fotografií (Pohľad)");
    let zaciatok = priecinok.map(|p| w(&p.to_string_lossy()));
    let mut o = OtvorSuborW {
        velkost: std::mem::size_of::<OtvorSuborW>() as u32,
        vlastnik: hwnd,
        instancia: 0,
        filter: filter.as_ptr(),
        vlastny_filter: std::ptr::null_mut(),
        max_vlastny_filter: 0,
        index_filtra: 1,
        subor: buf.as_mut_ptr(),
        max_subor: buf.len() as u32,
        nazov_suboru: std::ptr::null_mut(),
        max_nazov_suboru: 0,
        pociatocny_priecinok: zaciatok.as_ref().map_or(std::ptr::null(), |z| z.as_ptr()),
        titulok: titulok.as_ptr(),
        priznaky: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        posun_mena: 0,
        posun_pripony: 0,
        predvolena_pripona: std::ptr::null(),
        data: 0,
        hak: std::ptr::null(),
        sablona: std::ptr::null(),
        rezerva: std::ptr::null_mut(),
        rezerva2: 0,
        priznaky_ex: 0,
    };
    // SAFETY: štruktúra má rozloženie OPENFILENAMEW, všetky ukazovatele žijú počas volania
    if unsafe { GetOpenFileNameW(&mut o) } == 0 {
        return None;
    }
    Some(z_utf16(&buf))
}

unsafe extern "system" fn vyber_priecinka_init(hwnd: Hwnd, m: u32, _lp: isize, data: isize) -> i32 {
    if m == BFFM_INITIALIZED && data != 0 {
        // SAFETY: data je ukazovateľ na reťazec UTF-16 zakončený nulou, žije počas dialógu
        unsafe { SendMessageW(hwnd, BFFM_SETSELECTIONW, 1, data) };
    }
    0
}

/// Výber priečinka (na pridanie do katalógu).
fn dialog_priecinok(hwnd: Hwnd, priecinok: Option<&std::path::Path>) -> Option<PathBuf> {
    let titulok = w(
        "Vyberte priečinok s fotografiami – pridá sa do katalógu fotografií (Herbár), fotky ostanú na mieste",
    );
    let zaciatok = priecinok.map(|p| w(&p.to_string_lossy()));
    let mut meno = vec![0u16; 260];
    let b = VyberPriecinkaW {
        vlastnik: hwnd,
        koren: std::ptr::null(),
        meno: meno.as_mut_ptr(),
        titulok: titulok.as_ptr(),
        priznaky: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        spatne_volanie: Some(vyber_priecinka_init),
        data: zaciatok.as_ref().map_or(0, |z| z.as_ptr() as isize),
        obrazok: 0,
    };
    // SAFETY: štruktúra má rozloženie BROWSEINFOW; vrátený zoznam ID sa uvoľní cez CoTaskMemFree
    unsafe {
        let pidl = SHBrowseForFolderW(&b);
        if pidl.is_null() {
            return None;
        }
        let mut cesta = vec![0u16; 32768];
        let ok = SHGetPathFromIDListW(pidl, cesta.as_mut_ptr());
        CoTaskMemFree(pidl);
        (ok != 0).then(|| z_utf16(&cesta))
    }
}

/// Ukáže dialóg, o ktorý jadro požiadalo (mimo požičania stavu – dialóg má vlastnú slučku správ).
fn ukaz_dialog(hwnd: Hwnd) {
    let Some((z, samotest)) =
        s_stavom(|s| s.jadro.vezmi_ziadost_okna().map(|z| (z, s.samotest.is_some()))).flatten()
    else {
        return;
    };
    if samotest {
        // samotest nesmie čakať na človeka – dialóg sa len zaznamená
        s_stavom(|s| {
            if let Some(t) = s.samotest.as_mut() {
                t.dialogy += 1;
            }
        });
        return;
    }
    // tlačidlo myši je ešte dole – dialóg nesmie zdediť zachytenie myši ani ťahanie
    unsafe { ReleaseCapture() };
    s_stavom(|s| s.tahanie = None);
    let vyber = match z {
        ZiadostOkna::OtvorSubor { priecinok } => dialog_otvor(hwnd, priecinok.as_deref()),
        ZiadostOkna::VyberPriecinok { priecinok } => dialog_priecinok(hwnd, priecinok.as_deref()),
    };
    if let Some(c) = vyber {
        s_stavom(|s| vstup(s, Vstup::Otvor(c)));
    }
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
const WM_TIMER: u32 = 0x0113;
const GWL_STYLE: i32 = -16;
const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_NOZORDER: u32 = 0x0004;
const MONITOR_DEFAULTTONEAREST: u32 = 2;
const CASOVAC_PREZENTACIE: usize = 1;
const WM_HOTOVE: u32 = 0x8001;
const WM_SAMOTEST: u32 = 0x8002;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
const IDC_ARROW: usize = 32512;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
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
    /// Celá obrazovka: pôvodný štýl a poloha okna.
    cela_obrazovka: Option<(isize, Rect)>,
    /// Bežiaci časovač prezentácie (ms).
    casovac: Option<u32>,
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

/// Celá obrazovka a časovač podľa jadra (časovač beží len počas prezentácie).
fn synchronizuj(s: &mut Stav) {
    let chce = s.jadro.prezentuje();
    unsafe {
        if chce && s.cela_obrazovka.is_none() {
            let styl = GetWindowLongPtrW(s.hwnd, GWL_STYLE);
            let mut r = Rect::default();
            GetWindowRect(s.hwnd, &mut r);
            let mut mi =
                MonitorInfo { velkost: std::mem::size_of::<MonitorInfo>() as u32, ..Default::default() };
            GetMonitorInfoW(MonitorFromWindow(s.hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
            SetWindowLongPtrW(s.hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE) as isize);
            let m = mi.monitor;
            SetWindowPos(s.hwnd, 0, m.l, m.t, m.r - m.l, m.b - m.t, SWP_FRAMECHANGED | SWP_NOZORDER);
            ShowCursor(0);
            s.cela_obrazovka = Some((styl, r));
        } else if !chce && let Some((styl, r)) = s.cela_obrazovka.take() {
            SetWindowLongPtrW(s.hwnd, GWL_STYLE, styl);
            SetWindowPos(s.hwnd, 0, r.l, r.t, r.r - r.l, r.b - r.t, SWP_FRAMECHANGED | SWP_NOZORDER);
            ShowCursor(1);
        }
        let casovac = s.jadro.casovac();
        if casovac != s.casovac {
            KillTimer(s.hwnd, CASOVAC_PREZENTACIE);
            if let Some(ms) = casovac {
                SetTimer(s.hwnd, CASOVAC_PREZENTACIE, ms, std::ptr::null());
            }
            s.casovac = casovac;
        }
    }
}

/// Po zmene stavu: titulok a prípadné prekreslenie.
fn po_zmene(s: &mut Stav, prekreslit: bool) {
    synchronizuj(s);
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
        0x26 => Povel::Hore,
        0x28 => Povel::Dole,
        0x72 => Povel::Katalog,
        0x6B | 0xBB => Povel::Priblizit,
        0x6D | 0xBD => Povel::Oddialit,
        _ => return None,
    })
}

/// Pokračuje samotest: ak jadro nepracuje, použije ďalší povel; po poslednom uloží snímku a zavrie okno.
fn samotest_krok(s: &mut Stav) {
    let Some(mut t) = s.samotest.take() else { return };
    match t.krok(&mut s.jadro) {
        Dalej::Cakaj => {}
        Dalej::Vstupy(v) => {
            for v in v {
                vstup(s, v);
            }
            unsafe { PostMessageW(s.hwnd, WM_SAMOTEST, 0, 0) };
        }
        Dalej::Koniec => {
            t.dokonci(&mut s.jadro, &mut s.platno);
            unsafe { PostMessageW(s.hwnd, WM_CLOSE, 0, 0) };
        }
    }
    s.samotest = Some(t);
}

unsafe extern "system" fn wnd_proc(hwnd: Hwnd, m: u32, wp: usize, lp: isize) -> isize {
    // SAFETY: rovnaké parametre ako od systému
    let v = unsafe { spracuj_spravu(hwnd, m, wp, lp) };
    // žiadosť o dialóg môže vzniknúť po kláves, kliknutí aj po kroku samotestu (WM_HOTOVE, WM_SAMOTEST)
    if m != WM_PAINT && m != WM_DESTROY {
        ukaz_dialog(hwnd);
    }
    v
}

unsafe fn spracuj_spravu(hwnd: Hwnd, m: u32, wp: usize, lp: isize) -> isize {
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
        WM_TIMER => {
            if wp == CASOVAC_PREZENTACIE {
                s_stavom(|s| vstup(s, Vstup::Tik));
            }
            0
        }
        WM_KEYDOWN => {
            if wp == 0x1B {
                // Esc najprv zavrie nápovedu alebo prezentáciu, až potom okno
                let zavrene = s_stavom(|s| {
                    let z = s.jadro.esc();
                    po_zmene(s, z);
                    z
                })
                .unwrap_or(false);
                if !zavrene {
                    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
                }
            } else if wp == 0x70 {
                // F1 – nápoveda klávesov
                s_stavom(|s| vstup(s, Vstup::Povel(Povel::Napoveda)));
            } else if let Some(n) = (unsafe { GetKeyState(0x11) } < 0)
                .then(|| match wp {
                    0x30..=0x35 => Some((wp - 0x30) as i8),
                    0x60..=0x65 => Some((wp - 0x60) as i8),
                    _ => None,
                })
                .flatten()
            {
                // Ctrl+0 … Ctrl+5 – hodnotenie (do súboru XMP vedľa fotky)
                s_stavom(|s| vstup(s, Vstup::Povel(Povel::Hodnotenie(n))));
            } else if wp == 0x74 || wp == 0x7A {
                // F5 / F11 – prezentácia na celú obrazovku
                s_stavom(|s| vstup(s, Vstup::Povel(Povel::Prezentacia)));
            } else if let Some(p) = povel_klavesu(wp) {
                // pri písaní do poľa hľadania katalógu len šípky, Page Up/Down a F3
                let pise = s_stavom(|s| s.jadro.pise()).unwrap_or(false);
                if !pise || matches!(wp, 0x21..=0x28 | 0x72) {
                    s_stavom(|s| vstup(s, Vstup::Povel(p)));
                }
            }
            0
        }
        WM_CHAR => {
            let pise = s_stavom(|s| s.jadro.pise()).unwrap_or(false);
            if let Some(c) = char::from_u32(wp as u32) {
                if pise {
                    s_stavom(|s| vstup(s, Vstup::Znak(c)));
                } else if let Some(p) = povel_znaku(c) {
                    s_stavom(|s| vstup(s, Vstup::Povel(p)));
                }
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
                } else {
                    vstup(s, Vstup::Mys { x: x as f32, y: y as f32 });
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
            // zmeny katalógu fotografií na disk (súbory XMP sú zapísané hneď)
            s_stavom(|s| s.jadro.pri_zatvoreni());
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, m, wp, lp) },
    }
}

/// Spustí okno. `argumenty`: [cesta] alebo `--samotest <cesta> <výstup.png> [povely …]`.
pub fn spusti(argumenty: &[String]) -> i32 {
    let (cesta, samotest) = okno::argumenty(argumenty);
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
        let nazov = w("prehliadač a editor fotografií (Pohľad)");
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
            eprintln!("prehliadač a editor fotografií (Pohľad): okno sa nepodarilo vytvoriť");
            return 1;
        }
        let ciel = hwnd;
        // pracovné vlákno zobudí okno správou (žiadny časovač)
        let zobud = Arc::new(move || {
            PostMessageW(ciel, WM_HOTOVE, 0, 0);
        });
        let mut r = Rect::default();
        GetClientRect(hwnd, &mut r);
        // výber priečinka v novom vzhľade potrebuje COM v tomto vlákne
        CoInitializeEx(std::ptr::null(), 2);
        let mut jadro = Prehliadac::novy(zobud);
        jadro.povol_dialogy();
        // katalóg fotografií (Herbár): %APPDATA%\ORCHIDEA\herbar.txt, inak vedľa programu
        let katalog = std::env::var_os("APPDATA")
            .map(|a| PathBuf::from(a).join("ORCHIDEA"))
            .or_else(|| std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())))
            .map(|p| p.join("herbar.txt"));
        if let Some(k) = katalog {
            jadro.nastav_katalog(k);
        }
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
                cela_obrazovka: None,
                casovac: None,
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
