-- Bearer-token sessions get an explicit expiry. Tokens minted before this
-- migration ran are grandfathered with a 30-day window from now so existing
-- logins survive the upgrade; new tokens use the same default but the server
-- can override per-call.

ALTER TABLE auth_sessions ADD COLUMN expires_at TEXT;

-- Backfill existing rows: 30 days from "now" expressed in RFC3339 UTC.
UPDATE auth_sessions
SET expires_at = strftime('%Y-%m-%dT%H:%M:%SZ', datetime('now', '+30 days'))
WHERE expires_at IS NULL;

CREATE INDEX IF NOT EXISTS auth_sessions_expires_idx ON auth_sessions(expires_at);
