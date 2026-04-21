"""Worktree detail view component."""

from datetime import datetime
from uuid import UUID

import humanize
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.message import Message
from textual.widget import Widget
from textual.widgets import Button, Input, Label, Rule

from forestui.components.messages import (
    ContinueClaudeCustomSession,
    ContinueClaudeSession,
    ContinueClaudeYoloSession,
    OpenInEditor,
    OpenInFileManager,
    OpenInTerminal,
    StartClaudeCustomSession,
    StartClaudeSession,
    StartClaudeYoloSession,
)
from forestui.models import ClaudeSession, CustomClaudeButton, Repository, Worktree


class WorktreeDetail(Widget):
    """Detail view for a selected worktree."""

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

    class SyncRequested(Message):
        """Request to sync (fetch/pull) the worktree."""

        def __init__(self, worktree_id: UUID, path: str) -> None:
            self.worktree_id = worktree_id
            self.path = path
            super().__init__()

    def __init__(
        self,
        repository: Repository,
        worktree: Worktree,
        commit_hash: str = "",
        commit_time: datetime | None = None,
        has_remote: bool = True,
        custom_buttons: list[CustomClaudeButton] | None = None,
    ) -> None:
        super().__init__()
        self._repository = repository
        self._worktree = worktree
        self._commit_hash = commit_hash
        self._commit_time = commit_time
        self._has_remote = has_remote
        self._sessions: list[ClaudeSession] = []
        self._custom_buttons: list[CustomClaudeButton] = list(custom_buttons or [])
        self._buttons_by_prefix: dict[str, CustomClaudeButton] = {
            b.prefix: b for b in self._custom_buttons
        }

    def compose(self) -> ComposeResult:
        """Compose the worktree detail view."""
        with Vertical(classes="detail-content"):
            # Header - Worktree
            with Vertical(classes="detail-header"):
                yield Label("WORKTREE", classes="section-header")
                yield Label(
                    f"Repository: {self._repository.name}",
                    classes="detail-title",
                )
                yield Label(
                    f"Worktree:   {self._worktree.name}",
                    classes="label-primary",
                )
                yield Label(
                    f"Branch:     {self._worktree.branch}",
                    classes="label-accent",
                )
                # Base branch info (if available)
                if self._worktree.base_branch:
                    base_text = f"Based on:   {self._worktree.base_branch}"
                    if self._worktree.created_from_ref:
                        base_text += f" ({self._worktree.created_from_ref})"
                    yield Label(base_text, classes="label-muted")
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
                self._worktree.path,
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

            # Claude section — buttons wrap to multiple rows when they don't fit.
            yield Label("CLAUDE", classes="section-header")
            # (label, id, variant, is_destructive)
            claude_specs: list[tuple[str, str, str, bool]] = [
                ("New Session", "btn-claude-new", "primary", False),
                ("New Session: YOLO", "btn-claude-yolo", "error", True),
            ]
            for btn in self._custom_buttons:
                if btn.is_yolo_style:
                    claude_specs.append(
                        (btn.label, f"btn-claude-custom-{btn.prefix}", "error", True)
                    )
                else:
                    claude_specs.append(
                        (
                            btn.label,
                            f"btn-claude-custom-{btn.prefix}",
                            "primary",
                            False,
                        )
                    )
            for chunk_start in range(0, len(claude_specs), 4):
                with Horizontal(classes="action-row"):
                    for label, btn_id, variant, is_dest in claude_specs[
                        chunk_start : chunk_start + 4
                    ]:
                        if is_dest:
                            yield Button(
                                label,
                                id=btn_id,
                                variant=variant,  # type: ignore[arg-type]
                                classes="-destructive",
                            )
                        else:
                            yield Button(
                                label,
                                id=btn_id,
                                variant=variant,  # type: ignore[arg-type]
                            )

            # Sessions list (loaded async)
            yield Label("RECENT SESSIONS", classes="section-header")
            with Vertical(id="sessions-container"):
                yield Label("Loading...", classes="label-muted")

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
            case "btn-archive":
                self.post_message(self.ArchiveWorktreeRequested(self._worktree.id))
            case "btn-unarchive":
                self.post_message(self.UnarchiveWorktreeRequested(self._worktree.id))
            case "btn-delete":
                self.post_message(
                    self.DeleteWorktreeRequested(self._repository.id, self._worktree.id)
                )
            case "btn-sync":
                self.post_message(self.SyncRequested(self._worktree.id, path))
            case _ if btn_id.startswith("btn-resume-"):
                session_id = btn_id.replace("btn-resume-", "")
                self.post_message(ContinueClaudeSession(session_id, path))
            case _ if btn_id.startswith("btn-yolo-"):
                session_id = btn_id.replace("btn-yolo-", "")
                self.post_message(ContinueClaudeYoloSession(session_id, path))
            case _ if btn_id.startswith("btn-claude-custom-"):
                prefix = btn_id.removeprefix("btn-claude-custom-")
                custom = self._buttons_by_prefix.get(prefix)
                if custom:
                    self.post_message(StartClaudeCustomSession(path, custom))
            case _ if btn_id.startswith("btn-custom-"):
                # Format: btn-custom-<prefix>-<session_id>
                rest = btn_id.removeprefix("btn-custom-")
                for prefix in self._buttons_by_prefix:
                    marker = f"{prefix}-"
                    if rest.startswith(marker):
                        session_id = rest.removeprefix(marker)
                        custom = self._buttons_by_prefix[prefix]
                        self.post_message(
                            ContinueClaudeCustomSession(session_id, path, custom)
                        )
                        break

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

                    # (label, id, variant, is_destructive)
                    btn_specs: list[tuple[str, str, str, bool]] = [
                        ("Resume", f"btn-resume-{session.id}", "default", False),
                        ("YOLO", f"btn-yolo-{session.id}", "error", True),
                    ]
                    for cbtn in self._custom_buttons:
                        if cbtn.is_yolo_style:
                            btn_specs.append(
                                (
                                    cbtn.label,
                                    f"btn-custom-{cbtn.prefix}-{session.id}",
                                    "error",
                                    True,
                                )
                            )
                        else:
                            btn_specs.append(
                                (
                                    cbtn.label,
                                    f"btn-custom-{cbtn.prefix}-{session.id}",
                                    "primary",
                                    False,
                                )
                            )
                    children: list[Widget] = [*info_children]
                    for chunk_start in range(0, len(btn_specs), 4):
                        row_buttons: list[Button] = []
                        for label, btn_id, variant, is_dest in btn_specs[
                            chunk_start : chunk_start + 4
                        ]:
                            if is_dest:
                                row_buttons.append(
                                    Button(
                                        label,
                                        id=btn_id,
                                        variant=variant,  # type: ignore[arg-type]
                                        classes="session-btn -destructive",
                                    )
                                )
                            else:
                                row_buttons.append(
                                    Button(
                                        label,
                                        id=btn_id,
                                        variant=variant,  # type: ignore[arg-type]
                                        classes="session-btn",
                                    )
                                )
                        children.append(
                            Horizontal(*row_buttons, classes="session-buttons")
                        )
                    container.mount(Vertical(*children, classes="session-item"))
            else:
                container.mount(Label("No sessions found", classes="label-muted"))
        except Exception:
            pass  # Widget may have been removed
