//! Zariadenie Vulkan cez vlastné väzby (bez cudzích knižníc).
//!
//! Knižnica Vulkanu sa načíta za behu. Pre každú grafickú kartu (aj softvérový ovládač lavapipe) s frontou
//! pre výpočty vznikne jedno zariadenie. Kernel sa preloží vlastným prekladačom do SPIR-V; skompilované
//! programy sa pamätajú podľa odtlačku kernelu.
//!
//! Nebezpečný kód je len tu: volania ovládača cez ukazovatele na funkcie. Každá štruktúra zodpovedá
//! hlavičkám Vulkanu 1.1 a každé volanie má kontrolu výsledku.

#![allow(unsafe_code)]

use crate::ir::{Kernel, Pristup};
use crate::kniznica::Kniznica;
use crate::spirv;
use crate::zariadenie::{
    DataVazby, Druh, Hodnota32, PopisZariadenia, TypZariadenia, Zariadenie, over_argumenty,
};
use orchidea_jadro::{Chyba, Odtlacok, Vysledok};
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

type Disp = *mut c_void; // dispečerská rukoväť (VkInstance, VkDevice, …)
type Nd = u64; // nedispečerská rukoväť (VkBuffer, VkPipeline, …)
type VkResult = i32;

const API_1_1: u32 = (1 << 22) | (1 << 12);

// sType
const ST_APPLICATION_INFO: u32 = 0;
const ST_INSTANCE_CREATE_INFO: u32 = 1;
const ST_DEVICE_QUEUE_CREATE_INFO: u32 = 2;
const ST_DEVICE_CREATE_INFO: u32 = 3;
const ST_SUBMIT_INFO: u32 = 4;
const ST_MEMORY_ALLOCATE_INFO: u32 = 5;
const ST_FENCE_CREATE_INFO: u32 = 8;
const ST_BUFFER_CREATE_INFO: u32 = 12;
const ST_SHADER_MODULE_CREATE_INFO: u32 = 16;
const ST_PIPELINE_SHADER_STAGE_CREATE_INFO: u32 = 18;
const ST_COMPUTE_PIPELINE_CREATE_INFO: u32 = 29;
const ST_PIPELINE_LAYOUT_CREATE_INFO: u32 = 30;
const ST_DESCRIPTOR_SET_LAYOUT_CREATE_INFO: u32 = 32;
const ST_DESCRIPTOR_POOL_CREATE_INFO: u32 = 33;
const ST_DESCRIPTOR_SET_ALLOCATE_INFO: u32 = 34;
const ST_WRITE_DESCRIPTOR_SET: u32 = 35;
const ST_COMMAND_POOL_CREATE_INFO: u32 = 39;
const ST_COMMAND_BUFFER_ALLOCATE_INFO: u32 = 40;
const ST_COMMAND_BUFFER_BEGIN_INFO: u32 = 42;
const ST_MEMORY_BARRIER: u32 = 46;

const QUEUE_COMPUTE: u32 = 0x2;
const BUFFER_USAGE_STORAGE: u32 = 0x20;
const MEM_DEVICE_LOCAL: u32 = 0x1;
const MEM_HOST_VISIBLE: u32 = 0x2;
const MEM_HOST_COHERENT: u32 = 0x4;
const DESCRIPTOR_STORAGE_BUFFER: u32 = 7;
const STAGE_COMPUTE: u32 = 0x20;
const BIND_POINT_COMPUTE: u32 = 1;
const PIPELINE_STAGE_COMPUTE: u32 = 0x800;
const PIPELINE_STAGE_HOST: u32 = 0x4000;
const ACCESS_SHADER_WRITE: u32 = 0x40;
const ACCESS_HOST_READ: u32 = 0x2000;

#[repr(C)]
struct ApplicationInfo {
    s_type: u32,
    p_next: *const c_void,
    p_application_name: *const c_char,
    application_version: u32,
    p_engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    p_application_info: *const ApplicationInfo,
    enabled_layer_count: u32,
    pp_enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    pp_enabled_extension_names: *const *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct QueueFamilyProperties {
    queue_flags: u32,
    queue_count: u32,
    timestamp_valid_bits: u32,
    min_image_transfer_granularity: [u32; 3],
}

#[repr(C)]
struct DeviceQueueCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_family_index: u32,
    queue_count: u32,
    p_queue_priorities: *const f32,
}

#[repr(C)]
struct DeviceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_create_info_count: u32,
    p_queue_create_infos: *const DeviceQueueCreateInfo,
    enabled_layer_count: u32,
    pp_enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    pp_enabled_extension_names: *const *const c_char,
    p_enabled_features: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryType {
    property_flags: u32,
    heap_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryHeap {
    size: u64,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryProperties {
    memory_type_count: u32,
    memory_types: [MemoryType; 32],
    memory_heap_count: u32,
    memory_heaps: [MemoryHeap; 16],
}

#[repr(C)]
struct BufferCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    size: u64,
    usage: u32,
    sharing_mode: u32,
    queue_family_index_count: u32,
    p_queue_family_indices: *const u32,
}

#[repr(C)]
#[derive(Default)]
struct MemoryRequirements {
    size: u64,
    alignment: u64,
    memory_type_bits: u32,
}

#[repr(C)]
struct MemoryAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    allocation_size: u64,
    memory_type_index: u32,
}

#[repr(C)]
struct ShaderModuleCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    code_size: usize,
    p_code: *const u32,
}

#[repr(C)]
struct DescriptorSetLayoutBinding {
    binding: u32,
    descriptor_type: u32,
    descriptor_count: u32,
    stage_flags: u32,
    p_immutable_samplers: *const Nd,
}

#[repr(C)]
struct DescriptorSetLayoutCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    binding_count: u32,
    p_bindings: *const DescriptorSetLayoutBinding,
}

#[repr(C)]
struct PushConstantRange {
    stage_flags: u32,
    offset: u32,
    size: u32,
}

#[repr(C)]
struct PipelineLayoutCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    set_layout_count: u32,
    p_set_layouts: *const Nd,
    push_constant_range_count: u32,
    p_push_constant_ranges: *const PushConstantRange,
}

#[repr(C)]
struct PipelineShaderStageCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    stage: u32,
    module: Nd,
    p_name: *const c_char,
    p_specialization_info: *const c_void,
}

#[repr(C)]
struct ComputePipelineCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    stage: PipelineShaderStageCreateInfo,
    layout: Nd,
    base_pipeline_handle: Nd,
    base_pipeline_index: i32,
}

#[repr(C)]
struct DescriptorPoolSize {
    typ: u32,
    descriptor_count: u32,
}

#[repr(C)]
struct DescriptorPoolCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    max_sets: u32,
    pool_size_count: u32,
    p_pool_sizes: *const DescriptorPoolSize,
}

#[repr(C)]
struct DescriptorSetAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    descriptor_pool: Nd,
    descriptor_set_count: u32,
    p_set_layouts: *const Nd,
}

#[repr(C)]
struct DescriptorBufferInfo {
    buffer: Nd,
    offset: u64,
    range: u64,
}

#[repr(C)]
struct WriteDescriptorSet {
    s_type: u32,
    p_next: *const c_void,
    dst_set: Nd,
    dst_binding: u32,
    dst_array_element: u32,
    descriptor_count: u32,
    descriptor_type: u32,
    p_image_info: *const c_void,
    p_buffer_info: *const DescriptorBufferInfo,
    p_texel_buffer_view: *const Nd,
}

#[repr(C)]
struct CommandPoolCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_family_index: u32,
}

#[repr(C)]
struct CommandBufferAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    command_pool: Nd,
    level: u32,
    command_buffer_count: u32,
}

#[repr(C)]
struct CommandBufferBeginInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    p_inheritance_info: *const c_void,
}

#[repr(C)]
struct MemoryBarrier {
    s_type: u32,
    p_next: *const c_void,
    src_access_mask: u32,
    dst_access_mask: u32,
}

#[repr(C)]
struct SubmitInfo {
    s_type: u32,
    p_next: *const c_void,
    wait_semaphore_count: u32,
    p_wait_semaphores: *const Nd,
    p_wait_dst_stage_mask: *const u32,
    command_buffer_count: u32,
    p_command_buffers: *const Disp,
    signal_semaphore_count: u32,
    p_signal_semaphores: *const Nd,
}

#[repr(C)]
struct FenceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
}

type GetProc = unsafe extern "system" fn(Disp, *const c_char) -> *const c_void;

/// Načíta ukazovateľ na funkciu a pretypuje ho.
///
/// SAFETY: volajúci zaručí, že `T` je typ ukazovateľa na funkciu zodpovedajúci deklarácii vo Vulkane.
unsafe fn funkcia<T: Copy>(get: GetProc, rukovat: Disp, meno: &CStr) -> Vysledok<T> {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*const c_void>());
    // SAFETY: platný reťazec; rukoväť je platná alebo null pre globálne funkcie.
    let p = unsafe { get(rukovat, meno.as_ptr()) };
    if p.is_null() {
        return Err(Chyba::Chybajuce(format!("funkcia Vulkanu {}", meno.to_string_lossy())));
    }
    // SAFETY: veľkosť je overená, typ zaručuje volajúci.
    Ok(unsafe { std::mem::transmute_copy::<*const c_void, T>(&p) })
}

macro_rules! sada_funkcii {
    ($sada:ident, $($meno:ident : fn($($arg:ty),*) $(-> $nav:ty)?),* $(,)?) => {
        #[allow(non_snake_case)]
        struct $sada { $($meno: unsafe extern "system" fn($($arg),*) $(-> $nav)?,)* }
        impl $sada {
            /// SAFETY: `get` musí byť platná funkcia Vulkanu na získanie adries pre `rukovat`.
            unsafe fn nacitaj(get: GetProc, rukovat: Disp) -> Vysledok<Self> {
                Ok($sada { $($meno: unsafe {
                    funkcia(get, rukovat, &std::ffi::CString::new(stringify!($meno)).expect("meno"))?
                },)* })
            }
        }
    };
}

sada_funkcii!(FunkcieInstancie,
    vkDestroyInstance: fn(Disp, *const c_void),
    vkEnumeratePhysicalDevices: fn(Disp, *mut u32, *mut Disp) -> VkResult,
    vkGetPhysicalDeviceProperties: fn(Disp, *mut u8),
    vkGetPhysicalDeviceQueueFamilyProperties: fn(Disp, *mut u32, *mut QueueFamilyProperties),
    vkGetPhysicalDeviceMemoryProperties: fn(Disp, *mut MemoryProperties),
    vkCreateDevice: fn(Disp, *const DeviceCreateInfo, *const c_void, *mut Disp) -> VkResult,
    vkGetDeviceProcAddr: fn(Disp, *const c_char) -> *const c_void,
);

sada_funkcii!(FunkcieZariadenia,
    vkDestroyDevice: fn(Disp, *const c_void),
    vkGetDeviceQueue: fn(Disp, u32, u32, *mut Disp),
    vkDeviceWaitIdle: fn(Disp) -> VkResult,
    vkCreateBuffer: fn(Disp, *const BufferCreateInfo, *const c_void, *mut Nd) -> VkResult,
    vkDestroyBuffer: fn(Disp, Nd, *const c_void),
    vkGetBufferMemoryRequirements: fn(Disp, Nd, *mut MemoryRequirements),
    vkAllocateMemory: fn(Disp, *const MemoryAllocateInfo, *const c_void, *mut Nd) -> VkResult,
    vkFreeMemory: fn(Disp, Nd, *const c_void),
    vkBindBufferMemory: fn(Disp, Nd, Nd, u64) -> VkResult,
    vkMapMemory: fn(Disp, Nd, u64, u64, u32, *mut *mut c_void) -> VkResult,
    vkUnmapMemory: fn(Disp, Nd),
    vkCreateShaderModule: fn(Disp, *const ShaderModuleCreateInfo, *const c_void, *mut Nd) -> VkResult,
    vkDestroyShaderModule: fn(Disp, Nd, *const c_void),
    vkCreateDescriptorSetLayout: fn(Disp, *const DescriptorSetLayoutCreateInfo, *const c_void, *mut Nd) -> VkResult,
    vkDestroyDescriptorSetLayout: fn(Disp, Nd, *const c_void),
    vkCreatePipelineLayout: fn(Disp, *const PipelineLayoutCreateInfo, *const c_void, *mut Nd) -> VkResult,
    vkDestroyPipelineLayout: fn(Disp, Nd, *const c_void),
    vkCreateComputePipelines: fn(Disp, Nd, u32, *const ComputePipelineCreateInfo, *const c_void, *mut Nd) -> VkResult,
    vkDestroyPipeline: fn(Disp, Nd, *const c_void),
    vkCreateDescriptorPool: fn(Disp, *const DescriptorPoolCreateInfo, *const c_void, *mut Nd) -> VkResult,
    vkDestroyDescriptorPool: fn(Disp, Nd, *const c_void),
    vkAllocateDescriptorSets: fn(Disp, *const DescriptorSetAllocateInfo, *mut Nd) -> VkResult,
    vkUpdateDescriptorSets: fn(Disp, u32, *const WriteDescriptorSet, u32, *const c_void),
    vkCreateCommandPool: fn(Disp, *const CommandPoolCreateInfo, *const c_void, *mut Nd) -> VkResult,
    vkDestroyCommandPool: fn(Disp, Nd, *const c_void),
    vkAllocateCommandBuffers: fn(Disp, *const CommandBufferAllocateInfo, *mut Disp) -> VkResult,
    vkResetCommandBuffer: fn(Disp, u32) -> VkResult,
    vkBeginCommandBuffer: fn(Disp, *const CommandBufferBeginInfo) -> VkResult,
    vkEndCommandBuffer: fn(Disp) -> VkResult,
    vkCmdBindPipeline: fn(Disp, u32, Nd),
    vkCmdBindDescriptorSets: fn(Disp, u32, Nd, u32, u32, *const Nd, u32, *const u32),
    vkCmdPushConstants: fn(Disp, Nd, u32, u32, u32, *const c_void),
    vkCmdDispatch: fn(Disp, u32, u32, u32),
    vkCmdPipelineBarrier: fn(Disp, u32, u32, u32, u32, *const MemoryBarrier, u32, *const c_void, u32, *const c_void),
    vkQueueSubmit: fn(Disp, u32, *const SubmitInfo, Nd) -> VkResult,
    vkCreateFence: fn(Disp, *const FenceCreateInfo, *const c_void, *mut Nd) -> VkResult,
    vkDestroyFence: fn(Disp, Nd, *const c_void),
    vkWaitForFences: fn(Disp, u32, *const Nd, u32, u64) -> VkResult,
    vkResetFences: fn(Disp, u32, *const Nd) -> VkResult,
);

/// Podrobná stopa krokov pre hľadanie chýb ovládačov (premenná prostredia ORCHIDEA_GPU_STOPA).
fn stopa(krok: &str) {
    static ZAPNUTA: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ZAPNUTA.get_or_init(|| std::env::var("ORCHIDEA_GPU_STOPA").is_ok_and(|v| !v.is_empty())) {
        eprintln!("[vulkan] {krok}");
    }
}

fn over(vysledok: VkResult, co: &str) -> Vysledok<()> {
    stopa(co);
    if vysledok == 0 {
        Ok(())
    } else {
        let popis = match vysledok {
            1 => "nie je pripravené",
            2 => "vypršal čas",
            -1 => "nedostatok pamäte počítača",
            -2 => "nedostatok pamäte zariadenia",
            -3 => "inicializácia zlyhala",
            -4 => "zariadenie sa stratilo",
            -7 => "chýba rozšírenie",
            -9 => "nekompatibilný ovládač",
            _ => "chyba ovládača",
        };
        Err(Chyba::Vypocet(format!("Vulkan {co}: {popis} ({vysledok})")))
    }
}

/// Inštancia Vulkanu zdieľaná všetkými zariadeniami.
struct Instancia {
    kniznica: Kniznica,
    rukovat: Disp,
    f: FunkcieInstancie,
}

// SAFETY: VkInstance sa po vytvorení len číta; zničí sa raz v Drop.
unsafe impl Send for Instancia {}
unsafe impl Sync for Instancia {}

impl Drop for Instancia {
    fn drop(&mut self) {
        // SAFETY: všetky zariadenia (držia Arc) sú už zničené.
        unsafe { (self.f.vkDestroyInstance)(self.rukovat, null()) };
    }
}

struct Program {
    modul: Nd,
    rozlozenie_sady: Nd,
    rozlozenie: Nd,
    pipeline: Nd,
}

struct Vnutro {
    instancia: Arc<Instancia>,
    f: FunkcieZariadenia,
    zariadenie: Disp,
    fronta: Disp,
    pamat: MemoryProperties,
    pool: Nd,
    prikazy: Disp,
    plot: Nd,
    programy: HashMap<Odtlacok, Program>,
    najviac_skupin: [u32; 3],
    najviac_bajtov_bufferu: u64,
}

// SAFETY: k vnútru sa pristupuje len cez Mutex (Vulkan vyžaduje vonkajšiu synchronizáciu fronty a poolov).
unsafe impl Send for Vnutro {}

impl Drop for Vnutro {
    fn drop(&mut self) {
        let f = &self.f;
        let d = self.zariadenie;
        // SAFETY: zariadenie je platné; čakáme na dokončenie práce, potom ničíme v opačnom poradí.
        unsafe {
            (f.vkDeviceWaitIdle)(d);
            for p in self.programy.values() {
                (f.vkDestroyPipeline)(d, p.pipeline, null());
                (f.vkDestroyPipelineLayout)(d, p.rozlozenie, null());
                (f.vkDestroyDescriptorSetLayout)(d, p.rozlozenie_sady, null());
                (f.vkDestroyShaderModule)(d, p.modul, null());
            }
            (f.vkDestroyFence)(d, self.plot, null());
            (f.vkDestroyCommandPool)(d, self.pool, null());
            (f.vkDestroyDevice)(d, null());
        }
        let _ = &self.instancia;
    }
}

/// Zariadenie Vulkan.
pub struct Vulkan {
    popis: PopisZariadenia,
    vnutro: Mutex<Vnutro>,
}

impl std::fmt::Debug for Vulkan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Vulkan({})", self.popis)
    }
}

fn mena_kniznic() -> &'static [&'static str] {
    if cfg!(windows) {
        &["vulkan-1.dll"]
    } else if cfg!(target_vendor = "apple") {
        &["libvulkan.1.dylib", "libMoltenVK.dylib"]
    } else if cfg!(target_os = "android") {
        &["libvulkan.so"]
    } else {
        &["libvulkan.so.1", "libvulkan.so"]
    }
}

fn u32_na(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl Vulkan {
    /// Limity zariadenia: najviac skupín v každej osi a najväčší buffer v bajtoch.
    pub fn limity(&self) -> ([u32; 3], u64) {
        let v = self.vnutro.lock().unwrap_or_else(|e| e.into_inner());
        (v.najviac_skupin, v.najviac_bajtov_bufferu)
    }

    /// Otvorí všetky zariadenia Vulkan s podporou výpočtov a Vulkanom aspoň 1.1.
    pub fn otvor_vsetky() -> Vysledok<Vec<Vulkan>> {
        let kniznica = Kniznica::otvor(mena_kniznic())?;
        let get: GetProc = {
            let p = kniznica
                .symbol(c"vkGetInstanceProcAddr")
                .ok_or_else(|| Chyba::Chybajuce("vkGetInstanceProcAddr".into()))?;
            // SAFETY: vkGetInstanceProcAddr má presne tento podpis.
            unsafe { std::mem::transmute::<*const c_void, GetProc>(p) }
        };
        type CreateInstance =
            unsafe extern "system" fn(*const InstanceCreateInfo, *const c_void, *mut Disp) -> VkResult;
        // SAFETY: globálna funkcia, podpis podľa špecifikácie.
        let create: CreateInstance = unsafe { funkcia(get, null_mut(), c"vkCreateInstance")? };
        let app = ApplicationInfo {
            s_type: ST_APPLICATION_INFO,
            p_next: null(),
            p_application_name: c"ORCHIDEA".as_ptr(),
            application_version: 1,
            p_engine_name: c"ORCHIDEA".as_ptr(),
            engine_version: 1,
            api_version: API_1_1,
        };
        let info = InstanceCreateInfo {
            s_type: ST_INSTANCE_CREATE_INFO,
            p_next: null(),
            flags: 0,
            p_application_info: &app,
            enabled_layer_count: 0,
            pp_enabled_layer_names: null(),
            enabled_extension_count: 0,
            pp_enabled_extension_names: null(),
        };
        let mut rukovat: Disp = null_mut();
        // SAFETY: štruktúry sú platné počas volania.
        over(unsafe { create(&info, null(), &mut rukovat) }, "vytvorenie inštancie")?;
        // SAFETY: inštancia je platná.
        let f = unsafe { FunkcieInstancie::nacitaj(get, rukovat)? };
        let inst = Arc::new(Instancia { kniznica, rukovat, f });

        let mut pocet = 0u32;
        // SAFETY: dvojfázové zistenie počtu a zoznamu.
        over(
            unsafe { (inst.f.vkEnumeratePhysicalDevices)(inst.rukovat, &mut pocet, null_mut()) },
            "zoznam zariadení",
        )?;
        let mut fyzicke = vec![null_mut(); pocet as usize];
        over(
            unsafe { (inst.f.vkEnumeratePhysicalDevices)(inst.rukovat, &mut pocet, fyzicke.as_mut_ptr()) },
            "zoznam zariadení",
        )?;
        fyzicke.truncate(pocet as usize);
        let mut vysledok = Vec::new();
        for f in fyzicke {
            if let Ok(z) = Self::otvor(inst.clone(), f) {
                vysledok.push(z);
            }
        }
        if vysledok.is_empty() {
            return Err(Chyba::Chybajuce("zariadenie Vulkan 1.1 s podporou výpočtov".into()));
        }
        Ok(vysledok)
    }

    fn otvor(inst: Arc<Instancia>, fyzicke: Disp) -> Vysledok<Vulkan> {
        let fi = &inst.f;
        // Vlastnosti: čítajú sa len začiatočné polia, preto stačí dostatočne veľký zarovnaný priestor.
        stopa("vlastnosti fyzického zariadenia");
        let mut vlastnosti = vec![0u64; 256];
        // SAFETY: 2 KiB je viac ako VkPhysicalDeviceProperties (824 B).
        unsafe { (fi.vkGetPhysicalDeviceProperties)(fyzicke, vlastnosti.as_mut_ptr() as *mut u8) };
        let b: Vec<u8> = vlastnosti.iter().flat_map(|x| x.to_le_bytes()).collect();
        let api = u32_na(&b, 0);
        let typ = match u32_na(&b, 16) {
            1 => TypZariadenia::Integrovane,
            2 => TypZariadenia::Diskretne,
            3 => TypZariadenia::Virtualne,
            _ => TypZariadenia::Softverove,
        };
        let meno_konci = b[20..276].iter().position(|&c| c == 0).unwrap_or(256);
        let meno = String::from_utf8_lossy(&b[20..20 + meno_konci]).to_string();
        // VkPhysicalDeviceLimits začína na 296: maxStorageBufferRange +28, maxComputeWorkGroupCount +220
        let najviac_bajtov_bufferu = u32_na(&b, 296 + 28) as u64;
        let najviac_skupin = [u32_na(&b, 296 + 220), u32_na(&b, 296 + 224), u32_na(&b, 296 + 228)];
        if api < API_1_1 {
            return Err(Chyba::Chybajuce(format!("{meno}: Vulkan 1.1")));
        }

        let mut pocet = 0u32;
        // SAFETY: dvojfázové zistenie rodín front.
        unsafe { (fi.vkGetPhysicalDeviceQueueFamilyProperties)(fyzicke, &mut pocet, null_mut()) };
        let mut rodiny = vec![QueueFamilyProperties::default(); pocet as usize];
        unsafe { (fi.vkGetPhysicalDeviceQueueFamilyProperties)(fyzicke, &mut pocet, rodiny.as_mut_ptr()) };
        let rodina = rodiny
            .iter()
            .position(|r| r.queue_flags & QUEUE_COMPUTE != 0 && r.queue_count > 0)
            .ok_or_else(|| Chyba::Chybajuce(format!("{meno}: fronta pre výpočty")))?
            as u32;

        let priorita = 1.0f32;
        let fronta_info = DeviceQueueCreateInfo {
            s_type: ST_DEVICE_QUEUE_CREATE_INFO,
            p_next: null(),
            flags: 0,
            queue_family_index: rodina,
            queue_count: 1,
            p_queue_priorities: &priorita,
        };
        let info = DeviceCreateInfo {
            s_type: ST_DEVICE_CREATE_INFO,
            p_next: null(),
            flags: 0,
            queue_create_info_count: 1,
            p_queue_create_infos: &fronta_info,
            enabled_layer_count: 0,
            pp_enabled_layer_names: null(),
            enabled_extension_count: 0,
            pp_enabled_extension_names: null(),
            p_enabled_features: null(),
        };
        let mut zariadenie: Disp = null_mut();
        // SAFETY: štruktúry sú platné počas volania.
        over(
            unsafe { (fi.vkCreateDevice)(fyzicke, &info, null(), &mut zariadenie) },
            "vytvorenie zariadenia",
        )?;
        let get_dev: GetProc = fi.vkGetDeviceProcAddr;
        // SAFETY: zariadenie je platné.
        let f = unsafe { FunkcieZariadenia::nacitaj(get_dev, zariadenie)? };
        let mut fronta: Disp = null_mut();
        unsafe { (f.vkGetDeviceQueue)(zariadenie, rodina, 0, &mut fronta) };
        stopa("vlastnosti pamäte");
        let mut pamat: MemoryProperties = unsafe { std::mem::zeroed() };
        unsafe { (fi.vkGetPhysicalDeviceMemoryProperties)(fyzicke, &mut pamat) };

        let pool_info = CommandPoolCreateInfo {
            s_type: ST_COMMAND_POOL_CREATE_INFO,
            p_next: null(),
            flags: 0x2, // RESET_COMMAND_BUFFER
            queue_family_index: rodina,
        };
        let mut pool = 0;
        over(unsafe { (f.vkCreateCommandPool)(zariadenie, &pool_info, null(), &mut pool) }, "pool príkazov")?;
        let alloc = CommandBufferAllocateInfo {
            s_type: ST_COMMAND_BUFFER_ALLOCATE_INFO,
            p_next: null(),
            command_pool: pool,
            level: 0,
            command_buffer_count: 1,
        };
        let mut prikazy: Disp = null_mut();
        over(unsafe { (f.vkAllocateCommandBuffers)(zariadenie, &alloc, &mut prikazy) }, "buffer príkazov")?;
        let plot_info = FenceCreateInfo { s_type: ST_FENCE_CREATE_INFO, p_next: null(), flags: 0 };
        let mut plot = 0;
        over(unsafe { (f.vkCreateFence)(zariadenie, &plot_info, null(), &mut plot) }, "plot")?;

        let ovladac = format!(
            "Vulkan {}.{}.{} ({})",
            api >> 22 & 0x7f,
            api >> 12 & 0x3ff,
            api & 0xfff,
            inst.kniznica.meno
        );
        Ok(Vulkan {
            popis: PopisZariadenia { druh: Druh::Vulkan, typ, meno, ovladac },
            vnutro: Mutex::new(Vnutro {
                instancia: inst,
                f,
                zariadenie,
                fronta,
                pamat,
                pool,
                prikazy,
                plot,
                programy: HashMap::new(),
                najviac_skupin,
                najviac_bajtov_bufferu,
            }),
        })
    }
}

impl Vnutro {
    fn program(&mut self, k: &Kernel) -> Vysledok<&Program> {
        let kluc = k.odtlacok();
        if !self.programy.contains_key(&kluc) {
            let p = self.vytvor_program(k)?;
            self.programy.insert(kluc, p);
        }
        Ok(&self.programy[&kluc])
    }

    fn vytvor_program(&self, k: &Kernel) -> Vysledok<Program> {
        let (f, d) = (&self.f, self.zariadenie);
        let kod = spirv::do_spirv(k)?;
        let modul_info = ShaderModuleCreateInfo {
            s_type: ST_SHADER_MODULE_CREATE_INFO,
            p_next: null(),
            flags: 0,
            code_size: kod.len() * 4,
            p_code: kod.as_ptr(),
        };
        let mut modul = 0;
        // SAFETY: kód je platný SPIR-V (overený validátorom v CI), štruktúry žijú počas volaní.
        over(unsafe { (f.vkCreateShaderModule)(d, &modul_info, null(), &mut modul) }, "modul shadera")?;
        let vazby: Vec<DescriptorSetLayoutBinding> = (0..k.vazby.len())
            .map(|i| DescriptorSetLayoutBinding {
                binding: i as u32,
                descriptor_type: DESCRIPTOR_STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: STAGE_COMPUTE,
                p_immutable_samplers: null(),
            })
            .collect();
        let sada_info = DescriptorSetLayoutCreateInfo {
            s_type: ST_DESCRIPTOR_SET_LAYOUT_CREATE_INFO,
            p_next: null(),
            flags: 0,
            binding_count: vazby.len() as u32,
            p_bindings: vazby.as_ptr(),
        };
        let mut rozlozenie_sady = 0;
        over(
            unsafe { (f.vkCreateDescriptorSetLayout)(d, &sada_info, null(), &mut rozlozenie_sady) },
            "rozloženie sady",
        )?;
        let rozsah =
            PushConstantRange { stage_flags: STAGE_COMPUTE, offset: 0, size: 4 * k.konstanty.len() as u32 };
        let rozlozenie_info = PipelineLayoutCreateInfo {
            s_type: ST_PIPELINE_LAYOUT_CREATE_INFO,
            p_next: null(),
            flags: 0,
            set_layout_count: 1,
            p_set_layouts: &rozlozenie_sady,
            push_constant_range_count: if k.konstanty.is_empty() { 0 } else { 1 },
            p_push_constant_ranges: &rozsah,
        };
        let mut rozlozenie = 0;
        over(
            unsafe { (f.vkCreatePipelineLayout)(d, &rozlozenie_info, null(), &mut rozlozenie) },
            "rozloženie pipeline",
        )?;
        let info = ComputePipelineCreateInfo {
            s_type: ST_COMPUTE_PIPELINE_CREATE_INFO,
            p_next: null(),
            flags: 0,
            stage: PipelineShaderStageCreateInfo {
                s_type: ST_PIPELINE_SHADER_STAGE_CREATE_INFO,
                p_next: null(),
                flags: 0,
                stage: STAGE_COMPUTE,
                module: modul,
                p_name: c"main".as_ptr(),
                p_specialization_info: null(),
            },
            layout: rozlozenie,
            base_pipeline_handle: 0,
            base_pipeline_index: -1,
        };
        let mut pipeline = 0;
        over(
            unsafe { (f.vkCreateComputePipelines)(d, 0, 1, &info, null(), &mut pipeline) },
            "výpočtová pipeline",
        )?;
        Ok(Program { modul, rozlozenie_sady, rozlozenie, pipeline })
    }

    fn typ_pamate(&self, bity: u32) -> Option<u32> {
        let pozadovane = MEM_HOST_VISIBLE | MEM_HOST_COHERENT;
        let typy = &self.pamat.memory_types[..self.pamat.memory_type_count.min(32) as usize];
        let vhodny = |i: usize, extra: u32| {
            bity & (1 << i) != 0 && typy[i].property_flags & (pozadovane | extra) == pozadovane | extra
        };
        (0..typy.len())
            .find(|&i| vhodny(i, MEM_DEVICE_LOCAL))
            .or_else(|| (0..typy.len()).find(|&i| vhodny(i, 0)))
            .map(|i| i as u32)
    }
}

/// Buffer s pamäťou viditeľnou procesorom; uvoľní sa pri zániku.
struct Buffer<'a> {
    v: &'a Vnutro,
    buffer: Nd,
    pamat: Nd,
    mapa: *mut c_void,
}

impl Drop for Buffer<'_> {
    fn drop(&mut self) {
        let (f, d) = (&self.v.f, self.v.zariadenie);
        // SAFETY: objekty patria tomuto bufferu a GPU s nimi už nepracuje (čakalo sa na plot).
        unsafe {
            if !self.mapa.is_null() {
                (f.vkUnmapMemory)(d, self.pamat);
            }
            if self.buffer != 0 {
                (f.vkDestroyBuffer)(d, self.buffer, null());
            }
            if self.pamat != 0 {
                (f.vkFreeMemory)(d, self.pamat, null());
            }
        }
    }
}

impl<'a> Buffer<'a> {
    fn novy(v: &'a Vnutro, data: &DataVazby<'_>) -> Vysledok<Buffer<'a>> {
        let (f, d) = (&v.f, v.zariadenie);
        let bajtov = data.bajtov() as u64;
        if bajtov > v.najviac_bajtov_bufferu {
            return Err(Chyba::Limit(format!(
                "buffer {bajtov} B je väčší ako limit zariadenia {} B",
                v.najviac_bajtov_bufferu
            )));
        }
        let mut b = Buffer { v, buffer: 0, pamat: 0, mapa: null_mut() };
        let info = BufferCreateInfo {
            s_type: ST_BUFFER_CREATE_INFO,
            p_next: null(),
            flags: 0,
            size: bajtov,
            usage: BUFFER_USAGE_STORAGE,
            sharing_mode: 0,
            queue_family_index_count: 0,
            p_queue_family_indices: null(),
        };
        // SAFETY: platné štruktúry; výsledky sa kontrolujú a pri chybe Drop uvoľní, čo vzniklo.
        over(unsafe { (f.vkCreateBuffer)(d, &info, null(), &mut b.buffer) }, "buffer")?;
        let mut poziadavky = MemoryRequirements::default();
        unsafe { (f.vkGetBufferMemoryRequirements)(d, b.buffer, &mut poziadavky) };
        let typ = v
            .typ_pamate(poziadavky.memory_type_bits)
            .ok_or_else(|| Chyba::Chybajuce("pamäť viditeľná procesorom".into()))?;
        let alloc = MemoryAllocateInfo {
            s_type: ST_MEMORY_ALLOCATE_INFO,
            p_next: null(),
            allocation_size: poziadavky.size,
            memory_type_index: typ,
        };
        over(unsafe { (f.vkAllocateMemory)(d, &alloc, null(), &mut b.pamat) }, "pamäť")?;
        over(unsafe { (f.vkBindBufferMemory)(d, b.buffer, b.pamat, 0) }, "pripojenie pamäte")?;
        over(unsafe { (f.vkMapMemory)(d, b.pamat, 0, bajtov, 0, &mut b.mapa) }, "mapovanie pamäte")?;
        stopa("kopírovanie dát do bufferu");
        let zdroj = data.ako_bajty();
        // SAFETY: mapa ukazuje na aspoň `bajtov` bajtov súvislej pamäte viditeľnej procesorom.
        unsafe { std::ptr::copy_nonoverlapping(zdroj.as_ptr(), b.mapa as *mut u8, zdroj.len()) };
        Ok(b)
    }

    fn citaj(&self, data: &mut DataVazby<'_>) -> Vysledok<()> {
        stopa("čítanie výsledku");
        let mut b = vec![0u8; data.bajtov()];
        // SAFETY: mapa je platná a koherentná; GPU skončilo (plot) a bariéra sprístupnila zápisy procesoru.
        unsafe { std::ptr::copy_nonoverlapping(self.mapa as *const u8, b.as_mut_ptr(), b.len()) };
        data.nastav_z_bajtov(&b)
    }
}

impl Zariadenie for Vulkan {
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
        let mut v = self.vnutro.lock().unwrap_or_else(|e| e.into_inner());
        if skupiny.iter().zip(v.najviac_skupin).any(|(&s, m)| s > m) {
            return Err(Chyba::Limit(format!(
                "počet skupín {skupiny:?} je nad limit zariadenia {:?}",
                v.najviac_skupin
            )));
        }
        if skupiny.contains(&0) {
            return Ok(());
        }
        let (pipeline, rozlozenie, rozlozenie_sady) = {
            let p = v.program(k)?;
            (p.pipeline, p.rozlozenie, p.rozlozenie_sady)
        };
        let v = &*v;
        let (f, d) = (&v.f, v.zariadenie);
        let buffery: Vec<Buffer<'_>> =
            vazby.iter().map(|data| Buffer::novy(v, data)).collect::<Vysledok<_>>()?;

        // sada deskriptorov
        let velkost =
            DescriptorPoolSize { typ: DESCRIPTOR_STORAGE_BUFFER, descriptor_count: buffery.len() as u32 };
        let pool_info = DescriptorPoolCreateInfo {
            s_type: ST_DESCRIPTOR_POOL_CREATE_INFO,
            p_next: null(),
            flags: 0,
            max_sets: 1,
            pool_size_count: 1,
            p_pool_sizes: &velkost,
        };
        let mut pool = 0;
        // SAFETY: platné štruktúry; pool sa zničí na konci funkcie.
        over(unsafe { (f.vkCreateDescriptorPool)(d, &pool_info, null(), &mut pool) }, "pool deskriptorov")?;
        struct ZnicPool<'a>(&'a Vnutro, Nd);
        impl Drop for ZnicPool<'_> {
            fn drop(&mut self) {
                // SAFETY: GPU s poolom už nepracuje.
                unsafe { (self.0.f.vkDestroyDescriptorPool)(self.0.zariadenie, self.1, null()) };
            }
        }
        let _strazca = ZnicPool(v, pool);
        let alloc = DescriptorSetAllocateInfo {
            s_type: ST_DESCRIPTOR_SET_ALLOCATE_INFO,
            p_next: null(),
            descriptor_pool: pool,
            descriptor_set_count: 1,
            p_set_layouts: &rozlozenie_sady,
        };
        let mut sada = 0;
        over(unsafe { (f.vkAllocateDescriptorSets)(d, &alloc, &mut sada) }, "sada deskriptorov")?;
        let info_bufferov: Vec<DescriptorBufferInfo> = buffery
            .iter()
            .map(|b| DescriptorBufferInfo { buffer: b.buffer, offset: 0, range: u64::MAX })
            .collect();
        let zapisy: Vec<WriteDescriptorSet> = info_bufferov
            .iter()
            .enumerate()
            .map(|(i, info)| WriteDescriptorSet {
                s_type: ST_WRITE_DESCRIPTOR_SET,
                p_next: null(),
                dst_set: sada,
                dst_binding: i as u32,
                dst_array_element: 0,
                descriptor_count: 1,
                descriptor_type: DESCRIPTOR_STORAGE_BUFFER,
                p_image_info: null(),
                p_buffer_info: info,
                p_texel_buffer_view: null(),
            })
            .collect();
        stopa("aktualizácia sady deskriptorov");
        unsafe { (f.vkUpdateDescriptorSets)(d, zapisy.len() as u32, zapisy.as_ptr(), 0, null()) };

        // záznam príkazov
        let cb = v.prikazy;
        let zaciatok = CommandBufferBeginInfo {
            s_type: ST_COMMAND_BUFFER_BEGIN_INFO,
            p_next: null(),
            flags: 1, // ONE_TIME_SUBMIT
            p_inheritance_info: null(),
        };
        let bity: Vec<u32> = konstanty.iter().map(|h| h.bity()).collect();
        let bariera = MemoryBarrier {
            s_type: ST_MEMORY_BARRIER,
            p_next: null(),
            src_access_mask: ACCESS_SHADER_WRITE,
            dst_access_mask: ACCESS_HOST_READ,
        };
        // SAFETY: buffer príkazov patrí tomuto zariadeniu a je chránený zámkom.
        unsafe {
            over((f.vkResetCommandBuffer)(cb, 0), "reset príkazov")?;
            over((f.vkBeginCommandBuffer)(cb, &zaciatok), "začiatok príkazov")?;
            stopa("záznam: pipeline, sada, konštanty, dispatch, bariéra");
            (f.vkCmdBindPipeline)(cb, BIND_POINT_COMPUTE, pipeline);
            (f.vkCmdBindDescriptorSets)(cb, BIND_POINT_COMPUTE, rozlozenie, 0, 1, &sada, 0, null());
            if !bity.is_empty() {
                (f.vkCmdPushConstants)(
                    cb,
                    rozlozenie,
                    STAGE_COMPUTE,
                    0,
                    4 * bity.len() as u32,
                    bity.as_ptr() as *const c_void,
                );
            }
            (f.vkCmdDispatch)(cb, skupiny[0], skupiny[1], skupiny[2]);
            (f.vkCmdPipelineBarrier)(
                cb,
                PIPELINE_STAGE_COMPUTE,
                PIPELINE_STAGE_HOST,
                0,
                1,
                &bariera,
                0,
                null(),
                0,
                null(),
            );
            over((f.vkEndCommandBuffer)(cb), "koniec príkazov")?;
            let odoslanie = SubmitInfo {
                s_type: ST_SUBMIT_INFO,
                p_next: null(),
                wait_semaphore_count: 0,
                p_wait_semaphores: null(),
                p_wait_dst_stage_mask: null(),
                command_buffer_count: 1,
                p_command_buffers: &cb,
                signal_semaphore_count: 0,
                p_signal_semaphores: null(),
            };
            over((f.vkResetFences)(d, 1, &v.plot), "reset plota")?;
            over((f.vkQueueSubmit)(v.fronta, 1, &odoslanie, v.plot), "odoslanie")?;
            over((f.vkWaitForFences)(d, 1, &v.plot, 1, 120_000_000_000), "čakanie na výsledok")?;
        }
        for ((b, data), popis) in buffery.iter().zip(vazby.iter_mut()).zip(&k.vazby) {
            if popis.pristup != Pristup::Citanie {
                b.citaj(data)?;
            }
        }
        Ok(())
    }
}
