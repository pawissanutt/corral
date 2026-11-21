# Run as a disposable Windows user: this exercises the real per-user install.
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$Candidate
)
$ErrorActionPreference = 'Stop'
$programs = [Environment]::GetFolderPath('Programs')
$shortcut = Join-Path $programs 'Corral.lnk'
$state = Join-Path $env:LOCALAPPDATA 'corral-app'
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Corral Desktop'
$exe = Join-Path $installDir 'Corral.exe'
$uninstall = Join-Path $installDir 'unins000.exe'
$log = Join-Path ([IO.Path]::GetTempPath()) "corral-setup-$PID.log"

foreach ($existing in @($shortcut, $installDir, $state)) {
    if (Test-Path -LiteralPath $existing) { throw "Smoke requires a clean Windows user: $existing exists" }
}
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

function Invoke-Setup([string]$Path) {
    $process = Start-Process -FilePath $Path -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$Path exited with $($process.ExitCode)" }
}

function Get-Sha256([string]$Path) {
    $sha = [Security.Cryptography.SHA256]::Create()
    $file = [IO.File]::OpenRead($Path)
    try {
        [Convert]::ToBase64String($sha.ComputeHash($file))
    } finally {
        $file.Dispose()
        $sha.Dispose()
    }
}

function Assert-Installed {
    if ((Get-Sha256 $exe) -cne (Get-Sha256 $Candidate)) {
        throw 'Installed desktop binary does not match the release candidate'
    }
    foreach ($asset in @('icons\brand.svg', 'fonts\AtkinsonHyperlegibleNext-Regular.woff2')) {
        if (!(Test-Path -LiteralPath (Join-Path $installDir "assets\@nsh-app-corral\$asset"))) {
            throw "The installed app is missing its NSH asset: $asset"
        }
    }
    $shell = New-Object -ComObject WScript.Shell
    if (!(Test-Path -LiteralPath $shortcut) -or $shell.CreateShortcut($shortcut).TargetPath -ne $exe) {
        throw 'Corral Start menu shortcut is missing or has the wrong target'
    }
}

try {
    Invoke-Setup $Installer
    Assert-Installed
    $profile = Join-Path $state 'smoke-appdata'
    $null = New-Item -Path $profile -ItemType Directory -Force
    $start = [Diagnostics.ProcessStartInfo]::new($exe)
    $start.UseShellExecute = $false
    $start.WorkingDirectory = $installDir
    $start.EnvironmentVariables['APPDATA'] = $profile
    $app = [Diagnostics.Process]::Start($start)
    try {
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        $query = [System.Windows.Automation.PropertyCondition]::new(
            [System.Windows.Automation.AutomationElement]::AutomationIdProperty, 'pair-endpoint'
        )
        $ready = $false
        do {
            $app.Refresh()
            if ($app.HasExited) { throw "Corral Desktop exited during launch ($($app.ExitCode))" }
            if ($app.MainWindowHandle -ne 0 -and $app.MainWindowTitle -eq 'Corral') {
                $window = [System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
                $ready = $null -ne $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $query)
                if ($ready) { break }
            }
            Start-Sleep -Milliseconds 250
        } while ([DateTime]::UtcNow -lt $deadline)
        if (!$ready) {
            throw 'Corral Desktop did not open to its daemon pairing form'
        }
    } finally {
        if (!$app.HasExited) { Stop-Process -Id $app.Id; $app.WaitForExit() }
        $app.Dispose()
    }

    $null = New-Item -Path $state -ItemType Directory -Force
    $sentinel = Join-Path $state 'upgrade-smoke-state'
    Set-Content -LiteralPath $sentinel -Value 'local state' -NoNewline
    # Distinct previous bytes prove the installer replaces an existing executable.
    Set-Content -LiteralPath $exe -Value 'previous install' -NoNewline
    Invoke-Setup $Installer
    Assert-Installed
    if ((Get-Content -LiteralPath $sentinel -Raw) -ne 'local state') { throw 'Upgrade removed app data' }
    Invoke-Setup $uninstall
    if ((Test-Path -LiteralPath $exe) -or (Test-Path -LiteralPath $shortcut)) { throw 'Uninstall left app files' }
    if ((Get-Content -LiteralPath $sentinel -Raw) -ne 'local state') { throw 'Uninstall removed app data' }
    Write-Host 'Windows per-user install, Win32 launch, simulated upgrade and uninstall passed'
} catch {
    if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log }
    throw
} finally {
    if (Test-Path -LiteralPath $uninstall) { Invoke-Setup $uninstall }
    Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue
    # Only paths proved absent before the smoke run can be created by this script.
    if (Test-Path -LiteralPath $state) { Remove-Item -LiteralPath $state -Recurse -Force }
}
