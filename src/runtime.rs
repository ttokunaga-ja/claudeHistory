//! Conservative, read-only activity observation. No process arguments enter diagnostics.
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
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
fn parse(snapshot: &str) -> Result<Vec<Process>> {
    snapshot
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(|line| {
            let line = line.trim();
            let (pid, rest) = line
                .split_once(char::is_whitespace)
                .context("invalid process snapshot")?;
            let (ppid, exe) = rest
                .trim_start()
                .split_once(char::is_whitespace)
                .context("invalid process snapshot")?;
            let pid: i32 = pid.parse()?;
            let ppid: i32 = ppid.parse()?;
            if pid <= 0 || ppid < 0 || exe.trim().is_empty() {
                bail!("invalid process snapshot");
            }
            Ok(Process {
                pid,
                ppid,
                executable: exe.trim().into(),
                args: Vec::new(),
            })
        })
        .collect()
}
fn desktop(p: &Process) -> bool {
    let Some((_, tail)) = p.executable.split_once("/Claude.app/Contents/") else {
        return false;
    };
    tail == "MacOS/Claude"
        || (tail.starts_with("Frameworks/")
            && (matches!(
                Path::new(tail).file_name().and_then(|s| s.to_str()),
                Some(
                    "Claude Helper"
                        | "Claude Helper (GPU)"
                        | "Claude Helper (Renderer)"
                        | "Claude Helper (Plugin)"
                )
            ) || tail.ends_with("/chrome_crashpad_handler")))
}
fn main_desktop(p: &Process) -> bool {
    p.executable.ends_with("/Claude.app/Contents/MacOS/Claude")
}
fn cli(p: &Process) -> bool {
    let basename = Path::new(&p.executable)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if basename == "claude" || p.executable.contains("/.local/share/claude/versions/") {
        return true;
    }
    if basename != "node" && basename != "nodejs" {
        return false;
    }
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
fn classify(processes: &[Process], self_pid: i32) -> Activity {
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
            && Path::new(&p.executable)
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.to_lowercase() != "claudehistory")
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
/// Fixture-only parser entry point, without invoking processes or signals.
#[doc(hidden)]
pub fn classify_fixture(
    snapshot: &str,
    arguments: &[(i32, &str)],
    self_pid: u32,
) -> Result<Activity> {
    let mut processes = parse(snapshot)?;
    for p in &mut processes {
        p.args = arguments
            .iter()
            .find(|(pid, _)| *pid == p.pid)
            .map(|(_, a)| *a)
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_owned)
            .collect();
    }
    Ok(classify(&processes, self_pid as i32))
}
#[doc(hidden)]
pub fn classify_argv_fixture(
    snapshot: &str,
    arguments: &[(i32, &[&str])],
    self_pid: u32,
) -> Result<Activity> {
    let mut processes = parse(snapshot)?;
    for p in &mut processes {
        p.args = arguments
            .iter()
            .find(|(pid, _)| *pid == p.pid)
            .map(|(_, argv)| argv.iter().map(|s| (*s).to_owned()).collect())
            .unwrap_or_default();
    }
    Ok(classify(&processes, self_pid as i32))
}

fn parse_native_argv(bytes: &[u8]) -> Result<Vec<String>> {
    let argc = i32::from_ne_bytes(bytes.get(..4).context("missing argv count")?.try_into()?);
    if !(1..100_000).contains(&argc) {
        bail!("invalid argv count");
    }
    let mut cursor = 4;
    cursor += bytes[cursor..]
        .iter()
        .position(|b| *b == 0)
        .context("missing executable terminator")?
        + 1;
    while bytes.get(cursor) == Some(&0) {
        cursor += 1;
    }
    let mut argv = Vec::new();
    for _ in 0..argc {
        let rest = bytes.get(cursor..).context("truncated argv")?;
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .context("missing argv terminator")?;
        argv.push(std::str::from_utf8(&rest[..end])?.to_owned());
        cursor += end + 1;
    }
    Ok(argv)
}

#[cfg(target_os = "macos")]
fn native_argv(pid: i32) -> Result<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size = 0;
    // KERN_PROCARGS2 returns argc, executable path, then NUL-separated argv.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error()).context("argv observation unavailable");
    }
    if size == 0 || size > 16 * 1024 * 1024 {
        bail!("invalid argv buffer size");
    }
    let mut bytes = vec![0u8; size];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error()).context("argv observation failed");
    }
    bytes.truncate(size);
    parse_native_argv(&bytes)
}
#[cfg(not(target_os = "macos"))]
fn native_argv(_pid: i32) -> Result<Vec<String>> {
    bail!("unsupported platform");
}
fn supported() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("Activity inspection is supported only on macOS");
    }
    Ok(())
}
fn snapshot() -> Result<Vec<Process>> {
    supported()?;
    let out = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,comm="])
        .output()
        .context("cannot observe processes")?;
    if !out.status.success() {
        bail!("process observation failed");
    }
    let mut processes = parse(std::str::from_utf8(&out.stdout)?)?;
    for p in &mut processes {
        let name = Path::new(&p.executable)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if name == "node" || name == "nodejs" {
            p.args = native_argv(p.pid)?;
        }
    }
    Ok(processes)
}
#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: SystemTime,
}
type Metadata = BTreeMap<PathBuf, Stamp>;
fn record(path: &Path, out: &mut Metadata) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    if m.file_type().is_symlink() {
        bail!("history contains a symbolic link");
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
            if m.file_type().is_symlink() || !m.is_dir() {
                bail!("invalid history directory");
            }
        }
    }
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_depth(depth)
    {
        let entry = entry?;
        if entry.file_type().is_symlink()
            && (entry.depth() < depth
                || entry.path().extension().and_then(|s| s.to_str()) == Some(extension))
        {
            bail!("history contains a symbolic link");
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
            Ok(m) if m.file_type().is_symlink() => bail!("history path contains a symbolic link"),
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
fn tcp(pids: &[i32]) -> Result<bool> {
    let ids = pids
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let out = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-p", &ids, "-iTCP", "-F", "fPnT"])
        .output()
        .context("TCP observation unavailable")?;
    if !out.status.success() {
        if out.status.code() == Some(1) && out.stdout.is_empty() && out.stderr.is_empty() {
            return Ok(false);
        }
        bail!("TCP observation failed");
    }
    tcp_output(std::str::from_utf8(&out.stdout)?)
}
fn tcp_output(s: &str) -> Result<bool> {
    if s.is_empty() {
        return Ok(false);
    }
    let mut socket = false;
    let mut listening = false;
    let mut saw_socket = false;
    for line in s.lines() {
        if line.starts_with('f') {
            if socket && !listening {
                return Ok(true);
            }
            socket = true;
            listening = false;
            saw_socket = true;
        } else if line.starts_with("TST=") {
            if line != "TST=LISTEN" {
                return Ok(true);
            }
            listening = true;
        }
    }
    Ok(!saw_socket || (socket && !listening))
}

fn unknown() -> Activity {
    Activity::Unknown {
        reasons: vec!["Activity observation failed; close Claude manually and retry".into()],
    }
}
pub fn inspect(data_root: &Path, cli_root: &Path) -> Result<Activity> {
    supported()?;
    let result = (|| -> Result<Activity> {
        let before = snapshot()?;
        let initial = classify(&before, std::process::id() as i32);
        if matches!(initial, Activity::Busy { .. } | Activity::Unknown { .. }) {
            return Ok(initial);
        }
        let files = metadata(data_root, cli_root)?;
        thread::sleep(Duration::from_secs(1));
        let after = snapshot()?;
        let final_state = classify(&after, std::process::id() as i32);
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
            && tcp(pids)?
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
    if classify(&snapshot()?, std::process::id() as i32) != Activity::Stopped {
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
    let processes = snapshot()?;
    if classify(&processes, std::process::id() as i32)
        != (Activity::IdleDesktop {
            pids: pids.to_vec(),
        })
    {
        bail!("Desktop process identity changed");
    }
    let mains: Vec<_> = processes
        .iter()
        .filter(|p| main_desktop(p) && pids.contains(&p.pid))
        .collect();
    if mains.len() != 1 {
        bail!("Cannot identify one Desktop main process");
    }
    // Signal only the positively identified main app, never helpers or CLI workers.
    if unsafe { libc::kill(mains[0].pid, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error()).context("normal Desktop termination failed");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let procs = snapshot()?;
        let state = classify(&procs, std::process::id() as i32);
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
    fn native_buffer_preserves_argv_and_ignores_environment() {
        let mut bytes = 3i32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(
            b"/usr/bin/node\0\0node\0--conditions\0path with spaces\0SECRET=not argv\0",
        );
        assert_eq!(
            parse_native_argv(&bytes).unwrap(),
            vec!["node", "--conditions", "path with spaces"]
        );
        assert!(parse_native_argv(&bytes[..10]).is_err());
        assert!(parse_native_argv(&[]).is_err());
    }
    #[test]
    fn socket_states_fail_closed() {
        assert!(!tcp_output("").unwrap());
        assert!(!tcp_output("p10\nf10\nn*:3000\nTST=LISTEN\n").unwrap());
        assert!(tcp_output("p10\nTST=ESTABLISHED\n").unwrap());
        assert!(tcp_output("p10\nTST=SYN_SENT\n").unwrap());
        assert!(tcp_output("unexpected output\n").unwrap());
        assert!(tcp_output("p10\nf10\nTST=LISTEN\nf11\nPTCP\nnremote\n").unwrap());
    }
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
}
