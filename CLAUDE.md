# CLAUDE.md - AI Development Guidelines

This file provides context for AI assistants (like Claude) working on forestui.

## Project Overview

forestui is a terminal UI for managing Git worktrees, built with Rust and
ratatui. It runs inside tmux and gives developers one place to manage many
worktrees, their editors, their terminals, and their Claude Code sessions.

It was a Python/Textual application through v0.9.x. The rewrite is in
`doc/rust-rewrite/` — spec, architecture, migration plan, and the `tu`-driven
acceptance playbook.

## Tech Stack

- **Rust 1.88+**, edition 2024
- **ratatui 0.30** for the TUI, **crossterm 0.29** (re-exported as `ratatui::crossterm`)
- **tokio** for background work and subprocesses
- **serde / serde_json** for the two JSON config files
- **clap** (derive) for the CLI

Git and tmux are driven by shelling out to the real binaries, not through
bindings. That keeps the argv identical to the Python build and avoids a
libgit2 build dependency.

## Project Structure

```
forestui/
├── Cargo.toml
├── src/
│   ├── main.rs               # Entry point, tokio runtime, crash reporting
│   ├── cli.rs                # clap args + the tmux bootstrap (ensure_tmux)
│   ├── app/
│   │   ├── mod.rs            # App state, startup, sidebar, event folding
│   │   ├── detail.rs         # Detail-pane content walk (single source of truth)
│   │   ├── keys.rs           # Keyboard input and the rename fields
│   │   ├── mouse.rs          # Hit testing, hover, wheel routing, scrollbar
│   │   └── actions.rs        # What activating a control does; spawned flows
│   ├── event.rs              # AppEvent + the single event channel
│   ├── modal.rs              # Modal stack: state, focus model, key handling
│   ├── models.rs             # Serde models (config file schemas)
│   ├── state.rs              # Persisted repositories (.forestui-config.json)
│   ├── theme.rs              # Colour palette and shared styles
│   ├── util.rs               # Paths, slugs, relative times, fuzzy branch search
│   ├── version_check.rs      # Startup self-update (see below)
│   ├── ui/
│   │   ├── mod.rs            # draw(): layout, header, footer, notifications
│   │   ├── sidebar.rs        # Repository/worktree tree
│   │   ├── detail.rs         # Repository and worktree detail panes
│   │   ├── modals.rs         # Modal overlays
│   │   └── widgets.rs        # TextInput and small helpers
│   └── services/
│       ├── git.rs            # Async git operations
│       ├── tmux.rs           # tmux window management
│       ├── github.rs         # gh CLI integration
│       ├── claude_session.rs # Claude Code session tracking
│       └── settings.rs       # User preferences + runtime forest path
├── Makefile                  # Development commands
├── install.sh                # Installation script
└── README.md
```

## Development Commands

```bash
make lint         # cargo clippy --all-targets -- -D warnings
make typecheck    # cargo check --all-targets
make format-check # cargo fmt --check
make test         # cargo test
make check        # all of the above
make format       # cargo fmt
make run          # cargo run
make clean        # cargo clean
```

Always run `make check` before committing changes.

## Code Conventions

### Rust
- Edition 2024. Prefer let-chains (`if let Some(x) = ... && cond`) over nesting.
- No `unwrap()` or `expect()` on anything that can fail at runtime. Config
  parsing, subprocess spawning, and filesystem access all degrade gracefully —
  a corrupt config file must never block startup.
- Errors that reach the user become a notification, not a panic.
- Colours come from `theme.rs`. Never hardcode a `Color::Rgb` in a UI module.
- Comments explain *why*, not *what*.

### Immediate-mode UI
ratatui redraws the whole frame every event; there are no persistent widget
objects and no CSS. Three consequences shape the code:

1. **Focus is an index, not a widget.** `app/detail.rs::content()` walks the
   current selection once and produces the pane's full node list — text, cards,
   and every focusable control in order. `App::detail_items()` collects the
   focusable items from that list and `ui/detail.rs` renders the same list, so
   item N on screen is item N in the key handler *by construction*. To add or
   move a control, change the content walk; both consumers follow automatically.

   Activation goes through one more indirection: each frame the renderer
   snapshots the drawn items onto `App::drawn_items` (with each control's
   enabled bit), and a click or `Enter` resolves against that snapshot — never
   against a freshly derived list, which a background event drained in the same
   batch may already have reshaped. The snapshot is what was on screen, which
   is what the user acted on; a control drawn as disabled must not fire.
2. **Layout is `Layout`/`Constraint`, not stylesheets.** Sizes are computed per
   frame; nothing is "auto height".
3. **Mouse support is manual.** There is no widget tree to hit-test against, so
   every frame the renderers record the rectangle of each clickable thing via
   `App::push_hit(rect, HitTarget::…)`, and `App::handle_mouse` resolves a click
   against that list (last recorded wins, so a modal takes clicks from the panes
   it covers). A control that is drawn but never recorded is dead to the mouse —
   that was a real regression: the first Rust build enabled no mouse capture at
   all and every click did nothing.

   `main.rs` writes the mouse modes by hand rather than using crossterm's
   `EnableMouseCapture`, and **must keep `?1003h` (any-motion) among them** —
   without it the terminal never reports a bare pointer move, so hover becomes
   impossible rather than merely unstyled. Motion is affordable because
   `App::handle_mouse` sets `App::redraw` only when the *hovered target changes*.
   That gate is what stops a moving mouse repainting the app continuously;
   removing it brings back the flicker that motion reporting was first disabled
   for.

Controls are drawn as three-row bordered boxes from one builder in
`ui/widgets.rs`: `button_box`/`button_box_width` for modal buttons (Textual's
`min-width: 10`) and the same code via `boxed_rows`/`boxed_width` with no
minimum for the detail pane, whose boxes hug their labels. Use the matching
width function for the rectangle you record, so the hit region matches what
was drawn.

### Async and the event loop
Everything funnels through one `mpsc::UnboundedReceiver<AppEvent>` in
`event.rs`. Terminal input arrives from a blocking reader thread; background
work arrives from `tokio::spawn`.

**Never block the event loop.** For any operation that may take time (git, gh,
scanning session files):
1. Render immediately with a loading state (`app.sessions` / `app.issues` are
   `Option`, where `None` means "still loading").
2. `tokio::spawn` the work with a cloned `EventTx`.
3. Send an `AppEvent` when it completes; `App::handle_event` folds it into state.

Late results must be discarded when the selection has moved on — compare the
event's `path` against the current one before applying it.

**The main loop is the single writer of `.forestui-config.json`.** Background
tasks never load or save state themselves — they send their results as data
(`WorktreeAdded`, `WorktreesImported`, `WorktreeRenamed`) and `App::handle_event`
folds them in and persists. Two writers means last-write-wins, and a user
action mid-flight silently clobbers the task's save. Success toasts belong in
the fold, not the task: a result the fold drops must not be announced.

**The tick must earn its repaints.** Ticks arrive ten times a second; an idle
frame repaints once a second (matching the second granularity of the relative
times on screen) and at full rate only while a spinner is actually visible.
Anything animated that is added later has to mark the frame dirty from
`App::on_tick`, or it freezes; anything that is not visibly changing must not.

### Services
Services are free functions in modules, not singletons. Shared caches (the
GitHub issue cache, the auth status) live behind a `OnceLock<Mutex<...>>` inside
the module that owns them.

## Key Design Decisions

### tmux Requirement
forestui requires tmux and re-executes itself into a tmux session when started
outside one. This enables TUI editors in tmux windows, session persistence, and
fast switching between editor, app, and Claude windows.

The re-exec uses `std::env::current_exe()`, so a `cargo run` build re-launches
itself rather than whatever `forestui` happens to be on `PATH`.

### Self-update
forestui keeps itself current the way the Python build did — automatically, on
launch — but never on the UI thread. `App::check_for_update` spawns the check
once the terminal is up, and the only visible result is a notification after a
new version is already in place.

What it does depends on how the binary got there:

- **From a GitHub release** — built with `--features binary-release`, so it
  downloads the asset for its platform and renames it over `current_exe()`.
  Replacing the file under a running process is safe on Unix; the new build
  takes effect on the next launch.
- **From `cargo install`** — the feature is off, so it only reports that a newer
  version exists. Recompiling a crate underneath a running TUI is not something
  to do unasked.
- **From source** (version `0.0.0`) — nothing at all, which is what stops
  `cargo run` in a checkout from overwriting itself.

`--no-self-update` skips the check. The answer is cached for 24h in
`~/.config/forestui/latest_version_check.json`, so this is not a network call on
every launch.

`release_asset_url` must keep producing `forestui_<os>_<arch>`, matching
`install.sh` and the release workflow. Drift there is a silent 404 on every
update, which is why a test asserts the shape.

### Multi-Forest Support
The forest directory is a CLI argument, not a setting:

```bash
forestui ~/forest      # default
forestui ~/work        # different forest
```

Each forest has its own `.forestui-config.json` state file.

### Config Compatibility
`.forestui-config.json` and `~/.config/forestui/settings.json` keep the exact
filenames and JSON schemas the Python build used, so a user can move between
builds without losing state. Every field is `#[serde(default)]` so partial and
older files load cleanly. **Do not rename or restructure these fields.**

Both files are written via `util::write_atomically` (sibling temp file +
rename). A corrupt config deliberately loads as *empty* state so a bad byte
never blocks startup — which is exactly why the write itself must never be
able to produce a torn file.

### Coexistence with forest (macOS)
- forestui uses `.forestui-config.json`
- forest uses `.forest-config.json`
- Both can share the same `~/forest` directory safely

## Common Tasks

### Adding a New Service
1. Create `src/services/myservice.rs`
2. Add `pub mod myservice;` to `src/services/mod.rs`
3. Expose free functions; keep any shared state in a module-private `OnceLock`

### Adding a Detail-Pane Action
1. Add a variant to `Action` in `src/app/detail.rs`
2. Emit a `ControlSpec` for it from the content walk in `src/app/detail.rs`
   (`repository()` / `worktree()` / their sections) — the item list and the
   rendering both derive from that one walk, so there is no second place to
   keep in sync
3. Handle it in `App::run_action` in `src/app/actions.rs`
4. If it needs a new visual shape (not a boxed control), add a `DetailNode`
   variant and render it in `render_node` in `src/ui/detail.rs`

### Adding a Key Binding
1. Add it to `BINDINGS` in `src/app/keys.rs` if it should appear in the footer
   (the footer is Textual-parity: think before growing it), or to
   `EXTRA_BINDINGS` if only the `?` help toast should list it — both derive
   from these tables, so there is no second list to update
2. Handle the key in the `handle_key` match in the same file

### Adding a Modal
1. Add the state struct and its `handle_key` to `src/modal.rs`, declaring a
   `FOCUS_*` constant per control — the constants are the contract the
   renderer uses too
2. Add a variant to the `Modal` enum and wire it into `handle_key` / `tick`
3. Render it in `src/ui/modals.rs`
4. If it returns a value, add a `ModalResult` variant and handle it in
   `App::apply_modal_result`

### Adding a New CLI Option
1. Add the field to `Args` in `src/cli.rs`
2. Carry it through `self_command` so the tmux re-exec preserves it
3. Handle it in `main`

### Modifying Settings
1. Update `Settings` in `src/models.rs` (keep `#[serde(default)]`)
2. Update `SettingsModal` in `src/modal.rs` and its rendering in `src/ui/modals.rs`
3. Settings are persisted to `~/.config/forestui/settings.json`

## Testing

Unit tests live beside the code in `#[cfg(test)] mod tests` and run with
`make test`. Async code uses `#[tokio::test]`. Rendering is tested against
`ratatui::backend::TestBackend`, which renders into an in-memory buffer you can
assert on.

**Visual/behavioural verification:** forestui is a TUI app — lint, typecheck and
unit tests alone cannot catch visual bugs, wrong window names, broken
interactions, or UI regressions. Use the `test-forestui` skill to drive forestui
in a headless terminal with `tu` and verify the change works. Do this
proactively, not only when asked.

`doc/rust-rewrite/TU_USECASES.md` is the acceptance playbook: 94 numbered
scenarios with exact keystrokes and expected output. The P0 cases are the
regression suite — run the relevant ones after any behavioural change.

UC-53–70, the flow cases UC-78–84/86, and the guards UC-90/96 are automated —
frames plus on-disk assertions (`ASSERTIONS.txt`). Capture a build and compare
two builds with:

```bash
scripts/tu-sweep.sh rust ./target/release/forestui
scripts/tu-compare.sh rust python
scripts/tu-composite.sh rust python   # side-by-side PNGs, one per case
```

The committed `baseline/python/` frames are a **frozen reference**, captured
from the installed 1.3.0 release (the build users actually ran). That release
is no longer installed here, so treat those frames as read-only history:
re-sweep `rust` freely, never regenerate `python`. `tu-compare.sh` works from
the committed frames; `tu-composite.sh` needs PNGs from a live sweep of both
builds, which the Python side can no longer produce.

Each case writes a normalised text frame to
`doc/rust-rewrite/baseline/<build>/` (committed — this is the diffable
baseline) and a PNG to `doc/rust-rewrite/screenshots/<build>/` (gitignored,
for eyeballing colour and focus). After a UI change, re-run the sweep and
review the frame diff: an unexpected change there is a regression, an expected
one needs the baseline refreshed in the same commit.

**Text frames alone are not enough.** Colour, focus rings and selection
highlights exist only in the pixels — a button that lost its accent renders an
identical frame. `tu-composite.sh` pairs the two builds' PNGs per case so those
differences are visible rather than inferred; read the composites after any
change to `theme.rs` or a renderer.

A sweep that does not exercise a section will happily report parity for it.
UC-96 guards against that specifically: it captures the repository pane at full
height and asserts that sessions and issues actually rendered and that no
section fell back to its empty state. Keep that guard honest when adding cases.

The harness waits on screen conditions, never fixed sleeps — Textual repaints
far slower than ratatui, and fixed sleeps produced false mismatches. Anything
added to the sweep must use `await <regex>`.

Note that `ensure_tmux` re-executes the binary, so for `tu` runs you must
build first and pass the built binary's path as the sweep's command argument
(e.g. `scripts/tu-sweep.sh rust ./target/release/forestui`) — `cargo run` does
not work under `tu`. Pass an absolute path when the tu session's working
directory is not the repo. Run only one tu-driving script at a time:
concurrent drivers corrupt each other's sessions.

## Versioning

Version is `0.0.0` in source (`Cargo.toml` and `cli::VERSION`). Real versions
are derived from git tags at release time by the release workflow.

Running from source shows version `0.0.0` and auto-enables dev mode, which gives
the tmux window a timestamped name (`forestui-dev-HHMM`) so a dev instance never
collides with a real one.

### Development

1. Create a branch, make changes
2. **Always run `make check` before committing, pushing, or opening a PR.** Do
   not push code that fails formatting, clippy, or tests — fix issues first.
3. Open a PR and merge to `main`

### Releasing

```bash
gh release create v0.10.0 --generate-notes
```

This triggers the release workflow, which stamps the version from the tag,
builds the static musl binaries for Linux (x86_64, aarch64) and the macOS
Apple-silicon binary, attaches them to the release with their checksums, and
publishes to crates.io.

## Git Commits

- Do NOT include `Co-Authored-By` attribution
- Do NOT include "Generated with Claude Code" footer
- Write clear, concise commit messages

## References

- [forest (macOS)](https://github.com/ricwo/forest) - Original inspiration
- [ratatui Documentation](https://ratatui.rs)
- [crossterm Documentation](https://docs.rs/crossterm)
- [tokio Documentation](https://tokio.rs)
