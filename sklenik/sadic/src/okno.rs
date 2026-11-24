//! Okno SADIČA pre Windows 11 – jednoduché ako Raspberry Pi Imager: 1. obraz, 2. karta, 3. nastavenie,
//! tlačidlo **Zapísať** a ukazovateľ priebehu. Tlačidlo **Rozšírené…** prepne okno do pokročilého
//! režimu: napálenie ISO (Linux, Windows …) s rozborom a kontrolným súčtom, čisté formátovanie
//! (FAT32/exFAT, MBR/GPT) a kontrola falošnej kapacity. Súbor sa dá do okna aj pretiahnuť myšou.
//!
//! Vlastné väzby na user32, gdi32, comctl32 a comdlg32 (prvky okna Windows – platforma); hlavička je
//! nakreslená vlastným písmom STONKA. Zápis beží vo vlastnom vlákne a do okna posiela priebeh.

#![allow(unsafe_code)]

use crate::disk::{self, InfoDisku};
use crate::formatuj::{Formatovanie, System};
use crate::oddiely::Schema;
use crate::platforma;
use crate::{Faza, Uloha, iso, sha256};
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

#[link(name = "shell32")]
unsafe extern "system" {
    fn DragAcceptFiles(h: Hwnd, prijmi: i32);
    fn DragQueryFileW(hdrop: isize, i: u32, buf: *mut u16, n: u32) -> u32;
    fn DragFinish(hdrop: isize);
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
const WM_DROPFILES: u32 = 0x233;
const BM_GETCHECK: u32 = 0xF0;
const BM_SETCHECK: u32 = 0xF1;
const BS_AUTOCHECKBOX: u32 = 0x3;
const CBN_SELCHANGE: usize = 1;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
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
const ID_ROZSIRENE: i32 = 116;
// rozšírený režim
const ID_ULOHA: i32 = 120;
const ID_ISO: i32 = 121;
const ID_ISO_VYBER: i32 = 122;
const ID_ROZBOR: i32 = 123;
const ID_SYSTEM: i32 = 124;
const ID_SCHEMA: i32 = 125;
const ID_MENOVKA: i32 = 126;
const ID_UPLNE: i32 = 127;
const ID_SUCET: i32 = 128;

const SIRKA: i32 = 600;
const VYSKA: i32 = 600;
/// Rozšírený režim je o kúsok vyšší (sekcia Možnosti).
const VYSKA_ROZSIRENA: i32 = 650;
const HLAVICKA: i32 = 92;

/// Úlohy rozšíreného režimu.
const ULOHY: [&str; 3] = [
    "Napáliť ISO alebo obraz disku (Linux, Windows, iné systémy)",
    "Formátovať kartu alebo USB disk (čisté formátovanie)",
    "Skontrolovať kartu (falošná kapacita, chyby, rýchlosť)",
];
const U_NAPAL: usize = 0;
const U_FORMAT: usize = 1;
const U_KONTROLA: usize = 2;

/// Všetky prvky okna (na skrývanie prvkov, ktoré v režime nie sú).
const VSETKY: &[i32] = &[
    201,
    202,
    203,
    211,
    212,
    213,
    214,
    ID_OBRAZ,
    ID_VYBER,
    ID_KARTA,
    ID_OBNOV,
    ID_MENO,
    ID_WIFI,
    ID_HESLO,
    ID_JAZYK,
    ID_ZAPIS,
    ID_PRIEBEH,
    ID_STAV,
    ID_PATA,
    ID_ROZSIRENE,
    301,
    302,
    303,
    304,
    311,
    312,
    313,
    ID_ULOHA,
    ID_ISO,
    ID_ISO_VYBER,
    ID_ROZBOR,
    ID_SYSTEM,
    ID_SCHEMA,
    ID_MENOVKA,
    ID_UPLNE,
    ID_SUCET,
];

/// Rozloženie jednoduchého režimu: (id, x, y, šírka, výška) v logických bodoch (pri 96 DPI).
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
    (ID_PATA, 24, 568, 430, 22),
    (ID_ROZSIRENE, 464, 564, 112, 28),
];

/// Rozloženie podľa režimu a úlohy.
fn rozlozenie(rozsirene: bool, uloha: usize) -> Vec<(i32, i32, i32, i32, i32)> {
    if !rozsirene {
        return ROZLOZENIE.to_vec();
    }
    let mut r = vec![(301, 24, 104, 552, 24), (ID_ULOHA, 24, 132, 552, 200), (302, 24, 176, 552, 24)];
    if uloha == U_NAPAL {
        r.extend([
            (ID_ISO, 24, 206, 430, 24),
            (ID_ISO_VYBER, 464, 202, 112, 30),
            (ID_ROZBOR, 24, 238, 552, 50),
        ]);
    } else {
        r.push((ID_ROZBOR, 24, 206, 552, 80));
    }
    r.extend([(303, 24, 294, 552, 24), (ID_KARTA, 24, 322, 430, 240), (ID_OBNOV, 464, 320, 112, 30)]);
    r.push((304, 24, 366, 552, 24));
    match uloha {
        U_NAPAL => r.extend([
            (311, 24, 400, 120, 24),
            (ID_SCHEMA, 150, 396, 150, 200),
            (313, 24, 434, 120, 24),
            (ID_SUCET, 150, 430, 426, 28),
        ]),
        U_FORMAT => r.extend([
            (311, 24, 400, 120, 24),
            (ID_SYSTEM, 150, 396, 150, 200),
            (312, 320, 400, 80, 24),
            (ID_SCHEMA, 400, 396, 176, 200),
            (313, 24, 434, 120, 24),
            (ID_MENOVKA, 150, 430, 150, 28),
            (ID_UPLNE, 320, 432, 256, 24),
        ]),
        _ => r.push((ID_UPLNE, 24, 400, 552, 24)),
    }
    r.extend([
        (ID_ZAPIS, 24, 488, 552, 46),
        (ID_PRIEBEH, 24, 546, 552, 12),
        (ID_STAV, 24, 566, 552, 44),
        (ID_PATA, 24, 618, 430, 22),
        (ID_ROZSIRENE, 464, 614, 112, 28),
    ]);
    r
}

const JAZYKY: [(&str, &str); 3] = [("Slovenčina", "sk"), ("Čeština", "cs"), ("English", "en")];

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Skúška v CI: obraz, číslo disku a meno sa vyplnia samy a zápis sa spustí bez otázok.
/// Rozšírená skúška: (úloha, parameter) – napr. (formátovanie, „exfat:SKUSKA“) alebo (napálenie, ISO).
struct Skuska {
    obraz: String,
    disk: u32,
    meno: String,
    rozsirena: Option<(usize, String)>,
}

/// Práca, ktorá beží vo vlákne.
enum Praca {
    Zapis(Priprava),
    Napal { iso: String, disk: InfoDisku, schema: Schema, sucet: Option<String> },
    Formatuj { disk: InfoDisku, f: Formatovanie },
    Kontrola { disk: InfoDisku, uplne: bool },
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
    rozsirene: bool,
    uloha: usize,
    iso: Option<PathBuf>,
    rozbor: Option<iso::Rozbor>,
    /// Druh poslednej spustenej práce (na správu po dokončení): 0 zápis, 1 napálenie, 2 formát, 3 kontrola.
    druh_prace: u8,
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
    let (t, n) = (w(text), w("ORCHIDEA Zapisovač"));
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
    p.text(&s.pismo, "ORCHIDEA Zapisovač", 24.0 * k, 52.0 * k, 30.0 * k, 3.8 * k, 0x6FD08C);
    let podtitul = if s.rozsirene {
        "Rozšírené: ISO, formátovanie a kontrola kariet"
    } else {
        "Karta ORCHIDEA OS – Raspberry Pi, Odroid, TV krabičky"
    };
    p.text(&s.pismo, podtitul, 24.0 * k, 79.0 * k, 12.0 * k, 1.5 * k, 0xE8F0EA);
    let heslo = "Chladný systém";
    let hw = s.pismo.sirka(heslo, 11.0 * k, 1.3 * k);
    p.text(&s.pismo, heslo, sirka as f32 - 24.0 * k - hw, 52.0 * k, 11.0 * k, 1.3 * k, 0x9DB3A5);
    s.hlavicka = Some(p);
}

/// Nakreslí hlavičku znova (po prepnutí režimu sa mení podtitul).
fn nakresli_hlavicku_znova(s: &mut Stav) {
    let mut r = Rect::default();
    // SAFETY: platné okno a štruktúra.
    unsafe { GetClientRect(s.hwnd, &mut r) };
    nakresli_hlavicku(s, r.r - r.l);
    // SAFETY: platné okno.
    unsafe { InvalidateRect(s.hwnd, std::ptr::null(), 1) };
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
    let r = rozlozenie(s.rozsirene, s.uloha);
    for &id in VSETKY {
        if !r.iter().any(|z| z.0 == id) {
            // SAFETY: náš prvok.
            unsafe { ShowWindow(prvok(s.hwnd, id), SW_HIDE) };
        }
    }
    for &(id, x, y, sirka, vyska) in &r {
        let h = prvok(s.hwnd, id);
        let font = match id {
            201..=203 | 301..=304 => s.fonty[1],
            ID_ZAPIS => s.fonty[2],
            ID_PATA | ID_ROZBOR => s.fonty[3],
            _ => s.fonty[0],
        };
        let d = s.dpi;
        // SAFETY: platné okná; písmo žije, kým ho nenahradíme.
        unsafe {
            MoveWindow(h, skaluj(x, d), skaluj(y, d), skaluj(sirka, d), skaluj(vyska, d), 1);
            SendMessageW(h, WM_SETFONT, font as usize, 1);
            ShowWindow(h, SW_SHOW);
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
        } else if s.rozsirene && s.uloha == U_NAPAL && s.iso.is_none() {
            FARBA_STAVU.store(0x0050_6070, Ordering::Relaxed);
            nastav_text(stav, "Vyberte ISO alebo obraz disku (alebo ho sem pretiahnite myšou).");
        } else if s.rozsirene {
            FARBA_STAVU.store(0x0030_3030, Ordering::Relaxed);
            nastav_text(stav, "Pripravené. Skontrolujte disk a možnosti.");
        } else if s.obraz.is_none() {
            FARBA_STAVU.store(0x0050_6070, Ordering::Relaxed);
            nastav_text(stav, "Vyberte obraz ORCHIDEA OS (súbor .img alebo stiahnutý .zip).");
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

fn vyber_subor(hwnd: Hwnd, filter: &str, nadpis: &str) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let filter: Vec<u16> = filter.encode_utf16().collect();
    let nadpis = w(nadpis);
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
    for id in [
        ID_VYBER,
        ID_KARTA,
        ID_OBNOV,
        ID_MENO,
        ID_WIFI,
        ID_HESLO,
        ID_JAZYK,
        ID_ROZSIRENE,
        ID_ULOHA,
        ID_ISO_VYBER,
        ID_SYSTEM,
        ID_SCHEMA,
        ID_MENOVKA,
        ID_UPLNE,
        ID_SUCET,
    ] {
        // SAFETY: naše prvky.
        unsafe { EnableWindow(prvok(s.hwnd, id), (!pracuje) as i32) };
    }
    nastav_text(prvok(s.hwnd, ID_ZAPIS), if pracuje { "Zrušiť" } else { text_tlacidla(s) });
}

fn text_tlacidla(s: &Stav) -> &'static str {
    match (s.rozsirene, s.uloha) {
        (false, _) => "Zapísať",
        (true, U_NAPAL) => "Napáliť",
        (true, U_FORMAT) => "Formátovať",
        _ => "Skontrolovať",
    }
}

fn vyber_combo(s: &Stav, id: i32) -> usize {
    // SAFETY: náš prvok.
    unsafe { SendMessageW(prvok(s.hwnd, id), CB_GETCURSEL, 0, 0) }.max(0) as usize
}

fn zaskrtnute(s: &Stav, id: i32) -> bool {
    // SAFETY: náš prvok.
    unsafe { SendMessageW(prvok(s.hwnd, id), BM_GETCHECK, 0, 0) == 1 }
}

/// Texty prvkov podľa úlohy rozšíreného režimu.
fn obnov_texty(s: &mut Stav) {
    let h = s.hwnd;
    let (nadpis2, rozbor) = match s.uloha {
        U_NAPAL => ("2   Obraz (ISO, .img, .zip)", popis_rozboru(s)),
        U_FORMAT => (
            "2   Čo sa stane",
            "Vymaže všetko vrátane starých tabuliek a stôp po ISO a vytvorí jeden oddiel zarovnaný podľa \
             pamäte flash (ako SD Formatter). Automaticky: do 32 GB FAT32, väčšie exFAT."
                .to_string(),
        ),
        _ => (
            "2   Čo sa stane",
            "Zapíše jedinečné bloky po celej kapacite a prečíta ich späť – odhalí falošnú kapacitu a chybné \
             miesta a zmeria zápis a čítanie. Potom disk naformátuje (falošný len na skutočnú kapacitu)."
                .to_string(),
        ),
    };
    nastav_text(prvok(h, 302), nadpis2);
    nastav_text(prvok(h, ID_ROZBOR), &rozbor);
    let (l1, l2, l3) = match s.uloha {
        U_NAPAL => ("Tabuľka", "", "SHA-256"),
        U_FORMAT => ("Systém", "Tabuľka", "Menovka"),
        _ => ("", "", ""),
    };
    nastav_text(prvok(h, 311), l1);
    nastav_text(prvok(h, 312), l2);
    nastav_text(prvok(h, 313), l3);
    nastav_text(
        prvok(h, ID_UPLNE),
        if s.uloha == U_KONTROLA {
            "Úplná kontrola celého povrchu (dlho – celý disk sa zapíše a prečíta)"
        } else {
            "Úplné vymazanie (pomalé)"
        },
    );
    nastav_text(prvok(h, ID_ZAPIS), text_tlacidla(s));
    let nazov = if s.rozsirene {
        "ORCHIDEA Zapisovač – rozšírené: ISO, formátovanie, kontrola"
    } else {
        "ORCHIDEA Zapisovač – karta ORCHIDEA OS"
    };
    nastav_text(h, nazov);
}

fn popis_rozboru(s: &Stav) -> String {
    let Some(r) = &s.rozbor else {
        if s.iso.is_some() {
            return "Obraz v súbore ZIP – pred zápisom sa rozbalí a rozoberie.".into();
        }
        return "Linux (Ubuntu, Debian, Fedora …) aj Windows. Hybridné ISO sa zapíše bit po bite, ostatné sa \
                rozbalia na FAT32 a štartujú cez UEFI. Súbor môžete pretiahnuť myšou."
            .into();
    };
    let mut st = Vec::new();
    if r.bios() || r.mbr_kod && r.hybridne() {
        st.push("BIOS");
    }
    if r.efi() {
        st.push("UEFI");
    }
    let mut t = format!(
        "{} · {} · {}",
        r.druh().popis(),
        r.system.clone().unwrap_or_else(|| if r.menovka.is_empty() {
            "–".into()
        } else {
            r.menovka.clone()
        }),
        disk::velkost(r.velkost)
    );
    if !st.is_empty() {
        t += &format!(" · štart {}", st.join(" + "));
    }
    t += &format!("\n→ {}", r.odporucanie());
    if !r.nad_4gb().is_empty() {
        t += " – súbory nad 4 GB pôjdu na druhý oddiel exFAT";
    }
    if matches!(r.druh(), iso::Druh::WindowsIso | iso::Druh::IsoNaRozbalenie) && !r.efi() {
        t += "\n⚠ ISO nemá štart cez UEFI – z USB sa nespustí.";
    }
    t
}

/// Nastaví ISO (alebo obraz) rozšíreného režimu a hneď ho rozoberie.
fn nastav_iso(s: &mut Stav, p: Option<PathBuf>) {
    let zip = p.as_ref().is_some_and(|c| c.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")));
    s.rozbor = p.as_ref().filter(|_| !zip).and_then(|c| {
        let (cesta, docasny) = crate::zip::priprav_obraz(c).ok()?;
        let r = std::fs::File::open(&cesta).ok().and_then(|mut f| iso::rozober(&mut f).ok());
        if docasny {
            let _ = std::fs::remove_file(&cesta);
        }
        r
    });
    s.iso = p;
    let t = s
        .iso
        .as_ref()
        .map_or("(žiadny obraz – stlačte Vybrať…)".to_string(), |p| p.display().to_string());
    nastav_text(prvok(s.hwnd, ID_ISO), &t);
    obnov_texty(s);
}

/// Prepne jednoduchý ↔ rozšírený režim (zmení aj výšku okna).
fn prepni_rezim(s: &mut Stav, rozsirene: bool) {
    s.rozsirene = rozsirene;
    nastav_text(prvok(s.hwnd, ID_ROZSIRENE), if rozsirene { "◂ Jednoduché" } else { "Rozšírené…" });
    let styl = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let v = if rozsirene { VYSKA_ROZSIRENA } else { VYSKA };
    let mut r = Rect { l: 0, t: 0, r: skaluj(SIRKA, s.dpi), b: skaluj(v, s.dpi) };
    // SAFETY: platné okno a štruktúra.
    unsafe {
        AdjustWindowRectExForDpi(&mut r, styl, 0, WS_EX_CONTROLPARENT, s.dpi);
        SetWindowPos(s.hwnd, 0, 0, 0, r.r - r.l, r.b - r.t, 0x2 | 0x4);
    }
    obnov_texty(s);
    rozloz(s);
    nacitaj_disky(s);
}

/// Údaje na zápis prečítané z okna (alebo chyba pre používateľa).
struct Priprava {
    obraz: String,
    disk: InfoDisku,
    nastavenie: Nastavenie,
    zhrnutie: String,
}

fn priprav(s: &Stav) -> Result<Priprava, String> {
    let obraz = s.obraz.clone().ok_or("Najprv vyberte obraz ORCHIDEA OS (tlačidlo Vybrať…).")?;
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
        "Na kartu „{}“ sa zapíše ORCHIDEA OS.\nVšetko, čo je teraz na karte, sa zmaže.\n\nMeno zariadenia: {}",
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

fn text_fazy(f: Faza) -> &'static str {
    match f {
        Faza::Rozbalenie => "Pripravujem obraz…",
        Faza::Zapis | Faza::Citanie => "Zapisujem…",
        Faza::Overenie => "Overujem…",
        Faza::Sucet => "Kontrolný súčet SHA-256…",
        Faza::Mazanie => "Mažem disk…",
        Faza::Formatovanie => "Formátujem…",
        Faza::SkuskaZapis => "Skúšobný zápis…",
        Faza::SkuskaCitanie => "Kontrola čítaním…",
    }
}

fn spusti_pracu(hwnd: Hwnd, praca: Praca, dovol_virtualny: bool, zrus: Arc<AtomicBool>) {
    zrus.store(false, Ordering::Relaxed);
    std::thread::spawn(move || {
        let ocakavany = match &praca {
            Praca::Zapis(p) => p.disk.clone(),
            Praca::Napal { disk, .. } | Praca::Formatuj { disk, .. } | Praca::Kontrola { disk, .. } => {
                disk.clone()
            }
        };
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
                _ => (podiel * 1000.0) as i64,
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
                    text_fazy(f),
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
            unsafe { PostMessageW(hwnd, WM_PRIEBEH, promile.clamp(0, 1000) as usize, 0) };
        };
        let r = match praca {
            Praca::Zapis(p) => {
                let u = Uloha {
                    obraz: &p.obraz,
                    disk: p.disk.cislo,
                    nastavenie: &p.nastavenie,
                    dovol_virtualny,
                    zapisovat: true,
                };
                platforma::vykonaj(&u, &mut potvrd, &mut priebeh, &zrus)
            }
            Praca::Napal { iso, disk, schema, sucet } => platforma::napal_jadro(
                &iso,
                disk.cislo,
                schema,
                sucet.as_deref(),
                dovol_virtualny,
                &mut potvrd,
                &mut priebeh,
                &zrus,
            ),
            Praca::Formatuj { disk, f } => {
                platforma::formatuj_jadro(disk.cislo, &f, dovol_virtualny, &mut potvrd, &mut priebeh, &zrus)
            }
            Praca::Kontrola { disk, uplne } => platforma::skontroluj_jadro(
                disk.cislo,
                uplne,
                dovol_virtualny,
                &mut potvrd,
                &mut priebeh,
                &zrus,
            ),
        }
        .map_err(|e| e.1);
        if let Ok(mut g) = VYSLEDOK.lock() {
            *g = Some(r);
        }
        // SAFETY: ako vyššie.
        unsafe { PostMessageW(hwnd, WM_HOTOVO, 0, 0) };
    });
}

/// Pripraví prácu rozšíreného režimu z okna (alebo chybu pre používateľa) a text potvrdenia.
fn priprav_rozsirenu(s: &Stav) -> Result<(Praca, String), String> {
    let disk = vybrany_disk(s).ok_or("Vložte SD kartu alebo USB disk a vyberte ho v zozname.")?;
    let schema = if vyber_combo(s, ID_SCHEMA) == 1 { Schema::Gpt } else { Schema::Mbr };
    let nazov_disku = popis_disku(&disk);
    match s.uloha {
        U_NAPAL => {
            let iso = s.iso.clone().ok_or("Najprv vyberte ISO alebo obraz disku (tlačidlo Vybrať…).")?;
            let text_suctu = text(prvok(s.hwnd, ID_SUCET));
            let sucet = if text_suctu.trim().is_empty() {
                None
            } else {
                Some(sha256::najdi_sucet(&text_suctu).ok_or(
                    "Kontrolný súčet SHA-256 má 64 znakov 0–9 a a–f (skopírujte ho zo stránky distribúcie).",
                )?)
            };
            let co = s.rozbor.as_ref().map_or("obraz v ZIP".to_string(), |r| {
                format!("{} – {}", r.druh().popis(), r.system.clone().unwrap_or_else(|| r.menovka.clone()))
            });
            let ako = s.rozbor.as_ref().map_or("rozbalí sa a zapíše", |r| r.odporucanie());
            let meno = iso.file_name().map_or(String::new(), |m| m.to_string_lossy().into_owned());
            let zhrnutie = format!(
                "Na disk „{nazov_disku}“ sa napáli:\n{meno}\n{co}\n→ {ako}{}\n\nVšetko, čo je teraz na disku, sa zmaže.\n\nPokračovať?",
                if sucet.is_some() { "\n(najprv sa overí kontrolný súčet SHA-256)" } else { "" }
            );
            Ok((Praca::Napal { iso: iso.display().to_string(), disk, schema, sucet }, zhrnutie))
        }
        U_FORMAT => {
            let system = crate::formatuj::PONUKA
                .get(vyber_combo(s, ID_SYSTEM))
                .copied()
                .unwrap_or(System::Automaticky);
            let f = Formatovanie {
                system,
                schema,
                menovka: text(prvok(s.hwnd, ID_MENOVKA)).trim().to_string(),
                klaster: None,
                uplne: zaskrtnute(s, ID_UPLNE),
            };
            let zhrnutie = format!(
                "Disk „{nazov_disku}“ sa naformátuje:\n{}, tabuľka {}{}{}\n\nVšetko, čo je teraz na disku, sa zmaže.\n\nPokračovať?",
                system.pre(disk.velkost).nazov(),
                schema.nazov(),
                if f.menovka.is_empty() { String::new() } else { format!(", menovka „{}“", f.menovka) },
                if f.uplne {
                    "\nÚplné vymazanie: celý disk sa prepíše nulami (potrvá dlhšie)."
                } else {
                    ""
                }
            );
            Ok((Praca::Formatuj { disk, f }, zhrnutie))
        }
        _ => {
            let uplne = zaskrtnute(s, ID_UPLNE);
            let zhrnutie = format!(
                "Kontrola zapíše skúšobné údaje na disk „{nazov_disku}“ – všetko, čo je na ňom, sa zmaže.\n\
                 Potom sa disk naformátuje (falošný len na skutočnú kapacitu).{}\n\nPokračovať?",
                if uplne {
                    "\n\nÚplná kontrola zapíše a prečíta celý povrch – pri veľkých kartách aj hodinu."
                } else {
                    ""
                }
            );
            Ok((Praca::Kontrola { disk, uplne }, zhrnutie))
        }
    }
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
    let Some(priprava) = s_stavom(|s| {
        if s.rozsirene {
            priprav_rozsirenu(s).map(|(p, z)| {
                let druh = match p {
                    Praca::Napal { .. } => 1,
                    Praca::Formatuj { .. } => 2,
                    _ => 3,
                };
                (p, z, druh)
            })
        } else {
            priprav(s).map(|p| {
                let z = p.zhrnutie.clone();
                (Praca::Zapis(p), z, 0)
            })
        }
    }) else {
        return;
    };
    let skuska = s_stavom(|s| s.skuska.is_some()).unwrap_or(false);
    let (p, zhrnutie, druh) = match priprava {
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
    if !skuska && sprava(hwnd, &zhrnutie, MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2) != 1 {
        return;
    }
    let Some((dovol, zrus)) = s_stavom(|s| {
        s.pracuje = true;
        s.druh_prace = druh;
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
        match &p {
            Praca::Zapis(p) => {
                println!("okno: zapisujem {} na disk {} ({})", p.obraz, p.disk.cislo, popis_disku(&p.disk))
            }
            _ => println!("okno: rozšírené – {}", zhrnutie.replace('\n', " ")),
        }
    }
    spusti_pracu(hwnd, p, dovol, zrus);
}

fn hotovo(hwnd: Hwnd) {
    let r = VYSLEDOK.lock().ok().and_then(|mut g| g.take());
    let Some((skuska, zavriet, druh)) = s_stavom(|s| {
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
        let t = match (&r, s.druh_prace) {
            (Some(Ok(_)), 0) => {
                "✓ Hotovo – karta je zapísaná a overená. Môžete ju vybrať a vložiť do Raspberry Pi."
                    .to_string()
            }
            (Some(Ok(t)), _) => format!("✓ {}", t.lines().next().unwrap_or("Hotovo.")),
            (Some(Err(e)), _) => format!("✗ {}", e.lines().next().unwrap_or("chyba")),
            (None, _) => "✗ Neznámy výsledok.".into(),
        };
        FARBA_STAVU.store(if ok { 0x0028_8A2E } else { 0x0020_20C8 }, Ordering::Relaxed);
        nastav_text(prvok(s.hwnd, ID_STAV), &t);
        (s.skuska.is_some(), s.zavriet, s.druh_prace)
    }) else {
        return;
    };
    if skuska {
        match &r {
            Some(Ok(h)) if druh == 0 => println!("okno: ✅ zapísané a overené – odtlačok {h}"),
            Some(Ok(t)) => println!("okno: ✅ {}", t.replace('\n', " | ")),
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
        Some(Ok(t)) if druh != 0 => sprava(hwnd, &format!("Hotovo.\n\n{t}"), MB_ICONINFORMATION),
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
                    if let Some(p) = vyber_subor(
                        hwnd,
                        "Obraz ORCHIDEA OS (*.img, *.zip)\0*.img;*.zip\0Všetky súbory\0*.*\0\0",
                        "Vyberte obraz ORCHIDEA OS",
                    ) {
                        s_stavom(|s| {
                            nastav_obraz(s, Some(p));
                            nacitaj_disky(s);
                        });
                    }
                }
                (ID_ISO_VYBER, 0) => {
                    if let Some(p) = vyber_subor(
                        hwnd,
                        "ISO a obrazy diskov (*.iso, *.img, *.zip)\0*.iso;*.img;*.zip;*.raw;*.bin\0Všetky súbory\0*.*\0\0",
                        "Vyberte ISO alebo obraz disku",
                    ) {
                        s_stavom(|s| {
                            nastav_iso(s, Some(p));
                            nacitaj_disky(s);
                        });
                    }
                }
                (ID_ROZSIRENE, 0) => {
                    s_stavom(|s| {
                        let r = !s.rozsirene;
                        prepni_rezim(s, r);
                        nakresli_hlavicku_znova(s);
                    });
                }
                (ID_ULOHA, CBN_SELCHANGE) => {
                    s_stavom(|s| {
                        s.uloha = vyber_combo(s, ID_ULOHA).min(2);
                        obnov_texty(s);
                        rozloz(s);
                        nacitaj_disky(s);
                    });
                }
                (ID_OBNOV, 0) => {
                    s_stavom(nacitaj_disky);
                }
                (ID_ZAPIS, 0) => klik_zapis(hwnd),
                _ => {}
            }
            0
        }
        WM_DROPFILES => {
            let hdrop = wp as isize;
            let mut buf = vec![0u16; 1024];
            // SAFETY: hdrop pochádza zo správy WM_DROPFILES; buffer má udanú dĺžku; DragFinish ho uvoľní.
            let n = unsafe {
                let n = DragQueryFileW(hdrop, 0, buf.as_mut_ptr(), buf.len() as u32) as usize;
                DragFinish(hdrop);
                n
            };
            if n > 0 {
                let p = PathBuf::from(String::from_utf16_lossy(&buf[..n.min(buf.len())]));
                let iso = p
                    .extension()
                    .is_some_and(|e| !e.eq_ignore_ascii_case("img") && !e.eq_ignore_ascii_case("zip"));
                let meno = p.file_name().map_or(String::new(), |m| m.to_string_lossy().to_lowercase());
                s_stavom(|s| {
                    if s.pracuje {
                        return;
                    }
                    // obraz SKLENÍK do jednoduchého režimu, všetko ostatné (ISO …) do napálenia
                    if !iso && meno.starts_with("sklenik") && !s.rozsirene {
                        nastav_obraz(s, Some(p));
                    } else {
                        if !s.rozsirene || s.uloha != U_NAPAL {
                            s.uloha = U_NAPAL;
                            // SAFETY: náš prvok.
                            unsafe { SendMessageW(prvok(s.hwnd, ID_ULOHA), CB_SETCURSEL, U_NAPAL, 0) };
                            if !s.rozsirene {
                                prepni_rezim(s, true);
                                nakresli_hlavicku_znova(s);
                            } else {
                                rozloz(s);
                            }
                        }
                        nastav_iso(s, Some(p));
                    }
                    nacitaj_disky(s);
                });
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
                let (obraz, disk, meno, rozsirena) =
                    (k.obraz.clone(), k.disk, k.meno.clone(), k.rozsirena.clone());
                if let Some((uloha, param)) = rozsirena {
                    prepni_rezim(s, true);
                    s.uloha = uloha;
                    // SAFETY: naše prvky.
                    unsafe { SendMessageW(prvok(s.hwnd, ID_ULOHA), CB_SETCURSEL, uloha, 0) };
                    obnov_texty(s);
                    rozloz(s);
                    match uloha {
                        U_NAPAL => nastav_iso(s, Some(PathBuf::from(&param))),
                        U_FORMAT => {
                            let (sys, menovka) = param.split_once(':').unwrap_or((&param, ""));
                            let i = System::z_textu(sys)
                                .and_then(|x| crate::formatuj::PONUKA.iter().position(|&y| y == x))
                                .unwrap_or(0);
                            // SAFETY: naše prvky.
                            unsafe { SendMessageW(prvok(s.hwnd, ID_SYSTEM), CB_SETCURSEL, i, 0) };
                            nastav_text(prvok(s.hwnd, ID_MENOVKA), menovka);
                        }
                        _ => {}
                    }
                    if let Some(r) = &s.rozbor {
                        println!("okno: rozbor – {}", r.suhrn().replace('\n', " | "));
                    }
                } else {
                    nastav_obraz(s, Some(PathBuf::from(obraz)));
                }
                nacitaj_disky(s);
                println!(
                    "okno: v zozname kariet {} položka(y): {:?}",
                    s.disky.len(),
                    s.disky.iter().map(popis_disku).collect::<Vec<_>>()
                );
                let Some(i) = s.disky.iter().position(|d| d.cislo == disk) else { return false };
                // SAFETY: naše prvky.
                unsafe { SendMessageW(prvok(s.hwnd, ID_KARTA), CB_SETCURSEL, i, 0) };
                if !s.rozsirene {
                    nastav_text(prvok(s.hwnd, ID_MENO), &meno);
                }
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
    let skuska = std::env::var("SADIC_SKUSKA_OKNA")
        .ok()
        .and_then(|v| {
            let c: Vec<&str> = v.split('|').collect();
            (c.len() == 3).then(|| Skuska {
                obraz: c[0].into(),
                disk: c[1].parse().unwrap_or(u32::MAX),
                meno: c[2].into(),
                rozsirena: None,
            })
        })
        .or_else(|| {
            // rozšírená skúška: „napal|disk|cesta.iso“, „formatuj|disk|exfat:MENOVKA“, „kontrola|disk|“
            let v = std::env::var("SADIC_SKUSKA_ROZSIRENE").ok()?;
            let c: Vec<&str> = v.splitn(3, '|').collect();
            let uloha = match *c.first()? {
                "napal" => U_NAPAL,
                "formatuj" => U_FORMAT,
                _ => U_KONTROLA,
            };
            Some(Skuska {
                obraz: String::new(),
                disk: c.get(1)?.parse().unwrap_or(u32::MAX),
                meno: String::new(),
                rozsirena: Some((uloha, c.get(2).unwrap_or(&"").to_string())),
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
        let nazov = w("ORCHIDEA Zapisovač – karta ORCHIDEA OS");
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
            eprintln!("ORCHIDEA Zapisovač: okno sa nepodarilo vytvoriť");
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
        prvok_novy(hwnd, "BUTTON", "Rozšírené…", WS_TABSTOP, 0, ID_ROZSIRENE);
        // rozšírený režim (skrytý, kým sa neprepne)
        for (id, t) in [
            (301, "1   Úloha"),
            (302, "2   Obraz"),
            (303, "3   Disk"),
            (304, "4   Možnosti"),
            (311, ""),
            (312, ""),
            (313, ""),
        ] {
            prvok_novy(hwnd, "STATIC", t, SS_NOPREFIX, 0, id);
        }
        let uloha = prvok_novy(hwnd, "COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST, 0, ID_ULOHA);
        for t in ULOHY {
            let t = w(t);
            SendMessageW(uloha, CB_ADDSTRING, 0, t.as_ptr() as isize);
        }
        SendMessageW(uloha, CB_SETCURSEL, 0, 0);
        prvok_novy(hwnd, "STATIC", "", SS_NOPREFIX | SS_PATHELLIPSIS, 0, ID_ISO);
        prvok_novy(hwnd, "BUTTON", "Vybrať…", WS_TABSTOP, 0, ID_ISO_VYBER);
        prvok_novy(hwnd, "STATIC", "", SS_NOPREFIX, 0, ID_ROZBOR);
        let system =
            prvok_novy(hwnd, "COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST, 0, ID_SYSTEM);
        for sys in crate::formatuj::PONUKA {
            let t = w(sys.v_ponuke());
            SendMessageW(system, CB_ADDSTRING, 0, t.as_ptr() as isize);
        }
        SendMessageW(system, CB_SETCURSEL, 0, 0);
        let schema =
            prvok_novy(hwnd, "COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST, 0, ID_SCHEMA);
        for t in ["MBR (BIOS aj UEFI)", "GPT (UEFI)"] {
            let t = w(t);
            SendMessageW(schema, CB_ADDSTRING, 0, t.as_ptr() as isize);
        }
        SendMessageW(schema, CB_SETCURSEL, 0, 0);
        let m = prvok_novy(hwnd, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL, WS_EX_CLIENTEDGE, ID_MENOVKA);
        let n = w("napr. FOTKY (nepovinné)");
        SendMessageW(m, EM_SETCUEBANNER, 0, n.as_ptr() as isize);
        SendMessageW(m, EM_LIMITTEXT, 11, 0);
        let u =
            prvok_novy(hwnd, "BUTTON", "Úplné vymazanie (pomalé)", WS_TABSTOP | BS_AUTOCHECKBOX, 0, ID_UPLNE);
        SendMessageW(u, BM_SETCHECK, 0, 0);
        let k = prvok_novy(hwnd, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL, WS_EX_CLIENTEDGE, ID_SUCET);
        let n = w("kontrolný súčet zo stránky distribúcie (nepovinné)");
        SendMessageW(k, EM_SETCUEBANNER, 0, n.as_ptr() as isize);
        SendMessageW(k, EM_LIMITTEXT, 200, 0);
        DragAcceptFiles(hwnd, 1);

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
                rozsirene: false,
                uloha: U_NAPAL,
                iso: None,
                rozbor: None,
                druh_prace: 0,
            })
        });
        s_stavom(|s| {
            obnov_texty(s);
            nastav_iso(s, None);
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
