//! tmux integration.
//!
//! Every call shells out to the `tmux` binary. The commands are short-lived and
//! synchronous — the same blocking behaviour the libtmux-based build had.

use std::path::{Path, PathBuf};
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

/// Rename an arbitrary window by id.
///
/// This is how a *live* session is renamed: the title-sync plugin sees the tab
/// move and adopts the new name into the session on its next hook, exactly as
/// if the user had renamed the tab by hand. Nothing touches the transcript —
/// Claude holds it open, and the plugin path is the one that already works.
pub fn rename_window_by_id(window_id: &str, name: &str) -> bool {
    tmux(&["rename-window", "-t", window_id, name]).is_some()
}

/// Jump to a window, for the duplicate-open guard's "switch there" choice.
pub fn select_window(window_id: &str) -> bool {
    tmux(&["select-window", "-t", window_id]).is_some()
}

/// What `ensure_focus_events` found, and therefore what exit has to undo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEvents {
    /// Not inside tmux, or tmux refused the option.
    Unavailable,
    /// Someone else's setting — leave it exactly as it is.
    AlreadyOn,
    /// Ours to undo, either because we just turned it on or because a previous
    /// forestui did and died before it could.
    Ours,
}

/// Marks `focus-events` as forestui's doing, so the option can be handed back
/// even when the run that set it never reached its cleanup.
///
/// Without it, one panic or `SIGTERM` would leave the option on forever: every
/// later run would find it already on, conclude it was the user's, and refuse
/// to touch it. A tmux user option is the natural place for the flag — it
/// lives exactly as long as the server whose setting it describes.
const FOCUS_EVENTS_MARKER: &str = "@forestui_focus_events";

/// Enable the tmux `focus-events` option so the app can refresh on focus.
///
/// It is a **server** option: turning it on reaches every session, window and
/// client on that tmux server, and nothing turns it off again on its own. That
/// is not free — while it is on, tmux asks the terminal for focus reporting,
/// and a focus report that arrives between the prefix key and the next key
/// cancels the tmux command (issue #51). So this reports whether the option is
/// ours to undo, and [`disable_focus_events`] hands it back.
pub fn ensure_focus_events() -> FocusEvents {
    if !is_inside_tmux() {
        return FocusEvents::Unavailable;
    }

    if tmux(&["show-options", "-gv", "focus-events"]).is_some_and(|v| v.trim() == "on") {
        return if marked_as_ours() {
            FocusEvents::Ours
        } else {
            FocusEvents::AlreadyOn
        };
    }

    if tmux(&["set-option", "-g", "focus-events", "on"]).is_some() {
        let _ = tmux(&["set-option", "-g", FOCUS_EVENTS_MARKER, "on"]);
        FocusEvents::Ours
    } else {
        FocusEvents::Unavailable
    }
}

/// Put `focus-events` back to off, for the run that owns it.
///
/// Only called for [`FocusEvents::Ours`], so a user who set the option
/// themselves keeps it. Two forestui instances can still disagree — the first
/// to exit turns it off under the second, which costs that instance its
/// refresh-on-return until it restarts. That is the cheaper of the two
/// mistakes: the other one is changing the user's server permanently and never
/// telling them.
pub fn disable_focus_events() {
    if !is_inside_tmux() {
        return;
    }
    let _ = tmux(&["set-option", "-g", "focus-events", "off"]);
    let _ = tmux(&["set-option", "-gu", FOCUS_EVENTS_MARKER]);
}

/// Hand back a `focus-events` a previous forestui stranded, for a run that
/// wants nothing to do with it.
///
/// `--no-focus-events` is what someone reaches for when focus reporting is
/// costing them a tmux prefix, so it has to clear the setting a crashed run
/// left behind rather than only declining to add one.
pub fn release_stranded_focus_events() {
    if is_inside_tmux() && marked_as_ours() {
        disable_focus_events();
    }
}

/// Whether a forestui — this one or one that died before cleaning up — is what
/// turned `focus-events` on.
fn marked_as_ours() -> bool {
    // An unset user option is an error, not an empty value, so a failed call
    // is the "not ours" answer rather than something to report.
    tmux(&["show-options", "-gv", FOCUS_EVENTS_MARKER]).is_some_and(|v| v.trim() == "on")
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

/// Record which Claude session a window holds.
///
/// Written for every Claude window forestui opens — fresh sessions carry the
/// pre-minted `--session-id`, resumed ones the id being resumed — so window ↔
/// session becomes a recorded fact instead of a guess from mtimes. Everything
/// the live badge, the duplicate-open guard and the pane peek know starts here.
pub fn stamp_session_id(window_id: &str, session_id: &str) -> bool {
    tmux(&[
        "set-option",
        "-w",
        "-t",
        window_id,
        "@claude_session_id",
        session_id,
    ])
    .is_some()
}

/// A window forestui opened for Claude, as the live scan sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeWindow {
    pub window_id: String,
    pub window_name: String,
    /// Whether the pane's foreground process is still Claude (or anything that
    /// is not a shell). `false` means the window is open but Claude exited or
    /// was suspended — the shell prompt is what is running.
    pub running: bool,
    pub session_id: String,
}

/// Shells a Claude window can drop back to. A pane whose foreground command is
/// one of these holds no running Claude — see [`ClaudeWindow::running`].
const SHELLS: [&str; 13] = [
    "zsh", "bash", "sh", "dash", "ash", "ksh", "ksh93", "mksh", "loksh", "yash", "fish", "nu",
    "nushell",
];

/// Every window on our session that carries a `@claude_session_id` stamp.
///
/// One tmux round trip for the whole session, not one per window: this runs on
/// the same cadence as the session refresh and has to stay that cheap.
pub fn list_claude_windows() -> Vec<ClaudeWindow> {
    let Some(session) = current_session() else {
        return Vec::new();
    };
    let Some(out) = tmux(&[
        "list-windows",
        "-t",
        &session,
        "-F",
        "#{window_id}\t#{pane_current_command}\t#{@claude_session_id}\t#{window_name}",
    ]) else {
        return Vec::new();
    };
    parse_claude_windows(&out)
}

/// Pure form of [`list_claude_windows`], for testing.
fn parse_claude_windows(out: &str) -> Vec<ClaudeWindow> {
    let mut windows = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(4, '\t');
        let (Some(id), Some(command), Some(session_id), Some(name)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // Windows without the stamp are not Claude windows forestui opened.
        if session_id.is_empty() {
            continue;
        }
        windows.push(ClaudeWindow {
            window_id: id.to_string(),
            window_name: name.to_string(),
            running: !SHELLS.contains(&command),
            session_id: session_id.to_string(),
        });
    }
    windows
}

/// One coherent view of the live Claude windows: the windows themselves plus
/// a pane tail for each *running* one. The scan sweep and any flow that just
/// changed a window (a live rename) both send this, so the map on screen
/// never waits for the next sweep to catch up with an action the user watched
/// happen.
pub fn live_snapshot(
    peek_lines: usize,
) -> (
    Vec<ClaudeWindow>,
    std::collections::HashMap<String, Vec<String>>,
) {
    let windows = list_claude_windows();
    let peeks = windows
        .iter()
        .filter(|window| window.running)
        .map(|window| {
            (
                window.session_id.clone(),
                capture_pane_tail(&window.window_id, peek_lines),
            )
        })
        .collect();
    (windows, peeks)
}

/// The last `lines` non-blank rows of a window's pane, for the live peek.
pub fn capture_pane_tail(window_id: &str, lines: usize) -> Vec<String> {
    let Some(out) = tmux(&["capture-pane", "-p", "-t", window_id]) else {
        return Vec::new();
    };
    let kept: Vec<String> = out
        .lines()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    let start = kept.len().saturating_sub(lines);
    kept[start..].to_vec()
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

/// A string the shell will read back verbatim, whatever is in it.
///
/// Single quotes are the only shell quoting with no escapes inside them: no
/// `$`, no backtick, and — since this is typed into the window's *interactive*
/// shell — no `!` history expansion either, which double quotes would not stop
/// in zsh. This matters because window names now come from session titles, and
/// a session title can be set by a `SessionStart` hook in a repository's own
/// `.claude/settings.json`, so it is not the user's own text by the time it
/// reaches here.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Everything that is not text, removed.
///
/// The line ends up as one line of a file the shell reads, so a control
/// character in it is not a character: a newline inside a window name would
/// end the line and leave the rest standing as a command of its own. Names
/// arrive from session titles, which are not the user's own text, so they are
/// stripped here as well as by the hook that adopts them.
fn single_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// The command line a Claude window runs.
///
/// It runs as a job of the window's own interactive shell rather than as the
/// window's command, and that is the point. A window whose only process is
/// Claude has nothing underneath it: Ctrl-C leaves a dead pane, and Claude's
/// own Ctrl-Z suspend stops the process with no shell left to `fg` it back —
/// the session is unreachable and the window is closed to be rid of it.
/// Running Claude as a job makes both land at a prompt, where `fg` resumes and
/// the up arrow restarts.
pub fn claude_command_line(
    resume_session_id: Option<&str>,
    yolo: bool,
    custom_command: Option<&str>,
    custom_prefix: Option<&str>,
    window_name: &str,
    new_session_id: Option<&str>,
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
        //
        // Quoted like the window name: a resume id is a transcript's filename
        // stem, which forestui read off disk rather than wrote.
        cmd.push_str(&format!(" -r {}", sh_quote(id)));
    } else {
        // Claude renders this in the prompt box from the first frame, which no
        // hook can do: a title set before the session's UI is live is stored
        // but never drawn.
        cmd.push_str(&format!(" -n {}", sh_quote(window_name)));
        // The pre-minted id, so the window knows which session it holds from
        // birth instead of forestui guessing later from transcript mtimes.
        if let Some(id) = new_session_id {
            cmd.push_str(&format!(" --session-id {}", sh_quote(id)));
        }
    }
    single_line(&cmd)
}

/// The tail every generated startup file ends with.
///
/// `set -m` is the load-bearing line. Without it a command run from a startup
/// file is not a job: bash hands it the shell's own ignored `SIGTSTP`, so
/// Ctrl-Z does nothing at all and — because the shell is not waiting for a
/// stoppable child either — the window wedges with no way back. With it, the
/// command is a job in its own process group, which is what makes Ctrl-Z leave
/// a stopped job and a prompt, and `fg` bring it back. zsh already runs
/// interactive shells this way; saying so costs nothing and makes the file
/// mean the same thing everywhere.
///
/// The birth stamp is written here, from inside the window, for the reason it
/// always was: the shell starts Claude on its own, so a stamp forestui sends
/// after `new-window` returns is a separate tmux call that a fast-starting
/// Claude can beat to its first hook.
fn startup_tail(window_name: &str, session_id: &str, line: &str) -> String {
    // The session-id stamp rides along for the same reason as the birth name:
    // written from inside the window, ahead of the command, it cannot lose a
    // race to anything reading window options once Claude is up.
    format!(
        "tmux set-option -w @claude_birth_name {} >/dev/null 2>&1\n\
         tmux set-option -w @claude_session_id {} >/dev/null 2>&1\n\
         set -m\n{line}\n",
        sh_quote(window_name),
        sh_quote(session_id)
    )
}

/// The user's own startup files, as the generated ones have to name them.
struct ShellHome {
    home: String,
    zdotdir: Option<String>,
    env_file: Option<String>,
}

impl ShellHome {
    fn from_env() -> Self {
        Self {
            home: std::env::var("HOME").unwrap_or_default(),
            zdotdir: std::env::var("ZDOTDIR").ok().filter(|v| !v.is_empty()),
            env_file: std::env::var("ENV").ok().filter(|v| !v.is_empty()),
        }
    }
}

/// A shell that has been pointed at a startup file forestui wrote.
struct Startup {
    /// Files to write before the window is created, in order.
    files: Vec<(PathBuf, String)>,
    /// The command tmux runs for the window.
    command: String,
}

/// Point `shell` at a startup file that runs `line`, or `None` if this shell
/// has no hook we know.
///
/// Every one of these hooks *replaces* one of the user's own files rather than
/// adding to it — a new `ZDOTDIR` hides `.zshenv` and `.zshrc`, `--rcfile`
/// stands in for `.bashrc`, `ENV` for whatever `ENV` already named — so each
/// generated file sources the file it displaced. What the window ends up with
/// is the environment `$SHELL -ic` used to give it: the same interactive,
/// non-login startup, with the command running as a job at the end of it.
///
/// The generated files delete themselves as soon as the shell has read what it
/// needs, so a window that is never closed does not leave one behind.
fn startup_for(
    shell: &str,
    dir: &Path,
    home: &ShellHome,
    window_name: &str,
    session_id: &str,
    line: &str,
    remembered_line: &str,
) -> Option<Startup> {
    let base = Path::new(shell).file_name()?.to_str()?;
    let dir_str = dir.to_str()?;
    // Typing the command used to put it in the shell's history for free, and
    // that is worth keeping: after Ctrl-C the up arrow is the shortest way
    // back into a session. Only the shells with a builtin for it get one.
    //
    // What is remembered is not always what ran: a fresh session launches
    // with a pre-minted `--session-id`, and Claude refuses that flag once the
    // session exists ("Session ID is already in use") — which is exactly the
    // state an up-arrow rerun happens in. The caller hands the resume form
    // instead, so the up arrow goes back into the same conversation rather
    // than into an error.
    let remembered = match base {
        "zsh" => format!("print -s -- {}\n", sh_quote(remembered_line)),
        "bash" => format!("history -s {}\n", sh_quote(remembered_line)),
        _ => String::new(),
    };
    let tail = format!(
        "{remembered}{}",
        startup_tail(window_name, session_id, line)
    );
    let tail = tail.as_str();
    let sweep = format!("rm -rf {}\n", sh_quote(dir_str));
    let source = |path: String| {
        let quoted = sh_quote(&path);
        format!("[ -f {quoted} ] && . {quoted}\n")
    };

    match base {
        "zsh" => {
            // `.zshenv` is read for every shell and `.zshrc` for interactive
            // ones, so both are hidden by the new ZDOTDIR and both come back
            // here. ZDOTDIR itself is restored before the user's `.zshrc` runs
            // — a configuration that keys a cache or a plugin directory off it
            // must not see forestui's temporary one.
            let zdot = home.zdotdir.clone().unwrap_or_else(|| home.home.clone());
            let files = vec![
                (dir.join(".zshenv"), source(format!("{zdot}/.zshenv"))),
                (
                    dir.join(".zshrc"),
                    format!(
                        "ZDOTDIR={}\n{sweep}{}{tail}",
                        sh_quote(&zdot),
                        source(format!("{zdot}/.zshrc"))
                    ),
                ),
            ];
            Some(Startup {
                files,
                command: format!("env ZDOTDIR={} {} -i", sh_quote(dir_str), sh_quote(shell)),
            })
        }
        "bash" => {
            let rc = dir.join("rc");
            let rc_str = rc.to_str()?.to_string();
            Some(Startup {
                files: vec![(
                    rc,
                    format!("{sweep}{}{tail}", source(format!("{}/.bashrc", home.home))),
                )],
                command: format!("{} --rcfile {} -i", sh_quote(shell), sh_quote(&rc_str)),
            })
        }
        // The POSIX interactive startup file, which is whatever `ENV` names.
        "sh" | "dash" | "ash" | "ksh" | "ksh93" | "mksh" | "loksh" | "yash" => {
            let rc = dir.join("rc");
            let rc_str = rc.to_str()?.to_string();
            let inherited = home.env_file.clone().map(source).unwrap_or_default();
            Some(Startup {
                files: vec![(rc, format!("{sweep}{inherited}{tail}"))],
                command: format!("env ENV={} {} -i", sh_quote(&rc_str), sh_quote(shell)),
            })
        }
        // fish, nushell and friends: no hook here that has been tested, so the
        // line is typed into a plain shell instead. Nothing is lost but the
        // typing.
        _ => None,
    }
}

/// Write the startup files for this launch, or `None` to fall back to typing.
fn prepare_startup(
    window_name: &str,
    session_id: &str,
    line: &str,
    remembered_line: &str,
) -> Option<Startup> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty())?;
    let dir = std::env::temp_dir().join(format!(
        "forestui-launch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let startup = startup_for(
        &shell,
        &dir,
        &ShellHome::from_env(),
        window_name,
        session_id,
        line,
        remembered_line,
    )?;

    // A launch that cannot write its files is not an error to show anyone:
    // typing the line still works, so fall back to it.
    std::fs::create_dir_all(&dir).ok()?;
    for (path, contents) in &startup.files {
        std::fs::write(path, contents).ok()?;
    }
    Some(startup)
}

/// Type a line into a window's shell and run it.
///
/// The fallback for a shell with no startup hook of its own. Two calls, because
/// `-l` sends its arguments as literal characters — the only way to keep a
/// command line from being read as key names — and `Enter` is precisely a key
/// name.
fn send_line(window_id: &str, line: &str) -> bool {
    tmux(&["send-keys", "-t", window_id, "-l", "--", line]).is_some()
        && tmux(&["send-keys", "-t", window_id, "Enter"]).is_some()
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

    // The window knows its session from birth, both ways round: a fresh
    // session runs under an id forestui minted and passed as `--session-id`,
    // a resumed one under the id being resumed. Either way the stamp makes
    // "which window holds this conversation" a lookup, not a heuristic.
    let minted;
    let session_id = match resume_session_id {
        Some(id) => id,
        None => {
            minted = uuid::Uuid::new_v4().to_string();
            &minted
        }
    };

    let window_name = find_unique_window_name(base_name);
    let line = claude_command_line(
        resume_session_id,
        yolo,
        custom_command,
        custom_prefix,
        &window_name,
        resume_session_id.is_none().then_some(session_id),
    );
    // The history entry: for a resume it is the line itself, for a fresh
    // session the resume form of the same id — see `startup_for`.
    let remembered = claude_command_line(
        Some(session_id),
        yolo,
        custom_command,
        custom_prefix,
        &window_name,
        None,
    );

    match prepare_startup(&window_name, session_id, &line, &remembered) {
        // The window's command is the user's shell, interactive, reading a
        // startup file that ends in the Claude command. Nothing is typed, so
        // there is nothing to race the shell's startup.
        Some(startup) => {
            let id = new_window(&window_name, path, Some(&startup.command))?;
            // Belt and braces: the stamps inside the startup file are what
            // order correctly, this covers a shell that never read them.
            // Same values either way.
            stamp_birth_name(&id, &window_name);
            stamp_session_id(&id, session_id);
            Some(window_name)
        }
        // A shell with no startup hook we know. The window runs the plain
        // default shell and the line is typed into it — stamped first, which
        // orders it: Claude cannot reach its first hook until keys that have
        // not been sent yet arrive.
        None => {
            let id = new_window(&window_name, path, None)?;
            stamp_birth_name(&id, &window_name);
            stamp_session_id(&id, session_id);
            send_line(&id, &line).then_some(window_name)
        }
    }
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

    #[test]
    fn claude_command_building() {
        let build = |resume, yolo, cmd, prefix| {
            claude_command_line(resume, yolo, cmd, prefix, "claude:wt", None)
        };

        // A fresh session is named at launch; a resumed one keeps its own name.
        assert_eq!(build(None, false, None, None), "claude -n 'claude:wt'");
        assert_eq!(
            build(None, true, None, None),
            "claude --dangerously-skip-permissions -n 'claude:wt'"
        );
        assert_eq!(
            build(Some("abc123"), false, None, None),
            "claude -r 'abc123'"
        );
        // A resume id is a transcript filename stem — read off disk, not
        // written by forestui — so it is quoted like every other foreign
        // string that reaches a shell.
        assert_eq!(
            build(Some("x; rm -rf ~"), false, None, None),
            "claude -r 'x; rm -rf ~'"
        );
        // A custom button never gets the YOLO flag appended.
        assert_eq!(
            build(None, true, Some("claude --model opus"), Some("opus")),
            "claude --model opus -n 'claude:wt'"
        );
    }

    /// A fresh session runs under the id forestui minted, so the window knows
    /// which session it holds from birth. A resumed session must never get the
    /// flag — it already *has* an id, and `--session-id` alongside `-r` would
    /// be a contradiction for Claude to resolve.
    #[test]
    fn a_preminted_session_id_reaches_fresh_sessions_only() {
        assert_eq!(
            claude_command_line(None, false, None, None, "claude:wt", Some("id-123")),
            "claude -n 'claude:wt' --session-id 'id-123'"
        );
        assert_eq!(
            claude_command_line(Some("abc"), false, None, None, "claude:wt", Some("id-123")),
            "claude -r 'abc'"
        );
    }

    /// The live scan trusts only stamped windows, and decides "running" from
    /// the pane's foreground command: a shell at the prompt is an open window
    /// whose Claude has exited, not a live session.
    #[test]
    fn claude_windows_parse_from_stamps_and_foreground_command() {
        let out = "@1\tnode\tsess-a\tclaude:wt\n\
                   @2\tzsh\tsess-b\tyolo:other\n\
                   @3\tvim\t\tedit:wt\n\
                   garbage-line\n";
        let windows = parse_claude_windows(out);
        assert_eq!(windows.len(), 2, "unstamped windows are not Claude's");
        assert_eq!(windows[0].session_id, "sess-a");
        assert!(windows[0].running);
        assert_eq!(windows[0].window_name, "claude:wt");
        assert_eq!(windows[1].session_id, "sess-b");
        assert!(!windows[1].running, "a shell at the prompt is not running");
    }

    /// Window names may contain tabs? They cannot — but the name field is last
    /// in the format string on purpose, so a name containing the separator
    /// still parses: `splitn(4)` leaves everything after the third tab intact.
    #[test]
    fn a_tab_in_a_window_name_does_not_shift_fields() {
        let windows = parse_claude_windows("@1\tnode\tsess-a\tweird\tname\n");
        assert_eq!(windows[0].window_name, "weird\tname");
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

        // And the name is still literal in the line the window is given.
        assert_eq!(
            claude_command_line(None, false, None, None, "$(id); rm -rf /", None),
            "claude -n '$(id); rm -rf /'"
        );
    }

    /// The line becomes one line of a file a shell reads, so a newline in it
    /// would end that line and leave the rest standing as a command of its
    /// own — from a window name forestui did not write. Control characters
    /// never survive that far.
    #[test]
    fn a_newline_in_a_name_cannot_smuggle_a_command() {
        let built = claude_command_line(None, false, None, None, "wt'\nrm -rf /\n#", None);
        assert!(!built.contains('\n'), "a newline survived: {built:?}");
        assert_eq!(built, "claude -n 'wt'\\''rm -rf /#'");
    }

    fn home() -> ShellHome {
        ShellHome {
            home: "/home/u".into(),
            zdotdir: None,
            env_file: None,
        }
    }

    /// Each hook replaces one of the user's own startup files, so each
    /// generated file has to source the file it displaced. Getting this wrong
    /// is invisible until someone's aliases or PATH quietly stop applying in
    /// Claude windows only.
    #[test]
    fn a_generated_startup_file_sources_the_one_it_replaced() {
        let dir = Path::new("/tmp/launch");
        let tail = startup_tail("claude:wt", "sess-1", "claude");
        let tail = tail.as_str();

        let zsh = startup_for(
            "/usr/bin/zsh",
            dir,
            &home(),
            "claude:wt",
            "sess-1",
            "claude",
            "claude -r 'sess-1'",
        )
        .expect("zsh is known");
        assert_eq!(zsh.command, "env ZDOTDIR='/tmp/launch' '/usr/bin/zsh' -i");
        let names: Vec<_> = zsh.files.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(names, vec![dir.join(".zshenv"), dir.join(".zshrc")]);
        assert!(zsh.files[0].1.contains("'/home/u/.zshenv'"));
        // ZDOTDIR is put back before the user's own file runs.
        assert!(zsh.files[1].1.starts_with("ZDOTDIR='/home/u'\n"));
        assert!(zsh.files[1].1.contains("'/home/u/.zshrc'"));
        assert!(zsh.files[1].1.ends_with(tail));

        let bash = startup_for(
            "/bin/bash",
            dir,
            &home(),
            "claude:wt",
            "sess-1",
            "claude",
            "claude -r 'sess-1'",
        )
        .expect("bash is known");
        assert_eq!(bash.command, "'/bin/bash' --rcfile '/tmp/launch/rc' -i");
        assert!(bash.files[0].1.contains("'/home/u/.bashrc'"));
        assert!(bash.files[0].1.ends_with(tail));

        let sh = startup_for(
            "/bin/dash",
            dir,
            &home(),
            "claude:wt",
            "sess-1",
            "claude",
            "claude -r 'sess-1'",
        )
        .expect("dash is known");
        assert_eq!(sh.command, "env ENV='/tmp/launch/rc' '/bin/dash' -i");
        assert!(sh.files[0].1.ends_with(tail));

        // The command lands in the shell's history where there is a builtin
        // for it, so the up arrow restarts a session that was interrupted —
        // and what is remembered is the *resume* form, because a fresh
        // session's `--session-id` refuses to run a second time and the up
        // arrow exists precisely for the second run.
        let expected_zsh = format!("print -s -- {}", sh_quote("claude -r 'sess-1'"));
        let expected_bash = format!("history -s {}", sh_quote("claude -r 'sess-1'"));
        assert!(zsh.files[1].1.contains(&expected_zsh), "{}", zsh.files[1].1);
        assert!(
            bash.files[0].1.contains(&expected_bash),
            "{}",
            bash.files[0].1
        );
        assert!(!sh.files[0].1.contains("history"));

        // A shell whose ENV already names a file keeps reading it.
        let inherited = ShellHome {
            env_file: Some("/home/u/.shinit".into()),
            ..home()
        };
        let sh = startup_for(
            "/bin/sh",
            dir,
            &inherited,
            "claude:wt",
            "sess-1",
            "claude",
            "claude -r 'sess-1'",
        )
        .expect("sh is known");
        assert!(sh.files[0].1.contains("'/home/u/.shinit'"));
    }

    /// Every generated file removes itself once the shell has it, so a session
    /// left open for a week does not leave one behind.
    #[test]
    fn a_generated_startup_file_sweeps_itself_up() {
        let dir = Path::new("/tmp/launch");
        for shell in ["/usr/bin/zsh", "/bin/bash", "/bin/sh"] {
            let startup = startup_for(
                shell,
                dir,
                &home(),
                "claude:wt",
                "sess-1",
                "claude",
                "claude",
            )
            .expect("a known shell");
            let last = &startup.files.last().expect("a file").1;
            assert!(
                last.contains("rm -rf '/tmp/launch'"),
                "{shell} leaves its startup file behind: {last:?}"
            );
        }
    }

    /// An unknown shell is not a failure — the line is typed into a plain
    /// shell instead, which needs no hook at all.
    #[test]
    fn an_unknown_shell_falls_back_to_typing() {
        let call = |shell| {
            startup_for(
                shell,
                Path::new("/tmp/l"),
                &home(),
                "claude:wt",
                "sess-1",
                "claude",
                "claude",
            )
        };
        assert!(call("/usr/bin/fish").is_none());
        assert!(call("/usr/bin/nu").is_none());
    }

    /// The stamp goes in ahead of the command, from inside the window: the
    /// shell starts Claude on its own, so a stamp sent after `new-window`
    /// returns can lose the race to Claude's first hook.
    #[test]
    fn the_startup_file_stamps_before_it_launches() {
        let tail = startup_tail("yolo:wt", "sess-1", "claude -n 'yolo:wt'");
        let stamp = tail.find("@claude_birth_name").expect("a stamp");
        let monitor = tail.find("set -m").expect("job control");
        let launch = tail.find("claude -n").expect("the command");
        assert!(
            stamp < monitor && monitor < launch,
            "out of order: {tail:?}"
        );
        assert!(tail.contains("@claude_birth_name 'yolo:wt'"));
    }
}
