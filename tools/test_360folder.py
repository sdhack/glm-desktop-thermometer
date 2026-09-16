import ctypes, time
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')
g = ctypes.WinDLL('gdi32')
ctypes.windll.shcore.SetProcessDpiAwareness(2)

# 找 360FileBrowser64 窗口
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
if not target:
    raise SystemExit('未找到 360FileBrowser64 窗口')
r = wintypes.RECT()
u.GetWindowRect(target, ctypes.byref(r))
print(f'360文件管理器窗口: ({r.left},{r.top})-({r.right},{r.bottom})')

# 点击其标题栏中部激活
u.SetCursorPos((r.left + r.right) // 2, r.top + 10)
time.sleep(0.1)
u.mouse_event(0x0002, 0, 0, 0, 0)
time.sleep(0.04)
u.mouse_event(0x0004, 0, 0, 0, 0)
time.sleep(10)  # 给温度计 10s 同步窗口

print('foreground==360文件夹:', u.GetForegroundWindow() == target)

# 截顶条 400px 高（覆盖按钮行与温度计），测量
w, h = 1920, 400
hdc = u.GetDC(0)
mem = g.CreateCompatibleDC(hdc)
bmp = g.CreateCompatibleBitmap(hdc, w, h)
g.SelectObject(mem, bmp)
g.BitBlt(mem, 0, 0, w, h, hdc, 0, 0, 0x00CC0020)
class BMIH(ctypes.Structure):
    _fields_ = [(n, t) for n, t in [('biSize', ctypes.c_uint32), ('biWidth', ctypes.c_int32), ('biHeight', ctypes.c_int32), ('biPlanes', ctypes.c_uint16), ('biBitCount', ctypes.c_uint16), ('biCompression', ctypes.c_uint32), ('biSizeImage', ctypes.c_uint32), ('biX', ctypes.c_int32), ('biY', ctypes.c_int32), ('biClr', ctypes.c_uint32), ('biClrI', ctypes.c_uint32)]]
bmi = BMIH(40, w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
px = (ctypes.c_ubyte * (w * h * 4))()
g.GetDIBits(mem, bmp, 0, h, px, ctypes.byref(bmi), 0)
u.ReleaseDC(0, hdc)

def pixel(x, y):
    o = (y * w + x) * 4
    return px[o + 2], px[o + 1], px[o]

# 关闭按钮（右上角 x 1860-1916）
btn = [y for y in range(60)
       if sum(1 for x in range(1860, 1917)
              if sum(abs(a - c) for a, c in zip(pixel(x, y), (243, 243, 243))) > 60) >= 2]
print('360文件夹关闭按钮行:', (min(btn), max(btn)) if btn else None, '中心', (min(btn) + max(btn)) / 2 if btn else None)

# 温度计（绿点定位）
gcols = [x for x in range(w) for y in range(60)
         if (lambda p: p[1] > 150 and p[1] - p[0] > 40 and p[1] - p[2] > 40)(pixel(x, y))]
if gcols:
    gx0, gx1 = min(gcols), max(gcols)
    vrows = [y for y in range(60)
             if sum(1 for x in range(max(0, gx0 - 5), min(gx1 + 80, w))
                    if sum(abs(a - c) for a, c in zip(pixel(x, y), (243, 243, 243))) > 60) >= 5]
    print(f'温度计列范围 {gx0}-{gx1}，可见行 {min(vrows)}-{max(vrows)} 中心 {(min(vrows)+max(vrows))/2}')
else:
    print('顶部 60px 内未找到温度计（可能在别处）')
