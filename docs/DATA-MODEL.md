# Data model

SQLite, WAL mode, `synchronous=NORMAL`. Weekly `VACUUM` from the
watchdog. Schema lives in `migrations/0001_initial.sql`.

```sql
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,                  -- uuid v4
    name        TEXT NOT NULL UNIQUE,              -- human-friendly slug
    workdir     TEXT NOT NULL,
    tool        TEXT NOT NULL DEFAULT 'claude',    -- claude | codex | opencode | gemini | hermes | custom
    model       TEXT,                              -- e.g. claude-opus-4-8
    flags       TEXT NOT NULL DEFAULT '[]',        -- JSON array of CLI flags
    status      TEXT NOT NULL DEFAULT 'idle',      -- idle | running | stopped | crashed
    tmux_target TEXT,                              -- e.g. "agentum-Bandely"
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    last_activity_at TEXT
);

CREATE INDEX sessions_status_idx ON sessions(status);

CREATE TABLE events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    kind       TEXT NOT NULL,                      -- session.started, watchdog.compact, etc.
    payload    TEXT,                               -- JSON
    ts         TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE SET NULL
);

CREATE INDEX events_session_ts_idx ON events(session_id, ts);

-- Legacy migration compatibility only. Agentum preserves existing rows when
-- opening older databases but does not read or mutate them during normal work.
CREATE TABLE board_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    key         TEXT NOT NULL UNIQUE,              -- AG-1, AG-2…
    title       TEXT NOT NULL,
    body        TEXT,
    status      TEXT NOT NULL DEFAULT 'todo',      -- todo | doing | done | <custom>
    claimed_by  TEXT,                              -- session_id, atomic CAS
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE notes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    tags        TEXT NOT NULL DEFAULT '[]',        -- JSON
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE channels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    a_session   TEXT NOT NULL,
    b_session   TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(a_session, b_session),
    FOREIGN KEY(a_session) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(b_session) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id  INTEGER NOT NULL,
    sender      TEXT NOT NULL,
    body        TEXT NOT NULL,
    ts          TEXT NOT NULL,
    FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE TABLE token_usage (
    session_id   TEXT NOT NULL,
    day          TEXT NOT NULL,                    -- YYYY-MM-DD
    input        INTEGER NOT NULL DEFAULT 0,
    output       INTEGER NOT NULL DEFAULT 0,
    cache_read   INTEGER NOT NULL DEFAULT 0,
    cache_write  INTEGER NOT NULL DEFAULT 0,
    cost_usd     REAL NOT NULL DEFAULT 0,
    PRIMARY KEY(session_id, day),
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE auth_tokens (
    token       TEXT PRIMARY KEY,
    label       TEXT,
    created_at  TEXT NOT NULL,
    last_used_at TEXT
);
```
