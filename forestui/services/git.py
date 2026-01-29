"""Git service for executing git commands."""

import asyncio
from datetime import UTC, datetime
from pathlib import Path
from typing import NamedTuple


class GitError(Exception):
    """Git operation error."""

    pass


class WorktreeInfo(NamedTuple):
    """Information about a git worktree."""

    path: str
    head: str
    branch: str | None


class CommitInfo(NamedTuple):
    """Information about a git commit."""

    hash: str
    short_hash: str
    timestamp: datetime


class GitService:
    """Service for executing git operations."""

    _instance: GitService | None = None

    def __new__(cls) -> GitService:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    @staticmethod
    async def _run_git(
        *args: str, cwd: str | Path | None = None
    ) -> tuple[int, str, str]:
        """Run a git command and return exit code, stdout, stderr."""
        cmd = ["git", *args]
        process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            cwd=str(cwd) if cwd else None,
        )
        stdout, stderr = await process.communicate()
        return (
            process.returncode or 0,
            stdout.decode("utf-8").strip(),
            stderr.decode("utf-8").strip(),
        )

    async def is_git_repository(self, path: str | Path) -> bool:
        """Check if a path is a git repository."""
        path = Path(path).expanduser()
        if not path.exists():
            return False
        code, _, _ = await self._run_git("rev-parse", "--git-dir", cwd=path)
        return code == 0

    async def get_current_branch(self, path: str | Path) -> str:
        """Get the current branch of a repository."""
        path = Path(path).expanduser()
        code, stdout, stderr = await self._run_git("branch", "--show-current", cwd=path)
        if code != 0:
            raise GitError(f"Failed to get current branch: {stderr}")
        return stdout or "HEAD"

    async def list_branches(self, path: str | Path) -> list[str]:
        """List all branches (local and remote) for a repository."""
        path = Path(path).expanduser()
        code, stdout, stderr = await self._run_git(
            "branch", "-a", "--format=%(refname:short)", cwd=path
        )
        if code != 0:
            raise GitError(f"Failed to list branches: {stderr}")
        branches = []
        for line in stdout.split("\n"):
            line = line.strip()
            if line and not line.startswith("origin/HEAD"):
                # Clean up remote branch names
                if line.startswith("origin/"):
                    line = line[7:]
                if line and line not in branches:
                    branches.append(line)
        return sorted(branches)

    async def create_worktree(
        self,
        repo_path: str | Path,
        worktree_path: str | Path,
        branch: str,
        new_branch: bool = True,
    ) -> None:
        """Create a new worktree."""
        repo_path = Path(repo_path).expanduser()
        worktree_path = Path(worktree_path).expanduser()

        # Ensure parent directory exists
        worktree_path.parent.mkdir(parents=True, exist_ok=True)

        if new_branch:
            code, _stdout, stderr = await self._run_git(
                "worktree", "add", "-b", branch, str(worktree_path), cwd=repo_path
            )
        else:
            code, _stdout, stderr = await self._run_git(
                "worktree", "add", str(worktree_path), branch, cwd=repo_path
            )

        if code != 0:
            raise GitError(f"Failed to create worktree: {stderr}")

    async def remove_worktree(
        self, repo_path: str | Path, worktree_path: str | Path, force: bool = False
    ) -> None:
        """Remove a worktree."""
        repo_path = Path(repo_path).expanduser()
        worktree_path = Path(worktree_path).expanduser()

        args = ["worktree", "remove"]
        if force:
            args.append("--force")
        args.append(str(worktree_path))

        code, _stdout, stderr = await self._run_git(*args, cwd=repo_path)
        if code != 0:
            # Try force remove if normal remove fails
            if not force:
                await self.remove_worktree(repo_path, worktree_path, force=True)
            else:
                raise GitError(f"Failed to remove worktree: {stderr}")

    async def rename_branch(
        self, path: str | Path, old_name: str, new_name: str
    ) -> None:
        """Rename a branch."""
        path = Path(path).expanduser()
        code, _stdout, stderr = await self._run_git(
            "branch", "-m", old_name, new_name, cwd=path
        )
        if code != 0:
            raise GitError(f"Failed to rename branch: {stderr}")

    async def repair_worktree(
        self, repo_path: str | Path, worktree_path: str | Path
    ) -> None:
        """Repair worktree references after moving."""
        repo_path = Path(repo_path).expanduser()
        worktree_path = Path(worktree_path).expanduser()
        code, _stdout, stderr = await self._run_git(
            "worktree", "repair", str(worktree_path), cwd=repo_path
        )
        if code != 0:
            raise GitError(f"Failed to repair worktree: {stderr}")

    async def list_worktrees(self, repo_path: str | Path) -> list[WorktreeInfo]:
        """List all worktrees for a repository."""
        repo_path = Path(repo_path).expanduser()
        code, stdout, stderr = await self._run_git(
            "worktree", "list", "--porcelain", cwd=repo_path
        )
        if code != 0:
            raise GitError(f"Failed to list worktrees: {stderr}")

        worktrees: list[WorktreeInfo] = []
        current_path: str | None = None
        current_head: str | None = None
        current_branch: str | None = None

        for line in stdout.split("\n"):
            line = line.strip()
            if line.startswith("worktree "):
                if current_path and current_head:
                    worktrees.append(
                        WorktreeInfo(current_path, current_head, current_branch)
                    )
                current_path = line[9:]
                current_head = None
                current_branch = None
            elif line.startswith("HEAD "):
                current_head = line[5:]
            elif line.startswith("branch "):
                # refs/heads/branch-name -> branch-name
                current_branch = line[7:].replace("refs/heads/", "")
            elif line == "" and current_path and current_head:
                worktrees.append(
                    WorktreeInfo(current_path, current_head, current_branch)
                )
                current_path = None
                current_head = None
                current_branch = None

        # Don't forget the last one
        if current_path and current_head:
            worktrees.append(WorktreeInfo(current_path, current_head, current_branch))

        return worktrees

    async def branch_exists(self, repo_path: str | Path, branch: str) -> bool:
        """Check if a branch exists."""
        branches = await self.list_branches(repo_path)
        return branch in branches

    async def get_latest_commit(self, path: str | Path) -> CommitInfo:
        """Get the latest commit info for a repository."""
        path = Path(path).expanduser()
        # Get commit hash and unix timestamp
        code, stdout, stderr = await self._run_git(
            "log", "-1", "--format=%H|%h|%ct", cwd=path
        )
        if code != 0:
            raise GitError(f"Failed to get latest commit: {stderr}")
        parts = stdout.split("|")
        if len(parts) != 3:
            raise GitError("Unexpected git log output format")
        full_hash, short_hash, timestamp_str = parts
        timestamp = datetime.fromtimestamp(int(timestamp_str), tz=UTC)
        return CommitInfo(hash=full_hash, short_hash=short_hash, timestamp=timestamp)

    async def fetch(self, path: str | Path) -> None:
        """Fetch from remote."""
        path = Path(path).expanduser()
        code, _stdout, stderr = await self._run_git("fetch", cwd=path)
        if code != 0:
            raise GitError(f"Failed to fetch: {stderr}")

    async def pull(self, path: str | Path) -> None:
        """Pull from remote (fetch + merge)."""
        path = Path(path).expanduser()
        code, _stdout, stderr = await self._run_git("pull", cwd=path)
        if code != 0:
            raise GitError(f"Failed to pull: {stderr}")

    async def has_remote_tracking(self, path: str | Path) -> bool:
        """Check if the current branch has a remote tracking branch."""
        path = Path(path).expanduser()
        code, stdout, _stderr = await self._run_git(
            "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}", cwd=path
        )
        # Returns 0 if tracking branch exists, non-zero otherwise
        return code == 0 and bool(stdout.strip())


def get_git_service() -> GitService:
    """Get the singleton GitService instance."""
    return GitService()
