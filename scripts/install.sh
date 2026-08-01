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
LINUX_DATA_HOME="${AGENTUM_LINUX_DATA_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}}"
LINUX_APPLICATIONS_DIR="${AGENTUM_LINUX_APPLICATIONS_DIR:-$LINUX_DATA_HOME/applications}"
LINUX_ICON_DIR="${AGENTUM_LINUX_ICON_DIR:-$LINUX_DATA_HOME/icons/hicolor/256x256/apps}"
VERSION=""
RELEASE_VERSION=""
PLATFORM=""
TEMP_DIR=""
LINUX_STAGED_DESKTOP=""
LINUX_STAGED_ICON=""

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
    _platform_os="$(uname -s)"
    _platform_arch="$(uname -m)"
    case "$_platform_os-$_platform_arch" in
        Linux-x86_64|Linux-amd64) PLATFORM="linux-x64" ;;
        *) die "unsupported platform: $_platform_os $_platform_arch" ;;
    esac
}

asset_name() {
    _format="$1"
    case "$PLATFORM:$_format" in
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
    if [ -n "$LINUX_STAGED_DESKTOP" ]; then
        rm -f -- "$LINUX_STAGED_DESKTOP"
        LINUX_STAGED_DESKTOP=""
    fi
    if [ -n "$LINUX_STAGED_ICON" ]; then
        rm -f -- "$LINUX_STAGED_ICON"
        LINUX_STAGED_ICON=""
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
    case "$_directory" in
        *'
'*) die "install directory contains a line break" ;;
    esac
    if printf '%s' "$_directory" | LC_ALL=C grep -q '[[:cntrl:]]'; then
        die "install directory contains control characters"
    fi
    [ "$_directory" != "/" ] || die "refusing to use the filesystem root as an install directory"
    [ ! -L "$_directory" ] || die "install directory must not be a symlink: $_directory"
}

validate_regular_publication_target() {
    _target="$1"
    _description="$2"
    if [ -L "$_target" ] || { [ -e "$_target" ] && [ ! -f "$_target" ]; }; then
        die "refusing to overwrite a non-regular $_description target: $_target"
    fi
}

desktop_exec_argument() {
    _executable="$1"
    case "$_executable" in
        /*) ;;
        *) die "desktop executable path must be absolute: $_executable" ;;
    esac
    case "$_executable" in
        *'
'*) die "desktop executable path contains a line break" ;;
    esac
    if printf '%s' "$_executable" | LC_ALL=C grep -q '[[:cntrl:]]'; then
        die "desktop executable path contains control characters"
    fi
    printf '%s' "$_executable" \
        | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/`/\\`/g' -e 's/\$/\\$/g' -e 's/%/%%/g'
}

extract_linux_appimage_icon() {
    _source="$1"
    case "$_source" in
        /*) ;;
        *) _source="$(pwd)/$_source" ;;
    esac
    [ -f "$_source" ] && [ ! -L "$_source" ] && [ -s "$_source" ] \
        || die "verified AppImage must be a regular non-empty file"
    _extract_root="$TEMP_DIR/launcher-extract"
    [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ] && [ ! -L "$TEMP_DIR" ] \
        || die "private installer temporary directory is required"
    [ ! -e "$_extract_root" ] && [ ! -L "$_extract_root" ] \
        || die "AppImage launcher extraction target already exists"
    chmod 0700 "$_source" || die "could not make the verified AppImage executable"
    mkdir "$_extract_root" || die "could not create the AppImage extraction directory"
    (
        cd "$_extract_root"
        APPIMAGE_EXTRACT_AND_RUN=1 "$_source" --appimage-extract Agentum.png >/dev/null
    ) || die "could not extract the Agentum AppImage icon"
    _icon="$_extract_root/squashfs-root/Agentum.png"
    [ -f "$_icon" ] && [ ! -L "$_icon" ] && [ -s "$_icon" ] \
        || die "verified AppImage does not contain a regular Agentum.png icon"
    printf '%s\n' "$_icon"
}

prepare_linux_launcher() {
    _executable="$1"
    _icon_source="$2"
    validate_install_directory "$LINUX_APPLICATIONS_DIR"
    validate_install_directory "$LINUX_ICON_DIR"
    mkdir -p "$LINUX_APPLICATIONS_DIR" "$LINUX_ICON_DIR" \
        || die "could not create the Linux launcher directories"
    [ -d "$LINUX_APPLICATIONS_DIR" ] && [ ! -L "$LINUX_APPLICATIONS_DIR" ] \
        || die "unsafe Linux applications directory"
    [ -d "$LINUX_ICON_DIR" ] && [ ! -L "$LINUX_ICON_DIR" ] \
        || die "unsafe Linux icon directory"

    _desktop_target="$LINUX_APPLICATIONS_DIR/dev.agentum.app.desktop"
    _icon_target="$LINUX_ICON_DIR/agentum-desktop.png"
    validate_regular_publication_target "$_desktop_target" "Agentum desktop entry"
    validate_regular_publication_target "$_icon_target" "Agentum icon"

    LINUX_STAGED_DESKTOP="${_desktop_target}.new.$$"
    LINUX_STAGED_ICON="${_icon_target}.new.$$"
    [ ! -e "$LINUX_STAGED_DESKTOP" ] && [ ! -L "$LINUX_STAGED_DESKTOP" ] \
        || die "desktop entry staging target already exists"
    [ ! -e "$LINUX_STAGED_ICON" ] && [ ! -L "$LINUX_STAGED_ICON" ] \
        || die "icon staging target already exists"

    install -m 0644 "$_icon_source" "$LINUX_STAGED_ICON" \
        || die "could not stage the Agentum icon"
    _escaped_executable="$(desktop_exec_argument "$_executable")" \
        || die "could not encode the Agentum desktop executable path"
    (
        umask 022
        {
            printf '%s\n' '[Desktop Entry]'
            printf '%s\n' 'Type=Application'
            printf '%s\n' 'Name=Agentum'
            printf '%s\n' 'Comment=Agentum desktop application'
            printf 'Exec="%s"\n' "$_escaped_executable"
            printf '%s\n' 'Icon=agentum-desktop'
            printf '%s\n' 'Terminal=false'
            printf '%s\n' 'Categories=Development;'
            printf '%s\n' 'StartupWMClass=Agentum-desktop'
        } > "$LINUX_STAGED_DESKTOP"
    ) || die "could not stage the Agentum desktop entry"
}

publish_linux_launcher() {
    [ -n "$LINUX_STAGED_DESKTOP" ] && [ -f "$LINUX_STAGED_DESKTOP" ] \
        || die "prepared Agentum desktop entry is required"
    [ -n "$LINUX_STAGED_ICON" ] && [ -f "$LINUX_STAGED_ICON" ] \
        || die "prepared Agentum icon is required"
    _desktop_target="$LINUX_APPLICATIONS_DIR/dev.agentum.app.desktop"
    _icon_target="$LINUX_ICON_DIR/agentum-desktop.png"
    mv -f "$LINUX_STAGED_ICON" "$_icon_target" \
        || die "could not publish the Agentum icon"
    LINUX_STAGED_ICON=""
    mv -f "$LINUX_STAGED_DESKTOP" "$_desktop_target" \
        || die "could not publish the Agentum desktop entry"
    LINUX_STAGED_DESKTOP=""
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$LINUX_APPLICATIONS_DIR" >/dev/null 2>&1 \
            || info "desktop database refresh was unavailable; the launcher is still installed"
    fi
    info "installed $_desktop_target"
}

install_linux_file() {
    _source="$1"
    validate_install_directory "$INSTALL_DIR"
    mkdir -p "$INSTALL_DIR" || die "could not create the install directory"
    [ -d "$INSTALL_DIR" ] && [ ! -L "$INSTALL_DIR" ] || die "unsafe install directory"
    _destination="$INSTALL_DIR/agentum-desktop"
    if [ -L "$_destination" ] || { [ -e "$_destination" ] && [ ! -f "$_destination" ]; }; then
        die "refusing to overwrite a non-regular Agentum desktop target"
    fi
    _staged="$INSTALL_DIR/.agentum-desktop.new.$$"
    [ ! -e "$_staged" ] && [ ! -L "$_staged" ] || die "staging target already exists"
    install -m 0755 "$_source" "$_staged" || die "could not stage Agentum Desktop"
    mv -f "$_staged" "$_destination" || die "could not publish Agentum Desktop"
    info "installed $_destination"
}

install_linux_appimage() {
    _source="$1"
    _icon_source="$(extract_linux_appimage_icon "$_source")" \
        || die "could not prepare the Agentum AppImage launcher icon"
    prepare_linux_launcher "$INSTALL_DIR/agentum-desktop" "$_icon_source"
    install_linux_file "$_source"
    publish_linux_launcher
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

install_linux() {
    case "$LINUX_FORMAT" in
        appimage|deb|rpm|raw) ;;
        *) die "unsupported Linux format: $LINUX_FORMAT" ;;
    esac
    _asset="$(asset_name "$LINUX_FORMAT")" || die "no $LINUX_FORMAT asset for $PLATFORM"
    _download="$(download_asset "$_asset")"
    case "$LINUX_FORMAT" in
        appimage) install_linux_appimage "$_download" ;;
        raw) install_linux_file "$_download" ;;
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
    detect_platform
    detect_latest_version
    make_temp_dir
    install_linux
    info "Agentum Desktop $VERSION is ready"
}

if [ "${AGENTUM_INSTALL_SOURCE_ONLY:-}" != "1" ]; then
    main
fi
