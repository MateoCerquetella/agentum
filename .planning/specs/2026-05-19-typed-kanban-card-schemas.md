---
created: 2026-05-19
title: Typed kanban card schemas (per-status required fields)
area: board
stage: spec
files:
  - crates/agentum-server/src/routes/board.rs
  - crates/agentum-core/src/lib.rs
  - crates/agentum-store/src/*.rs
  - dashboard/src/lib/components/BoardItemDialog.svelte
  - dashboard/src/lib/stores/board.ts
---

# Spec: Typed Kanban Card Schemas

## Goal

Enforce per-status required fields on board cards so that moving a ticket between columns is a validated handoff, not a free-form drag. Default columns (`todo` / `doing` / `done`) each demand a defined minimum set of fields before a card can land there; transitions that violate the gate are rejected server-side with a `400` carrying the missing field list, and the dialog/drag UI surfaces the violation before submit. No schema migration — every required field already exists on `board_items`.

## Context

- Current board: `board_items` with free-form `status TEXT` and arbitrary columns; no per-column validation (`crates/agentum-store/migrations/0003_board.sql` + `0010`–`0013`).
- All gating fields already exist on the row: `workdir`, `model`, `tool`, `session_id`, `lbl`, `claimed_by`.
- API surface in scope: `POST /api/board`, `PATCH /api/board/{id}` (`crates/agentum-server/src/routes/board.rs`).
- `POST /api/board/{id}/claim` only stamps `claimed_by` — it does **not** mutate `status` (verified at `routes/board.rs:121`). So gating belongs on create + patch only; claim is unaffected.
- Frontend entry points: `BoardItemDialog.svelte` (create / edit) and drag-drop on the board page (uses `board.ts` store → PATCH).
- Existing planning convention for follow-ups: `.planning/todos/pending/`.

## Required-Field Matrix (slice 1, compile-time const in `agentum-core`)

| Column   | Required on entry                                                                |
|----------|----------------------------------------------------------------------------------|
| `todo`   | `title`, `lbl`                                                                   |
| `doing`  | `title`, `lbl`, `workdir`, `tool`, `claimed_by`                                  |
| `done`   | `title`, `lbl`, and (`session_id IS NOT NULL` **OR** `≥1 row in board_comments`) |

The `done` OR-clause preserves the manual-close path ("won't fix", "dup of AG-12") — a card without a session can still close as long as someone left a comment explaining why.

Custom columns (any status not in the matrix) bypass the gate — backwards-compatible passthrough so user-added columns like `blocked` or `review` keep working without code change.

## Acceptance Criteria

- [ ] `POST /api/board` with `status: "doing"` and any required field empty returns `400` with body `{ "missing": ["workdir", "tool", "claimed_by"], "status": "doing" }`.
- [ ] `PATCH /api/board/{id}` validates **only when the patch body sets a `status` that differs from the row's current `status`**. PATCHes that omit `status`, or that set `status` to the same value the row already has, skip the gate entirely — even if the row's current state would fail validation today. (This is the grandfathering invariant: gates run on transitions, never on existing state.)
- [ ] Transitions into `done` pass when `session_id IS NOT NULL` **OR** at least one row exists in `board_comments` for that `board_id`; otherwise return `400` with `missing: ["session_id_or_comment"]`.
- [ ] Transitions *out of* a gated column (e.g. `doing` → `todo`) never re-validate — moving a card back is always allowed.
- [ ] Existing rows in a gated column without all required fields stay editable: `PATCH /api/board/{id}` with `{ "claimed_by": "..." }` on a `doing` row missing `workdir` succeeds (no banner, no migration warning — silent). Validation only kicks in if that same PATCH also sets `status` to a new value.
- [ ] Custom column names bypass validation entirely (no rule = pass).
- [ ] `BoardItemDialog.svelte`:
  - On status change, recompute required fields and render an inline "Required for *<status>*" hint next to each empty required field.
  - Disable the submit button until all required fields for the selected status are filled.
  - On `400` from the server, map `missing[]` to field highlights (defensive — UI should already block this).
- [ ] Board drag-drop: if a drop into a gated column would fail validation, the card snaps back to its origin column, the edit dialog opens pre-focused on the first missing field, and a toast explains why ("Move to *doing* needs: workdir, tool"). **The wire contract is unchanged** — the server returns the same `400 { missing, status }` shape; snap-back is purely a client-side reaction in `board.ts` (capture origin, optimistic `moveLocal(id, target)`, revert via `moveLocal(id, origin)` on rejection, then open dialog).
- [ ] Server emits `board.transition.rejected` event with `{id, target_status, missing}` on every gate failure (debugging + future audit).
- [ ] Unit tests in `agentum-server` cover: create-into-gated-pass, create-into-gated-fail, patch-into-gated-fail, patch-out-of-gated-pass, custom-column-passthrough.
- [ ] At least one integration test exercises drag-drop snap-back via the dashboard test harness (if one exists; otherwise document this as a manual QA step in the PR description).

## Out of Scope

- Per-project / per-server custom column definitions (next spec — will need DB table).
- Per-`tool` or per-`lbl` field overrides (e.g. "bug tickets also require a repro link in body").
- Comment-template enforcement.
- Auto-deriving `session_id` from an in-flight session when transitioning to `done`.
- Runtime UI for editing the required-field matrix — matrix is a compile-time const for this slice.
- Mobile-specific drag-drop affordances (mobile uses the dialog's status select; covered by the dialog AC).
- Validation on `release`, `comment`, or `reorder` endpoints — none of them mutate `status`.

## Decisions

These resolve the open questions from the initial draft. All resolved 2026-05-19 before architecture handoff.

1. **Matrix location** — compile-time `const` in `agentum-core` (single source of truth). Rationale: simplest correct thing; per-project custom rules are explicitly out of scope. When custom rules become a real need (next spec), `const → table` is a refactor, not a re-architecture.
2. **`done` anchor** — accept *either* `session_id IS NOT NULL` *or* a row in `board_comments` for that `board_id`. Rationale: preserves the manual-close path without smuggling validation through `body`. Cost: one cheap `EXISTS` query per `done` transition.
3. **Grandfathering** — silent. No banner. Rationale: drift is transient; the next transition surfaces the issue organically.
4. **`tool` on `done`** — not required. Rationale: transitively present via either path (`doing` already required it, or manual-close doesn't need it).
5. **`400` vs `422`** — `400`, matching the existing `ApiError::BadRequest` convention in `routes/board.rs`. Rationale: don't add a new error variant just for this gate; clients branch on the payload shape (`{missing, status}`), not the status code.

## Workflow Position

- **Spec** (sdd-write-spec) — complete (2026-05-19).
- **Architecture** (sdd-architect) — complete (2026-05-19). See `./2026-05-19-typed-kanban-card-schemas.architecture.md` for the locked structural shape (matrix module, validator API, store method, validation site, event payload, frontend duplication strategy, and five risks).
- **This stage**: `sdd-developer` — implement against the spec + architecture notes. Follow the implementation order in the architecture file (core → store → server → dashboard → follow-up todo).
- **Then**: `sdd-tester` for the test matrix, `sdd-reviewer` before merge.
