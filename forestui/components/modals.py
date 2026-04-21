"""Modal dialog components for forestui."""

from pathlib import Path
from uuid import UUID

from textual import work
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.message import Message
from textual.screen import ModalScreen
from textual.timer import Timer
from textual.widget import Widget
from textual.widgets import Button, Checkbox, Input, Label, Select

from forestui.components.branch_search import (
    BranchSearchInput,
    FuzzyBranchSuggester,
)
from forestui.models import (
    MAX_BUTTON_LABEL_LENGTH,
    MAX_BUTTON_PREFIX_LENGTH,
    MAX_CLAUDE_COMMAND_LENGTH,
    CustomClaudeButton,
    GitHubIssue,
    Repository,
    Settings,
    derive_prefix,
    validate_button_label,
    validate_button_prefix,
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
        remotes: list[str] | None = None,
    ) -> None:
        super().__init__()
        self._repository = repository
        self._branches = branches
        self._remotes = remotes or []
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

            yield BranchSearchInput(
                self._branches,
                remotes=self._remotes,
                widget_id="branch-search",
            )

            yield Label("", id="label-error", classes="label-destructive")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Create Worktree", id="btn-create", variant="primary")

    def on_mount(self) -> None:
        """Set up initial state."""
        # Hide the branch search widget by default (new branch mode)
        self.query_one("#branch-search", BranchSearchInput).display = False

    def on_input_changed(self, event: Input.Changed) -> None:
        """Handle input changes."""
        # Clear any error when user types
        self._clear_error()

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

        self._update_create_button_state()

    def on_branch_search_input_changed(self, event: BranchSearchInput.Changed) -> None:
        """Handle branch search input changes."""
        self._clear_error()
        self._branch = event.value
        self._update_create_button_state()

    def _clear_error(self) -> None:
        """Clear the error label."""
        self.query_one("#label-error", Label).update("")

    def _update_create_button_state(self) -> None:
        """Enable/disable Create button based on validation."""
        btn = self.query_one("#btn-create", Button)
        # For existing branch mode, branch must be in the list
        if not self._new_branch and self._branch not in self._branches:
            btn.disabled = True
        else:
            btn.disabled = False

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
        branch_search = self.query_one("#branch-search", BranchSearchInput)

        if new_branch:
            new_btn.variant = "primary"
            existing_btn.variant = "default"
            branch_input.display = True
            branch_search.display = False
        else:
            new_btn.variant = "default"
            existing_btn.variant = "primary"
            branch_input.display = False
            branch_search.display = True

        self._update_create_button_state()

    def _create_worktree(self) -> None:
        """Create the worktree if valid."""
        error_label = self.query_one("#label-error", Label)

        if not self._name:
            error_label.update(" Worktree name is required")
            return

        if not self._branch:
            error_label.update(" Branch name is required")
            return

        # For existing branch mode, validate the branch exists
        if not self._new_branch and self._branch not in self._branches:
            error_label.update(f" Branch '{self._branch}' does not exist")
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
        self._custom_buttons: list[CustomClaudeButton] = list(settings.custom_buttons)

    def compose(self) -> ComposeResult:
        """Compose the modal UI."""
        with Vertical(classes="modal-container modal-wide"):
            yield Label(" Settings", classes="modal-title")

            with VerticalScroll(classes="modal-scroll modal-scroll-tall"):
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

                yield Label("CUSTOM CLAUDE BUTTONS", classes="section-header")
                yield Label(
                    self._buttons_summary(),
                    id="label-buttons-count",
                    classes="label-muted",
                )
                with Horizontal(classes="action-row"):
                    yield Button(
                        "Manage Custom Buttons...",
                        id="btn-manage-buttons",
                        variant="default",
                    )

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Save", id="btn-save", variant="primary")

    def _buttons_summary(self) -> str:
        count = len(self._custom_buttons)
        if count == 0:
            return "No custom buttons configured"
        if count == 1:
            return "1 custom button configured"
        return f"{count} custom buttons configured"

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        if event.button.id == "btn-cancel":
            self.dismiss(None)
        elif event.button.id == "btn-save":
            self._save_settings()
        elif event.button.id == "btn-manage-buttons":
            self._manage_buttons()

    @work
    async def _manage_buttons(self) -> None:
        """Open the custom-buttons sub-modal and store the returned list."""
        result = await self.app.push_screen_wait(
            CustomButtonsModal(self._custom_buttons)
        )
        if result is not None:
            self._custom_buttons = result
            self.query_one("#label-buttons-count", Label).update(
                self._buttons_summary()
            )

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
            custom_buttons=self._custom_buttons,
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
            base_branch: str | None = None,
        ) -> None:
            self.repo_id = repo_id
            self.name = name
            self.branch = branch
            self.new_branch = new_branch
            self.pull_first = pull_first
            self.base_branch = base_branch
            super().__init__()

    class FetchRequested(Message):
        """Request to fetch from remote."""

        def __init__(self, repo_path: str) -> None:
            self.repo_path = repo_path
            super().__init__()

    def __init__(
        self,
        repository: Repository,
        issue: GitHubIssue,
        branches: list[str],
        forest_dir: Path,
        branch_prefix: str = "feat/",
        current_branch: str = "main",
        remotes: list[str] | None = None,
    ) -> None:
        super().__init__()
        self._repository = repository
        self._issue = issue
        self._branches = branches
        self._remotes = remotes or []
        self._forest_dir = forest_dir
        self._branch_prefix = branch_prefix
        self._current_branch = current_branch
        # Pre-fill from issue
        self._name: str = issue.branch_name
        self._branch: str = f"{branch_prefix}{issue.branch_name}"
        self._pull_first: bool = True
        # Default base branch: prefer origin/<current> if available
        self._base_branch: str = self._compute_default_base_branch()
        self._is_fetching: bool = False
        # Spinner animation
        self._spinner_chars = "|/-\\"
        self._spinner_index = 0
        self._spinner_timer: Timer | None = None

    def _compute_default_base_branch(self) -> str:
        """Compute the default base branch, preferring remote version."""
        # Check each remote for <remote>/<current_branch>
        for remote in self._remotes:
            remote_branch = f"{remote}/{self._current_branch}"
            if remote_branch in self._branches:
                return remote_branch
        # Fall back to local current branch
        if self._current_branch in self._branches:
            return self._current_branch
        # Last resort: first branch in list or empty
        return self._branches[0] if self._branches else ""

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

            yield Label("Base Branch", classes="section-header")
            with Horizontal(classes="base-branch-row"):
                yield Input(
                    value=self._base_branch,
                    id="input-base-branch",
                    placeholder="origin/main",
                    suggester=FuzzyBranchSuggester(
                        self._branches, remotes=self._remotes
                    ),
                )
                yield Button("Fetch", id="btn-fetch", variant="default")

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
        elif event.input.id == "input-base-branch":
            self._base_branch = event.value
            self._update_create_button_state()

    def _update_create_button_state(self) -> None:
        """Enable/disable Create button based on base branch validation."""
        btn = self.query_one("#btn-create", Button)
        # Base branch must be in the list
        if self._base_branch and self._base_branch not in self._branches:
            btn.disabled = True
        else:
            btn.disabled = False

    def on_checkbox_changed(self, event: Checkbox.Changed) -> None:
        """Handle checkbox changes."""
        if event.checkbox.id == "checkbox-pull":
            self._pull_first = event.value

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        if event.button.id == "btn-cancel":
            self.dismiss(None)
        elif event.button.id == "btn-fetch":
            self._start_fetch()
        elif event.button.id == "btn-create" and self._name and self._branch:
            self.post_message(
                self.WorktreeCreated(
                    self._repository.id,
                    self._name,
                    self._branch,
                    True,
                    self._pull_first,
                    self._base_branch,
                )
            )
            self.dismiss((self._name, self._branch, True, self._pull_first))

    def _start_fetch(self) -> None:
        """Start fetching from remote."""
        if self._is_fetching:
            return
        self._is_fetching = True
        self._spinner_index = 0
        try:
            btn = self.query_one("#btn-fetch", Button)
            btn.label = self._spinner_chars[0]
            btn.disabled = True
            self._spinner_timer = self.set_interval(0.1, self._tick_spinner)
        except Exception:
            pass
        self.post_message(self.FetchRequested(self._repository.source_path))

    def _tick_spinner(self) -> None:
        """Advance the spinner animation."""
        self._spinner_index = (self._spinner_index + 1) % len(self._spinner_chars)
        try:
            btn = self.query_one("#btn-fetch", Button)
            btn.label = self._spinner_chars[self._spinner_index]
        except Exception:
            self._stop_spinner()

    def update_branches(
        self, branches: list[str], remotes: list[str] | None = None
    ) -> None:
        """Update the branch list after fetch."""
        self._is_fetching = False
        self._branches = branches
        if remotes is not None:
            self._remotes = remotes
        self._reset_fetch_button()
        try:
            input_widget = self.query_one("#input-base-branch", Input)
            # Update the suggester with new branches
            input_widget.suggester = FuzzyBranchSuggester(
                branches, remotes=self._remotes
            )
            # If current value is empty or not in new list, set to computed default
            if not self._base_branch or self._base_branch not in branches:
                self._base_branch = self._compute_default_base_branch()
                input_widget.value = self._base_branch
            self._update_create_button_state()
        except Exception:
            pass

    def fetch_failed(self, _error: str) -> None:
        """Handle fetch failure - error is shown via app notification."""
        self._is_fetching = False
        self._reset_fetch_button()

    def _stop_spinner(self) -> None:
        """Stop the spinner animation."""
        if self._spinner_timer is not None:
            self._spinner_timer.stop()
            self._spinner_timer = None

    def _reset_fetch_button(self) -> None:
        """Reset the fetch button to its default state."""
        self._stop_spinner()
        try:
            btn = self.query_one("#btn-fetch", Button)
            btn.label = "Fetch"
            btn.disabled = False
        except Exception:
            pass

    def action_cancel(self) -> None:
        """Cancel and close the modal."""
        self.dismiss(None)


class CustomButtonEditModal(ModalScreen[CustomClaudeButton | None]):
    """Modal for adding or editing a single CustomClaudeButton."""

    BINDINGS = [("escape", "cancel", "Cancel")]

    def __init__(
        self,
        existing: CustomClaudeButton | None,
        other_labels: set[str],
        other_prefixes: set[str],
    ) -> None:
        super().__init__()
        self._existing = existing
        self._other_labels = other_labels
        self._other_prefixes = other_prefixes
        # Track whether prefix should auto-follow the label
        if existing is None:
            self._follows = True
        else:
            self._follows = existing.prefix == derive_prefix(existing.label)
        self._suppress_prefix_event = False

    def compose(self) -> ComposeResult:
        title = "Edit Button" if self._existing else "Add Button"
        label_val = self._existing.label if self._existing else ""
        prefix_val = self._existing.prefix if self._existing else ""
        cmd_val = self._existing.command if self._existing else ""

        with Vertical(classes="modal-container"):
            yield Label(f" {title}", classes="modal-title")

            yield Label("LABEL", classes="section-header")
            yield Input(
                value=label_val,
                id="input-label",
                placeholder="e.g., YoloDisc",
                max_length=MAX_BUTTON_LABEL_LENGTH,
            )
            yield Label(
                "Shown on the button (e.g., 'New Session: YoloDisc')",
                classes="label-muted",
            )

            yield Label("TMUX PREFIX", classes="section-header")
            yield Input(
                value=prefix_val,
                id="input-prefix",
                placeholder="e.g., yolodisc",
                max_length=MAX_BUTTON_PREFIX_LENGTH,
            )
            yield Label(
                "Window prefix: <prefix>:<worktree>. Auto-derived from label "
                "until you edit it.",
                classes="label-muted",
            )

            yield Label("COMMAND", classes="section-header")
            yield Input(
                value=cmd_val,
                id="input-command",
                placeholder="e.g., claude --dangerously-skip-permissions",
                max_length=MAX_CLAUDE_COMMAND_LENGTH,
            )
            yield Label(
                "Run as-is. If it contains --dangerously-skip-permissions "
                "the button is styled red.",
                classes="label-muted",
            )

            yield Label("", id="label-edit-error", classes="label-destructive")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Save", id="btn-save", variant="primary")

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "input-label":
            if self._follows:
                self._suppress_prefix_event = True
                self.query_one("#input-prefix", Input).value = derive_prefix(
                    event.value
                )
                self._suppress_prefix_event = False
        elif event.input.id == "input-prefix":
            if self._suppress_prefix_event:
                return
            label = self.query_one("#input-label", Input).value
            self._follows = event.value == derive_prefix(label)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn-cancel":
            self.dismiss(None)
        elif event.button.id == "btn-save":
            self._save()

    def _save(self) -> None:
        label = self.query_one("#input-label", Input).value.strip()
        prefix = self.query_one("#input-prefix", Input).value.strip()
        command = self.query_one("#input-command", Input).value.strip()
        err_label = self.query_one("#label-edit-error", Label)

        for error in (
            validate_button_label(label),
            validate_button_prefix(prefix),
            validate_claude_command(command) if command else "Command cannot be empty",
        ):
            if error:
                err_label.update(f" {error}")
                return

        if label in self._other_labels:
            err_label.update(" Another button already uses this label")
            return
        if prefix in self._other_prefixes:
            err_label.update(" Another button already uses this prefix")
            return

        err_label.update("")
        self.dismiss(CustomClaudeButton(label=label, prefix=prefix, command=command))

    def action_cancel(self) -> None:
        self.dismiss(None)


class CustomButtonsModal(ModalScreen[list[CustomClaudeButton] | None]):
    """Modal for managing (add/edit/delete/reorder) custom Claude buttons."""

    BINDINGS = [("escape", "cancel", "Cancel")]

    def __init__(self, buttons: list[CustomClaudeButton]) -> None:
        super().__init__()
        # Work on a copy so cancel discards changes
        self._buttons: list[CustomClaudeButton] = [b.model_copy() for b in buttons]

    def compose(self) -> ComposeResult:
        with Vertical(classes="modal-container modal-wide"):
            yield Label(" Custom Claude Buttons", classes="modal-title")
            yield Label(
                "Order here matches display order in the Claude section.",
                classes="label-muted",
            )

            with VerticalScroll(
                classes="modal-scroll modal-scroll-tall",
                id="buttons-list",
            ):
                yield from self._build_rows()

            with Horizontal(classes="action-row"):
                yield Button(" Add Button", id="btn-add", variant="primary")

            with Horizontal(classes="modal-buttons"):
                yield Button("Cancel", id="btn-cancel", variant="default")
                yield Button("Save", id="btn-save", variant="primary")

    def _build_rows(self) -> list[Widget]:
        """Build the concrete widget list for the buttons scroll area."""
        if not self._buttons:
            return [Label("No buttons yet. Click Add.", classes="label-muted")]
        rows: list[Widget] = []
        last = len(self._buttons) - 1
        for idx, btn in enumerate(self._buttons):
            yolo_note = " (YOLO)" if btn.is_yolo_style else ""
            info = Vertical(
                Label(f"{btn.label}{yolo_note}", classes="session-title"),
                Label(f"prefix: {btn.prefix}", classes="session-meta label-muted"),
                Label(f"$ {btn.command}", classes="session-meta label-muted"),
                classes="session-info",
            )
            buttons = Horizontal(
                Button(
                    "↑",
                    id=f"btn-up-{idx}",
                    classes="session-btn",
                    disabled=idx == 0,
                ),
                Button(
                    "↓",
                    id=f"btn-down-{idx}",
                    classes="session-btn",
                    disabled=idx == last,
                ),
                Button("Edit", id=f"btn-edit-{idx}", classes="session-btn"),
                Button(
                    "Delete",
                    id=f"btn-delete-{idx}",
                    classes="session-btn -destructive",
                    variant="error",
                ),
                classes="session-buttons",
            )
            rows.append(
                Vertical(
                    Horizontal(info, buttons, classes="session-header-row"),
                    classes="session-item",
                )
            )
        return rows

    def _rerender(self) -> None:
        """Rebuild the list container after add/edit/delete/reorder."""
        container = self.query_one("#buttons-list", VerticalScroll)
        container.remove_children()
        container.mount_all(self._build_rows())

    def on_button_pressed(self, event: Button.Pressed) -> None:
        btn_id = event.button.id or ""
        if btn_id == "btn-cancel":
            self.dismiss(None)
        elif btn_id == "btn-save":
            self.dismiss(self._buttons)
        elif btn_id == "btn-add":
            self._add_button()
        elif btn_id.startswith("btn-up-"):
            self._swap(int(btn_id.removeprefix("btn-up-")), -1)
        elif btn_id.startswith("btn-down-"):
            self._swap(int(btn_id.removeprefix("btn-down-")), 1)
        elif btn_id.startswith("btn-edit-"):
            self._edit_button(int(btn_id.removeprefix("btn-edit-")))
        elif btn_id.startswith("btn-delete-"):
            idx = int(btn_id.removeprefix("btn-delete-"))
            self._buttons.pop(idx)
            self._rerender()

    def _swap(self, idx: int, delta: int) -> None:
        j = idx + delta
        if 0 <= j < len(self._buttons):
            self._buttons[idx], self._buttons[j] = self._buttons[j], self._buttons[idx]
            self._rerender()

    @work
    async def _add_button(self) -> None:
        result = await self.app.push_screen_wait(
            CustomButtonEditModal(
                existing=None,
                other_labels={b.label for b in self._buttons},
                other_prefixes={b.prefix for b in self._buttons},
            )
        )
        if result is not None:
            self._buttons.append(result)
            self._rerender()

    @work
    async def _edit_button(self, idx: int) -> None:
        existing = self._buttons[idx]
        others = [b for i, b in enumerate(self._buttons) if i != idx]
        result = await self.app.push_screen_wait(
            CustomButtonEditModal(
                existing=existing,
                other_labels={b.label for b in others},
                other_prefixes={b.prefix for b in others},
            )
        )
        if result is not None:
            self._buttons[idx] = result
            self._rerender()

    def action_cancel(self) -> None:
        self.dismiss(None)
