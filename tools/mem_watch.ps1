$log = "C:\Users\Administrator\mem_watch.csv"
"time,proc,pid,commit_mb,ws_mb" | Out-File $log -Encoding utf8
$deadline = (Get-Date).AddHours(3)
while ((Get-Date) -lt $deadline) {
  $ts = Get-Date -Format "HH:mm:ss.fff"
  $ps = Get-Process | Where-Object { $_.Name -match 'tempmon|lhm' }
  if ($ps) {
    foreach ($p in $ps) {
      "$ts,$($p.Name),$($p.Id),$([int]($p.PagedMemorySize64/1MB)),$([int]($p.WorkingSet64/1MB))" | Out-File $log -Append -Encoding utf8
    }
  } else {
    "$ts,none,0,0,0" | Out-File $log -Append -Encoding utf8
  }
  Start-Sleep -Milliseconds 500
}
