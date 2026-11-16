# Builds the Toolbox in release mode and installs it into dist\ – that folder is the installed program:
# dist\Toolbox.exe, dist\program.json, and dist\data\ with your settings (never touched by a build).
# Works while the Toolbox is running: the running exe is put aside (see below) and the new one starts next time.
#
# Requirement: Rust (https://rustup.rs). Nothing pops up; the first build takes a few minutes.
$ErrorActionPreference = 'Stop'
# Run from the script's folder, wherever it was started from. $PSScriptRoot is empty when the lines are pasted
# instead of run as a file – then the current folder is used.
if ($PSScriptRoot) { Set-Location $PSScriptRoot }

cargo build --release -p toolbox
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE } # stop here, so that no old exe is installed after a failed build

# Where cargo builds: target\ in this repo, or the shared build folder of a .cargo\config.toml further up.
$target = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).target_directory
$release = Join-Path $target 'release'

New-Item -ItemType Directory -Force dist | Out-Null # only ever created, never deleted: dist\data\ is yours

# Windows locks a running exe: it cannot be overwritten or deleted, but it can be renamed. So the old one becomes
# "Toolbox.<time>.old.exe" – it keeps running – and the new one takes its place. Old .old.exe files are removed as
# soon as they no longer run (those still running stay).
Get-ChildItem dist\*.old.exe -ErrorAction SilentlyContinue | ForEach-Object {
    Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue
}
function Install-Exe($from, $to) {
    $installed = Join-Path dist $to
    if (Test-Path $installed) {
        try {
            Copy-Item $from $installed -Force # the usual case: the program is not running
            return
        } catch {
            $old = [IO.Path]::GetFileNameWithoutExtension($to) + '.' + (Get-Date -Format 'HHmmss') + '.old.exe'
            Rename-Item $installed $old # it is running: put it aside
            Write-Host "$to is running - the running version is now called $old, the new one starts next time."
        }
    }
    Copy-Item $from $installed -Force
}
Install-Exe (Join-Path $release 'toolbox.exe') 'Toolbox.exe'
# What the Toolbox says about itself; written by its build script from toolbox\Cargo.toml.
Copy-Item (Join-Path $release 'toolbox.program.json') 'dist\program.json' -Force
Write-Host 'Done: dist\Toolbox.exe'
