-- Phase 6: events table for the broadcast bus / persisted history.

CREATE TABLE IF NOT EXISTS events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    kind       TEXT NOT NULL,
    payload    TEXT,
    ts         TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS events_session_ts_idx ON events(session_id, ts);
