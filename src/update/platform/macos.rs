use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

const ROOT: &str = "claudeHistory-macos-arm64";
const BINARY: &str = "claudeHistory-macos-arm64/claudeHistory";

pub(super) fn extract(archive: &Path, candidate: &Path) -> Result<()> {
    let list = Command::new("/usr/bin/tar")
        .arg("-tzf")
        .arg(archive)
        .output()?;
    let kinds = Command::new("/usr/bin/tar")
        .arg("-tvzf")
        .arg(archive)
        .output()?;
    if !list.status.success() || !kinds.status.success() {
        bail!("更新アーカイブを読み取れません");
    }
    validate_listing(
        std::str::from_utf8(&list.stdout)?,
        std::str::from_utf8(&kinds.stdout)?,
    )?;
    let mut child = Command::new("/usr/bin/tar")
        .arg("-xOzf")
        .arg(archive)
        .arg(BINARY)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .context("tar の出力がありません")?
        .take(134_217_729)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 134_217_728 {
        let _ = child.kill();
        let _ = child.wait();
        bail!("更新実行ファイルが大きすぎます");
    }
    if !child.wait()?.success() || bytes.is_empty() {
        bail!("更新実行ファイルを取り出せません");
    }
    fs::write(candidate, bytes)?;
    Ok(())
}

fn validate_listing(names: &str, kinds: &str) -> Result<()> {
    let names: Vec<_> = names.lines().collect();
    let kinds: Vec<_> = kinds.lines().collect();
    if names.len() != kinds.len() || names.is_empty() {
        bail!("更新アーカイブの一覧が不正です");
    }
    let mut seen = HashSet::new();
    let mut binary = false;
    for (name, kind) in names.into_iter().zip(kinds) {
        let normalized = name.trim_end_matches('/');
        if !seen.insert(normalized) {
            bail!("更新アーカイブに重複があります");
        }
        if name == format!("{ROOT}/") {
            if !kind.starts_with('d') {
                bail!("更新アーカイブのディレクトリが不正です");
            }
        } else {
            let file = normalized
                .strip_prefix(&format!("{ROOT}/"))
                .context("更新アーカイブのパスが不正です")?;
            if ![
                "claudeHistory",
                "install.sh",
                "README.md",
                "LICENSE",
                "SHA256SUMS",
            ]
            .contains(&file)
                || name.ends_with('/')
                || !kind.starts_with('-')
            {
                bail!("更新アーカイブに不正なファイルまたはリンクがあります");
            }
            binary |= name == BINARY;
        }
    }
    if !binary {
        bail!("更新アーカイブに実行ファイルがありません");
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn test_archive(binary: &[u8]) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(ROOT);
    fs::create_dir(&root).unwrap();
    fs::write(root.join("claudeHistory"), binary).unwrap();
    let output = Command::new("/usr/bin/tar")
        .args(["-czf", "-", "-C"])
        .arg(dir.path())
        .arg(ROOT)
        .output()
        .unwrap();
    assert!(output.status.success());
    output.stdout
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_traversal_duplicate_and_links_before_extraction() {
        validate_listing(&format!("{ROOT}/\n{BINARY}\n"), "drwx\n-rwx\n").unwrap();
        for (names, kinds) in [
            (format!("{BINARY}\n{BINARY}\n"), "-rwx\n-rwx\n"),
            (format!("{ROOT}/../evil\n{BINARY}\n"), "-rwx\n-rwx\n"),
            (format!("{BINARY}\n"), "lrwx\n"),
            (format!("{BINARY}\n"), "hrwx\n"),
            (format!("{ROOT}/README.md\n"), "-rwx\n"),
        ] {
            assert!(validate_listing(&names, kinds).is_err());
        }
    }
}
