//! What the installed `claude` binary understands.
//!
//! Remote Control is on by default, so an install whose Claude predates
//! `--remote-control` would otherwise have every launch refused as an unknown
//! option — each window dropping straight to a shell prompt after a forestui
//! update, with nothing changed on the user's side. One `claude --help` at
//! startup answers the question for the rest of the process.

use std::sync::OnceLock;
use std::time::Duration;

use tokio::process::Command;

/// `Some(false)` only once a clean `claude --help` has been read and the flag
/// is not in it. Every other outcome — not probed yet, `claude` not on this
/// process's `PATH`, a probe that failed or hung — leaves the flag in: a probe
/// that could not run is no evidence the flag is missing, and dropping it on a
/// guess would silently cost the user the feature they have on.
static REMOTE_CONTROL: OnceLock<bool> = OnceLock::new();

/// A help screen that never arrives is not an answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Whether launches may carry `--remote-control`.
pub fn remote_control_supported() -> bool {
    REMOTE_CONTROL.get().copied().unwrap_or(true)
}

/// Run `claude --help` once and record whether it offers `--remote-control`.
/// Returns `Some(false)` exactly when the answer is a definite no.
pub async fn probe_remote_control() -> Option<bool> {
    if cfg!(test) {
        // The suite must not depend on, or spawn, whatever Claude the
        // developer has installed.
        return None;
    }
    let mut command = Command::new("claude");
    command.arg("--help").kill_on_drop(true);
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let supported = help_offers_remote_control(&String::from_utf8_lossy(&output.stdout));
    Some(*REMOTE_CONTROL.get_or_init(|| supported))
}

/// Whether a `claude --help` screen lists the option itself — not just its
/// `--remote-control-session-name-prefix` sibling.
fn help_offers_remote_control(help: &str) -> bool {
    help.split(|c: char| c.is_whitespace() || c == ',')
        .any(|word| word == "--remote-control")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_option_is_recognised_in_a_help_screen() {
        let current = "  -r, --resume [value]    Resume a conversation\n\
                       \x20 --remote-control [name]  Start an interactive session with Remote\n\
                       \x20 --remote-control-session-name-prefix <prefix>\n";
        assert!(help_offers_remote_control(current));
    }

    /// The sibling option alone does not make the flag exist, and a help screen
    /// from before Remote Control must read as a definite no.
    #[test]
    fn an_older_help_screen_reads_as_unsupported() {
        let older = "  -r, --resume [value]    Resume a conversation\n\
                     \x20 -n, --name <name>       Set a display name\n";
        assert!(!help_offers_remote_control(older));
        assert!(!help_offers_remote_control(
            "  --remote-control-session-name-prefix <prefix>\n"
        ));
    }

    /// Until a probe has answered, launches keep the flag.
    #[test]
    fn unknown_means_supported() {
        assert!(remote_control_supported());
    }
}
