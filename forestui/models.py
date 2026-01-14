"""Data models for forestui."""

from datetime import UTC, datetime
from pathlib import Path
from typing import Self
from uuid import UUID, uuid4

from pydantic import BaseModel, Field


class Worktree(BaseModel):
    """Represents a Git worktree."""

    id: UUID = Field(default_factory=uuid4)
    name: str
    branch: str
    path: str
    is_archived: bool = False
    sort_order: int | None = None
    last_modified: datetime = Field(default_factory=lambda: datetime.now(UTC))

    def get_path(self) -> Path:
        """Get the worktree path as a Path object."""
        return Path(self.path).expanduser()


class Repository(BaseModel):
    """Represents a Git repository with its worktrees."""

    id: UUID = Field(default_factory=uuid4)
    name: str
    source_path: str
    worktrees: list[Worktree] = Field(default_factory=list)

    def get_source_path(self) -> Path:
        """Get the source path as a Path object."""
        return Path(self.source_path).expanduser()

    def active_worktrees(self) -> list[Worktree]:
        """Get active (non-archived) worktrees sorted by order/recency."""
        active = [w for w in self.worktrees if not w.is_archived]
        return sorted(
            active,
            key=lambda w: (
                w.sort_order if w.sort_order is not None else float("inf"),
                -w.last_modified.timestamp(),
            ),
        )

    def archived_worktrees(self) -> list[Worktree]:
        """Get archived worktrees sorted by recency."""
        archived = [w for w in self.worktrees if w.is_archived]
        return sorted(archived, key=lambda w: -w.last_modified.timestamp())

    def find_worktree(self, worktree_id: UUID) -> Worktree | None:
        """Find a worktree by ID."""
        for w in self.worktrees:
            if w.id == worktree_id:
                return w
        return None


class ClaudeSession(BaseModel):
    """Represents a Claude Code session."""

    id: str
    title: str
    last_timestamp: datetime
    message_count: int
    git_branches: list[str] = Field(default_factory=list)

    @property
    def relative_time(self) -> str:
        """Get a human-readable relative time string."""
        now = datetime.now(UTC)
        diff = now - self.last_timestamp
        seconds = diff.total_seconds()

        if seconds < 60:
            return "just now"
        elif seconds < 3600:
            mins = int(seconds / 60)
            return f"{mins}m ago"
        elif seconds < 86400:
            hours = int(seconds / 3600)
            return f"{hours}h ago"
        elif seconds < 604800:
            days = int(seconds / 86400)
            return f"{days}d ago"
        else:
            weeks = int(seconds / 604800)
            return f"{weeks}w ago"

    @property
    def primary_branch(self) -> str | None:
        """Get the first git branch if available."""
        return self.git_branches[0] if self.git_branches else None


class Settings(BaseModel):
    """Application settings."""

    default_editor: str = "code"
    default_terminal: str = ""
    branch_prefix: str = "feat/"
    theme: str = "system"

    @classmethod
    def default(cls) -> Self:
        """Create default settings."""
        return cls()


class Selection(BaseModel):
    """Represents the current selection state."""

    repository_id: UUID | None = None
    worktree_id: UUID | None = None

    @property
    def is_repository(self) -> bool:
        """Check if a repository is selected (not a worktree)."""
        return self.repository_id is not None and self.worktree_id is None

    @property
    def is_worktree(self) -> bool:
        """Check if a worktree is selected."""
        return self.worktree_id is not None
