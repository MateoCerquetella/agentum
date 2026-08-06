-- Item 4 (#48): fold the GitHub/Linear "Tasks" view into the Board as a sync
-- source. A board card can now carry a link back to the external issue it
-- mirrors, so re-syncing updates the same card instead of duplicating it.
--
-- `external_url` is the stable dedupe key (an issue's web URL is unique across
-- providers); `external_provider` ("github" / "linear" / "gitlab") is for the
-- card's source badge + link icon. Both NULL for native agentum cards.
ALTER TABLE board_items ADD COLUMN external_url      TEXT;
ALTER TABLE board_items ADD COLUMN external_provider TEXT;

-- Enforce one card per external issue. Partial index so the many native cards
-- (external_url IS NULL) are exempt from the uniqueness constraint.
CREATE UNIQUE INDEX IF NOT EXISTS board_items_external_url_idx
    ON board_items(external_url)
    WHERE external_url IS NOT NULL;
