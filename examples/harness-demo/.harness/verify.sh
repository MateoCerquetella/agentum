#!/usr/bin/env bash
# The VERIFICATION GATE. Run by the harness after each feature, from the project
# root, with the current feature id in $HARNESS_FEATURE_ID.
#
#   exit 0  -> green: feature passes, harness advances + writes handoff.md
#   exit !0 -> red:   advancement BLOCKED; the agent gets this output and retries.
set -uo pipefail

feature="${HARNESS_FEATURE_ID:-unknown}"
echo "[verify] gate running for feature: ${feature}"

fail() {
  echo "[verify] ✗ FAILED: $1"
  exit 1
}

case "${feature}" in
  hello-file)
    [ -f GREETING.md ] || fail "GREETING.md does not exist at the project root"
    grep -qx "Hello from the Agentum Harness Engine" GREETING.md \
      || fail "GREETING.md must contain the exact line: Hello from the Agentum Harness Engine"
    echo "[verify] ✓ GREETING.md present with the required greeting line"
    ;;

  add-build-stamp)
    [ -f GREETING.md ] || fail "GREETING.md does not exist"
    grep -qx "Hello from the Agentum Harness Engine" GREETING.md \
      || fail "the original greeting line must remain intact"
    grep -q "^Verified" GREETING.md \
      || fail "expected a second line starting with 'Verified'"
    echo "[verify] ✓ build-stamp line present and greeting intact"
    ;;

  *)
    # Unknown feature id: nothing to check, treat as a pass so the demo can grow.
    echo "[verify] no checks defined for '${feature}' — passing by default"
    ;;
esac

exit 0
