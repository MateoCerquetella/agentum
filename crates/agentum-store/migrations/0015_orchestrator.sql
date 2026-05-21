-- Phase 1 orchestrator schema: goal cards + planner session binding.
--
-- `parent_goal_id` on board_items is nullable + additive for backwards
-- compatibility. Per D-01 of 01-CONTEXT.md, a "goal" IS a BoardItem
-- with `lbl = "goal"` — no parallel table. Existing rows without a
-- parent relationship read as NULL; callers guard on IS NOT NULL.
--
-- `board_links` is its own table rather than an inline JSON column on
-- board_items for two reasons: (a) PROJECT.md requires the dependency-
-- aware column gate (Phase 3) to query edges directly, sub-10ms even
-- with hundreds of cards — that's only possible with an indexed JOIN,
-- not a JSON array scan; (b) the link set is mutable independently of
-- either endpoint's lifecycle, and ON DELETE CASCADE makes the FK the
-- only place that logic needs to live.
--
-- `card_id` on sessions is nullable because existing sessions predate
-- card binding. The planner session spawned by POST /api/board/goals
-- gets `card_id = goal.id`; all other sessions remain NULL.

ALTER TABLE board_items ADD COLUMN parent_goal_id INTEGER;

ALTER TABLE sessions ADD COLUMN card_id INTEGER;

CREATE TABLE board_links (
    from_card_id  INTEGER NOT NULL,
    to_card_id    INTEGER NOT NULL,
    kind          TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (from_card_id, to_card_id, kind),
    FOREIGN KEY (from_card_id) REFERENCES board_items(id) ON DELETE CASCADE,
    FOREIGN KEY (to_card_id)   REFERENCES board_items(id) ON DELETE CASCADE
);

-- Queried on every child status change by the watchdog goal-status
-- recomputer (Phase 1 plan 04): SELECT … WHERE parent_goal_id = ?
CREATE INDEX idx_board_items_parent_goal_id ON board_items(parent_goal_id)
    WHERE parent_goal_id IS NOT NULL;

-- Phase 3 dependency gate reads edges into a card on every PATCH:
-- SELECT … WHERE to_card_id = ? to find what blocks this card.
CREATE INDEX idx_board_links_to ON board_links(to_card_id);
