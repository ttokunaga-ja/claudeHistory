use claude_history::runtime::{
    Activity, classify_windows_fixture, windows_argv_fixture, windows_tcp_fixture,
};

fn process(pid: i32, parent: i32, path: &str, command: &str) -> serde_json::Value {
    serde_json::json!({"pid":pid,"ppid":parent,"path":path,"name":path.rsplit('\\').next().unwrap(),"command":command})
}
fn classify(rows: Vec<serde_json::Value>) -> Activity {
    classify_windows_fixture(
        &serde_json::to_string(&rows).unwrap(),
        r"C:\Users\Jane Doe\AppData\Local",
        999,
    )
    .unwrap()
}
#[test]
fn official_desktop_locations_and_helpers_are_idle() {
    for path in [
        r"C:\Users\Jane Doe\AppData\Local\AnthropicClaude\app-1.2\Claude.exe",
        r"C:\Users\Jane Doe\AppData\Local\Programs\Claude\Claude.exe",
        r"C:\Program Files\WindowsApps\Anthropic.Claude_1.2_x64__pzs8sxrjxfjjc\Claude.exe",
        r"C:\Program Files\WindowsApps\Claude_1.2_x64__pzs8sxrjxfjjc\app\Claude.exe",
    ] {
        assert_eq!(
            classify(vec![
                process(10, 1, path, "\"Claude.exe\""),
                process(11, 10, path, "Claude.exe --type=renderer")
            ]),
            Activity::IdleDesktop { pids: vec![10, 11] }
        );
    }
}
#[test]
fn native_cli_and_desktop_workers_are_busy() {
    assert!(matches!(
        classify(vec![process(
            10,
            1,
            r"C:\Users\me\.local\bin\claude.exe",
            "claude.exe"
        )]),
        Activity::Busy { .. }
    ));
    let app = process(
        10,
        1,
        r"C:\Users\Jane Doe\AppData\Local\Programs\Claude\Claude.exe",
        "Claude.exe",
    );
    assert!(matches!(
        classify(vec![
            app,
            process(11, 10, r"C:\Windows\System32\cmd.exe", "cmd.exe")
        ]),
        Activity::Busy { .. }
    ));
}
#[test]
fn windows_node_argv_preserves_quoted_paths_and_option_operands() {
    assert!(matches!(
        classify(vec![process(
            10,
            1,
            r"C:\Program Files\nodejs\node.exe",
            r#""C:\Program Files\nodejs\node.exe" --require "C:\Jane Doe\preload.js" "C:\Jane Doe\node_modules\@anthropic-ai\claude-code\cli.js""#
        )]),
        Activity::Busy { .. }
    ));
    assert_eq!(
        classify(vec![process(
            10,
            1,
            r"C:\node.exe",
            r#"node.exe "C:\server.js" "C:\node_modules\@anthropic-ai\claude-code\cli.js""#
        )]),
        Activity::Stopped
    );
    assert_eq!(
        windows_argv_fixture(r#"node.exe "a b" "a\"b" "C:\path\\""#).unwrap(),
        vec!["node.exe", "a b", "a\"b", r"C:\path\"]
    );
}
#[test]
fn inaccessible_observation_unknown_and_self_excluded() {
    assert!(matches!(
        classify(vec![
            serde_json::json!({"pid":10,"ppid":1,"path":null,"name":"node.exe","command":null})
        ]),
        Activity::Unknown { .. }
    ));
    assert!(matches!(
        classify(vec![process(10, 1, r"C:\tmp\Claude Helper.exe", "")]),
        Activity::Unknown { .. }
    ));
    assert_eq!(
        classify(vec![process(999, 1, r"C:\claudeHistory.exe", "")]),
        Activity::Stopped
    );
    assert!(classify_windows_fixture("bad", r"C:\Users\me\AppData\Local", 999).is_err());
    assert!(windows_argv_fixture("node.exe \"unfinished").is_err());
}
#[test]
fn network_ownership_and_unknown_states_fail_closed() {
    assert!(!windows_tcp_fixture("[]", &[10]).unwrap());
    assert!(
        !windows_tcp_fixture(
            r#"[{"pid":10,"state":"Listen"},{"pid":20,"state":"Established"}]"#,
            &[10]
        )
        .unwrap()
    );
    for state in ["Established", "SynSent", "CloseWait", "mystery"] {
        assert!(
            windows_tcp_fixture(&format!(r#"[{{"pid":10,"state":"{state}"}}]"#), &[10]).unwrap()
        );
    }
    assert!(windows_tcp_fixture("bad", &[10]).is_err());
    assert!(windows_tcp_fixture(r#"[{"state":"Listen"}]"#, &[10]).is_err());
}

#[test]
fn bundled_native_cli_is_not_an_electron_helper() {
    assert!(matches!(
        classify(vec![process(
            10,
            1,
            r"C:\Users\Jane Doe\AppData\Local\Programs\Claude\resources\claude-code\claude.exe",
            "claude.exe"
        )]),
        Activity::Busy { .. }
    ));
}
