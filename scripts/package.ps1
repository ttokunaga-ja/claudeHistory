[CmdletBinding()]
param([string]$CargoToolchain = 'stable-x86_64-pc-windows-msvc')

$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitOperatingSystem) { throw 'Packaging requires native Windows x64.' }
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $hostInfo = @(& rustc "+$CargoToolchain" -vV)
    if ($LASTEXITCODE -ne 0 -or -not ($hostInfo -match '^host: x86_64-pc-windows-msvc$')) {
        throw 'Packaging requires an x86_64 Windows MSVC Rust host.'
    }
    & cargo "+$CargoToolchain" build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed.' }
    $binary = Join-Path $root 'target\release\claudeHistory.exe'
    $version = @(& $binary --version)
    if ($LASTEXITCODE -ne 0 -or $version.Count -ne 1 -or $version[0] -cne 'claudeHistory 0.5.0') { throw 'Unexpected binary version.' }
    $destination = Join-Path $root 'target\package\claudeHistory-windows-x64'
    if (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Recurse -Force }
    [void][IO.Directory]::CreateDirectory($destination)
    Copy-Item -LiteralPath $binary -Destination (Join-Path $destination 'claudeHistory.exe')
    foreach ($file in @('scripts\install.ps1', 'README.md', 'LICENSE')) {
        Copy-Item -LiteralPath (Join-Path $root $file) -Destination $destination
    }
    $hash = (Get-FileHash -LiteralPath (Join-Path $destination 'claudeHistory.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText((Join-Path $destination 'SHA256SUMS'), "$hash  claudeHistory.exe`n", [Text.Encoding]::ASCII)
    $archive = "$destination.zip"
    Compress-Archive -LiteralPath $destination -DestinationPath $archive -Force
    Write-Host $archive
} finally { Pop-Location }
