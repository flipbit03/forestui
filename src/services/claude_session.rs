//! Reading and relocating Claude Code session history.
//!
//! Claude Code stores one JSONL file per session under
//! `~/.claude/projects/<path-with-slashes-replaced-by-dashes>/`.

use crate::models::{ClaudeSession, SessionTurn, Speaker};
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

/// What the directory looked like when a cached answer was built: how many
/// transcripts there were and the newest modification time among them.
///
/// This is what makes a frequent refresh affordable. Parsing is the expensive
/// half — every transcript is read whole and every line JSON-parsed, and a busy
/// conversation runs to megabytes — while listing the directory and stat-ing it
/// is not. An unchanged fingerprint means an unchanged answer, so the sweep
/// costs one `readdir` on the overwhelmingly common tick where nobody has said
/// anything.
type Fingerprint = (usize, Option<std::time::SystemTime>);

type CacheEntry = (Fingerprint, Vec<ClaudeSession>);

fn cache() -> &'static Mutex<HashMap<SessionCacheKey, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<SessionCacheKey, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The last result [`get_sessions_for_path`] produced for this path and limit,
/// if any. Safe on the render thread: the lock is held only for the map read.
pub fn peek_sessions(path: &str, limit: usize) -> Option<Vec<ClaudeSession>> {
    cache()
        .lock()
        .ok()?
        .get(&(path.to_string(), limit))
        .map(|(_, sessions)| sessions.clone())
}

/// Count of transcripts and the newest mtime among them. Agent transcripts are
/// skipped here for the same reason the scan skips them, so one starting does
/// not read as a change.
fn fingerprint(sessions_dir: &Path) -> Fingerprint {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return (0, None);
    };
    let mut count = 0usize;
    let mut newest: Option<std::time::SystemTime> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("agent-"))
        {
            continue;
        }
        count += 1;
        if let Ok(modified) = entry.metadata().and_then(|m| m.modified())
            && newest.is_none_or(|prev| modified > prev)
        {
            newest = Some(modified);
        }
    }
    (count, newest)
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
    let key = (path.to_string(), limit);
    let current = fingerprint(&sessions_dir);

    // Nothing in the directory has changed since the cached answer was built,
    // so re-reading every transcript would produce the same list.
    if let Ok(guard) = cache().lock()
        && let Some((cached, sessions)) = guard.get(&key)
        && *cached == current
    {
        return sessions.clone();
    }

    let sessions = read_sessions_dir(&sessions_dir, limit);
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, (current, sessions.clone()));
    }
    sessions
}

/// Re-read one transcript. `None` means it is gone or no longer has any
/// messages, which is how a session that was deleted while forestui was in the
/// background leaves the list.
///
/// This is the per-card refresh: five transcripts re-read concurrently answer
/// far sooner than one walk of a directory that may hold dozens, and each card
/// updates the moment its own read lands rather than all of them together.
pub fn refresh_one(path: &str, session_id: &str) -> Option<ClaudeSession> {
    let file = claude_projects_dir()
        .join(path_to_claude_folder(path))
        .join(format!("{session_id}.jsonl"));
    parse_session_file(&file)
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

/// A non-empty string field, trimmed. Claude re-appends title records, so the
/// last one wins; an empty value is treated as no name at all.
fn title_field(data: &serde_json::Value, key: &str) -> Option<String> {
    let value = data.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Whether a `user` record is something the person actually sent.
///
/// `user` is the transport, not the author: tool results come back as `user`
/// records, and so do messages the system injects — a background agent
/// reporting in, a task notification. `origin.kind` separates them. A record
/// with no `origin` predates the field, so it is taken at face value rather
/// than dropped, which would erase the older half of a long conversation.
fn is_from_the_user(data: &serde_json::Value) -> bool {
    match data
        .get("origin")
        .and_then(|o| o.get("kind"))
        .and_then(|k| k.as_str())
    {
        Some(kind) => kind == "human",
        None => true,
    }
}

/// One line of preview text.
///
/// Whitespace is flattened before clipping because a card draws a turn as a
/// single line: a real message whose first line ended a sentence came out as
/// `painho:**Repo` — two words from different lines with nothing between them.
fn clip(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(100)
        .collect()
}

/// Record a turn, keeping only the last two.
///
/// Consecutive records from the same speaker are one turn: a single answer is
/// usually several `assistant` records — text, a tool call, more text — and one
/// real session had runs of up to 77 of them. Taken as separate turns they
/// would fill both lines and hide the question they answer, which is the half
/// that identifies the conversation.
fn push_turn(turns: &mut Vec<SessionTurn>, speaker: Speaker, text: String) {
    match turns.last_mut() {
        Some(last) if last.speaker == speaker => last.text = text,
        _ => {
            turns.push(SessionTurn { speaker, text });
            if turns.len() > 2 {
                turns.remove(0);
            }
        }
    }
}

pub fn parse_session_file(file_path: &Path) -> Option<ClaudeSession> {
    let session_id = file_path.file_stem()?.to_string_lossy().to_string();
    let raw = std::fs::read_to_string(file_path).ok()?;

    let mut first_prompt = String::new();
    let mut custom_title: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut turns: Vec<SessionTurn> = Vec::new();
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

        // The names Claude itself records. A forked transcript carries the
        // parent's entries verbatim, so only entries stamped with this file's
        // own session id are allowed to name it.
        if data.get("sessionId").and_then(|s| s.as_str()) == Some(session_id.as_str()) {
            match data.get("type").and_then(|t| t.as_str()) {
                Some("custom-title") => custom_title = title_field(&data, "customTitle"),
                Some("ai-title") => ai_title = title_field(&data, "aiTitle"),
                _ => {}
            }
        }

        let is_user = data.get("type").and_then(|t| t.as_str()) == Some("user")
            || data.get("role").and_then(|r| r.as_str()) == Some("user");
        // Everything below counts turns the person took, so the count and the
        // preview agree with each other and with what they remember saying. It
        // used to count every `user` record, which on a long conversation is
        // mostly tool results: 5528 where 335 messages were sent.
        if is_user
            && is_from_the_user(&data)
            && let Some(content) = text_content(&data)
            && !content.is_empty()
            && !content.starts_with('<')
        {
            message_count += 1;
            let clipped = clip(&content);
            if first_prompt.is_empty() {
                first_prompt = clipped.clone();
            }
            push_turn(&mut turns, Speaker::User, clipped);
        }

        // Assistant records with no text only called tools; they leave the
        // previous answer standing, which is the readable one anyway.
        if data.get("type").and_then(|t| t.as_str()) == Some("assistant")
            && let Some(content) = text_content(&data)
            && !content.is_empty()
        {
            push_turn(&mut turns, Speaker::Claude, clip(&content));
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

    let title = custom_title
        .clone()
        .or(ai_title)
        .unwrap_or(first_prompt)
        .trim()
        .to_string();

    Some(ClaudeSession {
        id: session_id,
        title: if title.is_empty() {
            "Untitled session".to_string()
        } else {
            title
        },
        custom_title,
        recent_turns: turns,
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

    /// The session list is only useful if it shows the name the user gave the
    /// session. The first prompt is the last resort, not the first choice.
    /// `user` is the transport, not the author. A long conversation is mostly
    /// tool results — one real session showed 5528 `user` records against 335
    /// messages actually sent — and the system injects `user` records too, so
    /// the preview could show a background agent reporting in rather than
    /// anything the person wrote.
    #[test]
    fn only_what_the_person_sent_is_counted_or_previewed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sess-turns.jsonl");
        let lines = [
            // Typed.
            r#"{"type":"user","timestamp":"2026-08-01T10:00:00Z","origin":{"kind":"human"},"message":{"content":"first thing I said"}}"#,
            // A tool result: content is a block array, not text.
            r#"{"type":"user","timestamp":"2026-08-01T10:00:01Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            // Injected by the system, not by the person.
            r#"{"type":"user","timestamp":"2026-08-01T10:00:02Z","origin":{"kind":"peer"},"message":{"content":"Background agent finished"}}"#,
            r#"{"type":"user","timestamp":"2026-08-01T10:00:03Z","origin":{"kind":"task-notification"},"message":{"content":"a task completed"}}"#,
            // A reminder the harness wraps in a tag.
            r#"{"type":"user","timestamp":"2026-08-01T10:00:04Z","origin":{"kind":"human"},"message":{"content":"<system-reminder>ignore me</system-reminder>"}}"#,
            // Typed again — this is the last thing the person said.
            r#"{"type":"user","timestamp":"2026-08-01T10:00:05Z","origin":{"kind":"human"},"message":{"content":"last thing I said"}}"#,
            // No origin at all: an older transcript, taken at face value.
            r#"{"type":"user","timestamp":"2026-08-01T10:00:06Z","message":{"content":"from before origin existed"}}"#,
        ];
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();

        let session = parse_session_file(&file).expect("a session");
        assert_eq!(
            session.message_count, 3,
            "counted something the person did not send"
        );
        assert_eq!(
            session.recent_turns.last().map(|t| t.text.as_str()),
            Some("from before origin existed")
        );
        assert_eq!(session.title, "first thing I said");
    }

    /// The card shows the exchange a conversation stopped on, whichever way
    /// round it happens to be — and a single answer split across several
    /// `assistant` records is one turn, not several. Real sessions have runs of
    /// dozens; counted separately they would fill both lines and hide the
    /// question, which is the half that identifies the conversation.
    #[test]
    fn the_last_two_turns_are_kept_and_same_speaker_runs_collapse() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sess-turns2.jsonl");
        let user = |text: &str| {
            format!(
                r#"{{"type":"user","timestamp":"2026-08-01T10:00:00Z","origin":{{"kind":"human"}},"message":{{"content":"{text}"}}}}"#
            )
        };
        let claude = |text: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-08-01T10:00:00Z","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
            )
        };

        let lines = [
            user("an older question"),
            claude("an older answer"),
            user("what is going on?"),
            // One answer, three records: text, a tool call, more text.
            claude("let me look"),
            r#"{"type":"assistant","timestamp":"2026-08-01T10:00:00Z","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#.to_string(),
            claude("here is what I found"),
        ];
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();

        let session = parse_session_file(&file).expect("a session");
        let turns: Vec<_> = session
            .recent_turns
            .iter()
            .map(|t| (t.speaker, t.text.as_str()))
            .collect();
        assert_eq!(
            turns,
            vec![
                (Speaker::User, "what is going on?"),
                (Speaker::Claude, "here is what I found"),
            ]
        );

        // Ending on a question leaves that question as the newest turn, with
        // the answer before it — never an older answer pretending to reply.
        let mut raw = std::fs::read_to_string(&file).unwrap();
        raw.push_str(&user("and one more thing"));
        raw.push('\n');
        std::fs::write(&file, &raw).unwrap();

        let session = parse_session_file(&file).expect("a session");
        let turns: Vec<_> = session
            .recent_turns
            .iter()
            .map(|t| (t.speaker, t.text.as_str()))
            .collect();
        assert_eq!(
            turns,
            vec![
                (Speaker::Claude, "here is what I found"),
                (Speaker::User, "and one more thing"),
            ]
        );
    }

    #[test]
    fn title_prefers_the_recorded_name_over_the_first_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sess-name.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"please refactor the flaky retry loop\"}}\n\
             {\"type\":\"ai-title\",\"aiTitle\":\"Refactor the flaky retry loop\",\
              \"sessionId\":\"sess-name\"}\n",
        )
        .unwrap();

        let ai = parse_session_file(&file).expect("a session with an ai title");
        assert_eq!(ai.title, "Refactor the flaky retry loop");
        assert_eq!(ai.custom_title, None, "an ai title is not a chosen name");

        // A name the user chose outranks the generated one.
        let mut raw = std::fs::read_to_string(&file).unwrap();
        raw.push_str(
            "{\"type\":\"custom-title\",\"customTitle\":\"retry loop\",\
              \"sessionId\":\"sess-name\"}\n",
        );
        std::fs::write(&file, &raw).unwrap();

        let named = parse_session_file(&file).expect("a named session");
        assert_eq!(named.title, "retry loop");
        assert_eq!(named.custom_title.as_deref(), Some("retry loop"));
    }

    /// A fork copies the parent's transcript verbatim, parent title records
    /// included. Adopting those would name every fork after its parent.
    #[test]
    fn a_forked_transcript_does_not_inherit_the_parent_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("the-fork.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"carried over from the parent\"}}\n\
             {\"type\":\"custom-title\",\"customTitle\":\"the parent name\",\
              \"sessionId\":\"the-parent\"}\n",
        )
        .unwrap();

        let forked = parse_session_file(&file).expect("a forked session");
        assert_eq!(forked.custom_title, None);
        assert_eq!(forked.title, "carried over from the parent");
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
        assert_eq!(
            session.recent_turns.last().map(|t| t.text.as_str()),
            Some("second")
        );
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
}
