param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [switch]$InstallSmokeTest,
    [string]$PreviousInstaller
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path $Installer -PathType Leaf)) { throw "Windows installer is missing" }
$Hash = Get-FileHash -Algorithm SHA256 $Installer
$Signature = Get-AuthenticodeSignature $Installer
[pscustomobject]@{
    Path = (Resolve-Path $Installer).Path
    Sha256 = $Hash.Hash.ToLowerInvariant()
    SignatureStatus = $Signature.Status.ToString()
} | ConvertTo-Json

if (-not $InstallSmokeTest) { return }

function Invoke-Installer([string]$Path, [string]$Arguments) {
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru
    if (-not $process.WaitForExit(180000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "Installer timed out: $Path"
    }
    if ($process.ExitCode -notin @(0, 3010)) { throw "Installer failed: $($process.ExitCode)" }
}

# Run only on an isolated CI machine. Exercise Unicode paths, locked executables,
# same-version replacement, and preservation of application data during upgrade.
$InstallDir = Join-Path $env:TEMP ("红果 覆盖安装-" + [guid]::NewGuid())
$DataDir = Join-Path $env:APPDATA 'com.edking.hongguo.desktop'
$Marker = Join-Path $DataDir ("upgrade-test-" + [guid]::NewGuid() + '.txt')
$App = Join-Path $InstallDir 'hongguo-desktop.exe'
$AppProcess = $null
try {
    $InitialInstaller = if ($PreviousInstaller) { $PreviousInstaller } else { $Installer }
    Invoke-Installer $InitialInstaller "/S /D=$InstallDir"
    if (-not (Test-Path $App)) { throw 'Installed application is missing' }
    New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
    Set-Content -Path $Marker -Value 'preserve-settings-and-models' -Encoding utf8
    foreach ($Pass in @('upgrade', 'reinstall')) {
        $AppProcess = Start-Process -FilePath $App -PassThru
        Start-Sleep -Seconds 5
        if ($AppProcess.HasExited) { throw "Application failed to start before $Pass" }
        Invoke-Installer $Installer "/S /UPDATE /D=$InstallDir"
        if (-not $AppProcess.WaitForExit(10000)) { throw 'Old application still running after replacement' }
        if ((Get-Content $Marker -Raw).Trim() -ne 'preserve-settings-and-models') {
            throw 'Upgrade removed application data'
        }
        $Expected = Join-Path $PSScriptRoot '../desktop/src-tauri/target/release/hongguo-desktop.exe'
        if ((Get-FileHash $App).Hash -ne (Get-FileHash $Expected).Hash) {
            throw 'Installed executable does not match this build'
        }
        Write-Host "Passed running application $Pass in a Unicode path"
    }
    $AppProcess = Start-Process -FilePath $App -PassThru
    Start-Sleep -Seconds 5
    if ($AppProcess.HasExited) { throw 'Updated application failed to start' }
}
finally {
    if ($AppProcess -and -not $AppProcess.HasExited) {
        Stop-Process -Id $AppProcess.Id -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path (Join-Path $InstallDir 'uninstall.exe')) {
        Invoke-Installer (Join-Path $InstallDir 'uninstall.exe') '/S'
    }
    Remove-Item $Marker -Force -ErrorAction SilentlyContinue
}
