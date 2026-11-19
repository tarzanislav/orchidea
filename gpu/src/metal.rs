//! Zariadenie Metal (macOS, iOS) cez vlastné väzby na behové prostredie Objective-C.
//!
//! Knižnice systému (libobjc, Foundation, Metal) sa načítajú za behu, takže kód sa preloží všade a mimo
//! systémov Apple sa zariadenie len neponúkne. Kernel sa preloží vlastným prekladačom do MSL; strojový kód
//! vyrobí systémový prekladač Metal (rozhranie operačného systému) s vypnutou „rýchlou matematikou“.
//!
//! Nebezpečný kód je len tu: posielanie správ Objective-C cez `objc_msgSend` s presnými podpismi.

#![allow(unsafe_code)]

use crate::ir::{Kernel, Pristup};
use crate::kniznica::Kniznica;
use crate::texty::{Jazyk, do_textu};
use crate::zariadenie::{
    DataVazby, Druh, Hodnota32, PopisZariadenia, TypZariadenia, Zariadenie, over_argumenty,
};
use orchidea_jadro::{Chyba, Odtlacok, Vysledok};
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::null_mut;
use std::sync::Mutex;

type Id = *mut c_void;
type Sel = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct MtlSize {
    sirka: usize,
    vyska: usize,
    hlbka: usize,
}

const NS_UTF8_STRING_ENCODING: usize = 4;
const MTL_COMMAND_BUFFER_STATUS_COMPLETED: usize = 4;

/// Funkcie behového prostredia Objective-C a Metalu.
struct Objc {
    _objc: Kniznica,
    _foundation: Kniznica,
    _metal: Kniznica,
    get_class: unsafe extern "C" fn(*const c_char) -> Id,
    sel: unsafe extern "C" fn(*const c_char) -> Sel,
    msg: *const c_void,
    pool_push: unsafe extern "C" fn() -> *mut c_void,
    pool_pop: unsafe extern "C" fn(*mut c_void),
    vytvor_zariadenie: unsafe extern "C" fn() -> Id,
}

// SAFETY: ukazovatele na funkcie systémových knižníc sú nemenné a bezpečné z viacerých vlákien.
unsafe impl Send for Objc {}
unsafe impl Sync for Objc {}

fn sym<T: Copy>(k: &Kniznica, meno: &CStr) -> Vysledok<T> {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*const c_void>());
    let p = k.symbol(meno).ok_or_else(|| Chyba::Chybajuce(format!("funkcia {}", meno.to_string_lossy())))?;
    // SAFETY: podpis T zodpovedá dokumentácii funkcie (zaručené pri každom volaní nižšie).
    Ok(unsafe { std::mem::transmute_copy::<*const c_void, T>(&p) })
}

impl Objc {
    fn nacitaj() -> Vysledok<Objc> {
        let objc = Kniznica::otvor(&["/usr/lib/libobjc.A.dylib"])?;
        let foundation = Kniznica::otvor(&["/System/Library/Frameworks/Foundation.framework/Foundation"])?;
        let metal = Kniznica::otvor(&["/System/Library/Frameworks/Metal.framework/Metal"])?;
        Ok(Objc {
            get_class: sym(&objc, c"objc_getClass")?,
            sel: sym(&objc, c"sel_registerName")?,
            msg: objc.symbol(c"objc_msgSend").ok_or_else(|| Chyba::Chybajuce("objc_msgSend".into()))?,
            pool_push: sym(&objc, c"objc_autoreleasePoolPush")?,
            pool_pop: sym(&objc, c"objc_autoreleasePoolPop")?,
            vytvor_zariadenie: sym(&metal, c"MTLCreateSystemDefaultDevice")?,
            _objc: objc,
            _foundation: foundation,
            _metal: metal,
        })
    }

    /// Selektor podľa mena.
    fn s(&self, meno: &CStr) -> Sel {
        // SAFETY: sel_registerName s platným reťazcom.
        unsafe { (self.sel)(meno.as_ptr()) }
    }

    /// objc_msgSend pretypovaný na konkrétny podpis.
    ///
    /// SAFETY: `T` musí presne zodpovedať podpisu metódy, ktorej sa správa posiela.
    unsafe fn m<T: Copy>(&self) -> T {
        assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*const c_void>());
        // SAFETY: zaručuje volajúci.
        unsafe { std::mem::transmute_copy::<*const c_void, T>(&self.msg) }
    }

    fn posli0(&self, o: Id, sel: &CStr) -> Id {
        // SAFETY: metóda bez parametrov, ktorá vracia objekt alebo nil.
        unsafe { self.m::<unsafe extern "C" fn(Id, Sel) -> Id>()(o, self.s(sel)) }
    }

    fn posli_usize(&self, o: Id, sel: &CStr) -> usize {
        // SAFETY: metóda bez parametrov, ktorá vracia NSUInteger.
        unsafe { self.m::<unsafe extern "C" fn(Id, Sel) -> usize>()(o, self.s(sel)) }
    }

    fn uvolni(&self, o: Id) {
        if !o.is_null() {
            let _ = self.posli0(o, c"release");
        }
    }

    fn retazec(&self, t: &str) -> Id {
        // SAFETY: [[NSString alloc] initWithBytes:length:encoding:] – vracia vlastnenú referenciu.
        unsafe {
            let a = self.posli0((self.get_class)(c"NSString".as_ptr()), c"alloc");
            self.m::<unsafe extern "C" fn(Id, Sel, *const c_void, usize, usize) -> Id>()(
                a,
                self.s(c"initWithBytes:length:encoding:"),
                t.as_ptr() as *const c_void,
                t.len(),
                NS_UTF8_STRING_ENCODING,
            )
        }
    }

    fn text(&self, o: Id) -> String {
        if o.is_null() {
            return String::new();
        }
        // SAFETY: [NSString UTF8String] vracia reťazec platný počas života objektu.
        unsafe {
            let p = self.m::<unsafe extern "C" fn(Id, Sel) -> *const c_char>()(o, self.s(c"UTF8String"));
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        }
    }

    fn popis_chyby(&self, chyba: Id) -> String {
        if chyba.is_null() {
            return "neznáma chyba".into();
        }
        self.text(self.posli0(chyba, c"localizedDescription"))
    }
}

struct Program {
    pso: Id,
}

/// Vlastnené objekty Objective-C – pri zániku sa uvoľnia.
struct Vlastnene<'a> {
    o: &'a Objc,
    objekty: Vec<Id>,
}

impl Drop for Vlastnene<'_> {
    fn drop(&mut self) {
        self.objekty.iter().for_each(|x| self.o.uvolni(*x));
    }
}

struct Vnutro {
    o: Objc,
    zariadenie: Id,
    fronta: Id,
    programy: HashMap<Odtlacok, Program>,
}

// SAFETY: objekty Metalu sú bezpečné z viacerých vlákien; prístup chráni Mutex.
unsafe impl Send for Vnutro {}

impl Drop for Vnutro {
    fn drop(&mut self) {
        for p in self.programy.values() {
            self.o.uvolni(p.pso);
        }
        self.o.uvolni(self.fronta);
        self.o.uvolni(self.zariadenie);
    }
}

/// Zariadenie Metal.
pub struct Metal {
    popis: PopisZariadenia,
    vnutro: Mutex<Vnutro>,
}

impl std::fmt::Debug for Metal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Metal({})", self.popis)
    }
}

impl Metal {
    /// Otvorí predvolené zariadenie Metal.
    pub fn otvor() -> Vysledok<Metal> {
        let o = Objc::nacitaj()?;
        // SAFETY: MTLCreateSystemDefaultDevice nemá parametre a vracia vlastnenú referenciu alebo nil.
        let zariadenie = unsafe { (o.vytvor_zariadenie)() };
        if zariadenie.is_null() {
            return Err(Chyba::Chybajuce(
                "zariadenie Metal (MTLCreateSystemDefaultDevice vrátil nil)".into(),
            ));
        }
        let meno = o.text(o.posli0(zariadenie, c"name"));
        // SAFETY: [MTLDevice hasUnifiedMemory] a [MTLDevice isLowPower] vracajú BOOL.
        let (zjednotena, uspornej) = unsafe {
            let b = o.m::<unsafe extern "C" fn(Id, Sel) -> bool>();
            (b(zariadenie, o.s(c"hasUnifiedMemory")), b(zariadenie, o.s(c"isLowPower")))
        };
        let typ = if zjednotena || uspornej { TypZariadenia::Integrovane } else { TypZariadenia::Diskretne };
        let fronta = o.posli0(zariadenie, c"newCommandQueue");
        if fronta.is_null() {
            o.uvolni(zariadenie);
            return Err(Chyba::Vypocet("Metal: newCommandQueue vrátil nil".into()));
        }
        Ok(Metal {
            popis: PopisZariadenia { druh: Druh::Metal, typ, meno, ovladac: "Metal".into() },
            vnutro: Mutex::new(Vnutro { o, zariadenie, fronta, programy: HashMap::new() }),
        })
    }
}

impl Vnutro {
    fn program(&mut self, k: &Kernel) -> Vysledok<Id> {
        let kluc = k.odtlacok();
        if let Some(p) = self.programy.get(&kluc) {
            return Ok(p.pso);
        }
        let zdroj = do_textu(k, Jazyk::Msl)?;
        let o = &self.o;
        let src = o.retazec(&zdroj);
        // SAFETY: správy MTLCompileOptions a MTLDevice podľa dokumentácie Apple; vlastnené objekty uvoľníme.
        let pso = unsafe {
            let moznosti =
                o.posli0(o.posli0((o.get_class)(c"MTLCompileOptions".as_ptr()), c"alloc"), c"init");
            o.m::<unsafe extern "C" fn(Id, Sel, bool)>()(moznosti, o.s(c"setFastMathEnabled:"), false);
            let mut chyba: Id = null_mut();
            let kniznica = o.m::<unsafe extern "C" fn(Id, Sel, Id, Id, *mut Id) -> Id>()(
                self.zariadenie,
                o.s(c"newLibraryWithSource:options:error:"),
                src,
                moznosti,
                &mut chyba,
            );
            o.uvolni(moznosti);
            o.uvolni(src);
            if kniznica.is_null() {
                return Err(Chyba::Vypocet(format!("prekladač MSL: {}", o.popis_chyby(chyba))));
            }
            let meno = o.retazec("hlavna");
            let funkcia = o.m::<unsafe extern "C" fn(Id, Sel, Id) -> Id>()(
                kniznica,
                o.s(c"newFunctionWithName:"),
                meno,
            );
            o.uvolni(meno);
            o.uvolni(kniznica);
            if funkcia.is_null() {
                return Err(Chyba::Chybajuce("funkcia hlavna v knižnici Metal".into()));
            }
            let mut chyba: Id = null_mut();
            let pso = o.m::<unsafe extern "C" fn(Id, Sel, Id, *mut Id) -> Id>()(
                self.zariadenie,
                o.s(c"newComputePipelineStateWithFunction:error:"),
                funkcia,
                &mut chyba,
            );
            o.uvolni(funkcia);
            if pso.is_null() {
                return Err(Chyba::Vypocet(format!("Metal pipeline: {}", o.popis_chyby(chyba))));
            }
            pso
        };
        self.programy.insert(kluc, Program { pso });
        Ok(pso)
    }
}

impl Zariadenie for Metal {
    fn popis(&self) -> &PopisZariadenia {
        &self.popis
    }

    fn spusti(
        &self,
        k: &Kernel,
        vazby: &mut [DataVazby<'_>],
        konstanty: &[Hodnota32],
        skupiny: [u32; 3],
    ) -> Vysledok<()> {
        over_argumenty(k, vazby, konstanty)?;
        if skupiny.contains(&0) {
            return Ok(());
        }
        let mut v = self.vnutro.lock().unwrap_or_else(|e| e.into_inner());
        let pso = v.program(k)?;
        let o = &v.o;
        // SAFETY: správy MTLDevice, MTLCommandQueue, MTLCommandBuffer, MTLComputeCommandEncoder a MTLBuffer
        // podľa dokumentácie Apple; dočasné objekty uvoľní automatický pool, vlastnené uvoľníme sami.
        unsafe {
            let pool = (o.pool_push)();
            struct Pool<'a>(&'a Objc, *mut c_void);
            impl Drop for Pool<'_> {
                fn drop(&mut self) {
                    // SAFETY: pool vytvorený vyššie v tom istom vlákne.
                    unsafe { (self.0.pool_pop)(self.1) };
                }
            }
            let _pool = Pool(o, pool);
            let mut buffery = Vlastnene { o, objekty: Vec::with_capacity(vazby.len()) };
            let novy_buffer = o.m::<unsafe extern "C" fn(Id, Sel, *const c_void, usize, usize) -> Id>();
            for d in vazby.iter() {
                let b = d.ako_bajty();
                let buf = novy_buffer(
                    v.zariadenie,
                    o.s(c"newBufferWithBytes:length:options:"),
                    b.as_ptr() as *const c_void,
                    b.len(),
                    0,
                );
                if buf.is_null() {
                    return Err(Chyba::Vypocet("Metal: newBufferWithBytes vrátil nil".into()));
                }
                buffery.objekty.push(buf);
            }
            let cmd = o.posli0(v.fronta, c"commandBuffer");
            let enc = o.posli0(cmd, c"computeCommandEncoder");
            if cmd.is_null() || enc.is_null() {
                return Err(Chyba::Vypocet("Metal: nepodarilo sa vytvoriť príkazy".into()));
            }
            o.m::<unsafe extern "C" fn(Id, Sel, Id)>()(enc, o.s(c"setComputePipelineState:"), pso);
            let nastav_buffer = o.m::<unsafe extern "C" fn(Id, Sel, Id, usize, usize)>();
            for (i, b) in buffery.objekty.iter().enumerate() {
                nastav_buffer(enc, o.s(c"setBuffer:offset:atIndex:"), *b, 0, i);
            }
            let nastav_bajty = o.m::<unsafe extern "C" fn(Id, Sel, *const c_void, usize, usize)>();
            let dlzky: Vec<u32> = vazby.iter().map(|d| d.dlzka() as u32).collect();
            let n = vazby.len();
            nastav_bajty(enc, o.s(c"setBytes:length:atIndex:"), dlzky.as_ptr() as *const c_void, 4 * n, n);
            if !konstanty.is_empty() {
                let bity: Vec<u32> = konstanty.iter().map(|h| h.bity()).collect();
                nastav_bajty(
                    enc,
                    o.s(c"setBytes:length:atIndex:"),
                    bity.as_ptr() as *const c_void,
                    4 * bity.len(),
                    n + 1,
                );
            }
            let rozmer =
                |x: [u32; 3]| MtlSize { sirka: x[0] as usize, vyska: x[1] as usize, hlbka: x[2] as usize };
            o.m::<unsafe extern "C" fn(Id, Sel, MtlSize, MtlSize)>()(
                enc,
                o.s(c"dispatchThreadgroups:threadsPerThreadgroup:"),
                rozmer(skupiny),
                rozmer(k.skupina),
            );
            let _ = o.posli0(enc, c"endEncoding");
            let _ = o.posli0(cmd, c"commit");
            let _ = o.posli0(cmd, c"waitUntilCompleted");
            let stav = o.posli_usize(cmd, c"status");
            if stav != MTL_COMMAND_BUFFER_STATUS_COMPLETED {
                return Err(Chyba::Vypocet(format!(
                    "Metal: výpočet zlyhal: {}",
                    o.popis_chyby(o.posli0(cmd, c"error"))
                )));
            }
            let obsah = o.m::<unsafe extern "C" fn(Id, Sel) -> *const u8>();
            for ((b, d), popis) in buffery.objekty.iter().zip(vazby.iter_mut()).zip(&k.vazby) {
                if popis.pristup != Pristup::Citanie {
                    let p = obsah(*b, o.s(c"contents"));
                    let bajty = std::slice::from_raw_parts(p, d.bajtov()).to_vec();
                    d.nastav_z_bajtov(&bajty)?;
                }
            }
        }
        Ok(())
    }
}
