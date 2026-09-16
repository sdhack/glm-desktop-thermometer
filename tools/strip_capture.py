import ctypes
from ctypes import wintypes
import time

u = ctypes.WinDLL('user32')

# 1. 激活微信内置浏览器窗口
target = None
EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
found = []
def cb(hwnd, lp):
    buf = ctypes.create_unicode_buffer(256)
    u.GetClassNameW(hwnd, buf, 256)
    cls = buf.value
    if u.IsWindowVisible(hwnd) and cls == 'Chrome_WidgetWin_0':
        tbuf = ctypes.create_unicode_buffer(256)
        u.GetWindowTextW(hwnd, tbuf, 256)
        pid = wintypes.DWORD()
        u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        k = ctypes.WinDLL('kernel32')
        h = k.OpenProcess(0x1000, False, pid.value)
        name = ''
        if h:
            b = ctypes.create_unicode_buffer(512); size = wintypes.DWORD(512)
            if k.QueryFullProcessImageNameW(h, 0, b, ctypes.byref(size)):
                name = b.value.split('\\')[-1]
            k.CloseHandle(h)
        if name == 'WeChatAppEx.exe':
            found.append(hwnd)
    return True
u.EnumWindows(EnumWindowsProc(cb), 0)
print('WeChatAppEx windows:', [hex(h) for h in found])
if not found:
    raise SystemExit('not found')

hwnd = found[0]
# SW_RESTORE = 9 之前是最大化, 恢复成常规窗口更接近"激活浏览"场景? 先试 ShowWindow + SetForeground
u.ShowWindow(hwnd, 9)
time.sleep(0.3)
u.SetForegroundWindow(hwnd)
time.sleep(1.0)

fg = u.GetForegroundWindow()
print('foreground == target:', fg == hwnd)

# 2. 截屏顶部 48px（排除温度计自身: 临时用 WDA? 简化: 温度计在右上角会入镜，
#    但我们要看的是按钮行位置，温度计自身像素可辨认后忽略）
gdi = ctypes.WinDLL('gdi32')
user = ctypes.WinDLL('user32')
hdc = user.GetDC(0)
mem = gdi.CreateCompatibleDC(hdc)
bmp = gdi.CreateCompatibleBitmap(hdc, 1920, 48)
gdi.SelectObject(mem, bmp)
gdi.BitBlt(mem, 0, 0, 1920, 48, hdc, 0, 0, 0x00CC0020)

class BMIH(ctypes.Structure):
    _fields_ = [('biSize', wintypes.DWORD), ('biWidth', wintypes.LONG), ('biHeight', wintypes.LONG),
                ('biPlanes', wintypes.WORD), ('biBitCount', wintypes.WORD), ('biCompression', wintypes.DWORD),
                ('biSizeImage', wintypes.DWORD), ('biXPels', wintypes.LONG), ('biYPels', wintypes.LONG),
                ('biClrUsed', wintypes.DWORD), ('biClrImportant', wintypes.DWORD)]
bmi = BMIH(ctypes.sizeof(BMIH), 1920, -48, 1, 32, 0, 0, 0, 0, 0, 0)
px = (ctypes.c_ubyte * (1920 * 48 * 4))()
gdi.GetDIBits(mem, bmp, 0, 48, px, ctypes.byref(bmi), 0)

# 3. 保存 BMP（直接文件头写入）
import struct
fh = b'BM' + struct.pack('<IHHI', 54 + len(px), 0, 0, 54)
ih = struct.pack('<IiiHHIIiiII', 40, 1920, 48, 1, 32, 0, len(px), 0, 0, 0, 0)
open('tools/topstrip.bmp', 'wb').write(fh + ih + bytes(px))

# 4. 每行边缘密度统计（简单梯度），找按钮行
prev = None
print('\nrow: edge-density (top 48 rows)')
for y in range(48):
    c = 0
    row = y * 1920 * 4
    for x in range(1, 1919):
        o = row + x * 4
        b1, g1, r1 = px[o], px[o+1], px[o+2]
        b0, g0, r0 = px[o-4], px[o-3], px[o-2]
        if abs(b1-b0) + abs(g1-g0) + abs(r1-r0) > 60:
            c += 1
    if c:
        print(f'{y:3d}: {"#" * min(c // 8, 60)} {c}')
user.ReleaseDC(0, hdc)
