#!/bin/sh
# agentum managed agent-status hook.
#
# Installed per-agent by the server (its absolute path is written into each
# agent's hook config). It normalizes whatever lifecycle payload the agent CLI
# hands it into {kind: working|done|permission} and POSTs that to the
# server's $AGENTUM_HOOK_URL so the sidebar spinner / agent-row dot can follow
# real activity for agents that emit no status in their terminal title.
#
# Payload delivery differs per agent (modeled on the proven multi-agent
# contract in the wild): Codex passes the event JSON as argv[1]; Claude-family
# CLIs (Droid, OpenClaude, …) pipe it on stdin. The event field is "type" for
# Codex and "hook_event_name" for the Claude family. We read both.
#
# Never default to "done" on a parse miss — a false "done" would clear a live
# spinner while the agent is still working, which is worse than no signal.

if [ -n "$1" ]; then
  INPUT="$1"
else
  INPUT="$(cat 2>/dev/null)"
fi

# Preferred path: the registration encodes the kind in $AGENTUM_HOOK_KIND
# (e.g. the launcher registers `AGENTUM_HOOK_KIND=working <script>` on the
# turn-start event). This is reliable regardless of the agent's payload shape
# — and matches how real-world multi-agent hooks pass per-event context.
KIND=""
case "$AGENTUM_HOOK_KIND" in
  working|done|permission) KIND="$AGENTUM_HOOK_KIND" ;;
esac

# Fallback: a single script registered for all events parses the payload's
# event field. Claude family uses "hook_event_name"; Codex uses "type".
if [ -z "$KIND" ]; then
  EV="$(printf '%s' "$INPUT" | grep -oE '"hook_event_name"[[:space:]]*:[[:space:]]*"[^"]*"' | grep -oE '"[^"]*"$' | tr -d '"')"
  if [ -z "$EV" ]; then
    EV="$(printf '%s' "$INPUT" | grep -oE '"type"[[:space:]]*:[[:space:]]*"[^"]*"' | grep -oE '"[^"]*"$' | tr -d '"')"
  fi
  case "$EV" in
    UserPromptSubmit|task_started|turn_started|turn.started)
      KIND="working" ;;
    Stop|SubagentStop|agent-turn-complete|task_complete|turn_complete|turn.completed)
      KIND="done" ;;
    Notification|exec_approval_request|apply_patch_approval_request|request_user_input|elicitation)
      KIND="permission" ;;
  esac
fi

# Parse miss: stay silent rather than risk a false transition.
[ -z "$KIND" ] && exit 0
[ -z "$AGENTUM_HOOK_URL" ] && exit 0

curl -s -X POST "$AGENTUM_HOOK_URL" \
  -H "X-Agentum-Hook-Token: $AGENTUM_HOOK_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"kind\":\"$KIND\",\"payload\":{\"event\":\"$EV\"}}" \
  --connect-timeout 2 --max-time 5 >/dev/null 2>&1 || true

exit 0
