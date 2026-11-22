//! Okno SADIČA pre Windows 11 – jednoduché ako Raspberry Pi Imager: 1. obraz, 2. karta, 3. nastavenie,
//! tlačidlo **Zapísať** a ukazovateľ priebehu.
//!
//! Vlastné väzby na user32, gdi32, comctl32 a comdlg32 (prvky okna Windows – platforma); hlavička je
//! nakreslená vlastným písmom STONKA. Zápis beží vo vlastnom vlákne a do okna posiela priebeh.

#![allow(unsafe_code)]

use crate::disk::{self, InfoDisku};
use crate::platforma;
use crate::{Faza, Uloha};
use orchidea_pismo::{Pismo, Platno};
use sklenik_karta::{Nastavenie, navrhni_meno};
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

type Hwnd = isize;

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
}

#[allow(dead_code)]
#[repr(C)]
struct PaintStruct {
    hdc: isize,
    zmaz: i32,
    rc: Rect,
    obnov: i32,
    prirastok: i32,
    rezerva: [u8; 32],
}

#[allow(dead_code)]
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

#[allow(dead_code)]
#[repr(C)]
struct InitCommonControls {
    velkost: u32,
    triedy: u32,
}

#[allow(dead_code)]
#[repr(C)]
struct OpenFileNameW {
    velkost: u32,
    vlastnik: Hwnd,
    instancia: isize,
    filter: *const u16,
    vlastny_filter: *mut u16,
    max_vlastny: u32,
    index_filtra: u32,
    subor: *mut u16,
    max_subor: u32,
    nazov_suboru: *mut u16,
    max_nazov: u32,
    pociatocny: *const u16,
    nadpis: *const u16,
    priznaky: u32,
    posun_suboru: u16,
    posun_pripony: u16,
    pripona: *const u16,
    data: isize,
    hak: *const c_void,
    sablona: *const u16,
    rezerva_ptr: *mut c_void,
    rezerva: u32,
    priznaky_ex: u32,
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
    fn IsDialogMessageW(h: Hwnd, msg: *mut Msg) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(kod: i32);
    fn SendMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
    fn PostMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> i32;
    fn ShowWindow(h: Hwnd, prikaz: i32) -> i32;
    fn UpdateWindow(h: Hwnd) -> i32;
    fn DestroyWindow(h: Hwnd) -> i32;
    fn MessageBoxW(h: Hwnd, text: *const u16, nadpis: *const u16, typ: u32) -> i32;
    fn SetWindowTextW(h: Hwnd, text: *const u16) -> i32;
    fn GetWindowTextW(h: Hwnd, buf: *mut u16, max: i32) -> i32;
    fn GetWindowTextLengthW(h: Hwnd) -> i32;
    fn GetDlgItem(h: Hwnd, id: i32) -> Hwnd;
    fn EnableWindow(h: Hwnd, zapni: i32) -> i32;
    fn LoadCursorW(inst: isize, meno: *const u16) -> isize;
    fn BeginPaint(h: Hwnd, ps: *mut PaintStruct) -> isize;
    fn EndPaint(h: Hwnd, ps: *const PaintStruct) -> i32;
    fn GetClientRect(h: Hwnd, r: *mut Rect) -> i32;
    fn InvalidateRect(h: Hwnd, r: *const Rect, zmaz: i32) -> i32;
    fn SetProcessDpiAwarenessContext(k: isize) -> i32;
    fn GetDpiForWindow(h: Hwnd) -> u32;
    fn GetDpiForSystem() -> u32;
    fn AdjustWindowRectExForDpi(r: *mut Rect, styl: u32, menu: i32, ex: u32, dpi: u32) -> i32;
    fn SetWindowPos(h: Hwnd, za: Hwnd, x: i32, y: i32, w: i32, v: i32, priznaky: u32) -> i32;
    fn MoveWindow(h: Hwnd, x: i32, y: i32, w: i32, v: i32, prekresli: i32) -> i32;
    fn SetTimer(h: Hwnd, id: usize, ms: u32, f: *const c_void) -> usize;
    fn KillTimer(h: Hwnd, id: usize) -> i32;
    fn GetSysColorBrush(index: i32) -> isize;
    fn GetSysColor(index: i32) -> u32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateFontW(
        v: i32,
        s: i32,
        esc: i32,
        ori: i32,
        hrubka: i32,
        kurziva: u32,
        podciarknute: u32,
        preciarknute: u32,
        sada: u32,
        presnost: u32,
        orez: u32,
        kvalita: u32,
        rodina: u32,
        meno: *const u16,
    ) -> isize;
    fn DeleteObject(o: isize) -> i32;
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
    fn SetTextColor(hdc: isize, farba: u32) -> u32;
    fn SetBkColor(hdc: isize, farba: u32) -> u32;
}

#[link(name = "comctl32")]
unsafe extern "system" {
    fn InitCommonControlsEx(i: *const InitCommonControls) -> i32;
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(o: *mut OpenFileNameW) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(meno: *const u16) -> isize;
    fn GetUserDefaultUILanguage() -> u16;
    fn GetConsoleProcessList(zoznam: *mut u32, pocet: u32) -> u32;
    fn FreeConsole() -> i32;
}

// štýly a správy Windows
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_EX_CLIENTEDGE: u32 = 0x200;
const WS_EX_CONTROLPARENT: u32 = 0x1_0000;
const ES_AUTOHSCROLL: u32 = 0x80;
const ES_PASSWORD: u32 = 0x20;
const CBS_DROPDOWNLIST: u32 = 0x3;
const BS_DEFPUSHBUTTON: u32 = 0x1;
const SS_NOPREFIX: u32 = 0x80;
const SS_PATHELLIPSIS: u32 = 0x8000;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
const WM_DESTROY: u32 = 0x2;
const WM_PAINT: u32 = 0xF;
const WM_CLOSE: u32 = 0x10;
const WM_SETFONT: u32 = 0x30;
const WM_COMMAND: u32 = 0x111;
const WM_TIMER: u32 = 0x113;
const WM_CTLCOLORSTATIC: u32 = 0x138;
const WM_DEVICECHANGE: u32 = 0x219;
const WM_DPICHANGED: u32 = 0x2E0;
const WM_APP: u32 = 0x8000;
const WM_PRIEBEH: u32 = WM_APP + 1;
const WM_HOTOVO: u32 = WM_APP + 2;
const WM_SKUSKA: u32 = WM_APP + 3;
const CB_ADDSTRING: u32 = 0x143;
const CB_GETCURSEL: u32 = 0x147;
const CB_RESETCONTENT: u32 = 0x14B;
const CB_SETCURSEL: u32 = 0x14E;
const CB_SETITEMDATA: u32 = 0x151;
const EM_LIMITTEXT: u32 = 0xC5;
const EM_SETCUEBANNER: u32 = 0x1501;
const PBM_SETPOS: u32 = 0x402;
const PBM_SETRANGE32: u32 = 0x406;
const PBM_SETSTATE: u32 = 0x410;
const MB_OKCANCEL: u32 = 0x1;
const MB_YESNO: u32 = 0x4;
const MB_ICONERROR: u32 = 0x10;
const MB_ICONWARNING: u32 = 0x30;
const MB_ICONINFORMATION: u32 = 0x40;
const MB_DEFBUTTON2: u32 = 0x100;
const COLOR_WINDOW: i32 = 5;
const COLOR_GRAYTEXT: i32 = 17;

// prvky okna
const ID_OBRAZ: i32 = 101;
const ID_VYBER: i32 = 102;
const ID_KARTA: i32 = 103;
const ID_OBNOV: i32 = 104;
const ID_MENO: i32 = 105;
const ID_WIFI: i32 = 106;
const ID_HESLO: i32 = 107;
const ID_JAZYK: i32 = 108;
const ID_ZAPIS: i32 = 109;
const ID_PRIEBEH: i32 = 110;
const ID_STAV: i32 = 111;
const ID_PATA: i32 = 115;

const SIRKA: i32 = 600;
const VYSKA: i32 = 600;
const HLAVICKA: i32 = 92;

/// Rozloženie: (id, x, y, šírka, výška) v logických bodoch (pri 96 DPI).
const ROZLOZENIE: &[(i32, i32, i32, i32, i32)] = &[
    (201, 24, 110, 552, 24),
    (ID_OBRAZ, 24, 140, 430, 24),
    (ID_VYBER, 464, 136, 112, 30),
    (202, 24, 184, 552, 24),
    (ID_KARTA, 24, 212, 430, 240),
    (ID_OBNOV, 464, 210, 112, 30),
    (203, 24, 258, 552, 24),
    (211, 24, 292, 120, 24),
    (ID_MENO, 150, 288, 426, 28),
    (212, 24, 326, 120, 24),
    (ID_WIFI, 150, 322, 426, 28),
    (213, 24, 360, 120, 24),
    (ID_HESLO, 150, 356, 426, 28),
    (214, 24, 394, 120, 24),
    (ID_JAZYK, 150, 390, 220, 200),
    (ID_ZAPIS, 24, 438, 552, 46),
    (ID_PRIEBEH, 24, 496, 552, 12),
    (ID_STAV, 24, 516, 552, 44),
    (ID_PATA, 24, 568, 552, 22),
];

const JAZYKY: [(&str, &str); 3] = [("Slovenčina", "sk"), ("Čeština", "cs"), ("English", "en")];

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Skúška v CI: obraz, číslo disku a meno sa vyplnia samy a zápis sa spustí bez otázok.
struct Skuska {
    obraz: String,
    disk: u32,
    meno: String,
}

struct Stav {
    hwnd: Hwnd,
    dpi: u32,
    pismo: Pismo,
    hlavicka: Option<Platno>,
    fonty: Vec<isize>,
    obraz: Option<PathBuf>,
    disky: Vec<InfoDisku>,
    pracuje: bool,
    zrus: Arc<AtomicBool>,
    zavriet: bool,
    dovol_virtualny: bool,
    skuska: Option<Skuska>,
}

thread_local! {
    static STAV: RefCell<Option<Stav>> = const { RefCell::new(None) };
}

static VYSLEDOK: Mutex<Option<Result<String, String>>> = Mutex::new(None);
/// Farba textu stavu (COLORREF) – číta sa pri kreslení prvku bez požičania stavu okna.
static FARBA_STAVU: AtomicU32 = AtomicU32::new(0x0030_3030);
static TEXT_PRIEBEHU: Mutex<String> = Mutex::new(String::new());

fn s_stavom<T>(f: impl FnOnce(&mut Stav) -> T) -> Option<T> {
    STAV.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.as_mut().map(f)))
}

fn prvok(hwnd: Hwnd, id: i32) -> Hwnd {
    // SAFETY: obyčajné volanie Win32 s platným oknom.
    unsafe { GetDlgItem(hwnd, id) }
}

fn nastav_text(h: Hwnd, t: &str) {
    let t = w(t);
    // SAFETY: t je reťazec ukončený nulou, žije počas volania.
    unsafe { SetWindowTextW(h, t.as_ptr()) };
}

fn text(h: Hwnd) -> String {
    // SAFETY: vyrovnávacia pamäť má dĺžku, ktorú Windows vráti, plus nulu.
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let m = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..m])
    }
}

fn sprava(hwnd: Hwnd, text: &str, typ: u32) -> i32 {
    let (t, n) = (w(text), w("SADIČ"));
    // SAFETY: reťazce sú ukončené nulou; MessageBoxW je modálne okno (stav sa počas neho nepožičiava).
    unsafe { MessageBoxW(hwnd, t.as_ptr(), n.as_ptr(), typ) }
}

fn skaluj(v: i32, dpi: u32) -> i32 {
    (v as i64 * dpi as i64 / 96) as i32
}

/// Hľadá obraz SKLENÍKA: priečinok programu, aktuálny priečinok a Stiahnuté súbory; najnovší vyhráva.
fn najdi_obraz() -> Option<PathBuf> {
    let mut miesta: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::current_exe()
        && let Some(d) = p.parent()
    {
        miesta.push(d.to_path_buf());
    }
    if let Ok(d) = std::env::current_dir() {
        miesta.push(d);
    }
    if let Some(u) = std::env::var_os("USERPROFILE") {
        miesta.push(PathBuf::from(u).join("Downloads"));
    }
    let mut najlepsi: Option<(std::time::SystemTime, PathBuf)> = None;
    for m in miesta {
        for z in std::fs::read_dir(&m).into_iter().flatten().flatten() {
            let meno = z.file_name().to_string_lossy().to_lowercase();
            if !(meno.starts_with("sklenik") && (meno.ends_with(".img") || meno.ends_with(".zip"))) {
                continue;
            }
            let cas = z.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            if najlepsi.as_ref().is_none_or(|(c, _)| cas > *c) {
                najlepsi = Some((cas, z.path()));
            }
        }
    }
    najlepsi.map(|(_, p)| p)
}

fn popis_disku(d: &InfoDisku) -> String {
    format!("{} – {} ({})", d.model.trim(), disk::velkost(d.velkost), d.zbernica.popis())
}

fn nakresli_hlavicku(s: &mut Stav, sirka: i32) {
    let v = skaluj(HLAVICKA, s.dpi);
    let k = s.dpi as f32 / 96.0;
    let mut p = Platno::nove(sirka.max(1) as usize, v.max(1) as usize, 0x0F1C15);
    p.text(&s.pismo, "SADIČ", 24.0 * k, 52.0 * k, 30.0 * k, 3.8 * k, 0x6FD08C);
    p.text(&s.pismo, "Karta SKLENÍK pre každé Raspberry Pi", 24.0 * k, 79.0 * k, 12.0 * k, 1.5 * k, 0xE8F0EA);
    let heslo = "Chladné RPi";
    let hw = s.pismo.sirka(heslo, 11.0 * k, 1.3 * k);
    p.text(&s.pismo, heslo, sirka as f32 - 24.0 * k - hw, 52.0 * k, 11.0 * k, 1.3 * k, 0x9DB3A5);
    s.hlavicka = Some(p);
}

fn vytvor_fonty(s: &mut Stav) {
    for f in s.fonty.drain(..) {
        // SAFETY: písmo sme vytvorili my a už sa nepoužíva.
        unsafe { DeleteObject(f) };
    }
    let meno = w("Segoe UI");
    let font = |v: i32, hrubka: i32| {
        // SAFETY: parametre sú konštanty, meno je reťazec ukončený nulou.
        unsafe { CreateFontW(-skaluj(v, s.dpi), 0, 0, 0, hrubka, 0, 0, 0, 1, 0, 0, 5, 0, meno.as_ptr()) }
    };
    s.fonty = vec![font(15, 400), font(16, 600), font(18, 600), font(13, 400)];
}

fn rozloz(s: &mut Stav) {
    vytvor_fonty(s);
    for &(id, x, y, sirka, vyska) in ROZLOZENIE {
        let h = prvok(s.hwnd, id);
        let font = match id {
            201..=203 => s.fonty[1],
            ID_ZAPIS => s.fonty[2],
            ID_PATA => s.fonty[3],
            _ => s.fonty[0],
        };
        let d = s.dpi;
        // SAFETY: platné okná; písmo žije, kým ho nenahradíme.
        unsafe {
            MoveWindow(h, skaluj(x, d), skaluj(y, d), skaluj(sirka, d), skaluj(vyska, d), 1);
            SendMessageW(h, WM_SETFONT, font as usize, 1);
        }
    }
    let mut r = Rect::default();
    // SAFETY: platné okno a štruktúra.
    unsafe { GetClientRect(s.hwnd, &mut r) };
    nakresli_hlavicku(s, r.r - r.l);
    // SAFETY: platné okno.
    unsafe { InvalidateRect(s.hwnd, std::ptr::null(), 1) };
}

fn nacitaj_disky(s: &mut Stav) {
    let karta = prvok(s.hwnd, ID_KARTA);
    let predtym = vybrany_disk(s).map(|d| d.cislo);
    s.disky = platforma::povolene_disky(s.dovol_virtualny);
    // SAFETY: zoznam je náš prvok; reťazce žijú počas volania.
    unsafe {
        SendMessageW(karta, CB_RESETCONTENT, 0, 0);
        for (i, d) in s.disky.iter().enumerate() {
            let t = w(&popis_disku(d));
            SendMessageW(karta, CB_ADDSTRING, 0, t.as_ptr() as isize);
            SendMessageW(karta, CB_SETITEMDATA, i, d.cislo as isize);
        }
        let vyber = s.disky.iter().position(|d| Some(d.cislo) == predtym).unwrap_or(0);
        if !s.disky.is_empty() {
            SendMessageW(karta, CB_SETCURSEL, vyber, 0);
        }
    }
    if !s.pracuje {
        let stav = prvok(s.hwnd, ID_STAV);
        if s.disky.is_empty() {
            FARBA_STAVU.store(0x0050_6070, Ordering::Relaxed);
            nastav_text(stav, "Vložte SD kartu alebo USB disk – zoznam sa obnoví sám.");
        } else if s.obraz.is_none() {
            FARBA_STAVU.store(0x0050_6070, Ordering::Relaxed);
            nastav_text(stav, "Vyberte obraz SKLENÍK (súbor .img alebo stiahnutý .zip).");
        } else {
            FARBA_STAVU.store(0x0030_3030, Ordering::Relaxed);
            nastav_text(stav, "Pripravené. Skontrolujte kartu a stlačte Zapísať.");
        }
    }
}

fn vybrany_disk(s: &Stav) -> Option<InfoDisku> {
    // SAFETY: zoznam je náš prvok.
    let i = unsafe { SendMessageW(prvok(s.hwnd, ID_KARTA), CB_GETCURSEL, 0, 0) };
    if i < 0 {
        return None;
    }
    s.disky.get(i as usize).cloned()
}

fn nastav_obraz(s: &mut Stav, p: Option<PathBuf>) {
    s.obraz = p;
    let t = s
        .obraz
        .as_ref()
        .map_or("(žiadny obraz – stlačte Vybrať…)".to_string(), |p| p.display().to_string());
    nastav_text(prvok(s.hwnd, ID_OBRAZ), &t);
}

fn vyber_subor(hwnd: Hwnd) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let filter: Vec<u16> =
        "Obraz SKLENÍK (*.img, *.zip)\0*.img;*.zip\0Všetky súbory\0*.*\0\0".encode_utf16().collect();
    let nadpis = w("Vyberte obraz SKLENÍK");
    let mut o = OpenFileNameW {
        velkost: std::mem::size_of::<OpenFileNameW>() as u32,
        vlastnik: hwnd,
        instancia: 0,
        filter: filter.as_ptr(),
        vlastny_filter: std::ptr::null_mut(),
        max_vlastny: 0,
        index_filtra: 1,
        subor: buf.as_mut_ptr(),
        max_subor: buf.len() as u32,
        nazov_suboru: std::ptr::null_mut(),
        max_nazov: 0,
        pociatocny: std::ptr::null(),
        nadpis: nadpis.as_ptr(),
        priznaky: 0x1000 | 0x800 | 0x8_0000 | 0x8 | 0x4,
        posun_suboru: 0,
        posun_pripony: 0,
        pripona: std::ptr::null(),
        data: 0,
        hak: std::ptr::null(),
        sablona: std::ptr::null(),
        rezerva_ptr: std::ptr::null_mut(),
        rezerva: 0,
        priznaky_ex: 0,
    };
    // SAFETY: štruktúra a vyrovnávacie pamäte žijú počas modálneho dialógu.
    if unsafe { GetOpenFileNameW(&mut o) } == 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

fn zamkni(s: &Stav, pracuje: bool) {
    for id in [ID_VYBER, ID_KARTA, ID_OBNOV, ID_MENO, ID_WIFI, ID_HESLO, ID_JAZYK] {
        // SAFETY: naše prvky.
        unsafe { EnableWindow(prvok(s.hwnd, id), (!pracuje) as i32) };
    }
    nastav_text(prvok(s.hwnd, ID_ZAPIS), if pracuje { "Zrušiť" } else { "Zapísať" });
}

/// Údaje na zápis prečítané z okna (alebo chyba pre používateľa).
struct Priprava {
    obraz: String,
    disk: InfoDisku,
    nastavenie: Nastavenie,
    zhrnutie: String,
}

fn priprav(s: &Stav) -> Result<Priprava, String> {
    let obraz = s.obraz.clone().ok_or("Najprv vyberte obraz SKLENÍK (tlačidlo Vybrať…).")?;
    let disk = vybrany_disk(s).ok_or("Vložte SD kartu alebo USB disk a vyberte ho v zozname.")?;
    let meno_text = text(prvok(s.hwnd, ID_MENO));
    let meno = navrhni_meno(&meno_text);
    let wifi = text(prvok(s.hwnd, ID_WIFI)).trim().to_string();
    let heslo = text(prvok(s.hwnd, ID_HESLO));
    // SAFETY: náš prvok.
    let j = unsafe { SendMessageW(prvok(s.hwnd, ID_JAZYK), CB_GETCURSEL, 0, 0) }.clamp(0, 2) as usize;
    let mut arg = vec![format!("jazyk={}", JAZYKY[j].1)];
    if !meno.is_empty() {
        arg.push(format!("meno={meno}"));
    }
    if !wifi.is_empty() {
        arg.push(format!("wifi={wifi}"));
    }
    if !heslo.is_empty() {
        arg.push(format!("wifi_heslo={heslo}"));
    }
    let nastavenie =
        Nastavenie::z_argumentov(arg.iter().map(String::as_str)).map_err(|e| format!("Nastavenie: {e}"))?;
    let mut zhrnutie = format!(
        "Na kartu „{}“ sa zapíše SKLENÍK.\nVšetko, čo je teraz na karte, sa zmaže.\n\nMeno zariadenia: {}",
        popis_disku(&disk),
        if meno.is_empty() { "–" } else { &meno }
    );
    if !meno.is_empty() && meno != meno_text.trim() {
        zhrnutie += &format!("  (upravené z „{}“)", meno_text.trim());
    }
    zhrnutie += &format!(
        "\nWi-Fi: {}\nJazyk: {}\n\nPokračovať?",
        if wifi.is_empty() { "–" } else { &wifi },
        JAZYKY[j].0
    );
    Ok(Priprava { obraz: obraz.display().to_string(), disk, nastavenie, zhrnutie })
}

fn cas(sekundy: f64) -> String {
    let s = sekundy.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn spusti_zapis(hwnd: Hwnd, p: Priprava, dovol_virtualny: bool, zrus: Arc<AtomicBool>) {
    zrus.store(false, Ordering::Relaxed);
    std::thread::spawn(move || {
        let ocakavany = p.disk.clone();
        let mut potvrd = |d: &InfoDisku| d.model == ocakavany.model && d.velkost == ocakavany.velkost;
        let mut posledny = (Faza::Rozbalenie, -1i64, Instant::now());
        let mut start = Instant::now();
        let mut priebeh = |f: Faza, hotovo: u64, celkom: u64| {
            if f != posledny.0 {
                start = Instant::now();
            }
            let podiel = hotovo as f64 / celkom.max(1) as f64;
            let promile = match f {
                Faza::Rozbalenie => 0,
                Faza::Zapis | Faza::Citanie => (podiel * 600.0) as i64,
                Faza::Overenie => 600 + (podiel * 400.0) as i64,
            };
            if f == posledny.0 && promile == posledny.1 && posledny.2.elapsed().as_millis() < 250 {
                return;
            }
            let t = start.elapsed().as_secs_f64().max(0.001);
            let rychlost = hotovo as f64 / 1e6 / t;
            let zostava = if podiel > 0.01 { t * (1.0 - podiel) / podiel } else { 0.0 };
            let text = match f {
                Faza::Rozbalenie => "Pripravujem obraz (rozbaľujem ZIP)…".to_string(),
                _ => format!(
                    "{} {:.0} %  ·  {:.0} MB/s  ·  zostáva {}",
                    if f == Faza::Overenie { "Overujem kartu…" } else { "Zapisujem…" },
                    podiel * 100.0,
                    rychlost,
                    cas(zostava)
                ),
            };
            if let Ok(mut g) = TEXT_PRIEBEHU.lock() {
                *g = text;
            }
            posledny = (f, promile, Instant::now());
            // SAFETY: odoslanie správy oknu z iného vlákna je vo Win32 dovolené.
            unsafe { PostMessageW(hwnd, WM_PRIEBEH, promile as usize, 0) };
        };
        let u = Uloha {
            obraz: &p.obraz,
            disk: p.disk.cislo,
            nastavenie: &p.nastavenie,
            dovol_virtualny,
            zapisovat: true,
        };
        let r = platforma::vykonaj(&u, &mut potvrd, &mut priebeh, &zrus).map_err(|e| e.1);
        if let Ok(mut g) = VYSLEDOK.lock() {
            *g = Some(r);
        }
        // SAFETY: ako vyššie.
        unsafe { PostMessageW(hwnd, WM_HOTOVO, 0, 0) };
    });
}

fn klik_zapis(hwnd: Hwnd) {
    // ak práca beží, tlačidlo zápis ruší
    let pracuje = s_stavom(|s| s.pracuje).unwrap_or(false);
    if pracuje {
        if sprava(
            hwnd,
            "Zrušiť zápis? Karta zostane neúplná – bude ju treba zapísať znova.",
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        ) == 6
        {
            s_stavom(|s| s.zrus.store(true, Ordering::Relaxed));
        }
        return;
    }
    let Some(priprava) = s_stavom(|s| priprav(s)) else { return };
    let skuska = s_stavom(|s| s.skuska.is_some()).unwrap_or(false);
    let p = match priprava {
        Ok(p) => p,
        Err(e) => {
            if skuska {
                println!("okno: {e}");
                // SAFETY: koniec skúšky.
                unsafe { PostQuitMessage(1) };
            } else {
                sprava(hwnd, &e, MB_ICONWARNING);
            }
            return;
        }
    };
    if !skuska && sprava(hwnd, &p.zhrnutie, MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2) != 1 {
        return;
    }
    let Some((dovol, zrus)) = s_stavom(|s| {
        s.pracuje = true;
        zamkni(s, true);
        // SAFETY: náš ukazovateľ priebehu.
        unsafe {
            SendMessageW(prvok(s.hwnd, ID_PRIEBEH), PBM_SETSTATE, 1, 0);
            SendMessageW(prvok(s.hwnd, ID_PRIEBEH), PBM_SETPOS, 0, 0);
        }
        FARBA_STAVU.store(0x0030_3030, Ordering::Relaxed);
        nastav_text(prvok(s.hwnd, ID_STAV), "Začínam…");
        (s.dovol_virtualny, s.zrus.clone())
    }) else {
        return;
    };
    if skuska {
        println!("okno: zapisujem {} na disk {} ({})", p.obraz, p.disk.cislo, popis_disku(&p.disk));
    }
    spusti_zapis(hwnd, p, dovol, zrus);
}

fn hotovo(hwnd: Hwnd) {
    let r = VYSLEDOK.lock().ok().and_then(|mut g| g.take());
    let Some((skuska, zavriet)) = s_stavom(|s| {
        s.pracuje = false;
        zamkni(s, false);
        let ok = matches!(r, Some(Ok(_)));
        // SAFETY: náš ukazovateľ priebehu.
        unsafe {
            SendMessageW(prvok(s.hwnd, ID_PRIEBEH), PBM_SETSTATE, if ok { 1 } else { 2 }, 0);
            if ok {
                SendMessageW(prvok(s.hwnd, ID_PRIEBEH), PBM_SETPOS, 1000, 0);
            }
        }
        let t = match &r {
            Some(Ok(_)) => {
                "✓ Hotovo – karta je zapísaná a overená. Môžete ju vybrať a vložiť do Raspberry Pi."
                    .to_string()
            }
            Some(Err(e)) => format!("✗ {e}"),
            None => "✗ Neznámy výsledok.".into(),
        };
        FARBA_STAVU.store(if ok { 0x0028_8A2E } else { 0x0020_20C8 }, Ordering::Relaxed);
        nastav_text(prvok(s.hwnd, ID_STAV), &t);
        (s.skuska.is_some(), s.zavriet)
    }) else {
        return;
    };
    if skuska {
        match &r {
            Some(Ok(h)) => println!("okno: ✅ zapísané a overené – odtlačok {h}"),
            Some(Err(e)) => println!("okno: ❌ {e}"),
            None => println!("okno: ❌ bez výsledku"),
        }
        // SAFETY: koniec skúšky – návratový kód podľa výsledku.
        unsafe { PostQuitMessage(if matches!(r, Some(Ok(_))) { 0 } else { 1 }) };
        return;
    }
    if zavriet {
        // SAFETY: naše okno.
        unsafe { DestroyWindow(hwnd) };
        return;
    }
    match r {
        Some(Ok(_)) => sprava(
            hwnd,
            &format!(
                "Hotovo – karta je zapísaná a overená.\nMôžete ju vybrať a vložiť do Raspberry Pi.\n\n{}",
                sklenik_karta::RADA_UMIESTNENIE
            ),
            MB_ICONINFORMATION,
        ),
        Some(Err(e)) => sprava(hwnd, &e, MB_ICONERROR),
        None => 0,
    };
}

extern "system" fn okno_proc(hwnd: Hwnd, m: u32, wp: usize, lp: isize) -> isize {
    match m {
        WM_PAINT => {
            let mut ps = PaintStruct {
                hdc: 0,
                zmaz: 0,
                rc: Rect::default(),
                obnov: 0,
                prirastok: 0,
                rezerva: [0; 32],
            };
            // SAFETY: BeginPaint/EndPaint v páre, plátno žije počas kreslenia.
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                s_stavom(|s| {
                    if let Some(p) = &s.hlavicka {
                        let bmi = BitmapInfo {
                            velkost: 40,
                            sirka: p.sirka as i32,
                            vyska: -(p.vyska as i32),
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
                            p.sirka as u32,
                            p.vyska as u32,
                            0,
                            0,
                            0,
                            p.vyska as u32,
                            p.body.as_ptr() as *const c_void,
                            &bmi,
                            0,
                        );
                    }
                });
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_CTLCOLORSTATIC => {
            let hdc = wp as isize;
            let farba = if lp == prvok(hwnd, ID_STAV) {
                Some(FARBA_STAVU.load(Ordering::Relaxed))
            } else if lp == prvok(hwnd, ID_PATA) {
                // SAFETY: systémová farba.
                Some(unsafe { GetSysColor(COLOR_GRAYTEXT) })
            } else {
                None
            };
            // SAFETY: hdc patrí kresleniu prvku; vrátime systémový štetec (netreba ho uvoľniť).
            unsafe {
                if let Some(f) = farba {
                    SetTextColor(hdc, f);
                }
                SetBkColor(hdc, GetSysColor(COLOR_WINDOW));
                GetSysColorBrush(COLOR_WINDOW)
            }
        }
        WM_COMMAND => {
            let id = (wp & 0xFFFF) as i32;
            let kod = (wp >> 16) & 0xFFFF;
            match (id, kod) {
                (ID_VYBER, 0) => {
                    if let Some(p) = vyber_subor(hwnd) {
                        s_stavom(|s| {
                            nastav_obraz(s, Some(p));
                            nacitaj_disky(s);
                        });
                    }
                }
                (ID_OBNOV, 0) => {
                    s_stavom(nacitaj_disky);
                }
                (ID_ZAPIS, 0) => klik_zapis(hwnd),
                _ => {}
            }
            0
        }
        WM_DEVICECHANGE => {
            // karta pripojená alebo odpojená – zoznam sa obnoví o chvíľu (Windows ju ešte pripravuje)
            if wp == 0x8000 || wp == 0x8004 || wp == 0x7 {
                // SAFETY: časovač nášho okna.
                unsafe { SetTimer(hwnd, 1, 1200, std::ptr::null()) };
            }
            1
        }
        WM_TIMER => {
            // SAFETY: časovač nášho okna.
            unsafe { KillTimer(hwnd, wp) };
            s_stavom(|s| {
                if !s.pracuje {
                    nacitaj_disky(s)
                }
            });
            0
        }
        WM_PRIEBEH => {
            let t = TEXT_PRIEBEHU.lock().map(|g| g.clone()).unwrap_or_default();
            s_stavom(|s| {
                // SAFETY: náš ukazovateľ priebehu.
                unsafe { SendMessageW(prvok(s.hwnd, ID_PRIEBEH), PBM_SETPOS, wp, 0) };
                nastav_text(prvok(s.hwnd, ID_STAV), &t);
            });
            0
        }
        WM_HOTOVO => {
            hotovo(hwnd);
            0
        }
        WM_SKUSKA => {
            // skúška v CI: vyplní obraz, kartu a meno a stlačí Zapísať
            let ok = s_stavom(|s| {
                let Some(k) = &s.skuska else { return false };
                let (obraz, disk, meno) = (k.obraz.clone(), k.disk, k.meno.clone());
                nastav_obraz(s, Some(PathBuf::from(obraz)));
                nacitaj_disky(s);
                println!(
                    "okno: v zozname kariet {} položka(y): {:?}",
                    s.disky.len(),
                    s.disky.iter().map(popis_disku).collect::<Vec<_>>()
                );
                let Some(i) = s.disky.iter().position(|d| d.cislo == disk) else { return false };
                // SAFETY: naše prvky.
                unsafe { SendMessageW(prvok(s.hwnd, ID_KARTA), CB_SETCURSEL, i, 0) };
                nastav_text(prvok(s.hwnd, ID_MENO), &meno);
                true
            })
            .unwrap_or(false);
            if ok {
                klik_zapis(hwnd);
            } else {
                println!("okno: ❌ skúšobný disk nie je v zozname");
                // SAFETY: koniec skúšky.
                unsafe { PostQuitMessage(1) };
            }
            0
        }
        WM_DPICHANGED => {
            // SAFETY: lp ukazuje na navrhnutý obdĺžnik okna (dokumentácia WM_DPICHANGED).
            unsafe {
                let r = &*(lp as *const Rect);
                SetWindowPos(hwnd, 0, r.l, r.t, r.r - r.l, r.b - r.t, 0x4 | 0x10);
            }
            s_stavom(|s| {
                s.dpi = (wp & 0xFFFF) as u32;
                rozloz(s);
            });
            0
        }
        WM_CLOSE => {
            let pracuje = s_stavom(|s| s.pracuje).unwrap_or(false);
            if pracuje {
                if sprava(
                    hwnd,
                    "Zápis prebieha. Zavrieť a zápis zrušiť? Karta zostane neúplná.",
                    MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
                ) == 6
                {
                    s_stavom(|s| {
                        s.zavriet = true;
                        s.zrus.store(true, Ordering::Relaxed);
                    });
                }
            } else {
                // SAFETY: naše okno.
                unsafe { DestroyWindow(hwnd) };
            }
            0
        }
        WM_DESTROY => {
            // SAFETY: koniec slučky správ.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: ostatné správy spracuje Windows.
        _ => unsafe { DefWindowProcW(hwnd, m, wp, lp) },
    }
}

fn prvok_novy(rodic: Hwnd, trieda: &str, text: &str, styl: u32, ex: u32, id: i32) -> Hwnd {
    let (t, x) = (w(trieda), w(text));
    // SAFETY: reťazce žijú počas volania; poloha sa nastaví v `rozloz`.
    unsafe {
        CreateWindowExW(
            ex,
            t.as_ptr(),
            x.as_ptr(),
            WS_CHILD | WS_VISIBLE | styl,
            0,
            0,
            10,
            10,
            rodic,
            id as isize,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null_mut(),
        )
    }
}

/// Otvorí okno SADIČA a vráti sa až po jeho zatvorení (návratový kód programu).
pub fn spusti(dovol_virtualny: bool) -> i32 {
    let skuska = std::env::var("SADIC_SKUSKA_OKNA").ok().and_then(|v| {
        let c: Vec<&str> = v.split('|').collect();
        (c.len() == 3).then(|| Skuska {
            obraz: c[0].into(),
            disk: c[1].parse().unwrap_or(u32::MAX),
            meno: c[2].into(),
        })
    });
    // SAFETY: obyčajné volania Win32 pri štarte; štruktúry žijú počas volaní.
    unsafe {
        // spustené dvojklikom: konzolové okno, ktoré Windows otvorilo len pre nás, zatvoríme
        let mut procesy = [0u32; 2];
        if skuska.is_none() && GetConsoleProcessList(procesy.as_mut_ptr(), 2) == 1 {
            FreeConsole();
        }
        SetProcessDpiAwarenessContext(-4);
        InitCommonControlsEx(&InitCommonControls { velkost: 8, triedy: 0x20 | 0x4000 | 0xFF });
        let inst = GetModuleHandleW(std::ptr::null());
        let trieda = w("SadicOkno");
        let wc = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            styl: 0,
            proc_: Some(okno_proc),
            cls_extra: 0,
            wnd_extra: 0,
            instancia: inst,
            ikona: 0,
            kurzor: LoadCursorW(0, 32512usize as *const u16),
            pozadie: (COLOR_WINDOW + 1) as isize,
            menu: std::ptr::null(),
            meno_triedy: trieda.as_ptr(),
            ikona_mala: 0,
        };
        RegisterClassExW(&wc);
        let dpi = GetDpiForSystem().max(96);
        let styl = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let mut r = Rect { l: 0, t: 0, r: skaluj(SIRKA, dpi), b: skaluj(VYSKA, dpi) };
        AdjustWindowRectExForDpi(&mut r, styl, 0, WS_EX_CONTROLPARENT, dpi);
        let nazov = w("SADIČ – karta SKLENÍK pre Raspberry Pi");
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            trieda.as_ptr(),
            nazov.as_ptr(),
            styl,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            r.r - r.l,
            r.b - r.t,
            0,
            0,
            inst,
            std::ptr::null_mut(),
        );
        if hwnd == 0 {
            eprintln!("SADIČ: okno sa nepodarilo vytvoriť");
            return 1;
        }
        let dpi = GetDpiForWindow(hwnd).max(96);
        for (id, t) in [
            (201, "1   Obraz"),
            (202, "2   Karta"),
            (203, "3   Nastavenie zariadenia (nepovinné)"),
            (211, "Meno"),
            (212, "Wi-Fi sieť"),
            (213, "Heslo Wi-Fi"),
            (214, "Jazyk"),
        ] {
            prvok_novy(hwnd, "STATIC", t, SS_NOPREFIX, 0, id);
        }
        prvok_novy(hwnd, "STATIC", "", SS_NOPREFIX | SS_PATHELLIPSIS, 0, ID_OBRAZ);
        prvok_novy(hwnd, "BUTTON", "Vybrať…", WS_TABSTOP, 0, ID_VYBER);
        prvok_novy(hwnd, "COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST, 0, ID_KARTA);
        prvok_novy(hwnd, "BUTTON", "Obnoviť", WS_TABSTOP, 0, ID_OBNOV);
        for (id, napoveda, heslo) in [
            (ID_MENO, "napr. dielna-1 (nepovinné)", false),
            (ID_WIFI, "názov siete (nepovinné)", false),
            (ID_HESLO, "heslo siete – na kartu sa neuloží", true),
        ] {
            let e = prvok_novy(
                hwnd,
                "EDIT",
                "",
                WS_TABSTOP | ES_AUTOHSCROLL | if heslo { ES_PASSWORD } else { 0 },
                WS_EX_CLIENTEDGE,
                id,
            );
            let n = w(napoveda);
            SendMessageW(e, EM_SETCUEBANNER, 0, n.as_ptr() as isize);
            SendMessageW(e, EM_LIMITTEXT, if id == ID_HESLO { 63 } else { 32 }, 0);
        }
        let jazyk = prvok_novy(hwnd, "COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST, 0, ID_JAZYK);
        for (nazov, _) in JAZYKY {
            let t = w(nazov);
            SendMessageW(jazyk, CB_ADDSTRING, 0, t.as_ptr() as isize);
        }
        let predvoleny = match GetUserDefaultUILanguage() & 0x3FF {
            0x05 => 1,
            0x1B => 0,
            0x09 => 2,
            _ => 0,
        };
        SendMessageW(jazyk, CB_SETCURSEL, predvoleny, 0);
        prvok_novy(hwnd, "BUTTON", "Zapísať", WS_TABSTOP | BS_DEFPUSHBUTTON, 0, ID_ZAPIS);
        let pb = prvok_novy(hwnd, "msctls_progress32", "", 0, 0, ID_PRIEBEH);
        SendMessageW(pb, PBM_SETRANGE32, 0, 1000);
        prvok_novy(hwnd, "STATIC", "", SS_NOPREFIX, 0, ID_STAV);
        prvok_novy(
            hwnd,
            "STATIC",
            "Ponúkajú sa len SD karty a USB disky – disk so systémom počítača nikdy.",
            SS_NOPREFIX,
            0,
            ID_PATA,
        );

        STAV.with(|s| {
            *s.borrow_mut() = Some(Stav {
                hwnd,
                dpi,
                pismo: Pismo::stonka(),
                hlavicka: None,
                fonty: Vec::new(),
                obraz: None,
                disky: Vec::new(),
                pracuje: false,
                zrus: Arc::new(AtomicBool::new(false)),
                zavriet: false,
                dovol_virtualny,
                skuska,
            })
        });
        s_stavom(|s| {
            rozloz(s);
            nastav_obraz(s, najdi_obraz());
            nacitaj_disky(s);
        });
        ShowWindow(hwnd, 5);
        UpdateWindow(hwnd);
        if s_stavom(|s| s.skuska.is_some()).unwrap_or(false) {
            PostMessageW(hwnd, WM_SKUSKA, 0, 0);
        }
        let mut msg = Msg::default();
        let mut kod = 0;
        loop {
            let r = GetMessageW(&mut msg, 0, 0, 0);
            if r <= 0 {
                if r == 0 {
                    kod = msg.wparam as i32;
                }
                break;
            }
            if IsDialogMessageW(hwnd, &mut msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        STAV.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut()
                && let Some(s) = s.take()
            {
                for f in s.fonty {
                    DeleteObject(f);
                }
            }
        });
        kod
    }
}
