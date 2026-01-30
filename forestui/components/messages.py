"""Shared message classes for detail view components."""

from uuid import UUID

from textual.message import Message


class OpenInEditor(Message):
    """Request to open a path in editor."""

    def __init__(self, path: str) -> None:
        self.path = path
        super().__init__()


class OpenInTerminal(Message):
    """Request to open a path in terminal."""

    def __init__(self, path: str) -> None:
        self.path = path
        super().__init__()


class OpenInFileManager(Message):
    """Request to open a path in file manager."""

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


class ConfigureClaudeCommand(Message):
    """Request to configure custom Claude command for a repository or worktree."""

    def __init__(
        self,
        repo_id: UUID,
        worktree_id: UUID | None = None,
    ) -> None:
        self.repo_id = repo_id
        self.worktree_id = worktree_id
        super().__init__()
