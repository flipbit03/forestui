# CLAUDE.md - AI Development Guidelines

This file provides context for AI assistants (like Claude) working on forestui.

## Never touch the user's live tmux

**This rule applies to every task in this repo, not just testing.**

Whoever works on forestui is, almost by definition, *running* forestui. That
is what makes this different from an ordinary "be careful" note: a live tmux
server is up right now, holding their actual work — editors mid-edit,
terminals mid-command, Claude sessions mid-conversation, laid out the way they
left them. It is not a risk to weigh against convenience; it is the default
state of this repo's development environment, and a bare `tmux` command from a
shell attaches straight to it.

The session running this task is very likely one of those windows — forestui
opens Claude sessions in tmux windows, and this is a Claude session. A stray
`rename-window`, `kill-window` or `kill-server` can destroy the user's work,
up to and including the conversation that issued the command.

- **Never run a bare `tmux` command** — not to look, not to list, not "just
  once". `tmux list-sessions` is as dangerous as `tmux kill-server`, because
  it is the step that convinces you the next call is safe.
- **Drive tmux only through `tu`**, with `TMUX_TMPDIR` pointed at a throwaway
  directory. The `test-forestui` skill has the exact invocation.
- **Exploring tmux behaviour is not an exception.** Learning what a tmux
  command does, probing options or hooks, reproducing something from the
  docs — all of it goes on an isolated server under `tu`. A separate `-L`
  socket looks like it solves this, and it does not: it relies on never once
  forgetting the flag, against a default that reaches the user's live
  session.

The `test-forestui` skill carries the full harness recipe, and its description
asks to be loaded before *any* tmux command, not only before testing — because
tasks that do not look like testing are exactly where this gets broken: a
spike, doc research, "just checking how tmux options work". The rule is
repeated here because this file is always loaded and a skill is not.

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
│       ├── claude_plugin.rs  # Installing the Claude Code title-sync plugin
│       ├── claude_session.rs # Claude Code session tracking
│       └── settings.rs       # User preferences + runtime forest path
├── assets/
│   └── claude-plugin/        # The Claude Code plugin, embedded in the binary
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
make check-shipped # lint + tests under --features binary-release
make format       # cargo fmt
make run          # cargo run
make clean        # cargo clean
```

Always run `make check` before committing changes.

`make check-shipped` runs the same lint and tests with `--features binary-release`,
which is the configuration the release workflow builds. The default build
compiles strictly less code — the in-place updater is `#[cfg]`-ed out — so the
two fail differently, and a mistake inside the feature-gated block is invisible
to `make check`. CI runs both.

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
(`WorktreeAdded`, `WorktreesScanned`, `WorktreeRenamed`) and `App::handle_event`
folds them in and persists. Two writers means last-write-wins, and a user
action mid-flight silently clobbers the task's save. Success toasts belong in
the fold, not the task: a result the fold drops must not be announced.

**The worktree list is reconciled against git, not owned by the config.**
`git worktree list` is scanned in the background (startup, repo add, selection
change, focus return, a 30s sweep) and the fold adopts, prunes and
branch-corrects via `AppState::reconcile_worktrees` — the config only
annotates worktrees (name, archived flag, sort order). A scan result is a
*snapshot*, so it must never outrank a fresher mutation: results carry the
repository's mutation epoch and fold as stale when a create/remove/rename
folded in between, and entries in `removals_in_flight` / `renames_in_flight`
are excluded from reconciliation entirely. Any new worktree-mutating flow must
bump the epoch in its fold and, if it has a mid-flight window a listing could
misread, register itself in an in-flight set.

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

### Session names follow tmux window names

A tmux window forestui opened and the Claude session running in it carry the
same name, in both directions: renaming the tab renames the session, and
`/rename` renames the tab. The two are **one string, verbatim** — nothing is
added, stripped or parsed. A window called `yolo:thing` holds a session called
`yolo:thing`. Prefixes are forestui's opening move when it creates a window,
not a format anything reads back.

It is carried by a Claude Code **plugin** — a directory under the Claude config
dir, which Claude discovers on its own. Installing therefore writes nothing to
the user's `settings.json`, and hooks they configured themselves are never
parsed or rewritten; plugin hooks and settings hooks both run. Enabling and
disabling is Claude's own business (`claude plugin enable|disable`), which is
why nothing here edits `enabledPlugins`. Install it with
`forestui --claude-plugin install`; `status` prints every path an install would
write without writing any of them.

Three pieces make it work, and each is load-bearing:

A new session is named at launch, by passing Claude's own `-n` flag with the
window's name. That is not interchangeable with setting the title from a hook:
Claude draws the name in the prompt box from the first frame with `-n`, whereas
a title a hook sets at `SessionStart` is stored and never drawn — the session is
correctly named and looks unnamed. A hook still names one on its first prompt if
`-n` never reached it.

**A resumed session is not given `-n`.** It already carries the name it was
given, and the window's name may have picked up a `:2` from uniquifying against
a window still open for the same conversation — passing it would overwrite the
real name with the suffixed one. The hook adopts the stored name onto the
window instead.

- **`@claude_birth_name`**, a tmux window option stamped when the window is
  created, answers one question: did forestui open this window? A window the
  user made by hand carries none and is left alone, as is a bare `claude` in a
  plain terminal. It is written from *inside* the window, in a prelude ahead of
  the command that needs it — stamping it from forestui after `new-window`
  returns is a separate tmux call, and a fast-starting Claude could reach its
  first hook before it lands.
- **`@claude_synced_name`** records the last agreed name. Two-way sync cannot
  tell which side moved from one value: equal names are ambiguous between
  nobody changing and both changing alike, and unequal names do not say who is
  stale. When both moved, the tab wins.
- **Window names reach a shell**, so they are quoted literally with single
  quotes rather than by any escaping heuristic. A session title can be set by a
  `SessionStart` hook in a repository's own `.claude/settings.json`, so by the
  time a name gets here it is not necessarily the user's own text. These
  commands run through `-ic`, an *interactive* shell, where double quotes would
  not even stop zsh history expansion.

There is deliberately no off switch, per window or global. Installed means the
two names agree; not wanting that is an uninstall.

### Self-update
forestui keeps itself current the way the Python build did — automatically, on
launch — but never on the UI thread. `App::check_for_update` spawns the check
once the terminal is up. Success shows one notification after the new version
is already in place; network failures (offline, a download that dropped, a
release whose assets have not finished uploading) stay silent and retry next
launch. Only a *persistent local* failure — an unwritable install dir — shows
an error notification, and is remembered for an hour in
`~/.config/forestui/latest_version_check.json` (`install_failed`) so the
multi-MB download is not re-spent on every launch while it would fail
identically. That memo is the *only* thing that file now holds, and the only
state that survives a launch.

What it does depends on how the binary got there:

- **From a GitHub release** — built with `--features binary-release`, so it
  downloads the asset for its platform, verifies it against the published
  `.sha256` (no checksum, no update — release assets **must** ship their
  checksums), and atomically swaps it over `current_exe()` via the same
  fsync-then-rename writer the config files use. Replacing the file under a
  running process is safe on Unix; the new build takes effect on the next
  launch.
- **From `cargo install`** — the feature is off, so it only reports that a newer
  version exists. Recompiling a crate underneath a running TUI is not something
  to do unasked.
- **From source** (version `0.0.0`) — nothing at all, which is what stops
  `cargo run` in a checkout from overwriting itself.

**The version lookup is not cached.** GitHub is asked on every launch, so a
release reaches the user the next time they open forestui rather than whenever
a TTL happened to lapse — the check is already off the UI thread, so nobody
waits on the round trip. The corollary is that a failed lookup has nothing to
fall back on, which is the point: there is no remembered version to act on, so
offline is silence rather than a doomed download or a nag.
`--no-self-update` skips the check entirely.

`release_asset_url` must keep producing `forestui_<os>_<arch>`, matching
`install.sh` and the release workflow. Drift there is a silent 404 on every
update, which is why a test asserts the shape.

### Multi-Forest Support
The forest directory is a CLI argument, not a setting:

```bash
forestui ~/forest      # default
forestui ~/work        # different forest
```

Each forest has its own `.forestui-config.json` state file. Note that the
worktree *list* under a repository is reconciled from `git worktree list`,
which knows nothing about forests: tracking the same repository in two
forests shows the same worktrees in both, and removing one removes it for
both. Forests separate which *repositories* you look at, not worktree
ownership — git owns that.

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
1. Add it to `BINDINGS` in `src/app/keys.rs` — the footer renders this table
   and is the app's *only* key-discovery surface (there is no help screen; the
   `?` toast was removed with issue #30). A test asserts the whole footer fits
   140 columns, so a new entry means shortening a label somewhere; entries
   that do not fit a narrower terminal clip from the right and lose their
   click target
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

`doc/rust-rewrite/TU_USECASES.md` is the acceptance playbook: 96 numbered
scenarios with exact keystrokes and expected output. The P0 cases are the
regression suite — run the relevant ones after any behavioural change.

UC-53–70 (except the retired UC-59), the flow cases UC-78–84/86, and the
guards UC-90/96 are automated —
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

All of the below runs under the isolation rule at the top of this file: tmux
is driven through `tu` with `TMUX_TMPDIR`, never directly.

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
