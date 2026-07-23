# Handoff — PM to Architect

- **Spec:** 027-remove-internal-workspace-board
- **From:** PM
- **To:** Architect
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- A single-slice product contract for making configured external trackers the only workspace
  work-item authority while leaving historical internal-board data inert.
- A named workspace-operator persona, five observable acceptance criteria, explicit non-goals,
  grounded reuse boundaries, protected architecture invariants, and harness verification wiring.

## Acceptance-criteria evidence

- **AC 1:** Requires configured GitHub/Linear Tasks rendering or an explicit no-tracker state and
  forbids internal-board cards and sync actions.
- **AC 2:** Names every retired `/api/board*` family and requires observable 404 responses.
- **AC 3:** Requires external creation/selection/transition behavior without `board_items` writes.
- **AC 4:** Requires legacy-database startup while normal flows neither return nor mutate old rows.
- **AC 5:** Names the workspace Rust gate, desktop build, and focused route/tracker/UI checks.

## Verification

- `ai/skills/validate_handoff.md` checklist — PASS (all nine PM-gate conditions satisfied)

## Decisions and invariants

- Historical board migrations and rows remain untouched; deletion of durable user data is outside
  scope and remains human-gated.
- GitHub Projects and Linear kanban views are external tracker surfaces, not the retired internal
  Workspace board.
- One launch path, push streaming, tracker best-effort behavior, and both harness gates remain
  unchanged.

## Remaining risks / next action

- Architect must identify every board-only route, persistence, background-worker, and fallback
  seam while protecting unrelated external tracker and session behavior.
