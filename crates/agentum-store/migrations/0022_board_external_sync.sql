-- Spec 014a: two-way board ↔ external tracker sync (foundation + GitHub pull).
--
-- A board card can mirror an external issue (GitHub now; GitLab/Linear later).
-- These columns are the durable link the sync engine reconciles against so a
-- re-sync updates the existing card in place instead of duplicating it:
--   external_provider  "github" | "gitlab" | "linear"
--   external_id        provider-native id (GitHub issue number, as text)
--   external_url       web URL for the deep-link out
--   external_synced_at RFC3339 of the last sync that touched this card
-- All nullable; native cards leave them NULL and render unchanged.

ALTER TABLE board_items ADD COLUMN external_provider   TEXT;
ALTER TABLE board_items ADD COLUMN external_id         TEXT;
ALTER TABLE board_items ADD COLUMN external_url        TEXT;
ALTER TABLE board_items ADD COLUMN external_synced_at  TEXT;

-- Fast lookup for the sync reconcile (match a card by its external ref).
CREATE INDEX IF NOT EXISTS board_items_external_idx
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
