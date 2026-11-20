# Build + test + package a release.   Usage:  powershell -ExecutionPolicy Bypass -File build.ps1 [-SkipE2E]
param([switch]$SkipE2E)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$py = ".\.venv\Scripts\python.exe"
$ver = & $py -c "import core; print(core.__version__)"
Write-Host "== claude-usage v$ver" -ForegroundColor Cyan

Write-Host "== stop running copies" -ForegroundColor Cyan
Get-CimInstance Win32_Process -Filter "Name='claude-usage.exe'" | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Start-Sleep 2

Write-Host "== unit tests" -ForegroundColor Cyan
& $py tests\test_unit.py; if ($LASTEXITCODE -ne 0) { throw "unit tests failed" }

Write-Host "== assets" -ForegroundColor Cyan
& $py tools\make_assets.py

Write-Host "== pyinstaller" -ForegroundColor Cyan
Remove-Item dist, build, *.spec -Recurse -Force -ErrorAction SilentlyContinue
$ErrorActionPreference = "Continue"      # PyInstaller logs to stderr; judge by exit code instead
$log = & $py -m PyInstaller --noconfirm --onefile --noconsole --name claude-usage `
    --icon assets\icon.ico --version-file assets\version_info.txt `
    --add-data "ui.html;." --add-data "assets\icon.png;assets" --collect-all webview `
    --hidden-import promptguard --hidden-import dashboard --hidden-import widget --hidden-import hookinstall main.py 2>&1
$code = $LASTEXITCODE
$ErrorActionPreference = "Stop"
$log | Select-Object -Last 2
if ($code -ne 0 -or -not (Test-Path dist\claude-usage.exe)) { throw "build failed (exit $code)" }

Write-Host "== package" -ForegroundColor Cyan
$name = "claude-usage-v$ver-win-x64"
$out = "release\$name"
Remove-Item release -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item dist\claude-usage.exe, README.md, CHANGELOG.md $out
Compress-Archive -Path "$out\*" -DestinationPath "release\$name.zip" -Force
function Sha256($p) { $s = [System.Security.Cryptography.SHA256]::Create(); $f = [IO.File]::OpenRead((Resolve-Path $p)); try { -join ($s.ComputeHash($f) | ForEach-Object { $_.ToString("x2") }) } finally { $f.Dispose() } }   # no dependency on the Utility module
$hash = Sha256 "release\$name.zip"
"$hash  $name.zip" | Set-Content "release\$name.zip.sha256" -Encoding ascii
$exeHash = Sha256 "dist\claude-usage.exe"
Get-ChildItem release -File | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,1)}} | Format-Table -AutoSize
Write-Host "zip sha256: $hash"
Write-Host "exe sha256: $exeHash"
