using LibreHardwareMonitor.Hardware;

var computer = new Computer { IsCpuEnabled = true, IsMotherboardEnabled = true, IsGpuEnabled = false, IsMemoryEnabled = false, IsStorageEnabled = false, IsNetworkEnabled = false, IsControllerEnabled = false, IsPsuEnabled = false };
computer.Open();
foreach (var hw in computer.Hardware)
{
    hw.Update();
    foreach (var sub in hw.SubHardware) sub.Update();
    DumpHw(hw);
    foreach (var sub in hw.SubHardware) DumpHw(sub);
}

// WMI ACPI 热区
try
{
    var searcher = new System.Management.ManagementObjectSearcher("root/wmi", "SELECT * FROM MSAcpi_ThermalZoneTemperature");
    int i = 0;
    foreach (System.Management.ManagementObject o in searcher.Get())
    {
        if (o["CurrentTemperature"] != null)
            Console.WriteLine($"[WMI-ACPI] ThermalZone{i++}: {double.Parse(o["CurrentTemperature"].ToString()) / 10.0 - 273.15:F1} C");
    }
}
catch (Exception e) { Console.WriteLine($"[WMI-ACPI] unavailable: {e.Message.Split('\n')[0]}"); }

static void DumpHw(IHardware hw)
{
    foreach (var s in hw.Sensors)
        if (s.SensorType == SensorType.Temperature && s.Value.HasValue)
            Console.WriteLine($"[{hw.HardwareType}] {hw.Name} | {s.Name} = {s.Value:F1} C  (id={s.Index})");
}
