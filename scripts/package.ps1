# Build a release exe and package it as a versioned zip for distribution.
#
# Usage:
#   .\scripts\package.ps1                # build + zip
#   .\scripts\package.ps1 -SkipTests     # skip `cargo test` (faster iteration)
#   .\scripts\package.ps1 -Notes "Adds the docked taskbar overlay"
#
# Output (both under dist/, gitignored):
#   dist/claude-usage-widget-<version>.zip   the exe, ready to hand to a teammate
#   dist/version.json                        manifest for "Check for updates"
#
# version.json is NOT auto-uploaded anywhere — after packaging, copy dist/ to
# wherever it's hosted (OneDrive/Teams link, internal file share, ...) and
# point teammates' settings.json `update_manifest_url` at the hosted
# version.json's URL.

param(
    [switch]$SkipTests,
    [string]$Notes = ""
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Get-PackageVersion {
    $tomlLine = Select-String -Path "$root\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $tomlLine) {
        Write-Error "Could not find version in Cargo.toml"
    }
    return $tomlLine.Matches[0].Groups[1].Value
}

$version = Get-PackageVersion
Write-Output "Packaging claude-usage-widget v$version"

if (-not $SkipTests) {
    Write-Output "`n=== Running tests ==="
    & "$root\scripts\cargo.ps1" test
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Tests failed - fix them before packaging a release."
    }
}

Write-Output "`n=== Building release ==="
& "$root\scripts\cargo.ps1" build --release
if ($LASTEXITCODE -ne 0) {
    Write-Error "Release build failed."
}

$exePath = "$root\target\release\claude-usage-widget.exe"
if (-not (Test-Path $exePath)) {
    Write-Error "Expected exe not found at $exePath"
}

$distDir = "$root\dist"
New-Item -ItemType Directory -Force -Path $distDir | Out-Null

$zipName = "claude-usage-widget-$version.zip"
$zipPath = "$distDir\$zipName"
if (Test-Path $zipPath) {
    Remove-Item $zipPath -Force
}

Write-Output "`n=== Zipping ==="
Compress-Archive -Path $exePath -DestinationPath $zipPath
$zipSizeKb = [math]::Round((Get-Item $zipPath).Length / 1KB, 1)
Write-Output "Wrote $zipPath ($zipSizeKb KB)"

Write-Output "`n=== Writing version.json ==="
$manifest = [ordered]@{
    version      = $version
    notes        = $Notes
    download_url = $null  # fill in once the zip is uploaded somewhere; teammates' settings.json points here
}
$manifestPath = "$distDir\version.json"
$manifest | ConvertTo-Json | Set-Content -Path $manifestPath -Encoding utf8
Write-Output "Wrote $manifestPath"

Write-Output "`nDone. Next: upload dist/ (or at least the zip) somewhere teammates can reach, fill in"
Write-Output "download_url in version.json, host it too, and point settings.json's update_manifest_url"
Write-Output "at the hosted version.json's URL."
