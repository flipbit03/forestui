//! Reading and relocating Claude Code session history.
//!
//! Claude Code stores one JSONL file per session under
//! `<config dir>/projects/<folder>/`, where `<folder>` is the session's
//! working directory with every character that is not an ASCII letter or
//! digit replaced by a dash (see [`claude_folder_name`]). Different
//! directories can share a folder, so each transcript's recorded `cwd` is what
//! finally says whose it is (see [`recorded_elsewhere`]).

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

struct CacheEntry {
    fingerprint: Fingerprint,
    sessions: Vec<ClaudeSession>,
    /// Recorded directories whose transcripts were left out because they
    /// exist elsewhere. Their existence is part of the answer, so a hit
    /// re-checks them.
    elsewhere: Vec<String>,
}

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
        .map(|entry| entry.sessions.clone())
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

/// Claude names a folder longer than this by its prefix plus a hash.
const MAX_FOLDER_LEN: usize = 200;

/// The folder Claude files a resolved path's sessions under. This must match
/// Claude Code byte for byte, or a repository's sessions are simply never
/// found — replacing only slashes missed every path with an underscore, a dot
/// or a space in it.
///
/// Claude Code's own rule: `path.replace(/[^a-zA-Z0-9]/g, "-")`, and past
/// [`MAX_FOLDER_LEN`] the first 200 characters, a dash, and a hash of the
/// *unsanitised* path in base 36.
///
/// The regex has no `u` flag, so it matches UTF-16 code units: a character
/// outside the BMP becomes two dashes, not one.
fn claude_folder_name(path: &str) -> String {
    let mut folder = claude_slug(path);
    if folder.len() <= MAX_FOLDER_LEN {
        return folder;
    }
    // Every character is ASCII by now, so a byte slice is a character slice.
    folder.truncate(MAX_FOLDER_LEN);
    folder.push('-');
    folder.push_str(&to_base36(js_string_hash(path).unsigned_abs()));
    folder
}

/// The untruncated form of [`claude_folder_name`]. Claude compares paths by
/// this, not by the folder name, when it asks whether two directories collide.
fn claude_slug(path: &str) -> String {
    let mut slug = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else {
            slug.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    slug
}

/// `h = (h << 5) - h + charCode | 0` over UTF-16 code units — JavaScript's
/// 32-bit wrapping arithmetic, which is what Claude hashes long paths with.
fn js_string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, unit| {
        h.wrapping_shl(5)
            .wrapping_sub(h)
            .wrapping_add(i32::from(unit))
    })
}

fn to_base36(mut n: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(char::from(DIGITS[(n % 36) as usize]));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.iter().rev().collect()
}

pub fn claude_projects_dir() -> PathBuf {
    crate::services::claude_plugin::claude_config_dir().join("projects")
}

/// The folder named after the path: where Claude files its transcripts.
fn own_dir(base: &Path, resolved: &str) -> PathBuf {
    base.join(claude_folder_name(resolved))
}

/// Every sibling of a hashed folder that shares its 200-unit prefix and holds
/// a transcript recorded at this very path. Always empty for a path short
/// enough not to be hashed, which costs nothing to find out.
///
/// Claude's own resume looks in the same places, because the hash suffix has
/// not always been the one computed here; a long path's history can sit under
/// another suffix, and only the recorded `cwd` says whose it is.
///
/// Deciding means reading a window of every transcript in every candidate, so
/// the answer is memoized against the candidates' own mtimes: a folder's mtime
/// moves whenever a transcript is created or removed in it, and a transcript's
/// recorded directory is settled when it is created.
fn borrowed_dirs_in(base: &Path, resolved: &str) -> Vec<PathBuf> {
    let folder = claude_folder_name(resolved);
    if folder.len() <= MAX_FOLDER_LEN {
        return Vec::new();
    }
    let prefix = format!("{}-", &folder[..MAX_FOLDER_LEN]);
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut candidates: Vec<(PathBuf, Option<std::time::SystemTime>)> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name != folder && name.starts_with(&prefix))
        })
        .map(|entry| {
            let modified = entry.metadata().and_then(|m| m.modified()).ok();
            (entry.path(), modified)
        })
        .collect();
    candidates.sort();

    let key = (base.to_path_buf(), resolved.to_string());
    if let Ok(guard) = borrowed_cache().lock()
        && let Some((seen, borrowed)) = guard.get(&key)
        && *seen == candidates
    {
        return borrowed.clone();
    }

    let slug = claude_slug(resolved);
    let borrowed: Vec<PathBuf> = candidates
        .iter()
        .filter(|(dir, _)| {
            std::fs::read_dir(dir).is_ok_and(|files| {
                files.flatten().any(|file| {
                    let file = file.path();
                    is_transcript(&file)
                        && recorded_cwd(&file).is_some_and(|cwd| claude_slug(&cwd) == slug)
                })
            })
        })
        .map(|(dir, _)| dir.clone())
        .collect();
    if let Ok(mut guard) = borrowed_cache().lock() {
        guard.insert(key, (candidates, borrowed.clone()));
    }
    borrowed
}

type BorrowedCache =
    HashMap<(PathBuf, String), (Vec<(PathBuf, Option<std::time::SystemTime>)>, Vec<PathBuf>)>;

fn borrowed_cache() -> &'static Mutex<BorrowedCache> {
    static CACHE: OnceLock<Mutex<BorrowedCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn is_transcript(file: &Path) -> bool {
    file.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

/// The canonical form of a project path as a string: what Claude records as
/// `cwd` (it takes `realpath` of its working directory) and names folders by.
fn resolved_string(path: &str) -> String {
    crate::util::expand_and_resolve(path)
        .to_string_lossy()
        .into_owned()
}

/// Whether the transcript at `file`, recorded in `cwd`, is `resolved`'s to
/// list, rename, delete or carry along on a worktree rename.
///
/// In the path's own folder, everything is unless it was recorded elsewhere.
/// A borrowed folder was admitted for one transcript recorded at this path;
/// the rest of it is another long path's history, so there only a transcript
/// recorded under this path's slug is.
fn owns(resolved: &str, own: &Path, file: &Path, cwd: Option<&str>) -> bool {
    if file.parent() != Some(own) && cwd.is_none_or(|cwd| claude_slug(cwd) != claude_slug(resolved))
    {
        return false;
    }
    !recorded_elsewhere(cwd, resolved)
}

/// Whether a transcript filed under `resolved`'s folder belongs to a different
/// directory that only shares its folder name: `my_infra` and `my-infra` both
/// file under `...-my-infra`, and each must not list, rename or delete the
/// other's sessions.
///
/// This is the test Claude's own resume applies. The recorded directory has
/// the same slug but is a different string, still exists, and resolves to a
/// different place. A recorded directory that is gone cannot be told apart, so
/// it stays — as does a transcript forestui migrated here when it renamed a
/// worktree, whose recorded slug is the old one. A path with `..` in it is not
/// resolved at all, as in Claude.
///
/// History follows the recorded path, as it does in Claude: rename `feat_x` to
/// `feat-x` (one folder, so nothing moves) and later create a new `feat_x`,
/// and the old transcripts go with the new directory.
fn recorded_elsewhere(cwd: Option<&str>, resolved: &str) -> bool {
    let Some(cwd) = cwd else {
        return false;
    };
    let slug = claude_slug(resolved);
    if cwd == resolved || claude_slug(cwd) != slug {
        return false;
    }
    if Path::new(cwd)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return false;
    }
    std::fs::canonicalize(cwd).is_ok_and(|real| {
        let real = real.to_string_lossy();
        real != resolved && claude_slug(&real) == slug
    })
}

/// How much of each end of a transcript Claude reads to place it. Matching the
/// window matters, not only the cost: a first record bigger than this (a large
/// pasted prompt) gives Claude no `cwd` at all, so it never filters that
/// session out, and neither may forestui.
const TRANSCRIPT_WINDOW: usize = 65_536;

/// The directory a transcript was recorded in, as Claude reads it: the last
/// `relocated` record in the final window, else the first `cwd` in the first.
///
/// serde_json rejects a few lines `JSON.parse` accepts (nesting past 128
/// levels, a number like `1e400`, an unpaired `\ud800` escape); such a line
/// is skipped here where Claude would read it. Claude does not write them.
fn cwd_in_windows(head: &str, tail: &str) -> Option<String> {
    let field = |line: &str, key: &str| -> Option<String> {
        let data = serde_json::from_str::<serde_json::Value>(line).ok()?;
        Some(data.get(key)?.as_str()?.to_string())
    };
    tail.rsplit('\n')
        .filter(|line| {
            line.contains("\"relocatedCwd\":") && line.contains("\"type\":\"relocated\"")
        })
        .find_map(|line| {
            let data = serde_json::from_str::<serde_json::Value>(line).ok()?;
            (data.get("type")?.as_str()? == "relocated")
                .then(|| field(line, "relocatedCwd"))
                .flatten()
        })
        .or_else(|| {
            head.split('\n')
                .filter(|line| line.contains("\"cwd\":"))
                .find_map(|line| field(line, "cwd"))
        })
}

/// [`cwd_in_windows`] over a transcript already in memory.
fn cwd_in_bytes(raw: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&raw[..raw.len().min(TRANSCRIPT_WINDOW)]);
    let tail = String::from_utf8_lossy(&raw[raw.len().saturating_sub(TRANSCRIPT_WINDOW)..]);
    cwd_in_windows(&head, &tail)
}

/// [`cwd_in_windows`] read from disk: two reads, however large the file.
fn recorded_cwd(file: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(file).ok()?;
    let size = usize::try_from(f.metadata().ok()?.len()).ok()?;
    let mut head = Vec::with_capacity(size.min(TRANSCRIPT_WINDOW));
    (&mut f)
        .take(TRANSCRIPT_WINDOW as u64)
        .read_to_end(&mut head)
        .ok()?;
    if size <= TRANSCRIPT_WINDOW {
        return cwd_in_bytes(&head);
    }
    let mut tail = Vec::with_capacity(TRANSCRIPT_WINDOW);
    f.seek(SeekFrom::Start((size - TRANSCRIPT_WINDOW) as u64))
        .ok()?;
    f.read_to_end(&mut tail).ok()?;
    cwd_in_windows(
        &String::from_utf8_lossy(&head),
        &String::from_utf8_lossy(&tail),
    )
}

/// Find a session's transcript: its own folder first, and only when it is not
/// there the borrowed ones, whose scan is the expensive part. Session ids are
/// unique, so the first file found is the one.
fn locate_transcript(base: &Path, resolved: &str, session_id: &str) -> Option<PathBuf> {
    let name = format!("{session_id}.jsonl");
    let own = own_dir(base, resolved).join(&name);
    if own.exists() {
        return Some(own);
    }
    borrowed_dirs_in(base, resolved)
        .into_iter()
        .map(|dir| dir.join(&name))
        .find(|file| file.exists())
}

/// The transcript of `session_id` if it is `path`'s, for acting on it.
/// Deleting is permanent, so ownership is checked here against the file
/// itself rather than trusted from whichever list offered the session.
fn owned_transcript_in(base: &Path, path: &str, session_id: &str) -> Result<PathBuf, String> {
    let resolved = resolved_string(path);
    let file = locate_transcript(base, &resolved, session_id)
        .ok_or_else(|| "Session transcript no longer exists".to_string())?;
    let own = own_dir(base, &resolved);
    if !owns(&resolved, &own, &file, recorded_cwd(&file).as_deref()) {
        return Err("Session belongs to another directory".to_string());
    }
    Ok(file)
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
    sessions_in(&claude_projects_dir(), path, limit)
}

/// [`get_sessions_for_path`] under a given projects directory.
fn sessions_in(base: &Path, path: &str, limit: usize) -> Vec<ClaudeSession> {
    let resolved = resolved_string(path);
    let own = own_dir(base, &resolved);
    let mut dirs = vec![own.clone()];
    dirs.extend(borrowed_dirs_in(base, &resolved));
    let key = (path.to_string(), limit);
    let current = dirs
        .iter()
        .map(|dir| fingerprint(dir))
        .fold((0, None), |(count, newest): Fingerprint, (more, theirs)| {
            (count + more, newest.max(theirs))
        });

    // Nothing in the directory has changed since the cached answer was built,
    // so re-reading every transcript would produce the same list — unless a
    // neighbour that shares the folder has appeared or gone since, which
    // changes whose transcripts are whose without touching any of them. The
    // per-card refresh re-asks that every tick, so a list that did not would
    // re-add every ten seconds a card the refresh keeps removing.
    if let Ok(guard) = cache().lock()
        && let Some(entry) = guard.get(&key)
        && entry.fingerprint == current
        && entry
            .sessions
            .iter()
            .all(|s| !recorded_elsewhere(s.cwd.as_deref(), &resolved))
        && entry
            .elsewhere
            .iter()
            .all(|cwd| recorded_elsewhere(Some(cwd), &resolved))
    {
        return entry.sessions.clone();
    }

    let elsewhere = std::cell::RefCell::new(Vec::new());
    let sessions = read_sessions_dirs(&dirs, limit, &|file, session| {
        let keep = owns(&resolved, &own, file, session.cwd.as_deref());
        if !keep
            && let Some(cwd) = &session.cwd
            && recorded_elsewhere(Some(cwd), &resolved)
        {
            elsewhere.borrow_mut().push(cwd.clone());
        }
        keep
    });
    let mut elsewhere = elsewhere.into_inner();
    elsewhere.sort();
    elsewhere.dedup();
    if let Ok(mut guard) = cache().lock() {
        guard.insert(
            key,
            CacheEntry {
                fingerprint: current,
                sessions: sessions.clone(),
                elsewhere,
            },
        );
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
    refresh_one_in(&claude_projects_dir(), path, session_id)
}

/// [`refresh_one`] under a given projects directory.
fn refresh_one_in(base: &Path, path: &str, session_id: &str) -> Option<ClaudeSession> {
    let resolved = resolved_string(path);
    let file = locate_transcript(base, &resolved, session_id)?;
    // Through the cache: this runs for every card on screen on a ten-second
    // timer, and a transcript nobody has written to parses to what it did
    // before. One of them is 60 MB.
    let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
    parse_cached(&file, modified).filter(|session| {
        owns(
            &resolved,
            &own_dir(base, &resolved),
            &file,
            session.cwd.as_deref(),
        )
    })
}

/// Directory-scoped form of [`get_sessions_for_path`], for testing.
#[cfg(test)]
pub fn read_sessions_dir(sessions_dir: &Path, limit: usize) -> Vec<ClaudeSession> {
    read_sessions_dirs(&[sessions_dir.to_path_buf()], limit, &|_, _| true)
}

/// The newest `limit` sessions across `dirs` that `keep` accepts.
fn read_sessions_dirs(
    dirs: &[PathBuf],
    limit: usize,
    keep: &dyn Fn(&Path, &ClaudeSession) -> bool,
) -> Vec<ClaudeSession> {
    let entries = dirs
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten();

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
        if let Some(session) = parse_cached(&file_path, modified)
            && keep(&file_path, &session)
        {
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
        cwd: cwd_in_bytes(raw.as_bytes()),
    })
}

/// Name a session by appending the same records Claude's own `/rename` writes.
///
/// The parser takes the last `custom-title` record stamped with the file's own
/// session id, so appending is the whole operation — nothing is rewritten, and
/// a transcript this large is never read in. Callers only use this on sessions
/// with no live window: Claude holds a live transcript open and interleaving
/// appends with its writer is not a race worth having when renaming the tmux
/// window does the same job through the sync plugin.
///
/// Claude keeps a session's name in *two* record types and a runtime rename
/// writes them as a pair: `custom-title` is the session title (the resume
/// picker, hook-input `session_title`), `agent-name` is what the badge in the
/// prompt box draws. Appending only the title looked renamed everywhere except
/// the resumed UI itself — and the sync plugin could not heal it, because the
/// title it compares against the window was already correct, so it never
/// pushed the runtime rename that would have rewritten the agent name.
pub fn rename_session(path: &str, session_id: &str, name: &str) -> Result<(), String> {
    let file = owned_transcript_in(&claude_projects_dir(), path, session_id)?;
    rename_transcript(&file, session_id, name)
}

/// File-scoped form of [`rename_session`], for testing.
pub fn rename_transcript(file: &Path, session_id: &str, name: &str) -> Result<(), String> {
    if !file.exists() {
        return Err("Session transcript no longer exists".to_string());
    }
    let title = serde_json::json!({
        "type": "custom-title",
        "customTitle": name,
        "sessionId": session_id,
    });
    let agent_name = serde_json::json!({
        "type": "agent-name",
        "agentName": name,
        "sessionId": session_id,
    });
    use std::io::{Read, Seek, SeekFrom, Write};
    std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(file)
        .and_then(|mut f| {
            // A transcript normally ends in a newline, but a torn tail (a
            // crashed writer) must not swallow the rename: appended straight
            // onto an unterminated line, the first record and that line both
            // stop parsing while the second still lands — the exact
            // title/badge disagreement this function exists to prevent.
            let mut needs_newline = false;
            if f.metadata()?.len() > 0 {
                let mut last = [0u8; 1];
                f.seek(SeekFrom::End(-1))?;
                f.read_exact(&mut last)?;
                needs_newline = last[0] != b'\n';
            }
            let prefix = if needs_newline { "\n" } else { "" };
            f.write_all(format!("{prefix}{title}\n{agent_name}\n").as_bytes())
        })
        .map_err(|error| error.to_string())
}

/// Delete a session's transcript. Permanent — the caller has already asked.
pub fn delete_session(path: &str, session_id: &str) -> Result<(), String> {
    delete_session_in(&claude_projects_dir(), path, session_id)
}

/// [`delete_session`] under a given projects directory.
fn delete_session_in(base: &Path, path: &str, session_id: &str) -> Result<(), String> {
    let file = owned_transcript_in(base, path, session_id)?;
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
///
/// Runs once the directory has already moved, so the old path no longer
/// resolves; its parent still does, and resolving that keeps a symlinked
/// parent from naming the wrong folder.
pub fn migrate_sessions(old_path: &Path, new_path: &Path) {
    migrate_sessions_in(&claude_projects_dir(), old_path, new_path);
}

/// [`migrate_sessions`] under a given projects directory.
fn migrate_sessions_in(base: &Path, old_path: &Path, new_path: &Path) {
    let old = std::fs::canonicalize(old_path)
        .ok()
        .or_else(|| {
            let parent = std::fs::canonicalize(old_path.parent()?).ok()?;
            Some(parent.join(old_path.file_name()?))
        })
        .unwrap_or_else(|| old_path.to_path_buf());
    let old = old.to_string_lossy();
    let new_dir = base.join(claude_folder_name(&resolved_string(
        &new_path.to_string_lossy(),
    )));
    let own = own_dir(base, &old);
    let keep = |file: &Path| migrates(&old, &own, file, recorded_cwd(file).as_deref());
    for old_dir in std::iter::once(own.clone()).chain(borrowed_dirs_in(base, &old)) {
        migrate_dirs(&old_dir, &new_dir, Some(&keep));
    }
}

/// Whether a worktree rename carries this transcript along. Stricter than
/// [`owns`], which only mirrors what Claude would show: moving is forestui's
/// own act, so a transcript recorded at a different path stays put whether or
/// not that path exists right now. A neighbour that is missing at the moment
/// of the rename — deleted, unmounted, mid-rename itself — must not lose its
/// history to us.
///
/// What moves: what was recorded here, what cannot be placed, and what an
/// earlier rename migrated into this folder (recorded under another slug).
/// The last two only from the path's own folder; a borrowed folder lends
/// nothing but transcripts recorded at this very path.
fn migrates(old: &str, own: &Path, file: &Path, cwd: Option<&str>) -> bool {
    let in_own = file.parent() == Some(own);
    match cwd {
        Some(cwd) if cwd == old => true,
        Some(cwd) => in_own && claude_slug(cwd) != claude_slug(old),
        None => in_own,
    }
}

/// Directory-scoped form of [`migrate_sessions`]. Transcripts `keep` refuses
/// stay behind: they are another project's history, not this worktree's.
pub fn migrate_dirs(old_dir: &Path, new_dir: &Path, keep: Option<&dyn Fn(&Path) -> bool>) {
    if !old_dir.exists() {
        return;
    }
    if std::fs::create_dir_all(new_dir).is_err() {
        return;
    }

    if let Ok(entries) = std::fs::read_dir(old_dir) {
        for entry in entries.flatten() {
            let src = entry.path();
            if !is_transcript(&src) {
                continue;
            }
            let Some(name) = src.file_name() else {
                continue;
            };
            let dest = new_dir.join(name);
            // Checked before `keep`, which reads the transcript. This is also
            // what makes a rename that keeps the folder (`feat_x` to `feat-x`)
            // a no-op: every destination is its own source.
            if dest.exists() || keep.is_some_and(|keep| !keep(&src)) {
                continue;
            }
            let _ = std::fs::rename(&src, &dest);
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

    /// The wrapper resolves before it encodes: `~` expands and symlinks are
    /// followed, because Claude names the folder after its `getcwd()`, which
    /// is always the real path.
    #[test]
    fn folder_naming_encodes_the_resolved_path() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("my_infra");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link.to");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let canonical = std::fs::canonicalize(&real).unwrap();
        let expected = claude_folder_name(&canonical.to_string_lossy());
        assert!(expected.ends_with("-my-infra"), "{expected}");
        let folder = |path: &Path| claude_folder_name(&resolved_string(&path.to_string_lossy()));
        assert_eq!(folder(&real), expected);
        assert_eq!(folder(&link), expected);

        let home = crate::util::home_dir().join("forestui-no-such_dir");
        assert_eq!(
            claude_folder_name(&resolved_string("~/forestui-no-such_dir")),
            claude_folder_name(&home.to_string_lossy())
        );
    }

    /// Claude replaces every non-alphanumeric character, not just slashes: a
    /// repository called `my_infra` lives in `...-my-infra`, and looking for
    /// `...-my_infra` found no sessions at all.
    ///
    /// Every expected value here was produced by running Claude Code's own
    /// function (2.1.296) on the same input under node. The regex has no `u`
    /// flag, so it counts UTF-16 code units, not characters or bytes.
    #[test]
    fn folder_naming_matches_claude() {
        let cases = [
            ("/home/cadu/w/cadu/my_infra", "-home-cadu-w-cadu-my-infra"),
            (
                "/home/u/forest/my.repo/feat x",
                "-home-u-forest-my-repo-feat-x",
            ),
            (
                "/home/u/a~b@c+d#e%f(g)h[i]j'k\"l",
                "-home-u-a-b-c-d-e-f-g-h-i-j-k-l",
            ),
            ("/home/u/MiXeD/Case09", "-home-u-MiXeD-Case09"),
            // Precomposed accents: one unit each.
            ("/home/u/projeto/a\u{e7}\u{e3}o", "-home-u-projeto-a--o"),
            // A combining accent is a unit of its own; the base letter stays.
            ("/home/u/cafe\u{301}", "-home-u-cafe-"),
            // CJK and Cyrillic: one unit each.
            (
                "/home/u/\u{65e5}\u{672c}\u{8a9e}/\u{440}\u{435}\u{43f}\u{43e}",
                "-home-u---------",
            ),
            // Outside the BMP: a surrogate pair, two dashes.
            ("/home/u/\u{1f332}", "-home-u---"),
            // ZWJ family: three pairs and two joiners.
            (
                "/home/u/\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
                "-home-u---------",
            ),
            // Flag: two regional indicators, each a pair.
            ("/home/u/\u{1f1e7}\u{1f1f7}", "-home-u-----"),
            // Keycap: the digit, then VS16 and the combining keycap.
            ("/home/u/1\u{fe0f}\u{20e3}", "-home-u-1--"),
        ];
        for (path, expected) in cases {
            assert_eq!(claude_folder_name(path), expected, "{path:?}");
        }
    }

    /// Past 200 units Claude keeps the first 200 and appends a hash of the
    /// *original* path in base 36. Expected values from Claude's own function,
    /// as above.
    #[test]
    fn long_folder_names_are_truncated_and_hashed_like_claude() {
        let a = |n| "a".repeat(n);
        // Hashes with a known source path: one constructed so the hash is
        // exactly i32::MIN, where JavaScript's Math.abs does not overflow.
        const MIN: &str = "hgke85q0r9ssanz1ne69fyoohnl180zn5d3i5s4vbg55pgro4qwp3vvnu132f0j9qcrtilbk4yjxchvrwt8mvniuf23oldx4cd7mi3y46vpvpbkt8o3mwsd3uscsoeq6a5ft62r6ok2a4clvgkepxtmfxx9yyg3qqzpyfhpb6sbfd5sr4ejfq16euoq0fu4pk4vr7wwq";
        let cases = [
            // Exactly 200: left alone.
            (format!("/home/u/{}", a(192)), format!("-home-u-{}", a(192))),
            // 201: hashed.
            (
                format!("/home/u/{}", a(193)),
                format!("-home-u-{}-eyl4qu", a(192)),
            ),
            // 200 characters but 201 units, because the emoji is a pair.
            (
                format!("/home/u/{}\u{1f332}", a(191)),
                format!("-home-u-{}--ezn2jg", a(191)),
            ),
            // 200 characters and 200 units: é is one.
            (
                format!("/home/u/{}\u{e9}", a(191)),
                format!("-home-u-{}-", a(191)),
            ),
            // Negative hashes (b, c) and a positive one (d).
            (
                format!("/home/u/{}", "b".repeat(250)),
                format!("-home-u-{}-aq2nlx", "b".repeat(192)),
            ),
            (
                format!("/home/u/{}", "c".repeat(250)),
                format!("-home-u-{}-x11eud", "c".repeat(192)),
            ),
            (
                format!("/home/u/{}", "d".repeat(250)),
                format!("-home-u-{}-fp3vwb", "d".repeat(192)),
            ),
            // Non-ASCII feeds the hash as UTF-16 units.
            (
                format!("/home/u/{}", "\u{65e5}\u{672c}\u{8a9e}".repeat(80)),
                format!("-home-u-{}-yy18cl", "-".repeat(192)),
            ),
            (
                format!("/home/u/{MIN}\u{c056}"),
                format!("-home-u-{}-zik0zk", &MIN[..192]),
            ),
        ];
        for (path, expected) in &cases {
            assert_eq!(&claude_folder_name(path), expected, "{path:?}");
        }
        assert_eq!(js_string_hash(&cases[8].0), i32::MIN);
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

    /// Renaming a stopped session appends the records `/rename` writes — the
    /// `custom-title`/`agent-name` pair — so the next parse sees the new name
    /// and a resumed Claude draws it in its badge, which renders the agent
    /// name, not the title. An empty transcript path errors instead of
    /// creating a stray file.
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

        // Both records land, stamped with the session id, each a valid JSON
        // line — a torn or concatenated line would be skipped by every parser.
        let raw = std::fs::read_to_string(&file).unwrap();
        let records: Vec<serde_json::Value> = raw
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str(line).expect("each appended line parses alone"))
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["type"], "custom-title");
        assert_eq!(records[0]["customTitle"], "my rename");
        assert_eq!(records[1]["type"], "agent-name");
        assert_eq!(records[1]["agentName"], "my rename");
        for record in &records {
            assert_eq!(record["sessionId"], "sess-r");
        }

        // A missing transcript is an error, not a new file.
        assert!(rename_session("/nowhere/forestui-rename-test", "ghost", "x").is_err());
        assert!(delete_session("/nowhere/forestui-rename-test", "ghost").is_err());
    }

    /// A transcript whose last line was torn mid-write (no trailing newline)
    /// still takes the rename: appending blindly would fuse the first record
    /// onto the torn line and lose it, leaving the pair half-applied.
    #[test]
    fn renaming_survives_a_transcript_with_a_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sess-t.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"hello\"}}\n\
             {\"type\":\"assistant\",\"truncated",
        )
        .unwrap();

        rename_transcript(&file, "sess-t", "after the tear").expect("the append lands");
        let session = parse_session_file(&file).expect("a session");
        assert_eq!(session.custom_title.as_deref(), Some("after the tear"));

        let raw = std::fs::read_to_string(&file).unwrap();
        let tail: Vec<&str> = raw.lines().rev().take(2).collect();
        assert!(
            tail[1].starts_with("{\"customTitle\""),
            "title on its own line"
        );
        assert!(
            tail[0].starts_with("{\"agentName\""),
            "agent name on its own line"
        );
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
            cwd: None,
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

        migrate_dirs(&old, &new, None);
        assert!(new.join("a.jsonl").exists());
        assert!(!old.exists());
    }

    fn transcript(dir: &Path, id: &str, cwd: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join(format!("{id}.jsonl"));
        let record = serde_json::json!({
            "type": "user",
            "timestamp": "2026-08-01T10:00:00Z",
            "cwd": cwd,
            "sessionId": id,
            "message": {"content": format!("hello from {id}")},
        });
        std::fs::write(&file, format!("{record}\n")).unwrap();
        file
    }

    fn ids(sessions: Vec<ClaudeSession>) -> Vec<String> {
        let mut ids: Vec<String> = sessions.into_iter().map(|s| s.id).collect();
        ids.sort();
        ids
    }

    /// `my_infra` and `my-infra` side by side under a canonical temp root,
    /// with an empty projects dir next to them.
    fn neighbours() -> (tempfile::TempDir, PathBuf, String, String) {
        let root = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(root.path()).unwrap();
        let underscore = real.join("my_infra");
        let dash = real.join("my-infra");
        std::fs::create_dir(&underscore).unwrap();
        std::fs::create_dir(&dash).unwrap();
        (
            root,
            real.join("projects"),
            underscore.to_string_lossy().into_owned(),
            dash.to_string_lossy().into_owned(),
        )
    }

    /// The two share Claude's folder. Each lists, refreshes and deletes only
    /// its own transcripts, through the same functions the app calls.
    #[test]
    fn a_shared_folder_is_split_by_recorded_cwd() {
        let (_root, base, underscore, dash) = neighbours();
        assert_eq!(claude_folder_name(&underscore), claude_folder_name(&dash));
        let shared = base.join(claude_folder_name(&underscore));
        transcript(&shared, "mine", &underscore);
        transcript(&shared, "theirs", &dash);

        assert_eq!(ids(sessions_in(&base, &underscore, 10)), ["mine"]);
        assert_eq!(ids(sessions_in(&base, &dash, 10)), ["theirs"]);
        assert!(refresh_one_in(&base, &underscore, "mine").is_some());
        assert!(refresh_one_in(&base, &underscore, "theirs").is_none());

        // Deleting is permanent, so it re-checks the file, not the list.
        assert!(delete_session_in(&base, &underscore, "theirs").is_err());
        assert!(shared.join("theirs.jsonl").exists());
        delete_session_in(&base, &underscore, "mine").unwrap();
        assert!(!shared.join("mine.jsonl").exists());
    }

    /// Only a recorded directory that exists and resolves elsewhere is
    /// someone else's. Unknown, gone, or a different slug altogether (a
    /// transcript migrated in from a renamed worktree) stays.
    #[test]
    fn what_cannot_be_told_apart_stays() {
        let (_root, _base, underscore, _dash) = neighbours();
        let root = Path::new(&underscore).parent().unwrap();
        assert!(!recorded_elsewhere(None, &underscore));
        let gone = root.join("my.infra").to_string_lossy().into_owned();
        assert!(!recorded_elsewhere(Some(&gone), &underscore));
        let renamed_from = root.join("old_name");
        std::fs::create_dir(&renamed_from).unwrap();
        assert!(!recorded_elsewhere(
            Some(&renamed_from.to_string_lossy()),
            &underscore
        ));
    }

    /// A symlinked recorded `cwd` that resolves to the project itself is the
    /// project, not a neighbour.
    #[test]
    fn a_recorded_symlink_to_the_project_is_the_project() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let real = root.join("my_infra");
        std::fs::create_dir(&real).unwrap();
        let link = root.join("my-infra");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(!recorded_elsewhere(
            Some(&link.to_string_lossy()),
            &real.to_string_lossy()
        ));
    }

    /// Claude places a transcript from its first and last 64 KiB: the first
    /// `cwd` in the head, unless the tail holds a `relocated` record. Anything
    /// outside those windows does not count, for either reader.
    #[test]
    fn the_recorded_cwd_is_read_as_claude_reads_it() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let both = |file: &Path| {
            let cheap = recorded_cwd(file);
            assert_eq!(cheap, parse_session_file(file).and_then(|s| s.cwd));
            cheap
        };
        let append = |file: &Path, line: &str| {
            let mut f = std::fs::OpenOptions::new().append(true).open(file).unwrap();
            writeln!(f, "{line}").unwrap();
        };

        let file = transcript(dir.path(), "s", "/first");
        assert_eq!(both(&file).as_deref(), Some("/first"));
        append(&file, r#"{"type":"user","cwd":"/later","sessionId":"s"}"#);
        assert_eq!(both(&file).as_deref(), Some("/first"));
        append(
            &file,
            r#"{"type":"relocated","relocatedCwd":"/moved","sessionId":"s"}"#,
        );
        assert_eq!(both(&file).as_deref(), Some("/moved"));

        // A relocation scrolled out of the tail window is no longer seen.
        let filler = format!(r#"{{"type":"progress","data":"{}"}}"#, "z".repeat(1000));
        for _ in 0..80 {
            append(&file, &filler);
        }
        assert_eq!(both(&file).as_deref(), Some("/first"));

        // A first record bigger than the window gives Claude no cwd at all.
        let big = dir.path().join("big.jsonl");
        let record = serde_json::json!({
            "type": "user",
            "timestamp": "2026-08-01T10:00:00Z",
            "message": {"content": "p".repeat(70_000)},
            "cwd": "/x",
        });
        std::fs::write(&big, format!("{record}\n")).unwrap();
        assert_eq!(both(&big), None);
    }

    /// Renaming `my_infra` carries its own history and whatever cannot be
    /// placed, and leaves `my-infra`'s in the folder they share.
    #[test]
    fn migration_leaves_a_neighbours_transcripts_behind() {
        let (_root, base, underscore, dash) = neighbours();
        let shared = base.join(claude_folder_name(&underscore));
        transcript(&shared, "mine", &underscore);
        transcript(&shared, "theirs", &dash);
        std::fs::write(shared.join("unknown.jsonl"), "{}").unwrap();

        let renamed = Path::new(&underscore).with_file_name("renamed");
        std::fs::rename(&underscore, &renamed).unwrap();
        migrate_sessions_in(&base, Path::new(&underscore), &renamed);

        let moved_to = base.join(claude_folder_name(&renamed.to_string_lossy()));
        assert!(moved_to.join("mine.jsonl").exists());
        assert!(moved_to.join("unknown.jsonl").exists());
        assert!(shared.join("theirs.jsonl").exists());
        assert!(!moved_to.join("theirs.jsonl").exists());
    }

    /// `feat_x` to `feat-x` keeps the folder; nothing is touched.
    #[test]
    fn a_rename_that_keeps_the_folder_moves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = transcript(dir.path(), "s", "/feat_x");
        migrate_dirs(dir.path(), dir.path(), None);
        assert!(file.exists());
    }

    /// Two long paths whose names differ only in punctuation hash to two
    /// folders with the same prefix, so each borrows the other's. A borrowed
    /// folder lends only transcripts recorded at the borrower's slug — and of
    /// those, not one recorded at the neighbour itself.
    #[test]
    fn long_paths_borrow_only_their_own_transcripts() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let long = root.join("x".repeat(220));
        let (a, b) = (long.join("my_infra"), long.join("my-infra"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let (a, b) = (
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        );
        let old_b = long.join("old_b").to_string_lossy().into_owned();
        let base = root.join("projects");
        let (folder_a, folder_b) = (claude_folder_name(&a), claude_folder_name(&b));
        assert_ne!(folder_a, folder_b);
        let prefix = &folder_a[..MAX_FOLDER_LEN];

        transcript(&base.join(&folder_a), "a1", &a);
        transcript(&base.join(format!("{prefix}-oldhash")), "a_old", &a);
        transcript(&base.join(&folder_b), "b1", &b);
        // What an earlier rename of B left behind: B's, by a different slug.
        transcript(&base.join(&folder_b), "b_old", &old_b);
        transcript(&base.join("-unrelated"), "stray", &a);

        assert_eq!(own_dir(&base, &a), base.join(&folder_a));
        let mut borrowed: Vec<String> = borrowed_dirs_in(&base, &a)
            .iter()
            .map(|d| d.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        borrowed.sort();
        let mut expected = [folder_b.clone(), format!("{prefix}-oldhash")];
        expected.sort();
        assert_eq!(borrowed, expected);

        assert_eq!(ids(sessions_in(&base, &a, 10)), ["a1", "a_old"]);
        assert_eq!(ids(sessions_in(&base, &b, 10)), ["b1", "b_old"]);
        // Found in a borrowed folder, not only listed from it.
        assert!(refresh_one_in(&base, &a, "a_old").is_some());
        assert!(refresh_one_in(&base, &a, "b_old").is_none());
        assert!(delete_session_in(&base, &a, "b_old").is_err());

        let renamed = long.join("renamed");
        std::fs::rename(&a, &renamed).unwrap();
        migrate_sessions_in(&base, Path::new(&a), &renamed);
        let moved_to = base.join(claude_folder_name(&renamed.to_string_lossy()));
        assert!(moved_to.join("a1.jsonl").exists());
        assert!(moved_to.join("a_old.jsonl").exists());
        assert!(base.join(&folder_b).join("b1.jsonl").exists());
        assert!(base.join(&folder_b).join("b_old.jsonl").exists());

        // A short path never scans: its folder name is exact.
        assert!(borrowed_dirs_in(&base, "/home/u/short").is_empty());

        // The memo notices a folder gaining a transcript recorded here.
        transcript(&base.join(format!("{prefix}-late")), "late", &a);
        assert_eq!(borrowed_dirs_in(&base, &a).len(), 3);
    }

    /// A neighbour missing at the moment of a rename keeps its history: what
    /// was recorded at another path is not ours to move, existing or not.
    #[test]
    fn migration_leaves_an_absent_neighbours_transcripts_behind() {
        let (_root, base, underscore, dash) = neighbours();
        std::fs::remove_dir(&dash).unwrap();
        let shared = base.join(claude_folder_name(&underscore));
        transcript(&shared, "mine", &underscore);
        transcript(&shared, "theirs", &dash);
        // Earlier migrated here from another name: ours.
        transcript(&shared, "inherited", "/somewhere/older_name");

        let renamed = Path::new(&underscore).with_file_name("renamed");
        std::fs::rename(&underscore, &renamed).unwrap();
        migrate_sessions_in(&base, Path::new(&underscore), &renamed);

        let moved_to = base.join(claude_folder_name(&renamed.to_string_lossy()));
        assert!(moved_to.join("mine.jsonl").exists());
        assert!(moved_to.join("inherited.jsonl").exists());
        assert!(shared.join("theirs.jsonl").exists());
    }

    /// Whose a transcript is can change without any transcript changing: a
    /// neighbour directory appearing or going. The cached list follows, or it
    /// would disagree with the per-card refresh every tick.
    #[test]
    fn the_cached_list_follows_a_neighbour_appearing_and_going() {
        let (_root, base, underscore, dash) = neighbours();
        std::fs::remove_dir(&dash).unwrap();
        let shared = base.join(claude_folder_name(&underscore));
        transcript(&shared, "mine", &underscore);
        transcript(&shared, "theirs", &dash);

        // Nothing says otherwise yet, so it is listed — as Claude would.
        assert_eq!(ids(sessions_in(&base, &underscore, 10)), ["mine", "theirs"]);
        std::fs::create_dir(&dash).unwrap();
        assert_eq!(ids(sessions_in(&base, &underscore, 10)), ["mine"]);
        assert!(refresh_one_in(&base, &underscore, "theirs").is_none());
        std::fs::remove_dir(&dash).unwrap();
        assert_eq!(ids(sessions_in(&base, &underscore, 10)), ["mine", "theirs"]);
    }

    /// A recorded path with `..` is never resolved, as in Claude. Built so it
    /// would otherwise count: the same slug as the project, resolving (via a
    /// symlink) to a real directory with that slug too.
    #[test]
    fn a_recorded_parent_dir_is_not_resolved() {
        let root = tempfile::tempdir().unwrap();
        let r = std::fs::canonicalize(root.path()).unwrap();
        std::fs::create_dir_all(r.join("x.__").join("d")).unwrap();
        std::fs::create_dir_all(r.join("x.__").join("y")).unwrap();
        std::os::unix::fs::symlink(r.join("x.__").join("d"), r.join("x")).unwrap();
        let project = r.join("x_.._y");
        std::fs::create_dir(&project).unwrap();
        let project = project.to_string_lossy().into_owned();
        let cwd = format!("{}/x/../y", r.display());
        let real = std::fs::canonicalize(&cwd).unwrap();
        assert_eq!(claude_slug(&cwd), claude_slug(&project));
        assert_eq!(claude_slug(&real.to_string_lossy()), claude_slug(&project));

        assert!(!recorded_elsewhere(Some(&cwd), &project));
        // The same directory named without `..` does count.
        assert!(recorded_elsewhere(Some(&real.to_string_lossy()), &project));
    }

    /// A transcript already at the destination is never overwritten.
    #[test]
    fn migration_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join("old"), root.path().join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("s.jsonl"), "old").unwrap();
        std::fs::write(new.join("s.jsonl"), "new").unwrap();
        migrate_dirs(&old, &new, None);
        assert_eq!(std::fs::read_to_string(new.join("s.jsonl")).unwrap(), "new");
        assert!(old.join("s.jsonl").exists());
    }
}
