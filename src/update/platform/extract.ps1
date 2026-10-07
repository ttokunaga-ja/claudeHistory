param([string]$Archive, [string]$Candidate)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead($Archive)
try {
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $binary = $null
    foreach ($entry in $zip.Entries) {
        $name = $entry.FullName.Replace('\', '/')
        $normalized = $name.TrimEnd('/')
        if (-not $seen.Add($normalized)) { throw 'Duplicate archive entry.' }
        $type = ($entry.ExternalAttributes -shr 16) -band 0xF000
        if ($name -ceq 'claudeHistory-windows-x64/') {
            if ($type -notin @(0, 0x4000) -or $entry.Length -ne 0) { throw 'Invalid directory.' }
        } else {
            if ($name -cnotmatch '^claudeHistory-windows-x64/(claudeHistory\.exe|install\.ps1|README\.md|LICENSE|SHA256SUMS)$' -or $type -notin @(0, 0x8000)) { throw 'Invalid archive path or link.' }
            if ($name -ceq 'claudeHistory-windows-x64/claudeHistory.exe') {
                if ($entry.Length -le 0 -or $entry.Length -gt 134217728) { throw 'Invalid executable size.' }
                $binary = $entry
            }
        }
    }
    if ($null -eq $binary) { throw 'Executable is missing.' }
    $inputStream = $binary.Open()
    try {
        $outputStream = [IO.File]::Open($Candidate, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try {
            $buffer = New-Object byte[] 81920
            [long]$total = 0
            while (($count = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                $total += $count
                if ($total -gt 134217728 -or $total -gt $binary.Length) { throw 'Executable exceeds declared size.' }
                $outputStream.Write($buffer, 0, $count)
            }
            if ($total -ne $binary.Length) { throw 'Executable size does not match.' }
            $outputStream.Flush($true)
        } finally { $outputStream.Dispose() }
    } finally { $inputStream.Dispose() }
} finally { $zip.Dispose() }
