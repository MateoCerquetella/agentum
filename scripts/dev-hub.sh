#!/bin/bash
# Build and run a dev hub from THIS checkout on a port, on its own copy of the neutered snapshot
# of the live data (no mail sync, no pushes, read-only calendar). Prints the session token.
#   scripts/dev-hub.sh 7101          # build web + hub, (re)start on :7101
#   scripts/dev-hub.sh 7101 --no-web # skip the wasm build
set -euo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
cd "$(dirname "$0")/.."
PORT=${1:?port}
DIR=$HOME/.cache/agentum-dev/hub-$PORT
mkdir -p "$DIR/data" && chmod 700 "$HOME/.cache/agentum-dev"
[ -f "$DIR/data/cerq.db" ] || { cp "$HOME/.cache/agentum-dev/data/cerq.db" "$HOME/.cache/agentum-dev/data/.app_key" "$DIR/data/"; }
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/.cache/agentum-target} CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
[ "${2:-}" = "--no-web" ] || scripts/build-web.sh > "$DIR/web-build.log" 2>&1 || { tail -30 "$DIR/web-build.log"; exit 1; }
cargo build -p agentum 2>&1 | grep -E "^error" -A8 && exit 1
[ -f "$DIR/pid" ] && kill "$(cat "$DIR/pid")" 2>/dev/null && sleep 0.5 || true
cp "$CARGO_TARGET_DIR/debug/agentum" "$DIR/agentum"
( cd "$DIR" && AGENTUM_DATA=$DIR/data AGENTUM_BIND=127.0.0.1:$PORT CERQ_NO_PUSH=1 CERQ_CALENDAR_READONLY=1 CERQ_SKIP_OUTLOOK=1 RUST_LOG=info exec setsid ./agentum >> hub.log 2>&1 ) < /dev/null > /dev/null 2>&1 &
echo $! > "$DIR/pid"
sleep 1.5
curl -sf "http://127.0.0.1:$PORT/api/health" > /dev/null && echo "hub on http://127.0.0.1:$PORT (log $DIR/hub.log)" || { tail -20 "$DIR/hub.log"; exit 1; }
echo "TOKEN=$(cat "$HOME/.cache/agentum-dev/session.txt")"
