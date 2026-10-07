use serde_json::json;
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let data = t.path().canonicalize().unwrap().join("Claude");
    let account = "00000001-0000-4000-8000-000000000000";
    for i in 1..=2 {
        fs::create_dir_all(
            data.join("claude-code-sessions")
                .join(account)
                .join(format!("{i:08}-0000-4000-8000-000000000000")),
        )
        .unwrap();
    }
    fs::write(
        data.join("config.json"),
        json!({"lastKnownAccountUuid":account}).to_string(),
    )
    .unwrap();
    (t, data)
}

#[test]
fn menu_one_syncs_all_organizations_without_registration_or_confirmation() {
    let (t, data) = fixture();
    let malformed = t.path().join(".claude-history/accounts.json");
    fs::create_dir_all(malformed.parent().unwrap()).unwrap();
    fs::write(&malformed, b"invalid json").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_claudeHistory"))
        .env("HOME", t.path())
        .env("USERPROFILE", t.path())
        .arg("--data-dir")
        .arg(&data)
        .arg("--cli-dir")
        .arg(t.path().join("cli"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"1\n").unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(!output.contains("メールアドレス"));
    assert!(!output.contains("アカウント設定"));
    assert!(!output.contains("Yes/No"));
    assert_eq!(output.matches("追加=0件").count(), 2);
    assert_eq!(fs::read(malformed).unwrap(), b"invalid json");
}

#[test]
fn sync_dry_run_includes_empty_and_populated_organizations_without_settings() {
    let (t, data) = fixture();
    let populated = data.join("claude-code-sessions/00000001-0000-4000-8000-000000000000/00000002-0000-4000-8000-000000000000");
    let session = "local_00000010-0000-4000-8000-000000000000";
    fs::write(
        populated.join(format!("{session}.json")),
        json!({"sessionId":session,"cwd":t.path().join("project")}).to_string(),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_claudeHistory"))
        .env("HOME", t.path())
        .env("USERPROFILE", t.path())
        .arg("--data-dir")
        .arg(&data)
        .arg("--cli-dir")
        .arg(t.path().join("cli"))
        .args(["sync", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("追加=1件"));
    assert!(output.contains("追加=0件"));
    assert!(output.contains("Desktop保存情報の現在候補"));
    assert!(!output.contains("メールアドレス"));
    assert_eq!(fs::read_dir(populated).unwrap().count(), 1);
}

#[test]
fn removed_registration_commands_are_rejected_and_menu_abort_is_clean() {
    for args in [
        vec!["configure"],
        vec!["--profiles-file", "accounts.json", "accounts"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_claudeHistory"))
            .args(args)
            .output()
            .unwrap();
        assert!(!result.status.success());
    }
    let (_t, data) = fixture();
    let mut child = Command::new(env!("CARGO_BIN_EXE_claudeHistory"))
        .arg("--data-dir")
        .arg(data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(error.contains("選択を中止しました"));
    assert!(!error.contains("parse"));
}

#[test]
fn maintenance_commands_are_available_without_loading_claude_data() {
    let t = tempfile::tempdir().unwrap();
    let malformed = t.path().join(".claude-history/accounts.json");
    fs::create_dir_all(malformed.parent().unwrap()).unwrap();
    fs::write(&malformed, b"invalid json").unwrap();
    for command in ["update", "uninstall"] {
        let result = Command::new(env!("CARGO_BIN_EXE_claudeHistory"))
            .arg("--data-dir")
            .arg(t.path().join("missing"))
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains(command));
    }
    let result = Command::new(env!("CARGO_BIN_EXE_claudeHistory"))
        .arg("--data-dir")
        .arg(t.path().join("missing"))
        .arg("uninstall")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(&malformed).unwrap(), b"invalid json");
}
