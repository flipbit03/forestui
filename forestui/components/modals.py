"""Modal dialog components for forestui."""

from pathlib import Path
from uuid import UUID

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.message import Message
from textual.screen import ModalScreen
from textual.widgets import Button, Checkbox, Input, Label, Select

from forestui.models import (
    MAX_CLAUDE_COMMAND_LENGTH,
    ClaudeCommandResult,
    GitHubIssue,
    Repository,
    Settings,
    validate_claude_command,
)


class AddRepositoryModal(ModalScreen[str | None]):
    """Modal for adding a new repository."""

    BINDINGS = [
        ("escape", "cancel", "Cancel"),
    ]

    class RepositoryAdded(Message):
        """Sent when a repository is added."""

        def __init__(self, path: str, import_worktrees: bool = False) -> None:
            self.path = path
            self.import_worktrees = import_worktrees
            super().__init__()

    def __init__(self) -> None:
        super().__init__()
        self._path: str = ""
        self._error: str = ""
        self._import_worktrees: bool = False

    def compose(self) -> ComposeResult:
        """Compose the modal UI."""
        with Vertical(classes="modal-container"):
            yield Label(" Add Repository", classes="modal-title")

            yield Label("Repository Path", classes="section-header")
            yield Input(
                placeholder="Enter path or paste from clipboard...",
                id="input-path",
            )

            yield Label("", id="label-status", classes="label-secondary")

            yield Checkbox("Import existing worktrees", id="checkbox-import")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Add Repository", id="btn-add", variant="primary")

    def on_input_changed(self, event: Input.Changed) -> None:
        """Handle path input changes."""
        if event.input.id == "input-path":
            self._path = event.value
            self._validate_path()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        """Handle enter key in input."""
        if event.input.id == "input-path":
            self._add_repository()

    def _validate_path(self) -> None:
        """Validate the entered path."""
        status_label = self.query_one("#label-status", Label)

        if not self._path:
            status_label.update("")
            status_label.remove_class("label-destructive")
            status_label.add_class("label-secondary")
            return

        path = Path(self._path).expanduser()

        if not path.exists():
            status_label.update("Path does not exist")
            status_label.remove_class("label-secondary")
            status_label.add_class("label-destructive")
            return

        if not path.is_dir():
            status_label.update("Path is not a directory")
            status_label.remove_class("label-secondary")
            status_label.add_class("label-destructive")
            return

        # Check if it's a git repository
        git_dir = path / ".git"
        if not git_dir.exists():
            status_label.update("Not a git repository")
            status_label.remove_class("label-secondary")
            status_label.add_class("label-destructive")
            return

        status_label.update(f"Repository: {path.name}")
        status_label.remove_class("label-destructive")
        status_label.add_class("label-secondary")

    def on_checkbox_changed(self, event: Checkbox.Changed) -> None:
        """Handle checkbox changes."""
        if event.checkbox.id == "checkbox-import":
            self._import_worktrees = event.value

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        if event.button.id == "btn-cancel":
            self.dismiss(None)
        elif event.button.id == "btn-add":
            self._add_repository()

    def _add_repository(self) -> None:
        """Add the repository if valid."""
        if not self._path:
            return

        path = Path(self._path).expanduser()
        if not path.exists() or not (path / ".git").exists():
            return

        self.post_message(self.RepositoryAdded(str(path), self._import_worktrees))
        self.dismiss(str(path))

    def action_cancel(self) -> None:
        """Cancel and close the modal."""
        self.dismiss(None)


class AddWorktreeModal(ModalScreen[tuple[str, str, bool] | None]):
    """Modal for adding a new worktree."""

    BINDINGS = [
        ("escape", "cancel", "Cancel"),
    ]

    class WorktreeCreated(Message):
        """Sent when a worktree is created."""

        def __init__(
            self, repo_id: UUID, name: str, branch: str, new_branch: bool
        ) -> None:
            self.repo_id = repo_id
            self.name = name
            self.branch = branch
            self.new_branch = new_branch
            super().__init__()

    def __init__(
        self,
        repository: Repository,
        branches: list[str],
        forest_dir: Path,
        branch_prefix: str = "feat/",
    ) -> None:
        super().__init__()
        self._repository = repository
        self._branches = branches
        self._forest_dir = forest_dir
        self._branch_prefix = branch_prefix
        self._name: str = ""
        self._branch: str = ""
        self._new_branch: bool = True
        self._error: str = ""

    def compose(self) -> ComposeResult:
        """Compose the modal UI."""
        with Vertical(classes="modal-container"):
            yield Label(" Add Worktree", classes="modal-title")
            yield Label(f"to {self._repository.name}", classes="label-secondary")

            yield Label("Worktree Name", classes="section-header")
            yield Input(
                placeholder="my-feature",
                id="input-name",
            )

            yield Label("", id="label-path-preview", classes="label-muted")

            yield Label("Branch", classes="section-header")
            with Horizontal(classes="action-row"):
                yield Button(
                    " New Branch",
                    id="btn-new-branch",
                    variant="primary",
                )
                yield Button(
                    " Existing",
                    id="btn-existing-branch",
                    variant="default",
                )

            yield Input(
                placeholder=f"{self._branch_prefix}my-feature",
                id="input-branch",
            )

            yield Select(
                [(b, b) for b in self._branches],
                id="select-branch",
                prompt="Select a branch...",
            )

            yield Label("", id="label-error", classes="label-destructive")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Create Worktree", id="btn-create", variant="primary")

    def on_mount(self) -> None:
        """Set up initial state."""
        # Hide the select by default
        self.query_one("#select-branch", Select).display = False

    def on_input_changed(self, event: Input.Changed) -> None:
        """Handle input changes."""
        if event.input.id == "input-name":
            self._name = self._sanitize_name(event.value)
            self._update_path_preview()
            if self._new_branch:
                # Auto-populate branch name
                branch_input = self.query_one("#input-branch", Input)
                branch_input.value = f"{self._branch_prefix}{self._name}"
                self._branch = branch_input.value
        elif event.input.id == "input-branch":
            self._branch = event.value

    def on_select_changed(self, event: Select.Changed) -> None:
        """Handle select changes."""
        if event.select.id == "select-branch" and event.value:
            self._branch = str(event.value)

    def _sanitize_name(self, name: str) -> str:
        """Sanitize worktree name to valid characters."""
        return "".join(c for c in name if c.isalnum() or c in "-_")

    def _update_path_preview(self) -> None:
        """Update the path preview label."""
        preview = self.query_one("#label-path-preview", Label)
        if self._name:
            path = self._forest_dir / self._repository.name / self._name
            preview.update(f" {path}")
        else:
            preview.update("")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        match event.button.id:
            case "btn-cancel":
                self.dismiss(None)
            case "btn-create":
                self._create_worktree()
            case "btn-new-branch":
                self._set_new_branch_mode(True)
            case "btn-existing-branch":
                self._set_new_branch_mode(False)

    def _set_new_branch_mode(self, new_branch: bool) -> None:
        """Switch between new branch and existing branch modes."""
        self._new_branch = new_branch

        # Update button styles
        new_btn = self.query_one("#btn-new-branch", Button)
        existing_btn = self.query_one("#btn-existing-branch", Button)
        branch_input = self.query_one("#input-branch", Input)
        branch_select = self.query_one("#select-branch", Select)

        if new_branch:
            new_btn.variant = "primary"
            existing_btn.variant = "default"
            branch_input.display = True
            branch_select.display = False
        else:
            new_btn.variant = "default"
            existing_btn.variant = "primary"
            branch_input.display = False
            branch_select.display = True

    def _create_worktree(self) -> None:
        """Create the worktree if valid."""
        error_label = self.query_one("#label-error", Label)

        if not self._name:
            error_label.update(" Name is required")
            return

        if not self._branch:
            error_label.update(" Branch is required")
            return

        # Check if worktree path already exists
        path = self._forest_dir / self._repository.name / self._name
        if path.exists():
            error_label.update(" Worktree path already exists")
            return

        self.post_message(
            self.WorktreeCreated(
                self._repository.id, self._name, self._branch, self._new_branch
            )
        )
        self.dismiss((self._name, self._branch, self._new_branch))

    def action_cancel(self) -> None:
        """Cancel and close the modal."""
        self.dismiss(None)


class SettingsModal(ModalScreen[Settings | None]):
    """Modal for editing settings."""

    BINDINGS = [
        ("escape", "cancel", "Cancel"),
    ]

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

    THEMES = [
        ("System", "system"),
        ("Dark", "dark"),
        ("Light", "light"),
    ]

    def __init__(self, settings: Settings) -> None:
        super().__init__()
        self._settings = settings

    def compose(self) -> ComposeResult:
        """Compose the modal UI."""
        with Vertical(classes="modal-container"):
            yield Label(" Settings", classes="modal-title")

            with VerticalScroll(classes="modal-scroll"):
                yield Label("DEFAULT EDITOR", classes="section-header")
                yield Select(
                    [(name, cmd) for name, cmd in self.EDITORS],
                    value=self._settings.default_editor,
                    id="select-editor",
                )

                yield Label("BRANCH PREFIX", classes="section-header")
                yield Input(
                    value=self._settings.branch_prefix,
                    id="input-branch-prefix",
                    placeholder="feat/",
                )

                yield Label("THEME", classes="section-header")
                yield Select(
                    [(name, value) for name, value in self.THEMES],
                    value=self._settings.theme,
                    id="select-theme",
                )

                yield Label("CUSTOM CLAUDE COMMAND", classes="section-header")
                yield Input(
                    value=self._settings.custom_claude_command or "",
                    id="input-custom-claude-command",
                    placeholder="e.g., claude --model opus",
                    max_length=MAX_CLAUDE_COMMAND_LENGTH,
                )
                yield Label(
                    "Default command for all repos",
                    classes="label-muted",
                )
                yield Label(
                    "Can be overridden per-repo",
                    classes="label-muted",
                )
                yield Label("", id="label-claude-error", classes="label-destructive")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Save", id="btn-save", variant="primary")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        if event.button.id == "btn-cancel":
            self.dismiss(None)
        elif event.button.id == "btn-save":
            self._save_settings()

    def _save_settings(self) -> None:
        """Save the settings."""
        editor_select = self.query_one("#select-editor", Select)
        editor = str(editor_select.value) if editor_select.value else "code"
        branch_prefix = self.query_one("#input-branch-prefix", Input).value
        theme_select = self.query_one("#select-theme", Select)
        theme = str(theme_select.value) if theme_select.value else "system"
        custom_claude_command = (
            self.query_one("#input-custom-claude-command", Input).value.strip() or None
        )

        # Validate custom claude command
        error_label = self.query_one("#label-claude-error", Label)
        if custom_claude_command:
            error = validate_claude_command(custom_claude_command)
            if error:
                error_label.update(f" {error}")
                return
        error_label.update("")

        new_settings = Settings(
            default_editor=editor,
            branch_prefix=branch_prefix,
            theme=theme,
            custom_claude_command=custom_claude_command,
        )

        self.dismiss(new_settings)

    def action_cancel(self) -> None:
        """Cancel and close the modal."""
        self.dismiss(None)


class ConfirmDeleteModal(ModalScreen[bool]):
    """Modal for confirming deletion."""

    BINDINGS = [
        ("escape", "cancel", "Cancel"),
    ]

    def __init__(self, title: str, message: str) -> None:
        super().__init__()
        self._title = title
        self._message = message

    def compose(self) -> ComposeResult:
        """Compose the modal UI."""
        with Vertical(classes="modal-container"):
            yield Label(f" {self._title}", classes="modal-title label-destructive")
            yield Label(self._message, classes="label-secondary")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button(
                    "Delete", id="btn-delete", variant="error", classes="-destructive"
                )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        if event.button.id == "btn-cancel":
            self.dismiss(False)
        elif event.button.id == "btn-delete":
            self.dismiss(True)

    def action_cancel(self) -> None:
        """Cancel and close the modal."""
        self.dismiss(False)


class CreateWorktreeFromIssueModal(ModalScreen[tuple[str, str, bool, bool] | None]):
    """Modal for creating a worktree from a GitHub issue."""

    BINDINGS = [("escape", "cancel", "Cancel")]

    class WorktreeCreated(Message):
        """Worktree creation requested."""

        def __init__(
            self,
            repo_id: UUID,
            name: str,
            branch: str,
            new_branch: bool,
            pull_first: bool,
        ) -> None:
            self.repo_id = repo_id
            self.name = name
            self.branch = branch
            self.new_branch = new_branch
            self.pull_first = pull_first
            super().__init__()

    def __init__(
        self,
        repository: Repository,
        issue: GitHubIssue,
        branches: list[str],
        forest_dir: Path,
        branch_prefix: str = "feat/",
    ) -> None:
        super().__init__()
        self._repository = repository
        self._issue = issue
        self._branches = branches
        self._forest_dir = forest_dir
        self._branch_prefix = branch_prefix
        # Pre-fill from issue
        self._name: str = issue.branch_name
        self._branch: str = f"{branch_prefix}{issue.branch_name}"
        self._pull_first: bool = True

    def compose(self) -> ComposeResult:
        """Compose the modal UI."""
        with Vertical(classes="modal-container"):
            yield Label(
                f"Create Worktree from Issue #{self._issue.number}",
                classes="modal-title",
            )
            yield Label(self._issue.title, classes="issue-title-preview label-muted")

            yield Label("Worktree Name", classes="section-header")
            yield Input(value=self._name, id="input-name", placeholder="worktree-name")

            path_preview = self._forest_dir / self._repository.name / self._name
            yield Label(
                f"Path: {path_preview}", id="path-preview", classes="label-muted"
            )

            yield Label("Branch Name", classes="section-header")
            yield Input(
                value=self._branch, id="input-branch", placeholder="feat/branch-name"
            )

            yield Checkbox("Pull repo before creating", value=True, id="checkbox-pull")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Create", id="btn-create", variant="primary")

    def on_input_changed(self, event: Input.Changed) -> None:
        """Handle input changes."""
        if event.input.id == "input-name":
            self._name = event.value
            path_preview = self._forest_dir / self._repository.name / self._name
            self.query_one("#path-preview", Label).update(f"Path: {path_preview}")
        elif event.input.id == "input-branch":
            self._branch = event.value

    def on_checkbox_changed(self, event: Checkbox.Changed) -> None:
        """Handle checkbox changes."""
        if event.checkbox.id == "checkbox-pull":
            self._pull_first = event.value

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        if event.button.id == "btn-cancel":
            self.dismiss(None)
        elif event.button.id == "btn-create" and self._name and self._branch:
            self.post_message(
                self.WorktreeCreated(
                    self._repository.id,
                    self._name,
                    self._branch,
                    True,
                    self._pull_first,
                )
            )
            self.dismiss((self._name, self._branch, True, self._pull_first))

    def action_cancel(self) -> None:
        """Cancel and close the modal."""
        self.dismiss(None)


class ClaudeCommandModal(ModalScreen[ClaudeCommandResult]):
    """Modal for editing custom Claude command for a repository or worktree."""

    BINDINGS = [
        ("escape", "cancel", "Cancel"),
    ]

    # Common command presets
    PRESETS = [
        ("claude", "claude"),
        ("opus", "claude --model opus"),
        ("sonnet", "claude --model sonnet"),
    ]

    def __init__(
        self,
        name: str,
        current_command: str | None,
        *,
        is_worktree: bool = False,
    ) -> None:
        super().__init__()
        self._name = name
        self._current_command = current_command
        self._is_worktree = is_worktree

    def compose(self) -> ComposeResult:
        """Compose the modal UI."""
        with Vertical(classes="modal-container"):
            yield Label(" Custom Claude Command", classes="modal-title")
            yield Label(f"for {self._name}", classes="label-secondary")

            yield Label("PRESETS", classes="section-header")
            with Horizontal(classes="action-row"):
                for label, _cmd in self.PRESETS:
                    yield Button(label, id=f"btn-preset-{label}", variant="default")

            yield Label("COMMAND", classes="section-header")
            yield Input(
                value=self._current_command or "",
                id="input-claude-command",
                placeholder="e.g., claude --model opus",
                max_length=MAX_CLAUDE_COMMAND_LENGTH,
            )
            fallback = "repo default" if self._is_worktree else "folder default"
            yield Label(
                f"Leave empty to use {fallback}",
                classes="label-muted",
            )
            yield Label("", id="label-error", classes="label-destructive")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Save", id="btn-save", variant="primary")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        btn_id = event.button.id or ""

        if btn_id == "btn-cancel":
            self.dismiss(ClaudeCommandResult(command=None, cancelled=True))
        elif btn_id == "btn-save":
            self._save_command()
        elif btn_id.startswith("btn-preset-"):
            # Handle preset buttons
            preset_label = btn_id.replace("btn-preset-", "")
            for label, cmd in self.PRESETS:
                if label == preset_label:
                    self.query_one("#input-claude-command", Input).value = cmd
                    break

    def on_input_submitted(self, event: Input.Submitted) -> None:
        """Handle enter key in input."""
        if event.input.id == "input-claude-command":
            self._save_command()

    def _save_command(self) -> None:
        """Save the custom command."""
        command = self.query_one("#input-claude-command", Input).value.strip()
        error_label = self.query_one("#label-error", Label)

        error = validate_claude_command(command)
        if error:
            error_label.update(f" {error}")
            return

        # Return None command to clear the setting, or the command string
        self.dismiss(ClaudeCommandResult(command=command if command else None))

    def action_cancel(self) -> None:
        """Cancel and close the modal."""
        self.dismiss(ClaudeCommandResult(command=None, cancelled=True))
