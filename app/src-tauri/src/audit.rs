use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_AUDIT_BYTES: u64 = 1024 * 1024;
#[cfg(any(feature = "desktop", test))]
const MAX_AUDIT_EVENTS: usize = 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AuditEvent {
    pub timestamp_unix_seconds: u64,
    pub source: String,
    pub action: String,
    pub target: Option<String>,
    pub outcome: String,
}

pub(crate) fn append_event(
    path: &Path,
    source: &str,
    action: &str,
    target: Option<&str>,
    outcome: &str,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create activity directory: {error}"))?;
    }
    if path
        .metadata()
        .map(|metadata| metadata.len() > MAX_AUDIT_BYTES)
        .unwrap_or(false)
    {
        let previous = path.with_extension("jsonl.1");
        let _ = fs::remove_file(&previous);
        fs::rename(path, previous)
            .map_err(|error| format!("Cannot rotate activity log: {error}"))?;
    }

    let event = AuditEvent {
        timestamp_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        source: source.to_owned(),
        action: action.to_owned(),
        target: target.map(str::to_owned),
        outcome: outcome.to_owned(),
    };
    let mut line = serde_json::to_vec(&event)
        .map_err(|error| format!("Cannot serialize activity event: {error}"))?;
    line.push(b'\n');
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Cannot open activity log: {error}"))?;
    file.write_all(&line)
        .map_err(|error| format!("Cannot append activity event: {error}"))?;
    Ok(())
}

#[cfg(any(feature = "desktop", test))]
pub(crate) fn read_recent(path: &Path, limit: usize) -> Result<Vec<AuditEvent>, String> {
    use std::io::{BufRead, BufReader};
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file =
        fs::File::open(path).map_err(|error| format!("Cannot read activity log: {error}"))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|error| format!("Cannot read activity line: {error}"))?;
        if let Ok(event) = serde_json::from_str::<AuditEvent>(&line) {
            events.push(event);
        }
    }
    events.reverse();
    events.truncate(limit.clamp(1, MAX_AUDIT_EVENTS));
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_reads_recent_events_without_recording_payloads() {
        let directory = std::env::temp_dir().join(format!("sam-audit-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let path = directory.join("activity.jsonl");
        append_event(
            &path,
            "MCP",
            "write_text_file",
            Some("/project/note.txt"),
            "success",
        )
        .expect("append event");
        append_event(
            &path,
            "Desktop",
            "terminate_process",
            Some("PID 1234"),
            "success",
        )
        .expect("append second event");
        let events = read_recent(&path, 1).expect("read recent events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "terminate_process");
        assert_eq!(events[0].source, "Desktop");
        assert_eq!(events[0].outcome, "success");
        fs::remove_dir_all(directory).expect("cleanup audit fixture");
    }
}
