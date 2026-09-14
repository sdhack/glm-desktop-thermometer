//! 传感器采集库。
//! M1 范围：CPU 使用率（PDH）、内存（GlobalMemoryStatusEx）、
//! GPU/显存占用（PDH GPU 计数器，免厂商 SDK）、GPU 温度（NVAPI 直连 FFI）。
//! 所有读不到的数据保持 Option::None，由 UI 层决定隐藏（无数据不占位）。

#![allow(non_snake_case)]

use std::mem::{size_of, zeroed};

use windows::core::{w, PCSTR, PCWSTR};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_FMT,
    PDH_HCOUNTER, PDH_HQUERY,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    pub cpu_usage: f32,
    pub mem_used_gb: f32,
    pub mem_total_gb: f32,
    pub mem_used_pct: f32,
    pub gpu_usage: Option<f32>,
    pub gpu_vram_used_gb: Option<f32>,
    pub gpu_vram_pct: Option<f32>,
    pub gpu_temp: Option<f32>,
    pub disk_temps: Vec<f32>, // 最多两块盘
    pub cpu_temp: Option<f32>,
    pub fans: Vec<f32>,       // 最多三个风扇转速
}

pub struct SensorHub {
    cpu: Option<(PDH_HQUERY, PDH_HCOUNTER)>,
    gpu_util: Option<(PDH_HQUERY, PDH_HCOUNTER)>,
    gpu_vram: Option<(PDH_HQUERY, PDH_HCOUNTER)>,
    vram_total_bytes: Option<u64>,
    nvml: Option<Nvml>,
    nvapi: Option<NvApi>,
    queries: Vec<PDH_HQUERY>,
}

impl Drop for SensorHub {
    fn drop(&mut self) {
        unsafe {
            for q in self.queries.drain(..) {
                PdhCloseQuery(q);
            }
        }
    }
}

impl SensorHub {
    /// 打不开的采集通道自动跳过（对应字段保持 None，UI 隐藏），绝不 panic。
    pub fn new() -> Self {
        let cpu = open_query(w!("\\Processor(_Total)\\% Processor Time"));
        let gpu_util = open_query(w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage"));
        let gpu_vram = open_query(w!("\\GPU Adapter Memory(*)\\Dedicated Usage"));
        let mut queries = Vec::new();
        for q in [&cpu, &gpu_util, &gpu_vram].into_iter().flatten() {
            queries.push(q.0);
        }
        let hub = Self {
            cpu,
            gpu_util,
            gpu_vram,
            vram_total_bytes: query_max_dedicated_vram(),
            nvml: Nvml::load(),
            // nvapi64.dll 体积大（十几 MB），懒加载：仅当 NVML 不可用时才载入
            nvapi: None,
            queries,
        };
        // PDH 速率计数器需要先采一次基准样本
        unsafe {
            for q in &hub.queries {
                PdhCollectQueryData(*q);
            }
        }
        hub
    }

    pub fn sample(&mut self) -> Snapshot {
        let mut s = Snapshot::default();

        unsafe {
            for q in &self.queries {
                PdhCollectQueryData(*q);
            }

            // CPU
            if let Some((_, c)) = self.cpu {
                if let Some(v) = wildcard_sum(c, Fmt::Double) {
                    s.cpu_usage = v.clamp(0.0, 100.0) as f32;
                }
            }

            // 内存
            let mut ms = zeroed::<MEMORYSTATUSEX>();
            ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut ms).is_ok() {
                let total = ms.ullTotalPhys as f64;
                let used = total - ms.ullAvailPhys as f64;
                s.mem_total_gb = (total / 1024.0 / 1024.0 / 1024.0) as f32;
                s.mem_used_gb = (used / 1024.0 / 1024.0 / 1024.0) as f32;
                s.mem_used_pct = if total > 0.0 {
                    (used / total * 100.0) as f32
                } else {
                    0.0
                };
            }

            // GPU 使用率（各实例 3D 引擎求和）
            if let Some((_, c)) = self.gpu_util {
                if let Some(v) = wildcard_sum(c, Fmt::Double) {
                    s.gpu_usage = Some(v.clamp(0.0, 100.0) as f32);
                }
            }

            // 显存占用（专用显存求和 / 最大适配器总显存）
            if let (Some((_, c)), Some(total)) = (self.gpu_vram, self.vram_total_bytes) {
                if let Some(used) = wildcard_sum(c, Fmt::Large) {
                    let used = used as u64;
                    s.gpu_vram_used_gb = Some((used as f64 / 1024.0 / 1024.0 / 1024.0) as f32);
                    s.gpu_vram_pct = Some((used as f64 / total as f64 * 100.0) as f32);
                }
            }
        }

        // 硬盘温度（NVMe/SMART，免驱动；读不到的盘自动隐藏）
        s.disk_temps = disk_temps();

        // CPU 温度与风扇转速：来自 lhm-bridge 子进程的解析缓存（过期视为无效）
        if let Ok(c) = LHM_CACHE.lock() {
            let fresh = c.2.is_some_and(|t| t.elapsed() < LHM_TTL);
            if fresh {
                s.cpu_temp = c.0;
                s.fans = c.1.clone();
            }
        }

        // GPU 温度：NVML 优先，NVAPI 兜底（懒加载）
        if let Some(nv) = &mut self.nvml {
            s.gpu_temp = nv.gpu_temp();
        }
        if s.gpu_temp.is_none() {
            if self.nvapi.is_none() {
                self.nvapi = NvApi::load();
            }
            if let Some(nv) = &mut self.nvapi {
                s.gpu_temp = nv.gpu_temp();
            }
        }
        s
    }
}

fn open_query(counter_path: PCWSTR) -> Option<(PDH_HQUERY, PDH_HCOUNTER)> {
    unsafe {
        let mut q = zeroed::<PDH_HQUERY>();
        if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) != 0 {
            return None;
        }
        let mut c = zeroed::<PDH_HCOUNTER>();
        if PdhAddEnglishCounterW(q, counter_path, 0, &mut c) != 0 {
            PdhCloseQuery(q);
            return None;
        }
        Some((q, c))
    }
}

enum Fmt {
    Double,
    Large,
}

/// 通配符计数器：把所有实例的值求和。
fn wildcard_sum(counter: PDH_HCOUNTER, fmt: Fmt) -> Option<f64> {
    unsafe {
        let pf: PDH_FMT = match fmt {
            Fmt::Double => PDH_FMT_DOUBLE,
            Fmt::Large => PDH_FMT_LARGE,
        };
        let mut size: u32 = 0;
        let mut count: u32 = 0;
        // 第一次调用拿需要的缓冲区大小（PDH_MORE_DATA）
        if PdhGetFormattedCounterArrayW(counter, pf, &mut size, &mut count, None) == 0 {
            return None;
        }
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if PdhGetFormattedCounterArrayW(
            counter,
            pf,
            &mut size,
            &mut count,
            Some(buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
        ) != 0
        {
            return None;
        }
        let n = count as usize;
        if n == 0 {
            return None;
        }
        let items =
            std::slice::from_raw_parts(buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, n);
        let mut sum = 0.0f64;
        for it in items {
            let v = match fmt {
                Fmt::Double => it.FmtValue.Anonymous.doubleValue,
                Fmt::Large => it.FmtValue.Anonymous.largeValue as f64,
            };
            if v.is_finite() {
                sum += v;
            }
        }
        Some(sum)
    }
}

/// 取显存最大的适配器的专用显存总容量（免驱动，DXGI 枚举）。
fn query_max_dedicated_vram() -> Option<u64> {
    unsafe {
        let factory = CreateDXGIFactory1::<IDXGIFactory1>().ok()?;
        let mut max: u64 = 0;
        for i in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(i) else { break };
            let Ok(desc) = adapter.GetDesc1() else { continue };
            max = max.max(desc.DedicatedVideoMemory as u64);
        }
        (max > 0).then_some(max)
    }
}

// ---------------------------------------------------------------------------
// NVAPI 最小 FFI：只做 Initialize → EnumPhysicalGPUs → GetThermalSettings。
// 不引入第三方 nvapi crate，函数经 nvapi_QueryInterface 按已公开 ID 获取。
// ---------------------------------------------------------------------------

const NVAPI_INITIALIZE: u32 = 0x0150E828;
const NVAPI_ENUM_PHYSICAL_GPUS: u32 = 0xE5AC921F;
const NVAPI_GPU_GET_THERMAL_SETTINGS: u32 = 0xE3640A56;

type QueryInterfaceFn = unsafe extern "system" fn(u32) -> *mut core::ffi::c_void;
type InitializeFn = unsafe extern "system" fn() -> i32;
type EnumGpusFn = unsafe extern "system" fn(*mut isize, *mut u32) -> i32;
type GetThermalFn = unsafe extern "system" fn(isize, u32, *mut NvGpuThermalSettings) -> i32;

#[repr(C)]
struct NvSensor {
    controller: u32,
    default_min: i32,
    default_max: i32,
    current_temp: i32,
    target: u32,
}

#[repr(C)]
struct NvGpuThermalSettings {
    version: u32,
    count: u32,
    sensor: [NvSensor; 32],
}

struct NvApi {
    get_thermal: GetThermalFn,
    gpus: Vec<isize>,
}

impl NvApi {
    fn load() -> Option<Self> {
        unsafe {
            // nvapi64.dll 随 NVIDIA 驱动驻留 System32，无 N 卡时加载失败 → 返回 None
            let lib = match GetModuleHandleW(w!("nvapi64.dll")) {
                Ok(h) => h,
                Err(_) => windows::Win32::System::LibraryLoader::LoadLibraryW(w!("nvapi64.dll"))
                    .ok()?,
            };
            let q: QueryInterfaceFn = std::mem::transmute(
                GetProcAddress(lib, PCSTR::from_raw(b"nvapi_QueryInterface\0".as_ptr()))?,
            );

            let init: InitializeFn = std::mem::transmute(q(NVAPI_INITIALIZE));
            let init_ret = init();
            if std::env::var("TEMPMON_DEBUG").is_ok() {
                eprintln!("[nvapi] initialize ret = {init_ret:#x}");
            }
            if init_ret != 0 {
                return None;
            }
            let enum_gpus: EnumGpusFn = std::mem::transmute(q(NVAPI_ENUM_PHYSICAL_GPUS));
            let get_thermal: GetThermalFn =
                std::mem::transmute(q(NVAPI_GPU_GET_THERMAL_SETTINGS));

            let mut handles = [0isize; 64];
            let mut count = 0u32;
            let enum_ret = enum_gpus(handles.as_mut_ptr(), &mut count);
            if std::env::var("TEMPMON_DEBUG").is_ok() {
                eprintln!("[nvapi] enum ret = {enum_ret:#x}, count = {count}");
            }
            if enum_ret != 0 || count == 0 {
                return None;
            }
            Some(Self {
                get_thermal,
                gpus: handles[..count as usize].to_vec(),
            })
        }
    }

    fn gpu_temp(&mut self) -> Option<f32> {
        unsafe {
            let mut best: Option<f32> = None;
            for &gpu in &self.gpus {
                let mut ts = zeroed::<NvGpuThermalSettings>();
                ts.version = (size_of::<NvGpuThermalSettings>() as u32) | (1 << 16);
                let tr = (self.get_thermal)(gpu, 0, &mut ts);
                if std::env::var("TEMPMON_DEBUG").is_ok() {
                    eprintln!("[nvapi] thermal ret = {tr:#x}, count = {}, t0 = {}", ts.count, ts.sensor[0].current_temp);
                }
                if tr != 0 || ts.count == 0 {
                    continue;
                }
                // 取该 GPU 上目标为 GPU 的传感器；否则取第一个
                let s = ts
                    .sensor
                    .iter()
                    .take(ts.count as usize)
                    .find(|s| s.target == 1)
                    .unwrap_or(&ts.sensor[0]);
                if s.current_temp > -20 && s.current_temp < 150 {
                    best = best
                        .map(|b: f32| b.max(s.current_temp as f32))
                        .or(Some(s.current_temp as f32));
                }
            }
            best
        }
    }
}

// ---------------------------------------------------------------------------
// NVML（NVIDIA 驱动自带 nvml.dll）：温度获取主力，纯 C API，无结构体版本问题。
// ---------------------------------------------------------------------------

const NVML_TEMPERATURE_GPU: u32 = 0;
type NvmlInitFn = unsafe extern "system" fn() -> i32;
type NvmlGetHandleFn = unsafe extern "system" fn(u32, *mut isize) -> i32;
type NvmlGetTempFn = unsafe extern "system" fn(isize, u32, *mut u32) -> i32;

struct Nvml {
    _lib: windows::Win32::Foundation::HMODULE,
    device: isize,
    get_temp: NvmlGetTempFn,
}

impl Nvml {
    fn load() -> Option<Self> {
        unsafe {
            let lib = match windows::Win32::System::LibraryLoader::GetModuleHandleW(w!("nvml.dll")) {
                Ok(h) => h,
                Err(_) => windows::Win32::System::LibraryLoader::LoadLibraryW(w!("nvml.dll")).ok()?,
            };
            let get_proc = |name: &[u8]| -> Option<*mut core::ffi::c_void> {
                Some(std::mem::transmute(windows::Win32::System::LibraryLoader::GetProcAddress(
                    lib,
                    PCSTR::from_raw(name.as_ptr()),
                )?))
            };
            let init: NvmlInitFn = std::mem::transmute(get_proc(b"nvmlInit_v2\0")?);
            let get_handle: NvmlGetHandleFn =
                std::mem::transmute(get_proc(b"nvmlDeviceGetHandleByIndex_v2\0")?);
            let get_temp: NvmlGetTempFn =
                std::mem::transmute(get_proc(b"nvmlDeviceGetTemperature\0")?);

            if init() != 0 {
                return None;
            }
            let mut device = 0isize;
            if get_handle(0, &mut device) != 0 {
                return None;
            }
            Some(Nvml { _lib: lib, device, get_temp })
        }
    }

    fn gpu_temp(&mut self) -> Option<f32> {
        unsafe {
            let mut t = 0u32;
            if (self.get_temp)(self.device, NVML_TEMPERATURE_GPU, &mut t) == 0 && t > 0 && t < 150 {
                Some(t as f32)
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 进程间共享帧：采集子进程写、UI 进程读（共享内存，零序列化开销）
// ---------------------------------------------------------------------------

pub const NUM_SLOTS: usize = 14;
pub const SLOT_CPU_PCT: usize = 0;
pub const SLOT_MEM_PCT: usize = 1;
pub const SLOT_MEM_TOTAL_GB: usize = 2;
pub const SLOT_GPU_USAGE: usize = 3;
pub const SLOT_GPU_VRAM_PCT: usize = 4;
pub const SLOT_GPU_VRAM_GB: usize = 5;
pub const SLOT_GPU_TEMP: usize = 6;
pub const SLOT_DISK1_TEMP: usize = 7;
pub const SLOT_DISK2_TEMP: usize = 8;
pub const SLOT_CPU_TEMP: usize = 10;
pub const SLOT_FAN1: usize = 11;
pub const SLOT_FAN2: usize = 12;
pub const SLOT_FAN3: usize = 13;
pub const SLOT_SEQ: usize = 9; // 每次写入 +1，父进程据此判断子进程是否存活

const FRAME_MAGIC: u32 = 0x544D5047; // "TMPG"

#[repr(C)]
#[derive(Clone, Copy)]
pub struct F32Slot {
    pub has: u32,
    pub v: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Frame {
    pub magic: u32,
    pub slots: [F32Slot; NUM_SLOTS],
}

pub fn frame_len() -> usize {
    std::mem::size_of::<Frame>()
}

/// Snapshot → 共享内存帧
pub fn write_frame(ptr: *mut u8, s: &Snapshot) {
    unsafe {
        let f = ptr as *mut Frame;
        (*f).magic = FRAME_MAGIC;
        let put = |i: usize, v: Option<f32>, seq: Option<u32>| {
            (*f).slots[i] = F32Slot {
                has: if seq.is_some() || v.is_some() { 1 } else { 0 },
                v: seq.map(|x| x as f32).or(v).unwrap_or(0.0),
            };
        };
        put(SLOT_CPU_PCT, Some(s.cpu_usage), None);
        put(SLOT_MEM_PCT, Some(s.mem_used_pct), None);
        put(SLOT_MEM_TOTAL_GB, Some(s.mem_total_gb), None);
        put(SLOT_GPU_USAGE, s.gpu_usage, None);
        put(SLOT_GPU_VRAM_PCT, s.gpu_vram_pct, None);
        put(SLOT_GPU_VRAM_GB, s.gpu_vram_used_gb, None);
        put(SLOT_GPU_TEMP, s.gpu_temp, None);
        put(SLOT_DISK1_TEMP, s.disk_temps.first().copied(), None);
        put(SLOT_DISK2_TEMP, s.disk_temps.get(1).copied(), None);
        put(SLOT_CPU_TEMP, s.cpu_temp, None);
        put(SLOT_FAN1, s.fans.first().copied(), None);
        put(SLOT_FAN2, s.fans.get(1).copied(), None);
        put(SLOT_FAN3, s.fans.get(2).copied(), None);
        // 序号以 u32 位模式存进 f32 槽：直接存 f32 会在 2^24 后无法递增，
        // 导致父进程误判子进程卡死而无限重拉
        let seq = if (*f).slots[SLOT_SEQ].has == 1 {
            f32::from_bits((*f).slots[SLOT_SEQ].v.to_bits()).to_bits().wrapping_add(1)
        } else {
            1
        };
        (*f).slots[SLOT_SEQ] = F32Slot { has: 1, v: f32::from_bits(seq) };
    }
}

/// 读帧序号（父进程判断子进程是否存活）
pub fn frame_seq(ptr: *const u8) -> u32 {
    unsafe { f32::from_bits((*(ptr as *const Frame)).slots[SLOT_SEQ].v.to_bits()).to_bits() }
}

/// 共享内存帧 → Snapshot（magic 不对时返回 None，由 UI 层兜底处理）。
/// 读写无锁：读前后各取一次序号，不一致说明采样中被写穿，丢弃本帧。
pub fn read_frame(ptr: *const u8) -> Option<Snapshot> {
    unsafe {
        if (*(ptr as *const Frame)).magic != FRAME_MAGIC {
            return None;
        }
        let seq0 = frame_seq(ptr);
        let snap = read_frame_inner(ptr)?;
        if frame_seq(ptr) != seq0 {
            return None;
        }
        Some(snap)
    }
}

fn read_frame_inner(ptr: *const u8) -> Option<Snapshot> {
    unsafe {
        let f = ptr as *const Frame;
        if (*f).magic != FRAME_MAGIC {
            return None;
        }
        let get = |i: usize| -> Option<f32> {
            ((*f).slots[i].has == 1).then_some((*f).slots[i].v)
        };
        Some(Snapshot {
            cpu_usage: get(SLOT_CPU_PCT).unwrap_or(0.0),
            mem_used_gb: 0.0,
            mem_total_gb: get(SLOT_MEM_TOTAL_GB).unwrap_or(0.0),
            mem_used_pct: get(SLOT_MEM_PCT).unwrap_or(0.0),
            gpu_usage: get(SLOT_GPU_USAGE),
            gpu_vram_used_gb: get(SLOT_GPU_VRAM_GB),
            gpu_vram_pct: get(SLOT_GPU_VRAM_PCT),
            gpu_temp: get(SLOT_GPU_TEMP),
            disk_temps: get(SLOT_DISK1_TEMP)
                .into_iter()
                .chain(get(SLOT_DISK2_TEMP))
                .collect(),
            cpu_temp: get(SLOT_CPU_TEMP),
            fans: get(SLOT_FAN1)
                .into_iter()
                .chain(get(SLOT_FAN2))
                .chain(get(SLOT_FAN3))
                .collect(),
        })
    }
}

/// 采集子进程主循环：每秒采样一帧写入共享内存
pub fn sensor_loop() {
    use windows::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, FILE_MAP_WRITE, PAGE_READWRITE,
    };
    use windows::Win32::System::Threading::Sleep;
    unsafe {
        let mapping = CreateFileMappingW(
            windows::Win32::Foundation::HANDLE::default(),
            None,
            PAGE_READWRITE,
            0,
            frame_len() as u32,
            windows::core::w!("Local\\TempmonFrame"),
        )
        .expect("create file mapping");
        let view = MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, frame_len());
        assert!(!view.Value.is_null(), "map view failed");
        let mut hub = SensorHub::new();
        spawn_disk_poller();
        spawn_lhm_bridge();
        loop {
            let snap = hub.sample();
            write_frame(view.Value as *mut u8, &snap);
            // 刷新间隔跟配置走（UI 菜单可调，写回 tempmon.conf 的 refresh= 行）
            let ms = std::fs::read_to_string(config_refresh_ms())
                .ok()
                .and_then(|t| {
                    t.lines().find_map(|l| {
                        l.strip_prefix("refresh=")
                            .and_then(|v| v.trim().parse::<u32>().ok())
                    })
                })
                .filter(|v| (250..=5000).contains(v))
                .unwrap_or(1000);
            Sleep(ms);
        }
    }
}

// ---------------------------------------------------------------------------
// 硬盘温度：IOCTL_STORAGE_QUERY_PROPERTY 温度属性（NVMe/SATA 通用，免驱动）
// ---------------------------------------------------------------------------

/// 硬盘温度缓存：由轮询线程每 60 秒刷新一次（盘温变化慢，无需高频采样）
pub fn disk_temps() -> Vec<f32> {
    DISK_CACHE.lock().map(|c| c.clone()).unwrap_or_default()
}

static DISK_CACHE: std::sync::Mutex<Vec<f32>> = std::sync::Mutex::new(Vec::new());

/// 启动盘温轮询线程：原生 IOCTL 直读（免子进程）
pub fn spawn_disk_poller() {
    std::thread::spawn(|| loop {
        if let Some(temps) = query_disk_temps() {
            if !temps.is_empty() {
                if let Ok(mut c) = DISK_CACHE.lock() {
                    *c = temps;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(60));
    });
}

/// 读取 PhysicalDrive0..15 的温度。优先零开销的原生 IOCTL（部分盘不支持），
/// 不支持时回退 PowerShell Get-StorageReliabilityCounter。返回摄氏度，最多两块盘。
fn query_disk_temps() -> Option<Vec<f32>> {
    if let Some(t) = query_disk_temps_native() {
        if !t.is_empty() {
            return Some(t);
        }
    }
    query_disk_temps_fallback()
}

fn query_disk_temps_fallback() -> Option<Vec<f32>> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW，避免闪黑框
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-PhysicalDisk | Sort-Object DeviceId | ForEach-Object { ($_ | Get-StorageReliabilityCounter).Temperature }",
        ])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let temps: Vec<f32> = text
        .lines()
        .filter_map(|l| l.trim().parse::<f32>().ok())
        .filter(|t| (0.0..=120.0).contains(t))
        .take(2)
        .collect();
    Some(temps)
}

/// 原生路径：IOCTL_STORAGE_QUERY_PROPERTY / StorageDeviceTemperatureProperty。
/// 不少 SATA/NVMe 盘对该属性返回 ERROR_NOT_SUPPORTED，此时走 PowerShell 回退。
fn query_disk_temps_native() -> Option<Vec<f32>> {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_FLAGS_AND_ATTRIBUTES, OPEN_EXISTING,
    };
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
    use windows::Win32::System::IO::DeviceIoControl;

    const STORAGE_DEVICE_TEMPERATURE_PROPERTY: u32 = 13;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct StoragePropertyQuery {
        property_id: u32,
        query_type: u32,
    }

    let mut temps: Vec<f32> = Vec::new();
    unsafe {
        for i in 0..16u32 {
            let path: Vec<u16> = format!("\\\\.\\PhysicalDrive{i}\0").encode_utf16().collect();
            let Ok(h) = CreateFileW(
                windows::core::PCWSTR::from_raw(path.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            ) else {
                continue;
            };
            let query = StoragePropertyQuery {
                property_id: STORAGE_DEVICE_TEMPERATURE_PROPERTY,
                query_type: 0,
            };
            let mut out = [0u8; 64];
            let mut ret = 0u32;
            let ok = DeviceIoControl(
                h,
                0x002D_1400, // IOCTL_STORAGE_QUERY_PROPERTY
                Some(&query as *const _ as _),
                size_of::<StoragePropertyQuery>() as u32,
                Some(out.as_mut_ptr() as _),
                out.len() as u32,
                Some(&mut ret),
                None,
            );
            let _ = CloseHandle(h);
            if ok.is_err() || ret < 16 {
                continue;
            }
            // STORAGE_TEMPERATURE_DATA：Version(4) Size(4) NumOfTripPoints(2)
            // NumOfTemperatureSensors(2) + 每个 SENSOR：Length(2) RelativeLevel(2)
            // Temperature(4)。单位可能为 ℃、0.1℃ 或 K，按量级归一到 ℃。
            let n = u16::from_le_bytes([out[10], out[11]]) as usize;
            if n == 0 || ret < 20 {
                continue;
            }
            let raw = i32::from_le_bytes([out[16], out[17], out[18], out[19]]);
            let c = if (0..=120).contains(&raw) {
                raw as f32
            } else if (200..=400).contains(&raw) {
                raw as f32 - 273.15
            } else if (0..=1200).contains(&raw) {
                raw as f32 / 10.0
            } else {
                continue;
            };
            if (0.0..=120.0).contains(&c) && temps.len() < 2 {
                temps.push(c);
            }
        }
    }
    (!temps.is_empty()).then_some(temps)
}

// ---------------------------------------------------------------------------
// lhm-bridge 桥接：CPU 温度 + 风扇转速（LibreHardwareMonitorLib，无窗子进程）
// ---------------------------------------------------------------------------

type LhmData = (Option<f32>, Vec<f32>, Option<std::time::Instant>);
static LHM_CACHE: std::sync::Mutex<LhmData> = std::sync::Mutex::new((None, Vec::new(), None));

/// 桥接数据的有效期：bridge 进程活着但卡住（不再输出）时，超时的旧值
/// 视为无效，UI 隐藏而不是永久显示冻结的温度
const LHM_TTL: std::time::Duration = std::time::Duration::from_secs(10);

/// 启动桥接读取线程；桥接进程退出（崩溃/被杀）后 5 秒自动重启。
/// 桥接挂进本进程的 KILL_ON_JOB_CLOSE Job：本进程（sensor）被 UI 杀掉重启时，
/// 孤儿桥接随之退出，不再堆积。
pub fn spawn_lhm_bridge() {
    std::thread::spawn(move || {
        let job = create_bridge_job();
        loop {
            let _ = run_bridge_once(job);
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}

fn create_bridge_job() -> windows::Win32::Foundation::HANDLE {
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation,
    };
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null()).expect("create bridge job");
        let mut info = zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .expect("set bridge job limit");
        job
    }
}

fn bridge_exe_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    // 桥接与主程序同目录；开发期也可能在 lhm-bridge 发布目录
    let cand = dir.join("lhm-bridge.exe");
    if cand.exists() {
        Some(cand)
    } else {
        None
    }
}

fn run_bridge_once(job: windows::Win32::Foundation::HANDLE) -> Option<()> {
    use std::io::BufRead;
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows::Win32::Foundation::HANDLE;
    let exe = bridge_exe_path()?;
    let mut child = std::process::Command::new(exe)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .ok()?;
    // 挂入 Job：本进程意外退出时桥接一并终止
    unsafe {
        use std::os::windows::io::AsRawHandle;
        let _ = AssignProcessToJobObject(job, HANDLE(child.as_raw_handle() as _));
    }
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let reader = std::io::BufReader::new(stdout);
    let mut cpu_temp = None;
    let mut fans: Vec<f32> = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        if let Some(v) = line.strip_prefix("CPU_TEMP ") {
            cpu_temp = v.trim().parse::<f32>().ok();
        } else if let Some(rest) = line.strip_prefix("FAN ") {
            if let Some(rpm) = rest.rsplit(' ').next().and_then(|x| x.parse::<f32>().ok()) {
                if fans.len() < 3 {
                    fans.push(rpm);
                }
            }
        } else if line == "END" {
            if let Ok(mut c) = LHM_CACHE.lock() {
                *c = (cpu_temp, fans.clone(), Some(std::time::Instant::now()));
            }
            cpu_temp = None;
            fans.clear();
        }
    }
    // stdout 关闭 = 桥接退出；循环由外层负责重启
    let _ = child.wait();
    None
}

/// 配置文件路径（与 UI 侧 %APPDATA%	empmon.conf 一致）
fn config_refresh_ms() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(base).join("tempmon.conf")
}
