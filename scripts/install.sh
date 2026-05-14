#!/bin/sh
# agentum installer — interactive CLI with mode selection
#
# curl -fsSL https://agentum.dev/install.sh | sh
#
# Options (passed after -- when piping):
#   curl ... | sh -s -- --mode server        # Control Plane (full)
#   curl ... | sh -s -- --mode cli           # Terminal CLI
#   curl ... | sh -s -- --no-interactive     # Skip prompts
#
# Environment:
#   INSTALL_MODE=server|cli     CI / non-interactive mode
#   INSTALL_DIR=$HOME/.local/bin  Override install path
#
# SPDX-License-Identifier: MIT
set -eu

# ── Configuration ──────────────────────────────────────────────
REPO="mateocerquetella/agentum"
GH_API="https://api.github.com/repos/${REPO}/releases/latest"
GH_DL="https://github.com/${REPO}/releases/download"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
PLATFORM=""; VERSION=""; INSTALL_MODE="${INSTALL_MODE:-}"; INTERACTIVE=false; HAS_TTY=false
IS_UPDATE=false

# ── Colors (agentum: dark bg + gold/amber accents) ────────────
# Use actual ESC char so `printf '%s'` substitutes a real escape sequence.
# (POSIX sh has no $'\033'; %s does not interpret backslash escapes.)
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    ESC=$(printf '\033')
    C_R="${ESC}[0m";    C_B="${ESC}[1m";     C_D="${ESC}[2m"
    C_Y="${ESC}[1;33m"; C_BL="${ESC}[1;34m"; C_G="${ESC}[1;32m"
    C_RED="${ESC}[1;31m"; C_C="${ESC}[1;36m"; C_A="${ESC}[38;5;214m"
else
    C_R=''; C_B=''; C_D=''; C_Y=''; C_BL=''; C_G=''; C_RED=''; C_C=''; C_A=''
fi

# ── Logging ────────────────────────────────────────────────────
i() { printf '  %s●%s %s%s\n'            "${C_BL}" "${C_R}" "${C_B}$1${C_R}" "${2:+ $2}"; }
o() { printf '  %s✔%s %s%s\n'           "${C_G}"  "${C_R}" "${C_B}$1${C_R}" "${2:+ $2}"; }
w() { printf '  %s⚠%s  %s%s\n'          "${C_Y}"  "${C_R}" "$1" "${2:+ $2}" >&2; }
e() { printf '  %s✖%s %s\n'             "${C_RED}" "${C_R}" "$1" >&2; exit 1; }
s() { printf '\n  %s▸%s %s%s\n'         "${C_A}"  "${C_R}" "${C_B}$1${C_R}" "${2:+ $2}"; }
h() { printf '     %s%s%s\n'            "${C_D}"  "$1" "${C_R}"; }
div() { printf '  %s──%s%s\n' "${C_D}" "$1" "${C_R}"; }

# ── Parse CLI args ─────────────────────────────────────────────
while [ $# -gt 0 ]; do
    case "$1" in
        --mode) INSTALL_MODE="$2"; shift 2 ;;
        --mode=*) INSTALL_MODE="${1#*=}"; shift ;;
        --no-interactive) INTERACTIVE=false; shift ;;
        -h|--help) printf 'Usage: install.sh [--mode server|cli] [--no-interactive]\nEnv: INSTALL_MODE=server|cli\n'; exit 0 ;;
        *) shift ;;
    esac
done

# ── TTY detection ──────────────────────────────────────────────
if [ -t 0 ] && [ -t 1 ]; then
    INTERACTIVE=true; HAS_TTY=true
elif [ -t 1 ] && [ -c /dev/tty ]; then
    INTERACTIVE=true
    exec 3</dev/tty 2>/dev/null || INTERACTIVE=false
    [ "$INTERACTIVE" = true ] && HAS_TTY=true
fi
[ -n "${CI:-}" ] && INTERACTIVE=false

read_input() {
    if [ "$HAS_TTY" = true ] && ! [ -t 0 ]; then read -r "$1" <&3
    else read -r "$1" || true; fi
}

# ── Banner ─────────────────────────────────────────────────────
banner() {
    printf '\n'
    printf '  %s█████╗  ██████╗ ███████╗███╗   ██╗████████╗██╗   ██╗███╗   ███╗%s\n' "${C_A}" "${C_R}"
    printf '  %s██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝██║   ██║████╗ ████║%s\n' "${C_A}" "${C_R}"
    printf '  %s███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║   ██║   ██║██╔████╔██║%s\n' "${C_Y}" "${C_R}"
    printf '  %s██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║   ██║   ██║██║╚██╔╝██║%s\n' "${C_Y}" "${C_R}"
    printf '  %s██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║   ╚██████╔╝██║ ╚═╝ ██║%s\n' "${C_A}" "${C_R}"
    printf '  %s╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝    ╚═════╝ ╚═╝     ╚═╝%s\n' "${C_A}" "${C_R}"
    printf '               %sself-hosted AI agent control plane%s\n'              "${C_D}" "${C_R}"
    printf '\n'
}

# ── Mode selection ─────────────────────────────────────────────
# Both `server` and `cli` install the same binary — the only difference
# is the post-install guidance shown afterwards. `both` shows both
# guides so users who want to use the dashboard *and* the CLI quickly
# get pointed at every entrypoint.
select_mode() {
    case "${INSTALL_MODE}" in server|cli|both) return ;; esac
    if [ "$INTERACTIVE" != true ]; then
        INSTALL_MODE="both"
        i "mode" "defaulting to ${C_B}both${C_R} (use --mode server|cli to narrow)"
        return
    fi
    printf '\n  %sChoose your install:%s\n\n' "${C_Y}" "${C_R}"
    printf '  %s🖥️   [1] %sControl Plane%s\n'       "${C_B}" "${C_Y}" "${C_R}"
    printf '       Server · Dashboard · TLS · tmux — full web UI\n'
    printf '       %s›%s %sagentum serve%s on your LAN, dashboard from any device\n\n' "${C_D}" "${C_R}" "${C_C}" "${C_R}"
    printf '  %s⌨️   [2] %sTerminal CLI%s\n'         "${C_B}" "${C_BL}" "${C_R}"
    printf '       Fullscreen TUI + CLI session manager, no server/TLS\n'
    printf '       %s›%s %sagentum terminal%s for the TUI, %sagentum new/ls/tail%s for scripts\n\n' "${C_D}" "${C_R}" "${C_C}" "${C_R}" "${C_C}" "${C_R}"
    printf '  %s✨  [3] %sBoth%s %s(recommended)%s\n' "${C_B}" "${C_G}" "${C_R}" "${C_D}" "${C_R}"
    printf '       Install once, get the dashboard %sand%s the terminal workflow.\n' "${C_C}" "${C_R}"
    printf '       %s›%s same binary — %sagentum serve%s or %sagentum terminal%s whenever you like\n\n' "${C_D}" "${C_R}" "${C_C}" "${C_R}" "${C_C}" "${C_R}"
    while true; do
        printf '  %sChoice [1-3] (3):%s ' "${C_D}" "${C_R}"
        choice=""; read_input choice; choice="${choice:-3}"
        case "$choice" in
            1|server|Server) INSTALL_MODE="server"; break ;;
            2|cli|CLI|client) INSTALL_MODE="cli"; break ;;
            3|both|Both|all|All) INSTALL_MODE="both"; break ;;
            q|quit) printf '\n  %sCancelled.%s\n\n' "${C_D}" "${C_R}"; exit 0 ;;
            *) printf '  %sEnter 1, 2 or 3.%s\n' "${C_RED}" "${C_R}" ;;
        esac
    done
    printf '\n'
}

# ── Platform + version detection ───────────────────────────────
detect_platform() {
    case "$(uname -s)-$(uname -m)" in
        Darwin-x86_64|Darwin-amd64)    PLATFORM="x86_64-apple-darwin" ;;
        Darwin-arm64|Darwin-aarch64)   PLATFORM="aarch64-apple-darwin" ;;
        Linux-x86_64|Linux-amd64)      PLATFORM="x86_64-unknown-linux-gnu" ;;
        Linux-aarch64|Linux-arm64)     PLATFORM="aarch64-unknown-linux-gnu" ;;
        *) e "unsupported platform: $(uname -s) $(uname -m)" ;;
    esac
}

detect_version() {
    if command -v curl >/dev/null 2>&1; then
        VERSION="$(curl -fsSL "$GH_API" 2>/dev/null | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"//;s/".*//')"
    elif command -v wget >/dev/null 2>&1; then
        VERSION="$(wget -qO- "$GH_API" 2>/dev/null | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"//;s/".*//')"
    else e "curl or wget is required"; fi
    [ -n "$VERSION" ] || e "could not determine latest version"
}

# ── Existing-install detection (turns the script into an updater) ──
# Sets EXISTING_BIN / EXISTING_VERSION when an `agentum` is already on
# disk — preferring INSTALL_DIR, falling back to whatever's on PATH.
detect_existing() {
    EXISTING_BIN=""; EXISTING_VERSION=""
    if [ -x "$INSTALL_DIR/agentum" ]; then
        EXISTING_BIN="$INSTALL_DIR/agentum"
    elif command -v agentum >/dev/null 2>&1; then
        EXISTING_BIN="$(command -v agentum)"
    fi
    [ -n "$EXISTING_BIN" ] || return 0
    # `agentum --version` prints e.g. "agentum 0.6.5"
    EXISTING_VERSION="$("$EXISTING_BIN" --version 2>/dev/null | awk '{print $2}')"
    EXISTING_VERSION="${EXISTING_VERSION:-unknown}"
}

# ── Spinner ────────────────────────────────────────────────────
spin() {
    local pid="$1" msg="$2" i=0 c
    while kill -0 "$pid" 2>/dev/null; do
        case $(( i % 4 )) in 0) c='|' ;; 1) c='/' ;; 2) c='-' ;; 3) c='\' ;; esac
        printf '\r  %s%s %s%s...  ' "${C_A}" "$c" "${C_B}" "$msg"
        i=$(( i + 1 ))
        sleep 0.1 2>/dev/null || sleep 1
    done
    wait "$pid"
    printf '\r\033[K'
}

fetch() {
    if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then wget -q "$1" -O "$2"
    else e "curl or wget is required"; fi
}

# ── Download + verify + extract ────────────────────────────────
download() {
    ASSET="agentum-${VERSION}-${PLATFORM}.tar.gz"
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    s "Downloading" "${C_D}${VERSION} · ${PLATFORM}${C_R}"

    fetch "${GH_DL}/${VERSION}/${ASSET}" "$TMPDIR/$ASSET" &
    spin $! "fetching $ASSET"
    printf '\n'

    fetch "${GH_DL}/${VERSION}/SHA256SUMS" "$TMPDIR/SHA256SUMS" 2>/dev/null &
    spin $! "fetching checksums"

    if [ -f "$TMPDIR/SHA256SUMS" ] && command -v sha256sum >/dev/null 2>&1; then
        expected="$(grep " ${ASSET}\$" "$TMPDIR/SHA256SUMS" | awk '{print $1}')"
        if [ -z "$expected" ]; then
            w "no checksum entry for $ASSET — skipping verification"
        else
            actual="$(sha256sum "$TMPDIR/$ASSET" | awk '{print $1}')"
            if [ "$expected" != "$actual" ]; then
                e "checksum mismatch — download may be corrupted.
     Try again or install from source:
       cargo install --git https://github.com/${REPO} agentum"
            fi
            o "checksum" "verified"
        fi
    else
        w "sha256sum not found — skipping verification"
    fi

    s "Extracting" "${C_D}${ASSET}${C_R}"
    tar -xzf "$TMPDIR/$ASSET" -C "$TMPDIR"
    EXTRACTED="$TMPDIR/agentum-${VERSION}-${PLATFORM}"
}

# ── Install ────────────────────────────────────────────────────
install_binary() {
    mkdir -p "$INSTALL_DIR"
    src="$EXTRACTED/agentum"
    [ -f "$src" ] || src="$TMPDIR/agentum"
    [ -f "$src" ] || e "binary not found in tarball — release structure may have changed"
    mv "$src" "$INSTALL_DIR/agentum"
    chmod +x "$INSTALL_DIR/agentum"
    o "installed" "$INSTALL_DIR/agentum"
}

check_path() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            w "note" "$INSTALL_DIR is not on your PATH"
            h "Add:  export PATH=\"$INSTALL_DIR:\$PATH\""
            h "(or add to your shell rc file for persistence)"
            ;;
    esac
}

# ── Post-install helpers ───────────────────────────────────────
check_tmux() {
    if command -v tmux >/dev/null 2>&1; then
        o "tmux" "$(tmux -V 2>/dev/null || echo 'found')"
    else
        w "tmux not found — agentum requires tmux to manage sessions"
        case "$(uname -s)" in
            Linux)  h "Install: apt install tmux | dnf install tmux" ;;
            Darwin) h "Install: brew install tmux" ;;
        esac
    fi
}

AGENTUM_BIN() { echo "$INSTALL_DIR/agentum"; }

# ── Post-install onboarding ────────────────────────────────────
# Keep it short. One ordered list per mode. No auto-running doctor
# or --help dumps — those are one keystroke away if the user wants them.

# Ask whether to auto-start agentum serve as a daemon (systemd user
# unit when available, otherwise nohup background). Only offered for
# server/both modes and only in interactive installs.
offer_auto_start() {
    if [ "$INTERACTIVE" != true ]; then
        return 0
    fi
    printf '\n'
    printf '  %s▸%s %sStart agentum now?%s\n\n' "${C_A}" "${C_R}" "${C_B}" "${C_R}"
    printf '  %s🖥️   [1] %sYes — start in background (survives terminal close)%s\n' "${C_B}" "${C_G}" "${C_R}"
    printf '  %s📋  [2] %sYes — systemd user unit (auto-starts at login)%s\n' "${C_B}" "${C_BL}" "${C_R}"
    printf '  %s✖   [3] %sNo — I\u2019ll start it manually later%s\n' "${C_B}" "${C_D}" "${C_R}"
    while true; do
        printf '  %sChoice [1-3] (3):%s ' "${C_D}" "${C_R}"
        choice=""; read_input choice; choice="${choice:-3}"
        case "$choice" in
            1)
                printf '\n'
                if [ -x "$INSTALL_DIR/agentum" ]; then
                    # Start in background with nohup so it survives terminal close.
                    # stdout/stderr go to $XDG_STATE_HOME/agentum/daemon.log
                    LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/agentum"
                    mkdir -p "$LOG_DIR"
                    nohup "$INSTALL_DIR/agentum" serve </dev/null >"$LOG_DIR/daemon.log" 2>&1 &
                    PID=$!
                    sleep 1.5
                    if kill -0 "$PID" 2>/dev/null; then
                        o "agentum serve started" "pid ${PID}"
                        h "Logs: ${LOG_DIR}/daemon.log"
                        h "Dashboard: https://127.0.0.1:8822"
                    else
                        w "agentum serve may not have started — check logs: ${LOG_DIR}/daemon.log"
                    fi
                fi
                break ;;
            2)
                printf '\n'
                setup_systemd_user_unit
                break ;;
            3|q|quit|"")
                h "Start manually with:  ${C_C}agentum serve${C_R}   or   ${C_C}agentum serve --detach${C_R}"
                break ;;
            *) printf '  %sEnter 1, 2 or 3.%s\n' "${C_RED}" "${C_R}" ;;
        esac
    done
}

# Install a systemd user unit so agentum serve starts at login and
# restarts on crash. User units don't need root; they run as the
# installing user. Compatible with Linux distributions that ship
# systemd (>= 235 for user units).
setup_systemd_user_unit() {
    SYSTEMD_USER_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    UNIT_NAME="agentum.service"
    UNIT_PATH="$SYSTEMD_USER_DIR/$UNIT_NAME"

    if ! command -v systemctl >/dev/null 2>&1; then
        w "systemctl not found — falling back to background start"
        LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/agentum"
        mkdir -p "$LOG_DIR"
        nohup "$INSTALL_DIR/agentum" serve </dev/null >"$LOG_DIR/daemon.log" 2>&1 &
        h "agentum started in background (pid $!)"
        h "Dashboard: https://127.0.0.1:8822"
        return 0
    fi

    mkdir -p "$SYSTEMD_USER_DIR"

    # Write a simple user service that starts agentum serve, survives
    # logout (lingering), and auto-restarts on crash.
    cat >"$UNIT_PATH" <<UNITEOF
[Unit]
Description=agentum control plane
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/agentum serve
ExecReload=/bin/kill -HUP \$MAINPID
Restart=on-failure
RestartSec=5
StandardOutput=append:$XDG_STATE_HOME/agentum/daemon.log
StandardError=append:$XDG_STATE_HOME/agentum/daemon.log

[Install]
WantedBy=default.target
UNITEOF

    # Enable lingering so the user service survives logout.
    if command -v loginctl >/dev/null 2>&1; then
        loginctl enable-linger "$(whoami)" 2>/dev/null || true
    fi

    # Reload systemd user daemon
    systemctl --user daemon-reload 2>/dev/null || true

    # Enable and start the unit
    if systemctl --user enable "$UNIT_NAME" 2>/dev/null; then
        if systemctl --user start "$UNIT_NAME" 2>/dev/null; then
            o "systemd user unit installed" "enabled + started"
            h "Unit: $UNIT_PATH"
            h "Dashboard: https://127.0.0.1:8822"
            h "Manage: systemctl --user status|stop|restart agentum"
        else
            w "unit enabled but could not start — check: journalctl --user -u agentum"
        fi
    else
        w "could not enable systemd unit — wrote $UNIT_PATH, but check permissions"
    fi
}

post_server() {
    printf '\n'
    div " Get started "
    printf '\n'
    h "1. Start the server:    ${C_C}agentum serve${C_R}"
    h "2. Open the dashboard:  ${C_C}https://127.0.0.1:8822${C_R}  ${C_D}(self-signed TLS)${C_R}"
    h "3. Get your token:      ${C_C}agentum auth show${C_R}"
    h "4. Create an agent:     ${C_C}agentum new alpha --tool claude --dir . --up${C_R}"
    printf '\n'
    h "Health check: ${C_C}agentum doctor${C_R}    Phone trust: ${C_C}http://127.0.0.1:8823/api/cert${C_R}"
    printf '\n'
    offer_auto_start
}

post_cli() {
    printf '\n'
    div " Get started "
    printf '\n'
    h "1. Open the terminal UI:  ${C_C}agentum terminal${C_R}   ${C_D}(fullscreen TUI)${C_R}"
    h "2. Create an agent:       ${C_C}agentum new my-agent --tool claude --dir . --up${C_R}"
    h "3. Watch / manage:        ${C_C}agentum tail my-agent${C_R} · ${C_C}ls${C_R} · ${C_C}ps${C_R} · ${C_C}open${C_R} · ${C_C}down${C_R}"
    printf '\n'
    h "Want a web dashboard later? Run ${C_C}agentum serve${C_R} anytime."
    printf '\n'
}

post_both() {
    printf '\n'
    div " Get started "
    printf '\n'
    h "${C_B}From the terminal${C_R}"
    h "  1. Open the terminal UI:  ${C_C}agentum terminal${C_R}   ${C_D}(fullscreen TUI)${C_R}"
    h "  2. Create an agent:       ${C_C}agentum new my-agent --tool claude --dir . --up${C_R}"
    h "  3. Watch / manage:        ${C_C}agentum tail my-agent${C_R} · ${C_C}ls${C_R} · ${C_C}ps${C_R} · ${C_C}open${C_R} · ${C_C}down${C_R}"
    printf '\n'
    h "${C_B}From the dashboard${C_R}"
    h "  4. Start the server:      ${C_C}agentum serve${C_R}"
    h "  5. Open in browser:       ${C_C}https://127.0.0.1:8822${C_R}  ${C_D}(self-signed TLS)${C_R}"
    h "  6. Get your token:        ${C_C}agentum auth show${C_R}"
    printf '\n'
    h "Health check: ${C_C}agentum doctor${C_R}"
    printf '\n'
    offer_auto_start
}

# ── Main ───────────────────────────────────────────────────────
banner

detect_platform
i "platform" "${C_D}${PLATFORM}${C_R}"
detect_version
i "version"  "${C_D}${VERSION}${C_R}"
i "install"  "${C_D}${INSTALL_DIR}${C_R}"

detect_existing
if [ -n "$EXISTING_BIN" ]; then
    target="${VERSION#v}"
    have="${EXISTING_VERSION#v}"
    if [ "$have" = "$target" ] && [ -z "${AGENTUM_FORCE_UPDATE:-}" ]; then
        o "already up to date" "${C_D}v${have} at ${EXISTING_BIN}${C_R}"
        h "Re-install anyway: AGENTUM_FORCE_UPDATE=1 curl … | sh"
        printf '\n'
        exit 0
    fi
    i "updating" "${C_D}v${have} → v${target}${C_R}"
    IS_UPDATE=true
    # Updates re-use the existing install — skip the mode prompt unless the
    # user explicitly asked for a different mode. Same binary either way.
    [ -z "$INSTALL_MODE" ] && INSTALL_MODE="both"
fi

select_mode

if [ "$IS_UPDATE" = true ]; then
    s "Updating" "${C_D}v${EXISTING_VERSION#v} → ${VERSION}${C_R}"
else
    case "$INSTALL_MODE" in
        server) s "Installing" "${C_Y}Control Plane${C_R} — server + dashboard + TLS + tmux" ;;
        cli)    s "Installing" "${C_BL}Terminal CLI${C_R} — lightweight tmux session manager" ;;
        both)   s "Installing" "${C_G}Both${C_R} — Control Plane + Terminal CLI (same binary)" ;;
    esac
fi

download
install_binary
check_path

printf '\n'
check_tmux

if [ "$IS_UPDATE" = true ]; then
    printf '\n  %s%s✨  Updated to %s%s\n\n' "${C_Y}" "${C_B}" "${VERSION}" "${C_R}"
else
    case "$INSTALL_MODE" in
        server) post_server ;;
        cli)    post_cli ;;
        both)   post_both ;;
    esac
    printf '  %s%s✨  agentum is ready!%s\n\n' "${C_Y}" "${C_B}" "${C_R}"
fi
