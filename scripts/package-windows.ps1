# Build terminal-rs for Windows and package it into dist\:
#   terminal-rs-<version>-windows-x64.zip   portable: unzip and run terminal-rs.exe
#   terminal-rs-<version>-setup-x64.exe     installer (Start menu shortcut, uninstaller),
#                                           when Inno Setup 6 (iscc.exe) is installed
# Usage (from any folder):  powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1 [-NoBuild]
param([switch]$NoBuild)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $NoBuild) {
    cargo build --release -p terminal-rs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
$exe = "target\release\terminal-rs.exe"
if (-not (Test-Path $exe)) { throw "$exe not found (build first or drop -NoBuild)" }

$dist = Join-Path $root "dist"
$stage = Join-Path $dist "terminal-rs"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe $stage
Copy-Item README.md, NOTICE.md, docs\DATA_SOURCES.md $stage

$zip = Join-Path $dist "terminal-rs-$version-windows-x64.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Write-Host "Portable zip: $zip"

$iscc = Get-Command iscc.exe -ErrorAction SilentlyContinue
if (-not $iscc) {
    $default = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
    if (Test-Path $default) { $iscc = Get-Item $default }
}
if ($iscc) {
    & $iscc.Source "/DAppVersion=$version" "/DSourceDir=$stage" "/DOutputDir=$dist" scripts\windows\terminal-rs.iss
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
    Write-Host "Installer: $dist\terminal-rs-$version-setup-x64.exe"
} else {
    Write-Host "Inno Setup 6 not found; skipped the installer (https://jrsoftware.org/isinfo.php)."
}
