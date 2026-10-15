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
#[derive(Debug, Clone, PartialEq, Eq)]
struct Process {
    pid: i32,
    ppid: i32,
    executable: String,
    args: Vec<String>,
    started: String,
}
fn node_cli(p: &Process) -> bool {
    node_cli_with(p, true)
}
fn node_cli_owned(p: &Process) -> bool {
    node_cli_with(p, false)
}
fn node_cli_with(p: &Process, conservative: bool) -> bool {
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
            || (conservative
                && !script.ends_with(".js")
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
        .filter(|p| desktop(p) || cli(p))
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
            busy.push(format!("Claude job descendant is running (PID {})", p.pid));
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
/// A fresh, provider-scoped set of processes. Arguments stay private.
#[derive(Debug)]
pub struct ShutdownPlan {
    processes: Vec<Process>,
}
impl ShutdownPlan {
    pub fn pids(&self) -> Vec<i32> {
        self.processes.iter().map(|p| p.pid).collect()
    }
}
fn targets_with(
    processes: &[Process],
    self_pid: i32,
    known: impl Fn(&Process) -> bool,
) -> Result<Vec<Process>> {
    let unique: BTreeSet<_> = processes.iter().map(|p| p.pid).collect();
    if unique.len() != processes.len() {
        bail!("Duplicate process identities; no termination requests sent");
    }
    let mut protected = BTreeSet::from([self_pid]);
    let mut cursor = self_pid;
    loop {
        let Some(p) = processes.iter().find(|p| p.pid == cursor) else {
            bail!("Caller ancestry observation incomplete; run from an independent terminal");
        };
        if p.ppid == 0 {
            break;
        }
        if !protected.insert(p.ppid) {
            bail!("Caller ancestry cycle; no termination requests sent");
        }
        cursor = p.ppid;
    }
    let mut related: BTreeSet<_> = processes
        .iter()
        .filter(|p| known(p))
        .map(|p| p.pid)
        .collect();
    loop {
        let before = related.len();
        for p in processes {
            if related.contains(&p.ppid) {
                related.insert(p.pid);
            }
        }
        if related.len() == before {
            break;
        }
    }
    if !related.is_disjoint(&protected) {
        bail!(
            "Claude is this command's caller or ancestor; run from an independent terminal. No processes were terminated."
        );
    }
    Ok(processes
        .iter()
        .filter(|p| related.contains(&p.pid))
        .cloned()
        .collect())
}
fn target_processes_with(
    processes: &[Process],
    self_pid: i32,
    known: impl Fn(&Process) -> bool,
    uncertain_cli: impl Fn(&Process) -> bool,
) -> Result<Vec<Process>> {
    let targets = targets_with(processes, self_pid, known)?;
    let unknown: Vec<_> = processes
        .iter()
        .filter(|p| {
            let name = p
                .executable
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            name != "claudehistory"
                && name != "claudehistory.exe"
                && p.executable.to_ascii_lowercase().contains("claude")
                && !targets.iter().any(|target| target.pid == p.pid)
        })
        .map(|p| p.pid.to_string())
        .collect();
    if !unknown.is_empty() {
        bail!(
            "Unrecognized Claude process (PID {}); close it manually and retry",
            unknown.join(", ")
        );
    }
    if processes
        .iter()
        .any(|p| !targets.iter().any(|target| target.pid == p.pid) && uncertain_cli(p))
    {
        bail!("Uncertain Claude CLI ownership; close it manually and retry");
    }
    if targets.iter().any(|p| p.started.is_empty()) {
        bail!("Claude process creation identity unavailable");
    }
    Ok(targets)
}
fn uncertain_cli(p: &Process) -> bool {
    matches!(
        platform::classify_current(std::slice::from_ref(p), std::process::id() as i32),
        Activity::Busy { .. }
    )
}
fn target_processes(processes: &[Process]) -> Result<Vec<Process>> {
    target_processes_with(
        processes,
        std::process::id() as i32,
        platform::known_current,
        uncertain_cli,
    )
}
pub fn shutdown_plan(activity: &Activity) -> Result<ShutdownPlan> {
    supported()?;
    if let Activity::Unknown { reasons } = activity
        && reasons.iter().any(|r| {
            r != "Desktop helpers exist without the main app"
                && r != "Desktop has TCP connections; activity cannot be distinguished"
        })
    {
        bail!("Activity observation failed or changed; close Claude manually and retry");
    }
    let processes = target_processes(&platform::snapshot()?)?;
    if *activity == Activity::Stopped && !processes.is_empty() {
        bail!("Claude started after observation; retry");
    }
    Ok(ShutdownPlan { processes })
}
fn remaining_targets(
    original: &[Process],
    snapshot: &[Process],
    targets: &[Process],
) -> Result<Vec<Process>> {
    for current in snapshot
        .iter()
        .filter(|p| original.iter().any(|o| o.pid == p.pid))
    {
        let approved = original.iter().find(|o| o.pid == current.pid).unwrap();
        let mut changed = Vec::new();
        if approved.executable != current.executable {
            changed.push("executable");
        }
        if approved.started != current.started {
            changed.push("birth");
        }
        if !changed.is_empty() {
            bail!(
                "Claude process identity changed (PID {}, {}); no further termination requests sent",
                current.pid,
                changed.join(", ")
            );
        }
    }
    let mut related: BTreeSet<_> = targets
        .iter()
        .map(|p| p.pid)
        .chain(
            snapshot
                .iter()
                .filter(|p| original.iter().any(|o| o.pid == p.pid))
                .map(|p| p.pid),
        )
        .collect();
    loop {
        let before = related.len();
        for p in snapshot {
            if related.contains(&p.ppid) {
                related.insert(p.pid);
            }
        }
        if related.len() == before {
            break;
        }
    }
    if related
        .iter()
        .any(|pid| !original.iter().any(|o| o.pid == *pid))
    {
        bail!("New Claude process appeared; no further termination requests sent");
    }
    // Include confirmed descendants even when their parents have exited and they are reparented.
    Ok(snapshot
        .iter()
        .filter(|p| original.iter().any(|o| o.pid == p.pid))
        .cloned()
        .collect())
}
fn recheck_with(
    original: &[Process],
    snapshot: &[Process],
    self_pid: i32,
    known: impl Fn(&Process) -> bool,
    uncertain_cli: impl Fn(&Process) -> bool,
) -> Result<Vec<Process>> {
    // Retain only the exact confirmed identities, even after their parents exit.
    // Validate them before granting ownership to any reparented survivor.
    let survivors = remaining_targets(original, snapshot, &[])?;
    let targets = target_processes_with(
        snapshot,
        self_pid,
        |p| known(p) || survivors.iter().any(|approved| approved.pid == p.pid),
        uncertain_cli,
    )?;
    remaining_targets(original, snapshot, &targets)
}
fn recheck(plan: &ShutdownPlan) -> Result<Vec<Process>> {
    recheck_with(
        &plan.processes,
        &platform::snapshot()?,
        std::process::id() as i32,
        platform::known_current,
        uncertain_cli,
    )
}
fn stop_with(
    mut observe: impl FnMut() -> Result<Vec<Process>>,
    mut terminate: impl FnMut(&Process, bool) -> Result<()>,
    grace: Duration,
) -> Result<()> {
    for force in [false, true] {
        let remaining = observe()?;
        for process in remaining.iter().rev() {
            let current = observe()?;
            if current.iter().any(|p| p.pid == process.pid) {
                terminate(process, force)?;
            }
        }
        let deadline = Instant::now() + grace;
        loop {
            if observe()?.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    bail!("Confirmed Claude processes did not exit; no history changed")
}
pub fn close_confirmed(plan: &ShutdownPlan, data_root: &Path, cli_root: &Path) -> Result<()> {
    supported()?;
    // Never add targets after confirmation. Recheck before each request, including force.
    stop_with(
        || recheck(plan),
        platform::terminate,
        Duration::from_secs(5),
    )?;
    if inspect(data_root, cli_root)? != Activity::Stopped {
        bail!("Claude or history activity appeared after shutdown; no history changed");
    }
    require_stopped()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_process(pid: i32, ppid: i32, executable: &str) -> Process {
        Process {
            pid,
            ppid,
            executable: executable.into(),
            args: vec![],
            started: "birth".into(),
        }
    }
    fn target_fixture() -> Vec<Process> {
        vec![
            fixture_process(1, 0, "init"),
            fixture_process(10, 1, "desktop"),
            fixture_process(
                11,
                10,
                "/Users/me/Library/Application Support/Claude/Claude Extensions/ext/.venv/bin/python",
            ),
            fixture_process(99, 1, "claudeHistory"),
        ]
    }
    fn fixture_known(p: &Process) -> bool {
        p.executable == "desktop"
    }
    #[test]
    fn target_selection_accepts_attributed_claude_path_descendants() {
        let targets =
            target_processes_with(&target_fixture(), 99, fixture_known, |_| false).unwrap();
        assert_eq!(
            targets.iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![10, 11]
        );
    }
    #[test]
    fn recheck_preserves_approved_claude_path_worker_after_parent_exits() {
        let initial = target_fixture();
        let original = initial
            .iter()
            .filter(|p| p.pid == 10 || p.pid == 11)
            .cloned()
            .collect::<Vec<_>>();
        let mut snapshot = initial
            .into_iter()
            .filter(|p| p.pid != 10)
            .collect::<Vec<_>>();
        snapshot.iter_mut().find(|p| p.pid == 11).unwrap().ppid = 1;
        let remaining = recheck_with(&original, &snapshot, 99, fixture_known, |_| false).unwrap();
        assert_eq!(
            remaining.iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![11]
        );
    }
    #[test]
    fn target_selection_rejects_unowned_paths_and_uncertain_cli_roots() {
        for path in [
            "/tmp/Claude-worker",
            "/tmp/Claude.app/Contents/Helpers/disclaimer-fake",
            "/usr/bin/node",
        ] {
            let mut snapshot = target_fixture();
            snapshot.push(fixture_process(20, 1, path));
            let message = target_processes_with(&snapshot, 99, fixture_known, |p| {
                p.executable == "/usr/bin/node"
            })
            .unwrap_err()
            .to_string();
            if path.contains("Claude") {
                assert!(message.contains("PID 20"));
            }
            assert!(!message.contains(path));
        }
    }
    #[test]
    fn recheck_rejects_identity_changes_new_roots_descendants_and_caller_ancestry() {
        let initial = target_fixture();
        let original = initial
            .iter()
            .filter(|p| p.pid == 10 || p.pid == 11)
            .cloned()
            .collect::<Vec<_>>();
        for change in 0..7 {
            let mut snapshot = initial.clone();
            match change {
                0 => snapshot[2].started = "replacement birth".into(),
                1 => snapshot[2].executable = "/bin/replacement".into(),
                2 => snapshot.push(fixture_process(20, 1, "desktop")),
                3 => snapshot.push(fixture_process(20, 11, "/bin/new-worker")),
                4 => snapshot.push(fixture_process(20, 1, "/tmp/Claude-unowned")),
                5 => snapshot.last_mut().unwrap().ppid = 11,
                6 => snapshot[2].started.clear(),
                _ => unreachable!(),
            }
            assert!(
                recheck_with(&original, &snapshot, 99, fixture_known, |_| false).is_err(),
                "change {change}"
            );
        }
    }
    #[test]
    fn graceful_then_force_covers_all_confirmed_survivors() {
        use std::cell::RefCell;
        let alive = RefCell::new(vec![
            fixture_process(10, 1, "cli"),
            fixture_process(11, 10, "worker"),
        ]);
        let signals = RefCell::new(Vec::new());
        stop_with(
            || Ok(alive.borrow().clone()),
            |p, force| {
                signals.borrow_mut().push((p.pid, force));
                if force {
                    alive.borrow_mut().retain(|a| a.pid != p.pid);
                }
                Ok(())
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(
            *signals.borrow(),
            vec![(11, false), (10, false), (11, true), (10, true)]
        );
    }
    #[test]
    fn termination_rechecks_before_each_signal_and_aborts_on_change() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let signals = Cell::new(0);
        let result = stop_with(
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 3 {
                    bail!("new target appeared");
                }
                Ok(vec![
                    fixture_process(10, 1, "cli"),
                    fixture_process(11, 10, "worker"),
                ])
            },
            |_, _| {
                signals.set(signals.get() + 1);
                Ok(())
            },
            Duration::ZERO,
        );
        assert!(result.is_err());
        assert_eq!(signals.get(), 1);
    }
    #[test]
    fn graceful_exit_never_uses_force_and_timeout_never_succeeds() {
        use std::cell::Cell;
        let alive = Cell::new(true);
        stop_with(
            || {
                Ok(if alive.get() {
                    vec![fixture_process(10, 1, "cli")]
                } else {
                    vec![]
                })
            },
            |_, force| {
                assert!(!force);
                alive.set(false);
                Ok(())
            },
            Duration::ZERO,
        )
        .unwrap();
        assert!(
            stop_with(
                || Ok(vec![fixture_process(10, 1, "cli")]),
                |_, _| Ok(()),
                Duration::ZERO
            )
            .is_err()
        );
    }
    #[test]
    fn ambiguous_node_operand_blocks_but_never_grants_termination_ownership() {
        let mut process = fixture_process(10, 1, "/usr/bin/node");
        process.args = vec![
            "node".into(),
            "/tmp/worker".into(),
            "/tmp/@anthropic-ai/claude-code/cli.js".into(),
        ];
        assert!(node_cli(&process));
        assert!(!node_cli_owned(&process));
        process.args = vec![
            "node".into(),
            "/tmp/@anthropic-ai/claude-code/cli.js".into(),
        ];
        assert!(node_cli_owned(&process));
    }
    #[test]
    fn incomplete_duplicate_and_cyclic_caller_ancestry_fail_closed() {
        for processes in [
            vec![fixture_process(10, 0, "cli")],
            vec![
                fixture_process(99, 98, "claudeHistory"),
                fixture_process(10, 0, "cli"),
            ],
            vec![
                fixture_process(99, 98, "claudeHistory"),
                fixture_process(98, 99, "shell"),
            ],
            vec![
                fixture_process(99, 0, "claudeHistory"),
                fixture_process(99, 0, "shell"),
            ],
        ] {
            assert!(targets_with(&processes, 99, |p| p.executable == "cli").is_err());
        }
    }
    #[test]
    fn shutdown_targets_include_all_claude_trees_and_only_that_provider() {
        let procs = vec![
            fixture_process(1, 0, "init"),
            fixture_process(10, 1, "desktop"),
            fixture_process(11, 10, "helper"),
            fixture_process(12, 11, "python"),
            fixture_process(20, 1, "cli"),
            fixture_process(21, 20, "mcp"),
            fixture_process(30, 1, "codex"),
            fixture_process(31, 30, "node"),
            fixture_process(99, 1, "claudeHistory"),
        ];
        let targets = targets_with(&procs, 99, |p| {
            matches!(p.executable.as_str(), "desktop" | "helper" | "cli")
        })
        .unwrap();
        assert_eq!(
            targets.iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![10, 11, 12, 20, 21]
        );
        let mut inside = procs.clone();
        inside.last_mut().unwrap().ppid = 21;
        assert!(
            targets_with(&inside, 99, |p| p.executable == "cli")
                .unwrap_err()
                .to_string()
                .contains("terminal")
        );
    }
    #[test]
    fn new_descendant_of_reparented_approved_worker_blocks_shutdown() {
        let original = vec![
            fixture_process(10, 1, "claude"),
            fixture_process(11, 10, "worker"),
        ];
        let snapshot = vec![
            fixture_process(11, 1, "worker"),
            fixture_process(12, 11, "generic"),
        ];
        assert!(remaining_targets(&original, &snapshot, &[]).is_err());
    }
    #[test]
    fn shutdown_recheck_rejects_new_targets_and_reused_pids() {
        let original = vec![fixture_process(10, 1, "claude")];
        assert!(remaining_targets(&original, &original, &original).is_ok());
        let mut reused = original.clone();
        reused[0].started = "new birth".into();
        let message = remaining_targets(&original, &reused, &reused)
            .unwrap_err()
            .to_string();
        assert!(message.contains("PID 10, birth"));
        assert!(!message.contains("new birth"));
        let mut executed = original.clone();
        executed[0].executable = "/private/changed-executable".into();
        let message = remaining_targets(&original, &executed, &executed)
            .unwrap_err()
            .to_string();
        assert!(message.contains("PID 10, executable"));
        assert!(!message.contains("/private/"));
        let fresh = vec![fixture_process(20, 1, "claude")];
        assert!(remaining_targets(&original, &fresh, &fresh).is_err());
        assert!(remaining_targets(&original, &[], &[]).unwrap().is_empty());
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
