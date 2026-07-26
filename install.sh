#!/bin/sh
# Luft binary installer (Linux / macOS).
#
# Install latest:
#   curl -fsSL https://raw.githubusercontent.com/hi-youichi/luft/main/install.sh | sh
# Install a specific version:
#   curl -fsSL https://raw.githubusercontent.com/hi-youichi/luft/main/install.sh | sh -s -- --version v0.3.3
# Run directly:
#   sh install.sh [--version v0.3.3] [--variant gnu|musl] [--install-dir DIR] [--skip-verify]

set -eu

REPO="hi-youichi/luft"
INSTALL_DIR="${LUFT_INSTALL_DIR:-${HOME}/.luft/bin}"
VERSION=""
VARIANT=""
SKIP_VERIFY=0
SHOW_HELP=0

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || { echo "--version requires a value" >&2; exit 1; }
            VERSION="$2"; shift 2 ;;
        --variant)
            [ $# -ge 2 ] || { echo "--variant requires a value" >&2; exit 1; }
            VARIANT="$2"; shift 2 ;;
        --install-dir)
            [ $# -ge 2 ] || { echo "--install-dir requires a value" >&2; exit 1; }
            INSTALL_DIR="$2"; shift 2 ;;
        --skip-verify)
            SKIP_VERIFY=1; shift ;;
        -h|--help)
            SHOW_HELP=1; shift ;;
        *)
            echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

if [ "$SHOW_HELP" = 1 ]; then
    cat <<EOF
Luft installer

Usage:
  curl -fsSL https://raw.githubusercontent.com/hi-youichi/luft/main/install.sh | sh
  curl -fsSL https://raw.githubusercontent.com/hi-youichi/luft/main/install.sh | sh -s -- [options]
  sh install.sh [options]

Options:
  --version <ver>      Version to install, e.g. v0.3.3 (default: latest)
  --variant <variant>  Linux x86_64 build: gnu|musl (default: gnu)
  --install-dir <dir>  Install directory (default: ~/.luft/bin)
  --skip-verify        Skip SHA256 verification (not recommended)
  -h, --help           Show this help

Environment:
  LUFT_INSTALL_DIR     Override install directory
EOF
    exit 0
fi

# ---- detect platform ----
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$ARCH" in
    x86_64|amd64) ARCH_NORM="x86_64" ;;
    aarch64|arm64) ARCH_NORM="aarch64" ;;
    *)
        echo "Unsupported architecture: $ARCH" >&2
        exit 1
        ;;
esac

case "$OS" in
    Darwin)
        ASSET="luft-${ARCH_NORM}-apple-darwin"
        EXT="tar.gz"
        BINARY="luft"
        ;;
    Linux)
        if [ "$ARCH_NORM" = "aarch64" ]; then
            ASSET="luft-aarch64-linux-gnu"
        else
            VARIANT="${VARIANT:-gnu}"
            case "$VARIANT" in
                gnu)  ASSET="luft-x86_64-linux-gnu" ;;
                musl) ASSET="luft-x86_64-linux-musl" ;;
                *) echo "Invalid variant: $VARIANT (use 'gnu' or 'musl')" >&2; exit 1 ;;
            esac
        fi
        EXT="tar.gz"
        BINARY="luft"
        ;;
    *)
        echo "Unsupported OS: $OS" >&2
        exit 1
        ;;
esac

ARCHIVE="${ASSET}.${EXT}"

# ---- build download url ----
if [ -z "$VERSION" ]; then
    DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ARCHIVE}"
    VERSION_DISPLAY="latest"
else
    VERSION="$(echo "$VERSION" | sed 's/^v//')"
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ARCHIVE}"
    VERSION_DISPLAY="v${VERSION}"
fi

echo "==> Installing luft ${VERSION_DISPLAY} (${ARCHIVE}) to ${INSTALL_DIR}"

# ---- download helper ----
download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        echo "Neither curl nor wget is installed" >&2
        return 1
    fi
}

# ---- temp workspace ----
TMPDIR_WORK="$(mktemp -d 2>/dev/null || mktemp -d -t luft-install)"
trap 'rm -rf "$TMPDIR_WORK"' EXIT INT TERM

echo "==> Downloading ${ARCHIVE}"
download "$DOWNLOAD_URL" "${TMPDIR_WORK}/${ARCHIVE}"

# ---- checksum ----
if [ "$SKIP_VERIFY" != 1 ]; then
    if command -v sha256sum >/dev/null 2>&1; then
        SHACMD="sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        SHACMD="shasum -a 256"
    else
        echo "Neither sha256sum nor shasum is installed; use --skip-verify to bypass" >&2
        exit 1
    fi

    echo "==> Downloading checksum"
    download "${DOWNLOAD_URL}.sha256" "${TMPDIR_WORK}/${ARCHIVE}.sha256"

    echo "==> Verifying SHA256"
    (cd "$TMPDIR_WORK" && $SHACMD -c "${ARCHIVE}.sha256") >/dev/null
    echo "    OK"
fi

# ---- extract ----
echo "==> Extracting"
tar -xzf "${TMPDIR_WORK}/${ARCHIVE}" -C "$TMPDIR_WORK"
if [ ! -f "${TMPDIR_WORK}/${BINARY}" ]; then
    echo "Expected binary '${BINARY}' not found in archive" >&2
    exit 1
fi

# ---- install ----
mkdir -p "$INSTALL_DIR"
mv -f "${TMPDIR_WORK}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
chmod +x "${INSTALL_DIR}/${BINARY}"

# ---- PATH ----
PATH_LINE="export PATH=\"${INSTALL_DIR}:\$PATH\""

add_path_to_rc() {
    _rc="$1"
    if ! grep -qF "${INSTALL_DIR}" "$_rc" 2>/dev/null; then
        printf '\n# Added by luft installer\n%s\n' "$PATH_LINE" >> "$_rc"
        echo "    Updated: $_rc"
    else
        echo "    Already present: $_rc"
    fi
}

echo "==> Updating PATH"
_primary="${HOME}/.profile"
_shell_name="$(basename "${SHELL:-sh}")"
case "$_shell_name" in
    zsh)  _primary="${HOME}/.zshrc" ;;
    bash) _primary="${HOME}/.bashrc" ;;
esac
[ -f "$_primary" ] || touch "$_primary"
add_path_to_rc "$_primary"
for _rc in "${HOME}/.bashrc" "${HOME}/.zshrc" "${HOME}/.profile"; do
    [ "$_rc" = "$_primary" ] && continue
    [ -f "$_rc" ] && add_path_to_rc "$_rc"
done

# ---- verify ----
echo "==> Verifying installation"
"${INSTALL_DIR}/${BINARY}" --version

# ---- post-install setup (best effort) ----
echo "==> Running 'luft install' (post-install setup)"
if "${INSTALL_DIR}/${BINARY}" install; then
    echo "    OK"
else
    _rc=$?
    echo "    'luft install' exited with $_rc (non-fatal)."
    echo "    Re-run it later after installing an agent:  ${INSTALL_DIR}/${BINARY} install"
fi

echo
echo "==> luft ${VERSION_DISPLAY} installed to ${INSTALL_DIR}/${BINARY}"
echo
echo "Next steps:"
echo "  Restart your shell, or run:  export PATH=\"${INSTALL_DIR}:\$PATH\""
echo "  Then:  luft --version"
if [ "$_shell_name" = "fish" ]; then
    echo "  (fish users: add  set -gx PATH ${INSTALL_DIR} \$PATH  to ~/.config/fish/config.fish)"
fi
