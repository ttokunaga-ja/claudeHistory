# irm https://raw.githubusercontent.com/ttokunaga-ja/claudeHistory/main/install.ps1 | iex
# BIN_DIR overrides the destination. CLAUDE_HISTORY_INSTALL_NO_PATH=1 skips PATH changes.
# Keep this scoped and never call exit: iex runs inside the user's PowerShell.
& {
    $ErrorActionPreference = 'Stop'
    if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitOperatingSystem) {
        throw 'This installer requires native Windows x64.'
    }
    $base = 'https://github.com/ttokunaga-ja/claudeHistory/releases/latest/download'
    $bin = if ($env:BIN_DIR) { $env:BIN_DIR } else { Join-Path $env:USERPROFILE '.local\bin' }
    if ($bin -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$))') {
        throw 'BIN_DIR must be an absolute path.'
    }
    if ($bin.Contains(';') -or $bin.Contains("`r") -or $bin.Contains("`n")) {
        throw 'BIN_DIR must not contain PATH separators or newlines.'
    }
    $bin = [IO.Path]::GetFullPath($bin)
    if ($bin -ne [IO.Path]::GetPathRoot($bin)) { $bin = $bin.TrimEnd('\') }
    if ($env:CLAUDE_HISTORY_INSTALL_NO_PATH -ne '1' -and $bin.Contains('%')) {
        throw 'BIN_DIR must not contain % when configuring PATH.'
    }
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('claudeHistory-install-' + [Guid]::NewGuid().ToString('N'))
    try {
        [void][IO.Directory]::CreateDirectory($temp)
        $asset = 'claudeHistory-windows-x64.zip'
        $download = Join-Path $temp $asset
        $manifest = Join-Path $temp 'SHA256SUMS'
        Invoke-WebRequest -UseBasicParsing -TimeoutSec 300 -Uri "$base/$asset" -OutFile $download
        Invoke-WebRequest -UseBasicParsing -TimeoutSec 300 -Uri "$base/SHA256SUMS" -OutFile $manifest
        $entries = @(Get-Content -LiteralPath $manifest | Where-Object { $_ -match '\s+\*?claudeHistory-windows-x64\.zip\s*$' })
        if ($entries.Count -ne 1 -or $entries[0] -notmatch '^([0-9a-fA-F]{64}) [ *]claudeHistory-windows-x64\.zip$') {
            throw 'SHA256SUMS must contain exactly one well-formed Windows archive entry.'
        }
        $expected = $Matches[1]
        if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ine $expected) {
            throw 'Archive SHA-256 mismatch; the installed executable has not been changed.'
        }
        $expanded = Join-Path $temp 'expanded'
        Expand-Archive -LiteralPath $download -DestinationPath $expanded
        $packageInstaller = Join-Path $expanded 'claudeHistory-windows-x64\install.ps1'
        if (-not (Test-Path -LiteralPath $packageInstaller -PathType Leaf)) { throw 'Package installer is missing.' }
        # The package installer owns binary verification and atomic replacement.
        try {
            & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $packageInstaller -BinDir $bin
        } catch { throw "Package installation failed: $($_.Exception.Message)" }
        if ($LASTEXITCODE -ne 0) { throw 'Package installation failed.' }
        $exe = Join-Path $bin 'claudeHistory.exe'
        $version = @(& $exe --version)
        if ($LASTEXITCODE -ne 0 -or $version.Count -ne 1 -or $version[0] -cnotmatch '^claudeHistory [0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
            throw 'Could not confirm the installed executable version.'
        }
        Write-Host "Installed: $exe ($($version[0]))"

        if ($env:CLAUDE_HISTORY_INSTALL_NO_PATH -ne '1') {
            $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
            try {
                $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                $present = @($raw -split ';' | Where-Object { [Environment]::ExpandEnvironmentVariables($_).Trim().Trim('"').TrimEnd('\') -ieq $bin })
                if ($present.Count -eq 0) {
                    $kind = if ($key.GetValueNames() -contains 'Path') { $key.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
                    $separator = if ($raw.Length -eq 0 -or $raw.EndsWith(';')) { '' } else { ';' }
                    $key.SetValue('Path', ($raw + $separator + $bin), $kind)
                    # Notify Explorer of the user PATH change.
                    if (-not ('ClaudeHistoryInstaller.EnvironmentNotification' -as [type])) {
                        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace ClaudeHistoryInstaller {
    public static class EnvironmentNotification {
        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern IntPtr SendMessageTimeout(IntPtr window, uint message, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);
    }
}
'@
                    }
                    $result = [UIntPtr]::Zero
                    [void][ClaudeHistoryInstaller.EnvironmentNotification]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
                    Write-Host "Added to user PATH: $bin"
                }
            } finally { $key.Close() }
            $current = @($env:Path -split ';' | Where-Object { [Environment]::ExpandEnvironmentVariables($_).Trim().Trim('"').TrimEnd('\') -ieq $bin })
            if ($current.Count -eq 0) { $env:Path = $bin + ';' + $env:Path }
            Write-Host 'claudeHistory is available in this PowerShell session.'
        }
    } finally {
        if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Recurse -Force }
    }
}
