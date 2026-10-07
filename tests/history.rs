use claude_history::history::{Account, Store};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
const O: &str = "33333333-3333-4333-8333-333333333333";
const S: &str = "local_44444444-4444-4444-8444-444444444444";
fn fixture() -> (tempfile::TempDir, Store, Account, Account, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let base = t.path().canonicalize().unwrap();
    let store = Store {
        root: base.join("Claude"),
        backup_root: base.join("backup"),
    };
    let p = store.root.join("claude-code-sessions").join(A).join(O);
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join(format!("{S}.json")), serde_json::to_vec(&json!({"sessionId":S,"cwd":"/tmp/project","title":"Example","permissionMode":"bypassPermissions","permissionGrants":["all"],"auth":"secret","mcpServers":{"x":{}},"unknown":"discard"})).unwrap()).unwrap();
    let source = Account {
        account: A.into(),
        org: O.into(),
        sessions: 1,
    };
    let target = Account {
        account: B.into(),
        org: O.into(),
        sessions: 0,
    };
    let dest = store
        .root
        .join("claude-code-sessions")
        .join(B)
        .join(O)
        .join(format!("{S}.json"));
    (t, store, source, target, dest)
}
#[test]
fn missing_only_safe_copy_backup_and_undo() {
    let (_t, store, source, target, dest) = fixture();
    let plan = store.plan(&source, &target).unwrap();
    assert_eq!(plan.additions.len(), 1);
    let backup = store.apply(&plan).unwrap();
    let value: Value = serde_json::from_slice(&fs::read(&dest).unwrap()).unwrap();
    assert_eq!(value["permissionMode"], "default");
    assert!(value.get("auth").is_none());
    assert!(value.get("mcpServers").is_none());
    assert!(value.get("unknown").is_none());
    assert_eq!(store.plan(&source, &target).unwrap().additions.len(), 0);
    assert!(backup.join("manifest.json").is_file());
    assert_eq!(store.undo(&backup).unwrap(), 1);
    assert!(!dest.exists());
    assert!(
        store
            .root
            .join("claude-code-sessions")
            .join(A)
            .join(O)
            .join(format!("{S}.json"))
            .exists()
    );
}
#[test]
fn drift_and_changed_undo_abort() {
    let (_t, store, source, target, dest) = fixture();
    let plan = store.plan(&source, &target).unwrap();
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    fs::write(&dest, "{}").unwrap();
    assert!(store.apply(&plan).is_err());
    fs::remove_file(&dest).unwrap();
    let backup = store.apply(&store.plan(&source, &target).unwrap()).unwrap();
    fs::write(&dest, "user work").unwrap();
    assert!(store.undo(&backup).is_err());
    assert_eq!(fs::read_to_string(dest).unwrap(), "user work");
}

#[test]
fn activity_recheck_aborts_publication_and_undo() {
    let (_t, store, source, target, dest) = fixture();
    let plan = store.plan(&source, &target).unwrap();
    assert!(
        store
            .apply_checked(&plan, || anyhow::bail!("work started"))
            .is_err()
    );
    assert!(!dest.exists());
    let backup = store.apply(&plan).unwrap();
    assert!(
        store
            .undo_checked(&backup, || anyhow::bail!("work started"))
            .is_err()
    );
    assert!(dest.exists());
}

#[test]
fn activity_after_publication_keeps_journal_and_rolls_back_only_own_files() {
    let (_t, store, source, target, dest) = fixture();
    let mut checks = 0;
    assert!(
        store
            .apply_checked(&store.plan(&source, &target).unwrap(), || {
                checks += 1;
                if checks >= 3 {
                    anyhow::bail!("work resumed");
                }
                Ok(())
            })
            .is_err()
    );
    assert!(!dest.exists());
    let backups: Vec<_> = fs::read_dir(&store.backup_root).unwrap().collect();
    assert_eq!(backups.len(), 1);
    let manifest: Value = serde_json::from_slice(
        &fs::read(backups[0].as_ref().unwrap().path().join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["status"], "incomplete");
    assert_eq!(manifest["rollback"][format!("{S}.json")], "removed");
}
#[test]
fn conflict_malformed_and_symlink_rejected() {
    let (_t, store, source, target, dest) = fixture();
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    fs::write(
        &dest,
        serde_json::to_vec(&json!({"sessionId":S,"cwd":"/different"})).unwrap(),
    )
    .unwrap();
    assert!(store.plan(&source, &target).is_err());
    fs::remove_file(&dest).unwrap();
    std::os::unix::fs::symlink("/tmp", dest.parent().unwrap().join("bad")).unwrap();
    assert!(store.plan(&source, &target).is_err());
    fs::remove_file(dest.parent().unwrap().join("bad")).unwrap();
    fs::write(
        store
            .root
            .join("claude-code-sessions")
            .join(A)
            .join(O)
            .join(format!("{S}.json")),
        "{",
    )
    .unwrap();
    assert!(store.plan(&source, &target).is_err());
}
#[test]
fn rejects_relative_origin_and_preserves_existing_registrations() {
    let (_t, store, source, target, dest) = fixture();
    let src = store
        .root
        .join("claude-code-sessions")
        .join(A)
        .join(O)
        .join(format!("{S}.json"));
    let mut value: Value = serde_json::from_slice(&fs::read(&src).unwrap()).unwrap();
    value["originCwd"] = json!("../outside");
    fs::write(&src, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(store.plan(&source, &target).is_err());
    value["originCwd"] = json!("/tmp/project");
    fs::write(&src, serde_json::to_vec(&value).unwrap()).unwrap();
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let existing = serde_json::to_vec(&value).unwrap();
    fs::write(&dest, &existing).unwrap();
    let plan = store.plan(&source, &target).unwrap();
    assert!(plan.additions.is_empty());
    store.apply(&plan).unwrap();
    assert_eq!(fs::read(dest).unwrap(), existing);
}
#[test]
fn ordinary_operational_files_ignored_and_actual_permissions_reset() {
    let (_t, store, source, target, dest) = fixture();
    let dir = store.root.join("claude-code-sessions").join(A).join(O);
    fs::write(dir.join("scheduled-tasks.json"), "not session JSON").unwrap();
    fs::write(dir.join("cache.idx"), "cache").unwrap();
    let p = dir.join(format!("{S}.json"));
    let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    v["isArchived"] = json!(true);
    v["lastFocusedAt"] = json!(42);
    for key in [
        "remoteMcpServersConfig",
        "alwaysAllowedReasons",
        "sessionPermissionUpdates",
    ] {
        v[key] = json!([{"grant":"all"}]);
    }
    fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(store.accounts().unwrap()[0].sessions, 1);
    store.apply(&store.plan(&source, &target).unwrap()).unwrap();
    let copied: Value = serde_json::from_slice(&fs::read(dest).unwrap()).unwrap();
    assert_eq!(copied["isArchived"], true);
    assert_eq!(copied["lastFocusedAt"], 42);
    for key in [
        "remoteMcpServersConfig",
        "alwaysAllowedReasons",
        "sessionPermissionUpdates",
    ] {
        assert_eq!(copied[key], json!([]));
    }
    assert!(copied.get("permissionGrants").is_none());
}
#[test]
fn mutation_lock_blocks_apply_and_undo_without_writes() {
    use std::os::unix::io::AsRawFd;
    let (_t, store, source, target, dest) = fixture();
    let plan = store.plan(&source, &target).unwrap();
    let directory = fs::File::open(store.root.join("claude-code-sessions")).unwrap();
    assert_eq!(
        unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(store.apply(&plan).is_err());
    assert!(!dest.exists());
    assert_eq!(
        unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_UN) },
        0
    );
    let backup = store.apply(&plan).unwrap();
    assert_eq!(
        unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(store.undo(&backup).is_err());
    assert!(dest.exists());
}
#[test]
fn undo_checks_all_pointers_and_original_backup_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, store, source, target, dest) = fixture();
    let second = "local_55555555-5555-4555-8555-555555555555.json";
    let src = store.root.join("claude-code-sessions").join(A).join(O);
    fs::write(
        src.join(second),
        serde_json::to_vec(
            &json!({"sessionId":second.trim_end_matches(".json"),"cwd":"/tmp/second"}),
        )
        .unwrap(),
    )
    .unwrap();
    let backup = store.apply(&store.plan(&source, &target).unwrap()).unwrap();
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(backup.join("originals").join(second))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let second_dest = dest.parent().unwrap().join(second);
    fs::write(&second_dest, "new work").unwrap();
    assert!(store.undo(&backup).is_err());
    assert!(dest.exists());
    assert_eq!(fs::read_to_string(second_dest).unwrap(), "new work");
}
#[test]
fn failed_publication_retains_incomplete_recovery_journal() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, store, source, target, dest) = fixture();
    let dir = dest.parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    let plan = store.plan(&source, &target).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o500)).unwrap();
    let result = store.apply(&plan);
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    let backup = fs::read_dir(&store.backup_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let m: Value =
        serde_json::from_slice(&fs::read(backup.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["status"], "incomplete");
    assert!(m["planned"].get(format!("{S}.json")).is_some());
    assert!(m["source_hashes"].get(format!("{S}.json")).is_some());
    assert!(!dest.exists());
    assert!(
        store
            .undo(&backup)
            .unwrap_err()
            .to_string()
            .contains("incomplete")
    );
}
