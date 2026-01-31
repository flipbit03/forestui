#!/usr/bin/env bash
#
# forestui installer
#
# Installs forestui from PyPI using uv.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/flipbit03/forestui/main/install.sh | bash
#

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
    exit 1
}

check_command() {
    command -v "$1" &> /dev/null
}

info "forestui installer"
echo ""

# Check for tmux (required dependency)
if ! check_command tmux; then
    error "tmux is not installed. Please install tmux first:

    macOS:  brew install tmux
    Ubuntu: sudo apt install tmux
    Fedora: sudo dnf install tmux"
fi

# Install uv if not present
if ! check_command uv; then
    info "Installing uv (Python package manager)..."
    curl -LsSf https://astral.sh/uv/install.sh | sh

    # Add to PATH for this session
    export PATH="$HOME/.local/bin:$PATH"

    if ! check_command uv; then
        error "Failed to install uv. Please install manually: https://docs.astral.sh/uv/"
    fi
    info "uv installed successfully"
fi

# Install forestui from PyPI
info "Installing forestui from PyPI..."
if uv tool install forestui; then
    echo ""
    info "Installation complete!"
else
    error "Failed to install forestui from PyPI"
fi

echo ""
echo "  Run 'forestui' to start the application."
echo "  Run 'forestui --help' for usage information."
echo ""

# Check if uv tools are in PATH
if ! check_command forestui; then
    warn "forestui was installed but is not in your PATH."
    echo ""
    echo "  Add the following to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
    echo ""
    echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo ""
    echo "  Then restart your shell or run: source ~/.bashrc"
fi

# Notify about old installation directory
OLD_INSTALL_DIR="$HOME/.forestui-install"
if [ -d "$OLD_INSTALL_DIR" ]; then
    echo ""
    info "Note: Found old git-based installation at $OLD_INSTALL_DIR"
    echo "  This directory is no longer needed. You can remove it with:"
    echo "    rm -rf $OLD_INSTALL_DIR"
fi
