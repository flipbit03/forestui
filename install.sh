#!/usr/bin/env bash
#
# forestui installer
#
# Downloads a prebuilt binary from GitHub Releases, falling back to
# `cargo install` when no binary matches this platform.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/flipbit03/forestui/main/install.sh | bash
#

set -e

REPO="flipbit03/forestui"
INSTALL_DIR="${FORESTUI_INSTALL_DIR:-$HOME/.local/bin}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1" >&2; exit 1; }
check_command() { command -v "$1" &> /dev/null; }

info "forestui installer"
echo ""

# tmux is a hard requirement: forestui re-executes itself into a tmux session.
if ! check_command tmux; then
    error "tmux is not installed. Please install tmux first:

    macOS:  brew install tmux
    Ubuntu: sudo apt install tmux
    Fedora: sudo dnf install tmux"
fi

detect_target() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os/$arch" in
        Darwin/arm64)  echo "aarch64-apple-darwin" ;;
        Darwin/x86_64) echo "x86_64-apple-darwin" ;;
        Linux/x86_64)  echo "x86_64-unknown-linux-gnu" ;;
        *)             echo "" ;;
    esac
}

install_from_source() {
    if ! check_command cargo; then
        error "No prebuilt binary for this platform and cargo is not installed.

    Install Rust, then re-run this script:
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    fi
    info "Building forestui from crates.io with cargo..."
    cargo install forestui --locked
    info "Installation complete!"
}

TARGET="$(detect_target)"

if [ -z "$TARGET" ]; then
    warn "No prebuilt binary for $(uname -s)/$(uname -m)."
    install_from_source
else
    info "Detected platform: $TARGET"

    VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | grep '"tag_name"' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"

    if [ -z "$VERSION" ]; then
        warn "Could not determine the latest release."
        install_from_source
    else
        ARCHIVE="forestui-${VERSION}-${TARGET}.tar.gz"
        URL="https://github.com/$REPO/releases/download/$VERSION/$ARCHIVE"
        TMPDIR="$(mktemp -d)"
        trap 'rm -rf "$TMPDIR"' EXIT

        info "Downloading forestui $VERSION..."
        if curl -fsSL "$URL" -o "$TMPDIR/$ARCHIVE"; then
            tar -xzf "$TMPDIR/$ARCHIVE" -C "$TMPDIR"
            mkdir -p "$INSTALL_DIR"
            install -m 755 "$TMPDIR/forestui" "$INSTALL_DIR/forestui"
            info "Installed to $INSTALL_DIR/forestui"
        else
            warn "No prebuilt archive at $URL."
            install_from_source
        fi
    fi
fi

echo ""
echo "  Run 'forestui' to start the application."
echo "  Run 'forestui --help' for usage information."
echo ""

if ! check_command forestui; then
    warn "forestui was installed but is not in your PATH."
    echo ""
    echo "  Add the following to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
    echo ""
    echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
    echo "  Then restart your shell."
fi

# The Python build installed itself as a uv tool; point users at the cleanup.
if check_command uv && uv tool list 2>/dev/null | grep -q '^forestui'; then
    echo ""
    info "Note: an older Python forestui is still installed via uv."
    echo "  Remove it so the Rust binary takes precedence:"
    echo "    uv tool uninstall forestui"
fi
