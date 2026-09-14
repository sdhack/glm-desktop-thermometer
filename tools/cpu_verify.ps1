$ErrorActionPreference = "SilentlyContinue"

function AcpiTemp {
    $t = (Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature |
        Select-Object -First 1).CurrentTemperature
    if ($t) { [math]::Round($t / 10.0 - 273.15, 1) } else { $null }
}

function ReadLhm {
    $lhm = $null
    $p = Start-Process -FilePath ".\lhm-bridge.exe" -WorkingDirectory "D:\GLM桌面温度计260912\dist\GLM桌面温度计" -RedirectStandardOutput "$env:TEMP\lhm_out.txt" -NoNewWindow -PassThru
    Start-Sleep -Milliseconds 2200
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    Get-Content "$env:TEMP\lhm_out.txt" | ForEach-Object {
        if ($_ -match "CPU_TEMP (.+)") { $lhm = [double]$Matches[1] }
    }
    return $lhm
}

Write-Output "=== IDLE ==="
1..3 | ForEach-Object {
    Write-Output ("LHM=" + (ReadLhm) + "  ACPI=" + (AcpiTemp))
    Start-Sleep -Seconds 1
}

Write-Output "=== LOAD 40s ==="
$jobs = 1..4 | ForEach-Object {
    Start-Job -ScriptBlock {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalSeconds -lt 42) { $x = 1.1 * 1.1 }
    }
}
1..4 | ForEach-Object {
    Start-Sleep -Seconds 10
    Write-Output ("t+" + ($_ * 10) + "s  LHM=" + (ReadLhm) + "  ACPI=" + (AcpiTemp))
}
$jobs | Stop-Job -ErrorAction SilentlyContinue
$jobs | Remove-Job -Force -ErrorAction SilentlyContinue

Write-Output "=== COOLDOWN 20s ==="
Start-Sleep -Seconds 20
Write-Output ("LHM=" + (ReadLhm) + "  ACPI=" + (AcpiTemp))
