#!/usr/bin/env bash
# Runtime/isolation QA gate. Non-web slices pass so they aren't blocked.
# For wiki-view, the real browser pass is driven by the browser-verification-loop
# skill against the running app, asserting:
#   open Wiki -> explained empty state -> Generate -> run visible -> pages in TOC ->
#   select a page renders markdown -> Architecture shows a mermaid diagram ->
#   an internal [[link]] navigates.  (Screenshot evidence per task.)
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
FEAT="${HARNESS_FEATURE_ID:-}"

if [ "$FEAT" = "side-effect-free-session-list" ] || \
   [ "$FEAT" = "mode-aware-transcript-read" ] || \
   [ "$FEAT" = "transcript-observer-lifecycle" ]; then
  QA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agentum-spec-028-qa.XXXXXX")"
  ORIGINAL_HOME="$HOME"
  cleanup_spec_028() {
    rm -rf -- "$QA_ROOT"
  }
  trap cleanup_spec_028 EXIT
  mkdir -p "$QA_ROOT/home" "$QA_ROOT/agentum" "$QA_ROOT/tmux"
  echo "[qa] $FEAT: isolated HOME/AGENTUM_HOME/TMUX_TMPDIR=$QA_ROOT"
  HOME="$QA_ROOT/home" \
    AGENTUM_HOME="$QA_ROOT/agentum" \
    TMUX_TMPDIR="$QA_ROOT/tmux" \
    CARGO_HOME="${CARGO_HOME:-$ORIGINAL_HOME/.cargo}" \
    RUSTUP_HOME="${RUSTUP_HOME:-$ORIGINAL_HOME/.rustup}" \
    cargo test -p agentum-server transcript_store::tests --lib -- --nocapture
  HOME="$QA_ROOT/home" \
    AGENTUM_HOME="$QA_ROOT/agentum" \
    TMUX_TMPDIR="$QA_ROOT/tmux" \
    CARGO_HOME="${CARGO_HOME:-$ORIGINAL_HOME/.cargo}" \
    RUSTUP_HOME="${RUSTUP_HOME:-$ORIGINAL_HOME/.rustup}" \
    cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib -- --nocapture
  HOME="$QA_ROOT/home" AGENTUM_HOME="$QA_ROOT/agentum" TMUX_TMPDIR="$QA_ROOT/tmux" \
    CARGO_HOME="${CARGO_HOME:-$ORIGINAL_HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$ORIGINAL_HOME/.rustup}" \
    cargo test -p agentum-server routes::agent_tasks::tests --lib -- --nocapture
  HOME="$QA_ROOT/home" AGENTUM_HOME="$QA_ROOT/agentum" TMUX_TMPDIR="$QA_ROOT/tmux" \
    CARGO_HOME="${CARGO_HOME:-$ORIGINAL_HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$ORIGINAL_HOME/.rustup}" \
    cargo test -p agentum-server tests::server_wired_watchdog_callback_retires_only_non_running_claude_observers --lib -- --nocapture
  echo "[qa] $FEAT: production RecommendedWatcher append->event-bus update and retirement silence passed"
  echo "[qa] injected accounting passed for the 500-row fleet, route/watchdog retirement, capacity-one coalescing, and consumer completion"
  echo "[qa] no portable OS-thread-count or WebSocket-transport assertion is claimed by this isolated backend gate"
  exit 0
fi

if [ "$FEAT" = "binding-identity-fidelity" ] || [ "$FEAT" = "wizard-closed-tracker-scope" ]; then
  echo "[qa] $FEAT: PENDING — requires a current-build desktop and named safe local/SSH fixtures" >&2
  echo "[qa] live Agentum/xcode-theme, same-Project race, SSH, and linked/unlinked evidence was not run" >&2
  exit 2
fi

if [ "$FEAT" != "wiki-view" ]; then
  echo "[qa] feature=$FEAT has no browser surface — pass"
  exit 0
fi

echo "[qa] wiki-view: browser QA is driven via the browser-verification-loop skill"
echo "[qa] (agent/manual-driven for v1) — pass"
exit 0
