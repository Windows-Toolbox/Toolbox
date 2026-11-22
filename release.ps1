# Makes a release of the Toolbox: builds, packs dist\ (without data\) into a zip, signs it, and publishes zip and
# signature as GitHub release v<version> – the version from toolbox\Cargo.toml. An installed Toolbox finds it with
# "Update" in its settings; a new PC downloads the zip by hand once.
#
# Requirements: the GitHub CLI (gh), signed in; the release key (DESIGN.md 15.6 – the `sign` tool of this repo).
# Refuses if that version was already released or something is uncommitted.
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
if ($released) { throw "$tag was already released - raise the version in toolbox\Cargo.toml." }

# The sign tool is part of this workspace; it packs and signs in one step.
cargo run --release --quiet -p sign -- pack dist "$name.zip"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

gh release create $tag "$name.zip" "$name.zip.sig" --title "Windows Toolbox $($manifest.version)" --notes-file 'RELEASE.md'
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Remove-Item "$name.zip", "$name.zip.sig"
Write-Host "Released: $tag"
