#!/bin/sh
# Install forestui — a terminal UI for managing Git worktrees
# Usage: curl -fsSL https://raw.githubusercontent.com/flipbit03/forestui/main/install.sh | sh
set -e

REPO="flipbit03/forestui"
INSTALL_DIR="${FORESTUI_INSTALL_DIR:-$HOME/.local/bin}"

# tmux is a hard requirement: forestui re-executes itself into a tmux session.
if ! command -v tmux >/dev/null 2>&1; then
  echo "forestui requires tmux. Install it first:" >&2
  echo "  macOS:  brew install tmux" >&2
  echo "  Ubuntu: sudo apt install tmux" >&2
  echo "  Fedora: sudo dnf install tmux" >&2
  exit 1
fi

# Detect OS and architecture.
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Linux)  OS_TAG="linux" ;;
  Darwin) OS_TAG="macos" ;;
  *) echo "Unsupported OS: ${OS}" >&2; exit 1 ;;
esac

case "${ARCH}" in
  x86_64|amd64)  ARCH_TAG="x86_64" ;;
  aarch64|arm64) ARCH_TAG="aarch64" ;;
  *) echo "Unsupported architecture: ${ARCH}" >&2; exit 1 ;;
esac

# macOS x86_64 binaries are not provided.
if [ "${OS}" = "Darwin" ] && [ "${ARCH_TAG}" = "x86_64" ]; then
  echo "macOS x86_64 binaries are not provided. Use: cargo install forestui" >&2
  exit 1
fi

ASSET="forestui_${OS_TAG}_${ARCH_TAG}"

# Get latest release tag.
echo "Fetching latest release..."
TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | cut -d'"' -f4)
if [ -z "${TAG}" ]; then
  echo "Failed to determine latest release" >&2
  exit 1
fi
echo "Latest release: ${TAG}"

URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
echo "Downloading ${ASSET}..."
TMPDIR=$(mktemp -d)
trap 'rm -rf "${TMPDIR}"' EXIT

curl -fsSL "${URL}" -o "${TMPDIR}/forestui"

# The release publishes a checksum beside every asset. An installer that
# downloads a binary and runs it should check it.
if curl -fsSL "${URL}.sha256" -o "${TMPDIR}/${ASSET}.sha256" 2>/dev/null; then
  SHA_TOOL=""
  if command -v shasum >/dev/null 2>&1; then
    SHA_TOOL="shasum -a 256"
  elif command -v sha256sum >/dev/null 2>&1; then
    SHA_TOOL="sha256sum"
  fi

  if [ -z "${SHA_TOOL}" ]; then
    echo "Neither shasum nor sha256sum found; skipping checksum verification." >&2
  else
    EXPECTED=$(cut -d' ' -f1 < "${TMPDIR}/${ASSET}.sha256")
    ACTUAL=$($SHA_TOOL "${TMPDIR}/forestui" | cut -d' ' -f1)
    if [ "${EXPECTED}" != "${ACTUAL}" ]; then
      echo "Checksum mismatch for ${ASSET}. Refusing to install." >&2
      exit 1
    fi
    echo "Checksum verified."
  fi
else
  echo "No published checksum for ${ASSET}; skipping verification." >&2
fi

# Install.
mkdir -p "${INSTALL_DIR}"
mv "${TMPDIR}/forestui" "${INSTALL_DIR}/forestui"
chmod +x "${INSTALL_DIR}/forestui"

echo "Installed forestui ${TAG} to ${INSTALL_DIR}/forestui"

# Check if install dir is in PATH.
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *) echo "Add ${INSTALL_DIR} to your PATH:"; echo "  export PATH=\"${INSTALL_DIR}:\$PATH\"" ;;
esac

# The Python build installed itself as a uv tool, and ~/.local/bin usually
# precedes ~/.cargo/bin, so a leftover install would keep winning.
if command -v uv >/dev/null 2>&1 && uv tool list 2>/dev/null | grep -q '^forestui'; then
  echo ""
  echo "Note: an older Python forestui is still installed via uv. Remove it with:"
  echo "  uv tool uninstall forestui"
fi
