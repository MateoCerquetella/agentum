-- Phase 8 (redesign): kanban ticket discriminators.
--
-- `lbl` types tickets so the foot pill colors correctly
-- (bug/feat/chore/spike). `tool` lets the dot color reflect which
-- agent ecosystem owns the work (claude/codex/gemini/hermes).
-- Both nullable; existing rows render with the design's neutral grey.

ALTER TABLE board_items ADD COLUMN lbl   TEXT;
ALTER TABLE board_items ADD COLUMN tool  TEXT;
