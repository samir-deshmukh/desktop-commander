use super::{AccessPolicy, read_bounded_text_file};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, SyncSender, TryRecvError, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

const APP_IDENTIFIER: &str = "com.samdeshmukh.desktopcommander";
const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;
const MAX_SEARCH_RESULTS: usize = 200;
const MAX_SEARCH_VISITED: usize = 5_000;
const MAX_SEARCH_DEPTH: usize = 12;
const MAX_MULTI_READ_FILES: usize = 10;
const MAX_MULTI_READ_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONTENT_SEARCH_FILES: usize = 1_000;
const MAX_CONTENT_SEARCH_FILE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PathInput {
    /// Absolute path inside a folder approved in Sam's Desktop Commander.
    path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchInput {
    /// Approved directory in which to search.
    path: String,
    /// Case-insensitive substring matched against file and directory names.
    query: String,
    /// Maximum matches to return (1-200).
    max_results: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MultiplePathsInput {
    /// Absolute paths of files inside approved folders (maximum 10).
    paths: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ContentSearchInput {
    /// Approved directory in which to search.
    path: String,
    /// Case-insensitive text to find in UTF-8 files.
    query: String,
    /// Maximum matching files to return (1-100).
    max_results: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WriteFileInput {
    /// Absolute file path inside a folder separately approved for changes.
    path: String,
    /// UTF-8 text to write (maximum 1 MiB).
    content: String,
    /// Must be true to replace an existing regular file.
    overwrite: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ConfirmPathInput {
    /// Absolute path inside a folder separately approved for changes.
    path: String,
    /// Must be true to confirm the requested change.
    confirm: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MoveFileInput {
    /// Existing regular file inside a folder approved for changes.
    source: String,
    /// New path inside a folder approved for changes. Destination must not exist.
    destination: String,
    /// Must be true to confirm moving the file.
    confirm: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RunCommandInput {
    /// Executable name or absolute executable path. The command runs directly, without a shell.
    program: String,
    /// Arguments passed as separate values (maximum 64).
    args: Vec<String>,
    /// Working directory inside a folder approved in the desktop app.
    cwd: String,
    /// Timeout in milliseconds (100-30000).
    timeout_ms: Option<u64>,
    /// Must be true to confirm running this command.
    confirm: bool,
}

#[derive(Serialize)]
struct CommandOutput {
    exit_code: Option<i32>,
    timed_out: bool,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TerminateProcessInput {
    /// PID of a process owned by the current Linux user.
    pid: u32,
    /// Must be true to confirm sending SIGTERM.
    confirm: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct StartTerminalInput {
    /// Approved working directory for the interactive shell.
    cwd: String,
    /// Terminal columns (10-300).
    cols: Option<u16>,
    /// Terminal rows (2-120).
    rows: Option<u16>,
    /// Must be true to confirm opening an interactive shell.
    confirm: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TerminalSessionInput {
    /// Session ID returned by start_terminal.
    session_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TerminalWriteInput {
    /// Session ID returned by start_terminal.
    session_id: String,
    /// Input to send to the shell (maximum 16 KiB).
    data: String,
}

#[derive(Serialize)]
struct TerminalReadOutput {
    output: String,
    exited: bool,
}

struct McpTerminalSession {
    _master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    receiver: Receiver<String>,
}

impl Drop for McpTerminalSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Serialize)]
struct FileInfo {
    path: String,
    name: String,
    kind: &'static str,
    size_bytes: u64,
    modified_unix_seconds: Option<u64>,
    readonly: bool,
}

#[derive(Serialize)]
struct DirectoryEntry {
    name: String,
    path: String,
    kind: &'static str,
}

#[derive(Clone)]
struct DesktopCommanderMcp {
    policy: Arc<AccessPolicy>,
    terminal_sessions: Arc<Mutex<std::collections::HashMap<String, McpTerminalSession>>>,
}

fn terminal_output_reader(mut reader: Box<dyn std::io::Read + Send>, sender: SyncSender<String>) {
    let mut buffer = [0u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                if sender
                    .send(String::from_utf8_lossy(&buffer[..count]).into_owned())
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

fn read_capped_output(mut reader: impl Read) -> Result<(String, bool), String> {
    const MAX_OUTPUT_BYTES: usize = 256 * 1024;
    let mut output = Vec::with_capacity(MAX_OUTPUT_BYTES.min(8 * 1024));
    let mut buffer = [0u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("Cannot read command output: {error}"))?;
        if count == 0 {
            break;
        }
        let remaining = MAX_OUTPUT_BYTES.saturating_sub(output.len());
        let keep = count.min(remaining);
        output.extend_from_slice(&buffer[..keep]);
        if keep < count {
            truncated = true;
        }
    }
    Ok((String::from_utf8_lossy(&output).into_owned(), truncated))
}

#[tool_router(server_handler)]
impl DesktopCommanderMcp {
    #[tool(
        description = "List the folder roots explicitly approved in Sam's Desktop Commander. Returns absolute paths. No folders are approved by default."
    )]
    fn list_approved_folders(&self) -> String {
        if let Err(error) = self.policy.refresh() {
            return format!("ERROR: {error}");
        }
        match self.policy.approved_roots.lock() {
            Ok(roots) => serde_json::to_string_pretty(
                &roots
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|error| format!("ERROR: cannot serialize approved folders: {error}")),
            Err(_) => "ERROR: permission state is unavailable".into(),
        }
    }

    #[tool(
        description = "List the immediate files and folders at an absolute path. The path must be inside an approved folder. Symbolic links are not followed."
    )]
    fn list_directory(&self, Parameters(args): Parameters<PathInput>) -> String {
        match self.list_directory_inner(&args.path) {
            Ok(entries) => serde_json::to_string_pretty(&entries).unwrap_or_else(|error| {
                format!("ERROR: cannot serialize directory listing: {error}")
            }),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Read a UTF-8 text file inside an approved folder. Maximum file size is 1 MiB. Symbolic links and files outside approved folders are rejected."
    )]
    fn read_text_file(&self, Parameters(args): Parameters<PathInput>) -> String {
        match self.read_text_file_inner(&args.path) {
            Ok(contents) => contents,
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Search file and directory names recursively inside an approved folder. Matches a case-insensitive substring; skips symbolic links; bounded to depth 12, 5,000 visited entries, and at most 200 results."
    )]
    fn search_files(&self, Parameters(args): Parameters<SearchInput>) -> String {
        match self.search_files_inner(&args) {
            Ok(results) => serde_json::to_string_pretty(&results)
                .unwrap_or_else(|error| format!("ERROR: cannot serialize search results: {error}")),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Get basic metadata for a file or directory inside an approved folder. Does not read file contents or follow symbolic links."
    )]
    fn get_file_info(&self, Parameters(args): Parameters<PathInput>) -> String {
        match self.get_file_info_inner(&args.path) {
            Ok(info) => serde_json::to_string_pretty(&info).unwrap_or_else(|error| {
                format!("ERROR: cannot serialize file information: {error}")
            }),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Read up to 10 UTF-8 text files from approved folders, with a combined 4 MiB output limit and the standard 1 MiB per-file limit. Each path is authorized independently."
    )]
    fn read_multiple_files(&self, Parameters(args): Parameters<MultiplePathsInput>) -> String {
        match self.read_multiple_files_inner(&args.paths) {
            Ok(contents) => serde_json::to_string_pretty(&contents)
                .unwrap_or_else(|error| format!("ERROR: cannot serialize file contents: {error}")),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Search UTF-8 file contents recursively inside an approved folder. Skips symbolic links and files larger than 256 KiB; bounded to 1,000 files inspected and 100 matching files."
    )]
    fn search_content(&self, Parameters(args): Parameters<ContentSearchInput>) -> String {
        match self.search_content_inner(&args) {
            Ok(matches) => serde_json::to_string_pretty(&matches).unwrap_or_else(|error| {
                format!("ERROR: cannot serialize content search results: {error}")
            }),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Write UTF-8 text to a file in a folder explicitly enabled for changes. Maximum 1 MiB. Replacing an existing file requires overwrite=true; writes use a temporary sibling file and atomic rename."
    )]
    fn write_text_file(&self, Parameters(args): Parameters<WriteFileInput>) -> String {
        match self.write_text_file_inner(&args) {
            Ok(()) => "File written successfully".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Create one directory inside a folder explicitly enabled for changes. Parent must already exist; requires confirm=true."
    )]
    fn create_directory(&self, Parameters(args): Parameters<ConfirmPathInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to create this directory".into();
        }
        match self.create_directory_inner(&args.path) {
            Ok(()) => "Directory created successfully".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Delete one regular file inside a folder explicitly enabled for changes. Directories and symbolic links are rejected. Requires confirm=true; deletion is permanent."
    )]
    fn delete_file(&self, Parameters(args): Parameters<ConfirmPathInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to delete this file".into();
        }
        match self.delete_file_inner(&args.path) {
            Ok(()) => "File deleted successfully".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Move or rename one regular file between folders explicitly enabled for changes. Destination must not exist. Requires confirm=true."
    )]
    fn move_file(&self, Parameters(args): Parameters<MoveFileInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to move this file".into();
        }
        match self.move_file_inner(&args.source, &args.destination) {
            Ok(()) => "File moved successfully".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Run one executable directly (no shell) with separate arguments in an approved working directory. Requires the owner to enable MCP command execution in the desktop app and confirm=true for each run. Timeout is capped at 30 seconds; stdout and stderr are each capped at 256 KiB. Commands run with the desktop user's OS permissions and are not sandboxed."
    )]
    fn run_command(&self, Parameters(args): Parameters<RunCommandInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to run this command".into();
        }
        match self.run_command_inner(&args) {
            Ok(output) => serde_json::to_string_pretty(&output)
                .unwrap_or_else(|error| format!("ERROR: cannot serialize command output: {error}")),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Start an interactive local shell in an approved working directory. Requires MCP command execution to be enabled in the desktop app and confirm=true. Runs with the desktop user's permissions, is not sandboxed, and is limited to 3 sessions."
    )]
    fn start_terminal(&self, Parameters(args): Parameters<StartTerminalInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to start a terminal".into();
        }
        match self.start_terminal_inner(&args) {
            Ok(session_id) => format!("Terminal started: {session_id}"),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Send up to 16 KiB of input to an existing interactive MCP terminal session."
    )]
    fn write_terminal(&self, Parameters(args): Parameters<TerminalWriteInput>) -> String {
        match self.write_terminal_inner(&args.session_id, &args.data) {
            Ok(()) => "Terminal input sent".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Read available output from an interactive terminal session. Returns at most 64 KiB per call; call again to collect later output."
    )]
    fn read_terminal(&self, Parameters(args): Parameters<TerminalSessionInput>) -> String {
        match self.read_terminal_inner(&args.session_id) {
            Ok(output) => serde_json::to_string_pretty(&output).unwrap_or_else(|error| {
                format!("ERROR: cannot serialize terminal output: {error}")
            }),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(description = "Close an interactive terminal session and terminate its shell.")]
    fn close_terminal(&self, Parameters(args): Parameters<TerminalSessionInput>) -> String {
        match self.close_terminal_inner(&args.session_id) {
            Ok(()) => "Terminal session closed".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(description = "List active interactive terminal session IDs.")]
    fn list_terminal_sessions(&self) -> String {
        match self.terminal_sessions.lock() {
            Ok(sessions) => serde_json::to_string_pretty(&sessions.keys().collect::<Vec<_>>())
                .unwrap_or_else(|error| {
                    format!("ERROR: cannot serialize terminal sessions: {error}")
                }),
            Err(_) => "ERROR: terminal session state unavailable".into(),
        }
    }

    #[tool(
        description = "List process metadata for processes owned by the current Linux user. Does not collect command-line arguments or environment variables; capped at 500 processes."
    )]
    fn list_processes(&self) -> String {
        match super::processes::list_current_user_processes() {
            Ok(snapshot) => serde_json::to_string_pretty(&snapshot)
                .unwrap_or_else(|error| format!("ERROR: cannot serialize process list: {error}")),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Send SIGTERM to one process owned by the current Linux user. Requires confirm=true. PID 1, the current MCP process, and Sam's Desktop Commander processes are protected."
    )]
    fn terminate_process(&self, Parameters(args): Parameters<TerminateProcessInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to terminate this process".into();
        }
        match super::processes::terminate_current_user_process(args.pid) {
            Ok(()) => format!("SIGTERM sent to process {}", args.pid),
            Err(error) => format!("ERROR: {error}"),
        }
    }
}

impl DesktopCommanderMcp {
    fn authorize_path(&self, path: &str) -> Result<PathBuf, String> {
        self.policy.refresh()?;
        self.policy.authorize_path(Path::new(path))
    }

    fn list_directory_inner(&self, path: &str) -> Result<Vec<DirectoryEntry>, String> {
        let target = self.authorize_path(path)?;
        if !target.is_dir() {
            return Err("requested path is not a directory".into());
        }

        let mut entries = Vec::new();
        for item in
            fs::read_dir(&target).map_err(|error| format!("Cannot read directory: {error}"))?
        {
            let item = item.map_err(|error| format!("Cannot read directory entry: {error}"))?;
            let item_path = item.path();
            let metadata = fs::symlink_metadata(&item_path)
                .map_err(|error| format!("Cannot inspect directory entry: {error}"))?;
            if metadata.file_type().is_symlink() {
                continue;
            }

            entries.push(DirectoryEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                path: item_path.to_string_lossy().into_owned(),
                kind: if metadata.is_dir() {
                    "directory"
                } else if metadata.is_file() {
                    "file"
                } else {
                    "other"
                },
            });
        }
        entries.sort_by(|left, right| {
            left.kind
                .cmp(right.kind)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(entries)
    }

    fn read_text_file_inner(&self, path: &str) -> Result<String, String> {
        let target = self.authorize_path(path)?;
        read_bounded_text_file(&target, MAX_PREVIEW_BYTES)
    }

    fn search_files_inner(&self, args: &SearchInput) -> Result<Vec<String>, String> {
        if args.query.trim().is_empty() {
            return Err("Search query cannot be empty".into());
        }
        let root = self.authorize_path(&args.path)?;
        if !root.is_dir() {
            return Err("Search path is not a directory".into());
        }
        let limit = args.max_results.unwrap_or(100).clamp(1, MAX_SEARCH_RESULTS);
        let query = args.query.to_lowercase();
        let mut pending = vec![(root, 0usize)];
        let mut matches = Vec::new();
        let mut visited = 0usize;

        while let Some((directory, depth)) = pending.pop() {
            if depth > MAX_SEARCH_DEPTH || matches.len() >= limit || visited >= MAX_SEARCH_VISITED {
                break;
            }
            let entries = fs::read_dir(&directory)
                .map_err(|error| format!("Cannot search directory: {error}"))?;
            for entry in entries {
                if visited >= MAX_SEARCH_VISITED || matches.len() >= limit {
                    break;
                }
                let entry =
                    entry.map_err(|error| format!("Cannot inspect search entry: {error}"))?;
                visited += 1;
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| format!("Cannot inspect search entry: {error}"))?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.to_lowercase().contains(&query) {
                    matches.push(path.to_string_lossy().into_owned());
                }
                if metadata.is_dir() && depth < MAX_SEARCH_DEPTH {
                    pending.push((path, depth + 1));
                }
            }
        }
        matches.sort();
        Ok(matches)
    }

    fn start_terminal_inner(&self, args: &StartTerminalInput) -> Result<String, String> {
        if !self.policy.command_execution_allowed()? {
            return Err("MCP command execution is disabled in the desktop app".into());
        }
        let cwd = self.authorize_path(&args.cwd)?;
        if !cwd.is_dir() {
            return Err("Terminal working directory is not a directory".into());
        }
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: args.rows.unwrap_or(24).clamp(2, 120),
                cols: args.cols.unwrap_or(80).clamp(10, 300),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Cannot create terminal: {error}"))?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let mut command = CommandBuilder::new(shell);
        command.cwd(&cwd);
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("Cannot start terminal shell: {error}"))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("Cannot read terminal output: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("Cannot write terminal input: {error}"))?;
        let session_id = format!(
            "mcp-pty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let (sender, receiver) = sync_channel::<String>(32);
        let mut sessions = self
            .terminal_sessions
            .lock()
            .map_err(|_| "Terminal session state unavailable")?;
        if sessions.len() >= 3 {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return Err("At most 3 MCP terminal sessions may be active".into());
        }
        sessions.insert(
            session_id.clone(),
            McpTerminalSession {
                _master: pair.master,
                writer,
                child,
                receiver,
            },
        );
        drop(sessions);
        thread::spawn(move || terminal_output_reader(reader, sender));
        Ok(session_id)
    }

    fn write_terminal_inner(&self, session_id: &str, data: &str) -> Result<(), String> {
        if data.len() > 16 * 1024 {
            return Err("Terminal input chunk exceeds 16 KiB".into());
        }
        let mut sessions = self
            .terminal_sessions
            .lock()
            .map_err(|_| "Terminal session state unavailable")?;
        let session = sessions
            .get_mut(session_id)
            .ok_or("Terminal session not found")?;
        session
            .writer
            .write_all(data.as_bytes())
            .map_err(|error| format!("Terminal input failed: {error}"))?;
        session
            .writer
            .flush()
            .map_err(|error| format!("Terminal flush failed: {error}"))
    }

    fn read_terminal_inner(&self, session_id: &str) -> Result<TerminalReadOutput, String> {
        const MAX_READ_BYTES: usize = 64 * 1024;
        let mut sessions = self
            .terminal_sessions
            .lock()
            .map_err(|_| "Terminal session state unavailable")?;
        let session = sessions
            .get_mut(session_id)
            .ok_or("Terminal session not found")?;
        let exited = session
            .child
            .try_wait()
            .map_err(|error| format!("Cannot inspect terminal: {error}"))?
            .is_some();
        let mut bytes = Vec::with_capacity(MAX_READ_BYTES);
        while bytes.len() < MAX_READ_BYTES {
            match session.receiver.try_recv() {
                Ok(chunk) => {
                    let remaining = MAX_READ_BYTES - bytes.len();
                    bytes.extend_from_slice(&chunk.as_bytes()[..chunk.len().min(remaining)]);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        Ok(TerminalReadOutput {
            output: String::from_utf8_lossy(&bytes).into_owned(),
            exited,
        })
    }

    fn close_terminal_inner(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self
            .terminal_sessions
            .lock()
            .map_err(|_| "Terminal session state unavailable")?;
        sessions
            .remove(session_id)
            .ok_or("Terminal session not found")?;
        Ok(())
    }

    fn run_command_inner(&self, args: &RunCommandInput) -> Result<CommandOutput, String> {
        if !args.confirm {
            return Err("Set confirm=true to run this command".into());
        }
        if !self.policy.command_execution_allowed()? {
            return Err("MCP command execution is disabled in the desktop app".into());
        }
        if args.program.trim().is_empty() || args.program.len() > 256 {
            return Err("Executable name must contain 1-256 characters".into());
        }
        if args.args.len() > 64 || args.args.iter().any(|arg| arg.len() > 8 * 1024) {
            return Err("Command arguments exceed the configured count or size limit".into());
        }
        let total_args: usize = args.args.iter().map(String::len).sum();
        if total_args > 32 * 1024 {
            return Err("Combined command arguments exceed 32 KiB".into());
        }
        let cwd = self.authorize_path(&args.cwd)?;
        if !cwd.is_dir() {
            return Err("Command working directory is not a directory".into());
        }
        let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(10_000).clamp(100, 30_000));
        let mut child = Command::new(&args.program)
            .args(&args.args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start command: {error}"))?;
        let stdout = child.stdout.take().ok_or("Cannot capture command stdout")?;
        let stderr = child.stderr.take().ok_or("Cannot capture command stderr")?;
        let stdout_reader = thread::spawn(move || read_capped_output(stdout));
        let stderr_reader = thread::spawn(move || read_capped_output(stderr));
        let started = Instant::now();
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("Cannot inspect command status: {error}"))?
            {
                break Some(status);
            }
            if started.elapsed() >= timeout {
                timed_out = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            thread::sleep(Duration::from_millis(20));
        };
        let (stdout, stdout_truncated) = stdout_reader
            .join()
            .map_err(|_| "Command stdout reader failed".to_string())??;
        let (stderr, stderr_truncated) = stderr_reader
            .join()
            .map_err(|_| "Command stderr reader failed".to_string())??;
        Ok(CommandOutput {
            exit_code: status.and_then(|value| value.code()),
            timed_out,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
        })
    }

    fn write_text_file_inner(&self, args: &WriteFileInput) -> Result<(), String> {
        super::write_approved_text_file(
            &self.policy,
            Path::new(&args.path),
            &args.content,
            args.overwrite,
        )
    }

    fn create_directory_inner(&self, path: &str) -> Result<(), String> {
        super::create_approved_directory(&self.policy, Path::new(path))
    }

    fn delete_file_inner(&self, path: &str) -> Result<(), String> {
        super::delete_approved_file(&self.policy, Path::new(path))
    }

    fn move_file_inner(&self, source: &str, destination: &str) -> Result<(), String> {
        super::move_approved_file(&self.policy, Path::new(source), Path::new(destination))
    }

    fn read_multiple_files_inner(&self, paths: &[String]) -> Result<Vec<(String, String)>, String> {
        if paths.is_empty() || paths.len() > MAX_MULTI_READ_FILES {
            return Err(format!(
                "Provide between 1 and {MAX_MULTI_READ_FILES} file paths"
            ));
        }
        let mut total_bytes = 0usize;
        let mut output = Vec::with_capacity(paths.len());
        for path in paths {
            let target = self.authorize_path(path)?;
            let content = read_bounded_text_file(&target, MAX_PREVIEW_BYTES)?;
            total_bytes = total_bytes.saturating_add(content.len());
            if total_bytes > MAX_MULTI_READ_BYTES {
                return Err(format!(
                    "Combined file content exceeds the {MAX_MULTI_READ_BYTES}-byte limit"
                ));
            }
            output.push((target.to_string_lossy().into_owned(), content));
        }
        Ok(output)
    }

    fn search_content_inner(&self, args: &ContentSearchInput) -> Result<Vec<String>, String> {
        if args.query.trim().is_empty() {
            return Err("Search query cannot be empty".into());
        }
        let root = self.authorize_path(&args.path)?;
        if !root.is_dir() {
            return Err("Search path is not a directory".into());
        }
        let limit = args.max_results.unwrap_or(50).clamp(1, 100);
        let query = args.query.to_lowercase();
        let mut pending = vec![(root, 0usize)];
        let mut matches = Vec::new();
        let mut inspected = 0usize;
        while let Some((directory, depth)) = pending.pop() {
            if depth > MAX_SEARCH_DEPTH
                || matches.len() >= limit
                || inspected >= MAX_CONTENT_SEARCH_FILES
            {
                break;
            }
            for entry in fs::read_dir(&directory)
                .map_err(|error| format!("Cannot search directory: {error}"))?
            {
                if inspected >= MAX_CONTENT_SEARCH_FILES || matches.len() >= limit {
                    break;
                }
                let entry =
                    entry.map_err(|error| format!("Cannot inspect search entry: {error}"))?;
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| format!("Cannot inspect search entry: {error}"))?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    if depth < MAX_SEARCH_DEPTH {
                        pending.push((path, depth + 1));
                    }
                    continue;
                }
                if !metadata.is_file() || metadata.len() > MAX_CONTENT_SEARCH_FILE_BYTES {
                    continue;
                }
                inspected += 1;
                let Ok(authorized) = self.authorize_path(path.to_string_lossy().as_ref()) else {
                    continue;
                };
                if let Ok(contents) =
                    read_bounded_text_file(&authorized, MAX_CONTENT_SEARCH_FILE_BYTES)
                {
                    if contents.to_lowercase().contains(&query) {
                        matches.push(authorized.to_string_lossy().into_owned());
                    }
                }
            }
        }
        matches.sort();
        Ok(matches)
    }

    fn get_file_info_inner(&self, path: &str) -> Result<FileInfo, String> {
        let target = self.authorize_path(path)?;
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| format!("Cannot inspect file metadata: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("Access denied: symbolic links cannot be inspected".into());
        }
        let kind = if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        let modified_unix_seconds = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs());
        Ok(FileInfo {
            path: target.to_string_lossy().into_owned(),
            name: target
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            kind,
            size_bytes: metadata.len(),
            modified_unix_seconds,
            readonly: metadata.permissions().readonly(),
        })
    }
}

fn application_data_dir() -> Result<PathBuf, String> {
    if let Some(base) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(base).join(APP_IDENTIFIER));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "HOME is not set; cannot locate application permissions".to_string())?;
    Ok(PathBuf::from(home)
        .join(".local/share")
        .join(APP_IDENTIFIER))
}

pub async fn run_stdio_server() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = application_data_dir().map_err(std::io::Error::other)?;
    fs::create_dir_all(&data_dir)?;
    let policy_path = data_dir.join("approved_folders.json");
    let policy = AccessPolicy::load(policy_path).map_err(std::io::Error::other)?;

    let server = DesktopCommanderMcp {
        policy: Arc::new(policy),
        terminal_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
    }
    .serve(stdio())
    .await?;
    server.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_server() -> (DesktopCommanderMcp, PathBuf) {
        let test_name = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .replace(':', "-");
        let base =
            std::env::temp_dir().join(format!("sam-mcp-policy-{}-{test_name}", std::process::id()));
        let root = base.join("approved");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&root).expect("create approved test root");
        fs::write(root.join("note.txt"), "safe test text").expect("write test file");

        let storage_file = base.join("approvals.json");
        let saved = serde_json::json!({
            "version": 1,
            "folders": [root.canonicalize().expect("canonical root").to_string_lossy()],
            "writable_folders": [root.canonicalize().expect("canonical writable root").to_string_lossy()],
            "command_execution_enabled": true
        });
        fs::write(
            &storage_file,
            serde_json::to_vec_pretty(&saved).expect("serialize test approvals"),
        )
        .expect("write test approvals");
        let policy = AccessPolicy::load(storage_file).expect("load test policy");
        (
            DesktopCommanderMcp {
                policy: Arc::new(policy),
                terminal_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            },
            base,
        )
    }

    #[test]
    fn mcp_tools_respect_approved_roots_and_read_limits() {
        let (server, base) = test_server();
        let root = base.join("approved");
        assert!(
            server
                .list_directory_inner(root.to_str().expect("root path"))
                .is_ok()
        );
        assert_eq!(
            server
                .read_text_file_inner(root.join("note.txt").to_str().expect("file path"))
                .unwrap(),
            "safe test text"
        );
        let info = server
            .get_file_info_inner(root.join("note.txt").to_str().expect("file path"))
            .unwrap();
        assert_eq!(info.kind, "file");
        assert_eq!(info.size_bytes, "safe test text".len() as u64);
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("create nested directory");
        fs::write(nested.join("target-config.toml"), "test").expect("write nested file");
        let matches = server
            .search_files_inner(&SearchInput {
                path: root.to_string_lossy().into_owned(),
                query: "config".into(),
                max_results: Some(10),
            })
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].ends_with("target-config.toml"));
        let multiple = server
            .read_multiple_files_inner(&[
                root.join("note.txt").to_string_lossy().into_owned(),
                nested
                    .join("target-config.toml")
                    .to_string_lossy()
                    .into_owned(),
            ])
            .unwrap();
        assert_eq!(multiple.len(), 2);
        let content_matches = server
            .search_content_inner(&ContentSearchInput {
                path: root.to_string_lossy().into_owned(),
                query: "safe test".into(),
                max_results: Some(10),
            })
            .unwrap();
        assert!(
            content_matches
                .iter()
                .any(|path| path.ends_with("note.txt"))
        );

        let created = root.join("created.txt");
        server
            .write_text_file_inner(&WriteFileInput {
                path: created.to_string_lossy().into_owned(),
                content: "created safely".into(),
                overwrite: false,
            })
            .unwrap();
        assert_eq!(fs::read_to_string(&created).unwrap(), "created safely");
        assert!(
            server
                .write_text_file_inner(&WriteFileInput {
                    path: created.to_string_lossy().into_owned(),
                    content: "replace".into(),
                    overwrite: false,
                })
                .is_err()
        );
        server
            .write_text_file_inner(&WriteFileInput {
                path: created.to_string_lossy().into_owned(),
                content: "replaced".into(),
                overwrite: true,
            })
            .unwrap();
        assert_eq!(fs::read_to_string(&created).unwrap(), "replaced");
        let moved = root.join("moved.txt");
        server
            .move_file_inner(created.to_str().unwrap(), moved.to_str().unwrap())
            .unwrap();
        assert!(!created.exists() && moved.exists());
        server.delete_file_inner(moved.to_str().unwrap()).unwrap();
        assert!(!moved.exists());
        let new_dir = root.join("new-dir");
        server
            .create_directory_inner(new_dir.to_str().unwrap())
            .unwrap();
        assert!(new_dir.is_dir());
        let command = server
            .run_command_inner(&RunCommandInput {
                program: "pwd".into(),
                args: Vec::new(),
                cwd: root.to_string_lossy().into_owned(),
                timeout_ms: Some(2_000),
                confirm: true,
            })
            .unwrap();
        assert_eq!(command.exit_code, Some(0));
        assert!(command.stdout.trim().ends_with("approved"));
        let timed_out = server
            .run_command_inner(&RunCommandInput {
                program: "sleep".into(),
                args: vec!["1".into()],
                cwd: root.to_string_lossy().into_owned(),
                timeout_ms: Some(100),
                confirm: true,
            })
            .unwrap();
        assert!(timed_out.timed_out);
        let capped = server
            .run_command_inner(&RunCommandInput {
                program: "head".into(),
                args: vec!["-c".into(), "300000".into(), "/dev/zero".into()],
                cwd: root.to_string_lossy().into_owned(),
                timeout_ms: Some(2_000),
                confirm: true,
            })
            .unwrap();
        assert!(capped.stdout_truncated);
        assert!(capped.stdout.len() <= 256 * 1024);
        let not_confirmed = server.run_command_inner(&RunCommandInput {
            program: "pwd".into(),
            args: Vec::new(),
            cwd: root.to_string_lossy().into_owned(),
            timeout_ms: Some(2_000),
            confirm: false,
        });
        assert!(not_confirmed.is_err());

        let terminal_id = server
            .start_terminal_inner(&StartTerminalInput {
                cwd: root.to_string_lossy().into_owned(),
                cols: Some(80),
                rows: Some(24),
                confirm: true,
            })
            .unwrap();
        server
            .write_terminal_inner(&terminal_id, "printf 'MCP_TERMINAL_OK\\n'\n")
            .unwrap();
        let mut terminal_output = String::new();
        for _ in 0..30 {
            terminal_output.push_str(&server.read_terminal_inner(&terminal_id).unwrap().output);
            if terminal_output.contains("MCP_TERMINAL_OK") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            terminal_output.contains("MCP_TERMINAL_OK"),
            "terminal output was: {terminal_output:?}"
        );
        assert!(
            server
                .terminal_sessions
                .lock()
                .unwrap()
                .contains_key(&terminal_id)
        );
        server.close_terminal_inner(&terminal_id).unwrap();
        assert!(
            !server
                .terminal_sessions
                .lock()
                .unwrap()
                .contains_key(&terminal_id)
        );

        let large_file = root.join("large.txt");
        fs::write(&large_file, vec![b'x'; MAX_PREVIEW_BYTES as usize + 1])
            .expect("write oversized test file");
        assert!(
            server
                .read_text_file_inner(large_file.to_str().expect("large file path"))
                .is_err()
        );
        assert!(
            server
                .list_directory_inner(base.to_str().expect("outside path"))
                .is_err()
        );

        // Revoking a root in the shared settings file must affect an already-running MCP process.
        let revoked = serde_json::json!({"version": 1, "folders": []});
        fs::write(
            base.join("approvals.json"),
            serde_json::to_vec(&revoked).expect("serialize revoked approvals"),
        )
        .expect("write revoked approvals");
        assert!(
            server
                .list_directory_inner(root.to_str().expect("revoked root"))
                .is_err()
        );

        fs::remove_dir_all(base).expect("cleanup test files");
    }

    #[test]
    #[cfg(unix)]
    fn mcp_tools_reject_symlink_files() {
        use std::os::unix::fs::symlink;

        let (server, base) = test_server();
        let root = base.join("approved");
        let link = root.join("linked.txt");
        symlink(root.join("note.txt"), &link).expect("create symlink");
        assert!(
            server
                .read_text_file_inner(link.to_str().expect("link path"))
                .is_err()
        );
        fs::remove_dir_all(base).expect("cleanup test files");
    }
}
