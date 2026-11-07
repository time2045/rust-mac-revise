param(
    [ValidateSet('Release', 'Debug', 'Test')][string]$Mode = 'Release',
    [string]$TargetDir = ''
)
$ErrorActionPreference = 'Stop'
if (-not $TargetDir) {
    $TargetDir = Join-Path (Split-Path $PSScriptRoot -Parent) 'mac-manager-target'
}
$TargetDir = [System.IO.Path]::GetFullPath($TargetDir)
if ($TargetDir -match '[^\x00-\x7F]') {
    throw 'MinGW requires an ASCII build path. Pass -TargetDir with an English-only absolute path.'
}
$oldTarget = $env:CARGO_TARGET_DIR
Push-Location $PSScriptRoot
try {
    $env:CARGO_TARGET_DIR = $TargetDir
    if ($Mode -eq 'Test') {
        & cargo test --locked
    } elseif ($Mode -eq 'Debug') {
        & cargo build --locked
    } else {
        & cargo build --release --locked
    }
    if ($LASTEXITCODE -ne 0) { throw "Cargo exited with code $LASTEXITCODE" }
    if ($Mode -ne 'Test') {
        $profile = if ($Mode -eq 'Release') { 'release' } else { 'debug' }
        $source = Join-Path $TargetDir "$profile/mac-address-manager.exe"
        $destination = Join-Path $PSScriptRoot 'dist'
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        Copy-Item -LiteralPath $source -Destination (Join-Path $destination 'mac-address-manager.exe') -Force
        $loader = Join-Path $TargetDir "$profile/WebView2Loader.dll"
        if (Test-Path -LiteralPath $loader) {
            Copy-Item -LiteralPath $loader -Destination (Join-Path $destination 'WebView2Loader.dll') -Force
        }
        $archive = Join-Path $PSScriptRoot 'mac-address-manager-windows-x64.zip'
        if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive -Force }
        Compress-Archive -Path (Join-Path $destination '*') -DestinationPath $archive -CompressionLevel Optimal
        Write-Output (Join-Path $destination 'mac-address-manager.exe')
        Write-Output $archive
    }
} finally {
    $env:CARGO_TARGET_DIR = $oldTarget
    Pop-Location
}
