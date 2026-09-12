//! 显隐策略引擎（M2）
//! 规则：置顶常显是默认；唯一隐藏条件 = 前台窗口全屏 && 该应用（或已知播放器）
//! 的系统媒体会话（SMTC）正在播放。信号融合判定，不维护"游戏/视频"进程黑名单。

#![allow(non_snake_case)]

use std::future::{Future, IntoFuture};
use std::task::Poll;
use windows::core::HSTRING;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_NAME_WIN32,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
};

/// 已知媒体类前台进程（与 SMTC 判定取或，降低漏判）
const KNOWN_PLAYERS: &[&str] = &[
    "chrome", "msedge", "edge", "firefox", "opera", "brave",
    "vlc", "potplayer", "mpv", "mpc", "wmplayer", "movies", "splayer",
    "iqiyi", "bilibili", "cloudmusic", "qqmusic", "kugou", "kuwo",
    "youtube", "netflix", "spotify", "douyu", "huya", "qqplayer",
    "thunder", "xunlei", "storm", "baofeng", "sinhv", "tilted",
];

pub struct Strategy {
    smtc: Option<GlobalSystemMediaTransportControlsSessionManager>,
    self_pid: u32,
}

impl Strategy {
    pub fn new() -> Self {
        unsafe {
            // WinRT 调用需要 COM 公寓；已初始化时返回错误可忽略
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let smtc = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .ok()
            .and_then(block_on)
            .and_then(|r| r.ok());
        Self {
            smtc,
            self_pid: std::process::id(),
        }
    }

    /// 前台窗口所属进程 PID（排除自身）
    pub fn foreground_pid(&self) -> Option<u32> {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_invalid() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            (pid != 0 && pid != self.self_pid).then_some(pid)
        }
    }

    /// 前台是否全屏（不含隐藏判定，供游戏档采样切换使用）
    pub fn foreground_fullscreen(&self) -> bool {
        self.fullscreen_foreground_exe().is_some()
    }

    /// 是否应当隐藏：前台全屏 + 对应媒体正在播放
    pub fn should_hide(&self) -> bool {
        let Some(stem) = self.fullscreen_foreground_exe() else {
            return false;
        };
        let playing = self.playing_aumids();
        if playing.is_empty() {
            return false;
        }
        if KNOWN_PLAYERS.iter().any(|k| stem.contains(k)) {
            return true;
        }
        let s = norm(&stem);
        playing.iter().any(|a| {
            let a = norm(a);
            !a.is_empty() && (a.contains(&s) || s.contains(&a))
        })
    }

    /// 当前系统里"正在播放"的媒体会话来源 AUMID 列表
    fn playing_aumids(&self) -> Vec<String> {
        let Some(mgr) = &self.smtc else { return Vec::new() };
        let Ok(sessions) = mgr.GetSessions() else { return Vec::new() };
        let Ok(mut it) = sessions.First() else { return Vec::new() };
        let mut out = Vec::new();
        loop {
            match it.Current() {
                Ok(session) => {
                    if let Ok(pi) = session.GetPlaybackInfo() {
                        if let Ok(st) = pi.PlaybackStatus() {
                            if st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
                            {
                                if let Ok(aumid) = session.SourceAppUserModelId() {
                                    out.push(aumid.to_string());
                                }
                            }
                        }
                    }
                    if it.MoveNext().unwrap_or(false) {
                        continue;
                    }
                    break;
                }
                Err(_) => break,
            }
        }
        out
    }

    /// 前台窗口若铺满某显示器则返回其进程名主干（排除自身）
    fn fullscreen_foreground_exe(&self) -> Option<String> {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_invalid() || fg == std::mem::zeroed() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            if pid == 0 || pid == self.self_pid {
                return None;
            }
            let mut r = RECT::default();
            GetWindowRect(fg, &mut r).ok()?;
            let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(mon, &mut mi).as_bool() {
                return None;
            }
            if r != mi.rcMonitor {
                return None; // 非全屏
            }
            let exe = process_image_stem(pid)?;
            Some(exe)
        }
    }
}

/// 取小写化的纯字母数字串，用于 AUMID 与进程名的宽松匹配
fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// 极简 block_on：仅启动阶段使用（SMTC 初始化很快，忙等可接受）
fn block_on<F: std::future::IntoFuture>(fut: F) -> Option<F::Output> {
    use std::task::{Context, RawWaker, RawWakerVTable, Waker};

    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);

    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut pinned = std::pin::pin!(fut.into_future());
    loop {
        match pinned.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return Some(v),
            Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

fn process_image_stem(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        r.ok()?;
        let path = HSTRING::from_wide(&buf[..len as usize]).to_string();
        let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
        Some(
            name.trim_end_matches(".exe")
                .trim_end_matches(".EXE")
                .to_lowercase(),
        )
    }
}
