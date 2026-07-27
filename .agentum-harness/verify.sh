#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UI="$ROOT/crates/agentum-desktop/ui"

cd "$ROOT"
cargo test -p agentum-server --lib

cd "$UI"
npm exec vitest run \
  src/components/sidebar/workspace-kanban-worktree-groups.test.ts \
  src/components/sidebar/workspace-kanban-tracker-board.test.ts \
  src/components/sidebar/WorkspaceKanbanDrawerHeader.test.tsx
npm run build
