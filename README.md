# forestui

> A terminal UI for managing Git worktrees, inspired by [forest](https://github.com/ricwo/forest) for macOS by [@ricwo](https://github.com/ricwo).

forestui brings the power of Git worktree management to the terminal with a beautiful TUI interface built on [Textual](https://textual.textualize.io/), featuring deep integration with [Claude Code](https://claude.ai/code).

![forestui screenshot](doc/screenshot_small.png)

## Features

- **Repository Management**: Add and track multiple Git repositories
- **Worktree Operations**: Create, rename, archive, and delete worktrees
- **TUI Editor Integration**: Opens TUI editors (vim, nvim, helix, etc.) in tmux windows
- **Claude Code Integration**: Track and resume Claude Code sessions per worktree
- **Multi-Forest Support**: Manage multiple forest directories via CLI argument
- **tmux Native**: Runs inside tmux for a cohesive terminal experience

## Requirements

- Python 3.14+
- tmux
- uv (for installation)
- [gh](https://cli.github.com/) (optional, for GitHub integration)

## Installing

### Quick Install (recommended)

Installs [uv](https://github.com/astral-sh/uv) automatically if not present. No Python installation required.

```bash
curl -fsSL https://raw.githubusercontent.com/flipbit03/forestui/main/install.sh | bash
```

### Install via uv

If you already have [uv](https://github.com/astral-sh/uv) installed:

```bash
uv tool install forestui
```

### Updating

forestui auto-updates on startup. To manually update:

```bash
uv tool upgrade forestui
```

## Usage

```bash
# Start with default forest directory (~/forest)
forestui

# Start with a custom forest directory
forestui ~/my-projects

# Show help
forestui --help
```

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `a` | Add repository |
| `w` | Add worktree |
| `e` | Open in editor |
| `t` | Open in terminal |
| `o` | Open in file manager |
| `n` | Start Claude session |
| `y` | Start Claude session (YOLO mode) |
| `h` | Toggle archive |
| `d` | Delete |
| `s` | Settings |
| `r` | Refresh |
| `?` | Show help |
| `q` | Quit |

### TUI Editor Integration

When your default editor is a TUI editor (vim, nvim, helix, nano, etc.), forestui opens it in a new tmux window named `edit:<worktree>`. This keeps your editing session organized alongside forestui and any Claude sessions.

Supported TUI editors: `vim`, `nvim`, `vi`, `emacs`, `nano`, `helix`, `hx`, `micro`, `kakoune`, `kak`

### Multi-Forest Support

forestui stores its state (`.forestui-config.json`) in the forest directory itself, allowing you to manage multiple independent forests:

```bash
forestui ~/work      # Uses ~/work/.forestui-config.json
forestui ~/personal  # Uses ~/personal/.forestui-config.json
```

User preferences (editor, theme, branch prefix) are stored globally in `~/.config/forestui/settings.json`.

## Configuration

Settings are stored in `~/.config/forestui/settings.json`:

```json
{
  "default_editor": "nvim",
  "branch_prefix": "feat/",
  "theme": "system"
}
```

Press `s` in the app to open the settings modal.

## Development

```bash
# Clone and enter the repo
git clone https://github.com/flipbit03/forestui.git
cd forestui

# Install dev dependencies
make dev

# Run checks
make check

# Format code
make format

# Run the app
make run
```

See [CLAUDE.md](CLAUDE.md) for AI-assisted development guidelines.

## Compatibility with forest (macOS)

forestui is designed to coexist with [forest](https://github.com/ricwo/forest) for macOS:

- Both apps can share the same `~/forest` directory for worktrees
- Each app maintains its own state file:
  - forest: `.forest-config.json` (stored in `~/.config/forest/`)
  - forestui: `.forestui-config.json` (stored in the forest folder itself)
- Worktrees created by either app work seamlessly with both

**Key difference:** forestui stores its state inside the forest folder (`~/forest/.forestui-config.json`) rather than in a global config directory. This design enables multi-forest support - you can run `forestui ~/work` and `forestui ~/personal` with completely independent state for each.

## License

MIT
