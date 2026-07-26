# Luft binary installer (Windows).
#
# Install latest:
#   irm https://raw.githubusercontent.com/hi-youichi/luft/main/install.ps1 | iex
# Install a specific version:
#   $v = 'v0.3.3'; & ([scriptblock]::Create((irm https://raw.githubusercontent.com/hi-youichi/luft/main/install.ps1))) -Version $v
# Run directly:
#   .\install.ps1 [-Version v0.3.3] [-InstallDir DIR] [-SkipVerify]

[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$InstallDir = "",
    [switch]$SkipVerify,
    [switch]$Help
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# TLS 1.2 for older PowerShell
try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 } catch {}

if ($Help) {
    Write-Host @"
Luft installer (Windows)

Usage:
  irm https://raw.githubusercontent.com/hi-youichi/luft/main/install.ps1 | iex
  .\install.ps1 [-Version v0.3.3] [-InstallDir DIR] [-SkipVerify]

Options:
  -Version <ver>      Version to install, e.g. v0.3.3 (default: latest)
  -InstallDir <dir>   Install directory (default: %USERPROFILE%\.luft\bin)
  -SkipVerify         Skip SHA256 verification (not recommended)
  -Help               Show this help

Environment:
  LUFT_VERSION        Override version
  LUFT_INSTALL_DIR    Override install directory
"@
    return
}

$Repo = "hi-youichi/luft"
if (-not $Version)    { $Version = $env:LUFT_VERSION }
if (-not $InstallDir) { $InstallDir = $env:LUFT_INSTALL_DIR }
if (-not $InstallDir) { $InstallDir = Join-Path $env:USERPROFILE ".luft\bin" }

# ---- platform check ----
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -notin @("AMD64", "x64")) {
    throw "Unsupported architecture: $arch (only x86_64 Windows builds are available)"
}

$Asset   = "luft-x86_64-windows-msvc"
$Ext     = "zip"
$Archive = "$Asset.$Ext"
$Binary  = "luft.exe"

# ---- build download url ----
if (-not $Version) {
    $DownloadUrl = "https://github.com/$Repo/releases/latest/download/$Archive"
    $VersionDisplay = "latest"
} else {
    $Version = $Version -replace '^v', ''
    $DownloadUrl = "https://github.com/$Repo/releases/download/v$Version/$Archive"
    $VersionDisplay = "v$Version"
}

Write-Host "==> Installing luft $VersionDisplay ($Archive) to $InstallDir"

# ---- temp workspace ----
$Tmp = Join-Path $env:TEMP "luft-install-$(Get-Random)"
New-Item -ItemType Directory -Path $Tmp -Force | Out-Null
try {
    Write-Host "==> Downloading $Archive"
    Invoke-WebRequest -Uri $DownloadUrl -OutFile (Join-Path $Tmp $Archive) -UseBasicParsing

    # ---- checksum ----
    if (-not $SkipVerify) {
        Write-Host "==> Downloading checksum"
        $ShaFile = Join-Path $Tmp "$Archive.sha256"
        Invoke-WebRequest -Uri "$DownloadUrl.sha256" -OutFile $ShaFile -UseBasicParsing

        Write-Host "==> Verifying SHA256"
        $expected = ((Get-Content $ShaFile -Raw) -split '\s+')[0].Trim().ToUpper()
        $actual   = (Get-FileHash (Join-Path $Tmp $Archive) -Algorithm SHA256).Hash.ToUpper()
        if ($expected -ne $actual) {
            throw "SHA256 mismatch`n  expected: $expected`n  actual:   $actual"
        }
        Write-Host "    OK"
    }

    # ---- extract ----
    Write-Host "==> Extracting"
    Expand-Archive -Path (Join-Path $Tmp $Archive) -DestinationPath $Tmp -Force
    $extracted = Join-Path $Tmp $Binary
    if (-not (Test-Path $extracted)) {
        throw "Expected binary '$Binary' not found in archive"
    }

    # ---- install ----
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Move-Item -Path $extracted -Destination (Join-Path $InstallDir $Binary) -Force

    # ---- PATH ----
    Write-Host "==> Updating PATH"
    $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    if (($userPath -split ';') -notcontains $InstallDir) {
        $newPath = if ($userPath) { "$InstallDir;$userPath" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("PATH", $newPath, "User")
        Write-Host "    Added $InstallDir to user PATH"
    } else {
        Write-Host "    Already present in user PATH"
    }
    if (($env:PATH -split ';') -notcontains $InstallDir) {
        $env:PATH = "$InstallDir;$env:PATH"
    }

    # ---- verify ----
    Write-Host "==> Verifying installation"
    & (Join-Path $InstallDir $Binary) --version

    # ---- post-install setup (best effort) ----
    Write-Host "==> Running 'luft install' (post-install setup)"
    $luftExe = Join-Path $InstallDir $Binary
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & $luftExe install
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prevEAP
    if ($null -ne $code -and $code -ne 0) {
        Write-Host "    'luft install' exited with $code (non-fatal)."
        Write-Host "    Re-run it later after installing an agent:  $luftExe install"
    } else {
        Write-Host "    OK"
    }

    Write-Host ""
    Write-Host "==> luft $VersionDisplay installed to $(Join-Path $InstallDir $Binary)"
    Write-Host ""
    Write-Host "Next steps:"
    Write-Host "  Open a new terminal (to reload PATH), then:  luft --version"
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
