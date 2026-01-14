"""Main forestui application."""

import contextlib
import os
import platform
import shutil
import subprocess
from pathlib import Path
from uuid import UUID

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.widgets import Footer, Header, Label, Static

from forestui.components.modals import (
    AddRepositoryModal,
    AddWorktreeModal,
    ConfirmDeleteModal,
    SettingsModal,
)
from forestui.components.repository_detail import RepositoryDetail
from forestui.components.sidebar import Sidebar
from forestui.components.worktree_detail import WorktreeDetail
from forestui.models import Repository, Worktree
from forestui.services.claude_session import get_claude_session_service
from forestui.services.git import GitError, get_git_service
from forestui.services.settings import get_settings_service
from forestui.state import get_app_state
from forestui.theme import APP_CSS


class EmptyState(Static):
    """Empty state when nothing is selected."""

    def compose(self) -> ComposeResult:
        """Compose the empty state UI."""
        with Vertical(classes="empty-state"):
            yield Label(" forestui", classes="label-accent")
            yield Label("Git Worktree Manager", classes="label-secondary")
            yield Label("")
            yield Label("Select a repository or worktree", classes="label-muted")
            yield Label("or press [a] to add a repository", classes="label-muted")


class ForestApp(App[None]):
    """Main forestui application."""

    TITLE = "forestui"
    CSS = APP_CSS

    BINDINGS = [
        Binding("q", "quit", "Quit", show=True, priority=True),
        Binding("a", "add_repository", "Add Repo", show=True),
        Binding("w", "add_worktree", "Add Worktree", show=True),
        Binding("e", "open_editor", "Editor", show=True),
        Binding("t", "open_terminal", "Terminal", show=True),
        Binding("o", "open_files", "Files", show=True),
        Binding("n", "start_claude", "Claude", show=True),
        Binding("h", "toggle_archive", "Archive", show=True),
        Binding("d", "delete", "Delete", show=True),
        Binding("s", "open_settings", "Settings", show=True),
        Binding("r", "refresh", "Refresh", show=False),
        Binding("?", "show_help", "Help", show=True),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._state = get_app_state()
        self._settings_service = get_settings_service()
        self._git_service = get_git_service()
        self._claude_service = get_claude_session_service()

    def compose(self) -> ComposeResult:
        """Compose the application UI."""
        yield Header()
        with Horizontal(id="main-container"):
            yield Sidebar(
                repositories=self._state.repositories,
                selected_repo_id=self._state.selection.repository_id,
                selected_worktree_id=self._state.selection.worktree_id,
                show_archived=self._state.show_archived,
            )
            with Vertical(id="detail-pane"):
                yield EmptyState()
        yield Footer()

    def _refresh_sidebar(self) -> None:
        """Refresh the sidebar with current state."""
        sidebar = self.query_one(Sidebar)
        sidebar.update_repositories(
            repositories=self._state.repositories,
            selected_repo_id=self._state.selection.repository_id,
            selected_worktree_id=self._state.selection.worktree_id,
            show_archived=self._state.show_archived,
        )

    async def _refresh_detail_pane(self) -> None:
        """Refresh the detail pane based on selection."""
        detail_pane = self.query_one("#detail-pane")

        # Clear existing content
        await detail_pane.remove_children()

        selection = self._state.selection

        if selection.worktree_id:
            # Show worktree detail
            result = self._state.find_worktree(selection.worktree_id)
            if result:
                repo, worktree = result
                sessions = self._claude_service.get_sessions_for_path(worktree.path)
                await detail_pane.mount(
                    WorktreeDetail(repo, worktree, sessions=sessions)
                )
        elif selection.repository_id:
            # Show repository detail
            selected_repo = self._state.find_repository(selection.repository_id)
            if selected_repo:
                try:
                    branch = await self._git_service.get_current_branch(
                        selected_repo.source_path
                    )
                except GitError:
                    branch = ""
                sessions = self._claude_service.get_sessions_for_path(
                    selected_repo.source_path
                )
                await detail_pane.mount(
                    RepositoryDetail(
                        selected_repo, current_branch=branch, sessions=sessions
                    )
                )
        else:
            # Show empty state
            await detail_pane.mount(EmptyState())

    # Event handlers from sidebar
    async def on_sidebar_repository_selected(
        self, event: Sidebar.RepositorySelected
    ) -> None:
        """Handle repository selection."""
        self._state.select_repository(event.repo_id)
        await self._refresh_detail_pane()

    async def on_sidebar_worktree_selected(
        self, event: Sidebar.WorktreeSelected
    ) -> None:
        """Handle worktree selection."""
        self._state.select_worktree(event.repo_id, event.worktree_id)
        await self._refresh_detail_pane()

    def on_sidebar_add_repository_requested(
        self, event: Sidebar.AddRepositoryRequested
    ) -> None:
        """Handle add repository request."""
        self.action_add_repository()

    async def on_sidebar_add_worktree_requested(
        self, event: Sidebar.AddWorktreeRequested
    ) -> None:
        """Handle add worktree request."""
        await self._show_add_worktree_modal(event.repo_id)

    # Event handlers from detail views
    def on_repository_detail_open_in_editor(
        self, event: RepositoryDetail.OpenInEditor
    ) -> None:
        """Handle open in editor request."""
        self._open_in_editor(event.path)

    def on_worktree_detail_open_in_editor(
        self, event: WorktreeDetail.OpenInEditor
    ) -> None:
        """Handle open in editor request."""
        self._open_in_editor(event.path)

    def on_repository_detail_open_in_terminal(
        self, event: RepositoryDetail.OpenInTerminal
    ) -> None:
        """Handle open in terminal request."""
        self._open_in_terminal(event.path)

    def on_worktree_detail_open_in_terminal(
        self, event: WorktreeDetail.OpenInTerminal
    ) -> None:
        """Handle open in terminal request."""
        self._open_in_terminal(event.path)

    def on_repository_detail_open_in_file_manager(
        self, event: RepositoryDetail.OpenInFileManager
    ) -> None:
        """Handle open in file manager request."""
        self._open_in_file_manager(event.path)

    def on_worktree_detail_open_in_file_manager(
        self, event: WorktreeDetail.OpenInFileManager
    ) -> None:
        """Handle open in file manager request."""
        self._open_in_file_manager(event.path)

    def on_repository_detail_start_claude_session(
        self, event: RepositoryDetail.StartClaudeSession
    ) -> None:
        """Handle start Claude session request."""
        self._start_claude_session(event.path)

    def on_worktree_detail_start_claude_session(
        self, event: WorktreeDetail.StartClaudeSession
    ) -> None:
        """Handle start Claude session request."""
        self._start_claude_session(event.path)

    def on_repository_detail_continue_claude_session(
        self, event: RepositoryDetail.ContinueClaudeSession
    ) -> None:
        """Handle continue Claude session request."""
        self._continue_claude_session(event.session_id, event.path)

    def on_worktree_detail_continue_claude_session(
        self, event: WorktreeDetail.ContinueClaudeSession
    ) -> None:
        """Handle continue Claude session request."""
        self._continue_claude_session(event.session_id, event.path)

    async def on_repository_detail_add_worktree_requested(
        self, event: RepositoryDetail.AddWorktreeRequested
    ) -> None:
        """Handle add worktree request."""
        await self._show_add_worktree_modal(event.repo_id)

    @work
    async def on_repository_detail_remove_repository_requested(
        self, event: RepositoryDetail.RemoveRepositoryRequested
    ) -> None:
        """Handle remove repository request."""
        repo = self._state.find_repository(event.repo_id)
        if repo:
            confirmed = await self.push_screen_wait(
                ConfirmDeleteModal(
                    "Remove Repository",
                    f"Remove '{repo.name}' from forestui?\n(Files will not be deleted)",
                )
            )
            if confirmed:
                self._state.remove_repository(event.repo_id)
                self._refresh_sidebar()
                await self._refresh_detail_pane()

    async def on_worktree_detail_archive_worktree_requested(
        self, event: WorktreeDetail.ArchiveWorktreeRequested
    ) -> None:
        """Handle archive worktree request."""
        self._state.archive_worktree(event.worktree_id)
        self._refresh_sidebar()
        await self._refresh_detail_pane()

    async def on_worktree_detail_unarchive_worktree_requested(
        self, event: WorktreeDetail.UnarchiveWorktreeRequested
    ) -> None:
        """Handle unarchive worktree request."""
        self._state.unarchive_worktree(event.worktree_id)
        self._refresh_sidebar()
        await self._refresh_detail_pane()

    @work
    async def on_worktree_detail_delete_worktree_requested(
        self, event: WorktreeDetail.DeleteWorktreeRequested
    ) -> None:
        """Handle delete worktree request."""
        result = self._state.find_worktree(event.worktree_id)
        if result:
            repo, worktree = result
            confirmed = await self.push_screen_wait(
                ConfirmDeleteModal(
                    "Delete Worktree",
                    f"Permanently delete worktree '{worktree.name}'?\nThis cannot be undone.",
                )
            )
            if confirmed:
                with contextlib.suppress(GitError):
                    await self._git_service.remove_worktree(
                        repo.source_path, worktree.path
                    )
                self._state.remove_worktree(event.worktree_id)
                self._refresh_sidebar()
                await self._refresh_detail_pane()

    async def on_worktree_detail_rename_worktree_requested(
        self, event: WorktreeDetail.RenameWorktreeRequested
    ) -> None:
        """Handle rename worktree request."""
        result = self._state.find_worktree(event.worktree_id)
        if result:
            repo, worktree = result
            old_path = Path(worktree.path)
            new_path = old_path.parent / event.new_name

            if new_path.exists():
                self.notify("Path already exists", severity="error")
                return

            try:
                # Rename the directory
                old_path.rename(new_path)
                # Repair git references
                await self._git_service.repair_worktree(repo.source_path, new_path)
                # Migrate Claude sessions
                self._claude_service.migrate_sessions(old_path, new_path)
                # Update state
                self._state.update_worktree(
                    event.worktree_id, name=event.new_name, path=str(new_path)
                )
                self._refresh_sidebar()
                await self._refresh_detail_pane()
            except (OSError, GitError) as e:
                self.notify(f"Rename failed: {e}", severity="error")

    async def on_worktree_detail_rename_branch_requested(
        self, event: WorktreeDetail.RenameBranchRequested
    ) -> None:
        """Handle rename branch request."""
        result = self._state.find_worktree(event.worktree_id)
        if result:
            _repo, worktree = result
            try:
                await self._git_service.rename_branch(
                    worktree.path, worktree.branch, event.new_branch
                )
                self._state.update_worktree(event.worktree_id, branch=event.new_branch)
                self._refresh_sidebar()
                await self._refresh_detail_pane()
            except GitError as e:
                self.notify(f"Branch rename failed: {e}", severity="error")

    # Modal handlers
    async def on_add_repository_modal_repository_added(
        self, event: AddRepositoryModal.RepositoryAdded
    ) -> None:
        """Handle repository added from modal."""
        path = Path(event.path)
        repo = Repository(name=path.name, source_path=str(path))
        self._state.add_repository(repo)
        self._state.select_repository(repo.id)
        self._refresh_sidebar()
        await self._refresh_detail_pane()

        if event.import_worktrees:
            await self._import_existing_worktrees(repo)

    async def on_add_worktree_modal_worktree_created(
        self, event: AddWorktreeModal.WorktreeCreated
    ) -> None:
        """Handle worktree created from modal."""
        repo = self._state.find_repository(event.repo_id)
        if not repo:
            return

        settings = self._settings_service.settings
        forest_dir = settings.get_forest_path()
        worktree_path = forest_dir / repo.name / event.name

        try:
            await self._git_service.create_worktree(
                repo.source_path, worktree_path, event.branch, event.new_branch
            )
            worktree = Worktree(
                name=event.name, branch=event.branch, path=str(worktree_path)
            )
            self._state.add_worktree(event.repo_id, worktree)
            self._state.select_worktree(event.repo_id, worktree.id)
            self._refresh_sidebar()
            await self._refresh_detail_pane()
            self.notify(f"Created worktree '{event.name}'")
        except GitError as e:
            self.notify(f"Failed to create worktree: {e}", severity="error")

    # Actions
    def action_add_repository(self) -> None:
        """Show add repository modal."""
        self.push_screen(AddRepositoryModal())

    async def action_add_worktree(self) -> None:
        """Show add worktree modal for selected repository."""
        repo_id = self._state.selection.repository_id
        if repo_id:
            await self._show_add_worktree_modal(repo_id)
        else:
            self.notify("Select a repository first", severity="warning")

    async def _show_add_worktree_modal(self, repo_id: UUID) -> None:
        """Show the add worktree modal."""
        repo = self._state.find_repository(repo_id)
        if not repo:
            return

        try:
            branches = await self._git_service.list_branches(repo.source_path)
        except GitError:
            branches = []

        settings = self._settings_service.settings
        self.push_screen(
            AddWorktreeModal(
                repo,
                branches,
                settings.get_forest_path(),
                settings.branch_prefix,
            )
        )

    def action_open_editor(self) -> None:
        """Open selected item in editor."""
        path = self._get_selected_path()
        if path:
            self._open_in_editor(path)

    def action_open_terminal(self) -> None:
        """Open selected item in terminal."""
        path = self._get_selected_path()
        if path:
            self._open_in_terminal(path)

    def action_open_files(self) -> None:
        """Open selected item in file manager."""
        path = self._get_selected_path()
        if path:
            self._open_in_file_manager(path)

    def action_start_claude(self) -> None:
        """Start Claude session for selected item."""
        path = self._get_selected_path()
        if path:
            self._start_claude_session(path)

    async def action_toggle_archive(self) -> None:
        """Toggle archive status of selected worktree."""
        if self._state.selection.worktree_id:
            result = self._state.find_worktree(self._state.selection.worktree_id)
            if result:
                _, worktree = result
                if worktree.is_archived:
                    self._state.unarchive_worktree(worktree.id)
                else:
                    self._state.archive_worktree(worktree.id)
                self._refresh_sidebar()
                await self._refresh_detail_pane()

    @work
    async def action_delete(self) -> None:
        """Delete selected item."""
        selection = self._state.selection
        if selection.worktree_id:
            result = self._state.find_worktree(selection.worktree_id)
            if result:
                repo, worktree = result
                confirmed = await self.push_screen_wait(
                    ConfirmDeleteModal(
                        "Delete Worktree",
                        f"Permanently delete '{worktree.name}'?",
                    )
                )
                if confirmed:
                    with contextlib.suppress(GitError):
                        await self._git_service.remove_worktree(
                            repo.source_path, worktree.path
                        )
                    self._state.remove_worktree(worktree.id)
                    self._refresh_sidebar()
                    await self._refresh_detail_pane()
        elif selection.repository_id:
            selected_repo = self._state.find_repository(selection.repository_id)
            if selected_repo:
                confirmed = await self.push_screen_wait(
                    ConfirmDeleteModal(
                        "Remove Repository",
                        f"Remove '{selected_repo.name}' from forestui?",
                    )
                )
                if confirmed:
                    self._state.remove_repository(selected_repo.id)
                    self._refresh_sidebar()
                    await self._refresh_detail_pane()

    @work
    async def action_open_settings(self) -> None:
        """Open settings modal."""
        settings = self._settings_service.settings
        result = await self.push_screen_wait(SettingsModal(settings))
        if result:
            self._settings_service.save_settings(result)
            self.notify("Settings saved")

    async def action_refresh(self) -> None:
        """Refresh the UI."""
        self._refresh_sidebar()
        await self._refresh_detail_pane()

    def action_show_help(self) -> None:
        """Show help information."""
        self.notify(
            "a: Add Repo | w: Add Worktree | e: Editor | t: Terminal | "
            "n: Claude | h: Archive | d: Delete | s: Settings | q: Quit"
        )

    # Helper methods
    def _get_selected_path(self) -> str | None:
        """Get the path of the currently selected item."""
        selection = self._state.selection
        if selection.worktree_id:
            result = self._state.find_worktree(selection.worktree_id)
            if result:
                return result[1].path
        elif selection.repository_id:
            repo = self._state.find_repository(selection.repository_id)
            if repo:
                return repo.source_path
        return None

    def _open_in_editor(self, path: str) -> None:
        """Open path in configured editor."""
        editor = self._settings_service.settings.default_editor
        try:
            subprocess.Popen(
                [editor, path],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            self.notify(f"Opened in {editor}")
        except FileNotFoundError:
            self.notify(f"Editor '{editor}' not found", severity="error")

    def _open_in_terminal(self, path: str) -> None:
        """Open path in terminal."""
        system = platform.system()
        try:
            if system == "Darwin":
                # macOS
                terminal = os.environ.get("TERM_PROGRAM", "Terminal")
                if terminal == "iTerm.app":
                    subprocess.Popen(["open", "-a", "iTerm", path])
                else:
                    subprocess.Popen(["open", "-a", "Terminal", path])
            elif system == "Linux":
                # Try common terminal emulators
                terminal = os.environ.get("TERMINAL", "")
                if terminal:
                    subprocess.Popen([terminal, "--working-directory", path])
                elif shutil.which("gnome-terminal"):
                    subprocess.Popen(["gnome-terminal", "--working-directory", path])
                elif shutil.which("konsole"):
                    subprocess.Popen(["konsole", "--workdir", path])
                elif shutil.which("xterm"):
                    subprocess.Popen(["xterm", "-e", f"cd {path} && $SHELL"])
                else:
                    self.notify("No terminal found", severity="error")
                    return
            elif system == "Windows":
                subprocess.Popen(["wt", "-d", path])
            self.notify("Opened terminal")
        except FileNotFoundError as e:
            self.notify(f"Failed to open terminal: {e}", severity="error")

    def _open_in_file_manager(self, path: str) -> None:
        """Open path in file manager."""
        system = platform.system()
        try:
            if system == "Darwin":
                subprocess.Popen(["open", path])
            elif system == "Linux":
                subprocess.Popen(["xdg-open", path])
            elif system == "Windows":
                subprocess.Popen(["explorer", path])
            self.notify("Opened in file manager")
        except FileNotFoundError as e:
            self.notify(f"Failed to open file manager: {e}", severity="error")

    def _start_claude_session(self, path: str) -> None:
        """Start a new Claude session."""
        self._open_in_terminal(path)
        # The user will need to run 'claude' manually in the terminal
        self.notify("Start Claude with 'claude' in the terminal")

    def _continue_claude_session(self, session_id: str, path: str) -> None:
        """Continue an existing Claude session."""
        self._open_in_terminal(path)
        self.notify(f"Run 'claude -r {session_id}' to continue session")

    async def _import_existing_worktrees(self, repo: Repository) -> None:
        """Import existing worktrees from a repository."""
        try:
            worktrees = await self._git_service.list_worktrees(repo.source_path)
            settings = self._settings_service.settings
            forest_dir = settings.get_forest_path()

            for wt_info in worktrees:
                # Skip the main worktree (same as source_path)
                if Path(wt_info.path).resolve() == Path(repo.source_path).resolve():
                    continue

                # Check if already in forest directory
                wt_path = Path(wt_info.path)
                if str(wt_path).startswith(str(forest_dir)):
                    continue

                # Create worktree model
                name = wt_path.name
                branch = wt_info.branch or "HEAD"
                worktree = Worktree(name=name, branch=branch, path=str(wt_path))
                self._state.add_worktree(repo.id, worktree)

            self._refresh_sidebar()
            self.notify(f"Imported {len(worktrees) - 1} worktrees")
        except GitError as e:
            self.notify(f"Failed to import worktrees: {e}", severity="error")


def main() -> None:
    """Run the forestui application."""
    app = ForestApp()
    app.run()


if __name__ == "__main__":
    main()
