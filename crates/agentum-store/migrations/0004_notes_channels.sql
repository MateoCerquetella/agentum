-- Phase 8: notes + channels + messages.
--
-- `messages.channel_id` references `channels(id)` (cascading delete).

CREATE TABLE IF NOT EXISTS notes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    tags        TEXT NOT NULL DEFAULT '[]',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS notes_updated_idx ON notes(updated_at DESC);

CREATE TABLE IF NOT EXISTS channels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    a_session   TEXT NOT NULL,
    b_session   TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(a_session, b_session),
    FOREIGN KEY(a_session) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(b_session) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id  INTEGER NOT NULL,
    sender      TEXT NOT NULL,
    body        TEXT NOT NULL,
    ts          TEXT NOT NULL,
    FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS messages_channel_ts_idx ON messages(channel_id, ts);
