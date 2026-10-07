use super::*;

pub(super) fn identity(path: &Path) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}

pub(super) fn remove(executable: &Path) -> Result<()> {
    fs::remove_file(executable).context("実行ファイルを削除できません")
}
