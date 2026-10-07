//! OS-dependent application storage defaults, kept outside the command flow.
#[cfg(windows)]
use anyhow::Context;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn default_data_root(home: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let _ = home;
        Ok(
            PathBuf::from(std::env::var_os("APPDATA").context("APPDATAを取得できません")?)
                .join("Claude"),
        )
    }
    #[cfg(not(windows))]
    {
        Ok(home.join("Library/Application Support/Claude"))
    }
}
