param(
    [Parameter(Mandatory = $true)][string]$FfmpegArchive,
    [Parameter(Mandatory = $true)][string]$FfmpegArchiveSha256,
    [string]$Python = "python"
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$ProjectRoot = Split-Path -Parent $PSScriptRoot

& (Join-Path $PSScriptRoot "build-api-sidecar-windows.ps1") -Python $Python
& (Join-Path $PSScriptRoot "stage-media-tools-windows.ps1") `
    -Archive $FfmpegArchive -ArchiveSha256 $FfmpegArchiveSha256
& $Python -m unittest discover -s (Join-Path $ProjectRoot "ai_worker/tests") -v
npm ci --prefix (Join-Path $ProjectRoot "desktop")
npm test --prefix (Join-Path $ProjectRoot "desktop") -- --run
cargo test --manifest-path (Join-Path $ProjectRoot "desktop/src-tauri/Cargo.toml") --lib
Push-Location (Join-Path $ProjectRoot "desktop")
try {
    npx tauri build --config src-tauri/tauri.release.conf.json --bundles nsis
}
finally {
    Pop-Location
}
