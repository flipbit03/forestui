# CLAUDE.md - AI Development Guidelines

This file provides context for AI assistants (like Claude) working on forestui.

## Project Overview

forestui is a terminal UI for managing Git worktrees, built with Python and Textual. It's designed to run inside tmux and provides a cohesive experience for developers managing multiple worktrees.

## Tech Stack

- **Python 3.14+** with strict type hints (mypy strict mode)
- **Textual 7.2+** for the TUI framework
- **Pydantic 2.12+** for data models and validation
- **Click 8.1+** for CLI parsing
- **uv** for package management

## Project Structure

```
forestui/
├── forestui/
│   ├── __init__.py          # Package init, version
│   ├── __main__.py           # Module entry point
│   ├── app.py                # Main Textual application
│   ├── cli.py                # Click CLI with --self-update
│   ├── models.py             # Pydantic data models
│   ├── state.py              # Application state management
│   ├── theme.py              # CSS styling
│   ├── components/
│   │   ├── modals.py         # Modal dialogs (settings, add repo, etc.)
│   │   ├── sidebar.py        # Repository/worktree sidebar
│   │   ├── repository_detail.py
│   │   └── worktree_detail.py
│   └── services/
│       ├── settings.py       # User preferences + runtime forest path
│       ├── git.py            # Async git operations
│       ├── claude_session.py # Claude Code session tracking
│       └── tmux.py           # tmux window management
├── pyproject.toml            # Project config, dependencies
├── Makefile                  # Development commands
├── install.sh                # Installation script
└── README.md
```

## Development Commands

```bash
make lint       # Run ruff linter
make typecheck  # Run mypy type checker
make check      # Run both lint and typecheck
make format     # Format code with ruff
make dev        # Install dev dependencies
make run        # Run the app
make clean      # Clean build artifacts
```

Always run `make check` before committing changes.

## Code Conventions

### Type Hints
- All functions must have type hints (mypy strict mode)
- Use `from __future__ import annotations` for forward references
- Prefer `X | None` over `Optional[X]`

### Imports
- Use absolute imports: `from forestui.services.git import GitService`
- Imports are auto-sorted by ruff (isort rules)

### Textual Patterns
- Components emit `Message` classes for parent communication
- Use `@work` decorator for async operations that update UI
- CSS is defined in `theme.py` as `APP_CSS`

### Async Operations & Reactive UI

**IMPORTANT:** Never block the UI with slow operations. The app must start instantly.

For any operation that may take time (CLI calls, network requests, file I/O):
1. Render UI immediately with a loading state ("Loading...", "Checking...")
2. Dispatch work to a background task using `@work` decorator
3. When the task completes, update the UI reactively

Example pattern:
```python
# In compose(), show loading state:
yield Label("Loading...", id="my-loading")

# In on_mount() or elsewhere, dispatch background work:
self._fetch_data()

@work
async def _fetch_data(self) -> None:
    data = await slow_operation()
    # Update UI when ready
    self.query_one("#my-loading").update(f"Result: {data}")
```

This applies to:
- External CLI integrations (gh, git, etc.)
- Network requests
- File system operations that may be slow
- Any operation that could fail or hang

The UI should always remain responsive. Users should never see the app freeze.

### Services
- Services are singletons accessed via `get_*_service()` functions
- State is persisted automatically via `_save_state()` methods

## Key Design Decisions

### tmux Requirement
forestui requires tmux and will auto-exec into a tmux session if not already inside one. This enables:
- TUI editors opening in tmux windows
- Session persistence
- Fast switching between editor/app/claude windows

### Multi-Forest Support
The forest directory is a CLI argument, not a setting:
```bash
forestui ~/forest      # default
forestui ~/work        # different forest
```

Each forest has its own `.forestui-config.json` state file.

### Coexistence with forest (macOS)
- forestui uses `.forestui-config.json`
- forest uses `.forest-config.json`
- Both can share the same `~/forest` directory safely

## Common Tasks

### Adding a New Service
1. Create `forestui/services/myservice.py`
2. Implement singleton pattern with `get_myservice()` function
3. Export from `forestui/services/__init__.py`

### Adding a New Component
1. Create `forestui/components/mycomponent.py`
2. Define `Message` classes for events
3. Handle messages in `app.py` with `on_mycomponent_*` methods

### Adding a New CLI Option
1. Add to `@click.option` in `forestui/cli.py`
2. Handle in the `main()` function

### Modifying Settings
1. Update `Settings` model in `models.py`
2. Update `SettingsModal` in `components/modals.py`
3. Settings are auto-persisted to `~/.config/forestui/settings.json`

## Testing

Currently no test suite. When adding tests:
- Use pytest
- Place tests in `tests/` directory
- Add pytest to dev dependencies

## Versioning

Version is `0.0.0` in source code. Actual versions are derived from git tags at release time.

### Development

1. Create a branch, make changes
2. Run `make check` and test locally
3. Open a PR and merge to `main`

Running from source (`uv run forestui`) shows version `0.0.0` and auto-enables dev mode.

### Releasing

```bash
gh release create v0.9.1 --generate-notes
```

This triggers the publish workflow which builds and publishes to PyPI.

## Git Commits

- Do NOT include `Co-Authored-By` attribution
- Do NOT include "Generated with Claude Code" footer
- Write clear, concise commit messages

## References

- [forest (macOS)](https://github.com/ricwo/forest) - Original inspiration
- [Textual Documentation](https://textual.textualize.io/)
- [Pydantic Documentation](https://docs.pydantic.dev/)
- [Click Documentation](https://click.palletsprojects.com/)
