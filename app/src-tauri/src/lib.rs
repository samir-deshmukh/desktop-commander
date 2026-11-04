#[cfg(feature = "desktop")]
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
#[cfg(feature = "desktop")]
use std::{collections::HashMap, io::Write, thread, time::Duration};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};
#[cfg(feature = "desktop")]
use tauri::{Emitter, Manager, State};

#[cfg(any(feature = "desktop", feature = "mcp"))]
mod audit;
#[cfg(feature = "mcp")]
mod mcp;
#[cfg(any(feature = "desktop", feature = "mcp"))]
mod processes;
#[cfg(feature = "mcp")]
pub use mcp::run_stdio_server;

#[cfg(feature = "desktop")]
struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

#[cfg(feature = "desktop")]
impl Drop for PtySession {
    fn drop(&mut self) {
        // Reap the shell when a session is explicitly closed or app state is dropped.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(feature = "desktop")]
const MAX_TERMINAL_SESSIONS: usize = 3;
// Pace PTY output to prevent an output-heavy command from flooding Tauri events.
// The fixed read buffer and short sleep keep application-side buffering bounded.
#[cfg(feature = "desktop")]
const TERMINAL_OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
#[cfg(feature = "desktop")]
const TERMINAL_OUTPUT_INTERVAL: Duration = Duration::from_millis(40);

#[cfg(feature = "desktop")]
#[derive(Default)]
struct TerminalSessions(Mutex<HashMap<String, PtySession>>);

struct AccessPolicy {
    approved_roots: Mutex<Vec<PathBuf>>,
    writable_roots: Mutex<Vec<PathBuf>>,
    command_execution_enabled: Mutex<bool>,
    storage_file: PathBuf,
}

#[cfg(feature = "desktop")]
#[derive(Serialize)]
struct McpServerInfo {
    command: String,
    binary_exists: bool,
    transport: &'static str,
    tools: Vec<&'static str>,
}

#[cfg(feature = "desktop")]
#[derive(Serialize)]
struct FolderEntry {
    name: String,
    path: String,
    is_dir: bool,
}

#[derive(Serialize, Deserialize)]
struct SavedApprovals {
    version: u32,
    folders: Vec<String>,
    #[serde(default)]
    writable_folders: Vec<String>,
    #[serde(default)]
    command_execution_enabled: bool,
}

impl AccessPolicy {
    fn load(storage_file: PathBuf) -> Result<Self, String> {
        let policy = Self {
            approved_roots: Mutex::new(Vec::new()),
            writable_roots: Mutex::new(Vec::new()),
            command_execution_enabled: Mutex::new(false),
            storage_file,
        };
        policy.refresh()?;
        Ok(policy)
    }

    // MCP runs in a separate process from the desktop UI. Reload the shared
    // permission file before every tool call so revocations take effect immediately.
    fn refresh(&self) -> Result<(), String> {
        let loaded = (|| -> Result<(Vec<PathBuf>, Vec<PathBuf>, bool), String> {
            if !self.storage_file.exists() {
                return Ok((Vec::new(), Vec::new(), false));
            }
            let contents = fs::read_to_string(&self.storage_file)
                .map_err(|e| format!("Cannot read saved folder permissions: {e}"))?;
            let saved: SavedApprovals = serde_json::from_str(&contents)
                .map_err(|e| format!("Saved folder permissions are invalid: {e}"))?;
            if saved.version != 1 {
                return Err("Saved folder permissions use an unsupported version".into());
            }

            let command_execution_enabled = saved.command_execution_enabled;
            let mut valid = Vec::new();
            for folder in saved.folders {
                if let Ok(path) = canonical_directory(&folder) {
                    if !valid.contains(&path) {
                        valid.push(path);
                    }
                }
            }
            valid.sort();

            let mut writable = Vec::new();
            for folder in saved.writable_folders {
                if let Ok(path) = canonical_directory(&folder) {
                    if valid.contains(&path) && !writable.contains(&path) {
                        writable.push(path);
                    }
                }
            }
            writable.sort();
            Ok((valid, writable, command_execution_enabled))
        })();

        let mut roots = self
            .approved_roots
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        let mut writable = self
            .writable_roots
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        let mut command_enabled = self
            .command_execution_enabled
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        match loaded {
            Ok((valid, writable_valid, command_valid)) => {
                *roots = valid;
                *writable = writable_valid;
                *command_enabled = command_valid;
                Ok(())
            }
            Err(error) => {
                roots.clear();
                writable.clear();
                *command_enabled = false;
                Err(error)
            }
        }
    }

    #[cfg(feature = "mcp")]
    fn command_execution_allowed(&self) -> Result<bool, String> {
        self.refresh()?;
        self.command_execution_enabled
            .lock()
            .map(|enabled| *enabled)
            .map_err(|_| "Permission state unavailable".into())
    }

    fn audit_path(&self) -> PathBuf {
        self.storage_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("activity.jsonl")
    }

    fn authorize_path(&self, path: &Path) -> Result<PathBuf, String> {
        let metadata =
            fs::symlink_metadata(path).map_err(|error| format!("Cannot inspect path: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("Access denied: symbolic links cannot be accessed directly".into());
        }
        let target = path
            .canonicalize()
            .map_err(|error| format!("Cannot resolve path: {error}"))?;
        let roots = self
            .approved_roots
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        if !roots.iter().any(|root| target.starts_with(root)) {
            return Err("Access denied: path is outside every approved folder".into());
        }
        Ok(target)
    }

    #[cfg(any(feature = "desktop", feature = "mcp"))]
    fn authorize_write_target(&self, path: &Path) -> Result<PathBuf, String> {
        self.refresh()?;
        let parent = path
            .parent()
            .ok_or("Write target must have a parent directory")?;
        let name = path
            .file_name()
            .ok_or("Write target must have a file or directory name")?;
        if name == "." || name == ".." {
            return Err("Invalid write target name".into());
        }
        let canonical_parent = parent
            .canonicalize()
            .map_err(|error| format!("Cannot resolve write target parent: {error}"))?;
        if !canonical_parent.is_dir() {
            return Err("Write target parent is not a directory".into());
        }
        let writable = self
            .writable_roots
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        if !writable
            .iter()
            .any(|root| canonical_parent.starts_with(root))
        {
            return Err(
                "Write denied: parent directory is not explicitly approved for changes".into(),
            );
        }
        let target = canonical_parent.join(name);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err("Write denied: symbolic links cannot be modified".into())
            }
            Ok(metadata) if metadata.is_dir() => Err("Write target is a directory".into()),
            Ok(_) => Ok(target),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(target),
            Err(error) => Err(format!("Cannot inspect write target: {error}")),
        }
    }

    #[cfg(feature = "desktop")]
    fn persist(&self, roots: &[PathBuf]) -> Result<(), String> {
        let writable = self
            .writable_roots
            .lock()
            .map_err(|_| "Permission state unavailable")?
            .clone();
        let command_enabled = *self
            .command_execution_enabled
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        self.persist_with(roots, &writable, command_enabled)
    }

    #[cfg(feature = "desktop")]
    fn persist_with(
        &self,
        roots: &[PathBuf],
        writable: &[PathBuf],
        command_execution_enabled: bool,
    ) -> Result<(), String> {
        let saved = SavedApprovals {
            version: 1,
            folders: roots
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            writable_folders: writable
                .iter()
                .filter(|path| roots.contains(path))
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            command_execution_enabled,
        };
        let contents = serde_json::to_vec_pretty(&saved)
            .map_err(|e| format!("Cannot serialize folder permissions: {e}"))?;
        let temporary = self.storage_file.with_extension("json.tmp");
        fs::write(&temporary, contents)
            .map_err(|e| format!("Cannot save folder permissions: {e}"))?;
        fs::rename(&temporary, &self.storage_file)
            .map_err(|e| format!("Cannot finalize saved permissions: {e}"))
    }
}

#[cfg(feature = "desktop")]
fn record_desktop_activity(
    policy: &AccessPolicy,
    action: &str,
    target: Option<&str>,
    success: bool,
) {
    let _ = audit::append_event(
        &policy.audit_path(),
        "Desktop",
        action,
        target,
        if success {
            "success"
        } else {
            "denied_or_failed"
        },
    );
}

const MAX_MUTATION_BYTES: usize = 1024 * 1024;

fn write_approved_text_file(
    policy: &AccessPolicy,
    path: &Path,
    content: &str,
    overwrite: bool,
) -> Result<(), String> {
    if content.len() > MAX_MUTATION_BYTES {
        return Err(format!(
            "Text content exceeds the {MAX_MUTATION_BYTES}-byte limit"
        ));
    }
    let target = policy.authorize_write_target(path)?;
    let exists = target.exists();
    if exists && !overwrite {
        return Err("File already exists; overwrite permission is required".into());
    }
    if exists {
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| format!("Cannot inspect existing file: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("Only regular files can be overwritten".into());
        }
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary =
        target.with_file_name(format!(".sam-commander-{}-{nonce}.tmp", std::process::id()));
    let result = (|| -> Result<(), String> {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("Cannot create temporary file: {error}"))?;
        file.write_all(content.as_bytes())
            .map_err(|error| format!("Cannot write temporary file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Cannot sync temporary file: {error}"))?;
        fs::rename(&temporary, &target)
            .map_err(|error| format!("Cannot finalize file write: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_approved_directory(policy: &AccessPolicy, path: &Path) -> Result<(), String> {
    let target = policy.authorize_write_target(path)?;
    fs::create_dir(&target).map_err(|error| format!("Cannot create directory: {error}"))
}

fn delete_approved_file(policy: &AccessPolicy, path: &Path) -> Result<(), String> {
    let target = policy.authorize_write_target(path)?;
    let metadata = fs::symlink_metadata(&target)
        .map_err(|error| format!("Cannot inspect file to delete: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(
            "Only regular files can be deleted; directories and symbolic links are rejected".into(),
        );
    }
    fs::remove_file(&target).map_err(|error| format!("Cannot delete file: {error}"))
}

fn move_approved_file(
    policy: &AccessPolicy,
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    let source = policy.authorize_write_target(source)?;
    let metadata = fs::symlink_metadata(&source)
        .map_err(|error| format!("Cannot inspect source file: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Only regular files can be moved".into());
    }
    let destination = policy.authorize_write_target(destination)?;
    if destination.exists() {
        return Err("Destination already exists; refusing to overwrite".into());
    }
    fs::rename(&source, &destination).map_err(|error| format!("Cannot move file: {error}"))
}

/// Read a regular, non-symlink text file with a hard byte limit.
/// The size check is repeated while reading so a file that grows after
/// metadata inspection cannot cause an unbounded allocation.
pub(crate) fn read_bounded_text_file(path: &Path, max_bytes: u64) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("Cannot inspect file: {e}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Access denied: only regular, non-symlink files can be read".into());
    }
    if metadata.len() > max_bytes {
        return Err(format!(
            "File is too large to read (maximum {max_bytes} bytes)"
        ));
    }

    let file = fs::File::open(path).map_err(|e| format!("Cannot open file: {e}"))?;
    let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read file: {e}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "File is too large to read (maximum {max_bytes} bytes)"
        ));
    }
    String::from_utf8(bytes).map_err(|e| format!("Cannot read file as UTF-8 text: {e}"))
}

fn canonical_directory(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("Cannot resolve folder: {e}"))?;
    if !canonical.is_dir() {
        return Err("Selected path is not a directory".into());
    }
    Ok(canonical)
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn mcp_server_info() -> Result<McpServerInfo, String> {
    let app_executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate application executable: {error}"))?;
    let app_directory = app_executable
        .parent()
        .ok_or_else(|| "Application executable has no parent directory".to_string())?;

    let release_candidate = app_directory.join("sam-desktop-mcp");
    let debug_candidate = app_directory
        .parent()
        .map(|target_dir| target_dir.join("debug/sam-desktop-mcp"));
    let command = if release_candidate.is_file() {
        release_candidate
    } else if let Some(debug_path) = debug_candidate.filter(|path| path.is_file()) {
        debug_path
    } else {
        release_candidate
    };

    Ok(McpServerInfo {
        binary_exists: command.is_file(),
        command: command.to_string_lossy().into_owned(),
        transport: "stdio",
        tools: vec![
            "list_approved_folders",
            "list_directory",
            "read_text_file",
            "read_file",
            "edit_block",
            "search_files",
            "get_file_info",
            "read_multiple_files",
            "search_content",
            "write_text_file",
            "create_directory",
            "delete_file",
            "move_file",
            "run_command",
            "start_terminal",
            "write_terminal",
            "read_terminal",
            "close_terminal",
            "list_terminal_sessions",
            "list_processes",
            "terminate_process",
            "get_recent_tool_calls",
            "get_usage_stats",
        ],
    })
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn recent_activity(
    limit: Option<usize>,
    policy: State<'_, AccessPolicy>,
) -> Result<Vec<audit::AuditEvent>, String> {
    audit::read_recent(&policy.audit_path(), limit.unwrap_or(100))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn list_processes() -> Result<processes::ProcessSnapshot, String> {
    processes::list_current_user_processes()
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn terminate_process(
    pid: u32,
    confirm: bool,
    policy: State<'_, AccessPolicy>,
) -> Result<(), String> {
    if !confirm {
        return Err("Set confirm=true to terminate this process".into());
    }
    let result = processes::terminate_current_user_process(pid);
    let target = pid.to_string();
    record_desktop_activity(&policy, "terminate_process", Some(&target), result.is_ok());
    result
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn approved_folders(policy: State<'_, AccessPolicy>) -> Result<Vec<String>, String> {
    let roots = policy
        .approved_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    Ok(roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn writable_folders(policy: State<'_, AccessPolicy>) -> Result<Vec<String>, String> {
    let roots = policy
        .writable_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    Ok(roots
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn set_folder_write_access(
    path: String,
    enabled: bool,
    policy: State<'_, AccessPolicy>,
) -> Result<Vec<String>, String> {
    let canonical = canonical_directory(&path)?;
    let roots = policy
        .approved_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    if !roots.contains(&canonical) {
        return Err("Folder must be approved for read access first".into());
    }
    let mut writable = policy
        .writable_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    let mut updated = writable.clone();
    if enabled && !updated.contains(&canonical) {
        updated.push(canonical);
    } else if !enabled {
        updated.retain(|root| root != &canonical);
    }
    updated.sort();
    let command_enabled = *policy
        .command_execution_enabled
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    policy.persist_with(&roots, &updated, command_enabled)?;
    *writable = updated.clone();
    record_desktop_activity(
        &policy,
        if enabled {
            "enable_folder_writes"
        } else {
            "disable_folder_writes"
        },
        Some(&path),
        true,
    );
    Ok(updated
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn command_execution_enabled(policy: State<'_, AccessPolicy>) -> Result<bool, String> {
    policy
        .command_execution_enabled
        .lock()
        .map(|enabled| *enabled)
        .map_err(|_| "Permission state unavailable".into())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn set_command_execution_enabled(
    enabled: bool,
    policy: State<'_, AccessPolicy>,
) -> Result<bool, String> {
    let roots = policy
        .approved_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    let writable = policy
        .writable_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    let mut current = policy
        .command_execution_enabled
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    policy.persist_with(&roots, &writable, enabled)?;
    *current = enabled;
    record_desktop_activity(
        &policy,
        if enabled {
            "enable_command_execution"
        } else {
            "disable_command_execution"
        },
        None,
        true,
    );
    Ok(enabled)
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn approve_folder(path: String, policy: State<'_, AccessPolicy>) -> Result<Vec<String>, String> {
    let canonical = canonical_directory(&path)?;
    let mut roots = policy
        .approved_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    if !roots.contains(&canonical) {
        let mut updated = roots.clone();
        updated.push(canonical);
        updated.sort();
        policy.persist(&updated)?;
        *roots = updated;
    }
    Ok(roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn revoke_folder(path: String, policy: State<'_, AccessPolicy>) -> Result<Vec<String>, String> {
    let mut roots = policy
        .approved_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    // Revocation must work even if the approved folder was moved or deleted.
    let mut updated = roots.clone();
    updated.retain(|root| root.to_string_lossy() != path);
    if updated.len() != roots.len() {
        let mut writable = policy
            .writable_roots
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        let writable_updated: Vec<PathBuf> = writable
            .iter()
            .filter(|root| updated.contains(root))
            .cloned()
            .collect();
        let command_enabled = *policy
            .command_execution_enabled
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        policy.persist_with(&updated, &writable_updated, command_enabled)?;
        *roots = updated;
        *writable = writable_updated;
    }
    Ok(roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn read_text_file(path: String, policy: State<'_, AccessPolicy>) -> Result<String, String> {
    const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;
    let requested = Path::new(&path);
    let target = policy.authorize_path(requested)?;
    read_bounded_text_file(&target, MAX_PREVIEW_BYTES)
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn write_text_file(
    path: String,
    content: String,
    overwrite: bool,
    policy: State<'_, AccessPolicy>,
) -> Result<(), String> {
    let result = write_approved_text_file(&policy, Path::new(&path), &content, overwrite);
    record_desktop_activity(&policy, "write_text_file", Some(&path), result.is_ok());
    result
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn create_directory(path: String, policy: State<'_, AccessPolicy>) -> Result<(), String> {
    let result = create_approved_directory(&policy, Path::new(&path));
    record_desktop_activity(&policy, "create_directory", Some(&path), result.is_ok());
    result
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn delete_file(path: String, confirm: bool, policy: State<'_, AccessPolicy>) -> Result<(), String> {
    if !confirm {
        return Err("Set confirm=true to delete this file".into());
    }
    let result = delete_approved_file(&policy, Path::new(&path));
    record_desktop_activity(&policy, "delete_file", Some(&path), result.is_ok());
    result
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn move_file(
    source: String,
    destination: String,
    confirm: bool,
    policy: State<'_, AccessPolicy>,
) -> Result<(), String> {
    if !confirm {
        return Err("Set confirm=true to move this file".into());
    }
    let result = move_approved_file(&policy, Path::new(&source), Path::new(&destination));
    record_desktop_activity(&policy, "move_file", Some(&source), result.is_ok());
    result
}

#[cfg(feature = "desktop")]
#[derive(Serialize)]
struct TerminalStarted {
    session_id: String,
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn terminal_start(
    cwd: String,
    cols: u16,
    rows: u16,
    app: tauri::AppHandle,
    policy: State<'_, AccessPolicy>,
    sessions: State<'_, TerminalSessions>,
) -> Result<TerminalStarted, String> {
    let target = policy.authorize_path(Path::new(&cwd))?;
    if !target.is_dir() {
        return Err("Working path is not a directory".into());
    }
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: rows.clamp(2, 120),
            cols: cols.clamp(10, 300),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Cannot create terminal: {e}"))?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let mut command = CommandBuilder::new(shell);
    command.cwd(&target);
    command.env("TERM", "xterm-256color");
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| format!("Cannot start shell: {e}"))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Cannot read terminal output: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Cannot write terminal input: {e}"))?;
    let session_id = format!("pty-{}-{}", std::process::id(), uuid_like_id());
    let mut sessions_map = sessions
        .0
        .lock()
        .map_err(|_| "Terminal session state unavailable")?;
    if sessions_map.len() >= MAX_TERMINAL_SESSIONS {
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "At most {MAX_TERMINAL_SESSIONS} terminal sessions may be active"
        ));
    }
    sessions_map.insert(
        session_id.clone(),
        PtySession {
            master: pair.master,
            writer,
            child,
        },
    );
    drop(sessions_map);

    let output_id = session_id.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0_u8; TERMINAL_OUTPUT_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let data = String::from_utf8_lossy(&buffer[..count]).into_owned();
                    let _ = app.emit(
                        "terminal-output",
                        serde_json::json!({"session_id": output_id, "data": data}),
                    );
                    // Backpressure the PTY reader instead of accumulating output in
                    // application memory or emitting an unbounded stream of events.
                    thread::sleep(TERMINAL_OUTPUT_INTERVAL);
                }
            }
        }
        let _ = app.emit(
            "terminal-exited",
            serde_json::json!({"session_id": output_id}),
        );
    });
    Ok(TerminalStarted { session_id })
}

#[cfg(feature = "desktop")]
fn uuid_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn terminal_write(
    session_id: String,
    data: String,
    sessions: State<'_, TerminalSessions>,
) -> Result<(), String> {
    if data.len() > 16 * 1024 {
        return Err("Terminal input chunk exceeds 16 KiB".into());
    }
    let mut map = sessions
        .0
        .lock()
        .map_err(|_| "Terminal session state unavailable")?;
    let session = map
        .get_mut(&session_id)
        .ok_or("Terminal session not found")?;
    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| format!("Terminal input failed: {e}"))?;
    session
        .writer
        .flush()
        .map_err(|e| format!("Terminal flush failed: {e}"))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn terminal_resize(
    session_id: String,
    cols: u16,
    rows: u16,
    sessions: State<'_, TerminalSessions>,
) -> Result<(), String> {
    let map = sessions
        .0
        .lock()
        .map_err(|_| "Terminal session state unavailable")?;
    let session = map.get(&session_id).ok_or("Terminal session not found")?;
    session
        .master
        .resize(PtySize {
            rows: rows.clamp(2, 120),
            cols: cols.clamp(10, 300),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Terminal resize failed: {e}"))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn terminal_close(session_id: String, sessions: State<'_, TerminalSessions>) -> Result<(), String> {
    let mut map = sessions
        .0
        .lock()
        .map_err(|_| "Terminal session state unavailable")?;
    // PtySession::drop kills and reaps the shell. This also applies during app shutdown.
    map.remove(&session_id);
    Ok(())
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn list_folder(path: String, policy: State<'_, AccessPolicy>) -> Result<Vec<FolderEntry>, String> {
    let target = policy.authorize_path(Path::new(&path))?;
    if !target.is_dir() {
        return Err("Requested path is not a directory".into());
    }
    let mut entries = Vec::new();
    for item in fs::read_dir(&target).map_err(|e| format!("Cannot read folder: {e}"))? {
        let item = item.map_err(|e| format!("Cannot read directory entry: {e}"))?;
        let item_path = item.path();
        let metadata =
            fs::symlink_metadata(&item_path).map_err(|e| format!("Cannot inspect entry: {e}"))?;
        // Do not follow symlinks. Their targets may lie outside the approved root.
        if metadata.file_type().is_symlink() {
            continue;
        }
        entries.push(FolderEntry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: item_path.to_string_lossy().into_owned(),
            is_dir: metadata.is_dir(),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[cfg(all(test, feature = "desktop"))]
mod tests {
    use super::*;

    #[test]
    fn authorization_rejects_paths_outside_approved_root_and_after_revocation() {
        let base = std::env::temp_dir().join(format!("sam-policy-auth-{}", std::process::id()));
        let approved = base.join("approved");
        let outside = base.join("outside");
        let storage = base.join("approvals.json");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&approved).expect("create approved directory");
        fs::create_dir_all(&outside).expect("create outside directory");
        let inside_file = approved.join("inside.txt");
        let outside_file = outside.join("outside.txt");
        fs::write(&inside_file, "inside").expect("create inside file");
        fs::write(&outside_file, "outside").expect("create outside file");

        let policy = AccessPolicy::load(storage).expect("load policy");
        let canonical = approved.canonicalize().expect("canonical approved root");
        policy
            .persist(std::slice::from_ref(&canonical))
            .expect("persist approval");
        policy.refresh().expect("refresh approval");

        assert!(policy.authorize_path(&inside_file).is_ok());
        assert!(
            policy
                .authorize_path(&outside_file)
                .unwrap_err()
                .contains("outside every approved folder")
        );

        policy.approved_roots.lock().expect("lock roots").clear();
        assert!(
            policy
                .authorize_path(&inside_file)
                .unwrap_err()
                .contains("outside every approved folder")
        );
        fs::remove_dir_all(&base).expect("cleanup test directories");
    }

    #[test]
    fn invalid_saved_permissions_fail_closed() {
        let base = std::env::temp_dir().join(format!("sam-policy-invalid-{}", std::process::id()));
        let approved = base.join("approved");
        let storage = base.join("approvals.json");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&approved).expect("create approved directory");
        let policy = AccessPolicy::load(storage.clone()).expect("load empty policy");
        let canonical = approved.canonicalize().expect("canonical root");
        policy
            .approved_roots
            .lock()
            .expect("lock roots")
            .push(canonical);
        fs::write(&storage, b"not valid json").expect("write invalid policy");
        assert!(policy.refresh().is_err());
        assert!(policy.approved_roots.lock().expect("lock roots").is_empty());
        fs::remove_dir_all(base).expect("cleanup test directory");
    }

    #[cfg(unix)]
    #[test]
    fn authorization_rejects_symbolic_link_paths() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("sam-policy-symlink-{}", std::process::id()));
        let approved = base.join("approved");
        let target = base.join("target.txt");
        let link = approved.join("linked.txt");
        let storage = base.join("approvals.json");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&approved).expect("create approved directory");
        fs::write(&target, "target").expect("create link target");
        symlink(&target, &link).expect("create symlink");

        let policy = AccessPolicy::load(storage).expect("load policy");
        let canonical = approved.canonicalize().expect("canonical approved root");
        policy
            .persist(std::slice::from_ref(&canonical))
            .expect("persist approval");
        policy.refresh().expect("refresh approval");

        assert!(
            policy
                .authorize_path(&link)
                .unwrap_err()
                .contains("symbolic links")
        );
        fs::remove_dir_all(&base).expect("cleanup test directories");
    }

    #[test]
    #[cfg(feature = "desktop")]
    fn saved_folder_approvals_survive_reload_and_missing_roots_are_removed() {
        let base =
            std::env::temp_dir().join(format!("sam-desktop-permissions-{}", std::process::id()));
        let root = base.join("approved");
        let storage = base.join("approved_folders.json");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&root).expect("create test root");

        let policy = AccessPolicy::load(storage.clone()).expect("load empty policy");
        let canonical = root.canonicalize().expect("canonical test root");
        policy
            .persist(std::slice::from_ref(&canonical))
            .expect("persist root");

        let loaded = AccessPolicy::load(storage.clone()).expect("reload persisted policy");
        assert_eq!(
            loaded.approved_roots.lock().expect("lock roots").as_slice(),
            &[canonical]
        );

        fs::remove_dir_all(&root).expect("remove approved test root");
        let reloaded = AccessPolicy::load(storage.clone()).expect("reload after root removal");
        assert!(
            reloaded
                .approved_roots
                .lock()
                .expect("lock roots")
                .is_empty()
        );

        fs::remove_dir_all(&base).expect("cleanup test files");
    }
}

#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            mcp_server_info,
            list_processes,
            terminate_process,
            recent_activity,
            approved_folders,
            writable_folders,
            set_folder_write_access,
            command_execution_enabled,
            set_command_execution_enabled,
            approve_folder,
            revoke_folder,
            list_folder,
            read_text_file,
            write_text_file,
            create_directory,
            delete_file,
            move_file,
            terminal_start,
            terminal_write,
            terminal_resize,
            terminal_close
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let policy_file = data_dir.join("approved_folders.json");
            let policy = AccessPolicy::load(policy_file).map_err(std::io::Error::other)?;
            app.manage(policy);
            app.manage(TerminalSessions::default());
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}

#[cfg(all(test, feature = "mcp"))]
mod write_permission_tests {
    use super::*;
    use std::fs;

    #[test]
    fn legacy_approvals_remain_read_only() {
        let base = std::env::temp_dir().join(format!("sam-legacy-policy-{}", std::process::id()));
        let root = base.join("approved");
        let storage = base.join("approved_folders.json");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&root).expect("create approved root");
        let canonical = root.canonicalize().expect("canonical root");
        fs::write(
            &storage,
            serde_json::json!({
                "version": 1,
                "folders": [canonical.to_string_lossy()]
            })
            .to_string(),
        )
        .expect("write legacy policy");

        let policy = AccessPolicy::load(storage).expect("load legacy policy");
        assert_eq!(
            policy
                .writable_roots
                .lock()
                .expect("lock write roots")
                .len(),
            0,
            "older approval files must not silently gain write access"
        );
        assert!(
            policy
                .authorize_write_target(&root.join("new.txt"))
                .is_err()
        );
        fs::remove_dir_all(base).expect("cleanup test directory");
    }

    #[test]
    fn write_authorization_requires_a_separate_root_and_rejects_symlinks() {
        let base = std::env::temp_dir().join(format!("sam-write-policy-{}", std::process::id()));
        let root = base.join("approved");
        let outside = base.join("outside");
        let storage = base.join("approved_folders.json");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&root).expect("create approved root");
        fs::create_dir_all(&outside).expect("create outside root");
        let canonical = root.canonicalize().expect("canonical root");
        fs::write(
            &storage,
            serde_json::json!({
                "version": 1,
                "folders": [canonical.to_string_lossy()]
            })
            .to_string(),
        )
        .expect("write read-only policy");

        let policy = AccessPolicy::load(storage.clone()).expect("load policy");
        let new_file = root.join("new.txt");
        assert!(policy.authorize_write_target(&new_file).is_err());

        fs::write(
            &storage,
            serde_json::json!({
                "version": 1,
                "folders": [canonical.to_string_lossy()],
                "writable_folders": [canonical.to_string_lossy()]
            })
            .to_string(),
        )
        .expect("write writable policy");
        policy.refresh().expect("reload write approval");
        assert_eq!(
            policy
                .authorize_write_target(&new_file)
                .expect("authorize new file"),
            new_file
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let linked_parent = root.join("outside-link");
            symlink(&outside, &linked_parent).expect("create outside directory symlink");
            assert!(
                policy
                    .authorize_write_target(&linked_parent.join("escape.txt"))
                    .is_err()
            );
            let linked_file = root.join("linked-file.txt");
            symlink(outside.join("target.txt"), &linked_file).expect("create file symlink");
            assert!(policy.authorize_write_target(&linked_file).is_err());
        }
        fs::remove_dir_all(base).expect("cleanup test directory");
    }
}

#[cfg(test)]
mod bounded_file_tests {
    use super::read_bounded_text_file;
    use std::{fs, path::PathBuf};

    fn test_file() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "sam-bounded-read-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test directory");
        let file = dir.join("sample.txt");
        (dir, file)
    }

    #[test]
    fn enforces_limit_and_accepts_exact_limit() {
        let (dir, file) = test_file();
        fs::write(&file, b"1234").expect("write exact-limit file");
        assert_eq!(
            read_bounded_text_file(&file, 4).expect("read exact limit"),
            "1234"
        );

        fs::write(&file, b"12345").expect("write oversized file");
        assert!(
            read_bounded_text_file(&file, 4)
                .unwrap_err()
                .contains("too large")
        );
        fs::remove_dir_all(dir).expect("cleanup test directory");
    }

    #[test]
    fn rejects_directories_and_symbolic_links() {
        let (dir, file) = test_file();
        fs::write(&file, b"safe").expect("write target file");
        assert!(read_bounded_text_file(&dir, 8).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = dir.join("linked.txt");
            symlink(&file, &link).expect("create symbolic link");
            assert!(read_bounded_text_file(&link, 8).is_err());
        }
        fs::remove_dir_all(dir).expect("cleanup test directory");
    }

    #[test]
    fn accepts_empty_file_at_zero_byte_limit() {
        let (dir, file) = test_file();
        fs::write(&file, b"").expect("write empty file");
        assert_eq!(
            read_bounded_text_file(&file, 0).expect("read empty file"),
            ""
        );
        fs::write(&file, b"x").expect("write non-empty file");
        assert!(read_bounded_text_file(&file, 0).is_err());
        fs::remove_dir_all(dir).expect("cleanup test directory");
    }

    #[test]
    fn rejects_non_utf8_text() {
        let (dir, file) = test_file();
        fs::write(&file, [0xff, 0xfe]).expect("write non-UTF-8 file");
        assert!(
            read_bounded_text_file(&file, 8)
                .unwrap_err()
                .contains("UTF-8")
        );
        fs::remove_dir_all(dir).expect("cleanup test directory");
    }
}
