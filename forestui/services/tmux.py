"""Service for tmux integration using libtmux."""

from __future__ import annotations

import os
import shlex

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
        """Get the session of the most recently active tmux client.

        Not cached because the active client can change between grouped
        sessions — the user may be viewing forestui from any terminal.
        """
        if not self.is_inside_tmux or self.server is None:
            return None
        try:
            # Get our session group so we only consider clients attached to
            # sessions in the same group — not unrelated tmux sessions.
            group_result = self.server.cmd("display-message", "-p", "#{session_group}")
            our_group = group_result.stdout[0].strip() if group_result.stdout else ""

            # Find the most recently active client in our session group.
            # In grouped sessions, multiple clients are attached to different
            # sessions sharing the same windows. The user who just triggered
            # an action is the most recently active client.
            result = self.server.cmd(
                "list-clients",
                "-F",
                "#{client_activity} #{session_id} #{session_group}",
            )
            if result.stdout:
                best_id: str | None = None
                best_time = -1
                for line in result.stdout:
                    parts = line.strip().split(" ", 2)
                    if len(parts) == 3:
                        activity = int(parts[0])
                        session_id = parts[1]
                        group = parts[2]
                        if our_group and group != our_group:
                            continue
                        if activity > best_time:
                            best_time = activity
                            best_id = session_id
                if best_id:
                    for sess in self.server.sessions:
                        if sess.session_id == best_id:
                            return sess
            # Fallback: first attached session
            for sess in self.server.sessions:
                attached = sess.session_attached
                if attached and int(attached) > 0:
                    return sess
            # Fallback: first session
            if self.server.sessions:
                return self.server.sessions[0]
        except (LibTmuxException, ValueError, TypeError):
            return None
        return None

    @property
    def current_window(self) -> Window | None:
        """Get the tmux window this process is running in.

        Uses the TMUX_PANE environment variable to find our own window,
        rather than the session's active window — which may be a different
        window when forestui is starting up in a background window.
        """
        if self.server is None:
            return None
        pane_id = os.environ.get("TMUX_PANE")
        if not pane_id:
            return None
        try:
            for sess in self.server.sessions:
                for window in sess.windows:
                    for pane in window.panes:
                        if pane.pane_id == pane_id:
                            return window
        except LibTmuxException:
            return None
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

    def ensure_focus_events(self) -> bool:
        """Ensure tmux focus-events option is enabled for proper app refresh.

        Returns:
            True if focus events were enabled successfully, False otherwise.
        """
        if self.server is None:
            return False
        try:
            self.server.cmd("set-option", "-g", "focus-events", "on")
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

            # Create new window with editor as the command (closes when editor exits)
            self.session.new_window(
                window_name=window_name,
                start_directory=worktree_path,
                attach=True,
                window_shell=f"{editor} .",
            )

            return True

        except LibTmuxException:
            return False

    def create_shell_window(self, name: str, path: str) -> bool:
        """Create a tmux window with a shell.

        Args:
            name: Name for the window
            path: Working directory path

        Returns:
            True if window was created successfully, False otherwise
        """
        if self.session is None:
            return False

        base_window_name = f"term:{name}"

        try:
            # Always create a new window with unique name
            window_name = self._find_unique_window_name(base_window_name)

            self.session.new_window(
                window_name=window_name,
                start_directory=path,
                attach=True,
            )

            return True

        except LibTmuxException:
            return False

    def create_mc_window(self, name: str, path: str) -> bool:
        """Create a tmux window with Midnight Commander.

        Args:
            name: Name for the window
            path: Working directory path

        Returns:
            True if window was created successfully, False otherwise
        """
        if self.session is None:
            return False

        base_window_name = f"files:{name}"

        try:
            # Always create a new window with unique name
            window_name = self._find_unique_window_name(base_window_name)

            self.session.new_window(
                window_name=window_name,
                start_directory=path,
                attach=True,
                window_shell="mc",
            )

            return True

        except LibTmuxException:
            return False

    def _find_unique_window_name(self, base_name: str) -> str:
        """Find a unique window name by adding :2, :3, etc. suffix if needed.

        Args:
            base_name: The base window name (e.g., "yolo:cogram")

        Returns:
            A unique window name (e.g., "yolo:cogram" or "yolo:cogram:2")
        """
        if self.session is None:
            return base_name

        try:
            existing_names = {w.window_name for w in self.session.windows}
        except LibTmuxException:
            return base_name

        if base_name not in existing_names:
            return base_name

        # Find next available suffix
        counter = 2
        while f"{base_name}:{counter}" in existing_names:
            counter += 1
        return f"{base_name}:{counter}"

    def create_claude_window(
        self,
        name: str,
        path: str,
        resume_session_id: str | None = None,
        yolo: bool = False,
        custom_command: str | None = None,
        custom_prefix: str | None = None,
    ) -> str | None:
        """Create a tmux window with Claude Code.

        Args:
            name: Name for the window (e.g., worktree name)
            path: Working directory path
            resume_session_id: Optional session ID to resume
            yolo: If True, add --dangerously-skip-permissions flag
            custom_command: Optional custom Claude command (e.g., "claude --model opus")
            custom_prefix: Optional tmux window prefix override (for custom buttons).
                When set, overrides the "claude"/"yolo" prefix and the command is
                used as-is (no --dangerously-skip-permissions is appended).

        Returns:
            The window name if created/selected successfully, None otherwise
        """
        if self.session is None:
            return None

        if custom_prefix:
            base_window_name = f"{custom_prefix}:{name}"
        elif yolo:
            base_window_name = f"yolo:{name}"
        else:
            base_window_name = f"claude:{name}"

        try:
            # Always create a new window with unique name (add :2, :3 suffix if needed)
            window_name = self._find_unique_window_name(base_window_name)

            # Build claude command (closes when claude exits)
            # Use custom_command if provided, otherwise default to "claude"
            cmd = custom_command or "claude"
            # Only append YOLO flag for the built-in YOLO button, not custom buttons
            if yolo and not custom_prefix:
                cmd += " --dangerously-skip-permissions"
            if resume_session_id:
                cmd += f" -r {resume_session_id}"

            # Wrap in interactive shell to support aliases
            # Use shlex.quote to prevent shell injection from custom commands
            shell = os.environ.get("SHELL", "/bin/bash")
            shell_cmd = f"{shell} -ic {shlex.quote(cmd)}"

            self.session.new_window(
                window_name=window_name,
                start_directory=path,
                attach=True,
                window_shell=shell_cmd,
            )

            return window_name

        except LibTmuxException:
            return None


def get_tmux_service() -> TmuxService:
    """Get the singleton TmuxService instance."""
    return TmuxService()
