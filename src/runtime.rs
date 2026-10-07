//! Conservative, read-only activity observation. No process arguments enter diagnostics.
use anyhow::{Result, bail};
mod macos;
mod windows;
#[cfg(not(target_os = "windows"))]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

#[doc(hidden)]
pub fn classify_windows_fixture(
    snapshot: &str,
    local_app_data: &str,
    self_pid: u32,
) -> Result<Activity> {
    windows::classify_fixture(snapshot, local_app_data, self_pid)
}
#[doc(hidden)]
pub fn windows_argv_fixture(command: &str) -> Result<Vec<String>> {
    windows::argv(command)
}
#[doc(hidden)]
pub fn windows_tcp_fixture(snapshot: &str, pids: &[i32]) -> Result<bool> {
    windows::tcp_output(snapshot, pids)
}
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Stopped,
    IdleDesktop { pids: Vec<i32> },
    Busy { reasons: Vec<String> },
    Unknown { reasons: Vec<String> },
}
#[derive(Clone)]
struct Process {
    pid: i32,
    ppid: i32,
    executable: String,
    args: Vec<String>,
}
fn node_cli(p: &Process) -> bool {
    // Read native argv boundaries; paths may contain spaces.
    let mut args = p.args.iter().map(String::as_str);
    args.next();
    while let Some(arg) = args.next() {
        if matches!(arg, "-e" | "--eval" | "-p" | "--print") {
            return false;
        }
        if matches!(
            arg,
            "-r" | "--require"
                | "--import"
                | "--loader"
                | "--experimental-loader"
                | "--conditions"
                | "-C"
                | "--env-file"
                | "--env-file-if-exists"
        ) {
            args.next();
            continue;
        }
        if arg.starts_with('-') && arg != "--" {
            continue;
        }
        let script = if arg == "--" {
            args.next().unwrap_or("")
        } else {
            arg
        };
        let matches = script
            .trim_matches(['\'', '"'])
            .ends_with("/@anthropic-ai/claude-code/cli.js");
        // Unknown separate option operands cannot establish that the later Claude
        // script is only prompt text. Conservatively block in that case.
        return matches
            || (!script.ends_with(".js")
                && !script.ends_with(".mjs")
                && !script.ends_with(".cjs")
                && p.args
                    .iter()
                    .any(|a| a.ends_with("/@anthropic-ai/claude-code/cli.js")));
    }
    false
}
fn classify_with(
    processes: &[Process],
    self_pid: i32,
    desktop: impl Fn(&Process) -> bool,
    main_desktop: impl Fn(&Process) -> bool,
    cli: impl Fn(&Process) -> bool,
) -> Activity {
    let processes: Vec<_> = processes.iter().filter(|p| p.pid != self_pid).collect();
    let mut related: BTreeSet<i32> = processes
        .iter()
        .filter(|p| desktop(p))
        .map(|p| p.pid)
        .collect();
    loop {
        let before = related.len();
        for p in &processes {
            if related.contains(&p.ppid) {
                related.insert(p.pid);
            }
        }
        if related.len() == before {
            break;
        }
    }
    let mut busy = Vec::new();
    let mut unknown = Vec::new();
    for p in &processes {
        if cli(p) {
            busy.push(format!("Claude CLI is running (PID {})", p.pid));
        } else if related.contains(&p.pid) && !desktop(p) {
            busy.push(format!("Desktop job descendant is running (PID {})", p.pid));
        } else if !desktop(p)
            && p.executable.rsplit(['/', '\\']).next().is_some_and(|s| {
                !s.eq_ignore_ascii_case("claudehistory")
                    && !s.eq_ignore_ascii_case("claudehistory.exe")
            })
            && p.executable.to_lowercase().contains("claude")
        {
            unknown.push(format!("Unrecognized Claude process (PID {})", p.pid));
        }
    }
    if !busy.is_empty() {
        Activity::Busy { reasons: busy }
    } else if !unknown.is_empty() {
        Activity::Unknown { reasons: unknown }
    } else if related.is_empty() {
        Activity::Stopped
    } else if !processes.iter().any(|p| main_desktop(p)) {
        Activity::Unknown {
            reasons: vec!["Desktop helpers exist without the main app".into()],
        }
    } else {
        Activity::IdleDesktop {
            pids: related.into_iter().collect(),
        }
    }
}
/// Fixture-only macOS process classification without native observation.
#[doc(hidden)]
pub fn classify_fixture(
    snapshot: &str,
    arguments: &[(i32, &str)],
    self_pid: u32,
) -> Result<Activity> {
    macos::classify_fixture(snapshot, arguments, self_pid)
}
#[doc(hidden)]
pub fn classify_argv_fixture(
    snapshot: &str,
    arguments: &[(i32, &[&str])],
    self_pid: u32,
) -> Result<Activity> {
    macos::classify_argv_fixture(snapshot, arguments, self_pid)
}

fn supported() -> Result<()> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        bail!("Activity inspection is supported only on macOS and Windows");
    }
    Ok(())
}
#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: SystemTime,
}
type Metadata = BTreeMap<PathBuf, Stamp>;
fn record(path: &Path, out: &mut Metadata) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    if platform::redirected(&m) {
        bail!("history contains a redirected link");
    }
    if m.is_file() {
        out.insert(
            path.to_path_buf(),
            Stamp {
                size: m.len(),
                modified: m.modified()?,
            },
        );
    }
    Ok(())
}
fn collect_json(path: &Path, extension: &str, depth: usize, out: &mut Metadata) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
        Ok(m) => {
            if platform::redirected(&m) || !m.is_dir() {
                bail!("invalid history directory");
            }
        }
    }
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_depth(depth)
    {
        let entry = entry?;
        if platform::redirected(&fs::symlink_metadata(entry.path())?)
            && (entry.depth() < depth
                || entry.path().extension().and_then(|s| s.to_str()) == Some(extension))
        {
            bail!("history contains a redirected link");
        }
        if entry.file_type().is_file()
            && entry.path().extension().and_then(|s| s.to_str()) == Some(extension)
        {
            record(entry.path(), out)?;
        }
    }
    Ok(())
}
fn reject_symlink_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(m) if platform::redirected(&m) => bail!("history path contains a redirected link"),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn metadata(data_root: &Path, cli_root: &Path) -> Result<Metadata> {
    reject_symlink_ancestors(data_root)?;
    reject_symlink_ancestors(cli_root)?;
    let mut out = BTreeMap::new();
    collect_json(&data_root.join("claude-code-sessions"), "json", 3, &mut out)?;
    collect_json(
        &data_root.join("local-agent-mode-sessions"),
        "json",
        3,
        &mut out,
    )?;
    collect_json(&cli_root.join("projects"), "jsonl", 2, &mut out)?;
    Ok(out)
}
fn unknown() -> Activity {
    Activity::Unknown {
        reasons: vec!["Activity observation failed; close Claude manually and retry".into()],
    }
}
pub fn inspect(data_root: &Path, cli_root: &Path) -> Result<Activity> {
    supported()?;
    let result = (|| -> Result<Activity> {
        let before = platform::snapshot()?;
        let initial = platform::classify_current(&before, std::process::id() as i32);
        if matches!(initial, Activity::Busy { .. } | Activity::Unknown { .. }) {
            return Ok(initial);
        }
        let files = metadata(data_root, cli_root)?;
        thread::sleep(Duration::from_secs(1));
        let after = platform::snapshot()?;
        let final_state = platform::classify_current(&after, std::process::id() as i32);
        if matches!(
            final_state,
            Activity::Busy { .. } | Activity::Unknown { .. }
        ) {
            return Ok(final_state);
        }
        if files != metadata(data_root, cli_root)? {
            return Ok(Activity::Busy {
                reasons: vec!["History changed during observation".into()],
            });
        }
        if initial != final_state {
            return Ok(Activity::Unknown {
                reasons: vec!["Processes changed during observation".into()],
            });
        }
        if let Activity::IdleDesktop { ref pids } = final_state
            && platform::tcp(pids)?
        {
            return Ok(Activity::Unknown {
                reasons: vec![
                    "Desktop has TCP connections; activity cannot be distinguished".into(),
                ],
            });
        }
        Ok(final_state)
    })();
    Ok(result.unwrap_or_else(|_| unknown()))
}
pub fn require_stopped() -> Result<()> {
    supported()?;
    if platform::classify_current(&platform::snapshot()?, std::process::id() as i32)
        != Activity::Stopped
    {
        bail!("作業中です。作業を終了してから進めてください。（Claude関連プロセスを検知）");
    }
    Ok(())
}
pub fn close_idle_desktop(pids: &[i32], data_root: &Path, cli_root: &Path) -> Result<()> {
    supported()?;
    if inspect(data_root, cli_root)?
        != (Activity::IdleDesktop {
            pids: pids.to_vec(),
        })
    {
        bail!("Desktop activity changed; close Claude manually");
    }
    let processes = platform::snapshot()?;
    if platform::classify_current(&processes, std::process::id() as i32)
        != (Activity::IdleDesktop {
            pids: pids.to_vec(),
        })
    {
        bail!("Desktop process identity changed");
    }
    let mains: Vec<_> = processes
        .iter()
        .filter(|p| platform::main_desktop_current(p) && pids.contains(&p.pid))
        .collect();
    if mains.len() != 1 {
        bail!("Cannot identify one Desktop main process");
    }
    platform::close(mains[0].pid)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let procs = platform::snapshot()?;
        let state = platform::classify_current(&procs, std::process::id() as i32);
        if state == Activity::Stopped && !procs.iter().any(|p| pids.contains(&p.pid)) {
            return Ok(());
        }
        if matches!(state, Activity::Busy { .. }) {
            bail!("Claude work appeared during shutdown; no further signals were sent");
        }
        if Instant::now() >= deadline {
            bail!("Desktop did not exit normally; close it manually");
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_change_and_symlinks_are_detected() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let data = base.join("data");
        let cli = base.join("cli");
        let registry = data.join("claude-code-sessions/a/o");
        fs::create_dir_all(&registry).unwrap();
        fs::create_dir_all(cli.join("projects/p")).unwrap();
        fs::write(registry.join("a.json"), "{}").unwrap();
        fs::write(cli.join("projects/p/a.jsonl"), "{}").unwrap();
        fs::create_dir_all(data.join("vm_bundles")).unwrap();
        fs::write(data.join("vm_bundles/ignored.json"), "{}").unwrap();
        let before = metadata(&data, &cli).unwrap();
        fs::write(data.join("vm_bundles/ignored.json"), "changed").unwrap();
        assert_eq!(before, metadata(&data, &cli).unwrap());
        fs::write(cli.join("projects/p/a.jsonl"), "{updated}").unwrap();
        assert_ne!(before, metadata(&data, &cli).unwrap());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(registry.join("a.json"), registry.join("linked.json"))
                .unwrap();
            assert!(metadata(&data, &cli).is_err());
        }
    }
    #[cfg(target_os = "windows")]
    #[test]
    fn metadata_rejects_windows_junctions_and_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let cli = temp.path().join("cli");
        let registry = data.join("claude-code-sessions").join("a");
        let external = temp.path().join("external");
        fs::create_dir_all(&registry).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("work.json"), "{}").unwrap();
        let junction = registry.join("linked");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&external)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "fixture junction creation failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(metadata(&data, &cli).is_err());
        assert!(metadata(&junction.join("nested"), &cli).is_err());
        fs::remove_dir(&junction).unwrap();
        assert!(external.join("work.json").is_file());
    }
}
