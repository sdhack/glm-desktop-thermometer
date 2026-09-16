import ctypes, time
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')

target = None
EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

def cb(hwnd, lp):
    global target
    if u.IsWindowVisible(hwnd):
        pid = wintypes.DWORD()
        u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        h = k.OpenProcess(0x1000, False, pid.value)
        if h:
            b = ctypes.create_unicode_buffer(512)
            size = wintypes.DWORD(512)
            if k.QueryFullProcessImageNameW(h, 0, b, ctypes.byref(size)):
                if b.value.split(chr(92))[-1] == '360FileBrowser64.exe':
                    target = hwnd
            k.CloseHandle(h)
    return True

u.EnumWindows(EnumWindowsProc(cb), 0)
print('target:', hex(target))

fg = u.GetForegroundWindow()
fg_thread = u.GetWindowThreadProcessId(fg, None)
my_thread = k.GetCurrentThreadId()
u.AttachThreadInput(my_thread, fg_thread, True)
u.SetForegroundWindow(target)
u.AttachThreadInput(my_thread, fg_thread, False)
time.sleep(0.5)
print('foreground==target:', u.GetForegroundWindow() == target)
