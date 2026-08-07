#!/usr/bin/env bash
#
# Build Luft in release mode and install the binary for the current user.
#
# Steps:
#   1. cargo build --release
#   2. Copy binary to ~/.luft/bin/luft
#   3. Verify with --version
#   4. Run `luft install` (skill bridges + MCP config)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RELEASE_BINARY="$REPO_ROOT/target/release/luft"
INSTALL_DIR="$HOME/.luft/bin"
INSTALLED_BINARY="$INSTALL_DIR/luft"

echo "==> Building Luft (release)"
(cd "$REPO_ROOT" && cargo build --release)

if [[ ! -f "$RELEASE_BINARY" ]]; then
    echo "ERROR: Release binary not found: $RELEASE_BINARY" >&2
    exit 1
fi

echo "==> Installing binary to $INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
cp -f "$RELEASE_BINARY" "$INSTALLED_BINARY"
chmod +x "$INSTALLED_BINARY"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) export PATH="$INSTALL_DIR:$PATH" ;;
esac

echo "==> Verifying installed binary"
"$INSTALLED_BINARY" --version

echo "==> Running luft install"
"$INSTALLED_BINARY" install

echo ""
echo "==> Luft installed successfully: $INSTALLED_BINARY"
