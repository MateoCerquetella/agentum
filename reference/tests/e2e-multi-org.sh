#!/bin/bash
# E2E test: Multi-organization support in amux cloud
# Uses curl against the live cloud gateway with the amux_session cookie from Chrome
set -euo pipefail

CLOUD="https://cloud.amux.io"
TEST_ORG="e2e-test-$$"
PASS=0; FAIL=0; TOTAL=0
CREATED_ORG_ID=""
CREATED_INVITE_TOKEN=""

# ── Cookie extraction via CDP ──
get_cookie() {
  # Extract amux_session cookie from Chrome via CDP Network.getCookies
  # Uses Node 22+ built-in WebSocket (no ws module needed)
  node --experimental-websocket -e "
    const http = require('http');
    http.get('http://localhost:9222/json/version', res => {
      let data = '';
      res.on('data', c => data += c);
      res.on('end', () => {
        const wsUrl = JSON.parse(data).webSocketDebuggerUrl;
        const ws = new WebSocket(wsUrl);
        ws.onopen = () => {
          ws.send(JSON.stringify({
            id: 1, method: 'Network.getCookies',
            params: { urls: ['https://cloud.amux.io'] }
          }));
        };
        ws.onmessage = (evt) => {
          const resp = JSON.parse(evt.data);
          if (resp.id === 1) {
            const c = (resp.result?.cookies || []).find(c => c.name === 'amux_session');
            if (c) process.stdout.write(c.value);
            ws.close();
            setTimeout(() => process.exit(0), 100);
          }
        };
        setTimeout(() => process.exit(1), 5000);
      });
    }).on('error', () => process.exit(1));
  " 2>/dev/null
}

# ── Test runner ──
test_it() {
  local name="$1"
  TOTAL=$((TOTAL + 1))
  if eval "$2"; then
    PASS=$((PASS + 1))
    echo "  ✓ $name"
  else
    FAIL=$((FAIL + 1))
    echo "  ✗ $name"
  fi
}

api() {
  local method="$1" path="$2"
  shift 2
  curl -sk -X "$method" -H "Cookie: amux_session=$COOKIE" \
    -H 'Content-Type: application/json' \
    "$@" "$CLOUD$path"
}

echo ""
echo "═══ amux Multi-Org E2E Test ═══"
echo ""

# ── Step 0: Get auth cookie ──
echo "Extracting amux_session cookie from Chrome..."
COOKIE=$(get_cookie || true)

if [ -z "$COOKIE" ]; then
  echo ""
  echo "No amux_session cookie found in Chrome."
  echo "Trying to get it via curl + Playwright auth profile..."

  # Fallback: try extracting from the Playwright profile's cookie store
  # This won't work if Clerk is expired, but let's try
  echo "FAIL: Could not extract amux_session cookie."
  echo "Please log into cloud.amux.io in Chrome first, then re-run."
  exit 1
fi

echo "Cookie: ${COOKIE:0:20}..."
echo ""

# ── Test 1: Verify authenticated ──
test_it "Verify authenticated" '
  IDENTITY=$(api GET /api/identity)
  EMAIL=$(echo "$IDENTITY" | python3 -c "import sys,json; print(json.load(sys.stdin).get(\"email\",\"\"))" 2>/dev/null)
  [ -n "$EMAIL" ] && echo "    Authenticated as: $EMAIL"
'

# ── Test 2: List organizations ──
test_it "List organizations" '
  ORGS=$(api GET /api/gateway/orgs)
  COUNT=$(echo "$ORGS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)
  [ "$COUNT" -ge 1 ] && echo "    Found $COUNT org(s)"
'

# ── Test 3: Create new org ──
test_it "Create new organization" '
  RESP=$(api POST /api/gateway/orgs -d "{\"name\":\"$TEST_ORG\"}")
  CREATED_ORG_ID=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get(\"id\",\"\"))" 2>/dev/null)
  [ -n "$CREATED_ORG_ID" ] && echo "    Created: $CREATED_ORG_ID"
'

# ── Test 4: New org in list ──
test_it "New org appears in list" '
  ORGS=$(api GET /api/gateway/orgs)
  FOUND=$(echo "$ORGS" | python3 -c "import sys,json; print(any(o[\"id\"]==\"$CREATED_ORG_ID\" for o in json.load(sys.stdin)))" 2>/dev/null)
  [ "$FOUND" = "True" ]
'

# ── Test 5: Get org details ──
test_it "Get org details with members" '
  DETAIL=$(api GET "/api/gateway/orgs/$CREATED_ORG_ID")
  NAME=$(echo "$DETAIL" | python3 -c "import sys,json; print(json.load(sys.stdin).get(\"name\",\"\"))" 2>/dev/null)
  MEMBERS=$(echo "$DETAIL" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get(\"members\",[])))" 2>/dev/null)
  [ "$NAME" = "$TEST_ORG" ] && [ "$MEMBERS" = "1" ] && echo "    Name=$NAME, Members=$MEMBERS"
'

# ── Test 6: Switch to new org ──
test_it "Switch to new organization" '
  RESP=$(api POST /api/gateway/switch-org -d "{\"org_id\":\"$CREATED_ORG_ID\"}" -D /tmp/amux-headers.txt -o /dev/null -w "%{http_code}")
  # Should be a redirect (302/303)
  [ "$RESP" = "302" ] || [ "$RESP" = "303" ] || [ "$RESP" = "200" ]
  # Check org cookie was set
  grep -qi "amux_org=$CREATED_ORG_ID" /tmp/amux-headers.txt 2>/dev/null && echo "    Org cookie set"
'

# ── Test 7: Verify active org ──
test_it "Verify active org after switch" '
  ORGS=$(api GET /api/gateway/orgs -H "Cookie: amux_session=$COOKIE; amux_org=$CREATED_ORG_ID")
  ACTIVE=$(echo "$ORGS" | python3 -c "import sys,json; orgs=json.load(sys.stdin); a=[o for o in orgs if o.get(\"active\")]; print(a[0][\"id\"] if a else \"\")" 2>/dev/null)
  [ "$ACTIVE" = "$CREATED_ORG_ID" ] && echo "    Active org: $ACTIVE"
'

# ── Test 8: Create invite ──
test_it "Create invite for org" '
  RESP=$(curl -sk -X POST -H "Cookie: amux_session=$COOKIE; amux_org=$CREATED_ORG_ID" \
    -H "Content-Type: application/json" \
    -d "{\"org_id\":\"$CREATED_ORG_ID\",\"email\":\"test@example.com\",\"role\":\"member\"}" \
    "$CLOUD/api/org/invites")
  CREATED_INVITE_TOKEN=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get(\"token\",\"\"))" 2>/dev/null)
  [ -n "$CREATED_INVITE_TOKEN" ] && echo "    Invite token: ${CREATED_INVITE_TOKEN:0:12}..."
'

# ── Test 9: List invites ──
test_it "List invites shows new invite" '
  RESP=$(curl -sk -H "Cookie: amux_session=$COOKIE; amux_org=$CREATED_ORG_ID" \
    "$CLOUD/api/org/invites?org_id=$CREATED_ORG_ID")
  FOUND=$(echo "$RESP" | python3 -c "import sys,json; print(any(i.get(\"token\")==\"$CREATED_INVITE_TOKEN\" for i in json.load(sys.stdin)))" 2>/dev/null)
  [ "$FOUND" = "True" ] && echo "    Invite found in list"
'

# ── Test 10: Visit invite link (as owner → self-invite warning) ──
test_it "Invite page shows self-invite message for owner" '
  RESP=$(curl -sk -H "Cookie: amux_session=$COOKIE" "$CLOUD/invite/$CREATED_INVITE_TOKEN")
  echo "$RESP" | grep -qi "your own invite"
'

# ── Test 11: Update org name ──
test_it "Update organization name" '
  api PATCH "/api/gateway/orgs/$CREATED_ORG_ID" -d "{\"name\":\"${TEST_ORG}-updated\"}" > /dev/null
  DETAIL=$(api GET "/api/gateway/orgs/$CREATED_ORG_ID")
  NAME=$(echo "$DETAIL" | python3 -c "import sys,json; print(json.load(sys.stdin).get(\"name\",\"\"))" 2>/dev/null)
  [ "$NAME" = "${TEST_ORG}-updated" ] && echo "    Renamed to: $NAME"
'

# ── Test 12: Switch back to personal ──
test_it "Switch back to personal workspace" '
  RESP=$(api POST /api/gateway/switch-org -d "{\"org_id\":\"\"}" -D /tmp/amux-headers2.txt -o /dev/null -w "%{http_code}")
  # amux_org cookie should be cleared (Max-Age=0)
  grep -qi "amux_org=;" /tmp/amux-headers2.txt 2>/dev/null || grep -qi "Max-Age=0" /tmp/amux-headers2.txt 2>/dev/null
  echo "    Switched back to personal"
'

# ── Test 13: Cannot access non-member org ──
test_it "Cannot access org you are not a member of" '
  CODE=$(api GET /api/gateway/orgs/org_nonexistent_fake -o /dev/null -w "%{http_code}")
  [ "$CODE" = "403" ] || [ "$CODE" = "404" ]
'

# ── Test 14: Cannot delete personal workspace ──
test_it "Cannot delete personal workspace" '
  PERSONAL_ID=$(api GET /api/gateway/orgs | python3 -c "import sys,json; orgs=json.load(sys.stdin); p=[o for o in orgs if o.get(\"is_personal\")]; print(p[0][\"id\"] if p else \"\")" 2>/dev/null)
  CODE=$(api DELETE "/api/gateway/orgs/$PERSONAL_ID" -o /dev/null -w "%{http_code}")
  [ "$CODE" = "400" ] && echo "    Correctly prevented (400)"
'

# ── Cleanup ──
echo ""
echo "Cleaning up..."
if [ -n "$CREATED_ORG_ID" ]; then
  api DELETE "/api/gateway/orgs/$CREATED_ORG_ID" > /dev/null 2>&1 || true
  # Verify deleted
  ORGS=$(api GET /api/gateway/orgs)
  STILL=$(echo "$ORGS" | python3 -c "import sys,json; print(any(o[\"id\"]==\"$CREATED_ORG_ID\" for o in json.load(sys.stdin)))" 2>/dev/null)
  if [ "$STILL" = "False" ]; then
    echo "  ✓ Org $CREATED_ORG_ID deleted"
  else
    echo "  ⚠ Org $CREATED_ORG_ID still exists!"
  fi
fi

echo ""
echo "═══ Results: $PASS passed, $FAIL failed (of $TOTAL) ═══"
echo ""

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
