use anyhow::{Result, bail};
use std::fs;
use std::path::Path;
use std::process::Command;

const EXTRACT: &str = include_str!("extract.ps1");

pub(super) fn extract(archive: &Path, candidate: &Path) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let script = dir.path().join("extract.ps1");
    fs::write(&script, EXTRACT)?;
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .arg("-Archive")
        .arg(archive)
        .arg("-Candidate")
        .arg(candidate)
        .output()?;
    if !output.status.success() {
        bail!(
            "更新アーカイブを取り出せません: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn test_archive(binary: &[u8]) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("claudeHistory-windows-x64");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("claudeHistory.exe"), binary).unwrap();
    let archive = dir.path().join("archive.zip");
    let script = dir.path().join("pack.ps1");
    fs::write(&script, "param($Source,$Destination)\n$ErrorActionPreference='Stop'\nCompress-Archive -LiteralPath $Source -DestinationPath $Destination\n").unwrap();
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .arg("-Source")
        .arg(root)
        .arg("-Destination")
        .arg(&archive)
        .status()
        .unwrap();
    assert!(status.success());
    fs::read(archive).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(entries: &[(&str, i32)]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("entries.zip");
        let entries_file = dir.path().join("entries.json");
        fs::write(
            &entries_file,
            serde_json::to_vec(
                &entries
                    .iter()
                    .map(|(name, attrs)| serde_json::json!({ "name": name, "attrs": attrs }))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .unwrap();
        let script = dir.path().join("fixture.ps1");
        fs::write(&script, r#"param($Destination,$Entries)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip=[IO.Compression.ZipFile]::Open($Destination,[IO.Compression.ZipArchiveMode]::Create)
try {
    foreach($spec in (Get-Content -LiteralPath $Entries -Raw | ConvertFrom-Json)) {
        $entry=$zip.CreateEntry($spec.name)
        $entry.ExternalAttributes=[int]$spec.attrs
        $stream=$entry.Open()
        try { $bytes=[Text.Encoding]::ASCII.GetBytes('binary'); $stream.Write($bytes,0,$bytes.Length) } finally { $stream.Dispose() }
    }
} finally { $zip.Dispose() }
"#).unwrap();
        let status = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(script)
            .arg("-Destination")
            .arg(&archive)
            .arg("-Entries")
            .arg(entries_file)
            .status()
            .unwrap();
        assert!(status.success());
        (dir, archive)
    }

    #[test]
    fn archive_rejection_happens_before_candidate_write() {
        let bin = "claudeHistory-windows-x64/claudeHistory.exe";
        for entries in [
            vec![(bin, 0), (bin, 0)],
            vec![(bin, 0), ("claudeHistory-windows-x64/../escaped", 0)],
            vec![(bin, (0xa000_u32 << 16) as i32)],
            vec![("claudeHistory-windows-x64/README.md", 0)],
            vec![(bin, 0), ("CLAUDEHISTORY-WINDOWS-X64/CLAUDEHISTORY.EXE", 0)],
        ] {
            let (dir, archive) = fixture(&entries);
            let candidate = dir.path().join("candidate.exe");
            fs::write(&candidate, b"unchanged").unwrap();
            assert!(extract(&archive, &candidate).is_err());
            assert_eq!(fs::read(candidate).unwrap(), b"unchanged");
            assert!(!dir.path().join("escaped").exists());
        }
    }
}
