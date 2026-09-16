"""UIA 基准探针：读前台窗口关闭按钮真实矩形（PowerShell UIA）+ 温度计胶囊中心，给出对齐差。

用法: python tools/probe_btn.py <窗口标题子串> [...]
"""
import base64
import ctypes
import subprocess
import sys
import time
from ctypes import wintypes

u = ctypes.WinDLL('user32')
k = ctypes.WinDLL('kernel32')
g = ctypes.WinDLL('gdi32')
ctypes.windll.shcore.SetProcessDpiAwareness(2)
EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

PS_TEMPLATE = r'''
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [Windows.Automation.AutomationElement]::FromHandle(%HWND%)
function FindBtn($root, $scope, $name) {
  $c1 = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button)
  $c2 = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, $name)
  $cond = New-Object Windows.Automation.AndCondition($c1, $c2)
  return $root.FindFirst($scope, $cond)
}
$btn = FindBtn $root ([Windows.Automation.TreeScope]::Children) "关闭"
if (-not $btn) { $btn = FindBtn $root ([Windows.Automation.TreeScope]::Children) "Close" }
if (-not $btn) { $btn = FindBtn $root ([Windows.Automation.TreeScope]::Descendants) "关闭" }
if (-not $btn) { $btn = FindBtn $root ([Windows.Automation.TreeScope]::Descendants) "Close" }
if (-not $btn) {
  $cond = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button)
  $all = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $cond)
  $fgr = $root.Current.BoundingRectangle
  $bestx = -2147483648; $best = $null
  foreach ($b in $all) {
    $r = $b.Current.BoundingRectangle
    if ($r.X -gt ($fgr.X + $fgr.Width - 200) -and $r.Y -lt ($fgr.Y + 60) -and $r.Width -le 80 -and $r.Height -le 60 -and $r.X -gt $bestx) {
      $bestx = $r.X; $best = $b
    }
  }
  $btn = $best
}
if ($btn) {
  $r = $btn.Current.BoundingRectangle
  Write-Output ($r.Top + $r.Height / 2)
} else { Write-Output "NONE" }
'''


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


def uia_close_center(fg_hwnd):
    ps = PS_TEMPLATE.replace('%HWND%', str(fg_hwnd))
    enc = base64.b64encode(ps.encode('utf-16-le')).decode()
    out = subprocess.run(
        ['powershell', '-NoProfile', '-EncodedCommand', enc],
        capture_output=True, timeout=60,
    )
    s = out.stdout.decode('gbk', errors='replace').strip()
    if not s or s == 'NONE':
        return None
    try:
        return float(s.splitlines()[-1])
    except ValueError:
        return None


def capsule_center(tr):
    """温度计窗口几何中心 + 可见内容中心"""
    x0, y0 = max(tr[0], 0), max(tr[1], 0)
    w, h = tr[2] - tr[0], tr[3] - tr[1]
    hdc = u.GetDC(0)
    mem = g.CreateCompatibleDC(hdc)
    bmp = g.CreateCompatibleBitmap(hdc, w, h)
    g.SelectObject(mem, bmp)
    g.BitBlt(mem, 0, 0, w, h, hdc, x0, y0, 0x00CC0020)

    class BMIH(ctypes.Structure):
        _fields_ = [('biSize', ctypes.c_uint32), ('biWidth', ctypes.c_int32), ('biHeight', ctypes.c_int32),
                    ('biPlanes', ctypes.c_uint16), ('biBitCount', ctypes.c_uint16),
                    ('biCompression', ctypes.c_uint32), ('biSizeImage', ctypes.c_uint32),
                    ('biX', ctypes.c_int32), ('biY', ctypes.c_int32),
                    ('biClr', ctypes.c_uint32), ('biClrImportant', ctypes.c_uint32)]
    bmi = BMIH(40, w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
    px = (ctypes.c_ubyte * (w * h * 4))()
    g.GetDIBits(mem, bmp, 0, h, px, ctypes.byref(bmi), 0)
    u.ReleaseDC(0, hdc)
    rows = []
    for y in range(h):
        c = sum(1 for x in range(4, w - 4)
                if sum(abs(a - cc) for a, cc in zip(px[(y * w + x) * 4:(y * w + x) * 4 + 3], (243, 243, 243))) > 60)
        if c >= 5:
            rows.append(y + y0)
    vis = (min(rows) + max(rows)) / 2 if rows else None
    geo = (tr[1] + tr[3]) / 2
    return geo, vis


for name in sys.argv[1:]:
    hit = None
    for h, t in all_visible():
        if name in t and exe_name(h) != 'tempmon.exe':
            hit = h
            break
    if not hit:
        print(f'== {name}: 未找到窗口')
        continue
    u.ShowWindow(hit, 3)  # SW_MAXIMIZE
    time.sleep(0.3)
    u.keybd_event(0x12, 0, 0, 0)  # ALT 解锁 SetForegroundWindow
    fg0 = u.GetForegroundWindow()
    fgt = u.GetWindowThreadProcessId(fg0, None)
    my = k.GetCurrentThreadId()
    u.AttachThreadInput(my, fgt, True)
    u.BringWindowToTop(hit)
    u.SetForegroundWindow(hit)
    u.AttachThreadInput(my, fgt, False)
    u.keybd_event(0x12, 0, 2, 0)
    time.sleep(2.5)
    fg = u.GetForegroundWindow()
    fr = wintypes.RECT()
    u.GetWindowRect(fg, ctypes.byref(fr))
    tr = None
    for h, t in all_visible():
        if exe_name(h) == 'tempmon.exe':
            r = wintypes.RECT()
            u.GetWindowRect(h, ctypes.byref(r))
            if r.right - r.left > 10:
                tr = (r.left, r.top, r.right, r.bottom)
                break
    if tr is None:
        print(f'== {name}: 未找到温度计窗口')
        continue
    btn_cy = uia_close_center(fg)
    geo, vis = capsule_center(tr)
    if btn_cy is None:
        print(f'== {name}: UIA 关闭按钮未暴露（窗口 rect={fr.left},{fr.top}）胶囊几何中心={geo:.1f}')
        continue
    d = geo - btn_cy
    print(f'== {name}: UIA按钮中心{btn_cy:.1f} 胶囊几何中心{geo:.1f}(可见{vis}) 差{d:+.1f}px -> '
          + ('PASS' if abs(d) <= 2 else 'FAIL'))
