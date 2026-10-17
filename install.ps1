# Native Windows installer. No administrator rights or Rust toolchain required.
[CmdletBinding()]
param(
    [string]$Version = $env:CORRAL_VERSION,
    [string]$InstallDir = $env:CORRAL_INSTALL_DIR,
    [string]$Repo = $env:CORRAL_REPO,
    [string]$BaseUrl = $env:CORRAL_BASE_URL,
    [switch]$DryRun
)

function Install-Corral {
    $ErrorActionPreference = 'Stop'
    if ($env:OS -ne 'Windows_NT') {
        throw 'corral: install.ps1 supports native Windows only; use install.sh on Linux/macOS.'
    }
    $architecture = $env:PROCESSOR_ARCHITEW6432
    if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
    if ($architecture -ne 'AMD64') {
        throw "corral: unsupported Windows CPU $architecture; only native x86_64 releases are available."
    }
    if (-not $Version) { $Version = 'latest' }
    if ($Version -ne 'latest' -and $Version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$') {
        throw "corral: invalid release tag '$Version' (expected latest or vX.Y.Z)."
    }
    if (-not $Repo) { $Repo = 'pawissanutt/corral' }
    if (-not $BaseUrl) { $BaseUrl = "https://github.com/$Repo/releases" }
    if (-not $InstallDir) {
        $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\corral\bin'
    }
    $InstallDir = [IO.Path]::GetFullPath($InstallDir)
    $asset = 'corral-x86_64-pc-windows-msvc.zip'
    $base = $BaseUrl.TrimEnd('/')
    $url = if ($Version -eq 'latest') { "$base/latest/download/$asset" } else { "$base/download/$Version/$asset" }
    if ($DryRun) {
        Write-Output "would download $url into $InstallDir"
        Write-Output 'requires herdr >= 0.9.3, Git for Windows (including Git Bash), and an authenticated agent CLI before corral up'
        return
    }

    # Windows PowerShell 5.1 may otherwise use obsolete TLS defaults.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ("corral-install-" + [guid]::NewGuid())
    $staged = $null
    try {
        [IO.Directory]::CreateDirectory($temporary) | Out-Null
        $archive = Join-Path $temporary $asset
        try { Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $archive -TimeoutSec 120 }
        catch { throw "corral: download failed: $url`: $($_.Exception.Message)" }
        $checksum = Join-Path $temporary "$asset.sha256"
        try { Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256" -OutFile $checksum -TimeoutSec 120 }
        catch { throw "corral: checksum download failed: $url.sha256`: $($_.Exception.Message)" }
        $manifest = [IO.File]::ReadAllText($checksum)
        if ($manifest -notmatch ('\A([0-9a-fA-F]{64}) [ *]' + [regex]::Escape($asset) + '(?:\r?\n)?\z')) {
            throw "corral: invalid checksum manifest for $asset."
        }
        $expected = $Matches[1]
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expected) {
            throw "corral: checksum mismatch for $asset; existing install unchanged."
        }
        $expanded = Join-Path $temporary 'expanded'
        Expand-Archive -LiteralPath $archive -DestinationPath $expanded
        $binary = Join-Path $expanded 'corral.exe'
        if (-not [IO.File]::Exists($binary)) { throw 'corral: archive has no corral.exe.' }
        if ((Get-Item -LiteralPath $binary).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw 'corral: archive binary must be a regular file.'
        }
        [IO.Directory]::CreateDirectory($InstallDir) | Out-Null
        $staged = Join-Path $InstallDir ('.corral-install-' + [guid]::NewGuid() + '.exe')
        [IO.File]::Copy($binary, $staged)
        $identity = & $staged --version
        if ($LASTEXITCODE -ne 0 -or $identity -isnot [string] -or $identity -notmatch '^corral \S+$') {
            throw 'corral: downloaded binary cannot run or identify itself; existing install unchanged.'
        }
        if ($Version -ne 'latest' -and $identity -ne "corral $($Version.Substring(1))") {
            throw "corral: version mismatch: requested $Version, got $identity; existing install unchanged."
        }
        $destination = Join-Path $InstallDir 'corral.exe'
        if ([IO.File]::Exists($destination)) {
            # Replace on the same filesystem; sharing violations leave the old file intact.
            [IO.File]::Replace($staged, $destination, [System.Management.Automation.Language.NullString]::Value)
        } else {
            [IO.File]::Move($staged, $destination)
        }
        $staged = $null
        Write-Output "installed $identity to $destination"
        if (($env:PATH -split ';') -notcontains $InstallDir) {
            Write-Output "Add $InstallDir to your user PATH in Settings > System > About > Advanced system settings > Environment Variables, then open a new terminal."
            Write-Output ('For this PowerShell session: $env:PATH = "' + $InstallDir + ';$env:PATH"')
        }
        Write-Output 'Before corral up: install herdr >= 0.9.3 (https://herdr.dev/docs/install/), Git for Windows with Git Bash on PATH, and authenticate an agent CLI.'
        Write-Output "Windows pane-shell setup: https://github.com/$Repo/blob/main/docs/guide/install.md#native-windows"
    } finally {
        if ($staged -and [IO.File]::Exists($staged)) { [IO.File]::Delete($staged) }
        if ([IO.Directory]::Exists($temporary)) { [IO.Directory]::Delete($temporary, $true) }
    }
}

Install-Corral
