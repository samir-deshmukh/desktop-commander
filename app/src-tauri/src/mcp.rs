use super::{AccessPolicy, read_bounded_text_file};
use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

const APP_IDENTIFIER: &str = "com.samdeshmukh.desktopcommander";
const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;
const MAX_SEARCH_RESULTS: usize = 200;
const MAX_SEARCH_VISITED: usize = 5_000;
const MAX_SEARCH_DEPTH: usize = 12;

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
            "folders": [root.canonicalize().expect("canonical root").to_string_lossy()]
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
