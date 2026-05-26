#!/bin/sh
# agentum installer — one-question wizard.
#
# curl -fsSL https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh | sh
#
# Options (passed after -- when piping):
#   curl ... | sh -s -- --mode host          # this machine runs agents (daemon)
#   curl ... | sh -s -- --mode client        # this machine only connects to remote daemons
#   curl ... | sh -s -- --no-interactive     # skip prompts, default to host mode
#   curl ... | sh -s -- --expose lan         # bind daemon to 0.0.0.0 (VPS / shared box)
#   curl ... | sh -s -- --expose loopback    # bind daemon to 127.0.0.1 (default; laptops)
#
# Environment:
#   INSTALL_MODE=host|client    CI / non-interactive mode
#   AGENTUM_EXPOSE=lan|loopback CI / non-interactive bind selection (host mode only).
#                               When unset and not interactive, defaults to loopback.
#                               Pick "lan" on a VPS so the dashboard is reachable.
#   INSTALL_DIR=$HOME/.local/bin  Override install path
#
# Legacy mode tokens server/cli/both are still accepted: server→host,
# cli→client, both→host. The binary is the same; mode only affects the
# post-install wizard.
#
# SPDX-License-Identifier: MIT
set -eu

# ── Configuration ──────────────────────────────────────────────
REPO="mateocerquetella/agentum"
GH_API="https://api.github.com/repos/${REPO}/releases/latest"
GH_DL="https://github.com/${REPO}/releases/download"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
PLATFORM=""; VERSION=""; INSTALL_MODE="${INSTALL_MODE:-}"; INTERACTIVE=false; HAS_TTY=false
EXPOSE="${AGENTUM_EXPOSE:-}"
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
        --expose) EXPOSE="$2"; shift 2 ;;
        --expose=*) EXPOSE="${1#*=}"; shift ;;
        -h|--help) printf 'Usage: install.sh [--mode host|client] [--expose lan|loopback] [--no-interactive]\nEnv: INSTALL_MODE=host|client, AGENTUM_EXPOSE=lan|loopback\n'; exit 0 ;;
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
# One binary, one question: will this machine run agents (host mode)
# or just connect to a remote agentum (client mode)? Same artifact
# either way — the answer only steers the post-install wizard.
#
# Legacy tokens server|both fold into host; cli folds into client.
select_mode() {
    # Normalize legacy tokens up front so the rest of the script only
    # ever sees host|client.
    case "${INSTALL_MODE}" in
        server|both) INSTALL_MODE="host" ;;
        cli)         INSTALL_MODE="client" ;;
    esac
    case "${INSTALL_MODE}" in host|client) return ;; esac

    if [ "$INTERACTIVE" != true ]; then
        INSTALL_MODE="host"
        i "mode" "defaulting to ${C_B}host${C_R} (use --mode host|client to override)"
        return
    fi

    printf '\n  %sWill this machine run agents?%s\n\n' "${C_Y}" "${C_R}"
    printf '  %s🖥️   [1] %sYes — run agents here%s %s(recommended)%s\n' "${C_B}" "${C_G}" "${C_R}" "${C_D}" "${C_R}"
    printf '       %sStarts a local agentum daemon. Dashboard + TUI both work.%s\n\n' "${C_D}" "${C_R}"
    printf '  %s📡  [2] %sNo — just connect to a remote agentum%s\n' "${C_B}" "${C_BL}" "${C_R}"
    printf '       %sUse this machine as a client only. You’ll point it at%s\n' "${C_D}" "${C_R}"
    printf '       %sanother machine (VPS, work laptop, etc.) running agentum.%s\n\n' "${C_D}" "${C_R}"
    while true; do
        printf '  %sChoice [1-2] (1):%s ' "${C_D}" "${C_R}"
        choice=""; read_input choice; choice="${choice:-1}"
        case "$choice" in
            1|host|Host|server|Server|both|Both) INSTALL_MODE="host";   break ;;
            2|client|Client|cli|CLI|remote)      INSTALL_MODE="client"; break ;;
            q|quit) printf '\n  %sCancelled.%s\n\n' "${C_D}" "${C_R}"; exit 0 ;;
            *) printf '  %sEnter 1 or 2.%s\n' "${C_RED}" "${C_R}" ;;
        esac
    done
    printf '\n'
}

# ── LAN IP detection ───────────────────────────────────────────
# Best-effort: returns the IPv4 address other devices on the same LAN
# can reach the daemon at. Falls back to 127.0.0.1 if nothing is found,
# in which case the caller should warn the user the dashboard is local-only.
detect_lan_ip() {
    ip=""
    case "$(uname -s)" in
        Darwin)
            # Try common interface names in order; the first that returns
            # an address wins. en0 is wifi on most Macs, en1 on older ones.
            for iface in en0 en1 en2 en3 en4 en5; do
                ip="$(ipconfig getifaddr "$iface" 2>/dev/null)"
                [ -n "$ip" ] && break
            done
            # Last-ditch fallback: ask the routing table which iface
            # would handle traffic to a public IP.
            if [ -z "$ip" ]; then
                route_iface="$(route -n get 1.1.1.1 2>/dev/null | awk '/interface:/ {print $2; exit}')"
                [ -n "$route_iface" ] && ip="$(ipconfig getifaddr "$route_iface" 2>/dev/null)"
            fi
            ;;
        Linux)
            # `hostname -I` returns all addresses space-separated; first
            # one is usually the primary LAN address. Some minimal
            # busybox builds lack the -I flag, so we have a fallback.
            ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
            if [ -z "$ip" ] && command -v ip >/dev/null 2>&1; then
                ip="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for(i=1;i<=NF;i++) if($i=="src"){print $(i+1); exit}}')"
            fi
            ;;
    esac
    # Strip anything that doesn't look like an IPv4 dotted quad — guards
    # against locales or busybox builds that print extra noise.
    case "$ip" in
        *[!0-9.]*|"") ip="" ;;
    esac
    [ -z "$ip" ] && ip="127.0.0.1"
    echo "$ip"
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

# Offer to create the first admin account right after a fresh install.
# Skipped on updates (account already exists) and in non-interactive mode.
# Runs `agentum auth setup` with stdin from /dev/tty so password prompts
# reach the terminal even when the parent script has a redirected stdin.
offer_auth_setup() {
    [ "$INTERACTIVE" = true ] || return 0
    [ "$IS_UPDATE" = false ]  || return 0
    [ "$HAS_TTY" = true ]     || return 0

    printf '\n'
    printf '  %s▸%s %sCreate admin account now?%s ' "${C_A}" "${C_R}" "${C_B}" "${C_R}"
    printf '%s[Y/n]:%s ' "${C_D}" "${C_R}"
    choice=""; read_input choice; choice="${choice:-y}"
    case "$choice" in
        y|Y|yes|Yes|YES)
            printf '\n'
            if [ -x "$INSTALL_DIR/agentum" ]; then
                if "$INSTALL_DIR/agentum" auth setup </dev/tty; then
                    printf '\n'
                    o "admin account ready"
                else
                    w "setup failed — run '${C_C}agentum auth setup${C_R}' manually before starting the server"
                fi
            else
                w "binary not found at $INSTALL_DIR/agentum — cannot run setup"
            fi
            ;;
        *)
            h "Run '${C_C}agentum auth setup${C_R}' before starting the server"
            h "or register via the dashboard on first load."
            ;;
    esac
}

# Ask whether to auto-start agentum serve as a daemon (systemd user
# unit when available, otherwise nohup background). Only offered for
# host mode in interactive installs. The dashboard URL passed in is
# what we'll print on success; serve_args carries any extra flags
# (e.g. --host 0.0.0.0 when the user opted into LAN exposure).
offer_auto_start() {
    if [ "$INTERACTIVE" != true ]; then
        return 0
    fi
    dash_url="${1:-https://127.0.0.1:8822}"
    serve_args="${2:-}"
    printf '\n'
    printf '  %s▸%s %sStart agentum now?%s\n\n' "${C_A}" "${C_R}" "${C_B}" "${C_R}"
    printf '  %s🖥️   [1] %sYes — start now and auto-start at login%s %s(recommended)%s\n' "${C_B}" "${C_G}" "${C_R}" "${C_D}" "${C_R}"
    printf "  %s✖   [2] %sNot now — I'll start it manually later%s\n" "${C_B}" "${C_D}" "${C_R}"
    while true; do
        printf '  %sChoice [1-2] (1):%s ' "${C_D}" "${C_R}"
        choice=""; read_input choice; choice="${choice:-1}"
        case "$choice" in
            1|y|Y|yes|Yes)
                printf '\n'
                setup_autostart "$dash_url" "$serve_args"
                break ;;
            2|n|N|no|No|q|quit)
                h "Start manually with:  ${C_C}agentum serve${C_R}   or   ${C_C}agentum serve --detach${C_R}"
                break ;;
            *) printf '  %sEnter 1 or 2.%s\n' "${C_RED}" "${C_R}" ;;
        esac
    done
}

# Dispatch to the right OS-specific autostart installer. Each one
# installs persistence (LaunchAgent or systemd user unit) AND starts
# the daemon now, so the combined "Yes" answer always lands the user
# with a running, surviving daemon. serve_args, if non-empty, gets
# baked into the unit's ExecStart / ProgramArguments so the daemon
# always boots with the user-chosen bind address.
setup_autostart() {
    dash_url="${1:-https://127.0.0.1:8822}"
    serve_args="${2:-}"
    case "$(uname -s)" in
        Darwin) setup_launchd_user_agent "$dash_url" "$serve_args" ;;
        Linux)  setup_systemd_user_unit   "$dash_url" "$serve_args" ;;
        *)      fallback_nohup_start      "$dash_url" "$serve_args" ;;
    esac
}

# macOS: install a LaunchAgent plist in ~/Library/LaunchAgents that
# survives logout (RunAtLoad + KeepAlive). launchctl bootstrap is
# both "register persistence" and "start it now" — one call covers
# both halves of the user's "yes" answer.
setup_launchd_user_agent() {
    dash_url="${1:-https://127.0.0.1:8822}"
    serve_args="${2:-}"
    LA_DIR="$HOME/Library/LaunchAgents"
    LA_LABEL="dev.agentum.daemon"
    LA_PATH="$LA_DIR/${LA_LABEL}.plist"
    LOG_DIR="$HOME/Library/Logs/agentum"

    mkdir -p "$LA_DIR" "$LOG_DIR"

    # If a prior plist exists (reinstall), unload it first so the
    # next bootstrap picks up the new binary path / arguments.
    if [ -f "$LA_PATH" ]; then
        launchctl unload "$LA_PATH" 2>/dev/null || true
    fi

    # Build the optional extra <string> arg entries for ProgramArguments.
    # We split serve_args on whitespace so callers can pass e.g.
    # "--host 0.0.0.0" without quoting gymnastics here.
    extra_args_xml=""
    if [ -n "$serve_args" ]; then
        # shellcheck disable=SC2086
        set -- $serve_args
        for arg in "$@"; do
            extra_args_xml="${extra_args_xml}
        <string>${arg}</string>"
        done
    fi

    # Heredoc is the cleanest way to keep XML readable; we escape
    # nothing because every interpolated value here is a path we
    # control. ProgramArguments must be one entry per <string>.
    cat >"$LA_PATH" <<PLISTEOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${LA_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${INSTALL_DIR}/agentum</string>
        <string>serve</string>${extra_args_xml}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${LOG_DIR}/daemon.log</string>
    <key>StandardErrorPath</key>
    <string>${LOG_DIR}/daemon.log</string>
    <key>WorkingDirectory</key>
    <string>${HOME}</string>
</dict>
</plist>
PLISTEOF

    # Load and start in one step. `bootstrap gui/<uid>` is the
    # modern equivalent of `load -w` and works in user sessions.
    if launchctl bootstrap "gui/$(id -u)" "$LA_PATH" 2>/dev/null; then
        sleep 1
        o "LaunchAgent installed" "starts now + at login"
        h "Plist:     $LA_PATH"
        h "Logs:      $LOG_DIR/daemon.log"
        h "Dashboard: ${dash_url}"
        h "Manage:    launchctl bootout gui/$(id -u) $LA_PATH   ${C_D}# disable${C_R}"
    else
        # `bootstrap` returns non-zero if the label is already
        # loaded. Try the older `load` syntax as a fallback so
        # reinstalls don't trip on stale state.
        if launchctl load "$LA_PATH" 2>/dev/null; then
            sleep 1
            o "LaunchAgent loaded" "starts at login"
            h "Plist:     $LA_PATH"
            h "Dashboard: ${dash_url}"
        else
            w "could not register LaunchAgent — wrote $LA_PATH, falling back to background start"
            fallback_nohup_start "$dash_url"
        fi
    fi
}

# Final fallback: just nohup the daemon. Used on unknown platforms
# or when launchctl/systemctl fail. Persists across terminal close
# but not across reboot.
fallback_nohup_start() {
    dash_url="${1:-https://127.0.0.1:8822}"
    serve_args="${2:-}"
    LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/agentum"
    mkdir -p "$LOG_DIR"
    # shellcheck disable=SC2086  # deliberately unquoted to split args
    nohup "$INSTALL_DIR/agentum" serve $serve_args </dev/null >"$LOG_DIR/daemon.log" 2>&1 &
    PID=$!
    sleep 1.5
    if kill -0 "$PID" 2>/dev/null; then
        o "agentum serve started" "pid ${PID}"
        h "Logs:      ${LOG_DIR}/daemon.log"
        h "Dashboard: ${dash_url}"
        h "(no auto-start at login configured — restart this command after a reboot.)"
    else
        w "agentum serve may not have started — check logs: ${LOG_DIR}/daemon.log"
    fi
}

# Install a systemd user unit so agentum serve starts at login and
# restarts on crash. User units don't need root; they run as the
# installing user. Compatible with Linux distributions that ship
# systemd (>= 235 for user units).
setup_systemd_user_unit() {
    SYSTEMD_USER_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    UNIT_NAME="agentum.service"
    UNIT_PATH="$SYSTEMD_USER_DIR/$UNIT_NAME"
    dash_url="${1:-https://127.0.0.1:8822}"
    serve_args="${2:-}"

    if ! command -v systemctl >/dev/null 2>&1; then
        w "systemctl not found — falling back to background start"
        LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/agentum"
        mkdir -p "$LOG_DIR"
        # shellcheck disable=SC2086
        nohup "$INSTALL_DIR/agentum" serve $serve_args </dev/null >"$LOG_DIR/daemon.log" 2>&1 &
        h "agentum started in background (pid $!)"
        h "Dashboard: ${dash_url}"
        return 0
    fi

    mkdir -p "$SYSTEMD_USER_DIR"

    # Append serve_args (e.g. "--host 0.0.0.0") to ExecStart when the
    # user opted into LAN exposure; default is loopback-only.
    exec_start="$INSTALL_DIR/agentum serve"
    [ -n "$serve_args" ] && exec_start="$exec_start $serve_args"

    # Write a simple user service that starts agentum serve, survives
    # logout (lingering), and auto-restarts on crash.
    cat >"$UNIT_PATH" <<UNITEOF
[Unit]
Description=agentum control plane
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$exec_start
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
            h "Dashboard: ${dash_url}"
            h "Manage: systemctl --user status|stop|restart agentum"
        else
            w "unit enabled but could not start — check: journalctl --user -u agentum"
        fi
    else
        w "could not enable systemd unit — wrote $UNIT_PATH, but check permissions"
    fi
}

# Register the local daemon as the default `local` profile so
# `agentum terminal` Just Works the moment the daemon is up. We
# always use 127.0.0.1: this profile is the *same-machine* shortcut,
# and loopback is stable across networks (LAN IPs change when you
# move between WiFi networks, VPNs, tethering, etc.) and works even
# when the daemon is bound to loopback only. Other machines should
# be added as named profiles via `agentum profiles add NAME URL`.
#
# We upsert (saving over any prior `local` entry) and mark it default
# only when no other default exists — so users who already pointed
# at a remote VPS don't have their preference overwritten by a
# reinstall.
register_local_profile() {
    [ -x "$INSTALL_DIR/agentum" ] || return 0
    url="https://127.0.0.1:8822"
    set_default_flag=""
    if ! "$INSTALL_DIR/agentum" profiles list 2>/dev/null | awk 'NR>1 {print $2}' | grep -q '^\*$'; then
        set_default_flag="--set-default"
    fi
    if "$INSTALL_DIR/agentum" profiles add local "$url" --insecure $set_default_flag >/dev/null 2>&1; then
        o "profile saved" "local → ${url}"
    fi
}

# Ask whether the daemon should be reachable from other devices on
# the LAN. Default is "no" (workstation mode: loopback-only), which
# is the right call for a personal laptop. "Yes" is the shared-host
# case: a VPS, a lab box, a desktop you want the rest of the house
# to drive. Returns "lan" or "loopback" on stdout.
ask_expose() {
    # Pre-answered via --expose / AGENTUM_EXPOSE — skip the prompt.
    # Honored in both interactive and non-interactive runs so the
    # value is the authoritative choice when the caller has one.
    if [ -n "$EXPOSE" ]; then
        case "$EXPOSE" in
            lan|share|shared|0.0.0.0)     echo "lan"; return 0 ;;
            loopback|local|127.0.0.1|lo)  echo "loopback"; return 0 ;;
            *) w "ignoring unknown --expose value" "$EXPOSE (use lan|loopback)" ;;
        esac
    fi
    if [ "$INTERACTIVE" != true ]; then
        echo "loopback"
        return 0
    fi
    printf '\n  %sShould this daemon be reachable from other devices?%s\n\n' "${C_Y}" "${C_R}" >&2
    printf '  %s🔒  [1] %sNo — just me on this machine%s %s(recommended)%s\n' "${C_B}" "${C_G}" "${C_R}" "${C_D}" "${C_R}" >&2
    printf '       %sDaemon binds 127.0.0.1 only. Other devices can'"'"'t connect.%s\n\n' "${C_D}" "${C_R}" >&2
    printf '  %s🌐  [2] %sYes — share with other devices on my network%s\n' "${C_B}" "${C_BL}" "${C_R}" >&2
    printf '       %sDaemon binds 0.0.0.0. Anyone with the token + LAN access can connect.%s\n\n' "${C_D}" "${C_R}" >&2
    while true; do
        printf '  %sChoice [1-2] (1):%s ' "${C_D}" "${C_R}" >&2
        choice=""; read_input choice; choice="${choice:-1}"
        case "$choice" in
            1|n|N|no|No|local|loopback) echo "loopback"; return 0 ;;
            2|y|Y|yes|Yes|lan|share|shared) echo "lan"; return 0 ;;
            *) printf '  %sEnter 1 or 2.%s\n' "${C_RED}" "${C_R}" >&2 ;;
        esac
    done
}

# Register the local OS clip-agent so Ctrl-V image paste works from a
# remote TUI (the daemon brokers the clipboard hop). Skipped in
# non-interactive runs (CI, `curl … | sh` without a tty) so the
# installer never silently registers a launchd plist or systemd unit
# under automation. Users can run `agentum clip-agent --install` later.
#
# Escape hatch: AGENTUM_INSTALL_NO_CLIP_AGENT=1 forces skip. Used by
# `agentum update --skip-clip-agent` and exposed for ops scripts.
#
# Test seam: AGENTUM_INSTALL_DRY_RUN=1 prints `would run: …` instead
# of invoking the real subcommand, so `tests/install_clip_agent_autostart.sh`
# can pin the gate logic without touching launchctl/systemctl.
install_clip_agent_autostart() {
    if [ -n "${AGENTUM_INSTALL_NO_CLIP_AGENT:-}" ]; then
        h "clipboard agent autostart skipped (AGENTUM_INSTALL_NO_CLIP_AGENT set)"
        h "run later: ${C_C}agentum clip-agent --install${C_R}"
        return 0
    fi
    # Gate on the script-wide INTERACTIVE flag set by the tty probe at
    # install.sh:71-78. Works whether install.sh runs from a real tty,
    # via `curl | sh` (which sets INTERACTIVE=false because stdin is
    # piped), or under CI (CI=1 forces INTERACTIVE=false).
    if [ "$INTERACTIVE" != true ]; then
        h "clipboard agent autostart skipped (non-interactive install)"
        h "run later: ${C_C}agentum clip-agent --install${C_R}"
        return 0
    fi
    # Defensive — main flow runs install_binary first, but a future
    # refactor that reorders these calls should still get a clean
    # warning rather than a "binary missing" surprise from launchctl.
    if [ ! -x "$INSTALL_DIR/agentum" ]; then
        w "skipping clip-agent autostart" "binary missing at $INSTALL_DIR/agentum"
        return 0
    fi
    # `clip-agent --install` only knows launchd and systemd. Skip on
    # anything else (Windows, exotic Unix) — the user can run the
    # subcommand manually if they have a custom launch story.
    case "$(uname -s)" in
        Darwin|Linux) ;;
        *)
            h "clipboard agent autostart skipped (unsupported platform: $(uname -s))"
            return 0
            ;;
    esac
    if [ -n "${AGENTUM_INSTALL_DRY_RUN:-}" ]; then
        echo "would run: $INSTALL_DIR/agentum clip-agent --install"
        return 0
    fi
    if "$INSTALL_DIR/agentum" clip-agent --install; then
        o "clip-agent installed" "starts at login"
    else
        w "clip-agent --install failed — image paste from Mac→remote will not work"
        h "run later: ${C_C}agentum clip-agent --install${C_R}"
    fi
}

# Final tip both paths share. Surfaces multi-machine control without
# making first-run users learn the concept up front.
multi_server_tip() {
    printf '\n'
    h "Tip: agentum can connect to other machines too —"
    h "     run ${C_C}agentum profiles add NAME URL${C_R} to switch between them."
    printf '\n'
}

# Host mode: daemon lives on this machine. Two sub-modes:
#
#   workstation (default): daemon binds 127.0.0.1 only. Dashboard URL
#       is loopback. Right for a personal laptop where agentum is
#       just for you, even if you also point at a remote VPS profile.
#
#   shared: daemon binds 0.0.0.0 with --host 0.0.0.0. Dashboard URL
#       uses the detected LAN IP so other devices/users can connect.
#       Right for a VPS, a home server, a shared dev box.
#
# Either way the `local` profile is loopback — it's the same-machine
# shortcut and loopback is what works regardless of network.
post_host() {
    expose="$(ask_expose)"
    serve_args=""
    if [ "$expose" = "lan" ]; then
        lan_ip="$(detect_lan_ip)"
        dash_url="https://${lan_ip}:8822"
        serve_args="--host 0.0.0.0"
    else
        lan_ip="127.0.0.1"
        dash_url="https://127.0.0.1:8822"
    fi

    # Setup steps first — the user shouldn't have to scroll past a
    # reference card to find the actions still required of them.
    offer_auth_setup
    offer_auto_start "$dash_url" "$serve_args"
    register_local_profile
    # Install the clipboard agent for seamless Mac→remote Ctrl-V.
    # Gated by INTERACTIVE + AGENTUM_INSTALL_NO_CLIP_AGENT inside the
    # function, so non-interactive / CI runs become a no-op.
    install_clip_agent_autostart

    # Reference card last: by this point auth + autostart are done,
    # so these commands are "what to run when you want them again",
    # not "what to run next".
    printf '\n'
    div " Get started "
    printf '\n'
    h "Dashboard:   ${C_C}${dash_url}${C_R}  ${C_D}(self-signed TLS)${C_R}"
    h "Terminal:    ${C_C}agentum terminal${C_R}"
    h "New agent:   ${C_C}agentum new alpha --tool claude --dir . --up${C_R}"
    h "Health:      ${C_C}agentum doctor${C_R}"
    if [ "$expose" = "lan" ] && [ "$lan_ip" = "127.0.0.1" ]; then
        printf '\n'
        w "could not detect a LAN IP — daemon is bound to 0.0.0.0 but the dashboard URL above is loopback."
        h "Other devices: run 'agentum doctor' once the daemon is up to see a reachable URL."
    fi
    multi_server_tip
}

# Client mode: this machine never runs the daemon. We skip auth setup
# (no local server to auth against) and autostart, and offer to add a
# remote profile so the next command they run is `agentum terminal`.
post_client() {
    printf '\n'
    div " Get started "
    printf '\n'
    h "1. Add a remote agentum:  ${C_C}agentum profiles add NAME URL${C_R}"
    h "2. Open the terminal UI:  ${C_C}agentum terminal${C_R}   ${C_D}(connects to the default profile)${C_R}"
    h "3. Create an agent:       ${C_C}agentum new my-agent --tool claude --dir . --up${C_R}"
    printf '\n'

    # Offer to register a remote profile now so the user can go
    # straight into `agentum terminal` without a second round trip.
    if [ "$INTERACTIVE" = true ] && [ "$IS_UPDATE" = false ]; then
        printf '  %s▸%s %sAdd a remote agentum now?%s ' "${C_A}" "${C_R}" "${C_B}" "${C_R}"
        printf '%s(URL like https://my-vps:8822, blank to skip)%s\n' "${C_D}" "${C_R}"
        printf '  %sURL:%s ' "${C_D}" "${C_R}"
        remote_url=""; read_input remote_url
        if [ -n "$remote_url" ]; then
            if [ -x "$INSTALL_DIR/agentum" ]; then
                # --insecure is the safe default here: most fresh
                # daemons issue self-signed certs and we don't have
                # the fingerprint yet. Users can lock it down later
                # via `agentum profiles add --fingerprint`.
                if "$INSTALL_DIR/agentum" profiles add remote "$remote_url" --insecure --set-default >/dev/null 2>&1; then
                    o "profile saved" "remote → ${remote_url} (default)"
                    h "Connect: ${C_C}agentum terminal${C_R}"
                else
                    w "could not save profile — try manually: ${C_C}agentum profiles add remote ${remote_url}${C_R}"
                fi
            fi
        else
            h "Skipped. Run ${C_C}agentum profiles add NAME URL${C_R} when you have a daemon to point at."
        fi
    fi

    printf '\n'
    h "Tip: this machine can also run agents — ${C_C}agentum serve${C_R} anytime."
    multi_server_tip
}

# ── Source-only guard ─────────────────────────────────────────
# When sourced with `AGENTUM_INSTALL_SOURCE_ONLY=1`, return without
# running the main flow so tests can `source` the script just to get
# the function definitions. Skipping the `download`/`install_binary`
# calls keeps unit tests offline and idempotent.
if [ -n "${AGENTUM_INSTALL_SOURCE_ONLY:-}" ]; then
    return 0 2>/dev/null || exit 0
fi

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
    [ -z "$INSTALL_MODE" ] && INSTALL_MODE="host"
fi

select_mode

if [ "$IS_UPDATE" = true ]; then
    s "Updating" "${C_D}v${EXISTING_VERSION#v} → ${VERSION}${C_R}"
else
    case "$INSTALL_MODE" in
        host)   s "Installing" "${C_G}agentum${C_R} — this machine will run agents" ;;
        client) s "Installing" "${C_BL}agentum${C_R} — this machine will connect to a remote daemon" ;;
    esac
fi

download
install_binary
check_path

printf '\n'
check_tmux

if [ "$IS_UPDATE" = true ]; then
    # Re-run clip-agent install on update so newly-cut binaries pick
    # up any fixes to the launchd/systemd plist/unit they write. The
    # function is idempotent — re-installing over an existing plist
    # leaves the active service intact.
    install_clip_agent_autostart
    printf '\n  %s%s✨  Updated to %s%s\n\n' "${C_Y}" "${C_B}" "${VERSION}" "${C_R}"
else
    case "$INSTALL_MODE" in
        host)   post_host ;;
        client) post_client ;;
    esac
    printf '  %s%s✨  agentum is ready!%s\n\n' "${C_Y}" "${C_B}" "${C_R}"
fi
