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
const MAX_SEARCH_RESULTS: usize = 1_000;
const SEARCH_PAGE_SIZE: usize = 50;
const MAX_SEARCH_SESSIONS: usize = 5;
const MAX_SEARCH_VISITED: usize = 5_000;
const MAX_SEARCH_DEPTH: usize = 12;
const MAX_MULTI_READ_FILES: usize = 10;
const MAX_MULTI_READ_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONTENT_SEARCH_FILES: usize = 1_000;
const MAX_CONTENT_SEARCH_FILE_BYTES: u64 = 256 * 1024;

// High-impact system commands are blocked even when command execution is enabled.
// This is a guardrail, not a sandbox: allowed programs still run with the user's OS permissions.
const BLOCKED_COMMANDS: &[&str] = &[
    "adduser",
    "useradd",
    "usermod",
    "groupadd",
    "chsh",
    "passwd",
    "visudo",
    "sudo",
    "su",
    "doas",
    "pkexec",
    "mkfs",
    "format",
    "mount",
    "umount",
    "fdisk",
    "sfdisk",
    "parted",
    "diskpart",
    "dd",
    "wipefs",
    "blkdiscard",
    "sgdisk",
    "partprobe",
    "losetup",
    "cryptsetup",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "init",
    "iptables",
    "ip6tables",
    "nft",
    "firewall",
    "ufw",
    "netsh",
    "sfc",
    "bcdedit",
    "reg",
    "net",
    "sc",
    "runas",
    "cipher",
    "takeown",
    "kill",
    "killall",
    "pkill",
    "runuser",
    "setpriv",
    "systemd-run",
];

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PathInput {
    /// Absolute path inside a folder approved in Sam's Desktop Commander.
    path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReadFileInput {
    /// Absolute path to a UTF-8 text file inside an approved folder.
    path: String,
    /// Zero-based starting line (default 0).
    offset: Option<i64>,
    /// Maximum number of lines to return (1-2000, default 200).
    length: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EditBlockInput {
    /// Absolute file path inside a folder separately approved for changes.
    path: String,
    /// Exact existing text to replace.
    old_string: String,
    /// Replacement text.
    new_string: String,
    /// Expected number of exact matches (default 1).
    expected_replacements: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct StartSearchInput {
    /// Approved directory in which to search.
    path: String,
    /// Case-insensitive substring to search for.
    query: String,
    /// Search file contents instead of file and directory names.
    search_content: Option<bool>,
    /// Maximum total matches to retain (1-1000, default 200).
    max_results: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchSessionInput {
    /// Search ID returned by start_search.
    search_id: String,
}

#[derive(Serialize)]
struct SearchPage {
    search_id: String,
    results: Vec<String>,
    total_results: usize,
    has_more: bool,
}

#[derive(Serialize)]
struct SearchSessionSummary {
    search_id: String,
    total_results: usize,
    remaining_results: usize,
}

struct SearchSession {
    results: Vec<String>,
    next_index: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchInput {
    /// Approved directory in which to search.
    path: String,
    /// Case-insensitive substring matched against file and directory names.
    query: String,
    /// Maximum matches to return (1-1000).
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
struct AppendFileInput {
    /// Absolute path inside a folder separately approved for changes.
    path: String,
    /// Text to append (maximum 1 MiB; combined file remains limited to 1 MiB).
    content: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CopyFileInput {
    /// Source regular file inside a read-approved folder.
    source: String,
    /// New destination path inside a separately writable folder.
    destination: String,
    /// Must be true to confirm copying.
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

fn resolved_program_name(program: &str) -> String {
    let requested = Path::new(program);
    let resolved = if requested.components().count() > 1 {
        fs::canonicalize(requested).ok()
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|directory| directory.join(requested))
                .find_map(|candidate| fs::canonicalize(candidate).ok())
        })
    };
    resolved
        .as_deref()
        .and_then(Path::file_name)
        .unwrap_or_else(|| requested.file_name().unwrap_or(requested.as_os_str()))
        .to_string_lossy()
        .to_ascii_lowercase()
}

fn blocked_command_reason(program: &str, args: &[String]) -> Option<String> {
    let name = resolved_program_name(program);
    let requested_name = Path::new(program)
        .file_name()
        .unwrap_or_else(|| Path::new(program).as_os_str())
        .to_string_lossy()
        .to_ascii_lowercase();
    let is_blocked =
        |candidate: &str| BLOCKED_COMMANDS.contains(&candidate) || candidate.starts_with("mkfs.");

    if is_blocked(&name) || is_blocked(&requested_name) {
        return Some(format!(
            "'{program}' is blocked by the default system-command policy"
        ));
    }

    // Catch common executable wrappers that would otherwise hide the real command.
    let wrapped_program = if name == "busybox" || requested_name == "busybox" {
        args.first().map(String::as_str)
    } else if name == "env" || requested_name == "env" {
        if args
            .iter()
            .any(|arg| arg == "-S" || arg == "--split-string")
        {
            return Some("'env -S' is blocked because it can hide the executed command".into());
        }
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if arg == "--" {
                index += 1;
                break;
            }
            if matches!(
                arg.as_str(),
                "-u" | "--unset" | "-C" | "--chdir" | "-a" | "--argv0"
            ) {
                index += 2;
                continue;
            }
            if arg.starts_with('-') || (arg.contains('=') && !arg.starts_with('=')) {
                index += 1;
                continue;
            }
            break;
        }
        args.get(index).map(String::as_str)
    } else {
        None
    };
    if let Some(wrapped) = wrapped_program {
        let wrapped_name = resolved_program_name(wrapped);
        let wrapped_requested = Path::new(wrapped)
            .file_name()
            .unwrap_or_else(|| Path::new(wrapped).as_os_str())
            .to_string_lossy()
            .to_ascii_lowercase();
        if is_blocked(&wrapped_name) || is_blocked(&wrapped_requested) {
            return Some(format!(
                "'{program}' cannot be used to launch blocked command '{wrapped}'"
            ));
        }
    }

    // systemctl is useful for diagnostics, but lifecycle actions can end the session.
    if name == "systemctl" || requested_name == "systemctl" {
        if args.iter().any(|arg| {
            matches!(
                arg.to_ascii_lowercase().as_str(),
                "poweroff" | "reboot" | "halt" | "suspend" | "hibernate" | "kexec" | "exit"
            )
        }) {
            return Some(
                "'systemctl' lifecycle actions are blocked by the default system-command policy"
                    .into(),
            );
        }
    }

    if name == "loginctl" || requested_name == "loginctl" {
        if args.iter().any(|arg| {
            matches!(
                arg.to_ascii_lowercase().as_str(),
                "terminate-session"
                    | "terminate-user"
                    | "kill-session"
                    | "kill-user"
                    | "poweroff"
                    | "reboot"
                    | "suspend"
                    | "hibernate"
            )
        }) {
            return Some("'loginctl' session-control actions are blocked by the default system-command policy".into());
        }
    }

    None
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RecentToolCallsInput {
    /// Maximum recent MCP events to return (1-100, default 25).
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SetConfigValueInput {
    /// Configuration key. Supported keys: allowedDirectories, writableDirectories, commandExecutionEnabled.
    key: String,
    /// JSON value matching the selected key (directory arrays or a boolean).
    value: serde_json::Value,
    /// Must be true to confirm changing local permissions or command access.
    confirm: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WritePdfInput {
    /// Existing approved PDF to modify, or destination path for a new PDF.
    path: String,
    /// Markdown string to create a PDF, or an ordered array of page operations.
    content: serde_json::Value,
    /// Required output path when modifying an existing PDF; optional for creation.
    #[serde(rename = "outputPath")]
    output_path: Option<String>,
}

#[derive(Serialize)]
struct McpServerConfig {
    approved_folders: Vec<String>,
    writable_folders: Vec<String>,
    command_execution_enabled: bool,
    max_text_file_bytes: u64,
    max_command_timeout_ms: u64,
    max_terminal_sessions: usize,
    active_terminal_sessions: usize,
    max_search_sessions: usize,
    active_search_sessions: usize,
}

#[derive(Serialize)]
struct UsageStats {
    total_recorded_calls: usize,
    successful_calls: usize,
    failed_calls: usize,
    calls_by_tool: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AccessibilityConfirmInput {
    /// Must be true to inspect application metadata on the desktop accessibility bus.
    confirm: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AccessibilityTreeInput {
    /// Exact application name returned by list_accessible_apps.
    app_name: String,
    /// Maximum tree depth (1-8, default 8).
    max_depth: Option<usize>,
    /// Maximum number of accessible elements (1-200, default 200).
    max_nodes: Option<usize>,
    /// Must be true to read accessible names and roles from the selected application.
    confirm: bool,
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
    audit_file: PathBuf,
    terminal_sessions: Arc<Mutex<std::collections::HashMap<String, McpTerminalSession>>>,
    search_sessions: Arc<Mutex<std::collections::HashMap<String, SearchSession>>>,
}

struct PdfTempDir(PathBuf);

impl PdfTempDir {
    fn new() -> Result<Self, String> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "System clock is before the Unix epoch")?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("sam-pdf-{}-{nonce}", std::process::id()));
        fs::create_dir(&path)
            .map_err(|error| format!("Cannot create temporary PDF workspace: {error}"))?;
        Ok(Self(path))
    }
}

impl Drop for PdfTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn markdown_inline(value: &str) -> String {
    let escaped = html_escape(value);
    let mut output = String::new();
    let mut chars = escaped.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '*' && chars.peek() == Some(&'*') {
            chars.next();
            output.push_str("<strong>");
            let mut inner = String::new();
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'*') {
                    chars.next();
                    break;
                }
                inner.push(next);
            }
            output.push_str(&inner);
            output.push_str("</strong>");
        } else if ch == '`' {
            output.push_str("<code>");
            let mut inner = String::new();
            while let Some(next) = chars.next() {
                if next == '`' {
                    break;
                }
                inner.push(next);
            }
            output.push_str(&inner);
            output.push_str("</code>");
        } else {
            output.push(ch);
        }
    }
    output
}

fn markdown_to_html(markdown: &str) -> String {
    let mut body = String::new();
    let mut paragraph = Vec::<String>::new();
    let mut list_open = false;
    let mut code_open = false;
    let mut code = String::new();
    let flush_paragraph = |body: &mut String, paragraph: &mut Vec<String>| {
        if !paragraph.is_empty() {
            body.push_str("<p>");
            body.push_str(&markdown_inline(&paragraph.join(" ")));
            body.push_str("</p>\n");
            paragraph.clear();
        }
    };
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            flush_paragraph(&mut body, &mut paragraph);
            if list_open {
                body.push_str("</ul>\n");
                list_open = false;
            }
            if code_open {
                body.push_str("<pre><code>");
                body.push_str(&html_escape(&code));
                body.push_str("</code></pre>\n");
                code.clear();
                code_open = false;
            } else {
                code_open = true;
            }
            continue;
        }
        if code_open {
            code.push_str(line);
            code.push('\n');
            continue;
        }
        if trimmed.contains("<div style=\"page-break-before: always;\"></div>")
            || trimmed.contains("<div style='page-break-before: always;'></div>")
        {
            flush_paragraph(&mut body, &mut paragraph);
            if list_open {
                body.push_str("</ul>\n");
                list_open = false;
            }
            body.push_str("<div style=\"page-break-before: always;\"></div>\n");
            continue;
        }
        if trimmed.is_empty() {
            flush_paragraph(&mut body, &mut paragraph);
            if list_open {
                body.push_str("</ul>\n");
                list_open = false;
            }
            continue;
        }
        let heading = (1..=6).find_map(|level| {
            let prefix = format!("{} ", "#".repeat(level));
            trimmed.strip_prefix(&prefix).map(|text| (level, text))
        });
        if let Some((level, text)) = heading {
            flush_paragraph(&mut body, &mut paragraph);
            if list_open {
                body.push_str("</ul>\n");
                list_open = false;
            }
            body.push_str(&format!("<h{level}>{}</h{level}>\n", markdown_inline(text)));
        } else if ["- ", "* ", "+ "]
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
        {
            flush_paragraph(&mut body, &mut paragraph);
            if !list_open {
                body.push_str("<ul>\n");
                list_open = true;
            }
            body.push_str(&format!("<li>{}</li>\n", markdown_inline(&trimmed[2..])));
        } else if let Some(quote) = trimmed.strip_prefix("> ") {
            flush_paragraph(&mut body, &mut paragraph);
            if list_open {
                body.push_str("</ul>\n");
                list_open = false;
            }
            body.push_str(&format!(
                "<blockquote>{}</blockquote>\n",
                markdown_inline(quote)
            ));
        } else {
            if list_open {
                body.push_str("</ul>\n");
                list_open = false;
            }
            paragraph.push(trimmed.to_string());
        }
    }
    flush_paragraph(&mut body, &mut paragraph);
    if list_open {
        body.push_str("</ul>\n");
    }
    if code_open {
        body.push_str("<pre><code>");
        body.push_str(&html_escape(&code));
        body.push_str("</code></pre>\n");
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>@page{{size:A4;margin:20mm}}body{{font-family:'Liberation Sans',sans-serif;font-size:11pt;line-height:1.45;color:#222}}h1,h2,h3,h4,h5,h6{{color:#111;page-break-after:avoid}}pre,code{{font-family:'Liberation Mono',monospace;white-space:pre-wrap}}blockquote{{border-left:3px solid #999;padding-left:12px;color:#555}}li{{margin:3px 0}}</style></head><body>{body}</body></html>"
    )
}

fn run_pdf_command(mut command: Command, description: &str) -> Result<(), String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("Cannot start {description}; install the required PDF utility: {error}")
        })?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|error| format!("Cannot collect {description} output: {error}"))?;
                if output.status.success() {
                    return Ok(());
                }
                return Err(format!(
                    "{description} failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(None) if started.elapsed() > Duration::from_secs(45) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{description} timed out after 45 seconds"));
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                let _ = child.kill();
                return Err(format!("Cannot monitor {description}: {error}"));
            }
        }
    }
}

fn render_markdown_pdf(markdown: &str, workspace: &Path, prefix: &str) -> Result<PathBuf, String> {
    let normalized = markdown
        .replace(
            "<div style=\"page-break-before: always;\"></div>",
            "\n\n__SAM_PDF_PAGE_BREAK__\n\n",
        )
        .replace(
            "<div style='page-break-before: always;'></div>",
            "\n\n__SAM_PDF_PAGE_BREAK__\n\n",
        );
    let sections: Vec<&str> = normalized.split("__SAM_PDF_PAGE_BREAK__").collect();
    if sections.len() == 1 {
        return render_markdown_chunk_pdf(markdown, workspace, prefix);
    }
    let mut pages = Vec::new();
    for (index, section) in sections.iter().enumerate() {
        let part =
            render_markdown_chunk_pdf(section, workspace, &format!("{prefix}-section-{index}"))?;
        pages.extend(split_pdf(
            &part,
            workspace,
            &format!("{prefix}-section-{index}-page"),
        )?);
    }
    if pages.is_empty() {
        return Err("Markdown did not produce any PDF pages".into());
    }
    let merged = workspace.join(format!("{prefix}-sections.pdf"));
    merge_pdfs(&pages, &merged)?;
    Ok(merged)
}

fn render_markdown_chunk_pdf(
    markdown: &str,
    workspace: &Path,
    prefix: &str,
) -> Result<PathBuf, String> {
    let html_path = workspace.join(format!("{prefix}.html"));
    fs::write(&html_path, markdown_to_html(markdown))
        .map_err(|error| format!("Cannot write temporary PDF HTML: {error}"))?;
    let output_dir = workspace.join(format!("{prefix}-rendered"));
    fs::create_dir(&output_dir)
        .map_err(|error| format!("Cannot create PDF render folder: {error}"))?;
    let profile = workspace.join(format!("{prefix}-lo-profile"));
    let mut command = Command::new("libreoffice");
    command.args([
        "--headless",
        &format!("-env:UserInstallation=file://{}", profile.display()),
        "--convert-to",
        "pdf",
        "--outdir",
        output_dir.to_string_lossy().as_ref(),
        html_path.to_string_lossy().as_ref(),
    ]);
    run_pdf_command(command, "LibreOffice PDF rendering")?;
    let rendered = output_dir.join(format!("{prefix}.pdf"));
    if !rendered.is_file() {
        return Err("LibreOffice did not produce a PDF file".into());
    }
    Ok(rendered)
}

fn split_pdf(source: &Path, workspace: &Path, prefix: &str) -> Result<Vec<PathBuf>, String> {
    let pattern = workspace.join(format!("{prefix}-%d.pdf"));
    let mut command = Command::new("pdfseparate");
    command.arg(source).arg(&pattern);
    run_pdf_command(command, "Poppler pdfseparate")?;
    let mut pages = Vec::new();
    for entry in fs::read_dir(workspace)
        .map_err(|error| format!("Cannot list temporary PDF pages: {error}"))?
    {
        let entry = entry.map_err(|error| format!("Cannot inspect temporary PDF page: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(number) = name
            .strip_prefix(&format!("{prefix}-"))
            .and_then(|value| value.strip_suffix(".pdf"))
            .and_then(|value| value.parse::<usize>().ok())
        {
            pages.push((number, entry.path()));
        }
    }
    pages.sort_by_key(|(number, _)| *number);
    Ok(pages.into_iter().map(|(_, path)| path).collect())
}

fn merge_pdfs(pages: &[PathBuf], output: &Path) -> Result<(), String> {
    if pages.is_empty() {
        return Err("Cannot merge an empty PDF page list".into());
    }
    let mut command = Command::new("pdfunite");
    command.args(pages).arg(output);
    run_pdf_command(command, "Poppler pdfunite")
}

fn write_new_file_without_overwrite(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("PDF output path has no parent directory")?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "System clock is before the Unix epoch")?
        .as_nanos();
    let temporary = parent.join(format!(".sam-pdf-{}-{nonce}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("Cannot create temporary PDF output: {error}"))?;
    let write_result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Cannot write or sync temporary PDF output: {error}"
        ));
    }
    match fs::hard_link(&temporary, path) {
        Ok(()) => {
            let _ = fs::remove_file(&temporary);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Err("PDF output already exists; refusing to overwrite it".into())
            } else {
                Err(format!(
                    "Cannot finalize PDF without overwriting an existing file: {error}"
                ))
            }
        }
    }
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
        let result = self.list_directory_inner(&args.path);
        self.record_audit("list_directory", Some(&args.path), result.is_ok());
        match result {
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
        let result = self.read_text_file_inner(&args.path);
        self.record_audit("read_text_file", Some(&args.path), result.is_ok());
        match result {
            Ok(contents) => contents,
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Read a UTF-8 text file by line range. Offset is zero-based; length is capped at 2,000 lines. The file must be inside an approved folder and is limited to 1 MiB."
    )]
    fn read_file(&self, Parameters(args): Parameters<ReadFileInput>) -> String {
        let result = self.read_file_inner(&args);
        self.record_audit("read_file", Some(&args.path), result.is_ok());
        match result {
            Ok(contents) => contents,
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Replace an exact text block in a UTF-8 file inside a separately writable folder. Requires the expected exact match count (default 1); writes atomically and refuses mismatched or ambiguous edits."
    )]
    fn edit_block(&self, Parameters(args): Parameters<EditBlockInput>) -> String {
        let result = self.edit_block_inner(&args);
        self.record_audit("edit_block", Some(&args.path), result.is_ok());
        match result {
            Ok(count) => format!("Replaced {count} occurrence(s) successfully"),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Search file and directory names recursively inside an approved folder. Matches a case-insensitive substring; skips symbolic links; bounded to depth 12, 5,000 visited entries, and at most 200 results."
    )]
    fn search_files(&self, Parameters(args): Parameters<SearchInput>) -> String {
        let result = self.search_files_inner(&args);
        self.record_audit("search_files", Some(&args.path), result.is_ok());
        match result {
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
        let result = self.search_content_inner(&args);
        self.record_audit("search_content", Some(&args.path), result.is_ok());
        match result {
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
        let result = self.write_text_file_inner(&args);
        self.record_audit("write_text_file", Some(&args.path), result.is_ok());
        match result {
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
        let result = self.create_directory_inner(&args.path);
        self.record_audit("create_directory", Some(&args.path), result.is_ok());
        match result {
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
        let result = self.delete_file_inner(&args.path);
        self.record_audit("delete_file", Some(&args.path), result.is_ok());
        match result {
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
        let result = self.move_file_inner(&args.source, &args.destination);
        self.record_audit("move_file", Some(&args.source), result.is_ok());
        match result {
            Ok(()) => "File moved successfully".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Append UTF-8 text to a file inside a separately writable folder. The combined file size is limited to 1 MiB."
    )]
    fn append_file(&self, Parameters(args): Parameters<AppendFileInput>) -> String {
        let result = self.append_file_inner(&args);
        self.record_audit("append_file", Some(&args.path), result.is_ok());
        match result {
            Ok(()) => "Text appended successfully".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Copy one regular file from a read-approved folder to a separately writable folder. Maximum source size is 50 MiB; destination must not exist. Requires confirm=true."
    )]
    fn copy_file(&self, Parameters(args): Parameters<CopyFileInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to copy this file".into();
        }
        let result = super::copy_approved_file(
            &self.policy,
            Path::new(&args.source),
            Path::new(&args.destination),
        );
        self.record_audit("copy_file", Some(&args.source), result.is_ok());
        match result {
            Ok(()) => "File copied successfully".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Run one executable directly (no shell) with separate arguments in an approved working directory. High-impact system administration, disk/partition, shutdown, firewall, and process-termination commands are blocked by default. Requires the owner to enable MCP command execution in the desktop app and confirm=true for each run. Timeout is capped at 30 seconds; stdout and stderr are each capped at 256 KiB. Commands run with the desktop user's OS permissions and are not sandboxed; this blocklist is a guardrail, not a security boundary."
    )]
    fn run_command(&self, Parameters(args): Parameters<RunCommandInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to run this command".into();
        }
        let result = self.run_command_inner(&args);
        self.record_audit("run_command", Some(&args.program), result.is_ok());
        match result {
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
        let result = self.start_terminal_inner(&args);
        self.record_audit("start_terminal", Some(&args.cwd), result.is_ok());
        match result {
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
        let result = self.close_terminal_inner(&args.session_id);
        self.record_audit("close_terminal", Some(&args.session_id), result.is_ok());
        match result {
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
        description = "Start a bounded recursive filename or file-content search in an approved folder. Returns the first 50 matches and a search ID for pagination. Keeps at most 1,000 results and five active search sessions."
    )]
    fn start_search(&self, Parameters(args): Parameters<StartSearchInput>) -> String {
        let result = self.start_search_inner(&args);
        self.record_audit("start_search", Some(&args.path), result.is_ok());
        match result {
            Ok(page) => serde_json::to_string_pretty(&page)
                .unwrap_or_else(|error| format!("ERROR: cannot serialize search page: {error}")),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(description = "Return the next page of up to 50 results from an active search session.")]
    fn get_more_search_results(&self, Parameters(args): Parameters<SearchSessionInput>) -> String {
        match self.get_more_search_results_inner(&args.search_id) {
            Ok(page) => serde_json::to_string_pretty(&page)
                .unwrap_or_else(|error| format!("ERROR: cannot serialize search page: {error}")),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "List active paginated search sessions and their remaining result counts."
    )]
    fn list_searches(&self) -> String {
        match self.search_sessions.lock() {
            Ok(sessions) => {
                let summaries: Vec<_> = sessions
                    .iter()
                    .map(|(id, session)| SearchSessionSummary {
                        search_id: id.clone(),
                        total_results: session.results.len(),
                        remaining_results: session.results.len().saturating_sub(session.next_index),
                    })
                    .collect();
                serde_json::to_string_pretty(&summaries).unwrap_or_else(|error| {
                    format!("ERROR: cannot serialize search sessions: {error}")
                })
            }
            Err(_) => "ERROR: search session state unavailable".into(),
        }
    }

    #[tool(description = "Stop and remove an active search session and its retained results.")]
    fn stop_search(&self, Parameters(args): Parameters<SearchSessionInput>) -> String {
        match self.search_sessions.lock() {
            Ok(mut sessions) => {
                if sessions.remove(&args.search_id).is_some() {
                    "Search session stopped".into()
                } else {
                    "ERROR: search session not found".into()
                }
            }
            Err(_) => "ERROR: search session state unavailable".into(),
        }
    }

    #[tool(
        description = "Read the current MCP server configuration and permission state. Does not expose credentials or file contents."
    )]
    fn get_config(&self) -> String {
        let result = (|| -> Result<McpServerConfig, String> {
            self.policy.refresh()?;
            let approved_folders = self
                .policy
                .approved_roots
                .lock()
                .map_err(|_| "Permission state unavailable")?
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            let writable_folders = self
                .policy
                .writable_roots
                .lock()
                .map_err(|_| "Permission state unavailable")?
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            let command_execution_enabled = self
                .policy
                .command_execution_enabled
                .lock()
                .map_err(|_| "Permission state unavailable")
                .map(|value| *value)?;
            let active_terminal_sessions = self
                .terminal_sessions
                .lock()
                .map_err(|_| "Terminal session state unavailable")?
                .len();
            let active_search_sessions = self
                .search_sessions
                .lock()
                .map_err(|_| "Search session state unavailable")?
                .len();
            Ok(McpServerConfig {
                approved_folders,
                writable_folders,
                command_execution_enabled,
                max_text_file_bytes: MAX_PREVIEW_BYTES,
                max_command_timeout_ms: 30_000,
                max_terminal_sessions: 3,
                active_terminal_sessions,
                max_search_sessions: MAX_SEARCH_SESSIONS,
                active_search_sessions,
            })
        })();
        match result {
            Ok(config) => serde_json::to_string_pretty(&config)
                .unwrap_or_else(|error| format!("ERROR: cannot serialize MCP config: {error}")),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Set a supported local Desktop Commander configuration value. Changing approved folders, write permissions, or command execution requires confirm=true. Empty allowedDirectories is rejected rather than granting unrestricted access."
    )]
    fn set_config_value(&self, Parameters(args): Parameters<SetConfigValueInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to change configuration".into();
        }
        let result = self.set_config_value_inner(&args.key, &args.value);
        self.record_audit("set_config_value", Some(&args.key), result.is_ok());
        match result {
            Ok(()) => format!("Configuration value '{}' updated", args.key),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Create a PDF from Markdown or modify a PDF by inserting/deleting pages. Outputs are written only to approved writable folders and existing files are never overwritten. Requires LibreOffice and Poppler PDF utilities on Linux."
    )]
    fn write_pdf(&self, Parameters(args): Parameters<WritePdfInput>) -> String {
        let result = self.write_pdf_inner(&args);
        let target = args.output_path.as_deref().unwrap_or(&args.path);
        self.record_audit("write_pdf", Some(target), result.is_ok());
        match result {
            Ok(path) => path,
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Return recent MCP tool activity metadata from the local audit log. File contents and command arguments are not recorded."
    )]
    fn get_recent_tool_calls(&self, Parameters(args): Parameters<RecentToolCallsInput>) -> String {
        match super::audit::read_recent(&self.audit_file, args.limit.unwrap_or(25).clamp(1, 100)) {
            Ok(events) => {
                let mcp_events: Vec<_> = events
                    .into_iter()
                    .filter(|event| event.source == "MCP")
                    .collect();
                serde_json::to_string_pretty(&mcp_events).unwrap_or_else(|error| {
                    format!("ERROR: cannot serialize recent tool calls: {error}")
                })
            }
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Return aggregate MCP tool usage counts from the latest 1,000 local audit events. Does not include file contents or command arguments."
    )]
    fn get_usage_stats(&self) -> String {
        match super::audit::read_recent(&self.audit_file, 1_000) {
            Ok(events) => {
                let mut stats = UsageStats {
                    total_recorded_calls: 0,
                    successful_calls: 0,
                    failed_calls: 0,
                    calls_by_tool: std::collections::BTreeMap::new(),
                };
                for event in events.into_iter().filter(|event| event.source == "MCP") {
                    stats.total_recorded_calls += 1;
                    if event.outcome == "success" {
                        stats.successful_calls += 1;
                    } else {
                        stats.failed_calls += 1;
                    }
                    *stats.calls_by_tool.entry(event.action).or_default() += 1;
                }
                serde_json::to_string_pretty(&stats)
                    .unwrap_or_else(|error| format!("ERROR: cannot serialize usage stats: {error}"))
            }
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "List application names and basic window-level metadata exposed by the Linux AT-SPI accessibility bus. Read-only; does not capture screenshots, inspect clipboard contents, or send input. Requires confirm=true. Linux accessibility must already be enabled in desktop settings; this tool never changes that setting."
    )]
    async fn list_accessible_apps(
        &self,
        Parameters(args): Parameters<AccessibilityConfirmInput>,
    ) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to inspect desktop application metadata".into();
        }
        let result = super::accessibility::list_accessible_apps().await;
        self.record_audit("list_accessible_apps", None, result.is_ok());
        match result {
            Ok(apps) => serde_json::to_string_pretty(&apps).unwrap_or_else(|error| {
                format!("ERROR: cannot serialize accessible applications: {error}")
            }),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Read a bounded, read-only accessibility-tree snapshot for one exact application name from list_accessible_apps. Returns element names, roles, depths, and child counts; these may contain visible application text. Requires confirm=true. Maximum 200 nodes and depth 8. Does not click, type, capture screenshots, or change system settings."
    )]
    async fn read_accessibility_tree(
        &self,
        Parameters(args): Parameters<AccessibilityTreeInput>,
    ) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to read this application's accessibility tree".into();
        }
        let result = super::accessibility::read_accessibility_tree(
            &args.app_name,
            args.max_depth,
            args.max_nodes,
        )
        .await;
        self.record_audit(
            "read_accessibility_tree",
            Some(&args.app_name),
            result.is_ok(),
        );
        match result {
            Ok(tree) => serde_json::to_string_pretty(&tree).unwrap_or_else(|error| {
                format!("ERROR: cannot serialize accessibility tree: {error}")
            }),
            Err(error) => format!("ERROR: {error}"),
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
        let result = super::processes::terminate_current_user_process(args.pid);
        let target = args.pid.to_string();
        self.record_audit("terminate_process", Some(&target), result.is_ok());
        match result {
            Ok(()) => format!("SIGTERM sent to process {}", args.pid),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    // Compatibility aliases keep the established Desktop Commander tool names
    // available while reusing the same local permission and session policies.
    #[tool(
        description = "Write or replace a UTF-8 text file in a separately writable approved folder. Existing files require overwrite=true."
    )]
    fn write_file(&self, Parameters(args): Parameters<WriteFileInput>) -> String {
        let result = self.write_text_file_inner(&args);
        self.record_audit("write_file", Some(&args.path), result.is_ok());
        match result {
            Ok(()) => "File written successfully".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "List active interactive terminal sessions. Compatible with Desktop Commander's list_sessions tool."
    )]
    fn list_sessions(&self) -> String {
        self.list_terminal_sessions()
    }

    #[tool(
        description = "Send bounded input to an active interactive terminal session. Compatible with Desktop Commander's interact_with_process tool."
    )]
    fn interact_with_process(&self, Parameters(args): Parameters<TerminalWriteInput>) -> String {
        match self.write_terminal_inner(&args.session_id, &args.data) {
            Ok(()) => "Terminal input sent".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Read available output from an interactive terminal session. Compatible with Desktop Commander's read_process_output tool."
    )]
    fn read_process_output(&self, Parameters(args): Parameters<TerminalSessionInput>) -> String {
        match self.read_terminal_inner(&args.session_id) {
            Ok(output) => serde_json::to_string_pretty(&output).unwrap_or_else(|error| {
                format!("ERROR: cannot serialize terminal output: {error}")
            }),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Terminate an interactive terminal session. Compatible with Desktop Commander's force_terminate tool; only sessions created by this server can be closed."
    )]
    fn force_terminate(&self, Parameters(args): Parameters<TerminalSessionInput>) -> String {
        let result = self.close_terminal_inner(&args.session_id);
        self.record_audit("force_terminate", Some(&args.session_id), result.is_ok());
        match result {
            Ok(()) => "Terminal session closed".into(),
            Err(error) => format!("ERROR: {error}"),
        }
    }

    #[tool(
        description = "Send SIGTERM to a current-user process after explicit confirmation. Compatible with Desktop Commander's kill_process tool."
    )]
    fn kill_process(&self, Parameters(args): Parameters<TerminateProcessInput>) -> String {
        if !args.confirm {
            return "ERROR: set confirm=true to terminate this process".into();
        }
        let result = super::processes::terminate_current_user_process(args.pid);
        let target = args.pid.to_string();
        self.record_audit("kill_process", Some(&target), result.is_ok());
        match result {
            Ok(()) => format!("SIGTERM sent to process {}", args.pid),
            Err(error) => format!("ERROR: {error}"),
        }
    }
}

impl DesktopCommanderMcp {
    fn record_audit(&self, action: &str, target: Option<&str>, success: bool) {
        let _ = super::audit::append_event(
            &self.audit_file,
            "MCP",
            action,
            target,
            if success {
                "success"
            } else {
                "denied_or_failed"
            },
        );
    }

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

    fn read_file_inner(&self, args: &ReadFileInput) -> Result<String, String> {
        let content = self.read_text_file_inner(&args.path)?;
        let lines: Vec<&str> = content.lines().collect();
        let offset = args.offset.unwrap_or(0);
        let start = if offset < 0 {
            lines.len().saturating_sub(offset.unsigned_abs() as usize)
        } else {
            (offset as usize).min(lines.len())
        };
        let length = args.length.unwrap_or(200).clamp(1, 2_000);
        Ok(lines
            .iter()
            .skip(start)
            .take(length)
            .copied()
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn edit_block_inner(&self, args: &EditBlockInput) -> Result<usize, String> {
        if args.old_string.is_empty() {
            return Err("old_string cannot be empty".into());
        }
        let target = self.authorize_path(&args.path)?;
        let original = read_bounded_text_file(&target, MAX_PREVIEW_BYTES)?;
        let count = original.matches(&args.old_string).count();
        let expected = args.expected_replacements.unwrap_or(1);
        if expected == 0 || expected > 1_000 {
            return Err("expected_replacements must be between 1 and 1000".into());
        }
        if count != expected {
            return Err(format!(
                "Expected {expected} exact match(es), found {count}; file was not changed"
            ));
        }
        let updated = original.replace(&args.old_string, &args.new_string);
        super::write_approved_text_file(&self.policy, &target, &updated, true)?;
        Ok(count)
    }

    fn start_search_inner(&self, args: &StartSearchInput) -> Result<SearchPage, String> {
        if args.query.trim().is_empty() {
            return Err("Search query cannot be empty".into());
        }
        let limit = args.max_results.unwrap_or(200).clamp(1, MAX_SEARCH_RESULTS);
        let results = if args.search_content.unwrap_or(false) {
            self.search_content_inner(&ContentSearchInput {
                path: args.path.clone(),
                query: args.query.clone(),
                max_results: Some(limit),
            })?
        } else {
            self.search_files_inner(&SearchInput {
                path: args.path.clone(),
                query: args.query.clone(),
                max_results: Some(limit),
            })?
        };
        let search_id = format!(
            "search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let first_end = results.len().min(SEARCH_PAGE_SIZE);
        let page = SearchPage {
            search_id: search_id.clone(),
            results: results[..first_end].to_vec(),
            total_results: results.len(),
            has_more: first_end < results.len(),
        };
        let mut sessions = self
            .search_sessions
            .lock()
            .map_err(|_| "Search session state unavailable")?;
        if sessions.len() >= MAX_SEARCH_SESSIONS {
            return Err("At most five search sessions may be active".into());
        }
        sessions.insert(
            search_id,
            SearchSession {
                results,
                next_index: first_end,
            },
        );
        Ok(page)
    }

    fn get_more_search_results_inner(&self, search_id: &str) -> Result<SearchPage, String> {
        let mut sessions = self
            .search_sessions
            .lock()
            .map_err(|_| "Search session state unavailable")?;
        let session = sessions
            .get_mut(search_id)
            .ok_or("Search session not found")?;
        let end = session
            .next_index
            .saturating_add(SEARCH_PAGE_SIZE)
            .min(session.results.len());
        let page = SearchPage {
            search_id: search_id.to_owned(),
            results: session.results[session.next_index..end].to_vec(),
            total_results: session.results.len(),
            has_more: end < session.results.len(),
        };
        session.next_index = end;
        Ok(page)
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
        if !self.policy.command_execution_allowed()? {
            let _ = self.close_terminal_inner(session_id);
            return Err("MCP command execution was disabled; terminal session closed".into());
        }
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
        if !self.policy.command_execution_allowed()? {
            let _ = self.close_terminal_inner(session_id);
            return Err("MCP command execution was disabled; terminal session closed".into());
        }
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
        if let Some(reason) = blocked_command_reason(&args.program, &args.args) {
            return Err(reason);
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

    fn write_pdf_inner(&self, args: &WritePdfInput) -> Result<String, String> {
        const MAX_PDF_BYTES: u64 = 20 * 1024 * 1024;
        const MAX_PDF_PAGES: usize = 300;
        let markdown = args.content.as_str();
        let creating = markdown.is_some();
        let output_path = if creating {
            args.output_path.as_deref().unwrap_or(&args.path)
        } else {
            args.output_path
                .as_deref()
                .ok_or("outputPath is required when modifying an existing PDF")?
        };
        let output = self.policy.authorize_write_target(Path::new(output_path))?;
        if !output
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            return Err("PDF output path must use the .pdf extension".into());
        }
        if output.exists() {
            return Err("PDF output already exists; choose a new filename".into());
        }

        let temp = PdfTempDir::new()?;
        let generated = if let Some(markdown) = markdown {
            if markdown.len() as u64 > MAX_PDF_BYTES {
                return Err("Markdown content exceeds the 20 MiB PDF input limit".into());
            }
            render_markdown_pdf(markdown, &temp.0, "created")?
        } else {
            let source = self.policy.authorize_path(Path::new(&args.path))?;
            let metadata = fs::metadata(&source)
                .map_err(|error| format!("Cannot inspect source PDF: {error}"))?;
            if metadata.len() > MAX_PDF_BYTES {
                return Err("Source PDF exceeds the 20 MiB input limit".into());
            }
            if !source
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            {
                return Err("Source file must use the .pdf extension".into());
            }
            if source == output {
                return Err(
                    "PDF modifications must write to a different file than the source".into(),
                );
            }
            let signature =
                fs::read(&source).map_err(|error| format!("Cannot read source PDF: {error}"))?;
            if !signature.starts_with(b"%PDF-") {
                return Err("Source file does not have a valid PDF signature".into());
            }
            let mut pages = split_pdf(&source, &temp.0, "source")?;
            if pages.is_empty() || pages.len() > MAX_PDF_PAGES {
                return Err(format!(
                    "Source PDF must contain between 1 and {MAX_PDF_PAGES} pages"
                ));
            }
            let operations = args
                .content
                .as_array()
                .ok_or("PDF content must be Markdown text or an array of page operations")?;
            for (operation_index, operation) in operations.iter().enumerate() {
                let kind = operation
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        format!("PDF operation {} requires a type", operation_index + 1)
                    })?;
                match kind {
                    "delete" => {
                        let indexes = operation
                            .get("pageIndexes")
                            .and_then(serde_json::Value::as_array)
                            .ok_or("delete operation requires pageIndexes as an array")?;
                        if indexes.is_empty() {
                            return Err("delete operation pageIndexes cannot be empty".into());
                        }
                        let mut remove = Vec::with_capacity(indexes.len());
                        for index in indexes {
                            let index = index
                                .as_u64()
                                .and_then(|value| usize::try_from(value).ok())
                                .ok_or("pageIndexes must contain non-negative integer indexes")?;
                            if index >= pages.len() {
                                return Err(format!(
                                    "Delete page index {index} is outside the current {}-page PDF",
                                    pages.len()
                                ));
                            }
                            if remove.contains(&index) {
                                return Err(format!("Delete page index {index} is repeated"));
                            }
                            remove.push(index);
                        }
                        remove.sort_unstable();
                        for index in remove.into_iter().rev() {
                            pages.remove(index);
                        }
                        if pages.is_empty() {
                            return Err("PDF operations cannot remove every page".into());
                        }
                    }
                    "insert" => {
                        let index = operation
                            .get("pageIndex")
                            .and_then(serde_json::Value::as_u64)
                            .and_then(|value| usize::try_from(value).ok())
                            .ok_or("insert operation requires a non-negative pageIndex")?;
                        if index > pages.len() {
                            return Err(format!(
                                "Insert page index {index} exceeds the current page count {}",
                                pages.len()
                            ));
                        }
                        let inserted = if let Some(markdown) = operation
                            .get("markdown")
                            .and_then(serde_json::Value::as_str)
                        {
                            if markdown.len() as u64 > MAX_PDF_BYTES {
                                return Err(
                                    "Inserted Markdown exceeds the 20 MiB input limit".into()
                                );
                            }
                            let inserted_pdf = render_markdown_pdf(
                                markdown,
                                &temp.0,
                                &format!("insert-{operation_index}"),
                            )?;
                            split_pdf(
                                &inserted_pdf,
                                &temp.0,
                                &format!("insert-{operation_index}-page"),
                            )?
                        } else if let Some(source_pdf) = operation
                            .get("sourcePdfPath")
                            .and_then(serde_json::Value::as_str)
                        {
                            let source_pdf = self.policy.authorize_path(Path::new(source_pdf))?;
                            let metadata = fs::metadata(&source_pdf)
                                .map_err(|error| format!("Cannot inspect inserted PDF: {error}"))?;
                            if metadata.len() > MAX_PDF_BYTES {
                                return Err("Inserted PDF exceeds the 20 MiB input limit".into());
                            }
                            if !source_pdf
                                .extension()
                                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
                            {
                                return Err("Inserted source must use the .pdf extension".into());
                            }
                            split_pdf(
                                &source_pdf,
                                &temp.0,
                                &format!("insert-{operation_index}-page"),
                            )?
                        } else {
                            return Err(
                                "insert operation requires markdown or sourcePdfPath".into()
                            );
                        };
                        if inserted.is_empty() {
                            return Err("Inserted PDF contains no pages".into());
                        }
                        pages.splice(index..index, inserted);
                        if pages.len() > MAX_PDF_PAGES {
                            return Err(format!(
                                "PDF operations exceed the {MAX_PDF_PAGES}-page limit"
                            ));
                        }
                    }
                    _ => return Err(format!("Unsupported PDF operation type: {kind}")),
                }
            }
            let modified = temp.0.join("modified.pdf");
            merge_pdfs(&pages, &modified)?;
            modified
        };

        let metadata = fs::metadata(&generated)
            .map_err(|error| format!("PDF generation did not produce an output file: {error}"))?;
        if metadata.len() == 0 || metadata.len() > MAX_PDF_BYTES {
            return Err("Generated PDF is empty or exceeds the 20 MiB output limit".into());
        }
        let bytes =
            fs::read(&generated).map_err(|error| format!("Cannot read generated PDF: {error}"))?;
        if !bytes.starts_with(b"%PDF-") {
            return Err("Generated output does not have a valid PDF signature".into());
        }
        write_new_file_without_overwrite(&output, &bytes)?;
        Ok(format!("PDF written successfully: {}", output.display()))
    }

    fn set_config_value_inner(&self, key: &str, value: &serde_json::Value) -> Result<(), String> {
        self.policy.refresh()?;
        match key {
            "allowedDirectories" | "approved_folders" => {
                let values = value
                    .as_array()
                    .ok_or("allowedDirectories must be an array of existing directory paths")?;
                if values.is_empty() {
                    return Err("An empty allowedDirectories list would grant unrestricted access; use the app's folder approval controls instead".into());
                }
                let mut updated = Vec::with_capacity(values.len());
                for item in values {
                    let path = item
                        .as_str()
                        .ok_or("Every allowedDirectories entry must be a string")?;
                    let canonical = super::canonical_directory(path)?;
                    if !updated.contains(&canonical) {
                        updated.push(canonical);
                    }
                }
                updated.sort();
                let writable = self
                    .policy
                    .writable_roots
                    .lock()
                    .map_err(|_| "Permission state unavailable")?
                    .clone();
                let writable: Vec<PathBuf> = writable
                    .into_iter()
                    .filter(|root| updated.contains(root))
                    .collect();
                let command_enabled = *self
                    .policy
                    .command_execution_enabled
                    .lock()
                    .map_err(|_| "Permission state unavailable")?;
                self.policy.persist_with(&updated, &writable, command_enabled)?;
                *self
                    .policy
                    .approved_roots
                    .lock()
                    .map_err(|_| "Permission state unavailable")? = updated;
                *self
                    .policy
                    .writable_roots
                    .lock()
                    .map_err(|_| "Permission state unavailable")? = writable;
                Ok(())
            }
            "writableDirectories" | "writable_folders" => {
                let values = value
                    .as_array()
                    .ok_or("writableDirectories must be an array of approved directory paths")?;
                let approved = self
                    .policy
                    .approved_roots
                    .lock()
                    .map_err(|_| "Permission state unavailable")?
                    .clone();
                let mut updated = Vec::with_capacity(values.len());
                for item in values {
                    let path = item
                        .as_str()
                        .ok_or("Every writableDirectories entry must be a string")?;
                    let canonical = super::canonical_directory(path)?;
                    if !approved.contains(&canonical) {
                        return Err(format!("Write access can only be enabled for already approved folders: {}", canonical.display()));
                    }
                    if !updated.contains(&canonical) {
                        updated.push(canonical);
                    }
                }
                updated.sort();
                let command_enabled = *self
                    .policy
                    .command_execution_enabled
                    .lock()
                    .map_err(|_| "Permission state unavailable")?;
                self.policy.persist_with(&approved, &updated, command_enabled)?;
                *self
                    .policy
                    .writable_roots
                    .lock()
                    .map_err(|_| "Permission state unavailable")? = updated;
                Ok(())
            }
            "commandExecutionEnabled" | "command_execution_enabled" => {
                let enabled = value
                    .as_bool()
                    .ok_or("commandExecutionEnabled must be a boolean")?;
                let approved = self
                    .policy
                    .approved_roots
                    .lock()
                    .map_err(|_| "Permission state unavailable")?
                    .clone();
                let writable = self
                    .policy
                    .writable_roots
                    .lock()
                    .map_err(|_| "Permission state unavailable")?
                    .clone();
                self.policy.persist_with(&approved, &writable, enabled)?;
                *self
                    .policy
                    .command_execution_enabled
                    .lock()
                    .map_err(|_| "Permission state unavailable")? = enabled;
                Ok(())
            }
            _ => Err("Unsupported configuration key. Supported keys: allowedDirectories, writableDirectories, commandExecutionEnabled".into()),
        }
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

    fn append_file_inner(&self, args: &AppendFileInput) -> Result<(), String> {
        let target = self.policy.authorize_write_target(Path::new(&args.path))?;
        let mut content = if target.exists() {
            read_bounded_text_file(&target, MAX_PREVIEW_BYTES)?
        } else {
            String::new()
        };
        if content.len().saturating_add(args.content.len()) > MAX_PREVIEW_BYTES as usize {
            return Err(format!(
                "Combined file content exceeds the {MAX_PREVIEW_BYTES}-byte limit"
            ));
        }
        content.push_str(&args.content);
        super::write_approved_text_file(&self.policy, &target, &content, true)
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
        let limit = args.max_results.unwrap_or(200).clamp(1, MAX_SEARCH_RESULTS);
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

    let audit_file = policy.audit_path();
    let server = DesktopCommanderMcp {
        policy: Arc::new(policy),
        audit_file,
        terminal_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        search_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
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
        let audit_file = policy.audit_path();
        let server = DesktopCommanderMcp {
            policy: Arc::new(policy),
            audit_file,
            terminal_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            search_sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        };
        (server, base)
    }

    #[test]
    fn command_policy_blocks_high_impact_commands_and_allows_development_tools() {
        for program in [
            "sudo",
            "/usr/bin/sudo",
            "su",
            "passwd",
            "mkfs",
            "mkfs.ext4",
            "fdisk",
            "dd",
            "wipefs",
            "shutdown",
            "reboot",
            "iptables",
            "pkexec",
            "kill",
            "pkill",
            "killall",
        ] {
            assert!(
                blocked_command_reason(program, &[]).is_some(),
                "expected {program} to be blocked"
            );
        }

        assert!(blocked_command_reason("systemctl", &["reboot".into()]).is_some());
        assert!(
            blocked_command_reason("systemctl", &["--no-block".into(), "reboot".into()]).is_some()
        );
        assert!(blocked_command_reason("loginctl", &["terminate-session".into()]).is_some());
        assert!(blocked_command_reason("env", &["sudo".into(), "id".into()]).is_some());
        assert!(
            blocked_command_reason("env", &["-i".into(), "PATH=/usr/bin".into(), "sudo".into()])
                .is_some()
        );
        assert!(blocked_command_reason("busybox", &["sudo".into()]).is_some());
        assert!(
            blocked_command_reason(
                "env",
                &["--argv0".into(), "sudo".into(), format!("re{}", "boot")]
            )
            .is_some()
        );
        assert!(blocked_command_reason("env", &["-S".into(), "sudo reboot".into()]).is_some());
        for program in [
            "git", "npm", "node", "python", "pip", "cargo", "rustc", "curl", "wget",
        ] {
            assert!(
                blocked_command_reason(program, &[]).is_none(),
                "expected {program} to remain available"
            );
        }

        if Path::new("/usr/bin/sudo").exists() {
            let alias = std::env::temp_dir()
                .join(format!("sam-blocked-command-alias-{}", std::process::id()));
            let _ = fs::remove_file(&alias);
            std::os::unix::fs::symlink("/usr/bin/sudo", &alias).expect("create sudo alias");
            assert!(blocked_command_reason(alias.to_str().unwrap(), &[]).is_some());
            fs::remove_file(alias).expect("remove sudo alias");
        }
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
        fs::write(root.join("lines.txt"), "first\nsecond\nthird\n").unwrap();
        assert_eq!(
            server
                .read_file_inner(&ReadFileInput {
                    path: root.join("lines.txt").to_string_lossy().into_owned(),
                    offset: Some(1),
                    length: Some(1),
                })
                .unwrap(),
            "second"
        );
        assert_eq!(
            server
                .read_file_inner(&ReadFileInput {
                    path: root.join("lines.txt").to_string_lossy().into_owned(),
                    offset: Some(-2),
                    length: Some(2),
                })
                .unwrap(),
            "second\nthird"
        );
        fs::write(root.join("edit.txt"), "alpha beta alpha").unwrap();
        assert!(
            server
                .edit_block_inner(&EditBlockInput {
                    path: root.join("edit.txt").to_string_lossy().into_owned(),
                    old_string: "alpha".into(),
                    new_string: "gamma".into(),
                    expected_replacements: None,
                })
                .is_err(),
            "ambiguous default edit must be rejected"
        );
        assert_eq!(
            fs::read_to_string(root.join("edit.txt")).unwrap(),
            "alpha beta alpha"
        );
        assert_eq!(
            server
                .edit_block_inner(&EditBlockInput {
                    path: root.join("edit.txt").to_string_lossy().into_owned(),
                    old_string: "alpha".into(),
                    new_string: "gamma".into(),
                    expected_replacements: Some(2),
                })
                .unwrap(),
            2
        );
        assert_eq!(
            fs::read_to_string(root.join("edit.txt")).unwrap(),
            "gamma beta gamma"
        );

        for index in 0..60 {
            fs::write(root.join(format!("match-{index:03}.txt")), "search fixture").unwrap();
        }
        let search_page = server
            .start_search_inner(&StartSearchInput {
                path: root.to_string_lossy().into_owned(),
                query: "match-".into(),
                search_content: Some(false),
                max_results: Some(100),
            })
            .unwrap();
        assert_eq!(search_page.results.len(), 50);
        assert!(search_page.has_more);
        let next_page = server
            .get_more_search_results_inner(&search_page.search_id)
            .unwrap();
        assert_eq!(next_page.results.len(), 10);
        assert!(!next_page.has_more);
        assert!(
            server
                .search_sessions
                .lock()
                .unwrap()
                .contains_key(&search_page.search_id)
        );
        server
            .search_sessions
            .lock()
            .unwrap()
            .remove(&search_page.search_id);

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
        let append_target = root.join("append.txt");
        server
            .append_file_inner(&AppendFileInput {
                path: append_target.to_string_lossy().into_owned(),
                content: "one".into(),
            })
            .unwrap();
        server
            .append_file_inner(&AppendFileInput {
                path: append_target.to_string_lossy().into_owned(),
                content: " two".into(),
            })
            .unwrap();
        assert_eq!(fs::read_to_string(&append_target).unwrap(), "one two");
        let copy_target = root.join("copied-note.txt");
        super::super::copy_approved_file(&server.policy, &root.join("note.txt"), &copy_target)
            .unwrap();
        assert_eq!(fs::read_to_string(&copy_target).unwrap(), "safe test text");

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
        let blocked = server.run_command_inner(&RunCommandInput {
            program: "sudo".into(),
            args: vec!["true".into()],
            cwd: root.to_string_lossy().into_owned(),
            timeout_ms: Some(2_000),
            confirm: true,
        });
        assert!(blocked.is_err());

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

        let revoked_terminal = server
            .start_terminal_inner(&StartTerminalInput {
                cwd: root.to_string_lossy().into_owned(),
                cols: Some(80),
                rows: Some(24),
                confirm: true,
            })
            .unwrap();
        let policy_file = base.join("approvals.json");
        let approved_root = root.canonicalize().unwrap();
        fs::write(
            &policy_file,
            serde_json::json!({
                "version": 1,
                "folders": [approved_root.to_string_lossy()],
                "writable_folders": [approved_root.to_string_lossy()],
                "command_execution_enabled": false
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            server
                .write_terminal_inner(&revoked_terminal, "echo must-not-run\n")
                .is_err()
        );
        assert!(
            !server
                .terminal_sessions
                .lock()
                .unwrap()
                .contains_key(&revoked_terminal)
        );
        fs::write(
            &policy_file,
            serde_json::json!({
                "version": 1,
                "folders": [approved_root.to_string_lossy()],
                "writable_folders": [approved_root.to_string_lossy()],
                "command_execution_enabled": true
            })
            .to_string(),
        )
        .unwrap();
        server.policy.refresh().unwrap();

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
