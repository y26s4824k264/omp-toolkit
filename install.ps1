[CmdletBinding()]
param(
    [string]$Prefix,
    [string]$Version = 'latest',
    [ValidateSet('auto', 'none', 'codex', 'claude')]
    [string]$Client = 'auto',
    [switch]$WithLspServers,
    [switch]$WithoutLspServers,
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
                     Auto detects Codex and Claude Code and configures MCP and global instructions
  -WithoutLspServers Skip installing the default language servers
  -WithLspServers    Install the default language servers (already enabled)
  -Help              Show this help
'@
    return
}

if ($env:OS -ne 'Windows_NT') {
    throw 'Use install.sh on macOS or Linux.'
}
if ($WithLspServers -and $WithoutLspServers) {
    throw 'Choose either -WithLspServers or -WithoutLspServers.'
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

function Find-Node([bool]$NeedNpm) {
    $candidate = Get-Command -Name node.exe -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($candidate) {
        try {
            $valid = & $candidate.Path -p 'Number(process.versions.node.split(".")[0]) >= 24' 2>$null
            if ($LASTEXITCODE -ne 0 -or $valid -cne 'true') { return $null }
            if ($NeedNpm) {
                $npm = Get-Command -Name npm.cmd -CommandType Application -ErrorAction SilentlyContinue |
                    Select-Object -First 1
                if (-not $npm) { return $null }
                & $npm.Path --version 2>$null | Out-Null
                if ($LASTEXITCODE -ne 0) { return $null }
            }
            return $candidate.Path
        } catch {
            # A broken tool on PATH does not prevent a portable installation.
        }
    }
    return $null
}

$node = Find-Node (-not $WithoutLspServers)

$stage = Join-Path ([IO.Path]::GetTempPath()) ('omp-toolkit-' + [Guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($stage)
try {
    $npmCli = $null
    if (-not $node) {
        $machine = $env:PROCESSOR_ARCHITEW6432
        if (-not $machine) { $machine = $env:PROCESSOR_ARCHITECTURE }
        $arch = switch ($machine) {
            'AMD64' { 'x64' }
            'ARM64' { 'arm64' }
            default { throw 'Supported Windows architectures are x64 and arm64.' }
        }
        Write-Host "Downloading Node.js $nodeVersion with npm for Windows/$arch"
        $nodeDist = "https://nodejs.org/dist/$nodeVersion"
        $nodeName = "node-$nodeVersion-win-$arch"
        $nodeAsset = "$nodeName.zip"
        $manifest = Join-Path $stage 'node-checksums.txt'
        $archive = Join-Path $stage $nodeAsset
        Save-Download "$nodeDist/SHASUMS256.txt" $manifest
        Save-Download "$nodeDist/$nodeAsset" $archive
        Assert-Checksum $archive $nodeAsset $manifest
        Expand-Archive -LiteralPath $archive -DestinationPath $stage
        $nodeHome = Join-Path $stage $nodeName
        $node = Join-Path $nodeHome 'node.exe'
        $npmCli = Join-Path $nodeHome 'node_modules/npm/bin/npm-cli.js'
        if (-not (Test-Path -LiteralPath $node -PathType Leaf) -or -not (Test-Path -LiteralPath $npmCli -PathType Leaf)) {
            throw 'Node.js archive did not contain Node and npm.'
        }
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
    if ($WithoutLspServers) { $installArgs += '--without-lsp-servers' }
    if ($npmCli) { $installArgs += @('--portable-node', $node, '--npm-cli', $npmCli) }
    & $node @installArgs
    if ($LASTEXITCODE -ne 0) { throw "Toolkit installer exited with code $LASTEXITCODE." }
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
