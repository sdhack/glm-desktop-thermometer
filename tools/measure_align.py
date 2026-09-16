import ctypes
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')

r = wintypes.RECT()
found = None
EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

def cb(hwnd, lp):
    global found
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
u.GetWindowRect(found, ctypes.byref(r))
print(f'温度计窗口: ({r.left},{r.top})-({r.right},{r.bottom}) 高={r.bottom - r.top}')

fg = u.GetForegroundWindow()
b2 = ctypes.create_unicode_buffer(256)
u.GetWindowTextW(fg, b2, 256)
tbuf = ctypes.create_unicode_buffer(256)
u.GetClassNameW(fg, tbuf, 256)
print(f'前台: {b2.value!r} class={tbuf.value!r}')

# 截顶条测按钮带中心（前台窗口的标题栏按钮）
w, h = 1920, 48
g = ctypes.WinDLL('gdi32')
hdc = u.GetDC(0)
mem = g.CreateCompatibleDC(hdc)
bmp = g.CreateCompatibleBitmap(hdc, w, h)
g.SelectObject(mem, bmp)
g.BitBlt(mem, 0, 0, w, h, hdc, 0, 0, 0x00CC0020)

class BMIH(ctypes.Structure):
    _fields_ = [('biSize', ctypes.c_uint32), ('biWidth', ctypes.c_int32), ('biHeight', ctypes.c_int32),
                ('biPlanes', ctypes.c_uint16), ('biBitCount', ctypes.c_uint16), ('biCompression', ctypes.c_uint32),
                ('biSizeImage', ctypes.c_uint32), ('biX', ctypes.c_int32), ('biY', ctypes.c_int32),
                ('biClr', ctypes.c_uint32), ('biClrI', ctypes.c_uint32)]
bmi = BMIH(40, w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
px = (ctypes.c_ubyte * (w * h * 4))()
g.GetDIBits(mem, bmp, 0, h, px, ctypes.byref(bmi), 0)
u.ReleaseDC(0, hdc)

def pixel(x, y):
    o = (y * w + x) * 4
    return px[o + 2], px[o + 1], px[o]

# 按钮带：最右 240px 里逐行密度（只统计关闭按钮附近 x1880-1916，最稳定的锚点）
rows = [y for y in range(h)
        if sum(1 for x in range(1880, 1917)
               if sum(abs(a - c) for a, c in zip(pixel(x, y), (243, 243, 243))) > 60) >= 2]
if rows:
    bc = (min(rows) + max(rows)) / 2
    print(f'关闭按钮行 {min(rows)}-{max(rows)} 中心 {bc}')
else:
    print('未找到按钮行')

# 温度计可见胶囊：在窗口矩形内找非背景行的实际范围
wx0, wy0, wx1, wy1 = r.left, r.top, r.right, r.bottom
vrows = []
for y in range(max(0, wy0), min(h, wy1)):
    c = sum(1 for x in range(wx0 + 4, wx1 - 4)
            if sum(abs(a - cc) for a, cc in zip(pixel(x, y), (243, 243, 243))) > 60)
    if c >= 5:
        vrows.append(y)
if vrows:
    print(f'温度计可见内容行 {min(vrows)}-{max(vrows)} 中心 {(min(vrows)+max(vrows))/2}')
