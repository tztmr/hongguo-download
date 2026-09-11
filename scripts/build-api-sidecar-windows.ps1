param(
    [string]$Python = "python",
    [string]$TargetTriple = "x86_64-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("hongguo-api-build-" + [guid]::NewGuid())
$Destination = Join-Path $ProjectRoot "desktop/src-tauri/binaries"

try {
    & $Python -m venv (Join-Path $Temporary "venv")
    $VenvPython = Join-Path $Temporary "venv/Scripts/python.exe"
    & $VenvPython -m pip install --disable-pip-version-check -r (Join-Path $ProjectRoot "requirements.txt") -r (Join-Path $ProjectRoot "requirements-build.txt")
    if ($LASTEXITCODE -ne 0) { throw "API build dependencies failed" }
    & $VenvPython (Join-Path $PSScriptRoot "api_source_manifest.py") (Join-Path $Temporary "api-build.json")
    if ($LASTEXITCODE -ne 0) { throw "API source manifest failed" }
    & $VenvPython -m PyInstaller --noconfirm --clean --onefile --name hongguo-api `
        --add-data "$(Join-Path $Temporary 'api-build.json');." `
        --paths $ProjectRoot (Join-Path $ProjectRoot "main.py")
    if ($LASTEXITCODE -ne 0) { throw "API compilation failed" }
    $Built = Join-Path $ProjectRoot "dist/hongguo-api.exe"
    if (-not (Test-Path $Built -PathType Leaf)) {
        throw "PyInstaller did not produce hongguo-api.exe"
    }
    $Probe = Join-Path $Temporary "probe-data"
    New-Item -ItemType Directory -Force $Probe | Out-Null
    $env:HONGGUO_DATA_DIR = $Probe
    & $Built --health-probe
    if ($LASTEXITCODE -ne 0) { throw "API sidecar health probe failed" }
    node (Join-Path $PSScriptRoot "verify-api-sidecar.mjs") $Built
    if ($LASTEXITCODE -ne 0) { throw "API source verification failed" }
    New-Item -ItemType Directory -Force $Destination | Out-Null
    Copy-Item -Force $Built (Join-Path $Destination "hongguo-api-$TargetTriple.exe")
}
finally {
    Remove-Item Env:HONGGUO_DATA_DIR -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force (Join-Path $ProjectRoot "build") -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force (Join-Path $ProjectRoot "dist") -ErrorAction SilentlyContinue
    Remove-Item -Force (Join-Path $ProjectRoot "hongguo-api.spec") -ErrorAction SilentlyContinue
}
