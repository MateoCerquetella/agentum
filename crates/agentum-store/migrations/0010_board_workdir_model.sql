-- Per-ticket execution context. The board models work-to-do, so each
-- ticket should know *where* the agent runs (workdir) and optionally
-- *which model* should pick it up. The `tool` column added in 0008
-- already names the ecosystem; pairing it with workdir+model closes
-- the loop so a ticket can be turned into an agentum session without
-- re-asking the user the same questions.
--
-- Both nullable. Existing rows backfill as NULL.

ALTER TABLE board_items ADD COLUMN workdir TEXT;
ALTER TABLE board_items ADD COLUMN model   TEXT;
