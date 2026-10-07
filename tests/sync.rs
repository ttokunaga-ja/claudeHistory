use claude_history::history::{Account, Store};
use serde_json::json;
use std::{fs, path::PathBuf};
const ORG: &str = "00000000-0000-4000-8000-000000000000";
fn fixture() -> (tempfile::TempDir, Store, Vec<Account>) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let store = Store {
        root: root.join("Claude"),
        backup_root: root.join("backup"),
    };
    let accounts: Vec<_> = (1..=3)
        .map(|i| Account {
            account: format!("{i:08}-0000-4000-8000-000000000000"),
            org: ORG.into(),
            sessions: 1,
        })
        .collect();
    for (i, account) in accounts.iter().enumerate() {
        let dir = dir(&store, account);
        fs::create_dir_all(&dir).unwrap();
        let name = name(i);
        fs::write(dir.join(&name), serde_json::to_vec(&json!({"sessionId":name.trim_end_matches(".json"),"cwd":root.join(format!("project{i}")),"auth":"secret","permissionMode":"bypassPermissions"})).unwrap()).unwrap();
    }
    (temp, store, accounts)
}
fn dir(store: &Store, account: &Account) -> PathBuf {
    store
        .root
        .join("claude-code-sessions")
        .join(&account.account)
        .join(&account.org)
}
fn name(i: usize) -> String {
    format!("local_{:08}-0000-4000-8000-000000000000.json", i + 10)
}
#[test]
fn three_account_union_repeat_and_single_undo() {
    let (_temp, store, accounts) = fixture();
    let plan = store.plan_sync(&accounts).unwrap();
    assert!(
        plan.targets
            .iter()
            .all(|t| t.additions == 2 && t.existing == 1)
    );
    let backup = store.apply_sync(&plan).unwrap();
    for a in &accounts {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 3);
    }
    assert!(
        store
            .plan_sync(&accounts)
            .unwrap()
            .targets
            .iter()
            .all(|t| t.additions == 0)
    );
    assert_eq!(store.undo(&backup).unwrap(), 6);
    for a in &accounts {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 1);
    }
}
#[test]
fn global_collision_and_duplicate_account_abort() {
    let (_temp, store, accounts) = fixture();
    fs::copy(
        dir(&store, &accounts[0]).join(name(0)),
        dir(&store, &accounts[2]).join(name(0)),
    )
    .unwrap();
    let path = dir(&store, &accounts[2]).join(name(0));
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["cwd"] = json!(store.root.join("other"));
    fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(store.plan_sync(&accounts).is_err());
    assert!(
        store
            .plan_sync(&[accounts[0].clone(), accounts[0].clone()])
            .is_err()
    );
    assert!(!store.backup_root.exists());
}
#[test]
fn failed_sync_rolls_back_all_targets_and_changed_undo_preflights_all() {
    let (_temp, store, accounts) = fixture();
    let mut calls = 0;
    assert!(
        store
            .apply_sync_checked(&store.plan_sync(&accounts).unwrap(), || {
                calls += 1;
                if calls == 6 {
                    anyhow::bail!("activity resumed")
                }
                Ok(())
            })
            .is_err()
    );
    for a in &accounts {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 1);
    }
    let backup = store
        .apply_sync(&store.plan_sync(&accounts).unwrap())
        .unwrap();
    fs::write(dir(&store, &accounts[2]).join(name(0)), "new work").unwrap();
    assert!(store.undo(&backup).is_err());
    for a in &accounts {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 3);
    }
}
#[test]
fn undo_restores_prior_deletions_after_guard_failure() {
    let (_temp, store, accounts) = fixture();
    let backup = store
        .apply_sync(&store.plan_sync(&accounts).unwrap())
        .unwrap();
    let mut calls = 0;
    assert!(
        store
            .undo_checked(&backup, || {
                calls += 1;
                if calls == 3 {
                    anyhow::bail!("busy")
                }
                Ok(())
            })
            .is_err()
    );
    for a in &accounts {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 3);
    }
}
#[test]
fn sync_preserves_concurrent_registration_and_frozen_backup_tampering_aborts() {
    let (_temp, store, accounts) = fixture();
    let plan = store.plan_sync(&accounts).unwrap();
    let dest = dir(&store, &accounts[0]).join(name(1));
    let mut calls = 0;
    assert!(
        store
            .apply_sync_checked(&plan, || {
                calls += 1;
                if calls == 2 {
                    fs::write(&dest, "new concurrent work")?;
                }
                Ok(())
            })
            .is_err()
    );
    assert_eq!(fs::read_to_string(&dest).unwrap(), "new concurrent work");
    fs::remove_file(dest).unwrap();
    let plan = store.plan_sync(&accounts).unwrap();
    let mut calls = 0;
    assert!(
        store
            .apply_sync_checked(&plan, || {
                calls += 1;
                if calls == 3 {
                    let backup = fs::read_dir(&store.backup_root)?
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .find(|p| {
                            let v: serde_json::Value =
                                serde_json::from_slice(&fs::read(p.join("manifest.json")).unwrap())
                                    .unwrap();
                            v["created"].as_object().unwrap().len() == 1
                        })
                        .unwrap();
                    fs::write(
                        backup
                            .join("originals")
                            .join(&accounts[1].account)
                            .join(ORG)
                            .join(name(1)),
                        "tampered",
                    )?;
                }
                Ok(())
            })
            .is_err()
    );
    for a in &accounts {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 1);
    }
}
#[test]
fn modified_publication_retained_while_other_own_writes_rollback() {
    let (_temp, store, accounts) = fixture();
    let mut calls = 0;
    assert!(
        store
            .apply_sync_checked(&store.plan_sync(&accounts).unwrap(), || {
                calls += 1;
                if calls == 4 {
                    fs::write(
                        dir(&store, &accounts[0]).join(name(1)),
                        "user changed addition",
                    )?;
                    anyhow::bail!("busy");
                }
                Ok(())
            })
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(dir(&store, &accounts[0]).join(name(1))).unwrap(),
        "user changed addition"
    );
    assert!(!dir(&store, &accounts[0]).join(name(2)).exists());
    for a in &accounts[1..] {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 1);
    }
}
#[test]
fn deterministic_duplicate_source_and_public_plan_drift_rejected() {
    let (_temp, store, accounts) = fixture();
    let original = dir(&store, &accounts[0]).join(name(0));
    let duplicate = dir(&store, &accounts[1]).join(name(0));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(original).unwrap()).unwrap();
    value["title"] = json!("alternate");
    fs::write(duplicate, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut reversed = accounts.clone();
    reversed.reverse();
    let mut plan = store.plan_sync(&reversed).unwrap();
    plan.targets[0].additions += 1;
    assert!(store.apply_sync(&plan).is_err());
    assert!(!store.backup_root.exists());
    store
        .apply_sync(&store.plan_sync(&reversed).unwrap())
        .unwrap();
    let copied: serde_json::Value =
        serde_json::from_slice(&fs::read(dir(&store, &accounts[2]).join(name(0))).unwrap())
            .unwrap();
    assert!(copied.get("title").is_none());
    assert!(copied.get("auth").is_none());
    assert_eq!(copied["permissionMode"], "default");
}
#[test]
fn undo_tampered_original_aborts_before_any_removal() {
    let (_temp, store, accounts) = fixture();
    let backup = store
        .apply_sync(&store.plan_sync(&accounts).unwrap())
        .unwrap();
    fs::write(
        backup
            .join("originals")
            .join(&accounts[2].account)
            .join(ORG)
            .join(name(2)),
        "tampered",
    )
    .unwrap();
    assert!(store.undo(&backup).is_err());
    for a in &accounts {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 3);
    }
}
#[test]
fn malformed_manifest_fails_closed_without_panic_or_deletion() {
    let (_temp, store, accounts) = fixture();
    let backup = store
        .apply_sync(&store.plan_sync(&accounts).unwrap())
        .unwrap();
    let path = backup.join("manifest.json");
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let writes = m["writes"].as_object_mut().unwrap();
    let key = writes.keys().next().unwrap().clone();
    let value = writes.remove(&key).unwrap();
    writes.insert("bad-key".into(), value);
    fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
    let result = std::panic::catch_unwind(|| store.undo(&backup));
    assert!(result.is_ok(), "untrusted journal must never panic");
    assert!(result.unwrap().is_err());
    for a in &accounts {
        assert_eq!(fs::read_dir(dir(&store, a)).unwrap().count(), 3);
    }
}

#[test]
fn discovered_account_without_organization_fails_closed() {
    let (_temp, store, _accounts) = fixture();
    let empty = store
        .root
        .join("claude-code-sessions")
        .join("00000004-0000-4000-8000-000000000000");
    fs::create_dir_all(&empty).unwrap();
    let error = store.accounts().unwrap_err().to_string();
    assert!(error.contains("organization missing"), "{error}");
    assert!(
        error.contains("00000004-0000-4000-8000-000000000000"),
        "{error}"
    );
    assert!(!store.backup_root.exists());
}
