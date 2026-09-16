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
                    if b.value.split('\\')[-1] == 'WeChatAppEx.exe':
                        found.append(hwnd)
                k.CloseHandle(h)
    return True

u.EnumWindows(EnumWindowsProc(cb), 0)
hwnd = found[0]
r = wintypes.RECT()
u.GetWindowRect(hwnd, ctypes.byref(r))
# 点击窗口顶部中间偏左的标题栏区域（避开右上角按钮和温度计）
x = (r.left + r.right) // 2 - 200
y = r.top + 15
if r.top < 0:
    y = 8
print(f'clicking ({x},{y}) on hwnd 0x{hwnd:X}')
u.SetCursorPos(x, y)
time.sleep(0.1)
u.mouse_event(0x0002, 0, 0, 0, 0)  # left down
time.sleep(0.05)
u.mouse_event(0x0004, 0, 0, 0, 0)  # left up
time.sleep(0.5)
print('foreground==target:', u.GetForegroundWindow() == hwnd)
