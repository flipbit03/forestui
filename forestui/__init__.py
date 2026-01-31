"""forestui - A Terminal UI for managing Git worktrees."""

from importlib.metadata import PackageNotFoundError, version

try:
    __version__ = version("forestui")
except PackageNotFoundError:
    # Package not installed (running from source without install)
    __version__ = "0.0.0"
