"""Repository detail view component."""

from datetime import datetime
from uuid import UUID

import humanize
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.message import Message
from textual.timer import Timer
from textual.widget import Widget
from textual.widgets import Button, Label, Rule

from forestui.components.messages import (
    ConfigureClaudeCommand,
    ContinueClaudeSession,
    ContinueClaudeYoloSession,
    OpenInEditor,
    OpenInFileManager,
    OpenInTerminal,
    StartClaudeSession,
    StartClaudeYoloSession,
)
from forestui.models import ClaudeSession, GitHubIssue, Repository


class RepositoryDetail(Widget):
    """Detail view for a selected repository."""

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

    class CreateWorktreeFromIssue(Message):
        """Request to create worktree from GitHub issue."""

        def __init__(self, repo_id: UUID, issue: GitHubIssue) -> None:
            self.repo_id = repo_id
            self.issue = issue
            super().__init__()

    class RefreshIssuesRequested(Message):
        """Request to refresh GitHub issues."""

        def __init__(self, repo_path: str) -> None:
            self.repo_path = repo_path
            super().__init__()

    def __init__(
        self,
        repository: Repository,
        current_branch: str = "",
        commit_hash: str = "",
        commit_time: datetime | None = None,
        has_remote: bool = True,
    ) -> None:
        super().__init__()
        self._repository = repository
        self._current_branch = current_branch
        self._commit_hash = commit_hash
        self._commit_time = commit_time
        self._has_remote = has_remote
        self._sessions: list[ClaudeSession] = []
        self._issues: list[GitHubIssue] = []
        self._issues_by_number: dict[int, GitHubIssue] = {}
        self._spinner_chars = "|/-\\"
        self._spinner_index = 0
        self._spinner_timer: Timer | None = None

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

            # Sessions list (loaded async)
            yield Label("RECENT SESSIONS", classes="section-header")
            with Vertical(id="sessions-container"):
                yield Label("Loading...", classes="label-muted")

            # GitHub Issues section (loaded async)
            yield Rule()
            with Horizontal(classes="section-header-row"):
                yield Label("MY OPEN GITHUB ISSUES", classes="section-header")
                yield Button("↻", id="btn-refresh-issues", classes="refresh-btn")
            with Vertical(id="issues-container"):
                yield Label("Loading...", classes="label-muted")

            yield Rule()

            # Manage section
            yield Label("MANAGE", classes="section-header")
            with Horizontal(classes="action-row"):
                yield Button(
                    " Custom Claude Command",
                    id="btn-configure-claude",
                    variant="default",
                )
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
                self.post_message(OpenInEditor(path))
            case "btn-terminal":
                self.post_message(OpenInTerminal(path))
            case "btn-files":
                self.post_message(OpenInFileManager(path))
            case "btn-claude-new":
                self.post_message(StartClaudeSession(path))
            case "btn-claude-yolo":
                self.post_message(StartClaudeYoloSession(path))
            case "btn-configure-claude":
                self.post_message(ConfigureClaudeCommand(self._repository.id))
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
            case "btn-refresh-issues":
                self._start_refresh_spinner()
                self.post_message(
                    self.RefreshIssuesRequested(self._repository.source_path)
                )
            case _ if btn_id.startswith("btn-resume-"):
                session_id = btn_id.replace("btn-resume-", "")
                self.post_message(ContinueClaudeSession(session_id, path))
            case _ if btn_id.startswith("btn-yolo-"):
                session_id = btn_id.replace("btn-yolo-", "")
                self.post_message(ContinueClaudeYoloSession(session_id, path))
            case _ if btn_id.startswith("btn-issue-"):
                issue_num = int(btn_id.replace("btn-issue-", ""))
                issue = self._issues_by_number.get(issue_num)
                if issue:
                    self.post_message(
                        self.CreateWorktreeFromIssue(self._repository.id, issue)
                    )

    def update_sessions(self, sessions: list[ClaudeSession]) -> None:
        """Update the sessions section with fetched sessions."""
        self._sessions = sessions

        try:
            container = self.query_one("#sessions-container", Vertical)
            container.remove_children()

            if sessions:
                for session in sessions[:5]:
                    title_display = session.title[:60] + (
                        "..." if len(session.title) > 60 else ""
                    )

                    # Build session info widgets
                    info_children: list[Label] = [
                        Label(title_display, classes="session-title")
                    ]

                    if session.last_message and session.last_message != session.title:
                        last_display = session.last_message[:40] + (
                            "..." if len(session.last_message) > 40 else ""
                        )
                        info_children.append(
                            Label(
                                f"> {last_display}",
                                classes="session-last label-secondary",
                            )
                        )

                    meta = f"{session.relative_time} • {session.message_count} msgs"
                    info_children.append(
                        Label(meta, classes="session-meta label-muted")
                    )

                    row = Vertical(
                        Horizontal(
                            Vertical(*info_children, classes="session-info"),
                            Horizontal(
                                Button(
                                    "Resume",
                                    id=f"btn-resume-{session.id}",
                                    variant="default",
                                    classes="session-btn",
                                ),
                                Button(
                                    "YOLO",
                                    id=f"btn-yolo-{session.id}",
                                    variant="error",
                                    classes="session-btn -destructive",
                                ),
                                classes="session-buttons",
                            ),
                            classes="session-header-row",
                        ),
                        classes="session-item",
                    )
                    container.mount(row)
            else:
                container.mount(Label("No sessions found", classes="label-muted"))
        except Exception:
            pass  # Widget may have been removed

    def start_issues_spinner(self) -> None:
        """Start the refresh button spinner animation (public for initial load)."""
        self._start_refresh_spinner()

    def _start_refresh_spinner(self) -> None:
        """Start the refresh button spinner animation."""
        # Don't start if already spinning
        if self._spinner_timer is not None:
            return
        self._spinner_index = 0
        try:
            btn = self.query_one("#btn-refresh-issues", Button)
            btn.label = self._spinner_chars[0]
            btn.disabled = True
            self._spinner_timer = self.set_interval(0.05, self._tick_spinner)
        except Exception:
            pass

    def _tick_spinner(self) -> None:
        """Advance the spinner animation."""
        self._spinner_index = (self._spinner_index + 1) % len(self._spinner_chars)
        try:
            btn = self.query_one("#btn-refresh-issues", Button)
            btn.label = self._spinner_chars[self._spinner_index]
        except Exception:
            self._stop_refresh_spinner()

    def _stop_refresh_spinner(self) -> None:
        """Stop the spinner and restore the refresh icon."""
        if self._spinner_timer:
            self._spinner_timer.stop()
            self._spinner_timer = None
        try:
            btn = self.query_one("#btn-refresh-issues", Button)
            btn.label = "↻"
            btn.disabled = False
        except Exception:
            pass

    def update_issues(self, issues: list[GitHubIssue]) -> None:
        """Update the issues section with fetched issues."""
        self._stop_refresh_spinner()
        self._issues = issues
        self._issues_by_number = {i.number: i for i in issues}

        try:
            container = self.query_one("#issues-container", Vertical)
            container.remove_children()

            if issues:
                for issue in issues[:5]:
                    title_text = issue.title[:45] + (
                        "..." if len(issue.title) > 45 else ""
                    )
                    labels_str = ", ".join(lbl.name for lbl in issue.labels[:2])
                    meta = f"{issue.relative_time}"
                    if labels_str:
                        meta += f" \u2022 {labels_str}"

                    # Compose widgets using Textual's compose pattern
                    row = Horizontal(
                        Vertical(
                            Label(
                                f"#{issue.number} {title_text}", classes="issue-title"
                            ),
                            Label(meta, classes="issue-meta label-muted"),
                            classes="issue-info",
                        ),
                        Button(
                            "Create WT",
                            id=f"btn-issue-{issue.number}",
                            classes="issue-btn",
                        ),
                        classes="issue-row",
                    )
                    container.mount(row)
            else:
                container.mount(Label("No issues found", classes="label-muted"))
        except Exception:
            pass  # Widget may have been removed
