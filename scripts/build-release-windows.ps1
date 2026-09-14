param(
    [Parameter(Mandatory = $true)][string]$FfmpegArchive,
    [Parameter(Mandatory = $true)][string]$FfmpegArchiveSha256,
    [string]$Python = "python",
    [string]$AIBaseWorker = "",
    [string]$AIPython = "python",
    [string]$TargetTriple = "x86_64-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$AIConfig = @()
if ($AIBaseWorker) {
    & $AIPython (Join-Path $PSScriptRoot "repack-ai-worker.py") $AIBaseWorker (Join-Path $ProjectRoot "desktop/src-tauri/resources/ai-worker-modern-v034")
    if ($LASTEXITCODE -ne 0) { throw "AI worker update build failed (requires Python 3.11 and PyInstaller)" }
    $AIConfig = @('--config', 'src-tauri/tauri.ai-worker.conf.json')
}

& (Join-Path $PSScriptRoot "build-api-sidecar-windows.ps1") -Python $Python -TargetTriple $TargetTriple
& (Join-Path $PSScriptRoot "stage-media-tools-windows.ps1") `
    -Archive $FfmpegArchive -ArchiveSha256 $FfmpegArchiveSha256 -TargetTriple $TargetTriple
npm ci --prefix (Join-Path $ProjectRoot "desktop")
& (Join-Path $PSScriptRoot "verify-playback-windows.ps1") -Python $Python
& $Python -m unittest discover -s (Join-Path $ProjectRoot "ai_worker/tests") -v
npm test --prefix (Join-Path $ProjectRoot "desktop") -- --run
cargo test --manifest-path (Join-Path $ProjectRoot "desktop/src-tauri/Cargo.toml") --lib
Push-Location (Join-Path $ProjectRoot "desktop")
try {
    npx tauri build --config src-tauri/tauri.release.conf.json @AIConfig --bundles nsis
}
finally {
    Pop-Location
}
