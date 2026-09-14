using LibreHardwareMonitor.Hardware;
using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;

// 无窗后台桥：枚举 CPU 温度与全部风扇转速，每 1 秒向标准输出打印一行协议文本。
// 行格式：CPU_TEMP <摄氏度> | FAN <名称> <rpm>（多条）| END
// 由 tempmon 采集子进程拉起并解析；本进程崩溃会被自动重启。

var computer = new Computer
{
    IsCpuEnabled = true,
    IsGpuEnabled = true,      // 顺带抓显卡风扇
    IsMemoryEnabled = false,
    IsMotherboardEnabled = true, // SuperIO 风扇
    IsStorageEnabled = false,
    IsControllerEnabled = false,
    IsPsuEnabled = false,
};
computer.Open();

// 周期性工作集修剪：后台无窗采集器没有前台体验负担，
// 让 OS 把冷页换出，任务管理器里的占用常年保持低位
Native.TrimWorkingSet();

var stdout = Console.Out;
var sinceTrim = 0;
while (true)
{
    sinceTrim++;
    try
    {
        foreach (IHardware hw in computer.Hardware)
        {
            hw.Update();
            foreach (IHardware sub in hw.SubHardware)
                sub.Update();
        }

        float? cpuTemp = null;
        var fans = new List<(string name, float rpm)>();

        foreach (IHardware hw in computer.Hardware)
        {
            if (hw.HardwareType == HardwareType.Cpu)
            {
                foreach (ISensor s in hw.Sensors)
                {
                    // 排除 Distance to TjMax（是余量不是温度），优先 Package
                    if (s.SensorType != SensorType.Temperature || !s.Value.HasValue)
                        continue;
                    if (s.Name.IndexOf("TjMax", StringComparison.OrdinalIgnoreCase) >= 0)
                        continue;
                    if (s.Name.IndexOf("Package", StringComparison.OrdinalIgnoreCase) >= 0)
                    {
                        cpuTemp = s.Value.Value;
                        break;
                    }
                    if (!cpuTemp.HasValue || s.Value.Value > cpuTemp.Value)
                        cpuTemp = s.Value.Value;
                }
            }
            CollectFans(hw, fans);
            foreach (IHardware sub in hw.SubHardware)
                CollectFans(sub, fans);
        }

        if (cpuTemp.HasValue)
            stdout.WriteLine($"CPU_TEMP {cpuTemp.Value:0.#}");
        foreach (var (name, rpm) in fans.Take(3))
            stdout.WriteLine($"FAN {name.Replace(' ', '_')} {rpm:0}");
        stdout.WriteLine("END");
        stdout.Flush();
    }
    catch
    {
        stdout.WriteLine("END"); // 出错也输出 END，避免读取端阻塞
    }
    if (sinceTrim % 600 == 0)
    {
        GC.Collect(2, GCCollectionMode.Aggressive, blocking: true, compacting: true);
        GC.WaitForPendingFinalizers();
        Native.TrimWorkingSet();
    }
    Thread.Sleep(1000);
}

static void CollectFans(IHardware hw, List<(string, float)> fans)
{
    foreach (ISensor s in hw.Sensors)
    {
        if (s.SensorType == SensorType.Fan && s.Value.HasValue && s.Value.Value > 0)
            fans.Add((hw.Name + "/" + s.Name, s.Value.Value));
    }
}

static class Native
{
    [DllImport("kernel32.dll")]
    internal static extern bool SetProcessWorkingSetSize(IntPtr hProc, IntPtr min, IntPtr max);
    [DllImport("kernel32.dll")]
    internal static extern IntPtr GetCurrentProcess();
    internal static void TrimWorkingSet() => SetProcessWorkingSetSize(GetCurrentProcess(), (IntPtr)(-1), (IntPtr)(-1));
}
