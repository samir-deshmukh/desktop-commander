use serde::{Deserialize, Serialize};
#[cfg(feature = "desktop")]
use std::process::Command;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};
#[cfg(feature = "desktop")]
use tauri::{Manager, State};

#[cfg(feature = "mcp")]
mod mcp;
#[cfg(feature = "mcp")]
pub use mcp::run_stdio_server;

struct AccessPolicy {
    approved_roots: Mutex<Vec<PathBuf>>,
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
}

impl AccessPolicy {
    fn load(storage_file: PathBuf) -> Result<Self, String> {
        let policy = Self {
            approved_roots: Mutex::new(Vec::new()),
            storage_file,
        };
        policy.refresh()?;
        Ok(policy)
    }

    // MCP runs in a separate process from the desktop UI. Reload the shared
    // permission file before every tool call so revocations take effect immediately.
    fn refresh(&self) -> Result<(), String> {
        let loaded = (|| -> Result<Vec<PathBuf>, String> {
            if !self.storage_file.exists() {
                return Ok(Vec::new());
            }
            let contents = fs::read_to_string(&self.storage_file)
                .map_err(|e| format!("Cannot read saved folder permissions: {e}"))?;
            let saved: SavedApprovals = serde_json::from_str(&contents)
                .map_err(|e| format!("Saved folder permissions are invalid: {e}"))?;
            if saved.version != 1 {
                return Err("Saved folder permissions use an unsupported version".into());
            }

            let mut valid = Vec::new();
            for folder in saved.folders {
                if let Ok(path) = canonical_directory(&folder) {
                    if !valid.contains(&path) {
                        valid.push(path);
                    }
                }
            }
            valid.sort();
            Ok(valid)
        })();

        let mut roots = self
            .approved_roots
            .lock()
            .map_err(|_| "Permission state unavailable")?;
        match loaded {
            Ok(valid) => {
                *roots = valid;
                Ok(())
            }
            Err(error) => {
                // Fail closed: never retain permissions from an older valid file.
                roots.clear();
                Err(error)
            }
        }
    }

    #[cfg(feature = "desktop")]
    fn persist(&self, roots: &[PathBuf]) -> Result<(), String> {
        let saved = SavedApprovals {
            version: 1,
            folders: roots
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
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
        tools: vec!["list_approved_folders", "list_directory", "read_text_file"],
    })
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
fn read_text_file(path: String, policy: State<'_, AccessPolicy>) -> Result<String, String> {
    const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;
    let requested = Path::new(&path);
    let target = requested
        .canonicalize()
        .map_err(|e| format!("Cannot resolve file: {e}"))?;
    let roots = policy
        .approved_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    if !roots.iter().any(|root| target.starts_with(root)) {
        return Err("Access denied: file is not inside an approved root".into());
    }
    read_bounded_text_file(requested, MAX_PREVIEW_BYTES)
}

#[cfg(feature = "desktop")]
fn safe_command_spec(input: &str) -> Option<(&'static str, &'static [&'static str])> {
    match input.trim() {
        "pwd" => Some(("pwd", &[])),
        "git status --short --branch" => Some(("git", &["status", "--short", "--branch"])),
        "git log -5 --oneline" => Some(("git", &["log", "-5", "--oneline"])),
        "git diff --stat" => Some(("git", &["diff", "--stat"])),
        "git branch --show-current" => Some(("git", &["branch", "--show-current"])),
        "git rev-parse --show-toplevel" => Some(("git", &["rev-parse", "--show-toplevel"])),
        "rustc --version" => Some(("rustc", &["--version"])),
        "cargo --version" => Some(("cargo", &["--version"])),
        "node --version" => Some(("node", &["--version"])),
        "npm --version" => Some(("npm", &["--version"])),
        "python3 --version" => Some(("python3", &["--version"])),
        "free -h" => Some(("free", &["-h"])),
        "df -h" => Some(("df", &["-h"])),
        "uname -a" => Some(("uname", &["-a"])),
        _ => None,
    }
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn run_safe_command(
    command: String,
    cwd: String,
    policy: State<'_, AccessPolicy>,
) -> Result<String, String> {
    let (program, args) = safe_command_spec(&command).ok_or_else(|| {
        "Command not allowed. Choose one of the listed read-only commands.".to_string()
    })?;
    let target = Path::new(&cwd)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve working folder: {e}"))?;
    if !target.is_dir() {
        return Err("Working path is not a directory".into());
    }
    let roots = policy
        .approved_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    if !roots.iter().any(|root| target.starts_with(root)) {
        return Err("Access denied: terminal working folder is not inside an approved root".into());
    }
    drop(roots);
    let output = Command::new(program)
        .args(args)
        .current_dir(&target)
        // Prevent repository-local fsmonitor configuration from launching an external helper.
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .map_err(|e| format!("Could not start command: {e}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    if text.is_empty() {
        text = if output.status.success() {
            "Command completed successfully (no output).".into()
        } else {
            "Command failed without output.".into()
        };
    }
    const MAX_OUTPUT_BYTES: usize = 64 * 1024;
    if text.len() > MAX_OUTPUT_BYTES {
        let mut boundary = MAX_OUTPUT_BYTES;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        text.push_str("\n[Output truncated at 64 KiB]");
    }
    Ok(format!(
        "{}\nExit code: {}",
        text,
        output.status.code().unwrap_or(-1)
    ))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn list_folder(path: String, policy: State<'_, AccessPolicy>) -> Result<Vec<FolderEntry>, String> {
    let target = Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path: {e}"))?;
    let roots = policy
        .approved_roots
        .lock()
        .map_err(|_| "Permission state unavailable")?;
    if !roots.iter().any(|root| target.starts_with(root)) {
        return Err("Access denied: folder is not inside an approved root".into());
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
    #[cfg(feature = "desktop")]
    fn terminal_allows_only_fixed_read_only_commands() {
        assert!(safe_command_spec("git status --short --branch").is_some());
        assert!(safe_command_spec("free -h").is_some());
        assert!(safe_command_spec("rm -rf /").is_none());
        assert!(safe_command_spec("git status; rm -rf /").is_none());
        assert!(safe_command_spec("cat /etc/shadow").is_none());
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
            approved_folders,
            approve_folder,
            revoke_folder,
            list_folder,
            read_text_file,
            run_safe_command
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let policy_file = data_dir.join("approved_folders.json");
            let policy = AccessPolicy::load(policy_file).map_err(std::io::Error::other)?;
            app.manage(policy);
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
