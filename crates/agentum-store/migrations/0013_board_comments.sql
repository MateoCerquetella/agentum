-- Threaded comments on board items. The Edit-Ticket dialog renders
-- these as a scrolling thread; the watchdog and external integrations
-- can also append (e.g. "agent finished" auto-comments) to give the
-- board an audit trail that survives status churn.
--
-- `author` is free-form so both human actors (web-xxxxxx) and agent
-- ids can post without a separate join. ON DELETE CASCADE keeps the
-- table tidy when a ticket is removed.

CREATE TABLE board_comments (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    board_id   INTEGER NOT NULL REFERENCES board_items(id) ON DELETE CASCADE,
    author     TEXT NOT NULL,
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_board_comments_board ON board_comments(board_id, created_at);
