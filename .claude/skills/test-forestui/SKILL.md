---
name: test-forestui
description: Use tu to visually drive and test forestui in headless terminals. Invoke when developing forestui features, debugging UI issues, or verifying changes — lets you see and interact with the running app yourself.
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
The user is likely running their own tmux/forestui session right now. If you
connect to the default tmux server you will interfere with their live session —
creating windows, switching their active view, or corrupting their session state.

```bash
FUI_TEST_DIR=$(mktemp -d)

tu run --name fui --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX uv run forestui

tu wait --name fui --text "forestui" --timeout 15000
```

Need multiple terminals (e.g., testing grouped sessions)? Use the same
`TMUX_TMPDIR` so they share the same isolated tmux server:

```bash
tu run --name fui-a --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX uv run forestui
tu run --name fui-b --env TMUX_TMPDIR=$FUI_TEST_DIR \
  --cwd <project-root> -- env -u TMUX uv run forestui
```

## How to see the screen

```bash
tu screenshot --name fui        # full screen dump
tu scrollback --name fui        # if content scrolled off
```

The status bar is the last line. Parse it for session name, window list, etc.

## How to interact

### forestui hotkeys (direct, no tmux prefix)

`e` editor, `t` terminal, `o` files (mc), `n` claude, `y` yolo claude,
`a` add repo, `w` add worktree, `q` quit, `h` archive, `d` delete,
`Up`/`Down` navigate sidebar, `Enter` select.

### tmux navigation

Use the bindings from `.tmux.conf`. Translate to `tu press` yourself.

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
