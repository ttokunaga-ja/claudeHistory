use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, io::AsRawFd},
    path::{Component, Path, PathBuf},
};

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
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    root: PathBuf,
    source: Account,
    target: Account,
    source_hashes: Snapshot,
    target_before: Snapshot,
    planned: Snapshot,
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
        match fs::symlink_metadata(&prefix) {
            Ok(m) => ensure!(
                !m.file_type().is_symlink(),
                "symlink rejected: {}",
                prefix.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    safe_path(path)?;
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn atomic_new(path: &Path, bytes: &[u8]) -> Result<()> {
    safe_path(path)?;
    let mut f = tempfile::NamedTempFile::new_in(path.parent().context("missing parent")?)?;
    f.write_all(bytes)?;
    f.as_file().sync_all()?;
    f.persist_noclobber(path).map_err(|e| e.error)?;
    Ok(())
}
fn journal(path: &Path, manifest: &Manifest) -> Result<()> {
    safe_path(path)?;
    let mut f = tempfile::NamedTempFile::new_in(path.parent().context("journal parent missing")?)?;
    f.write_all(&serde_json::to_vec_pretty(manifest)?)?;
    f.as_file().sync_all()?;
    f.persist(path).map_err(|e| e.error)?;
    fs::File::open(path.parent().context("journal parent missing")?)?.sync_all()?;
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
        let path = root.join("claude-code-sessions");
        safe_path(&path)?;
        let file = fs::File::open(path)?;
        // Advisory lock coordinates tool instances; the Desktop app does not participate.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        ensure!(
            result == 0,
            "another history mutation is running: {}",
            std::io::Error::last_os_error()
        );
        Ok(Self(file))
    }
}
impl Drop for MutationLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
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
    pub fn apply(&self, plan: &Plan) -> Result<PathBuf> {
        self.apply_checked(plan, || Ok(()))
    }
    pub fn apply_checked(
        &self,
        plan: &Plan,
        mut before_write: impl FnMut() -> Result<()>,
    ) -> Result<PathBuf> {
        let _lock = MutationLock::acquire(&self.root)?;
        let src = self.dir(&plan.source)?;
        let dst = self.dir(&plan.target)?;
        ensure!(
            snapshot(&src)? == plan.source_snapshot && snapshot(&dst)? == plan.target_snapshot,
            "history changed since planning"
        );
        let current = self.plan(&plan.source, &plan.target)?;
        ensure!(
            current.additions == plan.additions && current.existing == plan.existing,
            "plan was modified"
        );
        private_dir(&self.backup_root)?;
        let backup = tempfile::Builder::new()
            .prefix("transfer-")
            .tempdir_in(&self.backup_root)?
            .keep();
        private_dir(&backup)?;
        private_dir(&backup.join("originals"))?;
        for (name, expected) in &plan.source_snapshot {
            let bytes = read(&src.join(name))?;
            ensure!(hash(&bytes) == *expected, "source changed during backup");
            atomic_new(&backup.join("originals").join(name), &bytes)?;
        }
        // Freeze sanitized data from the secured, hash-checked original backup.
        let mut payloads = BTreeMap::new();
        for name in &plan.additions {
            let original = read(&backup.join("originals").join(name))?;
            payloads.insert(
                name.clone(),
                sanitized(&registration_bytes(&original, name)?)?,
            );
        }
        let mut manifest = Manifest {
            version: 1,
            root: self.root.clone(),
            source: plan.source.clone(),
            target: plan.target.clone(),
            source_hashes: plan.source_snapshot.clone(),
            target_before: plan.target_snapshot.clone(),
            planned: payloads
                .iter()
                .map(|(name, bytes)| (name.clone(), hash(bytes)))
                .collect(),
            created: BTreeMap::new(),
            pending: None,
            status: "incomplete".into(),
            rollback: BTreeMap::new(),
        };
        let manifest_path = backup.join("manifest.json");
        // The intent is durable before any target pointer is published. Incomplete
        // journals are retained for manual recovery and never automatically undone.
        atomic_new(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
        fs::File::open(&backup)?.sync_all()?;
        let result = (|| -> Result<()> {
            before_write()?;
            ensure!(
                snapshot(&src)? == plan.source_snapshot && snapshot(&dst)? == plan.target_snapshot,
                "history changed before write"
            );
            if !dst.exists() {
                private_dir(&dst)?;
            } else {
                safe_path(&dst)?;
            }
            for (name, bytes) in &payloads {
                before_write()?;
                manifest.pending = Some(name.clone());
                journal(&manifest_path, &manifest)?;
                atomic_new(&dst.join(name), bytes)?;
                manifest.created.insert(name.clone(), hash(bytes));
                fs::File::open(&dst)?.sync_all()?;
                manifest.pending = None;
                journal(&manifest_path, &manifest)?;
            }
            let mut expected_target = plan.target_snapshot.clone();
            expected_target.extend(manifest.created.clone());
            ensure!(
                snapshot(&src)? == plan.source_snapshot && snapshot(&dst)? == expected_target,
                "history changed during transfer"
            );
            before_write()?;
            manifest.status = "complete".into();
            journal(&manifest_path, &manifest)?;
            Ok(())
        })();
        if let Err(e) = result {
            manifest.status = "incomplete".into();
            for (name, h) in &manifest.created {
                let path = dst.join(name);
                let outcome = match read(&path) {
                    Ok(bytes) if hash(&bytes) == *h => match fs::remove_file(&path) {
                        Ok(()) => "removed".into(),
                        Err(error) => format!("retained: removal failed: {error}"),
                    },
                    Ok(_) => "retained: modified".into(),
                    Err(error) => format!("not removed: {error}"),
                };
                manifest.rollback.insert(name.clone(), outcome);
            }
            let journal_result = journal(&manifest_path, &manifest);
            bail!(
                "transfer failed: {e}; recovery journal {}; rollback outcomes {:?}; journal update {:?}",
                manifest_path.display(),
                manifest.rollback,
                journal_result.err()
            );
        }
        Ok(backup)
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
        let m: Manifest = serde_json::from_slice(&read(&backup.join("manifest.json"))?)?;
        ensure!(
            m.version == 1 && m.root == self.root,
            "backup belongs to a different root or version"
        );
        ensure!(
            m.status == "complete" && m.pending.is_none(),
            "incomplete transfer; inspect recovery journal {} manually",
            backup.join("manifest.json").display()
        );
        ensure!(m.created == m.planned, "inconsistent completed manifest");
        let dst = self.dir(&m.target)?;
        self.dir(&m.source)?;
        for (name, h) in &m.source_hashes {
            ensure!(filename(name), "invalid backup filename");
            ensure!(
                hash(&read(&backup.join("originals").join(name))?) == *h,
                "original backup hash mismatch"
            );
        }
        let mut originals = BTreeMap::new();
        for (name, h) in &m.created {
            ensure!(
                filename(name) && m.source_hashes.contains_key(name),
                "invalid created filename"
            );
            let bytes = read(&dst.join(name))?;
            ensure!(
                hash(&bytes) == *h,
                "created registration changed; undo aborted"
            );
            originals.insert(name.clone(), bytes);
        }
        let mut removed = Vec::new();
        for name in m.created.keys() {
            let result = (|| -> Result<()> {
                before_write()?;
                ensure!(
                    hash(&read(&dst.join(name))?) == m.created[name],
                    "registration changed during undo"
                );
                fs::remove_file(dst.join(name))?;
                Ok(())
            })();
            if let Err(e) = result {
                let mut failed = Vec::new();
                for prior in &removed {
                    if let Err(restore) = atomic_new(&dst.join(prior), &originals[prior]) {
                        failed.push(format!("{prior}: {restore}"));
                    }
                }
                bail!("undo failed: {e}; restoration failures: {failed:?}");
            }
            removed.push(name.clone());
        }
        Ok(m.created.len())
    }
}
