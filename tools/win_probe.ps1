Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public struct RECT2 { public int L, T, R, B; }
public static class WEnum {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lp);
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lp);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT2 r);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT2 r, int size);
}
'@
$found = @()
$cb = {
  param($h, $lp)
  if ([WEnum]::IsWindowVisible($h)) {
    $t = New-Object System.Text.StringBuilder 256
    $c = New-Object System.Text.StringBuilder 256
    [void][WEnum]::GetWindowText($h, $t, 256)
    [void][WEnum]::GetClassName($h, $c, 256)
    $title = $t.ToString()
    $cls = $c.ToString()
    if ($cls -match 'Chrome|WeChat|Weixin|Cef' -or $title -match '微信|WeChat') {
      $r = New-Object RECT2
      [void][WEnum]::GetWindowRect($h, [ref]$r)
      $dwr = New-Object RECT2
      [WEnum]::DwmGetWindowAttribute($h, 9, [ref]$dwr, 16) | Out-Null  # DWMWA_EXTENDED_FRAME_BOUNDS
      $pid2 = 0
      [void][WEnum]::GetWindowThreadProcessId($h, [ref]$pid2)
      $proc = (Get-Process -Id $pid2 -ErrorAction SilentlyContinue).ProcessName
      Write-Output ("hwnd={0} pid={1} proc={2} class={3} title={4}" -f $h, $pid2, $proc, $cls, $title)
      Write-Output ("  GetWindowRect=({0},{1})-({2},{3})  DWMbounds=({4},{5})-({6},{7})" -f $r.L,$r.T,$r.R,$r.B,$dwr.L,$dwr.T,$dwr.R,$dwr.B)
    }
  }
  return $true
}
[WEnum]::EnumWindows($cb, [IntPtr]::Zero) | Out-Null
