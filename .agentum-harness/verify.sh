#!/usr/bin/env bash
set -euo pipefail
# The verification gate for spec 431: exit 0 = green (advance to review),
# non-zero = red (retry/block). Run the two exact commands required by AC5.
cd "$(dirname "$0")/.."

CARGO="${CARGO:-cargo}"
command -v "$CARGO" >/dev/null 2>&1 || CARGO="$HOME/.cargo/bin/cargo"

case "${HARNESS_FEATURE_ID:-F5}" in
  F1)
    npm exec --prefix crates/agentum-desktop/ui vitest run \
      src/components/sidebar/SidebarHeader.test.tsx \
      src/components/project-hub/ProjectTasksPage.test.tsx
    ;;
  F2)
    "$CARGO" test -p agentum-server --lib internal_board_route_families_are_unregistered
    ;;
  F3)
    "$CARGO" test -p agentum-server --lib task_sink::tests::only_github_and_linear_are_creation_sinks
    "$CARGO" test -p agentum-server --lib task_sink::tests::legacy_board_provider_is_non_mutating_and_best_effort
    ;;
  F4)
    "$CARGO" test -p agentum-store --lib legacy_board_rows_survive_reopen_and_normal_store_work_is_inert
    "$CARGO" test -p agentum-watchdog --lib
    ;;
  F5|*)
    "$CARGO" test --workspace --lib
    npm run build --prefix crates/agentum-desktop/ui
    ;;
esac
