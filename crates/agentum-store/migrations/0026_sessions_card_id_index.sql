-- get_session_by_card_id (sessions.rs) filters `WHERE s.card_id = ?` but no
-- index covered card_id — only (status), (pinned), (host_id) existed — so the
-- lookup was a full scan of `sessions`. The goal reconciler runs it on the
-- first `board.created` per goal (reconciler.rs). Index it to make that an
-- O(log n) point lookup. card_id is nullable and mostly NULL, so a partial
-- index over the bound rows keeps it small.
CREATE INDEX IF NOT EXISTS sessions_card_id_idx ON sessions(card_id) WHERE card_id IS NOT NULL;
