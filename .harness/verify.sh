#!/usr/bin/env bash
# AutoWiki unit gate (spec 001).  exit 0 = green (advance), non-zero = red (retry/block).
# $HARNESS_FEATURE_ID selects the relevant checks; absent => run everything.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
FEAT="${HARNESS_FEATURE_ID:-all}"
echo "[verify] feature=$FEAT  root=$ROOT"

case "$FEAT" in
  side-effect-free-session-list|mode-aware-transcript-read|transcript-observer-lifecycle)
    cargo test -p agentum-server transcript_store::tests --lib -- --nocapture
    cargo test -p agentum-server routes::agent_tasks::tests --lib -- --nocapture
    cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib -- --nocapture
    cargo test -p agentum-server tests::server_wired_watchdog_callback_retires_only_non_running_claude_observers --lib -- --nocapture
    cargo test -p agentum-watchdog reconcile_passes_authoritative_running_slice_to_optional_hook_once --lib -- --nocapture
    if rg -n 'spawn_blocking|std::sync::mpsc' crates/agentum-server/src/transcript_store.rs; then
      echo "[verify] transcript observer reintroduced a blocking receiver path" >&2
      exit 1
    fi
    cargo fmt --all -- --check
    cargo test --workspace --lib
    npm run build --prefix crates/agentum-desktop/ui
    ;;
  binding-identity-fidelity)
    cargo test -p agentum-server project_trackers --lib -- --nocapture
    cargo test -p agentum-server routes::util::tests::resolve_tracker --lib -- --nocapture
    ;;
  wizard-closed-tracker-scope)
    (
      cd crates/agentum-desktop/ui
      bunx vitest run \
        src/components/new-workspace/work-item-picker-model.test.ts \
        src/components/new-workspace/create-workspace-wizard-model.test.ts \
        src/components/new-workspace/tracker-section-scope.test.ts \
        src/runtime/github-projects-client.test.ts
      bunx vitest run src/store/slices/worktrees.test.ts \
        -t "persists exact selected tracker coordinates and omits them for an unlinked create"
    )
    npm run build --prefix crates/agentum-desktop/ui
    ;;
  wiki-contract|wiki-routes)
    cargo test -p agentum-server --lib
    ;;
  wiki-view)
    cargo test -p agentum-server --lib
    npm run build --prefix crates/agentum-desktop/ui
    ;;
  all)
    cargo fmt --all -- --check
    cargo test --workspace --lib
    (
      cd crates/agentum-desktop/ui
      bunx vitest run \
        src/components/new-workspace/work-item-picker-model.test.ts \
        src/components/new-workspace/create-workspace-wizard-model.test.ts \
        src/components/new-workspace/tracker-section-scope.test.ts \
        src/runtime/github-projects-client.test.ts
      bunx vitest run src/store/slices/worktrees.test.ts \
        -t "persists exact selected tracker coordinates and omits them for an unlinked create"
    )
    npm run build --prefix crates/agentum-desktop/ui
    ;;
  *)
    echo "[verify] unknown feature: $FEAT" >&2
    exit 2
    ;;
esac

git diff --check

echo "[verify] GREEN"
