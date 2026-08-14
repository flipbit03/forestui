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

use app::App;
use clap::Parser;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    // Re-executes into tmux and never returns when we are outside one.
    cli::ensure_tmux(&args)?;

    // Running from source always behaves as dev mode, as the Textual build did.
    let dev_mode = args.dev_mode || cli::VERSION == "0.0.0";
    services::tmux::rename_window(&cli::window_name(dev_mode));

    services::settings::set_forest_path(args.forest_path.as_deref());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let result = runtime.block_on(run());

    if let Err(error) = &result {
        report_crash(error);
    }
    result
}

async fn run() -> anyhow::Result<()> {
    let (tx, mut rx) = event::start();
    let mut app = App::new(tx, cli::VERSION.to_string());

    let mut terminal = ratatui::init();
    enable_mouse();
    app.on_start();

    let outcome = loop {
        if let Err(error) = terminal.draw(|frame| ui::draw(frame, &mut app)) {
            break Err(anyhow::Error::from(error));
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

/// Turn on mouse reporting for button presses and the wheel.
///
/// Deliberately not crossterm's `EnableMouseCapture`: that also enables
/// any-motion tracking (`?1003h`), so the terminal reports every pointer
/// movement. Each report wakes the loop and repaints, which showed up as the
/// whole app flickering while the mouse merely moved across it. `?1000h` is
/// press/release only, and `?1006h` asks for SGR coordinates so columns past
/// 223 still resolve.
fn enable_mouse() {
    print!("\x1b[?1000h\x1b[?1006h");
    let _ = std::io::stdout().flush();
}

fn disable_mouse() {
    print!("\x1b[?1006l\x1b[?1000l");
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
