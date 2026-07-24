#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UI="$ROOT/crates/agentum-desktop/ui"

cd "$UI"
npm exec vitest run \
  src/hooks/useWorktreeHarnessRun.test.ts \
  src/components/gated-run/GatedRunBar.test.tsx
echo "qa: reconnects, gate transitions, worktree switching, and progress-region markup passed"
