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
#[cfg(not(target_os = "windows"))]
pub(super) fn snapshot() -> Result<Vec<Process>> {
    super::supported()?;
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
pub(super) fn close(pid: i32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("normal Desktop termination failed");
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        bail!("unsupported platform")
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn redirected(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
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
}
