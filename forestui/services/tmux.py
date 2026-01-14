"""Service for tmux integration using libtmux."""

from __future__ import annotations

import os

from libtmux import Server
from libtmux.exc import LibTmuxException
from libtmux.session import Session
from libtmux.window import Window

TUI_EDITORS = {
    "vim",
    "nvim",
    "vi",
    "emacs",
    "nano",
    "helix",
    "hx",
    "micro",
    "kakoune",
    "kak",
}


class TmuxService:
    """Service for interacting with tmux sessions via libtmux."""

    _instance: TmuxService | None = None
    _server: Server | None = None
    _session: Session | None = None

    def __new__(cls) -> TmuxService:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    @property
    def is_inside_tmux(self) -> bool:
        """Check if we are running inside a tmux session."""
        return bool(os.environ.get("TMUX"))

    @property
    def server(self) -> Server | None:
        """Get the tmux server."""
        if not self.is_inside_tmux:
            return None
        if self._server is None:
            try:
                self._server = Server()
            except LibTmuxException:
                return None
        return self._server

    @property
    def session(self) -> Session | None:
        """Get the current tmux session."""
        if not self.is_inside_tmux or self.server is None:
            return None
        if self._session is None:
            try:
                # Get session from TMUX environment variable
                # Format: /tmp/tmux-1000/default,12345,0
                tmux_env = os.environ.get("TMUX", "")
                if tmux_env:
                    # Find the active session
                    for sess in self.server.sessions:
                        if sess.session_attached:
                            self._session = sess
                            break
            except LibTmuxException:
                return None
        return self._session

    @property
    def current_window(self) -> Window | None:
        """Get the current tmux window."""
        if self.session is None:
            return None
        try:
            return self.session.active_window
        except LibTmuxException:
            return None

    def rename_window(self, name: str) -> bool:
        """Rename the current tmux window."""
        window = self.current_window
        if window is None:
            return False
        try:
            window.rename_window(name)
            return True
        except LibTmuxException:
            return False

    def is_tui_editor(self, editor: str) -> bool:
        """Check if an editor is a TUI editor that should run in tmux."""
        # Get base command (handle "emacs -nw" -> "emacs")
        base_cmd = editor.split()[0]
        return base_cmd in TUI_EDITORS

    def find_window(self, name: str) -> Window | None:
        """Find a window by name in the current session."""
        if self.session is None:
            return None
        try:
            for window in self.session.windows:
                if window.window_name == name:
                    return window
        except LibTmuxException:
            pass
        return None

    def create_editor_window(
        self,
        worktree_name: str,
        worktree_path: str,
        editor: str,
    ) -> bool:
        """Create a tmux window with the editor open in the worktree.

        Args:
            worktree_name: Name of the worktree (used for window naming)
            worktree_path: Path to the worktree directory
            editor: Editor command to run

        Returns:
            True if window was created/selected successfully, False otherwise
        """
        if self.session is None:
            return False

        window_name = f"edit:{worktree_name}"

        try:
            # Check if window already exists
            existing_window = self.find_window(window_name)
            if existing_window is not None:
                existing_window.select()
                return True

            # Create new window
            window = self.session.new_window(
                window_name=window_name,
                start_directory=worktree_path,
                attach=True,
            )

            # Send editor command
            pane = window.active_pane
            if pane is not None:
                pane.send_keys(f"{editor} .")

            return True

        except LibTmuxException:
            return False

    def create_claude_window(
        self,
        name: str,
        path: str,
        resume_session_id: str | None = None,
    ) -> bool:
        """Create a tmux window with Claude Code.

        Args:
            name: Name for the window (e.g., worktree name)
            path: Working directory path
            resume_session_id: Optional session ID to resume

        Returns:
            True if window was created/selected successfully, False otherwise
        """
        if self.session is None:
            return False

        window_name = f"claude:{name}"

        try:
            # Check if window already exists
            existing_window = self.find_window(window_name)
            if existing_window is not None:
                existing_window.select()
                return True

            # Create new window
            window = self.session.new_window(
                window_name=window_name,
                start_directory=path,
                attach=True,
            )

            # Send claude command
            pane = window.active_pane
            if pane is not None:
                if resume_session_id:
                    pane.send_keys(f"claude -r {resume_session_id}")
                else:
                    pane.send_keys("claude")

            return True

        except LibTmuxException:
            return False


def get_tmux_service() -> TmuxService:
    """Get the singleton TmuxService instance."""
    return TmuxService()
