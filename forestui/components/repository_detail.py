"""Repository detail view component."""

from datetime import datetime
from uuid import UUID

import humanize
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.message import Message
from textual.widget import Widget
from textual.widgets import Button, Label, Rule

from forestui.models import ClaudeSession, Repository


class RepositoryDetail(Widget):
    """Detail view for a selected repository."""

    class OpenInEditor(Message):
        """Request to open repository in editor."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class OpenInTerminal(Message):
        """Request to open repository in terminal."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class OpenInFileManager(Message):
        """Request to open repository in file manager."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class StartClaudeSession(Message):
        """Request to start a new Claude session."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class StartClaudeYoloSession(Message):
        """Request to start a Claude YOLO session."""

        def __init__(self, path: str) -> None:
            self.path = path
            super().__init__()

    class ContinueClaudeSession(Message):
        """Request to continue an existing Claude session."""

        def __init__(self, session_id: str, path: str) -> None:
            self.session_id = session_id
            self.path = path
            super().__init__()

    class ContinueClaudeYoloSession(Message):
        """Request to continue an existing Claude session in YOLO mode."""

        def __init__(self, session_id: str, path: str) -> None:
            self.session_id = session_id
            self.path = path
            super().__init__()

    class AddWorktreeRequested(Message):
        """Request to add a worktree."""

        def __init__(self, repo_id: UUID) -> None:
            self.repo_id = repo_id
            super().__init__()

    class RemoveRepositoryRequested(Message):
        """Request to remove repository."""

        def __init__(self, repo_id: UUID) -> None:
            self.repo_id = repo_id
            super().__init__()

    class SyncRequested(Message):
        """Request to sync (fetch/pull) the repository."""

        def __init__(self, repo_id: UUID, path: str) -> None:
            self.repo_id = repo_id
            self.path = path
            super().__init__()

    def __init__(
        self,
        repository: Repository,
        current_branch: str = "",
        sessions: list[ClaudeSession] | None = None,
        commit_hash: str = "",
        commit_time: datetime | None = None,
        has_remote: bool = True,
    ) -> None:
        super().__init__()
        self._repository = repository
        self._current_branch = current_branch
        self._sessions = sessions or []
        self._commit_hash = commit_hash
        self._commit_time = commit_time
        self._has_remote = has_remote

    def compose(self) -> ComposeResult:
        """Compose the repository detail view."""
        with Vertical(classes="detail-content"):
            # Header - Main Repository
            with Vertical(classes="detail-header"):
                yield Label("MAIN REPOSITORY", classes="section-header")
                yield Label(
                    f"Repository: {self._repository.name}",
                    classes="detail-title",
                )
                if self._current_branch:
                    yield Label(
                        f"Branch:     {self._current_branch}",
                        classes="label-accent",
                    )
                # Commit info
                if self._commit_hash:
                    relative_time = (
                        humanize.naturaltime(self._commit_time)
                        if self._commit_time
                        else ""
                    )
                    commit_text = f"Commit:     {self._commit_hash}"
                    if relative_time:
                        commit_text += f" ({relative_time})"
                    yield Label(commit_text, classes="label-muted")
                # Sync button
                with Horizontal(classes="action-row"):
                    if self._has_remote:
                        yield Button("⟳ Git Pull", id="btn-sync", variant="default")
                    else:
                        yield Button(
                            "⟳ Git Pull (No remote)",
                            id="btn-sync",
                            variant="default",
                            disabled=True,
                        )

            yield Rule()

            # Location section
            yield Label("LOCATION", classes="section-header")
            yield Label(
                self._repository.source_path,
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
                yield Button("New Session", id="btn-claude-new", variant="primary")
                yield Button(
                    "New Session: YOLO",
                    id="btn-claude-yolo",
                    variant="error",
                    classes="-destructive",
                )
                yield Button(" Add Worktree", id="btn-add-worktree", variant="default")

            # Sessions list
            if self._sessions:
                yield Label("RECENT SESSIONS", classes="section-header")
                for session in self._sessions[:5]:
                    with Vertical(classes="session-item"):  # noqa: SIM117
                        with Horizontal(classes="session-header-row"):
                            with Vertical(classes="session-info"):
                                # Initial message (title)
                                title_display = session.title[:60] + (
                                    "..." if len(session.title) > 60 else ""
                                )
                                yield Label(title_display, classes="session-title")
                                # Last message if different from title
                                if (
                                    session.last_message
                                    and session.last_message != session.title
                                ):
                                    last_display = session.last_message[:40] + (
                                        "..." if len(session.last_message) > 40 else ""
                                    )
                                    yield Label(
                                        f"> {last_display}",
                                        classes="session-last label-secondary",
                                    )
                                # Meta info
                                meta = f"{session.relative_time} • {session.message_count} msgs"
                                yield Label(meta, classes="session-meta label-muted")
                            with Horizontal(classes="session-buttons"):
                                yield Button(
                                    "Resume",
                                    id=f"btn-resume-{session.id}",
                                    variant="default",
                                    classes="session-btn",
                                )
                                yield Button(
                                    "YOLO",
                                    id=f"btn-yolo-{session.id}",
                                    variant="error",
                                    classes="session-btn -destructive",
                                )

            yield Rule()

            # Manage section
            yield Label("MANAGE", classes="section-header")
            with Horizontal(classes="action-row"):
                yield Button(
                    " Remove Repository",
                    id="btn-remove-repo",
                    variant="error",
                    classes="-destructive",
                )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        path = self._repository.source_path
        btn_id = event.button.id or ""

        match btn_id:
            case "btn-editor":
                self.post_message(self.OpenInEditor(path))
            case "btn-terminal":
                self.post_message(self.OpenInTerminal(path))
            case "btn-files":
                self.post_message(self.OpenInFileManager(path))
            case "btn-claude-new":
                self.post_message(self.StartClaudeSession(path))
            case "btn-claude-yolo":
                self.post_message(self.StartClaudeYoloSession(path))
            case "btn-add-worktree":
                self.post_message(self.AddWorktreeRequested(self._repository.id))
            case "btn-remove-repo":
                self.post_message(self.RemoveRepositoryRequested(self._repository.id))
            case "btn-sync":
                self.post_message(
                    self.SyncRequested(
                        self._repository.id, self._repository.source_path
                    )
                )
            case _ if btn_id.startswith("btn-resume-"):
                session_id = btn_id.replace("btn-resume-", "")
                self.post_message(self.ContinueClaudeSession(session_id, path))
            case _ if btn_id.startswith("btn-yolo-"):
                session_id = btn_id.replace("btn-yolo-", "")
                self.post_message(self.ContinueClaudeYoloSession(session_id, path))
