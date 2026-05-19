-- Per-ticket manual ordering. Cards inside a column sort by
-- `priority ASC, created_at ASC` so a fresh row lands at the bottom
-- of its column (priority 0, newer created_at) and drag-to-reorder
-- works by rewriting priorities on the affected column.
--
-- Indexed on `(status, priority)` so listing-per-column stays cheap
-- even with thousands of rows. NOT NULL with a default keeps existing
-- rows on the same effective sort order they had before.

ALTER TABLE board_items ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_board_items_status_priority ON board_items(status, priority);
