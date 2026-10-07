$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run these tests on Windows.' }
$root = Split-Path $PSScriptRoot -Parent
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('claudeHistory-installer-test-' + [Guid]::NewGuid().ToString('N'))
$package = Join-Path $temporary 'package'
$bin = Join-Path $temporary 'bin'
$source = Join-Path $package 'claudeHistory.exe'
$installed = Join-Path $bin 'claudeHistory.exe'
$manifest = Join-Path $package 'SHA256SUMS'
$installer = Join-Path $package 'install.ps1'
function Write-Manifest {
    $hash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText($manifest, "$hash  claudeHistory.exe`n", [Text.Encoding]::ASCII)
}
function Assert-Rejected([string]$Case) {
    $failed = $false
    try { & $installer -BinDir $bin } catch { $failed = $true }
    if (-not $failed) { throw "Installer accepted $Case." }
    if ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ne $script:goodHash) {
        throw "Installer changed the existing binary after $Case."
    }
    if (@(Get-ChildItem -LiteralPath $bin -Force).Count -ne 1) { throw "Installer left temporary files after $Case." }
}
try {
    [void][IO.Directory]::CreateDirectory($package)
    Copy-Item -LiteralPath (Join-Path $root 'target\release\claudeHistory.exe') -Destination $source
    Copy-Item -LiteralPath (Join-Path $root 'scripts\install.ps1') -Destination $installer
    Write-Manifest
    & $installer -BinDir $bin
    if (@(& $installed --version)[0] -cne 'claudeHistory 0.3.0' -or $LASTEXITCODE -ne 0) { throw 'Valid installation failed.' }
    $script:goodHash = (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash
    # Exercise atomic replacement of an existing executable as well.
    & $installer -BinDir $bin
    if ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ne $script:goodHash) { throw 'Replacement failed.' }
    [IO.File]::WriteAllText($manifest, (('0' * 64) + "  claudeHistory.exe`n"), [Text.Encoding]::ASCII)
    Assert-Rejected 'corrupt checksum'
    Write-Manifest
    [IO.File]::AppendAllText($manifest, [IO.File]::ReadAllText($manifest), [Text.Encoding]::ASCII)
    Assert-Rejected 'duplicate manifest'
    # Replace the embedded ASCII version with an equal-length value, retaining a runnable PE.
    $bytes = [IO.File]::ReadAllBytes($source)
    $text = [Text.Encoding]::ASCII.GetString($bytes)
    $count = 0
    $offset = 0
    while (($offset = $text.IndexOf('0.3.0', $offset, [StringComparison]::Ordinal)) -ge 0) {
        $bytes[$offset + 4] = [byte][char]'1'
        $count++
        $offset += 5
    }
    if ($count -eq 0) { throw 'Could not create the wrong-version fixture.' }
    [IO.File]::WriteAllBytes($source, $bytes)
    $wrongVersion = @(& $source --version)
    if ($LASTEXITCODE -ne 0 -or $wrongVersion.Count -ne 1 -or $wrongVersion[0] -cne 'claudeHistory 0.3.1') {
        throw 'Wrong-version fixture did not run as expected.'
    }
    Write-Manifest
    Assert-Rejected 'wrong version with valid checksum'
    Write-Host 'Installer valid/replacement/corrupt/duplicate/wrong-version tests passed.'
} finally {
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Recurse -Force }
}
