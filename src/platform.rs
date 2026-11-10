//! OS-dependent application storage defaults, kept outside the command flow.
#[cfg(any(windows, test))]
use anyhow::Context;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn default_data_root(home: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let _ = home;
        let roaming =
            PathBuf::from(std::env::var_os("APPDATA").context("APPDATAを取得できません")?);
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        windows_data_root(&roaming, local.as_deref())
    }
    #[cfg(not(windows))]
    {
        Ok(home.join("Library/Application Support/Claude"))
    }
}

// Take paths explicitly so discovery can be tested without changing process-wide
// environment variables or touching the user's real Claude data.
#[cfg(any(windows, test))]
fn windows_data_root(roaming: &Path, local: Option<&Path>) -> Result<PathBuf> {
    use anyhow::{bail, ensure};
    use std::fs;

    let legacy = roaming.join("Claude");
    let mut candidates = vec![legacy.clone()];
    let modern = local.map(|p| p.join("Claude-Data"));
    if let Some(base) = &modern {
        crate::history::safe_path(base)?;
        if base
            .try_exists()
            .with_context(|| format!("保存先を確認できません: {}", base.display()))?
        {
            for entry in fs::read_dir(base)
                .with_context(|| format!("保存先を列挙できません: {}", base.display()))?
            {
                let entry = entry?;
                // Ignore files such as README.txt; inspect child roots without
                // following symlinks or Windows junctions.
                let path = entry.path();
                crate::history::safe_path(&path)?;
                if entry.file_type()?.is_dir() {
                    candidates.push(path);
                }
            }
        }
    }
    let mut valid = Vec::new();
    for path in candidates {
        crate::history::safe_path(&path)?;
        let config = path.join("config.json");
        let sessions = path.join("claude-code-sessions");
        crate::history::safe_path(&config)?;
        crate::history::safe_path(&sessions)?;
        if config
            .try_exists()
            .with_context(|| format!("設定を確認できません: {}", config.display()))?
            && sessions
                .try_exists()
                .with_context(|| format!("履歴を確認できません: {}", sessions.display()))?
        {
            ensure!(
                config.is_file() && sessions.is_dir(),
                "保存先の形式が不正です: {}",
                path.display()
            );
            valid.push(path);
        }
    }
    valid.sort();
    match valid.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!(
            "Claudeの保存先が見つかりません。確認先: {}{}。config.json と claude-code-sessions を含むフォルダーを --data-dir で指定してください",
            legacy.display(),
            modern
                .map(|p| format!(", {}/*", p.display()))
                .unwrap_or_default()
        ),
        _ => bail!(
            "Claudeの保存先候補が複数あります。--data-dir で指定してください:\n{}",
            valid
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        (temp, base.join("Roaming"), base.join("Local"))
    }
    fn store(path: &Path) {
        fs::create_dir_all(path.join("claude-code-sessions")).unwrap();
        fs::write(path.join("config.json"), b"{}").unwrap();
    }
    #[test]
    fn discovers_legacy_root_without_local_appdata() {
        let (_temp, roaming, _) = fixture();
        let legacy = roaming.join("Claude");
        store(&legacy);
        assert_eq!(windows_data_root(&roaming, None).unwrap(), legacy);
    }
    #[test]
    fn discovers_new_root_and_ignores_readme_and_incomplete_legacy() {
        let (_temp, roaming, local) = fixture();
        let modern = local.join("Claude-Data/package-id");
        store(&modern);
        fs::write(local.join("Claude-Data/README.txt"), b"keep data").unwrap();
        fs::create_dir_all(roaming.join("Claude")).unwrap();
        fs::write(roaming.join("Claude/config.json"), b"{}").unwrap();
        assert_eq!(windows_data_root(&roaming, Some(&local)).unwrap(), modern);
    }
    #[test]
    fn missing_roots_report_search_paths_and_override() {
        let (_temp, roaming, local) = fixture();
        let error = windows_data_root(&roaming, Some(&local))
            .unwrap_err()
            .to_string();
        assert!(error.contains(&roaming.join("Claude").display().to_string()));
        assert!(error.contains(&local.join("Claude-Data").display().to_string()));
        assert!(error.contains("--data-dir"));
        assert!(!roaming.exists() && !local.exists());
    }
    #[test]
    fn legacy_and_modern_are_ambiguous_even_if_legacy_exists() {
        let (_temp, roaming, local) = fixture();
        let legacy = roaming.join("Claude");
        let modern = local.join("Claude-Data/package-id");
        store(&legacy);
        store(&modern);
        let error = windows_data_root(&roaming, Some(&local))
            .unwrap_err()
            .to_string();
        assert!(error.contains("--data-dir"));
        assert!(error.contains(&legacy.display().to_string()));
        assert!(error.contains(&modern.display().to_string()));
    }
    #[test]
    fn multiple_modern_roots_require_explicit_selection() {
        let (_temp, roaming, local) = fixture();
        store(&local.join("Claude-Data/one"));
        store(&local.join("Claude-Data/two"));
        assert!(
            windows_data_root(&roaming, Some(&local))
                .unwrap_err()
                .to_string()
                .contains("複数")
        );
    }
    #[test]
    fn incorrect_storage_types_are_rejected() {
        let (_temp, roaming, local) = fixture();
        let legacy = roaming.join("Claude");
        store(&legacy);
        fs::remove_file(legacy.join("config.json")).unwrap();
        fs::create_dir(legacy.join("config.json")).unwrap();
        assert!(windows_data_root(&roaming, Some(&local)).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn modern_junction_is_rejected() {
        use std::process::Command;
        let (_temp, roaming, local) = fixture();
        let target = local.join("real");
        store(&target);
        fs::create_dir_all(local.join("Claude-Data")).unwrap();
        let junction = local.join("Claude-Data/redirect");
        assert!(
            Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&junction)
                .arg(&target)
                .output()
                .unwrap()
                .status
                .success()
        );
        let error = windows_data_root(&roaming, Some(&local))
            .unwrap_err()
            .to_string();
        fs::remove_dir(junction).unwrap();
        assert!(
            error.contains("reparse point") || error.contains("symlink rejected"),
            "{error}"
        );
        assert!(target.join("config.json").exists());
    }
}
