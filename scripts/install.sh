#!/bin/sh
# agentum installer — curl -fsSL https://agentum.dev/install.sh | sh
#
# Detects OS/arch, downloads the latest release from GitHub, and drops the
# binary into ~/.local/bin (override with INSTALL_DIR).
#
# SPDX-License-Identifier: MIT

set -eu

REPO="anthropics/agentum"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

info()  { printf '  \033[1;34m%s\033[0m %s\n' "$1" "$2"; }
ok()    { printf '  \033[1;32m%s\033[0m %s\n' "$1" "$2"; }
warn()  { printf '  \033[1;33m%s\033[0m %s\n' "$1" "$2" >&2; }
err()   { printf '  \033[1;31m%s\033[0m %s\n' "error" "$1" >&2; exit 1; }

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)   OS="linux" ;;
        Darwin)  OS="darwin" ;;
        *)       err "unsupported OS: $OS" ;;
    esac

    case "$ARCH" in
        x86_64|amd64)   ARCH="x86_64" ;;
        aarch64|arm64)  ARCH="aarch64" ;;
        *)              err "unsupported architecture: $ARCH" ;;
    esac

    PLATFORM="${OS}-${ARCH}"
}

detect_latest_version() {
    if command -v curl >/dev/null 2>&1; then
        VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"//;s/".*//')"
    elif command -v wget >/dev/null 2>&1; then
        VERSION="$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"//;s/".*//')"
    else
        err "curl or wget required"
    fi

    [ -n "$VERSION" ] || err "could not determine latest version"
}

download() {
    ASSET="agentum-${VERSION}-${PLATFORM}.tar.gz"
    URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"
    CHECKSUM_URL="${URL}.sha256"

    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    info "downloading" "$ASSET"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$URL" -o "$TMPDIR/$ASSET"
        # best-effort checksum — don't fail if the sidecar doesn't exist yet
        curl -fsSL "$CHECKSUM_URL" -o "$TMPDIR/${ASSET}.sha256" 2>/dev/null || true
    else
        wget -q "$URL" -O "$TMPDIR/$ASSET"
        wget -q "$CHECKSUM_URL" -O "$TMPDIR/${ASSET}.sha256" 2>/dev/null || true
    fi

    if [ -f "$TMPDIR/${ASSET}.sha256" ] && command -v sha256sum >/dev/null 2>&1; then
        info "verifying" "sha256 checksum"
        (cd "$TMPDIR" && sha256sum -c "${ASSET}.sha256" >/dev/null 2>&1) \
            || err "checksum verification failed"
    fi

    tar -xzf "$TMPDIR/$ASSET" -C "$TMPDIR"
}

install_binary() {
    mkdir -p "$INSTALL_DIR"
    mv "$TMPDIR/agentum" "$INSTALL_DIR/agentum"
    chmod +x "$INSTALL_DIR/agentum"
}

check_path() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            warn "note" "$INSTALL_DIR is not in your PATH"
            warn ""     "add it with:"
            warn ""     "  export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac
}

check_tmux() {
    if ! command -v tmux >/dev/null 2>&1; then
        warn "note" "tmux not found — agentum requires tmux to manage sessions"
        case "$(uname -s)" in
            Linux)  warn "" "  apt install tmux  or  dnf install tmux" ;;
            Darwin) warn "" "  brew install tmux" ;;
        esac
    fi
}

main() {
    printf '\n  \033[1magentum installer\033[0m\n\n'

    detect_platform
    info "platform" "$PLATFORM"

    detect_latest_version
    info "version" "$VERSION"

    download
    install_binary

    ok "installed" "$INSTALL_DIR/agentum"
    printf '\n'

    check_path
    check_tmux

    printf '\n'
    ok "done" "run \`agentum doctor\` to verify your setup"
    printf '\n'
}

main
