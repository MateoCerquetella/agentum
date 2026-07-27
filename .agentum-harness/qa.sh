#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UI="$ROOT/crates/agentum-desktop/ui"

cd "$UI"
npm exec vitest run \
  src/components/sidebar/WorkspaceKanbanDrawerHeader.test.tsx \
  src/components/sidebar/use-workspace-kanban-card-pointer-drag.test.ts \
  src/components/sidebar/use-workspace-kanban-outside-dismiss.test.ts \
  src/components/sidebar/workspace-kanban-area-selection.test.ts \
  src/components/sidebar/workspace-kanban-card-pointer-drag-dom.test.ts \
  src/components/sidebar/workspace-kanban-sidebar-drop.test.ts \
  src/components/sidebar/workspace-kanban-tracker-board.test.ts \
  src/components/sidebar/workspace-kanban-worktree-groups.test.ts \
  src/lib/issue-project-status.test.ts
echo "qa: tracker-authoritative lanes, pessimistic moves, stale-refresh guards, and retained board interactions passed"
