$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run these tests on Windows.' }
$root = Split-Path $PSScriptRoot -Parent
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('claudeHistory-remote-test-' + [Guid]::NewGuid().ToString('N'))
$package = Join-Path $temporary 'claudeHistory-windows-x64'
$fixtureArchive = Join-Path $temporary 'claudeHistory-windows-x64.zip'
$fixtureManifest = Join-Path $temporary 'SHA256SUMS'
$bin = Join-Path $temporary 'bin with spaces'
$installed = Join-Path $bin 'claudeHistory.exe'
$remoteSource = [IO.File]::ReadAllText((Join-Path $root 'install.ps1'))
$oldBin = $env:BIN_DIR
$oldNoPath = $env:CLAUDE_HISTORY_INSTALL_NO_PATH
$oldSessionPath = $env:Path

function Read-UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try {
        if (-not $key -or $key.GetValueNames() -notcontains 'Path') { return 'ABSENT' }
        $raw = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        return ($key.GetValueKind('Path').ToString() + ':' + $raw)
    } finally { if ($key) { $key.Close() } }
}
$oldUserPath = Read-UserPath

# These fixtures exercise the production download, hash, extraction, and package
# installer path. Only the network request is replaced, within this test scope.
function Invoke-WebRequest {
    param([switch]$UseBasicParsing, [int]$TimeoutSec, [string]$Uri, [string]$OutFile)
    $base = 'https://github.com/ttokunaga-ja/claudeHistory/releases/latest/download'
    switch ($Uri) {
        "$base/claudeHistory-windows-x64.zip" { Copy-Item -LiteralPath $fixtureArchive -Destination $OutFile }
        "$base/SHA256SUMS" { Copy-Item -LiteralPath $fixtureManifest -Destination $OutFile }
        default { throw "Unexpected download URL: $Uri" }
    }
}
function Write-ArchiveManifest {
    $hash = (Get-FileHash -LiteralPath $fixtureArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    # Other platform entries may coexist in the release's outer manifest.
    [IO.File]::WriteAllText($fixtureManifest, "$hash  claudeHistory-windows-x64.zip`n$('1' * 64)  claudeHistory-linux-x64.tar.gz`n", [Text.Encoding]::ASCII)
}
function Assert-UnchangedPath {
    if ($env:Path -cne $oldSessionPath -or (Read-UserPath) -cne $oldUserPath) {
        throw 'Installer modified PATH during an isolated test.'
    }
}
function Assert-Rejected([string]$Case, [string]$ErrorPattern) {
    $failure = $null
    try { Invoke-Expression $remoteSource } catch { $failure = $_ }
    if (-not $failure -or $failure.Exception.Message -notmatch $ErrorPattern) {
        throw "Installer did not reject $Case with the expected error: $failure"
    }
    if ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ne $goodHash) {
        throw "Installer changed the existing binary after $Case."
    }
    if (@(Get-ChildItem -LiteralPath $bin -Force).Count -ne 1) { throw "Installer left temporary files after $Case." }
    Assert-UnchangedPath
}
try {
    [void][IO.Directory]::CreateDirectory($package)
    Copy-Item -LiteralPath (Join-Path $root 'target\release\claudeHistory.exe') -Destination (Join-Path $package 'claudeHistory.exe')
    Copy-Item -LiteralPath (Join-Path $root 'scripts\install.ps1') -Destination (Join-Path $package 'install.ps1')
    $hash = (Get-FileHash -LiteralPath (Join-Path $package 'claudeHistory.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText((Join-Path $package 'SHA256SUMS'), "$hash  claudeHistory.exe`n", [Text.Encoding]::ASCII)
    Compress-Archive -LiteralPath $package -DestinationPath $fixtureArchive
    Write-ArchiveManifest
    $env:BIN_DIR = $bin
    $env:CLAUDE_HISTORY_INSTALL_NO_PATH = '1'
    Invoke-Expression $remoteSource
    $version = @(& $installed --version)
    if ($LASTEXITCODE -ne 0 -or $version.Count -ne 1 -or $version[0] -cne 'claudeHistory 0.4.0') { throw 'Valid remote installation failed.' }
    $goodHash = (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash
    Assert-UnchangedPath
    Invoke-Expression $remoteSource
    if ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash -ne $goodHash) { throw 'Replacement failed.' }
    Assert-UnchangedPath

    [IO.File]::WriteAllText($fixtureManifest, (('0' * 64) + "  claudeHistory-windows-x64.zip`n"), [Text.Encoding]::ASCII)
    Assert-Rejected 'corrupt archive checksum' 'Archive SHA-256 mismatch'
    Write-ArchiveManifest
    [IO.File]::AppendAllText($fixtureManifest, [IO.File]::ReadAllText($fixtureManifest), [Text.Encoding]::ASCII)
    Assert-Rejected 'duplicate archive manifest' 'exactly one well-formed'
    [IO.File]::WriteAllText($fixtureManifest, "invalid  claudeHistory-windows-x64.zip`n", [Text.Encoding]::ASCII)
    Assert-Rejected 'malformed archive manifest' 'exactly one well-formed'
    Write-ArchiveManifest
    foreach ($invalid in @('relative\bin', 'C:relative\bin', '\root-relative\bin', ($bin + ';extra'), ($bin + "`nextra"))) {
        $env:BIN_DIR = $invalid
        Assert-Rejected 'invalid BIN_DIR' 'BIN_DIR must'
    }
    $env:BIN_DIR = $bin

    # A verified archive with an invalid inner package must propagate the child
    # installer's failure without replacing the previously installed executable.
    [IO.File]::WriteAllText((Join-Path $package 'SHA256SUMS'), (('0' * 64) + "  claudeHistory.exe`n"), [Text.Encoding]::ASCII)
    Compress-Archive -LiteralPath $package -DestinationPath $fixtureArchive -Force
    Write-ArchiveManifest
    Assert-Rejected 'package installer failure' 'Package installation failed'
    Write-Host 'Remote installer success/replacement/corrupt/duplicate/malformed/invalid-path/child-failure tests passed.'
} finally {
    $env:BIN_DIR = $oldBin
    $env:CLAUDE_HISTORY_INSTALL_NO_PATH = $oldNoPath
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Recurse -Force }
}
