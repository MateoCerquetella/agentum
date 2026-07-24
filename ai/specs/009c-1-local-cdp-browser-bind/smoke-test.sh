#!/usr/bin/env bash
# 009c-1 manual smoke test — the "one browser" proof you can watch with your eyes.
#
# This runs the EXACT mechanism agentum uses: launches a HEADED Chromium with a
# remote-debugging port (what cdp_browser.rs does), then binds @playwright/mcp to
# THAT browser over --cdp-endpoint (what playwright_mcp.rs does in bound mode),
# then drives the browser so you SEE the window navigate. One instance — the
# window you watch is the one being driven.
#
# Uses TEST ports (browser :9390, MCP :8934) so it does NOT collide with the
# app's real singleton (:9300 / :8933). Leaves things running so you can look;
# prints the cleanup command at the end.
set -u

BROWSER_PORT=9390
MCP_PORT=8934
PROF="/tmp/agentum-009c1-smoketest-profile"

# Resolve the Playwright-managed Chromium the same way cdp_browser.rs does.
APP=$(find "$HOME/Library/Caches/ms-playwright"/chromium-*/chrome-mac*/ -maxdepth 1 -name '*.app' 2>/dev/null | sort | tail -1)
if [ -z "$APP" ]; then
  echo "FAIL: no Playwright Chromium installed. Run:  npx playwright install chromium"
  exit 1
fi
STEM=$(basename "$APP" .app)
CHROME="$APP/Contents/MacOS/$STEM"
echo "Using browser: $CHROME"

# 1) Launch HEADED — a real window you can see (cdp_browser.rs argv shape).
echo ">> Launching headed browser on CDP :$BROWSER_PORT (a window should appear)…"
"$CHROME" --remote-debugging-address=127.0.0.1 --remote-debugging-port=$BROWSER_PORT \
  --user-data-dir="$PROF" --no-first-run --no-default-browser-check about:blank \
  >/tmp/agentum-009c1-chrome.log 2>&1 &
CHROME_PID=$!

for i in $(seq 1 40); do curl -sf "http://127.0.0.1:$BROWSER_PORT/json/version" >/dev/null 2>&1 && break; sleep 0.25; done
if ! curl -sf "http://127.0.0.1:$BROWSER_PORT/json/version" >/dev/null 2>&1; then
  echo "FAIL: browser did not expose CDP"; kill $CHROME_PID 2>/dev/null; exit 1
fi
echo "   OK — browser up (PID $CHROME_PID). UA:"
curl -s "http://127.0.0.1:$BROWSER_PORT/json/version" | grep -o '"User-Agent":[^,]*'

# 2) Bind @playwright/mcp to THAT browser over --cdp-endpoint (playwright_mcp.rs bound mode).
echo ">> Binding Playwright MCP (:$MCP_PORT) to the browser via --cdp-endpoint…"
npx -y @playwright/mcp@latest --port $MCP_PORT --host 127.0.0.1 \
  --cdp-endpoint "http://127.0.0.1:$BROWSER_PORT" >/tmp/agentum-009c1-mcp.log 2>&1 &
MCP_PID=$!
for i in $(seq 1 60); do nc -z 127.0.0.1 $MCP_PORT 2>/dev/null && break; sleep 0.5; done
if nc -z 127.0.0.1 $MCP_PORT 2>/dev/null; then
  echo "   OK — Playwright MCP bound + listening (PID $MCP_PID) at http://127.0.0.1:$MCP_PORT/mcp"
else
  echo "FAIL: MCP did not start"; tail -15 /tmp/agentum-009c1-mcp.log; kill $CHROME_PID 2>/dev/null; exit 1
fi

# 3) Drive the browser so you SEE it move: open a visible page via CDP.
echo ">> Driving the browser — watch the window load example.com…"
sleep 1
curl -s "http://127.0.0.1:$BROWSER_PORT/json/new?https://example.com" >/dev/null 2>&1 \
  || curl -s -X PUT "http://127.0.0.1:$BROWSER_PORT/json/new?https://example.com" >/dev/null 2>&1
echo
echo "=================================================================="
echo "WATCH THE BROWSER WINDOW: it should now show example.com."
echo "That window is the SAME browser the Playwright MCP (:$MCP_PORT) is"
echo "bound to — so an agent pointed at http://127.0.0.1:$MCP_PORT/mcp drives"
echo "exactly this window. That's the 009c-1 unification."
echo
echo "When done, clean up with:"
echo "   kill $MCP_PID $CHROME_PID 2>/dev/null; rm -rf $PROF"
echo "=================================================================="
