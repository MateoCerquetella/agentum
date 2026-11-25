#!/bin/bash
# Run a hub on made-up demo data (scripts/demo.sql: three mail accounts, a week of calendar,
# notes, two coding threads) in a fresh temp directory. Nothing in it is real and it reaches
# no mail server. For trying Agentum out and for README screenshots.
#   scripts/demo.sh 7141                 # uses the hub at $AGENTUM_BIN or the last debug build
#   kill "$(cat /tmp/agentum-demo-7141/pid)"   # stop it
# Build first if needed: scripts/build-web.sh && cargo build -p agentum
set -euo pipefail
cd "$(dirname "$0")/.."
PORT=${1:?usage: scripts/demo.sh PORT}
BIN=${AGENTUM_BIN:-${CARGO_TARGET_DIR:-$HOME/.cache/agentum-target}/debug/agentum}
DIR=/tmp/agentum-demo-$PORT
[ -x "$BIN" ] || { echo "no hub binary at $BIN (set AGENTUM_BIN)"; exit 1; }
command -v sqlite3 > /dev/null || { echo "needs sqlite3"; exit 1; }
[ -f "$DIR/pid" ] && kill "$(cat "$DIR/pid")" 2>/dev/null && sleep 0.5 || true
rm -rf "$DIR" && mkdir -p "$DIR/data"

# The demo's times are local: the hub uses the same zone as this machine.
export CERQ_TZ=${CERQ_TZ:-$(timedatectl show -p Timezone --value 2>/dev/null || readlink /etc/localtime | sed 's|.*zoneinfo/||')}
# No pushes, no calendar writes, no model or speech server: the demo stays on this machine.
export AGENTUM_DATA=$DIR/data AGENTUM_BIND=127.0.0.1:$PORT CERQ_NO_PUSH=1 CERQ_CALENDAR_READONLY=1 CERQ_SKIP_OUTLOOK=1 \
  CERQ_MAIL_LLM_URL=http://127.0.0.1:9/v1 CERQ_VOICE_URL=http://127.0.0.1:9/v1 CERQ_MAIL_INTERVAL=31536000
DB=$DIR/data/agentum.db
start() {
  ( cd "$DIR" && exec setsid "$BIN" >> hub.log 2>&1 ) < /dev/null > /dev/null 2>&1 &
  echo $! > "$DIR/pid"
  for _ in $(seq 50); do curl -sf "http://127.0.0.1:$PORT/api/health" > /dev/null && return; sleep 0.2; done
  tail -20 "$DIR/hub.log"; exit 1
}

# The hub creates its tables on the first start; the demo rows go in while it's stopped.
start && kill "$(cat "$DIR/pid")" && sleep 0.5
TOKEN=$(openssl rand -hex 32)
{ echo "DELETE FROM kv WHERE k='mail.state';"; cat scripts/demo.sql; echo "INSERT INTO auth_sessions VALUES ('$TOKEN', 'demo', unixepoch() + 30 * 86400);"; } | sqlite3 -bail "$DB"
start
# The mail sync first runs 15 s after the start (then yearly, see CERQ_MAIL_INTERVAL): let it find
# no usable account, then give the accounts their (fake) password so Mail shows them.
echo "waiting for the hub's first mail sync…"
for _ in $(seq 150); do [ "$(sqlite3 "$DB" "SELECT v LIKE '%\"runs\":[{%' FROM kv WHERE k='mail.state'")" = 1 ] && break; sleep 0.2; done
sqlite3 "$DB" "UPDATE email_accounts SET imap_password='demo', smtp_password='demo'"
echo "Agentum demo on http://127.0.0.1:$PORT (user demo, password demo; log $DIR/hub.log)"
echo "TOKEN=$TOKEN   # cookie odysseus_session"
