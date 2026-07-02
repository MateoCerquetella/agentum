-- Both hot event queries filter on kind: the /api/events connect snapshot
-- (kind LIKE 'agent.%' grouped by session) and the watchdog feed (kind LIKE
-- 'watchdog.%' OR 'session.%'). The only index was (session_id, ts), so each
-- ran a full table scan over a log that grows for the life of the install.
-- (kind, session_id, id) serves the LIKE-prefix range scans and carries the
-- per-session MAX(id) columns for the snapshot's GROUP BY.
CREATE INDEX IF NOT EXISTS events_kind_session_id_idx ON events(kind, session_id, id);
