use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const MAX_VISIBLE_PROCESSES: usize = 500;

#[derive(Debug, Serialize)]
pub(crate) struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub state: String,
    pub resident_memory_bytes: Option<u64>,
    pub protected: bool,
    pub protection_reason: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ProcessSnapshot {
    pub processes: Vec<ProcessInfo>,
    pub truncated: bool,
}

pub(crate) fn list_current_user_processes() -> Result<ProcessSnapshot, String> {
    #[cfg(target_os = "linux")]
    {
        let proc_root = Path::new("/proc");
        let current_uid = read_uid(
            &fs::read_to_string(proc_root.join("self/status"))
                .map_err(|error| format!("Cannot inspect current user ID: {error}"))?,
        )?;
        let entries = fs::read_dir(proc_root)
            .map_err(|error| format!("Cannot enumerate Linux processes: {error}"))?;
        let protected_pids = current_process_ancestors(proc_root);
        let mut processes = Vec::new();

        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(mut process) = read_process(proc_root, pid, current_uid) {
                process.protection_reason = protection_reason(&process.name, pid, &protected_pids);
                process.protected = process.protection_reason.is_some();
                processes.push(process);
            }
        }

        processes.sort_by_key(|process| process.pid);
        let truncated = processes.len() > MAX_VISIBLE_PROCESSES;
        processes.truncate(MAX_VISIBLE_PROCESSES);
        Ok(ProcessSnapshot {
            processes,
            truncated,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err("Process inspection is currently supported on Linux only".into())
    }
}

pub(crate) fn terminate_current_user_process(pid: u32) -> Result<(), String> {
    if pid <= 1 || pid == std::process::id() {
        return Err("Refusing to terminate a protected process ID".into());
    }
    #[cfg(target_os = "linux")]
    {
        let proc_root = Path::new("/proc");
        let current_uid = read_uid(
            &fs::read_to_string(proc_root.join("self/status"))
                .map_err(|error| format!("Cannot inspect current user ID: {error}"))?,
        )?;
        let target = read_process(proc_root, pid, current_uid)
            .ok_or("Process is not running or is not owned by the current user")?;
        let protected_pids = current_process_ancestors(proc_root);
        if let Some(reason) = protection_reason(&target.name, pid, &protected_pids) {
            return Err(format!(
                "Refusing to stop protected process '{}': {reason}",
                target.name
            ));
        }
        let status = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map_err(|error| format!("Cannot send termination signal: {error}"))?;
        if !status.success() {
            return Err("The operating system rejected the termination request".into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Err("Process termination is currently supported on Linux only".into())
    }
}

fn read_uid(status: &str) -> Result<u32, String> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "Process status does not contain a valid user ID".into())
}

fn read_process(proc_root: &Path, pid: u32, current_uid: u32) -> Option<ProcessInfo> {
    let process_dir: PathBuf = proc_root.join(pid.to_string());
    let name = fs::read_to_string(process_dir.join("comm"))
        .ok()?
        .trim()
        .to_owned();
    let status = fs::read_to_string(process_dir.join("status")).ok()?;
    let uid = read_uid(&status).ok()?;
    if uid != current_uid {
        return None;
    }

    let parent_pid = status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))
        .and_then(|value| value.trim().parse().ok())?;
    let state = status
        .lines()
        .find_map(|line| line.strip_prefix("State:"))
        .map(str::trim)?
        .to_owned();
    let resident_memory_bytes = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|kilobytes| kilobytes.saturating_mul(1024));

    Some(ProcessInfo {
        pid,
        parent_pid,
        name,
        state,
        resident_memory_bytes,
        protected: false,
        protection_reason: None,
    })
}

// Never allow the process manager to terminate the desktop/login infrastructure
// or any process in the ancestry of this app. The latter protects the active
// graphical session even when its session manager has a distribution-specific name.
fn current_process_ancestors(proc_root: &Path) -> std::collections::HashSet<u32> {
    let mut protected = std::collections::HashSet::new();
    let mut pid = std::process::id();
    for _ in 0..64 {
        if pid <= 1 || !protected.insert(pid) {
            break;
        }
        let Ok(status) = fs::read_to_string(proc_root.join(pid.to_string()).join("status")) else {
            break;
        };
        let Some(parent) = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))
            .and_then(|v| v.trim().parse::<u32>().ok())
        else {
            break;
        };
        pid = parent;
    }
    protected.insert(1);
    protected
}

fn protection_reason(
    name: &str,
    pid: u32,
    ancestors: &std::collections::HashSet<u32>,
) -> Option<String> {
    if ancestors.contains(&pid) {
        return Some("required by the current app or its login-session ancestry".into());
    }
    let normalized = name.to_ascii_lowercase();
    let protected_names = [
        "systemd",
        "init",
        "login",
        "lightdm",
        "gdm",
        "gdm-session-wor",
        "sddm",
        "xdm",
        "cinnamon-session",
        "gnome-session",
        "xfce4-session",
        "mate-session",
        "lxsession",
        "ksmserver",
        "plasmashell",
        "cinnamon",
        "muffin",
        "xfwm4",
        "kwin_x11",
        "kwin_wayland",
        "Xorg",
        "Xwayland",
        "X",
        "dbus-daemon",
        "dbus-broker",
        "polkitd",
        "systemd-logind",
    ];
    if protected_names
        .iter()
        .any(|protected| normalized == protected.to_ascii_lowercase())
        || normalized == "app"
        || normalized.starts_with("sam-desktop")
    {
        return Some("critical desktop, login, or Desktop Commander infrastructure".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_only_processes_owned_by_requested_user() {
        let root = std::env::temp_dir().join(format!("sam-proc-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (pid, uid, name) in [(101, 1000, "owned-app"), (202, 1001, "other-app")] {
            let dir = root.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("comm"), name).unwrap();
            fs::write(
                dir.join("status"),
                format!("Name:\t{name}\nState:\tS (sleeping)\nPPid:\t1\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nVmRSS:\t12 kB\n"),
            )
            .unwrap();
        }

        let owned = read_process(&root, 101, 1000).expect("include current user's process");
        assert_eq!(owned.name, "owned-app");
        assert_eq!(owned.parent_pid, 1);
        assert_eq!(owned.resident_memory_bytes, Some(12 * 1024));
        assert!(!owned.protected);
        assert!(read_process(&root, 202, 1000).is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_user_process_snapshot_includes_test_runner() {
        let snapshot = list_current_user_processes().expect("read current user's processes");
        assert!(
            snapshot
                .processes
                .iter()
                .any(|process| process.pid == std::process::id()),
            "the current test process should be visible"
        );
        assert!(snapshot.processes.len() <= MAX_VISIBLE_PROCESSES);
    }

    #[test]
    fn terminates_a_child_owned_by_the_current_user() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("start test child process");
        terminate_current_user_process(child.id()).expect("terminate owned child");
        let status = child.wait().expect("reap test child");
        assert!(!status.success());
    }

    #[test]
    fn protects_desktop_and_login_processes() {
        let ancestors = std::collections::HashSet::from([10, 20, 1]);
        assert!(protection_reason("cinnamon-session", 99, &ancestors).is_some());
        assert!(protection_reason("Xorg", 98, &ancestors).is_some());
        assert!(protection_reason("app", 97, &ancestors).is_some());
        assert!(protection_reason("bash", 10, &ancestors).is_some());
        assert!(protection_reason("python", 101, &ancestors).is_none());
    }

    #[test]
    fn rejects_malformed_process_status() {
        assert!(read_uid("Name:\tmissing uid").is_err());
        assert!(read_uid("Uid:\tnope").is_err());
    }
}
