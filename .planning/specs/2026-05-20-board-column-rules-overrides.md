---
created: 2026-05-20
title: Per-server custom column rules (override the compile-time matrix)
area: board
stage: spec
depends_on:
  - 2026-05-19-typed-kanban-card-schemas.md
files:
  - crates/agentum-store/migrations/0014_board_column_rules.sql
  - crates/agentum-store/src/lib.rs
  - crates/agentum-core/src/board_schema.rs
  - crates/agentum-server/src/routes/board.rs
  - crates/agentum-server/src/lib.rs
---

# Spec: Board Column Rules (per-server overrides)

## Goal

Let each agentum daemon override the compile-time required-field matrix established in slice 1. A new `board_column_rules` table stores `(column_name, required_fields)` pairs; `enforce_transition` consults the table first and falls back to the slice-1 `required_fields_for` const when no row exists. This unlocks two use cases the const can't serve:

- **Tighten or loosen existing columns** (e.g. drop `tool` from `doing` for a team that doesn't care which executor ran the work).
- **Gate currently-bypass columns** (e.g. require `session_id` + a comment to enter a `review` column). Today every column outside `todo`/`doing`/`done` passthroughs.

Backend-only slice. The dashboard keeps using its hardcoded TS matrix; client-side validation may drift from server-defined rules, and the server stays authoritative via the 400 response. Frontend rule-fetching + edit UI is slice 3.

## Context

- Slice 1 (`./2026-05-19-typed-kanban-card-schemas.md`) shipped the const matrix in `crates/agentum-core/src/board_schema.rs::required_fields_for` and explicitly anticipated this migration: *"When custom rules become a real need (next spec), `const → table` is a refactor, not a re-architecture."*
- Slice 1's architecture (`./2026-05-19-typed-kanban-card-schemas.architecture.md` §1) named this exact next step.
- agentum has no "project" entity — each daemon owns one SQLite DB; `workdir` per ticket is a free-form string that the dashboard groups by basename via `projectOf()`. So "per-project rules" in user-speak == "per-server rules" in code-speak. Per-workdir scoping is **out of scope** (potential slice 4).
- `RequiredField` enum is the canonical name space for field identifiers. Slice 2 uses the same `as_missing_key()` strings as the wire vocabulary: `title`, `lbl`, `workdir`, `tool`, `claimed_by`, `session_id_or_comment`.
- Existing pattern for admin-scope endpoints: see `routes/watchdog.rs` and `routes/doctor.rs` for shape; auth middleware in `crates/agentum-server/src/auth.rs` already gates non-public paths.

## Schema (slice 2)

```sql
-- 0014_board_column_rules.sql
CREATE TABLE board_column_rules (
    column_name      TEXT PRIMARY KEY,
    required_fields  TEXT NOT NULL,    -- JSON array, e.g. ["title","lbl","workdir"]
    updated_at       TEXT NOT NULL
);
```

JSON-blob over a normalized two-column table because rules are looked up as a whole set per column, never queried by individual field. Slice 2 supports <20 columns × <10 fields total — denormalization is the right call.

## API surface

| Method | Path                              | Purpose                                                                                              |
|--------|-----------------------------------|------------------------------------------------------------------------------------------------------|
| `GET`  | `/api/board/rules`                | Returns the **merged** matrix: DB rows override defaults; default columns absent from DB fall back to the slice-1 const; custom columns absent from DB return `[]` (passthrough). |
| `PUT`  | `/api/board/rules/{column}`       | Upsert. Body: `{ "required_fields": ["title", "lbl", "workdir"] }`. Validates every field name against `RequiredField::as_missing_key()` — unknown names → 400. |
| `DELETE` | `/api/board/rules/{column}`     | Remove the override. Column falls back to const default (or passthrough for non-default columns).    |

Auth: standard token gate (non-public). No new permission tier in slice 2 — anyone with a daemon token can edit rules. Per-role auth is out of scope.

## Lookup behavior

`enforce_transition` consults rules in this order:

1. Look up `board_column_rules.required_fields` for the target status. If present, use it (interpreted as the *complete* required-field set for that column — defaults are replaced, not merged).
2. If absent and the status is one of `todo`/`doing`/`done`, use the slice-1 const.
3. Otherwise (custom column with no rule row), passthrough — `Ok(())`.

**Replace, not merge.** A user PUTting `["title", "lbl"]` for `doing` drops the `workdir`/`tool`/`claimed_by` requirements entirely for that daemon. This is the simpler invariant; merging would require a "subtract from defaults" syntax that nothing else in the codebase has.

## Caching

No cache in slice 2. Rules-table read on every gate transition. The lookup is a single indexed `SELECT required_fields FROM board_column_rules WHERE column_name = ?` — cheap. If profiling later shows it as a bottleneck, an `Arc<RwLock<HashMap>>` in `AppState` is the natural fix.

## Acceptance Criteria

- [ ] Migration `0014_board_column_rules.sql` creates the table with the schema above. `cargo test -p agentum-store --lib` passes.
- [ ] `GET /api/board/rules` returns a JSON object keyed by column name with arrays of field names. The default columns (`todo`, `doing`, `done`) appear even when the DB is empty, populated from the const. Custom columns with no rule row do NOT appear (empty maps).
- [ ] `PUT /api/board/rules/doing` with `{ "required_fields": ["title", "lbl"] }` succeeds (200). A subsequent `GET` shows `doing` returning `["title", "lbl"]` (the override, not the const).
- [ ] `PUT` with an unknown field name (e.g. `["title", "wat"]`) returns 400 with body `{ "error": "unknown field: wat" }` (use `ApiError::BadRequest` — no new variant needed; the validation error has a stable shape).
- [ ] `DELETE /api/board/rules/doing` removes the override. Subsequent `GET` shows `doing` back to the const default.
- [ ] After PUTting `["title","lbl"]` for `doing`, creating a board item with `status: "doing"` and only `title` + `lbl` succeeds (200). Without this slice it would 400.
- [ ] After PUTting `["title","lbl","session_id_or_comment"]` for a custom column `review`, transitioning a card to `review` without a session or comment returns 400 `{ missing: ["session_id_or_comment"], status: "review" }`.
- [ ] DELETEing a rule row for `review` restores passthrough — transitions into `review` succeed regardless of fields.
- [ ] `PUT /api/board/rules/doing` with `{ "required_fields": [] }` succeeds; subsequent transitions into `doing` passthrough (no fields required). Idempotent — issuing the same PUT twice produces the same state and the second call still returns 200.
- [ ] `enforce_transition` calls the store at most once per gate evaluation, regardless of which column is targeted.
- [ ] Unit tests cover: GET with empty table (returns const defaults), GET with overrides (returns merged), PUT happy path, PUT unknown field rejected, DELETE happy path, gate with override active (passes when const would fail), gate with custom column rule (fails when passthrough would have allowed).
- [ ] One `agentum-server` integration test exercises the full happy path: `PUT rule → POST board item satisfying override → assert 200; DELETE rule → POST same item shape → assert 400`.

## Out of Scope

- **Frontend rule fetching.** Dashboard keeps using the hardcoded TS matrix. Server 400 stays authoritative; users may see dialog hints that don't match what the server actually requires until slice 3.
- **Dashboard UI for editing rules.** Slice 3.
- **Per-workdir scoping.** Slice 4 (if needed).
- **Per-`tool` / per-`lbl` overrides.** Independent future spec.
- **Audit log of rule changes.** Slice 2's `updated_at` is the only audit affordance.
- **Role-based auth on rule editing.** Anyone with a daemon token can edit. No tiered permissions.
- **Bulk edit.** No `PUT /api/board/rules` (plural) endpoint. One column at a time keeps the contract small.
- **Validation that an existing card violates a new rule.** A user can `PUT` stricter rules; existing cards stay where they are (slice 1's grandfathering invariant covers this).

## Decisions

Resolved 2026-05-20 before architecture handoff.

1. **Lazy migration**. Table starts empty; `enforce_transition` looks up DB first, falls back to the slice-1 const when no row exists. `GET /api/board/rules` synthesizes default rows from the const for the three default columns when the DB is empty. Rationale: no migration ordering concerns, the const stays the single source of truth for defaults, and dropping the const later (if the table becomes authoritative) is a clean delete — not a data-rewrite.
2. **JSON schema versioning**: defer. The JSON array stores `RequiredField::as_missing_key()` strings. New variants in future slices will be additive on the *write* side; old rows pinning to old strings is correct behavior. Versioning the JSON shape is premature.
3. **Empty `required_fields` array**: accept as a synonym for DELETE. `PUT /api/board/rules/doing` with `{ "required_fields": [] }` succeeds and persists an empty array; subsequent gate evaluations for `doing` see an empty required-set and passthrough. Rationale: the user wrote an explicit choice; behavior is idempotent and matches the gate semantics directly (no required fields = no gate).
4. **Per-profile dialog adaptation**: out of scope for slice 2 (no frontend work). Flagged for slice 3 — the dashboard will fetch rules per active profile when it adopts server-driven validation. Until then, the hardcoded TS matrix may drift from the server; the 400 response is authoritative.

## Workflow Position

- **Spec** (sdd-write-spec) — complete (2026-05-20).
- **Next**: `sdd-architect` to validate structural shape — lookup function location (`agentum-core` vs `agentum-server`), `Store` method signature, whether to wrap the rules lookup in an `AppState`-level helper, and the const→table fallback expression. Decisions 1–4 are settled; architect should not reopen them.
- **Then**: `sdd-developer` to implement, `sdd-tester` for the test matrix, `sdd-reviewer` before merge.
