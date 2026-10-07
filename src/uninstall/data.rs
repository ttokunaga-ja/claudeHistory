//! Bounded cleanup of claudeHistory's default data directory.
use super::*;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Directory(Vec<u64>),
    File(Vec<u64>, Vec<u8>),
}

pub(super) struct Plan {
    root: PathBuf,
    entries: BTreeMap<PathBuf, Entry>,
}

fn metadata_identity(metadata: &fs::Metadata) -> Vec<u64> {
    #[cfg(unix)]
    {
        unix::metadata_identity(metadata)
    }
    #[cfg(windows)]
    {
        windows::metadata_identity(metadata)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        Vec::new()
    }
}

fn checked_metadata(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        bail!(
            "リンクを含むデータフォルダは削除できません: {}",
            path.display()
        );
    }
    #[cfg(windows)]
    windows::reject_reparse_point(&metadata)?;
    if !metadata.is_file() && !metadata.is_dir() {
        bail!(
            "通常ファイル・フォルダ以外は削除できません: {}",
            path.display()
        );
    }
    Ok(metadata)
}

fn collect(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) -> Result<()> {
    let metadata = checked_metadata(path)?;
    let identity = metadata_identity(&metadata);
    let relative = path.strip_prefix(root)?.to_path_buf();
    if metadata.is_file() {
        entries.insert(relative, Entry::File(identity, fingerprint(path)?));
    } else {
        entries.insert(relative, Entry::Directory(identity.clone()));
        for entry in fs::read_dir(path)? {
            collect(root, &entry?.path(), entries)?;
        }
        let after = checked_metadata(path)?;
        if !after.is_dir() || metadata_identity(&after) != identity {
            bail!("確認中にデータフォルダが変更されました: {}", path.display());
        }
    }
    Ok(())
}

fn snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Entry>> {
    let mut entries = BTreeMap::new();
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(entries),
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    if !checked_metadata(root)?.is_dir() {
        bail!("データ保存先がフォルダではありません: {}", root.display());
    }
    collect(root, root, &mut entries)?;
    Ok(entries)
}

pub(super) fn prepare(root: &Path, executable: &Path) -> Result<Plan> {
    if root.file_name() != Some(std::ffi::OsStr::new(".claude-history")) {
        bail!("claudeHistoryの既定データフォルダ以外は削除できません");
    }
    if executable.starts_with(root) {
        bail!("実行ファイルがデータフォルダ内にあるため削除を中止しました");
    }
    Ok(Plan {
        root: root.to_path_buf(),
        entries: snapshot(root)?,
    })
}

fn validate_entry(path: &Path, expected: &Entry) -> Result<()> {
    let metadata = checked_metadata(path)?;
    let actual = if metadata.is_dir() {
        Entry::Directory(metadata_identity(&metadata))
    } else {
        Entry::File(metadata_identity(&metadata), fingerprint(path)?)
    };
    if &actual != expected {
        bail!("削除中にデータが変更されました: {}", path.display());
    }
    Ok(())
}

fn validate_ancestors(plan: &Plan, relative: &Path) -> Result<()> {
    let mut ancestors = Vec::new();
    let mut ancestor = relative.parent();
    while let Some(path) = ancestor {
        ancestors.push(path);
        ancestor = path.parent();
    }
    for path in ancestors.into_iter().rev() {
        let expected = plan
            .entries
            .get(path)
            .context("親フォルダの記録がありません")?;
        if !matches!(expected, Entry::Directory(_)) {
            bail!("親フォルダが不正です");
        }
        validate_entry(&plan.root.join(path), expected)?;
    }
    Ok(())
}

pub(super) fn remove(mut plan: Plan) -> Result<()> {
    let mut paths: Vec<_> = plan.entries.keys().cloned().collect();
    paths.sort_by(|a, b| {
        b.components()
            .count()
            .cmp(&a.components().count())
            .then_with(|| a.cmp(b))
    });
    // Recheck the full snapshot once before any deletion. Per-path checks then
    // keep total content reads linear; empty-directory removal rejects additions.
    if snapshot(&plan.root)? != plan.entries {
        bail!("確認中にデータが変更されました。削除を中止しました");
    }
    for relative in paths {
        validate_ancestors(&plan, &relative)?;
        let path = plan.root.join(&relative);
        validate_entry(
            &path,
            plan.entries
                .get(&relative)
                .context("削除対象の記録がありません")?,
        )?;
        match plan
            .entries
            .get(&relative)
            .context("削除対象の記録がありません")?
        {
            Entry::Directory(_) => fs::remove_dir(&path)?,
            Entry::File(_, _) => fs::remove_file(&path)?,
        }
        plan.entries.remove(&relative);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(directory.path()).unwrap();
        let root = base.join(".claude-history");
        fs::create_dir_all(root.join("backups/test")).unwrap();
        fs::write(root.join("accounts.json"), b"settings").unwrap();
        fs::write(root.join("backups/test/manifest.json"), b"backup").unwrap();
        let executable = base.join("cli");
        fs::write(&executable, b"binary").unwrap();
        (directory, root, executable)
    }

    #[test]
    fn cleanup_only_removes_owned_root() {
        let (_directory, root, executable) = fixture();
        let claude = root.parent().unwrap().join(".claude");
        fs::create_dir(&claude).unwrap();
        fs::write(claude.join("history.json"), b"Claude").unwrap();
        let plan = prepare(&root, &executable).unwrap();
        remove(plan).unwrap();
        assert!(!root.exists());
        assert_eq!(fs::read(claude.join("history.json")).unwrap(), b"Claude");
        assert_eq!(fs::read(executable).unwrap(), b"binary");
    }

    #[test]
    fn changed_data_aborts_before_any_deletion() {
        let (_directory, root, executable) = fixture();
        let plan = prepare(&root, &executable).unwrap();
        fs::write(root.join("accounts.json"), b"new settings").unwrap();
        assert!(remove(plan).is_err());
        assert_eq!(
            fs::read(root.join("accounts.json")).unwrap(),
            b"new settings"
        );
        assert_eq!(
            fs::read(root.join("backups/test/manifest.json")).unwrap(),
            b"backup"
        );
        assert!(executable.exists());
    }

    #[test]
    fn absent_root_does_not_allow_new_data_to_be_deleted() {
        let (_directory, root, executable) = fixture();
        remove(prepare(&root, &executable).unwrap()).unwrap();
        let plan = prepare(&root, &executable).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("new"), b"new").unwrap();
        assert!(remove(plan).is_err());
        assert_eq!(fs::read(root.join("new")).unwrap(), b"new");
        remove(prepare(&root, &executable).unwrap()).unwrap();
        remove(prepare(&root, &executable).unwrap()).unwrap();
    }

    #[test]
    fn arbitrary_root_and_executable_inside_root_are_rejected() {
        let (_directory, root, executable) = fixture();
        assert!(prepare(root.parent().unwrap(), &executable).is_err());
        assert!(prepare(&root, &root.join("cli")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn root_and_descendant_links_are_rejected_without_touching_target() {
        let (_directory, root, executable) = fixture();
        let outside = root.parent().unwrap().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("history"), b"outside").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(prepare(&root, &executable).is_err());
        fs::remove_file(&link).unwrap();
        let plan = prepare(&root, &executable).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(remove(plan).is_err());
        assert!(root.join("accounts.json").exists());
        fs::remove_file(&link).unwrap();
        remove(prepare(&root, &executable).unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &root).unwrap();
        assert!(prepare(&root, &executable).is_err());
        assert_eq!(fs::read(outside.join("history")).unwrap(), b"outside");
        fs::remove_file(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn junction_is_rejected_and_outside_data_is_preserved() {
        let (_directory, root, executable) = fixture();
        let outside = root.parent().unwrap().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("history"), b"outside").unwrap();
        let link = root.join("junction");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(link.to_string_lossy().trim_start_matches(r"\\?\"))
            .arg(outside.to_string_lossy().trim_start_matches(r"\\?\"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(prepare(&root, &executable).is_err());
        assert_eq!(fs::read(outside.join("history")).unwrap(), b"outside");
        fs::remove_dir(link).unwrap();
    }
}
