//! Command-line interface and the tmux bootstrap.
//!
//! forestui runs inside tmux. When started outside one it re-executes itself
//! through `tmux`, creating or joining a session named after the forest folder.

use clap::Parser;
use std::path::{Path, PathBuf};

/// Taken from `Cargo.toml`, which the release workflow stamps from the git tag.
/// It stays `0.0.0` in the repository, which is what auto-enables dev mode when
/// running from source.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(
    name = "forestui",
    version = VERSION,
    about = "forestui - Git Worktree Manager",
    long_about = "forestui - Git Worktree Manager\n\n\
                  A terminal UI for managing Git worktrees, inspired by forest for macOS.\n\n\
                  FOREST_PATH: Optional path to forest directory (default: ~/forest)"
)]
pub struct Args {
    /// Path to the forest directory (default: ~/forest)
    pub forest_path: Option<String>,

    /// Accepted for compatibility with the Python build; the Rust build never
    /// self-updates, so this is a no-op.
    #[arg(long = "no-self-update")]
    pub no_self_update: bool,

    /// Reserved for parity with the Textual build; currently a no-op
    #[arg(long = "debug")]
    pub debug_mode: bool,

    /// Dev mode: use a timestamped window name (forestui-dev-HHMM)
    #[arg(long = "dev")]
    pub dev_mode: bool,
}

/// tmux window name for this instance.
pub fn window_name(dev_mode: bool) -> String {
    if dev_mode {
        let hhmm = chrono::Local::now().format("%H%M");
        format!("forestui-dev-{hhmm}")
    } else {
        "forestui".to_string()
    }
}

/// Is this window name one of ours?
pub fn is_forestui_window(name: &str) -> bool {
    name == "forestui" || name.starts_with("forestui-dev-")
}

/// tmux session name for a forest path.
pub fn session_name(forest_path: Option<&str>) -> String {
    let folder = match forest_path {
        Some(path) => crate::util::expand_and_resolve(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "forest".to_string()),
        None => "forest".to_string(),
    };
    format!("forestui-{}", crate::util::slugify(&folder))
}

/// Rebuild this invocation as a command line tmux can run.
///
/// Uses the real executable path rather than the bare name `forestui`, so a
/// binary that is not on `PATH` — a `cargo run` build, a checked-out release —
/// re-executes itself rather than a different copy.
fn self_command(args: &Args, program: &Path) -> String {
    let mut parts = vec![shell_quote(&program.to_string_lossy())];
    if args.debug_mode {
        parts.push("--debug".into());
    }
    if args.no_self_update {
        parts.push("--no-self-update".into());
    }
    if args.dev_mode {
        parts.push("--dev".into());
    }
    if let Some(path) = &args.forest_path {
        parts.push(shell_quote(path));
    }
    parts.join(" ")
}

/// Quote a value for a POSIX shell.
pub fn shell_quote(value: &str) -> String {
    shlex::try_quote(value)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| value.to_string())
}

fn tmux_output(args: &[&str]) -> Option<std::process::Output> {
    std::process::Command::new("tmux").args(args).output().ok()
}

/// Ensure forestui is running inside tmux, re-executing into it when it is not.
///
/// This never returns when it re-executes.
pub fn ensure_tmux(args: &Args) -> anyhow::Result<()> {
    if std::env::var("TMUX").is_ok_and(|v| !v.is_empty()) {
        return Ok(());
    }

    if which_tmux().is_none() {
        eprintln!("Error: forestui requires tmux to be installed.");
        eprintln!();
        eprintln!("Install tmux:");
        eprintln!("  macOS:  brew install tmux");
        eprintln!("  Ubuntu: sudo apt install tmux");
        eprintln!("  Fedora: sudo dnf install tmux");
        std::process::exit(1);
    }

    let session = session_name(args.forest_path.as_deref());
    let program = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("forestui"));
    let command = self_command(args, &program);

    let target = format!("={session}");
    let session_exists = tmux_output(&["has-session", "-t", &target])
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !session_exists {
        // No session: create one with forestui as the initial command.
        exec_tmux(&["new-session", "-s", &session, &command]);
    }

    // The session is alive but forestui may have been killed inside it.
    let windows = tmux_output(&["list-windows", "-t", &target, "-F", "#{window_name}"])
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    if !windows.lines().any(is_forestui_window) {
        let _ = tmux_output(&[
            "new-window",
            "-t",
            &target,
            "-n",
            &window_name(args.dev_mode),
            &command,
        ]);
    }

    // A grouped session per terminal: each client navigates windows
    // independently while sharing the same window list.
    let grouped = format!("{session}-{}", std::process::id());
    let grouped_ok = tmux_output(&["new-session", "-d", "-s", &grouped, "-t", &target])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !grouped_ok {
        exec_tmux(&["attach-session", "-t", &session]);
    }

    // Set destroy-unattached only once a client has attached — setting it on a
    // detached session destroys it immediately. keep-last spares the last
    // session in the group.
    let _ = tmux_output(&[
        "set-hook",
        "-t",
        &grouped,
        "client-attached",
        "set-option destroy-unattached keep-last",
    ]);

    // Show the base session name in the status bar rather than the PID-suffixed
    // internal name.
    if let Some(output) = tmux_output(&["show-options", "-gv", "status-left"])
        && output.status.success()
    {
        let raw = String::from_utf8_lossy(&output.stdout);
        let status_left = raw.trim_end_matches('\n').replace("#S", &session);
        if !status_left.is_empty() {
            let _ = tmux_output(&["set-option", "-t", &grouped, "status-left", &status_left]);
        }
    }

    exec_tmux(&["attach-session", "-t", &grouped]);
}

fn which_tmux() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("tmux"))
        .find(|candidate| candidate.is_file())
}

/// Replace this process with tmux. Never returns on success.
fn exec_tmux(args: &[&str]) -> ! {
    // Falling back to spawn+wait keeps behaviour sane on platforms without exec.
    let status = std::process::Command::new("tmux").args(args).status();
    match status {
        Ok(status) => std::process::exit(status.code().unwrap_or(0)),
        Err(error) => {
            eprintln!("Error: failed to run tmux: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_names() {
        assert_eq!(window_name(false), "forestui");
        assert!(window_name(true).starts_with("forestui-dev-"));
        assert!(is_forestui_window("forestui"));
        assert!(is_forestui_window("forestui-dev-0930"));
        assert!(!is_forestui_window("claude:demo"));
    }

    #[test]
    fn session_names_follow_the_forest_folder() {
        assert_eq!(session_name(None), "forestui-forest");
        assert_eq!(session_name(Some("/tmp/My Work")), "forestui-my-work");
    }

    #[test]
    fn self_command_quotes_and_carries_flags() {
        let args = Args {
            forest_path: Some("/tmp/my forest".into()),
            no_self_update: true,
            debug_mode: false,
            dev_mode: true,
        };
        let command = self_command(&args, Path::new("/usr/local/bin/forestui"));
        assert!(command.starts_with("/usr/local/bin/forestui"));
        assert!(command.contains("--no-self-update"));
        assert!(command.contains("--dev"));
        assert!(!command.contains("--debug"));
        assert!(command.contains("'/tmp/my forest'"));
    }

    #[test]
    fn self_command_quotes_paths_with_spaces_in_the_program() {
        let args = Args {
            forest_path: None,
            no_self_update: false,
            debug_mode: false,
            dev_mode: false,
        };
        let command = self_command(&args, Path::new("/tmp/my dir/forestui"));
        assert_eq!(command, "'/tmp/my dir/forestui'");
    }
}
