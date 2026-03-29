---
name: test-forestui
description: Use tu to visually drive and test forestui in headless terminals. Invoke PROACTIVELY when fixing behavioral/visual bugs or developing new features — don't wait to be asked. Lint and typecheck can't catch UI regressions, wrong window names, or broken interactions.
allowed-tools: Bash, Read
argument-hint: "[what to test or verify]"
---

# Self-drive forestui with `tu`

You can launch, see, and interact with forestui in headless virtual terminals
using `tu` (terminal-use). This gives you the ability to visually verify your
own code changes without the user needing to test manually.

Use this whenever you're developing forestui and want to confirm your changes
work. Design your own test approach based on what you changed — there is no
fixed test suite. You are the tester.

## FIRST: bootstrap context

Before doing ANYTHING else, run these two commands and read their output.
Do NOT skip. Do NOT proceed until both are in context.

```bash
tu usage
```
```bash
cat ~/.tmux.conf 2>/dev/null || echo "NO_TMUX_CONF"
```

From `tu usage`: learn all commands, key syntax, wait conditions.
From `.tmux.conf`: learn the user's tmux prefix and all keybindings.
Translate tmux bind syntax to `tu press` key names yourself from these two sources.
If no `.tmux.conf`: defaults are prefix `Ctrl+B`, next `Ctrl+B n`, prev `Ctrl+B p`.

## How to launch forestui

**NEVER run `uv run forestui` or any tmux command without `TMUX_TMPDIR` isolation.**
**NEVER call `tmux` directly from Bash** — not even with `TMUX_TMPDIR` set. The
user is likely running their own tmux/forestui session right now. Any direct
`tmux` call risks connecting to (and corrupting) their live session. ALL
interaction with the test tmux must go through `tu` commands.

```bash
FUI_TEST_DIR=$(mktemp -d)

tu run --name fui --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX uv run forestui

tu wait --name fui --text "forestui" --timeout 15000
```

### When you need to detach and reattach

If your test involves detaching from tmux and re-running `forestui` (e.g.,
testing reattach behavior), do NOT launch forestui directly as the `tu` process.
Instead, launch a **bash shell** inside `tu` and run forestui from it. This way,
when tmux detaches, bash gets its prompt back and you can run forestui again:

```bash
tu run --name fui --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX bash -l

# Wait for bash prompt, then start forestui
tu wait --name fui --text "\\$" --timeout 5000
tu type --name fui "uv run forestui"
tu press --name fui Enter

# ... do your test, detach with Ctrl+A d ...
# After detach, bash prompt returns. Run forestui again:
tu wait --name fui --text "\\$" --timeout 5000
tu type --name fui "uv run forestui"
tu press --name fui Enter
```

Why: forestui calls `os.execvp("tmux", ...)` to enter tmux. If forestui IS the
`tu` process, detaching kills the tu session (no shell to return to).

Need multiple terminals (e.g., testing grouped sessions)? Use the same
`TMUX_TMPDIR` so they share the same isolated tmux server:

```bash
tu run --name fui-a --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX uv run forestui
tu run --name fui-b --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX uv run forestui
```

## How to see the screen

**ALWAYS use PNG mode for screenshots.** Plain text loses colors, highlights, and
styling — you can't tell which tmux window is active, what's selected, or see
UI state like notifications. PNG gives you the real terminal with full color.

```bash
tu screenshot --name fui --png -o /tmp/fui-shot.png   # then Read the file to see it
tu scrollback --name fui                               # if content scrolled off
```

Use the Read tool on the PNG to visually inspect it. The status bar is at the
bottom — look for the orange-highlighted active window to know which window
each session is viewing.

## How to interact

### forestui hotkeys (direct, no tmux prefix)

`e` editor, `t` terminal, `o` files (mc), `n` claude, `y` yolo claude,
`a` add repo, `w` add worktree, `q` quit, `h` archive, `d` delete,
`Up`/`Down` navigate sidebar, `Enter` select.

### tmux navigation

Use the bindings from `.tmux.conf`. Translate to `tu press` yourself.

**Gotcha — renaming windows.** Check `.tmux.conf` for the rename-window binding.
If it uses `command-prompt -I "#W"`, the prompt is pre-filled with the current
window name — press `Ctrl+U` to clear before typing. If it uses
`command-prompt` without `-I`, the prompt starts empty and you can type directly.

### Text input

```bash
tu type --name fui "text"       # literal text
tu press --name fui Enter       # keystrokes
tu paste --name fui "text"      # bracketed paste
```

## How to verify

Screenshot, read, assert. Design checks based on what you changed:

- **UI change?** Screenshot and check the relevant region.
- **New window?** Check the status bar for the expected window name.
- **Multi-terminal behavior?** Launch two instances, act in one, screenshot both.
- **Key binding?** Press it, screenshot, confirm the expected result.
- **Error case?** Trigger it, screenshot, check for error message or correct state.

Use `tu wait --text "pattern"` when you know what to expect.
Use `sleep 1` after tmux window changes.

## Cleanup

ALWAYS clean up when done:

```bash
tu kill --name fui 2>/dev/null
tu kill --name fui-a 2>/dev/null
tu kill --name fui-b 2>/dev/null
rm -rf $FUI_TEST_DIR
```

## Tips

- `tu wait --text "pattern" --timeout 10000` is better than `sleep`
- `tu status --name fui` to check if a session is alive
- `tu list` to see all active sessions
- If something looks wrong, screenshot FIRST before retrying
- The active tmux window isn't visually distinct in plain text screenshots,
  but the first line of content tells you what's displayed

## Multi-terminal testing

forestui uses tmux grouped sessions so multiple terminals can share the same
windows but navigate independently. To test this kind of behavior — or any
scenario where two users/terminals interact with the same forestui session —
launch multiple `tu` instances sharing the same `TMUX_TMPDIR`:

```bash
tu run --name fui-a --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX uv run forestui
tu run --name fui-b --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX uv run forestui
```

Because they share `TMUX_TMPDIR`, they connect to the same tmux server.
The first creates the original session; subsequent ones join it as grouped
sessions — exactly like a user opening forestui from multiple terminals.

This lets you:
- Act in one session (`tu press --name fui-b t`) and screenshot the other
  (`tu screenshot --name fui-a`) to confirm it wasn't affected
- Verify that window creation from either terminal targets the correct session
- Check that both status bars show consistent info (same session name, same
  window list) despite being different tmux sessions internally
- Test any feature where "what happens in terminal A vs terminal B" matters

You can launch as many parallel sessions as needed (fui-a, fui-b, fui-c, ...).
Clean up ALL of them when done.
