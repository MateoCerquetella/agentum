-- Phase 8 (redesign): per-session metrics + lifecycle state.
--
-- These columns power the redesigned dashboard's hero, FleetRow, and
-- session detail rail: ctx %, tokens, USD cost, last log line, uptime
-- override, and the watchdog-aware lifecycle state.
--
-- All nullable / 0-defaulted so existing rows stay valid; the
-- watchdog crate populates them as it observes activity.

ALTER TABLE sessions ADD COLUMN tokens          INTEGER;
ALTER TABLE sessions ADD COLUMN cost_usd        REAL;
ALTER TABLE sessions ADD COLUMN ctx             INTEGER;
ALTER TABLE sessions ADD COLUMN last_log        TEXT;
ALTER TABLE sessions ADD COLUMN uptime_seconds  INTEGER;
-- Lifecycle state with /compact awareness. Values: live | idle | compact | crash.
-- When NULL, the API derives it from `status` for backwards compat.
ALTER TABLE sessions ADD COLUMN state           TEXT;
