import ctypes, time
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')

EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
found = []

def cb(hwnd, lp):
    if u.IsWindowVisible(hwnd):
        buf = ctypes.create_unicode_buffer(256)
        u.GetClassNameW(hwnd, buf, 256)
        if buf.value == 'Chrome_WidgetWin_0':
            pid = wintypes.DWORD()
            u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            h = k.OpenProcess(0x1000, False, pid.value)
            if h:
                b = ctypes.create_unicode_buffer(512)
                size = wintypes.DWORD(512)
                if k.QueryFullProcessImageNameW(h, 0, b, ctypes.byref(size)):
                    parts = b.value.split('\\')
                    if parts[-1] == 'WeChatAppEx.exe':
                        found.append(hwnd)
                k.CloseHandle(h)
    return True

u.EnumWindows(EnumWindowsProc(cb), 0)
print('targets:', [hex(x) for x in found])

# Alt 键技巧解除前台切换限制
u.keybd_event(0x12, 0, 0, 0)
time.sleep(0.05)
u.SetForegroundWindow(found[0])
u.keybd_event(0x12, 0, 2, 0)
time.sleep(0.5)
print('foreground==target:', u.GetForegroundWindow() == found[0])
time.sleep(10)
print('after 10s still foreground:', u.GetForegroundWindow() == found[0])
