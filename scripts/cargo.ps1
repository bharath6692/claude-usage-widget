# Wrapper around cargo that puts the GNU toolchain's prerequisites on PATH.
#
# Two things are needed beyond rustup itself on the x86_64-pc-windows-gnu target:
#   1. dlltool.exe, from rustup's bundled "self-contained" mingw
#   2. as.exe, which that bundle does NOT ship - dlltool spawns it to build the
#      import libraries the raw-dylib crates (windows-sys, windows-link) need
# A full mingw-w64 supplies both. Installed user-scope, no admin:
#   winget install BrechtSanders.WinLibs.POSIX.MSVCRT --scope user
#
# Usage: .\scripts\cargo.ps1 test
#        .\scripts\cargo.ps1 build --release

$ErrorActionPreference = 'Stop'

$cargoBin = "$env:USERPROFILE\.cargo\bin"
$selfContained = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained"

$mingw = $null
$candidates = @(
    "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.MSVCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\mingw64\bin",
    "C:\mingw64\bin",
    "C:\msys64\mingw64\bin"
)
foreach ($c in $candidates) {
    if (Test-Path (Join-Path $c 'as.exe')) { $mingw = $c; break }
}
if (-not $mingw) {
    Write-Error "No mingw-w64 assembler (as.exe) found. Install it with: winget install BrechtSanders.WinLibs.POSIX.MSVCRT --scope user"
}

$env:Path = "$cargoBin;$selfContained;$mingw;$env:Path"

& "$cargoBin\cargo.exe" @args
exit $LASTEXITCODE
