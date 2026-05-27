#!/bin/sh
# agentum uninstaller — reverses scripts/install.sh.
#
# curl -fsSL https://github.com/mateocerquetella/agentum/releases/latest/download/uninstall.sh | sh
#
# Options (passed after -- when piping):
#   sh -s -- --purge              # also remove the data directory (no prompt)
#   sh -s -- --keep-data          # keep the data directory (no prompt)
#   sh -s -- --no-interactive     # never prompt; equivalent to --keep-data
#
# Environment:
#   INSTALL_DIR=$HOME/.local/bin  override binary location
#   AGENTUM_PURGE=1               equivalent to --purge
#   AGENTUM_KEEP_DATA=1           equivalent to --keep-data
#
# SPDX-License-Identifier: MIT
set -eu

# ── Configuration ──────────────────────────────────────────────
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/agentum"
LA_PATH="$HOME/Library/LaunchAgents/dev.agentum.daemon.plist"
SYSTEMD_UNIT="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/agentum.service"

PURGE="${AGENTUM_PURGE:-}"
KEEP_DATA="${AGENTUM_KEEP_DATA:-}"
INTERACTIVE=true

# ── Colors ─────────────────────────────────────────────────────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    ESC=$(printf '\033')
    C_R="${ESC}[0m"; C_B="${ESC}[1m"; C_D="${ESC}[2m"
    C_Y="${ESC}[1;33m"; C_G="${ESC}[1;32m"; C_RED="${ESC}[1;31m"; C_BL="${ESC}[1;34m"
else
    C_R=''; C_B=''; C_D=''; C_Y=''; C_G=''; C_RED=''; C_BL=''
fi

i() { printf '  %s●%s %s\n' "${C_BL}" "${C_R}" "$1"; }
o() { printf '  %s✔%s %s\n' "${C_G}"  "${C_R}" "$1"; }
w() { printf '  %s⚠%s  %s\n' "${C_Y}"  "${C_R}" "$1" >&2; }
e() { printf '  %s✖%s %s\n' "${C_RED}" "${C_R}" "$1" >&2; exit 1; }

# ── Args ───────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
    case "$1" in
        --purge) PURGE=1; shift ;;
        --keep-data) KEEP_DATA=1; shift ;;
        --no-interactive) INTERACTIVE=false; shift ;;
        -h|--help) printf 'Usage: uninstall.sh [--purge|--keep-data] [--no-interactive]\n'; exit 0 ;;
        *) shift ;;
    esac
done

[ -n "${CI:-}" ] && INTERACTIVE=false
[ -t 0 ] || INTERACTIVE=false

# ── Stop and remove launchd LaunchAgent (macOS) ────────────────
remove_launchd() {
    [ -f "$LA_PATH" ] || return 0
    i "removing LaunchAgent $LA_PATH"
    launchctl bootout "gui/$(id -u)" "$LA_PATH" 2>/dev/null || \
        launchctl unload "$LA_PATH" 2>/dev/null || true
    rm -f "$LA_PATH"
    o "LaunchAgent removed"
}

# ── Stop and remove systemd user unit (Linux) ──────────────────
remove_systemd_unit() {
    [ -f "$SYSTEMD_UNIT" ] || return 0
    i "removing systemd user unit $SYSTEMD_UNIT"
    systemctl --user disable --now agentum.service 2>/dev/null || true
    rm -f "$SYSTEMD_UNIT"
    systemctl --user daemon-reload 2>/dev/null || true
    o "systemd unit removed"
}

# ── Remove binaries ────────────────────────────────────────────
remove_binaries() {
    for name in agentum lazyagentum; do
        bin="$INSTALL_DIR/$name"
        if [ -f "$bin" ]; then
            i "removing $bin"
            rm -f "$bin"
            o "$name removed"
        fi
    done
}

# ── Data directory ─────────────────────────────────────────────
# Default: keep. The user's SQLite DB lives here; deleting it loses all
# session metadata, board state, notes, channels, and credentials.
handle_data_dir() {
    [ -d "$DATA_DIR" ] || { o "no data directory at $DATA_DIR"; return 0; }

    if [ -n "$PURGE" ]; then
        i "purging data directory $DATA_DIR (--purge)"
        rm -rf "$DATA_DIR"
        o "data directory removed"
        return 0
    fi
    if [ -n "$KEEP_DATA" ] || [ "$INTERACTIVE" = false ]; then
        w "data directory kept at $DATA_DIR (remove manually with: rm -rf $DATA_DIR)"
        return 0
    fi

    printf '  %s?%s also remove %s ? %s[y/N]%s ' "${C_Y}" "${C_R}" "$DATA_DIR" "${C_D}" "${C_R}"
    read -r ans || ans=''
    case "$ans" in
        y|Y|yes|YES)
            rm -rf "$DATA_DIR"
            o "data directory removed"
            ;;
        *)
            w "data directory kept at $DATA_DIR"
            ;;
    esac
}

# ── Run ────────────────────────────────────────────────────────
printf '\n  %sagentum uninstaller%s\n\n' "${C_B}" "${C_R}"

case "$(uname -s)" in
    Darwin) remove_launchd ;;
    Linux)  remove_systemd_unit ;;
    *) w "unsupported OS ($(uname -s)) — skipping service unit removal" ;;
esac

remove_binaries
handle_data_dir

printf '\n  %sdone.%s\n\n' "${C_G}" "${C_R}"
