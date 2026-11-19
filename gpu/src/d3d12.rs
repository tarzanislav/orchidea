//! Zariadenie Direct3D 12 cez vlastné väzby na rozhrania COM (bez cudzích knižníc).
//!
//! Všetky knižnice systému Windows (d3d12.dll, dxgi.dll, d3dcompiler_47.dll, kernel32.dll) sa načítajú
//! za behu, takže kód sa preloží na každej platforme a mimo Windows sa zariadenie len neponúkne.
//! Kernel sa preloží vlastným prekladačom do HLSL a systémový prekladač Direct3D ho preloží v prísnom
//! režime IEEE (bez zjednodušení výpočtov). Funguje aj softvérový ovládač WARP – na ňom beží CI.
//!
//! Nebezpečný kód je len tu: volania metód COM cez tabuľky virtuálnych funkcií podľa hlavičiek d3d12.h
//! a dxgi.h. Každý výsledok HRESULT sa kontroluje.

#![allow(unsafe_code)]

use crate::ir::{Kernel, Pristup};
use crate::kniznica::Kniznica;
use crate::texty::{Jazyk, do_textu};
use crate::zariadenie::{
    DataVazby, Druh, Hodnota32, PopisZariadenia, TypZariadenia, Zariadenie, over_argumenty,
};
use orchidea_jadro::{Chyba, Odtlacok, Vysledok};
use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

type Obj = *mut c_void;
type HResult = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid(u32, u16, u16, [u8; 8]);

const IID_IDXGI_FACTORY1: Guid =
    Guid(0x770aae78, 0xf26f, 0x4dba, [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87]);
const IID_ID3D12_DEVICE: Guid =
    Guid(0x189819f1, 0x1db6, 0x4b57, [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7]);
const IID_ID3D12_COMMAND_QUEUE: Guid =
    Guid(0x0ec870a6, 0x5d7e, 0x4c22, [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed]);
const IID_ID3D12_COMMAND_ALLOCATOR: Guid =
    Guid(0x6102dee4, 0xaf59, 0x4b09, [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24]);
const IID_ID3D12_GRAPHICS_COMMAND_LIST: Guid =
    Guid(0x5b160d0f, 0xac1b, 0x4185, [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55]);
const IID_ID3D12_FENCE: Guid =
    Guid(0x0a753dcf, 0xc4d8, 0x4b91, [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76]);
const IID_ID3D12_ROOT_SIGNATURE: Guid =
    Guid(0xc54a6b66, 0x72df, 0x4ee8, [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14]);
const IID_ID3D12_PIPELINE_STATE: Guid =
    Guid(0x765a30f3, 0xf624, 0x4c6f, [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45]);
const IID_ID3D12_RESOURCE: Guid =
    Guid(0x696442be, 0xa72e, 0x4059, [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad]);

// indexy metód v tabuľkách virtuálnych funkcií
const UNKNOWN_RELEASE: usize = 2;
const FACTORY1_ENUM_ADAPTERS1: usize = 12;
const ADAPTER1_GET_DESC1: usize = 10;
const DEVICE_CREATE_COMMAND_QUEUE: usize = 8;
const DEVICE_CREATE_COMMAND_ALLOCATOR: usize = 9;
const DEVICE_CREATE_COMPUTE_PIPELINE_STATE: usize = 11;
const DEVICE_CREATE_COMMAND_LIST: usize = 12;
const DEVICE_CREATE_ROOT_SIGNATURE: usize = 16;
const DEVICE_CREATE_COMMITTED_RESOURCE: usize = 27;
const DEVICE_CREATE_FENCE: usize = 36;
const DEVICE_GET_DEVICE_REMOVED_REASON: usize = 37;
const QUEUE_EXECUTE_COMMAND_LISTS: usize = 10;
const QUEUE_SIGNAL: usize = 14;
const ALLOCATOR_RESET: usize = 8;
const FENCE_GET_COMPLETED_VALUE: usize = 8;
const FENCE_SET_EVENT_ON_COMPLETION: usize = 9;
const RESOURCE_MAP: usize = 8;
const RESOURCE_UNMAP: usize = 9;
const RESOURCE_GET_GPU_VIRTUAL_ADDRESS: usize = 11;
const LIST_CLOSE: usize = 9;
const LIST_RESET: usize = 10;
const LIST_DISPATCH: usize = 14;
const LIST_COPY_BUFFER_REGION: usize = 15;
const LIST_SET_PIPELINE_STATE: usize = 25;
const LIST_RESOURCE_BARRIER: usize = 26;
const LIST_SET_COMPUTE_ROOT_SIGNATURE: usize = 29;
const LIST_SET_COMPUTE_ROOT_32BIT_CONSTANTS: usize = 35;
const LIST_SET_COMPUTE_ROOT_UAV: usize = 41;
const BLOB_GET_BUFFER_POINTER: usize = 3;
const BLOB_GET_BUFFER_SIZE: usize = 4;

const LIST_TYPE_COMPUTE: u32 = 2;
const HEAP_DEFAULT: u32 = 1;
const HEAP_UPLOAD: u32 = 2;
const HEAP_READBACK: u32 = 3;
const STATE_COMMON: u32 = 0;
const STATE_UNORDERED_ACCESS: u32 = 0x8;
const STATE_COPY_DEST: u32 = 0x400;
const STATE_COPY_SOURCE: u32 = 0x800;
const STATE_GENERIC_READ: u32 = 0xac3;
const RESOURCE_FLAG_ALLOW_UAV: u32 = 0x4;
const ROOT_PARAM_32BIT_CONSTANTS: u32 = 1;
const ROOT_PARAM_UAV: u32 = 4;
const D3DCOMPILE_IEEE_STRICTNESS: u32 = 1 << 13;
const D3DCOMPILE_OPTIMIZATION_LEVEL3: u32 = 1 << 15;
const DXGI_ERROR_NOT_FOUND: HResult = 0x887a0002_u32 as i32;
const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;

#[repr(C)]
struct AdapterDesc1 {
    description: [u16; 128],
    vendor_id: u32,
    device_id: u32,
    sub_sys_id: u32,
    revision: u32,
    dedicated_video_memory: usize,
    dedicated_system_memory: usize,
    shared_system_memory: usize,
    adapter_luid: [u32; 2],
    flags: u32,
}

#[repr(C)]
struct CommandQueueDesc {
    typ: u32,
    priority: i32,
    flags: u32,
    node_mask: u32,
}

#[repr(C)]
struct RootParameter {
    typ: u32,
    /// únia D3D12_ROOT_PARAMETER (zarovnaná na 8 bajtov kvôli ukazovateľu v tabuľke deskriptorov)
    unia: [u64; 2],
    viditelnost: u32,
}

#[repr(C)]
struct RootSignatureDesc {
    num_parameters: u32,
    p_parameters: *const RootParameter,
    num_static_samplers: u32,
    p_static_samplers: *const c_void,
    flags: u32,
}

#[repr(C)]
struct ShaderBytecode {
    p: *const c_void,
    dlzka: usize,
}

#[repr(C)]
struct ComputePipelineStateDesc {
    root_signature: Obj,
    cs: ShaderBytecode,
    node_mask: u32,
    cached_pso: ShaderBytecode,
    flags: u32,
}

#[repr(C)]
struct HeapProperties {
    typ: u32,
    cpu_page_property: u32,
    memory_pool_preference: u32,
    creation_node_mask: u32,
    visible_node_mask: u32,
}

#[repr(C)]
struct ResourceDesc {
    dimension: u32,
    alignment: u64,
    width: u64,
    height: u32,
    depth_or_array_size: u16,
    mip_levels: u16,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    layout: u32,
    flags: u32,
}

#[repr(C)]
struct ResourceBarrier {
    typ: u32,
    flags: u32,
    resource: Obj,
    subresource: u32,
    state_before: u32,
    state_after: u32,
}

/// Získa metódu objektu COM z jeho tabuľky virtuálnych funkcií.
///
/// SAFETY: `objekt` je platný objekt COM, `index` je správny index metódy a `T` jej presný podpis.
unsafe fn metoda<T: Copy>(objekt: Obj, index: usize) -> T {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*const c_void>());
    // SAFETY: prvé slovo objektu COM je ukazovateľ na tabuľku metód.
    unsafe {
        let tabulka = *(objekt as *const *const *const c_void);
        let p = *tabulka.add(index);
        std::mem::transmute_copy::<*const c_void, T>(&p)
    }
}

fn over(hr: HResult, co: &str) -> Vysledok<()> {
    if hr >= 0 {
        Ok(())
    } else {
        Err(Chyba::Vypocet(format!("Direct3D 12 {co}: HRESULT 0x{:08x}", hr as u32)))
    }
}

/// Vlastník referencie COM – pri zániku zavolá Release.
struct Com(Obj);

// SAFETY: objekty Direct3D 12 sú voľne vláknové; prístup k zariadeniu chráni Mutex.
unsafe impl Send for Com {}
unsafe impl Sync for Com {}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: platný objekt COM, ktorého referenciu vlastníme.
            unsafe {
                let release: unsafe extern "system" fn(Obj) -> u32 = metoda(self.0, UNKNOWN_RELEASE);
                release(self.0);
            }
        }
    }
}

fn blob_bajty(b: &Com) -> Vec<u8> {
    // SAFETY: ID3DBlob – GetBufferPointer a GetBufferSize bez parametrov.
    unsafe {
        let ptr: unsafe extern "system" fn(Obj) -> *const u8 = metoda(b.0, BLOB_GET_BUFFER_POINTER);
        let size: unsafe extern "system" fn(Obj) -> usize = metoda(b.0, BLOB_GET_BUFFER_SIZE);
        let (p, n) = (ptr(b.0), size(b.0));
        if p.is_null() { Vec::new() } else { std::slice::from_raw_parts(p, n).to_vec() }
    }
}

type FnCreateDevice = unsafe extern "system" fn(Obj, u32, *const Guid, *mut Obj) -> HResult;
type FnSerializeRootSignature =
    unsafe extern "system" fn(*const RootSignatureDesc, u32, *mut Obj, *mut Obj) -> HResult;
type FnCreateFactory1 = unsafe extern "system" fn(*const Guid, *mut Obj) -> HResult;
#[allow(clippy::type_complexity)]
type FnD3DCompile = unsafe extern "system" fn(
    *const c_void,
    usize,
    *const c_char,
    *const c_void,
    *const c_void,
    *const c_char,
    *const c_char,
    u32,
    u32,
    *mut Obj,
    *mut Obj,
) -> HResult;
type FnCreateEventW = unsafe extern "system" fn(*const c_void, i32, i32, *const u16) -> *mut c_void;
type FnWaitForSingleObject = unsafe extern "system" fn(*mut c_void, u32) -> u32;
type FnCloseHandle = unsafe extern "system" fn(*mut c_void) -> i32;

/// Funkcie systémových knižníc.
struct Kniznice {
    _d3d12: Kniznica,
    _dxgi: Kniznica,
    _prekladac: Kniznica,
    _kernel32: Kniznica,
    create_device: FnCreateDevice,
    serialize_root_signature: FnSerializeRootSignature,
    create_factory1: FnCreateFactory1,
    d3d_compile: FnD3DCompile,
    create_event: FnCreateEventW,
    wait: FnWaitForSingleObject,
    close_handle: FnCloseHandle,
}

impl Kniznice {
    fn nacitaj() -> Vysledok<Kniznice> {
        let d3d12 = Kniznica::otvor(&["d3d12.dll"])?;
        let dxgi = Kniznica::otvor(&["dxgi.dll"])?;
        let prekladac = Kniznica::otvor(&["d3dcompiler_47.dll"])?;
        let kernel32 = Kniznica::otvor(&["kernel32.dll"])?;
        fn sym<T: Copy>(k: &Kniznica, meno: &std::ffi::CStr) -> Vysledok<T> {
            assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*const c_void>());
            let p = k
                .symbol(meno)
                .ok_or_else(|| Chyba::Chybajuce(format!("funkcia {}", meno.to_string_lossy())))?;
            // SAFETY: podpis T zodpovedá dokumentácii funkcie (zaručuje volajúci nižšie).
            Ok(unsafe { std::mem::transmute_copy::<*const c_void, T>(&p) })
        }
        Ok(Kniznice {
            create_device: sym(&d3d12, c"D3D12CreateDevice")?,
            serialize_root_signature: sym(&d3d12, c"D3D12SerializeRootSignature")?,
            create_factory1: sym(&dxgi, c"CreateDXGIFactory1")?,
            d3d_compile: sym(&prekladac, c"D3DCompile")?,
            create_event: sym(&kernel32, c"CreateEventW")?,
            wait: sym(&kernel32, c"WaitForSingleObject")?,
            close_handle: sym(&kernel32, c"CloseHandle")?,
            _d3d12: d3d12,
            _dxgi: dxgi,
            _prekladac: prekladac,
            _kernel32: kernel32,
        })
    }
}

struct Program {
    _korenovy_podpis: Com,
    pso: Com,
    korenovy_podpis: Obj,
}

struct Vnutro {
    k: Arc<Kniznice>,
    zariadenie: Com,
    fronta: Com,
    alokator: Com,
    zoznam: Com,
    plot: Com,
    udalost: *mut c_void,
    hodnota_plota: u64,
    programy: HashMap<Odtlacok, Program>,
}

// SAFETY: prístup k vnútru je chránený zámkom Mutex.
unsafe impl Send for Vnutro {}

impl Drop for Vnutro {
    fn drop(&mut self) {
        // Počká na dokončenie práce, potom zavrie udalosť; objekty COM uvoľní Drop.
        let _ = self.cakaj();
        if !self.udalost.is_null() {
            // SAFETY: platná rukoväť udalosti, ktorú vlastníme.
            unsafe { (self.k.close_handle)(self.udalost) };
        }
    }
}

/// Zariadenie Direct3D 12.
pub struct D3d12 {
    popis: PopisZariadenia,
    vnutro: Mutex<Vnutro>,
}

impl std::fmt::Debug for D3d12 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "D3d12({})", self.popis)
    }
}

impl D3d12 {
    /// Otvorí všetky adaptéry Direct3D 12 (aj softvérový WARP).
    pub fn otvor_vsetky() -> Vysledok<Vec<D3d12>> {
        let k = Arc::new(Kniznice::nacitaj()?);
        let mut tovaren: Obj = null_mut();
        // SAFETY: platný IID a výstupný ukazovateľ.
        over(unsafe { (k.create_factory1)(&IID_IDXGI_FACTORY1, &mut tovaren) }, "CreateDXGIFactory1")?;
        let tovaren = Com(tovaren);
        let mut vysledok = Vec::new();
        let mut dovody = Vec::new();
        for i in 0..64u32 {
            let mut adapter: Obj = null_mut();
            // SAFETY: IDXGIFactory1::EnumAdapters1(UINT, IDXGIAdapter1**).
            let hr = unsafe {
                let f: unsafe extern "system" fn(Obj, u32, *mut Obj) -> HResult =
                    metoda(tovaren.0, FACTORY1_ENUM_ADAPTERS1);
                f(tovaren.0, i, &mut adapter)
            };
            if hr == DXGI_ERROR_NOT_FOUND {
                break;
            }
            over(hr, "EnumAdapters1")?;
            let adapter = Com(adapter);
            match Self::otvor(k.clone(), &adapter) {
                Ok(z) => vysledok.push(z),
                Err(e) => dovody.push(e.to_string()),
            }
        }
        if vysledok.is_empty() {
            return Err(Chyba::Chybajuce(format!("adaptér Direct3D 12 ({})", dovody.join("; "))));
        }
        Ok(vysledok)
    }

    fn otvor(k: Arc<Kniznice>, adapter: &Com) -> Vysledok<D3d12> {
        // SAFETY: IDXGIAdapter1::GetDesc1(DXGI_ADAPTER_DESC1*); štruktúra má presné rozloženie z dxgi.h.
        let desc = unsafe {
            let mut d: AdapterDesc1 = std::mem::zeroed();
            let f: unsafe extern "system" fn(Obj, *mut AdapterDesc1) -> HResult =
                metoda(adapter.0, ADAPTER1_GET_DESC1);
            over(f(adapter.0, &mut d), "GetDesc1")?;
            d
        };
        let koniec = desc.description.iter().position(|&c| c == 0).unwrap_or(128);
        let meno = String::from_utf16_lossy(&desc.description[..koniec]);
        let typ = if desc.flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0
            || (desc.vendor_id == 0x1414 && desc.device_id == 0x8c)
        {
            TypZariadenia::Softverove
        } else if desc.dedicated_video_memory >= 512 << 20 {
            TypZariadenia::Diskretne
        } else {
            TypZariadenia::Integrovane
        };

        let mut zariadenie: Obj = null_mut();
        // SAFETY: platný adaptér, úroveň funkcií 11_0, IID zariadenia.
        over(
            unsafe { (k.create_device)(adapter.0, 0xb000, &IID_ID3D12_DEVICE, &mut zariadenie) },
            "D3D12CreateDevice",
        )?;
        let zariadenie = Com(zariadenie);
        let d = zariadenie.0;
        // SAFETY: všetky volania nižšie zodpovedajú podpisom metód ID3D12Device v d3d12.h.
        unsafe {
            let desc_fronty =
                CommandQueueDesc { typ: LIST_TYPE_COMPUTE, priority: 0, flags: 0, node_mask: 0 };
            let mut fronta: Obj = null_mut();
            let f: unsafe extern "system" fn(Obj, *const CommandQueueDesc, *const Guid, *mut Obj) -> HResult =
                metoda(d, DEVICE_CREATE_COMMAND_QUEUE);
            over(f(d, &desc_fronty, &IID_ID3D12_COMMAND_QUEUE, &mut fronta), "CreateCommandQueue")?;
            let fronta = Com(fronta);

            let mut alokator: Obj = null_mut();
            let f: unsafe extern "system" fn(Obj, u32, *const Guid, *mut Obj) -> HResult =
                metoda(d, DEVICE_CREATE_COMMAND_ALLOCATOR);
            over(
                f(d, LIST_TYPE_COMPUTE, &IID_ID3D12_COMMAND_ALLOCATOR, &mut alokator),
                "CreateCommandAllocator",
            )?;
            let alokator = Com(alokator);

            let mut zoznam: Obj = null_mut();
            let f: unsafe extern "system" fn(Obj, u32, u32, Obj, Obj, *const Guid, *mut Obj) -> HResult =
                metoda(d, DEVICE_CREATE_COMMAND_LIST);
            over(
                f(
                    d,
                    0,
                    LIST_TYPE_COMPUTE,
                    alokator.0,
                    null_mut(),
                    &IID_ID3D12_GRAPHICS_COMMAND_LIST,
                    &mut zoznam,
                ),
                "CreateCommandList",
            )?;
            let zoznam = Com(zoznam);
            let close: unsafe extern "system" fn(Obj) -> HResult = metoda(zoznam.0, LIST_CLOSE);
            over(close(zoznam.0), "Close")?;

            let mut plot: Obj = null_mut();
            let f: unsafe extern "system" fn(Obj, u64, u32, *const Guid, *mut Obj) -> HResult =
                metoda(d, DEVICE_CREATE_FENCE);
            over(f(d, 0, 0, &IID_ID3D12_FENCE, &mut plot), "CreateFence")?;
            let plot = Com(plot);
            let udalost = (k.create_event)(null(), 0, 0, null());
            if udalost.is_null() {
                return Err(Chyba::Vypocet("Direct3D 12: CreateEventW zlyhalo".into()));
            }
            Ok(D3d12 {
                popis: PopisZariadenia {
                    druh: Druh::D3d12,
                    typ,
                    meno,
                    ovladac: format!("Direct3D 12, úroveň 11_0, výrobca 0x{:04x}", desc.vendor_id),
                },
                vnutro: Mutex::new(Vnutro {
                    k,
                    zariadenie,
                    fronta,
                    alokator,
                    zoznam,
                    plot,
                    udalost,
                    hodnota_plota: 0,
                    programy: HashMap::new(),
                }),
            })
        }
    }
}

impl Vnutro {
    fn cakaj(&mut self) -> Vysledok<()> {
        self.hodnota_plota += 1;
        let h = self.hodnota_plota;
        // SAFETY: ID3D12CommandQueue::Signal a ID3D12Fence::GetCompletedValue/SetEventOnCompletion.
        unsafe {
            let signal: unsafe extern "system" fn(Obj, Obj, u64) -> HResult =
                metoda(self.fronta.0, QUEUE_SIGNAL);
            over(signal(self.fronta.0, self.plot.0, h), "Signal")?;
            let hotovo: unsafe extern "system" fn(Obj) -> u64 =
                metoda(self.plot.0, FENCE_GET_COMPLETED_VALUE);
            if hotovo(self.plot.0) < h {
                let nastav: unsafe extern "system" fn(Obj, u64, *mut c_void) -> HResult =
                    metoda(self.plot.0, FENCE_SET_EVENT_ON_COMPLETION);
                over(nastav(self.plot.0, h, self.udalost), "SetEventOnCompletion")?;
                if (self.k.wait)(self.udalost, 120_000) != 0 {
                    return Err(Chyba::Vypocet("Direct3D 12: vypršal čas čakania na výsledok".into()));
                }
            }
        }
        Ok(())
    }

    fn program(&mut self, k: &Kernel) -> Vysledok<(Obj, Obj)> {
        let kluc = k.odtlacok();
        if !self.programy.contains_key(&kluc) {
            let p = self.vytvor_program(k)?;
            self.programy.insert(kluc, p);
        }
        let p = &self.programy[&kluc];
        Ok((p.korenovy_podpis, p.pso.0))
    }

    fn vytvor_program(&self, kern: &Kernel) -> Vysledok<Program> {
        let zdroj = do_textu(kern, Jazyk::Hlsl)?;
        let d = self.zariadenie.0;
        // SAFETY: D3DCompile, D3D12SerializeRootSignature a metódy ID3D12Device podľa dokumentácie.
        unsafe {
            let (mut kod, mut chyby): (Obj, Obj) = (null_mut(), null_mut());
            let hr = (self.k.d3d_compile)(
                zdroj.as_ptr() as *const c_void,
                zdroj.len(),
                c"orchidea".as_ptr(),
                null(),
                null(),
                c"hlavna".as_ptr(),
                c"cs_5_1".as_ptr(),
                D3DCOMPILE_IEEE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut kod,
                &mut chyby,
            );
            let (kod, chyby) = (Com(kod), Com(chyby));
            if hr < 0 {
                let sprava = if chyby.0.is_null() {
                    String::new()
                } else {
                    String::from_utf8_lossy(&blob_bajty(&chyby)).to_string()
                };
                return Err(Chyba::Vypocet(format!(
                    "prekladač HLSL: {sprava} (HRESULT 0x{:08x})",
                    hr as u32
                )));
            }
            let bajtkod = blob_bajty(&kod);

            // koreňový podpis: UAV pre každý buffer + 32-bitové konštanty (konštanty kernelu a dĺžky)
            let n = kern.vazby.len();
            let mut parametre: Vec<RootParameter> = (0..n)
                .map(|i| RootParameter { typ: ROOT_PARAM_UAV, unia: [i as u64, 0], viditelnost: 0 })
                .collect();
            let konstant = (kern.konstanty.len() + n) as u64;
            parametre.push(RootParameter {
                typ: ROOT_PARAM_32BIT_CONSTANTS,
                unia: [0, konstant],
                viditelnost: 0,
            });
            let desc = RootSignatureDesc {
                num_parameters: parametre.len() as u32,
                p_parameters: parametre.as_ptr(),
                num_static_samplers: 0,
                p_static_samplers: null(),
                flags: 0,
            };
            let (mut blob, mut chyby): (Obj, Obj) = (null_mut(), null_mut());
            let hr = (self.k.serialize_root_signature)(&desc, 1, &mut blob, &mut chyby);
            let (blob, chyby) = (Com(blob), Com(chyby));
            if hr < 0 {
                let sprava = if chyby.0.is_null() {
                    String::new()
                } else {
                    String::from_utf8_lossy(&blob_bajty(&chyby)).to_string()
                };
                return Err(Chyba::Vypocet(format!(
                    "koreňový podpis: {sprava} (HRESULT 0x{:08x})",
                    hr as u32
                )));
            }
            let podpis_bajty = blob_bajty(&blob);
            let mut podpis: Obj = null_mut();
            let f: unsafe extern "system" fn(
                Obj,
                u32,
                *const c_void,
                usize,
                *const Guid,
                *mut Obj,
            ) -> HResult = metoda(d, DEVICE_CREATE_ROOT_SIGNATURE);
            over(
                f(
                    d,
                    0,
                    podpis_bajty.as_ptr() as *const c_void,
                    podpis_bajty.len(),
                    &IID_ID3D12_ROOT_SIGNATURE,
                    &mut podpis,
                ),
                "CreateRootSignature",
            )?;
            let podpis = Com(podpis);
            let pso_desc = ComputePipelineStateDesc {
                root_signature: podpis.0,
                cs: ShaderBytecode { p: bajtkod.as_ptr() as *const c_void, dlzka: bajtkod.len() },
                node_mask: 0,
                cached_pso: ShaderBytecode { p: null(), dlzka: 0 },
                flags: 0,
            };
            let mut pso: Obj = null_mut();
            let f: unsafe extern "system" fn(
                Obj,
                *const ComputePipelineStateDesc,
                *const Guid,
                *mut Obj,
            ) -> HResult = metoda(d, DEVICE_CREATE_COMPUTE_PIPELINE_STATE);
            over(f(d, &pso_desc, &IID_ID3D12_PIPELINE_STATE, &mut pso), "CreateComputePipelineState")?;
            Ok(Program { korenovy_podpis: podpis.0, _korenovy_podpis: podpis, pso: Com(pso) })
        }
    }

    fn zdroj(&self, halda: u32, bajtov: u64, priznaky: u32, stav: u32) -> Vysledok<Com> {
        let vlastnosti = HeapProperties {
            typ: halda,
            cpu_page_property: 0,
            memory_pool_preference: 0,
            creation_node_mask: 1,
            visible_node_mask: 1,
        };
        let desc = ResourceDesc {
            dimension: 1,
            alignment: 0,
            width: bajtov,
            height: 1,
            depth_or_array_size: 1,
            mip_levels: 1,
            format: 0,
            sample_count: 1,
            sample_quality: 0,
            layout: 1,
            flags: priznaky,
        };
        let d = self.zariadenie.0;
        let mut r: Obj = null_mut();
        // SAFETY: ID3D12Device::CreateCommittedResource podľa d3d12.h.
        unsafe {
            let f: unsafe extern "system" fn(
                Obj,
                *const HeapProperties,
                u32,
                *const ResourceDesc,
                u32,
                *const c_void,
                *const Guid,
                *mut Obj,
            ) -> HResult = metoda(d, DEVICE_CREATE_COMMITTED_RESOURCE);
            over(
                f(d, &vlastnosti, 0, &desc, stav, null(), &IID_ID3D12_RESOURCE, &mut r),
                "CreateCommittedResource",
            )?;
        }
        Ok(Com(r))
    }

    fn dovod_straty(&self) -> String {
        // SAFETY: ID3D12Device::GetDeviceRemovedReason().
        let hr = unsafe {
            let f: unsafe extern "system" fn(Obj) -> HResult =
                metoda(self.zariadenie.0, DEVICE_GET_DEVICE_REMOVED_REASON);
            f(self.zariadenie.0)
        };
        if hr == 0 { String::new() } else { format!(" (zariadenie stratené: 0x{:08x})", hr as u32) }
    }
}

fn mapuj(r: &Com) -> Vysledok<*mut u8> {
    let mut p: *mut c_void = null_mut();
    // SAFETY: ID3D12Resource::Map(0, null, &p) – celý buffer.
    unsafe {
        let f: unsafe extern "system" fn(Obj, u32, *const c_void, *mut *mut c_void) -> HResult =
            metoda(r.0, RESOURCE_MAP);
        over(f(r.0, 0, null(), &mut p), "Map")?;
    }
    Ok(p as *mut u8)
}

fn odmapuj(r: &Com) {
    // SAFETY: ID3D12Resource::Unmap(0, null).
    unsafe {
        let f: unsafe extern "system" fn(Obj, u32, *const c_void) = metoda(r.0, RESOURCE_UNMAP);
        f(r.0, 0, null());
    }
}

fn prechod(r: &Com, pred: u32, po: u32) -> ResourceBarrier {
    ResourceBarrier {
        typ: 0,
        flags: 0,
        resource: r.0,
        subresource: 0xffff_ffff,
        state_before: pred,
        state_after: po,
    }
}

impl Zariadenie for D3d12 {
    fn popis(&self) -> &PopisZariadenia {
        &self.popis
    }

    fn spusti(
        &self,
        kern: &Kernel,
        vazby: &mut [DataVazby<'_>],
        konstanty: &[Hodnota32],
        skupiny: [u32; 3],
    ) -> Vysledok<()> {
        over_argumenty(kern, vazby, konstanty)?;
        if skupiny.iter().any(|&s| s > 65_535) {
            return Err(Chyba::Limit(format!("počet skupín {skupiny:?} je nad limit Direct3D 12 (65 535)")));
        }
        if skupiny.contains(&0) {
            return Ok(());
        }
        let mut v = self.vnutro.lock().unwrap_or_else(|e| e.into_inner());
        let (podpis, pso) = v.program(kern)?;

        // zdroje: nahrávanie, buffery na GPU, čítanie výsledkov
        let mut nahravacie = Vec::new();
        let mut gpu = Vec::new();
        let mut citacie = Vec::new();
        for (data, popis) in vazby.iter().zip(&kern.vazby) {
            let bajtov = data.bajtov() as u64;
            let nahr = v.zdroj(HEAP_UPLOAD, bajtov, 0, STATE_GENERIC_READ)?;
            let bajty = data.ako_bajty();
            let p = mapuj(&nahr)?;
            // SAFETY: mapovaná pamäť má aspoň `bajtov` bajtov.
            unsafe { std::ptr::copy_nonoverlapping(bajty.as_ptr(), p, bajty.len()) };
            odmapuj(&nahr);
            nahravacie.push(nahr);
            gpu.push(v.zdroj(HEAP_DEFAULT, bajtov, RESOURCE_FLAG_ALLOW_UAV, STATE_COMMON)?);
            citacie.push(if popis.pristup != Pristup::Citanie {
                Some(v.zdroj(HEAP_READBACK, bajtov, 0, STATE_COPY_DEST)?)
            } else {
                None
            });
        }
        let mut hodnoty: Vec<u32> = konstanty.iter().map(|h| h.bity()).collect();
        hodnoty.extend(vazby.iter().map(|d| d.dlzka() as u32));

        let (alok, zoz) = (v.alokator.0, v.zoznam.0);
        // SAFETY: metódy ID3D12CommandAllocator, ID3D12GraphicsCommandList a ID3D12CommandQueue podľa
        // d3d12.h; všetky zdroje žijú do konca funkcie (po čakaní na plot).
        unsafe {
            let reset_alok: unsafe extern "system" fn(Obj) -> HResult = metoda(alok, ALLOCATOR_RESET);
            over(reset_alok(alok), "reset alokátora")?;
            let reset: unsafe extern "system" fn(Obj, Obj, Obj) -> HResult = metoda(zoz, LIST_RESET);
            over(reset(zoz, alok, pso), "reset zoznamu")?;
            let kopiruj: unsafe extern "system" fn(Obj, Obj, u64, Obj, u64, u64) =
                metoda(zoz, LIST_COPY_BUFFER_REGION);
            let bariera: unsafe extern "system" fn(Obj, u32, *const ResourceBarrier) =
                metoda(zoz, LIST_RESOURCE_BARRIER);
            for ((n, g), data) in nahravacie.iter().zip(&gpu).zip(vazby.iter()) {
                kopiruj(zoz, g.0, 0, n.0, 0, data.bajtov() as u64);
            }
            let do_uav: Vec<ResourceBarrier> =
                gpu.iter().map(|g| prechod(g, STATE_COPY_DEST, STATE_UNORDERED_ACCESS)).collect();
            bariera(zoz, do_uav.len() as u32, do_uav.as_ptr());
            let nastav_podpis: unsafe extern "system" fn(Obj, Obj) =
                metoda(zoz, LIST_SET_COMPUTE_ROOT_SIGNATURE);
            nastav_podpis(zoz, podpis);
            let nastav_pso: unsafe extern "system" fn(Obj, Obj) = metoda(zoz, LIST_SET_PIPELINE_STATE);
            nastav_pso(zoz, pso);
            let uav: unsafe extern "system" fn(Obj, u32, u64) = metoda(zoz, LIST_SET_COMPUTE_ROOT_UAV);
            let adresa: unsafe extern "system" fn(Obj) -> u64 =
                metoda(gpu[0].0, RESOURCE_GET_GPU_VIRTUAL_ADDRESS);
            for (i, g) in gpu.iter().enumerate() {
                uav(zoz, i as u32, adresa(g.0));
            }
            let konst: unsafe extern "system" fn(Obj, u32, u32, *const c_void, u32) =
                metoda(zoz, LIST_SET_COMPUTE_ROOT_32BIT_CONSTANTS);
            konst(zoz, gpu.len() as u32, hodnoty.len() as u32, hodnoty.as_ptr() as *const c_void, 0);
            let dispatch: unsafe extern "system" fn(Obj, u32, u32, u32) = metoda(zoz, LIST_DISPATCH);
            dispatch(zoz, skupiny[0], skupiny[1], skupiny[2]);
            let na_citanie: Vec<ResourceBarrier> = gpu
                .iter()
                .zip(&citacie)
                .filter(|(_, c)| c.is_some())
                .map(|(g, _)| prechod(g, STATE_UNORDERED_ACCESS, STATE_COPY_SOURCE))
                .collect();
            if !na_citanie.is_empty() {
                bariera(zoz, na_citanie.len() as u32, na_citanie.as_ptr());
            }
            for ((g, c), data) in gpu.iter().zip(&citacie).zip(vazby.iter()) {
                if let Some(c) = c {
                    kopiruj(zoz, c.0, 0, g.0, 0, data.bajtov() as u64);
                }
            }
            let close: unsafe extern "system" fn(Obj) -> HResult = metoda(zoz, LIST_CLOSE);
            over(close(zoz), "Close")?;
            let vykonaj: unsafe extern "system" fn(Obj, u32, *const Obj) =
                metoda(v.fronta.0, QUEUE_EXECUTE_COMMAND_LISTS);
            vykonaj(v.fronta.0, 1, &zoz);
        }
        if let Err(e) = v.cakaj() {
            return Err(Chyba::Vypocet(format!("{e}{}", v.dovod_straty())));
        }
        for (c, data) in citacie.iter().zip(vazby.iter_mut()) {
            if let Some(c) = c {
                let p = mapuj(c)?;
                let mut b = vec![0u8; data.bajtov()];
                // SAFETY: mapovaná pamäť na čítanie má aspoň `bajtov` bajtov a GPU skončilo (plot).
                unsafe { std::ptr::copy_nonoverlapping(p as *const u8, b.as_mut_ptr(), b.len()) };
                odmapuj(c);
                data.nastav_z_bajtov(&b)?;
            }
        }
        Ok(())
    }
}

#[cfg(all(test, target_pointer_width = "64"))]
mod testy {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Rozloženie štruktúr musí presne zodpovedať hlavičkám d3d12.h a dxgi.h (64-bitové systémy).
    #[test]
    fn rozlozenie_struktur_zodpoveda_hlavickam() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(size_of::<AdapterDesc1>(), 312);
        assert_eq!(offset_of!(AdapterDesc1, vendor_id), 256);
        assert_eq!(offset_of!(AdapterDesc1, dedicated_video_memory), 272);
        assert_eq!(offset_of!(AdapterDesc1, flags), 304);
        assert_eq!(size_of::<CommandQueueDesc>(), 16);
        assert_eq!(size_of::<RootParameter>(), 32);
        assert_eq!(offset_of!(RootParameter, unia), 8);
        assert_eq!(offset_of!(RootParameter, viditelnost), 24);
        assert_eq!(size_of::<RootSignatureDesc>(), 40);
        assert_eq!(size_of::<ComputePipelineStateDesc>(), 56);
        assert_eq!(offset_of!(ComputePipelineStateDesc, node_mask), 24);
        assert_eq!(offset_of!(ComputePipelineStateDesc, flags), 48);
        assert_eq!(size_of::<HeapProperties>(), 20);
        assert_eq!(size_of::<ResourceDesc>(), 56);
        assert_eq!(offset_of!(ResourceDesc, width), 16);
        assert_eq!(offset_of!(ResourceDesc, format), 32);
        assert_eq!(offset_of!(ResourceDesc, flags), 48);
        assert_eq!(size_of::<ResourceBarrier>(), 32);
        assert_eq!(offset_of!(ResourceBarrier, state_after), 24);
    }
}
