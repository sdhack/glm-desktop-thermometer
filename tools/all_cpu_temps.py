import ctypes, time
from ctypes import wintypes

k32 = ctypes.WinDLL('kernel32')
u = ctypes.WinDLL('user32')
k32.CreateFileW.restype = ctypes.c_void_p
k32.DeviceIoControl.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32,
                                ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p]

h = k32.CreateFileW('\\\\.\\WinRing0_1_2_0', 0xC0000000, 3, None, 3, 0, None)
assert h not in (0, None, 0xFFFFFFFFFFFFFFFF), '驱动未加载'

def read_msr(idx):
    out = (ctypes.c_ubyte * 8)()
    ret = ctypes.c_uint32()
    inp = (ctypes.c_uint * 1)(idx)
    ok = k32.DeviceIoControl(h, 0x9C402084, inp, 4, out, 8, ctypes.byref(ret), None)
    return int.from_bytes(bytes(out), 'little') if ok else None

tj = (read_msr(0x1A2) >> 16) & 0xFF

# 每个逻辑核: 亲和到该核读 0x19C（IA32_THERM_STATUS，逐核 DTS）
ncores = k32.GetActiveProcessorCount(0) if hasattr(k32, 'GetActiveProcessorCount') else 0
if not ncores:
    ncores = ctypes.windll.kernel32.GetActiveProcessorCount(0)
print(f'=== CPU: 13600KF (TjMax={tj}C, {ncores} 逻辑核) ===\n')

ULONG_PTR = ctypes.c_uint64
def core_temp(aff_mask):
    prev = ULONG_PTR()
    t = ctypes.c_void_p(k32.GetCurrentThread())
    k32.SetThreadAffinityMask(t, ULONG_PTR(aff_mask))
    v = read_msr(0x19C)
    k32.SetThreadAffinityMask(t, prev)
    if v is None:
        return None
    digital = (v >> 16) & 0x7F
    valid = (v >> 31) & 1
    return tj - digital if valid else None

print('--- 逐核 DTS (MSR 0x19C, 温度=TjMax-数字读数) ---')
mask = 1
i = 0
while mask <= 0xFFFFFFFF and i < ncores:
    t = core_temp(mask)
    if t is not None:
        print(f'  Core{i:2d}: {t}C')
    i += 1
    mask <<= 1

th = read_msr(0x1B1)
digital = (th >> 16) & 0x7F
valid = (th >> 31) & 1
print(f'\n--- 包温 (MSR 0x1B1 Package Thermal Status) ---')
print(f'  Package: {tj - digital}C  (有效位={valid}, 数字读数={digital})')

k32.CloseHandle(h)

print('\n--- ACPI 热区 (WMI MSAcpi_ThermalZoneTemperature, 用户态) ---')
import subprocess
out = subprocess.run(['powershell', '-NoProfile', '-Command',
    "Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature | ForEach-Object { '{0}: {1}C (可信度 {2})' -f $_.InstanceName, [math]::Round($_.CurrentTemperature/10.0-273.15,1), $_.Accuracy }"],
    capture_output=True, creationflags=0x08000000)
print(out.stdout.decode('gbk', errors='replace').strip() or '(无)')
