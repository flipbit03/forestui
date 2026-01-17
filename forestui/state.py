"""Application state management for forestui."""

import json
from datetime import UTC, datetime
from pathlib import Path
from uuid import UUID

from pydantic import BaseModel

from forestui.models import Repository, Selection, Worktree
from forestui.services.settings import get_forest_path


class AppStateData(BaseModel):
    """Serializable app state data."""

    repositories: list[Repository] = []


class AppState:
    """Centralized application state with persistence."""

    def __init__(self) -> None:
        self._repositories: list[Repository] = []
        self._selection: Selection = Selection()
        self._show_archived: bool = False
        self._load_state()

    def _get_config_path(self) -> Path:
        """Get the config file path."""
        forest_dir = get_forest_path()
        forest_dir.mkdir(parents=True, exist_ok=True)
        return forest_dir / ".forestui-config.json"

    def _load_state(self) -> None:
        """Load state from config file."""
        config_path = self._get_config_path()
        if config_path.exists():
            try:
                with config_path.open(encoding="utf-8") as f:
                    data = json.load(f)
                    state_data = AppStateData.model_validate(data)
                    self._repositories = state_data.repositories
            except (json.JSONDecodeError, OSError):
                pass

    def _save_state(self) -> None:
        """Save state to config file."""
        config_path = self._get_config_path()
        config_path.parent.mkdir(parents=True, exist_ok=True)
        state_data = AppStateData(repositories=self._repositories)
        with config_path.open("w", encoding="utf-8") as f:
            json.dump(state_data.model_dump(mode="json"), f, indent=2, default=str)

    @property
    def repositories(self) -> list[Repository]:
        """Get all repositories."""
        return self._repositories

    @property
    def selection(self) -> Selection:
        """Get current selection."""
        return self._selection

    @selection.setter
    def selection(self, value: Selection) -> None:
        """Set current selection."""
        self._selection = value

    @property
    def show_archived(self) -> bool:
        """Get show archived flag."""
        return self._show_archived

    @show_archived.setter
    def show_archived(self, value: bool) -> None:
        """Set show archived flag."""
        self._show_archived = value

    def add_repository(self, repository: Repository) -> None:
        """Add a repository."""
        self._repositories.append(repository)
        self._save_state()

    def remove_repository(self, repo_id: UUID) -> None:
        """Remove a repository by ID."""
        self._repositories = [r for r in self._repositories if r.id != repo_id]
        if self._selection.repository_id == repo_id:
            self._selection = Selection()
        self._save_state()

    def find_repository(self, repo_id: UUID) -> Repository | None:
        """Find a repository by ID."""
        for repo in self._repositories:
            if repo.id == repo_id:
                return repo
        return None

    def update_repository_command(self, repo_id: UUID, command: str | None) -> bool:
        """Update a repository's custom Claude command.

        Returns:
            True if the repository was found and updated, False otherwise.
        """
        for repo in self._repositories:
            if repo.id == repo_id:
                repo.custom_claude_command = command
                self._save_state()
                return True
        return False

    def update_worktree_command(self, worktree_id: UUID, command: str | None) -> bool:
        """Update a worktree's custom Claude command.

        Returns:
            True if the worktree was found and updated, False otherwise.
        """
        for repo in self._repositories:
            for worktree in repo.worktrees:
                if worktree.id == worktree_id:
                    worktree.custom_claude_command = command
                    self._save_state()
                    return True
        return False

    def find_worktree(self, worktree_id: UUID) -> tuple[Repository, Worktree] | None:
        """Find a worktree by ID and return with its parent repository."""
        for repo in self._repositories:
            for worktree in repo.worktrees:
                if worktree.id == worktree_id:
                    return repo, worktree
        return None

    def add_worktree(self, repo_id: UUID, worktree: Worktree) -> None:
        """Add a worktree to a repository."""
        for repo in self._repositories:
            if repo.id == repo_id:
                repo.worktrees.append(worktree)
                self._save_state()
                return

    def remove_worktree(self, worktree_id: UUID) -> None:
        """Remove a worktree by ID."""
        for repo in self._repositories:
            repo.worktrees = [w for w in repo.worktrees if w.id != worktree_id]
        if self._selection.worktree_id == worktree_id:
            self._selection = Selection(repository_id=self._selection.repository_id)
        self._save_state()

    def update_worktree(
        self, worktree_id: UUID, **kwargs: str | bool | int | None
    ) -> None:
        """Update a worktree's attributes."""
        for repo in self._repositories:
            for i, worktree in enumerate(repo.worktrees):
                if worktree.id == worktree_id:
                    data = worktree.model_dump()
                    data.update(kwargs)
                    repo.worktrees[i] = Worktree.model_validate(data)
                    self._save_state()
                    return

    def archive_worktree(self, worktree_id: UUID) -> None:
        """Archive a worktree."""
        self.update_worktree(worktree_id, is_archived=True)

    def unarchive_worktree(self, worktree_id: UUID) -> None:
        """Unarchive a worktree."""
        self.update_worktree(worktree_id, is_archived=False)

    def select_repository(self, repo_id: UUID) -> None:
        """Select a repository."""
        self._selection = Selection(repository_id=repo_id)

    def select_worktree(self, repo_id: UUID, worktree_id: UUID) -> None:
        """Select a worktree."""
        self._selection = Selection(repository_id=repo_id, worktree_id=worktree_id)

    def clear_selection(self) -> None:
        """Clear the current selection."""
        self._selection = Selection()

    @property
    def selected_repository(self) -> Repository | None:
        """Get the currently selected repository."""
        if self._selection.repository_id:
            return self.find_repository(self._selection.repository_id)
        return None

    @property
    def selected_worktree(self) -> tuple[Repository, Worktree] | None:
        """Get the currently selected worktree with its parent repo."""
        if self._selection.worktree_id:
            return self.find_worktree(self._selection.worktree_id)
        return None

    def has_archived_worktrees(self) -> bool:
        """Check if there are any archived worktrees."""
        for repo in self._repositories:
            if any(w.is_archived for w in repo.worktrees):
                return True
        return False

    def all_archived_worktrees(self) -> list[tuple[Repository, Worktree]]:
        """Get all archived worktrees with their parent repositories."""
        result: list[tuple[Repository, Worktree]] = []
        for repo in self._repositories:
            for worktree in repo.archived_worktrees():
                result.append((repo, worktree))
        return result

    def reorder_worktree(
        self, repo_id: UUID, worktree_id: UUID, new_index: int
    ) -> None:
        """Reorder a worktree within its repository."""
        repo = self.find_repository(repo_id)
        if not repo:
            return

        # Get active worktrees in current order
        active = repo.active_worktrees()
        worktree = next((w for w in active if w.id == worktree_id), None)
        if not worktree:
            return

        # Remove and reinsert at new position
        active = [w for w in active if w.id != worktree_id]
        active.insert(min(new_index, len(active)), worktree)

        # Update sort orders
        for i, w in enumerate(active):
            self.update_worktree(w.id, sort_order=i)

    def refresh_worktree_timestamp(self, worktree_id: UUID) -> None:
        """Update a worktree's last modified timestamp."""
        self.update_worktree(worktree_id, last_modified=datetime.now(UTC).isoformat())


# Global app state instance
_app_state: AppState | None = None


def get_app_state() -> AppState:
    """Get the global app state instance."""
    global _app_state
    if _app_state is None:
        _app_state = AppState()
    return _app_state
