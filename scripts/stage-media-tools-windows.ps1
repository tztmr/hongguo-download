param(
    [Parameter(Mandatory = $true)][string]$Archive,
    [Parameter(Mandatory = $true)][string]$ArchiveSha256,
    [string]$TargetTriple = "x86_64-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Destination = Join-Path $ProjectRoot "desktop/src-tauri/binaries"
$Temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("hongguo-ffmpeg-" + [guid]::NewGuid())

try {
    $Actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
    if ($Actual -ne $ArchiveSha256.ToLowerInvariant()) { throw "FFmpeg archive checksum mismatch" }
    New-Item -ItemType Directory -Force $Temporary | Out-Null
    tar.exe -xf $Archive -C $Temporary
    if ($LASTEXITCODE -ne 0) { throw "FFmpeg archive extraction failed" }
    $Ffmpeg = Get-ChildItem -Path $Temporary -Filter ffmpeg.exe -File -Recurse | Select-Object -First 1
    $Ffprobe = Get-ChildItem -Path $Temporary -Filter ffprobe.exe -File -Recurse | Select-Object -First 1
    if (-not $Ffmpeg -or -not $Ffprobe) { throw "FFmpeg archive does not contain ffmpeg.exe and ffprobe.exe" }
    & $Ffmpeg.FullName -version | Out-Null
    & $Ffprobe.FullName -version | Out-Null
    New-Item -ItemType Directory -Force $Destination | Out-Null
    Copy-Item -Force $Ffmpeg.FullName (Join-Path $Destination "ffmpeg-$TargetTriple.exe")
    Copy-Item -Force $Ffprobe.FullName (Join-Path $Destination "ffprobe-$TargetTriple.exe")
}
finally {
    Remove-Item -Recurse -Force $Temporary -ErrorAction SilentlyContinue
}
