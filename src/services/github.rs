//! GitHub integration via the `gh` CLI.

use crate::models::GitHubIssue;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tokio::process::Command;

pub const CACHE_TTL_SECONDS: i64 = 300;

/// Result of `gh auth status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    Authenticated,
    NotAuthenticated,
    NotInstalled,
}

impl AuthStatus {
    /// Short text shown in the sidebar header.
    pub fn display(&self, username: Option<&str>) -> String {
        match self {
            AuthStatus::Authenticated => match username {
                Some(u) => format!("ok ({u})"),
                None => "ok".to_string(),
            },
            AuthStatus::NotAuthenticated => "unauth'd".to_string(),
            AuthStatus::NotInstalled => "missing".to_string(),
        }
    }
}

struct Cached {
    issues: Vec<GitHubIssue>,
    fetched_at: DateTime<Utc>,
}

/// The cache is keyed by the repository *path* — the one identifier the app
/// holds synchronously. Keying by `owner/repo` made every lookup pay a
/// `gh repo view` subprocess before it could even ask the cache, which is why
/// the issues panel flashed "Loading…" on every selection change (#29).
#[derive(Default)]
struct State {
    cache: HashMap<String, Cached>,
    auth: Option<(AuthStatus, Option<String>)>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

/// Exit code reserved for a process that died to a signal.
///
/// It cannot be `0` (success), `-1` (reserved for "gh is not installed"), or
/// `1` — `gh auth status` returns 1 for "not logged in", and conflating the two
/// let one killed probe pin the whole session to "unauth'd".
pub const KILLED: i32 = -2;

fn exit_code(status: std::process::ExitStatus) -> i32 {
    crate::util::exit_code(status, KILLED)
}

/// Run a `gh` command. Exit code `-1` means the binary is missing.
async fn run_gh(args: &[&str], cwd: Option<&Path>) -> (i32, String, String) {
    let mut cmd = Command::new("gh");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    match cmd.output().await {
        Ok(output) => (
            exit_code(output.status),
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ),
        Err(_) => (-1, String::new(), "gh not found".to_string()),
    }
}

/// Authentication status and username. Cached for the lifetime of the process,
/// matching the Textual build.
pub async fn get_auth_status() -> (AuthStatus, Option<String>) {
    if let Ok(guard) = state().lock()
        && let Some(cached) = &guard.auth
    {
        return cached.clone();
    }

    let (code, _out, _err) = run_gh(&["auth", "status"], None).await;
    if code == KILLED {
        // A probe that was killed answered nothing. Caching that as an answer
        // would leave the sidebar reading "unauth'd" and the issues list empty
        // for the rest of the session, with no path back short of a restart.
        return (AuthStatus::NotAuthenticated, None);
    }
    let result = if code == -1 {
        (AuthStatus::NotInstalled, None)
    } else if code == 0 {
        let (code, stdout, _) = run_gh(&["api", "user", "--jq", ".login"], None).await;
        let username = if code == 0 && !stdout.is_empty() {
            Some(stdout)
        } else {
            None
        };
        (AuthStatus::Authenticated, username)
    } else {
        (AuthStatus::NotAuthenticated, None)
    };

    if let Ok(mut guard) = state().lock() {
        guard.auth = Some(result.clone());
    }
    result
}

/// `(owner, repo)` for a path, or `None` when it is not a GitHub repository.
pub async fn get_repo_info(path: &str) -> Option<(String, String)> {
    let (code, stdout, _) = run_gh(
        &["repo", "view", "--json", "owner,name"],
        Some(Path::new(path)),
    )
    .await;
    if code != 0 || stdout.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let owner = value.get("owner")?.get("login")?.as_str()?.to_string();
    let name = value.get("name")?.as_str()?.to_string();
    Some((owner, name))
}

/// Cached issues for a path without any I/O: `Some((issues, fresh))` when a
/// fetch has completed this run, `None` when nothing is cached. Safe to call
/// from the render thread — the lock is only ever held for map operations,
/// never across an await.
pub fn peek_issues(path: &str) -> Option<(Vec<GitHubIssue>, bool)> {
    let guard = state().lock().ok()?;
    let cached = guard.cache.get(path)?;
    let fresh = (Utc::now() - cached.fetched_at).num_seconds() < CACHE_TTL_SECONDS;
    Some((cached.issues.clone(), fresh))
}

/// Open issues assigned to or authored by the current user, newest first.
pub async fn list_issues(path: &str, limit: usize) -> Vec<GitHubIssue> {
    // The cache answers before any subprocess runs; a warm hit costs nothing.
    if let Ok(guard) = state().lock()
        && let Some(cached) = guard.cache.get(path)
        && (Utc::now() - cached.fetched_at).num_seconds() < CACHE_TTL_SECONDS
    {
        return cached.issues.clone();
    }

    // The empty answers are cached too: a local-only repository or an
    // unauthenticated `gh` would otherwise re-flash "Loading…" and re-spawn
    // subprocesses on every visit, forever.
    let (auth, _) = get_auth_status().await;
    if auth != AuthStatus::Authenticated {
        return store(path, Vec::new());
    }

    // Only the gate cares about owner/repo: a path that is not a GitHub
    // repository has no issues to list.
    if get_repo_info(path).await.is_none() {
        return store(path, Vec::new());
    }

    store(path, fetch_issues(path, limit).await)
}

fn store(path: &str, issues: Vec<GitHubIssue>) -> Vec<GitHubIssue> {
    if let Ok(mut guard) = state().lock() {
        guard.cache.insert(
            path.to_string(),
            Cached {
                issues: issues.clone(),
                fetched_at: Utc::now(),
            },
        );
    }
    issues
}

/// Seed the cache directly, for tests that must not shell out to `gh`.
#[cfg(test)]
pub fn seed_cache_for_test(path: &str, issues: Vec<GitHubIssue>, fetched_at: DateTime<Utc>) {
    if let Ok(mut guard) = state().lock() {
        guard
            .cache
            .insert(path.to_string(), Cached { issues, fetched_at });
    }
}

async fn fetch_issues(path: &str, limit: usize) -> Vec<GitHubIssue> {
    const JSON_FIELDS: &str = "number,title,state,url,createdAt,updatedAt,author,assignees,labels";
    let limit_str = limit.to_string();

    let mut issues: Vec<GitHubIssue> = Vec::new();
    let mut seen: Vec<i64> = Vec::new();

    for filter in ["--assignee", "--author"] {
        let (code, stdout, _) = run_gh(
            &[
                "issue",
                "list",
                filter,
                "@me",
                "--state",
                "open",
                "--limit",
                &limit_str,
                "--json",
                JSON_FIELDS,
            ],
            Some(Path::new(path)),
        )
        .await;
        if code != 0 || stdout.is_empty() {
            continue;
        }
        let parsed: Vec<GitHubIssue> = serde_json::from_str(&stdout).unwrap_or_default();
        for issue in parsed {
            if !seen.contains(&issue.number) {
                seen.push(issue.number);
                issues.push(issue);
            }
        }
    }

    issues.sort_by_key(|i| std::cmp::Reverse(i.created_at));
    issues.truncate(limit);
    issues
}

/// Drop cached issues for one repository path, or all of them when `path` is
/// `None`. Prefer the targeted form: clearing everything makes the next
/// selection change on every other repository a guaranteed cold miss.
pub fn invalidate_cache(path: Option<&str>) {
    if let Ok(mut guard) = state().lock() {
        match path {
            Some(path) => {
                guard.cache.remove(path);
            }
            None => guard.cache.clear(),
        }
    }
}

impl Clone for Cached {
    fn clone(&self) -> Self {
        Self {
            issues: self.issues.clone(),
            fetched_at: self.fetched_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_status_display_text() {
        assert_eq!(AuthStatus::Authenticated.display(Some("kir")), "ok (kir)");
        assert_eq!(AuthStatus::Authenticated.display(None), "ok");
        assert_eq!(AuthStatus::NotAuthenticated.display(None), "unauth'd");
        assert_eq!(AuthStatus::NotInstalled.display(None), "missing");
    }

    /// A killed `gh` must fail, but must not masquerade as a missing binary
    /// (-1 flips the sidebar to "missing"), nor as a real "not logged in"
    /// (1, which `get_auth_status` caches for the lifetime of the process).
    #[test]
    fn signal_killed_process_is_a_plain_failure() {
        let status = std::process::Command::new("sh")
            .args(["-c", "kill -9 $$"])
            .status()
            .unwrap();
        assert!(status.code().is_none(), "expected a signal-killed process");
        let code = exit_code(status);
        assert_ne!(code, 0);
        assert_ne!(code, -1);
        assert_ne!(
            code, 1,
            "a killed probe is indistinguishable from `gh auth status` saying \
             'not logged in', which is cached and never retried"
        );
        assert_eq!(code, KILLED);
    }

    /// The synchronous peek is what lets the render thread paint cached issues
    /// on selection change instead of flashing "Loading…" (#29).
    #[test]
    fn peek_reports_freshness_without_io() {
        assert!(peek_issues("/nowhere/never-seen").is_none());

        let issue: GitHubIssue = serde_json::from_str(
            r#"{"number":1,"title":"T","state":"OPEN","url":"https://x/1",
                "createdAt":"2026-08-01T10:00:00Z","updatedAt":"2026-08-01T10:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
        )
        .unwrap();

        seed_cache_for_test("/repo/fresh", vec![issue.clone()], Utc::now());
        let (issues, fresh) = peek_issues("/repo/fresh").expect("cached");
        assert_eq!(issues.len(), 1);
        assert!(fresh);

        let old = Utc::now() - chrono::Duration::seconds(CACHE_TTL_SECONDS + 10);
        seed_cache_for_test("/repo/stale", vec![issue], old);
        let (_, fresh) = peek_issues("/repo/stale").expect("cached");
        assert!(!fresh, "an expired entry must read as stale");

        invalidate_cache(Some("/repo/fresh"));
        assert!(peek_issues("/repo/fresh").is_none());
        assert!(
            peek_issues("/repo/stale").is_some(),
            "targeted invalidation"
        );
    }

    #[test]
    fn issue_list_parses_gh_json() {
        let raw = r#"[
          {"number":7,"title":"Bug","state":"OPEN","url":"https://x/7",
           "createdAt":"2026-08-01T10:00:00Z","updatedAt":"2026-08-02T10:00:00Z",
           "author":{"login":"me"},"assignees":[{"login":"me"}],
           "labels":[{"name":"bug","color":"ff0000"}]}
        ]"#;
        let parsed: Vec<GitHubIssue> = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed[0].number, 7);
        assert_eq!(parsed[0].labels[0].name, "bug");
        assert_eq!(parsed[0].branch_name(), "7-bug");
    }
}
