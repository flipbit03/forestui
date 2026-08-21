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

    let dir = claude_plugin::plugin_dir();
    println!("plugin:  {}", claude_plugin::PLUGIN_NAME);
    println!("path:    {}", dir.display());
    println!("status:  {}", claude_plugin::status().label());

    match action {
        cli::ClaudePluginAction::Status => {
            if let Status::Drifted(files) = claude_plugin::status() {
                for file in files {
                    println!("modified: {file}");
                }
            }
            println!();
            println!("An install writes only these files. Your settings.json is not touched;");
            println!("Claude discovers plugin directories on its own.");
            for path in claude_plugin::planned_paths() {
                println!("  {}", path.display());
            }
        }
        cli::ClaudePluginAction::Install => {
            claude_plugin::install(false).map_err(|e| anyhow::anyhow!(e))?;
            println!();
            println!("Installed. It takes effect in Claude sessions started from now on.");
            println!("Renaming a forestui tab now renames the Claude session in it.");
            println!("To stop that, uninstall — there is no per-tab switch to remember.");
        }
        cli::ClaudePluginAction::Uninstall => {
            claude_plugin::uninstall().map_err(|e| anyhow::anyhow!(e))?;
            println!();
            println!("Removed.");
        }
    }
    Ok(())
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
fn enable_mouse() {
    print!("\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h");
    let _ = std::io::stdout().flush();
}

fn disable_mouse() {
    print!("\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l");
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
