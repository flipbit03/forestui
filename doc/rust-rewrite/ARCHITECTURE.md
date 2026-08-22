# forestui — Rust + ratatui rewrite architecture

Target: a behaviour-identical port of the Python/Textual `forestui` (6331 lines across 17 modules)
to Rust + ratatui, keeping the on-disk config files byte-compatible so a user can flip between the
two builds at will.

Everything in the crate-research section was verified against the crates.io API and docs.rs on
**2026-08-14**. No version below is from memory.

---

## Part 1 — Crate research

### Ready-to-paste `Cargo.toml`

```toml
[package]
name = "forestui"
version = "0.0.0"
edition = "2024"
rust-version = "1.88"

[dependencies]
ratatui        = "0.30.2"                                          # TUI framework; re-exports crossterm 0.29
tui-input      = "0.15.4"                                          # single-line text input for modals
tokio          = { version = "1.53.1", features = ["rt-multi-thread", "macros", "process", "time", "sync"] }
serde          = { version = "1.0.229", features = ["derive"] }
serde_json     = "1.0.151"
clap           = { version = "4.6.6", features = ["derive"] }
uuid           = { version = "1.24.0", features = ["v4", "serde"] }
chrono         = { version = "0.4.45", features = ["serde"] }
timeago        = { version = "0.6.1", features = ["chrono"] }      # humanize.naturaltime equivalent
nucleo-matcher = "0.3.1"                                           # fuzzy branch search
anyhow         = "1.0.104"

[dev-dependencies]
insta    = "1.48.0"
tempfile = "3.27.0"
```

Eleven runtime crates, two dev crates. Deliberately **not** here: `crossterm` (comes via ratatui,
see below), `dirs` (stdlib covers it), `thiserror` (anyhow alone), `futures`/`tokio-stream`
(the event loop does not need a stream), `git2`/`gix` (we shell out), `shellexpand` (five lines).

### ratatui — 0.30.2

- Published **2026-06-19**. `edition = "2024"`, `rust_version = "1.88.0"` (from the crates.io version
  metadata). That MSRV is the binding constraint for the whole crate.
- 0.30 is a **split crate**: `ratatui` is a facade over `ratatui-core` 0.1.2, `ratatui-widgets` 0.3.2,
  and per-backend shims (`ratatui-crossterm` 0.1.2, `ratatui-termion`, `ratatui-termwiz`,
  `ratatui-termina`). Depend on the `ratatui` facade only — the sub-crates are implementation detail.
- Default features: `all-widgets`, `crossterm`, `layout-cache`, `macros`, `underline-color`.
  Leave them on.
- **Crossterm is re-exported**: `pub use ratatui_crossterm::crossterm;` — so `ratatui::crossterm::event::KeyCode`
  works and there is no need to list `crossterm` in `Cargo.toml` at all. `ratatui-crossterm` 0.1.2's
  default feature is `crossterm_0_29`, so the resolved version is **crossterm 0.29.0**. Adding
  crossterm directly is the classic way to get a version skew here; don't.
- Backend: `ratatui::backend::CrosstermBackend` (gated on the `crossterm` feature, which is default-on).
- Terminal lifecycle helpers **exist in this release**, at the crate root:
  `ratatui::init()`, `ratatui::restore()`, `ratatui::try_init()`, `ratatui::try_init_with_options()`,
  `ratatui::run()`, and the `ratatui::DefaultTerminal` type alias. `init()` enables raw mode, enters
  the alternate screen and installs a panic hook that restores the terminal; `restore()` undoes it.
  Use them — do not hand-roll `enable_raw_mode()` / `EnterAlternateScreen`.
- Layout API (verified signatures):
  ```rust
  Layout::vertical<I>(constraints: I) -> Layout      // I: IntoIterator<Item: Into<Constraint>>
  Layout::horizontal<I>(constraints: I) -> Layout
  Layout::areas<const N: usize>(&self, area: Rect) -> [Rect; N]   // preferred: destructuring
  Layout::split(&self, area: Rect) -> Rc<[Rect]>                  // dynamic N
  Layout::spacing<T: Into<Spacing>>(self, spacing: T) -> Layout
  ```
  plus `.constraints()`, `.direction()`, `.margin()`, `.flex()`.
  `Constraint` variants: `Length`, `Percentage`, `Ratio`, `Fill`, `Min`, `Max`.
- Rendering surface: `Terminal::draw(|frame: &mut Frame| ...)`; inside, `frame.area()`,
  `frame.render_widget(w, rect)`, `frame.render_stateful_widget(w, rect, &mut state)`,
  `frame.set_cursor_position(...)`. `Widget` is implemented for `&T` on the built-in widgets in
  0.30, so `render_widget(&list, area)` avoids clones.
- `Modifier`, `Style`, `Color::Rgb` live in `ratatui::style`. `ratatui::prelude::*` pulls the common set.

### crossterm — 0.29.0

Do not add it. Take it from `ratatui::crossterm`. If a direct dependency ever becomes necessary
(e.g. for `event::EventStream`, which needs crossterm's `event-stream` feature), pin it to `0.29`
so it unifies with `ratatui-crossterm`'s default `crossterm_0_29`.

Backend choice: `CrosstermBackend<std::io::Stdout>` — the only sane one on macOS/Linux, and the one
`ratatui::init()` gives you.

### tokio — 1.53.1

Features needed: `rt-multi-thread`, `macros`, `process`, `time`, `sync`.

- `process` — `tokio::process::Command` for every `git`, `tmux`, and `gh` invocation. This is the
  direct analogue of the Python `asyncio.create_subprocess_exec` in `services/git.py:53`.
- `sync` — `mpsc::Sender<AppEvent>` / `Receiver<AppEvent>`, the spine of the event loop.
- `time` — the spinner tick and the 300 s GitHub-issues refresh interval
  (`app.py:126 self.set_interval(300, ...)`).
- `macros` — `#[tokio::main]`.
- `rt-multi-thread` — the Claude-session scanner walks `~/.claude/projects/<slug>/*.jsonl` with
  blocking `std::fs`; that goes in `spawn_blocking`, which wants a real runtime.
  `rt` (current-thread) would technically suffice; the multi-thread flag costs nothing and removes
  a class of "why did my UI stutter" bugs.

**No `fs` feature.** File I/O here is small and bursty; `std::fs` inside `spawn_blocking` is simpler
than async file handles and avoids a second I/O model in the codebase.

### serde 1.0.229 + serde_json 1.0.151

`serde` with `derive`. Both config files are plain JSON objects; `serde_json::to_writer_pretty` with
a 2-space indent matches Python's `json.dump(..., indent=2)`. See §"Config compatibility" for the
exact attributes needed.

### clap — 4.6.6, derive

```rust
#[derive(Parser)]
#[command(name = "forestui", version, about = "forestui - Git Worktree Manager")]
struct Cli {
    /// Optional path to forest directory (default: ~/forest)
    forest_path: Option<PathBuf>,
    #[arg(long = "no-self-update")] no_self_update: bool,
    #[arg(long)] debug: bool,
    #[arg(long)] dev: bool,
}
```

One-to-one with `cli.py:167-200`. `--debug` loses its meaning (there are no Textual devtools); keep
the flag as "write a debug log to `~/.forestui-debug.log`" or drop it — decide at implementation time.

### Text input for modals — **`tui-input` 0.15.4**

Recommendation: **tui-input**, and the version evidence is decisive.
`tui-input` 0.15.4 (published 2026-08-10) depends on `ratatui ^0.30.2` and `crossterm ^0.29.0` —
exactly our resolved versions — whereas `tui-textarea` 0.7.0 (last published 2024-10-22) is pinned to
`ratatui ^0.29.0` / `crossterm ^0.28`, so it cannot be used with ratatui 0.30 without a fork.
Beyond that, every text field in forestui is single-line (`Input` in `modals.py`, never `TextArea`),
which is precisely tui-input's scope; a hand-rolled `String` + cursor would re-implement its
unicode-segmentation/width handling for no gain.

API: `Input::default()` / `Input::new(String)`, `.value()`, `.reset()`, `.with_value()`, `.cursor()`,
`.visual_cursor()`, `.visual_scroll(width)`, `.handle_event(&crossterm_event)` via
`tui_input::backend::crossterm::EventHandler`. Default features (`ratatui-crossterm`) are what we want.

### Fuzzy branch search — **`nucleo-matcher` 0.3.1**

Recommendation: **nucleo-matcher**. `fuzzy-matcher` 0.3.7 was last published **2020-10-04** and is
effectively unmaintained; nucleo-matcher is the matcher extracted from Helix's picker, is
allocation-light, and ships the higher-level `pattern` API:

```rust
let mut matcher = Matcher::new(Config::DEFAULT.match_paths());   // match_paths(): branch names are path-like
let matches = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart)
    .match_list(branches, &mut matcher);                          // -> Vec<(T, u32)>, higher score = better
```

`Config::DEFAULT.match_paths()` is the right config: branch names are `origin/feat/foo`, and it
biases toward segment boundaries — which is exactly what `utils.py:_match_score` hand-codes with its
`/-_.` word-boundary bonus. Note the polarity flip: the Python scorer is **lower = better**, nucleo is
**higher = better**. See §"Behaviour to preserve" for what the port must keep.

### Git access — **shell out to the `git` binary via `tokio::process`**

Strong default: shell out. The port then produces byte-identical behaviour to `services/git.py`,
which is a thin wrapper over 15 `git` invocations (`worktree add/remove/repair/list --porcelain`,
`branch -a --format`, `rev-parse`, `log -1 --format`, `fetch`, `pull`, `remote`).

The honest trade-off, three lines:
1. `git2` (libgit2 0.21.0) or `gix` (0.86.0) give typed errors, no `String` parsing, and no fork/exec
   per call — real wins if forestui ever polls repo status on a timer.
2. Both cost you the user's actual git configuration: credential helpers, `includeIf`, hooks,
   `worktree.*` settings, SSH agent handling, and `git pull`'s merge/rebase policy are all
   reimplementations rather than the same code path, and `git2` drags a C build (or vendored libgit2)
   into a tool that people install with `cargo install`.
3. Since forestui delegates authenticated network operations (`fetch`, `pull`) and worktree surgery
   to git, and the user's own config must apply, shelling out is not a shortcut — it is the correct
   semantics. Revisit only if per-call latency shows up in a profile.

### tmux access — **shell out to `tmux`**

There is no Rust equivalent of `libtmux` worth taking on. Every `libtmux` call in `services/tmux.py`
is already a thin wrapper over a tmux control command, and several places bypass the object model
entirely (`server.cmd("display-message", "-p", "#{session_group}")`, `list-clients -F ...`). Port
them as `tmux` argv:

| Python (libtmux) | tmux argv |
|---|---|
| `Server()` presence | `std::env::var("TMUX").is_ok()` |
| `server.cmd("display-message","-p","#{session_group}")` | same, verbatim |
| `server.cmd("list-clients","-F","#{client_activity} #{session_id} #{session_group}")` | same, verbatim |
| walk `sessions/windows/panes` to find `TMUX_PANE` | `tmux list-panes -a -F "#{pane_id} #{window_id} #{session_id}"` |
| `window.rename_window(n)` | `tmux rename-window -t <window_id> <n>` |
| `session.new_window(name, start_directory, attach, window_shell)` | `tmux new-window -t <session_id> -n <name> -c <dir> [cmd]` |
| `window.select()` | `tmux select-window -t <window_id>` |
| `server.cmd("set-option","-g","focus-events","on")` | same, verbatim |

These are fast and synchronous-ish; run them through `tokio::process::Command` anyway so the loop
never blocks. The bootstrap `exec` path in `cli.py` (`os.execvp`) becomes
`std::os::unix::process::CommandExt::exec()` — same semantics, replaces the process image.

### Error handling — **anyhow only**

Pick one: **anyhow, everywhere, with `.context()`**. `thiserror` earns its place when callers must
*match* on error variants. They don't here: the Python `GitError` is only ever used as
`except GitError: pass` or "put the string in a toast" (`app.py:412`, `app.py:449`, `app.py:539`).
A `Result<T>` plus `e.to_string()` reproduces that exactly. Skip `thiserror`; add it later if a
service ever grows a variant the UI needs to branch on.

Panic policy: `ratatui::init()` installs a restoring panic hook, and `main` mirrors
`app.py:run_app()` by writing the panic/error to `~/.forestui-error.log`.

### Relative timestamps — **`timeago` 0.6.1**

Replaces `humanize.naturaltime` (used at `models.py:ClaudeSession.relative_time`,
`models.py:GitHubIssue.relative_time`, `repository_detail.py:117`, `worktree_detail.py:131`).

`timeago` 0.6.1 was published **2026-07-02** and is actively maintained; `chrono-humanize` 0.2.3 has
not been touched since **2023-07-22**. Both accept `chrono 0.4`. With
`features = ["chrono"]`, `timeago::Formatter::new().convert_chrono(then, Utc::now())` yields
`"2 hours ago"`, matching Python's output shape. `chrono-humanize`'s
`HumanTime::from(dt).to_text_en(Accuracy::Rough, Tense::Past)` is marginally closer to humanize's
`"a minute ago"` phrasing — if exact string parity ever matters, swap; otherwise take the
maintained crate.

### Testing — `TestBackend` + `insta`

- `ratatui::backend::TestBackend` is a **first-class, non-feature-gated** re-export in
  `ratatui::backend` (0.30.2). `Terminal::new(TestBackend::new(w, h))`, draw, then assert on
  `terminal.backend()` — `TestBackend` implements `Display`, so it snapshots as an ASCII picture of
  the screen.
- `insta` 1.48.0 for the snapshots: `insta::assert_snapshot!(terminal.backend())`, reviewed with
  `cargo insta review`. This is the only mechanical defence against the class of bug the project's
  `CLAUDE.md` calls out ("lint and typecheck alone cannot catch visual bugs").
- `tempfile` 3.27.0 for config round-trip tests against a scratch forest directory.
- Kept out: `pretty_assertions` (insta covers the diffs), `rstest` (table tests are `for` loops).

### MSRV / edition

**`edition = "2024"`, `rust-version = "1.88"`.**

`ratatui` 0.30.2 declares `edition 2024` / `rust_version 1.88.0`, and `ratatui-crossterm` 0.1.2 the
same, so 1.88 is a floor we do not get to choose. Edition 2024 requires ≥1.85, so it is free.
No Rust toolchain is installed on this machine (`cargo --version` and `rustc --version` both return
"command not found"), so pin the toolchain explicitly:

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.88"
components = ["rustfmt", "clippy"]
```

One consequence worth planning for: **`std::env::home_dir()` was un-deprecated in Rust 1.87**, which
is below our floor — so `dirs`/`directories` is not needed for `~/forest`, `~/.config/forestui`, and
`~/.claude/projects`. Stdlib does it.

---

## Part 2 — Architecture

### The shape of the problem

Textual is declarative and retained-mode: widgets own state, CSS owns layout, `Message` objects
bubble up a widget tree, and `@work` coroutines mutate widgets after the fact
(`detail.update_issues(issues)` at `app.py:160`). ratatui is immediate-mode: there is no widget tree
between frames, so **all** of that state has to become explicit and live in one place.

The port is therefore three mechanical substitutions:

| Textual | Rust |
|---|---|
| widget instance fields | fields on `App` |
| `Message` subclass + `on_*` handler | `AppEvent` variant + arm in `App::update` |
| `@work async def` | `tokio::spawn` + `tx.send(AppEvent::…)` |
| `compose()` / CSS | `ui::draw(frame, &app)` + `Layout`/`Constraint` |
| `push_screen` / `push_screen_wait` | `app.modals.push(Modal::…)` / `PendingAction` on the modal |

### Crate layout — 16 files

```
src/
  main.rs               ~110   #[tokio::main]; tmux bootstrap, ratatui::init/restore, error log, run loop kick-off
  cli.rs                ~190   clap Parser; ensure_tmux() session/window/grouped-session bootstrap + exec()
  app.rs                ~470   App struct, Focus, Modal stack, key dispatch, App::update(AppEvent), task spawning
  event.rs              ~110   AppEvent enum, terminal reader thread, tick task, mpsc plumbing
  model.rs              ~280   serde types: Repository, Worktree, Settings, CustomClaudeButton,
                               ClaudeSession, GitHubIssue + validators + derive_prefix
  config.rs             ~140   load/save .forestui-config.json and ~/.config/forestui/settings.json
  fuzzy.rs              ~110   nucleo-backed branch matching + match highlight spans
  ui/
    mod.rs              ~180   colour palette consts + draw(frame,&App): frame split, header/footer, toasts
    sidebar.rs          ~150   repository/worktree tree pane
    detail.rs           ~360   repository + worktree detail panes; detail_actions() action list
    modal.rs            ~400   render + key handling for the seven modals
  services/
    mod.rs               ~10   pub mod re-exports
    git.rs              ~310   tokio::process git wrapper (1:1 with services/git.py)
    tmux.rs             ~300   tmux argv wrapper (replaces libtmux)
    github.rs           ~210   gh CLI wrapper + 300 s issue cache
    claude.rs           ~190   ~/.claude/projects JSONL scanner + session migration
tests/
  config_compat.rs       ~90   round-trip against fixtures written by the Python build
  render.rs             ~120   TestBackend + insta snapshots
```

> **Layout update (2026-08-15).** The single `app.rs` proposed here grew to
> 2,600 lines and was split in #34 into `src/app/{mod,detail,keys,mouse,actions}.rs`,
> with `app/detail.rs` holding the content walk that both `detail_items()` and
> the renderer derive from. Line references to `src/app.rs` elsewhere in these
> documents predate that split.

≈3.6k lines, versus 6.3k Python. The saving is almost entirely `theme.py` (696 lines of CSS →
~40 lines of `Color::Rgb` consts) and the disappearance of per-widget message plumbing.

**Deliberately not created:** `ui/theme.rs` (40 lines of consts belong at the top of `ui/mod.rs`),
`state.rs` (the Python `AppState` singleton is just fields on `App` plus `config.rs`),
`components/messages.rs` (nine `Message` classes collapse into `AppEvent` variants),
`error.rs` (anyhow).

### Event loop

Two producers, one consumer, one channel. No `tokio::select!`, no `EventStream`, no `futures`.

```rust
// event.rs
pub enum AppEvent {
    Term(ratatui::crossterm::event::Event),      // key / resize / focus-gained / mouse
    Tick,                                         // 100 ms — spinner frames only
    RefreshIssuesTimer,                           // 300 s — mirrors app.py:126
    Sessions   { path: String, sessions: Vec<ClaudeSession> },
    Issues     { repo_path: String, issues: Result<Vec<GitHubIssue>, String> },
    GhStatus   { status: GhStatus, user: Option<String> },
    RepoDetail { path: String, branch: String, commit: Option<CommitInfo>, has_remote: bool },
    Branches   { repo_path: String, branches: Vec<String>, remotes: Vec<String>, after_fetch: bool },
    OpDone     { what: &'static str, result: Result<(), String> },  // pull / create / remove / rename
    Toast      { text: String, severity: Severity },
}

pub fn spawn_producers(tx: mpsc::Sender<AppEvent>) {
    let t = tx.clone();
    std::thread::spawn(move || {                       // blocking crossterm reader
        while let Ok(ev) = ratatui::crossterm::event::read() {
            if t.blocking_send(AppEvent::Term(ev)).is_err() { break; }
        }
    });
    let t = tx.clone();
    tokio::spawn(async move {                          // spinner tick
        let mut i = interval(Duration::from_millis(100));
        loop { i.tick().await; if t.send(AppEvent::Tick).await.is_err() { break; } }
    });
    tokio::spawn(async move {                          // periodic issue refresh
        let mut i = interval(Duration::from_secs(300));
        i.tick().await;                                // interval fires immediately; discard
        loop { i.tick().await; if tx.send(AppEvent::RefreshIssuesTimer).await.is_err() { break; } }
    });
}
```

```rust
// main.rs
let mut terminal = ratatui::init();
let (tx, mut rx) = mpsc::channel::<AppEvent>(64);
event::spawn_producers(tx.clone());
let mut app = App::new(forest_path, tx.clone())?;
app.bootstrap();                                       // == on_mount: gh status, initial detail fetch
while !app.should_quit {
    terminal.draw(|f| ui::draw(f, &app))?;
    let Some(ev) = rx.recv().await else { break };
    app.update(ev);
}
ratatui::restore();
```

Why a reader thread instead of `crossterm::event::EventStream` + `tokio::select!`: EventStream needs
crossterm as a *direct* dependency with the `event-stream` feature (re-introducing the version-skew
risk the ratatui re-export avoids), pulls in `futures`, and buys nothing — a blocking `read()` on a
dedicated thread feeding the same channel gives one uniform `recv()` and one redraw site. The thread
parks in `read()` at shutdown and dies with the process.

**A `@work` background op maps to exactly this shape:**

```python
# app.py:149  — Textual
@work
async def _fetch_issues_for_repo(self, repo_path: str) -> None:
    issues = await self._github_service.list_issues(repo_path)
    self.query_one(RepositoryDetail).update_issues(issues)
```

```rust
// app.rs — Rust
fn spawn_fetch_issues(&self, repo_path: String) {
    let tx = self.tx.clone();
    tokio::spawn(async move {
        let issues = services::github::list_issues(&repo_path).await.map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::Issues { repo_path, issues }).await;
    });
}
```

The `query_one(...)` / `except: pass` dance at `app.py:158-162` ("detail pane changed, ignore")
becomes a guard in `App::update`: drop the result if `self.selection` no longer points at
`repo_path`. Same intent, but checkable rather than swallowed.

Redraw cadence: one draw per received event, so idle cost is the 10 Hz tick. If that ever matters,
gate on `app.dirty` and let `Tick` set it only while a spinner is running.

### State model

One struct. No `Rc<RefCell<…>>`, no service singletons (the Python `_instance` singletons exist to
share a cache; put the cache in `App` or pass it in).

```rust
pub struct App {
    // persisted
    pub repos: Vec<Repository>,          // .forestui-config.json
    pub settings: Settings,              // ~/.config/forestui/settings.json
    pub forest_path: PathBuf,            // CLI arg, never persisted (matches services/settings.py)

    // selection & focus
    pub selection: Selection,            // { repo: Option<Uuid>, worktree: Option<Uuid> }
    pub focus: Focus,
    pub sidebar_cursor: usize,           // index into the flattened sidebar row list
    pub detail_cursor: usize,            // index into detail_actions(&self)
    pub detail_scroll: u16,
    pub show_archived: bool,

    // modal stack — Settings -> CustomButtons -> EditButton nests three deep in the Python
    pub modals: Vec<Modal>,

    // async-filled panes
    pub detail: DetailData,              // branch/commit/has_remote for the selected item
    pub sessions: Vec<ClaudeSession>,
    pub issues: Vec<GitHubIssue>,
    pub issues_loading: bool,
    pub gh_status: GhStatus,
    pub spinner: usize,                  // "|/-\\" frame index, advanced by AppEvent::Tick
    pub toast: Option<(String, Severity, Instant)>,

    pub tx: mpsc::Sender<AppEvent>,
    pub should_quit: bool,
}

pub enum Focus { Sidebar, Detail }       // modals capture input while `modals` is non-empty
```

**Focus** is an explicit two-state enum, matching what the Textual app actually offers (tree on the
left, actions on the right); `Tab` toggles. When `modals` is non-empty the topmost modal owns every
key and `Focus` is ignored — that is the whole of Textual's `ModalScreen` semantics.

**Modal stack** is a `Vec<Modal>`, not `Option<Modal>`, because the Python genuinely nests:
`SettingsModal` → `CustomButtonsModal` → `CustomButtonEditModal`
(`modals.py:452`, `modals.py:984`). Each variant owns its own form state, including its
`tui_input::Input` fields and a `field: usize` cursor.

```rust
pub enum Modal {
    AddRepository(AddRepoForm),
    AddWorktree(AddWorktreeForm),
    CreateFromIssue(IssueForm),
    Settings(SettingsForm),
    CustomButtons(ButtonsForm),
    EditButton(EditButtonForm),
    Confirm { title: String, body: String, on_yes: PendingAction },
}

pub enum PendingAction {                 // replaces `await push_screen_wait(ConfirmDeleteModal(...))`
    RemoveRepository(Uuid),
    DeleteWorktree { repo: Uuid, worktree: Uuid },
}
```

`push_screen_wait` is an `await` on a modal result. In a single-loop design there is nothing to
await: the confirm modal carries the action it will trigger, and popping it with "yes" runs
`app.apply(pending)`. Child modals that return a value (`CustomButtonEditModal` → a
`CustomClaudeButton`) write straight into the parent frame beneath them on the stack — the parent is
still sitting in `modals[len-2]`.

**The immediate-mode trick for buttons.** Textual gives every `Button` an id and routes
`on_button_pressed` by string match (`repository_detail.py:230`, `worktree_detail.py:275`). ratatui
has no buttons. Instead, one pure function builds the list of actionable rows, and both the renderer
and the key handler consume it:

```rust
// ui/detail.rs
pub enum Action {
    OpenEditor, OpenTerminal, OpenFiles,
    ClaudeNew, ClaudeYolo, ClaudeCustom(usize),
    ResumeSession(String), ResumeYolo(String), ResumeCustom(usize, String),
    Sync, AddWorktree, RemoveRepo, Archive, Unarchive, Delete,
    CreateFromIssue(u64), RefreshIssues,
}
pub fn detail_actions(app: &App) -> Vec<(Action, Rect)>;   // built during layout, cached per frame
```

Render highlights `detail_cursor`; `Enter` dispatches `actions[detail_cursor]`. Mouse clicks (which
Textual gave for free) map by hit-testing the `Rect`s. This kills the entire string-id parsing block
(`btn-custom-<prefix>-<session_id>` at `repository_detail.py:267-278`) — the id **is** the enum.

### CSS → Layout mapping

`theme.py`'s 696 lines are two things: a colour palette and a box model. Both are small in ratatui.

| Textual CSS | ratatui |
|---|---|
| `#main-container { layout: horizontal }` | `Layout::horizontal([...]).areas::<2>(body)` |
| `#sidebar { width: 35; min-width: 30; max-width: 45 }` | `Constraint::Length(35)` (clamp if you add a resize key) |
| `#detail-pane { width: 1fr }` | `Constraint::Fill(1)` |
| `Header` / `Footer` | `Layout::vertical([Length(1), Fill(1), Length(1)]).areas::<3>(frame.area())` |
| `height: 3` (button rows) | `Constraint::Length(3)` |
| `height: auto` | compute the row count in the render fn and pass `Length(n)` |
| `dock: right` | last chunk of a `Layout::horizontal` |
| `VerticalScroll` | `app.detail_scroll: u16` + render a sub-slice; `Scrollbar` widget for the gutter |
| `.modal-container { width: 80; max-width: 90%; height: auto; max-height: 90% }` | `centered_rect(80, h, area)` helper, then `frame.render_widget(Clear, r)` before the modal body |
| `border-right: solid $border` | `Block::new().borders(Borders::RIGHT).border_style(BORDER)` |
| `$accent: #52B788` etc. | `const ACCENT: Color = Color::Rgb(0x52, 0xB7, 0x88);` — 14 consts total, top of `ui/mod.rs` |
| `variant="error"` / `.-destructive` | `Style::new().fg(DESTRUCTIVE)` chosen by the `Action`, not by a class string |
| `Button { min-width: 10 }` | pad the label to 10 columns when laying the row out |

The `theme` **setting** (`system` / `dark` / `light`) stays in the JSON for compatibility; the Python
app already ignores it beyond storing it. Wire it to two palettes later, or don't.

### Textual `Message` → `AppEvent`

`components/messages.py`'s nine classes, plus the per-widget nested ones, collapse. The distinction
Textual needs — "who sent this?" — is irrelevant when there is one `App`.

| Textual message | Rust |
|---|---|
| `OpenInEditor(path)`, `OpenInTerminal`, `OpenInFileManager` | `Action::OpenEditor` / `OpenTerminal` / `OpenFiles` — synchronous, handled in `App::update`, no channel round trip |
| `StartClaudeSession`, `StartClaudeYoloSession`, `StartClaudeCustomSession` | `Action::ClaudeNew` / `ClaudeYolo` / `ClaudeCustom(i)` |
| `ContinueClaude*Session(session_id, path[, button])` | `Action::ResumeSession(id)` / `ResumeYolo(id)` / `ResumeCustom(i, id)` |
| `Sidebar.RepositorySelected` / `.WorktreeSelected` | direct mutation of `app.selection` in the key handler, then `app.spawn_detail_fetch()` |
| `Sidebar.AddRepositoryRequested`, `.AddWorktreeRequested` | `app.modals.push(Modal::AddRepository(..))` |
| `RepositoryDetail.SyncRequested` / `.RemoveRepositoryRequested` / `.RefreshIssuesRequested` / `.CreateWorktreeFromIssue` | `Action::Sync` / `RemoveRepo` / `RefreshIssues` / `CreateFromIssue(n)` |
| `WorktreeDetail.Archive/Unarchive/Delete/Rename*/SyncRequested` | `Action::Archive` / `Unarchive` / `Delete` / rename via the detail-pane inputs |
| `*Modal.WorktreeCreated` / `.RepositoryAdded` / `.FetchRequested` | handled when the modal pops; `FetchRequested` spawns and returns `AppEvent::Branches { after_fetch: true }` |
| `BranchSearchInput.Changed` / `.BranchSelected` | local to the modal's form state; no event needed |

Rule of thumb: **`AppEvent` carries only things that cross the async boundary.** Anything a key press
can do immediately is a direct method call on `App`. That keeps the enum at ~10 variants instead of
the 25-odd Textual message classes.

### Rendering

One entry point, one function per pane, all taking `&App` (never `&mut`) so rendering cannot mutate
state:

```rust
// ui/mod.rs
pub fn draw(f: &mut Frame, app: &App) {
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(f.area());
    let [side, detail] =
        Layout::horizontal([Constraint::Length(35), Constraint::Fill(1)]).areas(body);

    draw_header(f, header, app);
    sidebar::draw(f, side, app);
    match app.selection.kind() {
        SelectionKind::Worktree   => detail::draw_worktree(f, detail, app),
        SelectionKind::Repository => detail::draw_repository(f, detail, app),
        SelectionKind::None       => detail::draw_empty(f, detail),
    }
    draw_footer(f, footer, app);                 // key hints — the Textual Footer
    if let Some(m) = app.modals.last() { modal::draw(f, f.area(), app, m); }   // Clear + centered rect
    if let Some(t) = &app.toast { draw_toast(f, f.area(), t); }                // == self.notify(...)
}
```

Only the topmost modal renders — Textual dims the stack beneath; if you want that, render the
lower modals first with a dimmed style. Not worth it for a first cut.

### Python → Rust module map

| Python module | LoC | Rust module | Notes |
|---|---:|---|---|
| `forestui/app.py` | 1050 | `src/app.rs` + `src/main.rs` | `ForestApp` → `App`; the 40 `on_*` handlers become `App::update` arms plus `Action` dispatch. `_auto_update` (`uv tool upgrade forestui`) has no meaning for a cargo-installed binary — either drop it or shell `cargo install forestui --force`; decide before porting, don't port it blindly. `run_app()`'s `~/.forestui-error.log` behaviour moves to `main.rs`. |
| `forestui/cli.py` | 219 | `src/cli.rs` | clap derive + `ensure_tmux()`. The grouped-session logic (`new-session -t =name`, `set-hook client-attached`, `status-left` `#S` rewrite, `os.execvp`) ports verbatim as `tmux` argv; `os.execvp` → `CommandExt::exec()`. |
| `forestui/models.py` | 258 | `src/model.rs` | Pydantic → serde. Validators (`validate_button_label/prefix/claude_command`, `derive_prefix`) become free functions returning `Result<(), String>` — same messages, so the modal error text is unchanged. `is_yolo_style`, `branch_name`, `relative_time` become methods. |
| `forestui/state.py` | 221 | `src/app.rs` + `src/config.rs` | `AppState` singleton dissolves: the `Vec<Repository>` lives on `App`, the `_save_state()` calls become one `config::save_state(&app)` after each mutation. Keep the save-on-every-mutation discipline — it is what makes the file always current. |
| `forestui/services/settings.py` | 84 | `src/config.rs` | `set_forest_path`/`get_forest_path` globals become an `App.forest_path` field passed down. `SettingsService` singleton → `config::load_settings()` / `save_settings()`. |
| `forestui/services/git.py` | 355 | `src/services/git.rs` | 1:1. `_run_git` → `async fn run_git(args, cwd) -> Result<(i32, String, String)>`. Keep the `--porcelain` parser for `worktree list` and the `%H\|%h\|%ct` log format verbatim. |
| `forestui/services/tmux.py` | 370 | `src/services/tmux.rs` | libtmux → argv (table above). The "most recently active client in our session group" heuristic (`tmux.py:56-107`) is subtle and load-bearing for multi-terminal use — port it exactly, including the two fallbacks. |
| `forestui/services/github.py` | 242 | `src/services/github.rs` | `gh` argv unchanged, same `--json` field list, same 300 s TTL cache keyed `owner/repo`, same de-dup across the `--assignee @me` and `--author @me` passes. |
| `forestui/services/claude_session.py` | 179 | `src/services/claude.rs` | JSONL scan under `~/.claude/projects/<path-with-slashes-replaced-by-dashes>/`. Run it in `spawn_blocking`. Keep the skip rules (`agent-*.jsonl`, `content.startswith("<")`, `message_count == 0` → drop) and the 100-char truncation. |
| `forestui/utils.py` | 173 | `src/fuzzy.rs` | Hand-rolled Levenshtein → nucleo. `strip_remote_prefix` stays (it needs the real remote list). `highlight_match` → build a `ratatui::text::Line` with a reversed-bold span. |
| `forestui/components/sidebar.py` | 255 | `src/ui/sidebar.rs` | `Tree` widget → a flattened `Vec<SidebarRow>` + `List`/`ListState`, or manual rows. Expansion state becomes `Repository`-keyed `HashSet<Uuid>` on `App`. The "smart collapse" rule (`sidebar.py:187-192`) is worth keeping. |
| `forestui/components/repository_detail.py` | 458 | `src/ui/detail.rs` | Merged with the worktree view — the two share ~70% of their sections (LOCATION / OPEN IN / CLAUDE / RECENT SESSIONS). Write the shared section renderers once. |
| `forestui/components/worktree_detail.py` | 424 | `src/ui/detail.rs` | ditto; the worktree-only bits are the MISSING banner, Based-on line, RENAME inputs, and Archive/Delete. |
| `forestui/components/modals.py` | 1011 | `src/ui/modal.rs` | Seven `ModalScreen`s → seven `Modal` variants. Each keeps its own `tui_input::Input`s and validation, rendered over `Clear`. |
| `forestui/components/branch_search.py` | 184 | `src/ui/modal.rs` + `src/fuzzy.rs` | The dropdown is a `List` under the input inside whichever modal owns it; `FuzzyBranchSuggester` (inline ghost-text completion) becomes "render the top match dimmed after the cursor". |
| `forestui/components/messages.py` | 82 | `src/event.rs` + `ui/detail.rs::Action` | Split by boundary: async results → `AppEvent`, synchronous intents → `Action`. |
| `forestui/theme.py` | 696 | `src/ui/mod.rs` (top) | 14 colour consts + a handful of `Style` helpers. This is where the 2.7k-line saving comes from. |
| `forestui/__init__.py`, `__main__.py` | 15 | `src/main.rs` | version comes from `env!("CARGO_PKG_VERSION")`. |

### Config compatibility — non-negotiable

Both files keep their **exact** paths, names, and schemas so the Python and Rust builds can share a
forest directory. Verified against the live files on this machine.

**`<forest>/.forestui-config.json`** — note the sibling `.forest-config.json` (the macOS `forest`
app) must be left alone:

```json
{
  "repositories": [
    { "id": "976ebd00-af5e-4b9a-9fa6-d7a6ed0daeaa",
      "name": "onlypro",
      "source_path": "/Users/kirill/Personal/projects/onlypro",
      "worktrees": [] }
  ]
}
```

**`~/.config/forestui/settings.json`**:

```json
{ "default_editor": "pycharm", "default_terminal": "", "branch_prefix": "ft-", "theme": "dark" }
```

Rules the Rust types must follow:

1. **snake_case field names, verbatim.** No `#[serde(rename_all)]`. Field-for-field:
   `Worktree { id, name, branch, path, is_archived, sort_order, last_modified, base_branch, created_from_ref }`.
2. **`#[serde(default)]` on every field that Pydantic defaults.** The live `settings.json` above has
   **no `custom_buttons` key** — it was written before the field existed. A missing key must not be a
   parse error, exactly as Pydantic tolerates it.
3. **Never fail closed on a bad file.** `state.py:_load_state` and `settings.py:_load_settings` both
   swallow `JSONDecodeError`/`OSError` and fall back to defaults. Match that: log, use defaults,
   keep running. Do *not* overwrite an unparseable file until the user makes a change.
4. **`Uuid` with `features = ["serde"]`** — serialises as the hyphenated lowercase string Pydantic
   produces. New worktrees use `Uuid::new_v4()` (`features = ["v4"]`).
5. **`last_modified` is a `chrono::DateTime<Utc>`.** Pydantic's `model_dump(mode="json")` emits
   RFC 3339; chrono's serde impl emits RFC 3339 and parses both `Z` and `+00:00`. Round-trips cleanly
   in both directions.
6. **`sort_order`, `base_branch`, `created_from_ref` are `Option<T>`** and must serialise as
   `null`, not be omitted — Pydantic writes the keys. Do **not** add `skip_serializing_if`.
7. **2-space pretty JSON**, matching `json.dump(..., indent=2)`, so switching builds does not
   produce a spurious diff if the file is ever version-controlled.
8. **Write atomically** (temp file in the same directory + `rename`) — an improvement over the
   Python, and free.

A `tests/config_compat.rs` fixture pair (one file written by the Python build, one by the Rust
build) asserting `serde_json::Value` equality after a round trip is the check that keeps this true.

### Behaviour to preserve (easy to lose in a port)

- **Fuzzy-match ordering.** `utils.py` scores in explicit tiers (exact 0.0, exact-local 0.5, prefix
  1.0/1.5, word-boundary substring 2.0, substring 3.0, Levenshtein 4.0+), lower is better, ties
  broken by lowercased name, capped at 50 results. nucleo's score has the opposite polarity and a
  different curve. Port the *tiers* as a pre-pass (exact / prefix / substring on both the full name
  and the remote-stripped name) and fall through to nucleo only for the fuzzy tail. The unit tests
  from `utils.py` transfer directly and are the acceptance criterion.
- **Empty query returns the first 50 branches in list order**, not an empty list (`utils.py:140`).
- **Window naming.** `edit:<n>`, `term:<n>`, `files:<n>`, `claude:<n>`, `yolo:<n>`,
  `<custom_prefix>:<n>`, where `<n>` is `repo:worktree` or bare `repo` (`app.py:879-889`), plus the
  `:2`, `:3` uniqueness suffix (`tmux.py:277-301`). Editor windows are *reused* if they exist;
  terminal/files/claude windows are *always new*. Snapshot-test this — the project's own testing
  note calls out "wrong window names" as a real regression class.
- **Claude command construction.** `$SHELL -ic <quoted cmd>` (`tmux.py:352`) so shell aliases work,
  with `shlex.quote` on the command. The Rust build keeps the quoting and drops the wrapper: the
  window runs the same interactive, non-login shell, but reads the command from a startup file
  forestui generates for it (`ZDOTDIR` / `--rcfile` / `ENV`), under `set -m`. Aliases still
  resolve and Claude is a job the shell can be handed back from. A window whose own process is
  Claude has nothing underneath it — Ctrl-C leaves a dead pane and Claude's Ctrl-Z suspend strands
  the session with no shell to `fg` it.
- **YOLO flag suppression for custom buttons.** `--dangerously-skip-permissions` is appended only
  for the built-in YOLO button, never for a custom one (`tmux.py:345`), even though a custom command
  may contain the flag itself (which is what turns the button red).
- **Stale-worktree tolerance.** `git.py:_run_git` raises when `cwd` no longer exists; the UI then
  shows the "⚠ MISSING: directory no longer exists on disk" banner and disables Sync
  (`worktree_detail.py:117`, `:141`). In Rust, `Command::current_dir` on a missing directory fails at
  spawn — catch it and set the same flag.

### Testing strategy

**1. Pure-logic unit tests** (`#[cfg(test)]` next to the code) — the bulk, and the cheapest:

- `fuzzy.rs`: the entire tier table from `utils.py`, empty-query behaviour, `strip_remote_prefix`
  against a real remote list, 50-result cap.
- `model.rs`: `derive_prefix`, `validate_button_label/prefix/claude_command` (message text included),
  `is_yolo_style`, `GitHubIssue::branch_name` slugging.
- `app.rs`: `active_worktrees()` ordering (`sort_order` ascending with `None` last, then
  `last_modified` descending — `models.py:145`), `reorder_worktree` index math, selection
  invalidation after a remove (`state.py:remove_repository`).
- `services/git.rs`: the `worktree list --porcelain` parser and the `%H|%h|%ct` log parser, fed
  captured fixture output. No subprocess needed — split parsing from invocation so the parser is a
  pure function over `&str`. Do the same in `tmux.rs` (`list-clients -F` output) and `github.rs`
  (`gh --json` payloads) and `claude.rs` (a fixture `.jsonl`).
- `config.rs`: round-trip a fixture written by the Python build; assert `Value` equality both ways.

**2. `TestBackend` render snapshots** (`tests/render.rs`) — the regression net for layout:

```rust
let mut terminal = Terminal::new(TestBackend::new(120, 40))?;
let app = App::fixture_with_two_repos();
terminal.draw(|f| ui::draw(f, &app))?;
insta::assert_snapshot!("sidebar_two_repos", terminal.backend());
```

Snapshot at minimum: empty state; repo selected with sessions loading; repo selected with sessions
and issues loaded; worktree selected with a missing directory; each of the seven modals; the
detail-cursor highlight on the first and last action. Also snapshot at 80×24 to catch overflow at the
smallest sane terminal — Textual's `max-width: 90%` did that for free and ratatui will not.

**3. `tu`-driven end-to-end** — the existing `test-forestui` skill, unchanged in spirit: launch the
real binary in a headless terminal inside tmux, drive keys, screenshot. This is the only layer that
covers what snapshots cannot: the tmux bootstrap/exec path, window creation and naming, editor
launch, and the alternate-screen enter/restore cycle. Keep the same proactive rule the project
already has — run it when fixing behavioural bugs, not only when asked.

**Not doing:** mocking `git`/`tmux`/`gh`. Split each service into `run_x()` (I/O) and `parse_x()`
(pure), unit-test the parsers, and let layer 3 cover the invocations. A mock subprocess layer is more
code than the thing it tests.

---

## Open decisions

1. **Self-update.** `app.py:182` shells `uv tool upgrade forestui`. A cargo-installed binary has no
   equivalent worth having. Recommend: drop it, print the latest version in the header if a cheap
   check is wanted later.
2. **`--debug`.** Textual devtools have no counterpart. Recommend: repurpose as a file log or delete.
3. **`theme` setting.** Stored and ignored today. Keep storing it; wire two palettes only if asked.
4. **Sidebar tree widget.** ratatui has no built-in tree. A flattened `Vec<SidebarRow>` + `List` is
   ~60 lines and avoids a dependency (`tui-tree-widget`); recommend the flat list.
