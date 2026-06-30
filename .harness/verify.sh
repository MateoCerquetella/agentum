#!/usr/bin/env bash
# AutoWiki unit gate (spec 001).  exit 0 = green (advance), non-zero = red (retry/block).
# $HARNESS_FEATURE_ID selects the relevant checks; absent => run everything.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
FEAT="${HARNESS_FEATURE_ID:-all}"
echo "[verify] feature=$FEAT  root=$ROOT"

# Backend gate — slices 1 & 2 live in agentum-server (also a safety net for 3).
cargo test -p agentum-server --lib

# UI gate — the view slice must typecheck + build.
if [ "$FEAT" = "wiki-view" ] || [ "$FEAT" = "all" ]; then
  npm run build --prefix crates/agentum-desktop/ui
fi

echo "[verify] GREEN"
