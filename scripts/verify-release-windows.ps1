param(
    [Parameter(Mandatory = $true)][string]$Installer
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
