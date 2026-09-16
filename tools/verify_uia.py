import ctypes, sys, time
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')
g = ctypes.WinDLL('gdi32')
ctypes.windll.shcore.SetProcessDpiAwareness(2)

EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)


def all_visible():
    wins = []

    def cb(h, lp):
        if u.IsWindowVisible(h):
            b = ctypes.create_unicode_buffer(256)
            u.GetWindowTextW(h, b, 256)
            wins.append((h, b.value))
        return True
    u.EnumWindows(EnumProc(cb), 0)
    return wins


def exe_name(hwnd):
    pid = wintypes.DWORD()
    u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    h = k.OpenProcess(0x1000, False, pid.value)
    name = '?'
    if h:
        b = ctypes.create_unicode_buffer(512)
        s = wintypes.DWORD(512)
        if k.QueryFullProcessImageNameW(h, 0, b, ctypes.byref(s)):
            name = b.value.split(chr(92))[-1]
        k.CloseHandle(h)
    return name


def activate(hwnd):
    u.ShowWindow(hwnd, 3)  # SW_MAXIMIZE：对齐锚点在屏幕顶条，最大化后才可测
    # 最小化抢前台的 360 安全卫士类窗口
    for h, t in all_visible():
        if h != hwnd:
            cls = ctypes.create_unicode_buffer(256)
            u.GetClassNameW(h, cls, 256)
            if cls.value == 'Q360SafeMainClass':
                u.ShowWindow(h, 6)  # SW_MINIMIZE
    time.sleep(0.3)
    # ALT 键解锁 SetForegroundWindow + AttachThreadInput 双保险
    u.keybd_event(0x12, 0, 0, 0)
    fg = u.GetForegroundWindow()
    fgt = u.GetWindowThreadProcessId(fg, None)
    my = k.GetCurrentThreadId()
    u.AttachThreadInput(my, fgt, True)
    u.BringWindowToTop(hwnd)
    u.SetForegroundWindow(hwnd)
    u.AttachThreadInput(my, fgt, False)
    u.keybd_event(0x12, 0, 2, 0)
    time.sleep(0.5)


def measure():
    """联合区域截屏：前台标题栏条带 ∪ 温度计窗口。
    返回 (按钮带中心y, 胶囊中心y, 温度计rect, 前台rect, 区域y0)"""
    thermo = None
    tr = wintypes.RECT()
    for h, t in all_visible():
        if exe_name(h) == 'tempmon.exe':
            r = wintypes.RECT()
            u.GetWindowRect(h, ctypes.byref(r))
            if r.right - r.left > 10:
                thermo, tr = h, r
                break
    if thermo is None:
        return None
    fg = u.GetForegroundWindow()
    fg_title = ctypes.create_unicode_buffer(256)
    u.GetWindowTextW(fg, fg_title, 256)
    fr = wintypes.RECT()
    u.GetWindowRect(fg, ctypes.byref(fr))

    y0 = min(fr.top, tr.top) - 2
    y1 = max(fr.top + 48, tr.bottom) + 2
    x1 = max(fr.right, tr.right) + 2
    w, h = x1, y1 - y0

    class BMIH(ctypes.Structure):
        _fields_ = [('biSize', ctypes.c_uint32), ('biWidth', ctypes.c_int32), ('biHeight', ctypes.c_int32),
                    ('biPlanes', ctypes.c_uint16), ('biBitCount', ctypes.c_uint16),
                    ('biCompression', ctypes.c_uint32), ('biSizeImage', ctypes.c_uint32),
                    ('biX', ctypes.c_int32), ('biY', ctypes.c_int32),
                    ('biClr', ctypes.c_uint32), ('biClrImportant', ctypes.c_uint32)]

    hdc = u.GetDC(0)
    mem = g.CreateCompatibleDC(hdc)
    bmp = g.CreateCompatibleBitmap(hdc, w, h)
    g.SelectObject(mem, bmp)
    g.BitBlt(mem, 0, 0, w, h, hdc, 0, y0, 0x00CC0020)
    bmi = BMIH(40, w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
    px = (ctypes.c_ubyte * (w * h * 4))()
    g.GetDIBits(mem, bmp, 0, h, px, ctypes.byref(bmi), 0)
    u.ReleaseDC(0, hdc)

    def pixel(x, y):
        yy = y - y0
        o = (yy * w + x) * 4
        return px[o + 2], px[o + 1], px[o]

    def diff(x, y):
        return sum(abs(a - c) for a, c in zip(pixel(x, y), (243, 243, 243))) > 60

    # 前台关闭按钮：X 字形在 fr.right-82..fr.right-27 附近，取 90px 带避开圆角边框
    band_l = max(fr.right - 90, fr.left)
    band_r = min(fr.right - 16, 1919)
    rows = [y for y in range(max(0, fr.top + 10), min(fr.top + 48, h + y0))
            if sum(1 for x in range(band_l, band_r) if diff(x, y)) >= 2]
    btn_cy = (min(rows) + max(rows)) / 2 if rows else None

    # 温度计可见胶囊行
    vrows = [y for y in range(tr.top, tr.bottom)
             if sum(1 for x in range(tr.left + 4, tr.right - 4) if diff(x, y)) >= 5]
    cap_cy = (min(vrows) + max(vrows)) / 2 if vrows else None
    return btn_cy, cap_cy, (tr.left, tr.top, tr.right, tr.bottom), (fr.left, fr.top, fr.right, fr.bottom), fg_title.value


targets = sys.argv[1:]
if not targets:
    raise SystemExit('usage: verify_uia.py <窗口标题子串> [...]')

for name in targets:
    hit = None
    want_exe = name.startswith('exe:') and name[4:]
    for h, t in all_visible():
        if exe_name(h) == 'tempmon.exe':
            continue
        r0 = wintypes.RECT()
        u.GetWindowRect(h, ctypes.byref(r0))
        if r0.right - r0.left <= 10:
            continue
        if want_exe:
            if exe_name(h) == want_exe:
                hit = h
                break
        elif name in t:
            hit = h
            break
    if not hit:
        print(f'== {name}: 未找到窗口')
        continue
    activate(hit)
    # 连续观测 16s：钉定模式下 y 同步有 10s 门控 + 2 次表决，收敛即记 PASS
    converged = None
    last = None
    t0 = time.time()
    while time.time() - t0 < 16:
        r = measure()
        if r and r[0] is not None and r[1] is not None:
            last = r
            d = r[1] - r[0]
            if abs(d) <= 2:
                converged = time.time() - t0
                break
        time.sleep(0.8)
    if not last:
        print(f'== {name}: 按钮/胶囊未检出 {r}')
        continue
    btn_cy, cap_cy, tr, fr, fg_title = last
    d = cap_cy - btn_cy
    tag = f'收敛{converged:.1f}s' if converged else '未收敛'
    print(f'== {name} (fg={fg_title!r}): 按钮{btn_cy:.1f} 胶囊{cap_cy:.1f} 差{d:+.1f}px '
          f'thermo={tr} -> ' + ('PASS' if converged else 'FAIL') + f' [{tag}]')
