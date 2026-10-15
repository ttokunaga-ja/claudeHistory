use super::{Activity, Process, classify_with};
use anyhow::{Context, Result, bail};
use std::path::Path;
#[cfg(not(target_os = "windows"))]
use std::process::Command;
pub(super) fn parse(snapshot: &str) -> Result<Vec<Process>> {
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
                started: String::new(),
            })
        })
        .collect()
}
fn desktop(p: &Process) -> bool {
    let Some((_, tail)) = p.executable.split_once("/Claude.app/Contents/") else {
        return false;
    };
    tail == "MacOS/Claude"
        || tail == "Helpers/disclaimer"
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
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(super) fn main_desktop_current(p: &Process) -> bool {
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
    super::node_cli(p)
}
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn native_cli_owned(p: &Process) -> bool {
    let name = Path::new(&p.executable)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if name == "claude" {
        return true;
    }
    if let Some((_, version)) = p.executable.rsplit_once("/.local/share/claude/versions/") {
        let parts: Vec<_> = version.split('.').collect();
        return parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    }
    matches!(name, "node" | "nodejs") && super::node_cli_owned(p)
}
#[cfg(any(not(target_os = "windows"), test))]
fn parse_native_snapshot(snapshot: &str) -> Result<Vec<Process>> {
    let mut live = Vec::new();
    for row in snapshot.lines().filter(|row| !row.trim().is_empty()) {
        let (pid, rest) = row
            .trim()
            .split_once(char::is_whitespace)
            .context("invalid native process snapshot")?;
        let (ppid, rest) = rest
            .trim_start()
            .split_once(char::is_whitespace)
            .context("invalid native process snapshot")?;
        let (state, executable) = rest
            .trim_start()
            .split_once(char::is_whitespace)
            .context("missing native process state")?;
        let process = parse(&format!("{pid} {ppid} {}", executable.trim_start()))?.remove(0);
        // Native ps STAT=Z is positive proof that this process cannot execute or write.
        // Its command name can already be <defunct>; never signal it or treat that as a replacement.
        if !state.starts_with('Z') {
            live.push(process);
        }
    }
    Ok(live)
}
/// Fixture-only parser entry point, without invoking processes or signals.
#[doc(hidden)]
pub(super) fn classify_fixture(
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
    Ok(classify_with(
        &processes,
        self_pid as i32,
        desktop,
        main_desktop_current,
        cli,
    ))
}
#[doc(hidden)]
pub(super) fn classify_argv_fixture(
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
    Ok(classify_with(
        &processes,
        self_pid as i32,
        desktop,
        main_desktop_current,
        cli,
    ))
}

#[cfg(any(target_os = "macos", test))]
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
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_argv(_pid: i32) -> Result<Vec<String>> {
    bail!("unsupported platform");
}
#[cfg(target_os = "macos")]
fn live_process_started(pid: i32) -> Result<Option<String>> {
    #[repr(C)]
    #[derive(Default)]
    struct BsdInfo {
        fields: [u32; 12],
        comm: [u8; 16],
        name: [u8; 32],
        trailing: [u32; 6],
        start_seconds: u64,
        start_microseconds: u64,
    }
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut std::ffi::c_void,
            size: i32,
        ) -> i32;
    }
    let mut info = BsdInfo::default();
    let size = std::mem::size_of::<BsdInfo>() as i32;
    let read = unsafe { proc_pidinfo(pid, 3, 0, (&mut info as *mut BsdInfo).cast(), size) };
    if read != size {
        let error = std::io::Error::last_os_error();
        if read == 0 && error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(error).context("Process creation time observation unavailable");
    }
    // proc_bsdinfo.pbi_status: SZOMB is positive proof the process cannot run.
    if info.fields[1] == 5 {
        return Ok(None);
    }
    if info.start_seconds == 0 && info.start_microseconds == 0 {
        bail!("Process creation time observation unavailable");
    }
    Ok(Some(format!(
        "{}:{}",
        info.start_seconds, info.start_microseconds
    )))
}
#[cfg(all(target_os = "macos", test))]
fn process_started(pid: i32) -> Result<String> {
    live_process_started(pid)?.context("Process has exited")
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn live_process_started(_pid: i32) -> Result<Option<String>> {
    bail!("unsupported platform")
}
#[cfg(not(target_os = "windows"))]
pub(super) fn snapshot() -> Result<Vec<Process>> {
    super::supported()?;
    let out = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,stat=,comm="])
        .output()
        .context("cannot observe processes")?;
    if !out.status.success() {
        bail!("process observation failed");
    }
    enrich_processes(parse_native_snapshot(std::str::from_utf8(&out.stdout)?)?)
}
#[cfg(not(target_os = "windows"))]
fn enrich_processes(processes: Vec<Process>) -> Result<Vec<Process>> {
    let mut live = Vec::new();
    for mut p in processes {
        // Only positive exit evidence may remove an enumerated process. Permission
        // failures remain unreadable identities and block any selected target.
        p.started = match live_process_started(p.pid) {
            Ok(Some(started)) => started,
            Ok(None) => continue,
            Err(_) => String::new(),
        };
        let name = Path::new(&p.executable)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if name == "node" || name == "nodejs" {
            match native_argv(p.pid) {
                Ok(args) => p.args = args,
                Err(error) => {
                    if matches!(live_process_started(p.pid), Ok(None)) {
                        continue;
                    }
                    return Err(error);
                }
            }
        }
        match live_process_started(p.pid) {
            Ok(None) => continue,
            Ok(Some(started)) if !p.started.is_empty() && p.started != started => {
                bail!(
                    "Process identity changed during observation (PID {}, birth)",
                    p.pid
                );
            }
            Err(_) => p.started.clear(),
            Ok(Some(_)) => {}
        }
        live.push(p);
    }
    Ok(live)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn tcp(pids: &[i32]) -> Result<bool> {
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
#[cfg(any(not(target_os = "windows"), test))]
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

#[cfg(not(target_os = "windows"))]
pub(super) fn classify_current(processes: &[Process], self_pid: i32) -> Activity {
    classify_with(processes, self_pid, desktop, main_desktop_current, cli)
}
#[cfg(not(target_os = "windows"))]
pub(super) fn terminate(process: &Process, force: bool) -> Result<()> {
    let pid = process.pid;
    #[cfg(target_os = "macos")]
    {
        let Some(started) = live_process_started(pid)? else {
            return Ok(());
        };
        if process.started.is_empty() || started != process.started {
            bail!("Termination target creation time changed (PID {pid}, birth)");
        }
        if unsafe { libc::kill(pid, if force { libc::SIGKILL } else { libc::SIGTERM }) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error).context("Claude termination request failed");
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, force);
        bail!("unsupported platform")
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn known_current(p: &Process) -> bool {
    desktop(p) || native_cli_owned(p)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn redirected(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_disclaimer_is_recognized_without_its_parent() {
        let p = parse("10 1 /Applications/Claude.app/Contents/Helpers/disclaimer")
            .unwrap()
            .remove(0);
        assert!(desktop(&p));
        assert_eq!(
            classify_with(&[p], 99, desktop, main_desktop_current, cli),
            Activity::Unknown {
                reasons: vec!["Desktop helpers exist without the main app".into()]
            }
        );
        for path in [
            "/Applications/Claude.app/Contents/Helpers/disclaimer-fake",
            "/Applications/Claude.app/Contents/Helpers/nested/disclaimer",
            "/tmp/Claude Extensions/disclaimer",
        ] {
            assert!(!desktop(&parse(&format!("10 1 {path}")).unwrap().remove(0)));
        }
    }
    #[test]
    fn native_cli_ownership_accepts_only_exact_version_leaf() {
        for (path, owned) in [
            ("/Users/me/.local/share/claude/versions/2.1.296", true),
            (
                "/Users/me/.local/share/claude/versions/2.1.296/worker",
                false,
            ),
            ("/Users/me/.local/share/claude/versions/backup", false),
            (
                "/Users/me/.local/share/claude/versions/2.1.296-backup",
                false,
            ),
        ] {
            let process = parse(&format!("10 1 {path}")).unwrap().remove(0);
            assert_eq!(native_cli_owned(&process), owned);
            assert!(cli(&process)); // Uncertain version paths remain conservative blockers.
        }
    }
    #[test]
    fn native_snapshot_filters_only_positively_observed_zombies() {
        let processes =
            parse_native_snapshot("10 1 Z <defunct>\n11 1 S <defunct>\n12 1 R /bin/zsh\n").unwrap();
        assert_eq!(
            processes.iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![11, 12]
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_exited_unreaped_child_is_not_a_live_target() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let observed = (|| -> Result<()> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let output = std::process::Command::new("/bin/ps")
                    .args(["-p", &pid.to_string(), "-o", "stat="])
                    .output()?;
                if String::from_utf8(output.stdout)?.trim().starts_with('Z') {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    bail!("Fixture did not become an observed zombie");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(!snapshot()?.iter().any(|p| p.pid == pid));
            Ok(())
        })();
        child.wait().unwrap();
        observed.unwrap();
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_termination_checks_birth_and_escalates_disposable_child() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Child, Stdio};
        struct Fixture(Child);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let ready_path = directory.path().join("ready");
        let mut child = Fixture(
            Command::new("/bin/sh")
                .args([
                    "-c",
                    "trap '' TERM; printf ready > \"$1\"; while :; do read line; done",
                    "fixture",
                ])
                .arg(&ready_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !ready_path.is_file() {
            assert!(child.0.try_wait().unwrap().is_none());
            assert!(
                std::time::Instant::now() < deadline,
                "fixture readiness timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let pid = child.0.id() as i32;
        let mut process = parse(&format!("{pid} 1 /bin/sh")).unwrap().remove(0);
        process.started = "wrong birth".into();
        assert!(terminate(&process, false).is_err());
        assert!(child.0.try_wait().unwrap().is_none());
        process.started = process_started(pid).unwrap();
        terminate(&process, false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(child.0.try_wait().unwrap().is_none());
        terminate(&process, true).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert_eq!(status.signal(), Some(libc::SIGKILL));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fixture exit timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn child_exiting_after_enumeration_is_not_a_changed_identity() {
        let mut child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let pid = child.id() as i32;
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "pid=,ppid=,stat=,comm="])
            .output()
            .unwrap();
        let enumerated =
            parse_native_snapshot(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
        assert_eq!(enumerated.len(), 1);
        let mut approved = enumerated[0].clone();
        approved.started = process_started(pid).unwrap();
        child.kill().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let output = Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()
                .unwrap();
            if String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .starts_with('Z')
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fixture zombie timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let zombie_excluded = enrich_processes(enumerated.clone()).unwrap().is_empty();
        let zombie_termination = terminate(&approved, false);
        child.wait().unwrap();
        assert!(zombie_excluded);
        zombie_termination.unwrap();
        assert!(enrich_processes(enumerated).unwrap().is_empty());
        terminate(&approved, false).unwrap();
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_confirmed_tree_shutdown_handles_exits_and_escalation() {
        use std::cell::{Cell, RefCell};
        use std::time::{Duration, Instant};
        struct Tree {
            root: std::process::Child,
            approved: Vec<Process>,
        }
        impl Drop for Tree {
            fn drop(&mut self) {
                for process in &self.approved {
                    let _ = terminate(process, true);
                }
                let _ = self.root.kill();
                let _ = self.root.wait();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let script = r#"trap 'exit 0' TERM
/bin/sh -c 'trap "" TERM; printf ready > "$1"; while [ "$SECONDS" -lt 10 ]; do :; done' fixture "$1/force" &
force=$!
/bin/sleep 10 &
normal=$!
/bin/sh -c 'while [ ! -e "$1" ] && [ "$SECONDS" -lt 10 ]; do :; done' fixture "$1/exit" &
spontaneous=$!
printf '%s %s %s %s' "$$" "$force" "$normal" "$spontaneous" > "$1/pids"
wait"#;
        let mut tree = Tree {
            root: Command::new("/bin/sh")
                .args(["-c", script, "fixture"])
                .arg(directory.path())
                .spawn()
                .unwrap(),
            approved: Vec::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        while !directory.path().join("force").is_file() || !directory.path().join("pids").is_file()
        {
            assert!(Instant::now() < deadline, "tree readiness timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        let pids: Vec<i32> = std::fs::read_to_string(directory.path().join("pids"))
            .unwrap()
            .split_whitespace()
            .map(|p| p.parse().unwrap())
            .collect();
        assert_eq!(pids.len(), 4);
        let ids = pids
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let observe = || {
            let output = Command::new("/bin/ps")
                .args(["-p", &ids, "-o", "pid=,ppid=,stat=,comm="])
                .output()?;
            anyhow::ensure!(
                output.status.success()
                    || (output.status.code() == Some(1) && output.stdout.is_empty()),
                "fixture ps failed"
            );
            enrich_processes(parse_native_snapshot(std::str::from_utf8(&output.stdout)?)?)
        };
        tree.approved = observe().unwrap();
        assert_eq!(tree.approved.len(), 4);
        let first = Cell::new(true);
        let signals = RefCell::new(Vec::new());
        super::super::stop_with(
            || {
                if first.replace(false) {
                    std::fs::write(directory.path().join("exit"), "exit")?;
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while live_process_started(pids[3])?.is_some() {
                        anyhow::ensure!(
                            Instant::now() < deadline,
                            "fixture spontaneous exit timed out"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                let mut snapshot = observe()?;
                snapshot.push(Process {
                    pid: -99,
                    ppid: 0,
                    executable: "test harness".into(),
                    args: Vec::new(),
                    started: "harness".into(),
                });
                super::super::recheck_with(
                    &tree.approved,
                    &snapshot,
                    -99,
                    |p| p.pid == pids[0],
                    |_| false,
                )
            },
            |process, force| {
                signals.borrow_mut().push((process.pid, force));
                terminate(process, force)
            },
            Duration::from_millis(100),
        )
        .unwrap();
        let signals = signals.borrow();
        assert!(signals.contains(&(pids[0], false)));
        assert!(signals.contains(&(pids[1], false)));
        assert!(signals.contains(&(pids[1], true)));
        assert!(signals.contains(&(pids[2], false)));
        assert!(!signals.iter().any(|(pid, _)| *pid == pids[3]));
        assert!(observe().unwrap().is_empty());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_creation_identity_is_stable_and_nonempty() {
        let pid = std::process::id() as i32;
        let first = process_started(pid).unwrap();
        assert!(!first.is_empty());
        assert_ne!(first, "0:0");
        assert_eq!(first, process_started(pid).unwrap());
        let processes = snapshot().unwrap();
        assert_eq!(
            processes.iter().find(|p| p.pid == pid).unwrap().started,
            first
        );
    }
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
}
