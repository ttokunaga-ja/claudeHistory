//! Native Windows observations. Captured command lines stay in memory and never enter errors.
use super::{Activity, Process, classify_with};
use anyhow::{Result, bail};
use serde::Deserialize;

#[derive(Deserialize)]
struct Row {
    pid: i32,
    ppid: i32,
    path: Option<String>,
    name: String,
    command: Option<String>,
}
fn parse(snapshot: &str) -> Result<Vec<Process>> {
    let rows: Vec<Row> = serde_json::from_str(snapshot)?;
    rows.into_iter()
        .map(|row| {
            if row.pid <= 0 || row.ppid < 0 || row.name.is_empty() {
                bail!("invalid Windows process snapshot");
            }
            let executable = row.path.filter(|s| !s.is_empty()).unwrap_or(row.name);
            let name = basename(&executable);
            let args = if matches!(name.as_str(), "node.exe" | "nodejs.exe" | "claude.exe") {
                match row.command {
                    Some(command) if !command.is_empty() => argv(&command)?,
                    _ if name == "claude.exe" => Vec::new(),
                    _ => bail!("Windows Node argv observation unavailable"),
                }
            } else {
                Vec::new()
            };
            Ok(Process {
                pid: row.pid,
                ppid: row.ppid,
                executable,
                args,
            })
        })
        .collect()
}
fn normalized(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}
fn basename(path: &str) -> String {
    normalized(path).rsplit('/').next().unwrap_or("").to_owned()
}
fn official(p: &Process, local_app_data: &str) -> bool {
    let path = normalized(&p.executable);
    let root = normalized(local_app_data);
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        return false;
    }
    // Electron helper processes use the app executable itself. Native CLI binaries
    // below resources/ are work, even when bundled underneath a Desktop install.
    let app_executable = |relative: &str| {
        let mut parts = relative.split('/');
        let first = parts.next().unwrap_or("");
        if parts.clone().next().is_none() {
            return !first.is_empty();
        }
        (first == "app" || first.starts_with("app-"))
            && parts.next().is_some_and(|name| !name.is_empty())
            && parts.next().is_none()
    };
    let local = [
        format!("{root}/anthropicclaude/"),
        format!("{root}/programs/claude/"),
    ]
    .iter()
    .any(|prefix| path.strip_prefix(prefix).is_some_and(app_executable));
    let store = path
        .split_once("/windowsapps/")
        .is_some_and(|(prefix, tail)| {
            prefix.split_once(":/").is_some_and(|(drive, directory)| {
                drive.len() == 1
                    && drive.as_bytes()[0].is_ascii_alphabetic()
                    && matches!(directory, "program files" | "program files (x86)")
            }) && tail.split_once('/').is_some_and(|(package, relative)| {
                (package.starts_with("claude_") || package.starts_with("anthropic.claude_"))
                    && package.ends_with("__pzs8sxrjxfjjc")
                    && app_executable(relative)
            })
        });
    (local || store) && !path.split('/').any(|part| matches!(part, "." | ".."))
}
fn desktop(p: &Process, root: &str) -> bool {
    official(p, root)
        && matches!(
            basename(&p.executable).as_str(),
            "claude.exe" | "chrome_crashpad_handler.exe" | "crashpad_handler.exe"
        )
}
pub(super) fn main_desktop(p: &Process, root: &str) -> bool {
    desktop(p, root)
        && basename(&p.executable) == "claude.exe"
        && !p
            .args
            .iter()
            .skip(1)
            .any(|a| a.starts_with("--type=") || a == "--type")
}
fn cli(p: &Process, root: &str) -> bool {
    if basename(&p.executable) == "claude.exe" {
        return !desktop(p, root);
    }
    if !matches!(basename(&p.executable).as_str(), "node.exe" | "nodejs.exe") {
        return false;
    }
    let mut native = p.clone();
    native.executable = "/node".into();
    native.args = native.args.iter().map(|a| a.replace('\\', "/")).collect();
    super::node_cli(&native)
}
pub(super) fn classify(processes: &[Process], root: &str, self_pid: i32) -> Activity {
    // A Claude.exe from an official directory with no readable command line could be a
    // helper or the main app; it cannot be accepted as safely idle.
    if processes.iter().any(|p| {
        p.pid != self_pid
            && desktop(p, root)
            && basename(&p.executable) == "claude.exe"
            && p.args.is_empty()
    }) {
        return super::unknown();
    }
    classify_with(
        processes,
        self_pid,
        |p| desktop(p, root),
        |p| main_desktop(p, root),
        |p| cli(p, root),
    )
}
pub(super) fn classify_fixture(snapshot: &str, root: &str, self_pid: u32) -> Result<Activity> {
    match parse(snapshot) {
        Ok(processes) => Ok(classify(&processes, root, self_pid as i32)),
        Err(_) if serde_json::from_str::<Vec<Row>>(snapshot).is_ok() => Ok(super::unknown()),
        Err(error) => Err(error),
    }
}

/// The Microsoft CRT quoting rules: runs of backslashes before quotes determine
/// whether a quote is escaped or changes quote state. No shell expansion occurs.
pub(super) fn argv(command: &str) -> Result<Vec<String>> {
    let chars: Vec<char> = command.chars().collect();
    let mut cursor = 0;
    let mut result = Vec::new();
    while cursor < chars.len() {
        while cursor < chars.len() && matches!(chars[cursor], ' ' | '\t') {
            cursor += 1;
        }
        if cursor == chars.len() {
            break;
        }
        let mut arg = String::new();
        let mut quoted = false;
        while cursor < chars.len() {
            if !quoted && matches!(chars[cursor], ' ' | '\t') {
                break;
            }
            let start = cursor;
            while cursor < chars.len() && chars[cursor] == '\\' {
                cursor += 1;
            }
            let slashes = cursor - start;
            if cursor < chars.len() && chars[cursor] == '"' {
                arg.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 1 {
                    arg.push('"');
                    cursor += 1;
                } else if quoted && chars.get(cursor + 1) == Some(&'"') {
                    arg.push('"');
                    cursor += 2;
                } else {
                    quoted = !quoted;
                    cursor += 1;
                }
            } else {
                arg.extend(std::iter::repeat_n('\\', slashes));
                if cursor < chars.len() {
                    if !quoted && matches!(chars[cursor], ' ' | '\t') {
                        break;
                    }
                    arg.push(chars[cursor]);
                    cursor += 1;
                }
            }
        }
        if quoted {
            bail!("Windows command line has unmatched quotes");
        }
        result.push(arg);
    }
    if result.is_empty() {
        bail!("Windows command line is empty");
    }
    Ok(result)
}

#[derive(Deserialize)]
struct Connection {
    pid: i32,
    state: String,
}
pub(super) fn tcp_output(snapshot: &str, pids: &[i32]) -> Result<bool> {
    let rows: Vec<Connection> = serde_json::from_str(snapshot)?;
    for row in rows {
        if row.pid < 0 || row.state.is_empty() {
            bail!("invalid Windows TCP snapshot");
        }
        if pids.contains(&row.pid) && row.state != "Listen" {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "windows")]
fn powershell(script: &str) -> Result<String> {
    use anyhow::Context;
    use std::os::windows::process::CommandExt;
    // Use the OS binary rather than a PATH-resolved executable.
    let system_root =
        std::env::var_os("SystemRoot").context("Windows system directory unavailable")?;
    let exe = std::path::PathBuf::from(system_root)
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let output = std::process::Command::new(exe)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .output()
        .context("Windows activity observation unavailable")?;
    if !output.status.success() {
        bail!("Windows activity observation failed");
    }
    String::from_utf8(output.stdout).context("Windows activity snapshot encoding unavailable")
}
#[cfg(target_os = "windows")]
pub(super) fn snapshot() -> Result<Vec<Process>> {
    let json = powershell(
        r#"$ErrorActionPreference='Stop'; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $rows=@(Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -gt 0 } | ForEach-Object { @{pid=[int]$_.ProcessId;ppid=[int]$_.ParentProcessId;path=$_.ExecutablePath;name=$_.Name;command=$_.CommandLine} }); ConvertTo-Json -InputObject $rows -Compress -Depth 3"#,
    )?;
    parse(&json)
}
#[cfg(target_os = "windows")]
pub(super) fn tcp(pids: &[i32]) -> Result<bool> {
    let json = powershell(
        r#"$ErrorActionPreference='Stop'; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $rows=@(Get-NetTCPConnection | ForEach-Object { @{pid=[int]$_.OwningProcess;state=$_.State.ToString()} }); ConvertTo-Json -InputObject $rows -Compress -Depth 3"#,
    )?;
    tcp_output(&json, pids)
}

#[cfg(target_os = "windows")]
pub(super) fn close(pid: i32) -> Result<()> {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    struct Windows {
        pid: u32,
        handles: Vec<Hwnd>,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(callback: unsafe extern "system" fn(Hwnd, isize) -> i32, data: isize)
        -> i32;
        fn GetWindowThreadProcessId(window: Hwnd, pid: *mut u32) -> u32;
        fn IsWindowVisible(window: Hwnd) -> i32;
    }
    unsafe extern "system" fn collect(window: Hwnd, data: isize) -> i32 {
        let windows = unsafe { &mut *(data as *mut Windows) };
        let mut owner = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut owner);
        }
        if owner == windows.pid && unsafe { IsWindowVisible(window) } != 0 {
            windows.handles.push(window);
        }
        1
    }
    let mut windows = Windows {
        pid: pid as u32,
        handles: Vec::new(),
    };
    if unsafe { EnumWindows(collect, &mut windows as *mut Windows as isize) } == 0
        || windows.handles.is_empty()
    {
        bail!("Desktop window observation unavailable; close Claude manually");
    }
    for window in windows.handles {
        post_close(window, pid)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn post_close(window: *mut std::ffi::c_void, pid: i32) -> Result<()> {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowThreadProcessId(window: *mut std::ffi::c_void, pid: *mut u32) -> u32;
        fn PostMessageW(
            window: *mut std::ffi::c_void,
            message: u32,
            wparam: usize,
            lparam: isize,
        ) -> i32;
    }
    // Recheck HWND ownership immediately before WM_CLOSE. Never TerminateProcess.
    let mut owner = 0;
    if pid <= 0
        || unsafe { GetWindowThreadProcessId(window, &mut owner) } == 0
        || owner != pid as u32
    {
        bail!("Desktop window identity changed; close Claude manually");
    }
    if unsafe { PostMessageW(window, 0x0010, 0, 0) } == 0 {
        bail!("Desktop graceful close failed; close Claude manually");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub(super) fn classify_current(processes: &[Process], self_pid: i32) -> Activity {
    let Ok(root) = std::env::var("LOCALAPPDATA") else {
        return super::unknown();
    };
    classify(processes, &root, self_pid)
}
#[cfg(target_os = "windows")]
pub(super) fn main_desktop_current(p: &Process) -> bool {
    std::env::var("LOCALAPPDATA").is_ok_and(|root| main_desktop(p, &root))
}

#[cfg(target_os = "windows")]
pub(super) fn redirected(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(all(test, target_os = "windows"))]
mod native_tests {
    use super::*;
    #[test]
    fn native_process_and_tcp_observation_are_available() {
        let rows = snapshot().unwrap();
        assert!(rows.iter().any(|p| p.pid == std::process::id() as i32));
        // Parse the actual native connection table, including the no-owned-sockets case.
        tcp(&[std::process::id() as i32]).unwrap();
        assert!(close(-1).is_err());
    }
    #[test]
    fn graceful_close_posts_wm_close_to_owned_fixture_window() {
        use std::ffi::c_void;
        type Hwnd = *mut c_void;
        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }
        #[repr(C)]
        struct Message {
            window: Hwnd,
            message: u32,
            wparam: usize,
            lparam: isize,
            time: u32,
            point: Point,
            private: u32,
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn CreateWindowExW(
                ex: u32,
                class: *const u16,
                title: *const u16,
                style: u32,
                x: i32,
                y: i32,
                width: i32,
                height: i32,
                parent: Hwnd,
                menu: Hwnd,
                instance: Hwnd,
                param: Hwnd,
            ) -> Hwnd;
            fn DestroyWindow(window: Hwnd) -> i32;
            fn IsWindow(window: Hwnd) -> i32;
            fn PeekMessageW(
                message: *mut Message,
                window: Hwnd,
                min: u32,
                max: u32,
                remove: u32,
            ) -> i32;
            fn DispatchMessageW(message: *const Message) -> isize;
        }
        struct Fixture(Hwnd);
        impl Drop for Fixture {
            fn drop(&mut self) {
                unsafe {
                    DestroyWindow(self.0);
                }
            }
        }
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let title: Vec<u16> = "claudeHistory WM_CLOSE test fixture\0"
            .encode_utf16()
            .collect();
        // A message-only window is deliberately invisible to EnumWindows (including
        // on an SSH window station). Directly exercise the same WM_CLOSE primitive
        // used after production enumeration, and assert discovery fails closed.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                1,
                1,
                -3isize as Hwnd, // HWND_MESSAGE
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert!(!window.is_null(), "fixture window creation failed");
        let fixture = Fixture(window);
        assert!(close(std::process::id() as i32).is_err());
        assert!(post_close(window, -1).is_err());
        post_close(window, std::process::id() as i32).unwrap();
        let mut message: Message = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe { PeekMessageW(&mut message, window, 0x0010, 0x0010, 1) },
            0
        );
        assert_eq!(message.message, 0x0010);
        unsafe {
            DispatchMessageW(&message);
        }
        assert_eq!(unsafe { IsWindow(window) }, 0);
        drop(fixture);
    }
}
