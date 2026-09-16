import ctypes
from ctypes import wintypes

u = ctypes.WinDLL('user32')
d = ctypes.WinDLL('dwmapi')

EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
u.EnumWindows.argtypes = [EnumWindowsProc, wintypes.LPARAM]

results = []

def cb(hwnd, lp):
    if not u.IsWindowVisible(hwnd):
        return True
    buf = ctypes.create_unicode_buffer(256)
    u.GetClassNameW(hwnd, buf, 256)
    cls = buf.value
    tbuf = ctypes.create_unicode_buffer(256)
    u.GetWindowTextW(hwnd, tbuf, 256)
    title = tbuf.value
    if not any(k in cls for k in ('Chrome', 'WeChat', 'Weixin', 'Cef', 'Booter')) and \
       not any(k in title for k in ('微信', 'WeChat')):
        return True
    r = wintypes.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    dwr = wintypes.RECT()
    d.DwmGetWindowAttribute(hwnd, 9, ctypes.byref(dwr), ctypes.sizeof(dwr))  # EXTENDED_FRAME_BOUNDS
    pid = wintypes.DWORD()
    u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    results.append((hwnd, pid.value, cls, title, r, dwr))
    return True

u.EnumWindows(EnumWindowsProc(cb), 0)

# 进程名
ps = ctypes.WinDLL('kernel32')
for hwnd, pid, cls, title, r, dwr in results:
    h = ps.OpenProcess(0x1000, False, pid)  # QUERY_LIMITED
    name = '?'
    if h:
        b = ctypes.create_unicode_buffer(512)
        size = wintypes.DWORD(512)
        if ps.QueryFullProcessImageNameW(h, 0, b, ctypes.byref(size)):
            name = b.value.split('\\')[-1]
        ps.CloseHandle(h)
    print(f'hwnd=0x{hwnd:X} pid={pid} proc={name}')
    print(f'  class={cls!r} title={title!r}')
    print(f'  GetWindowRect=({r.left},{r.top})-({r.right},{r.bottom})  DWMbounds=({dwr.left},{dwr.top})-({dwr.right},{dwr.bottom})')
