#!/usr/bin/env bash
set -euo pipefail
# The UNIT-TEST gate for spec 358 (016): exit 0 = green (advance to QA),
# non-zero = red (retry/block). $HARNESS_FEATURE_ID names the feature under
# test. Per the spec's verification section: the agentum-server lib tests
# (which include the new SDD-loop check-in tests) + rustfmt.
cd "$(dirname "$0")/.."

CARGO="${CARGO:-cargo}"
command -v "$CARGO" >/dev/null 2>&1 || CARGO="$HOME/.cargo/bin/cargo"

"$CARGO" test -p agentum-server --lib
"$CARGO" fmt --check
