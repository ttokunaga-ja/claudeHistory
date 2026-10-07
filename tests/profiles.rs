use claude_history::{
    history::Account,
    profiles::{Profile, Profiles, validate_email, validate_label},
};
use std::collections::BTreeMap;
fn account(id: &str, org: &str) -> Account {
    Account {
        account: id.into(),
        org: org.into(),
        sessions: 3,
    }
}
#[test]
fn labels_and_three_account_selection() {
    let mut profiles = Profiles::default();
    for id in ["a", "b", "c"] {
        profiles.accounts.insert(
            id.into(),
            Profile {
                email: format!("{id}@example.com"),
                selected_org: "org".into(),
                organizations: BTreeMap::from([("org".into(), "Personal".into())]),
            },
        );
    }
    let accounts: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|id| account(id, "org"))
        .collect();
    assert_eq!(profiles.selected(&accounts).unwrap().len(), 3);
    let label = profiles.label(&accounts[0], Some("a"));
    assert!(label.contains("a@example.com / Personal / 履歴=3件"));
    assert!(label.contains("Desktop保存情報の現在候補"));
    assert!(!label.contains("account=a"));
    assert!(
        profiles
            .selected(&[account("new", "org"), account("a", "org")])
            .unwrap_err()
            .to_string()
            .contains("configure")
    );
}
#[test]
fn validation_rejects_empty_and_controls() {
    for email in ["", "abc", "a@", "@b", "a@b\n", "a @b", " a@b"] {
        assert!(validate_email(email).is_err(), "{email:?}");
    }
    for name in ["", " ", "name\n", "name\0"] {
        assert!(validate_label(name).is_err());
    }
}
#[test]
fn private_atomic_persistence_and_read_only_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("new/accounts.json");
    assert_eq!(Profiles::load(&path).unwrap(), Profiles::default());
    assert!(!path.exists());
    let profiles = Profiles {
        accounts: BTreeMap::from([(
            "a".into(),
            Profile {
                email: "a@example.com".into(),
                selected_org: "org".into(),
                organizations: BTreeMap::from([("org".into(), "Personal".into())]),
            },
        )]),
    };
    profiles.save(&path).unwrap();
    assert_eq!(Profiles::load(&path).unwrap(), profiles);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[cfg(unix)]
#[test]
fn symlink_file_and_parent_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let target = base.join("real");
    std::fs::create_dir(&target).unwrap();
    let link = base.join("link");
    symlink(&target, &link).unwrap();
    assert!(Profiles::load(&link.join("accounts.json")).is_err());
    assert!(
        Profiles::default()
            .save(&link.join("accounts.json"))
            .is_err()
    );
    let file = target.join("file");
    std::fs::write(&file, b"{}").unwrap();
    let linkfile = base.join("filelink");
    symlink(&file, &linkfile).unwrap();
    assert!(Profiles::load(&linkfile).is_err());
}

#[cfg(unix)]
#[test]
fn saving_settings_preserves_existing_parent_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().canonicalize().unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o750)).unwrap();
    let path = parent.join("accounts.json");
    Profiles::default().save(&path).unwrap();
    Profiles::default().save(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(windows)]
fn parent_acl(parent: &std::path::Path) -> Vec<u8> {
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; (Get-Acl -LiteralPath $env:CLAUDE_HISTORY_PROFILE_TEST_PARENT).Sddl"])
        .env("CLAUDE_HISTORY_PROFILE_TEST_PARENT", parent)
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[cfg(windows)]
#[test]
fn settings_have_current_user_only_acl_and_preserve_existing_parent_acl() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().canonicalize().unwrap();
    let before = parent_acl(&parent);
    let file = parent.join("accounts.json");
    Profiles::default().save(&file).unwrap();
    Profiles::default().save(&file).unwrap();
    assert_eq!(
        parent_acl(&parent),
        before,
        "settings save changed existing parent ACL"
    );
    let new_file = parent.join("dedicated/accounts.json");
    Profiles::default().save(&new_file).unwrap();
    assert_eq!(
        parent_acl(&parent),
        before,
        "creating dedicated settings directory changed its existing parent ACL"
    );
    let verify = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", r#"
$ErrorActionPreference = 'Stop'
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$paths = @($env:CLAUDE_HISTORY_PROFILE_TEST_FILE, $env:CLAUDE_HISTORY_PROFILE_TEST_NEW_FILE, (Split-Path -Parent $env:CLAUDE_HISTORY_PROFILE_TEST_NEW_FILE))
foreach ($path in $paths) {
  $acl = Get-Acl -LiteralPath $path
  $rules = @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
  if (-not $acl.AreAccessRulesProtected -or $rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne $sid -or $rules[0].AccessControlType -ne 'Allow' -or $rules[0].IsInherited) { throw 'Settings ACL is not protected current-user-only' }
}
"#]).env("CLAUDE_HISTORY_PROFILE_TEST_FILE", &file).env("CLAUDE_HISTORY_PROFILE_TEST_NEW_FILE", &new_file).output().unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
}

#[cfg(windows)]
#[test]
fn settings_reject_junction_ancestor_without_writing_target() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let real = base.join("real");
    std::fs::create_dir(&real).unwrap();
    let junction = base.join("junction");
    let result = Command::new("cmd.exe")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&real)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let file = junction.join("accounts.json");
    let load = Profiles::load(&file);
    let save = Profiles::default().save(&file);
    std::fs::remove_dir(&junction).unwrap();
    for result in [load.map(|_| ()), save] {
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("reparse point rejected") || error.contains("symlink rejected"),
            "{error}"
        );
    }
    assert!(!real.join("accounts.json").exists());
}
