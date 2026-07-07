#!/usr/bin/env bash
set -euo pipefail
# Browser QA gate — runs after verify.sh is green. $HARNESS_FEATURE_ID names the feature.
# For a web app, verify the feature in a real browser, e.g. (requires AGENTUM_BROWSER_VERIFY + Playwright MCP):
#   claude -p "Use the browser-verification-loop skill to QA feature $HARNESS_FEATURE_ID against the running app. Exit non-zero if any check fails."
# Default: no browser surface to check — pass.
echo "qa: no browser checks configured — passing"
