-- Per-session pin flag for "favorite" sorting.
--
-- Pinned sessions float to the top of every list (sidebar, fleet,
-- sessions index) regardless of created_at order. Cheap quality-of-
-- life affordance for users running 10+ sessions where the one they
-- actually care about right now buries fast.
--
-- Default 0 keeps existing rows behaving the same way as before.

ALTER TABLE sessions ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_sessions_pinned ON sessions(pinned);
