-- Persist what each session was provisioned with, so a boot-time scan can
-- detect when the live embedded endpoint has drifted from the config/env the
-- session's agent was launched against (R1+R2 keep it stable across the common
-- restart, but a forced ephemeral rebind still moves it).
--
-- All columns are additive / backward-compatible: existing rows and any session
-- whose host isn't Local keep NULL/0, which the scan reads as "nothing recorded
-- → don't touch".
--
--  provisioned_api_base       — the embedded server's base URL the session's
--                               MCP config + AGENTUM_* env were written against
--                               (e.g. `http://127.0.0.1:8822`). NULL until first
--                               recorded at spawn (Local sessions only).
--  provisioned_token_hash     — hex SHA-256 of the `/mcp` bearer token in effect
--                               at provision time. We store the HASH, never the
--                               token, so a rotated token is detectable without
--                               widening the secret-at-rest surface.
--  provisioned_needs_reconnect — set to 1 by the boot drift scan when this
--                               session's recorded endpoint no longer matches the
--                               live one and its config/env were rewritten. The
--                               live agent must reconnect to pick up the change;
--                               this flag surfaces that to the UI. Cleared
--                               whenever the session is (re)provisioned current.
ALTER TABLE sessions ADD COLUMN provisioned_api_base        TEXT    NULL;
ALTER TABLE sessions ADD COLUMN provisioned_token_hash      TEXT    NULL;
ALTER TABLE sessions ADD COLUMN provisioned_needs_reconnect INTEGER NOT NULL DEFAULT 0;
