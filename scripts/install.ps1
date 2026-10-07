[CmdletBinding()]
param([string]$BinDir = (Join-Path $env:USERPROFILE '.local\bin'))

$ErrorActionPreference = 'Stop'

function Assert-NoReparsePoint([string]$Path) {
    $current = [IO.Path]::GetFullPath($Path)
    while ($current) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Reparse points are not allowed: $current"
            }
        }
        $parent = [IO.Path]::GetDirectoryName($current)
        if ($parent -eq $current) { break }
        $current = $parent
    }
}

function Assert-Version([string]$Path) {
    $version = @(& $Path --version)
    if ($LASTEXITCODE -ne 0 -or $version.Count -ne 1 -or $version[0] -cne 'claudeHistory 0.2.0') {
        throw 'Expected claudeHistory 0.2.0; the installed executable has not been changed.'
    }
}

if ($env:OS -ne 'Windows_NT') { throw 'This installer requires Windows.' }
if ($BinDir -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$))') {
    throw 'BinDir must be an absolute path.'
}
$bin = [IO.Path]::GetFullPath($BinDir)
$source = Join-Path $PSScriptRoot 'claudeHistory.exe'
$manifest = Join-Path $PSScriptRoot 'SHA256SUMS'
$destination = Join-Path $bin 'claudeHistory.exe'
foreach ($path in @($PSCommandPath, $source, $manifest, $destination)) { Assert-NoReparsePoint $path }
foreach ($path in @($source, $manifest)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing package file: $path" }
}
$lines = [IO.File]::ReadAllLines($manifest)
if ($lines.Count -ne 1 -or $lines[0] -cnotmatch '^([0-9a-fA-F]{64})  claudeHistory\.exe$') {
    throw 'SHA256SUMS must contain exactly one well-formed claudeHistory.exe entry.'
}
$expected = $Matches[1]
if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ine $expected) { throw 'SHA-256 mismatch.' }
Assert-Version $source
if (Test-Path -LiteralPath $destination -PathType Container) { throw 'Destination is a directory.' }
[void][IO.Directory]::CreateDirectory($bin)
Assert-NoReparsePoint $destination
$stage = Join-Path $bin ('.claudeHistory-install-' + [Guid]::NewGuid().ToString('N') + '.exe')
$backup = $null
try {
    [IO.File]::Copy($source, $stage, $false)
    if ((Get-FileHash -LiteralPath $stage -Algorithm SHA256).Hash -ine $expected) { throw 'Staged SHA-256 mismatch.' }
    Assert-Version $stage
    Assert-NoReparsePoint $destination
    if (Test-Path -LiteralPath $destination) {
        # A real backup path also works with Windows PowerShell 5.1.
        $backup = Join-Path $bin ('.claudeHistory-backup-' + [Guid]::NewGuid().ToString('N') + '.exe')
        [IO.File]::Replace($stage, $destination, $backup)
    } else {
        [IO.File]::Move($stage, $destination)
    }
    $stage = $null
    if ($backup) {
        try { Remove-Item -LiteralPath $backup -Force }
        catch { Write-Warning "Installed successfully; old executable backup remains: $backup" }
    }
    Write-Host "Installed: $destination"
    Write-Host "If needed, add $bin to your user PATH in Windows Environment Variables, then open a new terminal."
} finally {
    if ($stage -and (Test-Path -LiteralPath $stage)) { Remove-Item -LiteralPath $stage -Force }
}
