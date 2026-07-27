-- Agent session hooks over the authenticated loopback boundary.
--
-- hook_token            — ephemeral per-launch secret written by the server
--                         at `POST /api/sessions/{id}/start` time. NULL when
--                         the session has never been started or the daemon
--                         was restarted (tokens are authoritative in memory;
--                         this column is a no-op placeholder kept for schema
--                         completeness and future persistence if needed).
-- hook_events_enabled   — user-facing opt-in for Claude's --hook-* flags.
--                         When 1, the start handler injects
--                         AGENTUM_HOOK_URL / AGENTUM_HOOK_TOKEN env vars and
--                         appends --hook-post-tool-use to Claude's argv.
ALTER TABLE sessions ADD COLUMN hook_token          TEXT    NULL;
ALTER TABLE sessions ADD COLUMN hook_events_enabled BOOLEAN NOT NULL DEFAULT 0;
