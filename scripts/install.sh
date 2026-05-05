#!/bin/sh
# agentum installer — curl -fsSL https://agentum.dev/install.sh | sh
#
# Detects OS/arch, downloads the latest release from GitHub, and drops the
# binary into ~/.local/bin (override with INSTALL_DIR).
#
# SPDX-License-Identifier: MIT

set -eu

REPO="mateocerquetella/agentum"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

info()  { printf '  \033[1;34m%s\033[0m %s\n' "$1" "$2"; }
ok()    { printf '  \033[1;32m%s\033[0m %s\n' "$1" "$2"; }
warn()  { printf '  \033[1;33m%s\033[0m %s\n' "$1" "$2" >&2; }
err()   { printf '  \033[1;31m%s\033[0m %s\n' "error" "$1" >&2; exit 1; }

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    # Map (uname -s, uname -m) → Rust target triple, matching the asset
    # names produced by .github/workflows/release.yml.
    case "$OS-$ARCH" in
        Darwin-x86_64|Darwin-amd64)        PLATFORM="x86_64-apple-darwin" ;;
        Darwin-arm64|Darwin-aarch64)       PLATFORM="aarch64-apple-darwin" ;;
        Linux-x86_64|Linux-amd64)          PLATFORM="x86_64-unknown-linux-gnu" ;;
        Linux-aarch64|Linux-arm64)         PLATFORM="aarch64-unknown-linux-gnu" ;;
        *)                                 err "unsupported platform: $OS $ARCH" ;;
    esac
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
    BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    info "downloading" "$ASSET"
    fetch "$BASE_URL/$ASSET" "$TMPDIR/$ASSET"
    # SHA256SUMS is a single sidecar listing every asset; pull it once.
    fetch "$BASE_URL/SHA256SUMS" "$TMPDIR/SHA256SUMS" || true

    if [ -f "$TMPDIR/SHA256SUMS" ] && command -v sha256sum >/dev/null 2>&1; then
        info "verifying" "sha256 checksum"
        expected="$(grep " ${ASSET}\$" "$TMPDIR/SHA256SUMS" | awk '{print $1}')"
        if [ -z "$expected" ]; then
            warn "skipped" "no checksum entry for $ASSET in SHA256SUMS"
        else
            actual="$(sha256sum "$TMPDIR/$ASSET" | awk '{print $1}')"
            [ "$expected" = "$actual" ] || err "checksum mismatch for $ASSET"
        fi
    fi

    tar -xzf "$TMPDIR/$ASSET" -C "$TMPDIR"
    EXTRACTED="$TMPDIR/agentum-${VERSION}-${PLATFORM}"
}

# Unified curl/wget wrapper.
fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
    else
        err "curl or wget required"
    fi
}

install_binary() {
    mkdir -p "$INSTALL_DIR"
    # release.yml stages files under agentum-<ver>-<target>/agentum
    src="$EXTRACTED/agentum"
    [ -f "$src" ] || src="$TMPDIR/agentum"
    [ -f "$src" ] || err "binary not found in extracted tarball"
    mv "$src" "$INSTALL_DIR/agentum"
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
