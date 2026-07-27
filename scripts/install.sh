#!/bin/sh
# Install the signed Agentum desktop release.
#
#   curl -fsSL https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh | sh
#
# Linux defaults to AppImage. Select a native package explicitly with
# `--format deb`, `--format rpm`, or the standalone executable with
# `--format raw`.
#
# SPDX-License-Identifier: MIT
set -eu

REPOSITORY="mateocerquetella/agentum"
RELEASE_API="https://api.github.com/repos/${REPOSITORY}/releases/latest"
RELEASE_DOWNLOAD="https://github.com/${REPOSITORY}/releases/download"
LINUX_FORMAT="${AGENTUM_LINUX_FORMAT:-appimage}"
INSTALL_DIR="${AGENTUM_INSTALL_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}"
APPLICATIONS_DIR="${AGENTUM_APPLICATIONS_DIR:-/Applications}"
VERSION=""
RELEASE_VERSION=""
PLATFORM=""
TEMP_DIR=""
MOUNT_POINT=""
MOUNTED=false

info() { printf 'agentum: %s\n' "$*" >&2; }
die() { printf 'agentum install: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: install.sh [--desktop] [--no-interactive] [--format appimage|deb|rpm|raw]

Installs Agentum Desktop only. The separate CLI/TUI distribution is available
from https://github.com/mateocerquetella/agentum-tui.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --desktop|--app|--no-interactive) shift ;;
        --format)
            [ "$#" -ge 2 ] || die "--format requires a value"
            LINUX_FORMAT="$2"; shift 2 ;;
        --format=*) LINUX_FORMAT="${1#*=}"; shift ;;
        --cli) die "the CLI/TUI is distributed separately from https://github.com/mateocerquetella/agentum-tui" ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

fetch_stdout() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$1"
    else
        die "curl or wget is required"
    fi
}

fetch_file() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
    else
        die "curl or wget is required"
    fi
}

validate_release_tag() {
    printf '%s\n' "$1" | grep -Eq '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
}

detect_latest_version() {
    _metadata="$(fetch_stdout "$RELEASE_API")" || die "could not read the latest GitHub release"
    _tags="$(printf '%s\n' "$_metadata" \
        | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"//;s/"$//')"
    [ "$(printf '%s\n' "$_tags" | sed '/^$/d' | wc -l | tr -d ' ')" = "1" ] \
        || die "latest-release metadata must contain exactly one tag"
    VERSION="$_tags"
    validate_release_tag "$VERSION" || die "latest release tag is not strict vMAJOR.MINOR.PATCH"
    RELEASE_VERSION="${VERSION#v}"
}

detect_platform() {
    case "$(uname -s)-$(uname -m)" in
        Darwin-x86_64|Darwin-amd64) PLATFORM="macos-x64" ;;
        Darwin-arm64|Darwin-aarch64) PLATFORM="macos-arm64" ;;
        Linux-x86_64|Linux-amd64) PLATFORM="linux-x64" ;;
        Linux-*) die "Agentum does not currently publish a Linux bundle for $(uname -m)" ;;
        *) die "unsupported platform: $(uname -s) $(uname -m)" ;;
    esac
}

asset_name() {
    _format="$1"
    case "$PLATFORM:$_format" in
        macos-x64:dmg|macos-arm64:dmg)
            printf 'agentum-%s-%s.dmg\n' "$RELEASE_VERSION" "$PLATFORM" ;;
        linux-x64:appimage)
            printf 'agentum-%s-linux-x64.AppImage\n' "$RELEASE_VERSION" ;;
        linux-x64:deb)
            printf 'agentum-%s-linux-x64.deb\n' "$RELEASE_VERSION" ;;
        linux-x64:rpm)
            printf 'agentum-%s-linux-x64.rpm\n' "$RELEASE_VERSION" ;;
        linux-x64:raw)
            printf 'agentum-desktop-%s-linux-x64\n' "$RELEASE_VERSION" ;;
        *) return 1 ;;
    esac
}

checksum_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "sha256sum or shasum is required"
    fi
}

verify_release_asset() {
    _asset="$1"
    _path="$2"
    _checksums="$TEMP_DIR/SHA256SUMS"
    if [ ! -s "$_checksums" ]; then
        fetch_file "${RELEASE_DOWNLOAD}/${VERSION}/SHA256SUMS" "$_checksums" \
            || die "could not download required release checksums"
    fi
    _matches="$(awk -v file="$_asset" '$2 == file { print $1 }' "$_checksums")"
    [ "$(printf '%s\n' "$_matches" | sed '/^$/d' | wc -l | tr -d ' ')" = "1" ] \
        || die "release checksums must contain exactly one entry for $_asset"
    printf '%s\n' "$_matches" | grep -Eq '^[0-9a-fA-F]{64}$' \
        || die "release checksum for $_asset is malformed"
    _actual="$(checksum_file "$_path")"
    [ "$(printf '%s' "$_matches" | tr 'A-F' 'a-f')" = "$(printf '%s' "$_actual" | tr 'A-F' 'a-f')" ] \
        || die "checksum mismatch for $_asset"
}

cleanup() {
    if [ "$MOUNTED" = true ] && [ -n "$MOUNT_POINT" ]; then
        hdiutil detach "$MOUNT_POINT" -quiet >/dev/null 2>&1 || true
        MOUNTED=false
    fi
    if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ] && [ ! -L "$TEMP_DIR" ]; then
        case "$TEMP_DIR" in
            /tmp/*|/private/tmp/*|"${TMPDIR:-/tmp}"/*) rm -rf -- "$TEMP_DIR" ;;
            *) printf 'agentum install: refusing unsafe temporary cleanup: %s\n' "$TEMP_DIR" >&2 ;;
        esac
    fi
}

make_temp_dir() {
    TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/agentum-install.XXXXXX")" \
        || die "could not create a private temporary directory"
    [ -d "$TEMP_DIR" ] && [ ! -L "$TEMP_DIR" ] \
        || die "temporary directory is unsafe"
    trap cleanup EXIT HUP INT TERM
}

download_asset() {
    _asset="$1"
    _destination="$TEMP_DIR/$_asset"
    info "downloading $VERSION / $_asset"
    fetch_file "${RELEASE_DOWNLOAD}/${VERSION}/${_asset}" "$_destination" \
        || die "could not download $_asset"
    [ -s "$_destination" ] || die "downloaded asset is empty: $_asset"
    verify_release_asset "$_asset" "$_destination"
    printf '%s\n' "$_destination"
}

validate_install_directory() {
    _directory="$1"
    case "$_directory" in
        /*) ;;
        *) die "install directory must be absolute: $_directory" ;;
    esac
    case "/$_directory/" in
        *'/../'*|*'/./'*) die "install directory contains traversal: $_directory" ;;
    esac
    [ "$_directory" != "/" ] || die "refusing to use the filesystem root as an install directory"
    [ ! -L "$_directory" ] || die "install directory must not be a symlink: $_directory"
}

install_linux_file() {
    _source="$1"
    validate_install_directory "$INSTALL_DIR"
    mkdir -p "$INSTALL_DIR"
    [ -d "$INSTALL_DIR" ] && [ ! -L "$INSTALL_DIR" ] || die "unsafe install directory"
    _destination="$INSTALL_DIR/agentum-desktop"
    if [ -L "$_destination" ] || { [ -e "$_destination" ] && [ ! -f "$_destination" ]; }; then
        die "refusing to overwrite a non-regular Agentum desktop target"
    fi
    _staged="$INSTALL_DIR/.agentum-desktop.new.$$"
    [ ! -e "$_staged" ] && [ ! -L "$_staged" ] || die "staging target already exists"
    install -m 0755 "$_source" "$_staged"
    mv -f "$_staged" "$_destination"
    info "installed $_destination"
}

run_as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo "$@"
    else
        die "this package format requires root privileges (sudo is unavailable)"
    fi
}

verify_macos_bundle() {
    _app="$1"
    [ -d "$_app" ] && [ ! -L "$_app" ] || die "the DMG does not contain a real Agentum.app"
    command -v codesign >/dev/null 2>&1 || die "codesign is required"
    command -v spctl >/dev/null 2>&1 || die "spctl is required"
    command -v xcrun >/dev/null 2>&1 || die "xcrun is required"
    codesign --verify --deep --strict --verbose=2 "$_app" \
        || die "Agentum.app has an invalid Developer ID signature"
    spctl --assess --type execute --verbose=4 "$_app" \
        || die "Gatekeeper rejected Agentum.app"
    xcrun stapler validate "$_app" \
        || die "Agentum.app has no valid notarization ticket"
}

mac_file_operation() {
    if [ -w "$APPLICATIONS_DIR" ]; then
        "$@"
    else
        command -v sudo >/dev/null 2>&1 || die "installing into $APPLICATIONS_DIR requires sudo"
        sudo "$@"
    fi
}

install_macos_app() {
    _source="$1"
    validate_install_directory "$APPLICATIONS_DIR"
    [ -d "$APPLICATIONS_DIR" ] && [ ! -L "$APPLICATIONS_DIR" ] \
        || die "Applications directory is unsafe"
    _destination="$APPLICATIONS_DIR/Agentum.app"
    _staged="$APPLICATIONS_DIR/.Agentum.app.new.$$"
    _backup="$APPLICATIONS_DIR/.Agentum.app.backup.$$"
    [ ! -e "$_staged" ] && [ ! -L "$_staged" ] || die "application staging target exists"
    [ ! -e "$_backup" ] && [ ! -L "$_backup" ] || die "application backup target exists"
    [ ! -L "$_destination" ] || die "refusing to replace a symlinked Agentum.app"
    if [ -e "$_destination" ] && [ ! -d "$_destination" ]; then
        die "refusing to replace a non-application target at $_destination"
    fi

    mac_file_operation ditto "$_source" "$_staged"
    verify_macos_bundle "$_staged"
    _had_previous=false
    if [ -d "$_destination" ]; then
        mac_file_operation mv "$_destination" "$_backup"
        _had_previous=true
    fi
    if ! mac_file_operation mv "$_staged" "$_destination"; then
        if [ "$_had_previous" = true ]; then
            mac_file_operation mv "$_backup" "$_destination" || true
        fi
        die "could not publish Agentum.app"
    fi
    if [ "$_had_previous" = true ]; then
        mac_file_operation rm -rf -- "$_backup"
    fi
    info "installed $_destination"
}

install_macos() {
    _asset="$(asset_name dmg)" || die "no macOS asset for $PLATFORM"
    _dmg="$(download_asset "$_asset")"
    command -v hdiutil >/dev/null 2>&1 || die "hdiutil is required"
    command -v xcrun >/dev/null 2>&1 || die "xcrun is required"
    xcrun stapler validate "$_dmg" || die "the DMG has no valid notarization ticket"
    MOUNT_POINT="$TEMP_DIR/mount"
    mkdir "$MOUNT_POINT"
    hdiutil attach "$_dmg" -readonly -nobrowse -quiet -mountpoint "$MOUNT_POINT" \
        || die "could not mount the Agentum DMG"
    MOUNTED=true
    _app="$MOUNT_POINT/Agentum.app"
    verify_macos_bundle "$_app"
    install_macos_app "$_app"
}

install_linux() {
    case "$LINUX_FORMAT" in
        appimage|deb|rpm|raw) ;;
        *) die "unsupported Linux format: $LINUX_FORMAT" ;;
    esac
    _asset="$(asset_name "$LINUX_FORMAT")" || die "no $LINUX_FORMAT asset for $PLATFORM"
    _download="$(download_asset "$_asset")"
    case "$LINUX_FORMAT" in
        appimage|raw) install_linux_file "$_download" ;;
        deb)
            command -v dpkg >/dev/null 2>&1 || die "dpkg is required for --format deb"
            run_as_root dpkg -i "$_download"
            info "installed Agentum Desktop with dpkg"
            ;;
        rpm)
            command -v rpm >/dev/null 2>&1 || die "rpm is required for --format rpm"
            run_as_root rpm -Uvh "$_download"
            info "installed Agentum Desktop with rpm"
            ;;
    esac
}

main() {
    detect_latest_version
    detect_platform
    make_temp_dir
    case "$PLATFORM" in
        macos-*) install_macos ;;
        linux-x64) install_linux ;;
        *) die "unsupported release platform: $PLATFORM" ;;
    esac
    info "Agentum Desktop $VERSION is ready"
}

if [ "${AGENTUM_INSTALL_SOURCE_ONLY:-}" != "1" ]; then
    main
fi
