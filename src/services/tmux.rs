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

/// The tmux session forestui creates its windows in.
///
/// Resolved from `TMUX_PANE`, which names this process's own pane and so is
/// exact. This used to ask `list-clients` for the most recently active client
/// on the server, filtered by session group — but a session `ensure_tmux`
/// creates on first launch is not in a group at all (the grouped session is
/// only made when the base session already existed), so `session_group` came
/// back empty, the filter went inert, and the scan returned whichever session
/// the user had touched last. A second forestui, opened on a different forest,
/// therefore created its windows in the first one's session.
///
/// Sessions in a group genuinely share their windows, so there any member is a
/// correct target; the activity scan still runs in that case, so the window
/// opens in whichever terminal the user is actually looking at.
///
/// There is deliberately no fallback for a missing `TMUX_PANE`. The previous
/// implementation ended in "first attached session, else any session", and
/// guessing is what put windows in the wrong forestui: a caller that cannot
/// identify its own pane cannot identify its own session either, and failing
/// the action visibly beats acting on somebody else's session.
pub fn current_session() -> Option<String> {
    if !is_inside_tmux() {
        return None;
    }

    let pane = std::env::var("TMUX_PANE").ok()?;
    let own = tmux(&["display-message", "-p", "-t", &pane, "#{session_id}"])?
        .trim()
        .to_string();
    if own.is_empty() {
        return None;
    }

    let group = tmux(&["display-message", "-p", "-t", &pane, "#{session_group}"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if group.is_empty() {
        return Some(own);
    }

    let clients = tmux(&[
        "list-clients",
        "-F",
        "#{client_activity} #{session_id} #{session_group}",
    ])
    .unwrap_or_default();
    Some(most_active_in_group(&clients, &group, &own))
}

/// Of the clients attached to our own session group, the session of the most
/// recently active one — falling back to our own session. Pure, for testing.
fn most_active_in_group(clients: &str, group: &str, own: &str) -> String {
    let mut best = own.to_string();
    let mut best_activity = i64::MIN;
    for line in clients.lines() {
        let parts: Vec<&str> = line.trim().splitn(3, ' ').collect();
        if parts.len() != 3 || parts[2] != group {
            continue;
        }
        let Ok(activity) = parts[0].parse::<i64>() else {
            continue;
        };
        if activity > best_activity {
            best_activity = activity;
            best = parts[1].to_string();
        }
    }
    best
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

/// Record the name a window was born with.
///
/// The title-sync hook reads this to answer one question: did forestui open
/// this window? A window the user made by hand carries no stamp and is left
/// alone. Stamped whether or not the plugin is installed: it is one string on a
/// window that tmux discards when the window closes, and writing it eagerly
/// means installing the plugin later also works for windows already open.
pub fn stamp_birth_name(window_id: &str, window_name: &str) -> bool {
    tmux(&[
        "set-option",
        "-w",
        "-t",
        window_id,
        "@claude_birth_name",
        window_name,
    ])
    .is_some()
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
/// Create a window and return its id.
///
/// The id, not the name: a name is only unique at the moment it is chosen, and
/// the title-sync hook renames windows behind us, so anything that acts on the
/// window afterwards has to hold something that cannot come to mean a
/// different window.
fn new_window(name: &str, path: &str, shell_command: Option<&str>) -> Option<String> {
    let session = current_session()?;
    let mut args: Vec<&str> = vec![
        "new-window",
        "-t",
        &session,
        "-n",
        name,
        "-c",
        path,
        "-P",
        "-F",
        "#{window_id}",
    ];
    if let Some(cmd) = shell_command {
        args.push(cmd);
    }
    let id = tmux(&args)?.trim().to_string();
    (!id.is_empty()).then_some(id)
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
    new_window(&window_name, worktree_path, Some(&format!("{editor} ."))).is_some()
}

/// Create a shell window named `term:<name>` (always a new window).
pub fn create_shell_window(name: &str, path: &str) -> bool {
    if current_session().is_none() {
        return false;
    }
    let window_name = find_unique_window_name(&format!("term:{name}"));
    new_window(&window_name, path, None).is_some()
}

/// Create a Midnight Commander window named `files:<name>` (always new).
pub fn create_mc_window(name: &str, path: &str) -> bool {
    if current_session().is_none() {
        return false;
    }
    let window_name = find_unique_window_name(&format!("files:{name}"));
    new_window(&window_name, path, Some("mc")).is_some()
}

/// Build the shell command used for a Claude window.
///
/// Wrapped in an interactive shell so user aliases resolve; the inner command
/// is quoted to keep custom commands from breaking out.
/// A string the shell will read back verbatim, whatever is in it.
///
/// Single quotes are the only shell quoting with no escapes inside them: no
/// `$`, no backtick, and — since these commands run through `-ic`, an
/// *interactive* shell — no `!` history expansion either, which double quotes
/// would not stop in zsh. This matters because window names now come from
/// session titles, and a session title can be set by a `SessionStart` hook in a
/// repository's own `.claude/settings.json`, so it is not the user's own text
/// by the time it reaches here.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub fn claude_shell_command(
    resume_session_id: Option<&str>,
    yolo: bool,
    custom_command: Option<&str>,
    custom_prefix: Option<&str>,
    shell: &str,
    window_name: &str,
) -> String {
    let mut cmd = custom_command.unwrap_or("claude").to_string();
    // The YOLO flag belongs to the built-in YOLO button only, never to a custom one.
    if yolo && custom_prefix.is_none() {
        cmd.push_str(" --dangerously-skip-permissions");
    }
    if let Some(id) = resume_session_id {
        // No -n here on purpose. A resumed session already carries the name it
        // was given, and passing one would overwrite it — with the *window's*
        // name, which may have picked up a `:2` from uniquifying against a
        // window still open for the same conversation. The hook adopts the
        // stored name onto the window instead.
        cmd.push_str(&format!(" -r {id}"));
    } else {
        // Claude renders this in the prompt box from the first frame, which no
        // hook can do: a title set before the session's UI is live is stored
        // but never drawn.
        cmd.push_str(&format!(" -n {}", sh_quote(window_name)));
    }
    // The birth stamp is written from inside the window, before the command
    // that needs it runs. Stamping from forestui after new-window returns is a
    // separate tmux call, so a fast-starting Claude could reach its first hook
    // first and be read as a window forestui never opened. Ordering it here
    // makes that impossible rather than merely unlikely; a failure to stamp
    // still falls through to the command.
    let cmd = format!(
        "tmux set-option -w @claude_birth_name {}; {cmd}",
        sh_quote(window_name)
    );
    format!("{shell} -ic {}", sh_quote(&cmd))
}

/// Window name for a Claude session, before uniquifying.
pub fn claude_base_window_name(name: &str, yolo: bool, custom_prefix: Option<&str>) -> String {
    match (custom_prefix, yolo) {
        (Some(prefix), _) => format!("{prefix}:{name}"),
        (None, true) => format!("yolo:{name}"),
        (None, false) => format!("claude:{name}"),
    }
}

/// The name a Claude window opens with.
///
/// A resumed session keeps its own name verbatim: the window name and the
/// session name are one string, so re-applying a prefix here would accrete
/// (`claude:yolo:thing`) a little more on every resume. A fresh session has no
/// name yet, so it gets forestui's opening name instead.
pub fn opening_window_name(
    seed: Option<&str>,
    worktree_name: &str,
    yolo: bool,
    custom_prefix: Option<&str>,
) -> String {
    match seed {
        Some(name) => name.to_string(),
        None => claude_base_window_name(worktree_name, yolo, custom_prefix),
    }
}

/// Create a tmux window running Claude Code. Returns the window name.
pub fn create_claude_window(
    base_name: &str,
    path: &str,
    resume_session_id: Option<&str>,
    yolo: bool,
    custom_command: Option<&str>,
    custom_prefix: Option<&str>,
) -> Option<String> {
    current_session()?;

    let window_name = find_unique_window_name(base_name);

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let shell_cmd = claude_shell_command(
        resume_session_id,
        yolo,
        custom_command,
        custom_prefix,
        &shell,
        &window_name,
    );

    let id = new_window(&window_name, path, Some(&shell_cmd))?;
    // Belt and braces: the prelude above is what orders the stamp correctly,
    // this covers a shell that never ran it. Same value either way.
    stamp_birth_name(&id, &window_name);
    Some(window_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this replaced: a forestui opened on a second forest created its
    /// windows in the *first* forestui's session, because ungrouped sessions
    /// report an empty group and the scan then ranged over the whole server,
    /// returning whichever terminal the user had touched most recently.
    #[test]
    fn a_client_outside_our_group_never_wins() {
        let clients = "100 $1 ours\n900 $2 someone-else\n300 $3 ours\n";
        assert_eq!(most_active_in_group(clients, "ours", "$9"), "$3");
    }

    /// An ungrouped session short-circuits before this is reached, but a group
    /// whose clients have all detached must still answer with our own session
    /// rather than someone else's.
    #[test]
    fn our_own_session_is_the_fallback() {
        assert_eq!(most_active_in_group("900 $2 other\n", "ours", "$7"), "$7");
        assert_eq!(most_active_in_group("", "ours", "$7"), "$7");
        assert_eq!(most_active_in_group("garbage\n", "ours", "$7"), "$7");
    }

    /// Within our own group the windows are shared, so the most recently active
    /// client is the right one: the window opens where the user is looking.
    #[test]
    fn the_most_recently_active_client_in_our_group_wins() {
        let clients = "100 $1 ours\n500 $2 ours\n300 $3 ours\n";
        assert_eq!(most_active_in_group(clients, "ours", "$1"), "$2");
    }

    /// The window name and the session name are one string. Resuming must not
    /// re-apply a prefix, or a session resumed a few times ends up called
    /// `claude:claude:claude:thing`.
    #[test]
    fn a_resumed_session_keeps_its_name_verbatim() {
        assert_eq!(
            opening_window_name(Some("yolo:forestuiNAMESYNC"), "repo:wt", false, None),
            "yolo:forestuiNAMESYNC"
        );
        assert_eq!(
            opening_window_name(Some("retry loop"), "repo:wt", true, Some("opus")),
            "retry loop"
        );

        // Resuming the same session again is a fixed point.
        let once = opening_window_name(Some("claude:demo:wt"), "demo:wt", false, None);
        let twice = opening_window_name(Some(&once), "demo:wt", false, None);
        assert_eq!(once, twice);

        // A session with no name of its own still gets forestui's opening name.
        assert_eq!(
            opening_window_name(None, "repo:wt", true, None),
            "yolo:repo:wt"
        );
        assert_eq!(
            opening_window_name(None, "repo:wt", false, Some("opus")),
            "opus:repo:wt"
        );
    }

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

    /// What the shell actually receives, with both layers of quoting undone.
    fn decoded(built: &str, shell: &str) -> String {
        let payload = built
            .strip_prefix(&format!("{shell} -ic "))
            .unwrap_or_else(|| panic!("unexpected shape: {built}"));
        let mut parts = shlex::split(payload).expect("a single quoted argument");
        assert_eq!(parts.len(), 1, "not one argument: {payload}");
        parts.remove(0)
    }

    #[test]
    fn claude_command_building() {
        let stamp = "tmux set-option -w @claude_birth_name 'claude:wt'";
        let build = |resume, yolo, cmd, prefix| {
            decoded(
                &claude_shell_command(resume, yolo, cmd, prefix, "/bin/zsh", "claude:wt"),
                "/bin/zsh",
            )
        };

        // A fresh session is named at launch; a resumed one keeps its own name.
        assert_eq!(
            build(None, false, None, None),
            format!("{stamp}; claude -n 'claude:wt'")
        );
        assert_eq!(
            build(None, true, None, None),
            format!("{stamp}; claude --dangerously-skip-permissions -n 'claude:wt'")
        );
        assert_eq!(
            build(Some("abc123"), false, None, None),
            format!("{stamp}; claude -r abc123")
        );
        // A custom button never gets the YOLO flag appended.
        assert_eq!(
            build(None, true, Some("claude --model opus"), Some("opus")),
            format!("{stamp}; claude --model opus -n 'claude:wt'")
        );
    }

    /// Window names come from session titles, and a session title can be set by
    /// a `SessionStart` hook living in a repository's own `.claude/settings.json`
    /// — so cloning a repository must not be able to run code here. Proven by
    /// handing each name to a real shell and reading back what it made of it.
    #[test]
    fn a_hostile_window_name_stays_data() {
        for hostile in [
            "$(touch /tmp/forestui-pwned)",
            "`touch /tmp/forestui-pwned`",
            "'; touch /tmp/forestui-pwned; '",
            "it's",
            "a!b",
            "back\\slash",
            "claude:wt",
        ] {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf %s {}", sh_quote(hostile)))
                .output()
                .expect("the shell runs");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                hostile,
                "{hostile:?} did not survive as literal text"
            );
        }
        assert!(
            !std::path::Path::new("/tmp/forestui-pwned").exists(),
            "a window name executed a command"
        );

        // And the name is still literal after the second layer of quoting.
        let built = claude_shell_command(None, false, None, None, "/bin/sh", "$(id); rm -rf /");
        assert_eq!(
            decoded(&built, "/bin/sh"),
            "tmux set-option -w @claude_birth_name '$(id); rm -rf /'; \
             claude -n '$(id); rm -rf /'"
                .replace("\\\n             ", "")
        );
    }
}
