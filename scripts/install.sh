#!/bin/sh
# Install the terminal-only agentum binary from the latest GitHub release.
#
#   curl -fsSL https://github.com/mateocerquetella/agentum-tui/releases/latest/download/install.sh | sh
#
# Environment:
#   INSTALL_DIR=$HOME/.local/bin  Override the destination directory.
#
# SPDX-License-Identifier: MIT
set -eu

REPO="mateocerquetella/agentum-tui"
API="https://api.github.com/repos/$REPO/releases/latest"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

fail() {
    printf 'agentum: %s\n' "$1" >&2
    exit 1
}

fetch_stdout() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$1"
    else
        fail "curl or wget is required"
    fi
}

fetch_file() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
    else
        fail "curl or wget is required"
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        -h|--help)
            printf 'Usage: install.sh\nEnvironment: INSTALL_DIR=path\n'
            exit 0
            ;;
        --cli|--no-interactive)
            # Accepted as harmless compatibility flags from older installers.
            shift
            ;;
        --desktop|--app)
            fail "this package is terminal-only; install and run agentum"
            ;;
        *)
            fail "unknown option: $1"
            ;;
    esac
done

case "$(uname -s)-$(uname -m)" in
    Darwin-x86_64|Darwin-amd64)  PLATFORM="macos-x64" ;;
    Darwin-arm64|Darwin-aarch64) PLATFORM="macos-arm64" ;;
    Linux-x86_64|Linux-amd64)    PLATFORM="linux-x64" ;;
    Linux-aarch64|Linux-arm64)   PLATFORM="linux-arm64" ;;
    *) fail "unsupported platform: $(uname -s) $(uname -m)" ;;
esac

TAG="$(fetch_stdout "$API" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
[ -n "$TAG" ] || fail "could not determine the latest release"
VERSION="${TAG#v}"
ASSET="agentum-${VERSION}-${PLATFORM}.tar.gz"
BASE="https://github.com/$REPO/releases/download/$TAG"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT HUP INT TERM

printf 'Downloading agentum %s for %s...\n' "$VERSION" "$PLATFORM"
fetch_file "$BASE/$ASSET" "$TMP_DIR/$ASSET"
fetch_file "$BASE/SHA256SUMS" "$TMP_DIR/SHA256SUMS"

EXPECTED="$(awk -v asset="$ASSET" '$2 == asset { print $1; exit }' "$TMP_DIR/SHA256SUMS")"
[ -n "$EXPECTED" ] || fail "release checksum for $ASSET is missing"
if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "$TMP_DIR/$ASSET" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    ACTUAL="$(shasum -a 256 "$TMP_DIR/$ASSET" | awk '{print $1}')"
else
    fail "sha256sum or shasum is required to verify the download"
fi
[ "$EXPECTED" = "$ACTUAL" ] || fail "checksum mismatch for $ASSET"

tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
SOURCE="$TMP_DIR/agentum-${VERSION}-${PLATFORM}/agentum"
[ -f "$SOURCE" ] || fail "agentum binary is missing from $ASSET"

mkdir -p "$INSTALL_DIR"
cp "$SOURCE" "$INSTALL_DIR/agentum"
chmod 755 "$INSTALL_DIR/agentum"

printf 'Installed %s\n' "$INSTALL_DIR/agentum"
if ! command -v tmux >/dev/null 2>&1; then
    printf 'Warning: tmux is required (macOS: brew install tmux).\n' >&2
fi
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) printf 'Add %s to PATH, then run: agentum\n' "$INSTALL_DIR" ;;
esac
printf 'Run: agentum\n'
