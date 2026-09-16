# 交替激活 360 浏览器与 ZCode，模拟用户复现场景
import ctypes, time, sys
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')

EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

def find_by_exe(exe):
    hits = []
    def cb(hwnd, lp):
        if u.IsWindowVisible(hwnd):
            pid = wintypes.DWORD()
            u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            h = k.OpenProcess(0x1000, False, pid.value)
            if h:
                b = ctypes.create_unicode_buffer(512)
                size = wintypes.DWORD(512)
                if k.QueryFullProcessImageNameW(h, 0, b, ctypes.byref(size)):
                    if b.value.split(chr(92))[-1].lower() == exe:
                        hits.append(hwnd)
                k.CloseHandle(h)
        return True
    u.EnumWindows(EnumWindowsProc(cb), 0)
    return hits

def activate(hwnd):
    fg = u.GetForegroundWindow()
    fg_thread = u.GetWindowThreadProcessId(fg, None)
    my_thread = k.GetCurrentThreadId()
    u.AttachThreadInput(my_thread, fg_thread, True)
    u.SetForegroundWindow(hwnd)
    u.AttachThreadInput(my_thread, fg_thread, False)

rounds = int(sys.argv[1]) if len(sys.argv) > 1 else 3
chrome = find_by_exe('360ChromeX.exe')
zcode = find_by_exe('ZCode.exe')
print('chrome:', [hex(h) for h in chrome], 'zcode:', [hex(h) for h in zcode])
if not chrome or not zcode:
    sys.exit('missing windows')
for i in range(rounds):
    activate(chrome[0]); print(f'round {i}: 360'); time.sleep(4)
    activate(zcode[0]);  print(f'round {i}: ZCode'); time.sleep(4)
print('done')
