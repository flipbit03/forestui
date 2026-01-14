"""Worktree detail view component."""

from uuid import UUID

from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.message import Message
from textual.widgets import Button, Input, Label, Rule, Static

from forestui.models import ClaudeSession, Repository, Worktree


class WorktreeDetail(Static):
    """Detail view for a selected worktree."""

    class OpenInEditor(Message):
        """Request to open worktree in editor."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class OpenInTerminal(Message):
        """Request to open worktree in terminal."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class OpenInFileManager(Message):
        """Request to open worktree in file manager."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class StartClaudeSession(Message):
        """Request to start a new Claude session."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class ContinueClaudeSession(Message):
        """Request to continue an existing Claude session."""

        def __init__(self, session_id: str, path: str) -> None:
            self.session_id = session_id
            self.path = path
            super().__init__()

    class ArchiveWorktreeRequested(Message):
        """Request to archive the worktree."""

        def __init__(self, worktree_id: UUID) -> None:
            self.worktree_id = worktree_id
            super().__init__()

    class UnarchiveWorktreeRequested(Message):
        """Request to unarchive the worktree."""

        def __init__(self, worktree_id: UUID) -> None:
            self.worktree_id = worktree_id
            super().__init__()

    class DeleteWorktreeRequested(Message):
        """Request to delete the worktree."""

        def __init__(self, repo_id: UUID, worktree_id: UUID) -> None:
            self.repo_id = repo_id
            self.worktree_id = worktree_id
            super().__init__()

    class RenameWorktreeRequested(Message):
        """Request to rename the worktree."""

        def __init__(self, worktree_id: UUID, new_name: str) -> None:
            self.worktree_id = worktree_id
            self.new_name = new_name
            super().__init__()

    class RenameBranchRequested(Message):
        """Request to rename the branch."""

        def __init__(self, worktree_id: UUID, new_branch: str) -> None:
            self.worktree_id = worktree_id
            self.new_branch = new_branch
            super().__init__()

    def __init__(
        self,
        repository: Repository,
        worktree: Worktree,
        sessions: list[ClaudeSession] | None = None,
    ) -> None:
        super().__init__()
        self._repository = repository
        self._worktree = worktree
        self._sessions = sessions or []

    def compose(self) -> ComposeResult:
        """Compose the worktree detail view."""
        with Vertical(classes="detail-content"):
            # Header
            with Vertical(classes="detail-header"):
                yield Label(
                    f"  {self._worktree.name}",
                    classes="detail-title",
                )
                yield Label(
                    f"   {self._worktree.branch}",
                    classes="detail-subtitle label-accent",
                )
                yield Label(
                    f"in {self._repository.name}",
                    classes="label-muted",
                )

            yield Rule()

            # Location section
            yield Label("LOCATION", classes="section-header")
            yield Label(
                f" {self._worktree.path}",
                classes="path-display label-secondary",
            )

            yield Rule()

            # Actions section
            yield Label("OPEN IN", classes="section-header")
            with Horizontal(classes="action-row"):
                yield Button(" Editor", id="btn-editor", variant="default")
                yield Button(" Terminal", id="btn-terminal", variant="default")
                yield Button(" Files", id="btn-files", variant="default")

            yield Rule()

            # Claude section
            yield Label("CLAUDE", classes="section-header")
            with Horizontal(classes="action-row"):
                yield Button("󰚩 New Session", id="btn-claude-new", variant="primary")

            # Sessions list
            if self._sessions:
                yield Label("RECENT SESSIONS", classes="section-header")
                for session in self._sessions[:5]:
                    with Vertical(classes="session-item", id=f"session-{session.id}"):
                        yield Label(
                            session.title[:50]
                            + ("..." if len(session.title) > 50 else ""),
                            classes="session-title",
                        )
                        meta = f"{session.relative_time} • {session.message_count} messages"
                        if session.primary_branch:
                            meta += f" •  {session.primary_branch}"
                        yield Label(meta, classes="session-meta label-muted")

            yield Rule()

            # Rename section
            yield Label("RENAME", classes="section-header")
            with Horizontal(classes="action-row"):
                yield Input(
                    value=self._worktree.name,
                    placeholder="Worktree name",
                    id="input-worktree-name",
                )
            with Horizontal(classes="action-row"):
                yield Input(
                    value=self._worktree.branch,
                    placeholder="Branch name",
                    id="input-branch-name",
                )

            yield Rule()

            # Manage section
            yield Label("MANAGE", classes="section-header")
            with Horizontal(classes="action-row"):
                if self._worktree.is_archived:
                    yield Button(
                        " Unarchive",
                        id="btn-unarchive",
                        variant="default",
                    )
                else:
                    yield Button(
                        " Archive",
                        id="btn-archive",
                        variant="default",
                    )
                yield Button(
                    " Delete",
                    id="btn-delete",
                    variant="error",
                    classes="-destructive",
                )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        path = self._worktree.path

        match event.button.id:
            case "btn-editor":
                self.post_message(self.OpenInEditor(path))
            case "btn-terminal":
                self.post_message(self.OpenInTerminal(path))
            case "btn-files":
                self.post_message(self.OpenInFileManager(path))
            case "btn-claude-new":
                self.post_message(self.StartClaudeSession(path))
            case "btn-archive":
                self.post_message(self.ArchiveWorktreeRequested(self._worktree.id))
            case "btn-unarchive":
                self.post_message(self.UnarchiveWorktreeRequested(self._worktree.id))
            case "btn-delete":
                self.post_message(
                    self.DeleteWorktreeRequested(self._repository.id, self._worktree.id)
                )

    def on_input_submitted(self, event: Input.Submitted) -> None:
        """Handle input submission."""
        match event.input.id:
            case "input-worktree-name":
                if event.value and event.value != self._worktree.name:
                    self.post_message(
                        self.RenameWorktreeRequested(self._worktree.id, event.value)
                    )
            case "input-branch-name":
                if event.value and event.value != self._worktree.branch:
                    self.post_message(
                        self.RenameBranchRequested(self._worktree.id, event.value)
                    )

    def on_click(self, event: object) -> None:
        """Handle clicks on session items."""
        widget = getattr(event, "widget", None)
        while widget:
            if hasattr(widget, "id") and widget.id and widget.id.startswith("session-"):
                session_id = widget.id.replace("session-", "")
                self.post_message(
                    self.ContinueClaudeSession(session_id, self._worktree.path)
                )
                return
            widget = widget.parent
