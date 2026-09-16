//! 进程内原生传感器：经 WinRing0 内核驱动（LibreHardwareMonitor fork）直读
//! Intel CPU 包温 + Nuvoton SuperIO 风扇转速，替代 .NET lhm-bridge 子进程
//!（省 ~70MB 常驻内存）。
//!
//! IOCTL 功能码与 LibreHardwareMonitor v0.9.4 Interop/Ring0.cs 一致（该 fork
//! 改过上游 WinRing0 的功能码，不能照搬 OpenLibSys 原版）；驱动复用 dist 里的
//! lhm-bridge.sys，服务命名沿用 LHM 的 "R0<进程名>" 约定以便复用已加载实例。
//! 装载失败时读数返回 None/空，对应段位由 UI 隐藏。

use std::sync::Mutex;

use windows::core::HSTRING;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Services::{
    CloseServiceHandle, CreateServiceW, OpenSCManagerW, OpenServiceW, StartServiceW, SC_HANDLE,
    SC_MANAGER_ALL_ACCESS, SERVICE_DEMAND_START, SERVICE_ERROR_NORMAL, SERVICE_KERNEL_DRIVER,
    SERVICE_QUERY_CONFIG,
};

const DEVICE_PATH: &str = "\\\\.\\WinRing0_1_2_0";

// CTL_CODE(40000, func, access)：LibreHardwareMonitor fork 的功能码表
const IOCTL_READ_MSR: u32 = (40000 << 16) | (0x821 << 2); // access=Any
const IOCTL_READ_IO_PORT_BYTE: u32 = (40000 << 16) | (1 << 14) | (0x833 << 2); // access=Read
const IOCTL_WRITE_IO_PORT_BYTE: u32 = (40000 << 16) | (2 << 14) | (0x836 << 2); // access=Write

static DRIVER: Mutex<Option<usize>> = Mutex::new(None); // HANDLE(usize)，仅传感器线程使用

/// 拿到驱动句柄；首次调用时确保服务已装载。失败返回 None。
fn driver() -> Option<HANDLE> {
    if let Ok(guard) = DRIVER.lock() {
        if let Some(h) = *guard {
            return Some(HANDLE(h as _));
        }
    }
    let h = open_device().or_else(ensure_service_then_open)?;
    if let Ok(mut guard) = DRIVER.lock() {
        *guard = Some(h.0 as usize);
    }
    Some(h)
}

fn open_device() -> Option<HANDLE> {
    unsafe {
        CreateFileW(
            &HSTRING::from(DEVICE_PATH),
            0xC000_0000u32, // GENERIC_READ | GENERIC_WRITE
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
        .map(|f| HANDLE(f.0))
    }
}

/// 设备未就绪时：以 LHM 同款命名（R0<进程名>）建内核驱动服务并启动，再开设备。
/// 用 SCManager API 而非 sc.exe：不受路径引号/编码影响。
fn ensure_service_then_open() -> Option<HANDLE> {
    let sys = std::env::current_exe().ok()?.parent()?.join("lhm-bridge.sys");
    if !sys.exists() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let name = HSTRING::from(format!("R0{}", exe.file_stem()?.to_string_lossy()));
    let path = HSTRING::from(sys.as_os_str());
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS).ok()?;
        let _scm_guard = ScmGuard(scm);
        // 已存在则直接复用；OpenService 失败（服务不存在）才创建
        let svc: SC_HANDLE = match OpenServiceW(scm, &name, SERVICE_QUERY_CONFIG) {
            Ok(s) => s,
            Err(_) => CreateServiceW(
                scm,
                &name,
                &name,
                0,
                SERVICE_KERNEL_DRIVER,
                SERVICE_DEMAND_START,
                SERVICE_ERROR_NORMAL,
                &path,
                None,
                None,
                None,
                None,
                None,
            )
            .ok()?,
        };
        let _svc_guard = ScmGuard(svc);
        // 已在运行时 StartService 会失败，不碍事：设备能打开就行
        let _ = StartServiceW(svc, None);
    }
    open_device()
}

struct ScmGuard(SC_HANDLE);
impl Drop for ScmGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

/// 读 MSR（Intel：IA32_TEMPERATURE_TARGET 0x1A2、IA32_PACKAGE_THERM_STATUS 0x1B1）
fn read_msr(index: u32) -> Option<u64> {
    let h = driver()?;
    let mut value = 0u64;
    let mut ret = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_READ_MSR,
            Some(&index as *const u32 as _),
            4,
            Some(&mut value as *mut u64 as _),
            8,
            Some(&mut ret),
            None,
        )
        .ok()?;
    }
    Some(value)
}

fn read_port(port: u16) -> Option<u8> {
    let h = driver()?;
    let mut out = 0u8;
    let mut ret = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_READ_IO_PORT_BYTE,
            Some(&port as *const u16 as _),
            2,
            Some(&mut out as *mut u8 as _),
            1,
            Some(&mut ret),
            None,
        )
        .ok()?;
    }
    Some(out)
}

fn write_port(port: u16, value: u8) -> Option<()> {
    let h = driver()?;
    let mut ret = 0u32;
    // WinRing0 写端口入参：OLS_WRITE_IO_PORT_INPUT { u32 端口, u32 值 }
    let mut input = [0u8; 8];
    input[0..4].copy_from_slice(&(port as u32).to_le_bytes());
    input[4] = value;
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_WRITE_IO_PORT_BYTE,
            Some(input.as_ptr() as _),
            8,
            None,
            0,
            Some(&mut ret),
            None,
        )
        .ok()?;
    }
    Some(())
}

/// Intel CPU 包温：TjMax(0x1A2 bits 23:16) − 数字读数(0x1B1 bits 22:16)，带有效位与量程校验
pub fn intel_package_temp() -> Option<f32> {
    let tjmax_msr = read_msr(0x1A2)?;
    let tjmax = ((tjmax_msr >> 16) & 0xFF) as f32;
    if !(40.0..=125.0).contains(&tjmax) {
        return None; // 读出的 TjMax 不合理 → 非 Intel 或 MSR 不可用
    }
    let therm = read_msr(0x1B1)?;
    let valid = (therm >> 31) & 1 == 1;
    let digital = ((therm >> 16) & 0x7F) as f32;
    let t = tjmax - digital;
    if valid && (-10.0..=110.0).contains(&t) {
        Some(t)
    } else {
        None
    }
}

/// Nuvoton NCT67xx SuperIO 风扇转速（rpm）。协议同 LibreHardwareMonitor
/// LpcIO/Nct677X：0x87,0x87 进 PnP 读芯片 ID；选 LDN 0x0B 读基址寄存器 0x60/0x61
/// 得到监控端口基址；顺带清 0x28 的 I/O 空间锁；0xAA 退出 PnP。监控区按
/// BSR(0x4E)=地址高字节选 bank、索引=低字节。
pub fn nuvoton_fans() -> Vec<f32> {
    for (ap, vp) in [(0x2Eu16, 0x2Fu16), (0x4Eu16, 0x4Fu16)] {
        if let Some(fans) = try_fans(ap, vp) {
            if !fans.is_empty() {
                return fans;
            }
        }
    }
    Vec::new()
}

fn try_fans(ap: u16, vp: u16) -> Option<Vec<f32>> {
    // 进 MB PnP 模式读芯片 ID（Nuvoton 监控家族 ID 高字节白名单；本机 NCT6799D = 0xD42B）
    write_port(ap, 0x87)?;
    write_port(ap, 0x87)?;
    write_port(ap, 0x20)?;
    let hi = read_port(vp)?;
    write_port(ap, 0x21)?;
    let lo = read_port(vp)?;
    if !matches!(hi, 0xB4 | 0xC3 | 0xC4 | 0xC5 | 0xC7 | 0xC8 | 0xC9 | 0xD4) {
        write_port(ap, 0xAA);
        return None;
    }
    // 选硬件监控 LDN，读 I/O 基址（word 寄存器 0x60/0x61）
    pnp_select(ap, vp, 0x0B)?;
    let b_hi = pnp_read(ap, vp, 0x60)? as u16;
    let b_lo = pnp_read(ap, vp, 0x61)? as u16;
    // 清 I/O 空间锁（寄存器 0x28 bit4，已清则无操作）
    let lock = pnp_read(ap, vp, 0x28)?;
    if lock & 0x10 != 0 {
        pnp_write(ap, vp, 0x28, lock & !0x10)?;
    }
    write_port(ap, 0xAA)?; // 退出 PnP
    let base = (b_hi << 8) | b_lo;
    if base == 0 || base == 0xFFFF {
        return None;
    }
    // 监控区：index=base+5，data=base+6
    let (ma, md) = (base + 5, base + 6);
    let mut fans = Vec::new();
    // NCT67xxD 13 位风扇计数器（bank 4）：count = (high<<5) | (low & 0x1F)，
    // rpm = 1_350_000 / count；count=0x1FFF（无扇）或 <0x15（无效）时跳过
    for reg in [0x4B0u16, 0x4B2, 0x4B4, 0x4B6, 0x4B8, 0x4BA] {
        let hi = read_banked(ma, md, reg)? as u16;
        let lo = read_banked(ma, md, reg + 1)? as u16;
        let count = (hi << 5) | (lo & 0x1F);
        if (0x15..0x1FFF).contains(&count) {
            let rpm = 1_350_000f32 / count as f32;
            if (60.0..=9999.0).contains(&rpm) {
                fans.push(rpm);
            }
        }
    }
    Some(fans)
}

/// PnP 模式下选择逻辑设备（寄存器 0x07）
fn pnp_select(ap: u16, vp: u16, ldn: u8) -> Option<()> {
    write_port(ap, 0x07)?;
    write_port(vp, ldn)
}

fn pnp_read(ap: u16, vp: u16, reg: u8) -> Option<u8> {
    write_port(ap, reg)?;
    read_port(vp)
}

fn pnp_write(ap: u16, vp: u16, reg: u8, value: u8) -> Option<()> {
    write_port(ap, reg)?;
    write_port(vp, value)
}

/// 监控区读：BSR(0x4E) 选 bank（地址高字节），再按低字节索引读数据
fn read_banked(ma: u16, md: u16, addr: u16) -> Option<u8> {
    write_port(ma, 0x4E)?;
    write_port(md, (addr >> 8) as u8)?;
    write_port(ma, (addr & 0xFF) as u8)?;
    read_port(md)
}

/// 采样：CPU 温度 + 最多三个风扇。任一失败都是空/None，由 UI 决定隐藏。
pub fn sample() -> (Option<f32>, Vec<f32>) {
    let mut caps = CapLog::default();
    let temp = match cpu_vendor() {
        Vendor::Intel => intel_package_temp().or_else(amd_package_temp),
        Vendor::Amd => amd_package_temp().or_else(intel_package_temp),
        Vendor::Unknown => None,
    };
    caps.cpu = temp.map(|t| format!("{t:.1}"));
    let mut fans = nuvoton_fans();
    caps.superio = "Nuvoton".into();
    if fans.is_empty() {
        fans = it87_fans();
        caps.superio = if fans.is_empty() { "IT87(无读数)".into() } else { "ITE".into() };
    }
    if fans.len() > 3 {
        fans.truncate(3);
    }
    caps.fans = fans.iter().map(|f| format!("{f:.0}")).collect::<Vec<_>>().join("/");
    caps.flush();
    (temp, fans)
}

// ---------------------------------------------------------------------------
// CPU 厂商识别（cpuid）
// ---------------------------------------------------------------------------

#[derive(PartialEq, Eq, Debug)]
pub enum Vendor {
    Intel,
    Amd,
    Unknown,
}

pub fn cpu_vendor() -> Vendor {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::__cpuid;
        let v = __cpuid(0);
        let buf = [v.ebx, v.edx, v.ecx];
        let s: [u8; 12] = std::mem::transmute(buf);
        match &s {
            b"GenuineIntel" => Vendor::Intel,
            b"AuthenticAMD" => Vendor::Amd,
            _ => Vendor::Unknown,
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Vendor::Unknown
    }
}

/// CPU 家族号（cpuid leaf1）：AMD Zen = 0x17/0x19
fn cpu_family() -> u32 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::__cpuid;
        let eax = __cpuid(1).eax;
        let base = (eax >> 8) & 0xF;
        if base == 0xF {
            base + ((eax >> 20) & 0xFF)
        } else {
            base
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// AMD Zen（Family 17h/19h）：SMN 经 PCI 0:0:0 寄存器 0x60(index)/0x64(data) 访问，
// 温度 = THM_TCON_CUR_TMP(0x59800) bits 31:21 * 0.125，bit19 置位时再 -49。
// 流程与 LibreHardwareMonitor Amd17Cpu 一致。
// ---------------------------------------------------------------------------

const IOCTL_READ_PCI_CONFIG: u32 = (40000 << 16) | (1 << 14) | (0x851 << 2); // access=Read
const IOCTL_WRITE_PCI_CONFIG: u32 = (40000 << 16) | (2 << 14) | (0x852 << 2); // access=Write

fn read_pci_dword(pci_addr: u32, reg: u32) -> Option<u32> {
    let h = driver()?;
    let mut out = 0u32;
    let mut ret = 0u32;
    let input: [u32; 2] = [pci_addr, reg];
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_READ_PCI_CONFIG,
            Some(input.as_ptr() as _),
            8,
            Some(&mut out as *mut u32 as _),
            4,
            Some(&mut ret),
            None,
        )
        .ok()?;
    }
    Some(out)
}

fn write_pci_dword(pci_addr: u32, reg: u32, value: u32) -> Option<()> {
    let h = driver()?;
    let mut ret = 0u32;
    let input: [u32; 3] = [pci_addr, reg, value];
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_WRITE_PCI_CONFIG,
            Some(input.as_ptr() as _),
            12,
            None,
            0,
            Some(&mut ret),
            None,
        )
        .ok()?;
    }
    Some(())
}

pub fn amd_package_temp() -> Option<f32> {
    if !matches!(cpu_vendor(), Vendor::Amd) {
        return None;
    }
    let family = cpu_family();
    if !matches!(family, 0x17 | 0x19) {
        return None; // Zen 1-4/5(F19h)；更老的 0Fh/10h 与 Zen5(1Ah) 不支持
    }
    // SMN 地址写入 PCI 0:0:0 的 0x60，从 0x64 读数据
    write_pci_dword(0, 0x60, 0x0005_9800)?;
    let data = read_pci_dword(0, 0x64)?;
    let temp = ((data >> 21) * 125) as f32 * 0.001 + if data & 0x8_0000 != 0 { -49.0 } else { 0.0 };
    if (0.0..=110.0).contains(&temp) {
        Some(temp)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// ITE IT87xx 风扇（Nuvoton 不在场时兜底）。协议同 LibreHardwareMonitor
// IT87XX：PnP 进出同 NCT，芯片 ID 在 0x20/0x21；选 LDN 0x04 读基址 0x60/0x61；
// 监控区 index=base+5 / data=base+6（bank 0）。
// ---------------------------------------------------------------------------

pub fn it87_fans() -> Vec<f32> {
    for (ap, vp) in [(0x2Eu16, 0x2Fu16), (0x4Eu16, 0x4Fu16)] {
        if let Some(fans) = try_it87_fans(ap, vp) {
            if !fans.is_empty() {
                return fans;
            }
        }
    }
    Vec::new()
}

fn try_it87_fans(ap: u16, vp: u16) -> Option<Vec<f32>> {
    write_port(ap, 0x87)?;
    write_port(ap, 0x87)?;
    write_port(ap, 0x20)?;
    let hi = read_port(vp)?;
    write_port(ap, 0x21)?;
    let lo = read_port(vp)?;
    let chip_id = ((hi as u16) << 8) | lo as u16;
    // 16 位转速计数器的芯片（较新的 IT86xx/87xx）；老 8 位+分频芯片先不支持
    let has_16bit = matches!(
        chip_id,
        0x8613 | 0x8620 | 0x8625 | 0x8628 | 0x8631 | 0x8655 | 0x8665 | 0x8686 | 0x8688
            | 0x8689 | 0x8695 | 0x8696 | 0x8728 | 0x8733 | 0x8790
    );
    if !has_16bit {
        write_port(ap, 0xAA);
        return None;
    }
    // 选 EC LDN 0x04，读 I/O 基址
    pnp_select(ap, vp, 0x04)?;
    let b_hi = pnp_read(ap, vp, 0x60)? as u16;
    let b_lo = pnp_read(ap, vp, 0x61)? as u16;
    write_port(ap, 0xAA)?; // 退出 PnP
    let base = (b_hi << 8) | b_lo;
    if base == 0 || base == 0xFFFF {
        return None;
    }
    let (ma, md) = (base + 5, base + 6);
    // IT87 直读寄存器（无 bank select）
    let mut fans = Vec::new();
    // 转速寄存器对：低字节 / 扩展(高)字节
    for (lo_reg, hi_reg) in [(0x0Du16, 0x18u16), (0x0E, 0x19), (0x0F, 0x1A), (0x80, 0x81)] {
        write_port(ma, lo_reg as u8)?;
        let lo = read_port(md)? as u16;
        write_port(ma, hi_reg as u8)?;
        let hi = read_port(md)? as u16;
        let value = lo | (hi << 8);
        // >0x3F 才有效（LHM 同款门限），0xFFFF = 无扇
        if value > 0x3F && value < 0xFFFF {
            let rpm = 1_350_000f32 / (value * 2) as f32;
            if (60.0..=9999.0).contains(&rpm) {
                fans.push(rpm);
            }
        }
    }
    Some(fans)
}

// ---------------------------------------------------------------------------
// 能力自检日志：每次启动把探测结果写到 %APPDATA%\tempmon-caps.log，
// 供"段位为什么没显示"排查用。
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CapLog {
    cpu: Option<String>,
    fans: String,
    superio: String,
}

impl CapLog {
    fn flush(&self) {
        let line = format!(
            "{} | cpu_vendor={:?} | cpu_temp={} | superio={} | fans=[{}]\n",
            chrono_like_now(),
            cpu_vendor(),
            self.cpu.as_deref().unwrap_or("无"),
            self.superio,
            self.fans,
        );
        if let Ok(mut p) = std::env::var("APPDATA") {
            p.push_str("\\tempmon-caps.log");
            let _ = std::fs::write(p, line);
        }
    }
}

fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 仅用于日志排序的本地粗略时间戳（UTC 秒），避免引第三方时间库
    format!("#epoch={secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cpu_temp_and_fans() {
        let (t, fans) = sample();
        println!("CPU temp: {:?}", t);
        println!("Fans: {:?}", fans);
        assert!(t.is_some(), "MSR 读不到 CPU 温度（驱动装载失败？）");
        assert!((20.0..=100.0).contains(&t.unwrap()), "温度离谱: {:?}", t);
        assert!(!fans.is_empty(), "SuperIO 读不到风扇");
        assert!(fans.iter().all(|r| (300.0..=6000.0).contains(r)), "转速离谱: {:?}", fans);
    }
}
