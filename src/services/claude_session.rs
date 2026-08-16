//! Reading and relocating Claude Code session history.
//!
//! Claude Code stores one JSONL file per session under
//! `~/.claude/projects/<path-with-slashes-replaced-by-dashes>/`.

use crate::models::ClaudeSession;
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Last known sessions per `(path, limit)`. There is no TTL: the panel renders
/// this immediately on selection change while a fresh scan runs behind it, so
/// the worst case is content a few seconds old for the blink before the scan
/// lands — instead of a "Loading…" flash on every switch (#29). The limit is
/// part of the key so a future caller with a different cap cannot poison the
/// entry another consumer peeks.
type SessionCacheKey = (String, usize);

fn cache() -> &'static Mutex<HashMap<SessionCacheKey, Vec<ClaudeSession>>> {
    static CACHE: OnceLock<Mutex<HashMap<SessionCacheKey, Vec<ClaudeSession>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The last result [`get_sessions_for_path`] produced for this path and limit,
/// if any. Safe on the render thread: the lock is held only for the map read.
pub fn peek_sessions(path: &str, limit: usize) -> Option<Vec<ClaudeSession>> {
    cache()
        .lock()
        .ok()?
        .get(&(path.to_string(), limit))
        .cloned()
}

/// Convert a filesystem path to Claude's folder naming convention.
pub fn path_to_claude_folder(path: &str) -> String {
    let resolved = crate::util::expand_and_resolve(path);
    resolved.to_string_lossy().replace('/', "-")
}

pub fn claude_projects_dir() -> PathBuf {
    crate::util::home_dir().join(".claude").join("projects")
}

/// Sessions for a path, newest first, capped at `limit`.
pub fn get_sessions_for_path(path: &str, limit: usize) -> Vec<ClaudeSession> {
    let sessions_dir = claude_projects_dir().join(path_to_claude_folder(path));
    let sessions = read_sessions_dir(&sessions_dir, limit);
    if let Ok(mut guard) = cache().lock() {
        guard.insert((path.to_string(), limit), sessions.clone());
    }
    sessions
}

/// Directory-scoped form of [`get_sessions_for_path`], for testing.
pub fn read_sessions_dir(sessions_dir: &Path, limit: usize) -> Vec<ClaudeSession> {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return Vec::new();
    };

    let mut sessions: Vec<ClaudeSession> = Vec::new();
    for entry in entries.flatten() {
        let file_path = entry.path();
        if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        // Agent transcripts are not user-facing sessions.
        if name.starts_with("agent-") {
            continue;
        }
        if let Some(session) = parse_session_file(&file_path) {
            sessions.push(session);
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.last_timestamp));
    sessions.truncate(limit);
    sessions
}

fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|naive| Utc.from_utc_datetime(&naive))
        })
}

/// Collapse runs of 3+ newlines to 2, preserving single blank lines.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newline_run = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push(ch);
            }
        } else {
            newline_run = 0;
            out.push(ch);
        }
    }
    out
}

fn text_content(data: &serde_json::Value) -> Option<String> {
    let content = data
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| data.get("content"))?;

    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    // Block format: take the first text block.
    if let Some(blocks) = content.as_array() {
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                return block
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(str::to_string);
            }
        }
        return Some(String::new());
    }
    None
}

pub fn parse_session_file(file_path: &Path) -> Option<ClaudeSession> {
    let session_id = file_path.file_stem()?.to_string_lossy().to_string();
    let raw = std::fs::read_to_string(file_path).ok()?;

    let mut title = String::new();
    let mut last_message = String::new();
    let mut last_timestamp: Option<DateTime<Utc>> = None;
    let mut message_count = 0usize;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(data) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        if let Some(ts_raw) = data.get("timestamp").and_then(|t| t.as_str())
            && let Some(ts) = parse_timestamp(ts_raw)
            && last_timestamp.is_none_or(|prev| ts > prev)
        {
            last_timestamp = Some(ts);
        }

        let is_user = data.get("type").and_then(|t| t.as_str()) == Some("user")
            || data.get("role").and_then(|r| r.as_str()) == Some("user");
        if is_user {
            message_count += 1;
            if let Some(content) = text_content(&data)
                && !content.is_empty()
                && !content.starts_with('<')
            {
                let normalized = collapse_blank_lines(&content);
                let clipped: String = normalized.chars().take(100).collect();
                if title.is_empty() {
                    title = clipped.clone();
                }
                last_message = clipped;
            }
        }
    }

    if message_count == 0 {
        return None;
    }

    let last_timestamp = last_timestamp.unwrap_or_else(|| {
        file_path
            .metadata()
            .and_then(|m| m.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now())
    });

    Some(ClaudeSession {
        id: session_id,
        title: if title.is_empty() {
            "Untitled session".to_string()
        } else {
            title
        },
        last_message,
        last_timestamp,
        message_count,
    })
}

/// Move session history from an old worktree path to a new one after a rename.
pub fn migrate_sessions(old_path: &Path, new_path: &Path) {
    let base = claude_projects_dir();
    let old_dir = base.join(path_to_claude_folder(&old_path.to_string_lossy()));
    let new_dir = base.join(path_to_claude_folder(&new_path.to_string_lossy()));
    migrate_dirs(&old_dir, &new_dir);
}

/// Directory-scoped form of [`migrate_sessions`], for testing.
pub fn migrate_dirs(old_dir: &Path, new_dir: &Path) {
    if !old_dir.exists() {
        return;
    }
    if std::fs::create_dir_all(new_dir).is_err() {
        return;
    }

    if let Ok(entries) = std::fs::read_dir(old_dir) {
        for entry in entries.flatten() {
            let src = entry.path();
            if src.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(name) = src.file_name() else {
                continue;
            };
            let dest = new_dir.join(name);
            if !dest.exists() {
                let _ = std::fs::rename(&src, &dest);
            }
        }
    }

    // Remove the old directory once it is empty.
    if let Ok(mut remaining) = std::fs::read_dir(old_dir)
        && remaining.next().is_none()
    {
        let _ = std::fs::remove_dir(old_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The peek is what the panel renders on selection change while the scan
    /// re-runs behind it (#29): whatever the last scan produced, instantly.
    #[test]
    fn peek_returns_the_last_scan() {
        let path = "/nowhere/forestui-peek-test";
        assert!(peek_sessions(path, 5).is_none());
        let scanned = get_sessions_for_path(path, 5);
        let peeked = peek_sessions(path, 5).expect("the scan populates the cache");
        assert_eq!(peeked.len(), scanned.len());
        // A different limit is a different entry, never a poisoned shared one.
        assert!(peek_sessions(path, 50).is_none());
    }

    #[test]
    fn folder_naming_replaces_slashes() {
        let folder = path_to_claude_folder("/tmp");
        assert!(!folder.contains('/'));
        assert!(folder.starts_with('-') || folder.contains("tmp"));
    }

    #[test]
    fn parses_a_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sess-1.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"first question\"},\"gitBranches\":[\"main\"]}\n\
             {\"type\":\"assistant\",\"timestamp\":\"2026-08-01T10:00:05Z\"}\n\
             {\"type\":\"user\",\"timestamp\":\"2026-08-01T10:01:00Z\",\
              \"message\":{\"content\":[{\"type\":\"text\",\"text\":\"second\"}]}}\n",
        )
        .unwrap();

        let session = parse_session_file(&file).unwrap();
        assert_eq!(session.id, "sess-1");
        assert_eq!(session.title, "first question");
        assert_eq!(session.last_message, "second");
        assert_eq!(session.message_count, 2);
    }

    #[test]
    fn skips_agent_files_and_empty_sessions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("agent-x.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("empty.jsonl"), "{\"type\":\"system\"}\n").unwrap();
        assert!(read_sessions_dir(dir.path(), 5).is_empty());
    }

    #[test]
    fn sessions_sorted_newest_first_and_limited() {
        let dir = tempfile::tempdir().unwrap();
        for (i, ts) in [
            "2026-08-01T10:00:00Z",
            "2026-08-03T10:00:00Z",
            "2026-08-02T10:00:00Z",
        ]
        .iter()
        .enumerate()
        {
            std::fs::write(
                dir.path().join(format!("s{i}.jsonl")),
                format!(
                    "{{\"type\":\"user\",\"timestamp\":\"{ts}\",\"message\":{{\"content\":\"q\"}}}}\n"
                ),
            )
            .unwrap();
        }
        let sessions = read_sessions_dir(dir.path(), 2);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "s1");
        assert_eq!(sessions[1].id, "s2");
    }

    #[test]
    fn migration_moves_files_and_removes_empty_dir() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let new = root.path().join("new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("a.jsonl"), "{}").unwrap();

        migrate_dirs(&old, &new);
        assert!(new.join("a.jsonl").exists());
        assert!(!old.exists());
    }

    #[test]
    fn collapse_blank_lines_keeps_single_gap() {
        assert_eq!(collapse_blank_lines("a\n\n\n\nb"), "a\n\nb");
        assert_eq!(collapse_blank_lines("a\nb"), "a\nb");
    }
}
