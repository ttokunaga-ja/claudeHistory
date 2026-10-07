use claude_history::runtime::{Activity, classify_argv_fixture, classify_fixture};

#[test]
fn stopped_and_prompt_mentions_are_not_claude_processes() {
    assert_eq!(
        classify_fixture("10 1 /bin/zsh\n", &[(10, "zsh -c echo claude")], 999).unwrap(),
        Activity::Stopped
    );
    assert_eq!(
        classify_fixture(
            "10 1 /usr/local/bin/node\n",
            &[(
                10,
                "node /tmp/server.js claude /tmp/@anthropic-ai/claude-code/cli.js"
            )],
            999
        )
        .unwrap(),
        Activity::Stopped
    );
}
#[test]
fn cli_waiting_native_and_node_are_busy() {
    for (comm, args) in [
        ("/Users/me/.local/share/claude/versions/2.0.1", "claude"),
        ("/usr/local/bin/claude", "claude"),
        (
            "/usr/local/bin/node",
            "node /usr/local/lib/node_modules/@anthropic-ai/claude-code/cli.js",
        ),
    ] {
        assert!(matches!(
            classify_fixture(&format!("10 1 {comm}\n"), &[(10, args)], 999).unwrap(),
            Activity::Busy { .. }
        ));
    }
}
#[test]
fn desktop_helpers_idle_but_worker_descendants_busy() {
    let app = "10 1 /Applications/Claude.app/Contents/MacOS/Claude\n11 10 /Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper\n";
    assert_eq!(
        classify_fixture(app, &[], 999).unwrap(),
        Activity::IdleDesktop { pids: vec![10, 11] }
    );
    assert!(matches!(
        classify_fixture(
            &format!("{app}12 11 /bin/bash\n13 12 /usr/bin/python3\n"),
            &[],
            999
        )
        .unwrap(),
        Activity::Busy { .. }
    ));
}
#[test]
fn unknown_related_and_malformed_fail_closed_and_exclude_self() {
    assert!(matches!(
        classify_fixture("10 1 /tmp/Claude mystery\n", &[], 999).unwrap(),
        Activity::Unknown { .. }
    ));
    assert!(classify_fixture("bad row", &[], 999).is_err());
    assert_eq!(
        classify_fixture(
            "998 1 /tmp/claudeHistory\n999 1 /tmp/claudeHistory\n",
            &[],
            999
        )
        .unwrap(),
        Activity::Stopped
    );
}

#[test]
fn node_preload_options_do_not_hide_cli_script() {
    assert!(matches!(classify_fixture("10 1 /usr/local/bin/node\n", &[(10,"node --require /tmp/preload.js /usr/local/lib/node_modules/@anthropic-ai/claude-code/cli.js")],999).unwrap(),Activity::Busy{..}));
}

#[test]
fn native_argv_preserves_spaces_and_separate_option_values() {
    let argv: &[&str] = &[
        "node",
        "/Users/Jane Doe/node_modules/@anthropic-ai/claude-code/cli.js",
    ];
    assert!(matches!(
        classify_argv_fixture("10 1 /usr/local/bin/node\n", &[(10, argv)], 999).unwrap(),
        Activity::Busy { .. }
    ));
    assert!(matches!(
        classify_argv_fixture(
            "10 1 /usr/local/bin/node\n",
            &[(
                10,
                &[
                    "node",
                    "--conditions",
                    "development",
                    "/Users/Jane Doe/node_modules/@anthropic-ai/claude-code/cli.js"
                ]
            )],
            999
        )
        .unwrap(),
        Activity::Busy { .. }
    ));
}
