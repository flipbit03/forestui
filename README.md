# forestui

> A terminal UI for managing Git worktrees, inspired by [forest](https://github.com/ricwo/forest) for macOS by [@ricwo](https://github.com/ricwo).

forestui brings Git worktree management to the terminal with a TUI built on
[ratatui](https://ratatui.rs), featuring deep integration with
[Claude Code](https://claude.ai/code).

![forestui screenshot](doc/screenshot_small.png)

## Features

- **Repository Management**: Add and track multiple Git repositories
- **Worktree Operations**: Create, rename, archive, and delete worktrees
- **TUI Editor Integration**: Opens TUI editors (vim, nvim, helix, etc.) in tmux windows
- **Claude Code Integration**: Track and resume Claude Code sessions per worktree
- **GitHub Issues**: Create a worktree straight from an issue assigned to you
- **Multi-Forest Support**: Manage multiple forest directories via CLI argument
- **tmux Native**: Runs inside tmux for a cohesive terminal experience
- **Single Binary**: No runtime, no virtualenv — one static executable

## Requirements

- tmux
- [gh](https://cli.github.com/) (optional, for GitHub integration)
- Rust 1.88+ (only if you build from source)

## Installing

### Quick Install (recommended)

Downloads the prebuilt binary for your platform and verifies its published
checksum before installing.

```bash
curl -fsSL https://raw.githubusercontent.com/flipbit03/forestui/main/install.sh | sh
```

### Install with cargo

```bash
cargo install forestui --locked
```

Prebuilt binaries are published for:

| Platform | Target |
|---|---|
| Linux x86_64 | `x86_64-unknown-linux-musl` |
| Linux aarch64 | `aarch64-unknown-linux-musl` |
| macOS Apple silicon | `aarch64-apple-darwin` |

The Linux binaries are statically linked, so they carry no glibc floor and run
anywhere. Intel Macs and every other platform build from source with
`cargo install forestui`.

### Updating

forestui keeps itself up to date. It checks for a new release in the background
after the UI is up — never blocking startup — and tells you once a newer version
is in place:

```
forestui v2.0.1 installed — restart to use it
```

A binary installed from a release replaces itself. One installed with
`cargo install` reports the new version instead of recompiling underneath you,
so you can update when it suits:

```bash
cargo install forestui --locked
```

Pass `--no-self-update` to skip the check entirely. The result is cached for a
day, so this is not a network call on every launch, and a build from source
(version `0.0.0`) never updates itself at all.

> **Migrating from the Python build.** forestui was a Python/Textual
> application through v0.9.x. The Rust rewrite reads the same config files, so
> your repositories, worktrees, and settings carry over untouched. Remove the
> old install so the new binary wins: `uv tool uninstall forestui`.

## Usage

```bash
# Start with the default forest directory (~/forest)
forestui

# Start with a custom forest directory
forestui ~/my-projects

# Show help
forestui --help
```

### Keyboard Shortcuts

Focus moves between the sidebar and the detail pane with `Tab`. Inside either
pane, `↑`/`↓` move and `Enter` activates.

| Key | Action |
|-----|--------|
| `Tab` | Switch focus between sidebar and detail pane |
| `↑` / `↓` | Move within the focused pane |
| `Enter` | Select a row / activate the focused control |
| `a` | Add repository |
| `w` | Add worktree |
| `e` | Open in editor |
| `t` | Open in terminal |
| `o` | Open in file manager |
| `n` | Start Claude session |
| `y` | Start Claude session (YOLO mode) |
| `h` | Toggle archive on the selected worktree |
| `A` | Show or hide the archived section |
| `d` | Delete |
| `s` | Settings |
| `r` | Refresh |
| `q` | Quit |

In modals: `Tab` / `Shift+Tab` move between fields, `Enter` activates, `Esc`
cancels. Confirmation dialogs also accept `y` and `n`. The custom-buttons
manager uses `a` add, `e` edit, `d` delete, `K` / `J` reorder, `s` save.

### Mouse

The mouse works everywhere the keyboard does. Controls light up as the pointer
crosses them, so what is clickable is visible rather than guessed at:

- **Click** a repository or worktree in the sidebar, a control in the detail
  pane, a field to focus it, any modal button, or a key in the footer bar —
  clicking `s Settings` there is the same as pressing `s`.
- **The `▼` / `▶` twisty** beside a repository folds its worktrees away without
  changing what is selected.
- **The scroll wheel** scrolls whichever pane the pointer is over, whether or
  not it holds the keyboard focus.
- **The scrollbar** can be dragged, and clicking its track pages to that point.

### TUI Editor Integration

When your default editor is a TUI editor (vim, nvim, helix, nano, etc.),
forestui opens it in a new tmux window named `edit:<worktree>`. This keeps your
editing session organized alongside forestui and any Claude sessions.

Supported TUI editors: `vim`, `nvim`, `vi`, `emacs`, `nano`, `helix`, `hx`,
`micro`, `kakoune`, `kak`

### Multi-Forest Support

forestui stores its state (`.forestui-config.json`) in the forest directory
itself, so you can manage multiple independent forests:

```bash
forestui ~/work      # Uses ~/work/.forestui-config.json
forestui ~/personal  # Uses ~/personal/.forestui-config.json
```

User preferences (editor, theme, branch prefix, custom Claude buttons) are
stored globally in `~/.config/forestui/settings.json`.

### Themes

Settings → Theme opens a picker over 31 named palettes — Dracula, Nord,
Gruvbox, Solarized, the Catppuccin and Rosé Pine and Tokyo Night families,
GitHub, and more — with the app behind the dialog live-previewing the
highlighted theme. Enter applies, Esc reverts, Save persists. The default,
Forest Dark, is the palette forestui has always had. The chosen theme is
stored in `theme_name`; the legacy `theme` field (the old inert
System/Dark/Light choice) is preserved untouched so the settings file keeps
working in the Python build too.

## Configuration

Settings are stored in `~/.config/forestui/settings.json`:

```json
{
  "default_editor": "nvim",
  "default_terminal": "",
  "branch_prefix": "feat/",
  "theme": "system",
  "theme_name": "forest-dark",
  "custom_buttons": [
    {
      "label": "Opus",
      "prefix": "opus",
      "command": "claude --model opus"
    }
  ]
}
```

Press `s` in the app to open the settings modal.

Custom Claude buttons add extra entries to the CLAUDE section of the detail
pane. Each one opens a tmux window named `<prefix>:<worktree>` running its
command verbatim. A command containing `--dangerously-skip-permissions` is
styled red.

## Development

```bash
# Clone and enter the repo
git clone https://github.com/flipbit03/forestui.git
cd forestui

# Install the toolchain components
make dev

# Run checks (format, clippy, typecheck, tests)
make check

# Format code
make format

# Run the app
make run
```

See [CLAUDE.md](CLAUDE.md) for AI-assisted development guidelines, and
[doc/rust-rewrite/](doc/rust-rewrite/) for the specification, architecture,
migration plan, and the `tu`-driven acceptance playbook.

## Compatibility with forest (macOS)

forestui is designed to coexist with [forest](https://github.com/ricwo/forest)
for macOS:

- Both apps can share the same `~/forest` directory for worktrees
- Each app maintains its own state file:
  - forest: `.forest-config.json` (stored in `~/.config/forest/`)
  - forestui: `.forestui-config.json` (stored in the forest folder itself)
- Worktrees created by either app work seamlessly with both

**Key difference:** forestui stores its state inside the forest folder
(`~/forest/.forestui-config.json`) rather than in a global config directory.
This design enables multi-forest support — you can run `forestui ~/work` and
`forestui ~/personal` with completely independent state for each.

## License

MIT
