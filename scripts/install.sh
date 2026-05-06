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

# ── Colors (agentum: dark bg + gold/amber accents) ────────────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_R='\033[0m';    C_B='\033[1m';     C_D='\033[2m'
    C_Y='\033[1;33m'; C_BL='\033[1;34m'; C_G='\033[1;32m'
    C_RED='\033[1;31m'; C_C='\033[1;36m'; C_A='\033[38;5;214m'
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
select_mode() {
    case "${INSTALL_MODE}" in server|cli) return ;; esac
    if [ "$INTERACTIVE" != true ]; then
        INSTALL_MODE="server"
        i "mode" "defaulting to ${C_B}server${C_R} (use --mode cli for terminal-only)"
        return
    fi
    printf '\n  %sChoose your install:%s\n\n' "${C_Y}" "${C_R}"
    printf '  %s🖥️   [1] %sControl Plane%s\n'       "${C_B}" "${C_Y}" "${C_R}"
    printf '       Server · Dashboard · TLS · tmux — full web UI\n'
    printf '       %s›%s %sagentum serve%s on your LAN, dashboard from any device\n\n' "${C_D}" "${C_R}" "${C_C}" "${C_R}"
    printf '  %s⌨️   [2] %sTerminal CLI%s\n'         "${C_B}" "${C_BL}" "${C_R}"
    printf '       CLI-only tmux session manager, no server/TLS\n'
    printf '       %s›%s %sagentum new/up/down/ls/tail%s from your terminal\n\n' "${C_D}" "${C_R}" "${C_C}" "${C_R}"
    while true; do
        printf '  %sChoice [1-2] (1):%s ' "${C_D}" "${C_R}"
        choice=""; read_input choice; choice="${choice:-1}"
        case "$choice" in
            1|server|Server) INSTALL_MODE="server"; break ;;
            2|cli|CLI|client) INSTALL_MODE="cli"; break ;;
            q|quit) printf '\n  %sCancelled.%s\n\n' "${C_D}" "${C_R}"; exit 0 ;;
            *) printf '  %sEnter 1 or 2.%s\n' "${C_RED}" "${C_R}" ;;
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

# ── Post-install: Control Plane ────────────────────────────────
post_server() {
    printf '\n'
    div " Control Plane Setup "
    printf '\n'
    check_tmux
    printf '\n'
    bin="$(AGENTUM_BIN)"
    if [ -x "$bin" ]; then
        s "Running" "${C_C}agentum doctor${C_R}"
        "$bin" doctor 2>&1 || w "doctor reported issues — review output above"
    else
        h "Run ${C_C}agentum doctor${C_R} once the binary is on PATH"
    fi
    printf '\n'
    div " Next Steps "
    printf '\n'
    h "1. Start server:  ${C_C}agentum serve${C_R}"
    h "2. Dashboard:      ${C_C}https://127.0.0.1:8822${C_R}"
    h "3. Auth token:     ${C_C}agentum auth show${C_R}"
    h "4. First agent:    ${C_C}agentum new alpha --tool claude --dir ~/project --up${C_R}"
    printf '\n'
    h "TLS is self-signed — accept the browser warning."
    h "For phone: visit ${C_C}http://127.0.0.1:8823/api/cert${C_R} to trust."
    printf '\n'
}

# ── Post-install: Terminal CLI ─────────────────────────────────
post_cli() {
    printf '\n'
    div " Terminal CLI "
    printf '\n'
    check_tmux
    printf '\n'
    bin="$(AGENTUM_BIN)"
    if [ -x "$bin" ]; then
        s "Quick help" "${C_C}agentum --help${C_R}"
        "$bin" --help 2>&1 | head -20 || true
    fi
    printf '\n'
    div " Get Started "
    printf '\n'
    h "Create your first agent:"
    h "  ${C_C}agentum new my-agent --tool claude --dir ~/project --up${C_R}"
    printf '\n'
    h "Common:  ${C_C}ls${C_R} | ${C_C}ps${C_R} | ${C_C}open <n>${C_R} | ${C_C}tail <n>${C_R} | ${C_C}send <n> <t>${C_R}"
    h "Upgrade: ${C_C}agentum serve${C_R} to add the dashboard anytime"
    printf '\n'
}

# ── Main ───────────────────────────────────────────────────────
banner

detect_platform
i "platform" "${C_D}${PLATFORM}${C_R}"
detect_version
i "version"  "${C_D}${VERSION}${C_R}"
i "install"  "${C_D}${INSTALL_DIR}${C_R}"

select_mode

case "$INSTALL_MODE" in
    server) s "Installing" "${C_Y}Control Plane${C_R} — server + dashboard + TLS + tmux" ;;
    cli)    s "Installing" "${C_BL}Terminal CLI${C_R} — lightweight tmux session manager" ;;
esac

download
install_binary
check_path

case "$INSTALL_MODE" in
    server) post_server ;;
    cli)    post_cli ;;
esac

printf '\n  %s%s✨  agentum is ready!%s\n\n' "${C_Y}" "${C_B}" "${C_R}"
