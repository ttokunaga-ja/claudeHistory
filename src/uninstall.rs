//! Uninstall the CLI and its default data directory, preserving Claude data.
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self, Write},
    path::Path,
};

fn fingerprint(path: &Path) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        bail!("削除対象が通常ファイルではありません: {}", path.display());
    }
    #[cfg(windows)]
    windows::reject_reparse_point(&meta)?;
    Ok(Sha256::digest(fs::read(path)?).to_vec())
}

mod data;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Remove this CLI and its default owned data after confirmation.
pub fn run() -> Result<()> {
    let executable =
        fs::canonicalize(env::current_exe()?).context("実行中の実行ファイルを特定できません")?;
    let home = fs::canonicalize(dirs::home_dir().context("ホームフォルダを特定できません")?)?;
    run_at(&executable, &home.join(".claude-history"))
}

fn run_at(executable: &Path, app_root: &Path) -> Result<()> {
    let plan = data::prepare(app_root, executable)?;
    let original = fingerprint(executable)?;
    #[cfg(unix)]
    let original_identity = unix::identity(executable)?;
    eprintln!("claudeHistory の削除対象: {}", executable.display());
    eprintln!(
        "claudeHistoryの登録設定とバックアップも削除します。削除後はバックアップによる同期の取り消しができません。Claude本体の履歴とPATH設定は保持します。"
    );
    eprintln!("データフォルダの削除対象: {}", app_root.display());
    eprint!("実行ファイルとclaudeHistoryのデータを削除しますか？ [y/N]: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes" | "はい") {
        eprintln!("キャンセルしました。変更していません。");
        return Ok(());
    }
    if fingerprint(executable)? != original {
        bail!("確認中に実行ファイルが変更されました。削除を中止しました");
    }
    #[cfg(unix)]
    if unix::identity(executable)? != original_identity {
        bail!("確認中に実行ファイルが置き換えられました。削除を中止しました");
    }
    data::remove(plan)?;
    eprintln!(
        "claudeHistoryのデータを削除しました: {}",
        app_root.display()
    );
    #[cfg(windows)]
    {
        windows::schedule(executable, &original)
    }
    #[cfg(not(any(unix, windows)))]
    bail!("このOSではアンインストールに対応していません");
    #[cfg(unix)]
    {
        unix::remove(executable)?;
        eprintln!("実行ファイルを削除しました: {}", executable.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_rejects_non_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        assert!(fingerprint(directory.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn fingerprint_rejects_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("executable");
        let link = directory.path().join("link");
        fs::write(&file, b"executable").unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(fingerprint(&link).is_err());
    }
}
