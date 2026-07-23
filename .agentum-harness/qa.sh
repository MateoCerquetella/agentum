#!/usr/bin/env bash
set -euo pipefail

# Browser QA gate for spec 027. AGENTUM_BROWSER_VERIFY_CMD must name an
# executable wrapper that accepts the feature id and verification brief below.
# The wrapper owns browser setup and must exit zero only after real browser QA.
feature_id="${HARNESS_FEATURE_ID:-F1}"
verify_cmd="${AGENTUM_BROWSER_VERIFY_CMD:-}"
verification_brief="Verify the Agentum project Tasks surface in a real browser for: (1) a GitHub-bound project showing configured GitHub tasks, (2) a Linear-bound project showing configured Linear tasks, (3) an unbound project showing the explicit no-tracker state, and (4) a project whose configured tracker is unavailable showing the explicit unavailable state. In every scenario, confirm there are no internal-board cards and no Sync to Board action. Exit nonzero if any scenario is unverified or fails."

printf '%s\n' \
  "[qa] required: GitHub-bound Tasks scenario" \
  "[qa] required: Linear-bound Tasks scenario" \
  "[qa] required: unbound Tasks scenario" \
  "[qa] required: unavailable-tracker Tasks scenario" \
  "[qa] required: no internal-board cards or Sync to Board action"

if [[ -z "$verify_cmd" ]]; then
  printf '%s\n' \
    "[qa] PENDING/UNAVAILABLE: AGENTUM_BROWSER_VERIFY_CMD is not configured." \
    "[qa] No browser scenarios were verified; refusing to pass." >&2
  exit 2
fi

if ! command -v -- "$verify_cmd" >/dev/null 2>&1; then
  printf '[qa] PENDING/UNAVAILABLE: browser verifier is not available: %s\n' \
    "$verify_cmd" >&2
  exit 2
fi

printf '[qa] running configured browser verifier: %s\n' "$verify_cmd"
exec "$verify_cmd" "$feature_id" "$verification_brief"
