---
created: 2026-05-20
title: Add end-to-end test proving a `doing` row can be created via the API
area: board
---

# Follow-up: missing create-into-`doing` test coverage

The handler-level test `create_into_gated_passes` in
`crates/agentum-server/src/routes/board.rs` had to be rerouted to
target `todo` instead of `doing` because `NewBoardItem` lacks a
`claimed_by` field — the dedicated `/claim` endpoint owns that field,
so a single `POST /api/board` creating a `doing` row directly can't
satisfy the gate's `claimed_by` requirement. The consequence is that
there is currently no end-to-end test proving a `doing` row can be
*created* via the API. We have plenty of coverage for *transitioning
into* `doing` from `todo` via PATCH, but the "create directly" path is
untested at the handler level.

Two ways to close the gap:

(a) **Extend `NewBoardItem`** with an optional `claimed_by` field so a
    single POST can create a `doing` row in one shot. Cheapest in test
    code, but introduces a second way for `claimed_by` to land on a
    row (POST or `/claim`), which the spec's Decision 5 trade-off
    section deliberately avoided.

(b) **Accept the lifecycle approach** and add a multi-step handler
    test that mirrors the real dashboard flow: POST `todo` → POST
    `/{id}/claim` with `claimed_by` → PATCH `status: doing` with
    `workdir`/`tool`. This is faithful to the actual UX (the dashboard
    "new ticket" button only ever creates `todo` rows; the user drags
    or clicks "Start" to enter `doing`) and avoids touching the wire
    contract.

Origin spec: `.planning/specs/2026-05-19-typed-kanban-card-schemas.md`
(slice 1 — typed kanban card schemas, Pass A landed in commit
`184b4c7`). The reviewer's nice-to-have list during Pass B included
this as a deferred test gap; the Pass A merge-gating work was the
priority and this didn't make the cut.
