# Build + test + package the RUST app.   Usage:  powershell -ExecutionPolicy Bypass -File build-rust.ps1 [-SkipTests]
# Output: release\claude-usage-rust-v<ver>-win-x64.zip (+ .sha256). The Python build stays in build.ps1.
param([switch]$SkipTests)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$rust = Join-Path $PSScriptRoot "rust"
$ver = (Select-String -Path "$rust\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
Write-Host "== claude-usage (Rust) v$ver" -ForegroundColor Cyan

# GUI tests refuse to run while a Rust tray is up. Only copies running from rust\target or release\ are stopped:
# never dist\ (the Python build) and never an installed copy elsewhere.
$mine = @((Join-Path $rust "target"), (Join-Path $PSScriptRoot "release"))
Get-CimInstance Win32_Process -Filter "Name='claude-usage.exe' OR Name='claude-usage-dashboard.exe'" |
    Where-Object { $p = $_.ExecutablePath; $p -and ($mine | Where-Object { $p.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) }) } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

Push-Location $rust
try {
    if (-not $SkipTests) {
        Write-Host "== clippy" -ForegroundColor Cyan
        cargo clippy --workspace --all-targets --release -- -D warnings; if ($LASTEXITCODE) { throw "clippy failed" }
        Write-Host "== tests (GUI tests run one at a time)" -ForegroundColor Cyan
        cargo test --workspace --release --features claude-usage-dashboard/e2e,claude-usage/e2e -- --test-threads=1
        if ($LASTEXITCODE) { throw "tests failed" }
    }
    Write-Host "== release build (WITHOUT the e2e feature)" -ForegroundColor Cyan
    # the test run above rebuilds both exes with the e2e test seams: touch them so the shipped exes are the plain ones
    (Get-Item "dashboard\src\main.rs").LastWriteTime = Get-Date
    (Get-Item "app\src\main.rs").LastWriteTime = Get-Date
    cargo build --release; if ($LASTEXITCODE) { throw "build failed" }
    # prove the shipped exe has no test seam compiled in
    $blob = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes("$rust\target\release\claude-usage.exe"))
    foreach ($s in "CU_API_BASE", "CU_TOKEN_URL", "widget_state.json", "fake_taskbar.txt", "fake_fullscreen.flag") { if ($blob.Contains($s)) { throw "test seam '$s' found in the release exe: it was built with the e2e feature" } }
    $blobD = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes("$rust\target\release\claude-usage-dashboard.exe"))
    foreach ($s in "CU_E2E_VISIBLE", "e2e_stage", "e2e_go") { if ($blobD.Contains($s)) { throw "test seam '$s' found in the release dashboard exe: it was built with the e2e feature" } }
} finally { Pop-Location }

Write-Host "== package" -ForegroundColor Cyan
$name = "claude-usage-rust-v$ver-win-x64"
$out = "release\$name"
Remove-Item $out, "release\$name.zip", "release\$name.zip.sha256" -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "$rust\target\release\claude-usage.exe", "$rust\target\release\claude-usage-dashboard.exe" $out
Copy-Item README.md, CHANGELOG.md, BUILD.md $out
function Sha256($p) { $s = [Security.Cryptography.SHA256]::Create(); $f = [IO.File]::OpenRead((Resolve-Path $p)); try { -join ($s.ComputeHash($f) | ForEach-Object { $_.ToString("x2") }) } finally { $f.Dispose() } }
$lines = Get-ChildItem $out -Filter *.exe | ForEach-Object { "$(Sha256 $_.FullName)  $($_.Name)" }
$lines | Set-Content "$out\SHA256SUMS.txt" -Encoding ascii
Compress-Archive -Path "$out\*" -DestinationPath "release\$name.zip" -Force
$hash = Sha256 "release\$name.zip"
"$hash  $name.zip" | Set-Content "release\$name.zip.sha256" -Encoding ascii
Get-ChildItem $out -File | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,2)}} | Format-Table -AutoSize
Write-Host "zip: release\$name.zip"
Write-Host "zip sha256: $hash"
