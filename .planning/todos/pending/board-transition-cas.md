---
created: 2026-05-19
title: Serialize concurrent board status PATCHes (compare-and-swap)
area: board
---

# Follow-up: CAS on board_items.status

The typed-kanban gate (`.planning/specs/2026-05-19-typed-kanban-card-schemas.md`)
runs validation against a row snapshot, then UPDATEs. Two concurrent
PATCHes against the same `board_items.id` into different gated columns
both read the same pre-state, both pass validation, and both UPDATE —
the second write wins and the event log records two transitions, neither
of which was rejected. This was the case before the gate too, but the
gate makes the inconsistency more visible because the rejected-event
stream now implies "everything that landed was validated".

Architecture risk #2 in
`.planning/specs/2026-05-19-typed-kanban-card-schemas.architecture.md`.

Suggested fix: add a `WHERE status = ?` clause in `patch_board_item`
keyed on the pre-image status the handler observed, and treat a
zero-rows-affected result as a 409 (`conflict — re-fetch`). The handler
already fetches `get_board_item` immediately before validation, so the
pre-image is already in hand. Cost: one column in the WHERE clause +
one new error mapping. Not in slice 1's scope.
