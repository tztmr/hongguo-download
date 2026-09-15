param([string]$Python = "python")
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("hongguo-playback-" + [guid]::NewGuid())
try {
    New-Item -ItemType Directory -Force $Temporary | Out-Null
    foreach ($tool in @("ffmpeg", "ffprobe")) {
        Copy-Item (Join-Path $ProjectRoot "desktop/src-tauri/binaries/$tool-x86_64-pc-windows-msvc.exe") (Join-Path $Temporary "$tool.exe")
    }
    & $Python -m venv (Join-Path $Temporary "venv")
    $TestPython = Join-Path $Temporary "venv/Scripts/python.exe"
    & $TestPython -m pip install --disable-pip-version-check -r (Join-Path $ProjectRoot "requirements.txt") playwright
    $env:HONGGUO_TEST_PLAYBACK_TOOLS = $Temporary
    Push-Location $ProjectRoot
    try {
        & $TestPython -m unittest tests.test_playback tests.test_video_download tests.test_mp4_decrypt -v
        & $TestPython scripts/verify-playback-browser.py
        & $TestPython scripts/verify-playback-stream-browser.py
        & (Join-Path $PSScriptRoot "test-rust-windows.ps1") -TestArguments @("media::merge::performance_tests", "--", "--nocapture")
    } finally { Pop-Location }
} finally {
    Remove-Item Env:HONGGUO_TEST_PLAYBACK_TOOLS -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue
}
