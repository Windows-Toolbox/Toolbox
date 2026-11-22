# Makes a release: builds, packs dist\ (without data\) into a zip, signs it, and publishes zip and signature as
# GitHub release v<version> – the version from Cargo.toml. Programs that are installed find it with "Update".
#
# Requirements: the GitHub CLI (gh), signed in; the Toolbox repo next to this one (its `sign` tool holds the
# release key, see Toolbox\DESIGN.md 15.6). Refuses if that version was already released or something is uncommitted.
$ErrorActionPreference = 'Stop'
if ($PSScriptRoot) { Set-Location $PSScriptRoot }

if (git status --porcelain) { throw 'There are uncommitted changes - commit first, so the release is what the repo holds.' }

& .\build.ps1
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$manifest = Get-Content 'dist\program.json' -Raw | ConvertFrom-Json
$name = "$($manifest.id)-$($manifest.version)"
$tag = "v$($manifest.version)"
# "release not found" comes on stderr - with 'Stop', Windows PowerShell would end the script right there.
$ErrorActionPreference = 'Continue'
gh release view $tag *> $null
$released = $LASTEXITCODE -eq 0
$ErrorActionPreference = 'Stop'
if ($released) { throw "$tag was already released - raise the version in Cargo.toml." }

# The sign tool is part of the Toolbox repo; it packs and signs in one step.
$toolbox = Join-Path (Split-Path $PSScriptRoot -Parent) 'Toolbox'
cargo run --release --quiet --manifest-path (Join-Path $toolbox 'Cargo.toml') -p sign -- pack dist "$name.zip"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

gh release create $tag "$name.zip" "$name.zip.sig" --title "$($manifest.name) $($manifest.version)" --notes-file 'RELEASE.md'
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Remove-Item "$name.zip", "$name.zip.sig"
Write-Host "Released: $tag"
