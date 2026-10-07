//! User supplied labels and explicit organization selection; never authentication.
use crate::{fs_platform, history::Account};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::Path};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profiles {
    pub accounts: BTreeMap<String, Profile>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub email: String,
    pub selected_org: String,
    pub organizations: BTreeMap<String, String>,
}

pub fn validate_label(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value == value.trim() && !value.chars().any(char::is_control),
        "表示名は空白のみ・前後の空白・制御文字を含められません"
    );
    Ok(())
}
pub fn validate_email(value: &str) -> Result<()> {
    validate_label(value)?;
    let parts: Vec<_> = value.split('@').collect();
    ensure!(
        parts.len() == 2
            && !parts[0].is_empty()
            && !parts[1].is_empty()
            && !value.chars().any(char::is_whitespace),
        "メールアドレスを入力してください"
    );
    Ok(())
}
impl Profiles {
    fn validate(&self) -> Result<()> {
        for (id, profile) in &self.accounts {
            validate_label(id)?;
            validate_email(&profile.email)?;
            validate_label(&profile.selected_org)?;
            ensure!(
                profile.organizations.contains_key(&profile.selected_org),
                "選択組織の表示名がありません"
            );
            for (org, name) in &profile.organizations {
                validate_label(org)?;
                validate_label(name)?;
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        reject_ancestors(path)?;
        match fs::read(path) {
            Ok(bytes) => {
                let value: Self =
                    serde_json::from_slice(&bytes).context("アカウント設定JSONが不正です")?;
                value.validate()?;
                Ok(value)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        reject_ancestors(path)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        create_parents(parent)?;
        reject_ancestors(path)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        fs_platform::private_file(file.path())?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        reject_ancestors(path)?;
        fs_platform::publish(file, path, path.exists())?;
        fs_platform::sync_dir(parent)?;
        Ok(())
    }
    pub fn selected(&self, accounts: &[Account]) -> Result<Vec<Account>> {
        let mut selected = Vec::new();
        let mut groups = BTreeMap::new();
        for a in accounts {
            groups.entry(&a.account).or_insert_with(Vec::new).push(a);
        }
        for (id, choices) in groups {
            let profile = self.accounts.get(id).with_context(|| format!("未設定のアカウント {id} があります。configureでメールアドレスと組織を設定してください。"))?;
            let account = choices
                .into_iter()
                .find(|a| a.org == profile.selected_org)
                .with_context(|| {
                    format!(
                        "{}の選択組織が見つかりません。configureで再設定してください。",
                        profile.email
                    )
                })?;
            selected.push(account.clone());
        }
        ensure!(
            selected.len() >= 2,
            "同期には保存済みアカウントが2個以上必要です。"
        );
        Ok(selected)
    }
    pub fn label(&self, account: &Account, current: Option<&str>) -> String {
        let marker = if current == Some(account.account.as_str()) {
            " [Desktop保存情報の現在候補]"
        } else {
            ""
        };
        match self.accounts.get(&account.account) {
            Some(p) => format!(
                "{} / {} / 履歴={}件{}",
                p.email,
                p.organizations
                    .get(&account.org)
                    .map(String::as_str)
                    .unwrap_or("未設定組織"),
                account.sessions,
                marker
            ),
            None => format!(
                "未設定 account={} org={} / 履歴={}件{}",
                account.account, account.org, account.sessions, marker
            ),
        }
    }
}
fn reject_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                fs_platform::reject_redirect(&metadata, ancestor)?;
                if ancestor != path {
                    ensure!(metadata.is_dir(), "設定の親がディレクトリーではありません");
                } else {
                    ensure!(
                        metadata.is_file() || metadata.is_dir(),
                        "設定パスが通常ファイルではありません"
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(metadata.is_file(), "設定パスが通常ファイルではありません");
    }
    Ok(())
}
fn create_parents(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_parents(parent)?;
    }
    match fs::create_dir(path) {
        Ok(()) => fs_platform::private_dir(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!("設定フォルダーが作成中に変わりました")
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
