# forestui — Implementation-Grade Behavioral Specification

**Purpose.** Complete behavioral description of the existing Python/Textual `forestui`
application, sufficient to reimplement it in Rust byte-for-byte in observable behavior.
Everything here describes what the code **does**, not what it should do. Quirks and bugs are
documented as behavior.

**Source snapshot.** `main` @ `3021d72` ("Bump pypa/gh-action-pypi-publish to v1.14.2 (#25)"),
parent `b8f2bc5` ("Fix crash when selecting a stale worktree (#24)").
Total application source: 6,331 lines across `forestui/` + `tests/`.

**Runtime dependency versions that determine observable behavior** (`pyproject.toml:20-26`):

```
click>=8.1.0
humanize>=4.9.0
libtmux>=0.37.0
pydantic>=2.12.5
textual>=8.2.4
```

Resolved in the checked-in lock: `textual 8.2.4`, `rich` (Textual transitive), `libtmux`,
`pydantic 2.x`. Textual 8.2.4 and Rich's markup parser are *load-bearing* — several visible
strings are silently mangled by markup parsing (see §11.6). A Rust port that renders the same
literal strings without a markup parser will **not** match the current display; §11.6 gives the
exact rule to reproduce.

---

## Table of contents

1. [Purpose & runtime model](#1-purpose--runtime-model)
2. [CLI surface](#2-cli-surface)
3. [Data models & on-disk schemas](#3-data-models--on-disk-schemas)
4. [Screen inventory](#4-screen-inventory)
5. [Keybinding table](#5-keybinding-table)
6. [Every external command executed](#6-every-external-command-executed)
7. [State machine](#7-state-machine)
8. [Async / reactive behavior](#8-async--reactive-behavior)
9. [tmux integration](#9-tmux-integration)
10. [GitHub integration](#10-github-integration)
11. [Theme / visual](#11-theme--visual)
12. [Error handling & edge cases](#12-error-handling--edge-cases)
13. [Behavioral invariants](#13-behavioral-invariants)
14. [Appendix: dead code inventory](#14-appendix-dead-code-inventory)
15. [Build, test and release](#15-build-test-and-release-context-only)

Section 13 carries **132 numbered invariants**; sections 6, 9 and 11.6 are the ones most likely
to be got wrong by a reimplementation.

---

## 1. Purpose & runtime model

### 1.1 What the app is

A terminal UI for managing Git worktrees. It tracks a user-curated list of Git repositories,
creates/renames/archives/deletes worktrees under a "forest" directory, and launches editors,
shells, file managers and Claude Code sessions into dedicated tmux windows. It reads Claude
Code's on-disk session history to offer "resume session" buttons, and reads GitHub issues via
the `gh` CLI to offer "create worktree from issue".

Two-pane layout: a fixed-width sidebar tree (repositories → worktrees) on the left, a scrolling
detail pane on the right whose content is determined entirely by the sidebar selection.

### 1.2 tmux requirement and exec-into-tmux

forestui **requires** tmux and refuses to run outside it. `cli.main()` calls `ensure_tmux()`
before anything else (`cli.py:200`). `ensure_tmux` (`cli.py:43-164`) does:

1. If `$TMUX` is set → return immediately; the process continues in-place as the TUI
   (`cli.py:51-52`).
2. Else look up `tmux` on `PATH` via `shutil.which("tmux")` (`cli.py:55`). If absent, print the
   following to **stderr** and `sys.exit(1)` (`cli.py:57-63`):

   ```
   Error: forestui requires tmux to be installed.

   Install tmux:
     macOS:  brew install tmux
     Ubuntu: sudo apt install tmux
     Fedora: sudo dnf install tmux
   ```

3. Compute the session name (see §1.3) and rebuild its own command line (see §9.1).
4. Probe `tmux has-session -t =<session>`; branch on whether the session already exists
   (§9.2 / §9.3). Both branches end in `os.execvp("tmux", …)` — the Python process is
   **replaced**, so `ensure_tmux` never returns in the not-inside-tmux path.

The re-executed `forestui` runs inside tmux, `$TMUX` is set, and step 1 short-circuits.

### 1.3 Forest directory concept & multi-forest support

The "forest" directory is where worktrees are created and where per-forest state lives. It is a
**positional CLI argument, not a setting** (`cli.py:168`).

| Aspect | Value | Source |
|---|---|---|
| Default | `Path.home() / "forest"` | `services/settings.py:11` |
| Runtime override | `set_forest_path(argv)` → `Path(path).expanduser().resolve()` | `services/settings.py:17-23` |
| Accessor | `get_forest_path()` | `services/settings.py:26-30` |
| State file | `<forest>/.forestui-config.json` | `state.py:29-33` |
| Created if absent | yes — `forest_dir.mkdir(parents=True, exist_ok=True)` on every `_get_config_path()` call | `state.py:32` |
| Worktree path formula | `<forest>/<repo.name>/<worktree.name>` | `app.py:505`, `app.py:679`, `modals.py:282`, `modals.py:340`, `modals.py:606` |

Multi-forest: each forest directory gets an independent `.forestui-config.json`, so
`forestui ~/work` and `forestui ~/personal` have completely separate repository lists. The tmux
session name is derived from the forest folder name (§9.1), so each forest also gets its own
tmux session. Global user preferences (§3.6) are shared across all forests.

Coexistence with `forest` (macOS): forestui uses `.forestui-config.json` inside the forest dir;
`forest` uses `.forest-config.json` in `~/.config/forest/`. They never collide.

### 1.4 Dev mode

`--dev` (`cli.py:181-186`) only changes the tmux **window name**:

```python
def get_window_name(dev_mode: bool = False) -> str:
    """Get the tmux window name based on dev mode flag."""
    if dev_mode:
        from datetime import datetime

        hhmm = datetime.now().strftime("%H%M")
        return f"forestui-dev-{hhmm}"
    return "forestui"
```
— `cli.py:16-23`

Dev mode is **force-enabled** when the installed version string is `"0.0.0"`
(`cli.py:198`: `dev_mode = dev_mode or __version__ == "0.0.0"`). `__version__` is read from
installed package metadata and falls back to `"0.0.0"` when the package is not installed
(`__init__.py:5-9`), i.e. when running from source (`uv run forestui`). Consequence: running
from a source checkout always uses a timestamped window name, so multiple dev instances do not
collide in the reattach-detection scan (`cli.py:101-104` matches both `"forestui"` and any name
starting with `"forestui-dev-"`).

`--debug` (`cli.py:175-180`) sets `os.environ["TEXTUAL"] = "devtools"` **after** `ensure_tmux`
(`cli.py:202-203`), enabling Textual's devtools console connection.

### 1.5 Process lifecycle

```
forestui [args]                       ← click entrypoint, project.scripts = forestui.app:main
  → forestui.app.main()               app.py:1042-1046 → delegates to forestui.cli.main
    → cli.main()                       cli.py:188
      → dev_mode |= (__version__ == "0.0.0")
      → ensure_tmux(...)               cli.py:200 — execs into tmux if $TMUX unset
      → os.environ["TEXTUAL"]="devtools" if --debug
      → os.environ["FORESTUI_NO_AUTO_UPDATE"]="1" if --no-self-update
      → rename_tmux_window(get_window_name(dev_mode))   cli.py:208
      → set_forest_path(forest_path)   cli.py:214
      → run_app()                      cli.py:215
        → ForestApp().run()            app.py:1028-1029
```

Top-level crash handling (`app.py:1021-1038`): any exception escaping `app.run()` is caught,
the traceback is written to `~/.forestui-error.log`, the traceback plus
`\nError: {e}` plus `\nError log written to: {path}` are printed to stderr, then the process
blocks on `input("Press Enter to exit...")` and exits with status **1**.

---

## 2. CLI surface

Declared with Click (`cli.py:167-196`).

```python
@click.command()
@click.argument("forest_path", required=False, default=None)
@click.option(
    "--no-self-update",
    "no_self_update",
    is_flag=True,
    help="Disable automatic updates on startup",
)
@click.option(
    "--debug",
    "debug_mode",
    is_flag=True,
    help="Run with Textual devtools enabled",
)
@click.option(
    "--dev",
    "dev_mode",
    is_flag=True,
    help="Dev mode: use timestamped window name (forestui-dev-HHMM)",
)
@click.version_option(version=__version__, prog_name="forestui")
def main(
    forest_path: str | None, no_self_update: bool, debug_mode: bool, dev_mode: bool
) -> None:
```

### 2.1 Arguments and options

| Token | Kind | Default | Effect |
|---|---|---|---|
| `FOREST_PATH` | positional, optional | `None` → `~/forest` | Sets the forest directory; `expanduser().resolve()` applied (`services/settings.py:23`). Also drives the tmux session name (§9.1) and is re-appended, shell-quoted, to the re-exec command line (`cli.py:81-82`). |
| `--no-self-update` | flag | off | Sets `FORESTUI_NO_AUTO_UPDATE=1` in the environment (`cli.py:205-206`), which makes `_auto_update()` return immediately (`app.py:200-201`). |
| `--debug` | flag | off | Sets `TEXTUAL=devtools` (`cli.py:202-203`). |
| `--dev` | flag | off (but forced on when version == `0.0.0`) | Timestamped tmux window name `forestui-dev-HHMM`. |
| `--version` | flag | — | Click's `version_option`, `prog_name="forestui"`. Prints `forestui, version <X>` and exits 0. |
| `--help` | flag | — | Click default. Help body is the `main` docstring: `forestui - Git Worktree Manager` / `A terminal UI for managing Git worktrees, inspired by forest for macOS.` / `FOREST_PATH: Optional path to forest directory (default: ~/forest)`. Exits 0. |

There is **no** `--self-update` flag. Self-update is on by default and opt-out only. (Historical:
commit `042d8a0` replaced `--self-update` with `--no-self-update`; commit `5d5e2e1`/`7929eea`
migrated from git-pull-based updating to `uv tool upgrade`.)

### 2.2 Self-update behavior

Implemented as `ForestApp._auto_update` (`app.py:182-238`), decorated `@work(thread=True)` —
runs on a Textual worker **thread**, started from `on_mount` (`app.py:120`).

```python
result = subprocess.run(
    ["uv", "tool", "upgrade", "forestui"],
    capture_output=True,
    text=True,
    timeout=120,
)
```
— `app.py:206-211`

Decision table on the combined `stdout + stderr` (`app.py:213-238`):

| Condition | Title suffix set to |
|---|---|
| `FORESTUI_NO_AUTO_UPDATE` set (any truthy value) | *(never runs; returns before touching the title)* |
| — before the call — | `"checking for updates..."` |
| exit 0 and output contains `"Nothing to upgrade"` | `None` (suffix cleared) |
| exit 0 and output contains `"Updated"` and regex `\+ forestui==(\d+\.\d+\.\d+)` matches | `f"updated to v{version} - restart to apply"` |
| exit 0 and output contains `"Updated"` but regex does not match | `"updated - restart to apply"` |
| exit 0, neither string present | `None` |
| exit ≠ 0 | `None` |
| `CalledProcessError` / `TimeoutExpired` / `OSError` (e.g. `uv` not installed) | `None` |

Title format (`app.py:177-180`):

```python
def _set_title_suffix(self, suffix: str | None) -> None:
    """Update title with optional suffix."""
    base = f"forestui v{__version__}"
    self.title = f"{base} ({suffix})" if suffix else base
```

`App.TITLE` is initialized to `f"forestui v{__version__}"` (`app.py:68`); the title is rendered
in the Textual `Header`.

The update is **never applied to the running process** — the user must restart.

### 2.3 Exit codes

| Code | Cause |
|---|---|
| 0 | Normal quit (`q` → `App.action_quit`); `--help`; `--version` |
| 1 | tmux not found (`cli.py:63`); unhandled exception in `run_app` (`app.py:1038`) |
| 2 | Click usage error (unknown option, too many arguments) — Click default |
| *(replaced)* | On the not-inside-tmux path the process is replaced by `os.execvp`, so forestui's own exit code is whatever `tmux attach-session` / `tmux new-session` returns |

### 2.4 Environment variables

| Variable | Read at | Semantics |
|---|---|---|
| `TMUX` | `cli.py:51`, `services/tmux.py:41` | Presence ⇒ already inside tmux. `TmuxService.is_inside_tmux` gates the entire tmux service; when unset, `server`/`session` return `None` and every window-creating method returns `False`/`None`. |
| `TMUX_PANE` | `services/tmux.py:119` | Pane id used to locate *our own* window for renaming. When unset, `current_window` is `None` and `rename_window` returns `False`. |
| `SHELL` | `services/tmux.py:352` | Interactive shell used to wrap the Claude command. Default `"/bin/bash"`. |
| `FORESTUI_NO_AUTO_UPDATE` | `app.py:200` | Set by `--no-self-update`; any truthy value disables the update check. Also honoured if exported externally. |
| `TEXTUAL` | written `cli.py:203` | Set to `"devtools"` by `--debug`; consumed by Textual. |
| `HOME` | via `Path.home()` | Base for `~/.config/forestui/settings.json`, `~/forest`, `~/.claude/projects`, `~/.forestui-error.log`. |
| `LIBTMUX_TMUX_FORMAT_SEPARATOR` | libtmux `formats.py:14` | Separator used in libtmux `-F` format strings; default `␞` (U+241E). |

---

## 3. Data models & on-disk schemas

All models are Pydantic v2 `BaseModel` subclasses in `models.py`.

### 3.1 Validation constants and helpers (`models.py:12-67`)

```python
MAX_CLAUDE_COMMAND_LENGTH = 200
MAX_BUTTON_LABEL_LENGTH = 20
MAX_BUTTON_PREFIX_LENGTH = 20
```

```python
def derive_prefix(label: str) -> str:
    """Derive a tmux-safe window prefix from a button label.

    Lowercase, keep [a-z0-9_-], collapse other runs to '-', strip leading/trailing '-'.
    """
    slug = re.sub(r"[^a-z0-9_-]+", "-", label.lower()).strip("-")
    return slug[:MAX_BUTTON_PREFIX_LENGTH]
```
— `models.py:18-24`. Note the truncation happens **after** stripping, so a truncated prefix may
end in `-`. Verified: `derive_prefix("New Session: YoloDisc!") == "new-session-yolodisc"`.

| Validator | Rules | Returns |
|---|---|---|
| `validate_button_label(label)` (`models.py:27-35`) | non-empty; `len ≤ 20`; must not contain any of `\n \r \t \0` | error string or `None` |
| `validate_button_prefix(prefix)` (`models.py:38-46`) | non-empty; `len ≤ 20`; `re.fullmatch(r"[a-z0-9_-]+", prefix)` | error string or `None` |
| `validate_claude_command(cmd)` (`models.py:49-67`) | empty is **valid** (returns `None`); `len ≤ 200`; must not contain `\n \r \t \0` | error string or `None` |

Exact error strings (used verbatim in the UI):
`"Label cannot be empty"`, `"Label too long (max 20 characters)"`,
`"Label cannot contain control characters"`, `"Prefix cannot be empty"`,
`"Prefix too long (max 20 characters)"`,
`"Prefix must be lowercase letters, digits, '-' or '_'"`,
`"Command too long (max 200 characters)"`,
`"Command cannot contain newlines or control characters"`,
`"Command cannot be empty"` (raised only by the field validator / the modal, `models.py:105`,
`modals.py:856`).

### 3.2 `CustomClaudeButton` (`models.py:70-111`)

| Field | Type | Default | Semantics |
|---|---|---|---|
| `label` | `str` | required | Button text. Validated by `validate_button_label`; invalid → `ValueError`. |
| `prefix` | `str` | required | tmux window prefix; the window is named `<prefix>:<name>`. Validated by `validate_button_prefix`. |
| `command` | `str` | required | Executed verbatim (wrapped in `$SHELL -ic`). Validated by `validate_claude_command`; additionally **empty is rejected** (`models.py:104-105`). |

Derived property:

```python
@property
def is_yolo_style(self) -> bool:
    """Whether this button's command enables dangerous permissions bypass."""
    return "--dangerously-skip-permissions" in self.command
```
— `models.py:108-111`. Pure substring test; drives red styling only.

### 3.3 `Worktree` (`models.py:114-131`)

| Field | Type | Default | Semantics |
|---|---|---|---|
| `id` | `UUID` | `uuid4()` | Stable identity; selection key. |
| `name` | `str` | required | Directory basename under `<forest>/<repo>/`; also the tmux window suffix. |
| `branch` | `str` | required | Branch checked out in the worktree (as recorded by forestui, not re-read from git). |
| `path` | `str` | required | Absolute path as a string. |
| `is_archived` | `bool` | `False` | Archived worktrees are excluded from `active_worktrees()`. |
| `sort_order` | `int \| None` | `None` | Manual ordering key; `None` sorts last (`float("inf")`). Never written by any reachable code path (§14). |
| `last_modified` | `datetime` | `datetime.now(UTC)` at construction | Secondary sort key (descending). Never refreshed by reachable code (§14). |
| `base_branch` | `str \| None` | `None` | Branch this worktree was created from, e.g. `origin/main`. |
| `created_from_ref` | `str \| None` | `None` | Short commit hash of `base_branch` at creation time. |

`get_path()` → `Path(self.path).expanduser()` (`models.py:129-131`). Note: **no** `.resolve()`.

### 3.4 `Repository` (`models.py:134-167`)

| Field | Type | Default |
|---|---|---|
| `id` | `UUID` | `uuid4()` |
| `name` | `str` | required — always `Path(entered_path).name` (`app.py:660-661`) |
| `source_path` | `str` | required |
| `worktrees` | `list[Worktree]` | `[]` |

Ordering functions:

```python
def active_worktrees(self) -> list[Worktree]:
    """Get active (non-archived) worktrees sorted by order/recency."""
    active = [w for w in self.worktrees if not w.is_archived]
    return sorted(
        active,
        key=lambda w: (
            w.sort_order if w.sort_order is not None else float("inf"),
            -w.last_modified.timestamp(),
        ),
    )

def archived_worktrees(self) -> list[Worktree]:
    """Get archived worktrees sorted by recency."""
    archived = [w for w in self.worktrees if w.is_archived]
    return sorted(archived, key=lambda w: -w.last_modified.timestamp())
```
— `models.py:146-160`

Because `sort_order` is always `None` in practice, the effective active ordering is
**`last_modified` descending** (newest-created first). Python's `sorted` is stable.

### 3.5 `ClaudeSession` (`models.py:170-188`)

| Field | Type | Default | Semantics |
|---|---|---|---|
| `id` | `str` | required | Session UUID = JSONL filename stem. |
| `title` | `str` | required | First eligible user message, ≤100 chars; `"Untitled session"` when empty (`claude_session.py:147`). |
| `last_message` | `str` | `""` | Last eligible user message, ≤100 chars. |
| `last_timestamp` | `datetime` | required | Max timestamp seen; falls back to file mtime. |
| `message_count` | `int` | required | Count of user-role records. |
| `git_branches` | `list[str]` | `[]` | Deduplicated, insertion-ordered union of every `gitBranches` array. |

Properties: `relative_time` → `humanize.naturaltime(self.last_timestamp)` (`models.py:180-183`);
`primary_branch` → `git_branches[0] or None` (`models.py:185-188`, unused).

### 3.6 `Settings` (`models.py:191-203`)

| Field | Type | Default | Read by |
|---|---|---|---|
| `default_editor` | `str` | `"vim"` | `app.py:893`; `SettingsModal` Select |
| `default_terminal` | `str` | `""` | **never read** (§14) |
| `branch_prefix` | `str` | `"feat/"` | `app.py:745`, `app.py:489`; modals |
| `theme` | `str` | `"system"` | persisted only; **never applied** — the CSS is a single hard-coded dark theme (§14) |
| `custom_buttons` | `list[CustomClaudeButton]` | `[]` | `app.py:287`, `app.py:325` |

### 3.7 `Selection` (`models.py:206-220`)

| Field | Type | Default |
|---|---|---|
| `repository_id` | `UUID \| None` | `None` |
| `worktree_id` | `UUID \| None` | `None` |

`is_repository` ⇔ `repository_id is not None and worktree_id is None`;
`is_worktree` ⇔ `worktree_id is not None`. Not persisted — selection resets on every launch.

### 3.8 GitHub models (`models.py:223-258`)

`GitHubLabel{name: str, color: str = ""}`; `GitHubUser{login: str}`.

`GitHubIssue`:

| Field | Type | Default |
|---|---|---|
| `number` | `int` | required |
| `title` | `str` | required |
| `state` | `str` | required |
| `url` | `str` | required |
| `created_at` | `datetime` | required |
| `updated_at` | `datetime` | required |
| `author` | `GitHubUser` | required |
| `assignees` | `list[GitHubUser]` | `[]` |
| `labels` | `list[GitHubLabel]` | `[]` |

```python
@property
def branch_name(self) -> str:
    """Generate branch-safe name from issue. e.g., '42-fix-login-bug'."""
    slug = re.sub(r"[^a-z0-9]+", "-", self.title.lower())[:40].strip("-")
    return f"{self.number}-{slug}"
```
— `models.py:249-253`. Note: substitution first, **then** truncate to 40, **then** strip `-`.
Uppercase letters are lowercased before the character class is applied, so they survive.

`relative_time` → `humanize.naturaltime(self.updated_at)` (`models.py:255-258`).

### 3.9 `AppStateData` (`state.py:14-17`)

```python
class AppStateData(BaseModel):
    """Serializable app state data."""

    repositories: list[Repository] = []
```

### 3.10 On-disk schema: `<forest>/.forestui-config.json`

Written by `AppState._save_state` (`state.py:47-53`) as
`json.dump(AppStateData(...).model_dump(mode="json"), f, indent=2, default=str)`, UTF-8.
Read by `AppState._load_state` (`state.py:35-45`) via `AppStateData.model_validate(json.load(f))`;
`json.JSONDecodeError` and `OSError` are swallowed, leaving the repository list empty.
Note: a Pydantic `ValidationError` on a structurally-wrong file is **not** caught and will
propagate out of `AppState.__init__` → `ForestApp.__init__` → `run_app`'s handler (§12.9).

Exact real output (generated from the models, `indent=2`, keys in declaration order):

```json
{
  "repositories": [
    {
      "id": "15ef13a4-db4a-4612-b142-080dbf1efd96",
      "name": "forestui",
      "source_path": "/Users/x/work/repos/forestui",
      "worktrees": [
        {
          "id": "bfc7805f-5784-4b51-a3d5-3e72e6cfe334",
          "name": "my-feature",
          "branch": "feat/my-feature",
          "path": "/Users/x/forest/forestui/my-feature",
          "is_archived": false,
          "sort_order": null,
          "last_modified": "2026-08-14T19:22:06.540157Z",
          "base_branch": "origin/main",
          "created_from_ref": "a1b2c3d"
        }
      ]
    }
  ]
}
```

Serialization notes a Rust port must match:
- `id` — canonical lowercase hyphenated UUID string.
- `last_modified` — Pydantic v2 JSON mode: RFC-3339 with **microsecond** precision and a
  literal `Z` suffix for UTC (not `+00:00`).
- `sort_order: null` and `base_branch`/`created_from_ref: null` are always emitted (Pydantic
  does not exclude `None`).
- No trailing newline is written (`json.dump` does not append one).
- The forest directory is created (`parents=True, exist_ok=True`) on **every** call to
  `_get_config_path()` — including on load — so launching forestui against a non-existent path
  creates it (`state.py:32`).

### 3.11 On-disk schema: `~/.config/forestui/settings.json`

Path is a class attribute: `Path.home() / ".config" / "forestui" / "settings.json"`
(`services/settings.py:38`). Written by `save_settings` (`services/settings.py:60-65`) as
`json.dump(settings.model_dump(), f, indent=2)` — **python mode**, not JSON mode; all fields are
`str`/`list[dict]`, so the result is valid JSON regardless. Parent directory is created with
`parents=True, exist_ok=True`. Load errors (`json.JSONDecodeError`, `OSError`) fall back to
`Settings.default()`; a Pydantic `ValidationError` is **not** caught.

Exact real output:

```json
{
  "default_editor": "vim",
  "default_terminal": "",
  "branch_prefix": "feat/",
  "theme": "system",
  "custom_buttons": [
    {
      "label": "YoloDisc",
      "prefix": "yolodisc",
      "command": "claude --dangerously-skip-permissions"
    }
  ]
}
```

With no custom buttons, `"custom_buttons": []`.

### 3.12 Third on-disk artifact: `~/.forestui-error.log`

Plain text, overwritten on each crash, containing only `traceback.format_exc()` (`app.py:1031-1033`).

---

## 4. Screen inventory

### 4.0 Layout skeleton

```python
def compose(self) -> ComposeResult:
    """Compose the application UI."""
    yield Header()
    with Horizontal(id="main-container"):
        yield Sidebar(
            repositories=self._state.repositories,
            selected_repo_id=self._state.selection.repository_id,
            selected_worktree_id=self._state.selection.worktree_id,
            show_archived=self._state.show_archived,
        )
        with VerticalScroll(id="detail-pane"):
            yield EmptyState()
    yield Footer()
```
— `app.py:96-108`

Vertical order top→bottom: **Header** (Textual default, 1 row when not tall; shows
`App.title`) → **`#main-container`** (`height: 100%`, `layout: horizontal`) → **Footer**
(Textual default, 1 row; shows all bindings with `show=True`).

Inside `#main-container`, left→right:

| Region | id | Width | Other CSS |
|---|---|---|---|
| Sidebar | `#sidebar` | `35` cells, `min-width: 30`, `max-width: 45` | `background: $bg`, `border-right: solid $border` |
| Detail pane | `#detail-pane` | `1fr` | `height: 100%`, `background: $bg`, `padding: 1 2` (1 row top/bottom, 2 cols left/right), vertical scroll |

### 4.1 Sidebar (`components/sidebar.py`)

`Sidebar` is a `Static` subclass constructed with `id="sidebar"` (`sidebar.py:108`).

```python
def compose(self) -> ComposeResult:
    """Compose the sidebar UI."""
    # App header box
    with Vertical(id="sidebar-header-box"):
        yield Label(f"gh cli: {self._gh_status}", id="gh-status")
    # Tree view
    tree: Tree[RepoNode | WorktreeNode | ArchivedNode] = Tree(
        "Repositories", id="repo-tree"
    )
    tree.show_root = False
    tree.guide_depth = 2
    yield tree
```
— `sidebar.py:116-127`

**Header box** `#sidebar-header-box`: `width: 100%`, `height: 3`, `background: $bg-elevated`,
`border-bottom: solid $border`, `align: center middle`, `padding: 1 0 0 0`. Contains one
centered, full-width label `#gh-status`, `color: $text-muted` by default.

Initial text: `gh cli: ...` (`self._gh_status = "..."`, `sidebar.py:114`). After
`_check_gh_status` resolves, `set_gh_status` (`sidebar.py:228-255`) maps:

| `status` | `username` | Displayed text | Class added |
|---|---|---|---|
| `"authenticated"` | non-empty | `gh cli: ok (<login>)` | `gh-status-ok` (green `#52B788`) |
| `"authenticated"` | `None` | `gh cli: ok` | `gh-status-ok` |
| `"not_authenticated"` | — | `gh cli: unauth'd` | `gh-status-warn` (amber `#FFB347`) |
| `"not_installed"` | — | `gh cli: missing` | `gh-status-error` (muted `#7A7A7A`) |
| anything else | — | `gh cli: <status>` | `gh-status-error` |

The three classes are removed before the new one is added (`sidebar.py:247`). The whole update
is wrapped in `try/except Exception: pass` (`sidebar.py:243-255`).

**Tree** `#repo-tree`: root hidden (`show_root = False`), `guide_depth = 2`.
CSS: `Tree { background: $bg; padding: 0 1; }`, cursor row `background: $accent-dark;
color: $text-primary`, highlight/highlight-line `background: $bg-hover`.

Population (`_populate_tree`, `sidebar.py:133-163`) — the tree is fully cleared and rebuilt:

```python
for repo in self._repositories:
    # Add repository node
    repo_label = f" {repo.name}"
    repo_node = tree.root.add(repo_label, data=RepoNode(repo), expand=True)

    # Add active worktrees
    for worktree in repo.active_worktrees():
        prefix = "├─" if worktree != repo.active_worktrees()[-1] else "└─"
        wt_label = f"{prefix}  {worktree.name} [{worktree.branch}]"
        repo_node.add_leaf(wt_label, data=WorktreeNode(repo, worktree))
```

- Repository label = one leading **ASCII space** + repo name (`" forestui"`). Repository nodes
  are always created **expanded**.
- Worktree label = `├─` for every worktree except the last (`└─`), then **two** spaces, then the
  worktree name, then ` [branch]`. The `[branch]` portion is normally invisible (§11.6).
- Last-element detection compares by Pydantic model **equality** (`worktree != …[-1]`), not
  identity. Two worktrees with identical field values would both render `└─`.
- `repo.active_worktrees()` is recomputed inside the loop on every iteration (O(n² log n));
  behaviorally irrelevant.
- Archived worktrees are appended under a node labelled `" Archived"` (one leading space),
  created collapsed, with child labels `f"   {worktree.name} ({repo.name})"` (three leading
  spaces) — **but only when `self._show_archived` is `True`**, which never happens (§14). The
  archived section is unreachable in the shipped app.

Empty state: with no repositories, the tree renders as an empty widget below the header box —
there is no placeholder text in the sidebar.

### 4.2 `EmptyState` (`app.py:52-62`)

Mounted in `#detail-pane` when nothing is selected.

```python
with Vertical(classes="empty-state"):
    yield Label(" forestui", classes="label-accent")
    yield Label("Git Worktree Manager", classes="label-secondary")
    yield Label("")
    yield Label("Select a repository or worktree", classes="label-muted")
    yield Label("or press [a] to add a repository", classes="label-muted")
```

`.empty-state { align: center middle; height: 100%; }`, and
`.empty-state Label { color: $text-muted; text-align: center; }` — the `text-muted` colour
overrides `.label-accent`/`.label-secondary` by specificity, so **all five lines render muted
grey**, centered vertically and horizontally.

Verbatim rendered lines (note line 5 — see §11.6):

```
 forestui
Git Worktree Manager

Select a repository or worktree
or press  to add a repository
```

### 4.3 `RepositoryDetail` (`components/repository_detail.py:99-223`)

Mounted when `selection.repository_id` is set and `selection.worktree_id` is `None`.
Root: `Vertical(classes="detail-content")` (`height: auto; width: 100%`).

Section order, top→bottom:

| # | Content | Widget/class | Conditional |
|---|---|---|---|
| 1 | `MAIN REPOSITORY` | Label `.section-header` (bold, `$text-secondary`, `margin: 1 0 0 0`) | always |
| 2 | `Repository: <name>` | Label `.detail-title` (bold, `$text-primary`) | always |
| 3 | `Branch:     <branch>` | Label `.label-accent` (`#52B788`) | only if `current_branch` non-empty |
| 4 | `Commit:     <short-hash>` + ` (<relative>)` | Label `.label-muted` | only if `commit_hash` non-empty; the parenthetical only if `commit_time` is not `None` |
| 5 | Action row: sync button, `Add Worktree` | `Horizontal.action-row` (`height: 3; margin: 1 0`) | always |
| 6 | `Rule()` | horizontal rule, `color: $border`, `margin: 1 2 1 0` | always |
| 7 | `LOCATION` | `.section-header` | always |
| 8 | `<source_path>` | Label `.path-display .label-secondary` (bg `$bg-elevated`, border solid `$border`, `padding: 0 1`) | always |
| 9 | `Rule()` | | always |
| 10 | `OPEN IN` | `.section-header` | always |
| 11 | Action row: ` Editor`, ` Terminal`, ` Files` (ids `btn-editor`, `btn-terminal`, `btn-files`, variant `default`) | `Horizontal.action-row` | always |
| 12 | `Rule()` | | always |
| 13 | `CLAUDE` | `.section-header` | always |
| 14 | Claude button rows, 4 buttons per `Horizontal.action-row` | see below | always |
| 15 | `RECENT SESSIONS` | `.section-header` | always |
| 16 | `#sessions-container` (`Vertical`, `margin-top: 1`) | initially one Label `Loading...` `.label-muted` | always |
| 17 | `Rule()` | | always |
| 18 | Header row: `MY OPEN GITHUB ISSUES` + refresh button `↻` (id `btn-refresh-issues`, class `refresh-btn`) | `Horizontal.section-header-row` | always |
| 19 | `#issues-container` (`Vertical`, `margin-top: 1`) | initially one Label `Loading...` `.label-muted` | always |
| 20 | `Rule()` | | always |
| 21 | `MANAGE` | `.section-header` | always |
| 22 | Action row: ` Remove Repository` (id `btn-remove-repo`, variant `error`, class `-destructive`) | | always |

Sync button (`repository_detail.py:126-135`), mutually exclusive:

| Condition | Label | State |
|---|---|---|
| `has_remote` true | `⟳ Git Pull` | enabled |
| `has_remote` false | `⟳ Git Pull (No remote)` | **disabled** |

Both carry `id="btn-sync"`, `variant="default"`.

Claude button construction (`repository_detail.py:161-198`) — spec list, in order:

1. `("New Session", "btn-claude-new", "primary", False)`
2. `("New Session: YOLO", "btn-claude-yolo", "error", True)`
3. …then one entry per configured custom button, in settings order:
   `(btn.label, f"btn-claude-custom-{btn.prefix}", "error" if btn.is_yolo_style else "primary", btn.is_yolo_style)`

Entries are chunked **4 per row**; each chunk becomes one `Horizontal.action-row`. Entries with
`is_destructive` also get `classes="-destructive"`.

`RECENT SESSIONS` content after `update_sessions` (`repository_detail.py:287-377`):
- If sessions list is empty → single Label `No sessions found` `.label-muted`.
- Otherwise, first **5** sessions, each rendered as a `Vertical.session-item`
  (`background: $bg-elevated; border: solid $border; padding: 1; margin: 0 2 1 0`) containing:
  - Label `.session-title`: `session.title` truncated to 60 chars, with literal `...` appended
    iff the original exceeded 60.
  - Label `.session-last .label-secondary`: `> <last_message truncated to 40>` + `...` iff
    >40 — **only if** `last_message` is non-empty *and* differs from `title`.
  - Label `.session-meta .label-muted`: `f"{session.relative_time} • {session.message_count} msgs"`
    (U+2022 bullet).
  - One or more `Horizontal.session-buttons` (`align: right middle`), 4 buttons per row, from
    the spec list: `("Resume", f"btn-resume-{id}", "default", False)`,
    `("YOLO", f"btn-yolo-{id}", "error", True)`, then one per custom button
    `(label, f"btn-custom-{prefix}-{id}", …)`. All get class `session-btn`
    (`min-width: 8; height: 3; margin-left: 1`), destructive ones also `-destructive`.

`MY OPEN GITHUB ISSUES` content after `update_issues` (`repository_detail.py:418-458`):
- Empty → single Label `No issues found` `.label-muted`.
- Otherwise first **5** issues, each a `Horizontal.issue-row`
  (`padding: 1; background: $bg-elevated; border: solid $border; margin: 0 2 1 0`) containing:
  - `Vertical.issue-info` (`width: 1fr`) with
    Label `.issue-title` = `f"#{number} {title[:45]}"` + `...` iff title >45,
    Label `.issue-meta .label-muted` = `relative_time`, plus ` • <label1, label2>` when the issue
    has labels (first 2 only, joined with `", "`).
  - Button `Create WT`, id `btn-issue-<number>`, class `issue-btn` (`min-width: 12; margin-left: 1`).

Refresh button spinner: `.refresh-btn` is a 3×1 borderless transparent button coloured
`$text-muted`, `$accent` on hover.

### 4.4 `WorktreeDetail` (`components/worktree_detail.py:99-268`)

Mounted when `selection.worktree_id` is set. Same skeleton, different sections:

| # | Content | Conditional |
|---|---|---|
| 1 | `WORKTREE` `.section-header` | always |
| 2 | `Repository: <repo.name>` `.detail-title` | always |
| 3 | `Worktree:   <wt.name>` `.label-primary` | always |
| 4 | `Branch:     <wt.branch>` `.label-accent` | always |
| 5 | `⚠ MISSING:   directory no longer exists on disk` `.label-destructive` | only when `path_exists` is `False` |
| 6 | `Based on:   <base_branch>` + ` (<created_from_ref>)` `.label-muted` | only if `base_branch` set; parenthetical only if `created_from_ref` set |
| 7 | `Commit:     <hash>` + ` (<relative>)` `.label-muted` | only if `commit_hash` non-empty |
| 8 | Action row with sync button | always |
| 9 | `Rule()` / `LOCATION` / path label | always |
| 10 | `Rule()` / `OPEN IN` / row of ` Editor`, ` Terminal`, ` Files` | always |
| 11 | `Rule()` / `CLAUDE` / Claude button rows (identical construction to §4.3) | always |
| 12 | `RECENT SESSIONS` / `#sessions-container` with `Loading...` | always |
| 13 | `Rule()` / `RENAME` / two action rows, each with one `Input` | always |
| 14 | `Rule()` / `MANAGE` / archive-or-unarchive + delete | always |

Sync button, three-way (`worktree_detail.py:140-156`), evaluated in this order:

| Condition | Label | State |
|---|---|---|
| `not path_exists` | `⟳ Git Pull (Directory missing)` | disabled |
| `has_remote` | `⟳ Git Pull` | enabled |
| else | `⟳ Git Pull (No remote)` | disabled |

LOCATION label (`worktree_detail.py:161-171`):

| Condition | Text | Classes |
|---|---|---|
| `path_exists` | `<wt.path>` | `path-display label-secondary` |
| missing | `<wt.path>  (missing)` (two spaces before `(`) | `path-display label-destructive` |

RENAME inputs (`worktree_detail.py:233-244`):

| id | Initial value | Placeholder |
|---|---|---|
| `input-worktree-name` | `worktree.name` | `Worktree name` |
| `input-branch-name` | `worktree.branch` | `Branch name` |

Each lives in its own `Horizontal.action-row`. There is no explicit "apply" button — submission
is Enter (`on_input_submitted`, `worktree_detail.py:320-332`), and the message is only posted
when the value is non-empty **and** different from the current value.

MANAGE row (`worktree_detail.py:250-268`):

| Condition | Buttons |
|---|---|
| `worktree.is_archived` | ` Unarchive` (id `btn-unarchive`, variant `default`), ` Delete` (id `btn-delete`, variant `error`, class `-destructive`) |
| otherwise | ` Archive` (id `btn-archive`, variant `default`), ` Delete` (same) |

`RECENT SESSIONS` rendering is byte-identical to §4.3 (the two `update_sessions` methods are
duplicated code, `repository_detail.py:287-377` ≡ `worktree_detail.py:334-424`).

### 4.5 `AddRepositoryModal` (`components/modals.py:34-147`)

`ModalScreen[str | None]`. `ModalScreen { align: center middle; }`; container
`.modal-container` is `width: 80`, `max-width: 90%`, `height: auto`, `max-height: 90%`,
`background: $bg-elevated`, `border: solid $border`, `padding: 1 2`.

Contents, in order:
1. Label ` Add Repository` `.modal-title` (bold, centered, `margin-bottom: 1`)
2. Label `Repository Path` `.section-header`
3. `Input` id `input-path`, placeholder `Enter path or paste from clipboard...`
4. Label id `label-status`, initially empty, class `label-secondary`
5. `Checkbox` id `checkbox-import`, label `Import existing worktrees`, initially unchecked
6. `Horizontal.modal-buttons` (`margin-top: 1; align: center middle; height: 3`;
   each button `margin: 0 1`): `Cancel` (id `btn-cancel`, variant `default`),
   `Add Repository` (id `btn-add`, variant `primary`)

Live path validation on every keystroke (`_validate_path`, `modals.py:85-119`), evaluated in
order; each branch sets the status label text and swaps classes between `label-secondary` and
`label-destructive`:

| Condition | Status text | Class |
|---|---|---|
| empty input | `""` | `label-secondary` |
| `Path(p).expanduser()` does not exist | `Path does not exist` | `label-destructive` |
| not a directory | `Path is not a directory` | `label-destructive` |
| `<path>/.git` does not exist | `Not a git repository` | `label-destructive` |
| otherwise | `Repository: <path.name>` | `label-secondary` |

Note: `.git` existence is checked with the **filesystem**, not `git rev-parse`; a worktree's
`.git` **file** satisfies it.

Submit paths: Enter in the input (`modals.py:80-83`) or `btn-add`. `_add_repository`
(`modals.py:133-143`) silently returns when `self._path` is empty, or when the path does not
exist, or `<path>/.git` does not exist — the modal stays open with no new feedback. Otherwise it
posts `RepositoryAdded(str(expanded_path), import_worktrees)` and dismisses with the path
string. Note `str(path)` is the `expanduser()`-ed path, **not** resolved.

`escape` → `action_cancel` → `dismiss(None)`.

### 4.6 `AddWorktreeModal` (`components/modals.py:150-354`)

`ModalScreen[tuple[str, str, bool] | None]`. Contents:

1. Label ` Add Worktree` `.modal-title`
2. Label `to <repo.name>` `.label-secondary`
3. Label `Worktree Name` `.section-header`
4. `Input` id `input-name`, placeholder `my-feature`
5. Label id `label-path-preview` `.label-muted`, initially empty
6. Label `Branch` `.section-header`
7. `Horizontal.action-row`: ` New Branch` (id `btn-new-branch`, variant `primary`),
   ` Existing` (id `btn-existing-branch`, variant `default`)
8. `Input` id `input-branch`, placeholder `<branch_prefix>my-feature`
9. `BranchSearchInput` id `branch-search` — **hidden on mount** (`modals.py:232-235`)
10. Label id `label-error` `.label-destructive`, initially empty
11. `Horizontal.modal-buttons`: `Cancel` (id `btn-cancel`), `Create Worktree` (id `btn-create`,
    variant `primary`)

Mode toggle (`_set_new_branch_mode`, `modals.py:299-320`):

| Mode | `btn-new-branch` variant | `btn-existing-branch` variant | `input-branch` | `branch-search` |
|---|---|---|---|---|
| new (default) | `primary` | `default` | visible | hidden |
| existing | `default` | `primary` | hidden | visible |

Name sanitization on every keystroke (`modals.py:274-276`):
`"".join(c for c in name if c.isalnum() or c in "-_")` — note this uses Python's Unicode-aware
`str.isalnum()`, so non-ASCII letters and digits survive. **The `Input` widget's own value is
not rewritten**; only the internal `self._name`, the path preview, and the auto-derived branch
name use the sanitized form. So typing `my feature` leaves `my feature` visible in the input
while the created worktree is named `myfeature`.

Path preview (`_update_path_preview`, `modals.py:278-285`): when `_name` non-empty, the label
shows `f" {forest_dir / repo.name / name}"` (one leading space); otherwise empty.

Branch auto-population (`modals.py:242-249`): while in **new-branch** mode, every change to the
name input sets `input-branch.value = f"{branch_prefix}{sanitized_name}"`. This fires
`Input.Changed` for `input-branch`, which sets `self._branch` to the same value.

Create-button gating (`_update_create_button_state`, `modals.py:265-272`): disabled iff
*existing-branch mode* **and** the current branch string is not in the supplied `branches` list.
In new-branch mode the button is always enabled.

Validation on submit (`_create_worktree`, `modals.py:322-350`), in order, writing to
`#label-error` and returning:

| Condition | Error text (verbatim, one leading space) |
|---|---|
| `not self._name` | ` Worktree name is required` |
| `not self._branch` | ` Branch name is required` |
| existing-branch mode and branch not in list | ` Branch '<branch>' does not exist` |
| `<forest>/<repo>/<name>` exists on disk | ` Worktree path already exists` |

On success: posts `WorktreeCreated(repo.id, name, branch, new_branch)` and dismisses with
`(name, branch, new_branch)`.

Any `Input.Changed` clears the error label first (`modals.py:237-240`).

### 4.7 `BranchSearchInput` (`components/branch_search.py:17-164`)

A `Vertical` composite, used inside `AddWorktreeModal`. Composes (`branch_search.py:88-92`):

1. `Input(placeholder="Start typing to search branches...", value=<initial>)`
2. `Label("", classes="match-count")` — `color: $text-muted; height: 1; padding: 0 1`
3. `OptionList()` — `max-height: 10; height: auto; background: $bg; border: solid $border;
   margin-top: 0`; highlighted option `background: $accent-dark`

On mount it immediately populates results for the initial value (`branch_search.py:94-96`).

`_update_results(query)` (`branch_search.py:131-154`):
- `matches = fuzzy_match_branches(query, branches, remotes=remotes)` (max 50, §4.7.1)
- clears and repopulates the `OptionList`; each option's **display** is
  `highlight_match(query, branch)` and its **id** is the raw branch name
- match-count label text:

| Condition | Text |
|---|---|
| `query.strip()` empty and `shown < total` | `<shown> of <total> branches` |
| `query.strip()` empty and `shown == total` | `<total> branches` |
| `shown == 0` | `No matches` |
| `shown == 1` | `1 match` |
| `shown > 1` | `<shown> matches` |

Selecting an option (`branch_search.py:109-129`) sets the `Input` value to the branch (with the
re-entrancy guard `_updating_from_selection` set so the resulting `Input.Changed` is ignored),
re-runs `_update_results(branch)`, posts `Changed` and `BranchSelected`, and refocuses the Input.

`Input.Changed` is **stopped** (`event.stop()`, `branch_search.py:100`) so it never reaches the
parent modal; the parent listens for `BranchSearchInput.Changed` instead
(`on_branch_search_input_changed`, `modals.py:255-259`).

#### 4.7.1 Fuzzy matching algorithm (`utils.py`)

`MAX_DROPDOWN_RESULTS = 50` (`utils.py:9`).

`strip_remote_prefix(branch, remotes)` (`utils.py:31-40`): if `branch` contains `/` and the part
before the first `/` is in `remotes`, return the remainder; else return `branch` unchanged.

`_match_score(query, branch, remotes) -> float | None` (`utils.py:43-117`), lower is better,
`None` means no match. `q = query.lower()`, `b = branch.lower()`:

| Order | Condition | Score |
|---|---|---|
| 1 | `q` empty | `0.0` |
| 2 | `q == b` | `0.0` |
| 3 | `q == strip_remote_prefix(b, remotes)` | `0.5` |
| 4 | `b.startswith(q)` | `1.0` |
| 5 | `local.startswith(q)` | `1.5` |
| 6 | `q` found in `b` or `local` at index 0 or preceded by one of `/ - _ .` | `2.0` |
| 7 | `q` found anywhere in `b` or `local` | `3.0` |
| 8 | `len(q) >= 2`: split `b` on `[/\-_.]`; for each non-empty segment, Levenshtein(`q`, seg) ≤ `threshold` → `4.0 + dist*0.1`; and if `len(seg) > len(q)`, Levenshtein(`q`, `seg[:len(q)]`) ≤ threshold → `4.5 + dist*0.1`; take the minimum | `4.0`–`4.9` |
| 9 | otherwise | `None` |

`threshold = max(1, (len(q) + 2) // 3)` (`utils.py:92`).
Levenshtein is the standard two-row DP (`utils.py:12-28`), with the recursive swap so `s1` is
the longer string.

`fuzzy_match_branches(query, branches, remotes=None, max_results=50)` (`utils.py:120-150`):
- If `query.strip()` is empty → return `[(b, 0.0) for b in branches[:max_results]]` (input order
  preserved — i.e. whatever order `list_branches` produced, which is sorted).
- Otherwise score every branch, drop `None`s, sort by `(score, branch.lower())`, truncate to
  `max_results`.

`highlight_match(query, branch) -> rich.text.Text` (`utils.py:153-173`): if `query` is empty,
plain text. Otherwise find the first case-insensitive occurrence of `query` in `branch`; if
found, emit three segments with the middle styled `"bold reverse"`; if not found (pure fuzzy
match), emit the branch unstyled.

`FuzzyBranchSuggester` (`branch_search.py:167-184`) is a Textual `Suggester` with
`use_cache=False, case_sensitive=False`; `get_suggestion(value)` returns the single best fuzzy
match, or `None` for empty input / no matches. Used for the *inline ghost-text* suggestion on
the base-branch input in §4.9.

### 4.8 `SettingsModal` (`components/modals.py:357-480`)

`ModalScreen[Settings | None]`. Container has classes `modal-container modal-wide`
(`width: 140; max-width: 95%; height: 90%`).

Structure:
1. Label ` Settings` `.modal-title`
2. `VerticalScroll` classes `modal-scroll modal-scroll-tall` (`height: 1fr; max-height: 100vh`)
   containing:
   - Label `DEFAULT EDITOR` `.section-header`
   - `Select` id `select-editor`, value = `settings.default_editor`
   - Label `BRANCH PREFIX` `.section-header`
   - `Input` id `input-branch-prefix`, value = `settings.branch_prefix`, placeholder `feat/`
   - Label `THEME` `.section-header`
   - `Select` id `select-theme`, value = `settings.theme`
   - Label `CUSTOM CLAUDE BUTTONS` `.section-header`
   - Label id `label-buttons-count` `.label-muted`, text from `_buttons_summary()`
   - `Horizontal.action-row` with `Manage Custom Buttons...` (id `btn-manage-buttons`)
3. `Horizontal.modal-buttons`: `Cancel` (id `btn-cancel`), `Save` (id `btn-save`, variant `primary`)

Editor options (`modals.py:364-375`) — `(display, value)` pairs, in this exact order:

```python
EDITORS = [
    ("VS Code", "code"),
    ("Cursor", "cursor"),
    ("Neovim (tmux)", "nvim"),
    ("Vim (tmux)", "vim"),
    ("Helix (tmux)", "hx"),
    ("Emacs TUI (tmux)", "emacs -nw"),
    ("PyCharm", "pycharm"),
    ("Sublime Text", "subl"),
    ("Nano (tmux)", "nano"),
    ("Micro (tmux)", "micro"),
]
```

Theme options (`modals.py:377-381`): `("System", "system")`, `("Dark", "dark")`,
`("Light", "light")`.

`_buttons_summary()` (`modals.py:432-438`):

| count | Text |
|---|---|
| 0 | `No custom buttons configured` |
| 1 | `1 custom button configured` |
| n>1 | `<n> custom buttons configured` |

Save (`_save_settings`, `modals.py:461-476`) builds a **new** `Settings` from
`select-editor.value` (falling back to `"code"` when falsy), `input-branch-prefix.value` verbatim
(not trimmed, may be empty), `select-theme.value` (fallback `"system"`), and the working copy of
custom buttons. `default_terminal` is not carried over — it resets to `""`. Dismisses with the
new `Settings`; Cancel/escape dismiss with `None`.

### 4.9 `CreateWorktreeFromIssueModal` (`components/modals.py:519-749`)

`ModalScreen[tuple[str, str, bool, bool] | None]`. Structure:

1. Label `Create Worktree from Issue #<number>` `.modal-title`
2. Label `<issue.title>` classes `issue-title-preview label-muted` (`margin-bottom: 1`)
3. Label `Worktree Name` `.section-header`
4. `Input` id `input-name`, value = `issue.branch_name`, placeholder `worktree-name`
5. Label id `path-preview` `.label-muted`, text `Path: <forest>/<repo>/<name>`
6. Label `Branch Name` `.section-header`
7. `Input` id `input-branch`, value = `f"{branch_prefix}{issue.branch_name}"`, placeholder `feat/branch-name`
8. Label `Base Branch` `.section-header`
9. `Horizontal.base-branch-row` (`height: 3; width: 100%`) with
   `Input` id `input-base-branch` (`width: 1fr`), value = computed default, placeholder
   `origin/main`, `suggester=FuzzyBranchSuggester(branches, remotes)`, and
   `Button("Fetch", id="btn-fetch")` (`min-width: 10; margin-left: 1`)
10. `Checkbox` id `checkbox-pull`, label `Pull repo before creating`, **checked by default**
11. `Horizontal.modal-buttons`: `Cancel` (id `btn-cancel`), `Create` (id `btn-create`, variant `primary`)

Default base branch (`_compute_default_base_branch`, `modals.py:581-592`), in order:
1. first `<remote>/<current_branch>` present in `branches`, iterating `remotes` in order;
2. else `current_branch` if present in `branches`;
3. else `branches[0]`;
4. else `""`.

Name changes update the path preview live (`modals.py:636-639`). **No sanitization is applied**
here (unlike §4.6).

Create-button gating (`modals.py:646-653`): disabled iff base-branch is non-empty **and** not in
`branches`. An empty base branch leaves the button enabled.

Fetch (`_start_fetch`, `modals.py:679-692`): guarded by `_is_fetching`; sets the button label to
`|`, disables it, starts a `0.1`s interval spinner cycling `|`, `/`, `-`, `\` (`modals.py:577`),
and posts `FetchRequested(repo.source_path)`. On success the app calls
`update_branches(branches, remotes)` (`modals.py:703-724`) which stops the spinner, restores the
label to `Fetch`, re-enables the button, installs a fresh `FuzzyBranchSuggester`, and — if the
current base branch is empty or no longer in the list — recomputes and rewrites the default. On
failure the app calls `fetch_failed(error)` (`modals.py:726-729`) which only resets the button;
the error itself surfaces as an app notification `Fetch failed: <err>` (`app.py:567`).

Create (`modals.py:666-677`) requires non-empty `_name` **and** non-empty `_branch`; otherwise
the press is silently ignored. It posts `WorktreeCreated(repo.id, name, branch, True,
pull_first, base_branch)` — `new_branch` is hard-coded `True` — and dismisses with
`(name, branch, True, pull_first)`.

### 4.10 `CustomButtonsModal` (`components/modals.py:876-1011`)

`ModalScreen[list[CustomClaudeButton] | None]`, container `modal-container modal-wide`. Operates
on a deep-ish copy (`[b.model_copy() for b in buttons]`, `modals.py:884`) so Cancel discards.

1. Label ` Custom Claude Buttons` `.modal-title`
2. Label `Order here matches display order in the Claude section.` `.label-muted`
3. `VerticalScroll` id `buttons-list`, classes `modal-scroll modal-scroll-tall`
4. `Horizontal.action-row` with ` Add Button` (id `btn-add`, variant `primary`)
5. `Horizontal.modal-buttons`: `Cancel` (id `btn-cancel`), `Save` (id `btn-save`, variant `primary`)

Row rendering (`_build_rows`, `modals.py:907-949`):
- Empty list → single Label `No buttons yet. Click Add.` `.label-muted`.
- Otherwise, per button, a `Vertical.session-item` containing one
  `Horizontal.session-header-row` with:
  - `Vertical.session-info`: Label `<label>` + ` (YOLO)` when `is_yolo_style`, class
    `session-title`; Label `prefix: <prefix>` `.session-meta .label-muted`;
    Label `$ <command>` `.session-meta .label-muted`.
  - `Horizontal.session-buttons`: `↑` (id `btn-up-<idx>`, disabled at idx 0), `↓`
    (id `btn-down-<idx>`, disabled at last idx), `Edit` (id `btn-edit-<idx>`),
    `Delete` (id `btn-delete-<idx>`, variant `error`, class `-destructive`). All class `session-btn`.

Actions rebuild the whole list (`_rerender`, `modals.py:951-955`). Delete pops by index with no
confirmation. Save dismisses with the working list; Cancel/escape dismiss `None`.

### 4.11 `CustomButtonEditModal` (`components/modals.py:752-873`)

`ModalScreen[CustomClaudeButton | None]`.

Title: `" Edit Button"` when editing, `" Add Button"` when adding (`modals.py:775`, prefixed with
one space at `modals.py:781`).

Fields (each followed by a `.label-muted` helper line):

| id | max_length | Placeholder | Helper text |
|---|---|---|---|
| `input-label` | 20 | `e.g., YoloDisc` | `Shown on the button (e.g., 'New Session: YoloDisc')` |
| `input-prefix` | 20 | `e.g., yolodisc` | `Window prefix: <prefix>:<worktree>. Auto-derived from label until you edit it.` |
| `input-command` | 200 | `e.g., claude --dangerously-skip-permissions` | `Run as-is. If it contains --dangerously-skip-permissions the button is styled red.` |

Then Label id `label-edit-error` `.label-destructive`, then `Cancel` / `Save`.

Prefix auto-follow (`modals.py:767-772`, `827-839`): `_follows` starts `True` for a new button,
and for an existing button it starts as `existing.prefix == derive_prefix(existing.label)`. While
`_follows`, every label keystroke overwrites the prefix input with `derive_prefix(label)` (with
`_suppress_prefix_event` set so the resulting `Input.Changed` is ignored). A manual prefix edit
sets `_follows = (typed_prefix == derive_prefix(current_label))`, so re-typing the derived value
re-arms auto-follow.

Save validation (`_save`, `modals.py:847-870`), strings are `.strip()`ed first, checks run in
this order and the **first** error wins:
1. `validate_button_label(label)`
2. `validate_button_prefix(prefix)`
3. `validate_claude_command(command)` if command non-empty, else the literal `"Command cannot be empty"`
4. `label in other_labels` → ` Another button already uses this label`
5. `prefix in other_prefixes` → ` Another button already uses this prefix`

Errors render as `f" {error}"` (one leading space) in `#label-edit-error`. On success, dismisses
with a constructed `CustomClaudeButton`.

Note: because the tuple in step 1-3 is fully evaluated before iteration, all three validators run
even when the first fails — harmless, they are pure.

### 4.12 `ConfirmDeleteModal` (`components/modals.py:483-516`)

`ModalScreen[bool]`. Contents: Label `f" {title}"` classes `modal-title label-destructive`;
Label `<message>` `.label-secondary`; buttons `Cancel` (id `btn-cancel` → `dismiss(False)`) and
`Delete` (id `btn-delete`, variant `error`, class `-destructive` → `dismiss(True)`).
Escape → `dismiss(False)`.

Call sites and their exact title/message pairs:

| Trigger | Title | Message |
|---|---|---|
| `RemoveRepositoryRequested` (button) `app.py:428-433` | `Remove Repository` | `Remove '<name>' from forestui?\n(Files will not be deleted)` |
| `DeleteWorktreeRequested` (button) `app.py:593-598` | `Delete Worktree` | `Permanently delete worktree '<name>'?\nThis cannot be undone.` |
| `d` key on a worktree `app.py:802-807` | `Delete Worktree` | `Permanently delete '<name>'?` |
| `d` key on a repository `app.py:819-824` | `Remove Repository` | `Remove '<name>' from forestui?` |

### 4.13 Notifications (toasts)

All user feedback outside modals is `App.notify(...)`, rendered as a Textual toast. Default
timeout 5 s (`textual/app.py:430`), severity `information` unless stated.

| Message (verbatim) | Severity | Site |
|---|---|---|
| `Could not enable focus events` | warning | `app.py:114` |
| `Issue fetch error: <e>` | error | `app.py:156` |
| `Syncing...` | info | `app.py:407`, `app.py:445` |
| `Sync complete` | info | `app.py:410`, `app.py:447` |
| `Sync failed: <e>` | error | `app.py:413`, `app.py:450` |
| `Pulling repo...` | info | `app.py:510` |
| `Created worktree '<name>'` | info | `app.py:538`, `app.py:710` |
| `Failed to create worktree: <e>` | error | `app.py:540`, `app.py:712` |
| `Fetch failed: <e>` | error | `app.py:567` |
| `Path already exists` | error | `app.py:619` |
| `Rename failed: <e>` | error | `app.py:636` |
| `Branch rename failed: <e>` | error | `app.py:653` |
| `Select a repository first` | warning | `app.py:725` |
| `Settings saved` | info | `app.py:837` |
| `a: Add Repo \| w: Add Worktree \| e: Editor \| t: Terminal \| n: Claude \| h: Archive \| d: Delete \| s: Settings \| q: Quit` | info | `app.py:848-851` |
| `Opened <editor> in edit:<name>` | info | `app.py:901` |
| `Opened in <editor-argv0>` | info | `app.py:913` |
| `Editor '<editor>' not found` | error | `app.py:915` |
| `Opened terminal in term:<name>` | info | `app.py:921` |
| `Failed to create terminal window` | error | `app.py:923` |
| `Opened mc in files:<name>` | info | `app.py:929` |
| `Failed to create mc window` | error | `app.py:931` |
| `Started Claude<mode> in <window>` (`<mode>` = ` (YOLO)` or empty) | info | `app.py:939` |
| `Resuming Claude<mode> in <window>` | info | `app.py:956` |
| `Started Claude (<label>) in <window>` | info | `app.py:972` |
| `Resuming Claude (<label>) in <window>` | info | `app.py:989` |
| `Failed to create Claude window` | error | `app.py:941`, `958`, `974`, `991` |
| `Imported <n> worktrees` | info | `app.py:1016` |
| `Failed to import worktrees: <e>` | error | `app.py:1018` |

The help notification is a single concatenated string; note it omits `o` (Files), `y` (YOLO),
`r` (Refresh) and `?` (Help) even though those bindings exist.

---

## 5. Keybinding table

### 5.1 Declared bindings, verbatim

**App level** — `app.py:71-85`:

```python
    BINDINGS = [
        Binding("q", "quit", "Quit", show=True, priority=True),
        Binding("a", "add_repository", "Add Repo", show=True),
        Binding("w", "add_worktree", "Add Worktree", show=True),
        Binding("e", "open_editor", "Editor", show=True),
        Binding("t", "open_terminal", "Terminal", show=True),
        Binding("o", "open_files", "Files", show=True),
        Binding("n", "start_claude", "Claude", show=True),
        Binding("y", "start_claude_yolo", "ClaudeYOLO", show=True),
        Binding("h", "toggle_archive", "Archive", show=True),
        Binding("d", "delete", "Delete", show=True),
        Binding("s", "open_settings", "Settings", show=True),
        Binding("r", "refresh", "Refresh", show=False),
        Binding("?", "show_help", "Help", show=True),
    ]
```

**Sidebar** — `sidebar.py:41-43`:

```python
    BINDINGS = [
        Binding("a", "add_repository", "Add Repo", show=True),
    ]
```

**Modals** — each `ModalScreen` subclass declares exactly one binding:

| Class | Declaration | file:line |
|---|---|---|
| `AddRepositoryModal` | `BINDINGS = [("escape", "cancel", "Cancel"),]` | `modals.py:37-39` |
| `AddWorktreeModal` | `BINDINGS = [("escape", "cancel", "Cancel"),]` | `modals.py:153-155` |
| `SettingsModal` | `BINDINGS = [("escape", "cancel", "Cancel"),]` | `modals.py:360-362` |
| `ConfirmDeleteModal` | `BINDINGS = [("escape", "cancel", "Cancel"),]` | `modals.py:486-488` |
| `CreateWorktreeFromIssueModal` | `BINDINGS = [("escape", "cancel", "Cancel")]` | `modals.py:522` |
| `CustomButtonEditModal` | `BINDINGS = [("escape", "cancel", "Cancel")]` | `modals.py:755` |
| `CustomButtonsModal` | `BINDINGS = [("escape", "cancel", "Cancel")]` | `modals.py:879` |

`RepositoryDetail`, `WorktreeDetail`, `BranchSearchInput` and `EmptyState` declare **no**
bindings; they rely on Textual's built-in widget key handling.

### 5.2 Dispatch semantics (Textual 8.2.4) — required to reproduce behavior

Key events are processed by `App.on_event` (`textual/app.py:4105`):

```python
if not await self._check_bindings(event.key, priority=True):
    forward_target = self.focused or self.screen
    forward_target._forward_event(event)
```

`_check_bindings(key, priority=True)` walks `reversed(self.screen._binding_chain)` — App first,
then Screen, then widgets down to the focused one — and fires the **first** binding whose
`binding.priority` is `True` (`textual/app.py:3947-3956`). Non-priority bindings are checked
later, as the key bubbles up from the focused widget through `Widget._on_key` →
`App._check_bindings(key)` over `self.screen._modal_binding_chain`.

Two consequences that a Rust port must reproduce exactly:

1. **`q` is a priority binding on the App.** It fires *before* the focused widget sees the key,
   and `_binding_chain` (used for priority) is **not** truncated at a modal, unlike
   `_modal_binding_chain`. So pressing `q` while a modal is open — with a Button, Checkbox or
   Select focused — quits the whole application.

2. **Input widgets suppress printable-key bindings.** `Screen._binding_chain`
   (`textual/screen.py:427-438`) removes, from every *outer* namespace's binding map, any key
   that an inner namespace's `check_consume_key` claims. `Input` and `TextArea` consume all
   printable characters. Therefore, whenever an `Input` has focus, **every** single-character app
   binding (`q a w e t o n y h d s r ?`) is stripped from the chain and the character is typed
   normally. Verified against `textual/screen.py:427-438` and `textual/app.py:3947-3956`.

3. **Non-priority app bindings do not reach modals.** `_modal_binding_chain`
   (`textual/screen.py:450-456`) truncates at the first `is_modal` node. With a modal open and a
   non-Input widget focused, `a`/`w`/`e`/… do nothing.

### 5.3 Effective binding table

Context legend: **Main** = main screen, no modal; **Modal** = any `ModalScreen` on the stack.

| Key | Context | Focus | Action | Resulting state change |
|---|---|---|---|---|
| `q` | Main **and** Modal | anything except `Input`/`TextArea` | `App.action_quit` | Process exits, status 0. No state is flushed (state is already saved eagerly on every mutation). |
| `q` | any | `Input` focused | — | Types the character `q`. |
| `a` | Main | tree / non-Input | `Sidebar.action_add_repository` → posts `AddRepositoryRequested` → `ForestApp.action_add_repository` (`app.py:715-717`) | Pushes `AddRepositoryModal`. (When focus is inside the Sidebar the Sidebar binding wins; when focus is elsewhere the App binding wins — same outcome.) |
| `w` | Main | non-Input | `action_add_worktree` (`app.py:719-725`) | If `selection.repository_id` set → `_show_add_worktree_modal(repo_id)` (runs `git branch -a` and `git remote`, then pushes `AddWorktreeModal`). Else notifies `Select a repository first` (warning). |
| `e` | Main | non-Input | `action_open_editor` (`app.py:751-755`) | Resolves the selected path (§7.3); no-op when nothing selected. Opens editor (§9.6). |
| `t` | Main | non-Input | `action_open_terminal` (`app.py:757-761`) | Creates tmux window `term:<name>` (§9.5). |
| `o` | Main | non-Input | `action_open_files` (`app.py:763-767`) | Creates tmux window `files:<name>` running `mc`. |
| `n` | Main | non-Input | `action_start_claude` (`app.py:769-773`) | Creates tmux window `claude:<name>` running `$SHELL -ic claude`. |
| `y` | Main | non-Input | `action_start_claude_yolo` (`app.py:775-779`) | Creates tmux window `yolo:<name>` running `$SHELL -ic 'claude --dangerously-skip-permissions'`. |
| `h` | Main | non-Input | `action_toggle_archive` (`app.py:781-792`) | Only when a **worktree** is selected: flips `is_archived`, persists state, rebuilds sidebar and detail pane. Since `show_archived` is always false, archiving makes the worktree vanish from the sidebar while remaining the current selection (detail pane still shows it, now with an `Unarchive` button). No effect when a repository is selected. |
| `d` | Main | non-Input | `action_delete` (`app.py:794-828`), `@work` | Worktree selected → `ConfirmDeleteModal("Delete Worktree", "Permanently delete '<name>'?")`; on confirm runs `git worktree remove` (errors suppressed), removes from state, refreshes. Repository selected → `ConfirmDeleteModal("Remove Repository", "Remove '<name>' from forestui?")`; on confirm removes from state only (no filesystem changes). |
| `s` | Main | non-Input | `action_open_settings` (`app.py:830-839`), `@work` | Pushes `SettingsModal` and awaits it. On non-`None` result: `save_settings()` writes `~/.config/forestui/settings.json`, notifies `Settings saved`, and re-renders the detail pane so custom-button changes take effect. |
| `r` | Main | non-Input | `action_refresh` (`app.py:841-844`) | Rebuilds sidebar from in-memory state and re-renders the detail pane (which re-runs the git probes and re-dispatches session/issue fetches). Hidden from the Footer (`show=False`). |
| `?` | Main | non-Input | `action_show_help` (`app.py:846-851`) | Emits the one-line help notification (§4.13). |
| `escape` | Modal | any (including `Input`) | `action_cancel` of the top modal | Dismisses: `None` for all modals except `ConfirmDeleteModal`, which dismisses `False`. `escape` is not a printable character so `Input` does not consume it. |
| `Enter` | Main | tree node | `Tree.NodeSelected` → `Sidebar.on_tree_node_selected` (`sidebar.py:179-197`) | See §7.2. |
| `↑`/`↓` | Main | tree | Textual `Tree` cursor movement → `Tree.NodeHighlighted` → `Sidebar.on_tree_node_highlighted` (`sidebar.py:199-203`) | **Selection follows the cursor**: merely moving over a node posts `RepositorySelected`/`WorktreeSelected`, which re-renders the detail pane and re-runs its git probes. |
| `Enter` | Main | `#input-worktree-name` | `WorktreeDetail.on_input_submitted` (`worktree_detail.py:320-332`) | If value non-empty and ≠ current name → posts `RenameWorktreeRequested` (§7.6). |
| `Enter` | Main | `#input-branch-name` | same handler | If value non-empty and ≠ current branch → posts `RenameBranchRequested`. |
| `Enter` | Modal `AddRepositoryModal` | `#input-path` | `on_input_submitted` (`modals.py:80-83`) | Same as pressing `Add Repository`. |
| `Enter` | Modal, `OptionList` in `BranchSearchInput` | | `OptionList.OptionSelected` | Sets the input to the selected branch, refocuses the input. |
| `Tab`/`Shift+Tab` | any | | Textual focus traversal | Standard. |
| `Ctrl+P` | any | | Textual command palette | Standard Textual default binding, not overridden. |

Footer contents, left→right, in `BINDINGS` order excluding `show=False`:
`q Quit`, `a Add Repo`, `w Add Worktree`, `e Editor`, `t Terminal`, `o Files`, `n Claude`,
`y ClaudeYOLO`, `h Archive`, `d Delete`, `s Settings`, `? Help`.

---

## 6. Every external command executed

This is the authoritative section. Argv arrays are written as literal lists; `<…>` denotes
interpolation. All subprocesses inherit forestui's environment unmodified.

### 6.1 `git` — all via `GitService._run_git`

Every git invocation goes through one helper (`services/git.py:41-66`):

```python
    @staticmethod
    async def _run_git(
        *args: str, cwd: str | Path | None = None
    ) -> tuple[int, str, str]:
        """Run a git command and return exit code, stdout, stderr.

        Raises GitError if the process cannot be spawned at all — most commonly
        a stale worktree whose directory has been deleted, which makes `cwd`
        invalid and raises FileNotFoundError.
        """
        cmd = ["git", *args]
        try:
            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                cwd=str(cwd) if cwd else None,
            )
        except OSError as e:
            raise GitError(f"Failed to run git in {cwd}: {e}") from e
        stdout, stderr = await process.communicate()
        return (
            process.returncode or 0,
            stdout.decode("utf-8").strip(),
            stderr.decode("utf-8").strip(),
        )
```

Universal contract:
- `git` is resolved from `PATH`. No shell. stdout/stderr captured as pipes.
- Both streams are decoded as **UTF-8 (strict)** and `.strip()`ed of surrounding whitespace.
  A non-UTF-8 byte in git output raises `UnicodeDecodeError`, which is **not** caught anywhere.
- Return code `None` (impossible after `communicate()`) is coerced to `0`.
- `cwd=None` when the caller passes a falsy path; all real callers pass a path.
- **Spawn failure** (missing directory, missing `git` binary, permission error) → `GitError`
  with message `Failed to run git in <cwd>: <OSError repr>`. This is the fix from `b8f2bc5`.
- Every caller path prefixes `Path(path).expanduser()` — **no** `resolve()`.

| # | argv | cwd | Called from | Exit-code / output handling | Failure behavior |
|---|---|---|---|---|---|
| G1 | `git rev-parse --git-dir` | repo path | `is_git_repository` `git.py:73` | `code == 0` → `True` | **dead code** — no caller (§14). Returns `False` early if the path does not exist. |
| G2 | `git branch --show-current` | repo/worktree path | `get_current_branch` `git.py:79` | non-zero → `GitError("Failed to get current branch: <stderr>")`; stdout, or the literal `"HEAD"` when stdout is empty (detached HEAD) | Callers wrap in `try/except GitError`: `app.py:296-301` sets `branch = ""`; `app.py:474-479` falls back to `current_branch = "main"`; `app.py:685-687` propagates into the worktree-create worker where it is caught as `GitError` and notified. |
| G3 | `git remote` | repo path | `list_remotes` `git.py:127` | non-zero → `GitError("Failed to list remotes: <stderr>")`; else split stdout on `\n`, strip, drop empties | `_safe_list_remotes` (`git.py:132-137`) swallows `GitError` → `[]`. Direct callers in `app.py` catch `GitError` and use `[]`. |
| G4 | `git branch -a --format=%(refname:short)` | repo path | `list_branches` `git.py:98-100` | non-zero → `GitError("Failed to list branches: <stderr>")` | Callers fall back to `branches = []`. |
| G5 | `git worktree add -b <branch> <abs-worktree-path> [<base_branch>]` | repo path | `create_worktree`, new-branch path, `git.py:167-170` | non-zero → `GitError("Failed to create worktree: <stderr>")` | Notified as `Failed to create worktree: <e>`; no state is written. |
| G6 | `git branch --unset-upstream <branch>` | **the new worktree path** | `create_worktree` `git.py:177-179` | result ignored entirely | Runs only when G5 succeeded **and** `base_branch` is truthy **and** `base_branch` starts with `<remote>/` for some remote in G3's output. Prevents git's `branch.autoSetupMerge` from tracking the remote base. |
| G7 | `git worktree add --track -b <local-branch> <abs-path> <remote>/<branch>` | repo path | `create_worktree`, existing-remote-branch path, `git.py:193-201` | non-zero → `GitError("Failed to create worktree: <stderr>")` | `<local-branch>` = the branch string with the matched `<remote>/` prefix removed (`git.py:191`). The first remote in G3 order whose `<remote>/` prefixes the branch wins. |
| G8 | `git worktree add <abs-path> <branch>` | repo path | `create_worktree`, existing-local-branch path, `git.py:205-207` | same | |
| G9 | `git worktree remove <abs-path>` | repo path | `remove_worktree` `git.py:219-224` | non-zero → **retry** as G10 | |
| G10 | `git worktree remove --force <abs-path>` | repo path | `remove_worktree` recursion `git.py:228` | non-zero → `GitError("Failed to remove worktree: <stderr>")` | Both delete call sites wrap in `contextlib.suppress(GitError)` (`app.py:600`, `app.py:809`) — **deletion of the state entry proceeds regardless**. |
| G11 | `git branch -m <old> <new>` | **the worktree path** | `rename_branch` `git.py:237-239` | non-zero → `GitError("Failed to rename branch: <stderr>")` | Notified `Branch rename failed: <e>`; state untouched. |
| G12 | `git worktree repair <abs-new-path>` | repo path | `repair_worktree` `git.py:249-251` | non-zero → `GitError("Failed to repair worktree: <stderr>")` | Runs **after** the directory has already been renamed on disk; a failure leaves the directory renamed but git metadata stale, and the state update is skipped (§12.4). |
| G13 | `git worktree list --porcelain` | repo path | `list_worktrees` `git.py:258-260` | non-zero → `GitError("Failed to list worktrees: <stderr>")` | See parser below. |
| G14 | `git rev-parse --short <ref>` | repo path | `get_ref` `git.py:306-308` | non-zero → returns `None` (no exception); else `stdout.strip() or None` | Used to snapshot `created_from_ref`. |
| G15 | `git log -1 --format=%H\|%h\|%ct` | repo/worktree path | `get_latest_commit` `git.py:317-319` | non-zero → `GitError("Failed to get latest commit: <stderr>")`; stdout split on `\|` must yield exactly 3 parts else `GitError("Unexpected git log output format")`; `timestamp = datetime.fromtimestamp(int(parts[2]), tz=UTC)` | Caught at both call sites (`app.py:278`, `app.py:316`) leaving `commit_hash = ""`, `commit_time = None`, `has_remote = False`. Note the `except GitError` block also skips the subsequent `has_remote_tracking` call. |
| G16 | `git fetch` | repo path | `fetch` `git.py:332` | non-zero → `GitError("Failed to fetch: <stderr>")` | Only from the issue modal's Fetch button; error is notified and the modal's button resets. |
| G17 | `git pull` | repo/worktree path | `pull` `git.py:339` | non-zero → `GitError("Failed to pull: <stderr>")` | Notified `Sync failed: <e>`. Note: no `--ff-only`, no `--rebase` — whatever the user's git config does. |
| G18 | `git rev-parse --abbrev-ref --symbolic-full-name @{u}` | repo/worktree path | `has_remote_tracking` `git.py:346-348` | returns `code == 0 and bool(stdout.strip())` | Never raises on non-zero exit; only a spawn failure raises `GitError`. |

**G13 porcelain parser** (`git.py:264-296`) — reproduce exactly:

```python
for line in stdout.split("\n"):
    line = line.strip()
    if line.startswith("worktree "):
        if current_path and current_head:
            worktrees.append(WorktreeInfo(current_path, current_head, current_branch))
        current_path = line[9:]
        current_head = None
        current_branch = None
    elif line.startswith("HEAD "):
        current_head = line[5:]
    elif line.startswith("branch "):
        # refs/heads/branch-name -> branch-name
        current_branch = line[7:].replace("refs/heads/", "")
    elif line == "" and current_path and current_head:
        worktrees.append(WorktreeInfo(current_path, current_head, current_branch))
        current_path = None
        current_head = None
        current_branch = None

# Don't forget the last one
if current_path and current_head:
    worktrees.append(WorktreeInfo(current_path, current_head, current_branch))
```

Notes: lines are `.strip()`ed *before* the prefix tests; `detached`, `bare`, `locked`, `prunable`
markers are ignored; `refs/heads/` is removed with `str.replace` (all occurrences, not just the
prefix). Since `_run_git` already strips the trailing newline from stdout, the blank-line branch
fires only for interior blanks; the final record is emitted by the trailing `if`.

### 6.2 `gh` — all via `GitHubService._run_gh`

```python
    @staticmethod
    async def _run_gh(
        *args: str, cwd: str | Path | None = None
    ) -> tuple[int, str, str]:
        """Run a gh command and return (exit_code, stdout, stderr)."""
        try:
            process = await asyncio.create_subprocess_exec(
                "gh",
                *args,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                cwd=str(cwd) if cwd else None,
            )
            stdout, stderr = await process.communicate()
            return (
                process.returncode or 0,
                stdout.decode().strip(),
                stderr.decode().strip(),
            )
        except FileNotFoundError:
            return (-1, "", "gh not found")
```
— `services/github.py:39-59`

Contract differences from git: only `FileNotFoundError` is caught (a missing *cwd* raises
`NotADirectoryError`/`FileNotFoundError` too — both are `OSError`, and `FileNotFoundError` is
what a missing directory raises, so it is covered; a permission error is **not**). The sentinel
exit code for "gh missing" is **`-1`**. Decoding uses the default (UTF-8) codec.

| # | argv | cwd | Called from | Handling |
|---|---|---|---|---|
| H1 | `gh auth status` | `None` (inherits forestui's cwd) | `get_auth_status` `github.py:69` | `-1` → status `not_installed`; `0` → `authenticated`, then H2; anything else → `not_authenticated`. Result is memoized for the process lifetime (`github.py:66-67`) — never re-probed, even by the 5-minute refresh timer. |
| H2 | `gh api user --jq .login` | `None` | `get_auth_status` `github.py:76` | `username = stdout if code == 0 and stdout else None` |
| H3 | `gh repo view --json owner,name` | repository `source_path` | `get_repo_info` `github.py:85-87` | non-zero or empty stdout → `None`; else `json.loads` and read `data["owner"]["login"]`, `data["name"]`; `JSONDecodeError`/`KeyError` → `None` |
| H4 | `gh issue list --assignee @me --state open --limit <limit> --json number,title,state,url,createdAt,updatedAt,author,assignees,labels` | repository `source_path` | `_fetch_issues` `github.py:145-157` | Only when `assigned_to_me` (always `True`). On `code == 0 and stdout`, `json.loads(stdout)` is iterated; **an unparseable body raises `JSONDecodeError` out of the worker**. |
| H5 | `gh issue list --author @me --state open --limit <limit> --json <same fields>` | repository `source_path` | `_fetch_issues` `github.py:165-177` | Only when `authored_by_me` (always `True`). Issue numbers already seen from H4 are skipped. |

`limit` is always `10` (the default at `github.py:101`, never overridden).

After both calls, `issues.sort(key=lambda i: i.created_at, reverse=True)` then `issues[:limit]`
(`github.py:184-185`). The UI then shows only the first 5 (`repository_detail.py:429`).

Issue parsing (`_parse_issue`, `github.py:187-230`) is defensive:
- `author` missing/None/non-dict → login `"unknown"`.
- `assignees`/`labels` must be lists; each element must be a dict with the required key.
- `label.color` defaults to `""`.
- `createdAt`/`updatedAt` are parsed with `datetime.fromisoformat(s.replace("Z", "+00:00"))`;
  a malformed value raises `ValueError` out of the worker.
- `number` is `int(v)` when `v` is `int|str`, else `0`.

### 6.3 `tmux` — direct `subprocess` calls in `cli.py`

These run **before** the TUI starts, only on the not-inside-tmux path.

| # | argv | Options | file:line | Handling |
|---|---|---|---|---|
| T1 | `["tmux", "has-session", "-t", "=<session_name>"]` | `capture_output=True` | `cli.py:87-90` | `returncode == 0` ⇒ session exists. The `=` prefix forces exact-name matching. |
| T2 | `["tmux", "list-windows", "-t", "=<session_name>", "-F", "#{window_name}"]` | `capture_output=True, text=True` | `cli.py:95-99` | stdout split on lines; a forestui window is present iff any line equals `"forestui"` or starts with `"forestui-dev-"`. Errors are ignored (empty stdout ⇒ "no window"). |
| T3 | `["tmux", "new-window", "-t", "=<session_name>", "-n", "<window_name>", "<forestui_cmd>"]` | *(no capture — output goes to the terminal)* | `cli.py:108-118` | Only when T2 found no forestui window. Return code ignored. `<forestui_cmd>` is a **single argv element**, a shell command string tmux runs via its default shell. |
| T4 | `["tmux", "new-session", "-d", "-s", "<session>-<pid>", "-t", "=<session_name>"]` | `capture_output=True` | `cli.py:124-127` | Creates a **grouped** session sharing the base session's windows. Non-zero ⇒ fall through to T5. |
| T5 | `os.execvp("tmux", ["tmux", "attach-session", "-t", "<session_name>"])` | — | `cli.py:130` | Fallback when T4 failed. Process is replaced. |
| T6 | `["tmux", "set-hook", "-t", "<grouped_name>", "client-attached", "set-option destroy-unattached keep-last"]` | `capture_output=True` | `cli.py:135-145` | The hook body is one argv element. Deferring `destroy-unattached` to the `client-attached` hook is required because setting it on a *detached* session destroys it immediately; `keep-last` prevents destroying the last session of the group. |
| T7 | `["tmux", "show-options", "-gv", "status-left"]` | `capture_output=True, text=True` | `cli.py:149-153` | `-gv` yields the bare value. |
| T8 | `["tmux", "set-option", "-t", "<grouped_name>", "status-left", "<value>"]` | `capture_output=True` | `cli.py:157-160` | Only when T7 exited 0 and produced a non-empty value after `rstrip("\n")`. The value is `T7_stdout.rstrip("\n").replace("#S", session_name)` — only the trailing newline is stripped, deliberately preserving template spaces (`cli.py:155-156`). |
| T9 | `os.execvp("tmux", ["tmux", "attach-session", "-t", "<grouped_name>"])` | — | `cli.py:161` | Normal path when the session already existed. |
| T10 | `os.execvp("tmux", ["tmux", "new-session", "-s", "<session_name>", "<forestui_cmd>"])` | — | `cli.py:164` | The session did not exist. tmux creates it and runs `<forestui_cmd>` in window 0; the window is later renamed by T11. |

### 6.4 `tmux` — via libtmux, from the running TUI

libtmux builds argv as `[shutil.which("tmux"), *args]` and runs it with
`subprocess.Popen(..., stdout=PIPE, stderr=PIPE, text=True, errors="backslashreplace")`
(`libtmux/common.py:251-270`). **Any non-empty stderr is treated as an error** and raises
`LibTmuxException` (`libtmux/session.py`, `neo.py:207-208`), regardless of exit code.
`shutil.which("tmux")` returning `None` raises `TmuxCommandNotFound`.

| # | Effective argv | Called from | Notes |
|---|---|---|---|
| T11 | `tmux rename-window -t <window_id> <name>` | `TmuxService.rename_window` `tmux.py:139`, called by `cli.rename_tmux_window` `cli.py:26-30` after re-entering tmux, and by nothing else | `libtmux/window.py:462-489` wraps this in a bare `try/except Exception` that only **logs**, so a failure is invisible; `rename_window` then still returns `True`. Requires `$TMUX_PANE`. |
| T12 | `tmux set-option -g focus-events on` | `ensure_focus_events` `tmux.py:152`, from `on_mount` `app.py:113` | Server-global. On failure (`LibTmuxException`) returns `False` → notification `Could not enable focus events` (warning). |
| T13 | `tmux display-message -p #{session_group}` | `TmuxService.session` `tmux.py:67` | `Server.cmd` with `target=None` ⇒ no `-t`. Reads the *ambient* client's session group. |
| T14 | `tmux list-clients -F "#{client_activity} #{session_id} #{session_group}"` | `TmuxService.session` `tmux.py:74-78` | Parsed with `line.strip().split(" ", 2)`; rows with ≠3 fields are skipped; rows whose group ≠ ours are skipped when our group is non-empty; the row with the numerically largest `client_activity` wins. `int()` failures raise `ValueError`, caught at `tmux.py:105` → `session` returns `None`. |
| T15 | `tmux list-sessions -F "#{<field>}␞…"` | `Server.sessions` (`libtmux/neo.py:180-217`) | Enumerates every session on the server. Exceptions are swallowed → empty list (`libtmux/server.py`). |
| T16 | `tmux list-windows -t <session_id> -F "…"` | `Session.windows` | Used by `find_window`, `_find_unique_window_name`, `current_window`. |
| T17 | `tmux list-panes -t <window_id> -F "…"` | `Window.panes` | Used only by `current_window` to match `$TMUX_PANE`. |
| T18 | `tmux select-window -t <window_id>` | `Window.select` `tmux.py:200` | Only reachable from `create_editor_window` when a window named `edit:<name>` already exists. Non-empty stderr → `LibTmuxException` → `create_editor_window` returns `False`. |
| T19 | `tmux new-window -t <session_id>: -P -c<start_directory> -F#{window_id} -n <window_name> -t<session_id>: [<shell-command>]` | `Session.new_window` from `create_editor_window` (`tmux.py:205-209`), `create_shell_window` (`tmux.py:235-239`), `create_mc_window` (`tmux.py:265-270`), `create_claude_window` (`tmux.py:355-360`) | `attach=True` in all four call sites, so `-d` is **not** passed and the new window becomes current. `-t` appears twice (once from `Server.cmd`'s target binding, once from `new_window`'s own arg) — tmux honours the last. `<shell-command>` is a single argv element that tmux executes via its default shell; the window closes when the command exits. `create_shell_window` passes **no** shell command, so the window runs the default shell and persists. |

Per-call `<shell-command>` values:

| Call site | `-n <window_name>` | `-c` | shell-command |
|---|---|---|---|
| `create_editor_window` | `edit:<name>` | worktree path | `<editor> .` (e.g. `vim .`, `emacs -nw .`) |
| `create_shell_window` | `term:<name>` (uniquified) | path | *(none)* |
| `create_mc_window` | `files:<name>` (uniquified) | path | `mc` |
| `create_claude_window` | `<prefix>:<name>` (uniquified) | path | `<$SHELL> -ic <shlex.quote(cmd)>` |

### 6.5 Other subprocesses

| # | Call | file:line | Notes |
|---|---|---|---|
| X1 | `shutil.which("tmux")` | `cli.py:55` | Presence probe; exits 1 with the install hint when `None`. |
| X2 | `subprocess.run(["uv","tool","upgrade","forestui"], capture_output=True, text=True, timeout=120)` | `app.py:206-211` | See §2.2. Runs on a worker thread. |
| X3 | `subprocess.Popen([*editor.split(), path], stdout=DEVNULL, stderr=DEVNULL)` | `app.py:908-912` | GUI-editor fallback. `editor.split()` is whitespace splitting, **not** shell parsing — quoting in the editor setting is not honoured. Not awaited; the child is never reaped. `FileNotFoundError` → notification `Editor '<editor>' not found` (error). Other `OSError`s propagate. |

### 6.6 Filesystem operations that are not subprocesses

| Operation | file:line | Notes |
|---|---|---|
| `forest_dir.mkdir(parents=True, exist_ok=True)` | `state.py:32`, `state.py:50` | On every config-path resolution and every save. |
| `worktree_path.parent.mkdir(parents=True, exist_ok=True)` | `git.py:160` | `<forest>/<repo>/` is created before `git worktree add`. |
| `Path.rename(new_path)` | `app.py:624` | Worktree directory rename; fails with `OSError` across filesystems. |
| `config_path.open("w", encoding="utf-8")` + `json.dump` | `state.py:52-53` | Not atomic — no temp-file-and-rename. A crash mid-write truncates the state file. |
| `_config_path.parent.mkdir(parents=True, exist_ok=True)` + open/write | `services/settings.py:62-64` | Same non-atomic pattern. |
| `sessions_dir.glob("*.jsonl")` | `claude_session.py:45` | Non-recursive. |
| `shutil.move(old, new)` per session file | `claude_session.py:170` | Only when the destination does not already exist. |
| `old_dir.rmdir()` | `claude_session.py:174` | Only when the directory is empty afterwards. |
| `error_log.write_text(tb)` to `~/.forestui-error.log` | `app.py:1033` | |

---

## 7. State machine

### 7.1 State ownership

| Store | Lifetime | Persistence | Accessor |
|---|---|---|---|
| `AppState` | process singleton (`state.py:213-221`) | `<forest>/.forestui-config.json`, written eagerly on every mutation | `get_app_state()` |
| `SettingsService` | process singleton (`services/settings.py:36-43`, `__new__`-based) | `~/.config/forestui/settings.json`, written only on Save | `get_settings_service()` |
| `GitService` | process singleton, **stateless** | — | `get_git_service()` |
| `TmuxService` | process singleton; caches only `_server` | — | `get_tmux_service()` |
| `GitHubService` | process singleton; caches `_cache`, `_auth_status`, `_username` | — | `get_github_service()` |
| `ClaudeSessionService` | process singleton, **stateless** | reads `~/.claude/projects/` on demand | `get_claude_session_service()` |
| `_forest_path` | module global (`services/settings.py:14`) | set once at startup | `get_forest_path()` |

All singletons use `__new__` overrides that return the same instance, so the "global" is really
per-class. `SettingsService._settings` and `_config_path` are **class attributes**, meaning they
are shared even if a second instance were constructed.

### 7.2 Selection model

`Selection` has exactly three reachable shapes:

| Shape | Meaning | Detail pane |
|---|---|---|
| `(None, None)` | nothing selected | `EmptyState` |
| `(repo_id, None)` | repository selected | `RepositoryDetail` |
| `(repo_id, worktree_id)` | worktree selected | `WorktreeDetail` |

Transitions (`state.py:144-154`):

```python
def select_repository(self, repo_id: UUID) -> None:
    self._selection = Selection(repository_id=repo_id)

def select_worktree(self, repo_id: UUID, worktree_id: UUID) -> None:
    self._selection = Selection(repository_id=repo_id, worktree_id=worktree_id)
```

Both replace the whole `Selection`; selecting a repository always clears the worktree.

Selection is **not persisted**. On startup (`app.py:110-126`), if no repository is selected and
the repository list is non-empty, `repositories[0]` is auto-selected and the detail pane is
rendered. Since `Selection` starts empty every launch, this always fires when at least one
repository exists.

Sidebar → state edges:

| Event | Handler | Effect |
|---|---|---|
| `Tree.NodeHighlighted` (arrow keys, mouse hover-move of the cursor) | `sidebar.on_tree_node_highlighted` → `_select_node` `sidebar.py:199-217` | Posts `RepositorySelected`/`WorktreeSelected`. **Selection follows the cursor.** |
| `Tree.NodeSelected` (Enter, click) | `sidebar.on_tree_node_selected` `sidebar.py:179-197` | First applies "smart collapse" bookkeeping, then `_select_node`. |
| Archived-section node (`ArchivedNode`) | `_select_node` `sidebar.py:214-217` | Neither branch matches ⇒ **no message posted**; the previous selection and detail pane persist. |

Smart-collapse logic (`sidebar.py:186-195`):

```python
if isinstance(data, RepoNode):
    was_already_selected = self._last_selected_repo_id == data.id
    if not was_already_selected and not node.is_expanded:
        # Re-expand: user clicked to select, not to collapse
        node.expand()
    self._last_selected_repo_id = data.id
elif isinstance(data, WorktreeNode):
    # Clicking a worktree clears the "last selected repo" tracking
    self._last_selected_repo_id = None
```

Textual's `Tree` toggles expansion on `NodeSelected` for non-leaf nodes; this handler runs
afterwards and re-expands when the repo was not already the "last selected", so the first Enter
on a repo selects it (staying expanded) and a second Enter collapses it.
`_last_selected_repo_id` is reset to `None` by `_populate_tree`? — **no**, it survives rebuilds
(it is only assigned in this handler), so after a sidebar refresh the collapse state machine
retains its memory.

### 7.3 Selected-path resolution

```python
def _get_selected_path(self) -> str | None:
    """Get the path of the currently selected item."""
    selection = self._state.selection
    if selection.worktree_id:
        result = self._state.find_worktree(selection.worktree_id)
        if result:
            return result[1].path
    elif selection.repository_id:
        repo = self._state.find_repository(selection.repository_id)
        if repo:
            return repo.source_path
    return None
```
— `app.py:854-865`

Every path-consuming keyboard action (`e`, `t`, `o`, `n`, `y`) calls this and silently no-ops on
`None`. The path is the **raw stored string** — not expanded, not resolved, not existence-checked.

### 7.4 State mutations and persistence points

Every method below calls `_save_state()` before returning, so the JSON file is rewritten
synchronously on the event loop thread:

| Method | file:line | Save? | Notes |
|---|---|---|---|
| `add_repository` | `state.py:80-83` | yes | appends |
| `remove_repository` | `state.py:85-90` | yes | filters by id; clears the whole `Selection` when the removed repo was selected |
| `add_worktree` | `state.py:107-113` | yes | appends to the first matching repo; **silently no-ops** when the repo id is unknown |
| `remove_worktree` | `state.py:115-121` | yes | filters the worktree out of **every** repo; when it was selected, reduces `Selection` to `(repository_id, None)` |
| `update_worktree(**kwargs)` | `state.py:123-134` | yes | `model_dump()` → `dict.update(kwargs)` → `Worktree.model_validate(...)`. Re-validation means a bad kwarg raises `ValidationError`. Replaces the object in place, so the new instance has the same `id`. |
| `archive_worktree` / `unarchive_worktree` | `state.py:136-142` | yes (via `update_worktree`) | sets `is_archived` |
| `reorder_worktree` | `state.py:185-205` | yes | **unreachable** (§14) |
| `refresh_worktree_timestamp` | `state.py:207-209` | yes | **unreachable** (§14) |

`show_archived` and `selection` are in-memory only.

### 7.5 What triggers a detail-pane reload

`_refresh_detail_pane()` (`app.py:250-334`) is the single re-render entry point. It
`await detail_pane.remove_children()` first, then mounts exactly one of
`WorktreeDetail` / `RepositoryDetail` / `EmptyState`.

Callers:

| Trigger | file:line |
|---|---|
| startup auto-select | `app.py:118` |
| `AppFocus` event (tmux focus-events → terminal regains focus) | `app.py:128-131` |
| repository selected | `app.py:342` |
| worktree selected | `app.py:349` |
| sync (pull) completed, repo or worktree | `app.py:411`, `app.py:448` |
| repository removed via button | `app.py:437` |
| worktree created (from modal or from issue) | `app.py:537`, `app.py:709` |
| worktree archived / unarchived via button | `app.py:575`, `app.py:583` |
| worktree deleted via button | `app.py:606` |
| worktree renamed / branch renamed | `app.py:634`, `app.py:651` |
| repository added | `app.py:665` |
| `h` toggle archive | `app.py:792` |
| `d` delete (both branches) | `app.py:815`, `app.py:828` |
| `s` settings saved | `app.py:839` |
| `r` refresh | `app.py:844` |

`_refresh_sidebar()` (`app.py:240-248`) rebuilds the tree from in-memory state; called from the
same set minus the pure-selection changes (selecting a node does not rebuild the tree).

### 7.6 Detail-pane construction sequence

Worktree branch (`app.py:259-292`):

1. `find_worktree(id)`; if not found, fall through to the `elif`/`else` chain — with
   `worktree_id` set but unresolvable, **nothing is mounted** and the detail pane stays empty.
2. `path_exists = worktree.get_path().exists()` — filesystem stat, synchronous.
3. `commit_hash = ""`, `commit_time = None`, `has_remote = False`.
4. In a `try` over `GitError`: `get_latest_commit(worktree.path)` (G15) then
   `has_remote_tracking(worktree.path)` (G18). **Both are awaited serially**; a `GitError` from
   the first skips the second.
5. Mount `WorktreeDetail(repo, worktree, commit_hash, commit_time, has_remote, custom_buttons, path_exists)`.
6. Dispatch `_fetch_sessions_for_path(worktree.path, "worktree")` (background).

Repository branch (`app.py:293-331`):

1. `find_repository(id)`; if `None`, nothing is mounted.
2. `branch = await get_current_branch(source_path)` (G2) in its own `try`, `""` on `GitError`.
3. Commit info + remote tracking, same pattern as above.
4. Mount `RepositoryDetail(...)`.
5. `detail.start_issues_spinner()`.
6. Dispatch `_fetch_sessions_for_path(source_path, "repository")`.
7. Dispatch `_fetch_issues_for_repo(source_path)`.

Both branches run **three to four blocking-ish git subprocesses on the UI coroutine** before the
widget is mounted, so switching selection with arrow keys is throttled by git latency.

### 7.7 Timers and polling cadence

| Timer | Period | Started at | Callback | Notes |
|---|---|---|---|---|
| GitHub issue refresh | **300 s** | `app.py:126` (`self.set_interval(300, self._refresh_github_issues)`) | `_refresh_github_issues` (`app.py:140-147`) | Invalidates the **entire** issue cache, then re-fetches issues for the currently selected repository (if any). Runs for the process lifetime, regardless of what is selected. |
| Issues refresh-button spinner | **0.05 s** | `repository_detail.py:393` | `_tick_spinner` | Cycles `\|`, `/`, `-`, `\`. Started by `start_issues_spinner()` on mount and by pressing `↻`; stopped by `update_issues()`. Guarded against double-start (`repository_detail.py:386-387`). |
| Fetch-button spinner (issue modal) | **0.1 s** | `modals.py:689` | `_tick_spinner` | Same glyph cycle. Stopped by `update_branches()` or `fetch_failed()`. |

There is **no** periodic git polling, no filesystem watching, and no automatic re-scan of
worktrees. Everything else is event-driven.

### 7.8 Caching

| Cache | Key | TTL | Invalidated by |
|---|---|---|---|
| `GitHubService._cache: dict[str, IssueCache]` (`github.py:27`, `github.py:114-127`) | `"<owner>/<repo>"` from H3 | `CACHE_TTL_SECONDS = 300` (`github.py:25`), checked as `(now - fetched_at).total_seconds() < 300` | `invalidate_cache()` (whole dict) from the 5-min timer (`app.py:143`) and from the `↻` button (`app.py:462`) |
| `GitHubService._auth_status` / `_username` (`github.py:66-67`) | — | **never expires** | nothing |
| `TmuxService._server` (`tmux.py:31`, `48-52`) | — | never | nothing |
| `SettingsService._settings` (`services/settings.py:37`) | — | never | replaced wholesale by `save_settings` |

Note the H3 (`gh repo view`) call is **not** cached — it runs on every `list_issues` call even
when the issue list itself is a cache hit (`github.py:110-121`).

Claude session data is **not** cached; every detail-pane render re-globs and re-parses the JSONL
files.

---

## 8. Async / reactive behavior

### 8.1 Backgrounded operations

`@work`-decorated methods run as Textual workers (async workers on the event loop unless
`thread=True`).

| Method | Decorator | file:line | Purpose |
|---|---|---|---|
| `on_app_focus` | `@work` | `app.py:128-131` | re-render detail pane when the terminal regains focus |
| `_check_gh_status` | `@work` | `app.py:133-138` | H1/H2, then `sidebar.set_gh_status(...)` |
| `_refresh_github_issues` | `@work` | `app.py:140-147` | 5-minute cache flush + refetch |
| `_fetch_issues_for_repo` | `@work` | `app.py:149-162` | H3/H4/H5 then `RepositoryDetail.update_issues` |
| `_fetch_sessions_for_path` | `@work` | `app.py:164-175` | **synchronous** filesystem scan executed inside an async worker (`get_sessions_for_path` is not `async`), then `update_sessions` |
| `_auto_update` | `@work(thread=True)` | `app.py:182-238` | `uv tool upgrade` on a real thread |
| `on_worktree_detail_sync_requested` | `@work` | `app.py:402-413` | G17 pull |
| `on_repository_detail_remove_repository_requested` | `@work` | `app.py:421-437` | needs `push_screen_wait` |
| `on_repository_detail_sync_requested` | `@work` | `app.py:439-450` | G17 pull |
| `on_create_worktree_from_issue_modal_worktree_created` | `@work` | `app.py:495-540` | pull + get_ref + create_worktree |
| `on_create_worktree_from_issue_modal_fetch_requested` | `@work` | `app.py:542-567` | G16 fetch + G4 + G3 |
| `on_worktree_detail_delete_worktree_requested` | `@work` | `app.py:585-606` | needs `push_screen_wait` |
| `action_delete` | `@work` | `app.py:794-828` | needs `push_screen_wait` |
| `action_open_settings` | `@work` | `app.py:830-839` | needs `push_screen_wait` |
| `SettingsModal._manage_buttons` | `@work` | `modals.py:449-459` | nested `push_screen_wait` |
| `CustomButtonsModal._add_button` | `@work` | `modals.py:982-993` | nested `push_screen_wait` |
| `CustomButtonsModal._edit_button` | `@work` | `modals.py:995-1008` | nested `push_screen_wait` |

Everything **not** in this list — including `_refresh_detail_pane`'s git calls, all state saves,
and `worktree.get_path().exists()` — runs inline on the UI coroutine.

Textual's default worker policy for `@work` without `exclusive=True` allows **concurrent**
instances of the same worker. `_fetch_sessions_for_path` and `_fetch_issues_for_repo` are
therefore racy under rapid selection changes (§8.3).

### 8.2 Loading placeholders

| Region | Placeholder | Replaced by |
|---|---|---|
| `#sessions-container` (both detail views) | Label `Loading...` `.label-muted` (`repository_detail.py:203`, `worktree_detail.py:227`) | `update_sessions()` → session cards or `No sessions found` |
| `#issues-container` | Label `Loading...` `.label-muted` (`repository_detail.py:211`) | `update_issues()` → issue rows or `No issues found` |
| `#btn-refresh-issues` | label `↻` → spinner glyph, `disabled=True` | `update_issues()` restores `↻`, `disabled=False` |
| `#gh-status` | `gh cli: ...` | `set_gh_status()` |
| `#btn-fetch` (issue modal) | label `Fetch` → spinner glyph, `disabled=True` | `update_branches()` / `fetch_failed()` |
| App title | `forestui vX (checking for updates...)` | `_auto_update` outcome |

### 8.3 Ordering guarantees (and the lack of them)

- `_refresh_detail_pane` awaits `remove_children()` before mounting, so the old widget is gone
  before the new one appears — there is never a moment with two detail widgets.
- Background updaters locate their target with `self.query_one(RepositoryDetail)` /
  `query_one(WorktreeDetail)` wrapped in `try/except Exception: pass` (`app.py:158-162`,
  `app.py:169-175`). If the pane changed type in the meantime, the update is dropped silently.
- **No generation counter exists.** If selection changes from repo A to repo B while A's session
  fetch is in flight, and both are of the same detail type, A's results will be written into B's
  widget. This is observable when arrowing quickly through worktrees: a worktree can briefly show
  another worktree's sessions until its own fetch lands.
- `update_issues` unconditionally calls `_stop_refresh_spinner()` first
  (`repository_detail.py:420`), so a late-arriving fetch stops a spinner that a newer fetch
  started.
- `_fetch_issues_for_repo` sets `issues = []` before the `try` and still calls
  `detail.update_issues(issues)` after an exception (`app.py:152-162`), so a failed fetch renders
  `No issues found` **and** notifies `Issue fetch error: <e>`.
- Rename operations mutate the filesystem (`Path.rename`) **before** running `git worktree repair`
  and before updating state; a failure between steps leaves the three out of sync (§12.4).
- Detail-pane git probes are awaited serially before mount, so each selection change costs
  ~2-3 sequential `git` process spawns.

### 8.4 Message routing

Textual messages bubble from the emitting widget to the App. Handler names are derived from the
message class: `Namespace.MessageName` → `on_namespace_message_name`.

| Message | Emitter | Handler |
|---|---|---|
| `Sidebar.RepositorySelected` | Sidebar | `on_sidebar_repository_selected` `app.py:337` |
| `Sidebar.WorktreeSelected` | Sidebar | `on_sidebar_worktree_selected` `app.py:344` |
| `Sidebar.AddRepositoryRequested` | Sidebar | `on_sidebar_add_repository_requested` `app.py:351` |
| `Sidebar.AddWorktreeRequested` | *(never emitted)* | `on_sidebar_add_worktree_requested` `app.py:357` |
| `OpenInEditor` / `OpenInTerminal` / `OpenInFileManager` | both detail views | `on_open_in_editor` / `on_open_in_terminal` / `on_open_in_file_manager` `app.py:364-374` |
| `StartClaudeSession` / `StartClaudeYoloSession` | both detail views | `app.py:376-382` |
| `ContinueClaudeSession` / `ContinueClaudeYoloSession` | both detail views | `app.py:384-390` |
| `StartClaudeCustomSession` / `ContinueClaudeCustomSession` | both detail views | `app.py:392-400` |
| `WorktreeDetail.SyncRequested` | WorktreeDetail | `app.py:402` |
| `WorktreeDetail.{Archive,Unarchive,Delete,RenameWorktree,RenameBranch}Requested` | WorktreeDetail | `app.py:569-653` |
| `RepositoryDetail.{AddWorktree,RemoveRepository,Sync,CreateWorktreeFromIssue,RefreshIssues}Requested` | RepositoryDetail | `app.py:415-463` |
| `AddRepositoryModal.RepositoryAdded` | modal | `app.py:656` |
| `AddWorktreeModal.WorktreeCreated` | modal | `app.py:670` |
| `CreateWorktreeFromIssueModal.WorktreeCreated` / `.FetchRequested` | modal | `app.py:496`, `app.py:543` |

The shared messages in `components/messages.py` are deliberately module-level (not nested in a
widget class) so that one App handler serves both detail views — this was the consolidation in
commit `f07c716`.

---

## 9. tmux integration

### 9.1 Session naming

```python
if forest_path:
    forest_folder = Path(forest_path).expanduser().resolve().name
else:
    forest_folder = "forest"  # default ~/forest

session_name = f"forestui-{slugify(forest_folder)}"
```
— `cli.py:66-71`

```python
def slugify(text: str) -> str:
    """Convert text to a safe slug for tmux session names."""
    import re

    # Convert to lowercase and replace spaces/special chars with dashes
    slug = re.sub(r"[^a-zA-Z0-9]+", "-", text.lower())
    # Remove leading/trailing dashes
    return slug.strip("-")
```
— `cli.py:33-40`

Examples: `~/forest` → `forestui-forest`; `~/work` → `forestui-work`;
`~/My Projects` → `forestui-my-projects`; `/` → `forestui-` (the resolved name of `/` is `""`,
so the session name is the literal `forestui-`).

Grouped session name: `f"{session_name}-{os.getpid()}"` (`cli.py:123`), where the PID is that of
the *outer* forestui process performing the attach.

### 9.2 Cold start (session does not exist)

`os.execvp("tmux", ["tmux", "new-session", "-s", session_name, forestui_cmd])` (`cli.py:164`).
tmux creates the session, runs `<forestui_cmd>` in window 0, and attaches. The inner forestui
sees `$TMUX`, skips `ensure_tmux`, and renames its own window (§9.4).

`<forestui_cmd>` reconstruction (`cli.py:74-82`):

```python
forestui_cmd = "forestui"
if debug_mode:
    forestui_cmd += " --debug"
if no_self_update:
    forestui_cmd += " --no-self-update"
if dev_mode:
    forestui_cmd += " --dev"
if forest_path:
    forestui_cmd += f" {shlex.quote(forest_path)}"
```

Note: the literal string `"forestui"` is used — the re-exec resolves `forestui` from `PATH`
inside tmux's environment, not `sys.argv[0]`. `--dev` is appended whenever `dev_mode` is true,
including when it was force-enabled by the `0.0.0` version check, so a source checkout
propagates dev mode into tmux.

### 9.3 Reattach (session already exists)

Sequence (`cli.py:93-161`):

1. T2 lists window names. If **no** window is named `forestui` or `forestui-dev-*`, run T3 to
   create one running `<forestui_cmd>` — this is the "forestui was killed but the session
   survived" recovery from commit `3e56bc2`. If such a window already exists, it is reused as-is;
   no second instance is started.
2. T4 creates a grouped session `<session>-<pid>` linked with `-t =<session>`. Grouped sessions
   share the window list but have independent current-window pointers, so two terminals attached
   to the same forest can navigate windows independently (commit `e6c908d`).
3. If T4 fails, T5 attaches directly to the base session (shared navigation) and execs away.
4. T6 installs a `client-attached` hook on the grouped session that runs
   `set-option destroy-unattached keep-last`, so the throwaway grouped session is destroyed when
   its client detaches — but never the last one in the group.
5. T7/T8 rewrite the grouped session's `status-left` by substituting the base session name for
   `#S`, so the status bar shows `forestui-forest` rather than `forestui-forest-48213`.
6. T9 execs `tmux attach-session -t <grouped_name>`.

### 9.4 Own-window renaming

`cli.py:208` calls `rename_tmux_window(get_window_name(dev_mode))` **after** `ensure_tmux`
returns, i.e. only in the in-tmux process. It resolves the window via `$TMUX_PANE`:

```python
    @property
    def current_window(self) -> Window | None:
        """Get the tmux window this process is running in.

        Uses the TMUX_PANE environment variable to find our own window,
        rather than the session's active window — which may be a different
        window when forestui is starting up in a background window.
        """
        if self.server is None:
            return None
        pane_id = os.environ.get("TMUX_PANE")
        if not pane_id:
            return None
        try:
            for sess in self.server.sessions:
                for window in sess.windows:
                    for pane in window.panes:
                        if pane.pane_id == pane_id:
                            return window
        except LibTmuxException:
            return None
        return None
```
— `tmux.py:109-130`

This scans **every session × window × pane** on the server (T15/T16/T17), so it is O(n) tmux
subprocess calls in the number of windows. Using `$TMUX_PANE` rather than the session's active
window is what makes the T3 recovery path work: forestui may start in a background window.

Resulting window name: `forestui`, or `forestui-dev-HHMM` in dev mode.

### 9.5 Window naming scheme for spawned windows

| Kind | Format | Uniquified? | Reuses existing? |
|---|---|---|---|
| Editor | `edit:<name>` | **no** | **yes** — `find_window` then `select()` (`tmux.py:198-201`) |
| Terminal | `term:<name>` | yes | no — always a new window |
| File manager | `files:<name>` | yes | no |
| Claude (default) | `claude:<name>` | yes | no |
| Claude (YOLO) | `yolo:<name>` | yes | no |
| Claude (custom button) | `<button.prefix>:<name>` | yes | no |

`<name>` comes from `_get_tmux_window_name` (`app.py:879-889`):

```python
def _get_tmux_window_name(self, path: str) -> str:
    """Get the window name for Claude sessions: repo:worktree format."""
    # Check worktrees first
    for repo in self._state.repositories:
        for worktree in repo.worktrees:
            if worktree.path == path:
                return f"{repo.name}:{worktree.name}"
        # Check if it's the repository source path
        if repo.source_path == path:
            return repo.name
    return "session"
```

So a worktree yields `<repo>:<worktree>` and a repository yields `<repo>`; an unrecognised path
yields the literal `"session"`. Full window names therefore look like `claude:forestui:my-feature`
or `term:forestui`. Path matching is **exact string equality** against the stored path — a
path that differs by symlink resolution or a trailing slash falls through to `"session"`.

Note the repository check is inside the outer loop but after the inner loop, so it is evaluated
per-repository; correct, but a worktree of repo B is found before repo A's source path only if A
has no matching worktree — order is repo-major.

Uniquification (`_find_unique_window_name`, `tmux.py:277-301`):

```python
if base_name not in existing_names:
    return base_name

# Find next available suffix
counter = 2
while f"{base_name}:{counter}" in existing_names:
    counter += 1
return f"{base_name}:{counter}"
```

So repeated `t` presses on the same worktree give `term:forestui:my-feature`,
`term:forestui:my-feature:2`, `term:forestui:my-feature:3`, …

### 9.6 Editor launch decision

```python
def _open_in_editor(self, path: str) -> None:
    """Open path in configured editor."""
    editor = self._settings_service.settings.default_editor

    # If inside tmux and editor is TUI-based, use tmux window
    if self._tmux_service.is_inside_tmux and self._tmux_service.is_tui_editor(
        editor
    ):
        name = self._get_tmux_window_name(path)
        if self._tmux_service.create_editor_window(name, path, editor):
            self.notify(f"Opened {editor} in edit:{name}")
            return

    # GUI editor or not in tmux - spawn normally
    try:
        # Handle editors with arguments (e.g., "emacs -nw")
        editor_parts = editor.split()
        subprocess.Popen(
            [*editor_parts, path],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.notify(f"Opened in {editor_parts[0]}")
    except FileNotFoundError:
        self.notify(f"Editor '{editor}' not found", severity="error")
```
— `app.py:891-915`

TUI editor set (`tmux.py:13-24`), matched against `editor.split()[0]`:

```python
TUI_EDITORS = {
    "vim",
    "nvim",
    "vi",
    "emacs",
    "nano",
    "helix",
    "hx",
    "micro",
    "kakoune",
    "kak",
}
```

Note `create_editor_window` returning `False` (e.g. tmux session unresolvable) **falls through**
to the GUI path, so a failed tmux window spawn results in `vim <path>` being `Popen`ed with its
stdio sent to `/dev/null` — an invisible, wedged process.

### 9.7 Claude window construction

```python
    def create_claude_window(
        self,
        name: str,
        path: str,
        resume_session_id: str | None = None,
        yolo: bool = False,
        custom_command: str | None = None,
        custom_prefix: str | None = None,
    ) -> str | None:
        ...
        if custom_prefix:
            base_window_name = f"{custom_prefix}:{name}"
        elif yolo:
            base_window_name = f"yolo:{name}"
        else:
            base_window_name = f"claude:{name}"

        try:
            # Always create a new window with unique name (add :2, :3 suffix if needed)
            window_name = self._find_unique_window_name(base_window_name)

            # Build claude command (closes when claude exits)
            # Use custom_command if provided, otherwise default to "claude"
            cmd = custom_command or "claude"
            # Only append YOLO flag for the built-in YOLO button, not custom buttons
            if yolo and not custom_prefix:
                cmd += " --dangerously-skip-permissions"
            if resume_session_id:
                cmd += f" -r {resume_session_id}"

            # Wrap in interactive shell to support aliases
            # Use shlex.quote to prevent shell injection from custom commands
            shell = os.environ.get("SHELL", "/bin/bash")
            shell_cmd = f"{shell} -ic {shlex.quote(cmd)}"

            self.session.new_window(
                window_name=window_name,
                start_directory=path,
                attach=True,
                window_shell=shell_cmd,
            )

            return window_name

        except LibTmuxException:
            return None
```
— `tmux.py:303-365`

Resulting command matrix:

| Invocation | `cmd` before quoting | Window |
|---|---|---|
| `n` / New Session | `claude` | `claude:<name>` |
| `y` / New Session: YOLO | `claude --dangerously-skip-permissions` | `yolo:<name>` |
| Resume | `claude -r <session-id>` | `claude:<name>` |
| YOLO resume | `claude --dangerously-skip-permissions -r <session-id>` | `yolo:<name>` |
| Custom button (start) | `<button.command>` verbatim | `<prefix>:<name>` |
| Custom button (resume) | `<button.command> -r <session-id>` | `<prefix>:<name>` |

The final shell-command string handed to tmux is `"<$SHELL> -ic <shlex.quote(cmd)>"`, and tmux
itself runs that string through its default shell — so there are **two** levels of shell. The
`-i` makes the shell interactive so user aliases and functions for `claude` resolve.
`shlex.quote` protects the inner level only; the `$SHELL` value and the window/dir names are not
quoted.

`resume_session_id` is interpolated **unquoted** into `cmd` before `shlex.quote` is applied to
the whole string, so a hostile session id cannot break out — the whole thing is single-quoted.

Always creating a new window (never reusing) is deliberate — commit `8edb06c`.

### 9.8 Active-session resolution (which tmux session gets the new window)

```python
    @property
    def session(self) -> Session | None:
        """Get the session of the most recently active tmux client.

        Not cached because the active client can change between grouped
        sessions — the user may be viewing forestui from any terminal.
        """
```
— `tmux.py:55-61`

Algorithm (`tmux.py:62-107`):

1. `our_group = tmux display-message -p '#{session_group}'` (T13), `.strip()`ed; may be `""`.
2. `tmux list-clients -F '#{client_activity} #{session_id} #{session_group}'` (T14).
3. Among rows with exactly 3 whitespace-split fields, skip any whose group differs from
   `our_group` **when `our_group` is non-empty**; keep the one with the largest
   `int(client_activity)`.
4. If a winner exists, find the matching `Session` object in `server.sessions` (T15) and return it.
5. Fallback: first session with `session_attached` numerically > 0.
6. Fallback: `server.sessions[0]`.
7. `LibTmuxException` / `ValueError` / `TypeError` anywhere → `None`.

This is what makes new windows appear in *the terminal the user just acted in* under grouped
sessions.

### 9.9 Focus events

`ensure_focus_events()` runs `tmux set-option -g focus-events on` at mount (`app.py:113`). With
focus events on, the terminal emitting focus-in reaches Textual as `events.AppFocus`, handled by
`on_app_focus` (`app.py:128-131`), which re-renders the detail pane. Practical effect: switching
back to the forestui tmux window refreshes commit info, remote-tracking status, sessions and
issues.

### 9.10 Claude session tracking

`ClaudeSessionService` (`services/claude_session.py`).

Path mapping (`claude_session.py:22-31`):

```python
    @staticmethod
    def _path_to_claude_folder(path: str | Path) -> str:
        """Convert a path to Claude's folder naming convention."""
        path_str = str(Path(path).expanduser().resolve())
        return path_str.replace("/", "-")

    @staticmethod
    def _get_claude_projects_dir() -> Path:
        """Get the Claude projects directory."""
        return Path.home() / ".claude" / "projects"
```

So `/Users/x/forest/repo/wt` → `~/.claude/projects/-Users-x-forest-repo-wt/`. Note the **leading
dash** (from the leading `/`) and that `.resolve()` **is** applied here, unlike everywhere else.

Session enumeration (`get_sessions_for_path`, `claude_session.py:33-56`):
- Return `[]` if the directory does not exist.
- `glob("*.jsonl")`, skipping any filename starting with `agent-`.
- Parse each; drop `None`s.
- Sort by `last_timestamp` descending; return the first `limit` (default **5**).

JSONL parsing (`_parse_session_file`, `claude_session.py:58-151`) — per line:
- Blank lines skipped; `json.JSONDecodeError` skips the line.
- `timestamp` key: `datetime.fromisoformat(v.replace("Z","+00:00"))`; naive results are stamped
  UTC; the maximum wins. `ValueError`/`AttributeError` skip.
- A record counts as a user message when `data["type"] == "user"` **or** `data["role"] == "user"`;
  `message_count` increments.
- Content extraction: `data.get("message", {}).get("content", "") or data.get("content", "")`.
  If the result is a list, the **first** block whose `type == "text"` supplies `block["text"]`;
  if no such block exists, content becomes `""` (`for…else`).
- A content string is used only when it is non-empty and does **not** start with `<`
  (filters out `<command-*>`, `<system-reminder>` wrappers). It is normalized with
  `re.sub(r"\n{3,}", "\n\n", content)`; the first such message (truncated to 100 chars) becomes
  `title`, and every such message overwrites `last_message` (truncated to 100).
- `gitBranches`, when a list, contributes new non-empty branch names in first-seen order.
- `OSError` while reading → the whole file yields `None`.
- After the loop: `message_count == 0` → `None`. If no timestamp was ever seen,
  `last_timestamp` falls back to the file's `st_mtime` as UTC.
- `title` falls back to the literal `"Untitled session"`.

Session migration on worktree rename (`migrate_sessions`, `claude_session.py:153-174`): computes
both folder names, returns immediately if the old directory does not exist, creates the new
directory, `shutil.move`s each `*.jsonl` whose destination does not already exist, then `rmdir`s
the old directory if it ended up empty. Non-`.jsonl` files are left behind, which keeps the old
directory alive.

---

## 10. GitHub integration

### 10.1 Scope

Only **issues** are integrated. There is **no** PR listing, no CI/check status, no review state
— despite the sidebar's `gh cli:` badge suggesting broader integration. The five `gh` calls in
§6.2 are the complete surface.

### 10.2 Auth badge

`_check_gh_status` runs once at mount (`app.py:123`) and never again. It calls `get_auth_status()`
(H1, then H2 on success) and passes the result to `Sidebar.set_gh_status` (§4.1). Because
`_auth_status` is memoized for the process lifetime, authenticating `gh` while forestui is running
does not update the badge and does not enable issue fetching until restart.

### 10.3 Issue fetching

`list_issues(path)` (`github.py:96-128`) sequence:

1. `get_auth_status()` — memoized; anything other than `"authenticated"` returns `[]` immediately,
   so no `gh` process is spawned on subsequent calls.
2. `get_repo_info(path)` (H3). `None` → `[]`. This runs on **every** call, cache hit or not.
3. Cache key `"<owner>/<name>"`; a hit younger than 300 s returns the cached list.
4. `_fetch_issues(...)` (H4 + H5), dedup by issue number with H4's results taking precedence,
   sort by `created_at` descending, truncate to `limit` (10).
5. Store `IssueCache(issues, datetime.now(UTC))`.

The UI renders only the first 5 (`repository_detail.py:429`).

Note the fetch is issue-number-deduplicated but the sort is by `created_at`, while
`GitHubIssue.relative_time` displays `updated_at` — the displayed timestamp is not the sort key.

### 10.4 Create-worktree-from-issue flow

1. `Create WT` button (id `btn-issue-<number>`) posts
   `RepositoryDetail.CreateWorktreeFromIssue(repo_id, issue)` (`repository_detail.py:279-285`).
2. `_show_create_worktree_from_issue_modal` (`app.py:465-493`) runs G4, G2 and G3 in one `try`;
   on `GitError` it falls back to `branches=[]`, `current_branch="main"`, `remotes=[]`, then
   pushes `CreateWorktreeFromIssueModal`.
3. On `WorktreeCreated` (`app.py:495-540`), in a `@work` worker:
   - `worktree_path = get_forest_path() / repo.name / event.name`
   - if `pull_first`: notify `Pulling repo...`, `await pull(repo.source_path)` (G17)
   - if `base_branch` truthy: `base_ref = await get_ref(repo.source_path, base_branch)` (G14)
   - `await create_worktree(source_path, worktree_path, branch, new_branch=True, base_branch)`
     (G5 + possibly G6)
   - build `Worktree(name, branch, path, base_branch, created_from_ref=base_ref)`,
     `add_worktree`, `select_worktree`, refresh sidebar + detail, notify
     `Created worktree '<name>'`
   - any `GitError` → notify `Failed to create worktree: <e>` and abort (partial effects such as
     a completed pull remain).

### 10.5 Behavior when `gh` is absent or unauthenticated

| Situation | Effect |
|---|---|
| `gh` not on `PATH` | H1 returns `(-1, "", "gh not found")` → status `not_installed` → badge `gh cli: missing` (muted). `list_issues` returns `[]` without spawning anything. Issues panel shows `No issues found`. Refresh button still spins and stops. |
| `gh` present, not logged in | badge `gh cli: unauth'd` (amber); identical `[]` behavior. |
| `gh` present, repo has no GitHub remote | badge shows `ok`; H3 returns non-zero → `None` → `[]` → `No issues found`. |
| `gh issue list` returns malformed JSON | `json.loads` raises inside `_fetch_issues`; the exception propagates to `_fetch_issues_for_repo`'s `except Exception` (`app.py:155`) → notification `Issue fetch error: <e>` and the panel renders `No issues found`. |
| `gh` hangs | No timeout is set on `gh` calls. The worker blocks indefinitely; the UI stays responsive but the spinner never stops and the panel stays on `Loading...`. |

---

## 11. Theme / visual

### 11.1 Palette

Defined as Textual CSS variables at the top of `APP_CSS` (`theme.py:5-17`):

| Variable | Hex | Role |
|---|---|---|
| `$accent` | `#52B788` | branch names, focused borders, hover accents, `forestui` wordmark |
| `$accent-dark` | `#2D6A4F` | tree cursor row, primary button bg, footer key bg, branch tag bg, highlighted dropdown option |
| `$bg` | `#1C1C1E` | screen, sidebar, detail pane, tree, lists |
| `$bg-elevated` | `#2C2C2E` | cards, buttons, inputs, modals, footer, path display, header box |
| `$bg-hover` | `#3A3A3C` | hover states, tree highlight |
| `$bg-selected` | `#48484A` | *(declared, never used)* |
| `$border` | `#3D3D3F` | all default borders, rules |
| `$text-primary` | `#F5F5F5` | titles, primary labels, button text |
| `$text-secondary` | `#A8A8A8` | subtitles, section headers, path text, footer keys |
| `$text-muted` | `#7A7A7A` | metadata, hints, disabled-looking text, gh badge default |
| `$destructive` | `#FF6B6B` | destructive button text, error labels, missing-path warnings |
| `$success` | `#52B788` | `gh cli: ok` |
| `$warning` | `#FFB347` | `gh cli: unauth'd` |

The palette is unconditional: `Settings.theme` is persisted but never consulted, so light mode
does not exist (§14).

Additional literal colours appear only for the destructive button variant
(`theme.py:107-116`): background `#3d2020`, border `#5a3030`; on hover background `#4d2828`,
border `$destructive`.

### 11.2 Full CSS, verbatim

`forestui/theme.py:4-696` — the entire `APP_CSS` string:

```css
$accent: #52B788;
$accent-dark: #2D6A4F;
$bg: #1C1C1E;
$bg-elevated: #2C2C2E;
$bg-hover: #3A3A3C;
$bg-selected: #48484A;
$border: #3D3D3F;
$text-primary: #F5F5F5;
$text-secondary: #A8A8A8;
$text-muted: #7A7A7A;
$destructive: #FF6B6B;
$success: #52B788;
$warning: #FFB347;

Screen {
    background: $bg;
}

/* Main Layout */
#main-container {
    layout: horizontal;
    height: 100%;
}

#sidebar {
    width: 35;
    min-width: 30;
    max-width: 45;
    background: $bg;
    border-right: solid $border;
}

#detail-pane {
    width: 1fr;
    height: 100%;
    background: $bg;
    padding: 1 2;
}

/* Sidebar Header */
.sidebar-header {
    height: 3;
    padding: 1;
    background: $bg-elevated;
    border-bottom: solid $border;
}

.sidebar-header Label {
    text-style: bold;
    color: $text-primary;
}

.sidebar-header .header-buttons {
    dock: right;
}

/* Tree Items */
Tree {
    background: $bg;
    padding: 0 1;
}

Tree > .tree--cursor {
    background: $accent-dark;
    color: $text-primary;
}

Tree > .tree--highlight {
    background: $bg-hover;
}

Tree > .tree--highlight-line {
    background: $bg-hover;
}

/* Buttons */
Button {
    background: $bg-elevated;
    color: $text-primary;
    border: solid $border;
    min-width: 10;
    height: 3;
}

Button:hover {
    background: $bg-hover;
    border: solid $accent;
}

Button:focus {
    border: solid $accent;
}

Button.-primary {
    background: $accent-dark;
    border: solid $accent;
}

Button.-primary:hover {
    background: $accent;
}

Button.-destructive {
    background: #3d2020;
    color: $destructive;
    border: solid #5a3030;
}

Button.-destructive:hover {
    background: #4d2828;
    border: solid $destructive;
}

/* Action Cards */
.action-card {
    background: $bg-elevated;
    border: solid $border;
    padding: 1;
    margin: 0 0 1 0;
    height: auto;
}

.action-card:hover {
    background: $bg-hover;
    border: solid $accent;
}

.action-card:focus {
    border: solid $accent;
}

/* Section Headers */
.section-header {
    text-style: bold;
    color: $text-secondary;
    margin: 1 0 0 0;
    padding: 0 0 0 0;
}

/* Labels */
.label-primary {
    color: $text-primary;
}

.label-secondary {
    color: $text-secondary;
}

.label-muted {
    color: $text-muted;
}

.label-accent {
    color: $accent;
}

.label-destructive {
    color: $destructive;
}

/* Path Display */
.path-display {
    background: $bg-elevated;
    padding: 0 1;
    border: solid $border;
    color: $text-secondary;
}

/* Detail View */
.detail-content {
    height: auto;
    width: 100%;
}

RepositoryDetail {
    height: auto;
    width: 100%;
}

WorktreeDetail {
    height: auto;
    width: 100%;
}

EmptyState {
    height: auto;
    width: 100%;
}

.detail-header {
    height: auto;
    margin-bottom: 1;
}

.detail-title {
    text-style: bold;
    color: $text-primary;
}

.detail-subtitle {
    color: $text-secondary;
}

/* Input Fields */
Input {
    background: $bg-elevated;
    color: $text-primary;
    border: solid $border;
}

Input:focus {
    border: solid $accent;
}

Input.-invalid {
    border: solid $destructive;
}

/* Select */
Select {
    background: $bg-elevated;
    color: $text-primary;
    border: solid $border;
}

Select:focus {
    border: solid $accent;
}

SelectCurrent {
    background: $bg-elevated;
}

SelectOverlay {
    background: $bg-elevated;
    border: solid $border;
}

/* Modals */
ModalScreen {
    align: center middle;
}

.modal-container {
    width: 80;
    max-width: 90%;
    height: auto;
    max-height: 90%;
    background: $bg-elevated;
    border: solid $border;
    padding: 1 2;
}

.modal-container.modal-wide {
    width: 140;
    max-width: 95%;
    height: 90%;
}

.modal-scroll {
    width: 100%;
    height: auto;
    max-height: 20;
}

.modal-scroll.modal-scroll-tall {
    height: 1fr;
    max-height: 100vh;
}

.modal-title {
    text-style: bold;
    color: $text-primary;
    text-align: center;
    margin-bottom: 1;
}

.modal-buttons {
    margin-top: 1;
    align: center middle;
    height: 3;
}

.modal-buttons Button {
    margin: 0 1;
}

/* Session List */
.session-item {
    background: $bg-elevated;
    border: solid $border;
    padding: 1;
    margin: 0 2 1 0;
    height: auto;
    align: left middle;
}

.session-item:hover {
    background: $bg-hover;
}

.session-header-row {
    width: 100%;
    height: auto;
    align: left middle;
}

.session-info {
    width: 1fr;
    height: auto;
}

.session-title {
    color: $text-primary;
}

.session-last {
    color: $text-secondary;
}

.session-meta {
    color: $text-muted;
    margin-top: 1;
}

.session-buttons {
    height: auto;
    align: right middle;
}

.session-btn {
    min-width: 8;
    height: 3;
    margin-left: 1;
}

/* Status Messages */
.status-bar {
    dock: bottom;
    height: 1;
    background: $bg-elevated;
    color: $text-secondary;
    padding: 0 1;
}

/* Empty State */
.empty-state {
    align: center middle;
    height: 100%;
}

.empty-state Label {
    color: $text-muted;
    text-align: center;
}

/* Collapsible */
Collapsible {
    background: $bg;
    border: none;
    padding: 0;
}

CollapsibleTitle {
    background: $bg;
    color: $text-secondary;
    padding: 0 1;
}

CollapsibleTitle:hover {
    background: $bg-hover;
}

CollapsibleTitle:focus {
    background: $bg-hover;
}

/* OptionList for sidebar */
OptionList {
    background: $bg;
    border: none;
    padding: 0;
}

OptionList > .option-list--option {
    padding: 0 1;
}

OptionList > .option-list--option-highlighted {
    background: $bg-hover;
}

OptionList > .option-list--option-hover {
    background: $bg-hover;
}

/* DataTable */
DataTable {
    background: $bg;
}

DataTable > .datatable--header {
    background: $bg-elevated;
    color: $text-secondary;
    text-style: bold;
}

DataTable > .datatable--cursor {
    background: $accent-dark;
}

/* Horizontal Rule */
Rule.-horizontal {
    color: $border;
    margin: 1 2 1 0;
}

/* Footer */
Footer {
    background: $bg-elevated;
}

FooterKey {
    background: $bg-elevated;
    color: $text-secondary;
}

FooterKey > .footer-key--key {
    background: $accent-dark;
    color: $text-primary;
}

/* ListItem styling */
ListItem {
    background: $bg;
    padding: 0 1;
    height: 1;
}

ListItem:hover {
    background: $bg-hover;
}

ListItem.-selected {
    background: $accent-dark;
}

ListView {
    background: $bg;
}

/* Static content */
Static {
    color: $text-primary;
}

/* Checkbox and RadioButton */
Checkbox {
    background: transparent;
}

Checkbox > .toggle--button {
    background: $bg-elevated;
}

Checkbox:focus > .toggle--button {
    background: $accent-dark;
}

RadioButton {
    background: transparent;
}

RadioSet {
    background: transparent;
    border: none;
}

/* Tabs */
Tabs {
    background: $bg;
}

Tab {
    background: $bg;
    color: $text-secondary;
}

Tab:hover {
    background: $bg-hover;
}

Tab.-active {
    background: $bg-elevated;
    color: $accent;
}

/* ContentSwitcher */
ContentSwitcher {
    background: $bg;
}

/* Containers */
Horizontal {
    height: auto;
}

Vertical {
    height: auto;
}

Container {
    height: auto;
}

/* Scrollbars */
.scrollbar {
    background: $bg-elevated;
}

/* Action row */
.action-row {
    layout: horizontal;
    height: 3;
    margin: 1 0;
}

.action-row Button {
    margin-right: 1;
}

/* Base branch row in modal */
.base-branch-row {
    height: 3;
    width: 100%;
}

.base-branch-row Input {
    width: 1fr;
}

.base-branch-row Button {
    min-width: 10;
    margin-left: 1;
}

/* Branch tag */
.branch-tag {
    background: $accent-dark;
    color: $accent;
    padding: 0 1;
}

/* Keyboard shortcut badge */
.shortcut-badge {
    background: $bg-elevated;
    color: $text-muted;
    padding: 0 1;
}

/* Sidebar Header Box */
#sidebar-header-box {
    width: 100%;
    height: 3;
    background: $bg-elevated;
    border-bottom: solid $border;
    align: center middle;
    padding: 1 0 0 0;
}

#gh-status {
    text-align: center;
    width: 100%;
    color: $text-muted;
}

.gh-status-ok {
    color: $success;
}

.gh-status-warn {
    color: $warning;
}

.gh-status-error {
    color: $text-muted;
}

/* Async-loaded sections */
#issues-container {
    margin-top: 1;
}

#sessions-container {
    margin-top: 1;
}

/* Section header with refresh button */
.section-header-row {
    height: auto;
    width: auto;
}

.section-header-row .section-header {
    width: auto;
    margin: 0;
}

.refresh-btn {
    min-width: 3;
    width: 3;
    height: 1;
    padding: 0;
    margin: 0;
    border: none;
    background: transparent;
    color: $text-muted;
    text-style: none;
}

.refresh-btn:focus {
    color: $text-muted;
    background: transparent;
    border: none;
    text-style: none;
}

.refresh-btn:hover {
    color: $accent;
    background: transparent;
    border: none;
}

.refresh-btn:focus:hover {
    color: $accent;
    background: transparent;
    border: none;
}

/* Issue List */
.issue-row {
    height: auto;
    margin: 0 2 1 0;
    padding: 1;
    background: $bg-elevated;
    border: solid $border;
}

.issue-row:hover {
    background: $bg-hover;
}

.issue-info {
    width: 1fr;
}

.issue-title {
    color: $text-primary;
}

.issue-meta {
    color: $text-muted;
}

.issue-btn {
    min-width: 12;
    margin-left: 1;
}

.issue-title-preview {
    margin-bottom: 1;
}

/* Branch search dropdown */
BranchSearchInput OptionList {
    max-height: 10;
    height: auto;
    background: $bg;
    border: solid $border;
    margin-top: 0;
}

BranchSearchInput OptionList > .option-list--option-highlighted {
    background: $accent-dark;
}

BranchSearchInput .match-count {
    color: $text-muted;
    height: 1;
    padding: 0 1;
}
```

`BranchSearchInput` additionally carries a `DEFAULT_CSS` block (`branch_search.py:20-37`) which
is **overridden** by the app-level rules above for the `OptionList` background (`$surface` vs
`$bg`), because `App.CSS` has higher priority than a widget's `DEFAULT_CSS`.

### 11.3 Glyph inventory

Every non-ASCII character in the UI, with its exact code point:

| Glyph | Code point | Where | file:line |
|---|---|---|---|
| `├─` | U+251C U+2500 | sidebar, non-last worktree prefix | `sidebar.py:145` |
| `└─` | U+2514 U+2500 | sidebar, last worktree prefix | `sidebar.py:145` |
| `⚠` | U+26A0 | `⚠ MISSING:` header line | `worktree_detail.py:119` |
| `⟳` | U+27F3 | all three Git Pull button labels | `worktree_detail.py:143,149,152`; `repository_detail.py:128,131` |
| `•` | U+2022 | session meta separator, issue label separator | `worktree_detail.py:364`; `repository_detail.py:317`, `repository_detail.py:436` (as `•`) |
| `↻` | U+21BB | issues refresh button, idle state | `repository_detail.py:209`, `repository_detail.py:413` |
| `↑` / `↓` | U+2191 / U+2193 | custom-button reorder buttons | `modals.py:923`, `modals.py:929` |
| `\|` `/` `-` `\` | ASCII | spinner cycle | `repository_detail.py:91`, `modals.py:577` |

Additionally: several button and label strings begin with a **single leading ASCII space**
(U+0020), which is a remnant of removed icon glyphs. These must be reproduced literally:
`" forestui"`, `" Archived"`, `" Add Worktree"`, `" Editor"`, `" Terminal"`, `" Files"`,
`" Archive"`, `" Unarchive"`, `" Delete"`, `" Remove Repository"`, `" Add Repository"`,
`" Settings"`, `" Add Worktree"` (modal title), `" New Branch"`, `" Existing"`,
`" Custom Claude Buttons"`, `" Add Button"`, `" Edit Button"`, `" Add Button"` (list button),
and the leading-space error strings in §4.6 / §4.11.

### 11.4 State → visual mapping

| State | Visual |
|---|---|
| Repository selected | tree cursor row on the repo node, `$accent-dark` background; detail pane shows `MAIN REPOSITORY` |
| Worktree selected | cursor on the leaf; detail pane shows `WORKTREE` |
| Worktree archived | disappears from the tree entirely (`show_archived` is always false); the detail pane MANAGE row shows ` Unarchive` instead of ` Archive` |
| Worktree directory missing | `⚠ MISSING:   directory no longer exists on disk` in `$destructive`; LOCATION path suffixed `  (missing)` and rendered `$destructive` on the `.path-display` box; sync button labelled `⟳ Git Pull (Directory missing)` and disabled |
| No upstream branch | sync button `⟳ Git Pull (No remote)`, disabled |
| Detached HEAD (repository) | `Branch:     HEAD` in `$accent` (G2 returns the literal `"HEAD"` when `--show-current` prints nothing) |
| Custom button with `--dangerously-skip-permissions` | `Button.-destructive` + `variant="error"`: bg `#3d2020`, fg `#FF6B6B`, border `#5a3030`; in `CustomButtonsModal` the label also gets the suffix ` (YOLO)` |
| gh authenticated | `gh cli: ok (login)` in `$success` `#52B788` |
| gh unauthenticated | `gh cli: unauth'd` in `$warning` `#FFB347` |
| gh missing | `gh cli: missing` in `$text-muted` |
| Issues loading | `↻` replaced by a spinner glyph, button disabled; `#issues-container` shows `Loading...` |
| Update available | window title `forestui vX.Y.Z (updated to vA.B.C - restart to apply)` |

Textual `Button` variants used: `default`, `primary`, `error`. `variant="error"` supplies
Textual's own error styling, which the `-destructive` class then overrides.

### 11.5 Layout geometry summary

| Element | Height | Width |
|---|---|---|
| `Header` | Textual default (1 row collapsed) | full |
| `#sidebar` | fills `#main-container` | 35 (clamped 30-45) |
| `#sidebar-header-box` | 3 | 100% of sidebar |
| `Tree` | remaining sidebar height | sidebar width, `padding: 0 1` |
| `#detail-pane` | 100% | `1fr`, `padding: 1 2` |
| `.action-row` | 3 | auto, horizontal |
| `Button` (default) | 3 | `min-width: 10` |
| `.session-btn` | 3 | `min-width: 8`, `margin-left: 1` |
| `.issue-btn` | 3 | `min-width: 12`, `margin-left: 1` |
| `.refresh-btn` | 1 | 3 (fixed), no border, transparent |
| `.modal-container` | auto, `max-height: 90%` | 80, `max-width: 90%` |
| `.modal-container.modal-wide` | 90% | 140, `max-width: 95%` |
| `.modal-scroll` | auto, `max-height: 20` | 100% |
| `.modal-scroll.modal-scroll-tall` | `1fr`, `max-height: 100vh` | 100% |
| `BranchSearchInput OptionList` | auto, `max-height: 10` | inherited |
| `Rule.-horizontal` | 1 | with `margin: 1 2 1 0` |
| `Footer` | 1 | full |

### 11.6 Markup interpretation — mandatory to reproduce

Two different markup parsers are in play, and both silently delete text.

**(a) Tree labels use Rich markup.** `Tree.process_label` calls `rich.text.Text.from_markup`
(`textual/widgets/_tree.py:857-858`). Rich's tag regex is:

```python
RE_TAGS = re.compile(
    r"""((\\*)\[([a-z#/@][^[]*?)])""",
    re.VERBOSE,
)
```
— `rich/markup.py:12-15`

Any `[…]` whose first inner character is a lowercase ASCII letter, `#`, `/` or `@`, and which
contains no further `[`, is consumed as a style tag and **removed from the visible text**. The
resulting style name is resolved with `console.get_style(name, default="")`, so an unknown style
renders as no style rather than raising.

Applied to `wt_label = f"{prefix}  {worktree.name} [{worktree.branch}]"` (`sidebar.py:146`):

| Branch | Rendered tree label |
|---|---|
| `main` | `├─  my-feature ` (branch removed) |
| `feat/my-feature` | `├─  my-feature ` (branch removed) |
| `Foo` | `├─  my-feature [Foo]` (uppercase first char → not a tag → kept) |
| `123-fix` | `├─  my-feature [123-fix]` (digit first char → kept) |

Verified empirically against the installed Rich. Practical consequence: for conventional branch
names the sidebar shows `<prefix>  <name> ` with a trailing space and **no branch**. A Rust port
that prints the branch will not match.

**(b) `Label` content uses Textual's own markup.** `Content.from_markup` is stricter and only
recognises tags it can parse as styles. Verified:

| Source string | Rendered |
|---|---|
| `"or press [a] to add a repository"` (`app.py:62`) | `or press  to add a repository` — `[a]` consumed as a tag |
| `"Select a repository or worktree"` | unchanged |
| `"├─  my-feature [feat/my-feature]"` under Textual markup | unchanged (Textual, unlike Rich, does not treat `feat/my-feature` as a tag) |

So the same bracketed text behaves differently in a `Tree` label (Rich) and a `Label` (Textual).
All other user-supplied strings rendered through `Label` — branch names, paths, commit hashes,
issue titles, session titles, custom-button commands — are **not** escaped, so a value containing
`[b]`, `[/]`, `[red]`, `[#ff0000]` etc. will be interpreted as markup and disappear or restyle
the line.

---

## 12. Error handling & edge cases

### 12.1 Stale / pruned worktrees (the `b8f2bc5` fix)

Symptom before the fix: selecting a worktree whose directory had been deleted crashed the app.
`GitService._run_git` passes the worktree path as the subprocess `cwd`; `create_subprocess_exec`
raises `FileNotFoundError` when that path is gone. Every call site caught only `GitError`, so the
exception escaped through `_refresh_detail_pane` and killed the process.

Fix, at the single choke point (`git.py:52-60`):

```python
        try:
            process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                cwd=str(cwd) if cwd else None,
            )
        except OSError as e:
            raise GitError(f"Failed to run git in {cwd}: {e}") from e
```

This also covers `git` missing from `PATH` and permission errors on the cwd.

Second half of the fix — the UI now says so. `app.py:264` computes
`path_exists = worktree.get_path().exists()` and passes it to `WorktreeDetail`, producing the
three visual changes in §11.4. Regression test (`tests/test_git_service.py:10-18`):

```python
def test_missing_cwd_raises_git_error() -> None:
    """A stale worktree (deleted directory) must surface as GitError, not OSError."""
    git = get_git_service()

    with pytest.raises(GitError):
        asyncio.run(git.get_latest_commit("/nonexistent/stale/worktree"))

    with pytest.raises(GitError):
        asyncio.run(git.has_remote_tracking("/nonexistent/stale/worktree"))
```

Residual behavior for a stale worktree: it stays in the sidebar and in the state file; `e`/`t`/
`o`/`n`/`y` still create tmux windows with a non-existent `-c` directory (tmux itself will fail
or fall back to the default directory); Delete still works because `git worktree remove` failures
are suppressed and the state entry is removed unconditionally.

### 12.2 Missing directories generally

| Case | Behavior |
|---|---|
| Forest directory does not exist | created silently on first `_get_config_path()` (`state.py:32`) |
| `<forest>/<repo>/` does not exist | created by `create_worktree` before `git worktree add` (`git.py:160`) |
| `~/.config/forestui/` does not exist | created on save (`services/settings.py:62`); missing is fine on load |
| `~/.claude/projects/<folder>` does not exist | `get_sessions_for_path` returns `[]` → `No sessions found` |
| Repository `source_path` deleted | Every git probe raises `GitError`; `RepositoryDetail` renders with no branch line, no commit line, and a disabled `⟳ Git Pull (No remote)` button. There is **no** "missing" indicator for repositories — only worktrees get one. |

### 12.3 Dirty trees

forestui never inspects the working tree — no `git status`, no `--porcelain` status parsing, no
dirty indicator anywhere. Consequences:

- `git pull` on a dirty tree fails with git's own message, surfaced as `Sync failed: <stderr>`.
- `git worktree remove` on a dirty worktree fails, so the code immediately retries with
  `--force` (`git.py:226-230`), which **discards uncommitted work without any additional
  confirmation** beyond the initial `ConfirmDeleteModal`.

### 12.4 Rename failure modes

```python
old_path = Path(worktree.path)
new_path = old_path.parent / event.new_name

if new_path.exists():
    self.notify("Path already exists", severity="error")
    return

try:
    # Rename the directory
    old_path.rename(new_path)
    # Repair git references
    await self._git_service.repair_worktree(repo.source_path, new_path)
    # Migrate Claude sessions
    self._claude_service.migrate_sessions(old_path, new_path)
    # Update state
    self._state.update_worktree(
        event.worktree_id, name=event.new_name, path=str(new_path)
    )
    self._refresh_sidebar()
    await self._refresh_detail_pane()
except (OSError, GitError) as e:
    self.notify(f"Rename failed: {e}", severity="error")
```
— `app.py:615-636`

Four sequential side effects with no rollback:

| Failure point | Resulting inconsistency |
|---|---|
| `new_path.exists()` | none — early return with `Path already exists` |
| `old_path.rename` raises `OSError` (cross-device, permissions, name too long) | none |
| `repair_worktree` raises `GitError` | directory renamed, git metadata stale, state still points at the old path ⇒ a stale worktree (§12.1) plus an orphaned directory |
| `migrate_sessions` raises (only `OSError` from `shutil.move`/`rmdir` is possible; it is inside the `try`) | directory renamed and repaired, sessions half-moved, state stale |
| `update_worktree` raises `ValidationError` | not caught by `(OSError, GitError)` → propagates → crash |

The new name is **not sanitized** — `event.new_name` is whatever the user typed, so path
separators are possible (`old_path.parent / "a/b"` silently nests).

Branch rename (`app.py:638-653`) is simpler: `git branch -m` in the worktree, then
`update_worktree(branch=…)`. A `GitError` aborts before the state write, so the two stay
consistent.

### 12.5 Detached HEAD

- `get_current_branch` returns the literal `"HEAD"` when `git branch --show-current` prints
  nothing (`git.py:82`), so the repository detail shows `Branch:     HEAD`.
- `_import_existing_worktrees` uses `wt_info.branch or "HEAD"` (`app.py:1011`), so an imported
  detached worktree is stored with branch `"HEAD"` — and any later "rename branch" attempt on it
  would run `git branch -m HEAD <new>`.
- `has_remote_tracking` returns `False` for detached HEAD, disabling the pull button.

### 12.6 No repositories

`AppState._load_state` leaves `_repositories` empty; `on_mount`'s auto-select condition
`self._state.repositories` is falsy, so nothing is selected and `EmptyState` stays mounted. The
sidebar tree is empty. `w`, `e`, `t`, `o`, `n`, `y` silently do nothing (`_get_selected_path`
returns `None`), except `w` which notifies `Select a repository first`. `d` does nothing. `a`
opens the add-repository modal.

### 12.7 tmux absent or unusable

| Situation | Behavior |
|---|---|
| `tmux` not on `PATH` at startup, `$TMUX` unset | stderr message + `sys.exit(1)` (§1.2) |
| `$TMUX` set but the server is gone | `TmuxService.server` constructs a `Server()` which will raise on first use; `LibTmuxException` is caught in every method, so every window-creating call returns `False`/`None` → notifications `Failed to create terminal window` / `Failed to create mc window` / `Failed to create Claude window`. For editors it falls through to `Popen` (§9.6). |
| `$TMUX_PANE` unset | `rename_window` returns `False`; nothing is notified (the return value is discarded at `cli.py:30`). The window keeps whatever name tmux gave it. |
| `tmux set-option -g focus-events on` fails | notification `Could not enable focus events` (warning); the app runs but does not auto-refresh on focus. |
| `list-clients` returns no usable rows | `session` falls back to the first attached session, then to `sessions[0]`, then `None`. |

Note libtmux treats **any** stderr output as fatal (`raise LibTmuxException(cmd.stderr)`), so a
tmux warning printed to stderr on an otherwise successful command aborts the operation.

### 12.8 Modal-specific edge cases

| Case | Behavior |
|---|---|
| Add Repository with an invalid path and pressing `Add Repository` | `_add_repository` returns silently; the modal stays open showing whatever the validator wrote |
| Add Repository accepts a path that is itself a **worktree** | `<path>/.git` is a file, `Path.exists()` is `True`, so it is accepted as a repository |
| Add Repository with `import_worktrees` on | `_import_existing_worktrees` (`app.py:993-1018`) runs `git worktree list --porcelain`, skips the entry whose resolved path equals the resolved `source_path`, skips anything already under the forest directory (`str(wt_path).startswith(str(forest_dir))` — a **prefix string** test, so `/home/u/forestX` matches forest `/home/u/forest`), and adds the rest. The notification reports `len(worktrees) - 1`, which counts skipped forest-resident worktrees as imported and is therefore wrong whenever any were skipped. |
| Two repositories with the same basename | Both are added; `Repository.name` is the basename, so their worktrees collide under `<forest>/<name>/` and their tmux window names collide. Nothing prevents this. |
| Add Worktree where the target path exists | blocked with ` Worktree path already exists` |
| Add Worktree, existing-branch mode, branch already checked out elsewhere | `git worktree add` fails; `Failed to create worktree: <stderr>` |
| Add Worktree with a name that sanitizes to empty (e.g. `///`) | `_name` becomes `""` → ` Worktree name is required` |
| Settings modal with a `default_editor` not present in `EDITORS` | `Select(value=…)` raises `InvalidSelectValueError` (`textual/widgets/_select.py:591`) during compose, escaping `action_open_settings`'s worker. Reachable by hand-editing `settings.json`. Same for an out-of-range `theme`. |
| Save settings while `select-editor.value` is `Select.BLANK` | `str(value) if value else "code"` — but `Select.BLANK` is truthy, so `str(BLANK)` would be stored. Not reachable through the UI since a valid value is always preselected. |
| Custom button label/prefix duplicate | blocked in `CustomButtonEditModal._save` against the *other* buttons only, so editing a button to keep its own label is allowed |
| `derive_prefix` yields an empty string (label is all punctuation) | `validate_button_prefix("")` → ` Prefix cannot be empty` |
| Issue modal Create with an empty base branch | allowed; `create_worktree` runs `git worktree add -b <branch> <path>` with no base, so the new branch stems from the repo's current HEAD, and `created_from_ref` is `None` |

### 12.9 Corrupt or hostile config files

| File | Malformed JSON | Schema violation |
|---|---|---|
| `.forestui-config.json` | `json.JSONDecodeError` caught (`state.py:44`) → treated as empty | Pydantic `ValidationError` **not caught** → propagates out of `AppState.__init__` → caught by `run_app`'s blanket handler → traceback written to `~/.forestui-error.log`, printed, `Press Enter to exit...`, exit 1 |
| `settings.json` | `json.JSONDecodeError` caught (`services/settings.py:56`) → defaults | Same uncaught `ValidationError` path, triggered lazily at the first `SettingsService()` construction (in `ForestApp.__init__`) |

Neither file is written atomically, so a crash or a full disk during save can truncate them into
exactly these states.

### 12.10 Concurrency and multi-instance

Two forestui processes on the same forest share `.forestui-config.json` with **last-write-wins**
and no locking; each holds the whole repository list in memory and rewrites the entire file on
every mutation, so one instance's changes are silently clobbered by the other's next write.

The grouped-session design (§9.3) means a second terminal attaching to the same forest normally
does **not** start a second forestui process — it joins the existing one. A second process only
appears when the forestui window was killed (T3 recovery) or when the T4 grouped-session creation
failed.

### 12.11 Unicode and encoding

- `git` stdout/stderr is decoded strict UTF-8 (`git.py:64-65`); invalid bytes raise
  `UnicodeDecodeError`, uncaught.
- `gh` output uses the default codec, same exposure.
- Session JSONL files are opened with `encoding="utf-8"`; a decode error inside the read loop
  raises `UnicodeDecodeError`, which is **not** in the caught `OSError` set (`claude_session.py:135`)
  and propagates out of the `_fetch_sessions_for_path` worker.
- Config files are read and written with `encoding="utf-8"` explicitly.

### 12.12 Errors that are deliberately swallowed

| Site | Swallowed |
|---|---|
| `app.py:158-162`, `app.py:169-175` | `Exception` while locating the detail widget for a background update |
| `app.py:600`, `app.py:809` | `GitError` from `git worktree remove` (via `contextlib.suppress`) |
| `sidebar.py:243-255` | `Exception` while updating the gh badge |
| `repository_detail.py:376-377`, `:394-395`, `:403-404`, `:414-416`, `:457-458` | `Exception` around every widget query in the sessions/issues/spinner paths |
| `worktree_detail.py:423-424` | `Exception` in `update_sessions` |
| `modals.py:690-691`, `:700-701`, `:723-724`, `:744-745` | `Exception` around spinner/branch-list widget queries |
| `branch_search.py:77-78`, `:83-84` | `Exception` in `selected_value` / `set_value` |
| `git.py:132-137` | `GitError` in `_safe_list_remotes` |
| `tmux.py` throughout | `LibTmuxException` in every public method |
| `app.py:236-238` | `CalledProcessError`, `TimeoutExpired`, `OSError` in the updater |
| `libtmux/window.py:479-484` | **any** `Exception` in `rename_window`, logged only |

---

## 13. Behavioral invariants

Numbered, testable assertions the Rust port must satisfy. Each cites the enforcing code.

### Startup & CLI

1. **When `$TMUX` is unset and `tmux` is not on `PATH`, the app MUST print the five-line install
   hint to stderr and exit with status 1**, without touching any config file. (`cli.py:55-63`)
2. **When `$TMUX` is unset and tmux exists, the process MUST be replaced via `execvp` and MUST
   NOT render any UI.** (`cli.py:130`, `161`, `164`)
3. **When `$TMUX` is set, `ensure_tmux` MUST return immediately without spawning any tmux
   command.** (`cli.py:51-52`)
4. **The tmux session name MUST be `forestui-` + slugified basename of the resolved forest path,
   defaulting to `forestui-forest`.** Slugify = lowercase, every run of `[^a-zA-Z0-9]+` → `-`,
   then strip leading/trailing `-`. (`cli.py:33-40`, `66-71`)
5. **When the session already exists and no window is named `forestui` or `forestui-dev-*`, a new
   window MUST be created running the reconstructed command line before attaching.**
   (`cli.py:101-118`)
6. **When the session already exists, the client MUST attach to a grouped session named
   `<session>-<pid>`, falling back to the base session only if grouped-session creation returns
   non-zero.** (`cli.py:123-130`, `161`)
7. **The grouped session MUST receive a `client-attached` hook setting
   `destroy-unattached keep-last`, and MUST NOT have `destroy-unattached` set directly.**
   (`cli.py:135-145`)
8. **The grouped session's `status-left` MUST be the global `status-left` with every `#S`
   replaced by the base session name, and MUST be set only when `show-options -gv status-left`
   exits 0 with non-empty output after stripping the trailing newline only.** (`cli.py:149-160`)
9. **The reconstructed command line MUST be `forestui` plus, in this order, ` --debug`,
   ` --no-self-update`, ` --dev`, and ` <shlex-quoted forest path>`, each only when its flag is
   set.** (`cli.py:74-82`)
10. **When `__version__ == "0.0.0"`, dev mode MUST be enabled regardless of `--dev`.** (`cli.py:198`)
11. **The tmux window hosting forestui MUST be renamed to `forestui`, or `forestui-dev-HHMM` in
    dev mode, where `HHMM` is local wall-clock time at startup.** (`cli.py:16-23`, `208`)
12. **Window renaming MUST target the window containing `$TMUX_PANE`, not the session's active
    window; when `$TMUX_PANE` is unset the rename MUST be skipped silently.** (`tmux.py:109-130`)
13. **`--no-self-update` MUST set `FORESTUI_NO_AUTO_UPDATE=1` and the updater MUST return before
    spawning anything when that variable is truthy.** (`cli.py:205-206`, `app.py:200-201`)
14. **The updater MUST run exactly `uv tool upgrade forestui` with a 120-second timeout, on a
    thread, and MUST NOT restart or reload the running process under any outcome.**
    (`app.py:206-211`)
15. **The window title MUST be `forestui v<version>` with no suffix, except during the update
    check (`(checking for updates...)`) and after a successful upgrade
    (`(updated to v<X> - restart to apply)` or `(updated - restart to apply)`).**
    (`app.py:177-231`)
16. **Any exception escaping `app.run()` MUST write the traceback to `~/.forestui-error.log`,
    print it plus `Error: <e>` and the log path to stderr, block on `Press Enter to exit...`,
    and exit 1.** (`app.py:1030-1038`)

### Persistence

17. **The forest directory MUST be created (with parents) on every config-path resolution,
    including on load.** (`state.py:32`)
18. **Repository state MUST be written to `<forest>/.forestui-config.json` synchronously after
    every one of: add repository, remove repository, add worktree, remove worktree, update
    worktree (including archive/unarchive).** (`state.py:80-142`)
19. **Repository state MUST be serialized with 2-space indent, keys in model-declaration order,
    UUIDs as lowercase hyphenated strings, and `last_modified` as RFC-3339 with microseconds and
    a `Z` suffix.** (`state.py:53`; §3.10)
20. **A `.forestui-config.json` that is not valid JSON MUST be treated as an empty repository
    list without notification; a file that is valid JSON but violates the schema MUST crash the
    app through the top-level handler.** (`state.py:39-45`)
21. **Settings MUST be written to `~/.config/forestui/settings.json` only when the settings modal
    is saved, never on any other event.** (`app.py:836`)
22. **`Settings.theme` MUST be persisted and MUST have no effect on rendering.** (§14)
23. **`Settings.default_terminal` MUST be persisted as `""` and MUST never be read.** (§14)
24. **Saving settings MUST reset `default_terminal` to `""` because the save path constructs a
    fresh `Settings` without it.** (`modals.py:469-474`)
25. **Selection MUST NOT be persisted; every launch MUST start with an empty `Selection`.**
    (`state.py:25`)

### Selection & navigation

26. **On mount, when nothing is selected and at least one repository exists, `repositories[0]`
    MUST be selected and the detail pane rendered.** (`app.py:116-118`)
27. **Moving the tree cursor with arrow keys MUST change the selection and re-render the detail
    pane — selection follows the cursor, no Enter required.** (`sidebar.py:199-203`)
28. **Selecting a repository MUST clear `worktree_id`; selecting a worktree MUST set both ids.**
    (`state.py:144-150`)
29. **Highlighting or selecting the Archived section node MUST NOT change the selection.**
    (`sidebar.py:205-217`)
30. **The first Enter on a collapsed, not-previously-selected repository node MUST leave it
    expanded; a second Enter on the same node MUST collapse it.** (`sidebar.py:186-192`)
31. **Removing the selected repository MUST reset the selection to `(None, None)`; removing the
    selected worktree MUST reduce it to `(repository_id, None)`.** (`state.py:88-89`, `119-120`)

### Detail pane

32. **The detail pane MUST contain exactly one of `WorktreeDetail`, `RepositoryDetail` or
    `EmptyState`, and MUST be cleared before the replacement is mounted.** (`app.py:252-255`)
33. **When `worktree_id` is set but the worktree cannot be found, the detail pane MUST be left
    empty — neither `RepositoryDetail` nor `EmptyState` may be mounted.** (`app.py:259-292`)
34. **`path_exists` MUST be evaluated with a filesystem check on `Path(worktree.path).expanduser()`
    before mounting, and MUST NOT resolve symlinks.** (`app.py:264`, `models.py:129-131`)
35. **When a worktree directory is missing, the header MUST include the line
    `⚠ MISSING:   directory no longer exists on disk`, the LOCATION label MUST read
    `<path>  (missing)` in the destructive colour, and the sync button MUST read
    `⟳ Git Pull (Directory missing)` and be disabled.** (`worktree_detail.py:117-171`)
36. **The sync button's three states MUST be evaluated in the order: missing directory, then
    has-remote, then no-remote.** (`worktree_detail.py:140-156`)
37. **Repository detail MUST NOT display any missing-directory indicator.** (`repository_detail.py:99-135`)
38. **Commit hash and time MUST be omitted entirely when `git log -1` fails; the fallback MUST
    also skip the remote-tracking probe, leaving `has_remote` false.** (`app.py:267-279`, `304-317`)
39. **Claude buttons MUST be laid out 4 per row, in the order: `New Session`,
    `New Session: YOLO`, then each configured custom button in settings order.**
    (`repository_detail.py:163-198`, `worktree_detail.py:187-222`)
40. **A custom button whose command contains the substring `--dangerously-skip-permissions` MUST
    be rendered with `variant="error"` and class `-destructive`; all others with
    `variant="primary"`.** (`models.py:108-111`, `repository_detail.py:167-180`)
41. **At most 5 Claude sessions and at most 5 GitHub issues MUST be rendered.**
    (`repository_detail.py:296`, `:429`; `worktree_detail.py:343`)
42. **Session titles MUST be truncated to 60 characters with a literal `...` appended only when
    the original exceeded 60; last-message previews to 40 the same way.**
    (`repository_detail.py:297-309`)
43. **The last-message line MUST be omitted when `last_message` is empty or equal to `title`.**
    (`repository_detail.py:306`)
44. **Issue titles MUST be truncated to 45 characters the same way, and at most 2 label names
    MUST be shown, joined with `", "`.** (`repository_detail.py:430-436`)
45. **Empty session and issue lists MUST render exactly `No sessions found` and `No issues found`
    respectively; before data arrives both MUST render `Loading...`.**
    (`repository_detail.py:203`, `:211`, `:375`, `:456`)

### Git

46. **Every git invocation MUST be `git` + args with no shell, with `cwd` set to the
    `expanduser()`-ed (never `resolve()`-ed) path.** (`git.py:41-66`)
47. **A failure to spawn git — missing directory, missing binary, permission error — MUST surface
    as a `GitError` with the message `Failed to run git in <cwd>: <os-error>`, never as a raw
    OS error.** (`git.py:59-60`; enforced by `tests/test_git_service.py`)
48. **`git branch --show-current` returning empty output MUST be reported as the literal branch
    `HEAD`.** (`git.py:82`)
49. **Branch listing MUST use `git branch -a --format=%(refname:short)`, MUST drop lines ending
    in `/HEAD`, MUST drop lines exactly equal to a remote name, and MUST return the result
    sorted.** (`git.py:98-122`)
50. **Creating a worktree on a new branch with a remote base MUST run `git branch
    --unset-upstream <branch>` inside the new worktree afterwards, and MUST ignore its result.**
    (`git.py:172-179`)
51. **Creating a worktree from an existing remote branch MUST use
    `git worktree add --track -b <branch-without-remote-prefix> <path> <remote>/<branch>`.**
    (`git.py:181-201`)
52. **`git worktree remove` failing MUST be retried exactly once with `--force`, and a failure of
    the forced attempt MUST raise.** (`git.py:224-230`)
53. **Deleting a worktree MUST remove it from state even when `git worktree remove` fails at both
    attempts.** (`app.py:600-604`, `app.py:809-813`)
54. **Removing a repository MUST NOT touch the filesystem.** (`state.py:85-90`)
55. **`git log -1 --format=%H|%h|%ct` output MUST split into exactly 3 pipe-separated fields, and
    the timestamp MUST be parsed as a UTC Unix epoch.** (`git.py:322-327`)
56. **`has_remote_tracking` MUST be true only when `git rev-parse --abbrev-ref
    --symbolic-full-name @{u}` exits 0 **and** prints non-whitespace.** (`git.py:345-350`)
57. **`get_ref` MUST return `None` on non-zero exit rather than raising.** (`git.py:309-311`)
58. **Worktree import MUST skip the entry whose resolved path equals the repository's resolved
    source path, and MUST skip any entry whose path string starts with the forest path string.**
    (`app.py:999-1007`)

### tmux windows

59. **Editor windows MUST be named `edit:<name>` and MUST reuse an existing window of that name
    via `select-window` rather than creating a second one.** (`tmux.py:194-201`)
60. **Terminal, file-manager and Claude windows MUST always create a new window, appending
    `:2`, `:3`, … to the base name until it is unique among the session's window names.**
    (`tmux.py:277-301`)
61. **Window base names MUST be `term:<name>`, `files:<name>`, `claude:<name>`, `yolo:<name>`, and
    `<custom-prefix>:<name>`.** (`tmux.py:229`, `:259`, `:330-335`)
62. **`<name>` MUST be `<repo>:<worktree>` for a worktree path, `<repo>` for a repository source
    path, and the literal `session` for an unrecognised path, matched by exact string equality.**
    (`app.py:879-889`)
63. **The file-manager window MUST run exactly `mc`.** (`tmux.py:269`)
64. **The terminal window MUST be created with no shell command, so it runs the default shell and
    persists after commands exit.** (`tmux.py:235-239`)
65. **Claude windows MUST run `<$SHELL or /bin/bash> -ic <shlex-quoted command>`.**
    (`tmux.py:352-353`)
66. **The `--dangerously-skip-permissions` flag MUST be appended only for the built-in YOLO
    action, never for custom buttons.** (`tmux.py:345-346`)
67. **Resuming MUST append ` -r <session-id>` to the command, for both built-in and custom
    buttons.** (`tmux.py:347-348`)
68. **New windows MUST be created with `attach=True` (no `-d`), so they become the current window
    in the target session.** (`tmux.py:207`, `:238`, `:268`, `:358`)
69. **The target session for new windows MUST be the session of the most recently active client
    within our own session group, falling back to the first attached session, then the first
    session, then failure.** (`tmux.py:55-107`)
70. **A TUI editor (`vim nvim vi emacs nano helix hx micro kakoune kak`, matched on the first
    whitespace-separated token) MUST be launched in a tmux window running `<editor> .`; anything
    else MUST be launched with `Popen` and its stdio sent to `/dev/null`.**
    (`tmux.py:13-24`, `:157-161`, `:208`; `app.py:895-912`)
71. **If tmux window creation for a TUI editor fails, the app MUST fall through to the `Popen`
    path rather than reporting an error.** (`app.py:896-912`)
72. **`tmux set-option -g focus-events on` MUST be attempted at mount, and its failure MUST
    produce the warning `Could not enable focus events`.** (`app.py:113-114`)
73. **Regaining terminal focus MUST re-render the detail pane.** (`app.py:128-131`)

### GitHub

74. **`gh auth status` MUST be probed exactly once per process; the result MUST be cached for the
    process lifetime and never re-probed.** (`github.py:66-67`)
75. **When auth status is anything other than `authenticated`, `list_issues` MUST return an empty
    list without spawning any `gh` process.** (`github.py:106-108`)
76. **The gh badge MUST read `gh cli: ...` before resolution, then exactly one of
    `gh cli: ok (<login>)`, `gh cli: ok`, `gh cli: unauth'd`, `gh cli: missing`.**
    (`sidebar.py:114`, `:228-241`)
77. **Issues MUST be fetched with two `gh issue list` calls — `--assignee @me` then
    `--author @me` — each `--state open --limit 10` with the JSON field list
    `number,title,state,url,createdAt,updatedAt,author,assignees,labels`, deduplicated by issue
    number with the assignee results taking precedence.** (`github.py:138-185`)
78. **The merged issue list MUST be sorted by `created_at` descending and truncated to 10, while
    the UI displays `updated_at` as the relative time.** (`github.py:184-185`, `models.py:255-258`)
79. **The issue cache MUST be keyed on `"<owner>/<name>"` with a 300-second TTL, and
    `gh repo view` MUST run on every `list_issues` call even on a cache hit.** (`github.py:110-127`)
80. **A 300-second timer MUST invalidate the entire issue cache and refetch issues for the
    currently selected repository, for the process lifetime.** (`app.py:126`, `:140-147`)
81. **The `↻` button MUST invalidate the cache and refetch; while a fetch is in flight it MUST
    show a spinner cycling `|`, `/`, `-`, `\` at 20 Hz and be disabled.**
    (`app.py:458-463`, `repository_detail.py:383-395`)
82. **An issue fetch that raises MUST both notify `Issue fetch error: <e>` and render
    `No issues found`.** (`app.py:152-162`)
83. **`GitHubIssue.branch_name` MUST be `<number>-<slug>` where slug is the lowercased title with
    every run of `[^a-z0-9]+` replaced by `-`, truncated to 40 characters, then stripped of
    leading/trailing `-`.** (`models.py:249-253`)

### Claude sessions

84. **Session files MUST be read from `~/.claude/projects/<abs-resolved-path with '/' replaced by
    '-'>/*.jsonl`, non-recursively, skipping files whose name starts with `agent-`.**
    (`claude_session.py:22-49`)
85. **A session file with zero user-role records MUST be discarded.** (`claude_session.py:138-139`)
86. **`title` MUST be the first user message that is non-empty and does not start with `<`,
    truncated to 100 characters; `last_message` MUST be the last such message, also truncated to
    100; both MUST have runs of 3+ newlines collapsed to 2.** (`claude_session.py:114-125`)
87. **When no timestamp is found in the file, `last_timestamp` MUST fall back to the file's mtime
    interpreted as UTC.** (`claude_session.py:141-142`)
88. **An empty title MUST become the literal `Untitled session`.** (`claude_session.py:147`)
89. **Sessions MUST be sorted newest-first by `last_timestamp` and truncated to 5.**
    (`claude_session.py:55-56`)
90. **Renaming a worktree MUST move every `*.jsonl` from the old Claude project folder to the new
    one, skipping files that already exist at the destination, and MUST remove the old folder only
    if it ends up empty.** (`claude_session.py:153-174`)

### Keybindings

91. **`q` MUST be a priority binding that quits even while a modal is open, unless an `Input` or
    `TextArea` has focus, in which case the character MUST be typed.**
    (`app.py:72`; `textual/app.py:4105`, `textual/screen.py:427-438`)
92. **Every other single-character binding MUST be inert while a modal is open and while an
    `Input` has focus.** (`textual/screen.py:450-456`, `:427-438`)
93. **`escape` MUST dismiss the top modal with `None`, except `ConfirmDeleteModal` which MUST
    dismiss `False`.** (`modals.py:145-147`, `:514-516`, etc.)
94. **`w` with no repository selected MUST notify `Select a repository first` at warning
    severity.** (`app.py:724-725`)
95. **`h` MUST toggle `is_archived` only when a worktree is selected, and MUST be a no-op for a
    repository selection.** (`app.py:781-792`)
96. **`r` MUST be hidden from the footer while remaining functional.** (`app.py:83`)
97. **`?` MUST emit exactly the single-line help notification, which omits `o`, `y` and `r`.**
    (`app.py:846-851`)
98. **The footer MUST list, in order: `q a w e t o n y h d s ?`.** (`app.py:71-85`)

### Rendering fidelity

99. **The sidebar worktree label MUST be `<├─ or └─>` + two spaces + name + ` [` + branch + `]`,
    passed through a Rich-markup parser so that any bracketed segment starting with a lowercase
    letter, `#`, `/` or `@` is deleted from the visible text.** (`sidebar.py:145-147`; §11.6)
100. **The repository label MUST be one leading space plus the repository name.** (`sidebar.py:140`)
101. **The `└─` prefix MUST be chosen by value equality against the last active worktree, not
     identity.** (`sidebar.py:145`)
102. **All five `EmptyState` lines MUST render in `$text-muted` and centered, overriding their
     individual colour classes, and the last line MUST render as
     `or press  to add a repository` because `[a]` is consumed as markup.**
     (`theme.py:356-359`; `app.py:62`; §11.6)
103. **Archived worktrees MUST never appear in the sidebar, because `show_archived` is never set
     to true.** (`sidebar.py:150`; §14)
104. **Every one of the leading-space label strings listed in §11.3 MUST be reproduced with its
     leading `U+0020` intact.**
105. **The active-worktree ordering MUST be `sort_order` ascending with `None` last, then
     `last_modified` descending — which in practice is newest-first.** (`models.py:146-155`)

### Modal semantics

106. **The add-repository validator MUST check, in order: non-empty, exists, is a directory,
     has a `.git` entry — and MUST accept a `.git` file (i.e. another worktree) as valid.**
     (`modals.py:85-119`)
107. **Pressing `Add Repository` with an invalid path MUST silently do nothing.** (`modals.py:133-143`)
108. **Worktree names in the add-worktree modal MUST be sanitized to `[alnum]`, `-`, `_` for all
     internal use, while the visible input value MUST remain unsanitized.** (`modals.py:274-276`)
109. **In new-branch mode, changing the worktree name MUST overwrite the branch input with
     `<branch_prefix><sanitized-name>`.** (`modals.py:242-249`)
110. **In existing-branch mode, the Create button MUST be disabled unless the typed branch is
     exactly present in the branch list.** (`modals.py:265-272`)
111. **The add-worktree modal MUST reject a target path that already exists, with the message
     ` Worktree path already exists`.** (`modals.py:339-343`)
112. **The issue modal's default base branch MUST be the first `<remote>/<current-branch>` present
     in the branch list, else the local current branch, else the first branch, else empty.**
     (`modals.py:581-592`)
113. **The issue modal MUST always request `new_branch = True`.** (`modals.py:670`)
114. **`Pull repo before creating` MUST default to checked, and when checked the pull MUST run
     before `get_ref` and before `git worktree add`.** (`modals.py:628`, `app.py:508-526`)
115. **The custom-button prefix MUST auto-follow the label until the user types a prefix that
     differs from `derive_prefix(label)`, and MUST re-arm if the user types the derived value
     back.** (`modals.py:767-772`, `:827-839`)
116. **`derive_prefix` MUST lowercase, replace runs of `[^a-z0-9_-]` with `-`, strip leading and
     trailing `-`, then truncate to 20 — in that order.** (`models.py:18-24`)
117. **Custom-button save MUST reject a label or prefix already used by a different button, with
     the messages ` Another button already uses this label` / ` … prefix`.** (`modals.py:862-867`)
118. **Cancelling `CustomButtonsModal` MUST discard all add/edit/delete/reorder changes.**
     (`modals.py:884`, `:959-960`)
119. **Deleting a custom button MUST NOT ask for confirmation.** (`modals.py:971-974`)
120. **Saving settings MUST re-render the detail pane so custom-button changes take effect
     immediately.** (`app.py:838-839`)

### Concurrency & ordering

121. **Background session and issue updates MUST be dropped silently when the detail pane no
     longer hosts a widget of the expected type.** (`app.py:158-162`, `:169-175`)
122. **There MUST be no generation guard on background updates, so a stale fetch for a previous
     selection may overwrite the current pane's content when both are the same detail type.**
     (§8.3)
123. **Detail-pane git probes MUST complete before the widget is mounted, so the pane never shows
     a partially-populated header.** (`app.py:266-290`)
124. **State writes MUST be synchronous on the UI thread; there MUST be no debouncing or batching.**
     (`state.py:47-53`)
125. **Two forestui instances on the same forest MUST behave last-write-wins with no locking.**
     (§12.10)

### Error surfacing

126. **`git pull` failure MUST notify `Sync failed: <stderr-derived message>` at error severity
     and MUST NOT re-render the detail pane.** (`app.py:412-413`, `:449-450`)
127. **Worktree creation failure MUST notify `Failed to create worktree: <e>` and MUST NOT write
     any state.** (`app.py:539-540`, `:711-712`)
128. **Renaming to an existing path MUST notify `Path already exists` and abort before touching
     the filesystem.** (`app.py:618-620`)
129. **A rename that fails after `Path.rename` succeeded MUST leave the directory renamed and the
     state stale, notifying `Rename failed: <e>`.** (`app.py:622-636`; §12.4)
130. **Branch rename failure MUST notify `Branch rename failed: <e>` and leave state unchanged.**
     (`app.py:652-653`)
131. **Every tmux window-creation failure MUST notify the corresponding
     `Failed to create … window` message at error severity.** (`app.py:923`, `:931`, `:941`,
     `:958`, `:974`, `:991`)
132. **A GUI editor that cannot be found MUST notify `Editor '<editor>' not found` at error
     severity; any other `OSError` from `Popen` MUST propagate.** (`app.py:914-915`)

---

## 14. Appendix: dead code inventory

Code present in the source but not reachable through any user-facing path. A Rust port may omit
all of it; it is listed so nothing is mistaken for a missing feature.

| Item | file:line | Why unreachable |
|---|---|---|
| `GitService.is_git_repository` | `git.py:68-74` | no callers; the add-repository modal checks `.git` on the filesystem instead |
| `GitService.branch_exists` | `git.py:298-301` | no callers |
| `Settings.default_terminal` | `models.py:195` | never read; reset to `""` on every save |
| `Settings.theme` | `models.py:197` | persisted and shown in the modal, but the CSS is a single hard-coded dark theme with no theme switch |
| `AppState.show_archived` setter | `state.py:75-78` | never called ⇒ `_show_archived` is permanently `False` ⇒ the entire archived branch of `_populate_tree` (`sidebar.py:150-163`) and the `ArchivedNode` class are unreachable |
| `AppState.clear_selection` | `state.py:152-154` | no callers |
| `AppState.selected_repository` / `selected_worktree` | `state.py:156-168` | no callers |
| `AppState.has_archived_worktrees` / `all_archived_worktrees` | `state.py:170-183` | no callers |
| `AppState.reorder_worktree` | `state.py:185-205` | no callers ⇒ `Worktree.sort_order` is always `None` |
| `AppState.refresh_worktree_timestamp` | `state.py:207-209` | no callers ⇒ `last_modified` is only ever the creation time |
| `Sidebar.AddWorktreeRequested`, `DeleteRepositoryRequested`, `ArchiveWorktreeRequested`, `UnarchiveWorktreeRequested`, `DeleteWorktreeRequested` | `sidebar.py:65-99` | never posted (`AddWorktreeRequested` has an App handler at `app.py:357` that never fires) |
| `Sidebar.on_button_pressed` | `sidebar.py:219-222` | the sidebar composes no button with id `btn-add-repo` |
| `RepoNode.repo`, `WorktreeNode.repo`/`worktree` attributes | `sidebar.py:14-30` | only the ids are read |
| `BranchSearchInput.set_value` | `branch_search.py:80-86` | no callers |
| `BranchSearchInput.update_branches` | `branch_search.py:156-164` | no callers (the issue modal uses an `Input` + `FuzzyBranchSuggester`, not this widget) |
| `BranchSearchInput.BranchSelected` | `branch_search.py:47-52` | posted (`branch_search.py:127`) but no handler exists anywhere |
| `ClaudeSession.primary_branch` | `models.py:185-188` | no callers; `git_branches` is parsed but never displayed |
| `WorktreeInfo.head` | `git.py:15-19` | parsed but never read by callers |
| `CommitInfo.hash` | `git.py:23-28` | only `short_hash` and `timestamp` are read |
| `SettingsService.update` | `services/settings.py:74-79` | no callers |
| CSS classes `.sidebar-header`, `.header-buttons`, `.action-card`, `.detail-subtitle`, `.status-bar`, `.branch-tag`, `.shortcut-badge`, `.scrollbar`, `.label-primary` (partly), `$bg-selected` | `theme.py` | no widget carries these classes (`.label-primary` is used once, at `worktree_detail.py:111`) |
| CSS rules for `Collapsible`, `OptionList` (sidebar section), `DataTable`, `ListItem`, `ListView`, `RadioButton`, `RadioSet`, `Tabs`, `Tab`, `ContentSwitcher` | `theme.py:361-505` | those widgets are never instantiated (except `OptionList`, which only appears inside `BranchSearchInput` and has its own rules) |
| `AddWorktreeModal._error`, `AddRepositoryModal._error` | `modals.py:186`, `:52` | assigned in `__init__`, never used |
| `RepositoryDetail._sessions`, `_issues`; `WorktreeDetail._sessions` | `repository_detail.py:88-89`, `worktree_detail.py:93` | stored but never read |
| `forestui/__main__.py` | whole file | imports `forestui.app.main`; only reachable via `python -m forestui`, which is not the documented entry point |

### Known behavioral defects, documented as-is

| # | Defect | Evidence |
|---|---|---|
| D1 | Branch names are invisible in the sidebar for conventional names | §11.6(a) |
| D2 | `[a]` is deleted from the EmptyState hint | §11.6(b) |
| D3 | Archived worktrees vanish with no way to see or unarchive them except by keeping the selection alive | §14, `sidebar.py:150` |
| D4 | Worktree import notification reports `len(worktrees) - 1`, overcounting when entries were skipped | `app.py:1016` |
| D5 | Background fetches can write into the wrong detail widget after a fast selection change | §8.3 |
| D6 | A rename that fails mid-sequence leaves filesystem, git metadata and state inconsistent | §12.4 |
| D7 | A `default_editor` or `theme` value outside the fixed option lists crashes the settings modal | §12.8 |
| D8 | `gh auth status` is cached forever, so authenticating while running has no effect until restart | §10.2 |
| D9 | `git worktree remove` silently escalates to `--force`, discarding uncommitted work | §12.3 |
| D10 | Config files are written non-atomically | §12.9 |
| D11 | Failed TUI-editor window creation silently launches the editor into `/dev/null` | §9.6 |
| D12 | Worktree rename does not sanitize the new name, allowing path separators | §12.4 |

---

## 15. Build, test and release (context only)

| Concern | Value |
|---|---|
| Python | `requires-python = ">=3.14"`; `.python-version` pins `3.14` |
| Entry point | `[project.scripts] forestui = "forestui.app:main"` |
| Build backend | `hatchling`, wheel packages `["forestui"]` |
| Version in source | hard-coded `0.0.0`; the publish workflow rewrites `pyproject.toml` from the git tag (`sed -e 's,.*/\(.*\),\1,' -e 's/^v//'`) |
| Lint | `ruff check forestui/`, target `py314`, line length 88, rule set `E W F I B C4 UP ARG SIM TCH PTH RUF` minus `E501 TCH RUF012 ARG002` |
| Types | `mypy --strict` over `forestui`, `python_version = 3.14`, `textual.*` allowed missing imports |
| Format | `ruff format --check forestui/` |
| Tests | `pytest tests/` with `addopts = "-p no:libtmux"` (libtmux's pytest plugin is incompatible with pytest 8.x); one test file, `tests/test_git_service.py` |
| `make check` | `lint typecheck format-check test` |
| CI | `.github/workflows/check.yml` — `make check` on ubuntu-latest, on push to `main` and on all PRs |
| Release | `.github/workflows/publish.yml` — on GitHub release published: set version from `GITHUB_REF`, `uv build`, `pypa/gh-action-pypi-publish@v1.14.2` with `id-token: write` (trusted publishing) |
| Install | `install.sh` — requires tmux, installs `uv` if absent, `uv tool install forestui`, warns if `~/.local/bin` is not on `PATH`, and points out a legacy `~/.forestui-install` directory if present |
| Pre-commit | `ruff` (`--fix`) and `ruff-format` at `v0.14.11` |
