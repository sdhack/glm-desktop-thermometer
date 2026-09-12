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
    CreateCompatibleDC, CreateDIBSection, CreateRoundRectRgn, DeleteDC, DeleteObject, GetDC,
    ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, BITMAPINFO,
    BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
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
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, VK_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, DestroyMenu, GetMessageW, GetCursorPos, GetWindowLongPtrW, GetWindowRect,
    LoadCursorW, SendMessageW, PostMessageW, RegisterClassExW, SetForegroundWindow, SetTimer,
    SetWindowsHookExW, SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW,
    TrackPopupMenu, EVENT_SYSTEM_FOREGROUND, GWL_EXSTYLE, GWLP_USERDATA, HTCAPTION, HTCLIENT,
    HTTRANSPARENT, HWND_TOPMOST, IDC_ARROW, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, MF_CHECKED, MF_ENABLED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostQuitMessage, SPI_GETWORKAREA,
    SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, TPM_LEFTALIGN,
    TPM_RIGHTBUTTON, ULW_ALPHA, WH_KEYBOARD_LL, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT, WM_APP,
    WM_COMMAND, WM_DESTROY, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

// ── 常量 ──

const TIMER_ID: usize = 1;
const MENU_EXIT: usize = 100;
const MENU_COLLAPSE: usize = 200;
const MENU_ALPHA_25: usize = 205;
const MENU_ALPHA_50: usize = 201;
const MENU_ALPHA_70: usize = 202;
const MENU_ALPHA_90: usize = 203;
const MENU_AUTOSTART: usize = 210;
const MENU_RESET_POS: usize = 220;
const MENU_CAPSULE_BASE: usize = 230;
const MENU_ROW_BASE: usize = 250;
const MENU_ROTATE_BASE: usize = 260;
const MENU_REFRESH_BASE: usize = 270;
const MENU_BG_BASE: usize = 280;
const WM_APP_CTRL: u32 = WM_APP + 1;
const WM_APP_WHEEL: u32 = WM_APP + 2;
const APP_NAME: &str = "TempmonWidget";

const EDGE_MARGIN: f32 = 8.0;
const FONT_PX: f32 = 13.0;
const BAR_H: f32 = 24.0;
const CAP_H: f32 = 24.0;
const PAD_X: f32 = 12.0;
const GAP: f32 = 1.0;

const ROTATE_CHOICES_MS: [u32; 4] = [1000, 2000, 3000, 5000];
const REFRESH_CHOICES_MS: [u32; 3] = [500, 1000, 2000];

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
}

struct App {
    hwnd: HWND,
    job: HANDLE,
    view: *const u8,
    child: Option<Child>,
    last_seq: u32,
    stale_ticks: u32,
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
    h_gpu_usage: Hold,
    h_gpu_vram: Hold,
    h_gpu_temp: Hold,
    h_cpu_temp: Hold,
    h_disk: Hold,
    h_fans: Hold,
    capsule_page: u32,
    page_ticks: u32,
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
}

// ── 配置持久化 ──

fn config_path() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|d| PathBuf::from(d).join("tempmon.conf"))
}

fn load_config() -> Config {
    let mut cfg = Config {
        collapsed: false,
        alpha: 230,
        pos: None,
        capsule_items: CAPS_DEFAULT,
        row_items: ROW_DEFAULT,
        rotate_ms: 2000,
        refresh_ms: 1000,
        bg_mode: 0,
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
                    ("capsule", Ok(v)) => cfg.capsule_items = v as u32,
                    ("row", Ok(v)) => cfg.row_items = v as u32,
                    ("rotate", Ok(v)) if (500..=10000).contains(&v) => cfg.rotate_ms = v as u32,
                    ("refresh", Ok(v)) if (250..=5000).contains(&v) => cfg.refresh_ms = v as u32,
                    ("bg", Ok(v)) if (0..=3).contains(&v) => cfg.bg_mode = v as u8,
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
    cfg
}

fn save_config(cfg: &Config) {
    if let Some(p) = config_path() {
        let text = format!(
            "collapsed={}\nalpha={}\ncapsule={}\nrow={}\nrotate={}\nrefresh={}\nbg={}\nx={}\ny={}\n",
            cfg.collapsed as u8,
            cfg.alpha,
            cfg.capsule_items,
            cfg.row_items,
            cfg.rotate_ms,
            cfg.refresh_ms,
            cfg.bg_mode,
            cfg.pos.map(|q| q.x).unwrap_or(0),
            cfg.pos.map(|q| q.y).unwrap_or(0),
        );
        let _ = std::fs::write(p, text);
    }
}

fn autostart_enabled() -> bool {
    std::process::Command::new("schtasks")
        .args(["/Query", "/TN", APP_NAME])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn set_autostart(enable: bool) {
    let exe = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_default();
    if enable {
        let _ = std::process::Command::new("schtasks")
            .args([
                "/Create", "/TN", APP_NAME, "/SC", "ONLOGON", "/DELAY", "0000:30",
                "/TR", &format!("\"{}\"", exe), "/F",
            ])
            .creation_flags(0x0800_0000)
            .output();
    } else {
        let _ = std::process::Command::new("schtasks")
            .args(["/Delete", "/TN", APP_NAME, "/F"])
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

        // 圆角：毛玻璃用 DWM 系统圆角（雾面层一起圆）；普通用区域裁剪（rgn 窗口无投影）
        if bg_mode0 >= 2 {
            let pref = windows::Win32::Graphics::Dwm::DWMWCP_ROUND;
            let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(33),
                &pref as *const _ as *const core::ffi::c_void,
                4,
            );
        } else {
            let rgn = CreateRoundRectRgn(0, 0, 421, 41, 16, 16);
            let _ = windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, Some(rgn), true);
        }
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
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(hinst), 0);
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(hinst), 0);

        (*ptr).tick()?;
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
                stale_ticks: 99,
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
                h_gpu_usage: Hold::default(),
                h_gpu_vram: Hold::default(),
                h_gpu_temp: Hold::default(),
                h_cpu_temp: Hold::default(),
                h_disk: Hold::default(),
                h_fans: Hold::default(),
                capsule_page: 0,
                page_ticks: 0,
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
            };
            app.ensure_sensor_alive();
            Ok(app)
        }
    }

    fn ensure_sensor_alive(&mut self) {
        let stale = if self.view.is_null() {
            true
        } else {
            let seq = frame_seq(self.view);
            let s = if seq == self.last_seq {
                self.stale_ticks + 1
            } else {
                0
            };
            self.last_seq = seq;
            self.stale_ticks = s;
            self.stale_ticks > 5
        };
        if !stale {
            return;
        }
        if let Some(mut old) = self.child.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Ok(child) = std::process::Command::new(exe)
                .env("TEMPMON_SENSOR", "1")
                .creation_flags(0x0800_0000)
                .spawn()
            {
                unsafe {
                    let _ = AssignProcessToJobObject(self.job, HANDLE(child.as_raw_handle() as _));
                }
                self.child = Some(child);
            }
        }
    }

    fn tick(&mut self) -> windows::core::Result<()> {
        if self.dragging {
            return Ok(());
        }
        if self.collapsed {
            self.page_ticks += 1;
            let stay = (self.rotate_ms / 1000).max(1) as u32;
            if self.page_ticks >= stay {
                self.page_ticks = 0;
                self.capsule_page = self.capsule_page.wrapping_add(1);
            }
        }
        self.update_clickthrough();
        self.ensure_sensor_alive();

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
            let page_idx = self.capsule_page as usize % page_laid.len();
            let (laid, parts_total) = if self.collapsed {
                let (l, t) = page_laid[page_idx].clone();
                (l, t)
            } else {
                let (l, t) = page_laid.remove(0);
                (l, t)
            };

            let w = ((parts_total - GAP * self.scale) + PAD_X * 2.0 * self.scale).ceil() as i32;
            let bar_h = if self.collapsed { CAP_H } else { BAR_H };
            let h = (bar_h * self.scale).ceil() as i32;

            let mut wa = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut RECT as _),
                windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            let m = (EDGE_MARGIN * self.scale) as i32;
            let pos = self.pos.unwrap_or(POINT { x: wa.right - w - m, y: wa.top + m });

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

            if self.bg_mode < 2 {
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
                SourceConstantAlpha: self.alpha,
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
                for (id, label, v) in [
                    (MENU_ALPHA_25, w!("25%"), 64u8),
                    (MENU_ALPHA_50, w!("50%"), 128u8),
                    (MENU_ALPHA_70, w!("70%"), 179u8),
                    (MENU_ALPHA_90, w!("90%"), 230u8),
                ] {
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

    fn apply_bg_effect(&self) {
        unsafe {
            let acrylic = self.bg_mode >= 2;
            apply_acrylic(self.hwnd, acrylic, self.bg_mode == 3);
        }
    }

    fn toggle_collapse(&mut self) {
        self.collapsed = !self.collapsed;
        save_config(&self.as_config());
    }

    fn set_alpha(&mut self, alpha: u8) {
        self.alpha = alpha;
        save_config(&self.as_config());
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
        }
    }

    fn save_current_pos(&mut self) {
        unsafe {
            let mut r = RECT::default();
            if GetWindowRect(self.hwnd, &mut r).is_ok() {
                self.pos = Some(POINT { x: r.left, y: r.top });
                save_config(&self.as_config());
            }
        }
    }
}

// ── 平台辅助 ──

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
                    MENU_ALPHA_25 => (*ptr).set_alpha(64),
                    MENU_ALPHA_50 => (*ptr).set_alpha(128),
                    MENU_ALPHA_70 => (*ptr).set_alpha(179),
                    MENU_ALPHA_90 => (*ptr).set_alpha(230),
                    MENU_AUTOSTART => {
                        let enable = !autostart_enabled();
                        set_autostart(enable);
                    }
                    MENU_RESET_POS => {
                        (*ptr).pos = None;
                        save_config(&(*ptr).as_config());
                    }
                    id if (MENU_CAPSULE_BASE..MENU_CAPSULE_BASE + 9).contains(&id) => {
                        let bit = 1u32 << (id - MENU_CAPSULE_BASE);
                        (*ptr).capsule_items ^= bit;
                        save_config(&(*ptr).as_config());
                    }
                    id if (MENU_ROW_BASE..MENU_ROW_BASE + 5).contains(&id) => {
                        let bit = 1u32 << (id - MENU_ROW_BASE);
                        (*ptr).row_items ^= bit;
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
            }
            LRESULT(0)
        }
        WM_APP_CTRL => {
            if let Some(ptr) = non_null_app(hwnd) {
                let _ = (*ptr).update_clickthrough();
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
                (*ptr).page_ticks = 0;
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
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
