#[cfg(any(windows, test, not(target_os = "macos")))]
use anyhow::bail;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
#[cfg(any(windows, test))]
use std::path::PathBuf;
#[cfg(any(windows, test))]
use tempfile::NamedTempFile;

#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod macos;
#[cfg(windows)]
#[path = "platform/windows.rs"]
mod windows;

pub(super) fn extract(archive: &Path, candidate: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::extract(archive, candidate)
    }
    #[cfg(windows)]
    {
        windows::extract(archive, candidate)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (archive, candidate);
        bail!("Unsupported platform")
    }
}

pub(super) fn replace(exe: &Path, candidate: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let backup = windows_replace(exe, candidate, |from, to| fs::rename(from, to))?;
        if let Err(error) = fs::remove_file(&backup) {
            eprintln!(
                "更新前の実行ファイルを保持しました: {}（終了後に削除できます: {error}）",
                backup.display()
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fs::rename(candidate, exe).with_context(|| format!("置き換えられません: {}", exe.display()))
}

// Also compiled in Unix tests to exercise Windows rollback independently.
#[cfg(any(windows, test))]
pub(super) fn windows_replace(
    exe: &Path,
    candidate: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<PathBuf> {
    let backup = NamedTempFile::new_in(exe.parent().context("親ディレクトリがありません")?)?;
    let backup = backup.into_temp_path();
    // Reserve a unique name, then release it so Windows can rename into it.
    fs::remove_file(&backup)?;
    rename(exe, &backup).context("更新前の実行ファイルを移動できません")?;
    if let Err(error) = rename(candidate, exe) {
        if let Err(rollback) = rename(&backup, exe) {
            let saved = backup.keep()?;
            bail!(
                "置き換えと復元に失敗しました: {error}; {rollback}。更新前の実行ファイル: {}",
                saved.display()
            );
        }
        return Err(error).context("置き換えに失敗しました。現在の版を復元しました");
    }
    Ok(backup.keep()?)
}

#[cfg(test)]
pub(super) fn test_archive(binary: &[u8]) -> Vec<u8> {
    #[cfg(target_os = "macos")]
    {
        macos::test_archive(binary)
    }
    #[cfg(windows)]
    {
        windows::test_archive(binary)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        binary.to_vec()
    }
}
