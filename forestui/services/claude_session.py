"""Service for managing Claude Code session history."""

import json
import re
import shutil
from datetime import UTC, datetime
from pathlib import Path

from forestui.models import ClaudeSession


class ClaudeSessionService:
    """Service for reading and managing Claude Code sessions."""

    _instance: ClaudeSessionService | None = None

    def __new__(cls) -> ClaudeSessionService:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    @staticmethod
    def _path_to_claude_folder(path: str | Path) -> str:
        """Convert a path to Claude's folder naming convention."""
        path_str = str(Path(path).expanduser().resolve())
        return path_str.replace("/", "-")

    @staticmethod
    def _get_claude_projects_dir() -> Path:
        """Get the Claude projects directory."""
        return Path.home() / ".claude" / "projects"

    def get_sessions_for_path(
        self, path: str | Path, limit: int = 5
    ) -> list[ClaudeSession]:
        """Get Claude sessions for a given path."""
        folder_name = self._path_to_claude_folder(path)
        sessions_dir = self._get_claude_projects_dir() / folder_name

        if not sessions_dir.exists():
            return []

        sessions: list[ClaudeSession] = []

        for session_file in sessions_dir.glob("*.jsonl"):
            # Skip agent files
            if session_file.name.startswith("agent-"):
                continue

            session = self._parse_session_file(session_file)
            if session:
                sessions.append(session)

        # Sort by timestamp (newest first) and limit
        sessions.sort(key=lambda s: s.last_timestamp, reverse=True)
        return sessions[:limit]

    def _parse_session_file(self, file_path: Path) -> ClaudeSession | None:
        """Parse a session JSONL file."""

        session_id = file_path.stem
        title = ""
        last_message = ""
        last_timestamp = datetime.min.replace(tzinfo=UTC)
        message_count = 0
        git_branches: list[str] = []

        try:
            with file_path.open(encoding="utf-8") as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue

                    try:
                        data = json.loads(line)
                    except json.JSONDecodeError:
                        continue

                    # Extract timestamp
                    if "timestamp" in data:
                        try:
                            ts = datetime.fromisoformat(
                                data["timestamp"].replace("Z", "+00:00")
                            )
                            # Ensure timezone-aware (assume UTC if naive)
                            if ts.tzinfo is None:
                                ts = ts.replace(tzinfo=UTC)
                            if ts > last_timestamp:
                                last_timestamp = ts
                        except (ValueError, AttributeError):
                            pass

                    # Check for user messages
                    if data.get("type") == "user" or data.get("role") == "user":
                        message_count += 1

                        # Extract message content
                        content = data.get("message", {}).get(
                            "content", ""
                        ) or data.get("content", "")
                        if isinstance(content, list):
                            # Handle block format
                            for block in content:
                                if (
                                    isinstance(block, dict)
                                    and block.get("type") == "text"
                                ):
                                    content = block.get("text", "")
                                    break
                            else:
                                content = ""

                        # Use for title/last_message if valid
                        if (
                            isinstance(content, str)
                            and content
                            and not content.startswith("<")
                        ):
                            # Collapse 3+ newlines to 2 (preserve single blank lines)
                            normalized = re.sub(r"\n{3,}", "\n\n", content)
                            if not title:
                                title = normalized[:100]
                            # Always update last_message (will end up with the last one)
                            last_message = normalized[:100]

                    # Extract git branches
                    if "gitBranches" in data:
                        branches = data["gitBranches"]
                        if isinstance(branches, list):
                            for branch in branches:
                                if branch and branch not in git_branches:
                                    git_branches.append(branch)

        except OSError:
            return None

        if message_count == 0:
            return None

        if last_timestamp == datetime.min.replace(tzinfo=UTC):
            last_timestamp = datetime.fromtimestamp(file_path.stat().st_mtime, tz=UTC)

        return ClaudeSession(
            id=session_id,
            title=title or "Untitled session",
            last_message=last_message,
            last_timestamp=last_timestamp,
            message_count=message_count,
            git_branches=git_branches,
        )

    def migrate_sessions(self, old_path: str | Path, new_path: str | Path) -> None:
        """Migrate session history from old path to new path."""
        old_folder = self._path_to_claude_folder(old_path)
        new_folder = self._path_to_claude_folder(new_path)

        old_dir = self._get_claude_projects_dir() / old_folder
        new_dir = self._get_claude_projects_dir() / new_folder

        if not old_dir.exists():
            return

        new_dir.mkdir(parents=True, exist_ok=True)

        # Move session files
        for session_file in old_dir.glob("*.jsonl"):
            dest = new_dir / session_file.name
            if not dest.exists():
                shutil.move(str(session_file), str(dest))

        # Clean up empty directory
        if old_dir.exists() and not any(old_dir.iterdir()):
            old_dir.rmdir()


def get_claude_session_service() -> ClaudeSessionService:
    """Get the singleton ClaudeSessionService instance."""
    return ClaudeSessionService()
