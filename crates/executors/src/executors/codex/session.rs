use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::StreamExt;
use regex::Regex;
use tokio::time::{Instant, interval};
use tracing::{info, trace};
use utils::msg_store::MsgStore;

/// Handles session management for Codex
pub struct SessionHandler;

const FALLBACK_POLL_INTERVAL_MS: u64 = 250;
const FALLBACK_TIMEOUT_SECS: u64 = 15;
const FALLBACK_RECENT_WINDOW_SECS: u64 = 10;

impl SessionHandler {
    /// Start monitoring stderr lines for session ID extraction
    pub fn start_session_id_extraction(msg_store: Arc<MsgStore>, current_dir: PathBuf) {
        let session_found = Arc::new(AtomicBool::new(false));

        let stderr_store = msg_store.clone();
        let stderr_flag = session_found.clone();
        tokio::spawn(async move {
            let mut stderr_lines_stream = stderr_store.stderr_lines_stream();

            while let Some(Ok(line)) = stderr_lines_stream.next().await {
                if let Some(session_id) = Self::extract_session_id_from_line(&line) {
                    if !stderr_flag.swap(true, Ordering::SeqCst) {
                        info!("Captured Codex session id from stderr");
                        stderr_store.push_session_id(session_id);
                    }
                    break;
                }
            }
        });

        tokio::spawn({
            let fallback_store = msg_store;
            let fallback_flag = session_found;
            async move {
                let poll_interval = Duration::from_millis(FALLBACK_POLL_INTERVAL_MS);
                let max_duration = Duration::from_secs(FALLBACK_TIMEOUT_SECS);
                let search_started_at = SystemTime::now();
                let deadline = Instant::now() + max_duration;
                let mut ticker = interval(poll_interval);

                loop {
                    ticker.tick().await;

                    if fallback_flag.load(Ordering::SeqCst) {
                        break;
                    }

                    if Instant::now() >= deadline {
                        trace!("Fallback session lookup deadline reached without match");
                        break;
                    }

                    if let Some(session_id) =
                        Self::find_session_id_via_rollouts(&current_dir, search_started_at)
                    {
                        if !fallback_flag.swap(true, Ordering::SeqCst) {
                            info!(
                                "Resolved Codex session id via rollout fallback: {}",
                                session_id
                            );
                            fallback_store.push_session_id(session_id);
                        }
                        break;
                    }
                }
            }
        });
    }

    /// Extract session ID from codex stderr output. Supports:
    /// - Old:  session_id: <uuid>
    /// - New:  session_id: ConversationId(<uuid>)
    pub fn extract_session_id_from_line(line: &str) -> Option<String> {
        static SESSION_ID_REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let regex = SESSION_ID_REGEX.get_or_init(|| {
            Regex::new(r"session_id:\s*(?:ConversationId\()?(?P<id>[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\)?").unwrap()
        });

        regex
            .captures(line)
            .and_then(|cap| cap.name("id"))
            .map(|m| m.as_str().to_string())
    }

    fn find_session_id_via_rollouts(
        current_dir: &Path,
        search_started_at: SystemTime,
    ) -> Option<String> {
        let home_dir = dirs::home_dir()?;
        let sessions_dir = home_dir.join(".codex").join("sessions");

        if !sessions_dir.exists() {
            trace!(
                "Codex sessions directory not found at {}",
                sessions_dir.display()
            );
            return None;
        }

        Self::find_session_id_in_dir(&sessions_dir, current_dir, search_started_at)
    }

    fn find_session_id_in_dir(
        sessions_dir: &Path,
        current_dir: &Path,
        search_started_at: SystemTime,
    ) -> Option<String> {
        let files = Self::collect_session_files(sessions_dir);
        if files.is_empty() {
            trace!(
                "No Codex session files present in {}",
                sessions_dir.display()
            );
            return None;
        }

        let search_started_ms = search_started_at
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as u128)
            .unwrap_or(0);
        let fallback_cutoff_ms =
            search_started_ms.saturating_sub((FALLBACK_RECENT_WINDOW_SECS as u128) * 1_000);

        let mut fallback_candidate: Option<(String, u128)> = None;

        for (path, mtime_ms) in files {
            let (session_id, session_cwd) = match Self::read_session_meta(&path) {
                Some(meta) => meta,
                None => continue,
            };

            if !Self::is_valid_uuid(&session_id) {
                continue;
            }

            if let Some(session_cwd) = session_cwd {
                if Self::path_str_equivalent(&session_cwd, current_dir) {
                    info!(
                        "Matched Codex session id from {} to cwd {}",
                        path.display(),
                        session_cwd
                    );
                    return Some(session_id);
                }
            }

            if fallback_candidate.is_none() && mtime_ms >= fallback_cutoff_ms {
                fallback_candidate = Some((session_id, mtime_ms));
            }
        }

        if let Some((session_id, _)) = fallback_candidate {
            info!(
                "Using fallback Codex session id {} from most recent rollout",
                session_id
            );
            return Some(session_id);
        }

        None
    }

    fn collect_session_files(root: &Path) -> Vec<(PathBuf, u128)> {
        let mut result = Vec::new();
        let mut stack = vec![root.to_path_buf()];

        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(err) => {
                    trace!(
                        "Failed to read Codex session dir {}: {}",
                        dir.display(),
                        err
                    );
                    continue;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }

                let is_jsonl = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
                    .unwrap_or(false);
                if !is_jsonl {
                    continue;
                }

                let mtime_ms = fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_millis() as u128)
                    .unwrap_or(0);

                result.push((path, mtime_ms));
            }
        }

        result.sort_by(|a, b| b.1.cmp(&a.1));
        result
    }

    fn read_session_meta(path: &Path) -> Option<(String, Option<String>)> {
        let file = fs::File::open(path).ok()?;
        let mut reader = BufReader::new(file);
        let mut first_line = String::new();
        reader.read_line(&mut first_line).ok()?;
        let first_line = first_line.trim();
        if first_line.is_empty() {
            return None;
        }

        let json: serde_json::Value = serde_json::from_str(first_line).ok()?;
        let session_id = json
            .get("payload")
            .and_then(|payload| payload.get("id"))
            .and_then(|value| value.as_str())
            .or_else(|| json.get("id").and_then(|value| value.as_str()))?
            .to_string();

        let cwd = json
            .get("payload")
            .and_then(|payload| payload.get("cwd"))
            .and_then(|value| value.as_str())
            .or_else(|| json.get("cwd").and_then(|value| value.as_str()))
            .map(|value| value.to_string());

        Some((session_id, cwd))
    }

    fn is_valid_uuid(candidate: &str) -> bool {
        uuid::Uuid::parse_str(candidate).is_ok()
    }

    fn normalize_path(path: &Path) -> String {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        Self::normalize_string(&canonical.to_string_lossy())
    }

    fn normalize_string<S: AsRef<str>>(input: S) -> String {
        let mut value = input.as_ref().replace('\\', "/");
        if value.is_empty() {
            return value;
        }

        if cfg!(target_os = "windows") {
            value = value.to_lowercase();
        }

        if value.len() > 1 {
            value = value.trim_end_matches('/').to_string();
            if value.is_empty() {
                value = "/".to_string();
            }
        }

        value
    }

    fn strip_private_prefix(value: &str) -> Option<String> {
        if cfg!(target_os = "macos") && value.starts_with("/private/") {
            let stripped = &value["/private/".len()..];
            if stripped.is_empty() {
                Some("/".to_string())
            } else {
                Some(format!("/{}", stripped.trim_start_matches('/')))
            }
        } else {
            None
        }
    }

    fn paths_str_equivalent(left: &str, right: &str) -> bool {
        if left == right {
            return true;
        }

        if let Some(stripped) = Self::strip_private_prefix(left) {
            if stripped == right {
                return true;
            }
        }

        if let Some(stripped) = Self::strip_private_prefix(right) {
            if stripped == left {
                return true;
            }
        }

        false
    }

    fn path_str_equivalent(left: &str, right: &Path) -> bool {
        let left_norm = Self::normalize_string(left);
        let right_norm = Self::normalize_path(right);
        Self::paths_str_equivalent(&left_norm, &right_norm)
    }

    /// Find codex rollout file path for given session_id. Used during follow-up execution.
    pub fn find_rollout_file_path(session_id: &str) -> Result<PathBuf, String> {
        let home_dir = dirs::home_dir().ok_or("Could not determine home directory")?;
        let sessions_dir = home_dir.join(".codex").join("sessions");

        // Scan the sessions directory recursively for rollout files matching the session_id
        // Pattern: rollout-{YYYY}-{MM}-{DD}T{HH}-{mm}-{ss}-{session_id}.jsonl
        Self::scan_directory(&sessions_dir, session_id)
    }

    // Recursively scan directory for rollout files matching the session_id
    fn scan_directory(dir: &PathBuf, session_id: &str) -> Result<PathBuf, String> {
        if !dir.exists() {
            return Err(format!(
                "Sessions directory does not exist: {}",
                dir.display()
            ));
        }

        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("Failed to read directory {}: {}", dir.display(), e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
            let path = entry.path();

            if path.is_dir() {
                // Recursively search subdirectories
                if let Ok(found) = Self::scan_directory(&path, session_id) {
                    return Ok(found);
                }
            } else if path.is_file()
                && let Some(filename) = path.file_name()
                && let Some(filename_str) = filename.to_str()
                && filename_str.contains(session_id)
                && filename_str.starts_with("rollout-")
                && filename_str.ends_with(".jsonl")
            {
                return Ok(path);
            }
        }

        Err(format!(
            "Could not find rollout file for session_id: {session_id}"
        ))
    }

    /// Fork a Codex rollout file by copying it to a temp location and assigning a new session id.
    /// Returns (new_rollout_path, new_session_id).
    ///
    /// Migration behavior:
    /// - If the original header is old format, it is converted to new format on write.
    /// - Subsequent lines:
    ///   - If already new RolloutLine, pass through unchanged.
    ///   - If object contains "record_type", skip it (ignored in old impl).
    ///   - Otherwise, wrap as RolloutLine of type "response_item" with payload = original JSON.
    pub fn fork_rollout_file(session_id: &str) -> Result<(PathBuf, String), String> {
        use std::io::{BufRead, BufReader, Write};

        let original = Self::find_rollout_file_path(session_id)?;

        let file = std::fs::File::open(&original)
            .map_err(|e| format!("Failed to open rollout file {}: {e}", original.display()))?;
        let mut reader = BufReader::new(file);

        let mut first_line = String::new();
        reader
            .read_line(&mut first_line)
            .map_err(|e| format!("Failed to read first line from {}: {e}", original.display()))?;

        let mut meta: serde_json::Value = serde_json::from_str(first_line.trim()).map_err(|e| {
            format!(
                "Failed to parse first line JSON in {}: {e}",
                original.display()
            )
        })?;

        // Generate new UUID for forked session
        let new_id = uuid::Uuid::new_v4().to_string();
        Self::set_session_id_in_rollout_meta(&mut meta, &new_id)?;

        // Prepare destination path in the same directory, following Codex rollout naming convention.
        // Always create a fresh filename: rollout-<YYYY>-<MM>-<DD>T<HH>-<mm>-<ss>-<session_id>.jsonl
        let parent_dir = original
            .parent()
            .ok_or_else(|| format!("Unexpected path with no parent: {}", original.display()))?;
        let new_filename = Self::new_rollout_filename(&new_id);
        let dest = parent_dir.join(new_filename);

        // Write new file with modified first line and copy the rest with migration as needed
        let mut writer = std::fs::File::create(&dest)
            .map_err(|e| format!("Failed to create forked rollout {}: {e}", dest.display()))?;
        let meta_line = serde_json::to_string(&meta)
            .map_err(|e| format!("Failed to serialize modified meta: {e}"))?;
        writeln!(writer, "{meta_line}")
            .map_err(|e| format!("Failed to write meta to {}: {e}", dest.display()))?;

        // Wrap subsequent lines
        for line in reader.lines() {
            let line =
                line.map_err(|e| format!("I/O error reading {}: {e}", original.display()))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Try parse as JSON
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(trimmed);
            let value = match parsed {
                Ok(v) => v,
                Err(_) => {
                    // Skip invalid JSON lines during migration
                    continue;
                }
            };

            // If already a RolloutLine (has timestamp + type/payload or flattened item), pass through
            let is_rollout_line = value.get("timestamp").is_some()
                && (value.get("type").is_some() || value.get("payload").is_some());
            if is_rollout_line {
                writeln!(writer, "{value}")
                    .map_err(|e| format!("Failed to write to {}: {e}", dest.display()))?;
                continue;
            }

            // Ignore legacy bookkeeping lines like {"record_type": ...}
            if value.get("record_type").is_some() {
                continue;
            }

            // Otherwise, wrap as a new RolloutLine containing a ResponseItem payload
            let timestamp = chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string();
            let envelope = serde_json::json!({
                "timestamp": timestamp,
                "type": "response_item",
                "payload": value,
            });
            writeln!(writer, "{envelope}")
                .map_err(|e| format!("Failed to write to {}: {e}", dest.display()))?;
        }

        Ok((dest, new_id))
    }

    // Update session id inside the first-line JSON meta, supporting both old and new formats.
    // - Old format: top-level { "id": "<uuid>", ... } -> convert to new format
    // - New format: { "type": "session_meta", "payload": { "id": "<uuid>", ... }, ... }
    // If both are somehow present, new format takes precedence.
    pub(crate) fn set_session_id_in_rollout_meta(
        meta: &mut serde_json::Value,
        new_id: &str,
    ) -> Result<(), String> {
        match meta {
            serde_json::Value::Object(map) => {
                // If already new format, update payload.id and return
                if let Some(serde_json::Value::Object(payload)) = map.get_mut("payload") {
                    payload.insert(
                        "id".to_string(),
                        serde_json::Value::String(new_id.to_string()),
                    );
                    return Ok(());
                }

                // Convert old format to new format header
                let top_timestamp = map.get("timestamp").cloned();
                let instructions = map.get("instructions").cloned();
                let git = map.get("git").cloned();

                let mut new_top = serde_json::Map::new();
                if let Some(ts) = top_timestamp.clone() {
                    new_top.insert("timestamp".to_string(), ts);
                }
                new_top.insert(
                    "type".to_string(),
                    serde_json::Value::String("session_meta".to_string()),
                );

                let mut payload = serde_json::Map::new();
                payload.insert(
                    "id".to_string(),
                    serde_json::Value::String(new_id.to_string()),
                );
                if let Some(ts) = top_timestamp {
                    payload.insert("timestamp".to_string(), ts);
                }
                if let Some(instr) = instructions {
                    payload.insert("instructions".to_string(), instr);
                }
                if let Some(git_val) = git {
                    payload.insert("git".to_string(), git_val);
                }
                // Required fields in new format: cwd, originator, cli_version
                if !payload.contains_key("cwd") {
                    payload.insert(
                        "cwd".to_string(),
                        serde_json::Value::String(".".to_string()),
                    );
                }
                if !payload.contains_key("originator") {
                    payload.insert(
                        "originator".to_string(),
                        serde_json::Value::String("vibe_kanban_migrated".to_string()),
                    );
                }
                if !payload.contains_key("cli_version") {
                    payload.insert(
                        "cli_version".to_string(),
                        serde_json::Value::String("0.0.0-migrated".to_string()),
                    );
                }

                new_top.insert("payload".to_string(), serde_json::Value::Object(payload));

                *map = new_top; // replace the old map with the new-format one
                Ok(())
            }
            _ => Err("First line of rollout file is not a JSON object".to_string()),
        }
    }

    // Build a new rollout filename, ignoring any original name.
    // Always returns: rollout-<timestamp>-<id>.jsonl
    fn new_rollout_filename(new_id: &str) -> String {
        let now_ts = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string();
        format!("rollout-{now_ts}-{new_id}.jsonl")
    }
}

#[cfg(test)]
mod tests {
    use super::SessionHandler;

    #[test]
    fn test_new_rollout_filename_pattern() {
        let id = "ID-123";
        let out = SessionHandler::new_rollout_filename(id);
        // rollout-YYYY-MM-DDTHH-MM-SS-ID-123.jsonl
        let re = regex::Regex::new(r"^rollout-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-ID-123\.jsonl$")
            .unwrap();
        assert!(re.is_match(&out), "Unexpected filename: {out}");
    }
}
