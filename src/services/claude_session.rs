//! Reading and relocating Claude Code session history.
//!
//! Claude Code stores one JSONL file per session under
//! `~/.claude/projects/<path-with-slashes-replaced-by-dashes>/`.

use crate::models::{ClaudeSession, SessionTurn, Speaker, TokenUsage};
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

/// [`get_sessions_for_path`], plus every pinned session the recency cap would
/// have dropped.
///
/// The cap keeps the scan cheap by parsing only the newest transcripts, but a
/// pin is exactly a claim against recency — "keep this one visible however old
/// it gets" — so the pinned ids are read individually and appended. The merge
/// happens outside the directory cache: pins belong to the app's state file,
/// and keying the cache by them would split it per pin change for no gain.
pub fn get_sessions_with_pins(path: &str, limit: usize, pinned: &[String]) -> Vec<ClaudeSession> {
    let mut sessions = get_sessions_for_path(path, limit);
    for id in pinned {
        if sessions.iter().any(|s| &s.id == id) {
            continue;
        }
        if let Some(session) = refresh_one(path, id) {
            sessions.push(session);
        }
    }
    sort_sessions(&mut sessions, pinned);
    sessions
}

/// Pinned sessions first, in pin order; everything else newest first.
pub fn sort_sessions(sessions: &mut [ClaudeSession], pinned: &[String]) {
    let rank = |session: &ClaudeSession| {
        pinned
            .iter()
            .position(|id| id == &session.id)
            .unwrap_or(usize::MAX)
    };
    sessions.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.last_timestamp.cmp(&a.last_timestamp))
    });
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
    // Through the cache: this runs for every card on screen on a ten-second
    // timer, and a transcript nobody has written to parses to what it did
    // before. One of them is 60 MB.
    let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
    parse_cached(&file, modified)
}

/// Directory-scoped form of [`get_sessions_for_path`], for testing.
pub fn read_sessions_dir(sessions_dir: &Path, limit: usize) -> Vec<ClaudeSession> {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return Vec::new();
    };

    // Stat first, parse second. One real project directory holds 542 MB across
    // 51 transcripts, and parsing all of them to sort by their last message and
    // then show five took 1.9 seconds — on a sweep that now runs every ten
    // seconds. A transcript's mtime is its last activity, which is the same
    // ordering, and costs a stat.
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let file_path = entry.path();
        if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // Agent transcripts are not user-facing sessions.
        if file_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with("agent-"))
        {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        candidates.push((file_path, modified));
    }
    candidates.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));

    // Walk newest-first and stop as soon as no remaining file can qualify.
    //
    // A file's mtime is never older than its last message — writing that
    // message is what set it — so a candidate whose *mtime* already loses to
    // the oldest session held cannot win on its last message either, and
    // neither can anything after it, because they are sorted. That makes this
    // exactly the same answer as parsing all 51 and sorting, for the cost of
    // parsing about five. Sorting on mtime alone is not the same answer:
    // naming a session rewrites its transcript without adding a timestamped
    // record, so mtime moves and the last message does not.
    let mut sessions: Vec<ClaudeSession> = Vec::new();
    for (file_path, modified) in candidates {
        if sessions.len() >= limit
            && let Some(worst) = sessions.last()
            && modified <= std::time::SystemTime::from(worst.last_timestamp)
        {
            break;
        }
        if let Some(session) = parse_cached(&file_path, modified) {
            sessions.push(session);
            sessions.sort_by_key(|s| std::cmp::Reverse(s.last_timestamp));
            sessions.truncate(limit);
        }
    }
    sessions
}

/// [`parse_session_file`], skipped entirely when the file has not been written
/// to since it was last parsed.
///
/// The scan runs on a timer, so the same transcripts are read over and over;
/// an active conversation touches one of them and leaves the rest identical.
pub fn parse_cached(file_path: &Path, modified: std::time::SystemTime) -> Option<ClaudeSession> {
    if let Ok(guard) = parsed_cache().lock()
        && let Some((cached_at, session)) = guard.get(file_path)
        && *cached_at == modified
    {
        return Some(session.clone());
    }

    let session = parse_session_file(file_path)?;
    if let Ok(mut guard) = parsed_cache().lock() {
        // A transcript that has gone quiet is not worth remembering forever,
        // and this is a cache rather than an index: dropping it wholesale
        // costs one re-parse of whatever is still on screen.
        if guard.len() >= PARSED_CACHE_LIMIT {
            guard.clear();
        }
        guard.insert(file_path.to_path_buf(), (modified, session.clone()));
    }
    Some(session)
}

/// Enough for every transcript of several busy projects; the entries are a few
/// short strings each.
const PARSED_CACHE_LIMIT: usize = 512;

type ParsedCache = HashMap<PathBuf, (std::time::SystemTime, ClaudeSession)>;

fn parsed_cache() -> &'static Mutex<ParsedCache> {
    static CACHE: OnceLock<Mutex<ParsedCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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
    let mut git_branch: Option<String> = None;
    let mut tokens = TokenUsage::default();
    let mut model: Option<String> = None;

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

        // The last branch seen wins: a conversation is remembered by where it
        // ended up, and mid-session checkouts update `gitBranch` per record.
        if let Some(branch) = data.get("gitBranch").and_then(|b| b.as_str())
            && !branch.trim().is_empty()
        {
            git_branch = Some(branch.trim().to_string());
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

        if data.get("type").and_then(|t| t.as_str()) == Some("assistant") {
            // Every assistant record carries the usage of the API call that
            // produced it, so the sums are the transcript's whole spend.
            if let Some(message) = data.get("message") {
                if let Some(usage) = message.get("usage") {
                    let count = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
                    tokens.input += count("input_tokens");
                    tokens.cache_write += count("cache_creation_input_tokens");
                    tokens.cache_read += count("cache_read_input_tokens");
                    tokens.output += count("output_tokens");
                }
                if let Some(name) = message.get("model").and_then(|m| m.as_str())
                    && !name.is_empty()
                {
                    model = Some(name.to_string());
                }
            }
            // Assistant records with no text only called tools; they leave the
            // previous answer standing, which is the readable one anyway.
            if let Some(content) = text_content(&data)
                && !content.is_empty()
            {
                push_turn(&mut turns, Speaker::Claude, clip(&content));
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
        git_branch,
        tokens,
        model,
    })
}

/// Name a session by appending the same record Claude's own `/rename` writes.
///
/// The parser takes the last `custom-title` record stamped with the file's own
/// session id, so appending is the whole operation — nothing is rewritten, and
/// a transcript this large is never read in. Callers only use this on sessions
/// with no live window: Claude holds a live transcript open and interleaving
/// appends with its writer is not a race worth having when renaming the tmux
/// window does the same job through the sync plugin.
pub fn rename_session(path: &str, session_id: &str, name: &str) -> Result<(), String> {
    let file = claude_projects_dir()
        .join(path_to_claude_folder(path))
        .join(format!("{session_id}.jsonl"));
    rename_transcript(&file, session_id, name)
}

/// File-scoped form of [`rename_session`], for testing.
pub fn rename_transcript(file: &Path, session_id: &str, name: &str) -> Result<(), String> {
    if !file.exists() {
        return Err("Session transcript no longer exists".to_string());
    }
    let record = serde_json::json!({
        "type": "custom-title",
        "customTitle": name,
        "sessionId": session_id,
    });
    let line = format!("{record}\n");
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(file)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|error| error.to_string())
}

/// Delete a session's transcript. Permanent — the caller has already asked.
pub fn delete_session(path: &str, session_id: &str) -> Result<(), String> {
    let file = claude_projects_dir()
        .join(path_to_claude_folder(path))
        .join(format!("{session_id}.jsonl"));
    std::fs::remove_file(&file).map_err(|error| error.to_string())
}

/// Best-effort price table, per million tokens (input, output), matched on the
/// model id. Cache writes bill at 1.25× input and cache reads at 0.1×, which
/// has held across every generation so far. Prices drift with releases — that
/// is why the figure renders with a `~` and why an unknown model shows tokens
/// only rather than a made-up dollar amount.
fn model_prices(model: &str) -> Option<(f64, f64)> {
    if model.contains("opus-4-5") || model.contains("opus-5") || model.contains("fable") {
        Some((5.0, 25.0))
    } else if model.contains("opus") {
        Some((15.0, 75.0))
    } else if model.contains("sonnet") {
        Some((3.0, 15.0))
    } else if model.contains("haiku-3") {
        Some((0.8, 4.0))
    } else if model.contains("haiku") {
        Some((1.0, 5.0))
    } else {
        None
    }
}

/// Estimated dollar cost of a transcript, when the model's prices are known.
pub fn cost_estimate(model: Option<&str>, tokens: TokenUsage) -> Option<f64> {
    let (input, output) = model_prices(model?)?;
    let per = 1_000_000.0;
    Some(
        (tokens.input as f64 / per) * input
            + (tokens.cache_write as f64 / per) * input * 1.25
            + (tokens.cache_read as f64 / per) * input * 0.1
            + (tokens.output as f64 / per) * output,
    )
}

/// `999`, `12k`, `1.2M` — a count that fits a card's meta line.
pub fn fmt_tokens(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=999_499 => format!("{}k", (count + 500) / 1_000),
        _ => {
            let millions = count as f64 / 1_000_000.0;
            if millions >= 10.0 {
                format!("{millions:.0}M")
            } else {
                format!("{millions:.1}M")
            }
        }
    }
}

/// `~$0.42`, `~$123` — always marked as the estimate it is.
pub fn fmt_cost(cost: f64) -> String {
    if cost >= 100.0 {
        format!("~${cost:.0}")
    } else {
        format!("~${cost:.2}")
    }
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

    /// The scan stops early instead of parsing every transcript. That is only
    /// sound because a file's mtime is never older than its last message, so
    /// this pins the case where the two disagree: a transcript touched after
    /// its last message — which is what naming a session does, since a title
    /// record carries no timestamp — must not displace a genuinely newer
    /// conversation.
    ///
    /// Measured on a real 542 MB project directory, this took the full scan
    /// from 1.9 s to 56 ms cold and 0.2 ms warm, on a sweep that runs every
    /// ten seconds.
    #[test]
    fn the_early_exit_picks_what_a_full_scan_would() {
        use std::time::{Duration, SystemTime};
        let dir = tempfile::tempdir().unwrap();

        // `age` is how old the last message is; `touched` how long ago the file
        // was written. The two disagree on purpose.
        let write = |name: &str, age_h: u64, touched_h: u64| {
            let path = dir.path().join(format!("{name}.jsonl"));
            let when = chrono::Utc::now() - chrono::Duration::hours(age_h as i64);
            std::fs::write(
                &path,
                format!(
                    "{{\"type\":\"user\",\"timestamp\":\"{}\",\"message\":{{\"content\":\"{name}\"}}}}\n",
                    when.to_rfc3339()
                ),
            )
            .unwrap();
            let mtime = SystemTime::now() - Duration::from_secs(touched_h * 3600);
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(mtime))
                .unwrap();
        };

        write("newest-message", 1, 1);
        write("touched-but-stale", 40, 0); // freshest file, oldest conversation
        write("second", 2, 2);
        write("third", 3, 3);
        write("oldest", 30, 30);

        let picked: Vec<String> = read_sessions_dir(dir.path(), 3)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(
            picked,
            vec!["newest-message", "second", "third"],
            "the early exit dropped a session a full scan would have kept"
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

    /// Tokens, model and branch ride the same parse as everything else — the
    /// card's recognition line costs no extra read of a multi-MB transcript.
    #[test]
    fn usage_branch_and_model_are_summed_from_the_records() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sess-usage.jsonl");
        let lines = [
            r#"{"type":"user","timestamp":"2026-08-01T10:00:00Z","gitBranch":"main","message":{"content":"question"}}"#,
            r#"{"type":"assistant","timestamp":"2026-08-01T10:00:01Z","gitBranch":"main","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"cache_creation_input_tokens":1000,"cache_read_input_tokens":0,"output_tokens":50}}}"#,
            // A mid-session checkout: the last branch wins.
            r#"{"type":"assistant","timestamp":"2026-08-01T10:00:02Z","gitBranch":"feat/x","message":{"model":"claude-opus-5","usage":{"input_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":2000,"output_tokens":25}}}"#,
        ];
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();

        let session = parse_session_file(&file).expect("a session");
        assert_eq!(session.git_branch.as_deref(), Some("feat/x"));
        assert_eq!(session.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(session.tokens.input, 15);
        assert_eq!(session.tokens.cache_write, 1000);
        assert_eq!(session.tokens.cache_read, 2000);
        assert_eq!(session.tokens.output, 75);
        assert_eq!(session.tokens.total_in(), 3015);

        // An old minimal transcript has none of this, and must not pretend to.
        let bare = dir.path().join("sess-bare.jsonl");
        std::fs::write(
            &bare,
            "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n",
        )
        .unwrap();
        let session = parse_session_file(&bare).expect("a session");
        assert!(session.tokens.is_zero());
        assert_eq!(session.git_branch, None);
        assert_eq!(session.model, None);
    }

    /// The estimate exists for known models only: a made-up price is worse
    /// than showing tokens alone.
    #[test]
    fn cost_is_estimated_for_known_models_only() {
        let tokens = TokenUsage {
            input: 1_000_000,
            cache_write: 0,
            cache_read: 0,
            output: 1_000_000,
        };
        let opus5 = cost_estimate(Some("claude-opus-5"), tokens).expect("a known model");
        assert!((opus5 - 30.0).abs() < 0.001, "5 in + 25 out, got {opus5}");
        assert!(cost_estimate(Some("some-future-model"), tokens).is_none());
        assert!(cost_estimate(None, tokens).is_none());

        // Cache tiers price off the input rate.
        let cached = TokenUsage {
            input: 0,
            cache_write: 1_000_000,
            cache_read: 1_000_000,
            output: 0,
        };
        let cost = cost_estimate(Some("claude-sonnet-5"), cached).expect("sonnet is known");
        assert!((cost - (3.0 * 1.25 + 3.0 * 0.1)).abs() < 0.001, "{cost}");
    }

    #[test]
    fn token_and_cost_formatting() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1_000), "1k");
        assert_eq!(fmt_tokens(12_499), "12k");
        assert_eq!(fmt_tokens(999_499), "999k");
        assert_eq!(fmt_tokens(1_200_000), "1.2M");
        assert_eq!(fmt_tokens(14_700_000), "15M");
        assert_eq!(fmt_cost(0.416), "~$0.42");
        assert_eq!(fmt_cost(123.4), "~$123");
    }

    /// Renaming a stopped session appends the record `/rename` writes, so the
    /// next parse — every consumer — sees the new name. An empty transcript
    /// path errors instead of creating a stray file.
    #[test]
    fn renaming_a_stopped_session_appends_a_title_record() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sess-r.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"hello\"}}\n",
        )
        .unwrap();

        rename_transcript(&file, "sess-r", "my rename").expect("the append lands");
        let session = parse_session_file(&file).expect("a session");
        assert_eq!(session.custom_title.as_deref(), Some("my rename"));
        assert_eq!(session.title, "my rename");

        // A missing transcript is an error, not a new file.
        assert!(rename_session("/nowhere/forestui-rename-test", "ghost", "x").is_err());
        assert!(delete_session("/nowhere/forestui-rename-test", "ghost").is_err());
    }

    /// Pins outrank recency, in pin order; everything else stays newest-first.
    /// A pinned id that no longer parses to a session simply is not there.
    #[test]
    fn pinned_sessions_lead_in_pin_order() {
        let mk = |id: &str, hours_ago: i64| ClaudeSession {
            id: id.into(),
            title: id.into(),
            custom_title: None,
            recent_turns: Vec::new(),
            last_timestamp: chrono::Utc::now() - chrono::Duration::hours(hours_ago),
            message_count: 1,
            git_branch: None,
            tokens: TokenUsage::default(),
            model: None,
        };
        let mut sessions = vec![mk("new", 1), mk("mid", 5), mk("old", 20), mk("ancient", 90)];
        let pinned = vec!["old".to_string(), "mid".to_string()];
        sort_sessions(&mut sessions, &pinned);
        let order: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(order, vec!["old", "mid", "new", "ancient"]);

        // No pins: pure recency, unchanged from before pins existed.
        sort_sessions(&mut sessions, &[]);
        let order: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(order, vec!["new", "mid", "old", "ancient"]);
    }

    /// The recency cap must not hide a pinned session: pinning is precisely a
    /// claim against recency.
    #[test]
    fn a_pinned_session_survives_the_recency_cap() {
        let dir = tempfile::tempdir().unwrap();
        for (name, ts) in [
            ("a", "2026-08-05T10:00:00Z"),
            ("b", "2026-08-04T10:00:00Z"),
            ("c", "2026-08-03T10:00:00Z"),
        ] {
            std::fs::write(
                dir.path().join(format!("{name}.jsonl")),
                format!(
                    "{{\"type\":\"user\",\"timestamp\":\"{ts}\",\"message\":{{\"content\":\"q\"}}}}\n"
                ),
            )
            .unwrap();
        }
        // Capped at 2, the oldest falls out…
        let ids: Vec<String> = read_sessions_dir(dir.path(), 2)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
        // …but read_sessions_dir has no pin knowledge; the pin merge lives in
        // get_sessions_with_pins, which needs the real projects dir. Its logic
        // — append what refresh_one finds, then sort — is exercised through
        // sort order here: a pinned "c" would lead.
        let mut sessions = read_sessions_dir(dir.path(), 3);
        sort_sessions(&mut sessions, &["c".to_string()]);
        assert_eq!(sessions[0].id, "c");
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
