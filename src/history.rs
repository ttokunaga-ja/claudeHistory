use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

use crate::fs_platform as platform;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub account: String,
    pub org: String,
    pub sessions: usize,
}
pub struct Store {
    pub root: PathBuf,
    pub backup_root: PathBuf,
}
pub struct Plan {
    pub source: Account,
    pub target: Account,
    pub additions: Vec<String>,
    pub existing: usize,
    source_snapshot: Snapshot,
    target_snapshot: Snapshot,
}
type Snapshot = BTreeMap<String, String>;
pub struct SyncTarget {
    pub account: Account,
    pub additions: usize,
    pub existing: usize,
}
pub struct SyncPlan {
    pub targets: Vec<SyncTarget>,
    snapshots: Vec<Snapshot>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Participant {
    account: Account,
    before: Snapshot,
    originals: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Creation {
    participant: usize,
    name: String,
    hash: String,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    directory_sync: String,
    root: PathBuf,
    // Retained as useful summary fields for the transfer command.
    source_hashes: Snapshot,
    participants: Vec<Participant>,
    planned: Snapshot,
    writes: BTreeMap<String, Creation>,
    created: Snapshot,
    pending: Option<String>,
    status: String,
    rollback: BTreeMap<String, String>,
}
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn filename(s: &str) -> bool {
    s.strip_prefix("local_")
        .and_then(|s| s.strip_suffix(".json"))
        .is_some_and(uuid)
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn safe_path(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "path must be absolute");
    let mut prefix = PathBuf::new();
    for component in path.components() {
        ensure!(
            !matches!(component, Component::ParentDir),
            "parent traversal rejected"
        );
        prefix.push(component);
        // A Windows drive/UNC prefix alone is not an absolute filesystem path.
        // Inspect it only once the following root component has been appended.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&prefix) {
            Ok(m) => platform::reject_redirect(&m, &prefix)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    safe_path(path)?;
    fs::create_dir_all(path)?;
    safe_path(path)?;
    platform::private_dir(path)?;
    Ok(())
}
fn atomic_new(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_new_marked(path, bytes, || {})
}
fn atomic_new_marked(path: &Path, bytes: &[u8], published: impl FnOnce()) -> Result<()> {
    safe_path(path)?;
    let mut f = tempfile::NamedTempFile::new_in(path.parent().context("missing parent")?)?;
    f.write_all(bytes)?;
    f.as_file().sync_all()?;
    platform::publish(f, path, false)?;
    published();
    platform::sync_dir(path.parent().context("missing parent")?)?;
    Ok(())
}
fn journal(path: &Path, manifest: &Manifest) -> Result<()> {
    safe_path(path)?;
    let mut f = tempfile::NamedTempFile::new_in(path.parent().context("journal parent missing")?)?;
    f.write_all(&serde_json::to_vec_pretty(manifest)?)?;
    f.as_file().sync_all()?;
    platform::publish(f, path, true)?;
    platform::sync_dir(path.parent().context("journal parent missing")?)?;
    Ok(())
}
fn read(path: &Path) -> Result<Vec<u8>> {
    safe_path(path)?;
    ensure!(fs::symlink_metadata(path)?.is_file(), "not a regular file");
    Ok(fs::read(path)?)
}
fn snapshot(path: &Path) -> Result<Snapshot> {
    safe_path(path)?;
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    ensure!(path.is_dir(), "session directory is not a directory");
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let p = entry.path();
        safe_path(&p)?;
        ensure!(
            entry.file_type()?.is_file(),
            "unexpected session directory entry"
        );
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non UTF-8 filename"))?;
        if !name.starts_with("local_") || !name.ends_with(".json") {
            continue;
        }
        ensure!(filename(&name), "invalid registration filename: {name}");
        result.insert(name, hash(&read(&p)?));
    }
    Ok(result)
}
fn registration(path: &Path, name: &str) -> Result<Value> {
    registration_bytes(&read(path)?, name)
}
fn registration_bytes(bytes: &[u8], name: &str) -> Result<Value> {
    let v: Value = serde_json::from_slice(bytes).context("malformed registration JSON")?;
    ensure!(v.is_object(), "registration must be an object");
    ensure!(
        v["sessionId"].as_str() == name.strip_suffix(".json"),
        "sessionId differs from filename"
    );
    ensure!(
        v["cwd"]
            .as_str()
            .is_some_and(|s| Path::new(s).is_absolute()),
        "cwd must be absolute"
    );
    for key in ["originCwd", "worktreePath"] {
        if let Some(p) = v.get(key) {
            ensure!(
                p.as_str().is_some_and(|s| Path::new(s).is_absolute()),
                "{key} must be absolute"
            );
        }
    }
    if let Some(id) = v.get("cliSessionId") {
        ensure!(id.as_str().is_some_and(uuid), "cliSessionId must be UUID");
    }
    Ok(v)
}
fn sanitized(v: &Value) -> Result<Vec<u8>> {
    let mut m = Map::new();
    // These fields describe the local session; execution, authentication, grants and connector state are excluded.
    for key in [
        "sessionId",
        "cliSessionId",
        "cwd",
        "originCwd",
        "createdAt",
        "updatedAt",
        "lastActivityAt",
        "title",
        "model",
        "isArchived",
        "lastFocusedAt",
        "titleSource",
        "effort",
        "sourceBranch",
        "branch",
        "writtenBranches",
        "completedTurns",
        "priorCliSessionIds",
        "rewindEdges",
        "transcriptModelStates",
        "forkedFromSessionId",
        "forkedFromCliSessionId",
        "forkedFromMessageId",
        "worktreePath",
    ] {
        if let Some(value) = v.get(key) {
            m.insert(key.into(), value.clone());
        }
    }
    m.insert("permissionMode".into(), Value::String("default".into()));
    for key in [
        "remoteMcpServersConfig",
        "alwaysAllowedReasons",
        "sessionPermissionUpdates",
    ] {
        m.insert(key.into(), Value::Array(vec![]));
    }
    Ok(serde_json::to_vec_pretty(&m)?)
}
struct MutationLock(fs::File);
impl MutationLock {
    fn acquire(root: &Path) -> Result<Self> {
        use fs2::FileExt;
        let path = root.join(".claude-history.lock");
        safe_path(&path)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        safe_path(&path)?;
        file.try_lock_exclusive()
            .context("another history mutation is running")?;
        Ok(Self(file))
    }
}
impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
impl Store {
    fn dir(&self, a: &Account) -> Result<PathBuf> {
        ensure!(
            uuid(&a.account) && uuid(&a.org),
            "account and organization must be UUIDs"
        );
        let p = self
            .root
            .join("claude-code-sessions")
            .join(&a.account)
            .join(&a.org);
        safe_path(&p)?;
        Ok(p)
    }
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let base = self.root.join("claude-code-sessions");
        safe_path(&base)?;
        if !base.exists() {
            return Ok(vec![]);
        }
        let mut result = vec![];
        for a in fs::read_dir(base)? {
            let a = a?;
            safe_path(&a.path())?;
            ensure!(a.file_type()?.is_dir(), "unexpected account entry");
            let account = a.file_name().to_string_lossy().into_owned();
            ensure!(uuid(&account), "invalid account UUID");
            let before = result.len();
            for o in fs::read_dir(a.path())? {
                let o = o?;
                safe_path(&o.path())?;
                ensure!(o.file_type()?.is_dir(), "unexpected organization entry");
                let org = o.file_name().to_string_lossy().into_owned();
                ensure!(uuid(&org), "invalid organization UUID");
                let snap = snapshot(&o.path())?;
                for name in snap.keys() {
                    registration(&o.path().join(name), name)?;
                }
                result.push(Account {
                    account: account.clone(),
                    org,
                    sessions: snap.len(),
                });
            }
            ensure!(
                result.len() > before,
                "organization missing for account {account}; open Claude Desktop for this account to establish its organization before syncing"
            );
        }
        result.sort_by(|a, b| (&a.account, &a.org).cmp(&(&b.account, &b.org)));
        Ok(result)
    }
    pub fn current_account(&self) -> Result<String> {
        let v: Value = serde_json::from_slice(&read(&self.root.join("config.json"))?)?;
        let id = v["lastKnownAccountUuid"]
            .as_str()
            .context("lastKnownAccountUuid missing")?;
        ensure!(uuid(id), "invalid current account UUID");
        Ok(id.into())
    }
    pub fn plan(&self, source: &Account, target: &Account) -> Result<Plan> {
        ensure!(
            source.account != target.account,
            "source and target accounts must differ"
        );
        let src = self.dir(source)?;
        let dst = self.dir(target)?;
        ensure!(src.is_dir(), "source organization missing");
        let source_snapshot = snapshot(&src)?;
        let target_snapshot = snapshot(&dst)?;
        let mut additions = vec![];
        let mut existing = 0;
        for name in target_snapshot.keys() {
            registration(&dst.join(name), name)?;
        }
        for name in source_snapshot.keys() {
            let v = registration(&src.join(name), name)?;
            if target_snapshot.contains_key(name) {
                let t = registration(&dst.join(name), name)?;
                ensure!(
                    v.get("cliSessionId") == t.get("cliSessionId") && v["cwd"] == t["cwd"],
                    "conflicting registration: {name}"
                );
                existing += 1;
            } else {
                additions.push(name.clone());
            }
        }
        Ok(Plan {
            source: source.clone(),
            target: target.clone(),
            additions,
            existing,
            source_snapshot,
            target_snapshot,
        })
    }
    pub fn plan_sync(&self, accounts: &[Account]) -> Result<SyncPlan> {
        ensure!(accounts.len() >= 2, "sync requires at least two accounts");
        let mut accounts = accounts.to_vec();
        accounts.sort_by(|a, b| (&a.account, &a.org).cmp(&(&b.account, &b.org)));
        ensure!(
            accounts
                .windows(2)
                .all(|w| !w[0].account.eq_ignore_ascii_case(&w[1].account)),
            "sync requires distinct account UUIDs and one organization per account"
        );
        // Validate case-insensitive identity independently of textual sort order.
        let unique: std::collections::BTreeSet<_> = accounts
            .iter()
            .map(|a| a.account.to_ascii_lowercase())
            .collect();
        ensure!(unique.len() == accounts.len(), "duplicate account UUID");
        let mut snapshots = Vec::new();
        let mut union = BTreeMap::<String, Value>::new();
        for account in &accounts {
            let dir = self.dir(account)?;
            ensure!(dir.is_dir(), "sync organization missing");
            let snap = snapshot(&dir)?;
            for name in snap.keys() {
                let value = registration(&dir.join(name), name)?;
                if let Some(prior) = union.get(name) {
                    ensure!(
                        prior.get("cliSessionId") == value.get("cliSessionId")
                            && prior["cwd"] == value["cwd"],
                        "conflicting registration: {name}"
                    );
                } else {
                    union.insert(name.clone(), value);
                }
            }
            snapshots.push(snap);
        }
        let targets = accounts
            .into_iter()
            .zip(&snapshots)
            .map(|(account, snapshot)| SyncTarget {
                account,
                additions: union.len() - snapshot.len(),
                existing: snapshot.len(),
            })
            .collect();
        Ok(SyncPlan { targets, snapshots })
    }
    pub fn apply_sync(&self, plan: &SyncPlan) -> Result<PathBuf> {
        self.apply_sync_checked(plan, || Ok(()))
    }
    pub fn apply_sync_checked(
        &self,
        plan: &SyncPlan,
        before_write: impl FnMut() -> Result<()>,
    ) -> Result<PathBuf> {
        let _lock = MutationLock::acquire(&self.root)?;
        let accounts: Vec<_> = plan.targets.iter().map(|t| t.account.clone()).collect();
        let current = self.plan_sync(&accounts)?;
        ensure!(
            current.snapshots == plan.snapshots
                && current
                    .targets
                    .iter()
                    .zip(&plan.targets)
                    .all(|(a, b)| a.account.account == b.account.account
                        && a.account.org == b.account.org
                        && a.additions == b.additions
                        && a.existing == b.existing),
            "history changed since planning or plan was modified"
        );
        let mut sources = BTreeMap::new();
        for (i, snapshot) in plan.snapshots.iter().enumerate() {
            for name in snapshot.keys() {
                sources.entry(name.clone()).or_insert(i);
            }
        }
        let mut requests = BTreeMap::new();
        for (i, target) in plan.targets.iter().enumerate() {
            for (name, source) in &sources {
                if !plan.snapshots[i].contains_key(name) {
                    requests.insert(
                        format!("{}/{}/{}", target.account.account, target.account.org, name),
                        (i, name.clone(), *source),
                    );
                }
            }
        }
        self.transact(&accounts, &plan.snapshots, requests, true, before_write)
    }
    pub fn apply(&self, plan: &Plan) -> Result<PathBuf> {
        self.apply_checked(plan, || Ok(()))
    }
    pub fn apply_checked(
        &self,
        plan: &Plan,
        before_write: impl FnMut() -> Result<()>,
    ) -> Result<PathBuf> {
        let _lock = MutationLock::acquire(&self.root)?;
        let current = self.plan(&plan.source, &plan.target)?;
        ensure!(
            current.source_snapshot == plan.source_snapshot
                && current.target_snapshot == plan.target_snapshot
                && current.additions == plan.additions
                && current.existing == plan.existing,
            "history changed since planning or plan was modified"
        );
        let requests = plan
            .additions
            .iter()
            .map(|name| (name.clone(), (1, name.clone(), 0)))
            .collect();
        self.transact(
            &[plan.source.clone(), plan.target.clone()],
            &[plan.source_snapshot.clone(), plan.target_snapshot.clone()],
            requests,
            false,
            before_write,
        )
    }
    fn transact(
        &self,
        accounts: &[Account],
        snapshots: &[Snapshot],
        requests: BTreeMap<String, (usize, String, usize)>,
        sync: bool,
        mut before_write: impl FnMut() -> Result<()>,
    ) -> Result<PathBuf> {
        let dirs: Vec<_> = accounts
            .iter()
            .map(|a| self.dir(a))
            .collect::<Result<_>>()?;
        private_dir(&self.backup_root)?;
        let backup = tempfile::Builder::new()
            .prefix(if sync { "sync-" } else { "transfer-" })
            .tempdir_in(&self.backup_root)?
            .keep();
        private_dir(&backup)?;
        private_dir(&backup.join("originals"))?;
        let mut participants = Vec::new();
        for (i, account) in accounts.iter().enumerate() {
            let mut originals = BTreeMap::new();
            for (name, expected) in &snapshots[i] {
                let relative = if !sync && i == 0 {
                    name.clone()
                } else {
                    format!("{}/{}/{}", account.account, account.org, name)
                };
                let bytes = read(&dirs[i].join(name))?;
                ensure!(hash(&bytes) == *expected, "history changed during backup");
                let path = backup.join("originals").join(&relative);
                private_dir(path.parent().context("missing backup parent")?)?;
                atomic_new(&path, &bytes)?;
                originals.insert(name.clone(), relative);
            }
            participants.push(Participant {
                account: account.clone(),
                before: snapshots[i].clone(),
                originals,
            });
        }
        let mut payloads = BTreeMap::new();
        let mut writes = BTreeMap::new();
        for (key, (target, name, source)) in requests {
            let original = read(
                &backup
                    .join("originals")
                    .join(&participants[source].originals[&name]),
            )?;
            ensure!(
                hash(&original) == snapshots[source][&name],
                "original backup hash mismatch"
            );
            let bytes = sanitized(&registration_bytes(&original, &name)?)?;
            writes.insert(
                key.clone(),
                Creation {
                    participant: target,
                    name,
                    hash: hash(&bytes),
                },
            );
            payloads.insert(key, bytes);
        }
        let mut manifest = Manifest {
            version: 2,
            directory_sync: platform::DIRECTORY_SYNC.into(),
            root: self.root.clone(),
            source_hashes: snapshots[0].clone(),
            participants,
            planned: writes
                .iter()
                .map(|(k, v)| (k.clone(), v.hash.clone()))
                .collect(),
            writes,
            created: BTreeMap::new(),
            pending: None,
            status: "incomplete".into(),
            rollback: BTreeMap::new(),
        };
        let manifest_path = backup.join("manifest.json");
        atomic_new(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
        let mut owned = BTreeMap::new();
        let result = (|| -> Result<()> {
            before_write()?;
            self.verify_transaction(&manifest, &backup, &owned)?;
            for (key, bytes) in &payloads {
                let write = manifest.writes[key].clone();
                let dst = &dirs[write.participant];
                if !dst.exists() {
                    private_dir(dst)?;
                }
                before_write()?;
                self.verify_transaction(&manifest, &backup, &owned)?;
                // Persist intent before publication; mark our successful publication
                // in memory before fsync so any subsequent failure rolls it back.
                manifest.pending = Some(key.clone());
                journal(&manifest_path, &manifest)?;
                atomic_new_marked(&dst.join(&write.name), bytes, || {
                    owned.insert(key.clone(), write.hash.clone());
                })?;
                manifest.created = owned.clone();
                manifest.pending = None;
                journal(&manifest_path, &manifest)?;
            }
            before_write()?;
            self.verify_transaction(&manifest, &backup, &owned)?;
            manifest.status = "complete".into();
            journal(&manifest_path, &manifest)?;
            Ok(())
        })();
        if let Err(error) = result {
            manifest.status = "incomplete".into();
            manifest.created = owned.clone();
            for (key, expected) in &owned {
                let write = &manifest.writes[key];
                let dir = &dirs[write.participant];
                let path = dir.join(&write.name);
                let outcome = match read(&path) {
                    Ok(bytes) if hash(&bytes) == *expected => match fs::remove_file(&path) {
                        Ok(()) => match platform::sync_dir(dir) {
                            Ok(()) => "removed".into(),
                            Err(e) => format!("removed: directory sync failed: {e}"),
                        },
                        Err(e) => format!("retained: removal failed: {e}"),
                    },
                    Ok(_) => "retained: modified".into(),
                    Err(e) => format!("not removed: {e}"),
                };
                manifest.rollback.insert(key.clone(), outcome);
            }
            let journal_error = journal(&manifest_path, &manifest).err();
            bail!(
                "history transaction failed: {error}; recovery journal {}; rollback outcomes {:?}; journal update {:?}",
                manifest_path.display(),
                manifest.rollback,
                journal_error
            );
        }
        Ok(backup)
    }
    fn verify_transaction(
        &self,
        manifest: &Manifest,
        backup: &Path,
        owned: &Snapshot,
    ) -> Result<()> {
        self.verify_originals(manifest, backup)?;
        for (i, p) in manifest.participants.iter().enumerate() {
            let mut expected = p.before.clone();
            for key in owned.keys() {
                let write = &manifest.writes[key];
                if write.participant == i {
                    expected.insert(write.name.clone(), write.hash.clone());
                }
            }
            ensure!(
                snapshot(&self.dir(&p.account)?)? == expected,
                "history changed during transaction"
            );
        }
        Ok(())
    }
    fn verify_originals(&self, manifest: &Manifest, backup: &Path) -> Result<()> {
        ensure!(
            manifest.participants.len() >= 2,
            "missing transaction participants"
        );
        let mut identities = std::collections::BTreeSet::new();
        for p in &manifest.participants {
            ensure!(
                identities.insert(p.account.account.to_ascii_lowercase()),
                "duplicate transaction participant"
            );
            self.dir(&p.account)?;
            ensure!(
                p.before.len() == p.originals.len(),
                "inconsistent original backup manifest"
            );
            for (name, expected) in &p.before {
                ensure!(filename(name), "invalid backup filename");
                let relative = p.originals.get(name).context("missing original backup")?;
                let expected_relative = format!("{}/{}/{}", p.account.account, p.account.org, name);
                ensure!(
                    relative == &expected_relative
                        || (relative == name && std::ptr::eq(p, &manifest.participants[0])),
                    "invalid original backup path"
                );
                ensure!(
                    hash(&read(&backup.join("originals").join(relative))?) == *expected,
                    "original backup hash mismatch"
                );
            }
        }
        Ok(())
    }
    pub fn undo(&self, backup: &Path) -> Result<usize> {
        self.undo_checked(backup, || Ok(()))
    }
    pub fn undo_checked(
        &self,
        backup: &Path,
        mut before_write: impl FnMut() -> Result<()>,
    ) -> Result<usize> {
        let _lock = MutationLock::acquire(&self.root)?;
        safe_path(backup)?;
        let manifest_path = backup.join("manifest.json");
        let mut m: Manifest = serde_json::from_slice(&read(&manifest_path)?)?;
        ensure!(
            m.version == 2 && m.root == self.root,
            "backup belongs to a different root or version"
        );
        ensure!(
            m.status == "complete" && m.pending.is_none(),
            "incomplete transfer; inspect recovery journal {} manually",
            manifest_path.display()
        );
        ensure!(
            m.created == m.planned && m.writes.len() == m.planned.len(),
            "inconsistent completed manifest"
        );
        self.verify_originals(&m, backup)?;
        let mut originals = BTreeMap::new();
        let mut paths = BTreeMap::new();
        let mut identities = std::collections::BTreeSet::new();
        for (key, write) in &m.writes {
            let p = m
                .participants
                .get(write.participant)
                .context("invalid participant")?;
            ensure!(
                filename(&write.name)
                    && m.created.get(key) == Some(&write.hash)
                    && !p.before.contains_key(&write.name),
                "invalid created registration"
            );
            let target_key = format!("{}/{}/{}", p.account.account, p.account.org, write.name);
            ensure!(
                key == &write.name || key == &target_key,
                "invalid creation key"
            );
            let source = m
                .participants
                .iter()
                .find(|source| source.before.contains_key(&write.name))
                .context("created registration has no source")?;
            let source_bytes = read(
                &backup
                    .join("originals")
                    .join(&source.originals[&write.name]),
            )?;
            ensure!(
                hash(&sanitized(&registration_bytes(
                    &source_bytes,
                    &write.name
                )?)?)
                    == write.hash,
                "created payload differs from original backup"
            );
            ensure!(
                identities.insert((write.participant, write.name.clone())),
                "duplicate created registration"
            );
            ensure!(
                m.participants
                    .iter()
                    .any(|source| source.before.contains_key(&write.name)),
                "created registration has no source"
            );
            let path = self.dir(&p.account)?.join(&write.name);
            let bytes = read(&path)?;
            ensure!(
                hash(&bytes) == write.hash,
                "created registration changed; undo aborted"
            );
            originals.insert(key.clone(), bytes);
            paths.insert(key.clone(), path);
        }
        let mut removed = Vec::new();
        let result = (|| -> Result<()> {
            for (key, path) in &paths {
                before_write()?;
                self.verify_originals(&m, backup)?;
                // Revalidate every remaining pointer before each removal, not just
                // the next one, so resumed work cannot cause partial deletion.
                for (remaining, p) in &paths {
                    if !removed.contains(remaining) {
                        ensure!(
                            hash(&read(p)?) == m.created[remaining],
                            "registration changed during undo"
                        );
                    }
                }
                m.pending = Some(key.clone());
                m.status = "undoing".into();
                journal(&manifest_path, &m)?;
                fs::remove_file(path)?;
                removed.push(key.clone());
                platform::sync_dir(path.parent().context("missing parent")?)?;
                m.rollback.insert(key.clone(), "undo removed".into());
                m.pending = None;
                journal(&manifest_path, &m)?;
            }
            before_write()?;
            self.verify_originals(&m, backup)?;
            for path in paths.values() {
                safe_path(path)?;
                ensure!(!path.try_exists()?, "registration recreated during undo");
            }
            m.status = "undone".into();
            journal(&manifest_path, &m)?;
            Ok(())
        })();
        if let Err(error) = result {
            let mut failures = Vec::new();
            for key in &removed {
                if let Err(e) = atomic_new(&paths[key], &originals[key]) {
                    failures.push(format!("{key}: {e}"));
                }
            }
            m.pending = None;
            m.status = if failures.is_empty() {
                "complete"
            } else {
                "incomplete"
            }
            .into();
            let journal_error = journal(&manifest_path, &m).err();
            bail!(
                "undo failed: {error}; restoration failures: {failures:?}; journal update {journal_error:?}"
            );
        }
        Ok(m.created.len())
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn pending_publication_is_rolled_back_if_directory_sync_fails() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let store = Store {
            root: base.join("Claude"),
            backup_root: base.join("backup"),
        };
        let accounts: Vec<_> = (1..=3)
            .map(|i| Account {
                account: format!("{i:08}-0000-4000-8000-000000000000"),
                org: "00000000-0000-4000-8000-000000000000".into(),
                sessions: 1,
            })
            .collect();
        for (i, account) in accounts.iter().enumerate() {
            let dir = store.dir(account).unwrap();
            fs::create_dir_all(&dir).unwrap();
            let name = format!("local_{:08}-0000-4000-8000-000000000000.json", i + 10);
            fs::write(dir.join(&name),serde_json::to_vec(&serde_json::json!({"sessionId":name.trim_end_matches(".json"),"cwd":base.join(format!("project{i}"))})).unwrap()).unwrap();
        }
        let mut checks = 0;
        let result = store.apply_sync_checked(&store.plan_sync(&accounts).unwrap(), || {
            checks += 1;
            if checks == 4 {
                platform::fail_next_sync(&store.dir(&accounts[1])?);
            }
            Ok(())
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("injected directory sync failure")
        );
        for a in &accounts {
            assert_eq!(snapshot(&store.dir(a).unwrap()).unwrap().len(), 1);
        }
        let backup = fs::read_dir(&store.backup_root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let manifest: Manifest =
            serde_json::from_slice(&read(&backup.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.created.len(), 3);
        assert!(manifest.pending.is_some());
        assert!(
            manifest
                .rollback
                .values()
                .all(|outcome| outcome == "removed")
        );
    }
}
