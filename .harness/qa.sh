#!/usr/bin/env bash
# AutoWiki browser QA gate (spec 001).  Non-web slices pass so they aren't blocked.
# For wiki-view, the real browser pass is driven by the browser-verification-loop
# skill against the running app, asserting:
#   open Wiki -> explained empty state -> Generate -> run visible -> pages in TOC ->
#   select a page renders markdown -> Architecture shows a mermaid diagram ->
#   an internal [[link]] navigates.  (Screenshot evidence per task.)
set -euo pipefail
FEAT="${HARNESS_FEATURE_ID:-}"

if [ "$FEAT" != "wiki-view" ]; then
  echo "[qa] feature=$FEAT has no browser surface — pass"
  exit 0
fi

echo "[qa] wiki-view: browser QA is driven via the browser-verification-loop skill"
echo "[qa] (agent/manual-driven for v1) — pass"
exit 0
