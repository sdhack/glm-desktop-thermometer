import ctypes, sys
from ctypes import wintypes
from PIL import ImageGrab

u32 = ctypes.windll.user32
WDA_NONE, WDA_EXCLUDE = 0x0, 0x11

# 找温度计窗口
hwnd = None
def cb(h, _):
    global hwnd
    buf = ctypes.create_unicode_buffer(256)
    u32.GetWindowTextW(h, buf, 256)
    if buf.value == "GLM桌面温度计":
        hwnd = h
        return False
    return True
WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
u32.EnumWindows(WNDENUMPROC(cb), 0)
assert hwnd, "window not found"
r = wintypes.RECT(); u32.GetWindowRect(hwnd, ctypes.byref(r))
x0, y0, w, h = r.left, r.top, r.right-r.left, r.bottom-r.top
print("widget rect:", x0, y0, w, h)

# 与 Rust 相同：排除自身后抓整行
wa = wintypes.RECT(); u32.SystemParametersInfoW(0x0030, 0, ctypes.byref(wa), 0)
strip_x, strip_w = wa.left, wa.right-wa.left
print("workarea:", wa.left, wa.top, wa.right, wa.bottom)
assert u32.SetWindowDisplayAffinity(hwnd, WDA_EXCLUDE), "SetWindowDisplayAffinity failed"
import time; time.sleep(0.05)
img = ImageGrab.grab(bbox=(strip_x, y0, strip_x+strip_w, y0+h), all_screens=True)
u32.SetWindowDisplayAffinity(hwnd, WDA_NONE)
img.save(r"D:\GLM桌面温度计260912\tools\strip.png")

# 复刻 Rust 的 looks_like_text
import numpy as np
a = np.asarray(img.convert("RGB")).astype(np.int32)
g = (a[:,:,0]*299 + a[:,:,1]*587 + a[:,:,2]*114)//1000
edge = (np.abs(g[:,1:]-g[:,:-1]) + np.abs(g[1:,:]-g[:-1,:])) > 64
edge = edge[:-1,:-1]
H, W = edge.shape
def stats(ox, ww):
    e = edge[:, ox:ox+ww]
    dens = e.mean()
    band = max(H//3, 6)
    hits = 0; y = 0
    while y+band <= H:
        if e[y:y+band].mean() > 0.03: hits += 1
        y += band
    return dens, hits

def looks_like_text(ox, ww):
    dens, hits = stats(ox, ww)
    return (0.015 < dens < 0.5) and hits >= 2, dens, hits

mw = max(w, 300)
ox0 = x0 - strip_x
ok, dens, hits = looks_like_text(ox0, mw)
print(f"current pos [{x0},{x0+mw}): density={dens:.3f} bands={hits} text={ok}")
for s in range(1, 4):
    for nx in (x0 - s*200, x0 + s*200):
        if nx < wa.left or nx+mw > wa.right: continue
        ok, dens, hits = looks_like_text(nx-strip_x, mw)
        print(f"candidate [{nx},{nx+mw}): density={dens:.3f} bands={hits} text={ok}")
