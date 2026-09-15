param([string[]]$TestArguments = @())

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$TestManifest = Join-Path (Split-Path -Parent $PSScriptRoot) "desktop/src-tauri/Cargo.toml"
$PreviousTestResources = $env:HONGGUO_WINDOWS_TEST_RESOURCES
try {
    $env:HONGGUO_WINDOWS_TEST_RESOURCES = "1"
    cargo test --manifest-path $TestManifest --lib @TestArguments
    if ($LASTEXITCODE -ne 0) { throw "Windows Rust library tests failed" }
} finally {
    $env:HONGGUO_WINDOWS_TEST_RESOURCES = $PreviousTestResources
}
