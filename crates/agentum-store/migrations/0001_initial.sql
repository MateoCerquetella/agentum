-- Phase 1: sessions table only. Other tables land in their respective phases.

CREATE TABLE IF NOT EXISTS sessions (
    id               TEXT PRIMARY KEY,
    name             TEXT NOT NULL UNIQUE,
    workdir          TEXT NOT NULL,
    tool             TEXT NOT NULL,
    model            TEXT,
    flags            TEXT NOT NULL DEFAULT '[]',
    status           TEXT NOT NULL DEFAULT 'idle',
    tmux_target      TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    last_activity_at TEXT
);

CREATE INDEX IF NOT EXISTS sessions_status_idx ON sessions(status);
