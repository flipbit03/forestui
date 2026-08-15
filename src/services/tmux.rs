//! tmux integration.
//!
//! Every call shells out to the `tmux` binary. The commands are short-lived and
//! synchronous — the same blocking behaviour the libtmux-based build had.

use std::process::Command;

pub const TUI_EDITORS: [&str; 10] = [
    "vim", "nvim", "vi", "emacs", "nano", "helix", "hx", "micro", "kakoune", "kak",
];

/// Are we running inside a tmux session?
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok_and(|v| !v.is_empty())
}

/// Is this editor a TUI editor that should be opened in a tmux window?
pub fn is_tui_editor(editor: &str) -> bool {
    let base = editor.split_whitespace().next().unwrap_or("");
    TUI_EDITORS.contains(&base)
}

fn tmux(args: &[&str]) -> Option<String> {
    let output = Command::new("tmux").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// The session of the most recently active tmux client.
///
/// Not cached: the active client changes between grouped sessions, because the
/// user may be viewing forestui from any terminal.
pub fn current_session() -> Option<String> {
    if !is_inside_tmux() {
        return None;
    }

    // Only consider clients attached to sessions in our own group.
    let our_group = tmux(&["display-message", "-p", "#{session_group}"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if let Some(out) = tmux(&[
        "list-clients",
        "-F",
        "#{client_activity} #{session_id} #{session_group}",
    ]) {
        let mut best_id: Option<String> = None;
        let mut best_time: i64 = -1;
        for line in out.lines() {
            let parts: Vec<&str> = line.trim().splitn(3, ' ').collect();
            if parts.len() != 3 {
                continue;
            }
            let Ok(activity) = parts[0].parse::<i64>() else {
                continue;
            };
            if !our_group.is_empty() && parts[2] != our_group {
                continue;
            }
            if activity > best_time {
                best_time = activity;
                best_id = Some(parts[1].to_string());
            }
        }
        if best_id.is_some() {
            return best_id;
        }
    }

    // Fallback: first attached session, then any session.
    if let Some(out) = tmux(&["list-sessions", "-F", "#{session_id} #{session_attached}"]) {
        let mut first: Option<String> = None;
        for line in out.lines() {
            let mut parts = line.trim().split(' ');
            let (Some(id), Some(attached)) = (parts.next(), parts.next()) else {
                continue;
            };
            if first.is_none() {
                first = Some(id.to_string());
            }
            if attached.parse::<i64>().unwrap_or(0) > 0 {
                return Some(id.to_string());
            }
        }
        return first;
    }
    None
}

/// The tmux window this process is running in.
///
/// Resolved from `TMUX_PANE` rather than the session's active window, which may
/// be a different window while forestui is starting up in the background.
pub fn current_window() -> Option<String> {
    if !is_inside_tmux() {
        return None;
    }
    let pane = std::env::var("TMUX_PANE").ok()?;
    tmux(&["display-message", "-p", "-t", &pane, "#{window_id}"]).map(|s| s.trim().to_string())
}

/// Rename the window forestui itself runs in.
pub fn rename_window(name: &str) -> bool {
    let Some(window) = current_window() else {
        return false;
    };
    tmux(&["rename-window", "-t", &window, name]).is_some()
}

/// Enable the tmux `focus-events` option so the app can refresh on focus.
pub fn ensure_focus_events() -> bool {
    if !is_inside_tmux() {
        return false;
    }
    tmux(&["set-option", "-g", "focus-events", "on"]).is_some()
}

fn window_names() -> Vec<String> {
    let Some(session) = current_session() else {
        return Vec::new();
    };
    tmux(&["list-windows", "-t", &session, "-F", "#{window_name}"])
        .map(|out| out.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default()
}

/// Find a window by name in the current session.
pub fn find_window(name: &str) -> Option<String> {
    let session = current_session()?;
    let out = tmux(&[
        "list-windows",
        "-t",
        &session,
        "-F",
        "#{window_id} #{window_name}",
    ])?;
    for line in out.lines() {
        if let Some((id, wname)) = line.trim().split_once(' ')
            && wname == name
        {
            return Some(id.to_string());
        }
    }
    None
}

/// Append `:2`, `:3`, … until the window name is unused.
pub fn find_unique_window_name(base_name: &str) -> String {
    unique_name_among(base_name, &window_names())
}

/// Pure form of [`find_unique_window_name`], for testing.
pub fn unique_name_among(base_name: &str, existing: &[String]) -> String {
    if !existing.iter().any(|n| n == base_name) {
        return base_name.to_string();
    }
    let mut counter = 2;
    loop {
        let candidate = format!("{base_name}:{counter}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// Create a window and select it. `shell_command` runs instead of a shell, so
/// the window closes when the command exits.
fn new_window(name: &str, path: &str, shell_command: Option<&str>) -> bool {
    let Some(session) = current_session() else {
        return false;
    };
    let mut args: Vec<&str> = vec!["new-window", "-t", &session, "-n", name, "-c", path];
    if let Some(cmd) = shell_command {
        args.push(cmd);
    }
    tmux(&args).is_some()
}

/// Open the editor in a tmux window named `edit:<worktree_name>`, reusing an
/// existing window with that name when one is already open.
pub fn create_editor_window(worktree_name: &str, worktree_path: &str, editor: &str) -> bool {
    if current_session().is_none() {
        return false;
    }
    let window_name = format!("edit:{worktree_name}");

    if let Some(existing) = find_window(&window_name) {
        return tmux(&["select-window", "-t", &existing]).is_some();
    }
    new_window(&window_name, worktree_path, Some(&format!("{editor} .")))
}

/// Create a shell window named `term:<name>` (always a new window).
pub fn create_shell_window(name: &str, path: &str) -> bool {
    if current_session().is_none() {
        return false;
    }
    let window_name = find_unique_window_name(&format!("term:{name}"));
    new_window(&window_name, path, None)
}

/// Create a Midnight Commander window named `files:<name>` (always new).
pub fn create_mc_window(name: &str, path: &str) -> bool {
    if current_session().is_none() {
        return false;
    }
    let window_name = find_unique_window_name(&format!("files:{name}"));
    new_window(&window_name, path, Some("mc"))
}

/// Build the shell command used for a Claude window.
///
/// Wrapped in an interactive shell so user aliases resolve; the inner command
/// is quoted to keep custom commands from breaking out.
pub fn claude_shell_command(
    resume_session_id: Option<&str>,
    yolo: bool,
    custom_command: Option<&str>,
    custom_prefix: Option<&str>,
    shell: &str,
) -> String {
    let mut cmd = custom_command.unwrap_or("claude").to_string();
    // The YOLO flag belongs to the built-in YOLO button only, never to a custom one.
    if yolo && custom_prefix.is_none() {
        cmd.push_str(" --dangerously-skip-permissions");
    }
    if let Some(id) = resume_session_id {
        cmd.push_str(&format!(" -r {id}"));
    }
    let quoted = shlex::try_quote(&cmd)
        .map(|c| c.into_owned())
        .unwrap_or(cmd);
    format!("{shell} -ic {quoted}")
}

/// Window name for a Claude session, before uniquifying.
pub fn claude_base_window_name(name: &str, yolo: bool, custom_prefix: Option<&str>) -> String {
    match (custom_prefix, yolo) {
        (Some(prefix), _) => format!("{prefix}:{name}"),
        (None, true) => format!("yolo:{name}"),
        (None, false) => format!("claude:{name}"),
    }
}

/// Create a tmux window running Claude Code. Returns the window name.
pub fn create_claude_window(
    name: &str,
    path: &str,
    resume_session_id: Option<&str>,
    yolo: bool,
    custom_command: Option<&str>,
    custom_prefix: Option<&str>,
) -> Option<String> {
    current_session()?;

    let base = claude_base_window_name(name, yolo, custom_prefix);
    let window_name = find_unique_window_name(&base);

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let shell_cmd = claude_shell_command(
        resume_session_id,
        yolo,
        custom_command,
        custom_prefix,
        &shell,
    );

    if new_window(&window_name, path, Some(&shell_cmd)) {
        Some(window_name)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_editor_detection() {
        assert!(is_tui_editor("vim"));
        assert!(is_tui_editor("emacs -nw"));
        assert!(!is_tui_editor("code"));
        assert!(!is_tui_editor(""));
    }

    #[test]
    fn unique_window_names() {
        let existing = vec!["term:a".to_string(), "term:a:2".to_string()];
        assert_eq!(unique_name_among("term:b", &existing), "term:b");
        assert_eq!(unique_name_among("term:a", &existing), "term:a:3");
    }

    #[test]
    fn claude_window_naming() {
        assert_eq!(
            claude_base_window_name("repo:wt", false, None),
            "claude:repo:wt"
        );
        assert_eq!(
            claude_base_window_name("repo:wt", true, None),
            "yolo:repo:wt"
        );
        assert_eq!(
            claude_base_window_name("repo:wt", true, Some("opus")),
            "opus:repo:wt"
        );
    }

    #[test]
    fn claude_command_building() {
        assert_eq!(
            claude_shell_command(None, false, None, None, "/bin/zsh"),
            "/bin/zsh -ic claude"
        );
        assert_eq!(
            claude_shell_command(None, true, None, None, "/bin/zsh"),
            "/bin/zsh -ic 'claude --dangerously-skip-permissions'"
        );
        assert_eq!(
            claude_shell_command(Some("abc123"), false, None, None, "/bin/zsh"),
            "/bin/zsh -ic 'claude -r abc123'"
        );
        // A custom button never gets the YOLO flag appended.
        assert_eq!(
            claude_shell_command(
                None,
                true,
                Some("claude --model opus"),
                Some("opus"),
                "/bin/zsh"
            ),
            "/bin/zsh -ic 'claude --model opus'"
        );
    }
}
