//! forestui — a terminal UI for managing Git worktrees.

mod app;
mod cli;
mod event;
mod modal;
mod models;
mod services;
mod state;
mod theme;
mod ui;
mod util;
mod version_check;

use app::App;
use clap::Parser;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    // A one-shot maintenance action: report and exit, before anything
    // re-executes into tmux or paints a UI.
    if let Some(action) = args.claude_plugin {
        return run_claude_plugin_action(action);
    }

    // Re-executes into tmux and never returns when we are outside one.
    cli::ensure_tmux(&args)?;

    // Running from source always behaves as dev mode, as the Textual build did.
    let dev_mode = args.dev_mode || cli::VERSION == "0.0.0";
    services::tmux::rename_window(&cli::window_name(dev_mode));

    services::settings::set_forest_path(args.forest_path.as_deref());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let result = runtime.block_on(run(!args.no_self_update));

    if let Err(error) = &result {
        report_crash(error);
    }
    result
}

/// `--claude-plugin status|install|uninstall`.
///
/// Prints the plugin's directory and every file involved before doing anything,
/// so this is also the way to see exactly what an install writes without
/// running one.
fn run_claude_plugin_action(action: cli::ClaudePluginAction) -> anyhow::Result<()> {
    use services::claude_plugin::{self, Status};
    use std::fmt::Write as _;

    let dir = claude_plugin::plugin_dir();
    let mut out = String::new();
    let _ = writeln!(out, "plugin:  {}", claude_plugin::PLUGIN_NAME);
    let _ = writeln!(out, "path:    {}", dir.display());
    let _ = writeln!(out, "status:  {}", claude_plugin::status().label());

    let result = match action {
        cli::ClaudePluginAction::Status => {
            if let Status::Drifted(files) = claude_plugin::status() {
                for file in files {
                    let _ = writeln!(out, "modified: {file}");
                }
            }
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "An install writes only these files. Your settings.json is not touched;"
            );
            let _ = writeln!(out, "Claude discovers plugin directories on its own.");
            for path in claude_plugin::planned_paths() {
                let _ = writeln!(out, "  {}", path.display());
            }
            Ok(())
        }
        cli::ClaudePluginAction::Install => claude_plugin::install(false).map(|()| {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Installed. It takes effect in Claude sessions started from now on."
            );
            let _ = writeln!(
                out,
                "Renaming a forestui tab now renames the Claude session in it."
            );
            let _ = writeln!(
                out,
                "To stop that, uninstall — there is no per-tab switch to remember."
            );
            if !claude_plugin::jq_available() {
                let _ = writeln!(out);
                let _ = writeln!(
                    out,
                    "Note: `jq` is not on PATH. The hook reads the session's own name"
                );
                let _ = writeln!(
                    out,
                    "with it, so until jq is installed the sync does nothing at all"
                );
                let _ = writeln!(out, "rather than half of it.");
            }
        }),
        cli::ClaudePluginAction::Uninstall => claude_plugin::uninstall().map(|()| {
            let _ = writeln!(out);
            let _ = writeln!(out, "Removed.");
        }),
    };

    // Written once, and a write error is dropped: piping this into `head`
    // closes the pipe, and `println!` turns that into a panic.
    let _ = std::io::stdout().write_all(out.as_bytes());
    result.map_err(|e| anyhow::anyhow!(e))
}

async fn run(self_update: bool) -> anyhow::Result<()> {
    let (tx, mut rx) = event::start();
    let mut app = App::new(tx, cli::VERSION.to_string());
    app.self_update = self_update;

    let mut terminal = ratatui::init();
    enable_mouse();
    app.on_start();

    let outcome = loop {
        // Skip the repaint when the last batch of events changed nothing on
        // screen — pointer motion inside one control is the common case.
        if app.redraw {
            app.redraw = false;
            if let Err(error) = terminal.draw(|frame| ui::draw(frame, &mut app)) {
                break Err(anyhow::Error::from(error));
            }
        }
        let Some(event) = rx.recv().await else {
            break Ok(());
        };
        app.handle_event(event);

        app.drain(&mut rx);

        if app.should_quit {
            break Ok(());
        }
    };

    disable_mouse();
    ratatui::restore();
    outcome
}

/// Turn on mouse reporting for button presses, the wheel, and pointer motion.
///
/// `?1003h` (any-motion) is what makes hover possible at all: without it the
/// terminal never reports a bare move, so no control can light up under the
/// pointer. It was previously left off because every motion report woke the
/// loop and repainted, which read as the app flickering under a moving mouse.
/// That is fixed at the source instead — `App::handle_mouse` only marks the
/// frame dirty when the *hovered target changes*, so crossing a control costs
/// one repaint and sliding around inside it costs none.
///
/// `?1006h` asks for SGR coordinates so columns past 223 still resolve.
///
/// `?1004h` asks for focus reporting, and without it the terminal never sends
/// focus in/out at all — so `Event::FocusGained` never arrived and everything
/// hung off it was dead code. `ensure_focus_events` turning on tmux's
/// `focus-events` is only half of the handshake: tmux forwards the sequences
/// to a pane that asked for them, and this is the asking.
/// Kept as one string so a test can pin it: a mode dropped from here fails
/// silently and takes a whole feature with it, which is how focus reporting
/// went missing and left `Event::FocusGained` unable to fire at all.
const TERMINAL_MODES_ON: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1004h";
const TERMINAL_MODES_OFF: &str = "\x1b[?1004l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

fn enable_mouse() {
    print!("{TERMINAL_MODES_ON}");
    let _ = std::io::stdout().flush();
}

fn disable_mouse() {
    print!("{TERMINAL_MODES_OFF}");
    let _ = std::io::stdout().flush();
}

/// Mirror the Textual build's crash handling: write a log, print it, and pause
/// so the message is readable before the tmux window closes.
fn report_crash(error: &anyhow::Error) {
    let log_path = util::home_dir().join(".forestui-error.log");
    let report = format!("{error:?}\n");
    let _ = std::fs::write(&log_path, &report);
    eprintln!("{report}");
    eprintln!("\nError: {error}");
    eprintln!("\nError log written to: {}", log_path.display());
    eprint!("Press Enter to exit...");
    let _ = std::io::stderr().flush();
    let mut discard = String::new();
    let _ = std::io::stdin().read_line(&mut discard);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both of these are load-bearing and neither fails loudly.
    ///
    /// Without `?1003h` the terminal never reports a bare pointer move, so
    /// hover cannot work. Without `?1004h` it never reports focus at all, so
    /// `Event::FocusGained` never arrives and everything hanging off it —
    /// refreshing sessions and worktrees when the user comes back — is dead
    /// code that still compiles and still has passing tests.
    #[test]
    fn the_terminal_modes_we_depend_on_are_requested() {
        for (mode, why) in [
            ("?1003h", "any-motion, for hover"),
            ("?1004h", "focus reporting, for refresh on return"),
            ("?1006h", "SGR coordinates, for columns past 223"),
        ] {
            assert!(
                TERMINAL_MODES_ON.contains(mode),
                "{mode} ({why}) is not requested"
            );
            let off = mode.replace('h', "l");
            assert!(
                TERMINAL_MODES_OFF.contains(&off),
                "{mode} ({why}) is requested but never turned off"
            );
        }
    }
}
