[CmdletBinding()]
param(
    [string]$Prefix,
    [string]$Version = 'latest',
    [ValidateSet('auto', 'none', 'codex', 'claude')]
    [string]$Client = 'auto',
    [string]$OmpBin,
    [switch]$WithLspServers,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

if ($Help) {
    @'
Usage: .\install.ps1 [options]

  -Prefix DIR        Install into DIR
  -Version TAG       Install a release tag such as v0.2.0 (default: latest)
  -Client NAME       Configure local clients: auto (default), none, codex, or claude
                     Auto detects installed Codex and Claude Code clients
  -OmpBin FILE       Use an existing Oh My Pi 19.0.0 executable
  -WithLspServers    Install optional language servers (requires Node 24+ and npm on PATH)
  -Help              Show this help
'@
    return
}

if ($env:OS -ne 'Windows_NT') {
    throw 'Use install.sh on macOS or Linux.'
}

$repo = 'y26s4824k264/omp-toolkit'
$nodeVersion = 'v24.22.3'
$releaseTagPattern = '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
if ($Version -cne 'latest' -and $Version -cnotmatch $releaseTagPattern) {
    throw 'Version must be latest or a release tag such as v0.2.0.'
}

function Save-Download([string]$Url, [string]$Path) {
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Path -TimeoutSec 300
}

function Assert-Checksum([string]$Path, [string]$Name, [string]$Manifest) {
    $pattern = '^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($Name) + '$'
    $expected = @(
        foreach ($line in Get-Content -LiteralPath $Manifest) {
            if ($line -match $pattern) { $Matches[1] }
        }
    )
    if ($expected.Count -ne 1) { throw "Missing or invalid checksum for $Name." }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    if ($actual -ine $expected[0]) { throw "Checksum mismatch for $Name." }
}

function Find-Node {
    $candidate = Get-Command -Name node.exe -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($candidate) {
        try {
            $valid = & $candidate.Path -p 'Number(process.versions.node.split(".")[0]) >= 24' 2>$null
            if ($LASTEXITCODE -eq 0 -and $valid -ceq 'true') { return $candidate.Path }
        } catch {
            # A broken tool on PATH does not prevent a portable installation.
        }
    }
    return $null
}

$node = Find-Node
if ($WithLspServers -and (-not $node -or -not (Get-Command -Name npm.cmd -CommandType Application -ErrorAction SilentlyContinue))) {
    throw 'Install Node.js 24 or later with npm to use -WithLspServers.'
}

$stage = Join-Path ([IO.Path]::GetTempPath()) ('omp-toolkit-' + [Guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($stage)
try {
    $portableNode = -not $node
    if ($portableNode) {
        $machine = $env:PROCESSOR_ARCHITEW6432
        if (-not $machine) { $machine = $env:PROCESSOR_ARCHITECTURE }
        $arch = switch ($machine) {
            'AMD64' { 'x64' }
            'ARM64' { 'arm64' }
            default { throw 'Supported Windows architectures are x64 and arm64.' }
        }
        Write-Host "Downloading Node.js $nodeVersion for Windows/$arch"
        $nodeDist = "https://nodejs.org/dist/$nodeVersion"
        $nodeAsset = "win-$arch/node.exe"
        $manifest = Join-Path $stage 'node-checksums.txt'
        $node = Join-Path $stage 'node.exe'
        Save-Download "$nodeDist/SHASUMS256.txt" $manifest
        Save-Download "$nodeDist/$nodeAsset" $node
        Assert-Checksum $node $nodeAsset $manifest
    }

    if ($Version -ceq 'latest') {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -TimeoutSec 60
        $Version = $release.tag_name
        if ($Version -cnotmatch $releaseTagPattern) { throw 'GitHub returned an invalid release tag.' }
    }

    $releaseUrl = "https://github.com/$repo/releases/download/$Version"
    $releaseManifest = Join-Path $stage 'SHA256SUMS'
    $installer = Join-Path $stage 'install-release.mjs'
    Save-Download "$releaseUrl/SHA256SUMS" $releaseManifest
    Save-Download "$releaseUrl/install-release.mjs" $installer
    Assert-Checksum $installer 'install-release.mjs' $releaseManifest

    $installArgs = @($installer, '--version', $Version, '--client', $Client.ToLowerInvariant())
    if ($Prefix) { $installArgs += @('--prefix', $Prefix) }
    if ($OmpBin) { $installArgs += @('--omp-bin', $OmpBin) }
    if ($WithLspServers) { $installArgs += '--with-lsp-servers' }
    if ($portableNode) { $installArgs += @('--portable-node', $node) }
    & $node @installArgs
    if ($LASTEXITCODE -ne 0) { throw "Toolkit installer exited with code $LASTEXITCODE." }
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
