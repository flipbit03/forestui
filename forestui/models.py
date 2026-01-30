"""Data models for forestui."""

import re
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Self
from uuid import UUID, uuid4

import humanize
from pydantic import BaseModel, Field, field_validator

# Constants for validation
MAX_CLAUDE_COMMAND_LENGTH = 200


def validate_claude_command(command: str) -> str | None:
    """Validate a custom Claude command.

    Args:
        command: The command string to validate (can be empty).

    Returns:
        Error message if invalid, None if valid.
    """
    if not command:
        return None  # Empty is valid (clears/uses default)

    if len(command) > MAX_CLAUDE_COMMAND_LENGTH:
        return f"Command too long (max {MAX_CLAUDE_COMMAND_LENGTH} characters)"

    if any(c in command for c in "\n\r\t\0"):
        return "Command cannot contain newlines or control characters"

    return None


def _validate_command_field(v: str | None) -> str | None:
    """Pydantic field validator for custom_claude_command."""
    if v is None:
        return None
    error = validate_claude_command(v)
    if error:
        raise ValueError(error)
    return v


@dataclass
class ClaudeCommandResult:
    """Result from ClaudeCommandModal.

    Attributes:
        command: The command string, or None to clear the setting.
        cancelled: True if user cancelled the modal.
    """

    command: str | None
    cancelled: bool = False


class Worktree(BaseModel):
    """Represents a Git worktree."""

    id: UUID = Field(default_factory=uuid4)
    name: str
    branch: str
    path: str
    is_archived: bool = False
    sort_order: int | None = None
    last_modified: datetime = Field(default_factory=lambda: datetime.now(UTC))
    custom_claude_command: str | None = None

    @field_validator("custom_claude_command")
    @classmethod
    def validate_command(cls, v: str | None) -> str | None:
        """Validate custom_claude_command field."""
        return _validate_command_field(v)

    def get_path(self) -> Path:
        """Get the worktree path as a Path object."""
        return Path(self.path).expanduser()


class Repository(BaseModel):
    """Represents a Git repository with its worktrees."""

    id: UUID = Field(default_factory=uuid4)
    name: str
    source_path: str
    worktrees: list[Worktree] = Field(default_factory=list)
    custom_claude_command: str | None = None

    @field_validator("custom_claude_command")
    @classmethod
    def validate_command(cls, v: str | None) -> str | None:
        """Validate custom_claude_command field."""
        return _validate_command_field(v)

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
    last_message: str = ""
    last_timestamp: datetime
    message_count: int
    git_branches: list[str] = Field(default_factory=list)

    @property
    def relative_time(self) -> str:
        """Get a human-readable relative time string."""
        return humanize.naturaltime(self.last_timestamp)

    @property
    def primary_branch(self) -> str | None:
        """Get the first git branch if available."""
        return self.git_branches[0] if self.git_branches else None


class Settings(BaseModel):
    """Application settings."""

    default_editor: str = "vim"
    default_terminal: str = ""
    branch_prefix: str = "feat/"
    theme: str = "system"
    custom_claude_command: str | None = None

    @field_validator("custom_claude_command")
    @classmethod
    def validate_command(cls, v: str | None) -> str | None:
        """Validate custom_claude_command field."""
        return _validate_command_field(v)

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


class GitHubLabel(BaseModel):
    """GitHub issue label."""

    name: str
    color: str = ""


class GitHubUser(BaseModel):
    """GitHub user."""

    login: str


class GitHubIssue(BaseModel):
    """GitHub issue."""

    number: int
    title: str
    state: str
    url: str
    created_at: datetime
    updated_at: datetime
    author: GitHubUser
    assignees: list[GitHubUser] = Field(default_factory=list)
    labels: list[GitHubLabel] = Field(default_factory=list)

    @property
    def branch_name(self) -> str:
        """Generate branch-safe name from issue. e.g., '42-fix-login-bug'."""
        slug = re.sub(r"[^a-z0-9]+", "-", self.title.lower())[:40].strip("-")
        return f"{self.number}-{slug}"

    @property
    def relative_time(self) -> str:
        """Human-readable relative time since update."""
        return humanize.naturaltime(self.updated_at)
