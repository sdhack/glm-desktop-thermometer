import ctypes, time
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')

# 找温度计窗口位置
found = None
EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

def cb(hwnd, lp):
    global found
    if u.IsWindowVisible(hwnd):
        pid = wintypes.DWORD()
        u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        h = k.OpenProcess(0x1000, False, pid.value)
        if h:
            b = ctypes.create_unicode_buffer(512)
            size = wintypes.DWORD(512)
            if k.QueryFullProcessImageNameW(h, 0, b, ctypes.byref(size)):
                if b.value.split(chr(92))[-1] == 'tempmon.exe':
                    found = hwnd
            k.CloseHandle(h)
    return True

u.EnumWindows(EnumWindowsProc(cb), 0)
r = wintypes.RECT()
u.GetWindowRect(found, ctypes.byref(r))
sx, sy = (r.left + r.right) // 2, (r.top + r.bottom) // 2
print(f'温度计窗口 ({r.left},{r.top})-({r.right},{r.bottom})')

# 模拟 Ctrl 按住 + 左键拖到屏幕中部（360 浏览器内容区上）
VK_CTRL = 0x11
MOUSEEVENTF_MOVE = 0x0001
u.keybd_event(VK_CTRL, 0, 0, 0)
u.SetCursorPos(sx, sy)
time.sleep(0.1)
u.mouse_event(0x0002, 0, 0, 0, 0)  # left down
time.sleep(0.15)
# 分步移动到目标（产生真实 WM_MOVE 流）
tx, ty = 960, 540
for i in range(1, 21):
    u.SetCursorPos(sx + (tx - sx) * i // 20, sy + (ty - sy) * i // 20)
    time.sleep(0.03)
time.sleep(0.2)
u.mouse_event(0x0004, 0, 0, 0, 0)  # left up
u.keybd_event(VK_CTRL, 0, 2, 0)    # Ctrl up
time.sleep(0.5)
r2 = wintypes.RECT()
u.GetWindowRect(found, ctypes.byref(r2))
print(f'拖拽后窗口: ({r2.left},{r2.top})-({r2.right},{r2.bottom})')
print('拖到屏幕中部完成——该位置下面是浏览器内容，避让应把温度计挪开')
