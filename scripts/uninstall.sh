#!/bin/sh
# Remove Agentum Desktop. User data is preserved unless --purge-data is given.
# SPDX-License-Identifier: MIT
set -eu

UNINSTALL_HOME="${AGENTUM_UNINSTALL_HOME:-${HOME:?HOME is required}}"
INSTALL_DIR="${AGENTUM_INSTALL_DIR:-${XDG_BIN_HOME:-$UNINSTALL_HOME/.local/bin}}"
APPLICATIONS_DIR="${AGENTUM_APPLICATIONS_DIR:-/Applications}"
PURGE_DATA=false
PACKAGE_FORMAT=""
# Tauri Bundler 2.9.2 derives both Debian's Package field and RPM's Name from
# the kebab-case productName. tauri.conf.json fixes productName to "Agentum",
# so both native package managers install the exact identifier below.
PACKAGE_NAME="agentum"

info() { printf 'agentum uninstall: %s\n' "$*"; }
die() { printf 'agentum uninstall: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: uninstall.sh [--purge-data] [--package deb|rpm]

By default only Agentum Desktop is removed and all user data is preserved.
--purge-data deletes only Agentum's known data/config/cache directories.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --purge-data) PURGE_DATA=true; shift ;;
        --keep-data|--no-interactive) shift ;;
        --package)
            [ "$#" -ge 2 ] || die "--package requires deb or rpm"
            PACKAGE_FORMAT="$2"; shift 2 ;;
        --package=*) PACKAGE_FORMAT="${1#*=}"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

validate_parent_directory() {
    _directory="$1"
    case "$_directory" in
        /*) ;;
        *) die "target parent must be absolute: $_directory" ;;
    esac
    case "/$_directory/" in
        *'/../'*|*'/./'*) die "target parent contains traversal: $_directory" ;;
    esac
    [ "$_directory" != "/" ] || die "refusing a filesystem-root target"
    [ ! -L "$_directory" ] || die "target parent must not be a symlink: $_directory"
}

remove_exact_file() {
    _target="$1"
    if [ -L "$_target" ]; then
        rm -f -- "$_target"
    elif [ -e "$_target" ]; then
        [ -f "$_target" ] || die "refusing to remove a non-file target: $_target"
        rm -f -- "$_target"
    fi
}

remove_exact_app() {
    validate_parent_directory "$APPLICATIONS_DIR"
    _target="$APPLICATIONS_DIR/Agentum.app"
    if [ -L "$_target" ]; then
        rm -f -- "$_target"
    elif [ -e "$_target" ]; then
        [ -d "$_target" ] || die "refusing to remove a non-application target: $_target"
        if [ -w "$APPLICATIONS_DIR" ]; then
            rm -rf -- "$_target"
        else
            command -v sudo >/dev/null 2>&1 || die "removing $_target requires sudo"
            sudo rm -rf -- "$_target"
        fi
    fi
    info "removed exact application target $_target"
}

remove_standalone_linux() {
    validate_parent_directory "$INSTALL_DIR"
    remove_exact_file "$INSTALL_DIR/agentum-desktop"
    info "removed exact standalone target $INSTALL_DIR/agentum-desktop"
}

run_as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo "$@"
    else
        die "package removal requires root privileges (sudo is unavailable)"
    fi
}

remove_package() {
    case "$PACKAGE_FORMAT" in
        "") return 0 ;;
        deb)
            command -v dpkg >/dev/null 2>&1 || die "dpkg is required for --package deb"
            run_as_root dpkg -r "$PACKAGE_NAME"
            ;;
        rpm)
            command -v rpm >/dev/null 2>&1 || die "rpm is required for --package rpm"
            run_as_root rpm -e "$PACKAGE_NAME"
            ;;
        *) die "unsupported package format: $PACKAGE_FORMAT" ;;
    esac
}

purge_known_directory() {
    _target="$1"
    case "$UNINSTALL_HOME" in
        /*) ;;
        *) die "uninstall home must be absolute: $UNINSTALL_HOME" ;;
    esac
    case "/$UNINSTALL_HOME/" in
        *'/../'*|*'/./'*) die "uninstall home contains traversal: $UNINSTALL_HOME" ;;
    esac
    [ "$UNINSTALL_HOME" != "/" ] || die "refusing a filesystem-root uninstall home"
    case "/$_target/" in
        *'/../'*|*'/./'*) die "data purge target contains traversal: $_target" ;;
    esac
    case "$_target" in
        "$UNINSTALL_HOME"/*/agentum|"$UNINSTALL_HOME"/*/Agentum|"$UNINSTALL_HOME"/*/dev.agentum.app) ;;
        *) die "refusing unrecognized data purge target: $_target" ;;
    esac
    [ -e "$_target" ] || [ -L "$_target" ] || return 0
    [ ! -L "$_target" ] || die "refusing to follow a symlinked data directory: $_target"
    [ -d "$_target" ] || die "refusing to purge a non-directory data target: $_target"

    # Resolve the configured home and the target's parent independently. The
    # expected physical parent preserves the lexical path below UNINSTALL_HOME;
    # a mismatch means an intermediate component is a symlink. Never recurse
    # through that path, even when the link happens to point back inside home.
    _home_physical="$(unset CDPATH; cd -P -- "$UNINSTALL_HOME" && pwd -P)" \
        || die "could not resolve uninstall home: $UNINSTALL_HOME"
    _parent="${_target%/*}"
    _relative_parent="${_parent#"$UNINSTALL_HOME"/}"
    [ "$_relative_parent" != "$_parent" ] \
        || die "data purge target escaped uninstall home: $_target"
    _parent_physical="$(unset CDPATH; cd -P -- "$_parent" && pwd -P)" \
        || die "could not resolve data purge parent: $_parent"
    _expected_parent="$_home_physical/$_relative_parent"
    [ "$_parent_physical" = "$_expected_parent" ] \
        || die "refusing to follow a symlinked data parent: $_parent"

    rm -rf -- "$_target"
    info "purged $_target"
}

known_data_directories() {
    case "$(uname -s)" in
        Darwin)
            printf '%s\n' \
                "$UNINSTALL_HOME/Library/Application Support/agentum" \
                "$UNINSTALL_HOME/Library/Application Support/Agentum" \
                "$UNINSTALL_HOME/Library/Application Support/dev.agentum.app" \
                "$UNINSTALL_HOME/Library/Caches/agentum" \
                "$UNINSTALL_HOME/Library/Caches/dev.agentum.app"
            ;;
        Linux)
            printf '%s\n' \
                "${XDG_DATA_HOME:-$UNINSTALL_HOME/.local/share}/agentum" \
                "${XDG_DATA_HOME:-$UNINSTALL_HOME/.local/share}/Agentum" \
                "${XDG_CONFIG_HOME:-$UNINSTALL_HOME/.config}/agentum" \
                "${XDG_CACHE_HOME:-$UNINSTALL_HOME/.cache}/agentum" \
                "${XDG_STATE_HOME:-$UNINSTALL_HOME/.local/state}/agentum"
            ;;
        *) return 0 ;;
    esac
}

purge_known_data() {
    [ "$PURGE_DATA" = true ] || {
        info "user data preserved (use --purge-data for explicit removal)"
        return 0
    }
    known_data_directories | while IFS= read -r _directory; do
        purge_known_directory "$_directory"
    done
}

main() {
    case "$(uname -s)" in
        Darwin) remove_exact_app ;;
        Linux) remove_standalone_linux; remove_package ;;
        *) die "unsupported platform: $(uname -s)" ;;
    esac
    purge_known_data
    info "done"
}

if [ "${AGENTUM_UNINSTALL_SOURCE_ONLY:-}" != "1" ]; then
    main
fi
