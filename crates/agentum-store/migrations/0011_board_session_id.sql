-- Per-ticket session linkage. When a user spawns an agentum session
-- from a board item ("Start session"), the resulting session id gets
-- stamped here so subsequent UI surfaces (Ticket card jump-arrow,
-- dialog Open-session button) can deep-link back into the pane that
-- owns the work.
--
-- Nullable: tickets without an active session render the Start affordance
-- instead. The link is advisory — deleting the session does NOT
-- automatically clear `session_id` because the user often still wants
-- the historic association visible on the card.

ALTER TABLE board_items ADD COLUMN session_id TEXT;
