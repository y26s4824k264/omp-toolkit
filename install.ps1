<#
.SYNOPSIS
    Build, install, and configure OMP Toolkit for supported AI coding clients on Windows
    (antigravity, claude-code, codex, cursor, vscode, claude-desktop, windsurf, zed,
     workbuddy, codebuddy, trae, qoder, lingma, kiro, cline, roo-code).
.EXAMPLE
    .\install.ps1 -Client antigravity
.EXAMPLE
    .\install.ps1 -Client workbuddy,trae,qoder
.EXAMPLE
    .\install.ps1 -Client auto -DryRun
#>
[CmdletBinding()]
param(
    [string]$BinDir = (Join-Path $env:LOCALAPPDATA "OMP Toolkit\bin"),
    [string[]]$Client = @("none"),
    [switch]$DryRun,
    [switch]$Force,
    [switch]$Remove,
    [switch]$SkipBuild,
    [switch]$NoPathEnv,
    [switch]$WithoutLsp
)

$ErrorActionPreference = "Stop"
$RepoDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ReleaseBin = Join-Path $RepoDir "target\release\omp-toolkit.exe"

if (-not $SkipBuild) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo is required to build from source."
    }
    Push-Location $RepoDir
    try {
        & cargo build --release --bin omp-toolkit
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $ReleaseBin)) {
    throw "Binary not found at $ReleaseBin (omit -SkipBuild to compile)."
}

$InstalledBin = Join-Path $BinDir "omp-toolkit.exe"
if (-not $Remove) {
    if ($DryRun) {
        Write-Host "[dry-run] Would install $ReleaseBin -> $InstalledBin"
    } else {
        New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
        Copy-Item -Force $ReleaseBin $InstalledBin
        Write-Host "Installed binary: $InstalledBin"
    }

    if (-not $WithoutLsp) {
        $LspRunner = if (Test-Path $InstalledBin) { $InstalledBin } else { $ReleaseBin }
        $LspArgs = @("lsp-install")
        if ($DryRun) { $LspArgs += "--dry-run" }
        Write-Host ""
        Write-Host "Provisioning managed language servers (pass -WithoutLsp to skip)..."
        & $LspRunner @LspArgs
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "Some language servers could not be installed; re-run `"$InstalledBin`" lsp-install later."
        }
    }
}

if ($Client.Count -eq 1 -and $Client[0] -eq "none") {
    Write-Host ""
    Write-Host "Binary ready. Run `"$InstalledBin`" setup --client list to inspect supported AI clients."
    exit 0
}

$SetupArgs = @("setup")
foreach ($c in $Client) {
    $SetupArgs += @("--client", $c)
}
if ($DryRun) { $SetupArgs += "--dry-run" }
if ($Force) { $SetupArgs += "--force" }
if ($Remove) { $SetupArgs += "--remove" }
if ($NoPathEnv) { $SetupArgs += "--no-path-env" }

$Runner = if (Test-Path $InstalledBin) { $InstalledBin } else {
    $SetupArgs += @("--install-bin", $BinDir)
    $ReleaseBin
}

& $Runner @SetupArgs
exit $LASTEXITCODE
