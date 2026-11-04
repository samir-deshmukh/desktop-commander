use super::{AccessPolicy, read_bounded_text_file};
use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
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

    fn write_text_file_inner(&self, args: &WriteFileInput) -> Result<(), String> {
        if args.content.len() as u64 > MAX_PREVIEW_BYTES {
            return Err(format!(
                "Text content exceeds the {MAX_PREVIEW_BYTES}-byte limit"
            ));
        }
        let target = self.policy.authorize_write_target(Path::new(&args.path))?;
        let exists = target.exists();
        if exists && !args.overwrite {
            return Err("File already exists; set overwrite=true to replace it".into());
        }
        if exists
            && !fs::symlink_metadata(&target)
                .map_err(|error| format!("Cannot inspect existing file: {error}"))?
                .is_file()
        {
            return Err("Only regular files can be overwritten".into());
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary =
            target.with_file_name(format!(".sam-commander-{}-{nonce}.tmp", std::process::id()));
        let result = (|| -> Result<(), String> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| format!("Cannot create temporary file: {error}"))?;
            file.write_all(args.content.as_bytes())
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

    fn create_directory_inner(&self, path: &str) -> Result<(), String> {
        let target = self.policy.authorize_write_target(Path::new(path))?;
        fs::create_dir(&target).map_err(|error| format!("Cannot create directory: {error}"))
    }

    fn delete_file_inner(&self, path: &str) -> Result<(), String> {
        let target = self.policy.authorize_write_target(Path::new(path))?;
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| format!("Cannot inspect file to delete: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(
                "Only regular files can be deleted; directories and symbolic links are rejected"
                    .into(),
            );
        }
        fs::remove_file(&target).map_err(|error| format!("Cannot delete file: {error}"))
    }

    fn move_file_inner(&self, source: &str, destination: &str) -> Result<(), String> {
        let source = self.policy.authorize_write_target(Path::new(source))?;
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| format!("Cannot inspect source file: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("Only regular files can be moved".into());
        }
        let destination = self.policy.authorize_write_target(Path::new(destination))?;
        if destination.exists() {
            return Err("Destination already exists; refusing to overwrite".into());
        }
        fs::rename(&source, &destination).map_err(|error| format!("Cannot move file: {error}"))
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
            "writable_folders": [root.canonicalize().expect("canonical writable root").to_string_lossy()]
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
