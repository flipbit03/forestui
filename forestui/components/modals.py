"""Modal dialog components for forestui."""

from pathlib import Path
from uuid import UUID

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.message import Message
from textual.screen import ModalScreen
from textual.widgets import Button, Checkbox, Input, Label, Select

from forestui.models import Repository, Settings


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

            yield Label("", id="label-error", classes="label-destructive")
            yield Label("", id="label-name", classes="label-secondary")

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
        error_label = self.query_one("#label-error", Label)
        name_label = self.query_one("#label-name", Label)

        if not self._path:
            error_label.update("")
            name_label.update("")
            return

        path = Path(self._path).expanduser()

        if not path.exists():
            error_label.update(" Path does not exist")
            name_label.update("")
            return

        if not path.is_dir():
            error_label.update(" Path is not a directory")
            name_label.update("")
            return

        # Check if it's a git repository
        git_dir = path / ".git"
        if not git_dir.exists():
            error_label.update(" Not a git repository")
            name_label.update("")
            return

        error_label.update("")
        name_label.update(f"Repository: {path.name}")

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

        new_settings = Settings(
            default_editor=editor,
            branch_prefix=branch_prefix,
            theme=theme,
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
