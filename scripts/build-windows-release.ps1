#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Build + package a distributable Windows release of S21 HiJack.

.DESCRIPTION
    Produces, under dist\:
      s21_hijack-v<version>-windows-x64.zip             portable: exe + locales + docs
      s21_hijack-v<version>-windows-x64.zip.sha256      checksum
      s21_hijack-v<version>-windows-x64-setup.exe       Inno Setup installer (if ISCC found)
      s21_hijack-v<version>-windows-x64-setup.exe.sha256

    The zip bundles the release exe, the runtime locales\ tree (required for the
    help-bubble translations — the app scans for locales\ beside its executable),
    the README, and both license files: everything to run the app with no Rust
    toolchain. The installer (built from packaging\windows\s21_hijack.iss) adds
    Start-menu / optional desktop shortcuts and the .s21show file association.

    The version comes from Cargo.toml, so the artifact names match the tag_name
    the in-app update check compares against on GitHub (src/version.rs).

.PARAMETER SkipBuild
    Reuse an existing target\release\s21_hijack.exe instead of rebuilding.

.PARAMETER SkipInstaller
    Produce only the portable zip; skip the Inno Setup installer even if ISCC
    is available.

.EXAMPLE
    scripts\build-windows-release.ps1
.EXAMPLE
    scripts\build-windows-release.ps1 -SkipBuild
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipInstaller
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Write a sha256sum-compatible "<hash>  <filename>" sidecar next to $Path.
function Write-Sha256 {
    param([Parameter(Mandatory)][string]$Path)
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLower()
    $name = Split-Path -Leaf $Path
    "$hash  $name" | Set-Content -LiteralPath "$Path.sha256" -Encoding ascii
}

$RepoDir = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $RepoDir

# ── Resolve version from Cargo.toml ───────────────────────────────────────
$cargoToml = Get-Content -Raw (Join-Path $RepoDir 'Cargo.toml')
$verMatch = [regex]::Match($cargoToml, '(?m)^\s*version\s*=\s*"([^"]+)"')
if (-not $verMatch.Success) {
    throw 'could not read version from Cargo.toml'
}
$Version = $verMatch.Groups[1].Value
$Pkg = "s21_hijack-v$Version-windows-x64"
$Bin = Join-Path $RepoDir 'target\release\s21_hijack.exe'

Write-Host "==> Building S21 HiJack v$Version for windows/x64"

# ── Build (unless reusing an existing binary) ─────────────────────────────
if (-not $SkipBuild) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'cargo not found — install the Rust toolchain (https://rustup.rs)'
    }
    # --bin s21_hijack so the dev-only mock_console isn't built/shipped.
    & cargo build --release --bin s21_hijack
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
}

if (-not (Test-Path -LiteralPath $Bin)) {
    throw "release binary not found at $Bin`n       run without -SkipBuild, or 'cargo build --release --bin s21_hijack' first."
}

# ── Stage the package tree ────────────────────────────────────────────────
$Dist = Join-Path $RepoDir 'dist'
$Stage = Join-Path $Dist $Pkg
Write-Host "==> Staging $Stage"
if (Test-Path -LiteralPath $Stage) { Remove-Item -Recurse -Force -LiteralPath $Stage }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

Copy-Item -LiteralPath $Bin -Destination (Join-Path $Stage 's21_hijack.exe')
# locales\ MUST ship beside the exe or every tooltip falls back to English.
Copy-Item -LiteralPath (Join-Path $RepoDir 'assets\locales') -Destination (Join-Path $Stage 'locales') -Recurse
Copy-Item -LiteralPath (Join-Path $RepoDir 'README.md') -Destination $Stage
Copy-Item -LiteralPath (Join-Path $RepoDir 'LICENSE-MIT') -Destination $Stage
Copy-Item -LiteralPath (Join-Path $RepoDir 'LICENSE-APACHE') -Destination $Stage

# ── Zip + checksum ────────────────────────────────────────────────────────
$Zip = Join-Path $Dist "$Pkg.zip"
Write-Host "==> Creating $Zip"
if (Test-Path -LiteralPath $Zip) { Remove-Item -Force -LiteralPath $Zip }
# CreateFromDirectory with includeBaseDirectory=$true puts the package folder
# at the zip root (so it extracts to s21_hijack-v<ver>-windows-x64\), matching
# the Linux tarball layout.
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $Stage, $Zip, [System.IO.Compression.CompressionLevel]::Optimal, $true)
Write-Sha256 $Zip

# ── Inno Setup installer (optional) ───────────────────────────────────────
$Setup = $null
if (-not $SkipInstaller) {
    $isccCmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    $iscc = if ($isccCmd) { $isccCmd.Source } else { $null }
    if (-not $iscc) {
        $candidate = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
        if (Test-Path -LiteralPath $candidate) { $iscc = $candidate }
    }
    if ($iscc) {
        $Iss = Join-Path $RepoDir 'packaging\windows\s21_hijack.iss'
        Write-Host "==> Building installer ($iscc)"
        & $iscc "/DMyAppVersion=$Version" $Iss
        if ($LASTEXITCODE -ne 0) { throw "ISCC failed (exit $LASTEXITCODE)" }
        $Setup = Join-Path $Dist "$Pkg-setup.exe"
        if (Test-Path -LiteralPath $Setup) {
            Write-Sha256 $Setup
        } else {
            Write-Warning "ISCC reported success but $Setup was not found."
            $Setup = $null
        }
    } else {
        Write-Warning "Inno Setup (ISCC.exe) not found — skipping installer. Install from https://jrsoftware.org/isdl.php, or pass -SkipInstaller to silence this."
    }
}

# ── Summary ───────────────────────────────────────────────────────────────
function Format-Size { param([string]$Path) '{0:N1} MB' -f ((Get-Item -LiteralPath $Path).Length / 1MB) }

Write-Host ''
Write-Host 'Done:'
Write-Host ("  {0}  ({1})" -f $Zip, (Format-Size $Zip))
Write-Host ("  {0}" -f "$Zip.sha256")
if ($Setup) {
    Write-Host ("  {0}  ({1})" -f $Setup, (Format-Size $Setup))
    Write-Host ("  {0}" -f "$Setup.sha256")
}
