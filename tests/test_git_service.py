"""Tests for GitService."""

import asyncio

import pytest

from forestui.services.git import GitError, get_git_service


def test_missing_cwd_raises_git_error() -> None:
    """A stale worktree (deleted directory) must surface as GitError, not OSError."""
    git = get_git_service()

    with pytest.raises(GitError):
        asyncio.run(git.get_latest_commit("/nonexistent/stale/worktree"))

    with pytest.raises(GitError):
        asyncio.run(git.has_remote_tracking("/nonexistent/stale/worktree"))
