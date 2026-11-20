# Phase 0 baseline: time from starting "<exe> dashboard" until its window is visible. Sandboxed LOCALAPPDATA.
# Usage: powershell -ExecutionPolicy Bypass -File bench\bench_dashboard.ps1 [-Exe dist\claude-usage.exe] [-Runs 3]
param([string]$Exe = "dist\claude-usage.exe", [int]$Runs = 3, [string[]]$Args0 = @("dashboard"))
Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class W {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  public static bool Visible(int[] pids, string title) {
    bool found = false;
    EnumWindows((h, l) => { int p; GetWindowThreadProcessId(h, out p);
      if (Array.IndexOf(pids, p) >= 0 && IsWindowVisible(h)) { var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
        if (sb.ToString() == title) { found = true; return false; } } return true; }, IntPtr.Zero);
    return found;
  }
}
"@
$sandbox = Join-Path $env:TEMP ("cu-bench-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $sandbox | Out-Null
$env:LOCALAPPDATA = $sandbox
$res = @()
for ($i = 0; $i -lt $Runs; $i++) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $Exe -ArgumentList $Args0 -PassThru
  $ms = -1
  while ($sw.ElapsedMilliseconds -lt 20000) {
    $kids = @($p.Id) + @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)" | ForEach-Object { [int]$_.ProcessId })
    if ([W]::Visible([int[]]$kids, "claude-usage")) { $ms = $sw.ElapsedMilliseconds; break }
    Start-Sleep -Milliseconds 20
  }
  & taskkill /PID $p.Id /T /F 2>&1 | Out-Null
  Start-Sleep -Milliseconds 800
  $res += $ms
  "run $i : $ms ms"
}
Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
