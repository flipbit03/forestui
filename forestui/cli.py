"""Command-line interface for forestui."""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

import click

from forestui import __version__


def get_window_name(dev_mode: bool = False) -> str:
    """Get the tmux window name based on dev mode flag."""
    if dev_mode:
        from datetime import datetime

        hhmm = datetime.now().strftime("%H%M")
        return f"forestui-dev-{hhmm}"
    return "forestui"


def rename_tmux_window(name: str) -> None:
    """Rename the current tmux window."""
    from forestui.services.tmux import get_tmux_service

    get_tmux_service().rename_window(name)


def slugify(text: str) -> str:
    """Convert text to a safe slug for tmux session names."""
    import re

    # Convert to lowercase and replace spaces/special chars with dashes
    slug = re.sub(r"[^a-zA-Z0-9]+", "-", text.lower())
    # Remove leading/trailing dashes
    return slug.strip("-")


def ensure_tmux(
    forest_path: str | None,
    debug_mode: bool = False,
    no_self_update: bool = False,
    dev_mode: bool = False,
) -> None:
    """Ensure forestui is running inside tmux, or exec into tmux."""
    # Already inside tmux - good to go
    if os.environ.get("TMUX"):
        return

    # Check if tmux is available
    tmux_path = shutil.which("tmux")
    if not tmux_path:
        click.echo("Error: forestui requires tmux to be installed.", err=True)
        click.echo("", err=True)
        click.echo("Install tmux:", err=True)
        click.echo("  macOS:  brew install tmux", err=True)
        click.echo("  Ubuntu: sudo apt install tmux", err=True)
        click.echo("  Fedora: sudo dnf install tmux", err=True)
        sys.exit(1)

    # Determine forest folder name for session naming
    if forest_path:
        forest_folder = Path(forest_path).expanduser().resolve().name
    else:
        forest_folder = "forest"  # default ~/forest

    session_name = f"forestui-{slugify(forest_folder)}"

    # Build the forestui command with arguments
    forestui_cmd = "forestui"
    if debug_mode:
        forestui_cmd += " --debug"
    if no_self_update:
        forestui_cmd += " --no-self-update"
    if dev_mode:
        forestui_cmd += " --dev"
    if forest_path:
        forestui_cmd += f" {forest_path}"

    # Check if session already exists
    import subprocess

    result = subprocess.run(
        ["tmux", "has-session", "-t", session_name],
        capture_output=True,
    )
    session_exists = result.returncode == 0

    if session_exists:
        # Session exists: check if forestui window is already running
        result = subprocess.run(
            ["tmux", "select-window", "-t", f"{session_name}:forestui"],
            capture_output=True,
        )
        forestui_window_exists = result.returncode == 0

        if not forestui_window_exists:
            # forestui was killed but session remains - create new window
            subprocess.run(
                [
                    "tmux",
                    "new-window",
                    "-t",
                    session_name,
                    "-n",
                    "forestui",
                    forestui_cmd,
                ],
            )

        os.execvp("tmux", ["tmux", "attach-session", "-t", session_name])
    else:
        # No session: create new one with forestui as initial command
        os.execvp("tmux", ["tmux", "new-session", "-s", session_name, forestui_cmd])


@click.command()
@click.argument("forest_path", required=False, default=None)
@click.option(
    "--no-self-update",
    "no_self_update",
    is_flag=True,
    help="Disable automatic updates on startup",
)
@click.option(
    "--debug",
    "debug_mode",
    is_flag=True,
    help="Run with Textual devtools enabled",
)
@click.option(
    "--dev",
    "dev_mode",
    is_flag=True,
    help="Dev mode: use timestamped window name (forestui-dev-HHMM)",
)
@click.version_option(version=__version__, prog_name="forestui")
def main(
    forest_path: str | None, no_self_update: bool, debug_mode: bool, dev_mode: bool
) -> None:
    """forestui - Git Worktree Manager

    A terminal UI for managing Git worktrees, inspired by forest for macOS.

    FOREST_PATH: Optional path to forest directory (default: ~/forest)
    """
    ensure_tmux(forest_path, debug_mode, no_self_update, dev_mode)

    if debug_mode:
        os.environ["TEXTUAL"] = "devtools"

    if no_self_update:
        os.environ["FORESTUI_NO_AUTO_UPDATE"] = "1"

    rename_tmux_window(get_window_name(dev_mode))

    # Import here to avoid circular imports and speed up --help/--version
    from forestui.app import run_app
    from forestui.services.settings import set_forest_path

    set_forest_path(forest_path)
    run_app()


if __name__ == "__main__":
    main()
