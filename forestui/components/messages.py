"""Shared message classes for forestui components."""

from uuid import UUID

from textual.message import Message


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
