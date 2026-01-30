"""GitHub CLI service for interacting with gh."""

from __future__ import annotations

import asyncio
import json
from datetime import UTC, datetime
from pathlib import Path
from typing import NamedTuple

from forestui.models import GitHubIssue, GitHubLabel, GitHubUser


class IssueCache(NamedTuple):
    """Cached issues with timestamp."""

    issues: list[GitHubIssue]
    fetched_at: datetime


class GitHubService:
    """Service for interacting with GitHub via gh CLI."""

    _instance: GitHubService | None = None
    CACHE_TTL_SECONDS: int = 300  # 5 minutes

    _cache: dict[str, IssueCache]
    _auth_status: str | None
    _username: str | None

    def __new__(cls) -> GitHubService:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
            cls._instance._cache = {}
            cls._instance._auth_status = None
            cls._instance._username = None
        return cls._instance

    @staticmethod
    async def _run_gh(
        *args: str, cwd: str | Path | None = None
    ) -> tuple[int, str, str]:
        """Run a gh command and return (exit_code, stdout, stderr)."""
        try:
            process = await asyncio.create_subprocess_exec(
                "gh",
                *args,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                cwd=str(cwd) if cwd else None,
            )
            stdout, stderr = await process.communicate()
            return (
                process.returncode or 0,
                stdout.decode().strip(),
                stderr.decode().strip(),
            )
        except FileNotFoundError:
            return (-1, "", "gh not found")

    async def get_auth_status(self) -> tuple[str, str | None]:
        """Get GitHub CLI authentication status and username.

        Returns: (status, username) where status is "authenticated", "not_authenticated", or "not_installed"
        """
        if self._auth_status is not None:
            return self._auth_status, self._username

        code, _stdout, _ = await self._run_gh("auth", "status")
        if code == -1:
            self._auth_status = "not_installed"
            self._username = None
        elif code == 0:
            self._auth_status = "authenticated"
            # Get username
            code, stdout, _ = await self._run_gh("api", "user", "--jq", ".login")
            self._username = stdout if code == 0 and stdout else None
        else:
            self._auth_status = "not_authenticated"
            self._username = None
        return self._auth_status, self._username

    async def get_repo_info(self, path: str | Path) -> tuple[str, str] | None:
        """Get (owner, repo) for a git repository path. Returns None if not a GitHub repo."""
        code, stdout, _ = await self._run_gh(
            "repo", "view", "--json", "owner,name", cwd=path
        )
        if code != 0 or not stdout:
            return None
        try:
            data = json.loads(stdout)
            return (data["owner"]["login"], data["name"])
        except (json.JSONDecodeError, KeyError):
            return None

    async def list_issues(
        self,
        path: str | Path,
        assigned_to_me: bool = True,
        authored_by_me: bool = True,
        limit: int = 10,
        use_cache: bool = True,
    ) -> list[GitHubIssue]:
        """List GitHub issues for the repository at path."""
        # Check auth first
        auth, _ = await self.get_auth_status()
        if auth != "authenticated":
            return []

        repo_info = await self.get_repo_info(path)
        if not repo_info:
            return []

        cache_key = f"{repo_info[0]}/{repo_info[1]}"

        # Check cache
        if use_cache and cache_key in self._cache:
            cached = self._cache[cache_key]
            age = (datetime.now(UTC) - cached.fetched_at).total_seconds()
            if age < self.CACHE_TTL_SECONDS:
                return cached.issues

        # Fetch from GitHub
        issues = await self._fetch_issues(path, assigned_to_me, authored_by_me, limit)

        # Update cache
        self._cache[cache_key] = IssueCache(issues=issues, fetched_at=datetime.now(UTC))
        return issues

    async def _fetch_issues(
        self,
        path: str | Path,
        assigned_to_me: bool,
        authored_by_me: bool,
        limit: int,
    ) -> list[GitHubIssue]:
        """Fetch issues from GitHub API."""
        json_fields = (
            "number,title,state,url,createdAt,updatedAt,author,assignees,labels"
        )
        issues: list[GitHubIssue] = []
        seen_numbers: set[int] = set()

        if assigned_to_me:
            code, stdout, _ = await self._run_gh(
                "issue",
                "list",
                "--assignee",
                "@me",
                "--state",
                "open",
                "--limit",
                str(limit),
                "--json",
                json_fields,
                cwd=path,
            )
            if code == 0 and stdout:
                for item in json.loads(stdout):
                    if item["number"] not in seen_numbers:
                        issues.append(self._parse_issue(item))
                        seen_numbers.add(item["number"])

        if authored_by_me:
            code, stdout, _ = await self._run_gh(
                "issue",
                "list",
                "--author",
                "@me",
                "--state",
                "open",
                "--limit",
                str(limit),
                "--json",
                json_fields,
                cwd=path,
            )
            if code == 0 and stdout:
                for item in json.loads(stdout):
                    if item["number"] not in seen_numbers:
                        issues.append(self._parse_issue(item))
                        seen_numbers.add(item["number"])

        issues.sort(key=lambda i: i.created_at, reverse=True)
        return issues[:limit]

    def _parse_issue(self, data: dict[str, object]) -> GitHubIssue:
        """Parse JSON response into GitHubIssue model."""
        author_data = data.get("author") or {}
        author_login = (
            author_data.get("login", "unknown")
            if isinstance(author_data, dict)
            else "unknown"
        )

        assignees_data = data.get("assignees", [])
        assignees = []
        if isinstance(assignees_data, list):
            for a in assignees_data:
                if isinstance(a, dict) and "login" in a:
                    assignees.append(GitHubUser(login=str(a["login"])))

        labels_data = data.get("labels", [])
        labels = []
        if isinstance(labels_data, list):
            for lbl in labels_data:
                if isinstance(lbl, dict) and "name" in lbl:
                    labels.append(
                        GitHubLabel(
                            name=str(lbl["name"]), color=str(lbl.get("color", ""))
                        )
                    )

        created_at_str = str(data.get("createdAt", ""))
        updated_at_str = str(data.get("updatedAt", ""))

        number_val = data.get("number", 0)
        number = int(number_val) if isinstance(number_val, (int, str)) else 0

        return GitHubIssue(
            number=number,
            title=str(data.get("title", "")),
            state=str(data.get("state", "")),
            url=str(data.get("url", "")),
            created_at=datetime.fromisoformat(created_at_str.replace("Z", "+00:00")),
            updated_at=datetime.fromisoformat(updated_at_str.replace("Z", "+00:00")),
            author=GitHubUser(login=str(author_login)),
            assignees=assignees,
            labels=labels,
        )

    def invalidate_cache(self, repo_key: str | None = None) -> None:
        """Invalidate cache for a specific repo or all repos."""
        if repo_key:
            self._cache.pop(repo_key, None)
        else:
            self._cache.clear()


def get_github_service() -> GitHubService:
    """Get the singleton GitHubService instance."""
    return GitHubService()
