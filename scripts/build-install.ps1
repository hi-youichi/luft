<##
.SYNOPSIS
    Build Luft in release mode and install the binary for the current user.

.DESCRIPTION
    Runs `cargo build --release`, copies the resulting binary to
    %USERPROFILE%\.luft\bin, then runs `luft install` using that exact binary.

.EXAMPLE
    .\scripts\build-install.ps1
##>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\")).Path
$releaseBinary = Join-Path $repoRoot "target\release\luft.exe"
$installDir = Join-Path $env:USERPROFILE ".luft\bin"
$installedBinary = Join-Path $installDir "luft.exe"

Push-Location $repoRoot
try {
    Write-Host "==> Building Luft (release)"
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE"
    }

    if (-not (Test-Path -LiteralPath $releaseBinary -PathType Leaf)) {
        throw "Release binary not found: $releaseBinary"
    }

    Write-Host "==> Installing binary to $installDir"
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    Copy-Item -LiteralPath $releaseBinary -Destination $installedBinary -Force

    # Make the current PowerShell process resolve `luft` from the freshly
    # installed location, while also invoking the exact copied binary below.
    if (($env:PATH -split ';') -notcontains $installDir) {
        $env:PATH = "$installDir;$env:PATH"
    }

    Write-Host "==> Verifying installed binary"
    & $installedBinary --version
    if ($LASTEXITCODE -ne 0) {
        throw "Installed Luft binary failed version check"
    }

    Write-Host "==> Running luft install"
    & $installedBinary install
    if ($LASTEXITCODE -ne 0) {
        throw "luft install failed with exit code $LASTEXITCODE"
    }

    Write-Host ""
    Write-Host "==> Luft installed successfully: $installedBinary" -ForegroundColor Green
}
finally {
    Pop-Location
}
