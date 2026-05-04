-- Phase 7: kanban board with atomic claim (PRD §6 + §7).
--
-- `key` is set post-insert from the row id (AG-1, AG-2, …). NOT NULL but
-- not UNIQUE-constrained at the table level — generation is monotonic by
-- construction so collisions can't happen, and dropping the constraint
-- avoids a temporary-key dance during INSERT.

CREATE TABLE IF NOT EXISTS board_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    key         TEXT NOT NULL DEFAULT '',
    title       TEXT NOT NULL,
    body        TEXT,
    status      TEXT NOT NULL DEFAULT 'todo',
    claimed_by  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS board_items_status_idx ON board_items(status);
CREATE UNIQUE INDEX IF NOT EXISTS board_items_key_idx ON board_items(key);
