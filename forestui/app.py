"""Main forestui application."""

import contextlib
import subprocess
from pathlib import Path
from uuid import UUID

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.widget import Widget
from textual.widgets import Footer, Header, Label

from forestui import __version__
from forestui.components.messages import (
    ConfigureClaudeCommand,
    ContinueClaudeSession,
    ContinueClaudeYoloSession,
    OpenInEditor,
    OpenInFileManager,
    OpenInTerminal,
    StartClaudeSession,
    StartClaudeYoloSession,
)
from forestui.components.modals import (
    AddRepositoryModal,
    AddWorktreeModal,
    ClaudeCommandModal,
    ConfirmDeleteModal,
    CreateWorktreeFromIssueModal,
    SettingsModal,
)
from forestui.components.repository_detail import RepositoryDetail
from forestui.components.sidebar import Sidebar
from forestui.components.worktree_detail import WorktreeDetail
from forestui.models import ClaudeCommandResult, GitHubIssue, Repository, Worktree
from forestui.services.claude_session import get_claude_session_service
from forestui.services.git import GitError, get_git_service
from forestui.services.github import get_github_service
from forestui.services.settings import get_forest_path, get_settings_service
from forestui.services.tmux import get_tmux_service
from forestui.state import get_app_state
from forestui.theme import APP_CSS


class EmptyState(Widget):
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

    TITLE = f"forestui v{__version__}"
    CSS = APP_CSS

    BINDINGS = [
        Binding("q", "quit", "Quit", show=True, priority=True),
        Binding("a", "add_repository", "Add Repo", show=True),
        Binding("w", "add_worktree", "Add Worktree", show=True),
        Binding("e", "open_editor", "Editor", show=True),
        Binding("t", "open_terminal", "Terminal", show=True),
        Binding("o", "open_files", "Files", show=True),
        Binding("n", "start_claude", "Claude", show=True),
        Binding("y", "start_claude_yolo", "ClaudeYOLO", show=True),
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
        self._tmux_service = get_tmux_service()
        self._github_service = get_github_service()

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
            with VerticalScroll(id="detail-pane"):
                yield EmptyState()
        yield Footer()

    async def on_mount(self) -> None:
        """Handle app mount - auto-select first repo if nothing selected."""
        # Ensure tmux focus events are enabled for auto-refresh
        if not self._tmux_service.ensure_focus_events():
            self.notify("Could not enable focus events", severity="warning")

        if not self._state.selection.repository_id and self._state.repositories:
            self._state.select_repository(self._state.repositories[0].id)
            await self._refresh_detail_pane()
        # Auto-update in background
        self._auto_update()

        # Check GitHub CLI status
        self._check_gh_status()

        # Start GitHub issues refresh timer (5 minutes)
        self.set_interval(300, self._refresh_github_issues)

    @work
    async def on_app_focus(self) -> None:
        """Refresh detail pane when app regains focus."""
        await self._refresh_detail_pane()

    @work
    async def _check_gh_status(self) -> None:
        """Check and display GitHub CLI auth status."""
        status, username = await self._github_service.get_auth_status()
        sidebar = self.query_one(Sidebar)
        sidebar.set_gh_status(status, username)

    @work
    async def _refresh_github_issues(self) -> None:
        """Periodically refresh GitHub issues cache."""
        self._github_service.invalidate_cache()
        if self._state.selection.repository_id:
            repo = self._state.find_repository(self._state.selection.repository_id)
            if repo:
                self._fetch_issues_for_repo(repo.source_path)

    @work
    async def _fetch_issues_for_repo(self, repo_path: str) -> None:
        """Fetch GitHub issues in background and update the detail pane."""
        issues: list[GitHubIssue] = []
        try:
            issues = await self._github_service.list_issues(repo_path)
        except Exception as e:
            self.notify(f"Issue fetch error: {e}", severity="error")
        # Update the detail pane if it's still showing a RepositoryDetail
        try:
            detail = self.query_one(RepositoryDetail)
            detail.update_issues(issues)
        except Exception:
            pass  # Detail pane changed, ignore

    @work
    async def _fetch_sessions_for_path(self, path: str, detail_type: str) -> None:
        """Fetch Claude sessions in background and update the detail pane."""
        sessions = self._claude_service.get_sessions_for_path(path)
        # Update the appropriate detail pane
        try:
            if detail_type == "repository":
                self.query_one(RepositoryDetail).update_sessions(sessions)
            else:
                self.query_one(WorktreeDetail).update_sessions(sessions)
        except Exception:
            pass  # Detail pane changed, ignore

    def _set_title_suffix(self, suffix: str | None) -> None:
        """Update title with optional suffix."""
        base = f"forestui v{__version__}"
        self.title = f"{base} ({suffix})" if suffix else base

    @work(thread=True)
    def _auto_update(self) -> None:
        """Auto-update via PyPI with status in title bar.

        Uses `uv tool install forestui --force --upgrade` to check PyPI for updates.

        IMPORTANT: We use `install --force --upgrade` instead of `upgrade` because
        `uv tool upgrade` rebuilds from the original install source. For users who
        installed via the old git-clone method, that would rebuild from the local
        ~/.forestui-install directory instead of fetching from PyPI.

        The `--upgrade` flag ensures we only reinstall if a newer version exists.
        The `--force` flag ensures we overwrite the existing installation.
        """
        import os
        import re

        if os.environ.get("FORESTUI_NO_AUTO_UPDATE"):
            return

        try:
            self._set_title_suffix("checking for updates...")

            # Install from PyPI if newer version available
            # MUST use "install --force --upgrade", NOT "upgrade"
            # See docstring for explanation
            result = subprocess.run(
                ["uv", "tool", "install", "forestui", "--force", "--upgrade"],
                capture_output=True,
                text=True,
                timeout=120,
            )

            if result.returncode == 0:
                # Check if we actually upgraded (vs already up to date)
                # Output contains "Installed forestui v0.9.1" when installed/upgraded
                # Output contains "forestui is already installed" when up to date
                if "Installed" in result.stdout or "Upgraded" in result.stdout:
                    # Try to extract version from output
                    match = re.search(r"v(\d+\.\d+\.\d+)", result.stdout)
                    if match:
                        new_version = match.group(1)
                        self._set_title_suffix(
                            f"updated to v{new_version} - restart to apply"
                        )
                    else:
                        self._set_title_suffix("updated - restart to apply")
                else:
                    # Already up to date
                    self._set_title_suffix(None)
            else:
                # Command failed - might not be on PyPI yet or network issue
                self._set_title_suffix(None)

        except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError):
            # Silent failure - don't disrupt the app
            self._set_title_suffix(None)

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
                # Get commit info
                commit_hash = ""
                commit_time = None
                has_remote = False
                try:
                    commit_info = await self._git_service.get_latest_commit(
                        worktree.path
                    )
                    commit_hash = commit_info.short_hash
                    commit_time = commit_info.timestamp
                    has_remote = await self._git_service.has_remote_tracking(
                        worktree.path
                    )
                except GitError:
                    pass
                await detail_pane.mount(
                    WorktreeDetail(
                        repo,
                        worktree,
                        commit_hash=commit_hash,
                        commit_time=commit_time,
                        has_remote=has_remote,
                    )
                )
                # Fetch sessions in background
                self._fetch_sessions_for_path(worktree.path, "worktree")
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
                # Get commit info
                commit_hash = ""
                commit_time = None
                has_remote = False
                try:
                    commit_info = await self._git_service.get_latest_commit(
                        selected_repo.source_path
                    )
                    commit_hash = commit_info.short_hash
                    commit_time = commit_info.timestamp
                    has_remote = await self._git_service.has_remote_tracking(
                        selected_repo.source_path
                    )
                except GitError:
                    pass
                # Mount detail pane immediately, fetch data in background
                detail = RepositoryDetail(
                    selected_repo,
                    current_branch=branch,
                    commit_hash=commit_hash,
                    commit_time=commit_time,
                    has_remote=has_remote,
                )
                await detail_pane.mount(detail)
                # Start spinner and fetch sessions and issues in background
                detail.start_issues_spinner()
                self._fetch_sessions_for_path(selected_repo.source_path, "repository")
                self._fetch_issues_for_repo(selected_repo.source_path)
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

    # Consolidated event handlers for shared messages (from both detail views)
    def on_open_in_editor(self, event: OpenInEditor) -> None:
        """Handle open in editor request."""
        self._open_in_editor(event.path)

    def on_open_in_terminal(self, event: OpenInTerminal) -> None:
        """Handle open in terminal request."""
        self._open_in_terminal(event.path)

    def on_open_in_file_manager(self, event: OpenInFileManager) -> None:
        """Handle open in file manager request."""
        self._open_in_file_manager(event.path)

    def on_start_claude_session(self, event: StartClaudeSession) -> None:
        """Handle start Claude session request."""
        self._start_claude_session(event.path)

    def on_start_claude_yolo_session(self, event: StartClaudeYoloSession) -> None:
        """Handle start Claude YOLO session request."""
        self._start_claude_session(event.path, yolo=True)

    def on_continue_claude_session(self, event: ContinueClaudeSession) -> None:
        """Handle continue Claude session request."""
        self._continue_claude_session(event.session_id, event.path)

    def on_continue_claude_yolo_session(self, event: ContinueClaudeYoloSession) -> None:
        """Handle continue Claude YOLO session request."""
        self._continue_claude_session(event.session_id, event.path, yolo=True)

    @work
    async def on_worktree_detail_sync_requested(
        self, event: WorktreeDetail.SyncRequested
    ) -> None:
        """Handle sync (fetch/pull) request for worktree."""
        self.notify("Syncing...")
        try:
            await self._git_service.pull(event.path)
            self.notify("Sync complete")
            await self._refresh_detail_pane()
        except GitError as e:
            self.notify(f"Sync failed: {e}", severity="error")

    async def on_repository_detail_add_worktree_requested(
        self, event: RepositoryDetail.AddWorktreeRequested
    ) -> None:
        """Handle add worktree request."""
        await self._show_add_worktree_modal(event.repo_id)

    def on_configure_claude_command(self, event: ConfigureClaudeCommand) -> None:
        """Handle configure Claude command request."""
        self._show_claude_command_modal(event.repo_id, event.worktree_id)

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

    @work
    async def on_repository_detail_sync_requested(
        self, event: RepositoryDetail.SyncRequested
    ) -> None:
        """Handle sync (fetch/pull) request."""
        self.notify("Syncing...")
        try:
            await self._git_service.pull(event.path)
            self.notify("Sync complete")
            await self._refresh_detail_pane()
        except GitError as e:
            self.notify(f"Sync failed: {e}", severity="error")

    async def on_repository_detail_create_worktree_from_issue(
        self, event: RepositoryDetail.CreateWorktreeFromIssue
    ) -> None:
        """Handle create worktree from issue request."""
        await self._show_create_worktree_from_issue_modal(event.repo_id, event.issue)

    def on_repository_detail_refresh_issues_requested(
        self, event: RepositoryDetail.RefreshIssuesRequested
    ) -> None:
        """Handle manual refresh of GitHub issues."""
        self._github_service.invalidate_cache()
        self._fetch_issues_for_repo(event.repo_path)

    async def _show_create_worktree_from_issue_modal(
        self, repo_id: UUID, issue: GitHubIssue
    ) -> None:
        """Show the create worktree from issue modal."""
        repo = self._state.find_repository(repo_id)
        if not repo:
            return
        try:
            branches = await self._git_service.list_branches(repo.source_path)
        except GitError:
            branches = []
        settings = self._settings_service.settings
        self.push_screen(
            CreateWorktreeFromIssueModal(
                repo, issue, branches, get_forest_path(), settings.branch_prefix
            )
        )

    @work
    async def on_create_worktree_from_issue_modal_worktree_created(
        self, event: CreateWorktreeFromIssueModal.WorktreeCreated
    ) -> None:
        """Handle worktree created from issue modal."""
        repo = self._state.find_repository(event.repo_id)
        if not repo:
            return

        forest_dir = get_forest_path()
        worktree_path = forest_dir / repo.name / event.name

        try:
            # Pull repo first if requested
            if event.pull_first:
                self.notify("Pulling repo...")
                await self._git_service.pull(repo.source_path)

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

        forest_dir = get_forest_path()
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
                get_forest_path(),
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

    def action_start_claude_yolo(self) -> None:
        """Start Claude session with --dangerously-skip-permissions."""
        path = self._get_selected_path()
        if path:
            self._start_claude_session(path, yolo=True)

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

    def _get_name_for_path(self, path: str) -> str | None:
        """Get the worktree or repository name for a given path."""
        # First check worktrees
        for repo in self._state.repositories:
            for worktree in repo.worktrees:
                if worktree.path == path:
                    return worktree.name
            # Check if it's the repository source path
            if repo.source_path == path:
                return repo.name
        return None

    def _get_claude_window_name(self, path: str) -> str:
        """Get the window name for Claude sessions: repo:branch format."""
        # Check worktrees first
        for repo in self._state.repositories:
            for worktree in repo.worktrees:
                if worktree.path == path:
                    return f"{repo.name}:{worktree.branch}"
            # Check if it's the repository source path
            if repo.source_path == path:
                return repo.name
        return "session"

    def _open_in_editor(self, path: str) -> None:
        """Open path in configured editor."""
        editor = self._settings_service.settings.default_editor

        # If inside tmux and editor is TUI-based, use tmux window
        if self._tmux_service.is_inside_tmux and self._tmux_service.is_tui_editor(
            editor
        ):
            name = self._get_claude_window_name(path)
            if self._tmux_service.create_editor_window(name, path, editor):
                self.notify(f"Opened {editor} in edit:{name}")
                return

        # GUI editor or not in tmux - spawn normally
        try:
            # Handle editors with arguments (e.g., "emacs -nw")
            editor_parts = editor.split()
            subprocess.Popen(
                [*editor_parts, path],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            self.notify(f"Opened in {editor_parts[0]}")
        except FileNotFoundError:
            self.notify(f"Editor '{editor}' not found", severity="error")

    def _open_in_terminal(self, path: str) -> None:
        """Open path in a tmux terminal window."""
        name = self._get_claude_window_name(path)
        if self._tmux_service.create_shell_window(name, path):
            self.notify(f"Opened terminal in term:{name}")
        else:
            self.notify("Failed to create terminal window", severity="error")

    def _open_in_file_manager(self, path: str) -> None:
        """Open path in Midnight Commander (mc) in a tmux window."""
        name = self._get_claude_window_name(path)
        if self._tmux_service.create_mc_window(name, path):
            self.notify(f"Opened mc in files:{name}")
        else:
            self.notify("Failed to create mc window", severity="error")

    @work
    async def _show_claude_command_modal(
        self, repo_id: UUID, worktree_id: UUID | None = None
    ) -> None:
        """Show the Claude command configuration modal."""
        repo = self._state.find_repository(repo_id)
        if not repo:
            return

        # Determine if configuring worktree or repository
        if worktree_id:
            worktree = repo.find_worktree(worktree_id)
            if not worktree:
                return
            name = f"{repo.name}:{worktree.name}"
            current_command = worktree.custom_claude_command
            is_worktree = True
        else:
            name = repo.name
            current_command = repo.custom_claude_command
            is_worktree = False

        result: ClaudeCommandResult = await self.push_screen_wait(
            ClaudeCommandModal(name, current_command, is_worktree=is_worktree)
        )

        if result.cancelled:
            return

        # Update the appropriate level
        if worktree_id:
            self._state.update_worktree_command(worktree_id, result.command)
            target = f"{repo.name}:{worktree.name}"  # type: ignore[union-attr]
        else:
            self._state.update_repository_command(repo_id, result.command)
            target = repo.name

        await self._refresh_detail_pane()
        if result.command:
            self.notify(f"Custom Claude command set for {target}")
        else:
            self.notify(f"Custom Claude command cleared for {target}")

    def _find_repo_for_path(self, path: str) -> Repository | None:
        """Find the repository associated with a path (worktree or source)."""
        for repo in self._state.repositories:
            if repo.source_path == path:
                return repo
            for worktree in repo.worktrees:
                if worktree.path == path:
                    return repo
        return None

    def _resolve_claude_command(self, path: str) -> str | None:
        """Resolve Claude command with hierarchy: worktree > repo > folder > default.

        Args:
            path: The path to check for associated repository/worktree

        Returns:
            Custom command if set, None to use default "claude"
        """
        # Check worktree-level and repo-level
        for repo in self._state.repositories:
            # Check worktrees first (most specific)
            for worktree in repo.worktrees:
                if worktree.path == path:
                    if worktree.custom_claude_command:
                        return worktree.custom_claude_command
                    if repo.custom_claude_command:
                        return repo.custom_claude_command
                    break
            # Check repository source path
            if repo.source_path == path:
                if repo.custom_claude_command:
                    return repo.custom_claude_command
                break

        # Fall back to folder-level setting
        settings = self._settings_service.settings
        return settings.custom_claude_command

    def _start_claude_session(self, path: str, yolo: bool = False) -> None:
        """Start a new Claude session in a tmux window."""
        name = self._get_claude_window_name(path)
        custom_command = self._resolve_claude_command(path)
        window_name = self._tmux_service.create_claude_window(
            name, path, yolo=yolo, custom_command=custom_command
        )
        if window_name:
            mode = " (YOLO)" if yolo else ""
            self.notify(f"Started Claude{mode} in {window_name}")
        else:
            self.notify("Failed to create Claude window", severity="error")

    def _continue_claude_session(
        self, session_id: str, path: str, yolo: bool = False
    ) -> None:
        """Continue an existing Claude session in a tmux window."""
        name = self._get_claude_window_name(path)
        custom_command = self._resolve_claude_command(path)
        window_name = self._tmux_service.create_claude_window(
            name,
            path,
            resume_session_id=session_id,
            yolo=yolo,
            custom_command=custom_command,
        )
        if window_name:
            mode = " (YOLO)" if yolo else ""
            self.notify(f"Resuming Claude{mode} in {window_name}")
        else:
            self.notify("Failed to create Claude window", severity="error")

    async def _import_existing_worktrees(self, repo: Repository) -> None:
        """Import existing worktrees from a repository."""
        try:
            worktrees = await self._git_service.list_worktrees(repo.source_path)
            forest_dir = get_forest_path()

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


def run_app() -> None:
    """Run the forestui application."""
    import sys
    import traceback
    from pathlib import Path

    try:
        app = ForestApp()
        app.run()
    except Exception as e:
        error_log = Path.home() / ".forestui-error.log"
        tb = traceback.format_exc()
        error_log.write_text(tb)
        print(tb, file=sys.stderr)
        print(f"\nError: {e}", file=sys.stderr)
        print(f"\nError log written to: {error_log}", file=sys.stderr)
        input("Press Enter to exit...")
        sys.exit(1)


# Entry point for CLI
def main() -> None:
    """CLI entry point - delegates to cli module."""
    from forestui.cli import main as cli_main

    cli_main()


if __name__ == "__main__":
    main()
