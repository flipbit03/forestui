# forestui — end-to-end `tu` acceptance playbook

Acceptance suite for the Python/Textual → Rust/ratatui rewrite. Every use case is
written so the **same steps** run against both builds and produce the **same
on-screen text**. If a Rust use case diverges from the "Expected" block, the
rewrite is not done.

- **P0** — must pass before the Rust PR merges.
- **P1** — should pass; a known, written-down divergence is acceptable only with sign-off.
- **P2** — nice to have; covers rarely-hit paths.

Every use case is tagged:

- 🟢 **EXECUTED** — actually run against the Python build (0.0.0, commit `3021d72`) on 2026-08-14 with `tu`; the Expected block is transcribed from real screenshots.
- ⚪ **SOURCE** — derived from reading the Python source; not driven live. Verify against Python first if a Rust run disagrees.

---

## 0. Ground rules (non-negotiable)

These come from `.claude/skills/test-forestui/SKILL.md` and apply to every use case.

1. **Never call `tmux` directly from Bash.** Not even with `TMUX_TMPDIR` set. The
   developer is very likely running their own forestui in their own tmux right now.
   All tmux interaction goes through `tu press` / `tu screenshot`.
2. **Always isolate the tmux server** with `--env TMUX_TMPDIR=$FUI_TMUX`.
3. **Always isolate `$HOME`** with `--env HOME=$FUI_HOME`. forestui writes
   `~/.config/forestui/settings.json` and reads `~/.claude/projects` — without this
   the suite clobbers the developer's real settings and reads their real Claude
   sessions (making "RECENT SESSIONS" non-deterministic).
4. **Never point the suite at the real `~/forest`.** The fixture builds its own.
5. **Always `env -u TMUX`** in front of the launch command, so the app takes the
   cold-start path instead of thinking it is already inside tmux.
6. **Screenshot as PNG** (`tu screenshot --png -o …` then `Read` the file) whenever
   colour or highlight state matters — active tmux window, toast severity, checkbox
   on/off, button variant. Text screenshots silently lose all of it. Textual renders
   an unchecked checkbox as `▐X▌` in *text* and only the colour distinguishes it
   from checked.
7. **Clean up every session and temp dir** (see Teardown), even on failure.

### The one harness gotcha that will bite the Rust build

`forestui/cli.py:ensure_tmux()` re-execs itself through tmux with a **hardcoded
command name**:

```python
os.execvp("tmux", ["tmux", "new-session", "-s", session_name, forestui_cmd])
#                                                             ^ literally "forestui …"
```

So the process you launch under `tu` is *not* the process that renders the UI —
tmux starts a **second** process by running the literal string `forestui` through a
shell, inheriting `PATH` from the tmux server env. Consequences:

- `uv run forestui` works because `uv run` prepends `.venv/bin` to `PATH`.
- `cargo run -- …` will **not** work: the re-exec would try to run a binary called
  `forestui`, not `cargo`.
- Therefore for the Rust build, `$FUI_CMD` must be a **built binary named
  `forestui` that is on `PATH`**, e.g. `cargo build --release && export
  PATH=$PWD/target/release:$PATH`.

The Rust rewrite must keep this contract (respawn a command named `forestui`, with
the same argv reconstruction) or UC-04 / UC-05 will fail.

---

## 1. Parameterised launch command

Put this at the top of any script that runs the playbook. Everything below uses
`$FUI_CMD` and never hardcodes a build.

```bash
# ---- Python build (reference) --------------------------------------------
export FUI_ROOT=/Users/kirill/work/repos/forestui
export FUI_CMD="uv run forestui"

# ---- Rust build ----------------------------------------------------------
# export FUI_ROOT=/Users/kirill/work/repos/forestui
# cargo build --release --manifest-path "$FUI_ROOT/Cargo.toml"
# export PATH="$FUI_ROOT/target/release:$PATH"   # REQUIRED: see gotcha above
# export FUI_CMD="forestui"
```

Canonical launch line (every use case is a variation of this):

```bash
tu run --name fui \
  --env TMUX_TMPDIR=$FUI_TMUX \
  --env HOME=$FUI_HOME \
  --env UV_CACHE_DIR=$HOME/.cache/uv \
  --cwd "$FUI_ROOT" -- env -u TMUX $FUI_CMD "$FUI_FOREST"
```

`UV_CACHE_DIR` is only needed for the Python build — it points the isolated-`HOME`
uv at the real cache so the venv is not rebuilt on every run. Harmless for Rust.

---

## 2. Fixture setup

Deterministic test forest: **2 repos** (`alpha` clean with an extra branch and one
pre-existing worktree, `beta` with a dirty tree), an empty forest dir, and an
isolated `HOME`. Neither repo has a remote — that is intentional and pins the
`⟳ Git Pull (No remote)` assertions.

```bash
fui_fixture() {
  export FUI_FIX=$(mktemp -d /tmp/fui-fix.XXXXXX)
  export FUI_TMUX="$FUI_FIX/tmux"
  export FUI_HOME="$FUI_FIX/home"
  export FUI_FOREST="$FUI_FIX/forest"
  export FUI_SRC="$FUI_FIX/src"
  mkdir -p "$FUI_TMUX" "$FUI_HOME" "$FUI_FOREST" "$FUI_SRC"

  # keep the developer's git identity/hooks out of the fixture
  export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
  gc() { git -c user.email=t@t -c user.name=T "$@"; }

  # repo "alpha": clean, branches main/develop/feature/login, one worktree on disk
  git init -q -b main "$FUI_SRC/alpha"
  gc -C "$FUI_SRC/alpha" commit -q --allow-empty -m "alpha initial"
  git -C "$FUI_SRC/alpha" branch develop
  git -C "$FUI_SRC/alpha" branch feature/login
  git -C "$FUI_SRC/alpha" worktree add -q "$FUI_FOREST/alpha/wt-one" -b feat/wt-one

  # repo "beta": clean history, dirty working tree
  git init -q -b main "$FUI_SRC/beta"
  gc -C "$FUI_SRC/beta" commit -q --allow-empty -m "beta initial"
  echo dirty > "$FUI_SRC/beta/UNTRACKED.txt"

  echo "FUI_FIX=$FUI_FIX"
}
```

**Notes that matter for assertions**

- `alpha`'s pre-existing worktree lives *inside* the forest dir. "Import existing
  worktrees" deliberately **skips** anything already under the forest dir
  (`app.py:_import_existing_worktrees`), so it will not appear — that is correct,
  not a bug.
- Isolated `HOME` means `gh` is unauthenticated, so the sidebar always shows
  `gh cli: unauth'd` and the GitHub-issues section is always empty. Deterministic.
- Isolated `HOME` means settings are always defaults on first run:
  editor `vim`, branch prefix `feat/`, theme `system`, no custom buttons.

## 3. Teardown

```bash
fui_teardown() {
  for s in fui fui-a fui-b fui-c; do tu kill --name "$s" 2>/dev/null; done
  tu list                       # MUST print []
  rm -rf "$FUI_FIX"
  # sanity: the developer's real config must be untouched
  ls ~/.config/forestui/settings.json
}
```

Run `tu list` and confirm `[]` before declaring a run finished. A leaked `tu`
session keeps an isolated tmux server alive and will poison the next run's
grouped-session tests.

---

# Use cases

## Area A — Startup & tmux entry

### UC-01 — Cold start creates its own tmux session and dev-mode window
**Area:** Startup | **Priority:** P0 | 🟢 EXECUTED
**Setup:**
```bash
fui_fixture
```
**Steps:**
1. `tu run --name fui --env TMUX_TMPDIR=$FUI_TMUX --env HOME=$FUI_HOME --env UV_CACHE_DIR=$HOME/.cache/uv --cwd "$FUI_ROOT" -- env -u TMUX $FUI_CMD "$FUI_FOREST"`
2. `tu wait --name fui --text "forestui" --timeout 20000`
3. `tu screenshot --name fui --png -o /tmp/uc01.png` and `Read` it.

**Expected:**
- Header line centred: `forestui v0.0.0` (version `0.0.0` when built from source).
- Status bar bottom-left: `[forestui-` … `0:forestui-dev-HHMM*` — window index `0`,
  window name `forestui-dev-<HHMM>` (dev mode auto-enables at version `0.0.0`),
  and `*` marking it active.
- Footer key row, left to right, exactly:
  `a Add Repo  q Quit  w Add Worktree  e Editor  t Terminal  o Files  n Claude  y ClaudeYOLO  h Archive  d Del…`
- Sidebar top box: `gh cli: unauth'd`.

**Fails if:** the app runs but never re-execs into tmux (no status bar); the window
is named `forestui` instead of `forestui-dev-HHMM` when version is `0.0.0`; the
session name is not `forestui-<slugified-forest-dirname>`.

> The tmux status-left default `status-left-length` is **10**, so the session name is
> visually truncated to `[forestui-`. Assert on the **window list**, never on the
> full session name in the status bar.

---

### UC-02 — Release-mode window name has no `-dev-` suffix
**Area:** Startup | **Priority:** P1 | ⚪ SOURCE
**Setup:** an installed build reporting a real version (not `0.0.0`).
**Steps:**
1. Launch as UC-01 but with the installed binary.
2. `tu screenshot --name fui`

**Expected:** status bar shows `0:forestui*`, header shows `forestui vX.Y.Z`.
**Fails if:** dev-mode naming leaks into release builds — that would make two
concurrently-running forestuis fight over the same window name.

---

### UC-03 — `--dev` forces dev window naming on a release build
**Area:** Startup | **Priority:** P2 | ⚪ SOURCE
**Setup:** installed build.
**Steps:**
1. Launch with `… -- env -u TMUX $FUI_CMD --dev "$FUI_FOREST"`
2. `tu screenshot --name fui`

**Expected:** window name matches `forestui-dev-\d{4}`.
**Fails if:** `--dev` is dropped when reconstructing the inner command line
(`cli.py` re-appends `--dev`, `--debug`, `--no-self-update` and the forest path).

---

### UC-04 — Second terminal joins the existing session as a grouped session
**Area:** Startup / tmux | **Priority:** P0 | 🟢 EXECUTED
**Setup:** `fui_fixture`; UC-01 already running as `fui`.
**Steps:**
1. `tu run --name fui-b --env TMUX_TMPDIR=$FUI_TMUX --env HOME=$FUI_HOME --env UV_CACHE_DIR=$HOME/.cache/uv --cwd "$FUI_ROOT" -- env -u TMUX $FUI_CMD "$FUI_FOREST"`
2. `sleep 7`
3. `tu screenshot --name fui-b` and `tu screenshot --name fui`

**Expected:**
- `fui-b` shows the **same UI** as `fui` — it attached to the existing
  `forestui-dev-HHMM` window rather than starting a second app instance.
- Both status bars show the **same** window list and the **same** session label
  (`[forestui-…`), even though `fui-b` is internally attached to a PID-suffixed
  grouped session — `cli.py` rewrites `status-left` replacing `#S` with the base
  session name.

**Fails if:** a second forestui process starts (two apps writing
`.forestui-config.json`); the second terminal's status bar shows
`forestui-forest-12345` (PID leak); the second terminal is refused entry.

---

### UC-05 — Session exists but the forestui window was killed → new window
**Area:** Startup | **Priority:** P1 | ⚪ SOURCE
**Setup:** `fui_fixture`; launch via a **login shell** so a prompt survives:
```bash
tu run --name fui --env TMUX_TMPDIR=$FUI_TMUX --env HOME=$FUI_HOME \
  --cwd "$FUI_ROOT" -- env -u TMUX bash -l
tu wait --name fui --text "\\$" --timeout 5000
tu type --name fui "$FUI_CMD $FUI_FOREST"; tu press --name fui Enter
```
**Steps:**
1. Create a second window: `tu press --name fui t` (so the session survives).
2. `tu press --name fui Ctrl+B 0` then `tu press --name fui q` to kill the forestui window.
3. Re-run: `tu type --name fui "$FUI_CMD $FUI_FOREST"; tu press --name fui Enter`
4. `tu screenshot --name fui`

**Expected:** the existing session is reused (no new session name) and a **new**
`forestui-dev-HHMM` window is created in it. The old `term:…` window is still in
the list.
**Fails if:** a duplicate session `forestui-<slug>` is created, or the app attaches
to a session with no forestui window and shows a bare shell.

---

### UC-06 — Missing forest directory is created silently, no config written
**Area:** Startup | **Priority:** P0 | 🟢 EXECUTED
**Setup:**
```bash
fui_fixture
```
**Steps:**
1. `tu run --name fui-c --env TMUX_TMPDIR=$FUI_FIX/tmux2 --env HOME=$FUI_HOME --env UV_CACHE_DIR=$HOME/.cache/uv --cwd "$FUI_ROOT" -- env -u TMUX $FUI_CMD "$FUI_FIX/does-not-exist-yet"` (create `$FUI_FIX/tmux2` first)
2. `sleep 7`; `tu screenshot --name fui-c`
3. `ls -la "$FUI_FIX/does-not-exist-yet"`

**Expected:**
- App starts normally; header `forestui v0.0.0`; sidebar `gh cli: unauth'd`; tree empty.
- The directory now **exists** and is **empty** — `.forestui-config.json` is written
  lazily, only on the first state mutation.
- Session name derives from the new dir: `forestui-does-not-exist-yet`.

**Fails if:** the app errors out on a missing forest dir; or it eagerly writes an
empty `.forestui-config.json` (Python does not).

---

### UC-07 — Empty forest renders a blank detail pane (current Python behaviour)
**Area:** Startup | **Priority:** P1 | 🟢 EXECUTED
**Setup:** `fui_fixture`, launch as UC-01.
**Steps:**
1. `tu screenshot --name fui --png -o /tmp/uc07.png`; `Read` it.

**Expected (as-is parity):** the detail pane to the right of the sidebar border is
**completely blank**. The strings the source intends to show —
`" forestui"`, `"Git Worktree Manager"`, `"Select a repository or press [a] to add a
repository"` — are **not rendered**.

**Fails if:** you assume the empty state is visible and assert on its text.

> ⚠️ **Known Python bug, decide before porting.** `EmptyState` is a plain `Widget`
> whose `.empty-state` container is `height: 100%` inside an auto-height parent, so
> it collapses to zero rows (`app.py:52`, `theme.py:351`). The Rust build should
> almost certainly *fix* this and show the empty state. If it does, mark this UC as
> an **intentional divergence** and rewrite the Expected block to the three lines
> above — do not let it pass silently as "blank in both".

---

## Area B — Sidebar navigation & selection

### UC-08 — Adding the first repository auto-selects it
**Area:** Sidebar | **Priority:** P0 | 🟢 EXECUTED
**Setup:** `fui_fixture`, launch as UC-01.
**Steps:**
1. `tu press --name fui a`
2. `tu type --name fui "$FUI_SRC/alpha"`
3. `tu press --name fui Enter`
4. `sleep 2`; `tu screenshot --name fui`

**Expected:**
- Sidebar line 1: ` ▼  alpha` (expanded caret, leading space, no children yet).
- Detail pane header: `MAIN REPOSITORY`, then `Repository: alpha`.

**Fails if:** the repo is added but not selected (detail pane stays blank), or the
pre-existing `wt-one` worktree is auto-imported (it must not be — it is inside the
forest dir).

---

### UC-09 — `Up`/`Down` walk the tree and swap the detail pane
**Area:** Sidebar | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-08 plus a created worktree (UC-14) plus repo `beta` added (UC-10).
Tree order is `alpha`, `wt-two`, `beta`.
**Steps:**
1. `tu press --name fui Up`; `sleep 1.2`; `tu screenshot --name fui`
2. `tu press --name fui Down`; `sleep 1.2`; `tu screenshot --name fui`
3. `tu press --name fui Down`; `sleep 1.2`; `tu screenshot --name fui`

**Expected:**
- From `beta`: `Up` → detail header becomes `WORKTREE` / `Repository: alpha` / `Worktree:   wt-two`.
- `Down` → detail header becomes `MAIN REPOSITORY` / `Repository: beta`.
- `Down` at the last node → **no change** (stays on `beta`); the cursor does not wrap.

**Fails if:** highlight movement does not update the detail pane (Python fires the
detail refresh from `Tree.NodeHighlighted`, i.e. on *highlight*, not on Enter), or
the cursor wraps around at the ends.

> Observed quirk: immediately after a modal closes, the **first** arrow press can be
> absorbed and the detail pane not change. Re-press once before declaring a failure;
> if the Rust build never absorbs it, that is an improvement, not a regression.

---

### UC-10 — Second repository appends below the first
**Area:** Sidebar | **Priority:** P1 | 🟢 EXECUTED
**Setup:** UC-08 done.
**Steps:**
1. `tu press --name fui a`; `tu type --name fui "$FUI_SRC/beta"`; `tu press --name fui Enter`
2. `sleep 2.5`; `tu screenshot --name fui`

**Expected:** sidebar reads
```
 ▼  alpha
 └ └─  wt-two
 ▼  beta
```
and the detail pane shows `Repository: beta`. Repos are shown in **insertion
order**, never sorted.
**Fails if:** repos are alphabetised or the new repo is inserted at the top.

---

### UC-11 — Worktree rows do NOT display the branch (current Python behaviour)
**Area:** Sidebar | **Priority:** P1 | 🟢 EXECUTED
**Setup:** UC-14 done (worktree `wt-two` on branch `feat/wt-two`).
**Steps:**
1. `tu screenshot --name fui --png -o /tmp/uc11.png`; `Read` it.

**Expected (as-is parity):** the row renders as `wt-two` only. The `[feat/wt-two]`
suffix the source builds (`sidebar.py:146`,
`f"{prefix}  {worktree.name} [{worktree.branch}]"`) is **not visible** — Textual
parses `[...]` as console markup and swallows it.
**Fails if:** you assert `wt-two [feat/wt-two]` against Python.

> ⚠️ **Known Python bug.** The Rust build has no markup parser and will naturally
> render `wt-two [feat/wt-two]`. That is the *intended* design. Treat a Rust build
> showing the branch as an **intentional divergence** and update this Expected
> block; treat a Rust build hiding the branch as a bug.

---

## Area C — Repository detail pane

### UC-12 — Repository detail: full section layout
**Area:** Repo detail | **Priority:** P0 | 🟢 EXECUTED (partial — sections above
`RECENT SESSIONS` verified on screen; `MY OPEN GITHUB ISSUES` and `MANAGE` read from
source, they sit below the fold at 120x40)
**Setup:** UC-08 done, `alpha` selected, terminal `120x70`
(`tu resize --name fui 120x70`).
**Steps:**
1. `tu screenshot --name fui`

**Expected**, top to bottom, exact strings:
```
MAIN REPOSITORY
Repository: alpha
Branch:     main
Commit:     <7-hex> (<humanized time>)
[ ⟳ Git Pull (No remote) ]  [  Add Worktree ]
LOCATION
<absolute source path>
OPEN IN
[  Editor ] [  Terminal ] [  Files ]
CLAUDE
[ New Session ] [ New Session: YOLO ]
RECENT SESSIONS
No sessions found
MY OPEN GITHUB ISSUES            [↻]
No issues found
MANAGE
[  Remove Repository ]
```
The label columns are padded so `Repository:`, `Branch:` and `Commit:` values all
start at the same column (`Branch:` + 5 spaces).
**Fails if:** a section is missing or reordered; label padding differs; `Git Pull` is
enabled for a repo with no remote.

---

### UC-13 — `Git Pull` is disabled and re-labelled when the branch has no upstream
**Area:** Repo detail | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-08 (fixture repos have no remote).
**Steps:**
1. `tu screenshot --name fui --png -o /tmp/uc13.png`; `Read` it.

**Expected:** the button reads exactly `⟳ Git Pull (No remote)` and is rendered
disabled (dimmed, not focusable).
**Fails if:** the button says `⟳ Git Pull` and is clickable — pulling a repo with no
upstream errors out and the app would surface a raw git error.

---

### UC-14 — Dirty working tree is not surfaced anywhere
**Area:** Repo detail | **Priority:** P2 | 🟢 EXECUTED
**Setup:** `beta` (has `UNTRACKED.txt`) selected.
**Steps:**
1. `tu screenshot --name fui`

**Expected:** the detail pane is **identical in shape** to a clean repo — no dirty
marker, no `*`, no file count. forestui never runs `git status`.
**Fails if:** the Rust build adds a dirty indicator without this UC being updated —
that is a feature, and it changes the parity baseline.

---

### UC-15 — GitHub issues section shows the spinner then settles
**Area:** Repo detail | **Priority:** P2 | ⚪ SOURCE
**Setup:** UC-08; `gh` unauthenticated under the isolated `HOME`.
**Steps:**
1. Select a repository.
2. Immediately `tu screenshot --name fui` (within ~1s).
3. `sleep 3`; `tu screenshot --name fui`

**Expected:** the refresh button next to `MY OPEN GITHUB ISSUES` cycles through
`|`, `/`, `-`, `\` while loading (50ms tick) and returns to `↻` when the fetch
resolves. With `gh` unauthenticated the list settles on `No issues found` and
**never blocks the UI** — the pane is fully interactive during the fetch.
**Fails if:** the app freezes while `gh` runs, or the spinner never stops.

---

## Area D — Worktree detail pane

### UC-16 — Worktree detail: full section layout
**Area:** Worktree detail | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-08, then create `wt-two` via UC-24. Terminal `120x70`.
**Steps:**
1. `tu screenshot --name fui`

**Expected**, exact strings:
```
WORKTREE
Repository: alpha
Worktree:   wt-two
Branch:     feat/wt-two
Based on:   main (<7-hex>)
Commit:     <7-hex> (<humanized time>)
[ ⟳ Git Pull (No remote) ]
LOCATION
<forest>/alpha/wt-two
OPEN IN
[  Editor ] [  Terminal ] [  Files ]
CLAUDE
[ New Session ] [ New Session: YOLO ]
RECENT SESSIONS
No sessions found
RENAME
[ wt-two        ]   <- input pre-filled with the worktree name
[ feat/wt-two   ]   <- input pre-filled with the branch name
MANAGE
[  Archive ] [  Delete ]
```
**Fails if:** `Based on:` is missing (it comes from `base_branch` +
`created_from_ref` persisted at creation time, not from git); the rename inputs are
empty instead of pre-filled.

> The `LOCATION` path is the **resolved** path — on macOS `/tmp/...` resolves to
> `/private/tmp/...`. Assert with a suffix match, not an equality against `$FUI_FOREST`.

---

### UC-17 — Archived worktree swaps Archive → Unarchive
**Area:** Worktree detail | **Priority:** P1 | 🟢 EXECUTED
**Setup:** UC-16, worktree selected, terminal `120x70`.
**Steps:**
1. `tu press --name fui h`; `sleep 1.5`
2. `tu screenshot --name fui`

**Expected:** the `MANAGE` row becomes `[  Unarchive ] [  Delete ]`. The detail pane
still shows the worktree (it stays selected) even though it has left the tree.
**Fails if:** archiving clears the detail pane, or the button label does not flip.

---

### UC-18 — Rename worktree via the RENAME input
**Area:** Worktree detail | **Priority:** P1 | ⚪ SOURCE
**Setup:** UC-16.
**Steps:**
1. `tu mouse click --name fui --on-text "wt-two"` (the first RENAME input)
2. Select-all/clear, `tu type --name fui "wt-renamed"`, `tu press --name fui Enter`
3. `sleep 2`; `tu screenshot --name fui`; then
   `ls "$FUI_FOREST/alpha"` and `git -C "$FUI_SRC/alpha" worktree list`

**Expected:** the directory on disk is renamed, `git worktree list` points at the new
path (the app runs `git worktree repair`), the sidebar and detail show `wt-renamed`,
and `.forestui-config.json` has the new `name` **and** `path`.
**Fails if:** only the label changes and the directory/git metadata are left behind —
that produces a stale worktree on the next launch (see UC-34).

---

### UC-19 — Rename branch via the second RENAME input
**Area:** Worktree detail | **Priority:** P2 | ⚪ SOURCE
**Setup:** UC-16.
**Steps:**
1. Click the branch input, clear it, type `feat/renamed`, press `Enter`.
2. `sleep 2`; `git -C "$FUI_FOREST/alpha/wt-two" branch --show-current`

**Expected:** git reports `feat/renamed`; `Branch:` in the detail pane updates;
`.forestui-config.json` `branch` field updates.
**Fails if:** the rename is only cosmetic, or a git failure is swallowed instead of
raising the toast `Branch rename failed: …`.

---

## Area E — Hotkeys

All hotkeys are **global bindings** on the app (`app.py:BINDINGS`), not tree
bindings, so they fire regardless of which pane has focus. Full set from source:
`q` quit (priority), `a` add repo, `w` add worktree, `e` editor, `t` terminal,
`o` files, `n` claude, `y` claude YOLO, `h` toggle archive, `d` delete,
`s` settings, `r` refresh (hidden from footer), `?` help.

### UC-20 — `t` opens a shell window named `term:<repo>:<worktree>`
**Area:** Hotkeys | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-16, worktree `wt-two` selected.
**Steps:**
1. `tu press --name fui t`; `sleep 2`
2. `tu screenshot --name fui`

**Expected:** status bar shows `1:term:alpha:wt-two*` (active), and the pane shows a
shell prompt whose cwd basename is `wt-two`. Naming rule:
`term:<repo>:<worktree>` for a worktree, `term:<repo>` for a repository.
**Fails if:** the window is named after the path, or the new window is created but
not selected.

---

### UC-21 — `n` and `y` open `claude:` and `yolo:` windows
**Area:** Hotkeys | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-16.
**Steps:**
1. `tu press --name fui Ctrl+B 0`; `sleep 1`; `tu press --name fui n`; `sleep 3`; `tu screenshot --name fui`
2. `tu press --name fui Ctrl+B 0`; `sleep 1`; `tu press --name fui y`; `sleep 3`; `tu screenshot --name fui`

**Expected:** windows `2:claude:alpha:wt-two*` and `3:yolo:alpha:wt-two*`. The
command is run through an **interactive login shell** (`$SHELL -ic 'claude'`) so
shell aliases resolve; the YOLO variant appends `--dangerously-skip-permissions`.
**Fails if:** the prefixes are wrong; the YOLO flag is added to non-YOLO windows or
missing from YOLO ones; the command bypasses the interactive shell (aliases break).

---

### UC-22 — `e` opens a TUI editor in `edit:<repo>:<worktree>`
**Area:** Hotkeys | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-16; default editor is `vim` under the isolated `HOME`.
**Steps:**
1. `tu press --name fui Ctrl+B 0`; `sleep 1`; `tu press --name fui e`; `sleep 3`
2. `tu screenshot --name fui`

**Expected:** window `4:edit:alpha:wt-two*`; the pane shows vim's netrw directory
listing (`" Netrw Directory Listing`) because the app runs `vim .` in the worktree.
**Fails if:** a GUI editor path is taken for a TUI editor. The TUI set is exactly:
`vim, nvim, vi, emacs, nano, helix, hx, micro, kakoune, kak` (matched on the first
word, so `emacs -nw` counts). Anything else is `subprocess.Popen`'d detached with
the toast `Opened in <cmd>` and **no** tmux window.

---

### UC-23 — `o` opens Midnight Commander in `files:<repo>:<worktree>`
**Area:** Hotkeys | **Priority:** P1 | 🟢 EXECUTED
**Setup:** UC-16; `mc` installed.
**Steps:**
1. `tu press --name fui Ctrl+B 0`; `sleep 1`; `tu press --name fui o`; `sleep 3`
2. `tu screenshot --name fui`

**Expected:** window `5:files:alpha:wt-two*`; the pane's first line is mc's menu bar
`  Left     File     Command     Options     Right`.
**Fails if:** the window is created with a shell instead of `mc`, or `mc` is launched
in the wrong directory.

---

### UC-24 — Repeated window creation adds `:2`, `:3`, `:4` suffixes
**Area:** Hotkeys / tmux | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-20 already created `term:alpha:wt-two`.
**Steps:**
1. `tu press --name fui Ctrl+B 0`; `sleep 1`; `tu press --name fui t`; `sleep 2`; `tu screenshot --name fui`
2. Repeat once more.

**Expected:** `6:term:alpha:wt-two:2`, then `7:term:alpha:wt-two:3`. Counter starts at
`2` and skips names already taken.
**Fails if:** the second press reuses/selects the existing window (only `edit:` does
that — see UC-25), or names collide.

---

### UC-25 — `e` twice reuses the existing edit window
**Area:** Hotkeys / tmux | **Priority:** P1 | ⚪ SOURCE
**Setup:** UC-22 done.
**Steps:**
1. `tu press --name fui Ctrl+B 0`; `sleep 1`; `tu press --name fui e`; `sleep 2`
2. `tu screenshot --name fui`

**Expected:** **no** new window is created; the existing `edit:alpha:wt-two` is
selected. Asymmetry is intentional: `create_editor_window` looks up an existing
window first, while `term:` / `files:` / `claude:` / `yolo:` always create a fresh
one via `_find_unique_window_name`.
**Fails if:** `edit:alpha:wt-two:2` appears.

---

### UC-26 — `?` raises the help toast
**Area:** Hotkeys | **Priority:** P2 | 🟢 EXECUTED
**Setup:** any selection.
**Steps:**
1. `tu press --name fui "?"`; `sleep 1`; `tu screenshot --name fui`

**Expected:** an information toast containing exactly:
`a: Add Repo | w: Add Worktree | e: Editor | t: Terminal | n: Claude | h: Archive | d: Delete | s: Settings | q: Quit`
**Fails if:** the text differs. Note it deliberately (or accidentally) omits `o`, `y`,
`r` and `?` — port the string verbatim, or fix it in both the toast and this UC.

---

### UC-27 — `q` quits the app and leaves the tmux session alive
**Area:** Hotkeys | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-20 done, so at least one other tmux window exists.
**Steps:**
1. `tu press --name fui Ctrl+B 0`; `sleep 1.5`; `tu press --name fui q`; `sleep 3`
2. `tu screenshot --name fui`; `tu status --name fui`

**Expected:** the `forestui-dev-HHMM` window disappears from the window list; the
client falls through to a surviving window; `tu status` reports `"alive":true`
(tmux itself has not exited).
**Fails if:** quitting kills the whole tmux server and takes the user's `term:` /
`claude:` windows with it.

---

### UC-28 — Hotkeys are no-ops when nothing is selected
**Area:** Hotkeys | **Priority:** P1 | 🟢 EXECUTED
**Setup:** empty forest (UC-06), no repositories.
**Steps:**
1. `tu press --name fui-c t`; `sleep 2`; `tu screenshot --name fui-c`
2. `tu press --name fui-c w`; `sleep 1`; `tu screenshot --name fui-c --png -o /tmp/uc28.png`; `Read` it.

**Expected:**
- `t` (also `e`, `o`, `n`, `y`): **nothing happens** — no new tmux window, no toast.
- `w`: a **warning** toast reading `Select a repository first`, rendered with an
  orange/amber left bar (visible only in PNG).

**Fails if:** a window named `term:session` is created (the `_get_tmux_window_name`
fallback string is `session` — it must never be reachable), or `w` opens an empty
modal.

---

## Area F — Modals

### UC-29 — Add Repository modal: layout and live path validation
**Area:** Modals | **Priority:** P0 | 🟢 EXECUTED (partial — layout and the
`Repository: alpha` happy path driven live; the four error strings in the table are
from source)
**Setup:** `fui_fixture`, launched.
**Steps:**
1. `tu press --name fui a`; `sleep 1`; `tu screenshot --name fui`
2. `tu type --name fui "$FUI_SRC/alpha"`; `sleep 1`; `tu screenshot --name fui`

**Expected:**
- Modal title ` Add Repository`; section header `Repository Path`; an input with
  placeholder `Enter path or paste from clipboard...`; a checkbox
  `Import existing worktrees`; buttons `Cancel` / `Add Repository`.
- The app footer is **hidden** while a modal is up.
- After typing a valid repo path, a status line appears: `Repository: alpha`.

Validation strings, all rendered in the destructive colour:
| Input | Status line |
|---|---|
| empty | *(blank)* |
| non-existent path | `Path does not exist` |
| a file, not a dir | `Path is not a directory` |
| dir without `.git` | `Not a git repository` |
| valid repo | `Repository: <basename>` |

**Fails if:** validation is deferred to submit; or the modal accepts a non-repo
(the Add button is a hard no-op unless `<path>/.git` exists).

---

### UC-30 — Add Repository: `Enter` in the path input submits
**Area:** Modals | **Priority:** P1 | 🟢 EXECUTED
**Setup:** UC-29 step 2.
**Steps:**
1. `tu press --name fui Enter`; `sleep 2`; `tu screenshot --name fui`

**Expected:** modal closes, repo appears in the sidebar, detail pane shows
`MAIN REPOSITORY` / `Repository: alpha`.
**Fails if:** `Enter` inserts a newline or does nothing (the Add Worktree modal is
deliberately different — see UC-32).

---

### UC-31 — Add Worktree modal: name sanitising, path preview, auto branch
**Area:** Modals | **Priority:** P0 | 🟢 EXECUTED (partial — layout, preview and
auto-branch driven live; the `myfeature` sanitising example is from source)
**Setup:** UC-08, `alpha` selected.
**Steps:**
1. `tu press --name fui w`; `sleep 2`; `tu screenshot --name fui`
2. `tu type --name fui "wt-two"`; `sleep 1`; `tu screenshot --name fui`

**Expected:**
- Title ` Add Worktree`, subtitle `to alpha`.
- Name input placeholder `my-feature`; branch input placeholder `feat/my-feature`
  (the `feat/` half is `settings.branch_prefix`).
- Mode buttons `[  New Branch ] [  Existing ]`, `New Branch` active by default and
  the fuzzy branch search hidden.
- After typing the name: a preview line ` <resolved-forest>/alpha/wt-two`, and the
  branch input auto-fills to `feat/wt-two`.
- Names are sanitised to `[A-Za-z0-9-_]` as you type — typing `my feat/ure!` yields
  `myfeature`.

**Fails if:** the preview shows an unresolved path where Python shows the resolved
one; the branch does not track the name while in New Branch mode; sanitising differs.

---

### UC-32 — Add Worktree: `Create Worktree` must be clicked, and validates
**Area:** Modals | **Priority:** P1 | 🟢 EXECUTED
**Setup:** UC-31 with the name left **empty**, `Existing` mode, branch
`feature/login` picked from the dropdown.
**Steps:**
1. `tu mouse click --name fui --on-text "Create Worktree"`; `sleep 1.5`
2. `tu screenshot --name fui`

**Expected:** the modal stays open and shows ` Worktree name is required` in the
destructive colour. Other validation strings, same slot:
`  Branch name is required`, `  Branch '<x>' does not exist`,
`  Worktree path already exists`.
Also: in `Existing` mode the `Create Worktree` button is **disabled** whenever the
typed text is not an exact member of the branch list.
**Fails if:** submitting with an empty name creates `<forest>/<repo>/` as a worktree,
or the button stays enabled for an unknown branch.

---

### UC-33 — Branch search: dropdown, count label, fuzzy filter
**Area:** Modals | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-31, then `tu mouse click --name fui --on-text "Existing"`.
**Steps:**
1. `sleep 1.5`; `tu screenshot --name fui`
2. `tu mouse click --name fui --on-text "Start typing to search branches"`
3. `tu type --name fui "log"`; `sleep 1.5`; `tu screenshot --name fui`

**Expected:**
- Unfiltered: count label `5 branches`, dropdown listing in sort order
  `develop`, `feat/wt-one`, `feat/wt-two`, `feature/login`, `main`.
  (`5` = 3 fixture branches + `feat/wt-one` from the fixture worktree + `feat/wt-two`
  if UC-31 already created it; adjust to the fixture state you are in.)
- After typing `log`: count label `1 match`, dropdown shows `feature/login` only,
  with `log` rendered `bold reverse` inside the branch name.
- Singular/plural: `1 match`, `3 matches`, `No matches`, `N branches`,
  `N of M branches` when the list is truncated at 50.

Ranking (from `utils.py:_match_score`, lower wins, ties broken
case-insensitively by name): exact `0.0`, exact-minus-remote-prefix `0.5`, prefix
`1.0`, local prefix `1.5`, substring at a `/-_.` boundary `2.0`, substring anywhere
`3.0`, Levenshtein on path segments `4.0+`.
**Fails if:** the ordering or the count-label wording differs — these strings are
user-visible and easy to get subtly wrong.

---

### UC-34 — Settings modal: defaults, Save, and the toast
**Area:** Modals | **Priority:** P0 | 🟢 EXECUTED
**Setup:** `fui_fixture` (fresh isolated `HOME`, so no settings file exists).
**Steps:**
1. `tu press --name fui s`; `sleep 1.5`; `tu screenshot --name fui`
2. `tu mouse click --name fui --on-text "Save"`; `sleep 2`
3. `tu screenshot --name fui`; `cat "$FUI_HOME/.config/forestui/settings.json"`

**Expected on screen:**
```
 Settings
DEFAULT EDITOR      [ Vim (tmux)  ▼ ]
BRANCH PREFIX       [ feat/ ]
THEME               [ System  ▼ ]
CUSTOM CLAUDE BUTTONS
No custom buttons configured
[ Manage Custom Buttons... ]
[ Cancel ]  [ Save ]
```
Editor dropdown options, in order: `VS Code, Cursor, Neovim (tmux), Vim (tmux),
Helix (tmux), Emacs TUI (tmux), PyCharm, Sublime Text, Nano (tmux), Micro (tmux)`.
Theme options: `System, Dark, Light`.

**Expected on save:** toast `Settings saved`, and the file contains **exactly**:
```json
{
  "default_editor": "vim",
  "default_terminal": "",
  "branch_prefix": "feat/",
  "theme": "system",
  "custom_buttons": []
}
```
**Fails if:** a key is dropped or renamed, `custom_buttons` is omitted, or the file
is written somewhere other than `$HOME/.config/forestui/settings.json`.
`default_terminal` is vestigial (never read) but **must still be serialised** or the
Python build's config round-trip breaks.

---

### UC-35 — Settings: `Cancel` / `Escape` discards
**Area:** Modals | **Priority:** P1 | ⚪ SOURCE
**Setup:** UC-34 with the branch prefix edited to `xyz/`.
**Steps:**
1. `tu press --name fui Escape`; `sleep 1`
2. `cat "$FUI_HOME/.config/forestui/settings.json"`

**Expected:** no `Settings saved` toast; the file is unchanged (or still absent).
`Escape` is bound to `action_cancel` on **every** modal
(`AddRepository`, `AddWorktree`, `Settings`, `ConfirmDelete`,
`CreateWorktreeFromIssue`, `CustomButtons`, `CustomButtonEdit`).
**Fails if:** `Escape` saves, or does not close the modal.

---

### UC-36 — Delete confirm modal
**Area:** Modals | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-16, worktree `wt-two` selected.
**Steps:**
1. `tu press --name fui d`; `sleep 1.5`; `tu screenshot --name fui`
2. `tu press --name fui Escape`; `sleep 1`; `tu screenshot --name fui`

**Expected:**
- Modal title ` Delete Worktree` (destructive colour), body
  `Permanently delete 'wt-two'?`, buttons `[ Cancel ] [ Delete ]`.
- `Escape` dismisses with **no** deletion — the worktree is still in the sidebar and
  still on disk.
- With a **repository** selected instead, the title is ` Remove Repository` and the
  body is `Remove '<name>' from forestui?`.
- Deleting from the worktree detail's `Delete` button uses a longer body:
  `Permanently delete worktree '<name>'?\nThis cannot be undone.`
  Removing from the repository detail's button uses:
  `Remove '<name>' from forestui?\n(Files will not be deleted)`.

**Fails if:** the confirm step is skipped, or a repository removal deletes files
(it must only drop the entry from `.forestui-config.json`).

---

### UC-37 — Custom Claude buttons round-trip into the detail pane
**Area:** Modals | **Priority:** P1 | ⚪ SOURCE
**Setup:** UC-34 open.
**Steps:**
1. Click `Manage Custom Buttons...` → `Add Button`.
2. Type label `YoloDisc`; confirm the prefix input auto-fills to `yolodisc`.
3. Type command `claude --dangerously-skip-permissions --model opus`; `Save`; `Save`; `Save`.
4. `sleep 2`; `tu screenshot --name fui`; `cat "$FUI_HOME/.config/forestui/settings.json"`
5. Select a worktree and press the new button; `tu screenshot --name fui`.

**Expected:**
- Prefix auto-derives from the label (lowercase, non `[a-z0-9_-]` runs → `-`,
  trimmed, max 20 chars) and **stops following** once you hand-edit it.
- The `CLAUDE` section gains a third button labelled `YoloDisc`, styled red because
  the command contains `--dangerously-skip-permissions`. Buttons wrap to a new row
  after every 4.
- `settings.json` gains
  `"custom_buttons": [{"label": "YoloDisc", "prefix": "yolodisc", "command": "…"}]`.
- Pressing it creates tmux window `yolodisc:alpha:wt-two` and runs the command
  **as-is** — no extra `--dangerously-skip-permissions` is appended.
- Duplicate label → ` Another button already uses this label`; duplicate prefix →
  ` Another button already uses this prefix`.

**Fails if:** custom buttons get the YOLO flag appended twice, or the prefix
validation (`^[a-z0-9_-]+$`, ≤20 chars) is relaxed — the prefix becomes a tmux
window name.

---

### UC-38 — Create worktree from a GitHub issue
**Area:** Modals | **Priority:** P2 | ⚪ SOURCE
**Setup:** requires a **real** authenticated `gh` and a GitHub repo with an open
issue assigned to or authored by the user. Cannot run under the isolated `HOME`;
run it as an opt-in tier with `--env HOME=$REAL_HOME` and a scratch clone.
**Steps:**
1. Select the repo, wait for `MY OPEN GITHUB ISSUES` to populate.
2. `tu mouse click --name fui --on-text "Create WT"`; `sleep 2`; `tu screenshot --name fui`

**Expected:** modal title `Create Worktree from Issue #<n>`; name pre-filled
`<n>-<slugified-title-truncated-to-40>`; branch pre-filled
`<branch_prefix><that name>`; base branch defaults to `<remote>/<current branch>` if
that ref exists, else the local current branch; `Pull repo before creating` checked;
a `Fetch` button that spins `| / - \` at 10Hz while `git fetch` runs and refreshes
the suggester afterwards.
**Fails if:** the branch slug differs (`[^a-z0-9]+` → `-`, first 40 chars, trimmed of
`-`), or the base-branch preference order is wrong.

---

## Area G — tmux windows & grouped sessions

### UC-39 — Grouped sessions navigate independently
**Area:** tmux | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-01 running as `fui`, UC-04 running as `fui-b`, plus at least one
`term:` window.
**Steps:**
1. `tu press --name fui-b Ctrl+B 1`; `sleep 2`
2. `tu screenshot --name fui-b`; `tu screenshot --name fui`

**Expected:** `fui-b`'s status bar marks `1:term:alpha:wt-two*` active while `fui`'s
still marks `0:forestui-dev-HHMM*`. Both list the **same** windows.
**Fails if:** switching windows in one terminal drags the other along — that is the
whole reason `cli.py` builds a grouped session instead of a plain attach.

---

### UC-40 — A window created in terminal A does not steal terminal B
**Area:** tmux | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-39 state (`fui` on window 0, `fui-b` on window 1).
**Steps:**
1. `tu press --name fui t`; `sleep 2.5`
2. `tu screenshot --name fui`; `tu screenshot --name fui-b`

**Expected:** `fui` jumps to the new `term:alpha:wt-two:N*` window; `fui-b` stays on
`1:term:alpha:wt-two*`. The new window appears in **both** window lists.
**Fails if:** B is yanked to the new window. The Python implementation picks the
target session by scanning `list-clients` for the **most recently active client in
the same session group** (`tmux.py:session`) — a naive "first attached session" or
"server default session" port will fail this exact test.

---

### UC-41 — Window naming uses the repo name when a repository is selected
**Area:** tmux | **Priority:** P1 | ⚪ SOURCE
**Setup:** UC-08, `alpha` (the repository row) selected.
**Steps:**
1. `tu press --name fui t`; `sleep 2`; `tu screenshot --name fui`

**Expected:** window `term:alpha` — **no** `:worktree` half. Same rule for
`edit:`/`files:`/`claude:`/`yolo:`.
**Fails if:** the repository case emits `term:alpha:alpha` or falls back to
`term:session`.

---

## Area H — Errors & edge cases

### UC-42 — Stale worktree: directory deleted underneath the app
**Area:** Edge cases | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-16 with `wt-two` created and selected.
```bash
rm -rf "$FUI_FOREST/alpha/wt-two"
```
**Steps:**
1. `tu press --name fui r` (refresh); `sleep 2`
2. `tu screenshot --name fui`

**Expected**, exact strings:
```
WORKTREE
Repository: alpha
Worktree:   wt-two
Branch:     feat/wt-two
⚠ MISSING:   directory no longer exists on disk
Based on:   main (<7-hex>)
[ ⟳ Git Pull (Directory missing) ]      <- disabled
LOCATION
<path>  (missing)                        <- destructive colour
```
The `Commit:` line is **absent** (git cannot run in a missing cwd; the error is
swallowed). The app must **not** crash — regression guard for commit `b8f2bc5`
("Fix crash when selecting a stale worktree").
**Fails if:** the app raises, or the missing directory is silently rendered as a
healthy worktree.

---

### UC-43 — Hotkeys on a stale worktree still create a window (in `$HOME`)
**Area:** Edge cases | **Priority:** P1 | 🟢 EXECUTED
**Setup:** UC-42 state.
**Steps:**
1. `tu press --name fui t`; `sleep 2.5`
2. `tu screenshot --name fui` (window list **and** the shell prompt)

**Expected (as-is parity):** a window `term:alpha:wt-two:N` **is** created, and the
shell lands in `$HOME` — tmux silently ignores a `start_directory` that does not
exist. No error toast.
**Fails if:** you expect `Failed to create terminal window`. It does not happen.

> This is arguably wrong (a window pretending to be the worktree but sitting in
> `$HOME`). If the Rust build refuses instead and toasts, record it as an
> intentional divergence and update this Expected block.

---

### UC-44 — Deleting a worktree removes it from git and from config
**Area:** Edge cases | **Priority:** P0 | ⚪ SOURCE
**Setup:** UC-16.
**Steps:**
1. `tu press --name fui d`; `sleep 1.5`
2. `tu mouse click --name fui --on-text "Delete"`; `sleep 2.5`
3. `git -C "$FUI_SRC/alpha" worktree list`; `ls "$FUI_FOREST/alpha"`;
   `cat "$FUI_FOREST/.forestui-config.json"`

**Expected:** `git worktree list` no longer lists it, the directory is gone, the
`worktrees` array in config is empty, the sidebar row is gone, and the detail pane
falls back to the parent repository. A `git worktree remove` failure is
**suppressed** — the entry is dropped from config regardless, so a stale entry can
never wedge the UI.
**Fails if:** a git failure aborts the config update (leaving a ghost row forever).

---

### UC-45 — `gh` missing entirely
**Area:** Edge cases | **Priority:** P1 | ⚪ SOURCE
**Setup:** launch with `--env PATH=/usr/bin:/bin` (no Homebrew, so no `gh`).
**Steps:**
1. `sleep 5`; `tu screenshot --name fui --png -o /tmp/uc45.png`; `Read` it.

**Expected:** sidebar shows `gh cli: missing`, styled with the **error** class (not
the warn class used for `unauth'd`). The issues section settles on
`No issues found`. The app is fully usable.
Status → display mapping: `authenticated` + user → `ok (<login>)`; `authenticated`
alone → `ok`; `not_authenticated` → `unauth'd`; `not_installed` → `missing`.
**Fails if:** a missing `gh` produces a traceback, a hang, or an error toast.

---

### UC-46 — Corrupt `.forestui-config.json` is ignored, not fatal
**Area:** Edge cases | **Priority:** P1 | ⚪ SOURCE
**Setup:**
```bash
fui_fixture
printf 'not json at all' > "$FUI_FOREST/.forestui-config.json"
```
**Steps:**
1. Launch as UC-01; `sleep 6`; `tu screenshot --name fui`
2. Add a repository (UC-08); `cat "$FUI_FOREST/.forestui-config.json"`

**Expected:** the app starts with an **empty** repository list (the parse error is
swallowed), and the first mutation **overwrites** the file with valid JSON.
Same tolerance for `settings.json`: unparseable → fall back to defaults.
**Fails if:** the app refuses to start or, worse, exits without the user seeing why.

---

### UC-47 — The app never blocks on slow external commands
**Area:** Edge cases | **Priority:** P1 | ⚪ SOURCE
**Setup:** put a `gh` shim early on `PATH` that `sleep 30`s.
**Steps:**
1. Launch; immediately `tu press --name fui Down`, `tu press --name fui Up`,
   `tu press --name fui s`, `tu press --name fui Escape` within the first 2s.
2. `tu screenshot --name fui` after each.

**Expected:** every keystroke is honoured within ~1s. The issues section shows
`Loading...` then the spinner; the rest of the UI is fully interactive throughout.
**Fails if:** the UI freezes — the CLAUDE.md contract is "render immediately,
dispatch work to the background, update reactively", and it must survive the port.

---

## Area I — Config persistence

### UC-48 — `.forestui-config.json` shape after adding a repo
**Area:** Persistence | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-08 done.
**Steps:**
1. `cat "$FUI_FOREST/.forestui-config.json"`

**Expected:** exactly this shape (2-space indent, `id` a UUIDv4 string):
```json
{
  "repositories": [
    {
      "id": "787c8913-4f59-474c-98b7-249c8c740821",
      "name": "alpha",
      "source_path": "/tmp/fui-fix.XXXXXX/src/alpha",
      "worktrees": []
    }
  ]
}
```
`source_path` is stored **as typed** (unresolved), unlike worktree paths.
**Fails if:** keys are renamed/reordered, `id` is an integer, or the file lands
outside the forest dir. Cross-build compatibility matters: a config written by the
Python build must load in the Rust build and vice versa.

---

### UC-49 — Worktree entry shape after create + archive
**Area:** Persistence | **Priority:** P0 | 🟢 EXECUTED
**Setup:** UC-31 create `wt-two`, then `tu press --name fui h`.
**Steps:**
1. `cat "$FUI_FOREST/.forestui-config.json"`

**Expected:** the `worktrees` array contains exactly these keys, in this order:
```json
{
  "id": "<uuid>",
  "name": "wt-two",
  "branch": "feat/wt-two",
  "path": "/private/tmp/fui-fix.XXXXXX/forest/alpha/wt-two",
  "is_archived": true,
  "sort_order": null,
  "last_modified": "2026-08-14T19:21:41.258677Z",
  "base_branch": "main",
  "created_from_ref": "2488dce"
}
```
`path` is **resolved** (`/private/tmp` on macOS), `last_modified` is UTC ISO-8601
with a `Z` suffix, `base_branch` / `created_from_ref` are captured at creation time
and never recomputed.
**Fails if:** `is_archived` does not flip to `true` on `h`; `sort_order` is `0`
instead of `null` for an unordered worktree; the timestamp loses its timezone.

---

### UC-50 — Archived worktrees vanish from the sidebar with no way back
**Area:** Persistence / Sidebar | **Priority:** P1 | 🟢 EXECUTED
**Setup:** UC-49 (`wt-two` archived).
**Steps:**
1. `tu screenshot --name fui`
2. Press every unbound letter you like; then `tu press --name fui h` again.

**Expected (as-is parity):** after archiving, the sidebar shows only ` ▼  alpha` —
the worktree row is gone. There is **no key and no button** that reveals archived
worktrees: `AppState._show_archived` is initialised `False` and nothing ever sets it
`True`. The only route back is `h` again **while the archived worktree is still
selected**; navigate away and it is unreachable until the config is hand-edited.
**Fails if:** you assume an ` Archived` section appears — the code that renders it
(`sidebar.py:150`) is dead in the current build.

> ⚠️ **Known Python dead-end.** If the Rust build wires up a show-archived toggle,
> record it as an intentional divergence and extend this UC to cover the ` Archived`
> collapsed group with rows formatted `   <name> (<repo>)`.

---

### UC-51 — State survives a restart
**Area:** Persistence | **Priority:** P0 | ⚪ SOURCE
**Setup:** UC-49 state, worktree unarchived.
**Steps:**
1. `tu press --name fui q`; `sleep 2`; `tu kill --name fui`
2. Relaunch exactly as UC-01.
3. `sleep 6`; `tu screenshot --name fui`

**Expected:** both repositories and the worktree are back, in the same order; the
**first** repository is auto-selected (selection itself is *not* persisted —
`Selection` lives only in memory); settings (editor/prefix/theme/custom buttons)
are restored from `$FUI_HOME/.config/forestui/settings.json`.
**Fails if:** the Rust build persists selection (a divergence, even if nicer) or
loses worktrees on reload.

---

### UC-52 — Two forests keep completely separate state
**Area:** Persistence | **Priority:** P1 | ⚪ SOURCE
**Setup:** `fui_fixture`; also `mkdir -p "$FUI_FIX/forest2"`.
**Steps:**
1. Launch against `$FUI_FOREST`, add `alpha`, quit.
2. Launch against `$FUI_FIX/forest2` (needs its own `TMUX_TMPDIR` or it will land in
   a differently-named session on the same server), add `beta`.
3. `cat "$FUI_FOREST/.forestui-config.json"` and `cat "$FUI_FIX/forest2/.forestui-config.json"`

**Expected:** each file lists only its own repository. The tmux session names differ
(`forestui-forest` vs `forestui-forest2`), so the two forests never share windows.
Global settings are shared (single `~/.config/forestui/settings.json`).
**Fails if:** state leaks between forests, or both forests collide on one tmux
session name.

---

## Area J — Automated parity sweep (both builds)

Areas A–I are hand-driven. This area is the **automated** sweep: one script
drives a build through every case, capturing a PNG and a normalised text frame
per case, so the same run against two builds can be diffed mechanically.

```bash
# capture a build
scripts/tu-sweep.sh rust   ./target/release/forestui
scripts/tu-sweep.sh python ~/.local/bin/forestui   # the uv-installed release

# compare two captures
scripts/tu-compare.sh   rust python   # text frames + tmux window lists
scripts/tu-composite.sh rust python   # side-by-side PNGs, one per case
```

Capture the Python side from the **installed release** (`uv tool install
forestui`), not a source checkout. That is the build users actually ran, and a
checkout reports a different version and window name without differing in any
way that matters.

**Screenshot protocol.** Each case writes two artifacts:

| Artifact | Path | Committed |
|---|---|---|
| Text frame | `doc/rust-rewrite/baseline/<build>/UC-NN-<slug>.txt` | yes — this is the diffable baseline |
| Screenshot | `doc/rust-rewrite/screenshots/<build>/UC-NN-<slug>.png` | no (gitignored) — for eyeballing colour and focus |
| Composite | `doc/rust-rewrite/screenshots/composite/UC-NN-<slug>.png` | no (gitignored) — the two builds' frames side by side |

A frame diff cannot see colour. A button that lost its accent, a focus ring that
stopped rendering and a selection highlight in the wrong shade all produce a
byte-identical text frame, so the composites are the only artifact that catches
them — read them after any change to `theme.rs` or a renderer.

Text frames are normalised before writing: the temp root, commit SHAs, relative
times, the dev-mode window timestamp and the tmux clock are masked, so two runs
of the same build produce identical frames and a diff only shows real change.
Screenshots are deliberately not committed — colour rendering is
terminal-dependent and would churn on every run.

**The harness waits on conditions, never on fixed sleeps.** Textual repaints
noticeably slower than ratatui; an earlier fixed-sleep version captured the two
builds at different points in the same interaction and produced three false
mismatches. Anything added here must use `await <regex>`.

**Note for the Python build:** its `ensure_tmux` re-executes the bare name
`forestui`, so the harness puts the command's own directory on `PATH`. Without
that the tmux session dies instantly. The Rust build resolves itself through
`current_exe()` and does not need it.

### UC-53 — Boot into the repository detail
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Setup:** harness fixture — repos `alpha` (one worktree `wt-a`) and `beta`, one
custom Claude button `Opus`.
**Expected:** `MAIN REPOSITORY`, `Repository: alpha`, `Branch:     main`,
`Commit:     <sha> (<rel>)`, `⟳ Git Pull (No remote)`.
**Fails if:** the commit line is missing (fixture has no commits) or the sync
control is enabled without a remote.

### UC-54 — Worktree detail
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** `WORKTREE`, `Worktree:   wt-a`, `Branch:     feat/wt-a`,
`Based on:   main (<sha>)`.
**Fails if:** `Based on:` is absent — the base branch was not captured at create time.

### UC-55 — `a` opens Add Repository
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** `Repository Path`, the placeholder
`Enter path or paste from clipboard...` **while the empty field is focused**,
`Import existing worktrees`, `Add Repository`, `Cancel`.
**Fails if:** the placeholder is hidden on focus. The Rust build did hide it; the
sweep caught it and it was fixed in `ui/widgets.rs`.

### UC-56 — `w` opens Add Worktree
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** `to alpha`, `Worktree Name`, `Branch`, `New Branch`, `Existing`,
branch placeholder `feat/my-feature`.

### UC-57 — `s` opens Settings
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** `DEFAULT EDITOR` = `Vim (tmux)`, `BRANCH PREFIX` = `feat/`,
`THEME` = `System`, `1 custom button configured`.
**Fails if:** the count does not reflect `custom_buttons` in the settings file.

### UC-58 — `d` opens the delete confirmation
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** `Delete Worktree` and `Permanently delete 'wt-a'?` — identical
wording in both builds. Nothing is deleted yet.

### UC-59 — `?` shows the help toast
**Area:** Sweep | **Priority:** P1 | 🟢 EXECUTED (both builds)
**Expected:** `a: Add Repo | w: Add Worktree | e: Editor | t: Terminal | n: Claude
| h: Archive | d: Delete | s: Settings | q: Quit`.
**Known gap in both:** the toast omits `o`, `y`, `r` and `?` itself.

### UC-60 — `e` opens the editor window
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** window `edit:alpha:wt-a` running the editor. Window lists match
byte-for-byte between builds.

### UC-61 — `t` opens a terminal window
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** window `term:alpha:wt-a`.

### UC-62 — `o` opens the file manager window
**Area:** Sweep | **Priority:** P1 | 🟢 EXECUTED (both builds)
**Expected:** window `files:alpha:wt-a` running `mc`.

### UC-63 — `n` opens a Claude window
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** window `claude:alpha:wt-a` running `claude`.

### UC-64 — `y` opens a YOLO Claude window
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** window `yolo:alpha:wt-a`, and the command actually carries
`--dangerously-skip-permissions`.
**Fails if:** the flag is missing — the whole point of the separate button.

### UC-65 — `e` again reuses the editor window
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** no new window; `edit:alpha:wt-a` is selected.

### UC-66 — `t` again opens a second, uniquified terminal
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** a new window `term:alpha:wt-a:2`.
**Fails if:** the second terminal reuses the first — terminals are always new.

### UC-67 — `A` toggles the archived section
**Area:** Sweep | **Priority:** P1 | 🟢 EXECUTED (both builds)
**Expected (Rust):** the ` Archived` group appears once something is archived.
**Expected (Python):** nothing happens — there is no `A` binding, which is why
archived worktrees are unreachable (UC-50). This is an intentional divergence.

### UC-68 — `h` archives the selected worktree
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** the worktree leaves the active list and MANAGE flips to `Unarchive`.

### UC-69 — `h` again unarchives it
**Area:** Sweep | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Expected:** the worktree returns to the active list, MANAGE shows `Archive`.

### UC-70 — `r` refreshes without visible change
**Area:** Sweep | **Priority:** P2 | 🟢 EXECUTED (both builds)
**Expected:** the frame is unchanged. A diff here means refresh has a side effect.

---

## Area K — Paths the sweep does not cover

Driven by hand on the Rust build, not yet scripted. Add them to `tu-sweep.sh`
when they stabilise.

### UC-71 — Existing-branch mode with fuzzy search
**Area:** Modals | **Priority:** P0 | 🟢 EXECUTED (Rust)
**Steps:** `w`, type a name, `Tab`, `Right` (→ `Existing`), `Tab`, type `rel`.
**Expected:** the count line reads `1 match`; `release-2` is listed with `rel`
highlighted; `Create Worktree` is **disabled** while the typed text is not a real
branch, and enables once a branch is picked from the list.
**Then:** `Tab`, `Enter` to pick, `Tab`, `Enter` to create → the worktree checks
out `release-2` (not a detached HEAD) and records
`base_branch=release-2`, `created_from_ref=<sha>`.

### UC-72 — Delete confirmation: `n` cancels, `y` deletes
**Area:** Modals | **Priority:** P0 | 🟢 EXECUTED (Rust)
**Expected:** after `d` then `n`, the worktree still exists on disk and in config.
After `d` then `y`, the directory is gone, the config entry is gone, and
`git worktree list` no longer lists it.

### UC-73 — Custom buttons: add, reorder, delete, save
**Area:** Modals | **Priority:** P1 | 🟢 EXECUTED (Rust)
**Steps:** `s` → `Tab`×3 → `Enter` (Manage) → `a` add a second button →
`Down`, `K`, `J`, `d`, `s` → save settings.
**Expected:** `K` moves the selection up, `J` down, `d` removes it, `s` saves the
list back into the parent Settings modal, and the saved
`~/.config/forestui/settings.json` reflects the final order and contents.

### UC-74 — Input editing keys
**Area:** Modals | **Priority:** P1 | 🟢 EXECUTED (Rust)
**Steps:** in any modal input — `Home`, `Delete`, `End`, `Backspace`, `Ctrl+U`.
**Expected:** `Home`+`Delete` removes the first character, `End`+`Backspace` the
last, `Ctrl+U` clears everything before the cursor.

### UC-75 — Typing in a rename field does not fire hotkeys
**Area:** Worktree detail | **Priority:** P0 | 🟢 EXECUTED (Rust)
**Expected:** with a rename field focused, `q` types `q` instead of quitting.
`Escape` restores the original value **and hands focus back to the sidebar**, so
the global hotkeys work again.
**Fails if:** `Escape` only resets the text — the hotkeys then stay unreachable.
That was a real bug this case caught.

### UC-76 — Sidebar navigation clamps at the ends
**Area:** Sidebar | **Priority:** P1 | 🟢 EXECUTED (Rust)
**Expected:** `Down` past the last row stays on the last row; the detail pane
follows the cursor with no `Enter` needed.

### UC-77 — Create worktree from a GitHub issue
**Area:** Modals | **Priority:** P1 | 🔴 NOT EXECUTED
**Blocked by:** needs an authenticated `gh` against a real repository with issues.
The isolated `HOME` the harness uses has no `gh` credentials by design. Covered
only by unit tests (`modal.rs::base_branch_default_prefers_remote`).

---

## Area L — Flows and mouse (added to the sweep)

`scripts/tu-sweep.sh` runs these after the single-key sweep. They also write
`doc/rust-rewrite/baseline/<build>/ASSERTIONS.txt`, because the interesting part
of a rename or an import is what it did to the worktree and the config, not only
what the screen said. Both builds write the same file, so outcomes diff as
easily as frames.

The two builds reach the same controls differently — the Rust detail pane is a
keyboard focus ring, the Textual one is a scrollable pane of mouse-first widgets
— so each flow branches on the build for the *interaction* while asserting the
same *outcome*.

### UC-78 — Rename a worktree end to end
**Area:** Worktree detail | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Steps:** select the worktree, reach the `Worktree name` field, append `-renamed`, `Enter`.
**Expected:** the directory is renamed on disk, the old path is gone, the config
`name` and `path` both update, and `git rev-parse --git-dir` still resolves inside
the new directory (i.e. `git worktree repair` ran).
**Fails if:** the config updates but the directory does not, or git loses the worktree.

### UC-79 — Rename a branch end to end
**Area:** Worktree detail | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Steps:** reach the `Branch name` field, append `-v2`, `Enter`.
**Expected:** `git branch --show-current` inside the worktree returns the new name,
and the config `branch` matches.

### UC-80 — Add a repository with "Import existing worktrees"
**Area:** Modals | **Priority:** P1 | 🟢 EXECUTED (both builds)
**Setup:** the fixture's `gamma` repo is deliberately untracked and owns a worktree
**outside** the forest directory.
**Steps:** `a`, type the gamma path, tick the checkbox, confirm.
**Expected:** `gamma` joins the config **and** its external worktree is imported
with branch `feat/imported`.
**Fails if:** the repo is added but the worktree is not — the checkbox did nothing.

### UC-81 — A second terminal joins as a grouped session
**Area:** tmux | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Setup:** a second `tu` session sharing the same `TMUX_TMPDIR`.
**Expected:** both terminals see the same window count, and switching windows in
terminal B leaves terminal A on its own window.
**Fails if:** B's navigation drags A along — that is the bug grouped sessions exist
to prevent.

### UC-82 — Detach and relaunch reattaches without duplicating
**Area:** tmux | **Priority:** P0 | 🟢 EXECUTED (both builds)
**Setup:** launched through a login shell so the shell survives the detach.
**Steps:** run forestui, `Ctrl+B d` to detach, run it again.
**Expected:** it reattaches, and exactly one forestui window exists.
**Fails if:** a second forestui window appears on reattach.

---

### UC-83 — Clicking a sidebar row selects it
**Area:** Mouse | **Priority:** P0 | 🟢 EXECUTED (Rust)
**Expected:** the click selects that repository or worktree and the detail pane follows.

### UC-84 — Clicking a detail control runs it
**Area:** Mouse | **Priority:** P0 | 🟢 EXECUTED (Rust)
**Expected:** clicking `Terminal` opens `term:<name>` exactly as pressing `t` does.
Covers `Editor`, `Terminal`, `Files`, `New Session`, `New Session: YOLO`, custom
buttons, `Resume`, `Archive`, `Delete`, `Remove Repository`.
**Fails if:** nothing happens — the regression a user hit on the first Rust build,
where mouse capture was never enabled and no control recorded a clickable region.

### UC-85 — Clicking a rename field focuses it without acting
**Area:** Mouse | **Priority:** P1 | 🟢 EXECUTED (Rust)
**Expected:** focus moves to the field; no action fires, no window opens.

### UC-86 — Clicking a modal control activates it
**Area:** Mouse | **Priority:** P0 | 🟢 EXECUTED (Rust)
**Expected:** clicking `Delete` in the confirm modal deletes; clicking `Cancel`
dismisses. Clicking a pane behind an open modal does nothing — the modal keeps
every click.

### UC-87 — Clicking a branch row picks that branch
**Area:** Mouse | **Priority:** P1 | 🟢 EXECUTED (Rust)
**Expected:** in existing-branch mode, clicking a dropdown row selects it and
enables `Create Worktree`.

### UC-88 — Scroll wheel moves the focused pane
**Area:** Mouse | **Priority:** P2 | 🟢 EXECUTED (Rust)
**Expected:** the wheel moves the sidebar cursor or the detail focus ring
depending on which pane has focus.

### UC-89 — Controls render as buttons
**Area:** Visual | **Priority:** P1 | 🟢 EXECUTED (Rust)
**Expected:** every control renders as a filled pill (`▐ Label ▏`), visibly a
button rather than plain text, with the focused one highlighted and destructive
ones red.
**Fails if:** controls read as flat labels — a user reported exactly that.

---

## Area M — Visual parity against the Python build

Found by rendering both builds on the same fixture and comparing the captured
PNGs pair by pair (`doc/rust-rewrite/screenshots/<build>/`). Text frames prove
the *content* matches; these cover how it *looks*.

### UC-90 — Mouse reporting is on, but not any-motion
**Area:** Mouse | **Priority:** P0 | 🟢 EXECUTED (both builds; the second half is Rust-only)
**Steps:** `tu mouse state --name <session>` right after launch.
**Expected (both):** `enabled: true` — without it no click reaches the app at all.
**Expected (Rust):** the mode is **not** `AnyMotion`. `EnableMouseCapture` turns on
`?1003h`, so the terminal reports every pointer movement; the Rust loop redraws
per event, so that showed up as the whole app flickering when the mouse merely
moved across it. The build now requests `?1000h` + `?1006h` — buttons and wheel
only — and drains queued events before repainting.
**Not asserted for Python:** Textual legitimately uses any-motion tracking
because it supports hover.
**Fails if:** a click does nothing (reporting off), or the app repaints on
pointer movement (flicker).

### UC-91 — A modal dims what is behind it
**Area:** Visual | **Priority:** P1 | 🟢 EXECUTED (both builds)
**Expected:** with any modal open, the panes behind it are visibly darkened, so
the dialog reads as modal. Textual dimmed the backdrop; the first Rust cut did
not, leaving the modal competing with a fully-lit pane.

### UC-92 — Modals keep a margin from the screen edges
**Area:** Visual | **Priority:** P2 | 🟢 EXECUTED (Rust)
**Expected:** the wide modals (Settings, Custom Buttons) do not run flush to the
left and right edges. Textual capped them at `max-width: 95%`; the Rust build
clamped to the full width, so a 140-column terminal produced a dialog with no
margin at all.

### UC-93 — The help toast shows its whole text
**Area:** Visual | **Priority:** P1 | 🟢 EXECUTED (both builds)
**Steps:** press `?`.
**Expected:** the full key list is readable. Python wrapped the toast over
several lines; the Rust build truncated it to one line and hid most of what the
user pressed `?` to read. Notifications now wrap on word boundaries.

### UC-94 — Section rules and item cards
**Area:** Visual | **Priority:** P1 | 🟡 IN PROGRESS
**Expected:** the detail pane carries the same visual structure Textual gave it —
horizontal rules between sections, an elevated bordered card behind each Claude
session and each GitHub issue, and the LOCATION path in a bordered box. Without
them the pane reads as one flat undifferentiated list, which is what a user
comparing the two builds side by side noticed first.

---

## Known visual divergences (accepted, not defects)

These are consequences of the immediate-mode port and are **not** treated as
regressions. Any change here is a deliberate decision, not a test failure.

Verify these against the composites (`scripts/tu-composite.sh`), not the text
frames — most of them are invisible in a frame diff.

| | Textual | ratatui |
|---|---|---|
| Sidebar tree | `▼` arrows, repositories collapse | flat list, no collapse |
| Sidebar guides | tree guide *and* a hand-drawn prefix, so a worktree reads `└ └─  wt-a` | one prefix, `└─ wt-a` |
| Sidebar branch | swallowed by console-markup parsing | shown, as intended |
| Sidebar cursor | nothing highlighted until the cursor is first moved | the selected row is highlighted from boot |
| Selects | dropdown overlay | `◂ value ▸` cycled with Left/Right |
| Footer | `a` first, because Textual lists the focused widget's bindings ahead of the app's; includes `^p` command palette | fixed order; no command palette |
| Button labels | carry a vestigial leading space (`Button(" Editor")`), so every box is a cell wider | no leading space |
| `⟳ Git Pull` | one space after the glyph | two, because `⟳` is double-width in some terminals |

Everything else in the detail pane, the sidebar header box and the modals is
matched deliberately, down to the blank rows Textual's margins produced. If a
composite shows a difference not listed above, treat it as a regression.

---

## Appendix — quick coverage map

| Area | UCs | P0 count |
|---|---|---|
| Startup & tmux entry | 01–07 | 3 |
| Sidebar | 08–11 | 2 |
| Repository detail | 12–15 | 2 |
| Worktree detail | 16–19 | 1 |
| Hotkeys | 20–28 | 5 |
| Modals | 29–38 | 5 |
| tmux / grouped sessions | 39–41 | 2 |
| Errors & edge cases | 42–47 | 2 |
| Config persistence | 48–52 | 3 |
| Automated parity sweep | 53–70 | 12 |
| Hand-driven, unscripted | 71–77 | 4 |
| Flows & mouse | 78–89 | 8 |
| Visual parity | 90–94 | 1 |

**94 use cases · 50 P0 · 76 executed live · 17 written from source only.**

UC-01–52 were written against the Python build on 2026-08-14 (35 executed).
UC-53–70 are the automated sweep and have been executed against **both** builds;
UC-71–76 were hand-driven against the Rust build; UC-77 is blocked on `gh` auth.

Written from source, never driven live: UC-02, 03, 05, 15, 18, 19, 25, 35, 37, 38,
41, 44, 45, 46, 47, 51, 52. Everything else in this file was transcribed from a real
screenshot taken on 2026-08-14 against Python `0.0.0` at commit `3021d72`, in an
isolated `TMUX_TMPDIR` + `HOME`, against a throwaway `mktemp -d` forest.

Three "Expected" blocks encode current Python **bugs** rather than intended design —
UC-07 (invisible empty state), UC-11 (branch swallowed by markup), UC-50 (archived
worktrees unreachable) — plus UC-43 (window created in the wrong directory). Decide
per-item before the Rust PR whether the rewrite matches the bug or fixes it, and
update the block accordingly. Do not let a fix land as a silent test failure.
