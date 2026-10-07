use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

fn copied_executable(root: &std::path::Path) -> std::path::PathBuf {
    let executable = root.join(if cfg!(windows) {
        "claudeHistory.exe"
    } else {
        "claudeHistory"
    });
    fs::copy(env!("CARGO_BIN_EXE_claudeHistory"), &executable).unwrap();
    executable
}

fn preserved_data(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    [
        ".claude/history.json",
        "Library/Application Support/Claude/config.json",
        "Library/Application Support/Claude/claude-code-sessions/session/local.json",
        "custom-profiles.json",
        ".claude-history/accounts.json",
        ".claude-history/backups/test/manifest.json",
        ".profile",
    ]
    .into_iter()
    .map(|name| {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = name.as_bytes().to_vec();
        fs::write(&path, &bytes).unwrap();
        (path, bytes)
    })
    .collect()
}

fn assert_preserved(data: &[(std::path::PathBuf, Vec<u8>)]) {
    for (path, bytes) in data {
        assert_eq!(&fs::read(path).unwrap(), bytes, "{}", path.display());
    }
}

#[test]
fn uninstall_cancellation_preserves_executable_and_neighbor_data() {
    for answer in ["", "\n", "no\n", "はいではない\n"] {
        let directory = tempfile::tempdir().unwrap();
        let executable = copied_executable(directory.path());
        let data = preserved_data(directory.path());
        let mut child = Command::new(&executable)
            .arg("uninstall")
            .env("HOME", directory.path())
            .env("USERPROFILE", directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(answer.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("キャンセル"));
        assert!(executable.is_file());
        assert_preserved(&data);
    }
}

#[cfg(unix)]
#[test]
fn uninstall_confirmation_removes_tool_data_and_preserves_claude() {
    for answer in ["y\n", "YES\n", "はい\n"] {
        let directory = tempfile::tempdir().unwrap();
        let executable = copied_executable(directory.path());
        let data = preserved_data(directory.path());
        let mut child = Command::new(&executable)
            .arg("uninstall")
            .env("HOME", directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(answer.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!executable.exists());
        assert!(!directory.path().join(".claude-history").exists());
        for (path, bytes) in data {
            if !path.starts_with(directory.path().join(".claude-history")) {
                assert_eq!(fs::read(path).unwrap(), bytes);
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn executable_replacement_during_confirmation_is_preserved() {
    use std::io::Read;
    let directory = tempfile::tempdir().unwrap();
    let executable = copied_executable(directory.path());
    let mut child = Command::new(&executable)
        .arg("uninstall")
        .env("HOME", directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut prompt = Vec::new();
    while !prompt.ends_with(b"[y/N]: ") {
        let mut byte = [0];
        assert_eq!(stderr.read(&mut byte).unwrap(), 1);
        prompt.push(byte[0]);
    }
    let replacement = directory.path().join("replacement");
    // Even identical bytes must not allow a file installed by another process
    // during confirmation to be mistaken for the original executable.
    fs::copy(&executable, &replacement).unwrap();
    fs::rename(&replacement, &executable).unwrap();
    child.stdin.take().unwrap().write_all(b"yes\n").unwrap();
    let status = child.wait().unwrap();
    let mut error = String::new();
    stderr.read_to_string(&mut error).unwrap();
    assert!(!status.success());
    assert!(error.contains("置き換え"), "{error}");
    assert!(executable.is_file());
}

#[cfg(unix)]
#[test]
fn linked_data_aborts_uninstall_and_keeps_executable_and_claude() {
    for root_link in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let executable = copied_executable(directory.path());
        let data = preserved_data(directory.path());
        let root = directory.path().join(".claude-history");
        let outside = directory.path().join(".claude");
        let link = if root_link {
            fs::rename(&root, directory.path().join("saved-tool-data")).unwrap();
            root.clone()
        } else {
            root.join("linked-claude-data")
        };
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let mut child = Command::new(&executable)
            .arg("uninstall")
            .env("HOME", directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(b"yes\n");
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("リンク"));
        assert!(executable.exists());
        for (path, bytes) in data {
            let checked_path = if root_link && path.starts_with(&root) {
                directory
                    .path()
                    .join("saved-tool-data")
                    .join(path.strip_prefix(&root).unwrap())
            } else {
                path
            };
            assert_eq!(fs::read(checked_path).unwrap(), bytes);
        }
        fs::remove_file(link).unwrap();
    }
}
