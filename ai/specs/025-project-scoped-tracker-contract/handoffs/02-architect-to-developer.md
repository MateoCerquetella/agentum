# Handoff — Architect to Developer

- **Spec:** 025-project-scoped-tracker-contract
- **From:** Architect
- **To:** Developer
- **Date:** 2026-07-21
- **Gate:** PASS

## Delivered

- `architecture.md` with seven locked decisions, persistence/wire contracts,
  migration precedence, control flow, race/error behavior, exact edit map, build
  order, and AC-to-test traceability.
- `tasks.md` with four ordered, independently gated implementation slices.

## Acceptance-criteria evidence

- **AC 1–2:** D1–D3 define typed revisioned SQLite ownership and one API/action
  seam.
- **AC 3–4:** D5 defines repo/host/generation-keyed UI resolution and consumers.
- **AC 5–6:** D6 separates immutable ticket coordinates from project mapping and
  supplies project-aware fail-closed transitions.
- **AC 7–10:** D4/D7 define migration, provenance, host/deletion isolation, and
  explicit creation-time defaults.

## Verification

- Architecture traceability table maps every AC to an edit seam and test — PASS.
- Architecture-principle review (launch, YOLO, streaming, MCP/API reuse) — PASS.

## Decisions and invariants

- SQLite row keyed by `Repo.id` is the only canonical durable owner.
- `auto` is not runtime configuration; unconfigured is explicit and cannot read
  global last-used state.
- Existing exact worktree/feature URLs are never retargeted by config edits.
- Legacy GitHub/desktop state is migration input only; canonical presence wins.

## Remaining risks / next action

- Implement F1 first and keep old GitHub binding callers alive through adapters;
  do not begin broad TaskPage rewiring until store/API migration tests are green.
