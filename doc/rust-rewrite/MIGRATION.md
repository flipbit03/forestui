# forestui — Python/Textual → Rust/ratatui migration plan

Companion documents in this directory:

| Doc | Role |
|---|---|
| `ARCHITECTURE.md` | Crate/module design, verified dependency versions, CSS→Layout mapping |
| `SPEC.md` | Implementation-grade behavioural spec of the **Python** build (2 200+ lines, present and complete — sections 1–10) |
| `TU_USECASES.md` | 52-case `tu` acceptance playbook, 25 of them P0 |
| `MIGRATION.md` | This document: what actually changed, what is left, how to roll back |

**Verification status of this document.** Every claim about the Rust side is derived from reading
the files on disk at commit `3021d72` + working tree, on 2026-08-14. **No Rust toolchain is
installed on this machine** (`cargo --version` and `rustc --version` both return "command not
found"), so nothing here has been compiled, tested, or run. Statements about compilation success,
runtime behaviour, and render output are therefore *unverified by execution* and are flagged where
they matter. The Python-side claims are backed by `SPEC.md` and by the 35 use cases in
`TU_USECASES.md` that were driven live against Python `0.0.0`.

---

## 1. Summary

forestui is being replaced, not extended: the Python/Textual package is deleted and a Rust/ratatui
binary becomes the app, with git history and the last PyPI release (`forestui 1.3.1`, verified on
PyPI 2026-08-14) as the only surviving copies of the old build. The port keeps every external
command byte-identical — same `git` argv, same `gh --json` field list, same tmux window names — and
keeps both on-disk config files at identical paths with identical JSON schemas, so a user can flip
between builds inside one forest directory. The user-visible interaction model changes: ratatui has
no widgets and no mouse wiring, so the detail pane's buttons become an index into a flat
`App::detail_items()` list (`src/app.rs:324-381`), `Tab` switches between sidebar and detail, and
`Enter` fires the focused item. Five Python behaviours that were bugs are deliberately fixed rather
than reproduced — the invisible empty state, the markup-eaten sidebar branch name, the unreachable
`show_archived` flag, hotkeys firing on a deleted worktree directory, and `ensure_tmux` re-execing
the bare name `forestui` off `PATH`. Distribution moves from PyPI/`uv tool install` to crates.io
plus prebuilt release tarballs; `install.sh`, `Makefile`, `.github/workflows/check.yml`, `README.md`,
`CLAUDE.md`, `.gitignore` and `.pre-commit-config.yaml` are already rewritten in the working tree,
`.github/workflows/release.yml` is new, `publish.yml` (PyPI) is deleted, and the Python package
itself is the last thing still standing.

---

## 2. Why Rust + ratatui — and what it costs

### Gained

| Gain | Evidence |
|---|---|
| Single static binary, no interpreter | `Cargo.toml:38-41` (`strip`, `lto`, `codegen-units = 1`); `install.sh` ships a `.tar.gz` with one file |
| No Python runtime, no `uv`, no venv resolution at launch | Python required `>=3.14` plus 5 runtime deps (`pyproject.toml:19-25`) |
| Startup cost is process exec, not import of Textual + Pydantic + libtmux | unverified by measurement — no toolchain here to time it |
| Explicit state: one `App` struct, one event channel | `src/app.rs:104-135`, `src/event.rs:44-84` |
| Async results are checkable, not swallowed | Python dropped stale results with `except Exception: pass` (`app.py:158-162`, `:174-175`); Rust guards on the path (`src/app.rs:503-513`) |
| Compile-time exhaustiveness over button routing | the `btn-custom-<prefix>-<session_id>` string parser (`repository_detail.py:267-278`, `worktree_detail.py:307-318`) becomes `Action::ResumeCustom { button, session }` (`src/app.rs:75`) |
| Pure, unit-testable parsers | `parse_worktree_porcelain` (`src/services/git.rs:268`), `unique_name_among` (`src/services/tmux.rs:156`), `claude_shell_command` (`src/services/tmux.rs:219`) are pure and already have tests |

### Lost

| Loss | Consequence |
|---|---|
| Textual's CSS layout engine | 696 lines of `theme.py` become 89 lines of `src/theme.rs`, but every `height: auto`, `dock`, `max-width: 90%` and wrap rule is now hand-computed in the render functions. Overflow at 80×24 is no longer handled for free. |
| Mouse-first buttons | Textual routed clicks for free. ratatui gets no mouse capture here (`ratatui::init()` at `src/main.rs:50` does not enable mouse reporting). Six acceptance cases use `tu mouse click` (UC-18, 32, 33, 34, 37, 38) and must be rewritten as key sequences. |
| Retained-mode widgets holding their own state | Every scroll offset, cursor, and form field is now a field on `App` or on a `Modal` variant. `TextInput` is hand-rolled (`src/ui/widgets.rs:16-152`) instead of Textual's `Input`. |
| Python's edit-run loop | A behavioural change now costs a compile. Mitigated by 60+ `#[cfg(test)]` unit tests already in-tree, which Python had almost none of (`tests/test_git_service.py` is 18 lines, one test). |
| Textual's `push_screen_wait` | Awaiting a modal result becomes an explicit `Vec<Modal>` stack plus `ModalResult`/`ConfirmAction` payloads (`src/modal.rs:36-95`). More code, but the nesting (Settings → CustomButtons → EditButton) is now visible instead of implied. |
| Free accessibility/rendering niceties | Textual's Header/Footer/Rule/Checkbox/Select widgets are re-drawn by hand (`src/ui/mod.rs:44-83`). |

**Where the design deviates from `ARCHITECTURE.md`.** The architecture doc was written before the
code; the implementation diverged on six points, all of them toward fewer dependencies:

| `ARCHITECTURE.md` proposed | Implemented | Note |
|---|---|---|
| `tui-input 0.15.4` | hand-rolled `TextInput` (`src/ui/widgets.rs`) | 152 lines, unicode-aware by char index; loses `visual_scroll` for long values |
| `nucleo-matcher 0.3.1` | hand-ported Python scorer (`src/util.rs:104-236`) | keeps the exact tier table, so `utils.py`'s ordering transfers 1:1 — a better fidelity outcome than nucleo |
| `timeago 0.6.1` | hand-rolled `naturaltime` (`src/util.rs:51-87`) | phrasing matches `humanize` for the common cases; tested at `src/util.rs:260-273` |
| `insta` snapshots | absent | no render snapshots exist; `tests/` (Rust) does not exist |
| stdlib `home_dir()` (MSRV 1.88) | `dirs 6.0.0` | `Cargo.toml:21` |
| `rust-version = "1.88"` | `rust-version = "1.90"` | `Cargo.toml:5`; no `rust-toolchain.toml` is checked in |
| — | `shlex 1.3.0` added | `Cargo.toml:25`, used for `shell_quote` (`src/cli.rs:87-91`) and the Claude command (`src/services/tmux.rs:234`) |
| `src/config.rs`, `src/fuzzy.rs`, `src/model.rs` | `src/services/settings.rs`, `src/util.rs`, `src/models.rs` | naming follows the Python tree instead |

---

## 3. Module mapping

Line counts from `wc -l` on 2026-08-14 (`find forestui -name '*.py'` → **6 313** lines / 21 files;
`find src -name '*.rs'` → **6 203** lines / 20 files). The Rust total is not final: two render
modules are stubs totalling 18 lines.

| Python module | LoC | Rust module | LoC | Notes |
|---|---:|---|---:|---|
| `forestui/app.py` | 1050 | `src/app.rs` | 1320 | `ForestApp` → `App`. 13 `BINDINGS` + ~30 `on_*` handlers collapse into `handle_key` (`:557`) + `handle_event` (`:483`) + `run_action` (`:954`). `_auto_update` retargeted to `cargo install` (`:208`). |
| — | — | `src/main.rs` | 83 | `run_app()`'s `~/.forestui-error.log` handling (`app.py:1021-1038`) moves here (`:72-83`); tokio runtime + draw loop (`:46-68`). |
| `forestui/cli.py` | 219 | `src/cli.rs` | 246 | clap derive + `ensure_tmux`. Grouped-session block ported verbatim (`:144-175`). `os.execvp` → spawn+wait (`:188-197`), **not** `CommandExt::exec`. |
| `forestui/models.py` | 258 | `src/models.rs` | 458 | Pydantic → serde. Validators become free functions with identical message strings (`:43-91`). 6 unit tests added. |
| `forestui/state.py` | 221 | `src/state.rs` | 259 | `AppState` singleton → an owned struct; `_save_state()` → `save()` on every mutation (`:44-54`). `reorder_worktree` dropped (dead in Python). |
| `forestui/services/settings.py` | 84 | `src/services/settings.rs` | 105 | Module-level `OnceLock<RwLock<PathBuf>>` for the forest path (`:10-36`) replaces the Python global. |
| `forestui/services/git.py` | 355 | `src/services/git.rs` | 464 | 1:1 on argv. Parsing split out as pure `parse_worktree_porcelain` (`:268`). 3 tests including a real-repo round trip. |
| `forestui/services/tmux.py` | 370 | `src/services/tmux.rs` | 328 | libtmux → `tmux` argv. The "most recently active client in our session group" heuristic ported exactly, both fallbacks included (`:35-91`). |
| `forestui/services/github.py` | 242 | `src/services/github.rs` | 234 | Same argv, same `--json` fields (`:147`), same 300 s TTL (`:10`), same assignee/author dedupe (`:153-180`). Cache is a `OnceLock<Mutex<State>>` (`:45-48`). |
| `forestui/services/claude_session.py` | 179 | `src/services/claude_session.rs` | 310 | Same skip rules (`agent-*`, `content.startswith("<")`, `message_count == 0`) and 100-char clip (`:138-166`). Runs in `spawn_blocking` (`src/app.rs:435`). |
| `forestui/utils.py` | 173 | `src/util.rs` | 330 | Fuzzy tiers ported literally, including polarity (lower = better) and the tie-break on lowercased name (`:229-233`). Also absorbs `slugify`, `expanduser`, `naturaltime`, `truncate`. |
| `forestui/components/sidebar.py` | 255 | `src/ui/sidebar.rs` + `App::rebuild_rows` | 117 + (app.rs:240-278) | `Tree` → flat `Vec<SidebarRow>` + `List`/`ListState`. Expansion state dropped. |
| `forestui/components/repository_detail.py` | 458 | `src/ui/detail.rs` | **9 (stub)** | Action model lives in `App::detail_items()` (`src/app.rs:357-379`); rendering not yet written. |
| `forestui/components/worktree_detail.py` | 424 | `src/ui/detail.rs` | (same file) | ditto (`src/app.rs:329-356`). |
| `forestui/components/modals.py` | 1011 | `src/modal.rs` | 1296 | All seven modals' **state, validation and key handling** are complete, with 11 unit tests (`:1060-1296`). |
| — | — | `src/ui/modals.rs` | **9 (stub)** | Modal **rendering** not yet written. |
| `forestui/components/branch_search.py` | 184 | `src/util.rs` + `src/modal.rs` | — | Matching in `util::fuzzy_match_branches`; dropdown state in `AddWorktreeModal.search_index` (`:296`); inline ghost suggestion in `CreateFromIssueModal::base_suggestion` (`:578-587`). |
| `forestui/components/messages.py` | 82 | `src/event.rs` + `App::Action` | 170 + (app.rs:64-82) | Split by boundary: async results → `AppEvent`, synchronous intents → `Action`. |
| `forestui/theme.py` | 696 | `src/theme.rs` | 89 | 12 colour consts + 11 style helpers. Hex values carried over unchanged. |
| — | — | `src/ui/mod.rs` | 119 | Frame split, header, footer, notification stack. |
| — | — | `src/ui/widgets.rs` | 250 | `TextInput`, `framed`, `centered_rect`, `section`. 5 tests. |
| `forestui/__init__.py`, `__main__.py` | 15 | `src/cli.rs:9` | — | **Version is a hardcoded `"0.0.0"` const, not `env!("CARGO_PKG_VERSION")` — see Risk R1.** |
| `forestui/components/__init__.py`, `services/__init__.py` | 37 | `src/services/mod.rs`, `src/ui/mod.rs` | 7 + — | |
| `tests/test_git_service.py` | 18 | inline `#[cfg(test)]` | — | The single Python test is reproduced at `src/services/git.rs:401-407`. |

---

## 4. Behavioural parity matrix

`Identical` = same observable behaviour and, where text is involved, the same string.
`Changed` = deliberate divergence forced by the immediate-mode model or by the port.
`Fixed` = Python behaviour was a defect; the Rust build does the right thing instead.
`Dropped` = the capability no longer exists.

### 4.1 Global keys (no modal open)

| Key | Verdict | Reason / citation |
|---|---|---|
| `q` quit | Identical | `src/app.rs:622` ↔ `app.py:72` |
| `a` add repository | Identical | `src/app.rs:623` |
| `w` add worktree; warns `Select a repository first` when none | Identical | `src/app.rs:901-906` ↔ `app.py:719-725` (same string) |
| `e` editor | Identical | `src/app.rs:625` |
| `t` terminal | Identical | `src/app.rs:626` |
| `o` files (mc) | Identical | `src/app.rs:627` |
| `n` Claude | Identical | `src/app.rs:628` |
| `y` Claude YOLO | Identical | `src/app.rs:629` |
| `h` toggle archive | Identical | `src/app.rs:630` |
| `d` delete | Identical (message text differs — see 4.4) | `src/app.rs:631` |
| `s` settings | Identical | `src/app.rs:632` |
| `r` refresh sidebar + detail | Identical | `src/app.rs:635-638` ↔ `app.py:841-844` |
| `?` help toast | Identical — string copied verbatim, still omits `o`, `y`, `r`, `?` | `src/app.rs:643-647` ↔ `app.py:848-851` |
| `A` toggle archived section | Fixed | New key; Python's `_show_archived` was initialised `False` and never set (`state.py:26`, UC-50). `src/app.rs:639-642` |
| `Tab` switch sidebar ↔ detail | Changed | New concept; Textual had per-widget focus. `src/app.rs:579-585` |
| `Up`/`Down` | Changed | Now context-dependent: sidebar cursor, detail cursor, or in-field cursor. `src/app.rs:586-604` |
| `Enter` | Changed | Sidebar: re-select. Detail: fire `detail_items()[detail_index]`. `src/app.rs:605-617` |
| `Ctrl+C` quit | Identical | `src/app.rs:560-563`; Textual bound it by default |
| Hotkeys while a rename field has focus | Changed | Printable keys go to the field, so `q`/`a`/`d` no longer fire. Python's `q` was `priority=True` (`app.py:72`) and fired even inside an `Input`. `src/app.rs:571-576` |
| Mouse click to select a row or press a button | Dropped | No mouse capture is enabled (`src/main.rs:50`); no `MouseEvent` arm in `handle_term_event` (`src/app.rs:545-553`) |
| Textual command palette (`ctrl+p`), built-in help panel | Dropped | Framework features with no ratatui equivalent |
| `A` and `Tab` absent from the footer and from the `?` toast | Changed | `src/ui/mod.rs:56-69` lists 12 keys; `A` is not among them |
| Footer key order | Changed | Rust: `q a w e t o n y h d s ?` (`src/ui/mod.rs:56-69`). Python rendered `a Add Repo  q Quit  w …` (UC-01 transcript) |

### 4.2 Startup, CLI, tmux entry

| Item | Verdict | Reason / citation |
|---|---|---|
| Positional forest path | Identical | `src/cli.rs:22` ↔ `cli.py:168` |
| `--no-self-update` | Identical (sets `FORESTUI_NO_AUTO_UPDATE`) | `src/main.rs:24-26` ↔ `cli.py:205-206` |
| `--dev` forces dev window naming | Identical | `src/cli.rs:33-34`, `src/main.rs:29` |
| `--debug` | Dropped | Kept as a parsed no-op; Python set `TEXTUAL=devtools` (`cli.py:202-203`). `src/cli.rs:28-30` |
| `--version` / `--help` text | Changed | clap layout differs from click's; `about`/`long_about` carry the same prose (`src/cli.rs:12-19`) |
| Reported version | Changed (defect) | Hardcoded `"0.0.0"` (`src/cli.rs:9`); the release workflow patches only `Cargo.toml` (`.github/workflows/release.yml:37-43`). See R1. |
| Dev mode auto-on at version `0.0.0` | Identical in code, broken in release by the above | `src/main.rs:29` ↔ `cli.py:198` |
| Session name `forestui-<slugified forest dirname>` | Identical | `src/cli.rs:53-62` + `src/util.rs:31-47`, tested at `src/cli.rs:213-217` |
| Window name `forestui` / `forestui-dev-HHMM` | Identical | `src/cli.rs:38-45` |
| Reattach: reuse session, add a window if none is ours | Identical | `src/cli.rs:130-142` ↔ `cli.py:93-118` |
| Grouped session `<session>-<pid>`, `client-attached` hook, `status-left` `#S` rewrite | Identical | `src/cli.rs:144-175` ↔ `cli.py:120-160` |
| Re-exec command line | Fixed | Uses `std::env::current_exe()`, shell-quoted (`src/cli.rs:69-84`, `:116`), instead of the literal string `"forestui"` resolved off the tmux server's `PATH` (`cli.py:74`). Makes `cargo run` and non-`PATH` binaries work; removes the `$FUI_CMD` constraint documented in `TU_USECASES.md:41-64`. |
| Re-exec mechanism | Changed | `Command::status()` + `exit()` (`src/cli.rs:188-197`), not `execvp`. The launcher process survives as a parent wrapper for the whole session. |
| tmux-missing error text | Identical | `src/cli.rs:106-112` ↔ `cli.py:57-62` |
| Missing forest dir created lazily, no eager config write | Identical | `src/state.rs:21-25` ↔ `state.py:29-33`; satisfies UC-06 |
| `~/.forestui-error.log` + "Press Enter to exit..." | Identical | `src/main.rs:72-83` ↔ `app.py:1021-1038` |
| Startup self-update | Changed | `cargo install --locked --quiet forestui` (`src/app.rs:215-218`) replaces `uv tool upgrade forestui` (`app.py:206-211`). See R2. |
| Self-update timeout | Dropped | Python passed `timeout=120` (`app.py:210`); the Rust task has no timeout |
| "updated to vX.Y.Z - restart to apply" | Changed | Version is no longer extracted; the suffix is the generic `updated - restart to apply` (`src/app.rs:226-230`) |
| `focus-events on` at startup, warning toast on failure | Identical | `src/app.rs:182-184` ↔ `app.py:113-114` (same string) |
| Auto-select first repository when nothing is selected | Identical | `src/app.rs:185-192` ↔ `app.py:116-118` |
| Refresh detail on terminal focus gain | Identical | `src/app.rs:550` ↔ `app.py:128-131` |

### 4.3 Sidebar

| Item | Verdict | Reason / citation |
|---|---|---|
| `gh cli: ok (login) / ok / unauth'd / missing` | Identical | `src/services/github.rs:22-31` ↔ `sidebar.py:230-241`, tested at `src/services/github.rs:212-218` |
| Repositories in insertion order, never sorted | Identical | `src/app.rs:242` |
| Active worktrees: `sort_order` asc with `None` last, then `last_modified` desc | Identical | `src/models.rs:188-197` ↔ `models.py:146-155`, tested at `src/models.rs:423-439` |
| Moving the cursor selects and reloads the detail pane | Identical | `src/app.rs:293-319` reproduces Textual's `NodeHighlighted` behaviour (`sidebar.py:199-203`) |
| Cursor does not wrap at either end | Identical | `src/app.rs:298-299` |
| Worktree row shows `[branch]` | Fixed | Textual parsed `[...]` as console markup and swallowed it (UC-11, `sidebar.py:146`). Rust emits a separate styled span (`src/ui/sidebar.rs:78-82`) with a regression test at `:103-116`. |
| Tree glyph spacing | Changed | `└─ name [branch]` (`src/ui/sidebar.rs:78-81`) vs Python's `└─  name [branch]` with a leading space (`sidebar.py:146`) |
| Archived group with rows `   <name> (<repo>)` | Fixed | Dead code in Python (`sidebar.py:150-163`); reachable in Rust behind `A` (`src/app.rs:260-272`, `src/ui/sidebar.rs:84-94`) |
| Smart collapse / expand state on repository nodes | Dropped | The flattened row list has no expansion state (`src/app.rs:240-278`); `sidebar.py:187-192` had it |
| Empty sidebar shows `No repositories` / `Press [a] to add one` | Fixed | Python rendered nothing usable; `src/ui/sidebar.rs:27-33` |
| Detail-pane empty state visible | Fixed *(pending render pass)* | `EmptyState` collapsed to zero rows in Python (UC-07, `app.py:52-62`). `src/ui/detail.rs:7` is still a no-op, so nothing renders **today** either. |

### 4.4 Detail pane

| Item | Verdict | Reason / citation |
|---|---|---|
| Buttons → focusable list index | Changed | `App::detail_items()` returns `Vec<DetailItem>`; `Enter` dispatches `detail_items()[detail_index]` (`src/app.rs:324-381`, `:605-617`) |
| `btn-custom-<prefix>-<session_id>` id parsing | Fixed | Replaced by typed `Action::ResumeCustom { button, session }` (`src/app.rs:75`), removing the ambiguity when a prefix contains `-` (`repository_detail.py:267-278`) |
| Per-session controls: Resume, YOLO, one per custom button | Identical set and order | `src/app.rs:337-343` ↔ `repository_detail.py:322-345` |
| Sessions capped at 5 | Identical | `SESSION_LIMIT = 5` (`src/app.rs:18`) ↔ `claude_session.py:34` default + `[:5]` slice |
| Issues: 10 fetched | Identical | `ISSUE_LIMIT = 10` (`src/app.rs:19`) ↔ `github.py:101` |
| Issues: how many get a "Create WT" control | Changed | Rust exposes all 10 (`src/app.rs:374-377`); Python rendered `issues[:5]` (`repository_detail.py:429`) |
| Rename via two inline fields, pre-filled, `Enter` submits | Changed | Now `DetailItem::Field` entries edited in place, `Esc` restores the stored values (`src/app.rs:653-714`); Python used Textual `Input`s (`worktree_detail.py:234-244`) |
| Rename does `fs::rename` → `git worktree repair` → Claude session migration → state update | Identical | `src/app.rs:1137-1170` ↔ `app.py:608-636` |
| `Path already exists` guard before rename | Identical | `src/app.rs:1146-1149` (same string) |
| Branch rename error toast `Branch rename failed: …` | Identical | `src/app.rs:1183` ↔ `app.py:653` |
| Hotkeys/actions on a worktree whose directory is gone | Fixed | Rust refuses and toasts `Directory no longer exists: <path>` (`src/app.rs:959-980`); Python created a tmux window that silently landed in `$HOME` (UC-43). **Intentional divergence — UC-43's Expected block must be rewritten.** |
| Sync (`git pull`) toasts `Syncing...` / `Sync complete` / `Sync failed: …` | Identical | `src/app.rs:1027-1039` ↔ `app.py:407-413` |
| `⟳ Git Pull (No remote)` / `(Directory missing)` disabled labels | Changed *(pending render pass)* | The refusal is enforced in `run_action` (`src/app.rs:961-980`); the disabled-label text does not exist anywhere in `src/` yet |
| Section headers `MAIN REPOSITORY` / `WORKTREE` / `LOCATION` / `OPEN IN` / `CLAUDE` / `RECENT SESSIONS` / `MY OPEN GITHUB ISSUES` / `RENAME` / `MANAGE` | Not yet ported | `grep -r` over `src/` finds none of these strings; they arrive with the detail render pass |
| Issue-refresh spinner cadence | Changed | Global 100 ms tick (`src/event.rs:134`) vs Python's 50 ms interval on the refresh button (`repository_detail.py:393`) |
| Detail cursor survives async data arrival | Changed (defect) | `AppEvent::Sessions`/`Issues` mutate the list length without re-clamping `detail_index` (`src/app.rs:503-513`). See R3. |

### 4.5 Modals

| Item | Verdict | Reason / citation |
|---|---|---|
| `Esc` closes every modal | Identical | `src/modal.rs:135-137`, asserted for 5 variants at `:1079-1097` ↔ `modals.py` `BINDINGS` on all 7 |
| Add Repository live validation: `Path does not exist` / `Path is not a directory` / `Not a git repository` / `Repository: <name>` | Identical | `src/modal.rs:224-244` ↔ `modals.py:85-119` |
| Add Repository `Enter` in the path field submits | Identical | `src/modal.rs:276` ↔ `modals.py:80-83` |
| Add Repository is a hard no-op unless `<path>/.git` exists | Identical | `src/modal.rs:246-252`, tested at `:1100-1113` |
| `Import existing worktrees` checkbox | Identical | `src/modal.rs:272-275` |
| Worktree name sanitised to `[A-Za-z0-9-_]` as you type | Identical | `src/modal.rs:355-361` ↔ `modals.py:274-276`, tested at `:1124-1142` |
| Branch auto-fills `<prefix><name>` in New Branch mode | Identical | `src/modal.rs:430-436` |
| Path preview under the name field | Identical | `src/modal.rs:363-369` |
| Create disabled unless an existing branch is an exact list member | Identical | `src/modal.rs:380-386` ↔ `modals.py:265-272` |
| Validation strings `Worktree name is required`, `Branch name is required`, `Branch '<x>' does not exist`, `Worktree path already exists` | Changed | Text identical, the Python leading space is gone (`src/modal.rs:394-414` vs `modals.py:327-342`) |
| Mode switch New Branch ↔ Existing | Changed | `←`/`→`/`Space`/`Enter` on the mode row (`src/modal.rs:475-492`); Python used two clickable buttons |
| Fuzzy branch ordering and tie-breaks | Identical | `src/util.rs:143-236` is a literal port of `utils.py:43-150`; empty query returns the first 50 in list order (`src/util.rs:216-222` ↔ `utils.py:140-141`); tested at `src/util.rs:283-307` |
| Dropdown match-count label (`N branches`, `N of M branches`, `1 match`, `3 matches`, `No matches`) | Not yet ported | None of these strings exist in `src/`; `branch_search.py:143-154` owns them. Blocks UC-33 (P0). |
| Highlight of the matched substring in the dropdown | Not yet ported | `util::highlight_range` (`src/util.rs:239-245`) is written and tested but has no caller |
| Settings: editor and theme selection | Changed | `←`/`→` cycle a fixed list (`src/modal.rs:744-762`); Python used `Select` dropdowns. Option lists and order identical (`src/modal.rs:19-32` ↔ `modals.py:364-381`). |
| Settings saves 5 keys incl. `custom_buttons` | Identical | `src/services/settings.rs:62-69`, tested at `:93-105` |
| `default_terminal` reset to `""` on save | Identical | `src/modal.rs:729` ↔ `modals.py:469-474` — both discard a hand-edited value |
| `Settings saved` toast | Identical | `src/app.rs:790` ↔ `app.py:837` |
| Custom Buttons list management | Changed | `a` add, `e`/`Enter` edit, `d`/`Del` remove, `K`/`J` reorder, `s` save (`src/modal.rs:821-867`); Python used per-row `↑ ↓ Edit Delete` buttons (`modals.py:907-949`) |
| Prefix auto-derives from label until hand-edited | Identical | `src/modal.rs:979-993` ↔ `modals.py:827-839`, tested at `src/modal.rs:1171-1185` |
| `derive_prefix` slug rules (lowercase, `[a-z0-9_-]`, collapse, trim, ≤20) | Identical | `src/models.rs:20-37` ↔ `models.py:18-24`, tested at `src/models.rs:378-384` |
| `Another button already uses this label` / `… this prefix` | Changed | Same text, leading space dropped (`src/modal.rs:955-962` vs `modals.py:862-867`) |
| `Command cannot be empty` and the three validators | Identical | `src/modal.rs:937-952` ↔ `modals.py:853-860` |
| Confirm modal default action | Changed | Focus starts on Cancel; `Enter` cancels, `y`/`Y` confirms, `n`/`N` cancels, `←/→/Tab` switch (`src/modal.rs:1035-1057`, tested at `:1277-1285`) |
| Confirm modal body text from the detail-pane Delete / Remove buttons | Changed | Rust always uses the short forms `Permanently delete '<name>'?` and `Remove '<name>' from forestui?` (`src/app.rs:923-941`); Python used two-line variants for the detail-pane buttons (`app.py:596`, `app.py:431`) |
| Repository removal deletes no files | Identical | `src/state.rs:65-71` only drops the entry |
| Create-from-issue prefills name `<n>-<slug>` and branch `<prefix><name>` | Identical | `src/modal.rs:549-563`; `branch_name()` slug rules at `src/models.rs:335-355`, tested at `:411-420` |
| Base-branch default: `<remote>/<current>` → local `<current>` → first branch | Identical | `src/modal.rs:673-684` ↔ `modals.py:581-592`, tested at `src/modal.rs:1288-1295` |
| `Fetch` button spinner at 100 ms while fetching | Identical | `src/modal.rs:124-130` + `src/event.rs:134` ↔ `modals.py:689` |
| Inline ghost suggestion for the base branch | Changed | Textual's `Suggester` completed as you typed; Rust computes it (`src/modal.rs:578-587`) and requires `→` to accept (`:627-633`) |
| `Pull repo before creating` default checked | Identical | `src/modal.rs:564` |
| Modal nesting Settings → CustomButtons → EditButton, child result flows to parent | Identical | `src/modal.rs:111-121`, `:769-771`, `:837-850`, tested at `:1233-1256` |
| `Fetch failed: <e>` toast | Identical | `src/app.rs:530` ↔ `app.py:567` |

### 4.6 tmux window management

| Item | Verdict | Reason / citation |
|---|---|---|
| Window names `edit:` `term:` `files:` `claude:` `yolo:` `<custom prefix>:` + `repo[:worktree]` | Identical | `src/services/tmux.rs:185-277`, `src/state.rs:165-177`, tested at `src/services/tmux.rs:298-306` and `src/state.rs:232-242` |
| `:2`, `:3` uniquifying, counter starts at 2 | Identical | `src/services/tmux.rs:151-168`, tested at `:292-296` |
| `edit:` reuses an existing window; the others always create | Identical | `src/services/tmux.rs:191-194` vs `:202`, `:211`, `:261` |
| `$SHELL -ic <quoted cmd>` for Claude | Identical | `src/services/tmux.rs:219-238`, tested at `:309-327` |
| `--dangerously-skip-permissions` only for the built-in YOLO button | Identical | `src/services/tmux.rs:227-229` ↔ `tmux.py:345` |
| `-r <session_id>` appended on resume | Identical | `src/services/tmux.rs:231-233` |
| Target session = most recently active client in our session group, with both fallbacks | Identical | `src/services/tmux.rs:35-91` ↔ `tmux.py:56-107` — the behaviour UC-40 pins |
| Own window resolved via `TMUX_PANE` | Identical | `src/services/tmux.rs:97-103` ↔ `tmux.py:109-130` |
| TUI editor set (10 commands, matched on the first word) | Identical | `src/services/tmux.rs:8-21`, tested at `:283-289` |
| GUI editor spawned detached with `Opened in <cmd>` | Identical | `src/app.rs:1052-1067` ↔ `app.py:904-915` |
| `term:session` fallback name still reachable when the path matches nothing | Identical | `src/state.rs:176` ↔ `app.py:889` |
| tmux calls block the render loop | Identical | Both are synchronous (`src/services/tmux.rs:23-29`; libtmux was synchronous too) |

### 4.7 Services, state, data

| Item | Verdict | Reason / citation |
|---|---|---|
| Every `git` invocation and format string | Identical | `src/services/git.rs` — `branch --show-current`, `branch -a --format=%(refname:short)`, `remote`, `worktree add/remove/repair/list --porcelain`, `rev-parse --short`, `log -1 --format=%H\|%h\|%ct`, `rev-parse --abbrev-ref --symbolic-full-name @{u}`, `fetch`, `pull`, `branch -m`, `branch --unset-upstream` |
| Missing `cwd` surfaces as an error, not a panic | Identical | `src/services/git.rs:43-54`, tested at `:401-407` — the guard for commit `b8f2bc5` / UC-42 |
| `worktree remove` retried with `--force` | Identical | `src/services/git.rs:222-236` |
| A failed `git worktree remove` never blocks the config update | Identical | `src/app.rs:814-819` ↔ `app.py:600-604` |
| Remote-tracking branch auto-checkout with `--track -b <local>` | Identical | `src/services/git.rs:186-212` ↔ `git.py:180-208` |
| `branch --unset-upstream` after creating from a remote base | Identical | `src/services/git.rs:173-184` |
| `gh` argv, `--json` field list, assignee+author dedupe, sort by `createdAt` desc, truncate to limit | Identical | `src/services/github.rs:146-185` ↔ `github.py:130-185` |
| Malformed `gh` JSON | Changed | Rust yields an empty list silently (`src/services/github.rs:173`); Python raised and toasted `Issue fetch error: <e>` (`app.py:155-156`) |
| 300 s issue cache keyed `owner/repo`; auth cached for the process | Identical | `src/services/github.rs:10`, `:69-95`, `:125-130` |
| Periodic issue refresh every 300 s | Identical cadence, different mechanism | Elapsed check on the 100 ms tick (`src/app.rs:492-498`) vs `set_interval(300, …)` (`app.py:126`) |
| Claude session discovery, skip rules, 100-char clip, blank-line collapse, newest-first | Identical | `src/services/claude_session.rs:30-188`, 5 tests at `:229-313` |
| Session directory migration on rename | Identical | `src/services/claude_session.rs:191-227` ↔ `claude_session.py` |
| Relative-time phrasing | Changed | Hand-rolled `naturaltime` (`src/util.rs:51-87`) approximates `humanize`; matches on the tested cases (`:260-273`) but is not the same library and may differ at boundaries |
| `Imported N worktrees` count | Fixed | Rust counts the worktrees it actually added (`src/app.rs:1247`); Python printed `len(worktrees) - 1`, over-reporting whenever entries inside the forest dir were skipped (`app.py:1016`) |
| Worktrees already inside the forest dir are skipped on import | Identical | `src/app.rs:1230-1236` ↔ `app.py:1000-1007` |
| Config written on every mutation, non-atomically | Identical | `src/state.rs:44-54` ↔ `state.py:47-53`; `ARCHITECTURE.md`'s atomic-write recommendation was **not** implemented |
| Unparseable config → empty state, file left intact until the first mutation | Identical | `src/state.rs:29-42`, tested at `:204-210`; UC-46 |
| Unparseable `settings.json` → defaults | Identical | `src/services/settings.rs:51-56`, tested at `:75-91` |
| Selection is in-memory only, never persisted | Identical | `src/state.rs:36-41` |
| Selection survives a background state reload | Changed (defect) | `AppEvent::StateChanged` replaces `self.state` with a freshly loaded `AppState`, whose `selection` is `Selection::default()` (`src/app.rs:533-538` + `src/state.rs:36-41`). Python re-selected the new worktree explicitly (`app.py:707`). See R4. |
| `theme` setting stored and ignored | Identical | `src/modal.rs:731`; no palette switch exists |
| `reorder_worktree` / `sort_order` writes | Dropped | Dead in Python — `state.py:185` has no caller anywhere in `forestui/` |
| `refresh_worktree_timestamp` | Identical (dead in both) | `src/state.rs:152`; no caller |

### 4.8 Tally

Counted mechanically over §4.1–4.7 (qualified verdicts such as "Identical (dead in both)" fold into
their base category; "Not yet ported" is kept separate because those rows are unimplemented, not
decided):

| Verdict | Rows |
|---|---:|
| Identical | 93 |
| Changed | 29 |
| Fixed | 9 |
| Dropped | 6 |
| Not yet ported (Phase 2 work) | 3 |
| **Total** | **140** |

Five rows are outstanding render work rather than settled decisions: the detail-pane empty state and
disabled Sync labels (marked *pending render pass*), and the three "Not yet ported" rows — section
header strings, the dropdown match-count label, and the match highlight. Three `Changed` rows are
defects rather than choices (R1 version constant, R3 focus-index drift, R4 selection wipe); a fourth,
the retargeted self-updater, is a deliberate change with a defect-grade consequence (R2).

---

## 5. Data compatibility

Both files keep their exact path, filename, key names, key order, and 2-space indentation.
`.forest-config.json` (the macOS `forest` app) is untouched by either build.

### 5.1 `<forest>/.forestui-config.json`

Written by `AppState::save` (`src/state.rs:44-54`) via `serde_json::to_string_pretty`, which emits
2-space indentation — matching Python's `json.dump(..., indent=2)` (`state.py:53`).

| Field | Python type / source | Rust type / source | Compatible? |
|---|---|---|---|
| `repositories` | `list[Repository]` (`state.py:17`) | `Vec<Repository>`, `#[serde(default)]` (`src/models.rs:363-367`) | Yes |
| `repositories[].id` | `UUID`, `uuid4()` default (`models.py:137`) | `Uuid`, `#[serde(default = "Uuid::new_v4")]` (`src/models.rs:165`) | Yes — hyphenated lowercase both ways |
| `repositories[].name` | `str` | `String` | Yes |
| `repositories[].source_path` | `str`, stored **as typed** (unresolved) | `String`, stored as typed (`src/app.rs:1193-1200`) | Yes |
| `repositories[].worktrees` | `list[Worktree]`, default `[]` | `Vec<Worktree>`, `#[serde(default)]` (`src/models.rs:169-170`) | Yes |
| `…worktrees[].id` | `UUID` | `Uuid`, defaulted | Yes |
| `…worktrees[].name` | `str` | `String` | Yes |
| `…worktrees[].branch` | `str` | `String` | Yes |
| `…worktrees[].path` | `str`, **resolved** at creation | `String`, from `get_forest_path().join(...)` which is resolved (`src/services/settings.rs:24`) | Yes |
| `…worktrees[].is_archived` | `bool`, default `False` | `bool`, `#[serde(default)]` (`src/models.rs:128-129`) | Yes |
| `…worktrees[].sort_order` | `int \| None`, default `None`, emitted as `null` | `Option<i64>`, `#[serde(default)]`, **no** `skip_serializing_if` (`src/models.rs:130-131`) | Yes — key is always present |
| `…worktrees[].last_modified` | `datetime` UTC, pydantic JSON mode → `2026-08-14T19:21:41.258677Z` (µs) | `DateTime<Utc>`, chrono serde → RFC 3339, up to **ns** precision | Yes semantically; see the precision note below |
| `…worktrees[].base_branch` | `str \| None` | `Option<String>`, `#[serde(default)]` | Yes |
| `…worktrees[].created_from_ref` | `str \| None` | `Option<String>`, `#[serde(default)]` | Yes |

Real example (Rust-writable, Python-readable; transcribed from UC-48/UC-49):

```json
{
  "repositories": [
    {
      "id": "787c8913-4f59-474c-98b7-249c8c740821",
      "name": "alpha",
      "source_path": "/tmp/fui-fix.XXXXXX/src/alpha",
      "worktrees": [
        {
          "id": "0f2f2b7e-4d9c-4a1f-9f1a-1f2a3b4c5d6e",
          "name": "wt-two",
          "branch": "feat/wt-two",
          "path": "/private/tmp/fui-fix.XXXXXX/forest/alpha/wt-two",
          "is_archived": true,
          "sort_order": null,
          "last_modified": "2026-08-14T19:21:41.258677Z",
          "base_branch": "main",
          "created_from_ref": "2488dce"
        }
      ]
    }
  ]
}
```

A round-trip of this exact shape through serde is asserted at `src/models.rs:442-457`.

**Round-tripping: lossless in content, not byte-identical.** Two known differences, neither of
which loses data:

1. `last_modified` precision. Python writes microseconds (`…258677Z`); chrono's `Serialize` for
   `DateTime<Utc>` emits `to_rfc3339_opts(AutoSi, true)`, which prints 0, 3, 6 or 9 fractional
   digits depending on the value — so a Rust-written timestamp may carry nanoseconds
   (`…258677123Z`). Pydantic parses that and truncates to microseconds, so Python→Rust→Python is
   lossless and Rust→Python→Rust loses sub-microsecond precision on a field used only for sort
   ordering. Not verified by execution; inferred from chrono's documented serde impl.
2. Key **order** is stable and identical (serde emits struct fields in declaration order, which
   matches the Pydantic field order field-for-field), so a version-controlled config produces no
   spurious diff other than (1).

**Verdict on the hard requirement: it holds**, with the precision caveat above. Both builds tolerate
a missing optional key, both ignore unknown keys, and neither rewrites an unparseable file until the
user makes a change.

### 5.2 `~/.config/forestui/settings.json`

Written by `save_settings_to` (`src/services/settings.rs:62-69`), same path as `settings.py:38`.

| Field | Python default | Rust default | Compatible? |
|---|---|---|---|
| `default_editor` | `"vim"` (`models.py:194`) | `default_editor()` → `"vim"` (`src/models.rs:232-234`, 247) | Yes |
| `default_terminal` | `""`, vestigial, never read | `String::new()` via `#[serde(default)]` (`src/models.rs:249-250`) | Yes — still serialised, as UC-34 requires |
| `branch_prefix` | `"feat/"` | `"feat/"` (`src/models.rs:236-238`) | Yes |
| `theme` | `"system"` | `"system"` (`src/models.rs:240-242`) | Yes |
| `custom_buttons` | `[]` of `{label, prefix, command}` | `Vec<CustomClaudeButton>`, `#[serde(default)]` (`src/models.rs:255-256`, `:98-103`) | Yes — a file predating the field still loads (tested at `src/services/settings.rs:83-91`) |

Real example (the exact bytes UC-34 asserts):

```json
{
  "default_editor": "vim",
  "default_terminal": "",
  "branch_prefix": "feat/",
  "theme": "system",
  "custom_buttons": []
}
```

**Round-tripping: lossless and byte-identical.** All five fields are plain strings/arrays with no
formatting ambiguity, field order matches, and both builds write 2-space pretty JSON.

One shared quirk worth restating because it looks like a Rust bug and is not: saving from the
settings modal resets `default_terminal` to `""` in **both** builds (`src/modal.rs:729` ↔
`modals.py:469-474`), because neither modal carries the loaded value through.

---

## 6. Phased execution plan

Phases 0–2 describe work already on disk (so the exit criteria are evidence statements, not
promises); phases 3–6 are the remaining work.

### Phase 0 — Foundation *(complete)*

- **Entry:** `ARCHITECTURE.md` + `SPEC.md` agreed.
- **Delivered:** `Cargo.toml` (10 runtime deps, 1 dev dep), `src/models.rs`, `src/state.rs`,
  `src/util.rs`, `src/theme.rs`, `src/services/{git,tmux,github,claude_session,settings}.rs`.
- **Exit evidence:** every service module carries `#[cfg(test)]` tests (git 3, tmux 4, github 2,
  claude_session 5, settings 3, models 6, state 5, util 6).
- **Gates:** none — no UI yet.

### Phase 1 — App core *(complete)*

- **Entry:** Phase 0 modules present.
- **Delivered:** `src/event.rs` (channel, reader thread, 100 ms tick), `src/app.rs` (state, focus,
  key dispatch, `detail_items`, actions, background task spawning), `src/modal.rs` (seven modals'
  state + validation + key handling, 11 tests), `src/cli.rs` (clap + `ensure_tmux`, 4 tests),
  `src/main.rs`, `src/ui/{mod,sidebar,widgets}.rs`.
- **Exit evidence:** `src/ui/sidebar.rs:11-46` renders the tree; `src/ui/mod.rs:19-42` composes
  header/sidebar/detail/footer/notifications/modal.
- **Gates:** none until something renders.

### Phase 2 — Rendering *(in flight — the two stubs)*

- **Entry:** Phase 1 merged.
- **Work:** `src/ui/detail.rs` (currently 9 lines, `draw` is a no-op at `:7`) and
  `src/ui/modals.rs` (currently 9 lines, `:7`). This is the whole of the repository detail pane, the
  worktree detail pane, the empty state, and all seven modal overlays — roughly 1 900 Python lines of
  layout collapsing into an estimated 700–900 Rust lines.
- **Exit criteria:**
  1. Every section header string from `SPEC.md` §4.3–4.4 exists in `src/` (`grep` currently finds
     none of `MAIN REPOSITORY`, `LOCATION`, `RECENT SESSIONS`, `No sessions found`,
     `No issues found`, `⚠ MISSING`, `Git Pull`, `Based on`, `Create WT`).
  2. The dropdown count label wording (`N branches` / `N of M branches` / `1 match` / `N matches` /
     `No matches`, `branch_search.py:143-154`) is implemented.
  3. `util::highlight_range` (`src/util.rs:239-245`) has a caller.
  4. Rendered focus is driven by the *same* `detail_items()` the key handler consumes — no second
     list (this is R3's mitigation).
  5. Detail-pane empty state is visible (the deliberate fix to UC-07).
- **P0 gates:** UC-12 (repo detail layout), UC-16 (worktree detail layout), UC-13 (Git Pull disabled
  with no remote), UC-29 (Add Repository layout + validation), UC-31 (Add Worktree layout/preview),
  UC-33 (branch dropdown + count label), UC-34 (Settings layout), UC-36 (Delete confirm).

### Phase 3 — Build and test gate

- **Entry:** Phase 2 renders.
- **Work:** install a toolchain (none exists on this machine — verify `rustc --version` first), add
  a `rust-toolchain.toml` pinning the channel to match `rust-version = "1.90"` (`Cargo.toml:5`;
  `ARCHITECTURE.md` assumed 1.88), then `make check` = `cargo fmt --check` + `cargo clippy
  --all-targets -D warnings` + `cargo check --all-targets` + `cargo test` (`Makefile:5-24`).
- **Exit criteria:** `make check` green on macOS and Linux; ~60 unit tests pass; `.gitignore` gains
  `/target` (it currently lists only Python artefacts) and a decision is recorded on committing
  `Cargo.lock` — `release.yml` uses `cargo build --locked` and `install.sh` uses
  `cargo install --locked`, so **the lockfile must be committed**.
- **P0 gates:** none (no runtime behaviour proven yet).

### Phase 4 — Replacement and packaging *(mostly complete in the working tree)*

- **Entry:** Phase 3 green.
- **Already done** (working tree at the time of writing — `git status` shows them modified but
  uncommitted):
  - `Makefile` → cargo targets (`fmt --check`, `clippy -D warnings`, `check`, `test`).
  - `install.sh` → prebuilt tarball for three targets with a `cargo install` fallback, plus a
    `uv tool uninstall forestui` hint for machines still carrying the Python build.
  - `.github/workflows/check.yml` → dtolnay toolchain + `Swatinem/rust-cache` + `make check` on
    ubuntu/macos.
  - `.github/workflows/release.yml` → new: three-target binary matrix + crates.io publish.
  - `.github/workflows/publish.yml` → deleted (PyPI).
  - `README.md`, `CLAUDE.md`, `.claude/skills/test-forestui/SKILL.md` → rewritten for Rust.
  - `.gitignore` → `/target`, `**/*.rs.bk`, Python leftovers retained for old checkouts.
  - `.pre-commit-config.yaml` → local `cargo fmt` / `cargo clippy` hooks replacing ruff.
- **Still outstanding:**
  1. Delete `forestui/`, `pyproject.toml`, `uv.lock`, `.python-version`, `tests/test_git_service.py`
     and the stale `.mypy_cache/`, `.ruff_cache/`, `.pytest_cache/`, `.venv/` directories — all
     still present.
  2. `git add Cargo.toml Cargo.lock src/ doc/rust-rewrite/` — none of them are tracked yet, and
     `--locked` builds need the committed lockfile.
- **Exit criteria:** no `.py` file remains outside git history; `grep -ri textual` returns only
  historical references; `Cargo.toml` `repository` matches the `origin` remote
  (`git@github.com:flipbit03/forestui.git` — it does).
- **P0 gates:** none.

### Phase 5 — End-to-end acceptance

- **Entry:** Phase 4 done, release binary built (`cargo build --release`).
- **Work:** run the `tu` playbook per `TU_USECASES.md` §0–3 rules (isolated `TMUX_TMPDIR`, isolated
  `HOME`, throwaway forest, `env -u TMUX`, never call `tmux` from Bash). Rewrite the six
  mouse-driven steps as key sequences first (UC-18, 32, 33, 34, 37, 38) — the port has no mouse.
- **Exit criteria:** 25/25 P0 pass, with four Expected blocks rewritten in `TU_USECASES.md` and
  signed off as intentional divergences:
  - UC-07 → the empty state is now visible;
  - UC-11 → the sidebar row now shows `wt-two [feat/wt-two]`;
  - UC-50 → `A` reveals the ` Archived` group;
  - UC-43 → a hotkey on a deleted worktree directory now refuses with an error toast.
- **P0 gates (all 25):** UC-01, 04, 06 (startup/tmux); 08, 09 (sidebar); 12, 13 (repo detail); 16
  (worktree detail); 20, 21, 22, 24, 27 (hotkeys/windows); 29, 31, 33, 34, 36 (modals); 39, 40
  (grouped sessions); 42, 44 (stale worktree, delete); 48, 49, 51 (persistence). UC-39/UC-40 are the
  highest-value ones — they are the only check on the spawn+wait re-exec (`src/cli.rs:188-197`) not
  having broken client/session attribution.

### Phase 6 — Release

- **Entry:** Phase 5 green; R1 and R2 resolved (see §8).
- **Work:** merge, tag, publish. Detail in §9.
- **Exit criteria:** `cargo install forestui` and `install.sh` both yield a working binary on
  macOS arm64, macOS x86_64 and Linux x86_64.

---

## 7. Rollback plan

### For a user

1. `cargo uninstall forestui`, or delete `~/.local/bin/forestui` if it came from `install.sh`.
2. `uv tool install forestui==1.3.1` — the last Python release, verified present on PyPI on
   2026-08-14 (`forestui 1.3.1`, `requires_python >=3.14`). Nothing about the Rust build unpublishes
   it.
3. Nothing else. `<forest>/.forestui-config.json` and `~/.config/forestui/settings.json` are read
   unchanged by the Python build (§5), so no state is lost in either direction. Archived worktrees
   toggled on with `A` stay archived and become invisible again, exactly as they were before.

### For a maintainer

- Tag `v1.3.1` is the last Python commit lineage; `git checkout v1.3.1` restores the whole Python
  tree including `pyproject.toml`, `publish.yml` and the Textual sources.
- The merge commit for the rewrite should be a normal merge so `git revert -m 1 <merge>` restores
  the Python package wholesale.

### What is irreversible

| Item | Why |
|---|---|
| A published crates.io version | crates.io permits yanking, never deletion or reuse of a version number |
| A GitHub release's asset set once downloaded | tarballs can be replaced but not un-downloaded |
| The `forestui` name on crates.io | verified **unregistered** on 2026-08-14 (`crates.io/api/v1/crates/forestui` → `crate 'forestui' does not exist`); once published, the name is permanently owned by whoever published first |
| Version numbering continuity | Python is at `1.3.1`; a Rust `v1.4.0` inherits that line across two different package registries with no shared upgrade path |
| A `cargo install` triggered by the startup self-updater | it writes `~/.cargo/bin/forestui`, which may shadow the `install.sh` binary depending on `PATH` order (see R2) |

---

## 8. Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| **R1** | **Released binaries report version `0.0.0`.** `src/cli.rs:9` hardcodes `pub const VERSION: &str = "0.0.0"`, while `release.yml:37-43` patches only `Cargo.toml`. Because `src/main.rs:29` computes `dev_mode = args.dev_mode \|\| VERSION == "0.0.0"`, **every release build silently runs in dev mode** and names its tmux window `forestui-dev-HHMM`. `--version` also lies. | Certain | High — breaks UC-02 outright and makes the header/window name wrong for every user | One-line change to `env!("CARGO_PKG_VERSION")`; add a UC-02 run against a tagged build before announcing |
| **R2** | **Startup self-update runs `cargo install --locked --quiet forestui` unconditionally** (`src/app.rs:215-218`) with no version check and no timeout (Python had `timeout=120`, `app.py:210`). For anyone who installed a prebuilt tarball, this compiles the whole crate in the background on **every launch**, needs a Rust toolchain that may not exist, and installs a second binary into `~/.cargo/bin` that can shadow `~/.local/bin/forestui`. | Certain if shipped as-is | High — CPU burn on every start, plus two divergent binaries on `PATH` | Gate on a cheap version probe (crates.io index or the GitHub releases API) and only then offer an upgrade; or drop self-update entirely as `ARCHITECTURE.md` §"Open decisions" recommends and print an available-version hint in the header |
| **R3** | **Focus-index drift between `detail_items()` and the renderer.** `detail_index` is an index into a list whose length changes asynchronously: `AppEvent::Sessions` and `AppEvent::Issues` set `self.sessions`/`self.issues` without re-clamping (`src/app.rs:503-513`), while `detail_items()` (`src/app.rs:324-381`) grows by `2 + custom_buttons` entries per session that lands. A cursor parked on Archive/Delete silently becomes a cursor on a Resume button, and `Enter` then spawns a tmux window instead of the intended action. The renderer will recompute the list independently, so highlight and dispatch can also disagree within one frame. | High | Medium–High — wrong action fired from a correct-looking highlight | Compute `detail_items()` **once** per frame and pass the slice to both the renderer and the dispatcher; on `Sessions`/`Issues` arrival, remap `detail_index` by identity (`DetailItem` already derives `PartialEq`) or clamp it to `len - 1`; snapshot-test the first and last cursor positions with `TestBackend` |
| **R4** | **`AppEvent::StateChanged` wipes the selection.** It replaces `self.state` with `AppState::load()` (`src/app.rs:533-538`), and `load_from` initialises `selection: Selection::default()` (`src/state.rs:36-41`). Nothing re-selects afterwards. Every worktree create, import, rename and branch rename therefore ends with an empty detail pane — Python explicitly re-selected the new worktree (`app.py:707`, `app.py:535`). | Certain | High — the primary happy path ("create a worktree") ends on a blank pane | Carry the selection across the reload (save `self.state.selection` before, restore after, then `sync_sidebar_index()`); for `create_worktree`, ship the new worktree id in the event and select it. Covered by UC-31/UC-51 once rendering exists |
| R5 | **tmux grouped-session behaviour differs under the new re-exec.** `exec_tmux` spawns and waits (`src/cli.rs:188-197`) instead of `os.execvp` (`cli.py:161`), so an extra parent process lives for the whole session. Client attribution for new windows depends on `list-clients` activity ordering (`src/services/tmux.rs:35-91`); a stray process or a changed attach order can make window creation steal the wrong terminal. | Medium | High — UC-40 is exactly this, and it is the reason grouped sessions exist | Run UC-39/UC-40 as a two-terminal `tu` pair before merge; if attribution is wrong, switch to `std::os::unix::process::CommandExt::exec()` as `ARCHITECTURE.md` specified |
| R6 | **crates.io name availability.** `forestui` is unregistered as of 2026-08-14 (verified). Between now and release, anyone can take it — including a squatter who noticed this branch. | Low | High — an unavailable name forces a rename across README, install.sh, release.yml, the tmux session prefix, and every doc | Publish a `0.0.1` placeholder from the maintainer account as soon as the crate compiles, then release over it |
| R7 | **Missing binary targets.** `release.yml:14-22` builds `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`. `install.sh:detect_target` maps exactly those three and nothing else. Linux arm64 (`aarch64-unknown-linux-gnu`) and musl/Alpine users fall through to `cargo install`, which needs a toolchain they may not have. `x86_64-apple-darwin` is cross-compiled on `macos-latest` (arm64 runner) and is never executed in CI. | Medium | Medium — a silent "install from source or nothing" for a slice of users | Add `aarch64-unknown-linux-gnu` (cross or an arm runner) and, ideally, `x86_64-unknown-linux-musl`; smoke-run each artefact (`./forestui --version`) in the release job |
| R8 | **Users on the old auto-update path never learn the tool moved.** Installed Python builds run `uv tool upgrade forestui` at every start (`app.py:206-211`). If PyPI `forestui` stops receiving releases, they upgrade to nothing, forever, with no message. `.github/workflows/publish.yml` has **already been deleted** in the working tree, so the automated path for a farewell release is gone. | Certain for existing users | Medium — a silently stale user base | Publish one final Python release whose only change is a startup notification pointing at the Rust install command. Do it from the `v1.3.1` lineage (where `publish.yml` still exists) before the rewrite merges, or restore the workflow temporarily, or `uv build && uv publish` by hand |
| R9 | **Terminal reader thread starts before raw mode.** `event::start()` (`src/main.rs:47`) spawns the crossterm reader before `ratatui::init()` (`src/main.rs:50`), so keystrokes in that window are read in cooked mode and may echo or be line-buffered. | Low | Low | Move `ratatui::init()` above `event::start()` |
| R10 | **No render regression net.** `ARCHITECTURE.md` prescribed `TestBackend` + `insta` snapshots; neither exists (`insta` is not in `Cargo.toml`, and there is no `tests/` directory). `CLAUDE.md` explicitly warns that lint and typecheck cannot catch visual bugs. | High | Medium — layout regressions land unnoticed between releases | Add `tests/render.rs` with `TestBackend` snapshots at 120×40 and 80×24 during Phase 2, per `ARCHITECTURE.md` §"Testing strategy" |
| R11 | **Non-atomic config writes.** `src/state.rs:52` is a plain `fs::write`; a crash mid-write truncates `.forestui-config.json`. Same as Python, so not a regression — but the port was the moment to fix it, and the loader silently falls back to an empty repository list (`src/state.rs:29-34`), so the damage looks like "all my repos disappeared". | Low | High when it happens | Write to a temp file in the same directory and `rename` — four lines |
| R12 | **`naturaltime` is a re-implementation, not `humanize`.** `src/util.rs:51-87` matches on the six tested cases but is not the same algorithm; boundary phrasings ("a year ago" vs "1 year ago") can drift from the strings the acceptance blocks recorded. | Medium | Low — cosmetic | Assert the exact strings in the UC-12/UC-16 Expected blocks, or accept a documented divergence |

---

## 9. Post-merge checklist — cutting the first Rust release

1. **Fix R1 first.** Replace `src/cli.rs:9` with `env!("CARGO_PKG_VERSION")`; confirm
   `forestui --version` on a tagged build and that the tmux window is named `forestui`, not
   `forestui-dev-HHMM` (UC-02).
2. **Decide R2 before shipping.** Either gate the self-updater behind a cheap version probe or
   remove it. Shipping it as-is means every user's launch compiles a crate.
3. **Commit `Cargo.lock`** — it is still untracked, and `release.yml` plus `install.sh` both pass
   `--locked`, which fails without it. (`/target` is already ignored.)
4. **Add `rust-toolchain.toml`** pinning the channel consistent with `rust-version = "1.90"`.
5. **Claim the crate name.** `cargo publish` a `0.0.1` from the maintainer account (R6), and set
   `CARGO_REGISTRY_TOKEN` in the repository secrets — `release.yml:75-79` needs it.
6. **Final Python release** (R8): one commit on the `v1.3.1` lineage that prints the migration
   notice at startup, tagged `v1.3.2`. `publish.yml` is already deleted on the rewrite branch, so
   cut this from the old lineage (or `uv build && uv publish` by hand) — and do it *before* the
   rewrite merges, or the tooling to build a wheel is gone from `main`.
7. **Run the full `tu` playbook** on the release binary; record 25/25 P0 and update the four
   divergent Expected blocks (UC-07, 11, 43, 50) plus the six mouse-driven steps.
8. **Tag `v1.4.0`.** `gh release create v1.4.0 --generate-notes` triggers `release.yml`: three
   `cargo build --release --locked --target …` jobs, tarballs plus `.sha256`, uploaded with
   `gh release upload --clobber`, then `crates-io` publishes.
9. **Verify the artefacts before announcing:** download each tarball, check the sha256, run
   `./forestui --version` on macOS arm64 and Linux x86_64. `x86_64-apple-darwin` is cross-compiled
   and untested in CI (R7) — test it on real hardware or drop the target.
10. **Verify both install paths end to end:**
    `curl -fsSL …/install.sh | bash` on a clean machine, and `cargo install forestui --locked`.
    Confirm the script's `uv tool uninstall forestui` hint fires for a machine that still has the
    Python build.
11. **Confirm config coexistence by hand:** run Python `1.3.1` against a scratch forest, add a repo
    and a worktree, quit; run the Rust binary against the same forest; confirm both entries load,
    archive one with `h`, quit; run Python `1.3.1` again and confirm it still parses the file. This
    is the only real proof of §5.
12. **Update `README.md` badges/links** (crates.io, releases) and remove the "Python 3.14+ / uv"
    requirements block.
