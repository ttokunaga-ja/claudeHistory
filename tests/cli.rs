use serde_json::json;
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};
#[test]
fn three_account_labels_dry_run_and_clean_menu_abort() {
    let t = tempfile::tempdir().unwrap();
    let base = t.path().canonicalize().unwrap();
    let data = base.join("Claude");
    let profiles = base.join("accounts.json");
    let org = "00000000-0000-4000-8000-000000000000";
    let mut settings = serde_json::Map::new();
    for i in 1..=3 {
        let a = format!("{i:08}-0000-4000-8000-000000000000");
        let p = data.join("claude-code-sessions").join(&a).join(org);
        fs::create_dir_all(&p).unwrap();
        let session = format!("local_{:08}-0000-4000-8000-000000000000", i + 10);
        fs::write(
            p.join(format!("{session}.json")),
            json!({"sessionId":session,"cwd":base.join("project")}).to_string(),
        )
        .unwrap();
        settings.insert(a.clone(),json!({"email":format!("user{i}@example.com"),"selected_org":org,"organizations":{org:"Personal"}}));
    }
    fs::write(
        data.join("config.json"),
        json!({"lastKnownAccountUuid":"00000002-0000-4000-8000-000000000000"}).to_string(),
    )
    .unwrap();
    fs::write(&profiles, json!({"accounts":settings}).to_string()).unwrap();
    let original = fs::read(&profiles).unwrap();
    let run = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_claudeHistory"));
        c.arg("--data-dir")
            .arg(&data)
            .arg("--cli-dir")
            .arg(base.join("cli"))
            .arg("--profiles-file")
            .arg(&profiles)
            .args(args);
        c
    };
    let result = run(&["sync", "--dry-run"]).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    for i in 1..=3 {
        assert!(output.contains(&format!("user{i}@example.com / Personal")));
    }
    assert!(output.contains("追加=2件"));
    assert!(output.contains("Desktop保存情報の現在候補"));
    assert_eq!(fs::read(&profiles).unwrap(), original);
    for i in 1..=3 {
        assert_eq!(
            fs::read_dir(
                data.join("claude-code-sessions")
                    .join(format!("{i:08}-0000-4000-8000-000000000000"))
                    .join(org)
            )
            .unwrap()
            .count(),
            1
        );
    }
    let mut child = run(&[])
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
