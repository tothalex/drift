#!/bin/sh
# drift installer — https://github.com/tothalex/drift
#
#   curl -fsSL https://tothalex.github.io/drift/install.sh | sh
#
# Installs the latest release to ~/.local/bin (override: DRIFT_INSTALL_DIR).
set -eu

REPO="tothalex/drift"
BIN="drift"
INSTALL_DIR="${DRIFT_INSTALL_DIR:-$HOME/.local/bin}"

log() { printf '  %s\n' "$1"; }
err() { printf 'error: %s\n' "$1" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || err "'$1' is required"; }

main() {
    printf '\n  ~ drift installer\n\n'

    OS="$(uname -s)"
    case "$OS" in
        Linux)  os="linux" ;;
        Darwin) os="macos" ;;
        MINGW*|MSYS*|CYGWIN*)
            err "on Windows, download drift-windows-x86_64.zip from https://github.com/$REPO/releases/latest" ;;
        *)      err "unsupported OS: $OS" ;;
    esac

    ARCH="$(uname -m)"
    case "$ARCH" in
        x86_64|amd64)  arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)             err "unsupported architecture: $ARCH" ;;
    esac

    need curl
    need tar

    NAME="${BIN}-${os}-${arch}"
    URL="https://github.com/${REPO}/releases/latest/download/${NAME}.tar.gz"
    log "detected ${os}/${arch}"
    log "downloading ${URL}"

    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT
    curl -fsSL --retry 3 --connect-timeout 10 -o "$TMP/$NAME.tar.gz" "$URL" \
        || err "download failed — is there a published release?"
    tar -xzf "$TMP/$NAME.tar.gz" -C "$TMP"

    mkdir -p "$INSTALL_DIR"
    mv "$TMP/$BIN" "$INSTALL_DIR/$BIN"
    chmod +x "$INSTALL_DIR/$BIN"
    log "installed $INSTALL_DIR/$BIN"

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            log ""
            log "note: $INSTALL_DIR is not on your PATH; add this to your shell profile:"
            log "  export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac

    printf '\n  done — run: %s\n\n' "$BIN"
}

main "$@"
