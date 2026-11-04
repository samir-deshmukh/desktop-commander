use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_VISIBLE_PROCESSES: usize = 500;

#[derive(Debug, Serialize)]
pub(crate) struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub state: String,
    pub resident_memory_bytes: Option<u64>,
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
        let mut processes = Vec::new();

        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(process) = read_process(proc_root, pid, current_uid) {
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
    })
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
    fn rejects_malformed_process_status() {
        assert!(read_uid("Name:\tmissing uid").is_err());
        assert!(read_uid("Uid:\tnope").is_err());
    }
}
