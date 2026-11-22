# Builds the program in release mode and installs it into dist\ – that folder is the installed program:
# dist\Example.exe, dist\program.json, and dist\data\ with your settings (never touched by a build).
# Works while the program is running: the running exe is put aside (see below) and the new one starts next time.
#
# Requirement: Rust (https://rustup.rs). Nothing pops up; the first build takes a few minutes.
$ErrorActionPreference = 'Stop'
if ($PSScriptRoot) { Set-Location $PSScriptRoot }

# The two names that differ from program to program: the package in Cargo.toml and the exe in dist\.
$package = 'example'
$exe = 'Example.exe'

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE } # stop here, so that no old exe is installed after a failed build

# Where cargo builds: target\ in this repo, or the shared build folder of a .cargo\config.toml further up.
$target = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).target_directory
$release = Join-Path $target 'release'

New-Item -ItemType Directory -Force dist | Out-Null # only ever created, never deleted: dist\data\ is yours

# Windows locks a running exe: it cannot be overwritten or deleted, but it can be renamed. So the old one becomes
# "Example.<time>.old.exe" – it keeps running – and the new one takes its place. The program removes such
# files itself at its next start.
$installed = Join-Path dist $exe
if (Test-Path $installed) {
    try {
        Copy-Item (Join-Path $release "$package.exe") $installed -Force # the usual case: the program is not running
    } catch {
        $old = [IO.Path]::GetFileNameWithoutExtension($exe) + '.' + (Get-Date -Format 'HHmmss') + '.old.exe'
        Rename-Item $installed $old # it is running: put it aside
        Write-Host "$exe is running - the running version is now called $old, the new one starts next time."
        Copy-Item (Join-Path $release "$package.exe") $installed -Force
    }
} else {
    Copy-Item (Join-Path $release "$package.exe") $installed -Force
}
# What the Toolbox reads; written by the build script from Cargo.toml.
Copy-Item (Join-Path $release "$package.program.json") 'dist\program.json' -Force
Copy-Item 'README.md' 'dist\README.md' -Force
Write-Host "Done: dist\$exe"
