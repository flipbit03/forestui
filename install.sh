#!/usr/bin/env bash
#
# forestui installer
#
# This script installs forestui by:
#   1. Checking for required dependencies (uv, tmux, git)
#   2. Cloning the repository to ~/.forestui-install
#   3. Installing via uv tool install
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/flipbit03/forestui/main/install.sh | bash
#
# Or clone and run locally:
#   ./install.sh
#

set -e

REPO_URL="https://github.com/flipbit03/forestui.git"
INSTALL_DIR="$HOME/.forestui-install"

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

# Check for required commands
check_command() {
    if ! command -v "$1" &> /dev/null; then
        return 1
    fi
    return 0
}

info "forestui installer"
echo ""

# Check for git
if ! check_command git; then
    error "git is not installed. Please install git first."
fi

# Check for uv
if ! check_command uv; then
    error "uv is not installed. Please install uv first:

    curl -LsSf https://astral.sh/uv/install.sh | sh

    Or visit: https://docs.astral.sh/uv/getting-started/installation/"
fi

# Check for tmux
if ! check_command tmux; then
    error "tmux is not installed. Please install tmux first:

    macOS:  brew install tmux
    Ubuntu: sudo apt install tmux
    Fedora: sudo dnf install tmux"
fi

info "All dependencies found: git, uv, tmux"

# Clone or update repository
if [ -d "$INSTALL_DIR" ]; then
    info "Updating existing installation at $INSTALL_DIR"
    cd "$INSTALL_DIR"
    git fetch origin main
    git reset --hard origin/main
else
    info "Cloning repository to $INSTALL_DIR"
    git clone "$REPO_URL" "$INSTALL_DIR"
    cd "$INSTALL_DIR"
fi

# Install with uv
info "Installing forestui with uv tool install..."
uv tool install . --force

# "forestui" is now available in your $PATH

echo ""
info "Installation complete!"
echo ""
echo "  Run 'forestui' to start the application."
echo "  Run 'forestui --help' for usage information."
echo "  Run 'forestui --self-update' to update to the latest version."
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
