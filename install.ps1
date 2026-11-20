# Native Windows installer: static web app or the classic CLI from a published legacy tag.
[CmdletBinding()]
param(
    [string]$Version = $env:CORRAL_VERSION,
    [string]$InstallDir = $env:CORRAL_INSTALL_DIR,
    [string]$Repo = $env:CORRAL_REPO,
    [string]$BaseUrl = $env:CORRAL_BASE_URL,
    [switch]$DryRun,
    [switch]$AppOnly,
    [string]$AppInstallDir = $env:CORRAL_APP_INSTALL_DIR,
    [switch]$Legacy
)

function Receive-VerifiedArchive($Url, $Temporary, $Asset) {
    $archive = Join-Path $Temporary $Asset
    try { Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $archive -TimeoutSec 120 }
    catch { throw "corral: download failed: $Url`: $($_.Exception.Message)" }
    $checksum = Join-Path $Temporary "$Asset.sha256"
    try { Invoke-WebRequest -UseBasicParsing -Uri "$Url.sha256" -OutFile $checksum -TimeoutSec 120 }
    catch { throw "corral: checksum download failed: $Url.sha256`: $($_.Exception.Message)" }
    $manifest = [IO.File]::ReadAllText($checksum)
    if ($manifest -cnotmatch ('\A([0-9a-fA-F]{64}) [ *]' + [regex]::Escape($Asset) + '(?:\r?\n)?\z')) {
        throw "corral: invalid checksum manifest for $Asset."
    }
    $expected = $Matches[1]
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expected) {
        throw "corral: checksum mismatch for $Asset; existing install unchanged."
    }
    return $archive
}

function Test-RegularFile($Path) {
    return ([IO.File]::Exists($Path) -and
        -not ((Get-Item -LiteralPath $Path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint))
}

function Assert-WebAppSite($Path, $Marker, $MarkerContents) {
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "corral: web app destination must be a directory, not a link: $Path"
    }
    $markerPath = Join-Path $Path $Marker
    $managed = ((Test-RegularFile $markerPath) -and
        ((Get-Item -LiteralPath $markerPath -Force).Length -eq $MarkerContents.Length) -and
        ([IO.File]::ReadAllText($markerPath) -ceq $MarkerContents))
    if (-not $managed -and (Get-ChildItem -LiteralPath $Path -Force -ErrorAction Stop | Select-Object -First 1)) {
        throw "corral: web app destination is nonempty and not managed by this installer; choose a dedicated directory: $Path"
    }
}

function Install-CorralWebApp($Archive, $Destination, $Tag) {
    $tar = (Get-Command tar.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
    if (-not $tar) { throw 'corral: install the Windows tar.exe utility or unpack the verified web archive on a supported host.' }
    $entries = & $tar -tzf $Archive
    if ($LASTEXITCODE -ne 0) { throw 'corral: invalid web app archive.' }
    foreach ($entry in $entries) {
        $entry = $entry -creplace '^\./', '' -creplace '/$', ''
        if ($entry -in @('', '.')) { continue }
        if ($entry -match '^/|[\x00-\x1F\\:<>"|?*]|(^|/)\.\.?($|/)|//') { throw 'corral: unsafe archive paths.' }
    }
    $entries = & $tar -tvzf $Archive
    if ($LASTEXITCODE -ne 0) { throw 'corral: invalid web app archive.' }
    if ($entries -match '^[^d-]') { throw 'corral: archive contains links or special files.' }

    $parent = [IO.Path]::GetDirectoryName($Destination)
    if (-not $parent) { throw "corral: unsafe web app directory: $Destination" }
    $marker = '.corral-web-install'
    $markerContents = "corral-web-install-v1`n"
    # Lock the parent rather than the leaf, so aliases of the same destination contend.
    $lockPath = Join-Path $parent '.corral-web-install.lock'
    if ($Destination -eq $lockPath) { throw "corral: web app destination is reserved: $Destination" }
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    $transaction = Join-Path $parent ('.corral-app-install-' + [guid]::NewGuid())
    $site = Join-Path $transaction 'site'
    $old = Join-Path $transaction 'old'
    $lock = $null
    $backedUp = $false
    $promoted = $false
    try {
        [IO.Directory]::CreateDirectory($site) | Out-Null
        & $tar -xzf $Archive -C $site
        if ($LASTEXITCODE -ne 0) { throw 'corral: web app archive cannot be extracted.' }
        if (-not (Test-RegularFile (Join-Path $site 'index.html'))) {
            throw 'corral: web app archive has no root index.html.'
        }
        $stagedMarker = Join-Path $site $marker
        if (Get-Item -LiteralPath $stagedMarker -Force -ErrorAction SilentlyContinue) {
            throw "corral: web app archive contains reserved file $marker."
        }
        [IO.File]::WriteAllText($stagedMarker, $markerContents, [Text.Encoding]::ASCII)
        try {
            $lock = [IO.File]::Open($lockPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        } catch {
            throw "corral: cannot acquire install lock: $lockPath (another install may be running; remove a stale lock only after checking the destination): $($_.Exception.Message)"
        }
        if (Get-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue) {
            Assert-WebAppSite $Destination $marker $markerContents
            [IO.Directory]::Move($Destination, $old)
            $backedUp = $true
            # An independent writer can change the site between preflight and move.
            # Validate the actual backup before promoting the new site.
            Assert-WebAppSite $old $marker $markerContents
        }
        [IO.Directory]::Move($site, $Destination)
        $promoted = $true
    } finally {
        try {
            # Exact-path moves refuse to overwrite unexpected destinations. If restore
            # fails, throw before deleting the transaction so the backup survives.
            if ($backedUp -and -not $promoted) {
                try { [IO.Directory]::Move($old, $Destination) }
                catch { throw "corral: cannot restore $Destination; backup in $old`: $($_.Exception.Message)" }
            }
            if ([IO.Directory]::Exists($transaction)) { [IO.Directory]::Delete($transaction, $true) }
        } finally {
            if ($null -ne $lock) {
                $lock.Dispose()
                [IO.File]::Delete($lockPath)
            }
        }
    }
    Write-Output "installed the Corral web app ($Tag) to $Destination"
    Write-Output 'Keep this directory dedicated to Corral; upgrades replace its contents.'
    Write-Output 'Serve this directory with a web server that falls back to index.html for app routes. The current daemon must run on a supported Unix host.'
}

function Install-LegacyCorral($Archive, $Temporary, $Directory, $Tag, $ReleaseRepo) {
    $staged = $null
    try {
        $expanded = Join-Path $Temporary 'expanded'
        Expand-Archive -LiteralPath $Archive -DestinationPath $expanded
        $binary = Join-Path $expanded 'corral.exe'
        if (-not (Test-RegularFile $binary)) { throw 'corral: archive has no regular corral.exe.' }
        [IO.Directory]::CreateDirectory($Directory) | Out-Null
        $staged = Join-Path $Directory ('.corral-install-' + [guid]::NewGuid() + '.exe')
        [IO.File]::Copy($binary, $staged)
        $identity = & $staged --version
        if ($LASTEXITCODE -ne 0 -or $identity -isnot [string] -or $identity -notmatch '^corral \S+$') {
            throw 'corral: downloaded binary cannot run or identify itself; existing install unchanged.'
        }
        if ($Tag -ne 'latest' -and $identity -cne "corral $($Tag.Substring(1))") {
            throw "corral: version mismatch: requested $Tag, got $identity; existing install unchanged."
        }
        $destination = Join-Path $Directory 'corral.exe'
        if ([IO.File]::Exists($destination)) {
            # Replace on the same filesystem; sharing violations leave the old file intact.
            [IO.File]::Replace($staged, $destination, [System.Management.Automation.Language.NullString]::Value)
        } else {
            [IO.File]::Move($staged, $destination)
        }
        $staged = $null
        Write-Output "installed $identity to $destination"
        if (($env:PATH -split ';') -notcontains $Directory) {
            Write-Output "Add $Directory to your user PATH in Settings > System > About > Advanced system settings > Environment Variables, then open a new terminal."
            Write-Output ('For this PowerShell session: $env:PATH = "' + $Directory + ';$env:PATH"')
        }
        Write-Output 'Before corral up: install herdr >= 0.9.3 (https://herdr.dev/docs/install/), Git for Windows with Git Bash on PATH, and authenticate an agent CLI.'
        Write-Output "Windows pane-shell setup: https://github.com/$ReleaseRepo/blob/main/docs/guide/install.md#native-windows"
    } finally {
        if ($staged -and [IO.File]::Exists($staged)) { [IO.File]::Delete($staged) }
    }
}

function Install-Corral {
    $ErrorActionPreference = 'Stop'
    if ($env:OS -ne 'Windows_NT') {
        throw 'corral: install.ps1 supports native Windows only; use install.sh on Linux/macOS.'
    }
    if ($AppOnly -and $Legacy) { throw 'corral: -AppOnly and -Legacy are mutually exclusive.' }
    if (-not $Version) { $Version = 'latest' }
    if ($Version -cne 'latest' -and $Version -cnotmatch '\Av[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\z') {
        throw "corral: invalid release tag '$Version' (expected latest or vX.Y.Z)."
    }
    if (-not $Repo) { $Repo = 'pawissanutt/corral' }
    if (-not $BaseUrl) { $BaseUrl = "https://github.com/$Repo/releases" }
    $base = $BaseUrl.TrimEnd('/')
    if ($AppOnly) {
        if (-not $AppInstallDir) { $AppInstallDir = Join-Path $env:LOCALAPPDATA 'Programs\corral\web' }
        $asset = 'corral-app-web.tar.gz'
        $destination = [IO.Path]::GetFullPath($AppInstallDir)
        if ($destination -eq [IO.Path]::GetPathRoot($destination)) { throw "corral: unsafe web app directory: $destination" }
        $destination = $destination.TrimEnd([char[]] '\/')
    } else {
        if ($Version -eq 'latest' -and $base -eq 'https://github.com/pawissanutt/corral/releases') {
            throw 'corral: the current runtime is Unix-only. Use -AppOnly for the static app or -Version vX.Y.Z for a published legacy Windows CLI.'
        }
        $architecture = $env:PROCESSOR_ARCHITEW6432
        if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
        if ($architecture -ne 'AMD64') {
            throw "corral: unsupported Windows CPU $architecture; only legacy x86_64 CLI releases are available. Use -AppOnly for the static app."
        }
        if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\corral\bin' }
        $asset = 'corral-x86_64-pc-windows-msvc.zip'
        $destination = [IO.Path]::GetFullPath($InstallDir)
    }
    $url = if ($Version -eq 'latest') { "$base/latest/download/$asset" } else { "$base/download/$Version/$asset" }
    if ($DryRun) {
        Write-Output "would download and verify $url into $destination"
        if ($AppOnly) {
            Write-Output 'static web app only; the current CLI, daemon and Pi are not available for native Windows'
        } else {
            Write-Output 'legacy CLI only; requires herdr >= 0.9.3, Git for Windows (including Git Bash), and an authenticated agent CLI before corral up'
        }
        return
    }

    # Windows PowerShell 5.1 may otherwise use obsolete TLS defaults.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ('corral-install-' + [guid]::NewGuid())
    try {
        [IO.Directory]::CreateDirectory($temporary) | Out-Null
        $archive = Receive-VerifiedArchive $url $temporary $asset
        if ($AppOnly) {
            Install-CorralWebApp $archive $destination $Version
        } else {
            Install-LegacyCorral $archive $temporary $destination $Version $Repo
        }
    } finally {
        if ([IO.Directory]::Exists($temporary)) { [IO.Directory]::Delete($temporary, $true) }
    }
}

Install-Corral
