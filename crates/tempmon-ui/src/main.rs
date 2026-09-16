//! GLM 桌面温度计 —— 置顶透明系统监控小部件
//! Win32 分层窗口 + Direct2D 渲染 + 逐像素透明 + 毛玻璃（可选）。
//! 双进程：UI（本进程）+ 采集子进程（tempmon-sensor sensor_loop）+ lhm-bridge。

#![allow(non_snake_case)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod strategy;

use std::mem::zeroed;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Child;
use std::ptr::null_mut;
use std::time::Instant;

use tempmon_sensor::{frame_len, frame_seq, read_frame, sensor_loop, Snapshot};
use windows::core::{w, PCSTR, PCWSTR};
use windows_numerics::Vector2;
use windows::Win32::Foundation::{
    COLORREF, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, CreateRoundRectRgn, DeleteDC, DeleteObject,
    GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JobObjectExtendedLimitInformation,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READWRITE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, HWINEVENTHOOK, SetWinEventHook,
    TreeScope_Children, TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_ControlTypePropertyId,
    UIA_NamePropertyId,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, VK_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, DestroyMenu, GetMessageW, GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowRect,
    GetClassNameW, KillTimer, LoadCursorW, SendMessageW, PostMessageW, RegisterClassExW,
    SetForegroundWindow,
    SetTimer, SetWindowDisplayAffinity, SetWindowsHookExW, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, SystemParametersInfoW, TrackPopupMenu, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND, GWL_EXSTYLE,
    GWLP_USERDATA, HTCAPTION, HTCLIENT, HTTRANSPARENT, HWND_TOPMOST, IDC_ARROW, KBDLLHOOKSTRUCT,
    MSLLHOOKSTRUCT, MF_CHECKED, MF_ENABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    MF_UNCHECKED, PostQuitMessage, SPI_GETWORKAREA, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, TPM_LEFTALIGN, TPM_RIGHTBUTTON, ULW_ALPHA, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WH_KEYBOARD_LL, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT, WM_APP, WM_COMMAND,
    WM_DESTROY, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WindowFromPoint, WM_TIMER, WNDCLASSEXW, WINEVENT_SKIPOWNPROCESS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

// ── 常量 ──

const TIMER_ID: usize = 1;
const DODGE_TIMER_ID: usize = 2;
const CAP_TIMER_ID: usize = 3;
const DODGE_RETRY_TIMER_ID: usize = 4;
// 前台切换后的补查定时器（0.3/0.8/1.6s）：切换后软件可能延迟绘制标题栏，
// 截图判定只能等真实像素出现——连发补查把响应压到 2s 内
const DODGE_BURST_TIMER_ID: usize = 5;
const DODGE_BURST_COUNT: usize = 5;
const DODGE_BURST_MS: [u32; DODGE_BURST_COUNT] = [300, 800, 1600, 2600, 4000];
// 事件节流窗口：窗口动画期间的事件风暴合并为一次检测
const DODGE_THROTTLE_MS: usize = 300;
// 兜底轮询周期：即使没有任何窗口事件也定期查一次（后台窗口内容变化等场景）
const DODGE_SWEEP_MS: usize = 1500;
// 排除标志生效到截屏之间的等待（定时器，不阻塞 UI 线程）
const CAP_CAPTURE_DELAY_MS: u32 = 45;
// 检测上下文：请求阶段计算一次，截屏回调中复用
#[derive(Clone, Copy)]
struct DodgeCtx {
    x0: i32,
    y: i32,
    h: i32,
    max_w: i32,
    cur_w: i32,
    left0: i32,
    wa: RECT,
    /// true = 第二阶段执行"回归右缘"选位（当前未遮挡、只找更靠右的空位）；
    /// false = 常规避让选位（当前位置被遮挡）
    ret_right: bool,
}
struct CapReq {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    full: bool,
    /// true = 标题栏同步截取（右缘按钮条带，只做行密度分析）
    sync: bool,
    excluded: bool,
    hidden: bool,
}

// ── 防遮挡自动避让 ──
// 周期可在菜单自定义（DODGE_CHOICES_SECS，0 = 关闭）；
// 若检测到遮挡则向右优先找空位，淡出→移动→淡入
const DODGE_TIMER_MS: u32 = 16;
const FADE_OUT_STEP: f32 = 0.12;
const FADE_IN_STEP: f32 = 0.08;
const MENU_EXIT: usize = 100;
const MENU_COLLAPSE: usize = 200;
const MENU_ALPHA_25: usize = 205;
const MENU_ALPHA_40: usize = 206;
const MENU_ALPHA_55: usize = 201;
const MENU_ALPHA_70: usize = 202;
const MENU_ALPHA_85: usize = 203;
const MENU_ALPHA_100: usize = 204;
/// 透明度菜单档位（与 MENU_ALPHA_* 一一对应）：加载配置时吸附到最近档，
/// 保证右键菜单永远有当前值的勾选（旧版默认 230 不落在档位上 = 永远无勾）
const ALPHA_CHOICES: [u8; 6] = [64, 102, 140, 179, 217, 255];
const MENU_AUTOSTART: usize = 210;
const MENU_RESET_POS: usize = 220;
const MENU_PIN: usize = 225;
const MENU_CAPSULE_BASE: usize = 230;
const MENU_ROW_BASE: usize = 250;
const MENU_ROTATE_BASE: usize = 260;
const MENU_REFRESH_BASE: usize = 270;
const MENU_BG_BASE: usize = 280;
const MENU_DODGE_BASE: usize = 290;
const WM_APP_CTRL: u32 = WM_APP + 1;
const WM_APP_WHEEL: u32 = WM_APP + 2;
const WM_APP_DODGE: u32 = WM_APP + 3;
const APP_NAME: &str = "TempmonWidget";

const EDGE_MARGIN: f32 = 8.0;
/// UIA 按钮簇左缘之外的安全间距：簇矩形不含分隔线与图标溢出，
/// 留得太小时胶囊会贴到 1px 缝（用户看到的"压着按钮"）
const CAPTION_GAP: i32 = 32;
/// UIA 不可信时按标准三按钮 + 间距预留（24px 小按钮风格簇从 159px 起）
const CAPTION_FALLBACK: i32 = 190;
const FONT_PX: f32 = 13.0;
const BAR_H: f32 = 24.0;
const CAP_H: f32 = 24.0;
const PAD_X: f32 = 12.0;
const GAP: f32 = 1.0;

const ROTATE_CHOICES_MS: [u32; 4] = [1000, 2000, 3000, 5000];
const REFRESH_CHOICES_MS: [u32; 3] = [500, 1000, 2000];
// 防遮挡检测开关：0 = 关闭，1 = 开启（事件驱动：窗口变化即检测，无轮询）
const DODGE_CHOICES_SECS: [u32; 2] = [0, 1];

const CAPS_MAXTEMP: u32 = 1 << 0;
const CAPS_CPU: u32 = 1 << 1;
const CAPS_GPU: u32 = 1 << 2;
const CAPS_GPUTEMP: u32 = 1 << 3;
const CAPS_VRAM: u32 = 1 << 4;
const CAPS_MEM: u32 = 1 << 5;
const CAPS_DISK: u32 = 1 << 6;
const CAPS_CPUTEMP: u32 = 1 << 7;
const CAPS_FAN: u32 = 1 << 8;
const CAPS_DEFAULT: u32 = CAPS_CPU | CAPS_CPUTEMP | CAPS_GPU | CAPS_GPUTEMP;

const ROW_CPU: u32 = 1 << 0;
const ROW_GPU: u32 = 1 << 1;
const ROW_MEM: u32 = 1 << 2;
const ROW_DISK: u32 = 1 << 3;
const ROW_FAN: u32 = 1 << 4;
const ROW_DEFAULT: u32 = ROW_CPU | ROW_GPU | ROW_MEM | ROW_DISK | ROW_FAN;

static TOPMOST_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
// 事件驱动防遮挡：其他窗口发生移动/显示/隐藏/前台切换时置位，tick 中消费
static DODGE_EVENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
// 进程启动时刻（用于节流的毫秒计时）
static START_MS: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
// 上次避让检测的时刻（相对 START_MS 的毫秒数），节流窗口 DODGE_THROTTLE_MS
static DODGE_LAST_CHECK_MS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// ── UIA 标题栏按钮对齐 ──
//
// 用 UI Automation 读取前台窗口"关闭"按钮的真实矩形，y 中心做对齐锚点；
// 查不到（元素不暴露/超时）时回退 strip_button_band_center_y 截图启发式。
// UIA 树遍历可能几十~几百 ms，禁止在 UI 线程同步调：独立线程查询，
// 结果写 UIA_CACHE，主线程只读。

/// 前台 hwnd → (按钮行 y 中心, 按钮簇左缘, 时刻)。y == i32::MIN 表示该窗口
/// 查过但 UIA 拿不到按钮（成功 TTL 60s，失败 TTL 10s，到期后允许重试）
static UIA_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<isize, (i32, i32, Instant)>>> =
    std::sync::OnceLock::new();

fn uia_cache() -> std::sync::MutexGuard<'static, std::collections::HashMap<isize, (i32, i32, Instant)>> {
    UIA_CACHE
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
// 查询工作线程互斥：同一时刻最多一个 UIA 查询在跑
static UIA_WORKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 请求后台查询前台 hwnd 的标题栏按钮行。
/// force=true 用于前台切换：强制重新查询——窗口在后台期间布局可能已变
/// （如浏览器切换标签页显隐 tab 条，按钮行会移动），旧锚点不可信
fn uia_ensure_request(hwnd_id: isize, force: bool) {
    if hwnd_id == 0 {
        return;
    }
    if !force {
        let map = uia_cache();
        if let Some(&(y, _, t)) = map.get(&hwnd_id) {
            let ttl = if y == i32::MIN { 10 } else { 60 };
            if t.elapsed() < std::time::Duration::from_secs(ttl) {
                return; // 缓存新鲜
            }
        }
    }
    if UIA_WORKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let debug = std::env::var("TEMPMON_DODGE_DEBUG").is_ok();
    let res = std::thread::Builder::new().name("uia-close-btn".into()).spawn(move || {
        let mut anchor = unsafe { uia_query_caption_row(HWND(hwnd_id as _)) }.unwrap_or((i32::MIN, i32::MIN));
        if force {
            // Chromium 系窗口的 UIA 矩形对布局变更滞后一次：强制刷新时
            // 隔 250ms 采第二次，取较新的结果
            std::thread::sleep(std::time::Duration::from_millis(250));
            if let Some(a2) = unsafe { uia_query_caption_row(HWND(hwnd_id as _)) } {
                anchor = a2;
            }
        }
        if debug {
            eprintln!("[uia] hwnd={hwnd_id:#x} y={} zone_left={}",
                if anchor.0 == i32::MIN { "无".into() } else { anchor.0.to_string() },
                if anchor.1 == i32::MIN { "无".into() } else { anchor.1.to_string() });
        }
        uia_cache().insert(hwnd_id, (anchor.0, anchor.1, Instant::now()));
        UIA_WORKING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
    if res.is_err() {
        UIA_WORKING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 读缓存：返回前台窗口标题栏按钮行的 (y 中心, 按钮簇左缘)；无值/过期返回 None 并顺路请求刷新
fn uia_cached_anchor(hwnd_id: isize) -> Option<(i32, i32)> {
    if hwnd_id == 0 {
        return None;
    }
    let fresh = {
        let map = uia_cache();
        match map.get(&hwnd_id) {
            Some(&(y, zl, t)) => {
                let ttl = if y == i32::MIN { 10 } else { 60 };
                if t.elapsed() < std::time::Duration::from_secs(ttl) && y != i32::MIN {
                    Some((y, zl))
                } else {
                    None
                }
            }
            None => None,
        }
    };
    if fresh.is_none() {
        uia_ensure_request(hwnd_id, false);
    }
    fresh
}

/// y 中心便捷读取
fn uia_cached_close_y(hwnd_id: isize) -> Option<i32> {
    uia_cached_anchor(hwnd_id).map(|(y, _)| y)
}

/// 缓存三态：Ready 查到真实锚点 / Failed 该窗口 UIA 拿不到 / Pending 还在查。
/// Pending 时调用方应不动作（避免先按估算值跳一次、再按真实值跳第二次）
enum AnchorState {
    Ready(i32, i32),
    Failed,
    Pending,
}

fn uia_anchor_state(hwnd_id: isize) -> AnchorState {
    if hwnd_id == 0 {
        return AnchorState::Failed;
    }
    let st = {
        let map = uia_cache();
        match map.get(&hwnd_id) {
            Some(&(y, zl, t)) => {
                let ttl = if y == i32::MIN { 10 } else { 60 };
                if t.elapsed() >= std::time::Duration::from_secs(ttl) {
                    AnchorState::Pending
                } else if y == i32::MIN {
                    AnchorState::Failed
                } else {
                    AnchorState::Ready(y, zl)
                }
            }
            None => AnchorState::Pending,
        }
    };
    if matches!(st, AnchorState::Pending) {
        uia_ensure_request(hwnd_id, false);
    }
    st
}

/// UIA 查询标题栏按钮行：右上角区域里的 Button 簇。
/// 锚点优先 Name=="关闭"/"Close"（浏览器标签页同名按钮会被区域校验排除），
/// 否则取最右一枚；簇 = 与锚点同行（|y 差| ≤ 15px）的按钮。
/// 返回 (行 y 中心, 簇左缘)。簇左缘用于胶囊贴右时预留整个按钮区——
/// ZCode 等应用标题栏除标准三枚外还有附加按钮，固定预留 160px 会压住它们。
unsafe fn uia_query_caption_row(hwnd: HWND) -> Option<(i32, i32)> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
    let root = automation.ElementFromHandle(hwnd).ok()?;
    let mut wr = RECT::default();
    if GetWindowRect(hwnd, &mut wr).is_err() {
        return None;
    }
    let type_cond = automation
        .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ButtonControlTypeId.0))
        .ok()?;
    let all = root.FindAll(TreeScope_Descendants, &type_cond).ok()?;
    let debug = std::env::var("TEMPMON_DODGE_DEBUG").is_ok();
    // 收集右上角区域的按钮（区域限定排除文档内容区的普通按钮）
    let mut cands: Vec<(i32, i32, i32, i32, String)> = Vec::new(); // (left, top, w, h, name)
    let n = all.Length().unwrap_or(0).max(0);
    for i in 0..n {
        let Ok(btn) = all.GetElement(i) else { continue };
        let Ok(r) = btn.CurrentBoundingRectangle() else { continue };
        let (bx, by, bw, bh) = (r.left, r.top, r.right - r.left, r.bottom - r.top);
        if bx <= wr.right - 300 || by >= wr.top + 60 || bw > 80 || bh > 60 {
            continue;
        }
        let name = btn.CurrentName().unwrap_or_default();
        if debug {
            eprintln!("[uia-cand] name={name:?} x={bx} y={by} w={bw} h={bh}");
        }
        cands.push((bx, by, bw, bh, name.to_string()));
    }
    if cands.is_empty() {
        return None;
    }
    // 锚点：优先命名的关闭按钮，否则最右一枚
    let anchor = cands
        .iter()
        .find(|(_, _, _, _, nm)| nm == "关闭" || nm.eq_ignore_ascii_case("close"))
        .or_else(|| cands.iter().max_by_key(|(bx, _, _, _, _)| *bx))?;
    // 过期矩形检测：真实 Win11 标题栏按钮宽 ≥40px（46px@100%DPI）。
    // Chromium 系窗口布局变更后 UIA 树会滞后报旧版 24px 小按钮——
    // y 恰好接近骗得过交叉校验，但 x 簇边界被算短、胶囊压住真实按钮。
    // 锚点按钮过小即判不可信，整体回退截图启发式 + 160px 预留
    if anchor.2 <= 32 || anchor.3 <= 32 {
        if debug {
            eprintln!("[uia-stale] 锚点按钮 {}x{} 过小（UIA 过期矩形），弃用", anchor.2, anchor.3);
        }
        return None;
    }
    let row_y = anchor.1 + anchor.3 / 2;
    // 簇：与锚点同行的按钮，取最左左缘
    let zone_left = cands
        .iter()
        .filter(|(_, by, _, bh, _)| ((by + bh / 2) - row_y).abs() <= 15)
        .map(|(bx, _, _, _, _)| *bx)
        .min()?;
    Some((row_y, zone_left))
}



// ── 防抖 ──

#[derive(Default)]
struct Hold {
    val: Option<f32>,
    since: Option<Instant>,
    vec: Option<Vec<f32>>,
}

fn hold(h: &mut Hold, new: Option<f32>) -> Option<f32> {
    const TTL: std::time::Duration = std::time::Duration::from_secs(15);
    match new {
        Some(v) => {
            h.val = Some(v);
            h.since = Some(Instant::now());
            Some(v)
        }
        None => match (h.val, h.since) {
            (Some(v), Some(t)) if t.elapsed() < TTL => Some(v),
            _ => {
                h.val = None;
                h.since = None;
                None
            }
        },
    }
}

impl Hold {
    fn set_vec(&mut self, new: Vec<f32>) -> Vec<f32> {
        const TTL: std::time::Duration = std::time::Duration::from_secs(15);
        if !new.is_empty() {
            self.val = new.first().copied();
            self.since = Some(Instant::now());
            self.vec = Some(new.clone());
            return new;
        }
        match (&self.vec, self.since) {
            (Some(v), Some(t)) if t.elapsed() < TTL => v.clone(),
            _ => {
                self.vec = None;
                self.since = None;
                Vec::new()
            }
        }
    }
}

#[derive(Clone)]
struct Part {
    text: String,
    color: D2D1_COLOR_F,
    slot: Option<String>,
    dot: bool,
}

impl Part {
    fn new(text: String, color: D2D1_COLOR_F) -> Self {
        Self { text, color, slot: None, dot: false }
    }
    fn val(text: String, color: D2D1_COLOR_F, slot: &str) -> Self {
        Self { text, color, slot: Some(slot.to_string()), dot: false }
    }
    fn dot(color: D2D1_COLOR_F) -> Self {
        Self { text: String::new(), color, slot: None, dot: true }
    }
}

fn temp_color(t: f32, light: bool) -> D2D1_COLOR_F {
    if t >= 75.0 {
        D2D1_COLOR_F { r: 0.85, g: 0.15, b: 0.15, a: 1.0 }
    } else if t >= 60.0 {
        D2D1_COLOR_F { r: 0.80, g: 0.42, b: 0.00, a: 1.0 }
    } else if light {
        D2D1_COLOR_F { r: 0.10, g: 0.12, b: 0.16, a: 1.0 }
    } else {
        D2D1_COLOR_F { r: 0.97, g: 0.97, b: 0.97, a: 1.0 }
    }
}

#[derive(Clone)]
struct Config {
    collapsed: bool,
    alpha: u8,
    pos: Option<POINT>,
    capsule_items: u32,
    row_items: u32,
    rotate_ms: u32,
    refresh_ms: u32,
    bg_mode: u8,
    dodge_secs: u32,
    pinned: bool,
    widest: i32,
}

struct App {
    hwnd: HWND,
    job: HANDLE,
    view: *const u8,
    child: Option<Child>,
    last_seq: u32,
    last_seq_change: Option<Instant>,
    child_spawned_at: Option<Instant>,
    child_produced: bool,
    respawn_streak: u32,
    respawn_not_before: Option<Instant>,
    strategy: strategy::Strategy,
    hidden: bool,
    collapsed: bool,
    alpha: u8,
    pos: Option<POINT>,
    capsule_items: u32,
    row_items: u32,
    rotate_ms: u32,
    refresh_ms: u32,
    bg_mode: u8,
    click_pos: Option<POINT>,
    clickthrough_state: bool,
    trim_tick: u32,
    h_gpu_usage: Hold,
    h_gpu_vram: Hold,
    h_gpu_temp: Hold,
    h_cpu_temp: Hold,
    h_disk: Hold,
    h_fans: Hold,
    capsule_page: u32,
    // 上次胶囊翻页时刻：按真实时间翻页；避让动画的 16ms 手动 tick 不再加速轮换
    last_page_rot: Option<Instant>,
    // 上次前台窗口：切换时立即解除标题栏同步门（不等 10s 轮换）
    last_fg_hwnd: isize,
    /// UIA 锚点与截图启发式差 >6px 的窗口：UIA 矩形过期（Chromium 树滞后），
    /// 该窗口在 TTL 内改用截图启发式（y 与 x 预留都退到 160px 估算）
    uia_distrust: Option<(isize, Instant)>,
    // 连续判定为遮挡的轮数：标题栏动画（载入 spinner 等）会让遮挡判定逐帧
    // 翻转，单轮即挪会造成左右乒乓；连续两轮才挪
    occ_streak: u32,
    // 最近 3 次 y 对齐提议：取多数/中位，重绘空帧等单次异常不生效
    sync_ty_hist: [i32; 3],
    sync_ty_n: u32,
    dragging: bool,
    d2d: ID2D1Factory,
    rt: ID2D1DCRenderTarget,
    dwrite: IDWriteFactory,
    format: IDWriteTextFormat,
    scale: f32,
    dc_mem: HDC,
    hbmp: Option<HBITMAP>,
    hbmp_old: HGDIOBJ,
    buf_w: i32,
    buf_h: i32,
    dwm_round_ok: bool,
    fade: f32,
    fade_phase: u8,
    fade_x: i32,
    fade_y: i32,
    dodging: bool,
    last_dodge: Option<Instant>,
    widest: i32,
    last_checked_w: i32,
    rendered_once: bool,
    // 上次成功渲染的（快照, 胶囊页, 透明度）：全都没变且无动画时跳过整帧 D2D 排版重绘
    last_rendered: Option<(Snapshot, u32, u8)>,
    dodge_secs: u32,
    user_pinned: bool,
    started_at: Instant,
    cap: Option<CapReq>,
    dctx: Option<DodgeCtx>,
    fb_x: i32,
    free_streak: u32,
    ret_cooldown_until: Option<Instant>,
    last_sync_check: Option<Instant>,
    fb_d: f32,
    fb_runs: u32,
}

// ── 配置持久化 ──

fn config_path() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|d| PathBuf::from(d).join("tempmon.conf"))
}

fn load_config() -> Config {
    let mut cfg = Config {
        collapsed: false,
        alpha: 217,
        pos: None,
        capsule_items: CAPS_DEFAULT,
        row_items: ROW_DEFAULT,
        rotate_ms: 2000,
        refresh_ms: 1000,
        bg_mode: 0,
        dodge_secs: 1,
        pinned: false,
        widest: 0,
    };
    if let Some(p) = config_path() {
        if let Ok(text) = std::fs::read_to_string(p) {
            for line in text.lines() {
                let (k, v) = match line.split_once('=') {
                    Some(kv) => kv,
                    None => continue,
                };
                match (k.trim(), v.trim().parse::<i32>()) {
                    ("collapsed", Ok(b)) => cfg.collapsed = b != 0,
                    ("alpha", Ok(a)) if (60..=255).contains(&a) => cfg.alpha = a as u8,
                    ("capsule", Ok(v)) if (0..0x200).contains(&v) => cfg.capsule_items = v as u32,
                    ("row", Ok(v)) if (0..0x20).contains(&v) => cfg.row_items = v as u32,
                    ("rotate", Ok(v)) if (500..=10000).contains(&v) => cfg.rotate_ms = v as u32,
                    ("refresh", Ok(v)) if (250..=5000).contains(&v) => cfg.refresh_ms = v as u32,
                    ("bg", Ok(v)) if (0..=3).contains(&v) => cfg.bg_mode = v as u8,
                    ("dodge", Ok(v)) if v == 0 || (2..=3600).contains(&v) => {
                        // 事件驱动开关：任何非 0 值视为开启
                        cfg.dodge_secs = if v == 0 { 0 } else { 1 }
                    }
                    ("pinned", Ok(b)) => cfg.pinned = b != 0,
                    ("widest", Ok(n)) => cfg.widest = n,
                    ("x", Ok(x)) => {
                        cfg.pos = Some(POINT { x, y: cfg.pos.map(|q| q.y).unwrap_or(0) })
                    }
                    ("y", Ok(y)) => {
                        cfg.pos = Some(POINT { x: cfg.pos.map(|q| q.x).unwrap_or(0), y })
                    }
                    _ => {}
                }
            }
        }
    }
    // 吸附到最近档位：历史配置里的任意值（如旧默认 230）在菜单中无勾可对
    cfg.alpha = *ALPHA_CHOICES
        .iter()
        .min_by_key(|&&a| (a as i16 - cfg.alpha as i16).abs())
        .unwrap_or(&217);
    cfg
}

fn save_config(cfg: &Config) {
    if let Some(p) = config_path() {
        // pos=None（吸附右上角）不写 x/y 行：写了 0/0 会被 load 读回 Some((0,0))，
        // 重启后温度计会钉在屏幕左上角
        let pos_line = match cfg.pos {
            Some(q) => format!("x={}\ny={}\n", q.x, q.y),
            None => String::new(),
        };
        let text = format!(
            "collapsed={}\nalpha={}\ncapsule={}\nrow={}\nrotate={}\nrefresh={}\nbg={}\ndodge={}\npinned={}\n{}",
            cfg.collapsed as u8,
            cfg.alpha,
            cfg.capsule_items,
            cfg.row_items,
            cfg.rotate_ms,
            cfg.refresh_ms,
            cfg.bg_mode,
            cfg.dodge_secs,
            cfg.pinned as u8,
            pos_line,
        );
        let _ = std::fs::write(p, text);
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn autostart_enabled() -> bool {
    std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", APP_NAME])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn set_autostart(enable: bool) {
    // HKCU Run：无需管理员权限，schtasks ONLOGON 在普通账户下会静默失败
    let exe = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_default();
    let key = format!(r"HKCU\{}", RUN_KEY);
    if enable {
        let _ = std::process::Command::new("reg")
            .args(["add", &key, "/v", APP_NAME, "/t", "REG_SZ",
                   "/d", &format!("\"{}\"", exe), "/f"])
            .creation_flags(0x0800_0000)
            .output();
    } else {
        let _ = std::process::Command::new("reg")
            .args(["delete", &key, "/v", APP_NAME, "/f"])
            .creation_flags(0x0800_0000)
            .output();
    }
}

// ── 主入口 ──

fn main() -> windows::core::Result<()> {
    if std::env::var("TEMPMON_SENSOR").as_deref() == Ok("1") {
        tempmon_sensor::sensor_loop();
        return Ok(());
    }

    unsafe {
        // 单实例保护：多开会导致两个 widget 叠放、两个 sensor 竞写同一块
        // 命名共享内存、避让逻辑互相触发。持有互斥体直到进程退出。
        let mutex = windows::Win32::System::Threading::CreateMutexW(None, false, w!("Local\\TempmonWidgetInstance"))?;
        if mutex.is_invalid()
            || windows::Win32::Foundation::GetLastError()
                == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
        {
            return Ok(());
        }

        let _ = START_MS.set(std::time::Instant::now());
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hmodule = GetModuleHandleW(None)?;
        let hinst: windows::Win32::Foundation::HINSTANCE = hmodule.into();
        let class_name = w!("TempmonWidget");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..zeroed()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            class_name,
            w!("GLM桌面温度计"),
            WS_POPUP,
            0,
            0,
            420,
            30,
            None,
            None,
            Some(hinst),
            None,
        )?;

        let cfg = load_config();
        let bg_mode0 = cfg.bg_mode;
        let app = Box::new(App::new(hwnd, cfg)?);
        let ptr = Box::into_raw(app);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);

        // 圆角：普通模式用区域裁剪（无投影）；毛玻璃（Win11 雾面层不吃 rgn 裁剪）
        // 用 DWM 系统圆角，并叠加属性尽量去除自带的投影
        let mut dwm_round_ok = false;
        if bg_mode0 >= 2 {
            let pref = windows::Win32::Graphics::Dwm::DWMWCP_ROUND;
            let hr = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(33),
                &pref as *const _ as *const core::ffi::c_void,
                4,
            );
            dwm_round_ok = hr.is_ok();
            set_no_shadow(hwnd);
        }
        if !dwm_round_ok {
            let rgn = CreateRoundRectRgn(0, 0, 421, 41, 16, 16);
            let _ = windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, Some(rgn), true);
        }
        (*ptr).dwm_round_ok = dwm_round_ok;
        (*ptr).apply_bg_effect();

        TOPMOST_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_hook),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        // 事件驱动防遮挡：监听其他窗口的位置变化 / 显示 / 隐藏 / 前台切换
        for (lo, hi) in [
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        ] {
            SetWinEventHook(
                lo,
                hi,
                None,
                Some(dodge_event_hook),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
        }
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(hinst), 0);
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(hinst), 0);

        // 窗口图标（嵌入的 ico 资源，ID 1）
        {
            let icon = windows::Win32::UI::WindowsAndMessaging::LoadImageW(
                Some(hinst),
                PCWSTR::from_raw(1 as _),
                windows::Win32::UI::WindowsAndMessaging::IMAGE_ICON,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::LR_DEFAULTSIZE,
            )
            .unwrap_or_default();
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                hwnd,
                0x0080, // WM_SETICON
                Some(WPARAM(1)), // ICON_BIG
                Some(LPARAM(icon.0 as isize)),
            );
        }
        (*ptr).tick()?;
        // 显示前预定位（在首次渲染后，窗口宽度已定）：同步等一次 UIA 查询
        // （启动仅此一次），窗口一出现就在"最靠右 + 标题栏对齐"的位置上，
        // 不再"先出现在旧位置、几秒后才挪过去"
        unsafe { (*ptr).initial_place() };
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), TIMER_ID, (*ptr).refresh_ms, None);

        let mut msg = zeroed();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

// ── App ──

impl App {
    fn new(hwnd: HWND, cfg: Config) -> windows::core::Result<Self> {
        unsafe {
            let scale = GetDpiForWindow(hwnd) as f32 / 96.0;

            let d2d: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let rt = d2d.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                ..zeroed()
            })?;

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let format = dwrite.CreateTextFormat(
                w!("Microsoft YaHei UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                FONT_PX * scale,
                w!("zh-CN"),
            )?;

            let job = CreateJobObjectW(None, PCWSTR::null())?;
            let mut info = zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;

            let mapping = CreateFileMappingW(
                windows::Win32::Foundation::HANDLE::default(),
                None,
                PAGE_READWRITE,
                0,
                frame_len() as u32,
                w!("Local\\TempmonFrame"),
            )?;
            let view_ptr = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, frame_len());

            let mut app = Self {
                hwnd,
                job,
                view: view_ptr.Value as *const u8,
                child: None,
                last_seq: 0,
                last_seq_change: None,
                child_spawned_at: None,
                child_produced: false,
                respawn_streak: 0,
                respawn_not_before: None,
                strategy: strategy::Strategy::new(),
                hidden: false,
                collapsed: cfg.collapsed,
                alpha: cfg.alpha,
                pos: cfg.pos,
                capsule_items: cfg.capsule_items,
                row_items: cfg.row_items,
                rotate_ms: cfg.rotate_ms,
                refresh_ms: cfg.refresh_ms,
                bg_mode: cfg.bg_mode,
                click_pos: None,
                clickthrough_state: true,
                trim_tick: 540,
                h_gpu_usage: Hold::default(),
                h_gpu_vram: Hold::default(),
                h_gpu_temp: Hold::default(),
                h_cpu_temp: Hold::default(),
                h_disk: Hold::default(),
                h_fans: Hold::default(),
                capsule_page: 0,
                last_page_rot: None,
                last_fg_hwnd: 0,
                uia_distrust: None,
                occ_streak: 0,
                sync_ty_hist: [0; 3],
                sync_ty_n: 0,
                dragging: false,
                d2d,
                rt,
                dwrite,
                format,
                scale,
                dc_mem: HDC::default(),
                hbmp: None,
                hbmp_old: HGDIOBJ::default(),
        buf_w: 0,
        buf_h: 0,
        dwm_round_ok: false,
        fade: 1.0,
        fade_phase: 0,
        fade_x: 0,
        fade_y: 0,
        dodging: false,
        last_dodge: None,
        widest: cfg.widest,
        last_checked_w: 0,
        rendered_once: false,
        last_rendered: None,
        dodge_secs: cfg.dodge_secs,
        user_pinned: cfg.pinned,
        started_at: Instant::now(),
        cap: None,
        dctx: None,
        fb_x: 0,
        free_streak: 0,
        ret_cooldown_until: None,
        last_sync_check: None,
        fb_d: 0.0,
        fb_runs: 0,
            };
            app.ensure_sensor_alive();
            Ok(app)
        }
    }

    fn ensure_sensor_alive(&mut self) {
        // 重拉退避：上一个子进程从未产出过帧（启动即崩）时按倍数拉长间隔，
        // 避免环境性故障导致每 6 秒无限拉起进程（连带 bridge 生灭）
        if let Some(t) = self.respawn_not_before {
            if Instant::now() < t {
                return;
            }
        }
        // 卡死判定用真实时间而非 tick 计数：避让淡入淡出每 16ms 手动调一次
        // tick，按计数会把一帧的正常等待（1s）误判成 6 次卡死，导致每次
        // 避让都杀掉并重拉 sensor（也是当年孤儿 bridge 泄漏的总根源）
        let stale = if self.view.is_null() || self.child.is_none() {
            // 首次拉起，或上一只子进程已确认死亡
            true
        } else {
            let seq = frame_seq(self.view);
            if seq != self.last_seq {
                self.child_produced = true;
                self.last_seq_change = Some(Instant::now());
                self.last_seq = seq;
            }
            match self.last_seq_change {
                // 出过帧后帧间隔超 8s 判卡死；从未出帧则等 20s（冷启动余量）
                Some(t) => t.elapsed() > std::time::Duration::from_secs(8),
                None => self
                    .child_spawned_at
                    .is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(20)),
            }
        };
        if !stale {
            return;
        }
        if self.debug_on() {
            eprintln!(
                "[sensor-watch] stale at +{:.1}s: frozen_for={:?} last_seq={} produced={} streak={}",
                (Instant::now() - self.started_at).as_secs_f32(),
                self.last_seq_change.map(|t| t.elapsed()),
                self.last_seq,
                self.child_produced,
                self.respawn_streak,
            );
        }
        if let Some(mut old) = self.child.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        if self.child_produced {
            self.respawn_streak = 0;
        } else {
            self.respawn_streak = (self.respawn_streak + 1).min(5);
        }
        if let Ok(exe) = std::env::current_exe() {
            match std::process::Command::new(exe)
                .env("TEMPMON_SENSOR", "1")
                .creation_flags(0x0800_0000)
                .spawn()
            {
                Ok(child) => {
                    unsafe {
                        let _ = AssignProcessToJobObject(self.job, HANDLE(child.as_raw_handle() as _));
                    }
                    self.child = Some(child);
                    self.child_spawned_at = Some(Instant::now());
                    self.child_produced = false;
                }
                Err(e) => {
                    if self.debug_on() {
                        eprintln!("[sensor-watch] spawn FAILED: {e}");
                    }
                }
            }
            let delay = std::time::Duration::from_secs(6 << self.respawn_streak.min(4));
            self.respawn_not_before = Some(Instant::now() + delay);
        }
    }

    fn tick(&mut self) -> windows::core::Result<()> {
        if self.dragging {
            // 拖拽期间冻结翻页计时，松手后不会立刻跳页
            self.last_page_rot = Some(Instant::now());
            return Ok(());
        }
        if self.collapsed && self.fade_phase == 0 {
            // 按真实时间翻页：避让淡入淡出期间（fade_phase!=0，16ms 一次手动
            // tick）不再翻页——旧实现按 tick 计数，动画期间页面飞速轮换，
            // 窗口宽度随 CPU/GPU 页反复伸缩
            let due = self
                .last_page_rot
                .map_or(true, |t| t.elapsed().as_millis() as u32 >= self.rotate_ms);
            if due {
                self.last_page_rot = Some(Instant::now());
                self.capsule_page = self.capsule_page.wrapping_add(1);
            }
        }
        self.update_clickthrough();
        self.ensure_sensor_alive();

        // 周期性工作集修剪（约每 10 分钟）：让 OS 换出冷页，
        // 任务管理器占用常年保持低位；悬停/拖拽期间跳过避免微小卡顿
        self.trim_tick += 1;
        if self.trim_tick >= 600 && self.clickthrough_state && !self.dragging {
            self.trim_tick = 0;
            unsafe {
                let _ = windows::Win32::System::Threading::SetProcessWorkingSetSize(
                    windows::Win32::System::Threading::GetCurrentProcess(),
                    usize::MAX,
                    usize::MAX,
                );
            }
        }

        // 事件驱动防遮挡（dodge_secs>0 时启用）：
        // 其他窗口移动/显示/隐藏/前台切换，或自身宽度变化（翻页/形态切换）时立即检测；
        // 另加 2.5s 兜底轮询：覆盖不产生窗口事件的场景（后台窗口加载内容、
        // 非激活窗口还原/变化等）。last_dodge 为避让后的防震荡冷却（未来时刻）
        if self.dodge_secs > 0 {
            let event = DODGE_EVENT.swap(false, std::sync::atomic::Ordering::Relaxed);
            let mut width_changed = false;
            unsafe {
                let mut r = RECT::default();
                if GetWindowRect(self.hwnd, &mut r).is_ok() && r.right - r.left != self.last_checked_w
                {
                    width_changed = true;
                }
            }
            let now_ms = START_MS
                .get_or_init(std::time::Instant::now)
                .elapsed()
                .as_millis() as usize;
            let stale = now_ms.saturating_sub(DODGE_LAST_CHECK_MS.load(std::sync::atomic::Ordering::Relaxed))
                >= DODGE_SWEEP_MS;
            if event || width_changed || stale {
                self.dodge_if_occluding();
            }
        }

        let hide = self.strategy.should_hide();
        if hide != self.hidden {
            self.hidden = hide;
            unsafe {
                let _ = ShowWindow(self.hwnd, if hide { SW_HIDE } else { SW_SHOWNOACTIVATE });
            }
        }

        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }

        let mut snap = if self.view.is_null() {
            Snapshot::default()
        } else {
            read_frame(self.view).unwrap_or_default()
        };
        snap.gpu_usage = hold(&mut self.h_gpu_usage, snap.gpu_usage);
        snap.gpu_vram_pct = hold(&mut self.h_gpu_vram, snap.gpu_vram_pct);
        snap.gpu_temp = hold(&mut self.h_gpu_temp, snap.gpu_temp);
        snap.cpu_temp = hold(&mut self.h_cpu_temp, snap.cpu_temp);
        snap.disk_temps = self.h_disk.set_vec(snap.disk_temps);
        snap.fans = self.h_fans.set_vec(snap.fans);
        // 脏检查：数值、页码、透明度、隐藏态全都没变且避让动画不在进行时，
        // 跳过整帧排版+重绘（温度数据 1s 才动一点，多数 tick 是纯浪费）
        if self.fade_phase == 0
            && !self.hidden
            && self.last_rendered.as_ref() == Some(&(snap.clone(), self.capsule_page, self.alpha))
        {
            return Ok(());
        }
        self.last_rendered = Some((snap.clone(), self.capsule_page, self.alpha));
        self.render(&snap)
    }

    fn update_clickthrough(&mut self) -> windows::core::Result<()> {
        let ctrl = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0 };
        let want_transparent = !ctrl;
        if want_transparent != self.clickthrough_state {
            self.clickthrough_state = want_transparent;
            unsafe {
                let cur = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
                let next = if want_transparent {
                    cur | WS_EX_TRANSPARENT.0
                } else {
                    cur & !WS_EX_TRANSPARENT.0
                };
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, next as isize);
            }
        }
        Ok(())
    }

    fn build_parts(&self, s: &Snapshot) -> Vec<Part> {
        let light = self.bg_mode == 1 || self.bg_mode == 3;
        let gray = if light {
            D2D1_COLOR_F { r: 0.35, g: 0.38, b: 0.45, a: 0.95 }
        } else {
            D2D1_COLOR_F { r: 0.70, g: 0.70, b: 0.70, a: 0.95 }
        };
        let white = if light {
            D2D1_COLOR_F { r: 0.08, g: 0.10, b: 0.14, a: 1.0 }
        } else {
            D2D1_COLOR_F { r: 0.97, g: 0.97, b: 0.97, a: 1.0 }
        };
        let blue = if light {
            D2D1_COLOR_F { r: 0.05, g: 0.35, b: 0.85, a: 1.0 }
        } else {
            D2D1_COLOR_F { r: 0.45, g: 0.72, b: 1.00, a: 1.0 }
        };
        let purple = if light {
            D2D1_COLOR_F { r: 0.55, g: 0.25, b: 0.85, a: 1.0 }
        } else {
            D2D1_COLOR_F { r: 0.78, g: 0.58, b: 1.00, a: 1.0 }
        };
        let sep = if light {
            D2D1_COLOR_F { r: 0.60, g: 0.62, b: 0.68, a: 0.7 }
        } else {
            D2D1_COLOR_F { r: 0.45, g: 0.45, b: 0.50, a: 0.7 }
        };

        let mut parts: Vec<Part> = Vec::new();
        if self.row_items & ROW_CPU != 0 {
            parts.push(Part::new("CPU ".into(), gray));
            parts.push(Part::val(format!("{:.0}%", s.cpu_usage), white, "100%"));
            if let Some(t) = s.cpu_temp {
                parts.push(Part::val(format!(" {:.0}°", t), temp_color(t, light), " 100°"));
            }
        }
        if self.row_items & ROW_GPU != 0
            && (s.gpu_usage.is_some() || s.gpu_vram_pct.is_some() || s.gpu_temp.is_some())
        {
            parts.push(Part { text: " │ ".into(), color: sep, slot: None, dot: false });
            parts.push(Part::new("GPU ".into(), gray));
            if let Some(u) = s.gpu_usage {
                parts.push(Part::val(format!("{:.0}%", u), blue, "100%"));
            }
            if let Some(t) = s.gpu_temp {
                parts.push(Part::val(format!(" {:.0}°", t), temp_color(t, light), " 100°"));
            }
            if let Some(v) = s.gpu_vram_pct {
                parts.push(Part { text: "   VR ".into(), color: gray, slot: None, dot: false });
                parts.push(Part::val(format!("{:.0}%", v), purple, "100%"));
            }
        }
        if self.row_items & ROW_MEM != 0 && s.mem_total_gb > 0.0 {
            parts.push(Part { text: " │ ".into(), color: sep, slot: None, dot: false });
            parts.push(Part::new("MEM ".into(), gray));
            parts.push(Part::val(format!("{:.0}%", s.mem_used_pct), white, "100%"));
        }
        if self.row_items & ROW_DISK != 0 && !s.disk_temps.is_empty() {
            parts.push(Part { text: " │ ".into(), color: sep, slot: None, dot: false });
            parts.push(Part::new("DISK ".into(), gray));
            for t in &s.disk_temps {
                parts.push(Part::val(format!("{:.0}° ", t), temp_color(*t, light), "100° "));
            }
        }
        if self.row_items & ROW_FAN != 0 && !s.fans.is_empty() {
            parts.push(Part { text: " │ ".into(), color: sep, slot: None, dot: false });
            parts.push(Part::new("FAN ".into(), gray));
            for r in &s.fans {
                parts.push(Part::val(format!("{:.0} ", r), white, "9999 "));
            }
        }
        parts
    }

    fn capsule_pages(&self, s: &Snapshot) -> Vec<Vec<Part>> {
        let light = self.bg_mode == 1 || self.bg_mode == 3;
        // 温度取 GPU/硬盘/CPU 最高；阈值按本机日常状态校准（NVMe 待机 60° 左右属正常）
        let tmax: Option<f32> = s
            .gpu_temp
            .into_iter()
            .chain(s.disk_temps.iter().copied())
            .chain(s.cpu_temp.into_iter())
            .fold(None::<f32>, |a, b| a.map(|x| x.max(b)).or(Some(b)));
        let load = [s.cpu_usage, s.gpu_usage.unwrap_or(0.0), s.mem_used_pct]
            .into_iter()
            .fold(0.0f32, f32::max);
        let dot = if tmax.map_or(false, |t| t >= 85.0) || load >= 90.0 {
            D2D1_COLOR_F { r: 1.0, g: 0.32, b: 0.32, a: 1.0 } // 红：过热/满载
        } else if tmax.map_or(false, |t| t >= 78.0) || load >= 80.0 {
            D2D1_COLOR_F { r: 1.0, g: 0.72, b: 0.20, a: 1.0 } // 橙：偏高
        } else if tmax.map_or(false, |t| t >= 68.0) || load >= 65.0 {
            D2D1_COLOR_F { r: 1.0, g: 0.90, b: 0.30, a: 1.0 } // 黄：轻度繁忙
        } else {
            D2D1_COLOR_F { r: 0.36, g: 0.85, b: 0.45, a: 1.0 } // 绿：日常正常
        };
        let gray = if light {
            D2D1_COLOR_F { r: 0.35, g: 0.38, b: 0.45, a: 0.95 }
        } else {
            D2D1_COLOR_F { r: 0.70, g: 0.70, b: 0.70, a: 0.95 }
        };
        let white = if light {
            D2D1_COLOR_F { r: 0.08, g: 0.10, b: 0.14, a: 1.0 }
        } else {
            D2D1_COLOR_F { r: 0.97, g: 0.97, b: 0.97, a: 1.0 }
        };
        let blue = if light {
            D2D1_COLOR_F { r: 0.05, g: 0.35, b: 0.85, a: 1.0 }
        } else {
            D2D1_COLOR_F { r: 0.45, g: 0.72, b: 1.00, a: 1.0 }
        };
        let purple = if light {
            D2D1_COLOR_F { r: 0.55, g: 0.25, b: 0.85, a: 1.0 }
        } else {
            D2D1_COLOR_F { r: 0.78, g: 0.58, b: 1.00, a: 1.0 }
        };
        let it = self.capsule_items;

        let mut pages: Vec<Vec<Part>> = Vec::new();

        if it & CAPS_MAXTEMP != 0 {
            let mut pg = vec![Part::dot(dot)];
            match tmax {
                Some(t) => pg.push(Part::val(format!("{:.0}°", t), white, "100°")),
                None => pg.push(Part::new("--".into(), gray)),
            }
            pages.push(pg);
        }

        if it & (CAPS_CPU | CAPS_CPUTEMP) != 0 {
            let mut pg = vec![Part::dot(dot), Part::new("CPU ".into(), gray)];
            if it & CAPS_CPU != 0 {
                pg.push(Part::val(format!("{:.0}%", s.cpu_usage), white, "100%"));
            }
            if it & CAPS_CPUTEMP != 0 {
                if let Some(t) = s.cpu_temp {
                    pg.push(Part::val(format!(" {:.0}°", t), temp_color(t, light), " 100°"));
                }
            }
            if pg.len() > 2 {
                pages.push(pg);
            }
        }

        if it & (CAPS_GPU | CAPS_GPUTEMP | CAPS_VRAM) != 0 {
            let mut pg = vec![Part::dot(dot), Part::new("GPU ".into(), gray)];
            if it & CAPS_GPU != 0 {
                if let Some(u) = s.gpu_usage {
                    pg.push(Part::val(format!("{:.0}%", u), blue, "100%"));
                }
            }
            if it & CAPS_GPUTEMP != 0 {
                if let Some(t) = s.gpu_temp {
                    pg.push(Part::val(format!(" {:.0}°", t), temp_color(t, light), " 100°"));
                }
            }
            if it & CAPS_VRAM != 0 {
                if let Some(v) = s.gpu_vram_pct {
                    pg.push(Part::new("   VR ".into(), gray));
                    pg.push(Part::val(format!("{:.0}%", v), purple, "100%"));
                }
            }
            if pg.len() > 2 {
                pages.push(pg);
            }
        }

        if it & CAPS_MEM != 0 && s.mem_total_gb > 0.0 {
            pages.push(vec![
                Part::dot(dot),
                Part::new("MEM ".into(), gray),
                Part::val(format!("{:.0}%", s.mem_used_pct), white, "100%"),
            ]);
        }

        if it & CAPS_DISK != 0 && !s.disk_temps.is_empty() {
            let mut pg = vec![Part::dot(dot), Part::new("DISK ".into(), gray)];
            for t in &s.disk_temps {
                pg.push(Part::val(format!("{:.0}° ", t), temp_color(*t, light), "100° "));
            }
            pages.push(pg);
        }

        if it & CAPS_FAN != 0 && !s.fans.is_empty() {
            let mut pg = vec![Part::dot(dot), Part::new("FAN ".into(), gray)];
            for r in &s.fans {
                pg.push(Part::val(format!("{:.0} ", r), white, "9999 "));
            }
            pages.push(pg);
        }

        if pages.is_empty() {
            pages.push(vec![Part::dot(dot)]);
        }
        pages
    }

    fn render(&mut self, s: &Snapshot) -> windows::core::Result<()> {
        unsafe {
            let rt = self.rt.clone();
            let dwrite = self.dwrite.clone();

            let pages: Vec<Vec<Part>> = if self.collapsed {
                self.capsule_pages(s)
            } else {
                vec![self.build_parts(s)]
            };

            let mut page_laid: Vec<(Vec<(Part, IDWriteTextLayout, f32, f32)>, f32)> = Vec::new();
            let mut text_h = 0.0f32;
            for pg in &pages {
                let mut laid: Vec<(Part, IDWriteTextLayout, f32, f32)> = Vec::new();
                let mut total = 0.0f32;
                for p in pg {
                    let wide: Vec<u16> = p.text.encode_utf16().collect();
                    let layout =
                        dwrite.CreateTextLayout(&wide, &self.format, 10000.0, 10000.0)?;
                    let mut m = zeroed();
                    layout.GetMetrics(&mut m)?;
                    let w0 = m.widthIncludingTrailingWhitespace;
                    let slot_w = if p.dot {
                        12.0 * self.scale
                    } else {
                        match &p.slot {
                            Some(ph) => {
                                let pw: Vec<u16> = ph.encode_utf16().collect();
                                let pl =
                                    dwrite.CreateTextLayout(&pw, &self.format, 10000.0, 10000.0)?;
                                let mut pm = zeroed();
                                pl.GetMetrics(&mut pm)?;
                                pm.widthIncludingTrailingWhitespace.max(w0)
                            }
                            None => w0,
                        }
                    };
                    text_h = text_h.max(m.height);
                    total += slot_w + GAP * self.scale;
                    laid.push((p.clone(), layout, w0, slot_w));
                }
                page_laid.push((laid, total));
            }
            if page_laid.is_empty() {
                return Ok(());
            }
            // 所有翻页的总宽每帧都已排版算出：取最大者作为最宽形态宽度，
            // 防遮挡检测无需等宽页轮播显示就能覆盖真实占位
            let max_total = page_laid.iter().map(|(_, t)| *t).fold(0.0f32, f32::max);
            let page_idx = self.capsule_page as usize % page_laid.len();
            let (laid, parts_total) = if self.collapsed {
                let (l, t) = page_laid[page_idx].clone();
                (l, t)
            } else {
                let (l, t) = page_laid.remove(0);
                (l, t)
            };

            let bar_h = if self.collapsed { CAP_H } else { BAR_H };
            let h = (bar_h * self.scale).ceil() as i32;

            // 右侧空间不足时优先显示左边的内容：截掉放不下的尾部项目
            let mut wa = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut RECT as _),
                windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            let m = (EDGE_MARGIN * self.scale) as i32;
            let pos = self.pos.unwrap_or(POINT { x: wa.right - 420, y: wa.top + m });
            let avail = ((wa.right - m) - pos.x) as f32;
            let mut laid = laid;
            let mut parts_total = parts_total;
            let pad2 = PAD_X * 2.0 * self.scale;
            let budget = avail - pad2;
            if self.pos.is_some() && budget > 20.0 && parts_total > budget {
                let mut acc = 0.0f32;
                let mut keep = 0usize;
                for (i, (_, _, _, slot_w)) in laid.iter().enumerate() {
                    let add = slot_w + if keep > 0 { GAP * self.scale } else { 0.0 };
                    if acc + add <= budget {
                        acc += add;
                        keep = i + 1;
                    } else {
                        break;
                    }
                }
                laid.truncate(keep.max(1));
                parts_total = acc;
            }

            // 渲染宽度跟随当前页（胶囊随翻页变宽变窄；被右侧裁剪时以裁剪后为准）
            let w = ((parts_total - GAP * self.scale) + pad2).ceil() as i32;
            // 防遮挡检测则按所有翻页中的最大宽度计算占位，
            // 无需等宽页轮播显示就不会在翻到宽页时压住内容
            let widest_w = ((max_total - GAP * self.scale) + PAD_X * 2.0 * self.scale).ceil() as i32;
            if widest_w > self.widest {
                self.widest = widest_w;
            }

            let mut pos = if let Some(p) = self.pos {
                p
            } else {
                POINT { x: wa.right - w - m, y: wa.top + m }
            };
            // 右缘放不下时整体左移（靠左完整显示）；左缘越界同理夹回
            let right_lim = wa.right - m;
            if pos.x + w > right_lim {
                pos.x = right_lim - w;
            }
            if pos.x < wa.left {
                pos.x = wa.left;
            }

            if self.buf_w != w || self.buf_h != h {
                self.recreate_buffer(w, h)?;
            }

            rt.BindDC(self.dc_mem, &RECT { left: 0, top: 0, right: w, bottom: h })?;
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));

            let bg: ID2D1SolidColorBrush = match self.bg_mode {
                1 => rt.CreateSolidColorBrush(
                    &D2D1_COLOR_F { r: 0.97, g: 0.98, b: 1.00, a: 0.78 },
                    None,
                )?,
                2 => rt.CreateSolidColorBrush(
                    &D2D1_COLOR_F { r: 0.06, g: 0.07, b: 0.10, a: 0.30 },
                    None,
                )?,
                3 => rt.CreateSolidColorBrush(
                    &D2D1_COLOR_F { r: 0.97, g: 0.98, b: 1.00, a: 0.35 },
                    None,
                )?,
                _ => rt.CreateSolidColorBrush(
                    &D2D1_COLOR_F { r: 0.04, g: 0.04, b: 0.06, a: 0.62 },
                    None,
                )?,
            };
            rt.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: 0.0, top: 0.0, right: w as f32, bottom: h as f32 },
                    radiusX: 8.0 * self.scale,
                    radiusY: 8.0 * self.scale,
                },
                &bg,
            );

            // 普通模式用区域裁剪保持圆角（无投影）；毛玻璃走 DWM 系统圆角
            if self.bg_mode < 2 || !self.dwm_round_ok {
                let rgn = CreateRoundRectRgn(
                    0,
                    0,
                    w + 1,
                    h + 1,
                    (8.0 * self.scale) as i32 + 1,
                    (8.0 * self.scale) as i32 + 1,
                );
                let _ = windows::Win32::Graphics::Gdi::SetWindowRgn(self.hwnd, Some(rgn), true);
            }

            let mut x = PAD_X * self.scale;
            let y = (h as f32 - text_h) / 2.0;
            for (p, layout, w0, slot_w) in &laid {
                let brush = rt.CreateSolidColorBrush(&p.color, None)?;
                if p.dot {
                    let r = 2.8 * self.scale;
                    rt.FillEllipse(
                        &windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                            point: Vector2 { X: x + r, Y: h as f32 / 2.0 },
                            radiusX: r,
                            radiusY: r,
                        },
                        &brush,
                    );
                } else {
                    rt.DrawTextLayout(
                        Vector2 { X: x + (slot_w - w0), Y: y },
                        layout,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
                x += slot_w + GAP * self.scale;
            }
            rt.EndDraw(None, None)?;

            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: ((self.alpha as f32 * self.fade) as i32).clamp(0, 255) as u8,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let screen = GetDC(None);
            let _ = UpdateLayeredWindowSafe(
                self.hwnd,
                screen,
                &pos,
                &SIZE { cx: w, cy: h },
                self.dc_mem,
                &POINT { x: 0, y: 0 },
                &blend,
            );
            ReleaseDC(None, screen);

            self.rendered_once = true;
            Ok(())
        }
    }

    fn recreate_buffer(&mut self, w: i32, h: i32) -> windows::core::Result<()> {
        unsafe {
            if !self.dc_mem.is_invalid() {
                if !self.hbmp_old.is_invalid() {
                    SelectObject(self.dc_mem, self.hbmp_old);
                }
                if let Some(b) = self.hbmp.take() {
                    let _ = DeleteObject(b.into());
                }
                let _ = DeleteDC(self.dc_mem);
            }
            let screen = GetDC(None);
            let dc_mem = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0,
                    ..zeroed()
                },
                ..zeroed()
            };
            let mut bits: *mut core::ffi::c_void = null_mut();
            let hbmp = CreateDIBSection(Some(dc_mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old = SelectObject(dc_mem, hbmp.into());

            self.dc_mem = dc_mem;
            self.hbmp = Some(hbmp);
            self.hbmp_old = old;
            self.buf_w = w;
            self.buf_h = h;
            Ok(())
        }
    }

    unsafe fn show_menu(&self) {
        if let Ok(menu) = CreatePopupMenu() {
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_COLLAPSE,
                if self.collapsed { w!("展开为完整条") } else { w!("收起为小胶囊") },
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            if let Ok(sub) = CreatePopupMenu() {
                for (id, label, want) in [
                    (MENU_BG_BASE, w!("深色"), 0u8),
                    (MENU_BG_BASE + 1, w!("浅色"), 1u8),
                    (MENU_BG_BASE + 2, w!("深色毛玻璃"), 2u8),
                    (MENU_BG_BASE + 3, w!("浅色毛玻璃"), 3u8),
                ] {
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING | if self.bg_mode == want { MF_CHECKED } else { MF_UNCHECKED },
                        id,
                        label,
                    );
                }
                let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("背景颜色"));
            }
            if let Ok(sub) = CreatePopupMenu() {
                for ((id, label), v) in [
                    (MENU_ALPHA_25, w!("25%")),
                    (MENU_ALPHA_40, w!("40%")),
                    (MENU_ALPHA_55, w!("55%")),
                    (MENU_ALPHA_70, w!("70%")),
                    (MENU_ALPHA_85, w!("85%")),
                    (MENU_ALPHA_100, w!("100%")),
                ]
                .into_iter()
                .zip(ALPHA_CHOICES)
                {
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING | if self.alpha == v { MF_CHECKED } else { MF_UNCHECKED },
                        id,
                        label,
                    );
                }
                let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("透明度"));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            if let Ok(sub) = CreatePopupMenu() {
                let opts: [(usize, PCWSTR, u32); 5] = [
                    (0, w!("CPU 段"), ROW_CPU),
                    (1, w!("GPU 段"), ROW_GPU),
                    (2, w!("内存段"), ROW_MEM),
                    (3, w!("硬盘段"), ROW_DISK),
                    (4, w!("风扇段"), ROW_FAN),
                ];
                for (i, label, bit) in opts {
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING
                            | if self.row_items & bit != 0 { MF_CHECKED } else { MF_UNCHECKED },
                        MENU_ROW_BASE + i,
                        label,
                    );
                }
                let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("显示内容"));
            }
            if let Ok(sub) = CreatePopupMenu() {
                let opts: [(usize, PCWSTR, u32); 9] = [
                    (0, w!("最高温度"), CAPS_MAXTEMP),
                    (1, w!("CPU 占用"), CAPS_CPU),
                    (7, w!("CPU 温度"), CAPS_CPUTEMP),
                    (2, w!("GPU 占用"), CAPS_GPU),
                    (3, w!("GPU 温度"), CAPS_GPUTEMP),
                    (4, w!("显存占用"), CAPS_VRAM),
                    (5, w!("内存占用"), CAPS_MEM),
                    (6, w!("硬盘温度"), CAPS_DISK),
                    (8, w!("风扇转速"), CAPS_FAN),
                ];
                for (i, label, bit) in opts {
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING
                            | if self.capsule_items & bit != 0 { MF_CHECKED } else { MF_UNCHECKED },
                        MENU_CAPSULE_BASE + i,
                        label,
                    );
                }
                let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("胶囊显示项"));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            if let Ok(sub) = CreatePopupMenu() {
                for (i, ms) in ROTATE_CHOICES_MS.iter().enumerate() {
                    let label = match ms {
                        1000 => w!("1 秒"),
                        2000 => w!("2 秒"),
                        3000 => w!("3 秒"),
                        _ => w!("5 秒"),
                    };
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING
                            | if self.rotate_ms == *ms { MF_CHECKED } else { MF_UNCHECKED },
                        MENU_ROTATE_BASE + i,
                        label,
                    );
                }
                let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("轮播间隔"));
            }
            if let Ok(sub) = CreatePopupMenu() {
                for (i, ms) in REFRESH_CHOICES_MS.iter().enumerate() {
                    let label = match ms {
                        500 => w!("500 毫秒"),
                        1000 => w!("1 秒"),
                        _ => w!("2 秒"),
                    };
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING
                            | if self.refresh_ms == *ms { MF_CHECKED } else { MF_UNCHECKED },
                        MENU_REFRESH_BASE + i,
                        label,
                    );
                }
                let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("刷新频率"));
            }
            if let Ok(sub) = CreatePopupMenu() {
                for (i, s) in DODGE_CHOICES_SECS.iter().enumerate() {
                    let label = if *s == 0 { w!("关闭") } else { w!("开启") };
                    let on = self.dodge_secs > 0;
                    let checked = if *s == 0 { !on } else { on };
                    let _ = AppendMenuW(
                        sub,
                        MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED },
                        MENU_DODGE_BASE + i,
                        label,
                    );
                }
                let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("防遮挡检测"));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

            let _ = AppendMenuW(
                menu,
                MF_STRING | if autostart_enabled() { MF_CHECKED } else { MF_UNCHECKED },
                MENU_AUTOSTART,
                w!("开机自启"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING | if self.pos.is_some() { MF_ENABLED } else { MF_GRAYED },
                MENU_RESET_POS,
                w!("恢复吸附右上角"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING | if self.user_pinned { MF_CHECKED } else { MF_UNCHECKED },
                MENU_PIN,
                w!("固定位置"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("退出"));

            let mut r = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut r);
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mut wa2 = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa2 as *mut RECT as _),
                windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            let mut mx = pt.x;
            if mx < wa2.left + 8 {
                mx = wa2.left + 8;
            }
            if mx > wa2.right - 180 {
                mx = wa2.right - 180;
            }
            let _ = SetForegroundWindow(self.hwnd);
            let _ = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON,
                mx,
                r.bottom + 4,
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
        }
    }

    fn apply_bg_effect(&mut self) {
        unsafe {
            let acrylic = self.bg_mode >= 2;
            apply_acrylic(self.hwnd, acrylic, self.bg_mode == 3);
            if acrylic {
                // DWM 圆角只在启动时背景为毛玻璃的分支里尝试过——从普通模式
                // 切到毛玻璃会永远拿不到圆角。切换时当场尝试，失败再退区域裁剪
                let pref = windows::Win32::Graphics::Dwm::DWMWCP_ROUND;
                let hr = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                    self.hwnd,
                    windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(33),
                    &pref as *const _ as *const core::ffi::c_void,
                    4,
                );
                self.dwm_round_ok = hr.is_ok();
                if self.dwm_round_ok {
                    let _ = windows::Win32::Graphics::Gdi::SetWindowRgn(self.hwnd, None, true);
                    set_no_shadow(self.hwnd);
                } else {
                    let rgn = CreateRoundRectRgn(0, 0, 421, 41, 16, 16);
                    let _ =
                        windows::Win32::Graphics::Gdi::SetWindowRgn(self.hwnd, Some(rgn), true);
                }
            } else {
                let rgn = CreateRoundRectRgn(0, 0, 421, 41, 16, 16);
                let _ = windows::Win32::Graphics::Gdi::SetWindowRgn(self.hwnd, Some(rgn), true);
            }
        }
    }

    fn toggle_collapse(&mut self) {
        self.collapsed = !self.collapsed;
        self.widest = 0;
        save_config(&self.as_config());
    }

    fn set_alpha(&mut self, alpha: u8) {
        self.alpha = alpha;
        save_config(&self.as_config());
    }

    /// 发起防遮挡检测（请求阶段，不阻塞 UI 线程）：
    /// 计算检测区域并设置截图排除，实际截屏在 45ms 定时器回调中执行
    fn dodge_if_occluding(&mut self) {
        // 首次渲染前窗口还在初始位置（配置位置尚未应用），不检测不覆盖配置
        if self.dragging || self.hidden || self.dodging || !self.rendered_once {
            return;
        }
        // 编辑态（按住 Ctrl）或光标正在靠近时暂停避让——别躲着用户跑
        if !self.clickthrough_state {
            return;
        }
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mut r = RECT::default();
            if GetWindowRect(self.hwnd, &mut r).is_ok() {
                let pad = 40;
                if pt.x >= r.left - pad
                    && pt.x <= r.right + pad
                    && pt.y >= r.top - pad
                    && pt.y <= r.bottom + pad
                {
                    return;
                }
            }
        }
        if self.cap.is_some() {
            return; // 上一次截取尚未完成
        }
        let debug = std::env::var("TEMPMON_DODGE_DEBUG").is_ok();
        if debug {
            eprintln!("[{}] [dodge] check at +{:.1}s", wallclock(), (Instant::now() - self.started_at).as_secs_f32());
        }
        // 前台窗口切换 → 立即允许标题栏同步（burst 定时器 0.3s 内响应）
        unsafe {
            let fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
            let fg_id = if fg.is_invalid() { 0 } else { fg.0 as isize };
            if fg_id != self.last_fg_hwnd {
                if self.debug_on() {
                    eprintln!("[fg-change] {:?} -> {:?} (立即触发同步)", self.last_fg_hwnd, fg_id);
                }
                self.last_fg_hwnd = fg_id;
                self.last_sync_check = None;
                // 目标窗口变了：旧布局下的回归/避让冷却失去意义，清掉，
                // 否则避让移动后 30s 内的前台切换会被挡住贴右（x 卡在旧位）
                self.ret_cooldown_until = None;
                // 前台切换即后台发起 UIA 按钮查询，2s 内缓存可用
                uia_ensure_request(fg_id, true);
            }
        }
        // 节流：两次实际检测至少间隔 700ms（事件风暴/多来源触发时合并）
        let now_ms = START_MS
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis() as usize;
        if now_ms.saturating_sub(DODGE_LAST_CHECK_MS.load(std::sync::atomic::Ordering::Relaxed))
            < DODGE_THROTTLE_MS
        {
            return;
        }
        DODGE_LAST_CHECK_MS.store(now_ms, std::sync::atomic::Ordering::Relaxed);
        unsafe {
            let mut r = RECT::default();
            if GetWindowRect(self.hwnd, &mut r).is_err() {
                return;
            }
            let h = r.bottom - r.top;
            let cur_w = r.right - r.left;
            let max_w = self.widest.max(cur_w);
            let (x0, y) = (r.left, r.top);
            self.last_checked_w = cur_w;
            let mut wa = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut RECT as _),
                windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            // 温度计可能变宽到 max_w：手动定位过则左缘固定向右伸展，
            // 吸附默认态右缘固定向左伸展
            let left0 = if self.pos.is_some() { x0 } else { r.right - max_w };
            let cx = left0.max(wa.left);
            let cw = ((left0 + max_w).min(wa.right) - cx).max(0);
            // 外扩 16px：骑在占位边缘上的小图标（只压住一角）也能完整进块被检出
            let cw = (cw + 16).min(wa.right - cx).max(0);
            if cw <= 0 {
                return;
            }
            self.dctx = Some(DodgeCtx { x0, y, h, max_w, cur_w, left0, wa, ret_right: false });
            if !self.begin_capture(cx, y, cw, h, false, false) {
                self.dctx = None;
            }
        }
    }

    /// 自动模式的最靠右 x：贴着前台窗口标题栏按钮区左侧，
    /// 不压最小化/最大化/关闭按钮。前台窗口不够宽（小窗口）时不约束。
    unsafe fn auto_x_target(&self, cluster_left: Option<i32>) -> Option<i32> {
        if self.user_pinned {
            return None;
        }
        let fg = HWND(self.last_fg_hwnd as _);
        if self.last_fg_hwnd == 0 {
            return None;
        }
        let mut fr = RECT::default();
        if GetWindowRect(fg, &mut fr).is_err() {
            return None;
        }
        let mut wr = RECT::default();
        if GetWindowRect(self.hwnd, &mut wr).is_err() {
            return None;
        }
        let mut wa = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut wa as *mut RECT as _),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        // 前台窗口太窄（<60% 屏宽）时不贴它，保持常规选位
        if fr.right - fr.left < (wa.right - wa.left) * 3 / 5 {
            return None;
        }
        // 用最大页宽定位：轮播时窗口宽度在页间变化，若按当前宽算 x，
        // 每次翻页都会左右挪一下；按最宽页锁定左缘后 x 稳定不动
        let w = self.widest.max(wr.right - wr.left);
        // 贴按钮簇左缘（UIA 枚举的真实边界，ZCode 等应用附加按钮也在内）。
        // 查询还在跑（Pending）时不动——先按估算值跳一次、再按真实值跳一次
        // 就是"跳两跳"的来源；查询确认失败才退回标准三按钮的 160px 估算
        // UIA 不可信（过期矩形被判弃用/查不到）时优先用边缘图扫出的真实
        // 簇左缘；固定预留只作扫描失败的兜底
        let fb = || {
            cluster_left
                .map(|cl| (cl - 24).max(fr.right - 320))
                .unwrap_or(fr.right - CAPTION_FALLBACK)
        };
        let right_limit = if self.uia_distrusted() {
            fb()
        } else {
            match uia_anchor_state(self.last_fg_hwnd) {
                AnchorState::Ready(_, zone_left) => zone_left - CAPTION_GAP,
                AnchorState::Failed => fb(),
                AnchorState::Pending => return None,
            }
        };
        let x = right_limit - w;
        (x >= wa.left).then_some(x)
    }

    /// 启动预定位：非固定位置时，窗口显示前就把 y 对齐到前台窗口标题栏
    /// 按钮（同步等待 UIA 查询至多 ~400ms，超时则沿用旧位置由后续同步修正）
    unsafe fn initial_place(&mut self) {
        if self.user_pinned {
            return; // 固定位置：尊重保存的坐标
        }
        let fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        let fg_id = if fg.is_invalid() { 0 } else { fg.0 as isize };
        self.last_fg_hwnd = fg_id;
        uia_ensure_request(fg_id, true);
        // 最多等 1s（Electron/Chromium 树遍历可能数百 ms）：等到结论再落位，
        // 显示后一步到位，避免"先按估算落位、查询返回后再修"的二次跳动
        for _ in 0..50 {
            match uia_anchor_state(fg_id) {
                AnchorState::Pending => std::thread::sleep(std::time::Duration::from_millis(20)),
                _ => break,
            }
        }
        let Some((anchor, _zone_left)) = uia_cached_anchor(fg_id) else {
            return; // UIA 查不到（如刚开机壳窗口）：保持旧行为
        };
        let mut wa = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut wa as *mut RECT as _),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let bar_h = if self.collapsed { CAP_H } else { BAR_H };
        let h = (bar_h as f32 * self.scale).ceil() as i32;
        let y = (anchor - h / 2).max(wa.top);
        // x 取"最靠右"目标；取不到（小窗口前台）时沿用保存的 x
        let x = self.auto_x_target(None).or_else(|| self.pos.map(|p| p.x))
            .unwrap_or(wa.right - 420);
        self.pos = Some(POINT { x, y });
        SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
    }

    fn debug_on(&self) -> bool {
        std::env::var("TEMPMON_DODGE_DEBUG").is_ok()
    }

    /// 截屏定时器回调：执行截屏（毫秒级）并根据阶段继续流程
    fn capture_timer_step(&mut self) {
        unsafe { self.capture_timer_step_inner() }
    }

    #[allow(clippy::too_many_lines)]
    unsafe fn capture_timer_step_inner(&mut self) {
        let _ = KillTimer(Some(self.hwnd), CAP_TIMER_ID);
        let Some(req) = self.cap.take() else { return };
        let Some(px) = self.finish_capture(req.x, req.y, req.w, req.h, req.excluded, req.hidden)
        else {
            if self.debug_on() { eprintln!("[capture] grab FAILED at ({},{}) {}x{}", req.x, req.y, req.w, req.h); }
            return;
        };
        // 调试：转储检测画面
        let dump = self.debug_on();
        if dump {
            let mut data = vec![0u8; 54];
            data[0..2].copy_from_slice(b"BM");
            data[2..6].copy_from_slice(&((54 + px.len() as u32)).to_le_bytes());
            data[10..14].copy_from_slice(&54u32.to_le_bytes());
            data[14..18].copy_from_slice(&40u32.to_le_bytes());
            data[18..22].copy_from_slice(&(req.w).to_le_bytes());
            data[22..26].copy_from_slice(&(-req.h).to_le_bytes());
            data[26..28].copy_from_slice(&1u16.to_le_bytes());
            data[28..30].copy_from_slice(&32u16.to_le_bytes());
            data.extend_from_slice(&px);
            let name = if req.full { "cap_full.bmp" } else { "cap_cur.bmp" };
            let _ = std::fs::write(format!("D:/GLM桌面温度计260912/tools/{}", name), &data);
        }
        let Some(edge) = compute_edge_map(&px, req.w as usize, req.h as usize) else {
            if dump { eprintln!("[capture] edge_map FAILED w={} h={} px={}", req.w, req.h, px.len()); }
            return;
        };
        if req.sync {
            self.title_sync_from_strip(&edge, &req);
            return;
        }
        if !req.full {
            // 第一阶段：当前占位是否压住内容；边界超界也进入重选流程
            let Some(ctx) = self.dctx else { return };
            let in_bounds = ctx.left0 >= ctx.wa.left && ctx.left0 + ctx.cur_w <= ctx.wa.right;
            // 前台窗口自己的标题栏条带不算内容遮挡（贴右缘跟随必然覆盖标题
            // 文字），按钮区仍由 caption_zone_hit 判定
            let cap_only = in_caption_strip_of_window_at(ctx.x0, ctx.y, ctx.cur_w, ctx.h);
            let stats = if cap_only {
                None
            } else {
                edge_region_stats(&edge, req.w as usize, req.h as usize, 0, req.w as usize, 0, req.h as usize)
            };
            if dump {
                eprintln!("[dodge] phase1 w={} stats={:?}", req.w, stats);
            }
            let has = stats.is_some_and(|(d, bh, bt, ic)| {
                if ic {
                    return true;
                }
                if !(0.012..0.5).contains(&d) {
                    return false;
                }
                if req.h < 24 { bh >= 1 } else { bh >= 2 && bh * 2 > bt }
            });
            // 标题栏按钮区（最小化/最大化/关闭）用窗口枚举识别，
            // 细线图标的边缘密度注定低于内容阈值
            let mut cap_hit = false;
            if !has {
                // 采样胶囊真实右缘（ctx.x0..+cur_w）——截取矩形外扩了 16px，
                // 按 req.w 采样会探到胶囊外，在贴右边界处误报"压按钮"空转
                let mut sx = ctx.x0 + ctx.cur_w - 8;
                let sy = req.y + req.h / 2;
                while sx >= ctx.x0 {
                    if caption_zone_hit(sx, sy) {
                        cap_hit = true;
                        break;
                    }
                    sx -= 24;
                }
            }
            let has = has || cap_hit;
            if dump {
                eprintln!(
                    "[dodge] phase1 cap_only={} has={} cap_hit={} at ({},{})",
                    cap_only, has, cap_hit, ctx.x0, ctx.y
                );
            }
            // 仅压住按钮区（多因页宽增长右缘探进按钮区）：自动模式直接
            // 原地校正 x 到贴右目标位，不走完整避让流程——否则会跳到
            // 任意空位再被 snap 修回，表现为连续跳动
            if cap_hit && !self.user_pinned {
                let cluster_left = strip_button_cluster_left(
                    &edge,
                    req.w as usize,
                    req.h as usize,
                    (req.h / 2) as usize,
                    req.x,
                );
                if let Some(ax) = self.auto_x_target(cluster_left) {
                    // 目标位仍压任何窗口按钮区（含非前台的顶窗）就不原地校正，
                    // 交给完整避让找空位——否则 snap 过去下一轮又被判遮挡躲回，
                    // 两窗按钮簇不一致时无限乒乓
                    let zone_free = {
                        let mut ok = true;
                        let mut sx = ax + ctx.max_w - 8;
                        let sy = ctx.y + ctx.h / 2;
                        while sx >= ax {
                            if caption_zone_hit(sx, sy) {
                                ok = false;
                                break;
                            }
                            sx -= 24;
                        }
                        ok
                    };
                    if zone_free && ax != ctx.x0 {
                        self.pos = Some(POINT { x: ax, y: ctx.y });
                        let _ = SetWindowPos(
                            self.hwnd,
                            Some(HWND_TOPMOST),
                            ax,
                            ctx.y,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                        if self.debug_on() {
                            eprintln!("[cap-fix] {} -> {ax}", ctx.x0);
                        }
                        return;
                    }
                }
            }
            if has {
                self.occ_streak = self.occ_streak.saturating_add(1);
            } else {
                self.occ_streak = 0;
            }
            if in_bounds && !has {
                self.fb_runs = 0;
                // 未遮挡：顺路做标题栏同步 / 回归右缘的整行截取
                // （绝不在此前的遮挡检测前抢占轮次，窗口切换永远即时响应）
                let sync_due = self
                    .last_sync_check
                    .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(10));
                if sync_due {
                    self.last_sync_check = Some(Instant::now());
                }
                let cd_ok = self.ret_cooldown_until.is_none_or(|t| t <= Instant::now());
                let want_right = self.pos.is_some()
                    && !self.user_pinned
                    && ctx.left0 + ctx.max_w < ctx.wa.right
                    && cd_ok;
                if !self.user_pinned && (want_right || sync_due) {
                    self.dctx = Some(DodgeCtx { ret_right: true, ..ctx });
                    let strip_x = ctx.wa.left;
                    let strip_w = ctx.wa.right - ctx.wa.left;
                    // 截取自工作区顶部到温度计底缘：上半是标题栏按钮区（y 对齐锚点），
                    // 下半含温度计当前行（遮挡评估），一次截取同时服务两者
                    let sh = (ctx.y + ctx.h - ctx.wa.top).max(48);
                    if self.begin_capture(strip_x, ctx.wa.top, strip_w, sh, true, false) {
                        return;
                    }
                } else if self.user_pinned && sync_due {
                    // 手动钉定：x 尊重用户，仅同步 y 到标题栏按钮带（sync=true 走
                    // title_sync_from_strip，只修 y）
                    let strip_x = ctx.wa.left;
                    let strip_w = ctx.wa.right - ctx.wa.left;
                    let sh = (ctx.y + ctx.h - ctx.wa.top).max(48);
                    if self.begin_capture(strip_x, ctx.wa.top, strip_w, sh, false, true) {
                        return;
                    }
                }
                self.dctx = None;
                return; // 常态：无遮挡
            }
            // 确认遮挡（或超界）：固定当前位置（此后左缘锚定向右伸展），整行截取。
            // 界内遮挡需连续两轮确认（动画帧噪声只出现一轮，挪了就会乒乓）
            if in_bounds && self.occ_streak < 2 {
                return;
            }
            self.occ_streak = 0;
            self.pos = Some(POINT { x: ctx.x0, y: ctx.y });
            let strip_x = ctx.wa.left;
            let strip_w = ctx.wa.right - ctx.wa.left;
            // 同上：顶部到温度计底缘的纵向条带
            let sh = (ctx.y + ctx.h - ctx.wa.top).max(48);
            if !self.begin_capture(strip_x, ctx.wa.top, strip_w, sh, true, false) {
                self.dctx = None;
            }
            return;
        }

        // 第二阶段：整行边缘图上选新位置
        let Some(ctx) = self.dctx else { return };
        // UIA 查询未出结论时不做任何选位/移动：等 burst 下一轮再定位，
        // 避免"先按估算值跳一次、查询返回后再修正"的多段跳动
        if !self.user_pinned && matches!(uia_anchor_state(self.last_fg_hwnd), AnchorState::Pending) {
            return;
        }
        // 整条截取自工作区顶部到温度计底缘；候选评估用温度计所在行带 wy0..wy0+rh，
        // y 对齐用顶部 48px 的标题栏按钮带（strip_button_band_center_y 内部限定）
        let (strip_w, strip_h, max_w) = (req.w as usize, req.h as usize, ctx.max_w);
        let wy0 = (ctx.y - ctx.wa.top).max(0) as usize;
        let rh = ctx.h as usize;
        // 内容遮挡 + 标题栏按钮区（避免挪到的新位置又压住按钮）
        let occ = |x: i32| -> bool {
            let ox = (x - ctx.wa.left) as usize;
            if ox + max_w as usize > strip_w {
                return true; // 越界视为有遮挡
            }
            // 前台标题栏条带内豁免内容判定（同 phase1）
            if !in_caption_strip_of_window_at(x, ctx.y, max_w, ctx.h)
                && edge_region_has_content(&edge, strip_w, strip_h, ox, max_w as usize, wy0, rh)
            {
                return true;
            }
            let mut sx = x + max_w - 8;
            let sy = ctx.y + ctx.h / 2;
            while sx >= x {
                if caption_zone_hit(sx, sy) {
                    return true;
                }
                sx -= 24;
            }
            false
        };
        if dump {
            let mut cand_log = String::new();
            let step0 = (max_w / 2).max(40);
            let mut s0 = step0;
            while s0 <= (ctx.wa.right - ctx.wa.left - max_w).max(0) && cand_log.len() <= 400 {
                for nx in [ctx.x0 - s0, ctx.x0 + s0] {
                    if nx >= ctx.wa.left && nx + max_w <= ctx.wa.right {
                        cand_log.push_str(&format!(" x={}occ={}", nx, occ(nx)));
                    }
                }
                s0 += step0;
            }
            eprintln!(
                "[dodge] occluded at ({},{}) max_w={} |{}",
                ctx.x0, ctx.y, max_w, cand_log
            );
        }
        // 候选位置：优先向右（按距离升序），右侧无空位再向左。
        // 固定 40px 细步长：全部从同一张边缘图评估，细粒度才能找到
        // 「只压住少量空白」的低遮挡位置（大步长会整段跳过）
        let step = 40;
        let max_shift = ctx.wa.right - ctx.wa.left - max_w;
        let mut cands: Vec<i32> = Vec::new();
        let mut shift = step;
        while shift <= max_shift {
            let nr = ctx.x0 + shift;
            if nr >= ctx.wa.left && nr + max_w <= ctx.wa.right {
                cands.push(nr);
            }
            shift += step;
        }
        let mut shift = step;
        while shift <= max_shift {
            let nl = ctx.x0 - shift;
            if nl >= ctx.wa.left && nl + max_w <= ctx.wa.right {
                cands.push(nl);
            }
            shift += step;
        }
        // 每个候选只评估一次：内容占用 + 遮挡密度
        let evaluated: Vec<(i32, bool, f32)> = cands
            .iter()
            .filter_map(|&nx| {
                let ox = (nx - ctx.wa.left) as usize;
                if ox + max_w as usize > strip_w {
                    return None;
                }
                let all = edge_region_stats(&edge, strip_w, strip_h, ox, max_w as usize, wy0, rh)?;
                let occupied = occ(nx);
                if dump {
                    eprintln!("[dodge] cand x={} stats={:?}", nx, all);
                }
                Some((nx, occupied, all.0))
            })
            .collect();

        // 回归右缘模式：当前未遮挡，只在完全空位中挑最靠右的（x 最大）；
        // 已在最右（无更靠右空位）则原地不动
        if ctx.ret_right {
            // 目标 y：标题栏按钮带中心（UIA 优先，截图启发式回退），与 x 一步到位
            let ty = self.align_anchor_top(&edge, strip_w, strip_h, ctx.wa.top, ctx.h);
            // 最靠右 x：直接贴前台窗口标题栏按钮区左侧（不压按钮），
            // 不再依赖"空位搜索 + 100px 阈值 + 冷却"——最大化窗口的标题栏
            // 文字会让空位搜索处处误判占用，x 就永远停在旧位置。
            // 覆盖标题文字是贴右缘的预期行为；按钮安全由目标公式保证
            let mut dest: Option<POINT> = None;
            let cluster_left = {
                let cy = uia_cached_close_y(self.last_fg_hwnd)
                    .map(|y| (y - ctx.wa.top).max(16) as usize)
                    .or_else(|| strip_button_band_center_y(&edge, strip_w, strip_h).map(|c| c.max(16) as usize))
                    .unwrap_or(24);
                strip_button_cluster_left(&edge, strip_w, strip_h, cy, ctx.wa.left)
            };
            // 贴右目标必须过与遮挡判定同一把尺子（occ）：auto_x_target 按
            // 前台窗口的按钮簇算右界，caption_zone_hit 按该像素实际最顶的
            // 窗口算——两窗交叠且按钮簇不同时，snap 过去立刻被 phase1 判
            // 压按钮躲开，下一轮 snap 又贴回来，1437↔1477 无限乒乓。
            // 目标位被任何窗口按钮区否决就原地不动，等前台/簇变化再 snap。
            let snap_x = self.auto_x_target(cluster_left).filter(|&ax| {
                let ox = (ax - ctx.wa.left) as usize;
                ox + max_w as usize <= strip_w && !occ(ax)
            });
            if let Some(ax) = snap_x {
                if ax != ctx.left0 {
                    dest = Some(POINT { x: ax, y: ty.unwrap_or(ctx.y) });
                } else if let Some(y) = ty {
                    if (y - ctx.y).abs() >= 2 {
                        dest = Some(POINT { x: ctx.left0, y });
                    }
                }
            }
            let cd_ok = self.ret_cooldown_until.is_none_or(|t| t <= Instant::now());
            if dest.is_none() {
                let best = evaluated
                    .iter()
                    .filter(|(_, occupied, _)| !occupied)
                    .map(|&(nx, _, _)| nx)
                    .max();
                if let Some(nx) = best {
                    // 明显更靠右才动（≥100px），且冷却期外；y 对齐不受冷却限制
                    if nx > ctx.left0 + 100 && cd_ok {
                        dest = Some(POINT { x: nx, y: ty.unwrap_or(ctx.y) });
                    }
                }
                if dest.is_none() {
                    // x 已在最右：只修 y 偏差（≥2px 即修：浏览器标题栏高度差
                    // 往往只有两三像素，死区大了用户肉眼可见不对齐）
                    if let Some(y) = ty {
                        if (y - ctx.y).abs() >= 2 {
                            dest = Some(POINT { x: ctx.left0, y });
                        }
                    }
                }
            }
            if let Some(d) = dest {
                self.fb_x = 0;
                self.fade_x = d.x;
                self.fade_y = d.y;
                self.dodging = true;
                self.fade_phase = 1;
                self.ret_cooldown_until =
                    Some(Instant::now() + std::time::Duration::from_secs(30));
                let _ = SetTimer(Some(self.hwnd), DODGE_TIMER_ID, DODGE_TIMER_MS, None);
            }
            return;
        }

        // 自动模式：避让选位优先评估"贴右缘目标位"（与回归右缘一致）。
        // 否则避让会先把胶囊扔到任意空位、随后贴右 snap 再修一次——
        // 这就是切换程序时"跳好几次才到位"的来源
        // 先算 y（顺路完成 UIA×像素交叉校验，distrust 落定后 x 才用对预留）
        let ny_pre = self.align_anchor_top(&edge, strip_w, strip_h, ctx.wa.top, ctx.h);
        // 按钮簇左缘（边缘图扫描）：UIA 过期矩形弃用时的真实边界
        let cluster_left = {
            let cy = uia_cached_close_y(self.last_fg_hwnd)
                .map(|y| (y - ctx.wa.top).max(16) as usize)
                .or_else(|| strip_button_band_center_y(&edge, strip_w, strip_h).map(|c| c.max(16) as usize))
                .unwrap_or(24);
            strip_button_cluster_left(&edge, strip_w, strip_h, cy, ctx.wa.left)
        };
        let mut target = if !self.user_pinned {
            self.auto_x_target(cluster_left).filter(|&ax| {
                let ox = (ax - ctx.wa.left) as usize;
                ox + max_w as usize <= strip_w && !occ(ax)
            })
        } else {
            None
        };
        // 靠右原则：有空位就取**最靠右**的完全空位（不再就近）
        if target.is_none() {
            target = evaluated
                .iter()
                .filter(|(_, occupied, _)| !occupied)
                .map(|&(nx, _, _)| nx)
                .max();
        }
        if target.is_some() {
            self.fb_x = 0;
        }

        // 都没有空位：退而求其次，选遮挡密度最小的候选。
        // 双重防震荡：①候选密度必须比当前位置低 0.01 以上（明显更优才挪）；
        // ②已在兜底位且密度未明显上升（内容没变化）时留在原地
        // 连续 3 次兜底移动仍没找到真正空位 → 强制长冷却，打破循环
        if target.is_none() && self.fb_runs >= 3 {
            return;
        }
        if target.is_none() && self.last_dodge.map_or(true, |t| t <= Instant::now()) {
            let cur_d = edge_region_stats(
                &edge,
                strip_w,
                strip_h,
                (ctx.left0 - ctx.wa.left) as usize,
                max_w as usize,
                wy0,
                rh,
            )
            .map(|s| s.0)
            .unwrap_or(1.0);
            // fb_d 固定为选定时的密度：只有密度较当时明显上升（内容变了）才重选
            if self.fb_x != 0 && ctx.left0 == self.fb_x && cur_d <= self.fb_d + 0.015 {
                return;
            }
            // 靠右原则兜底：在遮挡明显更少的候选里取**最靠右**的，
            // 绝不往左边跑
            target = evaluated
                .iter()
                .filter(|(_, _, d)| *d < cur_d - 0.01)
                .max_by(|a, b| a.0.cmp(&b.0))
                .map(|&(nx, _, _)| nx);
            if let Some(nx) = target {
                self.fb_x = nx;
                self.fb_d = evaluated
                    .iter()
                    .find(|&(x, _, _)| *x == nx)
                    .map(|&(_, _, d)| d)
                    .unwrap_or(0.0);
            }
        }

        if dump {
            eprintln!(
                "[{}] [dodge] phase2 done: candidates={} free={} target={:?}",
                wallclock(),
                evaluated.len(),
                evaluated.iter().filter(|(_, o, _)| !o).count(),
                target,
            );
        }

        // 确定目标位置后，淡出→挪过去→淡入
        if let Some(nx) = target {
            let ny = ny_pre.unwrap_or(ctx.y);
            // 目标与当前位置相同：什么都不用做。带动画"挪"到原地 = 无限闪烁
            if nx == ctx.x0 && (ny - ctx.y).abs() < 2 {
                return;
            }
            // 兜底移动（无空位时）计入连击，冷却拉长到 30 秒打破循环
            let fallback = evaluated.iter().all(|(_, o, _)| *o);
            self.fb_runs = if fallback { self.fb_runs + 1 } else { 0 };
            self.fade_x = nx;
            // 一步到位：x 与标题栏对齐 y 同时移动，不再分两段（UIA 优先）
            self.fade_y = ny;
            self.dodging = true;
            self.fade_phase = 1;
            // 避让后 30s 内不做回归右缘检查——先稳定驻留，防乒乓
            self.ret_cooldown_until =
                Some(Instant::now() + std::time::Duration::from_secs(30));
            if fallback && self.fb_runs >= 3 {
                self.last_dodge = Some(Instant::now() + std::time::Duration::from_secs(26));
            }
            let _ = SetTimer(Some(self.hwnd), DODGE_TIMER_ID, DODGE_TIMER_MS, None);
        }
    }

    /// y 对齐锚点（屏幕坐标下温度计窗口的目标 top）：
    /// 优先 UIA 缓存的关闭按钮真实 y 中心，查不到回退截图启发式条带中心
    fn align_anchor_top(&mut self, edge: &[u8], w: usize, h: usize, wa_top: i32, wh: i32) -> Option<i32> {
        // 只跟随宽窗口：前台是弹窗/小窗（浏览器下载条、提示框等瞬态前台）
        // 时锚点几何很怪（可能贴近屏幕底部），跟随会造成来回弹跳；
        // 保持原位等 burst 下一轮（届时前台已回到正常窗口）
        if !self.fg_is_wide() {
            return None;
        }
        let strip = strip_button_band_center_y(edge, w, h).map(|c| wa_top + c);
        let uia = uia_cached_close_y(self.last_fg_hwnd);
        let y = match (uia, strip) {
            (Some(u), Some(sp)) if (u - sp).abs() > 6 => {
                // UIA 与像素矛盾：Chromium 系窗口的 UIA 矩形会过期
                // （布局变了树还报旧值），该窗口 TTL 内弃用 UIA
                self.uia_distrust = Some((self.last_fg_hwnd, Instant::now()));
                if self.debug_on() {
                    eprintln!("[uia-distrust] hwnd={:#x} uia={u} strip={sp}", self.last_fg_hwnd);
                }
                sp
            }
            (Some(u), _) => u,
            (None, Some(sp)) => sp,
            (None, None) => return None,
        };
        Some((y - wh / 2).max(wa_top))
    }

    /// 当前前台窗口的 UIA 是否已被交叉校验否决（TTL 60s）
    fn uia_distrusted(&self) -> bool {
        self.uia_distrust
            .is_some_and(|(h, t)| h == self.last_fg_hwnd && t.elapsed() < std::time::Duration::from_secs(60))
    }

    /// 前台窗口宽度是否 ≥60% 屏宽
    fn fg_is_wide(&self) -> bool {
        unsafe {
            let fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
            if fg.is_invalid() {
                return false;
            }
            let mut fr = RECT::default();
            if GetWindowRect(fg, &mut fr).is_err() {
                return false;
            }
            let mut wa = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut RECT as _),
                windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            fr.right - fr.left >= (wa.right - wa.left) * 3 / 5
        }
    }

    /// 从右缘条带（200×48）的边缘图判定标题栏按钮行的垂直中心并同步。
    /// 按钮行表现为一条水平密集带；取带中心，把温度计垂直居中对齐过去。
    /// 只在明显偏移（>3px）时移动；移动后 10s 内不重复同步
    fn title_sync_from_strip(&mut self, edge: &[u8], req: &CapReq) {
        if self.debug_on() {
            eprintln!("[title-sync] enter w={} h={} edge_len={}", req.w, req.h, edge.len());
        }
        let w = req.w as usize;
        let h = req.h as usize;
        let strip_cy = strip_button_band_center_y(edge, w, h);
        let h_w0 = unsafe {
            let mut r0 = RECT::default();
            if GetWindowRect(self.hwnd, &mut r0).is_ok() { r0.bottom - r0.top } else { 24 }
        };
        let center = self
            .align_anchor_top(edge, w, h, req.y, h_w0)
            .map(|ty| ty + h_w0 / 2);
        let Some(center) = center else {
            if self.debug_on() {
                eprintln!("[title-sync] strip has no content rows");
            }
            return;
        };
        if self.debug_on() {
            eprintln!("[title-sync] anchor={}", if self.uia_distrusted() { "strip(交叉校验否决uia)" } else if uia_cached_close_y(self.last_fg_hwnd).is_some() { "uia" } else { "strip" });
        }
        unsafe {
            let mut r = RECT::default();
            if GetWindowRect(self.hwnd, &mut r).is_err() {
                return;
            }
            let mut wa = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut RECT as _),
                windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            let h_w = r.bottom - r.top;
            let ty = (center - h_w / 2).max(wa.top);
            let d = ty - r.top;
            if self.debug_on() {
                eprintln!(
                    "[{}] [title-sync] center_row={center} h={h_w} cur_y={} ty={ty} d={d}",
                    wallclock(),
                    r.top
                );
            }
            // 多数表决：新提议与最近两条历史任一一致（≥2/3）才挪。
            // 头部动画/重绘空帧造成的一次性跳变无法凑齐 2/3，被自然滤除；
            // 前台切到新窗口后，新按钮行的提议两次内即成多数，自动收敛
            let agree = |a: i32, b: i32| (a - b).abs() < 2;
            let n = self.sync_ty_n as usize;
            let confirmed = n >= 1 && agree(self.sync_ty_hist[(n + 2) % 3], ty)
                || n >= 2 && agree(self.sync_ty_hist[(n + 1) % 3], ty);
            let n2 = n.min(2);
            self.sync_ty_hist[n2] = ty;
            self.sync_ty_n = (n + 1).min(3) as u32;
            if !confirmed {
                if self.debug_on() {
                    eprintln!("[title-sync] ty={ty} 待确认 (历史 {:?})", &self.sync_ty_hist[..n2 + 1]);
                }
                return;
            }
            self.sync_ty_n = 0;
            self.pos = Some(POINT { x: r.left, y: ty });
            self.fade_x = r.left;
            self.fade_y = ty;
            self.dodging = true;
            self.fade_phase = 1;
            self.ret_cooldown_until =
                Some(Instant::now() + std::time::Duration::from_secs(10));
            let _ = SetTimer(Some(self.hwnd), DODGE_TIMER_ID, DODGE_TIMER_MS, None);
        }
    }

    /// 发起截屏：设置截图排除（瞬时，不阻塞），实际截屏在定时器回调中执行
    unsafe fn begin_capture(&mut self, x: i32, y: i32, w: i32, h: i32, full: bool, sync: bool) -> bool {
        if w <= 0 || h <= 0 || self.cap.is_some() {
            return false;
        }
        let excluded = SetWindowDisplayAffinity(self.hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok();
        let mut hidden = false;
        if !excluded {
            // 旧系统回退：先隐藏窗口，定时器到点时截屏（短暂不可见）
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            hidden = true;
        }
        self.cap = Some(CapReq { x, y, w, h, full, sync, excluded, hidden });
        let _ = SetTimer(Some(self.hwnd), CAP_TIMER_ID, CAP_CAPTURE_DELAY_MS, None);
        true
    }

    /// 截屏收尾：BitBlt（毫秒级）并恢复窗口的截图可见性
    unsafe fn finish_capture(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        excluded: bool,
        hidden: bool,
    ) -> Option<Vec<u8>> {
        let px = grab_screen(x, y, w, h);
        if excluded {
            let _ = SetWindowDisplayAffinity(self.hwnd, WDA_NONE);
        }
        if hidden {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        px
    }

    /// 淡出动画步进：先淡出，到位后移动窗口并保存位置，再淡入
    fn fade_step(&mut self) {
        match self.fade_phase {
            1 => {
                self.fade -= FADE_OUT_STEP;
                if self.fade <= 0.0 {
                    self.fade = 0.0;
                    self.fade_phase = 2;
                    unsafe {
                        let _ = SetWindowPos(
                            self.hwnd,
                            Some(HWND_TOPMOST),
                            self.fade_x,
                            self.fade_y,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                    self.pos = Some(POINT { x: self.fade_x, y: self.fade_y });
                    save_config(&self.as_config());
                }
            }
            2 => {
                self.fade += FADE_IN_STEP;
                if self.fade >= 1.0 {
                    self.fade = 1.0;
                    self.fade_phase = 0;
                    self.dodging = false;
                    // 避让完成后冷却 4 秒（完全空位移动不受限，仅限制
                    // “最少遮挡”兜底移动），避免临界内容导致来回震荡
                    self.last_dodge =
                        Some(Instant::now() + std::time::Duration::from_secs(4));
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), DODGE_TIMER_ID);
                    }
                }
            }
            _ => {}
        }
        // 立即重绘以应用新的透明度（正常内容按刷新周期才重绘）
        if self.fade_phase != 0 || self.fade < 1.0 {
            let _ = self.tick();
        }
    }

    fn as_config(&self) -> Config {
        Config {
            collapsed: self.collapsed,
            alpha: self.alpha,
            pos: self.pos,
            capsule_items: self.capsule_items,
            row_items: self.row_items,
            rotate_ms: self.rotate_ms,
            refresh_ms: self.refresh_ms,
            bg_mode: self.bg_mode,
            dodge_secs: self.dodge_secs,
            pinned: self.user_pinned,
            widest: self.widest,
        }
    }

    fn save_current_pos(&mut self) {
        unsafe {
            let mut r = RECT::default();
            if GetWindowRect(self.hwnd, &mut r).is_ok() {
                self.pos = Some(POINT { x: r.left, y: r.top });
                self.user_pinned = true;
                save_config(&self.as_config());
            }
        }
    }

}

// ── 防遮挡辅助 ──

/// 关闭 DWM 投影：NC 渲染禁用 + 边框无色（Win11 系统圆角默认自带阴影）
unsafe fn set_no_shadow(hwnd: HWND) {
    let disabled: u32 = 1; // DWMNCRENDERING_POLICY_DISABLED
    let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
        hwnd,
        windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(2),
        &disabled as *const _ as *const core::ffi::c_void,
        4,
    );
    let none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
    let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
        hwnd,
        windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(34), // DWMWA_BORDER_COLOR
        &none as *const _ as *const core::ffi::c_void,
        4,
    );
}

/// 该区域是否落在宽窗口的标题栏条带内（按胶囊所在位置的窗口判定，
/// 而非前台——前台切到窄窗口（控制台等）时浏览器标题文字仍是标题文字，
/// 不该重新变成"被压住的内容"，否则会对着原地反复播放避让动画=闪烁）。
/// 贴右缘跟随必然覆盖标题文字——那是预期行为，遮挡判定应豁免；
/// 按钮区由 caption_zone_hit 单独保护。
unsafe fn in_caption_strip_of_window_at(x: i32, y: i32, w: i32, h: i32) -> bool {
    let win = WindowFromPoint(POINT { x: x + w / 2, y: y + h / 2 });
    if win.is_invalid() {
        return false;
    }
    let mut fr = RECT::default();
    if GetWindowRect(win, &mut fr).is_err() {
        return false;
    }
    let mut wa = RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut wa as *mut RECT as _),
        windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    // 窗口太窄（<60% 屏宽）时标题栏条带不延伸到胶囊所在的天头区域
    if fr.right - fr.left < (wa.right - wa.left) * 3 / 5 {
        return false;
    }
    y >= fr.top && y + h <= fr.top + 60 && x + w > fr.left && x < fr.right
}

/// 采样点是否落在某个窗口右上角的标题栏按钮区（最小化/最大化/关闭）。/// 最小化按钮只有一条 1px 细线，边缘密度注定低于内容阈值，
/// 但所有 Win11 应用的窗口控制按钮都固定在窗口右上角，用窗口枚举识别。
/// 自身是 WS_EX_TRANSPARENT 穿透窗口，WindowFromPoint 会跳过它。
unsafe fn caption_zone_hit(x: i32, y: i32) -> bool {
    let hwnd = WindowFromPoint(POINT { x, y });
    if hwnd.is_invalid() {
        return false;
    }
    let mut cls = [0u16; 64];
    let n = GetClassNameW(hwnd, &mut cls);
    let cls = String::from_utf16_lossy(&cls[..n as usize]);
    // 排除桌面与任务栏（它们矩形覆盖全屏/全宽，会误报出假的“按钮区”）
    if matches!(
        cls.as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    ) {
        return false;
    }
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return false;
    }
    // 按钮区左缘：优先用 UIA 枚举的真实按钮簇边界（ZCode 等应用标题栏
    // 除标准三枚外还有附加按钮，通用 160px 会把贴右位置误判为压按钮，
    // 触发"避让挪走 → snap 贴回"的来回拉扯）；UIA 未就绪退回 160px 估算
    let zone = match uia_anchor_state(hwnd.0 as isize) {
        AnchorState::Ready(_, zl) => zl - CAPTION_GAP,
        _ => r.right - CAPTION_FALLBACK,
    };
    x >= zone && x <= r.right && y >= r.top && y < r.top + 60
}

/// 对截屏像素计算内容图（整条只算一次，供所有候选位置复用）：
/// 高频边缘（文字/图标轮廓）∪ 高色差像素（平滑渐变的彩色图标/彩色文字）
fn compute_edge_map(px: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    if w < 8 || h < 8 || px.len() < w * h * 4 {
        return None;
    }
    let mut gray = vec![0u8; w * h];
    for i in 0..w * h {
        let (b, g, r) = (px[i * 4] as u32, px[i * 4 + 1] as u32, px[i * 4 + 2] as u32);
        gray[i] = ((r * 299 + g * 587 + b * 114) / 1000) as u8;
    }
    let mut edge = vec![0u8; w * h];
    for yy in 1..h - 1 {
        for xx in 1..w - 1 {
            let i = yy * w + xx;
            let dx = (gray[i + 1] as i32 - gray[i - 1] as i32).abs();
            let dy = (gray[i + w] as i32 - gray[i - w] as i32).abs();
            let chroma = px[i * 4].max(px[i * 4 + 1]).max(px[i * 4 + 2]) as i32
                - px[i * 4].min(px[i * 4 + 1]).min(px[i * 4 + 2]) as i32;
            if dx + dy > 64 || chroma > 60 {
                edge[i] = 1;
            }
        }
    }
    Some(edge)
}

/// 在边缘图上统计子区域：整体密度、命中的 8px 条带数/总数、是否含紧凑图标块
/// 条带内容的加权垂直中心（按每行边缘像素数加权）：
/// 连续平滑无带边界跳变；只统计上部 44 行（再往下是页面内容）
fn wallclock() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let sec = d.as_secs() % 86400;
    format!("{:02}:{:02}:{:02}.{:03}", sec / 3600 + 8, (sec % 3600) / 60, sec % 60, d.subsec_millis())
}

fn strip_content_center_y(edge: &[u8], w: usize, h: usize) -> Option<i32> {
    if w < 16 || h < 8 {
        return None;
    }
    let rows = h.min(44);
    let mut sum = 0f64;
    let mut cnt = 0u64;
    for y in 1..rows - 1 {
        let mut c = 0u32;
        for x in 1..w - 1 {
            c += edge[y * w + x] as u32;
        }
        if c > 0 {
            sum += c as f64 * y as f64;
            cnt += c as u64;
        }
    }
    (cnt > 0).then_some((sum / cnt as f64) as i32)
}

/// 标题栏按钮带中心：只统计条带最右 200px（最小化/最大化/关闭按钮簇）的内容。
/// 全行加权平均会被头部任意内容（天气、标签页、页首横幅）拉偏；带取 200px
/// 恰好覆盖三枚按钮又排除更左侧的头部元素。按钮始终贴窗口右缘，右带是锚点。
fn strip_button_band_center_y(edge: &[u8], w: usize, h: usize) -> Option<i32> {
    // 关闭按钮锚定：自顶向下找最右窄带里的第一个密度簇取加权中心。
    // 带宽梯式回退：48px 只含关闭按钮（最准，360极速/微信/360文件夹都命中）；
    // 部分程序（ZCode 等）按钮左侧留白大，48px 会空转，逐级放宽到 96/200px。
    // 不可用全带密度峰值/加权平均：网页内容永远比按钮字形更密，两者都会把
    // 锚点拉进页面区域（实测 360 极速浏览器锚到第 44 行的页面横幅）
    if w < 16 || h < 8 {
        return None;
    }
    for band_w in [48usize, 96, 200] {
        let bw = band_w.min(w);
        let x0 = w - bw;
        let rows = h.min(48);
        let mut row_density = [0u32; 48];
        for y in 2..rows - 1 {
            let mut c = 0u32;
            for x in x0 + 1..w - 1 {
                c += edge[y * w + x] as u32;
            }
            row_density[y] = c;
        }
        // 绝对簇阈值 5：相对峰值会被稠密的页面横幅抬高，按钮字形行
        //（密度 14-28）大面积掉出门槛，顶部簇跳到页面横幅上
        if row_density.iter().copied().max().unwrap_or(0) < 5 {
            continue; // 该带宽内没有按钮簇，放宽带宽重试
        }
        // 自顶向下找第一个够格的行（跳过 0-1 的窗口边框线）
        let mut y0 = None;
        for y in 2..rows - 1 {
            if row_density[y] >= 5 {
                y0 = Some(y);
                break;
            }
        }
        let y0 = y0?;
        // 簇延伸：连续 2 行低于阈值即认为簇结束
        let mut y1 = y0;
        let mut miss = 0u32;
        for y in y0 + 1..rows - 1 {
            if row_density[y] >= 5 {
                y1 = y;
                miss = 0;
            } else {
                miss += 1;
                if miss >= 2 {
                    break;
                }
            }
        }
        let mut sum = 0f64;
        let mut cnt = 0u64;
        for y in y0..=y1 {
            let c = row_density[y] as f64;
            if c > 0.0 {
                sum += c * y as f64;
                cnt += row_density[y] as u64;
            }
        }
        if cnt > 0 {
            return Some((sum / cnt as f64) as i32);
        }
    }
    None
}

/// 从边缘图扫标题栏按钮簇的左缘（屏幕 x）：按钮行带内自右缘向左找
/// 连续字形列，允许 ≤24px 的按钮间距，遇到更大空隙（标签页文字/工具栏
/// 的空白）即停。UIA 矩形过期（Chromium 滞后）时用它定位真实按钮簇——
/// 固定 190px 预留盖不住扩展按钮（"5"/下载/T恤图标），会贴成 1px 缝。
fn strip_button_cluster_left(
    edge: &[u8],
    w: usize,
    h: usize,
    cy: usize,
    strip_left: i32,
) -> Option<i32> {
    if w < 60 || h < 8 {
        return None;
    }
    let y0 = cy.saturating_sub(16);
    let y1 = (cy + 16).min(h - 1);
    let col_on = |x: usize| -> bool {
        (y0..=y1).any(|y| edge[y * w + x] != 0)
    };
    // 起点：右缘 60px 内第一个有字形的列（按钮贴窗口右缘；找不到则不判）
    let mut x = w - 2;
    let mut start = None;
    while x > w - 60 {
        if col_on(x) {
            start = Some(x);
            break;
        }
        x -= 1;
    }
    let mut x = start?;
    let mut left = x;
    let mut gap = 0usize;
    while x > 0 {
        x -= 1;
        if col_on(x) {
            left = x;
            gap = 0;
        } else {
            gap += 1;
            if gap > 24 {
                break;
            }
        }
    }
    Some(strip_left + left as i32)
}

fn edge_region_stats(
    edge: &[u8],
    w: usize,
    h: usize,
    ox: usize,
    rw: usize,
    oy: usize,
    rh: usize,
) -> Option<(f32, usize, usize, bool)> {
    if rw < 8 || rh < 8 || ox + rw > w || oy + rh > h {
        return None;
    }
    let mut total = 0u32;
    for yy in oy + 1..oy + rh - 1 {
        for xx in ox + 1..ox + rw - 1 {
            total += edge[yy * w + xx] as u32;
        }
    }
    let area = ((rw - 2) * (rh - 2)) as f32;
    if area <= 0.0 {
        return None;
    }
    let density = total as f32 / area;

    // 文字：固定 8px 条带覆盖全部行高（含末尾不足 8px 的部分）
    let band = 8usize;
    let (mut bands_hit, mut bands_total) = (0usize, 0usize);
    let mut y0 = oy + 1;
    while y0 < oy + rh - 1 {
        let ye = (y0 + band).min(oy + rh - 1);
        let mut c = 0u32;
        for yy in y0..ye {
            for xx in ox + 1..ox + rw - 1 {
                c += edge[yy * w + xx] as u32;
            }
        }
        let bd = c as f32 / ((ye - y0) as f32 * (rw - 2) as f32);
        bands_total += 1;
        if bd > 0.03 {
            bands_hit += 1;
        }
        y0 += band;
    }

    // 图标/标识等紧凑高对比内容：任一 24×24 块（步进 12 重叠扫描）密度 ≥0.08
    let bs = 24usize;
    let mut has_icon_block = false;
    if rw >= bs && rh >= bs {
        // 步进 12 扫描；末尾补一个对齐区域右/下缘的块，避免边缘处的图标漏检
        let xs: Vec<usize> = {
            let mut v: Vec<usize> = (0..=rw - bs).step_by(12).collect();
            let last = rw - bs;
            if *v.last().unwrap() != last {
                v.push(last);
            }
            v
        };
        let ys: Vec<usize> = {
            let mut v: Vec<usize> = (oy..=oy + rh - bs).step_by(12).collect();
            let last = oy + rh - bs;
            if *v.last().unwrap() != last {
                v.push(last);
            }
            v
        };
        'outer: for &by in &ys {
            for &bx in &xs {
                let mut c = 0u32;
                for yy in by..by + bs {
                    for xx in ox + bx..ox + bx + bs {
                        c += edge[yy * w + xx] as u32;
                    }
                }
                if c as f32 / (bs * bs) as f32 >= 0.12 {
                    has_icon_block = true;
                    break 'outer;
                }
            }
        }
    }
    Some((density, bands_hit, bands_total, has_icon_block))
}

/// 子区域是否含有文字/图标等内容
fn edge_region_has_content(
    edge: &[u8],
    w: usize,
    h: usize,
    ox: usize,
    rw: usize,
    oy: usize,
    rh: usize,
) -> bool {
    let Some((density, bands_hit, bands_total, has_icon_block)) =
        edge_region_stats(edge, w, h, ox, rw, oy, rh)
    else {
        return false;
    };
    // 图标/标识单独判定：摊到整条后整体密度可能低于文字下限
    if has_icon_block {
        return true;
    }
    if !(0.015..0.5).contains(&density) {
        return false;
    }
    // 行高较小（如 24px 的悬浮条）时文字可能只落入一两条带，只要求 1 条命中；
    // 行高较大时要求至少 2 条且命中过半，以排除照片类内容
    if rh < 24 {
        bands_hit >= 1
    } else {
        bands_hit >= 2 && bands_hit * 2 > bands_total
    }
}

unsafe fn grab_screen(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    let sdc = GetDC(None);
    let mdc = CreateCompatibleDC(Some(sdc));
    let mut bi = BITMAPINFO::default();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h; // 自顶向下
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB.0;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut out = None;
    if let Ok(hbmp) = CreateDIBSection(Some(mdc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
        let old = SelectObject(mdc, hbmp.into());
        if BitBlt(mdc, 0, 0, w, h, Some(sdc), x, y, SRCCOPY).is_ok() && !bits.is_null() {
            let len = (w as usize) * (h as usize) * 4;
            let mut buf = vec![0u8; len];
            std::ptr::copy_nonoverlapping(bits as *const u8, buf.as_mut_ptr(), len);
            out = Some(buf);
        }
        SelectObject(mdc, old);
        let _ = DeleteObject(hbmp.into());
    }
    let _ = DeleteDC(mdc);
    ReleaseDC(None, sdc);
    out
}

/// 文字特征判断：文字区域表现为「高频边缘 + 水平条带分布」，
/// 纯色/渐变壁纸密度极低，照片类内容密度过高且不成条带

#[allow(non_snake_case)]
unsafe fn UpdateLayeredWindowSafe(
    hwnd: HWND,
    screen: HDC,
    pos: &POINT,
    size: &SIZE,
    src: HDC,
    src_origin: &POINT,
    blend: &BLENDFUNCTION,
) -> windows::core::Result<()> {
    windows::Win32::UI::WindowsAndMessaging::UpdateLayeredWindow(
        hwnd,
        Some(screen),
        Some(pos),
        Some(size),
        Some(src),
        Some(src_origin),
        COLORREF(0),
        Some(blend),
        ULW_ALPHA,
    )
}

unsafe fn app_ptr(hwnd: HWND) -> *mut App {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App
}

unsafe fn non_null_app(hwnd: HWND) -> Option<*mut App> {
    let p = app_ptr(hwnd);
    (!p.is_null()).then_some(p)
}

const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
const ACCENT_DISABLED: u32 = 0;
const WCA_ACCENT_POLICY: u32 = 19;

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: i32,
}

#[repr(C)]
struct WinCompAttribData {
    attribute: u32,
    data: *mut AccentPolicy,
    size_of_data: usize,
}

type SetWindowCompositionAttributeFn =
    unsafe extern "system" fn(HWND, *mut WinCompAttribData) -> i32;

unsafe fn apply_acrylic(hwnd: HWND, enable: bool, light: bool) {
    let (state, tint) = if !enable {
        (ACCENT_DISABLED, 0u32)
    } else if light {
        (ACCENT_ENABLE_ACRYLICBLURBEHIND, 0xB8FAFBFFu32)
    } else {
        (ACCENT_ENABLE_ACRYLICBLURBEHIND, 0xA0141418u32)
    };
    let mut policy = AccentPolicy {
        accent_state: state,
        accent_flags: 2,
        gradient_color: tint,
        animation_id: 0,
    };
    let mut data = WinCompAttribData {
        attribute: WCA_ACCENT_POLICY,
        data: &mut policy,
        size_of_data: std::mem::size_of::<AccentPolicy>(),
    };
    let lib = match GetModuleHandleW(w!("user32.dll")) {
        Ok(h) => h,
        Err(_) => return,
    };
    let f = match GetProcAddress(lib, PCSTR::from_raw(b"SetWindowCompositionAttribute\0".as_ptr()))
    {
        Some(f) => std::mem::transmute::<_, SetWindowCompositionAttributeFn>(f),
        None => return,
    };
    f(hwnd, &mut data);
}

/// 防遮挡事件钩子：任何顶层窗口位置变化/显示/隐藏/前台切换时置位
/// （WINEVENT_SKIPOWNPROCESS 已排除自身；id_object==0 即 OBJID_WINDOW，
///  过滤掉光标、插入符等对象的高频事件）
unsafe extern "system" fn dodge_event_hook(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    id_object: i32,
    _idchild: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != 0 {
        return;
    }
    DODGE_EVENT.store(true, std::sync::atomic::Ordering::Relaxed);
    let hwnd = TOPMOST_HWND.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd == 0 {
        return;
    }
    let hwnd = HWND(hwnd as _);
    // 前台切换：安排 0.3/0.8/1.6s 三次补查（覆盖软件延迟绘制标题栏）
    if _event == EVENT_SYSTEM_FOREGROUND {
        for (i, &ms) in DODGE_BURST_MS.iter().enumerate() {
            let _ = SetTimer(Some(hwnd), DODGE_BURST_TIMER_ID + i, ms, None);
        }
        return;
    }
    // 节流 DODGE_THROTTLE_MS（读上次检测时刻，不写入——由 dodge_if_occluding 统一记账）
    let now = START_MS
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as usize;
    let last = DODGE_LAST_CHECK_MS.load(std::sync::atomic::Ordering::Relaxed);
    if now.saturating_sub(last) > DODGE_THROTTLE_MS {
        let hwnd = TOPMOST_HWND.load(std::sync::atomic::Ordering::Relaxed);
        if hwnd != 0 {
            PostMessageW(
                Some(HWND(hwnd as _)),
                WM_APP_DODGE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    } else {
        // 节流窗口内：安排一个到期即查的重试定时器，避免被动等下一次
        // 刷新 tick（最长 refresh_ms）才消费事件
        let hwnd = TOPMOST_HWND.load(std::sync::atomic::Ordering::Relaxed);
        if hwnd != 0 {
            let remain = (DODGE_THROTTLE_MS - (now - last)) as u32 + 10;
            let _ = SetTimer(Some(HWND(hwnd as _)), DODGE_RETRY_TIMER_ID, remain, None);
        }
    }
}

unsafe extern "system" fn foreground_hook(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _idobject: i32,
    _idchild: i32,
    _thread: u32,
    _time: u32,
) {
    let hwnd = TOPMOST_HWND.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd != 0 {
        let _ = SetWindowPos(
            HWND(hwnd as _),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        if matches!(msg, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            if kb.vkCode == VK_CONTROL.0 as u32 {
                let hwnd = TOPMOST_HWND.load(std::sync::atomic::Ordering::Relaxed);
                if hwnd != 0 {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), WM_APP_CTRL, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == 0x020A {
        let hwnd = TOPMOST_HWND.load(std::sync::atomic::Ordering::Relaxed);
        if hwnd != 0 {
            let h = HWND(hwnd as _);
            let mut r = RECT::default();
            let mut pt = POINT::default();
            if GetWindowRect(h, &mut r).is_ok()
                && GetCursorPos(&mut pt).is_ok()
                && pt.x >= r.left
                && pt.x < r.right
                && pt.y >= r.top
                && pt.y < r.bottom
            {
                let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
                let delta = (ms.mouseData >> 16) as i16;
                let dir = if delta > 0 { 1u8 } else { 0u8 };
                let _ = PostMessageW(Some(h), WM_APP_WHEEL, WPARAM(dir as usize), LPARAM(0));
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0;
            if ctrl {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            }
        }
        WM_LBUTTONDOWN => {
            let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0;
            if ctrl {
                if let Some(ptr) = non_null_app(hwnd) {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    (*ptr).click_pos = Some(pt);
                }
                let _ = ReleaseCapture();
                let _ = SendMessageW(
                    hwnd,
                    WM_NCLBUTTONDOWN,
                    Some(WPARAM(HTCAPTION as usize)),
                    Some(LPARAM(0)),
                );
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0;
            if ctrl {
                let ptr = app_ptr(hwnd);
                if !ptr.is_null() && (*ptr).collapsed {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let moved = (*ptr)
                        .click_pos
                        .map(|a| (a.x - pt.x).abs() > 6 || (a.y - pt.y).abs() > 6)
                        .unwrap_or(true);
                    if !moved {
                        (*ptr).toggle_collapse();
                    }
                }
                if let Some(ptr) = non_null_app(hwnd) {
                    (*ptr).click_pos = None;
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0;
            if ctrl {
                if let Some(ptr) = non_null_app(hwnd) {
                    let _ = (*ptr).show_menu();
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            if let Some(ptr) = non_null_app(hwnd) {
                match wparam.0 & 0xFFFF {
                    MENU_EXIT => PostQuitMessage(0),
                    MENU_COLLAPSE => (*ptr).toggle_collapse(),
                    MENU_ALPHA_25 => (*ptr).set_alpha(ALPHA_CHOICES[0]),
                    MENU_ALPHA_40 => (*ptr).set_alpha(ALPHA_CHOICES[1]),
                    MENU_ALPHA_55 => (*ptr).set_alpha(ALPHA_CHOICES[2]),
                    MENU_ALPHA_70 => (*ptr).set_alpha(ALPHA_CHOICES[3]),
                    MENU_ALPHA_85 => (*ptr).set_alpha(ALPHA_CHOICES[4]),
                    MENU_ALPHA_100 => (*ptr).set_alpha(ALPHA_CHOICES[5]),
                    MENU_AUTOSTART => {
                        let enable = !autostart_enabled();
                        set_autostart(enable);
                    }
                    MENU_RESET_POS => {
                        (*ptr).pos = None;
                        (*ptr).user_pinned = false;
                        save_config(&(*ptr).as_config());
                    }
                    MENU_PIN => {
                        (*ptr).user_pinned = !(*ptr).user_pinned;
                        save_config(&(*ptr).as_config());
                    }
                    id if (MENU_CAPSULE_BASE..MENU_CAPSULE_BASE + 9).contains(&id) => {
                        let bit = 1u32 << (id - MENU_CAPSULE_BASE);
                        (*ptr).capsule_items ^= bit;
                        (*ptr).widest = 0;
                        save_config(&(*ptr).as_config());
                    }
                    id if (MENU_ROW_BASE..MENU_ROW_BASE + 5).contains(&id) => {
                        let bit = 1u32 << (id - MENU_ROW_BASE);
                        (*ptr).row_items ^= bit;
                        (*ptr).widest = 0;
                        save_config(&(*ptr).as_config());
                    }
                    id if (MENU_ROTATE_BASE..MENU_ROTATE_BASE + 4).contains(&id) => {
                        (*ptr).rotate_ms = ROTATE_CHOICES_MS[id - MENU_ROTATE_BASE];
                        save_config(&(*ptr).as_config());
                    }
                    id if (MENU_BG_BASE..MENU_BG_BASE + 4).contains(&id) => {
                        (*ptr).bg_mode = (id - MENU_BG_BASE) as u8;
                        (*ptr).apply_bg_effect();
                        save_config(&(*ptr).as_config());
                    }
                    id if (MENU_REFRESH_BASE..MENU_REFRESH_BASE + 3).contains(&id) => {
                        (*ptr).refresh_ms = REFRESH_CHOICES_MS[id - MENU_REFRESH_BASE];
                        SetTimer(Some(hwnd), TIMER_ID, (*ptr).refresh_ms, None);
                        save_config(&(*ptr).as_config());
                    }
                    id if (MENU_DODGE_BASE..MENU_DODGE_BASE + DODGE_CHOICES_SECS.len())
                        .contains(&id) =>
                    {
                        (*ptr).dodge_secs = DODGE_CHOICES_SECS[id - MENU_DODGE_BASE];
                        (*ptr).last_dodge = None; // 立即按新周期执行一次检测
                        save_config(&(*ptr).as_config());
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_ENTERSIZEMOVE => {
            if let Some(ptr) = non_null_app(hwnd) {
                (*ptr).dragging = true;
            }
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            if let Some(ptr) = non_null_app(hwnd) {
                (*ptr).dragging = false;
                (*ptr).save_current_pos();
                let _ = (*ptr).tick();
                (*ptr).dodge_if_occluding();
            }
            LRESULT(0)
        }
        WM_APP_CTRL => {
            if let Some(ptr) = non_null_app(hwnd) {
                let _ = (*ptr).update_clickthrough();
            }
            LRESULT(0)
        }
        WM_APP_DODGE => {
            // 窗口事件钩子触发的即时检测（已在钩子中节流）
            if let Some(ptr) = non_null_app(hwnd) {
                (*ptr).dodge_if_occluding();
            }
            LRESULT(0)
        }
        WM_APP_WHEEL => {
            if let Some(ptr) = non_null_app(hwnd) {
                if wparam.0 == 1 {
                    (*ptr).capsule_page = (*ptr).capsule_page.wrapping_add(1);
                } else {
                    (*ptr).capsule_page = (*ptr).capsule_page.wrapping_sub(1);
                }
                (*ptr).last_page_rot = Some(Instant::now());
                let _ = (*ptr).tick();
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_ID {
                let ptr = app_ptr(hwnd);
                if !ptr.is_null() {
                    let _ = (*ptr).tick();
                }
            } else if wparam.0 == DODGE_TIMER_ID {
                let ptr = app_ptr(hwnd);
                if !ptr.is_null() {
                    (*ptr).fade_step();
                }
            } else if wparam.0 == CAP_TIMER_ID {
                let ptr = app_ptr(hwnd);
                if !ptr.is_null() {
                    (*ptr).capture_timer_step();
                }
            } else if wparam.0 == DODGE_RETRY_TIMER_ID {
                let _ = KillTimer(Some(hwnd), DODGE_RETRY_TIMER_ID);
                let ptr = app_ptr(hwnd);
                if !ptr.is_null() {
                    (*ptr).dodge_if_occluding();
                }
            } else if (DODGE_BURST_TIMER_ID..DODGE_BURST_TIMER_ID + DODGE_BURST_COUNT)
                .contains(&wparam.0)
            {
                let _ = KillTimer(Some(hwnd), wparam.0);
                let ptr = app_ptr(hwnd);
                if !ptr.is_null() {
                    (*ptr).dodge_if_occluding();
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = app_ptr(hwnd);
            if !ptr.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
