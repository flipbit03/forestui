//! Data models for forestui.
//!
//! The on-disk JSON produced here is byte-compatible with the Pydantic models
//! the Python implementation used, so `.forestui-config.json` and
//! `~/.config/forestui/settings.json` can be shared between builds.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_CLAUDE_COMMAND_LENGTH: usize = 200;
pub const MAX_BUTTON_LABEL_LENGTH: usize = 20;
pub const MAX_BUTTON_PREFIX_LENGTH: usize = 20;

/// Derive a tmux-safe window prefix from a button label.
///
/// Lowercase, keep `[a-z0-9_-]`, collapse other runs to `-`, strip leading and
/// trailing `-`, truncate to [`MAX_BUTTON_PREFIX_LENGTH`].
pub fn derive_prefix(label: &str) -> String {
    let lowered = label.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-' {
            if pending_dash {
                out.push('-');
                pending_dash = false;
            }
            out.push(ch);
        } else if !out.is_empty() {
            pending_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    trimmed.chars().take(MAX_BUTTON_PREFIX_LENGTH).collect()
}

fn has_control_chars(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\n' | '\r' | '\t' | '\0'))
}

pub fn validate_button_label(label: &str) -> Option<String> {
    if label.is_empty() {
        return Some("Label cannot be empty".into());
    }
    if label.chars().count() > MAX_BUTTON_LABEL_LENGTH {
        return Some(format!(
            "Label too long (max {MAX_BUTTON_LABEL_LENGTH} characters)"
        ));
    }
    if has_control_chars(label) {
        return Some("Label cannot contain control characters".into());
    }
    None
}

pub fn validate_button_prefix(prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return Some("Prefix cannot be empty".into());
    }
    if prefix.chars().count() > MAX_BUTTON_PREFIX_LENGTH {
        return Some(format!(
            "Prefix too long (max {MAX_BUTTON_PREFIX_LENGTH} characters)"
        ));
    }
    let valid = prefix
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        return Some("Prefix must be lowercase letters, digits, '-' or '_'".into());
    }
    None
}

/// Validate a custom Claude command. An empty command is valid here (it clears
/// the value); callers that require a command check for emptiness themselves.
pub fn validate_claude_command(command: &str) -> Option<String> {
    if command.is_empty() {
        return None;
    }
    if command.chars().count() > MAX_CLAUDE_COMMAND_LENGTH {
        return Some(format!(
            "Command too long (max {MAX_CLAUDE_COMMAND_LENGTH} characters)"
        ));
    }
    if has_control_chars(command) {
        return Some("Command cannot contain newlines or control characters".into());
    }
    None
}

/// A user-configured custom Claude command button.
///
/// `label` is shown on the button. `prefix` is the tmux window prefix
/// (`"yolodisc"` produces `yolodisc:<name>`). `command` runs as-is; if it
/// contains `--dangerously-skip-permissions` the button is styled red.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomClaudeButton {
    pub label: String,
    pub prefix: String,
    pub command: String,
}

impl CustomClaudeButton {
    /// Whether this button's command enables the permissions bypass.
    pub fn is_yolo_style(&self) -> bool {
        self.command.contains("--dangerously-skip-permissions")
    }
}

fn default_true() -> bool {
    true
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Represents a Git worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub branch: String,
    pub path: String,
    #[serde(default)]
    pub is_archived: bool,
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default = "now")]
    pub last_modified: DateTime<Utc>,
    /// Branch this worktree was created from (e.g. `origin/main`).
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Git commit ref at the time the worktree was created.
    #[serde(default)]
    pub created_from_ref: Option<String>,
}

impl Worktree {
    pub fn new(name: String, branch: String, path: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            branch,
            path,
            is_archived: false,
            sort_order: None,
            last_modified: Utc::now(),
            base_branch: None,
            created_from_ref: None,
        }
    }
}

/// Represents a Git repository with its worktrees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub source_path: String,
    #[serde(default)]
    pub worktrees: Vec<Worktree>,
}

impl Repository {
    pub fn new(name: String, source_path: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            source_path,
            worktrees: Vec::new(),
        }
    }

    /// Active (non-archived) worktrees sorted by explicit order, then recency.
    pub fn active_worktrees(&self) -> Vec<&Worktree> {
        let mut active: Vec<&Worktree> = self.worktrees.iter().filter(|w| !w.is_archived).collect();
        active.sort_by(|a, b| {
            let ao = a.sort_order.unwrap_or(i64::MAX);
            let bo = b.sort_order.unwrap_or(i64::MAX);
            ao.cmp(&bo)
                .then_with(|| b.last_modified.cmp(&a.last_modified))
        });
        active
    }

    /// Archived worktrees sorted by recency.
    pub fn archived_worktrees(&self) -> Vec<&Worktree> {
        let mut archived: Vec<&Worktree> =
            self.worktrees.iter().filter(|w| w.is_archived).collect();
        archived.sort_by_key(|w| std::cmp::Reverse(w.last_modified));
        archived
    }
}

/// A Claude Code session discovered on disk.
#[derive(Debug, Clone)]
pub struct ClaudeSession {
    pub id: String,
    pub title: String,
    pub last_message: String,
    pub last_timestamp: DateTime<Utc>,
    pub message_count: usize,
}

impl ClaudeSession {
    pub fn relative_time(&self) -> String {
        crate::util::naturaltime(self.last_timestamp)
    }
}

fn default_editor() -> String {
    "vim".into()
}

fn default_branch_prefix() -> String {
    "feat/".into()
}

fn default_theme() -> String {
    // A theme slug from `theme::THEMES`. Old settings files carry the legacy
    // "system"/"dark"/"light" strings, which load fine and resolve to this
    // default at activation time — the field's schema never changed shape.
    "forest-dark".into()
}

/// Application settings, persisted to `~/.config/forestui/settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_editor")]
    pub default_editor: String,
    #[serde(default)]
    pub default_terminal: String,
    #[serde(default = "default_branch_prefix")]
    pub branch_prefix: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub custom_buttons: Vec<CustomClaudeButton>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_editor: default_editor(),
            default_terminal: String::new(),
            branch_prefix: default_branch_prefix(),
            theme: default_theme(),
            custom_buttons: Vec::new(),
        }
    }
}

/// The current sidebar selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub repository_id: Option<Uuid>,
    pub worktree_id: Option<Uuid>,
}

impl Selection {
    pub fn is_repository(&self) -> bool {
        self.repository_id.is_some() && self.worktree_id.is_none()
    }

    pub fn is_worktree(&self) -> bool {
        self.worktree_id.is_some()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubLabel {
    pub name: String,
}

/// A GitHub issue as returned by `gh issue list --json ...`.
///
/// Only the fields the UI shows are modelled; serde ignores the rest of the
/// payload, so the `--json` request stays unchanged.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubIssue {
    #[serde(default)]
    pub number: i64,
    #[serde(default)]
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub labels: Vec<GitHubLabel>,
}

impl GitHubIssue {
    /// Branch-safe name derived from the issue, e.g. `42-fix-login-bug`.
    pub fn branch_name(&self) -> String {
        let lowered = self.title.to_lowercase();
        let mut slug = String::with_capacity(lowered.len());
        let mut pending_dash = false;
        for ch in lowered.chars() {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
                if pending_dash && !slug.is_empty() {
                    slug.push('-');
                }
                pending_dash = false;
                slug.push(ch);
            } else {
                pending_dash = true;
            }
        }
        if pending_dash && !slug.is_empty() {
            slug.push('-');
        }
        let truncated: String = slug.chars().take(40).collect();
        format!("{}-{}", self.number, truncated.trim_matches('-'))
    }

    pub fn relative_time(&self) -> String {
        crate::util::naturaltime(self.updated_at)
    }
}

/// Serializable form of the per-forest state file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppStateData {
    #[serde(default)]
    pub repositories: Vec<Repository>,
}

/// Marker so `serde` keeps `default_true` referenced even if unused by fields.
#[allow(dead_code)]
const _: fn() -> bool = default_true;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_prefix_slugifies() {
        assert_eq!(derive_prefix("YoloDisc"), "yolodisc");
        assert_eq!(derive_prefix("New Session: Opus"), "new-session-opus");
        assert_eq!(derive_prefix("--weird--"), "weird");
        assert_eq!(derive_prefix(""), "");
        assert_eq!(derive_prefix("a".repeat(40).as_str()).len(), 20);
    }

    #[test]
    fn button_validation_matches_python_rules() {
        assert!(validate_button_label("").is_some());
        assert!(validate_button_label(&"x".repeat(21)).is_some());
        assert!(validate_button_label("ok\nno").is_some());
        assert!(validate_button_label("fine").is_none());

        assert!(validate_button_prefix("Bad").is_some());
        assert!(validate_button_prefix("good-1_2").is_none());

        assert!(validate_claude_command("").is_none());
        assert!(validate_claude_command(&"x".repeat(201)).is_some());
        assert!(validate_claude_command("claude --model opus").is_none());
    }

    #[test]
    fn yolo_style_detection() {
        let b = CustomClaudeButton {
            label: "x".into(),
            prefix: "x".into(),
            command: "claude --dangerously-skip-permissions".into(),
        };
        assert!(b.is_yolo_style());
    }

    #[test]
    fn issue_branch_name() {
        let issue: GitHubIssue = serde_json::from_str(
            r#"{"number":42,"title":"Fix login bug!","state":"OPEN","url":"u",
                "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
        )
        .unwrap();
        assert_eq!(issue.branch_name(), "42-fix-login-bug");
    }

    #[test]
    fn active_worktrees_sorted_by_order_then_recency() {
        let mut repo = Repository::new("r".into(), "/tmp/r".into());
        let mut a = Worktree::new("a".into(), "b".into(), "/tmp/a".into());
        a.sort_order = Some(1);
        let mut b = Worktree::new("b".into(), "b".into(), "/tmp/b".into());
        b.sort_order = Some(0);
        let mut c = Worktree::new("c".into(), "b".into(), "/tmp/c".into());
        c.is_archived = true;
        repo.worktrees = vec![a, b, c];
        let names: Vec<&str> = repo
            .active_worktrees()
            .iter()
            .map(|w| w.name.as_str())
            .collect();
        assert_eq!(names, vec!["b", "a"]);
        assert_eq!(repo.archived_worktrees().len(), 1);
    }

    #[test]
    fn state_roundtrips_python_json_shape() {
        let json = r#"{
          "repositories": [
            {"id": "0f2f2b7e-4d9c-4a1f-9f1a-1f2a3b4c5d6e", "name": "demo",
             "source_path": "/tmp/demo", "worktrees": [
               {"id": "1f2f2b7e-4d9c-4a1f-9f1a-1f2a3b4c5d6e", "name": "wt",
                "branch": "feat/x", "path": "/tmp/forest/demo/wt",
                "is_archived": false, "sort_order": null,
                "last_modified": "2026-08-14T18:37:28.123456Z",
                "base_branch": "origin/main", "created_from_ref": "abc1234"}]}]}"#;
        let state: AppStateData = serde_json::from_str(json).unwrap();
        assert_eq!(state.repositories.len(), 1);
        assert_eq!(state.repositories[0].worktrees[0].branch, "feat/x");
        let out = serde_json::to_string(&state).unwrap();
        assert!(out.contains("\"base_branch\":\"origin/main\""));
    }
}
