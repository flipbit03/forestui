"""Command-line interface for forestui."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

import click

from forestui import __version__

INSTALL_DIR = Path.home() / ".forestui-install"


def get_installed_version() -> str | None:
    """Get the currently installed version from the install directory."""
    version_file = INSTALL_DIR / "forestui" / "__init__.py"
    if not version_file.exists():
        return None
    try:
        content = version_file.read_text()
        for line in content.splitlines():
            if line.startswith("__version__"):
                # Extract version from: __version__ = "x.y.z"
                return line.split("=")[1].strip().strip('"').strip("'")
    except OSError:
        pass
    return None


def get_remote_version() -> str | None:
    """Get the latest version from git remote."""
    if not INSTALL_DIR.exists():
        return None
    try:
        # Fetch latest from remote
        subprocess.run(
            ["git", "fetch", "origin", "main"],
            cwd=INSTALL_DIR,
            capture_output=True,
            check=True,
        )
        # Check if there are updates
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=INSTALL_DIR,
            capture_output=True,
            text=True,
            check=True,
        )
        local_head = result.stdout.strip()

        result = subprocess.run(
            ["git", "rev-parse", "origin/main"],
            cwd=INSTALL_DIR,
            capture_output=True,
            text=True,
            check=True,
        )
        remote_head = result.stdout.strip()

        if local_head != remote_head:
            # Get version from remote
            result = subprocess.run(
                ["git", "show", "origin/main:forestui/__init__.py"],
                cwd=INSTALL_DIR,
                capture_output=True,
                text=True,
                check=True,
            )
            for line in result.stdout.splitlines():
                if line.startswith("__version__"):
                    return line.split("=")[1].strip().strip('"').strip("'")
    except (subprocess.CalledProcessError, OSError):
        pass
    return None


def self_update() -> None:
    """Update forestui from git and reinstall."""
    if not INSTALL_DIR.exists():
        click.echo("Error: forestui was not installed via install.sh", err=True)
        click.echo(f"Expected install directory: {INSTALL_DIR}", err=True)
        sys.exit(1)

    click.echo("Checking for updates...")

    current_version = __version__
    remote_version = get_remote_version()

    if remote_version is None:
        click.echo("Already up to date.")
        return

    if remote_version == current_version:
        click.echo(f"Already at latest version ({current_version}).")
        return

    click.echo(f"Updating from {current_version} to {remote_version}...")

    try:
        # Pull latest changes
        subprocess.run(
            ["git", "pull", "origin", "main"],
            cwd=INSTALL_DIR,
            check=True,
        )

        # Reinstall with uv
        subprocess.run(
            ["uv", "tool", "install", ".", "--force"],
            cwd=INSTALL_DIR,
            check=True,
        )

        click.echo(f"Updated successfully to {remote_version}!")
        click.echo("Run 'forestui' to use the new version.")

    except subprocess.CalledProcessError as e:
        click.echo(f"Update failed: {e}", err=True)
        sys.exit(1)


def rename_tmux_window(name: str) -> None:
    """Rename the current tmux window."""
    if os.environ.get("TMUX"):
        subprocess.run(
            ["tmux", "rename-window", name],
            capture_output=True,
        )


def ensure_tmux(forest_path: str | None) -> None:
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

    # Build the forestui command with optional path argument
    forestui_cmd = "forestui"
    if forest_path:
        forestui_cmd = f"forestui {forest_path}"

    # Exec into tmux with forestui
    # -A: attach to existing session or create new one
    # -s forestui: session name
    os.execvp("tmux", ["tmux", "new-session", "-A", "-s", "forestui", forestui_cmd])


@click.command()
@click.argument("forest_path", required=False, default=None)
@click.option(
    "--self-update",
    "do_self_update",
    is_flag=True,
    help="Update forestui to the latest version",
)
@click.version_option(version=__version__, prog_name="forestui")
def main(forest_path: str | None, do_self_update: bool) -> None:
    """forestui - Git Worktree Manager

    A terminal UI for managing Git worktrees, inspired by forest for macOS.

    FOREST_PATH: Optional path to forest directory (default: ~/forest)
    """
    if do_self_update:
        self_update()
        return

    ensure_tmux(forest_path)
    rename_tmux_window("forestui")

    # Import here to avoid circular imports and speed up --help/--version
    from forestui.app import run_app
    from forestui.services.settings import set_forest_path

    set_forest_path(forest_path)
    run_app()


if __name__ == "__main__":
    main()
