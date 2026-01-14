# forestui

> A terminal UI for managing Git worktrees, inspired by [forest](https://github.com/ricwo/forest) for macOS by [@ricwo](https://github.com/ricwo).

forestui brings the power of Git worktree management to the terminal with a beautiful TUI interface built on [Textual](https://textual.textualize.io/).

## Features

- **Repository Management**: Add and track multiple Git repositories
- **Worktree Operations**: Create, rename, archive, and delete worktrees
- **TUI Editor Integration**: Opens TUI editors (vim, nvim, helix, etc.) in tmux windows
- **Claude Code Integration**: Track and resume Claude Code sessions per worktree
- **Multi-Forest Support**: Manage multiple forest directories via CLI argument
- **tmux Native**: Runs inside tmux for a cohesive terminal experience

## Installing

```bash
curl -fsSL https://raw.githubusercontent.com/flipbit03/forestui/main/install.sh | bash
```

## Requirements

- Python 3.14+
- tmux
- uv (for installation)

## Usage

```bash
# Start with default forest directory (~/forest)
forestui

# Start with a custom forest directory
forestui ~/my-projects

# Update to latest version
forestui --self-update

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
| `h` | Toggle archive |
| `d` | Delete |
| `s` | Settings |
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

User preferences (editor, theme, branch prefix) are stored globally in `~/.config/forest-tui/settings.json`.

## Configuration

Settings are stored in `~/.config/forest-tui/settings.json`:

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
  - forest: `.forest-config.json`
  - forestui: `.forestui-config.json`
- Worktrees created by either app work seamlessly with both

## License

MIT
