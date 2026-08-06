-- Spec 016a: server-side two-way board ↔ tracker sync (foundation + GitHub pull).
--
-- Builds ON TOP of #58 (migration 0022_board_external_link), which already added
-- `external_url` + `external_provider` and a partial-unique index on `external_url`.
-- Two-way sync needs a STABLE external identity (the issue *number*, not its URL,
-- which is a derived/mutable label) plus a last-synced marker, so the reconcile
-- can match a card by `(external_provider, external_id)` and never ping-pong.
--
-- This migration adds ONLY the two columns #58 omitted, the durable binding
-- table, and the reconcile lookup index. It deliberately does NOT re-add
-- `external_url`/`external_provider` (they exist) and does NOT touch #58's
-- partial-unique index on `external_url`.

-- Stable provider-native id (GitHub issue number as text) — the reconcile match
-- key. NULL for native agentum cards and for #58's client-mirror cards until a
-- server pull stamps it.
ALTER TABLE board_items ADD COLUMN external_id TEXT;

-- RFC3339 timestamp of the last sync that touched this card.
ALTER TABLE board_items ADD COLUMN external_synced_at TEXT;

-- Fast lookup for the sync reconcile (match a card by its stable external ref).
CREATE INDEX IF NOT EXISTS board_items_external_id_idx
    ON board_items(external_provider, external_id);

-- The durable board ↔ tracker binding. `project` is "owner/repo" for GitHub.
-- Unique on (provider, project) so re-binding the same repo updates in place.
CREATE TABLE IF NOT EXISTS board_tracker_bindings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    provider    TEXT NOT NULL,
    project     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS board_tracker_bindings_uniq
    ON board_tracker_bindings(provider, project);
