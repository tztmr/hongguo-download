param(
    [ValidateSet("modern", "legacy", "cpu")][string]$Flavor = "modern",
    [string]$Python = "python",
    [string]$OutputDirectory = ""
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $ProjectRoot "dist/ai-components" }
$Temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("hongguo-ai-build-" + [guid]::NewGuid())

try {
    & $Python -m venv (Join-Path $Temporary "venv")
    $VenvPython = Join-Path $Temporary "venv/Scripts/python.exe"
    & $VenvPython -m pip install --disable-pip-version-check -r (Join-Path $ProjectRoot "requirements-ai-windows-$Flavor.txt")
    & $VenvPython -m pip install --disable-pip-version-check -r (Join-Path $ProjectRoot "requirements-ai-windows-common.txt")
    & $VenvPython -m PyInstaller --clean --noconfirm --onedir --name hongguo-ai-worker `
        --paths $ProjectRoot --collect-all torch --collect-all torchaudio --collect-all demucs --collect-all whisper --collect-all soundfile `
        --distpath (Join-Path $Temporary "dist") --workpath (Join-Path $Temporary "work") `
        --specpath (Join-Path $Temporary "spec") (Join-Path $ProjectRoot "ai_worker/main.py")
    $Worker = Join-Path $Temporary "dist/hongguo-ai-worker/hongguo-ai-worker.exe"
    if (-not (Test-Path $Worker -PathType Leaf)) { throw "PyInstaller did not produce the AI worker" }
    & $Worker --self-test | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "AI worker self test failed" }
    New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
    $Archive = Join-Path $OutputDirectory "hongguo-ai-runtime-windows-$Flavor.zip"
    Compress-Archive -Path (Join-Path $Temporary "dist/hongguo-ai-worker") -DestinationPath $Archive -Force
    [pscustomobject]@{
        Flavor = $Flavor
        Archive = (Resolve-Path $Archive).Path
        Sha256 = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
        DownloadBytes = (Get-Item $Archive).Length
        InstalledBytes = (Get-ChildItem (Join-Path $Temporary "dist/hongguo-ai-worker") -File -Recurse | Measure-Object Length -Sum).Sum
    } | ConvertTo-Json
}
finally {
    Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue
}
