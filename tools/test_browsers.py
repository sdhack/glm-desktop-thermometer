import ctypes, time, sys
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')
g = ctypes.WinDLL('gdi32')
ctypes.windll.shcore.SetProcessDpiAwareness(2)

def find_window(substr):
    target = None
    EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, lp):
        nonlocal target
        if u.IsWindowVisible(hwnd):
            b = ctypes.create_unicode_buffer(256)
            u.GetWindowTextW(hwnd, b, 256)
            if substr in b.value and target is None:
                target = hwnd
    # note: nonlocal in nested def inside def-in-def
    return target

# 用简单方式：直接枚举收集
wins = []
EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
def cb2(hwnd, lp):
    if u.IsWindowVisible(hwnd):
        b = ctypes.create_unicode_buffer(256)
        u.GetWindowTextW(hwnd, b, 256)
        if b.value:
            wins.append((hwnd, b.value))
    return True
u.EnumWindows(EnumWindowsProc(cb2), 0)

which = sys.argv[1] if len(sys.argv) > 1 else '360极速浏览器'
target = next((h for h, t in wins if which in t), None)
if not target:
    raise SystemExit(f'未找到窗口: {which}')

fg = u.GetForegroundWindow()
fgt = u.GetWindowThreadProcessId(fg, None)
my = k.GetCurrentThreadId()
u.AttachThreadInput(my, fgt, True)
u.SetForegroundWindow(target)
u.AttachThreadInput(my, fgt, False)
time.sleep(0.5)
print(f'激活 {which}:', u.GetForegroundWindow() == target)
