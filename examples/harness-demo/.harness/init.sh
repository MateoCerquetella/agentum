#!/usr/bin/env bash
# Environment smoke-test, run ONCE before any feature. A non-zero exit aborts
# the whole harness run (the environment isn't ready). Keep it fast and cheap.
set -euo pipefail

echo "[init] harness-demo environment check"
echo "[init] pwd: $(pwd)"

# Tools this demo's gate relies on.
command -v bash >/dev/null 2>&1 || { echo "[init] FAIL: bash not found"; exit 1; }
command -v grep >/dev/null 2>&1 || { echo "[init] FAIL: grep not found"; exit 1; }

echo "[init] OK — environment is ready"
exit 0
