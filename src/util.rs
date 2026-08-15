//! Shared helpers: path expansion, slugs, relative times, fuzzy branch search.

use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

pub const MAX_DROPDOWN_RESULTS: usize = 50;

/// The user's home directory, falling back to the current directory.
///
/// `std::env::home_dir` was un-deprecated in Rust 1.87 with corrected
/// semantics, so no `dirs`-style crate is needed here.
pub fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Expand a leading `~` to the user's home directory.
pub fn expanduser(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if path == "~" {
        return home_dir();
    }
    PathBuf::from(path)
}

/// Expand `~` and resolve the path, falling back to the expanded form when the
/// path does not exist yet.
pub fn expand_and_resolve(path: &str) -> PathBuf {
    let expanded = expanduser(path);
    std::fs::canonicalize(&expanded).unwrap_or(expanded)
}

/// Convert text to a safe slug for tmux session names.
pub fn slugify(text: &str) -> String {
    let lowered = text.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Human-readable relative time, matching the phrasing of Python's
/// `humanize.naturaltime` so screen text is unchanged from the Textual build.
pub fn naturaltime(when: DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(when);
    let future = delta.num_seconds() < 0;
    let secs = delta.num_seconds().unsigned_abs();

    if secs < 1 {
        return "now".to_string();
    }
    let magnitude = naturaldelta(secs);
    if future {
        format!("{magnitude} from now")
    } else {
        format!("{magnitude} ago")
    }
}

/// The bare magnitude phrase used by [`naturaltime`].
///
/// A port of `humanize.naturaldelta`, which the Textual build called directly.
/// Its shape is not obvious and the boundaries are not round numbers — sub-day
/// units are *rounded* rather than truncated, and anything from 16 days up is
/// fuzzed into months at 30.5 days each — so it is transcribed rather than
/// reinvented. Getting this wrong is quiet: every session and issue card grows
/// a slightly different timestamp than the Python build showed.
pub fn naturaldelta(secs: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let total_days = secs / DAY;
    // `timedelta.seconds` is the remainder below a whole day, not the total.
    let seconds = secs % DAY;
    let years = total_days / 365;
    let days = total_days % 365;
    // `round(days / 30.5)`, kept in integers as `round(2 * days / 61)`.
    let months = round_half_even(days * 2, 61);

    if total_days == 0 {
        return match seconds {
            0 => "a moment".into(),
            1 => "a second".into(),
            s if s < MINUTE => format!("{s} seconds"),
            s if s < HOUR => match round_half_even(s, MINUTE) {
                1 => "a minute".into(),
                60 => "an hour".into(),
                minutes => format!("{minutes} minutes"),
            },
            s => match round_half_even(s, HOUR) {
                1 => "an hour".into(),
                24 => "a day".into(),
                hours => format!("{hours} hours"),
            },
        };
    }

    match (years, days, months) {
        (0, 1, _) => "a day".into(),
        (0, _, 0) => format!("{days} days"),
        (0, _, 1) => "a month".into(),
        (0, _, 12) => "a year".into(),
        (0, _, _) => format!("{months} months"),
        (1, 0, 0) => "a year".into(),
        (1, 1, 0) => "1 year, 1 day".into(),
        (1, _, 0) => format!("1 year, {days} days"),
        (1, _, 1) => "1 year, 1 month".into(),
        (1, _, 12) => "2 years".into(),
        (1, _, _) => format!("1 year, {months} months"),
        _ => format!("{years} years"),
    }
}

/// Divide and round half to even, the way Python's `round` does. The tie
/// matters: 90 seconds is 2 minutes but 150 seconds is also 2, not 3.
fn round_half_even(value: u64, unit: u64) -> u64 {
    let quotient = value / unit;
    let twice_remainder = (value % unit) * 2;
    if twice_remainder > unit || (twice_remainder == unit && quotient % 2 == 1) {
        quotient + 1
    } else {
        quotient
    }
}

/// Truncate to `max` characters, appending `...` when the text was longer.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let head: String = text.chars().take(max).collect();
        format!("{head}...")
    } else {
        text.to_string()
    }
}

/// Whether the directory exists on disk.
pub fn path_exists(path: &str) -> bool {
    Path::new(&expanduser(path)).exists()
}

/// Write a file via a sibling temp file and an atomic rename.
///
/// `.forestui-config.json` is the only record of every tracked repository and
/// worktree, and a corrupt file deliberately loads as *empty* state so a bad
/// byte never blocks startup — which turns a crash mid-`fs::write` into total,
/// silent data loss on the next launch. The rename makes that window
/// impossible: readers see either the old file or the new one, never a torn
/// half. The temp file lives beside the target because a rename is only atomic
/// within one filesystem.
pub fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut temp = path.to_path_buf();
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    name.push_str(&format!(".tmp-{}", std::process::id()));
    temp.set_file_name(name);

    std::fs::write(&temp, contents)?;
    let renamed = std::fs::rename(&temp, path);
    if renamed.is_err() {
        // Leave no half-written sibling behind; the original is untouched.
        let _ = std::fs::remove_file(&temp);
    }
    renamed
}

fn levenshtein_distance(s1: &[char], s2: &[char]) -> usize {
    if s1.len() < s2.len() {
        return levenshtein_distance(s2, s1);
    }
    if s2.is_empty() {
        return s1.len();
    }

    let mut prev_row: Vec<usize> = (0..=s2.len()).collect();
    for &c1 in s1 {
        let mut curr_row = Vec::with_capacity(s2.len() + 1);
        curr_row.push(prev_row[0] + 1);
        for (j, &c2) in s2.iter().enumerate() {
            let insertions = prev_row[j + 1] + 1;
            let deletions = curr_row[j] + 1;
            let substitutions = prev_row[j] + usize::from(c1 != c2);
            curr_row.push(insertions.min(deletions).min(substitutions));
        }
        prev_row = curr_row;
    }
    prev_row[s2.len()]
}

/// Strip a remote prefix (e.g. `origin/`) using the repository's real remotes.
pub fn strip_remote_prefix<'a>(branch: &'a str, remotes: &[String]) -> &'a str {
    if let Some((prefix, rest)) = branch.split_once('/')
        && remotes.iter().any(|r| r == prefix)
    {
        return rest;
    }
    branch
}

/// Score a branch against a query. Lower is better; `None` means no match.
///
/// Scoring tiers (identical to the Textual implementation):
/// `0.0` exact, `0.5` exact on local name, `1.0` prefix on full name,
/// `1.5` prefix on local name, `2.0` substring at a word boundary,
/// `3.0` substring anywhere, `4.0+` fuzzy (Levenshtein) on path segments.
fn match_score(query: &str, branch: &str, remotes: &[String]) -> Option<f64> {
    let q = query.to_lowercase();
    let b = branch.to_lowercase();

    if q.is_empty() {
        return Some(0.0);
    }
    if q == b {
        return Some(0.0);
    }

    let local = strip_remote_prefix(&b, remotes).to_string();

    if q == local {
        return Some(0.5);
    }
    if b.starts_with(&q) {
        return Some(1.0);
    }
    if local.starts_with(&q) {
        return Some(1.5);
    }

    for target in [&b, &local] {
        if let Some(idx) = target.find(&q) {
            let boundary = idx == 0
                || target[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| matches!(c, '/' | '-' | '_' | '.'));
            return Some(if boundary { 2.0 } else { 3.0 });
        }
    }

    let q_chars: Vec<char> = q.chars().collect();
    if q_chars.len() >= 2 {
        let mut best: Option<f64> = None;
        let threshold = q_chars.len().div_ceil(3).max(1);

        for seg in b.split(['/', '-', '_', '.']) {
            if seg.is_empty() {
                continue;
            }
            let seg_chars: Vec<char> = seg.chars().collect();

            let dist = levenshtein_distance(&q_chars, &seg_chars);
            if dist <= threshold {
                let score = 4.0 + dist as f64 * 0.1;
                best = Some(best.map_or(score, |b: f64| b.min(score)));
            }

            if seg_chars.len() > q_chars.len() {
                let prefix: Vec<char> = seg_chars[..q_chars.len()].to_vec();
                let prefix_dist = levenshtein_distance(&q_chars, &prefix);
                if prefix_dist <= threshold {
                    let score = 4.5 + prefix_dist as f64 * 0.1;
                    best = Some(best.map_or(score, |b: f64| b.min(score)));
                }
            }
        }
        return best;
    }

    None
}

/// Match branches against a query, best first.
pub fn fuzzy_match_branches(
    query: &str,
    branches: &[String],
    remotes: &[String],
    max_results: usize,
) -> Vec<(String, f64)> {
    if query.trim().is_empty() {
        return branches
            .iter()
            .take(max_results)
            .map(|b| (b.clone(), 0.0))
            .collect();
    }

    let mut results: Vec<(String, f64)> = branches
        .iter()
        .filter_map(|b| match_score(query, b, remotes).map(|s| (b.clone(), s)))
        .collect();

    results.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    results.truncate(max_results);
    results
}

/// Byte range of the literal `query` inside `branch`, for highlighting.
pub fn highlight_range(query: &str, branch: &str) -> Option<(usize, usize)> {
    if query.is_empty() {
        return None;
    }
    let idx = branch.to_lowercase().find(&query.to_lowercase())?;
    Some((idx, idx + query.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn slugify_matches_python() {
        assert_eq!(slugify("My Forest!"), "my-forest");
        assert_eq!(slugify("forest"), "forest");
        assert_eq!(slugify("--a--b--"), "a-b");
    }

    /// Every expectation here was read off `humanize.naturaldelta` itself
    /// rather than reasoned about, because its boundaries are not where you
    /// would put them: 16 days is already "a month", 150 seconds is 2 minutes
    /// and not 3, and 364 days is "a year".
    #[test]
    fn naturaldelta_matches_humanize() {
        const DAY: u64 = 24 * 60 * 60;
        let cases: [(u64, &str); 27] = [
            (1, "a second"),
            (2, "2 seconds"),
            (59, "59 seconds"),
            (60, "a minute"),
            (89, "a minute"),
            (90, "2 minutes"),
            (150, "2 minutes"),
            (151, "3 minutes"),
            (1800, "30 minutes"),
            (3599, "an hour"),
            (3600, "an hour"),
            (5400, "2 hours"),
            (18000, "5 hours"),
            (86399, "a day"),
            (86400, "a day"),
            (108000, "a day"),
            (129600, "a day"),
            (2 * DAY, "2 days"),
            (5 * DAY, "5 days"),
            (15 * DAY, "15 days"),
            (16 * DAY, "a month"),
            (45 * DAY, "a month"),
            (46 * DAY, "2 months"),
            (200 * DAY, "7 months"),
            (364 * DAY, "a year"),
            (366 * DAY, "1 year, 1 day"),
            (1000 * DAY, "2 years"),
        ];
        for (secs, expected) in cases {
            assert_eq!(naturaldelta(secs), expected, "at {secs} seconds");
        }
    }

    #[test]
    fn naturaltime_adds_tense() {
        let now = Utc::now();
        assert_eq!(naturaltime(now - Duration::seconds(30)), "30 seconds ago");
        assert_eq!(naturaltime(now - Duration::hours(30)), "a day ago");
        // A second of slack: the elapsed time between `now` and the call would
        // otherwise round 5h down.
        assert_eq!(
            naturaltime(now + Duration::hours(5) + Duration::seconds(1)),
            "5 hours from now"
        );
    }

    #[test]
    fn strip_remote_prefix_uses_real_remotes() {
        let remotes = vec!["origin".to_string(), "upstream".to_string()];
        assert_eq!(strip_remote_prefix("origin/main", &remotes), "main");
        assert_eq!(strip_remote_prefix("feat/thing", &remotes), "feat/thing");
    }

    #[test]
    fn fuzzy_scoring_tiers() {
        let remotes = vec!["origin".to_string()];
        let branches: Vec<String> = ["main", "origin/main", "feat/login", "release-2"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let out = fuzzy_match_branches("main", &branches, &remotes, 10);
        assert_eq!(out[0].0, "main");
        assert_eq!(out[0].1, 0.0);
        assert_eq!(out[1].0, "origin/main");
        assert_eq!(out[1].1, 0.5);

        let out = fuzzy_match_branches("login", &branches, &remotes, 10);
        assert_eq!(out[0].0, "feat/login");
        assert_eq!(out[0].1, 2.0);

        // Empty query returns everything, capped.
        let out = fuzzy_match_branches("", &branches, &remotes, 2);
        assert_eq!(out.len(), 2);

        // Typo still matches through the Levenshtein tier.
        let out = fuzzy_match_branches("relase", &branches, &remotes, 10);
        assert!(out.iter().any(|(b, _)| b == "release-2"));
    }

    #[test]
    fn highlight_range_finds_literal() {
        assert_eq!(highlight_range("ain", "main"), Some((1, 4)));
        assert_eq!(highlight_range("zzz", "main"), None);
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.json");
        std::fs::write(&target, "old").unwrap();

        write_atomically(&target, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");

        // No staging file may survive the swap.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name() != "config.json")
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind: {leftovers:?}");
    }

    #[test]
    fn truncate_appends_ellipsis() {
        assert_eq!(truncate("abcdef", 3), "abc...");
        assert_eq!(truncate("ab", 3), "ab");
    }
}
