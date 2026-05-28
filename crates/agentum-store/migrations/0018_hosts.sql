-- SSH-agentless hosts.
--
-- Profiles are remote Agentum daemons. Hosts are machines controlled by
-- this daemon directly: either the local machine or an SSH target where
-- Agentum drives tmux without installing an Agentum binary there.

CREATE TABLE IF NOT EXISTS hosts (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    kind         TEXT NOT NULL CHECK (kind IN ('local', 'ssh')),
    user         TEXT,
    hostname     TEXT,
    port         INTEGER,
    auth_kind    TEXT,
    key_path     TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    last_seen_at TEXT
);

INSERT OR IGNORE INTO hosts
    (id, name, kind, created_at, updated_at)
VALUES
    ('00000000-0000-0000-0000-000000000000', 'local', 'local', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z');

ALTER TABLE sessions ADD COLUMN host_id TEXT NULL;

UPDATE sessions
SET host_id = '00000000-0000-0000-0000-000000000000'
WHERE host_id IS NULL;

CREATE INDEX IF NOT EXISTS sessions_host_id_idx ON sessions(host_id);
