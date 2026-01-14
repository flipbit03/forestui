"""Service for tmux integration."""

from __future__ import annotations

import os
import subprocess

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
    """Service for interacting with tmux sessions."""

    _instance: TmuxService | None = None

    def __new__(cls) -> TmuxService:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    @property
    def is_inside_tmux(self) -> bool:
        """Check if we are running inside a tmux session."""
        return bool(os.environ.get("TMUX"))

    @property
    def session_name(self) -> str | None:
        """Get the current tmux session name."""
        if not self.is_inside_tmux:
            return None
        try:
            result = subprocess.run(
                ["tmux", "display-message", "-p", "#S"],
                capture_output=True,
                text=True,
                check=True,
            )
            return result.stdout.strip()
        except (subprocess.CalledProcessError, FileNotFoundError):
            return None

    def is_tui_editor(self, editor: str) -> bool:
        """Check if an editor is a TUI editor that should run in tmux."""
        # Get base command (handle "emacs -nw" -> "emacs")
        base_cmd = editor.split()[0]
        return base_cmd in TUI_EDITORS

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
        if not self.is_inside_tmux:
            return False

        window_name = f"edit:{worktree_name}"

        try:
            # Check if window already exists
            result = subprocess.run(
                ["tmux", "list-windows", "-F", "#{window_name}"],
                capture_output=True,
                text=True,
                check=True,
            )
            existing_windows = result.stdout.strip().split("\n")

            if window_name in existing_windows:
                # Window exists, select it
                subprocess.run(
                    ["tmux", "select-window", "-t", window_name],
                    check=True,
                )
                return True

            # Create new window
            subprocess.run(
                [
                    "tmux",
                    "new-window",
                    "-n",
                    window_name,
                    "-c",
                    worktree_path,
                ],
                check=True,
            )

            # Send editor command to the new window
            subprocess.run(
                ["tmux", "send-keys", f"{editor} .", "Enter"],
                check=True,
            )

            return True

        except (subprocess.CalledProcessError, FileNotFoundError):
            return False


def get_tmux_service() -> TmuxService:
    """Get the singleton TmuxService instance."""
    return TmuxService()
