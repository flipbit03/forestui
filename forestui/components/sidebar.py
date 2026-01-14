"""Sidebar component for forestui."""

from uuid import UUID

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal
from textual.message import Message
from textual.widgets import Button, Label, Static, Tree

from forestui.models import Repository, Worktree


class RepoNode:
    """Data for a repository node."""

    def __init__(self, repo: Repository) -> None:
        self.repo = repo
        self.id = repo.id


class WorktreeNode:
    """Data for a worktree node."""

    def __init__(self, repo: Repository, worktree: Worktree) -> None:
        self.repo = repo
        self.worktree = worktree
        self.repo_id = repo.id
        self.id = worktree.id


class ArchivedNode:
    """Data for the archived section node."""

    pass


class Sidebar(Static):
    """Sidebar widget with repository and worktree tree."""

    BINDINGS = [
        Binding("a", "add_repository", "Add Repo", show=True),
    ]

    class RepositorySelected(Message):
        """Sent when a repository is selected."""

        def __init__(self, repo_id: UUID) -> None:
            self.repo_id = repo_id
            super().__init__()

    class WorktreeSelected(Message):
        """Sent when a worktree is selected."""

        def __init__(self, repo_id: UUID, worktree_id: UUID) -> None:
            self.repo_id = repo_id
            self.worktree_id = worktree_id
            super().__init__()

    class AddRepositoryRequested(Message):
        """Sent when user wants to add a repository."""

        pass

    class AddWorktreeRequested(Message):
        """Sent when user wants to add a worktree."""

        def __init__(self, repo_id: UUID) -> None:
            self.repo_id = repo_id
            super().__init__()

    class DeleteRepositoryRequested(Message):
        """Sent when user wants to delete a repository."""

        def __init__(self, repo_id: UUID) -> None:
            self.repo_id = repo_id
            super().__init__()

    class ArchiveWorktreeRequested(Message):
        """Sent when user wants to archive a worktree."""

        def __init__(self, worktree_id: UUID) -> None:
            self.worktree_id = worktree_id
            super().__init__()

    class UnarchiveWorktreeRequested(Message):
        """Sent when user wants to unarchive a worktree."""

        def __init__(self, worktree_id: UUID) -> None:
            self.worktree_id = worktree_id
            super().__init__()

    class DeleteWorktreeRequested(Message):
        """Sent when user wants to delete a worktree."""

        def __init__(self, repo_id: UUID, worktree_id: UUID) -> None:
            self.repo_id = repo_id
            self.worktree_id = worktree_id
            super().__init__()

    def __init__(
        self,
        repositories: list[Repository],
        selected_repo_id: UUID | None = None,
        selected_worktree_id: UUID | None = None,
        show_archived: bool = False,
    ) -> None:
        super().__init__(id="sidebar")  # Apply sidebar ID to the widget itself
        self._repositories = repositories
        self._selected_repo_id = selected_repo_id
        self._selected_worktree_id = selected_worktree_id
        self._show_archived = show_archived

    def compose(self) -> ComposeResult:
        """Compose the sidebar UI."""
        # Header
        with Horizontal(classes="sidebar-header"):
            yield Label(" Worktrees", classes="label-primary")
            with Horizontal(classes="header-buttons"):
                yield Button("+", id="btn-add-repo", variant="default")
        # Tree view
        tree: Tree[RepoNode | WorktreeNode | ArchivedNode] = Tree(
            "Repositories", id="repo-tree"
        )
        tree.show_root = False
        tree.guide_depth = 2
        yield tree

    def on_mount(self) -> None:
        """Populate the tree when mounted."""
        self._populate_tree()

    def _populate_tree(self) -> None:
        """Populate the tree with repositories and worktrees."""
        tree = self.query_one("#repo-tree", Tree)
        tree.clear()

        for repo in self._repositories:
            # Add repository node
            repo_label = f" {repo.name}"
            repo_node = tree.root.add(repo_label, data=RepoNode(repo), expand=True)

            # Add active worktrees
            for worktree in repo.active_worktrees():
                prefix = "├─" if worktree != repo.active_worktrees()[-1] else "└─"
                wt_label = f"{prefix}  {worktree.name} [{worktree.branch}]"
                repo_node.add_leaf(wt_label, data=WorktreeNode(repo, worktree))

        # Add archived section if there are archived worktrees
        if self._show_archived:
            has_archived = any(
                w.is_archived for r in self._repositories for w in r.worktrees
            )
            if has_archived:
                archived_node = tree.root.add(
                    " Archived", data=ArchivedNode(), expand=False
                )
                for repo in self._repositories:
                    for worktree in repo.archived_worktrees():
                        wt_label = f"   {worktree.name} ({repo.name})"
                        archived_node.add_leaf(
                            wt_label, data=WorktreeNode(repo, worktree)
                        )

    def update_repositories(
        self,
        repositories: list[Repository],
        selected_repo_id: UUID | None = None,
        selected_worktree_id: UUID | None = None,
        show_archived: bool = False,
    ) -> None:
        """Update the sidebar with new data."""
        self._repositories = repositories
        self._selected_repo_id = selected_repo_id
        self._selected_worktree_id = selected_worktree_id
        self._show_archived = show_archived
        self._populate_tree()

    def on_tree_node_selected(
        self, event: Tree.NodeSelected[RepoNode | WorktreeNode | ArchivedNode]
    ) -> None:
        """Handle tree node selection (Enter key or click)."""
        self._select_node(event.node)

    def on_tree_node_highlighted(
        self, event: Tree.NodeHighlighted[RepoNode | WorktreeNode | ArchivedNode]
    ) -> None:
        """Handle tree node highlight (keyboard navigation)."""
        self._select_node(event.node)

    def _select_node(self, node: object) -> None:
        """Select a node and post the appropriate message."""
        if node is None:
            return

        data = getattr(node, "data", None)
        if data is None:
            return

        if isinstance(data, RepoNode):
            self.post_message(self.RepositorySelected(data.id))
        elif isinstance(data, WorktreeNode):
            self.post_message(self.WorktreeSelected(data.repo_id, data.id))

    def on_button_pressed(self, event: Button.Pressed) -> None:
        """Handle button presses."""
        if event.button.id == "btn-add-repo":
            self.post_message(self.AddRepositoryRequested())

    def action_add_repository(self) -> None:
        """Action to add a repository."""
        self.post_message(self.AddRepositoryRequested())
